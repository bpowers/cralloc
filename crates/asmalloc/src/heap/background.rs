//! The background thread (feature `background`, on by default): an ordinary thread of the
//! process, created with the C library's `pthread_create`, that takes rounds of
//! [`Asmalloc::maintain`]'s steps, sleeps an interval between them, and parks while the
//! process is quiet (`depot::thread`; design: "Heartbeat").
//!
//! **When it starts.** By default, when a heartbeat finds the heap over the floor of the
//! allowance and the calling thread not the process's first, the next slow path that came
//! through the Rust interface creates it ([`start_wanted`]), unless the calling thread has a
//! signal blocked or is on a signal stack, the marks of a handler, in which case a heartbeat
//! of a later interval wants it again. The C interface never creates it from a slow path: its
//! caller may be the C library under its own locks. The preload library creates it when the
//! program creates its first thread ([`Asmalloc::start_dormant`]). [`Asmalloc::start_background`]
//! starts it whatever the conditions. `ASMALLOC_BACKGROUND=0` turns the default off, as does a
//! build without the feature and a process that calls `maintain` itself.
//!
//! **What remains.** A slow path through the Rust interface may still be a Rust callback that
//! the C library runs under one of its locks (the loader's, in a backtrace, say), whose thread
//! has no signal blocked; if it is the one that finds the thread wanted, `pthread_create` may
//! wait on that lock. The window is one slow path, once.
//!
//! The thread is created with every signal blocked (the C library's `sigfillset`, which
//! leaves out its own internal signals, so that a change of credentials still reaches it).
//! It holds no lock and no allocator state between steps, so process exit and a fork find it
//! in a state they already handle; a forked child has no thread and decides afresh. The
//! ledger is `docs/safety/asmalloc/heap/background-safety.md`.

use super::global::{Asmalloc, process};
use crate::thread_word::{AWAKE, DORMANT, NONE, OFF, WANTED};
use core::ffi::{CStr, c_char, c_int, c_void};
use core::fmt;
use core::ptr;
use core::sync::atomic::AtomicBool;
use core::sync::atomic::Ordering::Relaxed;

/// Set once the program calls `maintain` itself: it then gets no default thread.
pub(super) static OPTED_OUT: AtomicBool = AtomicBool::new(false);

/// The variable that turns the default thread off (`0` or empty) or starts it at once (the
/// preload library's constructor, anything else).
const VARIABLE: &CStr = c"ASMALLOC_BACKGROUND";

/// The thread's stack: rounds need a few kilobytes, and the stack is mapped lazily anyway.
const STACK: usize = 256 << 10;

/// `sigset_t`: 1024 bits in glibc and in musl.
#[repr(C)]
struct SigSet([u64; 16]);

/// `stack_t` on x86-64.
#[repr(C)]
struct SignalStack {
    sp: *mut c_void,
    flags: c_int,
    size: usize,
}

/// `pthread_attr_t`: 56 bytes in glibc and in musl on x86-64, aligned to 8.
#[repr(C, align(8))]
struct Attr([u8; 56]);

const SIG_BLOCK: c_int = 0;
const SIG_SETMASK: c_int = 2;
const SS_ONSTACK: c_int = 1;

/// The C library's `pthread_create`, or the preload library's handle on the next one.
pub type CreateThread = unsafe extern "C" fn(
    *mut usize,
    *const c_void,
    extern "C" fn(*mut c_void) -> *mut c_void,
    *mut c_void,
) -> c_int;

// SAFETY: BACKGROUND-01.
#[link(name = "c")]
unsafe extern "C" {
    fn pthread_create(
        thread: *mut usize,
        attr: *const c_void,
        start: extern "C" fn(*mut c_void) -> *mut c_void,
        arg: *mut c_void,
    ) -> c_int;
    fn pthread_attr_init(attr: *mut Attr) -> c_int;
    fn pthread_attr_setstacksize(attr: *mut Attr, size: usize) -> c_int;
    fn pthread_attr_destroy(attr: *mut Attr) -> c_int;
    fn pthread_detach(thread: usize) -> c_int;
    fn pthread_setname_np(thread: usize, name: *const c_char) -> c_int;
    fn sigfillset(set: *mut SigSet) -> c_int;
    fn pthread_sigmask(how: c_int, set: *const SigSet, old: *mut SigSet) -> c_int;
    fn sigaltstack(stack: *const SignalStack, old: *mut SignalStack) -> c_int;
    fn getenv(name: *const c_char) -> *const c_char;
}

/// What the environment asks for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Choice {
    /// Unset: the default, on its conditions.
    Default,
    /// `0` or empty: no thread by default.
    Off,
    /// Anything else: a thread at once (the preload library's constructor).
    On,
}

/// `ASMALLOC_BACKGROUND`, read in place: `getenv` does not allocate.
pub fn choice() -> Choice {
    // SAFETY: BACKGROUND-02.
    let value = unsafe { getenv(VARIABLE.as_ptr()) };
    if value.is_null() {
        return Choice::Default;
    }
    // SAFETY: BACKGROUND-02.
    match unsafe { CStr::from_ptr(value) }.to_bytes() {
        b"" | b"0" => Choice::Off,
        _ => Choice::On,
    }
}

/// The thread could not be created; the allocator carries on without it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct StartError(pub i32);

impl fmt::Display for StartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "asmalloc's background thread was not created (error {})",
            self.0
        )
    }
}

impl core::error::Error for StartError {}

/// The heap's start hook: a Rust-interface slow path found the thread wanted.
pub(super) fn start_wanted() {
    let heap = process();
    if OPTED_OUT.load(Relaxed) || choice() == Choice::Off {
        heap.claim_thread(WANTED, OFF);
        return;
    }
    if !signals_quiet() {
        // Perhaps a handler: a heartbeat of a later interval wants it again.
        heap.claim_thread(WANTED, NONE);
        return;
    }
    if heap.claim_thread(WANTED, AWAKE)
        // SAFETY: BACKGROUND-03; the C library's own `pthread_create`.
        && unsafe { spawn(pthread_create) }.is_err()
    {
        heap.claim_thread(AWAKE, OFF);
    }
}

/// Whether the calling thread has no signal blocked and is not on a signal stack.
fn signals_quiet() -> bool {
    let mut mask = SigSet([0; 16]);
    let mut stack = SignalStack {
        sp: ptr::null_mut(),
        flags: 0,
        size: 0,
    };
    // SAFETY: BACKGROUND-04. Queries only: a null set changes no mask, a null stack no stack.
    unsafe {
        pthread_sigmask(SIG_BLOCK, ptr::null(), &mut mask) == 0
            && mask.0.iter().all(|&word| word == 0)
            && sigaltstack(ptr::null(), &mut stack) == 0
            && stack.flags & SS_ONSTACK == 0
    }
}

/// Creates the thread with `create`, with every signal blocked and a small stack, names it
/// `asmalloc` and detaches it; the caller's mask is put back after.
///
/// # Safety
///
/// `create` is the C library's `pthread_create`, or a function with its contract.
unsafe fn spawn(create: CreateThread) -> Result<(), StartError> {
    let mut all = SigSet([0; 16]);
    let mut old = SigSet([0; 16]);
    let mut attr = Attr([0; 56]);
    let mut thread = 0usize;
    // SAFETY: BACKGROUND-03.
    unsafe {
        sigfillset(&mut all);
        let blocked = pthread_sigmask(SIG_BLOCK, &all, &mut old);
        if blocked != 0 {
            return Err(StartError(blocked));
        }
        let attributed = pthread_attr_init(&mut attr) == 0;
        if attributed {
            pthread_attr_setstacksize(&mut attr, STACK);
        }
        let made = create(
            &mut thread,
            if attributed {
                ptr::from_ref(&attr).cast()
            } else {
                ptr::null()
            },
            run,
            ptr::null_mut(),
        );
        if attributed {
            pthread_attr_destroy(&mut attr);
        }
        pthread_sigmask(SIG_SETMASK, &old, ptr::null_mut());
        if made != 0 {
            return Err(StartError(made));
        }
        pthread_setname_np(thread, c"asmalloc".as_ptr());
        pthread_detach(thread);
    }
    Ok(())
}

extern "C" fn run(_: *mut c_void) -> *mut c_void {
    process().run_thread(Asmalloc::MAINTAIN_EVERY.as_nanos() as u64)
}

impl Asmalloc {
    /// Starts the background thread now, whatever the conditions on which it starts by
    /// default: a thread that takes a round of [`Asmalloc::maintain`]'s steps each interval,
    /// so that the heap returns idle memory even when no thread takes a slow path, and parks
    /// while the process is quiet. It is named `asmalloc`, is detached, and runs until the
    /// process exits.
    ///
    /// A call when the thread is running, from any thread, returns `Ok` and starts nothing.
    /// The child of a fork does not inherit the thread, and a call in the child starts one.
    ///
    /// # Errors
    ///
    /// The C library's error if the thread could not be created.
    pub fn start_background() -> Result<(), StartError> {
        let heap = process();
        // SAFETY: BACKGROUND-03; the C library's own `pthread_create`.
        heap.start_thread(AWAKE, |create| unsafe { spawn(create) }, pthread_create)
    }

    /// For the preload library: creates the thread at the program's first `pthread_create`,
    /// which is a safe moment to create one, through `create`, the C library's own. It waits
    /// dormant until a heartbeat finds the heap over the floor, or runs at once if the
    /// environment asks for that. Nothing if the environment turns it off or a thread is
    /// already running in this process.
    ///
    /// # Safety
    ///
    /// `create` is the C library's `pthread_create`, or a function with its contract.
    #[doc(hidden)]
    pub unsafe fn start_dormant(create: CreateThread) {
        let state = match choice() {
            Choice::Off => return,
            Choice::On => AWAKE,
            Choice::Default => DORMANT,
        };
        let heap = process();
        // SAFETY: passed through from the caller.
        let _ = heap.start_thread(state, |create| unsafe { spawn(create) }, create);
    }
}
