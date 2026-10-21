# Standings

Measured 2026-10-20 and 2026-10-21. asmalloc at main `efb9cfa` (the lean slow path, pacing by
bytes, half full on a reversal with the follow-up rule), against the incumbents, on the three
suites of `bench/`: the kernel suite (`--suite full`), the apps and the servers. The last kernel
session was at `e96086d` (`baselines.md`), before commit-ahead, 1,024-slot magazines, the lean
slow path and the reversal rule.

**First cut.** Still to come: tcmalloc re-run on the jobs that grow blocks with `realloc` (its
wrapper changed in `ea192e1`, below); main as of `bb80288` on the jobs its later changes
should move, including DataFusion's memory, which this cut reports from `efb9cfa` and which
main's large-slot hold supersedes; and a `benchrun layout` check of the RPC log jobs and
BTreeMap.

## Summary

- **Kernel suite:** ahead of the best incumbent on 37 of 55 jobs, level on 3, behind on 15
  (at `e96086d`: 33, 5, 17). The geometric mean of asmalloc's time over the best incumbent's
  fell from 0.865 to 0.823. Cross-thread small objects gained most (pipelines 5 to 58% ahead,
  up to 13 points more than before); single-threaded churn is 11 to 34% ahead.
- **Servers:** ahead on 4 of 7 jobs and level on 3, behind on none (geomean 0.993). asmalloc
  does the least work per request on the three connect-rust jobs, 2 to 3% under mimalloc, and
  is level with per-CPU tcmalloc on the order services. Before the lean slow path asmalloc
  was 11 to 18% behind on axum over HTTP/1.1 (`servers-h2.md`).
- **Apps:** boa 2% ahead, ruff level with the mimalloc crate, DataFusion 3% behind per-CPU
  tcmalloc. DataFusion's memory at `efb9cfa`, 1.2 GiB against 0.86 for jemalloc and glibc, is
  superseded by main's large-slot hold (to be measured here; the coordinator measured 853 to 865
  MiB on main against jemalloc's 853 to 871).
- **After a peak, with no thread or task:** the after-peak probe whose light work fits its
  magazines (`burst-small`) holds all 215 MiB to the end in 8 of 8 runs at `efb9cfa`; `e96086d`
  held it in 3 of 8 and returned to 1.7 MiB in the others. The program is single-threaded.
  burst-idle returns as before (3.9 MiB at 12 s, `e96086d` 1.4, jemalloc 4.5).

Where we lose, ranked by Bobby's weighting (servers and tokio programs first, memory on large
heaps as a ratio, C programs as regression checks) at the end.

## Method

Every suite through `benchrun run` under `scripts/measure` (`--both-l3` for jobs that use the
second L3 and for the servers and apps), one group and one round a piece with two minutes
between pieces, allocators in a new random order each round, every run memory-capped. Kernel
suite: ten rounds of the native groups, seven of the preload groups. Apps: six rounds.
Servers: the seven jobs of the suite as of `efb9cfa` (connect-rust over HTTP/1.1, HTTP/2 and
gRPC; the order service on axum over HTTP/1.1 and HTTP/2 and on actix-web; mini-redis), three
rounds, seven allocators (glibc, mimalloc, the control, asmalloc, jemalloc, per-CPU tcmalloc,
snmalloc). No run invalid.

The comparison is `baselines.md`'s: the best incumbent is the incumbent (not asmalloc, not the
control) with the lowest median cost; the gap is the median of per-round ratios of asmalloc's
cost to it; "level" means its 95% bootstrap interval includes no difference; the geomean is over
every kernel job but xmalloc-test, which runs in two modes. The script reproduces `e96086d`'s
numbers from that session's raw results exactly (33, 5, 17; 0.865)
(`bench/probes/standings/compare.py`). Memory is the cgroup peak,
compared the same way with the smallest incumbent and with jemalloc. Raw results:
`bench/results/standings-2026-10-20/` in the main checkout.

**tcmalloc's wrapper.** tcmalloc's apps, servers and native kernel numbers before 2026-10-21
came from a wrapper (`bench/alloc/src/tc.rs`) that kept a block in place when `realloc` grew it
within its size class; `ea192e1` makes it copy on every `realloc`. The preload jobs use
tcmalloc's own library and are untouched. This cut still has the old wrapper's numbers.

## Kernel suite

Cost (lower is better), asmalloc and the best incumbent; the gap now and at `e96086d`; cgroup
peak MiB against the smallest incumbent; and the peak against jemalloc's.

| Job | asmalloc | Best incumbent | Gap | At `e96086d` | Peak MiB, asm / smallest | Peak vs jemalloc |
|---|---:|---|---|---|---|---|
| cache-scratch 1 (preload) | 0.5074 | glibc 0.501 | level (+1%) | level (+3%) | 1.0 / glibc 0.8 | ahead 20% |
| cache-scratch 8 (preload) | 0.06703 | jemalloc 0.06705 | level (+0%) | level (-0%) | 2.8 / glibc 2.5 | behind 0% |
| cfrac (preload) | 2.075 | mimalloc 2.065 | behind 0% | behind 2% | 1.8 / jemalloc 1.4 | behind 21% |
| churn live=100 small | 22.73 | jemalloc 33.88 | ahead 33% | ahead 33% | 0.8 / glibc 0.5 | behind 20% |
| churn live=100k mixed | 82.76 | tcmalloc-percpu 93.64 | ahead 11% | ahead 9% | 34.7 / tcmalloc-thread 34.5 | ahead 18% |
| churn live=100k small | 41.69 | jemalloc 52.8 | ahead 21% | ahead 17% | 12.2 / jemalloc 11.7 | behind 4% |
| churn live=100k small (preload) | 48.31 | mimalloc 50.68 | ahead 5% | level (+0%) | 12.2 / jemalloc 12.0 | behind 3% |
| churn live=10M small | 210.6 | jemalloc 245.8 | ahead 14% | ahead 13% | 1069.7 / jemalloc 1091.5 | ahead 2% |
| churn live=10k small | 27.87 | jemalloc 39.63 | ahead 30% | ahead 29% | 2.0 / glibc 1.8 | behind 14% |
| churn live=1M small | 102.1 | jemalloc 121.3 | ahead 16% | ahead 12% | 110.5 / jemalloc 110.2 | behind 0% |
| churn live=1k small | 23.26 | jemalloc 35.23 | ahead 34% | ahead 34% | 1.0 / glibc 0.5 | behind 34% |
| espresso (preload) | 2.212 | mimalloc 2.226 | ahead 1% | behind 0% | 2.8 / glibc 1.3 | behind 58% |
| exchange t2 sizes=mixed local | 8.016 | tcmalloc-percpu 8.862 | ahead 9% | ahead 11% | 5.5 / tcmalloc-percpu 3.8 | level (-1%) |
| exchange t2 sizes=small local | 3.839 | tcmalloc-percpu 6.958 | ahead 45% | ahead 40% | 3.0 / snmalloc 1.7 | behind 24% |
| exchange t2 sizes=small split | 12.96 | jemalloc 18.62 | ahead 30% | ahead 26% | 3.7 / glibc 1.3 | behind 57% |
| exchange t4 sizes=small local | 2.027 | tcmalloc-percpu 3.657 | ahead 45% | ahead 41% | 5.1 / snmalloc 2.5 | behind 21% |
| exchange t8 sizes=small both | 2.617 | jemalloc 3.971 | ahead 34% | ahead 34% | 9.2 / snmalloc 4.7 | level (+0%) |
| forkjoin t4 local | 3.43 | tcmalloc-percpu 4.398 | ahead 22% | ahead 20% | 16.4 / tcmalloc-percpu 16.0 | ahead 47% |
| forkjoin t8 both | 3.698 | jemalloc 3.94 | ahead 6% | ahead 2% | 19.5 / tcmalloc-percpu 17.3 | ahead 14% |
| json 32Mi | 16.24 | mimalloc-crate 16.41 | ahead 1% | behind 2% | 353.7 / mimalloc-crate 355.2 | ahead 1% |
| json 32Mi (preload) | 16.85 | mimalloc 16.37 | behind 3% | behind 7% | 354.7 / jemalloc 360.5 | ahead 2% |
| largepair 64Ki+1Mi | 46.53 | glibc 8.123 | behind 473% | behind 533% | 0.8 / glibc 0.2 | behind 9% |
| larson 8 (preload) | 0.002061 | mimalloc 0.001938 | behind 4% | behind 14% | 78.9 / tcmalloc-thread 53.5 | ahead 11% |
| larson-sized 8 (preload) | 0.002025 | mimalloc 0.001853 | behind 9% | behind 16% | 79.3 / tcmalloc-thread 54.4 | ahead 11% |
| map btree 1M | 926.2 | mimalloc-crate 857.2 | behind 8% | behind 6% | 123.5 / jemalloc 125.2 | ahead 1% |
| map hash 1M | 537.8 | mimalloc-crate 445.7 | behind 20% | behind 24% | 204.5 / jemalloc 183.3 | behind 11% |
| mpmc p1 c3 sizes=small local all-alloc | 4.661 | jemalloc 7.545 | ahead 38% | ahead 34% | 4.2 / glibc 1.5 | behind 87% |
| mpmc p1 c3 sizes=small local free-only | 4.615 | tcmalloc-percpu 8.511 | ahead 45% | ahead 41% | 4.2 / glibc 1.6 | behind 81% |
| mpmc p3 c1 sizes=small local all-alloc | 4.589 | tcmalloc-percpu 7.122 | ahead 36% | ahead 34% | 5.0 / glibc 2.1 | behind 57% |
| mpmc p3 c1 sizes=small local free-only | 4.586 | tcmalloc-percpu 7.117 | ahead 35% | ahead 34% | 5.0 / glibc 2.1 | behind 72% |
| mpmc p4 c4 sizes=small both all-alloc | 4.252 | jemalloc 7.585 | ahead 44% | ahead 39% | 8.7 / glibc 3.0 | behind 88% |
| mstress 8 (preload) | 0.2596 | mimalloc 0.2503 | behind 4% | behind 5% | 141.5 / jemalloc 144.3 | level (-2%) |
| pipeline t2 sizes=large objects=1M inflight=512 local all-alloc | 59.52 | tcmalloc-thread 35.63 | behind 67% | behind 51% | 2.9 / tcmalloc-thread 3.8 | ahead 65% |
| pipeline t2 sizes=mixed local all-alloc | 10.64 | tcmalloc-percpu 12.43 | ahead 14% | ahead 10% | 5.7 / tcmalloc-thread 3.2 | behind 13% |
| pipeline t2 sizes=s64 local all-alloc | 2.982 | jemalloc 6.329 | ahead 53% | ahead 40% | 1.5 / glibc 1.1 | behind 6% |
| pipeline t2 sizes=small local all-alloc | 3.385 | tcmalloc-percpu 6.664 | ahead 49% | ahead 43% | 2.9 / glibc 1.2 | behind 53% |
| pipeline t2 sizes=small local all-alloc (preload) | 3.664 | tcmalloc-percpu 6.334 | ahead 42% | ahead 37% | 2.4 / glibc 1.2 | behind 28% |
| pipeline t2 sizes=small local free-only | 3.384 | tcmalloc-percpu 6.701 | ahead 49% | ahead 43% | 3.0 / glibc 1.2 | behind 72% |
| pipeline t2 sizes=small split all-alloc | 10.4 | jemalloc 18.27 | ahead 43% | ahead 34% | 3.5 / glibc 0.9 | behind 92% |
| pipeline t2 sizes=small split free-only | 10.3 | tcmalloc-percpu 24.37 | ahead 58% | ahead 49% | 3.5 / glibc 1.0 | behind 105% |
| pipeline t2 x2 sizes=small local all-alloc | 3.599 | jemalloc 7.802 | ahead 54% | ahead 48% | 4.8 / glibc 2.2 | behind 51% |
| pipeline t2 x2 sizes=small split all-alloc | 3.87 | jemalloc 8.027 | ahead 52% | ahead 43% | 5.0 / snmalloc 2.0 | behind 53% |
| pipeline t2 x3 sizes=small both all-alloc | 4.342 | jemalloc 8.092 | ahead 46% | ahead 34% | 7.0 / glibc 3.0 | behind 48% |
| pipeline t2 x4 sizes=small both all-alloc | 4.542 | jemalloc 8.118 | ahead 44% | ahead 33% | 8.5 / glibc 3.8 | behind 40% |
| pipeline t4 sizes=small local all-alloc | 2.962 | jemalloc 3.626 | ahead 18% | ahead 16% | 5.0 / glibc 2.8 | behind 36% |
| pipeline t8 sizes=small both all-alloc | 4.17 | jemalloc 4.39 | ahead 5% | ahead 5% | 9.8 / glibc 5.8 | behind 49% |
| rpc echo c64 | 4.562e+04 | tcmalloc-percpu 4.54e+04 | level (+0%) | level (-0%) | 6.4 / glibc 3.4 | behind 11% |
| rpc log c256 | 1.234e+05 | mimalloc 1.183e+05 | behind 4% | behind 3% | 14.2 / glibc 9.5 | level (-5%) |
| rpc log c64 | 1.264e+05 | mimalloc-crate 1.216e+05 | behind 4% | behind 2% | 9.4 / glibc 5.2 | ahead 5% |
| sh8bench 16 (preload) | 0.3861 | mimalloc 0.2516 | behind 56% | behind 102% | 134.4 / tcmalloc-percpu 119.8 | ahead 19% |
| spawn t4 local | 129.4 | glibc 99.99 | behind 29% | behind 31% | 2.9 / glibc 1.2 | behind 23% |
| spawn t8 both | 252 | glibc 179 | behind 41% | behind 12% | 6.0 / glibc 2.8 | behind 35% |
| tree depth=20 | 40.68 | mimalloc-sized 45.22 | ahead 10% | level (+1%) | 50.2 / glibc 64.5 | ahead 25% |
| vecgrow 1x256Mi | 2.173 | mimalloc-crate 4.151 | ahead 48% | ahead 48% | 258.5 / glibc 257.0 | ahead 2% |
| vecgrow 64x4Mi | 4.871 | mimalloc-sized 3.953 | behind 23% | behind 24% | 277.7 / mimalloc-sized 265.0 | ahead 8% |
| xmalloc-test 4+4 (preload) | 0.00104 | tcmalloc-percpu 0.001803 | ahead 42% | ahead 23% | 35.4 / tcmalloc-thread 34.0 | ahead 31% |

**What moved since `e96086d`.** Ahead now and not then: tree depth 20 (10%, a placement job,
`baselines.md` section 14), json 32Mi native (1%), churn 100k preloaded (5%), espresso (1%).
The cross-thread jobs gained 2 to 13 points; larson moved from 14 to 4% behind, sh8bench from
102 to 56%. Two gaps widened without asmalloc's own time moving: spawn t8 (12 to 41% behind,
asmalloc's time 0.9% lower; glibc, now the best incumbent, ran faster) and the large-object
pipeline (51 to 67%, asmalloc's time +1.0%; tcmalloc's per-thread mode ran faster). asmalloc's
own time moved against it only on the RPC log jobs (+2.0% at c64, +1.9% at c256) and BTreeMap
(+1.4%), which the layout check is to explain.

## Apps

Gcycles of the measured work, six rounds; peak against the smallest incumbent and jemalloc.

| Job | asmalloc | Best incumbent | Gap | Peak MiB, asm / smallest | Peak vs jemalloc |
|---|---:|---|---|---|---|
| boa v8 | 44.38 | snmalloc 45.31 | ahead 2% | 264.7 / tcmalloc-percpu 253.0 | level (-1%) |
| datafusion tpch sf3 t4 | 69.35 | tcmalloc-percpu 67.74 | behind 3% | 1213.7 / glibc 856.1 | behind 40% |
| ruff check ha-core t4 | 22.85 | mimalloc-crate 22.50 | level (+1%) | 46.8 / tcmalloc-percpu 32.7 | behind 24% |

At the first apps session (`apps.md`, `515412e`) asmalloc was 1.4% behind mimalloc on ruff
and 1.7% ahead on boa. ruff's 47 MiB is a small heap.

## Servers

Server cycles per request at 70% of the slowest allocator's saturated rate, three rounds.

| Job | asmalloc | Best incumbent | Gap | Peak MiB, asm / smallest | Peak vs jemalloc |
|---|---:|---|---|---|---|
| connect log | 121.2k | mimalloc 125.5k | ahead 3% | 9.5 / glibc 8.7 | level (+2%) |
| connect log grpc h2 | 129.8k | mimalloc 132.4k | ahead 2% | 10.7 / glibc 7.3 | ahead 23% |
| connect log h2 | 127.3k | mimalloc 131.6k | ahead 3% | 10.9 / glibc 7.3 | ahead 23% |
| orders axum | 69.5k | tcmalloc-percpu 69.9k | ahead 1% | 206.1 / jemalloc 202.8 | behind 2% |
| orders axum h2 | 60.7k | tcmalloc-percpu 60.4k | level (+1%) | 206.1 / jemalloc 203.6 | behind 1% |
| orders actix | 56.8k | tcmalloc-percpu 55.8k | level (+2%) | 206.0 / jemalloc 203.8 | behind 1% |
| miniredis | 36.0k | snmalloc 36.1k | level (+1%) | 45.2 / glibc 36.4 | behind 9% |

Since `servers-h2.md` (`fffde81`, before the lean slow path) asmalloc moved from 136k to 130k on
gRPC over HTTP/2, 81.4k to 69.5k on axum over HTTP/1.1 and 64.2k to 60.7k over HTTP/2, and
38.3k to 36.0k on mini-redis, where it was the only allocator 6% over the rest.

Measured by others on the same allocator code as main's (no change under `crates/` between
their commits and `bb80288`), and not repeated here: Sōzu (`servers-sozu.md`): every allocator
within its runs' spread on time, asmalloc 2.5 MiB a process over glibc; Meilisearch
(`servers-meilisearch.md`): the least work per request (17.1M cycles, 3% under jemalloc), heap
704 to 931 MiB under load against jemalloc's 740 to 774. The large-heap servers
(`servers-large.md`) were measured at `e0baed4`, before the large-slot hold and bounded purge.

## After a peak

`quiet live=2M sizes=small secs=12 after=burst`, the light work after the free either 64-byte
objects (`after_sizes=s64`, burst-idle) or the twelve small classes (`after_sizes=small`),
native, one thread, CPU 2, sampled every 250 ms; anonymous resident MiB, median of three runs:

| Probe, build | 1.5 s | 2.5 | 4.5 | 6.5 | 8.5 | 11.9 | peak |
|---|---:|---:|---:|---:|---:|---:|---:|
| burst-idle, asmalloc `efb9cfa` | 215.0 | 68.2 | 39.3 | 22.0 | 11.9 | 3.9 | 216 |
| burst-idle, asmalloc `e96086d` | 214.9 | 68.0 | 38.3 | 21.3 | 11.4 | 1.4 | 216 |
| burst-idle, jemalloc | 141.5 | 141.5 | 95.5 | 22.3 | 4.5 | 4.5 | 219 |
| burst-idle, mimalloc | 226.2 | 226.2 | 226.2 | 226.2 | 14.8 | 16.2 | 227 |
| burst-idle, glibc / tcmalloc | 208.5 / 210.8 | | | | | 208.5 / 210.8 | 240 / 224 |
| burst-small, asmalloc `efb9cfa` | 215.0 | 215.0 | 215.0 | 215.0 | 215.0 | 215.0 | 216 |
| burst-small, asmalloc `e96086d` | 214.9 | 68.3 | 39.1 | 21.7 | 11.7 | 1.7 | 216 |
| burst-small, jemalloc | 181.1 | 86.5 | 86.5 | 43.0 | 4.6 | 4.6 | 219 |
| burst-small, mimalloc | 226.2 | 226.2 | 54.7 | 54.7 | 54.7 | 34.2 | 227 |
| burst-small, glibc / tcmalloc | 208.5 / 210.9 | | | | | 208.5 / 210.9 | 240 / 224 |

burst-small at `efb9cfa` held all 215 MiB in 8 of 8 runs; `e96086d` in 3 of 8 (the known
"burst that is never given back", `memory.md`). The light work allocates and frees fewer
objects of each class than a magazine holds, so after the free it never leaves the fast path:
no slow path, no heartbeat, nothing returned. That 1,024-slot magazines and pacing by bytes
turned "sometimes" into "always" is my reading, not yet measured. The program has one thread, so a background thread
that starts only in programs with several would not reach it; it keeps the memory unless it
calls `maintain` or `collect`.

## Where we lose: first diagnoses

Counted with `perf stat` (user and kernel cycles, instructions, faults, dTLB and L1 misses) and
`strace -c`, three runs each of the job's own command line against its best incumbents, under
`scripts/measure` (`bench/probes/standings/diag.py`).

1. **tokio spawn** (29% behind glibc at four threads, 41% at eight). Front end, object
   placement: asmalloc retires 20% fewer user instructions than glibc at eight threads (19.0 against
   23.6 billion) and spends 40% more user cycles (124 against 89 billion), with 70% more L1
   data misses (1.08 against 0.63 billion; jemalloc 0.74). The extra cycles are cache
   traffic, not allocator work: tokio's task structures, allocated by one worker and touched by
   others, land where they share lines (`baselines.md` section 15). The same at four threads.
2. **DataFusion's memory** (40% over jemalloc at `efb9cfa`; superseded on main, to be
   confirmed). Back end retention: a mean resident 474 MiB against jemalloc's 332, with similar
   faults and no huge pages to speak of; freed large blocks stay resident between query phases.
   Main's large-slot hold was built for this.
3. **HashMap build and drop** (20% behind the mimalloc crate; 11% over jemalloc's peak) and
   **64 Vecs grown to 4 MiB** (23% behind). Back end, large blocks returned and faulted back:
   HashMap takes 61k page faults to mimalloc's 307, four times its kernel cycles and 550 times
   its dTLB misses (no huge pages under the tables); vecgrow's user cycles are under
   mimalloc's, but its kernel cycles are nine times as many, 57k faults against 335, 1,593
   `madvise` and 252 `mincore` calls against 130 and none. Main's hold and bounded purge bear
   on both.
4. **Large objects crossing threads** (the 4 to 256 KiB pipeline, 67% behind per-thread
   tcmalloc). Back end, the large path: 2.3 times tcmalloc's user instructions (1.65 against
   0.73 billion; mimalloc 0.54) and 20 times its dTLB misses. There is no per-CPU cache for
   these sizes; every allocation and free is a back-end call.

Not ranked as targets: `largepair` (5.7 times glibc's time, from the top of glibc's heap, a
microbenchmark), the C programs (sh8bench 56%, larson 4 to 9%, mstress 4%, json preloaded
3%), and the RPC log jobs' 4% (the servers suite, which runs the same service under open load,
has asmalloc ahead).

## What to work on next

1. **The after-peak hold in single-threaded programs.** Not a speed gap but the clearest
   regression: a quiet single-threaded program now keeps everything after a peak. Decide what
   such a program gets by default (a step on the fast path's rare events, the age limit run
   from somewhere, or a documented need to call `maintain`).
2. **tokio spawn's cache traffic.** The tokio case Bobby weights first, and the largest gap
   left on a Rust runtime: find which objects share lines across workers (`perf c2c`) and
   whether placement by class or by CPU can keep a task's header from its neighbours'.
3. **Large blocks returned and faulted back** (HashMap, vecgrow 64x4Mi; DataFusion's memory),
   after confirming what main's hold and bounded purge already fix.
4. **The large path across threads**, a per-CPU cache or batched frees for 16 to 256 KiB
   blocks, if the servers' traces show these sizes cross threads.
