//! ruff checking a project with the project's own configuration, from the project's root, as
//! a CI job runs `ruff check`: no cache, every file parsed and checked, diagnostics rendered
//! in the default format.
//!
//!   ruff check root=DIR [expect=N]
//!
//! `ruff::run` is all that ruff's own `main` does after reading its arguments. Ruff's thread
//! pool is rayon's global pool, one thread per CPU the process may run on.
//!
//! The diagnostics go to a memory file instead of stdout, so that the runner does not read
//! them; they are counted, and with `expect=N` a run that finds a different number is
//! invalid.

use bench_app::{Driver, fail};
use clap::Parser;
use std::io::{Read, Seek};
use std::os::fd::FromRawFd;

fn main() {
    let mut d = Driver::start("ruff");
    if d.job != "check" {
        fail("usage: ruff check root=DIR [expect=N]");
    }
    let root = d.args.str("root", "");
    let expect = d.args.str("expect", "");
    std::env::set_current_dir(&root).unwrap_or_else(|e| fail(&format!("root={root}: {e}")));

    let mut out = memfd();
    let out_path = format!("/proc/self/fd/{}", std::os::fd::AsRawFd::as_raw_fd(&out));
    let argv = ["ruff", "check", "--no-cache", "--output-file", out_path.as_str(), "."];
    let status = d.measure("check", || ruff::run(ruff::args::Args::parse_from(argv)));
    let status = status.unwrap_or_else(|e| fail(&format!("ruff failed: {e:#}")));

    let mut text = String::new();
    out.rewind().and_then(|()| out.read_to_string(&mut text)).unwrap_or_else(|e| fail(&format!("output: {e}")));
    let diagnostics = count_diagnostics(&text);
    d.report()
        .metric("diagnostics", diagnostics)
        .metric("output_bytes", text.len())
        .metric("found_problems", !matches!(status, ruff::ExitStatus::Success))
        .metric("threads", std::thread::available_parallelism().map_or(0, |n| n.get()));
    if !expect.is_empty() && expect != diagnostics.to_string() {
        fail(&format!("expected {expect} diagnostics, ruff reported {diagnostics}:\n{}", tail(&text)));
    }
    d.finish();
}

/// The count on ruff's summary line, "Found N error(s)." (0 for "All checks passed!").
fn count_diagnostics(text: &str) -> u64 {
    text.lines().rev().find_map(|l| l.strip_prefix("Found ")?.split_whitespace().next()?.parse().ok()).unwrap_or(0)
}

fn tail(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(20)..].join("\n")
}

fn memfd() -> std::fs::File {
    // SAFETY: a plain system call with a NUL-terminated name; the descriptor it returns is
    // new and owned by the File.
    let fd = unsafe { libc::memfd_create(c"ruff-output".as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        fail(&format!("memfd_create: {}", std::io::Error::last_os_error()));
    }
    // SAFETY: as above.
    unsafe { std::fs::File::from_raw_fd(fd) }
}
