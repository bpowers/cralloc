# Real Rust programs as allocator benchmarks

A small, opinionated collection of real Rust programs, each run with the allocator under test
as its `#[global_allocator]`: what a Rust user gets by changing one line. Servers come first:
interactive services on tokio, measured by the latency and the work per request at a fixed
load and by the memory they hold while load comes and goes. Batch programs follow, measured
by the work of a fixed job. They are the `servers` and `apps` suites of `bench/` (see
`bench/README.md` for the method every suite shares), written so that the collection could
move to a repository of its own.

Servers (`--suite servers`), each on four cores of one L3 domain with the load generator on
four of the other:

| Server | Request | Runtime | Shape |
|---|---|---|---|
| `connect log` | connect-rust's log-ingest service (`bench/rpc`): a 50-record protobuf batch per request, Connect protocol over HTTP/1.1 | tokio, workers pinned | decode and walk a batch: many small blocks freed on the thread that made them |
| `orders axum` | the screening's order service (`orders/`): parse a JSON order, price it, keep the last 100k in a sharded map | axum on tokio, work stealing | a cache: blocks freed by whichever worker evicts them, often another |
| `orders actix` | the same service on actix-web | one single-threaded runtime per worker | the same, with requests pinned to a worker |
| `connect log grpc h2`, `connect log h2` | the log-ingest service over HTTP/2: gRPC (as connect-rust's own log_bench drives it) and the Connect protocol | tokio, a connection task between the socket and the handlers | frames, header blocks and bodies made by one task and freed by another |
| `orders axum h2` | the axum order service over HTTP/2 | the same | the same |
| `miniredis` | tokio's mini-redis 0.4.1, alternate SET and GET over 100k keys (Redis protocol) | tokio, one `Mutex<HashMap>` | the control: values of 32 to 512 bytes replaced in place |

HTTP/2 jobs use eight connections of eight streams each: the same 64 requests at once as the
HTTP/1.1 jobs' 64 connections.

Batch programs (`--suite apps`):

| Program | Work | Threads | Shape | Ships with |
|---|---|---|---|---|
| `ruff` 0.16.9 | `ruff check` of Home Assistant core 2026.10.2 (19k files, 3.8M lines) with its own configuration, no cache; 1.1 s, 40 MB | 4 (rayon) | a compiler-like CLI: parse, many passes, short-lived process | jemalloc |
| `datafusion` 55.2.0 | the 22 TPC-H queries at scale factor 3 on Parquet, as datafusion-cli runs them; 3.6 s, 1 GB | 4 (tokio) | analytics: large buffers, hash tables that grow, async tasks moving between workers | mimalloc |
| `boa` 0.22.0 | the V8 benchmark suite (v7), each benchmark a fixed number of times; 8 s, 250 MB | 1 | an interpreter with a tracing GC: many small objects, one thread | jemalloc |

(Times and peaks are rough, with glibc on the reference machine; `docs/research/apps.md` has
measurements.) "Ships with" is the allocator the project's own release binary uses on Linux.

## What belongs here

- **Real programs that people run, at pinned releases**, built with the program's own release
  profile, the same for every allocator, so that only the allocator differs. A driver may call
  the program's library instead of its binary when that runs the same code (ruff's `main` is
  argument parsing and one call). Where the shipped binary is the point, a patch of the one
  line that names the global allocator is allowed; it is kept in the program's directory and
  named in its table row. A service written for the suite (the order service) is allowed as a
  framework's representative until a real program covers that shape, and says so.
- **The allocator must matter.** Allocators must differ on the program by clearly more than
  the run-to-run noise, in work, latency or memory. A program on which they all agree may stay
  only as a named control.
- **Each program adds an axis the others do not cover**: thread count, the share of frees made
  on another thread, the size mix, heap size, growth by `realloc`, async or not, a
  short-lived process or a server. Coverage over count.
- **Deterministic, fixed work**: the same work in every run whatever the allocator's speed (no
  "as many iterations as fit in a second"; a server gets requests at a fixed rate, not as
  fast as it answers), inputs generated deterministically, vendored when small, or fetched
  once and checked against a digest. No network at run time. Seconds per run (roughly 1 to
  20 for a batch job, under a minute for a server's day): long enough that the spread between
  runs is well under the differences between allocators, short enough that a session of many
  rounds fits in minutes. Do not repeat a short job inside the process to make it longer;
  that changes its shape.
- **The program checks its own output**, so that an allocator that breaks it cannot win: the
  number of diagnostics, the rows each query returns, the benchmark's self-checks; for a
  server, every response must be a success, and the generator fails the run otherwise.
- **Rank by work, not by wall time, when threads are involved.** A parallel program's wall
  time depends on how its threads were scheduled, which can move it by more than the
  allocator does (DataFusion on two L3 domains ran in two modes 40% apart for every
  allocator). Batch programs rank by the cycles of all their threads over the measured work;
  servers by cycles per request at a fixed rate, with latency percentiles beside it. Wall time
  and throughput are recorded, not ranked.
- **Memory for every run**: the run's cgroup peak and the process's own peak resident set;
  for a server, the resident set as a curve through load, silences and idleness. For large
  heaps, transparent huge pages and compaction matter: every run records the machine's
  `thp_*` and `compact_*` counters over the run and the process's huge-page-backed memory.

## Running

```
bench/apps/fetch                                   # inputs, once (about 1.2 GB; builds tpchgen-cli)
cd bench
cargo build --release -p bench-runner
scripts/heavy ./target/release/benchrun build      # the workloads, including connect-rust's server
scripts/heavy ./target/release/benchrun build-apps # each program once per allocator, and loadgen
scripts/measure --both-l3 ./target/release/benchrun run --suite servers --rounds 3
scripts/measure --both-l3 ./target/release/benchrun run --suite apps --rounds 5
./target/release/benchrun report results/servers-*.jsonl --detail
```

(`scripts/` is the repository's; run from `bench/`, they are `../scripts/heavy` and
`../scripts/measure`.) `build-apps` takes `--apps ruff,boa` and `--allocators a,b`; `run`
takes everything it takes for the other suites (`bench/README.md`). Every run is capped at
`--mem-cap` (default 4 GiB), and a run that reaches it is invalid. A server session takes
about 45 s a run, so split it into sessions of a few minutes with gaps between them: a
measurement on both L3 domains pauses everyone's builds. `--only` takes `=NAME` for one job
exactly (`--only "=connect log"` without the HTTP/2 jobs whose names contain it).

## Servers

A server job starts the server on its CPUs, waits for it to print `BENCH_LISTEN addr`, then
starts the load generator, `bench/probes/loadgen`, on the other L3 domain. The generator is
open loop: requests come due on a fixed schedule whatever the server does, each connection
carries one at a time, and latency runs from when a request was due to when its response
arrived, so a stall shows as latency, and as requests that missed their slot, not as fewer
requests. It is built once with glibc (`build-apps`) and never changes. It speaks HTTP/1.1,
HTTP/2 over cleartext with prior knowledge (a set number of connections, each with a set
number of concurrent streams; a gRPC response must also carry `grpc-status: 0`), and the
Redis protocol (a file of commands sent in turn).

The generator runs a plan of segments, the same for every allocator, a server's day
(`day` in `bench/runner/src/suite.rs`): two seconds of closed-loop load to fill the server's
pools, four of closed loop to measure the saturated rate (`sat`), six seconds at 30% of the
slowest allocator's saturated rate (`s30`), six at 70% (`s70`), three bursts of two seconds
at 70% separated by three-second silences with the connections open (`on`, `off`), then the
connections close and the server is watched for ten seconds (`idle`). A fixed-rate segment
after a closed one starts with a second that is not counted. The rates are found with
`--suite servers-sat` and written into the job, so a change of rate is a change of job.

The generator marks each segment on its stdout; at every mark the runner reads the server's
counters (cycles, instructions, CPU time, page faults, all threads), so each group of
segments gets the server's own work per request over exactly that group. The runner samples
the server's resident set every 50 ms for the whole run and keeps a 250 ms curve of it from
the first mark to the end, with the marks on the same clock, not values at round seconds,
where an allocator's clocked steps may fall.

A server job reports, per group of segments: the rate achieved, latency p50, p90, p99,
p99.9 and max, requests that missed their slot, errors, the server's cycles, instructions, CPU
time and page faults per request, and its peak resident set; and over the run, the peak under
load, the curve, the cgroup peak and the THP counters. It ranks by cycles per request at 70%
(`s70`).

**Idle states.** Between requests a server's cores go idle, and the deepest state on the
reference machine (C3) takes about 350 us to leave. A faster allocator leaves more idle time,
hence more C3, and with the states left alone the 70% tail ranks the machine's idle states
and the runtime's parking, not the allocator: p99.9 of 1 to 7 ms for every allocator, with
the slowest allocator's often the best (`docs/research/server-latency.md`). So by default the
runner holds the server's cores out of deep idle while the server runs, the same way for
every allocator: a spinning thread at `SCHED_IDLE` on each of its CPUs' SMT siblings (it
needs no root, which disabling the state would). p99.9 then falls to tens of microseconds.
The spinner shares the core, so the server's cycles per request rise (by a third on the
order service) and its saturated rate falls, alike for every allocator. Every run records
which way it ran (`idle_states`); `--idle-states free` leaves the states alone, and its
tails measure the machine as much as the allocator.

`benchrun report` prints a table per server: latency and work at each rate, and memory under
load, in the silences and after the connections close.

**Large heaps** (`--suite servers-large`, rates from `servers-large-sat`). The same servers
holding about 0.5, 2 and 4 GiB live while they serve, so that memory can be judged as a
ratio: the order service keeping its last `keep` orders, and mini-redis preloaded with
`preload` keys before it listens, then churned by overwrites of log-uniform 64 B to 4 KiB
values, one in ten with an expiry (`miniredis/fetch.sh`). Their day fills the heap with
closed-loop load, measures the saturated rate, then holds 70% for a minute in six marked
ten-second pieces, then the bursts and the idle close as above. Run each size as a session of
its own with `--mem-cap` sized to it (3, 7 and 12 GiB). The denominator is the program's own
live bytes: build the servers with `bench-alloc`'s `live-bytes` feature (asmalloc with a
counter of requested bytes, never a build that is compared) and add it as an allocator entry
with `build = "live-bytes"`; the runner reads its count at every mark (`marks_ms`), and
`bench/probes/partial-load/largesum.py` puts each allocator's resident set over it.

## How a program is built

Each program is a cargo workspace of its own in `<program>/`: a driver crate with the
program's crates as dependencies, its own `Cargo.lock`, and the program's release profile
copied from its repository (a service written for the suite takes the suite's: fat LTO, one
codegen unit). The lock file starts as the program's own at that release (cargo then adds
only what the driver needs), so the program is built from the versions it was released with.
The allocator crates in it must be the versions `bench/Cargo.lock` pins; `build-apps` checks.

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

connect-rust's server is `rpc-server` of the bench workspace, which `benchrun build` builds
with the workloads; `connect/` holds only its input.

## Inputs

`bench/apps/fetch [program...]` runs `<program>/fetch.sh DEST` for each program, into
`<data>/inputs/<program>`, where `<data>` is `$BENCH_APPS_DATA`, by default `bench/apps/target`
of the main checkout (gitignored, shared by every worktree). A fetch script must leave its
directory complete and verified (a digest, or a git commit id) or fail. The runner passes the
directory to a job as `{inputs}`, and the program's source directory as `{app}`.

## The driver contract

A program's driver is a binary named after the program:

```
<program> <job> [key=value ...]
```

- Start with `bench_app::Driver::start`, before the program starts any thread.
- A batch program wraps its work in `Driver::measure` and ends with `Driver::finish`, which
  prints the `BENCH_RESULT` line (`bench/harness`): the allocator the process really ran
  with, each measured phase's counters and wall time, the program's metrics, and the primary
  metric, the cycles of all threads over the measured phases.
- A server binds an ephemeral port on loopback, prints `BENCH_LISTEN addr`, serves until
  SIGTERM, shuts down cleanly and ends with `Driver::finish_server`, which prints the result
  line without a primary metric (the runner measures a server from outside).
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

## Adding a program

1. Make `<program>/` with `Cargo.toml` (a `[workspace]` of its own; `bench-alloc` and
   `bench-app` by path; the program's crates at the release; the allocator features block,
   copied from another program; the program's release profile, copied and cited), and
   `src/main.rs` (the driver).
2. Seed `Cargo.lock` with the program's own at that release, then let cargo add the rest
   (`cargo metadata`). Pin any allocator crate to `bench/Cargo.lock`'s version with
   `cargo update -p NAME --precise VERSION`.
3. `fetch.sh` for its inputs, with a digest.
4. A batch program: a job in `apps()` of `bench/runner/src/suite.rs`, with `{inputs}` for the
   input directory, the CPUs (one L3 domain or both), and a timeout; the id names the
   program, the input and the thread count. A server: an entry in `served()` there, with its
   request (path, content type, headers, body file) and its two fixed rates, found with
   `--suite servers-sat` as 30% and 70% of the slowest allocator's saturated rate.
5. `bench/apps/check <program>` (format and lints), `benchrun build-apps --apps <program>`,
   then a session of a few rounds against every allocator. Keep it only if it meets the
   criteria above, and record what it showed in `docs/research/`.

## Moving it out

The collection depends on `bench/alloc` (the allocator features), `bench/harness` (counters,
the result line), `bench/probes/loadgen` (the load generator), `bench/runner` (`build-apps`,
the suites, sessions and reports) and `bench/rpc` (connect-rust's server), and on
`bench/baselines/build.sh` for the C allocators. Those, with `bench/apps`, are what a
repository of its own would take.
