//! The background thread's start in the child of a fork taken while another thread of the
//! parent was inside `dl_iterate_phdr` (ledger review B).
//!
//! glibc's `dl_iterate_phdr` holds `dl_load_write_lock` while its callback runs. `fork`
//! resets `dl_load_lock` and `dl_load_tls_lock` in the child (`posix/fork.c`, glibc 2.43) and
//! not `dl_load_write_lock`, so a child forked at that moment inherits it locked by a thread
//! that does not exist there. The program's own `pthread_create` still works in that child:
//! it takes only the locks the fork reset. The allocator's start once called
//! `dl_iterate_phdr` itself, to size the thread's stack, and waited on that lock for ever. It
//! now creates the thread with the C library's defaults, as the program does, so an explicit
//! start and the default start both return in such a child.
//!
//! A limit, older than the stack sizing and documented in BACKGROUND-03: a Rust slow path
//! inside a `dl_iterate_phdr` callback, as Rust's backtrace code makes, that finds the thread
//! wanted calls `pthread_create` with `dl_load_write_lock` held, and `pthread_create` takes
//! `dl_load_tls_lock` (`_dl_allocate_tls_init`). A `dlopen` or `dlclose` in another thread
//! takes the two in the other order (`dl_open_worker`, `_dl_close_worker`), so the two
//! threads can wait on each other for ever. The case
//! `a_free_in_a_callback_returns_beside_a_dlopen` arranges that; it fails, so it runs only
//! with `ASMALLOC_REVIEW_KNOWN_FAILURES=1`:
//!
//! ```text
//! ASMALLOC_REVIEW_KNOWN_FAILURES=1 scripts/heavy cargo test -p asmalloc --test fork_loader_lock
//! ```
//!
//! Each case runs this binary again as a child process.
#![cfg(all(feature = "background", target_env = "gnu"))]

use asmalloc::Asmalloc;
use std::ffi::c_void;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

#[global_allocator]
static ALLOCATOR: Asmalloc = Asmalloc;

const MODE: &str = "ASMALLOC_FORK_LOADER_LOCK_CHILD";
const KNOWN_FAILURES: &str = "ASMALLOC_REVIEW_KNOWN_FAILURES";

/// The fork child's exit status when its work did not finish in time.
const HUNG: i32 = 42;
const SIGKILL: i32 = 9;
const WNOHANG: i32 = 1;

unsafe extern "C" {
    fn fork() -> i32;
    fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
    fn kill(pid: i32, signal: i32) -> i32;
    fn _exit(code: i32) -> !;
    fn dl_iterate_phdr(
        callback: extern "C" fn(*mut c_void, usize, *mut c_void) -> i32,
        data: *mut c_void,
    ) -> i32;
    fn dlopen(name: *const std::ffi::c_char, flags: i32) -> *mut c_void;
    fn pthread_create(
        thread: *mut usize,
        attr: *const c_void,
        start: extern "C" fn(*mut c_void) -> *mut c_void,
        arg: *mut c_void,
    ) -> i32;
}

static INSIDE: AtomicBool = AtomicBool::new(false);

/// A callback that stays in the iteration, as one that symbolizes a backtrace does for a
/// while: the thread holds the loader's iteration lock until the process exits.
extern "C" fn stay(_: *mut c_void, _: usize, _: *mut c_void) -> i32 {
    INSIDE.store(true, Ordering::SeqCst);
    loop {
        thread::sleep(Duration::from_secs(3600));
    }
}

extern "C" fn nothing(_: *mut c_void) -> *mut c_void {
    std::ptr::null_mut()
}

/// In the fork child: what the mode asks, then `_exit(0)`.
fn in_fork_child(mode: &str) -> ! {
    match mode {
        // The program's own thread, with the C library's defaults.
        "control" => {
            let mut thread = 0usize;
            // SAFETY: valid arguments for `pthread_create`; the thread returns at once.
            let made = unsafe {
                pthread_create(&mut thread, std::ptr::null(), nothing, std::ptr::null_mut())
            };
            // SAFETY: ends the child.
            unsafe { _exit(if made == 0 { 0 } else { 3 }) }
        }
        "explicit" => {
            let started = Asmalloc::start_background();
            // SAFETY: ends the child.
            unsafe { _exit(if started.is_ok() { 0 } else { 3 }) }
        }
        // The default start: a second thread of the child frees a burst over the floor, and
        // one of its frees finds the thread wanted.
        "default" => {
            let (done, wait) = mpsc::channel();
            thread::spawn(move || {
                let burst: Vec<Box<[u8; 64]>> = (0..(128 << 20) / 64)
                    .map(|i| Box::new([i as u8; 64]))
                    .collect();
                drop(burst);
                done.send(()).unwrap();
            });
            let finished = wait.recv_timeout(Duration::from_secs(30)).is_ok();
            // SAFETY: ends the child.
            unsafe { _exit(if finished { 0 } else { HUNG }) }
        }
        _ => {
            // SAFETY: ends the child.
            unsafe { _exit(4) }
        }
    }
}

static OPENING: AtomicBool = AtomicBool::new(false);

/// A callback that allocates, as one that symbolizes a backtrace does: once another thread is
/// in `dlopen`, it frees a burst over the floor, and one of the frees finds the thread wanted.
extern "C" fn allocate(_: *mut c_void, _: usize, _: *mut c_void) -> i32 {
    INSIDE.store(true, Ordering::SeqCst);
    while !OPENING.load(Ordering::SeqCst) {
        thread::sleep(Duration::from_millis(1));
    }
    // Long enough for the other thread to be waiting inside `dlopen`.
    thread::sleep(Duration::from_millis(500));
    let burst: Vec<Box<[u8; 64]>> = (0..(128 << 20) / 64)
        .map(|i| Box::new([i as u8; 64]))
        .collect();
    drop(burst);
    1
}

/// The child process of the second finding: no fork.
fn beside_a_dlopen() -> ! {
    let (done, wait) = mpsc::channel();
    thread::spawn(move || {
        // SAFETY: a callback of the right type that reads nothing it is passed.
        unsafe { dl_iterate_phdr(allocate, std::ptr::null_mut()) };
        done.send(()).unwrap();
    });
    while !INSIDE.load(Ordering::SeqCst) {
        thread::sleep(Duration::from_millis(1));
    }
    thread::spawn(|| {
        OPENING.store(true, Ordering::SeqCst);
        // A library of glibc's that the test binary has not loaded. RTLD_NOW.
        // SAFETY: a NUL-terminated name.
        unsafe { dlopen(c"libBrokenLocale.so.1".as_ptr(), 2) };
    });
    if wait.recv_timeout(Duration::from_secs(30)).is_ok() {
        println!("callback returned");
    } else {
        println!("callback hung");
    }
    // The threads may be stuck, one of them inside `dlopen` with the loader's lock, which
    // `exit` would wait for: leave without running exit handlers.
    std::io::Write::flush(&mut std::io::stdout()).unwrap();
    // SAFETY: ends the process.
    unsafe { _exit(0) }
}

/// The child process: a thread enters `dl_iterate_phdr` and stays, then the process forks.
#[test]
fn child() {
    let Ok(mode) = std::env::var(MODE) else {
        return;
    };
    if mode == "dlopen" {
        beside_a_dlopen();
    }
    thread::spawn(|| {
        // SAFETY: a callback of the right type that reads nothing it is passed.
        unsafe { dl_iterate_phdr(stay, std::ptr::null_mut()) };
    });
    while !INSIDE.load(Ordering::SeqCst) {
        thread::sleep(Duration::from_millis(1));
    }
    // SAFETY: the child calls what a program calls in a fork child that goes on running.
    let pid = unsafe { fork() };
    if pid == 0 {
        in_fork_child(&mode);
    }
    let started = Instant::now();
    let mut status = 0;
    loop {
        // SAFETY: a valid pointer for the status.
        if unsafe { waitpid(pid, &mut status, WNOHANG) } == pid {
            break;
        }
        if started.elapsed() > Duration::from_secs(45) {
            // SAFETY: the process this test forked, by its ID.
            unsafe {
                kill(pid, SIGKILL);
                waitpid(pid, &mut status, 0);
            }
            println!("fork_child hung");
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }
    println!("fork_child status {status}");
}

/// Runs the child process for `mode` and returns what it printed.
fn run(mode: &str) -> String {
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "child", "--nocapture", "--test-threads=1"])
        .env(MODE, mode)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn known_failures() -> bool {
    std::env::var_os(KNOWN_FAILURES).is_some()
}

/// The program's own `pthread_create` works in such a fork child, as the allocator's start
/// must.
#[test]
fn the_programs_own_thread_is_created_in_the_fork_child() {
    let report = run("control");
    assert!(report.contains("fork_child status 0\n"), "{report}");
}

#[test]
fn an_explicit_start_returns_in_the_fork_child() {
    let report = run("explicit");
    assert!(report.contains("fork_child status 0\n"), "{report}");
}

#[test]
fn a_free_that_starts_the_thread_returns_in_the_fork_child() {
    let report = run("default");
    assert!(report.contains("fork_child status 0\n"), "{report}");
}

/// The limit BACKGROUND-03 documents: it hangs, so it runs only when asked for.
#[test]
fn a_free_in_a_callback_returns_beside_a_dlopen() {
    if !known_failures() {
        return;
    }
    let report = run("dlopen");
    assert!(report.contains("callback returned\n"), "{report}");
}
