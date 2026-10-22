# Which blocks do the missed loads land on

The tools behind `docs/research/spawn-gap.md`: record every allocation, sample loads with IBS,
and put each load that missed L1 to the heap block (layout and offset) it fell in; then move
chosen blocks and time the difference. Not live code: the bench-crate parts are a patch.

## The patch

`diagnostics.patch` (apply from the repository root with `git apply
bench/probes/spawn-gap/diagnostics.patch`, and revert before committing) adds:

- `bench-alloc` feature `trace` (`src/trace.rs`): wraps the chosen allocator (asmalloc, `je`, or
  glibc with neither) and records each call's layout, address, CPU and time stamp in a per-thread
  mapping of up to 1M records; at exit it writes them to the file named by `BENCH_ALLOC_TRACE`.
  The recording perturbs timing; use it for placement, never for time.
- `bench-alloc` feature `shape` (`src/shape.rs`): wraps asmalloc and, from the environment,
  serves a size range as another size (`SHAPE_BUMP=33-40:48`) or starts blocks of one size some
  bytes into a larger block (`SHAPE_SHIFT=416:16`), up to eight entries each, comma-separated.
  It is out of line and costs a few hundred instructions a task: compare its runs with its own
  plain run, not with asmalloc's.
- Both features forwarded by `apps/orders`, and three `allocators.toml` entries (`asm-shape`,
  `asm-s40`, `asm-both`) that run the order service through `shape`.
- The runner counts raw events for a server from `BENCH_PROC_EVENTS` (`name=config,...`, the
  same encoding as `BENCH_EVENTS`) and reports `<name>_per_req` per segment group.

## Scripts

All run from the repository root; they find it from their own path. Binaries are expected as
`bench/target/run/<NAME>/workload`: hard-link each build there, so that every binary's path has
the same length (glibc's speed on the spawn jobs depends on it).

- `spawnrun.py PLACE ROUNDS BUILDS [EVENTS]`: the spawn job, builds interleaved in a random
  order each round, medians per task. `PLACE` is `t4` (CPUs 0-3) or `t8` (0-3, 8-11); `BUILDS`
  is `label[:dir[:ENV=VAL;ENV=VAL]]` joined by `/`; `EVENTS` is a `BENCH_EVENTS` value. Run it
  under `scripts/measure` (`--both-l3` for t8).
- `c2c.sh PLACE TAG BUILD...`: `perf c2c record` (IBS, system-wide on the job's CPUs) of a spawn
  run into `bench/target/perf/<BUILD>-<PLACE><TAG>.c2c`; with `TRACE=1` and a traced build also
  the allocation trace. `ROUNDS`, `PERIOD` and `EXTRA` override the run.
- `analyze.sh NAME [TOP]`: `perf script` of `NAME.c2c` and `attrib.py` against `NAME.trace`.
- `attrib.py SAMPLES TRACE [TOP]`: L1-missing user loads by block layout and line offset, by
  layout, and by symbol, split by where they were served (L2, L3, HITM, RAM).
- `trace.py TRACE [N]`: the commonest layouts and the first `N` allocations.
- `serve.py BUILD MODE PLAN`: the order service on CPUs 0-3 and the load generator on 8-11 by
  hand, with `TRACE=` and `C2C=` as above (`MODE` `axum` or `axum-h2`).
- `srv.py RESULTS...`: s70 medians from `benchrun` server results, with any extra counters.
- `ab-build.sh [--trace] NAME=COMMIT...`: builds commits from `git archive` for benchrun's
  `build = "asm-NAME"` entries (workloads, rpc-server, the order service, boa), or with
  `--trace` the traced workloads and rpc-server into `bench/target/trc-NAME`. Under
  `scripts/heavy`.
- `rpc-headers.sh NAME...`: where tokio's run-queue headers land in a traced rpc-server, and
  how many pairs share a line. Under `scripts/memlimit`.
- `tokio-pad-queue.diff`: tokio 1.54.0 with the run queue's shared header cache-padded, for a
  local `[patch.crates-io]` (see the note).

Example, t4, asmalloc against jemalloc, with demand L1 fills and those from L3 or another core:

```
scripts/measure python3 bench/probes/spawn-gap/spawnrun.py t4 4 "A:asmalloc/J:je" \
  dmnd_all=0xff43,ccx=0x243,rdblkx=0x1064
TRACE=1 scripts/measure bash bench/probes/spawn-gap/c2c.sh t4 "" AT
bash bench/probes/spawn-gap/analyze.sh AT-t4
```

Needs `perf_event_paranoid` of -1 for system-wide IBS.
