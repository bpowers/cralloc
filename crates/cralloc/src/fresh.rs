//! Running a test of the process's own heap in a process of its own, for a test that must see
//! the heap as only it left it: libtest runs the other tests of this binary on the same heap at
//! the same time, and they may take the slots or the counts the test looks at.
//!
//! A test calls [`run`] first and goes on only in the fresh process:
//!
//! ```ignore
//! if !fresh::is_child("c::tests::name") {
//!     return fresh::run("c::tests::name");
//! }
//! ```

use crate::front::{Real, Routines};
use std::string::String;
use std::time::Duration;
use std::vec::Vec;

unsafe extern "C" {
    fn sched_getaffinity(pid: i32, len: usize, mask: *mut u64) -> i32;
    fn sched_setaffinity(pid: i32, len: usize, mask: *const u64) -> i32;
}

/// The CPU numbers the sets [`allowed_cpus`] and [`pin`] use hold, up to this.
const CPUS: usize = 1024;

/// Names, in the fresh process, the one test it was started for.
const CHILD: &str = "CRALLOC_TEST_CHILD";
/// How often [`run`] looks at the fresh process, and how many looks it waits for it. Counted in
/// looks and not read off a clock, since a measurement freezes heavy work, this process with it, for
/// minutes at a time, and the fresh process would be found to have hung when it thawed.
const LOOK: Duration = Duration::from_millis(20);
const LOOKS: u32 = 6000;

/// Runs the test at `path` (its path in this binary, such as `c::tests::name`) again in a fresh
/// process, in which [`is_child`] holds and it does its work, and fails if that process fails,
/// runs no test, or outlives two minutes. A fresh process, not a fork: a child forked from the
/// test harness's threads may inherit a lock of std's held by one of them, and the first thread
/// it starts then waits for it for ever.
pub(crate) fn run(path: &str) {
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", path, "--nocapture", "--include-ignored"])
        .env(CHILD, path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut looks = 0;
    while child.try_wait().unwrap().is_none() {
        looks += 1;
        if looks > LOOKS {
            child.kill().unwrap();
            panic!("{path} hung: {:?}", child.wait_with_output().unwrap());
        }
        std::thread::sleep(LOOK);
    }
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{path} failed: {output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("1 passed"),
        "{path} did not run in the child: {stdout}"
    );
}

/// Whether this process is the fresh one [`run`] started for the test at `path`.
pub(crate) fn is_child(path: &str) -> bool {
    std::env::var_os(CHILD).is_some_and(|v| v == path)
}

/// The CPUs this thread may run on.
pub(crate) fn allowed_cpus() -> Vec<usize> {
    let mut mask = [0u64; CPUS / 64];
    // SAFETY: the call writes at most `len` bytes of `mask`, a live local of that size.
    let done = unsafe { sched_getaffinity(0, size_of_val(&mask), mask.as_mut_ptr()) };
    assert_eq!(done, 0);
    (0..CPUS)
        .filter(|cpu| mask[cpu / 64] & (1 << (cpu % 64)) != 0)
        .collect()
}

/// Moves this thread to `cpu`, and shows that the routines see it there.
pub(crate) fn pin(cpu: usize) {
    let mut mask = [0u64; CPUS / 64];
    mask[cpu / 64] = 1 << (cpu % 64);
    // SAFETY: the call reads `len` bytes of `mask`, a live local of that size, and changes only
    // where this thread runs.
    let done = unsafe { sched_setaffinity(0, size_of_val(&mask), mask.as_ptr()) };
    assert_eq!(done, 0, "moving to CPU {cpu}");
    assert_eq!(Real.cpu().map(|cpu| cpu.index()), Some(cpu));
}
