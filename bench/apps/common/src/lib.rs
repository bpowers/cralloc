//! The part of a program's driver that every program shares.
//!
//! A driver is a small binary that runs a real program's code through its library crates, on
//! a pinned input, with the allocator under test as the global allocator (whichever
//! bench-alloc feature the build chose). It is invoked as
//!
//!   <program> <job> [key=value ...]
//!
//! and prints one `BENCH_RESULT` line at the end, as the suite's workloads do: the allocator
//! it ran with, the counters and wall time of each measured phase, the program's own
//! metrics, and the primary metric: the cycles of every thread (user and kernel) over the
//! measured phases, in billions. Not wall time: a parallel program's wall time depends on how
//! its threads were scheduled, which moves it by more than the allocator does (DataFusion on
//! two L3 domains ran in two modes 40% apart; docs/research/workloads.md, "Batch programs").
//! The wall time is recorded as the metric `s`.
//!
//! A driver checks that the program did the work it was meant to (the number of
//! diagnostics, the rows each query returned, the script's result). A run whose output is
//! not the expected one exits with status 1, which the runner records as invalid, so a
//! fast allocator cannot win by breaking the program.

use bench_harness::Counters;
use bench_harness::report::Measure;
use std::time::Duration;

pub use bench_harness::{Args, Better, Report};

pub struct Driver {
    pub args: Args,
    pub job: String,
    counters: Counters,
    report: Report,
    measured: Duration,
    cycles: u64,
}

impl Driver {
    /// Starts the allocator's own setup and opens the counters. Call it first in `main`,
    /// before the program starts any thread: the counters count only threads created after
    /// they are opened.
    pub fn start(program: &str) -> Driver {
        bench_alloc::init();
        let (job, args) = Args::from_env();
        let report = Report::new(&format!("{program} {job}"), &args);
        Driver { args, job, counters: Counters::region(), report, measured: Duration::ZERO, cycles: 0 }
    }

    /// Runs `f` as a measured phase named `phase`. The primary metric is the sum over every
    /// phase, so whatever is not measured (reading arguments, checking output) stays out of it.
    pub fn measure<T>(&mut self, phase: &str, f: impl FnOnce() -> T) -> T {
        let m = Measure::start(&self.counters);
        let out = f();
        let p = m.stop();
        self.measured += Duration::from_nanos(p.wall_ns);
        self.cycles += p.counters.get("cycles").unwrap_or(0);
        self.report.phase(phase, &p);
        out
    }

    /// For the program's own metrics (`metric`) and parameters (`param`).
    pub fn report(&mut self) -> &mut Report {
        &mut self.report
    }

    /// Prints the result line. The process should exit soon after: what it does at exit is
    /// in the runner's process counters but not in the primary metric.
    pub fn finish(mut self) {
        self.report.metric("s", self.measured.as_secs_f64());
        self.report.primary("Gcycles", self.cycles as f64 / 1e9, Better::Lower);
        self.report.emit();
    }
}

impl Driver {
    /// Prints the result line of a server, which the runner measures from outside (counters
    /// over the load generator's segments, latency from the generator), so it names no
    /// primary metric: only the allocator the process ran with and the program's metrics.
    pub fn finish_server(mut self) {
        self.report.emit();
    }
}

/// Stops the run as invalid: the program did not produce what it should have.
pub fn fail(msg: &str) -> ! {
    eprintln!("bench-app: {msg}");
    std::process::exit(1)
}
