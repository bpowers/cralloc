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
contains it), `--allocators a,b`, `--rounds N`, `--out FILE`, `--seed N`, `--mem-cap SIZE`
(below), and the quiet-machine gate below. Builds run on `BENCH_BUILD_CPUS` (default
`4-7,20-23`).

The baseline libraries and the raw results live in the main checkout, shared by every
worktree: `bench/baselines/out` and `bench/results` there (the parent of
`git rev-parse --git-common-dir`). `BENCH_BASELINES` and `BENCH_RESULTS` override them.
Workloads are built and run from the checkout you are in. `build.sh` holds
`out/.lock` exclusively and `benchrun build` and `run` hold it shared, so the libraries
cannot be rebuilt under a session.

## Layout

| Path | What |
|---|---|
| `allocators.toml` | The allocators compared. Adding one is one more entry. |
| `baselines/` | `build.sh` rebuilds glibc's peers into the shared `baselines/out/` (gitignored) and records versions, commits and flags in `out/manifest/`. `probe.c` is a preloaded library that reports which allocator, mode and library a process ran with. |
| `alloc/` | `bench-alloc`: the `#[global_allocator]`, chosen by cargo feature: the allocator's usual Rust crate itself (jemalloc), or the library called the way that crate calls it. |
| `harness/` | `bench-harness`: pinning, in-process counters over a measured phase, seeded size distributions, the result line. |
| `workloads/` | The `workload` binary: cross-thread, work-stealing and single-threaded workloads. |
| `rpc/` | connect-rust's echo and log-ingest benchmarks as `rpc-server` (allocator under test) and `rpc-client` (fixed). See `rpc/README.md`. |
| `runner/` | `benchrun`: builds, runs and reports. Jobs are defined in `runner/src/suite.rs`. |

## Adding an allocator

1. For native Rust runs, add a cargo feature to `alloc/Cargo.toml`; a module in `alloc/src/`
   with `NAME`, `entry()` (a function of the allocator's, to show which object its code is
   in) and `version()`, plus a `GlobalAlloc` unless the allocator's crate provides one; the
   four lines in `alloc/src/lib.rs` that select the module and set `#[global_allocator]`;
   and a branch in `alloc/build.rs` only if it links a library from `baselines/out`. For
   asmalloc the feature is a path dependency, `asmalloc = ["dep:asmalloc"]` with
   `asmalloc = { path = "../../crates/asmalloc", optional = true }`, as `je` is for
   `tikv-jemallocator`.
2. Add an entry to `allocators.toml` with that `feature`, the `preload` library if it has
   one, and what the process must report about itself (`expect`). A preload library must
   also be recognisable by `baselines/probe.c`, which tells allocators apart by the symbols
   they export.
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
  multiplexed is invalid: the counts are not scaled, so it would look fast.
- **Memory.** Each run records its cgroup's `memory.peak`, which ranks memory: everything charged to it, which
  excludes file pages already cached by an earlier run (the binary, shared libraries) and
  includes page tables. The runner samples the resident set and `VmHWM` every 50 ms from
  `/proc/<pid>/statm` and `status`, which read counters, and `/proc/<pid>/smaps_rollup`
  (proportional and huge-page-backed memory) once a second, because it walks the page tables
  under the process's `mmap_lock`. The peak resident set comes from the process itself at
  exit (the result line, or the preload probe), because `ru_maxrss` of a forked child
  includes the parent's memory at the fork.
- **Memory cap.** `benchrun run` always runs in a systemd scope of its own, capped at
  `--mem-cap` plus 2 GiB and never above 16 GiB, with swap off; started anywhere else, it
  re-executes itself inside one. Each measured process runs in a fresh cgroup of that scope
  capped at `--mem-cap` (default 4 GiB, about three times the largest peak the full suite
  needs), and each RPC driver in one capped at 1 GiB; the process is moved there before it
  execs. A run killed at its cap, or one that reached the cap and was slowed by reclaim, is
  recorded as invalid with that reason, and the session goes on. Every process is charged to
  some memory cgroup anyway, so the cap adds no work to allocation; an interleaved A/B is in
  `docs/research/baselines.md` section 11.
- **Checks.** Each run must report the allocator, and for tcmalloc the mode, it was meant to
  run with, and where the allocator's code was: linked into the executable, or which shared
  library. A preload run must report the library it preloaded. Native runs report from
  inside, preloaded ones through `baselines/probe.c`. Runs that disagree, fail, or time out
  are recorded as invalid and left out of reports.
- **Control.** `mimalloc-ctl` is mimalloc under a second name and from its own build
  directory, run as if it were another allocator. The report compares it with mimalloc on
  every job, pairing runs from the same round, with a 95% bootstrap interval of the median
  ratio. A difference beyond the chance rate means the setup, not the allocator, moved the
  result. It does not test effects that differ by allocator entry, such as the length of the
  environment (`GLIBC_TUNABLES`, `LD_PRELOAD`), which shifts the initial stack.
- **Quiet gate.** Before each job the runner measures, over one second, how busy the
  measurement CPUs, their siblings and the other CPUs are; other processes are not confined,
  so they may land on measurement CPUs too. It waits (re-checking every five seconds) while
  anything runs on the measurement CPUs or siblings, or more than `--max-busy` CPUs' worth
  (default 1.0) runs elsewhere, for at most `--gate-wait` seconds (default 900), then runs
  anyway and records what it saw. Every run records the load on the siblings, on the other
  CPUs, and the work on its own CPUs that was not its own; `report --quiet-only` leaves out
  runs where the last two were high.
- **Ranking.** Each job has one primary metric (named in the report with its unit; lower is
  better except rates). The report gives medians, half the interquartile range as a
  percentage, the fastest and the smallest (peak resident set) allocator, the runner-up
  margin, and "tie" when the 95% bootstrap interval of the margin, taken over per-round
  ratios, includes zero.

- **Consumers.** Cross-thread jobs whose consumers only free come in two forms, named in the
  job id: `all-alloc`, where every thread allocates once before the work, as a service's
  threads do (the primary form), and `free-only`, where consumers never allocate, as a
  dedicated drop thread. jemalloc gives a free-only thread no thread cache, which makes it two
  to three times slower there.

Choices that can favour one allocator, not yet tested either way; read the numbers with them:

- Transparent huge pages are as the machine has them (`madvise` mode on the reference
  machine). Only mimalloc asks for huge pages, which decides the build-and-drop jobs; a
  session with THP `always` would show the other side.
- Single-threaded build jobs measure every repetition from a cold process, so allocators that
  keep freed memory gain over those that return it; jemalloc's time-based purging never fires
  in runs this short.
- Wall-time metrics of the C programs include process start-up, which grows with the size of
  the preloaded library; a few percent on the shortest (cache-scratch 8).
- The sampler reads `smaps_rollup` once a second, which holds the process's `mmap_lock`
  during a page-table walk and slightly delays allocators that `munmap` or `mremap` often
  (`--rollup-ms` changes the interval).
- CPUs 4-7, where builds run, share an L3 with measurement CPUs 0-3; memory-bound jobs have
  drifted by up to three times on a busy machine without any load on their own CPU.

Output is JSON lines: a `session` record (machine, settings, allocators, the baseline
manifests) followed by one `run` record per process.
