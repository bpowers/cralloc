//! The background thread's start, as a program sees it: the thread is named `cralloc`; its
//! start opens no file, so a process that forbids opening files after its first allocation
//! (a seccomp filter that kills `open`, `openat` and `openat2`) survives it and gets the
//! thread, by an explicit start and by the default one, which the count of threads in a
//! `status` file opened beforehand shows; and a start, explicit or through the free that
//! finds the thread wanted, leaves `errno` as it was, whether it succeeds or fails.
//!
//! Each case runs this binary again as a child process with the case's mode set.

use cralloc::Cralloc;
use std::os::unix::process::ExitStatusExt;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

#[global_allocator]
static ALLOCATOR: Cralloc = Cralloc;

const MODE: &str = "CRALLOC_THREAD_START_CHILD";
/// A value no call here sets.
const SENTINEL: i32 = 4321;
const SIGSYS: i32 = 31;

#[repr(C)]
struct SockFilter {
    code: u16,
    jt: u8,
    jf: u8,
    k: u32,
}

#[repr(C)]
struct SockFprog {
    len: u16,
    filter: *const SockFilter,
}

unsafe extern "C" {
    fn prctl(option: i32, ...) -> i32;
    fn __errno_location() -> *mut i32;
    fn setrlimit(resource: i32, limit: *const [u64; 2]) -> i32;
    fn getrlimit(resource: i32, limit: *mut [u64; 2]) -> i32;
    fn geteuid() -> u32;
}

fn errno() -> i32 {
    // SAFETY: the C library's accessor of the calling thread's `errno`.
    unsafe { *__errno_location() }
}

fn set_errno(value: i32) {
    // SAFETY: as above.
    unsafe { *__errno_location() = value }
}

/// Kills the process on any `open`, `openat` or `openat2` from now on.
fn forbid_opening_files() {
    const LD_NR: u16 = 0x20; // BPF_LD | BPF_W | BPF_ABS, offset 0: the system call's number
    const JEQ: u16 = 0x15; // BPF_JMP | BPF_JEQ | BPF_K
    const RET: u16 = 0x06; // BPF_RET | BPF_K
    let filter = [
        SockFilter {
            code: LD_NR,
            jt: 0,
            jf: 0,
            k: 0,
        },
        SockFilter {
            code: JEQ,
            jt: 3,
            jf: 0,
            k: 2,
        }, // open
        SockFilter {
            code: JEQ,
            jt: 2,
            jf: 0,
            k: 257,
        }, // openat
        SockFilter {
            code: JEQ,
            jt: 1,
            jf: 0,
            k: 437,
        }, // openat2
        SockFilter {
            code: RET,
            jt: 0,
            jf: 0,
            k: 0x7fff_0000,
        }, // SECCOMP_RET_ALLOW
        SockFilter {
            code: RET,
            jt: 0,
            jf: 0,
            k: 0x8000_0000,
        }, // SECCOMP_RET_KILL_PROCESS
    ];
    let program = SockFprog {
        len: filter.len() as u16,
        filter: filter.as_ptr(),
    };
    // SAFETY: PR_SET_NO_NEW_PRIVS (38) takes integers; PR_SET_SECCOMP (22) with
    // SECCOMP_MODE_FILTER (2) reads the program, which outlives the call.
    unsafe {
        assert_eq!(prctl(38, 1u64, 0u64, 0u64, 0u64), 0, "no_new_privs");
        assert_eq!(prctl(22, 2u64, &raw const program), 0, "seccomp");
    }
}

/// A second thread that stays until the process exits, so the default start's condition of
/// a thread of the program's own holds.
fn keep_a_thread() {
    let (keep, kept) = mpsc::channel::<()>();
    std::mem::forget(keep);
    thread::spawn(move || kept.recv());
}

/// A worker frees a burst over the floor, so that one of its frees finds the thread wanted,
/// with `errno` set to the sentinel first; what `errno` is after the frees.
fn burst_on_a_worker() -> i32 {
    thread::spawn(|| {
        let burst: Vec<Box<[u8; 64]>> = (0..(128 << 20) / 64)
            .map(|i| Box::new([i as u8; 64]))
            .collect();
        set_errno(SENTINEL);
        drop(burst);
        errno()
    })
    .join()
    .unwrap()
}

/// The process's `status` file, opened before the sandbox: read in place, it gives the
/// count of threads without opening anything, as `default_thread.rs` reads its fields.
fn status() -> std::fs::File {
    std::fs::File::open("/proc/self/status").unwrap()
}

/// The process's threads, from its `status` file.
fn threads(status: &std::fs::File) -> u64 {
    use std::os::unix::fs::FileExt;
    let mut buffer = [0u8; 4096];
    let n = status.read_at(&mut buffer, 0).unwrap();
    let text = std::str::from_utf8(&buffer[..n]).unwrap();
    text.lines()
        .find_map(|line| line.strip_prefix("Threads:"))
        .and_then(|count| count.trim().parse().ok())
        .unwrap()
}

/// Waits up to two seconds for one thread more than `before`; whether it came. The worker of
/// a burst has been joined by then, so the one more is the allocator's.
fn one_more_thread(status: &std::fs::File, before: u64) -> bool {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(2) {
        if threads(status) == before + 1 {
            return true;
        }
        thread::sleep(Duration::from_millis(20));
    }
    false
}

fn has_allocator_thread() -> bool {
    std::fs::read_dir("/proc/self/task").is_ok_and(|tasks| {
        tasks.filter_map(Result::ok).any(|task| {
            std::fs::read_to_string(task.path().join("comm")).is_ok_and(|n| n.trim() == "cralloc")
        })
    })
}

#[test]
fn child() {
    let Ok(mode) = std::env::var(MODE) else {
        return;
    };
    // The process's first slow path, which reads what it reads at start.
    drop(Box::new(0u64));
    keep_a_thread();
    match mode.as_str() {
        "named" => {
            Cralloc::start_background().unwrap();
            let started = Instant::now();
            while !has_allocator_thread() && started.elapsed() < Duration::from_secs(5) {
                thread::sleep(Duration::from_millis(20));
            }
            println!("named {}", u8::from(has_allocator_thread()));
        }
        "sandbox-explicit" => {
            let status = status();
            let before = threads(&status);
            forbid_opening_files();
            Cralloc::start_background().unwrap();
            println!(
                "survived, started {}",
                u8::from(one_more_thread(&status, before))
            );
        }
        "sandbox-default" => {
            let status = status();
            let before = threads(&status);
            forbid_opening_files();
            burst_on_a_worker();
            println!(
                "survived, started {}",
                u8::from(one_more_thread(&status, before))
            );
        }
        "errno-explicit" => {
            set_errno(SENTINEL);
            let started = Cralloc::start_background();
            println!("errno {} started {}", errno(), u8::from(started.is_ok()));
        }
        "errno-default" => {
            println!("errno {}", burst_on_a_worker());
        }
        "errno-failed" => {
            // RLIMIT_NPROC (6) at 1 refuses every new thread of a user who runs anything
            // else (`EAGAIN`), unless the process is root.
            let mut old = [0u64; 2];
            // SAFETY: valid pointers to two-word limits.
            unsafe {
                getrlimit(6, &mut old);
                setrlimit(6, &[1, old[1]]);
            }
            set_errno(SENTINEL);
            let started = Cralloc::start_background();
            println!(
                "errno {} started {} root {}",
                errno(),
                u8::from(started.is_ok()),
                // SAFETY: takes nothing.
                u8::from(unsafe { geteuid() } == 0)
            );
        }
        _ => panic!("unknown mode {mode}"),
    }
}

/// Runs the child for `mode`, with the variable that would turn the thread off removed.
fn run(mode: &str) -> (std::process::ExitStatus, String) {
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "child", "--nocapture", "--test-threads=1"])
        .env(MODE, mode)
        .env_remove("CRALLOC_BACKGROUND")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    (
        output.status,
        String::from_utf8_lossy(&output.stdout).into_owned(),
    )
}

#[test]
fn the_thread_is_named() {
    let (status, out) = run("named");
    assert!(
        status.success() && out.contains("named 1"),
        "{status:?} {out}"
    );
}

#[test]
fn a_start_opens_no_file() {
    for mode in ["sandbox-explicit", "sandbox-default"] {
        let (status, out) = run(mode);
        assert_ne!(
            status.signal(),
            Some(SIGSYS),
            "{mode}: killed for opening a file"
        );
        assert!(
            status.success() && out.contains("survived, started 1"),
            "{mode}: {status:?} {out}"
        );
    }
}

#[test]
fn a_start_keeps_errno() {
    let (status, out) = run("errno-explicit");
    assert!(
        status.success() && out.contains(&format!("errno {SENTINEL} started 1")),
        "{status:?} {out}"
    );
    let (status, out) = run("errno-default");
    assert!(
        status.success() && out.contains(&format!("errno {SENTINEL}")),
        "{status:?} {out}"
    );
    let (status, out) = run("errno-failed");
    assert!(status.success(), "{status:?} {out}");
    if !out.contains("root 1") {
        assert!(
            out.contains(&format!("errno {SENTINEL} started 0")),
            "a failed start: {out}"
        );
    }
}
