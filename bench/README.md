# Benchmarks

Workload benchmarks that compare allocators on whole programs: cross-thread patterns, a
work-stealing pool, an RPC server, single-threaded allocation-heavy shapes, and selected
mimalloc-bench programs. Results and what they mean are in `docs/research/baselines.md`.
The fast-path microbenchmarks are in `roofline/` and `docs/research/zen5-roofline.md`.

Everything here is x86-64 Linux only and needs `perf_event_open` for hardware counters
(`/proc/sys/kernel/perf_event_paranoid` of 2 or lower for user-space counts; the runner also
counts kernel cycles, which needs 1 or lower).

## Running

```
bench/baselines/build.sh                         # every baseline, from scratch (minutes)
cd bench
cargo build --release -p bench-runner
./target/release/benchrun build                  # workloads, once per allocator
./target/release/benchrun run --suite quick      # a cross-section, about two minutes
./target/release/benchrun run --suite full       # everything, ten rounds, about two hours
./target/release/benchrun report results/full-*.jsonl --detail
```

`benchrun list --suite full` prints the jobs. `run` takes `--only TEXT` (jobs whose id
contains it), `--allocators a,b`, `--rounds N`, `--out FILE`, `--seed N`, and the quiet-machine
gate below. Builds run on `BENCH_BUILD_CPUS` (default `4-7,20-23`).

## Layout

| Path | What |
|---|---|
| `allocators.toml` | The allocators compared. Adding one is one more entry. |
| `baselines/` | `build.sh` rebuilds glibc's peers into `baselines/out/` (gitignored) and records versions, commits and flags in `out/manifest/`. `probe.c` is a preloaded library that reports which allocator and mode a process ran with. |
| `alloc/` | `bench-alloc`: the `#[global_allocator]`, chosen by cargo feature, called the way each allocator's usual Rust crate calls it. |
| `harness/` | `bench-harness`: pinning, in-process counters over a measured phase, seeded size distributions, the result line. |
| `workloads/` | The `workload` binary: cross-thread, work-stealing and single-threaded workloads. |
| `rpc/` | connect-rust's echo and log-ingest benchmarks as `rpc-server` (allocator under test) and `rpc-client` (fixed). See `rpc/README.md`. |
| `runner/` | `benchrun`: builds, runs and reports. Jobs are defined in `runner/src/suite.rs`. |

## Adding an allocator

1. For native Rust runs, add a cargo feature to `alloc/Cargo.toml`, a module in `alloc/src/`
   with its `GlobalAlloc` and a `#[global_allocator]` line in `alloc/src/lib.rs`, and a
   branch in `alloc/build.rs` if it links a library. For asmalloc this is a path dependency on
   `crates/asmalloc` behind the feature.
2. Add an entry to `allocators.toml` with that `feature`, the `preload` library if it has
   one, and what the process must report about itself (`expect`).
3. `benchrun build`.

## Method

- **One process per measurement.** Each run is a fresh process; a round runs every job
  under every allocator, with the allocators in a new random order for every job. A full
  session is ten rounds.
- **Pinning.** Jobs use CPUs 0-3 (one L3 domain) and 8-11 (the other), one thread per
  physical core, with the SMT siblings 16-19 and 24-27 left idle. Each process is confined to
  its CPU list before `execve`, and workloads pin thread `i` to the `i`-th CPU of
  `BENCH_CPUS`. Single-threaded jobs run on CPU 2. The runner itself runs on CPU 15.
- **Counters.** A workload counts cycles (user and kernel), user cycles, instructions,
  page faults and CPU time over its measured phase only, summed over all its threads
  (`perf_event_open` with `inherit`). The runner also counts the whole process from its
  first instruction (counters attached between `fork` and `execve`, as `perf stat` does),
  and for a server, over the window the driver marks. A run whose hardware counters were
  multiplexed is flagged.
- **Memory.** The runner samples `/proc/<pid>/smaps_rollup` every 50 ms (resident,
  proportional, anonymous and huge-page-backed memory) and `VmHWM`. The peak resident set
  comes from the process itself at exit (the result line, or the preload probe), because
  `ru_maxrss` of a forked child includes the parent's memory at the fork.
- **Checks.** Each run must report the allocator, and for tcmalloc the mode, it was meant to
  run with; native runs report from inside, preloaded ones through `baselines/probe.c`.
  Runs that disagree, fail, or time out are recorded as invalid and left out of reports.
- **Control.** `mimalloc-ctl` is mimalloc under a second name, run as if it were another
  allocator. The report compares it with mimalloc on every job; a difference whose 95%
  bootstrap interval excludes zero means the setup, not the allocator, moved the result.
- **Quiet gate.** Before each job the runner measures, over one second, how busy the CPUs
  outside the measurement set and its siblings are. It waits (re-checking every five
  seconds) while more than `--max-busy` CPUs' worth (default 1.0) is running, or the
  siblings are busy, for at most `--gate-wait` seconds (default 900), then runs anyway and
  records what it saw. Every run records the load on the siblings and on the other CPUs
  while it ran.
- **Ranking.** Each job has one primary metric (named in the report with its unit; lower is
  better except rates). The report gives medians, half the interquartile range as a
  percentage, the fastest and the smallest (peak resident set) allocator, the runner-up
  margin, and "tie" when the bootstrap interval of the margin includes zero.

Output is JSON lines: a `session` record (machine, settings, allocators, the baseline
manifests) followed by one `run` record per process.
