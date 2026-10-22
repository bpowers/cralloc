# Standings

Measured 2026-10-20 and 2026-10-21. asmalloc at main `efb9cfa` (the lean slow path, pacing by
bytes, half full on a reversal with its follow-up rule) against the incumbents, on the three
suites of `bench/`: the kernel suite (`--suite full`), the apps and the servers; and main as of
`bb80288` on the jobs its later changes (counted intervals, purge by runs, the large-slot hold,
bounded purge) should move. The last kernel session was at `e96086d` (`baselines.md`), before
commit-ahead, 1,024-slot magazines, the lean slow path and the reversal rule.

**Read the RPC server rows with care (added 2026-10-22).** Each allocator's row is one build,
and the jobs served by `rpc-server` (the connect, gRPC and RPC log jobs) move by 2.5 to 4% with
where the service's own code lands in the binary, whatever the allocator
(`grpc-placement.md`). A lead or a deficit of a few percent on those rows is inside that range
until it has been measured over a sweep of placements. The tokio spawn rows are superseded by
`spawn-gap.md`.

Two of those jobs measured over a sweep of five placements (`benchrun layout --layouts pads`,
main `040033a`, three one-round sessions each; k server cycles a request, others relative to
asmalloc, positive is slower):

| Job | asmalloc, mean (range over placements) | jemalloc | tcmalloc per-CPU | mimalloc |
|---|---|---|---|---|
| connect log grpc h2 | 129.9 (128.2 to 132.6) | +5.4% | +2.5% | |
| rpc log c64 | 123.3 (122.1 to 125.1) | +3.9% | -0.9% | -0.8% |

The sessions separate differences of 2.5% and more, not ones under 1%.

## Summary

- **Kernel suite:** ahead of the best incumbent on 37 of 55 jobs, level on 3, behind on 15 (at
  `e96086d`: 33, 5, 17). The geometric mean of asmalloc's time over the best incumbent's fell
  from 0.865 to 0.823. Cross-thread small objects gained most (pipelines 5 to 58% ahead, up to
  13 points more than before); single-threaded churn is 11 to 34% ahead. No job's standing
  fell; the gaps that widened did so because an incumbent ran faster, and asmalloc's own time
  moved by more than 1% only on one RPC job, by placement (below).
- **Servers:** ahead on 3 of 7 jobs and level on 4, behind on none (geomean 0.987). Before the
  lean slow path asmalloc was 11 to 18% behind on the order service over HTTP/1.1
  (`servers-h2.md`); now it is 1 to 3% ahead of the best incumbent there.
- **Apps:** boa 2% ahead, ruff level with the mimalloc crate, DataFusion 2% behind per-CPU
  tcmalloc. DataFusion's memory, 1.2 GiB at `efb9cfa` against 0.87 to 0.90 for jemalloc and
  glibc, is fixed on main by the large-slot hold: 866 MiB, level with jemalloc's 874.
- **After a peak, with no thread or task:** the after-peak probe whose light work fits its
  magazines (`burst-small`, one thread) holds all 215 MiB to the end in every run at `efb9cfa`
  (8 of 8) and on main (3 of 3); `e96086d` held it in 3 of 8 and returned to 1.7 MiB in the
  rest. On burst-idle main levels off at 32 MiB, its retained floor, where `efb9cfa` goes on to
  4.9.

The largest gaps that matter, ranked by Bobby's weighting (servers and tokio programs first,
memory on large heaps as a ratio, C programs as regression checks): large blocks returned and
faulted back (HashMap 20%, 64 Vecs grown to 4 MiB 23%) and large objects crossing threads
(67%), with the after-peak hold in single-threaded programs ahead of both. tokio spawn (29 to
41% behind glibc) is placement-sensitive and explained in `spawn-gap.md`, not the allocator's.

## Method

Every suite through `benchrun run` under `scripts/measure` (`--both-l3` for jobs that use the
second L3, and for the servers and apps), one group and one round a piece with two minutes
between pieces, allocators in a new random order each round, every run memory-capped. Kernel
suite: ten rounds of the native groups, seven of the preload groups. Apps: six rounds.
Servers: the seven jobs of the suite at `efb9cfa` (connect-rust over HTTP/1.1, HTTP/2 and
gRPC; the order service on axum over HTTP/1.1 and HTTP/2 and on actix-web; mini-redis), three
rounds, seven allocators (glibc, mimalloc, the control, asmalloc, jemalloc, per-CPU tcmalloc,
snmalloc). Main (`bb80288`) on six kernel jobs and DataFusion: three rounds, interleaved with
`efb9cfa` and the best incumbents. One DataFusion piece ran with other work on the job's CPUs,
every run of it invalid, and was repeated.

The comparison is `baselines.md`'s: the best incumbent is the incumbent (not asmalloc, not the
control) with the lowest median cost; the gap is the median of per-round ratios of asmalloc's
cost to it; "level" means its 95% bootstrap interval includes no difference; the geomean is over
every kernel job but xmalloc-test, which runs in two modes. `bench/probes/standings/compare.py`
reproduces `e96086d`'s numbers from that session's raw results exactly (33, 5, 17; 0.865).
Memory is the cgroup peak, compared the same way with the smallest incumbent and with
jemalloc. Raw results: `docs/research/data/standings-2026-10-21.jsonl.gz` (`benchrun report`
reads it after `gunzip`), and the after-peak probes' and diagnoses' output in
`standings-2026-10-21-after-peak.tar.gz` beside it.

**tcmalloc's wrapper.** tcmalloc's apps and servers numbers, and its native kernel numbers,
before 2026-10-21 came from a wrapper (`bench/alloc/src/tc.rs`) that kept a block in place when
`realloc` grew it within its size class; `ea192e1` makes it copy on every `realloc`. Only a job
that grows blocks with `realloc` can have moved, so tcmalloc was re-run, three rounds beside
asmalloc, on those jobs, and its old runs of them dropped; every other job's tcmalloc numbers
stand by construction, and the preload jobs use tcmalloc's own library. Which kernel jobs: a
scratch build of the suite's live-bytes allocator counting every `realloc` that grows a block,
one round of the native jobs. Six grow blocks in their measured work: the three RPC jobs (1.8 to
18 million times), json 32Mi (100k), vecgrow 64x4Mi (4,367) and vecgrow 1x256Mi (107, blocks of
up to 256 MiB). Every other job grows a block 15 to 73 times in a whole run, at startup. Every
app and server job was re-run, since real programs grow strings and buffers. The fix moved
tcmalloc against asmalloc by more than its runs' spread on four jobs: 1.4 to 3.1% faster on
connect-rust over HTTP/1.1 and HTTP/2 and on ruff, 5.2% slower on actix. It changed two
standings: per-CPU tcmalloc is now the best incumbent on connect-rust over HTTP/2 and gRPC, and
asmalloc level with it there instead of 2 to 3% ahead of mimalloc.

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
| json 32Mi | 16.26 | mimalloc-crate 16.41 | ahead 1% | behind 2% | 353.7 / mimalloc-crate 355.2 | ahead 1% |
| json 32Mi (preload) | 16.79 | mimalloc 16.37 | behind 3% | behind 7% | 354.7 / jemalloc 360.5 | ahead 2% |
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
| rpc echo c64 | 4.563e+04 | tcmalloc-percpu 4.558e+04 | level (+0%) | level (-0%) | 6.4 / glibc 3.4 | behind 11% |
| rpc log c256 | 1.238e+05 | mimalloc 1.183e+05 | behind 4% | behind 3% | 13.8 / glibc 9.5 | level (-5%) |
| rpc log c64 | 1.263e+05 | mimalloc-crate 1.216e+05 | behind 4% | behind 2% | 9.3 / glibc 5.2 | ahead 5% |
| sh8bench 16 (preload) | 0.3861 | mimalloc 0.2516 | behind 56% | behind 102% | 134.4 / tcmalloc-percpu 119.8 | ahead 19% |
| spawn t4 local (placement-sensitive, `spawn-gap.md`) | 129.4 | glibc 99.99 | behind 29% | behind 31% | 2.9 / glibc 1.2 | behind 23% |
| spawn t8 both (placement-sensitive, `spawn-gap.md`) | 252 | glibc 179 | behind 41% | behind 12% | 6.0 / glibc 2.8 | behind 35% |
| tree depth=20 | 40.68 | mimalloc-sized 45.22 | ahead 10% | level (+1%) | 50.2 / glibc 64.5 | ahead 25% |
| vecgrow 1x256Mi | 2.175 | mimalloc-crate 4.151 | ahead 48% | ahead 48% | 258.5 / glibc 257.0 | ahead 2% |
| vecgrow 64x4Mi | 4.879 | mimalloc-sized 3.953 | behind 23% | behind 24% | 277.7 / mimalloc-sized 265.0 | ahead 8% |
| xmalloc-test 4+4 (preload) | 0.00104 | tcmalloc-percpu 0.001803 | ahead 42% | ahead 23% | 35.4 / tcmalloc-thread 34.0 | ahead 31% |

**What moved since `e96086d`.** Ahead now and not then: tree depth 20 (10%, a placement job,
`baselines.md` section 14), json 32Mi native (1%), churn 100k preloaded (5%), espresso (1%).
The cross-thread jobs gained 2 to 13 points; larson moved from 14 to 4% behind, sh8bench from
102 to 56%. Two gaps widened with asmalloc's own time unchanged: spawn t8 (12 to 41% behind;
asmalloc 0.9% faster, glibc, now the best incumbent, faster still) and the large-object
pipeline (51 to 67%; asmalloc +1.0%, tcmalloc's per-thread mode faster).

asmalloc's own time moved against it across sessions on the RPC log jobs (+2.0% at c64, +1.9%
at c256) and BTreeMap (+1.4%). Run head to head with `e96086d`'s build, three interleaved
rounds: BTreeMap is level (-0.2%, 1.8% fewer instructions), the session's drift; RPC log c256
is level (+0.3%); RPC log c64 is 1.6% slower (1.1 to 2.3% by round) at the same instructions
(-0.3%), stalls rather than added work. `benchrun layout` moves the same jobs by 2.3 to 2.6%
with the same code at four placements (noise 0.3 to 0.5%, instructions within 1.4 to 2.7%), so
the c64 move is within what placement alone does to that job, and no allocator cause is found.

## Apps

Gcycles of the measured work, six rounds (tcmalloc three, with its fixed wrapper); peak against
the smallest incumbent and against jemalloc.

| Job | asmalloc | Best incumbent | Gap | Peak MiB, asm / smallest | Peak vs jemalloc |
|---|---:|---|---|---|---|
| boa v8 | 44.36 | snmalloc 45.31 | ahead 2% | 264.6 / tcmalloc-percpu 253.0 | level (-1%) |
| datafusion tpch sf3 t4 | 69.34 | tcmalloc-percpu 67.42 | behind 2% | 1207.5 / glibc 856.1 | behind 40% |
| ruff check ha-core t4 | 22.86 | mimalloc-crate 22.50 | level (+1%) | 46.0 / tcmalloc-percpu 31.2 | behind 24% |

DataFusion's memory on main, three rounds beside `efb9cfa`: 866 MiB against `efb9cfa`'s 1,199,
jemalloc's 874, glibc's 901 and per-CPU tcmalloc's 997; its time is level with `efb9cfa`'s
(+0.4%). The `efb9cfa` peak above is superseded. ruff's 46 MiB is a small heap. At the first apps
session (`apps.md`, `515412e`) asmalloc was 1.4% behind mimalloc on ruff and 1.7% ahead on boa.

## Servers

Server cycles per request at 70% of the slowest allocator's saturated rate, three rounds
(tcmalloc with its fixed wrapper).

| Job | asmalloc | Best incumbent | Gap | Peak MiB, asm / smallest | Peak vs jemalloc |
|---|---:|---|---|---|---|
| connect log | 121.0k | tcmalloc-percpu 125.0k | ahead 3% | 9.6 / glibc 8.7 | level (+2%) |
| connect log grpc h2 | 130.0k | tcmalloc-percpu 130.5k | level (-1%) | 10.7 / glibc 7.3 | ahead 23% |
| connect log h2 | 127.6k | tcmalloc-percpu 128.4k | level (-0%) | 10.9 / glibc 7.3 | ahead 23% |
| orders axum | 69.5k | jemalloc 70.5k | ahead 1% | 206.1 / jemalloc 202.8 | behind 2% |
| orders axum h2 | 60.7k | jemalloc 61.4k | level (-1%) | 206.0 / jemalloc 203.6 | behind 1% |
| orders actix | 56.8k | jemalloc 58.0k | ahead 3% | 206.1 / jemalloc 203.8 | behind 1% |
| miniredis | 35.9k | snmalloc 36.1k | level (+1%) | 45.2 / glibc 36.4 | behind 9% |

Since `servers-h2.md` (`fffde81`, before the lean slow path) asmalloc moved from 136k to 130k on
gRPC over HTTP/2, 81.4k to 69.5k on axum over HTTP/1.1 and 64.2k to 60.7k over HTTP/2, and
38.3k to 35.9k on mini-redis, where it was the only allocator 6% over the rest.

Measured by others on the same allocator code as main's (nothing under `crates/` changed
between their commits and `bb80288`), not repeated here: Sōzu (`servers-sozu.md`), where every
allocator is within its runs' spread on time and asmalloc costs 2.5 MiB a process over glibc;
Meilisearch (`servers-meilisearch.md`), the least work per request (17.1M cycles, 3% under
jemalloc), its heap 704 to 931 MiB under load against jemalloc's 740 to 774, tcmalloc measured
with the fixed wrapper. The large-heap servers (`servers-large.md`) were measured at `e0baed4`,
before the large-slot hold and bounded purge, and want measuring again on main.

## After a peak

`quiet live=2M sizes=small secs=12 after=burst`: allocate two million small objects, free
them, then light work, either 64-byte objects (`after_sizes=s64`, burst-idle) or the twelve
small classes (`after_sizes=small`), for 12 s; native, one thread, CPU 2, sampled every 250 ms.
Anonymous resident MiB, median of three runs:

| Probe, build | 1.5 s | 2.5 | 4.5 | 6.5 | 8.5 | 11.9 | peak |
|---|---:|---:|---:|---:|---:|---:|---:|
| burst-idle, asmalloc `efb9cfa` | 215.0 | 68.2 | 39.3 | 22.0 | 11.9 | 3.9 | 216 |
| burst-idle, asmalloc main `bb80288` | 215.1 | 75.0 | 43.6 | 32.2 | 32.2 | 32.2 | 216 |
| burst-idle, asmalloc `e96086d` | 214.9 | 68.0 | 38.3 | 21.3 | 11.4 | 1.4 | 216 |
| burst-idle, jemalloc | 141.5 | 141.5 | 95.5 | 22.3 | 4.5 | 4.5 | 219 |
| burst-idle, mimalloc | 226.2 | 226.2 | 226.2 | 226.2 | 14.8 | 16.2 | 227 |
| burst-idle, glibc / tcmalloc | 208.5 / 210.8 | | | | | 208.5 / 210.8 | 240 / 224 |
| burst-small, asmalloc `efb9cfa` | 215.0 | 215.0 | 215.0 | 215.0 | 215.0 | 215.0 | 216 |
| burst-small, asmalloc main `bb80288` | 215.0 | 215.0 | 215.0 | 215.0 | 215.0 | 215.0 | 216 |
| burst-small, asmalloc `e96086d` | 214.9 | 68.3 | 39.1 | 21.7 | 11.7 | 1.7 | 216 |
| burst-small, jemalloc | 181.1 | 86.5 | 86.5 | 43.0 | 4.6 | 4.6 | 219 |
| burst-small, mimalloc | 226.2 | 226.2 | 54.7 | 54.7 | 54.7 | 34.2 | 227 |
| burst-small, glibc / tcmalloc | 208.5 / 210.9 | | | | | 208.5 / 210.9 | 240 / 224 |

burst-small held all 215 MiB to the end in 8 of 8 runs at `efb9cfa` and 3 of 3 on main;
`e96086d` held it in 4 of 11 (the known "burst that is never given back", `memory.md`). The
light work allocates and frees fewer objects of each class than a magazine holds, so after the
free it never leaves the fast path: no slow path, no heartbeat, nothing returned. That
1,024-slot magazines and pacing by bytes turned "sometimes" into "always" is my reading, not
measured. **The program has one thread** (`quiet` in `bench/workloads/src/mem.rs` spawns none):
a background thread that starts only in programs with more than one thread would not reach it,
and on main it keeps the memory unless it calls `maintain` or `collect`. On burst-idle, whose
light work does reach slow paths, main stops at 32 MiB from 6.5 s on, the floor under which
main leaves retained memory to the program (`af04801`), where `efb9cfa` returns to 4.9.

## Where we lose: first diagnoses

Counted with `perf stat` (user and kernel cycles, instructions, faults, dTLB and L1 misses) and
`strace -c`, three runs each of the job's own command line against its best incumbents, under
`scripts/measure` (`bench/probes/standings/diag.py`; for jobs whose command line is the whole
job, not servers).

1. **Large blocks returned and faulted back**: HashMap build and drop (20% behind the mimalloc
   crate, 11% over jemalloc's peak) and 64 Vecs grown to 4 MiB (23% behind). Back end:
   HashMap takes 61k page faults to mimalloc's 307, four times its kernel cycles and 550 times
   its dTLB misses (no huge pages under its tables); vecgrow's user cycles are under mimalloc's,
   but its kernel cycles are nine times as many, 57k faults against 335, 1,593 `madvise` and 252
   `mincore` calls against 130 and none. Main's hold and bounded purge do not close either:
   HashMap 2.3% faster on main, vecgrow 0.6%.
2. **Large objects crossing threads** (the 4 to 256 KiB pipeline, 67% behind per-thread
   tcmalloc; main the same within 1.3%). Back end, the large path: 2.3 times tcmalloc's user
   instructions (1.65 against 0.73 billion; mimalloc 0.54) and 20 times its dTLB misses. There
   is no per-CPU cache for these sizes; every allocation and free is a back-end call.

Not ranked as targets: tokio spawn (29 and 41% behind glibc), which is placement-sensitive
and explained in `spawn-gap.md`: false sharing inside two of tokio's own structures (the
scheduler handle, fixed in tokio 1.54, and its 40-byte run-queue headers allocated side by
side), not the allocator's handling of task memory, and a glibc number that is a lucky draw
(192 to 227 ns a task at eight threads by the length of its path alone). The counters here
agree: 20% fewer user instructions than glibc at eight threads and 40% more cycles, from 70%
more L1 data misses. Those rows are to be replaced by the re-measurement on tokio 1.54.
`largepair` (5.7 times glibc's time, from the top of glibc's heap, a
microbenchmark), the C programs (sh8bench 56%, larson 4 to 9%, mstress 4%, json preloaded
3%), the RPC log jobs' 4% (placement, above; the servers suite runs the same service under open
load and has asmalloc 3% ahead), and DataFusion's 2% on time (level with every incumbent but
per-CPU tcmalloc).

## What to work on next

1. **The after-peak hold in single-threaded programs.** Not a speed gap but the clearest
   regression since `e96086d`: a quiet single-threaded program keeps everything after a peak,
   on main too. Decide what such a program gets by default (a step from the fast path's rare
   events, the age limit run from somewhere, or a documented need to call `maintain`).
2. **Large blocks returned and faulted back** (HashMap, vecgrow 64x4Mi), which main's hold does
   not reach: why these frees are purged and refaulted within the job, and whether huge pages
   under large tables would take the dTLB misses with them.
3. **The large path across threads**: a per-CPU cache or batched frees for 16 to 256 KiB
   blocks, if the servers' traces show these sizes crossing threads.
4. **Measure the large-heap servers again on main**, the heaps where Bobby judges memory: they
   were last measured before the hold and bounded purge.
