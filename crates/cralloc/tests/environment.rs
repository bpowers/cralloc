//! `CRALLOC_BACKGROUND` end to end, through the Rust interface: each case runs this test
//! binary again as a child with the variable set or not, and the child reports whether the
//! background thread started after a worker freed a burst over the floor. The switch is read
//! from the environment the process was started with, so a `set_var` in the child changes
//! nothing.

use cralloc::Cralloc;
use cralloc::background::choice;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

#[global_allocator]
static ALLOCATOR: Cralloc = Cralloc;

const MODE: &str = "CRALLOC_ENVIRONMENT_CHILD";
const VARIABLE: &str = "CRALLOC_BACKGROUND";

fn has_allocator_thread() -> bool {
    std::fs::read_dir("/proc/self/task").is_ok_and(|tasks| {
        tasks.filter_map(Result::ok).any(|task| {
            std::fs::read_to_string(task.path().join("comm")).is_ok_and(|n| n.trim() == "cralloc")
        })
    })
}

/// The bytes of a burst well over the floor: 128 MiB, or twice the floor where that is more. The
/// floor is 16 huge pages, and a huge page is `page^2 / 8`, 32 MiB on a kernel with 16 KiB pages.
fn burst_bytes() -> usize {
    unsafe extern "C" {
        fn sysconf(name: i32) -> i64;
    }
    const SC_PAGESIZE: i32 = 30;
    // SAFETY: a query of a constant of the system.
    let page = unsafe { sysconf(SC_PAGESIZE) } as usize;
    (128usize << 20).max(32 * (page * page / 8))
}

/// A block of a size with a small class.
type Block = Box<[u8; 1024]>;

/// A burst of `burst_bytes` in blocks.
fn burst() -> Vec<Block> {
    (0..burst_bytes() / 1024)
        .map(|i| Box::new([i as u8; 1024]))
        .collect()
}

/// In the child: optionally sets the variable, keeps a second thread, frees a burst on a worker
/// (over the floor), and reports what the switch says and whether the thread
/// appeared within a second. It appears at once when it starts at all: the look that starts
/// it is in the burst's own slow paths.
#[test]
fn child() {
    let Ok(mode) = std::env::var(MODE) else {
        return;
    };
    let before = choice();
    match mode.as_str() {
        "plain" => {}
        // SAFETY: no other thread reads the environment yet.
        "set-on" => unsafe { std::env::set_var(VARIABLE, "1") },
        // SAFETY: as above.
        "set-off" => unsafe { std::env::set_var(VARIABLE, "0") },
        _ => panic!("unknown mode {mode}"),
    }
    let (keep, kept) = mpsc::channel::<()>();
    let other = thread::spawn(move || kept.recv());
    thread::spawn(|| drop(burst())).join().unwrap();
    let started = Instant::now();
    let mut found = has_allocator_thread();
    while !found && started.elapsed() < Duration::from_secs(1) {
        thread::sleep(Duration::from_millis(50));
        found = has_allocator_thread();
    }
    println!(
        "before {before:?} after {:?} thread {}",
        choice(),
        u8::from(found)
    );
    drop(keep);
    other.join().unwrap().ok();
}

/// Starts the child for `mode` with the variable set to `value`, or unset.
fn spawn(value: Option<&str>, mode: &str) -> std::process::Child {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "child", "--nocapture", "--test-threads=1"])
        .env(MODE, mode)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    match value {
        Some(value) => command.env(VARIABLE, value),
        None => command.env_remove(VARIABLE),
    };
    command.spawn().unwrap()
}

/// The child's report.
fn report(child: std::process::Child) -> String {
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    // The harness prints the test's name on the same line.
    stdout
        .lines()
        .find_map(|line| line.find("before ").map(|at| line[at..].to_owned()))
        .unwrap_or_else(|| panic!("no report: {stdout}"))
}

/// The cases run at once, each in a process of its own, if their bursts fit in the test's cap
/// together.
#[test]
fn the_switch_is_the_environment_the_process_started_with() {
    let cases = [
        (Some("0"), "plain", "before Off after Off thread 0"),
        (Some(""), "plain", "before Off after Off thread 0"),
        (Some("1"), "plain", "before On after On thread 1"),
        (None, "plain", "before Default after Default thread 1"),
        // A `set_var` after start changes nothing.
        (Some("0"), "set-on", "before Off after Off thread 0"),
        (None, "set-off", "before Default after Default thread 1"),
    ];
    if burst_bytes() > 128 << 20 {
        for (value, mode, expected) in cases {
            let child = spawn(value, mode);
            assert_eq!(report(child), expected, "{VARIABLE}={value:?}, {mode}");
        }
        return;
    }
    let children: Vec<_> = cases
        .iter()
        .map(|&(value, mode, _)| spawn(value, mode))
        .collect();
    for (child, (value, mode, expected)) in children.into_iter().zip(cases) {
        assert_eq!(report(child), expected, "{VARIABLE}={value:?}, {mode}");
    }
}
