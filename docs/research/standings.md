# Standings

Measured 2026-10-22 to 2026-10-23 on the reference host (Ryzen 9950X, Linux 7.2.9), asmalloc at
main `80643d2` against jemalloc, tcmalloc (per-CPU), mimalloc and glibc. Servers first: they are
what this allocator is for. The kernel suite and the C programs are regression checks.

## Summary

- **Servers, time: asmalloc is ahead or level on every job, behind on none.** Over a sweep of
  five code placements it is 4 to 23% ahead of every incumbent on the four RPC servers that
  carry real work (the Connect and gRPC log services, the log store), 2 to 4% ahead of jemalloc
  and tcmalloc on the order service over HTTP/1.1 (resolved in five of six comparisons), and level
  with all of them on the order service over HTTP/2, Sozu, mini-redis and the RPC microjobs.
  glibc is 15 to 50% behind on every server that allocates much.
- **Servers, memory: the smallest or level on the large heaps.** The 1M-record log store holds
  1,431 MiB a minute after the clients leave, against 1,520 to 1,733 for the incumbents
  (jemalloc's 1,733 the largest but glibc's); Meilisearch 661 MiB against jemalloc's 674 and
  933 to 1,324 for the others; the 2 GiB order service 2,186 against 2,220 to 2,339. Under load
  Meilisearch's heap peaks at 949 MiB against jemalloc's 781.
  **2026-10-24:** that gap was zeroed 50 MiB buffers cleared with `memset` and so made
  resident (`meili-peak.md`). Since main `dc2f01a` a zeroed large block its tenant left
  untouched goes back at its free, and on this job the heap peaks at 741 to 757 MiB (three
  rounds), with 51 to 53 page faults a request where it took 92 to 96.
  **2026-10-24:** the merge of `outgrown-count` keeps the slots growing buffers leave behind
  within a count of their own (`grown-slots.md`). Against the main before it, same sessions:
  Meilisearch 36 to 40 faults a request at the same heap; tpchgen 4.5% fewer cycles and 218k
  faults for 706k, with its peak up from 578 to 633 MiB; 64 Vecs grown to 4 MiB 15% fewer
  cycles, peak 278 to 327 MiB; DataFusion and the 2 GiB order service level.
- **2026-10-25, three real servers added** (`servers-router.md`, `servers-surrealdb.md`,
  `servers-qdrant.md`; main `81437b0` and before). Apollo Router, over five placements, cycles a
  request against asmalloc: tcmalloc +1.3%, mimalloc +3.0%, jemalloc +6.1%, glibc +37.7%.
  SurrealDB's in-memory engine at 1.7 GiB: tcmalloc +7.0% (three placements each), mimalloc
  +12.6%, jemalloc +13.6%, glibc +27.2% (single builds); p99.9 2.7 ms against 3.3 to 3.6; heap
  1,716 MiB against 1,792 to 1,924, and 3,323 against 3,498 to 3,774 at 3.3 GiB. Qdrant's
  cycles do not rank allocators (malloc is 2% of them); its heap a minute after close is 195
  MiB against jemalloc's 131, pages stranded in partly used huge-page units (`huge-tail.md`).
  `heap-drift.md` has the long-run memory table, where asmalloc leads on a steady heap and
  trails jemalloc where class mixes wander and under churn of large blocks.
- **Servers, tails: the best p50 and p99 on most jobs, with two exceptions.** On Connect over
  HTTP/2 asmalloc's p99.9 at 70% load is 45 us against 39 to 41 for the others, and it was 39 at
  the last standings. After a 30 s silence the log store's slowest request in the first 5 ms
  takes 0.56 ms against 0.30 to 0.46 for the incumbents (the background thread's idle drains,
  as measured when it landed).
  **2026-10-25:** with C2 and C3 off on the server CPUs (`tails-idle-off.md`, main
  `3b5a3f5`, five pads), asmalloc's Connect h2 p99.9 is 37.8 us against 38.6 to 41.6 for the
  others, the lowest at every pad. gRPC h2 is 38.2 against 39.2 to 41.6. Cycles and saturated
  rates moved under 1%. The other server jobs are not yet re-measured this way: Connect over
  HTTP/1.1, the order service (axum, actix, h2, 2 GiB), Sozu, mini-redis, the Router, the log
  store, the RPC c64 jobs, Meilisearch, and the saturation suites.
- **Meilisearch** does the least work a request (16.96M cycles against 17.27 to 18.09M) and has
  the best p99.9 (43 ms against 48 to 60). It takes twice jemalloc's page faults.
- **DataFusion** is 3% behind per-CPU tcmalloc on time, level with mimalloc and glibc, 4%
  ahead of jemalloc; its peak (865 MiB) is the smallest with jemalloc's.
- **tokio spawn** is 1% (four threads) and 5% (eight) behind jemalloc, from 29 and 41% behind:
  tokio 1.54 and the strided first fill closed it.
- **Kernel suite:** ahead of the best incumbent on 38 of 55 jobs, level on 2, behind on 15
  (37, 3 and 15 at `efb9cfa`); the geometric mean of asmalloc's time over the best
  incumbent's is 0.803 (0.823).

## Method

- **Placement sweeps for the servers.** These jobs move by 2.5 to 5% with where the program's
  code lands (`grpc-placement.md`), so each allocator was built at bench-alloc's five layout pads
  (0, 16, 32, 48 and 64 bytes; `bench/probes/standings/pad-build.sh`, entries from
  `pad-entries.sh`, local and not committed), and every session ran all allocators at a subset
  of pads, interleaved. Two rounds of every pad for asmalloc, jemalloc, tcmalloc and mimalloc
  (three for Connect h2, the order service over h2 and the RPC kernel jobs), one for glibc. A
  job's figure for an allocator is the mean over the pads of its median per pad; the range is
  the lowest and highest pad median. A difference is **resolved** when the two ranges do not
  overlap, or when they overlap but an exact permutation test over the pads' medians puts the
  means apart at p < 0.05 (marked "overlap"); otherwise it is inside the range
  (`bench/probes/standings/servers.py`).
- **Tails** are the medians over every run (every build at every pad) of each run's p50, p99 and
  p99.9 at 70% of the slowest allocator's saturated rate. Each run is one build: tails differ
  between builds of identical behaviour by up to 2 ms at p99.9 on Meilisearch (`meili-tail.md`),
  and by a few microseconds on the small servers.
- **Memory under load and at rest:** the servers suites' resident peak while loaded and the
  resident set 10 s after the connections close; for the large heaps, the `servers-thread`
  suite (`default-thread-servers.md`): 30 s at 70%, 30 s of silence, a 2 s burst, a minute after
  close, three rounds, normal builds, anonymous memory. Saturation: `servers-sat` and
  `servers-large-sat`, three rounds of single builds.
- **Kernel suite** as at the last standings: `benchrun run --suite full`, ten rounds of the
  native groups and seven of the preload groups, two groups a session, all ten allocators,
  `bench/probes/standings/compare.py`. Apps: DataFusion, six rounds. After a peak: the
  `quiet` probes, three runs each.
- Every session under `scripts/measure --both-l3`, at most about ten minutes, then a gap as
  long, through `bench/probes/standings/queue.sh`. Six runs of 6,053 were marked invalid for
  other work on their CPUs: one kernel run, left out, and five of the 2 GiB order service's
  silence-and-burst runs, by 0.10 to 0.14 CPUs over the burst's 5 ms piece, kept as
  `default-thread-servers.md` keeps them. Raw results: `docs/research/data/standings-2026-10-23.tar.gz` (benchrun's
  records, the watcher's samples, the after-peak output, the session logs).

## Servers: time

Server cycles a request at 70% load (k), asmalloc's mean over the pads (range); the others
relative to asmalloc's mean, positive is slower, then whether it is resolved.

| Job | asmalloc | jemalloc | tcmalloc per-CPU | mimalloc | glibc |
|---|---|---|---|---|---|
| connect log grpc h2 | 126.6 (123.8 to 128.0) | +8.3% resolved | +5.1% resolved | +4.8% resolved | +15.0% resolved |
| connect log h2 | 125.0 (122.7 to 126.1) | +8.2% resolved | +4.8% resolved | +4.4% resolved | +14.9% resolved |
| connect log | 118.4 (114.9 to 120.0) | +9.5% resolved | +7.0% resolved | +7.0% resolved | +17.9% resolved |
| connect store 1M (1.4 GiB live) | 363.1 (361.6 to 365.0) | +8.0% resolved | +14.4% resolved | +23.1% resolved | +50.5% resolved |
| orders axum | 67.9 (67.2 to 68.6) | +2.7% resolved | +3.6% resolved | +8.0% resolved | +25.8% resolved |
| orders axum cache 2G | 67.9 (66.8 to 68.7) | +2.5% resolved (overlap) | +3.5% resolved | +7.4% resolved | +16.9% resolved |
| orders actix | 55.8 (54.4 to 56.8) | +2.6% resolved (overlap) | +2.1% inside | +9.5% resolved | +27.7% resolved |
| orders axum h2 | 61.1 (58.5 to 64.8) | -0.6% inside | +1.5% inside | +2.5% inside | +37.7% resolved |
| sozu proxy | 92.7 (90.7 to 93.6) | +1.1% inside | +0.5% inside | +0.2% inside | -0.4% inside |
| miniredis | 36.0 (35.6 to 36.4) | +0.6% inside | +1.0% inside | +0.1% inside | -0.0% inside |
| rpc log c64 (kernel suite) | 122.7 (119.8 to 125.0) | +2.9% resolved | -0.4% inside | -0.5% inside | +19.6% resolved |
| rpc echo c64 (kernel suite) | 45.9 (45.6 to 46.0) | +0.1% inside | +0.3% inside | +1.8% resolved | +6.7% resolved |

- **The order service over HTTP/2 has two modes.** asmalloc's pad 64 ran 64.4 to 66.4k in three
  runs of three and pad 48 63.5 to 64.6k in two of three; the other pads 58.4 to 60.7k. The slow
  runs retire 4 to 6% more instructions a request (108.8 to 111.0k against 104.3 to 106.0k)
  and have a higher p99 (39 us against 32 to 37), so it is a mode of the program's work, not a
  stall on the same work. tcmalloc shows it too (pads 32 to 64, up to 67.7k, 8.7% range), jemalloc
  and mimalloc not in these runs. Which mode a build falls into follows its placement; why it
  costs instructions was not looked for. Over the pads it leaves the job level.
- **rpc log c64**: the kernel suite's single build has asmalloc 4% behind mimalloc; over the
  pads it is level with mimalloc and tcmalloc, 2.9% ahead of jemalloc. The kernel table's RPC
  rows read with that in mind.
- Sozu and mini-redis spend little of their time in the allocator; every allocator, glibc
  included, is inside every other's range.

## Servers: tails and memory

p50, p99 and p99.9 in us at 70% load; medians over runs (asmalloc 10 to 15 runs a job, the
others 10 to 15, glibc 5). Resident MiB: the peak under load, and 10 s after the connections
close.

| Job | asmalloc | jemalloc | tcmalloc per-CPU | mimalloc | glibc | Peak MiB asm / je / tc / mi / glibc | After close |
|---|---|---|---|---|---|---|---|
| connect log grpc h2 | 29.6 / 36.5 / 38.3 | 31.2 / 38.0 / 39.8 | 30.5 / 37.3 / 39.1 | 30.1 / 37.2 / 39.8 | 33.0 / 39.3 / 41.3 | 12 / 16 / 14 / 25 / 8 | 12 / 7 / 14 / 25 / 8 |
| connect log h2 | 29.4 / 36.5 / **45.1** | 31.0 / 37.9 / 39.8 | 30.4 / 37.3 / 39.4 | 30.0 / 37.1 / 39.0 | 32.5 / 39.2 / 41.4 | 12 / 15 / 14 / 25 / 8 | 12 / 7 / 14 / 25 / 8 |
| connect log | 27.1 / 29.9 / 34.0 | 28.1 / 31.5 / 35.0 | 27.6 / 31.0 / 34.2 | 27.6 / 30.9 / 34.6 | 30.3 / 33.3 / 36.1 | 10 / 10 / 14 / 23 / 9 | 10 / 8 / 14 / 23 / 7 |
| connect store 1M | 84 / 120 / 124 | 89 / 129 / 134 | 92 / 139 / 145 | 97 / 141 / 182 | 131 / 12,917 / 15,867 | 1,494 / 1,770 / 1,527 / 1,547 / 2,164 | 1,436 / 1,468 / 1,527 / 1,532 / 2,164 |
| orders axum | 15.8 / 22.1 / 25.3 | 16.1 / 22.3 / 25.1 | 16.3 / 22.4 / 25.4 | 16.9 / 23.6 / 38.9 | 22.6 / 57.2 / 112.5 | 209 / 206 / 213 / 262 / 333 | 210 / 206 / 213 / 262 / 332 |
| orders axum cache 2G | 15.9 / 22.1 / 25.9 | 16.2 / 22.2 / 25.7 | 16.4 / 22.4 / 26.8 | 17.0 / 24.8 / 37.5 | 20.5 / 30.1 / 4,983 | 2,193 / 2,226 / 2,229 / 2,346 / 3,146 | 2,195 / 2,126 / 2,229 / 2,346 / 3,080 |
| orders actix | 18.4 / 27.4 / 33.4 | 18.5 / 26.6 / 33.5 | 18.4 / 25.8 / 33.1 | 18.9 / 38.6 / 70.0 | 28.6 / 61.0 / 76.0 | 211 / 207 / 214 / 273 / 368 | 211 / 204 / 214 / 271 / 366 |
| orders axum h2 | 28.5 / 36.0 / 41.6 | 28.4 / 33.8 / 41.9 | 28.4 / 35.8 / 42.0 | 28.4 / 36.6 / 77.6 | 28.5 / 42.8 / 75.8 | 210 / 207 / 213 / 262 / 395 | 210 / 205 / 213 / 262 / 394 |
| sozu proxy | 31.4 / 55.5 / 77.6 | 31.3 / 54.0 / 68.6 | 31.2 / 54.0 / 74.3 | 31.8 / 54.1 / 74.6 | 31.7 / 55.6 / 73.9 | 25 / 19 / 43 / 57 / 16 | 25 / 19 / 43 / 57 / 16 |
| miniredis | 12.3 / 21.1 / 23.1 | the same | the same | the same | the same | 48 / 43 / 43 / 54 / 35 | 48 / 37 / 43 / 46 / 35 |

- **Connect over HTTP/2, p99.9.** 13 of asmalloc's 15 runs are above jemalloc's highest
  (40.9 us), at every pad (33.6 to 50.8), while its p50 and p99 are the lowest. It is real for
  this job. Switching the background thread off (`ASMALLOC_BACKGROUND=0`, against a dummy
  variable of the same length, three pads) does not change it: 37.6 to 48.6 off, 37.0 to 37.9
  for the dummy, 37.9 to 48.5 on, the same spread. At the last standings asmalloc's p99.9 here
  was 38.9 (38.2 to 47.6, six runs). gRPC over the same service and transport is not affected
  (38.3). At saturation the same job's p99.9 is 853 us against 760 to 821 for the incumbents
  (below). Not diagnosed.
  **2026-10-24:** diagnosed in `h2-tail.md`. The slow requests are server CPUs waking from C2 or
  C3, which the faster server reaches more often because it idles longer at the same rate. In
  one binary, slowing the handler to jemalloc's cycles a request brings p99.9 to 40 us;
  asmalloc at 68k to 80k requests a second has 37 to 39 us, and jemalloc at 52k has 53 to 56.
  It is not the allocator's work, and this row's p99.9 does not compare allocators.
- On the small servers asmalloc holds 2 to 5 MiB more than glibc and jemalloc after close
  (Sozu 25 against 16 to 19; mini-redis 48 against 35 to 46). These are small heaps.

**At saturation** (closed loop; three rounds of single builds), requests a second and p99.9
(us):

| Job | asmalloc | jemalloc | tcmalloc per-CPU | mimalloc | glibc |
|---|---|---|---|---|---|
| connect log grpc h2 | 195,485 / 747 | 177,042 / 859 | 181,759 / 803 | 190,017 / 772 | 148,710 / 995 |
| connect log h2 | 198,602 / **853** | 179,015 / 821 | 183,998 / 794 | 192,002 / 760 | 152,285 / 964 |
| connect log | 211,464 / 703 | 192,078 / 775 | 190,856 / 781 | 198,935 / 749 | 177,130 / 847 |
| connect store 1M | 65,604 / 2,522 | 58,891 / 2,903 | 55,855 / 2,965 | 51,903 / 3,183 | 42,064 / 7,865 |
| orders axum | 384,770 / 384 | 374,092 / 395 | 372,432 / 398 | 351,091 / 423 | 287,601 / 520 |
| orders axum cache 2G | 384,209 / 385 | 372,613 / 397 | 370,224 / 401 | 355,710 / 418 | 294,911 / 509 |
| orders actix | 436,623 / 159 | 430,396 / 162 | 433,256 / 161 | 398,470 / 222 | 339,560 / 220 |
| orders axum h2 | 307,080 / 256 | 304,821 / 254 | 306,063 / 250 | 304,074 / 258 | 233,932 / 41,294 |
| sozu proxy | 271,576 / 293 | 272,787 / 326 | 273,858 / 277 | 272,484 / 294 | 271,416 / 338 |
| miniredis | 733,507 / 167 | 733,467 / 166 | 730,421 / 168 | 732,889 / 166 | 733,927 / 167 |

**Large heaps at rest and after a silence** (`servers-thread`, three rounds, asmalloc six;
anonymous MiB; the slowest request due in the first 5 ms of a burst after 30 s of silence,
median and range of runs):

| | asmalloc | jemalloc | tcmalloc per-CPU | mimalloc | glibc |
|---|---|---|---|---|---|
| store: peak under load | 1,453 | 1,763 | 1,520 | 1,543 | 2,076 |
| store: a minute after close | 1,431 | 1,733 | 1,520 | 1,537 | 2,077 |
| store: burst, slowest of first 5 ms, us | 562 (504 to 1,018) | 331 (271 to 387) | 299 (226 to 423) | 464 (385 to 566) | 556 (451 to 703) |
| orders 2G: a minute after close | 2,186 | 2,220 | 2,222 | 2,339 | 3,141 |
| orders 2G: burst, slowest of first 5 ms, us | 141 (119 to 173) | 131 (114 to 238) | 137 (127 to 145) | 333 (222 to 354) | 332 (321 to 341) |

## Meilisearch and DataFusion

Meilisearch (`servers-thread`, 930 requests a second, six runs of asmalloc and three of the
others, single builds):

| | asmalloc | jemalloc | tcmalloc per-CPU | mimalloc | glibc |
|---|---|---|---|---|---|
| server cycles a request at 70% | 16.96M | 17.41M | 17.27M | 17.94M | 18.09M |
| p50 / p99 / p99.9, ms | 1.7 / 27.4 / 42.6 | 1.9 / 34.7 / 48.3 | 1.9 / 34.6 / 52.5 | 2.2 / 38.1 / 60.1 | 2.4 / 39.1 / 55.4 |
| page faults a request | 88 | 42 | 0.1 | 21 | 5 |
| heap under load, mean / peak, MiB | 870 / 949 | 750 / 781 | 967 / 1,039 | 1,575 / 1,734 | 1,237 / 1,302 |
| heap a minute after close, MiB | 661 | 674 | 933 | 1,324 | 1,239 |

DataFusion (TPC-H SF3, four threads, six rounds, medians):

| | asmalloc | jemalloc | tcmalloc per-CPU | mimalloc | glibc |
|---|---|---|---|---|---|
| Gcycles | 69.42 | 72.12 | 67.30 | 69.70 | 70.04 |
| cgroup peak, MiB | 865 (855 to 865) | 866 (838 to 871) | 997 | 1,695 (1,683 to 2,383) | 882 (839 to 938) |
| page faults | 1.03M | 1.02M | 0.46M | 0.12M | 0.80M |

## tokio spawn

The kernel suite's jobs (tokio 1.54), ns a task, medians of ten rounds: four threads asmalloc
95.8, jemalloc 94.2, tcmalloc per-thread 95.0, per-CPU 96.1, mimalloc 97.2, glibc 97.9; eight
threads asmalloc 187.1, jemalloc 177.3, tcmalloc per-thread 178.2, glibc 185.2, mimalloc
188.5, per-CPU 187.7. asmalloc is 1.7% and 5.5% behind jemalloc, single builds of a
placement-sensitive job (`spawn-gap.md`).

## Kernel suite

Cost against the best incumbent (the incumbent with the lowest median; the gap the median of
per-round ratios; "level" when its 95% bootstrap interval holds no difference); asmalloc's own
median against the `efb9cfa` session's (positive is slower; across sessions, so drift is in
it); cgroup peak against the smallest incumbent.

| Job | Best incumbent | Gap | asmalloc against `efb9cfa` | Peak MiB, asm / smallest |
|---|---|---|---:|---|
| cache-scratch 1 (preload) | jemalloc | level (+1%) | +0.5% | 1.0 / glibc 0.8 |
| cache-scratch 8 (preload) | mimalloc | level (+1%) | +5.7% | 2.8 / glibc 2.5 |
| cfrac (preload) | mimalloc | behind 1% | -0.1% | 1.7 / jemalloc 1.4 |
| churn live=100 small | jemalloc | ahead 37% | -4.9% | 0.8 / glibc 0.5 |
| churn live=100k mixed | tcmalloc-percpu | ahead 12% | -0.9% | 34.8 / tcmalloc-thread 34.5 |
| churn live=100k small | jemalloc | ahead 24% | -3.2% | 12.2 / jemalloc 11.7 |
| churn live=100k small (preload) | mimalloc | ahead 7% | -2.5% | 12.2 / jemalloc 12.0 |
| churn live=10M small | jemalloc | ahead 15% | -1.1% | 1069.8 / jemalloc 1091.5 |
| churn live=10k small | jemalloc | ahead 32% | -3.2% | 2.1 / glibc 1.7 |
| churn live=1M small | jemalloc | ahead 16% | -1.0% | 110.5 / jemalloc 110.2 |
| churn live=1k small | jemalloc | ahead 37% | -4.6% | 1.0 / glibc 0.6 |
| espresso (preload) | mimalloc | ahead 1% | -0.7% | 2.8 / glibc 1.3 |
| exchange t2 sizes=mixed local | tcmalloc-percpu | ahead 9% | -0.8% | 5.6 / tcmalloc-percpu 4.0 |
| exchange t2 sizes=small local | tcmalloc-percpu | ahead 46% | -2.6% | 3.0 / snmalloc 1.5 |
| exchange t2 sizes=small split | jemalloc | ahead 36% | -8.4% | 3.6 / glibc 1.3 |
| exchange t4 sizes=small local | tcmalloc-percpu | ahead 46% | -1.9% | 5.0 / snmalloc 2.9 |
| exchange t8 sizes=small both | jemalloc | ahead 39% | -6.7% | 9.2 / tcmalloc-percpu 5.0 |
| forkjoin t4 local | tcmalloc-percpu | ahead 23% | -1.6% | 16.4 / tcmalloc-percpu 15.9 |
| forkjoin t8 both | jemalloc | ahead 7% | -0.9% | 19.4 / tcmalloc-percpu 17.1 |
| json 32Mi | mimalloc-crate | ahead 2% | -1.4% | 353.7 / mimalloc-crate 355.2 |
| json 32Mi (preload) | mimalloc | behind 2% | -1.0% | 354.7 / jemalloc 358.5 |
| largepair 64Ki+1Mi | glibc | behind 520% | +7.2% | 0.8 / glibc 0.5 |
| larson 8 (preload) | mimalloc | behind 9% | +4.3% | 77.0 / tcmalloc-thread 53.1 |
| larson-sized 8 (preload) | mimalloc | behind 12% | +1.6% | 78.8 / tcmalloc-thread 54.2 |
| map btree 1M | mimalloc-sized | behind 8% | +0.0% | 123.5 / jemalloc 125.2 |
| map hash 1M | mimalloc-crate | behind 21% | -1.0% | 204.5 / jemalloc 183.7 |
| mpmc p1 c3 sizes=small local all-alloc | jemalloc | ahead 38% | -0.8% | 4.0 / glibc 1.5 |
| mpmc p1 c3 sizes=small local free-only | tcmalloc-percpu | ahead 46% | -0.0% | 4.1 / glibc 1.5 |
| mpmc p3 c1 sizes=small local all-alloc | tcmalloc-percpu | ahead 37% | -2.8% | 5.0 / glibc 2.0 |
| mpmc p3 c1 sizes=small local free-only | tcmalloc-percpu | ahead 37% | -3.3% | 5.0 / glibc 2.1 |
| mpmc p4 c4 sizes=small both all-alloc | jemalloc | ahead 45% | -0.5% | 8.3 / glibc 2.8 |
| mstress 8 (preload) | mimalloc | behind 7% | +1.9% | 145.8 / jemalloc 142.1 |
| pipeline t2 sizes=large objects=1M inflight=512 local all-alloc | tcmalloc-thread | behind 71% | +1.8% | 2.8 / tcmalloc-thread 3.7 |
| pipeline t2 sizes=mixed local all-alloc | tcmalloc-percpu | ahead 15% | -1.0% | 5.7 / tcmalloc-thread 3.1 |
| pipeline t2 sizes=s64 local all-alloc | jemalloc | ahead 53% | -2.0% | 1.5 / glibc 1.0 |
| pipeline t2 sizes=small local all-alloc | tcmalloc-percpu | ahead 50% | -2.3% | 3.0 / snmalloc 1.2 |
| pipeline t2 sizes=small local all-alloc (preload) | tcmalloc-percpu | ahead 44% | +0.6% | 2.5 / glibc 1.2 |
| pipeline t2 sizes=small local free-only | tcmalloc-percpu | ahead 50% | -2.2% | 3.0 / glibc 1.2 |
| pipeline t2 sizes=small split all-alloc | jemalloc | ahead 43% | +0.8% | 3.5 / glibc 1.0 |
| pipeline t2 sizes=small split free-only | tcmalloc-percpu | ahead 57% | +1.9% | 3.6 / glibc 1.0 |
| pipeline t2 x2 sizes=small local all-alloc | jemalloc | ahead 55% | -2.5% | 4.7 / glibc 2.1 |
| pipeline t2 x2 sizes=small split all-alloc | jemalloc | ahead 53% | -2.4% | 5.0 / glibc 2.0 |
| pipeline t2 x3 sizes=small both all-alloc | jemalloc | ahead 47% | -1.0% | 6.7 / glibc 3.0 |
| pipeline t2 x4 sizes=small both all-alloc | jemalloc | ahead 45% | -1.1% | 8.6 / snmalloc 4.0 |
| pipeline t4 sizes=small local all-alloc | jemalloc | ahead 22% | -6.0% | 5.0 / glibc 2.8 |
| pipeline t8 sizes=small both all-alloc | jemalloc | ahead 20% | -16.6% | 9.9 / glibc 5.5 |
| rpc echo c64 | tcmalloc-percpu | ahead 0% | +0.2% | 6.5 / glibc 3.4 |
| rpc log c256 | mimalloc | behind 4% | -0.5% | 15.9 / glibc 10.0 |
| rpc log c64 | mimalloc | behind 4% | -0.3% | 10.1 / glibc 5.5 |
| sh8bench 16 (preload) | mimalloc | behind 30% | +0.1% | 129.7 / tcmalloc-percpu 119.3 |
| spawn t4 local | jemalloc | behind 1% | -26.0% | 2.9 / glibc 1.6 |
| spawn t8 both | jemalloc | behind 5% | -25.5% | 5.8 / glibc 2.9 |
| tree depth=20 | mimalloc | ahead 3% | +3.9% | 50.2 / glibc 64.5 |
| vecgrow 1x256Mi | mimalloc-crate | ahead 48% | +0.2% | 258.5 / glibc 257.0 |
| vecgrow 64x4Mi | mimalloc | behind 23% | +0.1% | 277.7 / mimalloc 265.0 |
| xmalloc-test 4+4 (preload) | tcmalloc-percpu | ahead 41% | +2.3% | 36.8 / tcmalloc-thread 33.9 |

The RPC rows are single builds; over placement rpc log c64 is level (above).

## After a peak, one thread

`quiet live=2M sizes=small secs=12 after=burst`, native, CPU 2, anonymous resident MiB, median
of three runs. A quiet program with one thread keeps its memory after a peak: a documented
limit that matches glibc (`docs/design.md`, the "So there is a background thread" paragraph).

| Probe, allocator | 1.5 s | 2.5 | 4.5 | 6.5 | 8.5 | 11.9 | peak |
|---|---:|---:|---:|---:|---:|---:|---:|
| burst-idle, asmalloc | 215.0 | 215.1 | 93.0 | 48.6 | 33.0 | 33.0 | 216 |
| burst-idle, jemalloc | 141.5 | 141.5 | 112.5 | 45.2 | 4.5 | 4.5 | 219 |
| burst-idle, mimalloc | 226.2 | 226.2 | 226.2 | 226.2 | 14.8 | 16.2 | 227 |
| burst-idle, glibc / tcmalloc | 208.5 / 210.9 | | | | | 208.5 / 210.9 | 240 / 224 |
| burst-small, asmalloc | 215.0 | 215.0 | 215.0 | 215.0 | 215.0 | 215.0 | 216 |
| burst-small, jemalloc | 183.0 | 86.5 | 86.5 | 51.2 | 4.6 | 4.6 | 218 |
| burst-small, mimalloc | 226.2 | 226.2 | 54.7 | 54.7 | 54.7 | 34.2 | 227 |
| burst-small, glibc / tcmalloc | 208.5 / 210.9 | | | | | 208.5 / 210.9 | 240 / 224 |

asmalloc holds 6.5 MiB (3%) more than glibc on burst-small and has a 24 MiB lower peak.

## Where asmalloc is behind

On servers, nowhere on time. Elsewhere, by how much:

- **Connect over HTTP/2, p99.9:** 45 us against 39 to 41 at 70% load; 853 against 760 to 821
  us at saturation. New since the last standings, not diagnosed.
- **The log store's first requests after a silence:** 0.56 ms against 0.30 to 0.46 for the
  slowest of the first 5 ms. Known (`purge-off-path.md`, the idle drains).
- **Meilisearch's heap under load:** 949 MiB peak against jemalloc's 781, and twice its page
  faults; at rest it is below jemalloc.
- **DataFusion:** 3% behind per-CPU tcmalloc on time.
- **tokio spawn:** 1% (four threads) and 5% (eight) behind jemalloc.
- **Kernel jobs**: large blocks returned and faulted back (HashMap 21%, 64 Vecs grown to 4 MiB
  23%); large objects crossing threads (71%); `largepair` (520%, glibc's top-of-heap case);
  BTreeMap 8%; the C programs (sh8bench 30%, larson 9 to 12%, mstress 7%, json preloaded 2%,
  cfrac 1%).

## What changed since the last standings

The last standings measured `efb9cfa` (2026-10-20/21) with single builds of the servers and
tokio 1.53.3.

- **Servers moved ahead.** On Connect over HTTP/2 and gRPC asmalloc was level with per-CPU
  tcmalloc; it is now 4.8 to 5.1% ahead of it, resolved. Over the same placement sweep gRPC h2
  went from 129.9k to 126.6k cycles a request (the lean `realloc` took about 2% of it,
  `lean-realloc.md`). Connect over HTTP/1.1 121.0k to 118.4k, the order service on axum 69.5k
  to 67.9k and on actix 56.8k to 55.8k (single builds then, sweeps now). The order service over
  HTTP/2 (60.7k then, 61.1k now with its two modes) and mini-redis (35.9k, 36.0k) did not move.
- **The large-heap servers, last measured before the hold, bounded purge and the thread
  (`servers-large.md`):** the store and the 2 GiB order service are ahead on time and the
  smallest or level on memory; Meilisearch's heap at rest is now below jemalloc's (661 against
  674 MiB; 893 against 747 in `servers-meilisearch.md`).
- **tokio spawn** went from 29 and 41% behind glibc to 1 and 5% behind jemalloc: the suite moved
  to tokio 1.54, which pads the scheduler's idle state, and the strided first fill separates the
  run-queue headers (`spawn-gap.md`).
- **Kernel suite:** 38 ahead, 2 level, 15 behind (37, 3, 15); geomean 0.803 (0.823). sh8bench
  went from 56 to 30% behind with asmalloc's time unchanged.
- **Moved the wrong way:** Connect over HTTP/2's p99.9 (38.9 to 45.1 us, above). On the kernel
  suite asmalloc's own time rose by more than 2% on larson (+4.3%, now 9% behind from 4%),
  `largepair` (+7.2%), `tree depth=20` (+3.9%, a placement job) and cache-scratch 8 (+5.7%,
  a 70 ms wall-time job); these are across sessions and were not re-run head to head.
