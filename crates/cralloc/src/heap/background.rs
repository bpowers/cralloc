//! The background thread (feature `background`, on by default): an ordinary thread of the
//! process, created with the C library's `pthread_create`, that takes rounds of
//! [`Cralloc::maintain`]'s steps and the purge steps slow paths leave to it while it runs,
//! sleeps between them until a slow path wakes it for work that came due or its timeout, and
//! parks while the process is quiet (`depot::thread`; design: "The background thread").
//!
//! **When it starts.** By default, once free memory is over the floor of the allowance and the
//! program has created a thread of its own, a slow path that came through the Rust interface
//! creates it ([`start_wanted`]; the condition is a level, looked at as `crate::thread_word`
//! describes). Not if the calling thread has a signal blocked or is on a signal stack, the
//! marks of a handler: the look once an interval wants it again. The C interface never creates
//! it from a slow path: its caller may be the C library under its own locks. The preload
//! library creates it when the program creates its first thread ([`Cralloc::start_dormant`]).
//! [`Cralloc::start_background`] starts it whatever the conditions. `CRALLOC_BACKGROUND=0`
//! in the environment the process was started with turns the default off, as does a build
//! without the feature and a process that calls `maintain` itself. The variable is read once,
//! from the kernel's copy of that environment, so a `setenv` in the program changes nothing
//! ([`choice`], `crate::environment`).
//!
//! **Limits.** A Rust program that keeps a signal blocked in every thread (one that waits for
//! signals on a thread of its own with `sigwait`, say) never passes the test, and starts the
//! thread itself. A handler installed with `SA_NODEFER` and no alternate stack has neither
//! mark and passes it. A thread with no rseq registration has no activity counter, so the
//! thread does not see it work; it takes a slow path on every call, so it wakes a parked
//! thread all the same. Where the C library keeps no record of threads created (musl), the
//! evidence is that the thread at the crossing is not the process's first.
//!
//! **What remains.** A slow path through the Rust interface may still be a Rust callback that
//! the C library runs under one of its locks (the loader's, in a backtrace, say), whose thread
//! has no signal blocked; if it is the one that finds the thread wanted, `pthread_create` may
//! wait on that lock. The window is one slow path, once. Inside a `dl_iterate_phdr` callback,
//! while another thread is in `dlopen` or `dlclose`, that wait is a deadlock with glibc
//! (BACKGROUND-03, "Locks").
//!
//! The thread is created with every signal blocked (the C library's `sigfillset`, which
//! leaves out its own internal signals, so that a change of credentials still reaches it).
//! It holds no lock and no allocator state between steps, so process exit and a fork find it
//! in a state they already handle; a forked child has no thread and decides afresh. It has the
//! C library's default stack, as the program's own threads do (`docs/research/portability.md`,
//! "The background thread's stack").
//! The ledger is `docs/safety/cralloc/heap/background-safety.md`.

use super::global::{Cralloc, process};
use crate::c::errno;
use crate::os::Os;
use crate::thread_word::{AWAKE, DORMANT, NONE, OFF, WANTED};
use core::ffi::{c_int, c_void};
use core::fmt;
use core::ptr;
use core::sync::atomic::AtomicBool;
use core::sync::atomic::Ordering::Relaxed;

/// Set once the program calls `maintain` itself: it then gets no default thread.
pub(super) static OPTED_OUT: AtomicBool = AtomicBool::new(false);

/// `sigset_t`: 1024 bits in glibc and in musl.
#[repr(C)]
struct SigSet([u64; 16]);

/// `stack_t` on x86-64 and aarch64.
#[repr(C)]
struct SignalStack {
    sp: *mut c_void,
    flags: c_int,
    size: usize,
}

/// What a start reports when the heap could not start, so that there is no generation to key
/// the thread's word with: `ENOMEM`.
const ENOMEM: i32 = 12;

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
    fn pthread_detach(thread: usize) -> c_int;
    fn sigfillset(set: *mut SigSet) -> c_int;
    fn pthread_sigmask(how: c_int, set: *const SigSet, old: *mut SigSet) -> c_int;
    fn sigaltstack(stack: *const SignalStack, old: *mut SignalStack) -> c_int;
}

pub use crate::environment::Choice;

/// What `CRALLOC_BACKGROUND` asks for in the environment the process was started with: read
/// once per process, at its first slow path or here, whichever comes first, and the same
/// for the rest of its life and its forked children's. A `setenv` in the program does not
/// change it (`crate::environment`). Why this needs no C library call: BACKGROUND-02.
pub fn choice() -> Choice {
    let heap = process().heap();
    heap.depot.background_choice(heap.os())
}

/// The thread could not be created; the allocator carries on without it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct StartError(pub i32);

impl fmt::Display for StartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "cralloc's background thread was not created (error {})",
            self.0
        )
    }
}

impl core::error::Error for StartError {}

/// The heap's start hook: a Rust-interface slow path found the thread wanted.
///
/// A limit: a child that shares its parent's memory (`vfork`, `CLONE_VM`) shares its
/// generation, so a Rust slow path there that finds the word wanted creates the thread in the
/// child. Allocating in such a child is outside what the allocator supports (design,
/// "Non-Goals").
pub(super) fn start_wanted() {
    let heap = process();
    if OPTED_OUT.load(Relaxed) || choice() == Choice::Off {
        heap.claim_thread(WANTED, OFF);
        return;
    }
    if !signals_quiet() {
        // Perhaps a handler, or a program that keeps a signal blocked in this thread: the
        // look at the level once an interval wants it again, on whichever thread then takes
        // a slow path. A program that keeps a signal blocked in every thread never passes, and
        // starts the thread itself (`Cralloc::start_background`).
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

/// Whether the calling thread has no signal blocked and is not on a signal stack: the marks of
/// a signal handler, in which creating a thread is not safe. A limit: a handler installed with
/// `SA_NODEFER` and no alternate stack has neither mark, and passes.
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

/// Creates the thread with `create`, with every signal blocked and the C library's default
/// attributes, and detaches it; the caller's mask and `errno` are put back after, so that the
/// allocation or free that started it leaves both as it found them. The default stack is the
/// one the library gives the program's own threads, sized by the library against its own
/// thread-local data (`docs/research/portability.md`, "The background thread's stack"). The
/// thread names itself (`run`).
///
/// # Safety
///
/// `create` is the C library's `pthread_create`, or a function with its contract.
unsafe fn spawn(create: CreateThread) -> Result<(), StartError> {
    let saved = errno::get();
    // SAFETY: BACKGROUND-03; `create` passed through from the caller.
    let made = unsafe { create_detached(create) };
    errno::set(saved);
    made
}

/// [`spawn`] without keeping `errno`.
///
/// # Safety
///
/// As for [`spawn`].
unsafe fn create_detached(create: CreateThread) -> Result<(), StartError> {
    let mut all = SigSet([0; 16]);
    let mut old = SigSet([0; 16]);
    let mut thread = 0usize;
    // SAFETY: BACKGROUND-03.
    unsafe {
        sigfillset(&mut all);
        let blocked = pthread_sigmask(SIG_BLOCK, &all, &mut old);
        if blocked != 0 {
            return Err(StartError(blocked));
        }
        let made = create(&mut thread, ptr::null(), run, ptr::null_mut());
        pthread_sigmask(SIG_SETMASK, &old, ptr::null_mut());
        if made != 0 {
            return Err(StartError(made));
        }
        pthread_detach(thread);
    }
    Ok(())
}

/// The thread's body. It names itself with the crate's own system call: the C library's
/// `pthread_setname_np` on another thread opens a file under `/proc`, which a sandbox
/// entered after start may forbid.
extern "C" fn run(_: *mut c_void) -> *mut c_void {
    let heap = process();
    heap.heap().os().name_thread(c"cralloc");
    heap.run_thread(Cralloc::MAINTAIN_EVERY.as_nanos() as u64)
}

impl Cralloc {
    /// Starts the background thread now, whatever the conditions on which it starts by
    /// default: a thread that takes a round of [`Cralloc::maintain`]'s steps each interval,
    /// so that the heap returns idle memory even when no thread takes a slow path, and parks
    /// while the process is quiet. It is named `cralloc`, is detached, and runs until the
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
            .map_err(|error| error.unwrap_or(StartError(ENOMEM)))
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
            Choice::Off => {
                // So that no slow path finds the word wanted, and goes to the cold look, for
                // the rest of the process: nothing here would act on it.
                let heap = process();
                heap.claim_thread(NONE, OFF);
                heap.claim_thread(WANTED, OFF);
                return;
            }
            Choice::On => AWAKE,
            Choice::Default => DORMANT,
        };
        let heap = process();
        // SAFETY: BACKGROUND-03; `create` passed through from the caller.
        let _ = heap.start_thread(state, |create| unsafe { spawn(create) }, create);
    }
}
