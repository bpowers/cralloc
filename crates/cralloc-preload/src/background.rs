//! The background thread, for a program run with the library (design: "The background thread").
//!
//! No slow path of the library ever creates the thread: its caller may be the C library under
//! one of its own locks. The library interposes `pthread_create` instead, and creates the
//! allocator's thread when the program creates its first thread, a moment that is safe by
//! construction and is also when the process stops having only one thread. The thread waits
//! dormant until a heartbeat finds the heap over the floor of the allowance, so a small
//! multi-threaded program has a thread that never runs. `CRALLOC_BACKGROUND=0` turns it off;
//! any other value starts it running from the library's constructor, before `main`, as
//! before. The variable is read from the environment the process was started with, by the
//! dynamic loader's first `malloc` or else by the constructor, before `main` either way: a
//! program that later writes a process title over that environment, or calls `setenv`,
//! keeps the setting it started with (`cralloc::background::choice`).
//!
//! The interposer forwards every call to the next `pthread_create` (`dlsym(RTLD_NEXT)`), the
//! C library's, and creates the allocator's thread through that one too. The C library's own
//! thread creators (timers that notify by thread, POSIX AIO) call its internal entry and pass
//! the interposer by, as does every thread of a statically linked program: a program whose
//! only other threads come from those gets no thread. The child of a fork creates its own at
//! its first `pthread_create`.
//!
//! The ledger is `docs/safety/cralloc-preload/background-safety.md`.

use core::ffi::{c_char, c_int, c_void};
use core::sync::atomic::Ordering::{AcqRel, Acquire, Release};
use core::sync::atomic::{AtomicPtr, AtomicU32};
use cralloc::Cralloc;
use cralloc::background::{Choice, CreateThread, choice};

/// `RTLD_NEXT`: the next object after this one in the lookup order.
const RTLD_NEXT: *mut c_void = -1isize as *mut c_void;

/// `EAGAIN`, `pthread_create`'s error when a thread cannot be had.
const EAGAIN: c_int = 11;

// SAFETY: PRELOAD-03.
#[link(name = "c")]
unsafe extern "C" {
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn getpid() -> c_int;
}

/// The next `pthread_create`, once looked up; null before.
static NEXT: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());

/// The process that created the allocator's thread at its first `pthread_create`; zero for
/// none. A forked child's differs from its own.
static CREATED_IN: AtomicU32 = AtomicU32::new(0);

/// The next `pthread_create`, looked up on first use. `dlsym` may allocate; the library's
/// allocator serves that like any other call.
fn next() -> Option<CreateThread> {
    let mut found = NEXT.load(Acquire);
    if found.is_null() {
        // SAFETY: PRELOAD-05. A NUL-terminated name and the special handle.
        found = unsafe { dlsym(RTLD_NEXT, c"pthread_create".as_ptr()) };
        NEXT.store(found, Release);
    }
    // SAFETY: PRELOAD-05. The symbol the C library defines under that name is its
    // `pthread_create`, whose type is `CreateThread`.
    (!found.is_null()).then(|| unsafe { core::mem::transmute::<*mut c_void, CreateThread>(found) })
}

/// The constructor, which the dynamic loader runs once the library and its dependencies are
/// relocated and initialized, before `main`.
// SAFETY: PRELOAD-04.
#[used]
#[unsafe(link_section = ".init_array")]
static CONSTRUCTOR: extern "C" fn() = start;

extern "C" fn start() {
    // Reads the switch, if no `malloc` has yet, while the environment is still the one the
    // process was started with.
    if choice() == Choice::On
        && let Some(create) = next()
    {
        // SAFETY: PRELOAD-06; the C library's `pthread_create`.
        unsafe { Cralloc::start_dormant(create) };
    }
}

// SAFETY: PRELOAD-06, the exported name.
/// The program's `pthread_create`: the allocator's thread first, at the process's first
/// call, then the program's.
///
/// # Safety
///
/// The C contract of `pthread_create`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pthread_create(
    thread: *mut usize,
    attr: *const c_void,
    start: extern "C" fn(*mut c_void) -> *mut c_void,
    arg: *mut c_void,
) -> c_int {
    let Some(create) = next() else {
        return EAGAIN;
    };
    // SAFETY: PRELOAD-03. `getpid` takes nothing and cannot fail.
    let process = unsafe { getpid() } as u32;
    let seen = CREATED_IN.load(Acquire);
    if seen != process
        && CREATED_IN
            .compare_exchange(seen, process, AcqRel, Acquire)
            .is_ok()
    {
        // SAFETY: PRELOAD-06; the C library's `pthread_create`.
        unsafe { Cralloc::start_dormant(create) };
    }
    // SAFETY: PRELOAD-06; the program's own call, forwarded whole.
    unsafe { create(thread, attr, start, arg) }
}
