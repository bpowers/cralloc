# Benchmarks

Workload benchmarks that compare allocators on whole programs: cross-thread patterns, a
work-stealing pool, an RPC server, single-threaded allocation-heavy shapes, and selected
mimalloc-bench programs. Results and what they mean are in `docs/research/standings.md`.
The fast-path microbenchmarks are in `roofline/` and `docs/research/roofline.md`. Real
Rust programs are the `servers` suite (connect-rust, axum and actix-web services under an
open load) and the `apps` suite (ruff, DataFusion, boa), with their own build step and
inputs: see `apps/README.md`.

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
./target/release/benchrun run --suite backend --rounds 3   # after any change to the back end
./target/release/benchrun run --suite full       # everything, ten rounds, about two hours
./target/release/benchrun report results/full-*.jsonl --detail
```

The `backend` suite is seven small jobs that each exposed a cost of cralloc's back end
(`docs/research/workloads.md`, "The smoke set for the back end"); run it, against the
incumbents, on every change to the back end. `benchrun list --suite full` prints the jobs.
`run` takes `--only TEXT` (jobs whose id
contains it), `--allocators a,b`, `--rounds N`, `--out FILE`, `--seed N`, `--mem-cap SIZE`
(below), `--no-warm` (skip the clock warm-up below, only to measure what it changes), and
the quiet-machine gate below. Builds run on `BENCH_BUILD_CPUS` (default `12-15,28-31`, the cores `scripts/heavy` uses).
`report` takes `--steady-only` to leave out runs whose clock was off.

`run` takes no lock itself: start it under `scripts/measure --both-l3`, since the suite uses
both L3s.

`build` and `run` refuse an cralloc build whose routines load the rseq offset at a slab hot
word's page offset (`scripts/rseq-offset-check`): every allocation of that class could wait for
the previous one, and the session would measure the link (`docs/research/roofline.md`,
"A load at the page offset of a store in flight can stall the whole loop"). `run` records
the offsets in its session record.

`--suite` also takes several suites joined by commas, `--only` several texts joined by
commas, and `--group NAME` keeps one group's jobs (`benchrun list` prints the groups).

**When a job moves after a change that should not have touched it**, check whether it is
measuring where its code landed before looking for a cause in the allocator:

```
./target/release/benchrun layout --suite backend --only "vecgrow 1x4Mi"   # cralloc, five rounds
./target/release/benchrun layout --suite full --group single-thread --allocators cralloc,jemalloc
```

`layout` builds the workloads again in several ways that place the same code at other
addresses (`--layouts`, default `default,loops64,shuffle1,shuffle2`: the normal build; every
loop aligned to 64 bytes; the linker's function order shuffled with seed 1, and with seed 2),
runs the jobs under each, interleaved, as `run` interleaves allocators, and prints each
layout's median against the default, the spread (largest median over smallest), the noise
(the largest half-IQR of one layout's runs) and the spread of the process's instructions. It
takes the shared measurement lock (`/tmp/cralloc-measure.lock`) itself, for the
measurement only, so do not wrap it in `flock`. `--no-build` reuses the variants built last
time. A spread of more than about 3% beyond the noise, with instructions level, means the
job's time depends on code placement; `perf annotate` of plain cycles samples
(`perf record -e cycles:u`, not an event group) names the loop. `loops64` executes a little
padding, so its instructions are up to 1 to 3% higher in loop-heavy jobs; the shuffles retire
the same instructions.

The variants move the allocator's code as well as the workload's, so a job that moves can be
sensitive in either; the profile says which. They are for this check only: the suite measures
the normal build, which is what ships.

**Comparing allocators on a placement-sensitive job.** `--layouts pads` builds five variants,
`pad00` to `pad64`, in which a padding function of N bytes in bench-alloc moves all the code
the linker places after it, the allocator's and the program's, by exactly N bytes and changes
nothing else (`padN` takes any multiple of 16, written with two digits or more). With several
allocators the summary also prints each one's mean and range over the layouts, and each
against the first by the first layout's median (what single builds compare) and by the mean
over the layouts. Run one round a session and combine the sessions with
`standings/padsweep.py`, or with `standings/servers.py`, which also says whether each difference
is resolved or inside the range placement alone covers. The server jobs of `rpc-server` move by several percent this way
(`docs/research/measuring.md`, "One number from each of two builds measures where the code
landed").

```
./target/release/benchrun layout --suite servers --only "=connect log grpc h2" \
  --allocators cralloc,jemalloc,tcmalloc-percpu --layouts pads --rounds 1
```

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
| `backend/` | `backbench`: cralloc's back-end launder and fill on their own, per class and pointer order. |
| `workloads/` | The `workload` binary: cross-thread, work-stealing and single-threaded workloads. |
| `rpc/` | connect-rust's echo and log-ingest benchmarks as `rpc-server` (allocator under test) and `rpc-client` (fixed). See `rpc/README.md`. |
| `apps/` | Real Rust programs, each a workspace of its own with a driver; the `apps` and `servers` suites, built by `benchrun build-apps`. See `apps/README.md`. |
| `loadgen/` | The servers' open-loop load generator: latency from when each request was due. `benchrun build-apps` builds it. |
| `runner/` | `benchrun`: builds, runs and reports. Jobs are defined in `runner/src/suite.rs`. |
| `roofline/` | Fast-path microbenchmarks, each a cargo project of its own. See `roofline/README.md`. |
| `standings/` | What a standings session runs and reports with: session and queue scripts, placement-pad builds, summarizers. See `standings/README.md`. |
| `tools/` | Small standalone helpers, each explained in its header: `acct.py` (every resident page by structure), `thp-off.py`, `largesum.py` and `doublesum.py` (tables of `servers-large` and `servers-doubling` runs). |
| `probes/` | Standalone programs that each check one assumption about the kernel, glibc or Rust. See `probes/README.md`. |

## Adding an allocator

1. For native Rust runs, add a cargo feature to `alloc/Cargo.toml`; a module in `alloc/src/`
   with `NAME`, `entry()` (a function of the allocator's, to show which object its code is
   in) and `version()`, plus a `GlobalAlloc` unless the allocator's crate provides one; the
   four lines in `alloc/src/lib.rs` that select the module and set `#[global_allocator]`;
   and a branch in `alloc/build.rs` only if it links a library from `baselines/out`. For
   cralloc the feature is a path dependency,
   `cralloc = ["dep:cralloc", "cralloc/executable"]` with
   `cralloc = { path = "../../crates/cralloc", optional = true }`, as `je` is for
   `tikv-jemallocator`.
2. Add an entry to `allocators.toml` with that `feature`, the `preload` library if it has
   one, and what the process must report about itself (`expect`). A preload library must
   also be recognisable by `baselines/probe.c`, which tells allocators apart by the symbols
   they export.
3. Forward the feature in the `[features]` of every `apps/<program>/Cargo.toml`, and pin the
   allocator's crates in each program's lock file to the versions in `Cargo.lock`
   (`benchrun build-apps` checks both).
4. `benchrun build`, and `benchrun build-apps`.

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
  multiplexed is invalid: the counts are not scaled, so it would look fast. For
  investigations, a workload run by hand with `BENCH_EVENTS=name=config,...` also counts up
  to three raw PMU events per phase, in user mode, in place of user cycles (for example
  `dtlb_miss=0xff45,walk_4k=0x1045,walk_2m=0x4045` on Zen 5; `perf list --details` gives the
  codes).
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
  `docs/research/measuring.md`, "The method in brief".
- **Checks.** Each run must report the allocator, and for tcmalloc the mode, it was meant to
  run with, and where the allocator's code was: linked into the executable, or which shared
  library. A preload run must report the library it preloaded. Native runs report from
  inside, preloaded ones through `baselines/probe.c`. Runs that disagree, fail, or time out
  are recorded as invalid and left out of reports.
- **Other work on the job's CPUs.** The gate waits for a quiet machine before a run, but
  other work can still land on the job's CPUs during it. The runner sets the time those CPUs
  spent running tasks against the process's own task clock (every thread and child process).
  The CPUs' time comes from two clocks, each wrong in its own regime: /proc/schedstat's run
  queue time, which is added only when a task is switched out (so a worker that runs through
  a mark is counted in the next segment), and the time not idle in /proc/stat, whose idle
  time this kernel samples at the tick (so a CPU that idles often reads up to half a CPU off
  either way). The runner takes the smaller, since real other work shows in both, and leaves
  out each CPU's `ksoftirqd`, which runs the server's own deferred network work. For a server
  it does this over every counted segment of the plan, and more than 0.1 CPUs of other work in
  any one segment makes the run invalid; for any other job, over the whole run, with a limit
  of 0.25 CPUs. Quiet server segments measure at most 0.06, quiet runs of the quick suite at
  most 0.1; three seconds of one busy CPU, 0.55 to 0.97 in its segment. The results are
  `interference.foreign_by_segment` and `interference.foreign_task_cpus`.
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
  runs where the last two were high. The gate spins on the runner's own CPU while it
  measures, to keep the clock up (next item); that CPU is in none of the sets it measures.
- **Clock.** With the CPU's boost left to the firmware (amd-pstate-epp, EPP
  `balance_performance` on the reference machine), a job's time depends on what the machine
  did just before it: after about 300 ms with every core idle, every core runs at the
  nominal clock (4.38 GHz) for the first 250 ms of work, then boosts to 5.56 to 5.71 GHz.
  One busy core anywhere keeps the boost for all of them. Runs started after the gate took
  up to 30% longer at the same cycles, so medians of jobs ranked by time carried noise from
  the order of the runs (`docs/research/measuring.md`, "A short job after idle runs at the
  wrong clock"). So before each run the runner spins on the job's CPUs, measuring each one's
  clock in 5 ms windows, until all of them have held their warm clock for three windows, and
  starts the job straight after (the boost outlasts 200 ms of idle; the start takes a few).
  The warm clock of each CPU set is learned from an 800 ms spin the first time the set is
  used. The warm-up comes before the run's interference baseline, so it is not counted as
  foreign work. It adds about 15 ms a run, about a minute and a half to a full session.
  Every run records the clock it got (`clock.ghz`: cycles over on-CPU time of its measured
  phases) and its warm-up (`clock.warmup`); the report gives each job's median clock and the
  runs more than 5% below their allocator's usual clock on that job. An allocator can set
  the clock itself: glibc's large-object pipeline, all system calls and page faults, runs
  near nominal. Machine settings (governor, EPP) are left as they come.
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
  machine). mimalloc asks for huge pages for its whole heap and cralloc for dense classes
  past a threshold, which decides the build-and-drop jobs; a session with THP `always` would
  show the other side. `tools/thp-off.py COMMAND` runs a workload with THP disabled
  for the process, which treats every allocator alike.
- Single-threaded build jobs measure every repetition from a cold process, so allocators that
  keep freed memory gain over those that return it; jemalloc's time-based purging never fires
  in runs this short.
- Wall-time metrics of the C programs include process start-up, which grows with the size of
  the preloaded library; a few percent on the shortest (cache-scratch 8).
- The sampler reads `smaps_rollup` once a second, which holds the process's `mmap_lock`
  during a page-table walk and slightly delays allocators that `munmap` or `mremap` often
  (`--rollup-ms` changes the interval).
- Builds run on CPUs 12-15 and their siblings 28-31 (`scripts/heavy`), which share an L3 with
  measurement CPUs 8-11 and not with 0-3. A session under `scripts/measure --both-l3` freezes
  them, so it has no heavy work beside it; a job on CPUs 0-3 alone, under plain
  `scripts/measure`, runs beside them and shares only memory. When builds shared the job's L3,
  memory-bound jobs drifted by up to three times with no load on their own CPU.

Output is JSON lines: a `session` record (machine, settings, allocators, the baseline
manifests) followed by one `run` record per process.
