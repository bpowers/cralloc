//! Allocation workloads, one process per measurement. The allocator is whatever
//! `bench-alloc` was built with.
//!
//!   workload <name> [key=value ...]
//!
//! Every workload prints one `BENCH_RESULT` line (see bench-harness). Threads are pinned to
//! the CPUs in `BENCH_CPUS`, in order. Sizes and choices come from seeded generators, so each
//! allocator serves the same sequence of requests.
mod obj;
mod single;
mod steal;
mod xthread;

use bench_harness::Args;

fn main() {
    let (name, args) = Args::from_env();
    match name.as_str() {
        "pipeline" => xthread::pipeline(&args),
        "exchange" => xthread::exchange(&args),
        "mpmc" => xthread::mpmc(&args),
        "forkjoin" => steal::forkjoin(&args),
        "spawn" => steal::spawn(&args),
        "churn" => single::churn(&args),
        "map" => single::map(&args),
        "tree" => single::tree(&args),
        "vecgrow" => single::vecgrow(&args),
        "json" => single::json(&args),
        _ => {
            eprintln!(
                "usage: workload <pipeline|exchange|mpmc|forkjoin|spawn|churn|map|tree|vecgrow|json> [key=value ...]"
            );
            std::process::exit(2);
        }
    }
}
