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

use std::string::String;
use std::time::{Duration, Instant};

/// Names, in the fresh process, the one test it was started for.
const CHILD: &str = "ASMALLOC_TEST_CHILD";
const DEADLINE: Duration = Duration::from_secs(120);

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
    let started = Instant::now();
    while child.try_wait().unwrap().is_none() {
        if started.elapsed() > DEADLINE {
            child.kill().unwrap();
            panic!("{path} hung: {:?}", child.wait_with_output().unwrap());
        }
        std::thread::sleep(Duration::from_millis(20));
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
