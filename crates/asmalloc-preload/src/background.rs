//! The background thread, for a program run with the library: with `ASMALLOC_BACKGROUND` set
//! to anything but empty or `0`, a thread that sleeps `Asmalloc::MAINTAIN_EVERY` and calls
//! `asmalloc::Asmalloc::maintain`, forever, so that a process whose threads stay inside their
//! caches still returns its idle memory.
//!
//! The library's constructor reads the variable once, when the dynamic loader runs it before
//! `main`. `getenv` reads the environment in place, without allocating. If asked, it starts
//! the thread with the C library's `pthread_create`, so that the thread is an ordinary one of
//! the program's, with its TLS and rseq registration, and with every signal blocked in it
//! (the C library keeps its own internal signals, which `sigfillset` leaves out): the
//! program's handlers never run on it, and process-directed signals go to the program's own
//! threads. The thread is never joined and never exits. A fork's child does not have it.
//!
//! These are the library's only references to the C library besides the allocator's
//! `__errno_location` (`asmalloc::c`). The ledger is
//! `docs/safety/asmalloc-preload/background-safety.md`.

use core::ffi::{CStr, c_char, c_int, c_long, c_void};
use core::ptr;

/// The variable that asks for the thread.
const VARIABLE: &CStr = c"ASMALLOC_BACKGROUND";

/// `sigset_t`: 1024 bits in glibc and in musl.
#[repr(C)]
struct SigSet([u64; 16]);

/// `struct timespec` on x86-64.
#[repr(C)]
struct Timespec {
    tv_sec: c_long,
    tv_nsec: c_long,
}

/// `pthread_sigmask`'s `how`: add to the mask, and set it.
const SIG_BLOCK: c_int = 0;
const SIG_SETMASK: c_int = 2;

// SAFETY: PRELOAD-03.
#[link(name = "c")]
unsafe extern "C" {
    fn getenv(name: *const c_char) -> *const c_char;
    fn sigfillset(set: *mut SigSet) -> c_int;
    fn pthread_sigmask(how: c_int, set: *const SigSet, old: *mut SigSet) -> c_int;
    fn pthread_create(
        thread: *mut usize,
        attr: *const c_void,
        start: extern "C" fn(*mut c_void) -> *mut c_void,
        arg: *mut c_void,
    ) -> c_int;
    fn nanosleep(request: *const Timespec, remaining: *mut Timespec) -> c_int;
}

/// The constructor, which the dynamic loader runs once the library and its dependencies are
/// relocated and initialized, before `main`.
// SAFETY: PRELOAD-04.
#[used]
#[unsafe(link_section = ".init_array")]
static CONSTRUCTOR: extern "C" fn() = start;

extern "C" fn start() {
    if requested() {
        spawn();
    }
}

/// Whether the environment asks for the thread.
fn requested() -> bool {
    // SAFETY: PRELOAD-05.
    let value = unsafe { getenv(VARIABLE.as_ptr()) };
    if value.is_null() {
        return false;
    }
    // SAFETY: PRELOAD-05.
    let value = unsafe { CStr::from_ptr(value) }.to_bytes();
    !value.is_empty() && value != b"0"
}

/// Starts the thread with every signal blocked, which it inherits from the mask in force when
/// it is created; the caller's own mask is put back after. A failure starts nothing, silently:
/// the program runs as it would without the variable.
fn spawn() {
    let mut all = SigSet([0; 16]);
    let mut old = SigSet([0; 16]);
    let mut thread = 0usize;
    // SAFETY: PRELOAD-06.
    unsafe {
        sigfillset(&mut all);
        if pthread_sigmask(SIG_BLOCK, &all, &mut old) != 0 {
            return;
        }
        let _ = pthread_create(&mut thread, ptr::null(), maintain_forever, ptr::null_mut());
        pthread_sigmask(SIG_SETMASK, &old, ptr::null_mut());
    }
}

extern "C" fn maintain_forever(_: *mut c_void) -> *mut c_void {
    let every = asmalloc::Asmalloc::MAINTAIN_EVERY;
    let period = Timespec {
        tv_sec: every.as_secs() as c_long,
        tv_nsec: every.subsec_nanos() as c_long,
    };
    loop {
        // An early wake, by a signal the C library sends itself, only makes this round early.
        // SAFETY: PRELOAD-07.
        unsafe { nanosleep(&period, ptr::null_mut()) };
        asmalloc::Asmalloc::maintain();
    }
}
