//! The background thread's default start, through the Rust interface, each case in a process
//! of its own (the crate's own test binary calls `maintain`, which opts it out). The binary
//! runs itself once per case with `ASMALLOC_DEFAULT_THREAD_MODE` set; each child does what its
//! mode says, goes silent, and reports whether a thread named `asmalloc` exists, how often it
//! woke once it had parked, and its anonymous resident memory. The thread's wake-ups are its
//! voluntary context switches, read from its `status` file opened beforehand and read in
//! place, so that the child takes no slow path while it counts them.
//!
//! Every case takes tens of seconds of silence, so all are ignored by default; `scripts/gate
//! --all` runs them, in parallel. One case: `cargo test -p asmalloc --test default_thread --
//! --include-ignored worker`. A child alone: `ASMALLOC_DEFAULT_THREAD_MODE=worker` and the
//! test binary's path. The modes are the review's probe modes and the cases its findings
//! named (`docs/research/default-thread.md`).

use asmalloc::Asmalloc;
use std::collections::BTreeMap;
use std::fs::File;
use std::os::unix::fs::FileExt;
use std::process::Command;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

#[global_allocator]
static ALLOCATOR: Asmalloc = Asmalloc;

const MODE: &str = "ASMALLOC_DEFAULT_THREAD_MODE";

type Block = Box<[u8; 64]>;

fn burst(mib: usize) -> Vec<Block> {
    (0..mib * (1 << 20) / 64)
        .map(|i| Box::new([i as u8; 64]))
        .collect()
}

/// A field of a `status` file, read in place into a buffer on the stack: no allocation.
fn field(file: &File, name: &[u8]) -> Option<u64> {
    let mut buffer = [0u8; 4096];
    let n = file.read_at(&mut buffer, 0).ok()?;
    let at = buffer[..n].windows(name.len()).position(|w| w == name)? + name.len();
    let mut value = 0u64;
    let mut digits = buffer[at..n].iter().skip_while(|b| !b.is_ascii_digit());
    for &byte in &mut digits {
        if !byte.is_ascii_digit() {
            break;
        }
        value = value * 10 + u64::from(byte - b'0');
    }
    Some(value)
}

/// The allocator's thread's `status` file, if the thread exists.
fn allocator_thread() -> Option<File> {
    std::fs::read_dir("/proc/self/task").ok()?.find_map(|task| {
        let path = task.ok()?.path();
        let name = std::fs::read_to_string(path.join("comm")).ok()?;
        (name.trim() == "asmalloc")
            .then(|| File::open(path.join("status")).ok())
            .flatten()
    })
}

/// What a child reports, printed with `prefix` at the end of its run.
struct Report {
    thread: bool,
    parked_after: Option<u64>,
    wakes: Option<u64>,
    rss_mib: u64,
}

impl Report {
    fn print(&self, prefix: &str) {
        println!("{prefix}thread {}", u64::from(self.thread));
        if let Some(seconds) = self.parked_after {
            println!("{prefix}parked_after {seconds}");
        }
        if let Some(wakes) = self.wakes {
            println!("{prefix}wakes {wakes}");
        }
        println!("{prefix}rss_mib {}", self.rss_mib);
    }
}

/// Silence: looks for the thread, waits for it to park (no voluntary switch for three seconds,
/// where an awake thread makes about two a second), counts its wake-ups over five seconds more,
/// and reports. Takes no slow path after the look for the thread.
fn silence() -> Report {
    let me = File::open("/proc/self/status").unwrap();
    let status = allocator_thread();
    let switches = || {
        status
            .as_ref()
            .and_then(|file| field(file, b"\nvoluntary_ctxt_switches:"))
    };
    let (mut parked_after, mut wakes) = (None, None);
    if status.is_some() {
        let mut last = switches();
        let mut quiet = 0;
        for second in 1..=45 {
            thread::sleep(Duration::from_secs(1));
            let now = switches();
            quiet = if now == last { quiet + 1 } else { 0 };
            last = now;
            if quiet >= 3 {
                parked_after = Some(second);
                break;
            }
        }
        thread::sleep(Duration::from_secs(5));
        wakes = switches().zip(last).map(|(now, then)| now - then);
    } else {
        thread::sleep(Duration::from_secs(10));
    }
    Report {
        thread: status.is_some(),
        parked_after,
        wakes,
        rss_mib: field(&me, b"\nRssAnon:").unwrap_or(0) / 1024,
    }
}

/// Small allocations and frees for `seconds`, 64 KiB every 100 ms: slow paths in several
/// intervals, so that the look at the level once an interval runs, and too little to cross
/// another eighth of the floor (4 MiB) of frees, which is what retried a start before the
/// look did.
fn light_work(seconds: u64) {
    for _ in 0..seconds * 10 {
        drop(
            (0..1024)
                .map(|i| Box::new([i as u8; 64]))
                .collect::<Vec<Block>>(),
        );
        thread::sleep(Duration::from_millis(100));
    }
}

/// A thread that stays alive until the process exits, blocked, so that the process has more
/// than one thread throughout.
fn keep_a_thread() {
    let (keep, kept) = mpsc::channel::<()>();
    std::mem::forget(keep);
    thread::spawn(move || kept.recv());
}

unsafe extern "C" {
    fn pthread_sigmask(how: i32, set: *const [u64; 16], old: *mut [u64; 16]) -> i32;
    fn fork() -> i32;
    fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
    fn setrlimit(resource: i32, limit: *const [u64; 2]) -> i32;
    fn getrlimit(resource: i32, limit: *mut [u64; 2]) -> i32;
    fn geteuid() -> u32;
}

std::thread_local! {
    /// More static thread-local data than the thread's small stack leaves room for.
    static BIG: std::cell::Cell<[u8; 1 << 20]> = const { std::cell::Cell::new([0; 1 << 20]) };
}

/// A worker frees a 128 MiB burst while the main thread waits.
fn worker_frees(before: impl FnOnce() + Send + 'static, after: impl FnOnce() + Send + 'static) {
    let (done, wait) = mpsc::channel();
    thread::spawn(move || {
        before();
        drop(burst(128));
        after();
        done.send(()).unwrap();
    });
    wait.recv().unwrap();
}

fn child(mode: &str) {
    keep_a_thread();
    match mode {
        "worker" | "off" => worker_frees(|| {}, || {}),
        "maintain" => {
            Asmalloc::maintain();
            worker_frees(|| {}, || {});
        }
        "mainfrees" => {
            let (give, take) = mpsc::channel();
            thread::spawn(move || give.send(burst(128)).unwrap());
            drop(take.recv().unwrap());
        }
        "sigmask" => worker_frees(
            || {
                let mut set = [0u64; 16];
                set[0] = 1 << (10 - 1); // SIGUSR1
                // SAFETY: a valid set; a null old mask is allowed.
                unsafe { pthread_sigmask(0, &set, std::ptr::null_mut()) };
            },
            || {
                let none = [0u64; 16];
                // SAFETY: as above; SIG_SETMASK to the empty set.
                unsafe { pthread_sigmask(2, &none, std::ptr::null_mut()) };
                light_work(3);
            },
        ),
        "late" => {
            // The burst is freed before the program's second thread exists. The thread kept
            // alive above already exists, so this mode makes its own process's first frees
            // before that: see `late` in `main`.
            unreachable!()
        }
        "frag" => {
            let (done, wait) = mpsc::channel();
            thread::spawn(move || {
                let all = burst(384);
                let kept: Vec<Block> = all.into_iter().step_by(2).collect();
                done.send(()).unwrap();
                std::mem::forget(kept);
            });
            wait.recv().unwrap();
        }
        "tls" => {
            // Written and read through `black_box`, so that the data is not optimized away.
            let at = std::hint::black_box(12345);
            let byte = BIG.with(|big| {
                let cells = big.as_array_of_cells();
                cells[at].set(7);
                std::hint::black_box(cells)[at].get()
            });
            println!("tls_byte {byte}");
            worker_frees(|| {}, || {});
        }
        "failed" => {
            // RLIMIT_NPROC at the processes this user runs already refuses every new thread
            // (EAGAIN); the start fails, and the allocator carries on without the thread for
            // good, even once the limit is lifted.
            let mut old = [0u64; 2];
            // SAFETY: valid pointers to two-word limits.
            unsafe { getrlimit(6, &mut old) };
            let (done, wait) = mpsc::channel();
            let (go, start) = mpsc::channel::<()>();
            thread::spawn(move || {
                start.recv().unwrap();
                drop(burst(128));
                done.send(()).unwrap();
            });
            // SAFETY: as above.
            unsafe { setrlimit(6, &[1, old[1]]) };
            go.send(()).unwrap();
            wait.recv().unwrap();
            // SAFETY: as above.
            unsafe { setrlimit(6, &old) };
            light_work(3);
        }
        "single" => {
            // The kept thread above makes the process threaded; this mode is run as `single`
            // in `main` instead.
            unreachable!()
        }
        "steady" => {
            // Not a case: for the research note, the thread's wake-ups while a program works
            // steadily, light work on two threads for 20 s after a burst started the thread.
            worker_frees(|| {}, || {});
            let status = allocator_thread();
            let switches = || {
                status
                    .as_ref()
                    .and_then(|file| field(file, b"\nvoluntary_ctxt_switches:"))
            };
            let other = thread::spawn(|| light_work(22));
            thread::sleep(Duration::from_secs(2));
            let before = switches();
            light_work(20);
            let after = switches();
            other.join().unwrap();
            if let (Some(before), Some(after)) = (before, after) {
                println!("steady_wakes_per_10s {}", (after - before) / 2);
            }
        }
        // The parent's thread is awake at the fork, started by hand or by a worker's burst a
        // few seconds before: the child inherits `AWAKE` under the parent's key, which must not
        // hide the child's own crossings.
        "forkawake" => {
            Asmalloc::start_background().unwrap();
            drop(burst(1));
            fork_and_free();
            return;
        }
        "workerfork" => {
            worker_frees(|| {}, || {});
            thread::sleep(Duration::from_secs(5));
            // Slow paths in the parent, whose look clears the level bit the parent's thread
            // left: the child then inherits a plain `AWAKE`.
            light_work(1);
            fork_and_free();
            return;
        }
        "fork" => {
            worker_frees(|| {}, || {});
            silence().print("");
            // The parent's thread is parked by now; the child inherits the word, not the
            // thread, and its first thread's frees are not evidence of a thread of its own.
            // SAFETY: the child goes on as an ordinary process; the other threads are idle.
            let pid = unsafe { fork() };
            if pid == 0 {
                drop(burst(128));
                let first = silence();
                println!("child_first_thread {}", u64::from(first.thread));
                keep_a_thread();
                worker_frees(|| {}, || {});
                silence().print("child_");
                std::process::exit(0);
            }
            let mut status = 0;
            // SAFETY: a valid pointer for the status.
            unsafe { waitpid(pid, &mut status, 0) };
            println!("child_status {status}");
            return;
        }
        _ => panic!("unknown mode {mode}"),
    }
    silence().print("");
}

/// Forks; the child's second thread frees a 128 MiB burst, and the child reports with the
/// prefix `child_`, the parent its child's status.
fn fork_and_free() {
    // SAFETY: the child goes on as an ordinary process; the other threads are idle.
    let pid = unsafe { fork() };
    if pid == 0 {
        keep_a_thread();
        worker_frees(|| {}, || {});
        silence().print("child_");
        std::process::exit(0);
    }
    let mut status = 0;
    // SAFETY: a valid pointer for the status.
    unsafe { waitpid(pid, &mut status, 0) };
    println!("child_status {status}");
}

/// The child of a fork whose parent's thread was awake got its own thread and returned its
/// burst.
fn child_returned(report: &BTreeMap<String, u64>) -> Result<(), String> {
    expect(get(report, "child_status")? == 0, "the child failed", report)?;
    expect(get(report, "child_thread")? == 1, "no thread in the child", report)?;
    expect(get(report, "child_rss_mib")? < 72, "the child kept its burst", report)
}

/// The modes that must not have a second thread before their burst: run before anything else
/// in `main`.
fn early_child(mode: &str) -> bool {
    match mode {
        "late" => {
            drop(burst(128));
            keep_a_thread();
            thread::spawn(|| light_work(3)).join().unwrap();
        }
        "single" => drop(burst(128)),
        _ => return false,
    }
    silence().print("");
    true
}

/// What a case expects of a child's report.
struct Case {
    mode: &'static str,
    env: &'static [(&'static str, &'static str)],
    check: fn(&BTreeMap<String, u64>) -> Result<(), String>,
}

fn get(report: &BTreeMap<String, u64>, key: &str) -> Result<u64, String> {
    report.get(key).copied().ok_or(format!("no {key}"))
}

fn expect(ok: bool, what: &str, report: &BTreeMap<String, u64>) -> Result<(), String> {
    if ok {
        Ok(())
    } else {
        Err(format!("{what}: {report:?}"))
    }
}

/// The thread started, parked, made no wake-up once parked, and the burst went back to about
/// the floor (32 MiB retained, and the test's own memory).
fn returned(report: &BTreeMap<String, u64>) -> Result<(), String> {
    expect(get(report, "thread")? == 1, "no thread", report)?;
    get(report, "parked_after").map_err(|_| format!("did not park: {report:?}"))?;
    expect(get(report, "wakes")? == 0, "woke while parked", report)?;
    expect(get(report, "rss_mib")? < 72, "kept the burst", report)
}

fn no_thread(report: &BTreeMap<String, u64>) -> Result<(), String> {
    expect(get(report, "thread")? == 0, "a thread started", report)
}

const CASES: &[Case] = &[
    // A worker frees a burst: the thread starts, returns it, parks, and stays parked.
    Case {
        mode: "worker",
        env: &[],
        check: returned,
    },
    // Review finding 2: the main thread frees what a worker allocated. musl keeps no record of
    // threads created, and there the evidence is a thread other than the first: a limit.
    Case {
        mode: "mainfrees",
        env: &[],
        check: if cfg!(target_env = "musl") {
            no_thread
        } else {
            returned
        },
    },
    // Review finding 3: the freeing thread has a signal blocked at the crossing, not after.
    Case {
        mode: "sigmask",
        env: &[],
        check: returned,
    },
    // Review finding 3: the burst is freed before the second thread exists.
    Case {
        mode: "late",
        env: &[],
        check: returned,
    },
    // Review finding 1: a fragmented heap over the floor; the thread parks and stays parked.
    Case {
        mode: "frag",
        env: &[],
        check: |report| {
            expect(get(report, "thread")? == 1, "no thread", report)?;
            get(report, "parked_after").map_err(|_| format!("did not park: {report:?}"))?;
            expect(get(report, "wakes")? == 0, "woke while parked", report)
        },
    },
    // Review finding 4: static thread-local data the small stack cannot hold.
    Case {
        mode: "tls",
        env: &[],
        check: returned,
    },
    Case {
        mode: "off",
        env: &[("ASMALLOC_BACKGROUND", "0")],
        check: no_thread,
    },
    Case {
        mode: "maintain",
        env: &[],
        check: no_thread,
    },
    Case {
        mode: "single",
        env: &[],
        check: no_thread,
    },
    Case {
        mode: "failed",
        env: &[],
        check: |report| {
            if get(report, "root").unwrap_or(0) == 1 {
                return Ok(());
            }
            no_thread(report)
        },
    },
    // Fork children of a parent whose thread is awake (second review's finding).
    Case { mode: "forkawake", env: &[], check: child_returned },
    Case { mode: "workerfork", env: &[], check: child_returned },
    // A fork child of a parked parent: no thread for its first thread's frees, one once a
    // thread of its own frees, and the memory goes back.
    Case {
        mode: "fork",
        env: &[],
        check: |report| {
            returned(report)?;
            expect(
                get(report, "child_status")? == 0,
                "the child failed",
                report,
            )?;
            expect(
                get(report, "child_first_thread")? == 0,
                "the child's first thread got a thread",
                report,
            )?;
            expect(
                get(report, "child_thread")? == 1,
                "no thread in the child",
                report,
            )?;
            expect(
                get(report, "child_rss_mib")? < 72,
                "the child kept its burst",
                report,
            )
        },
    },
];

fn main() {
    if let Ok(mode) = std::env::var(MODE) {
        if !early_child(&mode) {
            if mode == "failed" {
                // SAFETY: takes nothing.
                println!("root {}", u64::from(unsafe { geteuid() } == 0));
            }
            child(&mode);
        }
        return;
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    let ignored = args
        .iter()
        .any(|a| a == "--ignored" || a == "--include-ignored");
    let filters: Vec<&String> = args.iter().filter(|a| !a.starts_with('-')).collect();
    if args.iter().any(|a| a == "--list") {
        for case in CASES {
            println!("{}: test", case.mode);
        }
        return;
    }
    let chosen: Vec<&Case> = CASES
        .iter()
        .filter(|case| filters.is_empty() || filters.iter().any(|f| case.mode.contains(f.as_str())))
        .collect();
    if !ignored {
        for case in &chosen {
            println!(
                "test {} ... ignored, slow: run with scripts/gate --all",
                case.mode
            );
        }
        println!(
            "\ntest result: ok. 0 passed; 0 failed; {} ignored; 0 measured; 0 filtered out",
            chosen.len()
        );
        return;
    }
    let exe = std::env::current_exe().unwrap();
    let running: Vec<_> = chosen
        .iter()
        .map(|case| {
            let mut command = Command::new(&exe);
            command
                .env(MODE, case.mode)
                .env_remove("ASMALLOC_BACKGROUND");
            for (key, value) in case.env {
                command.env(key, value);
            }
            command
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped());
            (case, command.spawn())
        })
        .collect();
    let mut failed = 0;
    for (case, child) in running {
        let output = child.and_then(|child| child.wait_with_output());
        let result = output.map_err(|e| e.to_string()).and_then(|output| {
            let text = String::from_utf8_lossy(&output.stdout).into_owned();
            let report: BTreeMap<String, u64> = text
                .lines()
                .filter_map(|line| {
                    let (key, value) = line.split_once(' ')?;
                    Some((key.to_owned(), value.parse().ok()?))
                })
                .collect();
            if !output.status.success() {
                return Err(format!(
                    "{:?}: {text}{}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
            (case.check)(&report)
        });
        match result {
            Ok(()) => println!("test {} ... ok", case.mode),
            Err(why) => {
                println!("test {} ... FAILED\n    {why}", case.mode);
                failed += 1;
            }
        }
    }
    println!(
        "\ntest result: {}. {} passed; {failed} failed; 0 ignored; 0 measured; 0 filtered out",
        if failed == 0 { "ok" } else { "FAILED" },
        chosen.len() - failed
    );
    if failed != 0 {
        std::process::exit(101);
    }
}
