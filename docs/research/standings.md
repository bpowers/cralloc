# Standings against the incumbents

**On servers cralloc does the least work: ahead or level on every job, behind on none.** Over ten
server jobs, the others' cycles a request against cralloc's, geometric mean (geometric standard
deviation across jobs): jemalloc 1.046 (x1.034), per-CPU tcmalloc 1.045 (x1.036), mimalloc 1.065
(x1.053), glibc 1.197 (x1.130). The gap is widest where the allocator is the largest share of the
work, and vanishes on the two jobs that barely allocate.

- **Tails.** The lowest p50 and p99 on nearly every server job, with the CPUs' deep idle states off.
  The real exception is the first few milliseconds after a long silence, where jemalloc's slowest
  request on the log store takes half of cralloc's.
- **Programs.** Level, within a few percent either way. On DataFusion and tpchgen cralloc spends the
  fewest user cycles (tcmalloc ties it on DataFusion) and loses what it loses in the kernel, to page
  faults.
- **Memory.** On large heaps under load it holds the least: jemalloc 1.10, tcmalloc 1.14, mimalloc
  1.42 and glibc 1.51 times cralloc's heap. It loses in troughs (0.95 against jemalloc's 0.66 GiB at
  2 GiB live) and on small heaps at rest, by a few MiB.
- **For the design.** The request path is not where the next gain is. What is left is page faults
  from large blocks returned and taken back, and where objects land after a fall in live data.

Latest session: 2026-10-27 to 28, cralloc at `2ff7322`, on the reference host (Ryzen 9 9950X, Linux
7.2.9). Main has since gained two memory rules (`607ef11`, `ab6fe66`), measured alone: they leave
the 2 GiB order service and the steady cache level, and take the 2 GiB trough from 1.11 to 0.95 GiB.
"What the latest session did not run", below, dates every older number. [workloads.md](workloads.md)
defines the jobs; [measuring.md](measuring.md) gives the method and how far to trust a difference.

## Servers: time

Thousands of server cycles a request at 70% of the slowest allocator's saturated rate, and each
allocator against cralloc. `*` marks a difference inside the range of code placements; the other
differences are resolved.

| Job | cralloc | jemalloc | tcmalloc | mimalloc | glibc |
|---|---:|---:|---:|---:|---:|
| Connect log service, 3 jobs | 118 to 126 | +8.8 to +9.6% | +5.8 to +6.9% | +4.8 to +6.5% | +15.4 to +17.6% |
| Log store, 1.4 GiB live | 366 | +7.0% | +13.2% | +21.4% | +48.5% |
| Order service, 4 jobs | 56 to 68 | +2.1 to +3.1% | +1.8* to +4.6% | +5.7 to +8.4% | +16.8 to +40.5% |
| Sozu proxy | 93 | -0.3%* | -0.8%* | -0.6%* | -1.2%* |
| mini-redis | 36 | +1.0% | +1.0% | +0.2%* | +0.3%* |
| **Geomean over the ten** | | **1.046 (x1.034)** | **1.045 (x1.036)** | **1.065 (x1.053)** | **1.197 (x1.130)** |
| Instructions, the same ten | | 1.051 (x1.034) | 1.060 (x1.047) | 1.046 (x1.029) | 1.137 (x1.105) |

2026-10-27 to 28 at `2ff7322`. Each figure is the mean over five code placements of the median at
each: two rounds a placement, three on HTTP/2 Connect and HTTP/2 orders, one for glibc. Geomeans
computed across jobs from the raw records. The ten are not independent: three are one service and
four are one program.

- **The gap to jemalloc and tcmalloc is instructions, not stalls.** Their instructions a request
  rise more than their cycles, so their instructions per cycle (IPC) are level or better. mimalloc
  and glibc lose about 70% of their gap in instructions and the rest in stalls. glibc on the order
  service is the clearest stall: +25.3% cycles on +8.3% instructions, IPC 1.34 against 1.55
  ([program-effects.md](program-effects.md)).
- **Three more servers barely move it.** Over thirteen jobs: jemalloc 1.053 (x1.037), tcmalloc 1.042
  (x1.034), mimalloc 1.068 (x1.050), glibc 1.206 (x1.126). The three:

| Job | cralloc | jemalloc | tcmalloc | mimalloc | glibc | Evidence |
|---|---:|---:|---:|---:|---:|---|
| Apollo Router | 2.72M | +6.1% | +1.3% | +3.0% | +37.7% | five placements, `46126f0`, 2026-10-24 |
| SurrealDB, 1.7 GiB | 2.02M | +13.6% | +7.0% | +12.6% | +27.2% | tcmalloc three placements, others single builds, `46126f0`, 2026-10-25 |
| Meilisearch | 16.76M | +4.0% | +1.8% | +7.2% | +7.1% | single builds, `2ff7322` |

- **Qdrant's cycles do not rank allocators:** allocation is 2% of them, and runs differ by 5% in
  work.
- **Closed-loop RPC**, five placements at `2ff7322`: `rpc log c64` jemalloc +3.0%, tcmalloc -0.1%*,
  mimalloc -0.2%*, glibc +20.0%; `rpc echo c64` +0.6%*, +0.4%*, +2.4%, +6.8%.
- **Saturated rates agree.** cralloc's over the other's, geomean over ten jobs, three rounds of
  single builds: jemalloc 1.048 (x1.035), tcmalloc 1.054 (x1.050), mimalloc 1.063 (x1.070), glibc
  1.241 (x1.138). The highest rate on every job that allocates much but the order service over
  HTTP/2, where all but glibc are level within 2%.

## Servers: tails

Each allocator's percentile against cralloc's at 70% load, geomean over the ten jobs: the median
over runs (10 to 15 a job, glibc 5). C2 and C3 were off on CPUs 0 to 3, and every run has zero deep
idle entries, so none of this ranks idle states.

| | jemalloc | tcmalloc | mimalloc | glibc |
|---|---:|---:|---:|---:|
| p50 | 1.028 (x1.023) | 1.029 (x1.026) | 1.038 (x1.044) | 1.199 (x1.187) |
| p99 | 1.028 (x1.027) | 1.035 (x1.038) | 1.092 (x1.119) | 1.914 (x3.11) |
| p99.9 | 1.029 (x1.030) | 1.024 (x1.048) | 1.332 (x1.340) | 3.77 (x7.2) |

cralloc's own p99.9: Connect log service 31 to 38 us, order service 24 to 40, log store 126, Sozu
78, mini-redis 23; glibc's reach 9 ms on two jobs. Exceptions, all small: on actix jemalloc's p99
and p99.9 are 0.4 and 0.7 us lower; on Sozu glibc's p50 and p99 are lower (30.8 and 48.1 against
31.8 and 54.2 us); Meilisearch's p99.9 is 44.6 ms for cralloc and jemalloc. [servers.md](servers.md)
says what sets these tails.

## Memory

Each allocator's memory against cralloc's, geomean by kind of job. Below 1 means the other holds
less.

| Job set, measure | jemalloc | tcmalloc | mimalloc | glibc |
|---|---:|---:|---:|---:|
| Large-heap servers, peak through 70% load (3) | 1.099 (x1.078) | 1.136 (x1.137) | 1.415 (x1.479) | 1.506 (x1.065) |
| The same, a minute after close | 1.074 (x1.091) | 1.156 (x1.165) | 1.333 (x1.358) | 1.562 (x1.120) |
| Small-heap servers, 10 s after close (8) | 0.778 (x1.239) | 1.130 (x1.215) | 1.579 (x1.345) | 0.934 (x1.623) |
| Batch programs, peak (4) | 0.953 (x1.070) | 0.949 (x1.169) | 1.674 (x1.424) | 0.948 (x1.119) |
| Kernel jobs that peak at 30 MiB or more (14) | 1.075 (x1.112) | 1.120 (x1.318) | 1.352 (x1.285) | 1.158 (x1.148) |

Servers `2ff7322`; batch programs `2ff7322` and, for ruff and boa, `515412e`; kernel jobs `80643d2`.

The three large-heap servers, anonymous MiB, peak through 70% load and a minute after close (three
rounds, cralloc six runs; Meilisearch's heap sits beside a mapped store, 3.5 to 4.6 GiB resident):

| | cralloc | jemalloc | tcmalloc | mimalloc | glibc |
|---|---:|---:|---:|---:|---:|
| Log store | 1,432 / 1,432 | 1,740 / 1,740 | 1,520 / 1,520 | 1,541 / 1,541 | 2,066 / 2,071 |
| Order service, 2 GiB | 2,186 / 2,186 | 2,220 / 2,220 | 2,222 / 2,222 | 2,340 / 2,341 | 3,141 / 3,141 |
| Meilisearch | 732 / 652 | 787 / 655 | 995 / 933 | 1,801 / 1,339 | 1,206 / 1,195 |

Meilisearch's first sample reads 885 to 987 MiB for cralloc, left from indexing (jemalloc 655 to
1,006), and about 690 within a second. Its page faults a request: 42 for cralloc and jemalloc, 23
for mimalloc, 1 for glibc, 0.1 for tcmalloc.

Long runs, resident heap over the bytes the program holds:

| Job | cralloc | jemalloc | tcmalloc | mimalloc | glibc | Evidence |
|---|---:|---:|---:|---:|---:|---|
| Steady cache, 2 GiB, 15G operations | 1.105 | 1.148 | 1.127 | 2.97 | 1.27 | `5536610`, 2026-10-24 |
| Order service, 10 GiB, under load | 1.073 | 1.095 | 1.095 | 1.149 | | `bf837db`, 2026-10-26 |
| Lifetime mix, 2 GiB, peaks | 1.10 | 1.12 | 1.13 | 1.15 | 1.09 | `bf837db`, 2026-10-27 |

A 10.6-hour soak of Qdrant beside jemalloc drifted in neither: 155 MiB at the quiet points against
178 (`8e9d097`, 2026-10-28). SurrealDB's 750k records peak at 1,723 MiB against 1,800 to 1,931
(`46126f0`). [memory.md](memory.md) has the mechanisms.

## Programs

Gcycles of all threads over the work, with user and kernel cycles in brackets. Six rounds of
DataFusion, four of tpchgen; medians. An allocator's DataFusion runs lie within about 1%, its
tpchgen runs within 0.5 to 4.6%.

| | cralloc | jemalloc | tcmalloc | mimalloc | glibc |
|---|---:|---:|---:|---:|---:|
| DataFusion, 4 threads | 70.62 (64.04, 6.60) | 72.26 (66.04, 6.22) | 67.58 (64.02, 3.53) | 69.96 (66.83, 3.10) | 70.51 (65.20, 5.26) |
| DataFusion, faults; peak MiB | 1.09M; 857 | 1.03M; 861 | 0.45M; 999 | 0.12M; 1,659 | 0.80M; 933 |
| tpchgen, 8 threads, preloaded | 25.27 (23.81, 1.45) | 24.79 (23.91, 0.85) | 25.04 (24.30, 0.74) | 24.60 (24.02, 0.58) | 25.16 (23.91, 1.22) |
| tpchgen, faults; peak MiB | 337k; 559 | 182k; 567 | 156k; 540 | 78k; 813 | 282k; 518 |
| boa, s | 8.04 | 8.58 | 8.36 | 8.18 | 8.51 |
| ruff, 4 threads, s | 1.064 | 1.101 | 1.097 | 1.049 | 1.140 |
| Geomean of the four against cralloc | | 1.026 (x1.030) | 1.004 (x1.034) | 0.992 (x1.016) | 1.030 (x1.034) |

DataFusion and tpchgen `2ff7322`; boa and ruff `515412e`, 2026-10-20, six rounds, wall time.

**All of the loss is the kernel.** On DataFusion cralloc's user cycles equal tcmalloc's and beat the
others; its kernel cycles are 3.07 G over tcmalloc's, against a total gap of 3.04 G. On tpchgen it
has the fewest user cycles and the most kernel cycles.

**The screening** of 34 program configurations, preloaded, three rounds (`515412e`, 2026-10-19):
cycles against cralloc's, jemalloc 1.019 (x1.044), mimalloc 0.995 (x1.067), glibc 1.055 (x1.075). It
predates most of the memory rules, and a preloaded free carries no size. Its worst gaps, not
measured again: 5 to 13% behind mimalloc on rust-analyzer, rhai, egglog, typst and cargo.

**The kernel suite** is a regression check, not a target (`80643d2`, 2026-10-22 to 23, ten rounds of
native jobs, seven of preloaded, medians). Against the best incumbent on each job cralloc is ahead
on 38, level on 2 and behind on 15, and the geomean of its time over the best's is 0.803. The others
against cralloc, by group (the RPC jobs are above):

| Group (jobs) | jemalloc | tcmalloc | mimalloc | glibc |
|---|---:|---:|---:|---:|
| Cross-thread (23) | 2.23 (x1.60) | 2.05 (x1.62) | 3.27 (x1.61) | 17.4 (x1.72) |
| Single-thread (14) | 1.66 (x1.59) | 1.61 (x1.51) | 1.51 (x1.57) | 1.68 (x1.98) |
| Work stealing (4) | 1.09 (x1.17) | 1.16 (x1.16) | 1.37 (x1.37) | 1.98 (x2.00) |
| Rust programs preloaded (3) | 1.56 (x1.23) | 1.55 (x1.21) | 1.59 (x1.85) | 4.29 (x4.40) |
| C programs preloaded (8) | 1.16 (x1.19) | 1.26 (x1.70) | 0.94 (x1.07) | 1.36 (x1.53) |

## Where cralloc is behind, and why

Each cause is the one its own study found; none was profiled again at the latest commit.

- **The first requests after a long silence.** The slowest request due in the first 5 ms of a burst
  after 30 s of silence: the log store 581 us (538 to 638) against jemalloc 279, tcmalloc 359,
  mimalloc 542, glibc 725; the 2 GiB order service 151 against 135, 162, 308, 293. The background
  thread drained the per-CPU caches in the silence and the first requests refill them, about 0.2 ms
  on the store. Left as it is ([servers.md](servers.md)).
- **Troughs.** At a 2 GiB trough cralloc holds 0.95 GiB, jemalloc 0.66 (`ab6fe66`, `d36e0cb`,
  2026-10-28), on the same live slots. Each allocation takes the slot the last free pushed, so
  survivors stay spread over the peak's pages: free slots beside live ones are 273 MiB against 40.
  The rule of 2026-10-28 took about 15% off for 0.6% of cycles on that job. Classes of 128 bytes and
  down do not move ([memory.md](memory.md)).
- **Size mixes that shift, and large buffers.** The old classes' tails stay: 1.53 against jemalloc's
  1.19 at a 384-byte median, and mini-redis rewritten at changing sizes 1.33 against 1.16
  (`5536610`; the tail rule should reach both, not measured). A mix of large buffers ends its phases
  at 1.30, 1.25, 1.45, 1.48 against 1.42, 1.89, 1.29, 1.36 (`0eaf879`; a phase moves up to 0.3
  between runs), with page tables three times the incumbents' ([large-blocks.md](large-blocks.md)).
- **Small heaps at rest.** 2 to 7 MiB over jemalloc 10 s after close (Connect log 10 to 12 against 7
  to 8; Sozu 25 against 18; mini-redis 48 against 37), and the Router 96 to 101 against 78 to 81.
  Fixed footprint, and accepted: small-heap peaks do not lead what we optimize. Qdrant's 483 MiB
  peak and 139 after close (`bf837db`) stand against jemalloc's 449 and 131 (`46126f0`), across
  sessions.
- **A quiet program with one thread keeps its memory after a peak:** 215 MiB against jemalloc's 4.6
  at 12 s; glibc's 208 and tcmalloc's 211 stay too. A documented limit (`docs/design.md`, "So there
  is a background thread").
- **Page faults on batch programs**, as above. Large slots go back and are faulted again. The trim
  of a slot's slack cost about 0.8% on DataFusion and 2.0% on tpchgen for lower peaks. Huge pages
  for large blocks allocated whole would give DataFusion 2.9%, but no server gains and a sparse
  block would pay many times its size ([large-blocks.md](large-blocks.md)).
- **Kernel jobs** (`80643d2`). `largepair`, 520% behind glibc, which serves both from the top of its
  heap, and ahead of everyone else. Large objects crossing threads, 71% behind per-thread tcmalloc:
  a leaf word's line moves between cores each time, and no server makes such a flow. `vecgrow
  64x4Mi`, 23% behind mimalloc: faulting back outgrown slots; keeping them within a count
  (`81437b0`) later took 15% off, for a peak of 327 MiB against 265. `map hash 1M`, 21% behind: a
  quarter first-touch faults, the rest dTLB misses on a table allocated whole. `map btree 1M`, 8%: a
  locked OR per scattered slot on the drop. tokio `spawn`, 1% and 5% behind jemalloc: the sibling
  stride closed most of it.
- **C programs preloaded** (`80643d2`), regression checks: larson 9 to 12% behind mimalloc, through
  libstdc++'s operators, whose export is deferred; mstress 7%; JSON 2%; cfrac 1%. sh8bench's 0.387 s
  against mimalloc's 0.323 is not resolved: mimalloc's runs range from 0.23 to 0.41 s, and a second
  copy of it has a median of 0.248.

## What the latest session did not run

The session was wound down before these ran. Their latest results:

| What | Commit | Date |
|---|---|---|
| Router, SurrealDB, Qdrant, all with deep idle on | `46126f0` | 2026-10-24 to 25; Qdrant's cralloc `bf837db`, 2026-10-27 |
| Meilisearch's saturated rate: cralloc 1,413, jemalloc 1,372, tcmalloc 1,423, mimalloc 1,328 req/s | `d945f62` | 2026-10-21 |
| Long-run memory, all five: steady, shift, lifetime, mini-redis | `5536610` | 2026-10-24 to 25 |
| The large-block mix; heaps of 0.5 to 10 GiB; troughs | `0eaf879`; `bf837db`; `d36e0cb` | 2026-10-25 to 28 |
| Kernel suite, with the 64-Vec, 256 MiB and spawn jobs; memory after a peak | `80643d2` | 2026-10-22 to 23 |
| ruff and boa; the screening | `515412e` | 2026-10-20; 2026-10-19 |

## Open questions

- **Tails with deep idle off** are not measured for the Router, SurrealDB or Qdrant, whose tails are
  in milliseconds.
- **The rules since `2ff7322`.** Measured alone they leave the 2 GiB order service level; the full
  server table has not been run since.
- **One DataFusion run** peaked at 1,454 MiB against 852 to 862 for cralloc's other five, at the
  same faults and cycles. It was the first of its session. Not explained.
- **Placement on the kernel suite.** Its spawn jobs are single builds, so gaps under about 4% there
  are not findings.
