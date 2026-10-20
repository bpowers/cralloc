# Real Rust programs as allocator benchmarks

A small, opinionated collection of real Rust programs, each run on a pinned input with the
allocator under test as its `#[global_allocator]`: what a Rust user gets by changing one line.
It is the `apps` suite of `bench/` (see `bench/README.md` for the method every suite shares),
and it is written so that it could move to a repository of its own.

| Program | Work | Threads | Shape | Ships with |
|---|---|---|---|---|
| `ruff` 0.16.9 | `ruff check` of Home Assistant core 2026.10.2 (19k files, 3.8M lines) with its own configuration, no cache; 1.1 s, 40 MB | 4 (rayon) | a compiler-like CLI: parse, many passes, short-lived process | jemalloc |
| `datafusion` 55.2.0 | the 22 TPC-H queries at scale factor 3 on Parquet, as datafusion-cli runs them; 3.6 s, 1 GB | 4 (tokio) | analytics: large buffers, hash tables that grow, async tasks moving between workers | mimalloc |
| `boa` 0.22.0 | the V8 benchmark suite (v7), each benchmark a fixed number of times; 8 s, 250 MB | 1 | an interpreter with a tracing GC: many small objects, one thread | jemalloc |

(Times and peaks are rough, with glibc on the reference machine; `docs/research/` has
measurements.)

"Ships with" is the allocator the project's own release binary uses on Linux. All three chose
one other than glibc's because it mattered to them, which is a reason to measure them.

## What belongs here

- **Real programs that people run, at pinned releases**, built with the program's own release
  profile, the same for every allocator, so that only the allocator differs. A driver may call
  the program's library instead of its binary when that runs the same code (ruff's `main` is
  argument parsing and one call).
- **The allocator must matter.** Allocators must differ on the program by clearly more than
  the run-to-run noise, in time or in peak memory. A program on which they all agree may stay
  only as a named control.
- **Each program adds an axis the others do not cover**: thread count, the share of frees made
  on another thread, the size mix, heap size, growth by `realloc`, async or not, a
  short-lived process or a server. Coverage over count.
- **Deterministic, fixed work**: the same work in every run whatever the allocator's speed (no
  "as many iterations as fit in a second"), inputs generated deterministically, vendored when
  small, or fetched once and checked against a digest. No network at run time. Seconds per
  run (roughly 1 to 20): long enough that the spread between runs is well under the
  differences between allocators, short enough that a session of many rounds fits in
  minutes. Do not repeat a short job inside the process to make it longer; that changes its
  shape. ruff's whole check of a 3.8-million-line project takes 1.1 s, and that is the job.
- **The program checks its own output**, so that an allocator that breaks it cannot win: the
  number of diagnostics, the rows each query returns, the benchmark's self-checks.
- **Time and memory for every run**: the measured work's wall time ranks the allocators;
  peak memory (the run's cgroup peak and the process's own peak resident set), cycles,
  instructions and page faults are recorded alongside.

## Running

```
bench/apps/fetch                                   # inputs, once (about 1.2 GB; builds tpchgen-cli)
cd bench
cargo build --release -p bench-runner
scripts/heavy ./target/release/benchrun build-apps # each program once per allocator
scripts/measure --both-l3 ./target/release/benchrun run --suite apps --rounds 5
./target/release/benchrun report results/apps-*.jsonl --detail
```

(`scripts/` is the repository's; run from `bench/`, they are `../scripts/heavy` and
`../scripts/measure`.) `build-apps` takes `--apps ruff,boa` and `--allocators a,b`; `run`
takes everything it takes for the other suites (`bench/README.md`). Every run is capped at
`--mem-cap` (default 4 GiB), and a run that reaches it is invalid.

## How a program is built

Each program is a cargo workspace of its own in `<program>/`: a driver crate with the
program's crates as dependencies, its own `Cargo.lock`, and the program's release profile
copied from its repository. The lock file starts as the program's own at that release
(cargo then adds only what the driver needs), so the program is built from the versions it
was released with. The allocator crates in it must be the versions `bench/Cargo.lock` pins;
`build-apps` checks.

The driver forwards every allocator feature of `bench-alloc` (`bench/alloc`), which declares
the global allocator. `build-apps` builds each program once per allocator entry of
`bench/allocators.toml`, with `cargo build --release --locked --features <feature>`.

Building a program from scratch for each of nine allocators would compile it nine times.
Instead every build of a program shares one cargo build directory
(`<data>/build/<program>`, below). The program's crates do not depend on `bench-alloc`, so
they compile once; each further allocator compiles `bench-alloc`, the driver crates and the
allocator itself, then links with the program's profile (LTO included), into a target
directory of its own (`bench/target/<build>/apps/<program>/release/<program>`). Each binary is
the one a from-scratch build with that feature makes. Builds from other checkouts reuse the
same directory; cargo locks it.

## Inputs

`bench/apps/fetch [program...]` runs `<program>/fetch.sh DEST` for each program, into
`<data>/inputs/<program>`, where `<data>` is `$BENCH_APPS_DATA`, by default `bench/apps/target`
of the main checkout (gitignored, shared by every worktree). A fetch script must leave its
directory complete and verified (a digest, or a git commit id) or fail. The runner passes the
directory to a job as `{inputs}`.

## The driver contract

A program's driver is a binary named after the program:

```
<program> <job> [key=value ...]
```

- Start with `bench_app::Driver::start`, before the program starts any thread; wrap the work
  in `Driver::measure`; finish with `Driver::finish`, which prints the `BENCH_RESULT` line
  (`bench/harness`): the allocator the process really ran with, each measured phase's
  counters and wall time, the program's metrics, and the primary metric, the measured phases'
  wall time in seconds.
- Check the program's output and call `bench_app::fail` (exit status 1, so the run is
  invalid) when it is not the expected one. Results that legitimately vary between runs
  (floating-point sums over parallel partitions) are not compared.
- Threads: a program sizes its pools from the CPUs it may run on, which the runner sets
  before `exec` (`BENCH_CPUS` lists them). The runner does not pin each thread, because the
  program creates its own.
- Environment: a program sees only `PATH`, `HOME`, `USER`, `LANG`, `BENCH_CPUS` and the
  allocator entry's variables, so that nothing in the caller's shell (`RAYON_NUM_THREADS`,
  `RUST_LOG`, `RUFF_*`) changes the work.
- Do not write large output to stdout: the runner reads it. Count it, or write it to a
  memory file and check it, as `ruff` does.

A server (not yet in the suite) follows the RPC jobs' protocol (`bench/rpc`): it prints
`BENCH_LISTEN addr` when ready, a load generator built with glibc marks the measured window
with `BENCH_MARK warm` and `BENCH_MARK done`, and the runner ranks it by cycles per request
over that window. Latency at a fixed load and memory after going idle would be metrics the
load generator reports. The runner's `Driver` currently names a binary of the bench
workspace; a server program would need it to name an app.

## Adding a program

1. Make `<program>/` with `Cargo.toml` (a `[workspace]` of its own; `bench-alloc` and
   `bench-app` by path; the program's crates at the release; the allocator features block,
   copied from another program; the program's release profile, copied and cited), and
   `src/main.rs` (the driver).
2. Seed `Cargo.lock` with the program's own at that release, then let cargo add the rest
   (`cargo metadata`). Pin any allocator crate to `bench/Cargo.lock`'s version with
   `cargo update -p NAME --precise VERSION`.
3. `fetch.sh` for its inputs, with a digest.
4. Jobs in `apps()` of `bench/runner/src/suite.rs`, with `{inputs}` for the input directory,
   the CPUs (one L3 domain or both), and a timeout. The id names the program, the input
   and the thread count.
5. `bench/apps/check <program>` (format and lints), `benchrun build-apps --apps <program>`,
   then a session of a few rounds against every allocator. Keep it only if it meets the
   criteria above, and record what it showed in `docs/research/`.

## Moving it out

The collection depends on `bench/alloc` (the allocator features), `bench/harness` (counters,
the result line) and `bench/runner` (`build-apps`, the `apps` suite, sessions and reports),
and on `bench/baselines/build.sh` for the C allocators. Those, with `bench/apps`, are what a
repository of its own would take.
