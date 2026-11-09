# Standings

How to measure where cralloc stands against the incumbents (jemalloc, per-CPU tcmalloc,
mimalloc, glibc) at one commit of main. A standings is many short sessions over a day or two,
interleaved with everyone else's work; `docs/research/` records what each one found.

## Before the sessions

1. Build the baselines if needed (`bench/baselines/build.sh`) and fetch the inputs
   (`bench/apps/fetch`).
2. From `bench/`, under `scripts/heavy`: `benchrun build` and `benchrun build-apps`. They make
   the runner, the fixed RPC client, the load generator and the normal builds.
3. Server jobs move by several percent with where the program's code lands, so each allocator
   runs at five placements. Under `scripts/heavy`, `pad-build.sh workloads` and `pad-build.sh
   PROGRAM` build them (`cra jem tcm mim gli`, pads 0, 16, 32, 48, 64, or on aarch64 steps of 32 to
   128). Append `pad-entries.sh`'s
   output to `bench/allocators.toml` for the sessions; it is local and never committed.
4. Tails rank idle states unless deep idle is off: `scripts/deep-idle off` (root). `guard.sh`
   checks it before each session and stops the queue if it is on.

## The sessions

Each line of a file `DIR/queue` is one session of about ten minutes. `queue.sh DIR` runs them one
at a time, with a gap as long as the session after each; lines can be added while it runs.
`plan.sh` prints a line for jobs at chosen pads:

```
plan.sh DIR OUTDIR 1 servers "connect log" 00-16 cra,jem,tcm,mim,gli >> DIR/queue
```

`session.sh OUT SUITE ONLY ALLOCATORS [options]` is what a line runs, under `scripts/measure
--both-l3`; write one by hand for jobs without pads. In the order of the last standings:

| Part | Sessions | Report |
|---|---|---|
| Servers over the pads, two or three rounds (glibc one) | `plan.sh`: `servers` jobs; `servers-large` jobs `orders axum cache 2G` and `connect store 1M`, `--mem-cap 6G`; `full` jobs `rpc log c64` and `rpc echo c64` | `servers.py`; `against.py` for the last standings |
| Saturated rates, three rounds of pad-0 builds | `session.sh`: the `sat` jobs of `servers-sat` and `servers-large-sat` | `servers.py` |
| Large heaps, three rounds | `session.sh`: `servers-thread` jobs `orders axum cache 2G thread`, `connect store 1M thread`, `meilisearch hn thread`, normal builds, `--mem-cap 8G` or `12G` | `threadsum.py`, `heapsum.py` |
| Real servers | `plan.sh`: `router graphql`; `session.sh`: `servers-thread` jobs of SurrealDB and Qdrant, `--mem-cap 10G` or `12G` | `servers.py`, `heapsum.py` |
| Tails on a machine with nothing else running | `quiet-session.sh`, which needs no agent running until it exits and logs every context switch beside the results | `whoran.py` says which runs were quiet; `servers.py` |
| Long-run memory | `drift/` (below) | `drift/summary.py`, `drift/redis_summary.py` |
| Batch programs | `session.sh`: `apps` job `datafusion tpch sf3 t4`, six rounds; `tpch.py OUT WORKDIR --rounds 4` under `scripts/measure --both-l3` | `benchrun report`; `tpch.py` prints its table as it runs |
| Kernel suite, five to ten rounds | `session.sh`: `full`, two groups of jobs a session, every allocator | `compare.py`, `groups.py` |

`--only` takes `=NAME` for one job exactly. `benchrun list --suite NAME` prints a suite's jobs.
`padsweep.py` combines sessions of `benchrun layout --layouts pads`, the runner's own pad builds.
`diag.py` is a first look at why a job is behind.

## Long-run memory

`drift/` runs are not timed, so they run under `scripts/heavy`, one at a time. Queue them with
`NOGAP=1` so that they never run beside a memory-heavy session:

```
NOGAP=1 cd ROOT && scripts/heavy bench/standings/drift/matrix.sh OUT steady 'cralloc je tc mi glibc'
```

`drift/build.sh` builds the workload per allocator and `matrix.sh` runs `steady`, `shift`,
`lifetime` and `large`. `drift/build-miniredis.sh` builds mini-redis per allocator, `requests.py`
writes its inputs and `redis-matrix.sh` runs them with the `live-bytes` build as denominator.
