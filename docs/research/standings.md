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
- **Memory.** On the three large-heap servers under load it holds the least: jemalloc 1.10, tcmalloc
  1.14, mimalloc 1.42 and glibc 1.51 times cralloc's heap. It loses in troughs (0.95 against
  jemalloc's 0.66 GiB at 2 GiB live), on Qdrant's peak, and on small heaps at rest, by 2 to 11 MiB.
- **For the design.** The request path is not where the next gain is. What is left is page faults
  from large blocks returned and taken back, and where objects land after a fall in live data.

Latest session: 2026-10-27 to 28, cralloc at `e3753b5`, on the reference host (Ryzen 9 9950X, Linux
7.2.9). Main has since gained the tail walk's question to the kernel (`7bf9ff8`), the clocked pass's
goal (`98b630a`) and the refresh on evidence (`b622714`), and tokio moved to 1.54.1. The refresh
rule, switched in one binary, leaves the 2 GiB order service and the steady cache level and takes
the 2 GiB trough from 1.11 to 0.95 GiB. "What the latest session did not run", below, dates every
older number. [workloads.md](workloads.md) defines the jobs; [measuring.md](measuring.md) gives the
method and how far to trust a difference.

## Servers: time

Thousands of server cycles a request at 70% of the slowest allocator's saturated rate, and each
allocator against cralloc. `*` marks a difference inside the range of code placements; the other
differences are resolved.

| Job | cralloc | jemalloc | tcmalloc | mimalloc | glibc |
|---|---:|---:|---:|---:|---:|
| Connect log service, 3 jobs | 118 to 126 | +8.8 to +9.6% | +5.8 to +6.9% | +4.8 to +6.5% | +15.4 to +17.6% |
| Log store, 1.4 GiB heap | 366 | +7.0% | +13.2% | +21.4% | +48.5% |
| Order service, 4 jobs | 56 to 68 | +2.1 to +3.1% | +1.8* to +4.6% | +5.7 to +8.4% | +16.8 to +40.5% |
| Sozu proxy | 93 | -0.3%* | -0.8%* | -0.6%* | -1.2%* |
| mini-redis | 36 | +1.0% | +1.0% | +0.2%* | +0.3%* |
| **Geomean over the ten** | | **1.046 (x1.034)** | **1.045 (x1.036)** | **1.065 (x1.053)** | **1.197 (x1.130)** |
| Instructions, the same ten | | 1.051 (x1.034) | 1.060 (x1.047) | 1.046 (x1.029) | 1.137 (x1.105) |

2026-10-27 to 28 at `e3753b5`. Each figure is the mean over five code placements of the median at
each: two rounds a placement, three on HTTP/2 Connect and HTTP/2 orders, one for glibc. Geomeans
computed across jobs from the raw records. The ten are not independent: three are one service and
four are one program.

- **Over the ten, most of the gap to jemalloc and tcmalloc is instructions.** Their instructions a
  request rise more than their cycles, so their instructions per cycle (IPC) are level or better. No
  stall shows, though this split cannot exclude one ([program-effects.md](program-effects.md)).
  tcmalloc on the Connect jobs is the exception, losing a fifth to a third of its gap in stalls.
  mimalloc and glibc lose about 70% of their gap in instructions and the rest in stalls. glibc on
  the order service is the clearest stall: +25.3% cycles on +8.3% instructions, IPC 1.34 against
  1.55 ([program-effects.md](program-effects.md)).
- **The order service over HTTP/2 reads as resolved only because no cralloc placement's median fell
  in its slow mode this session (one run of 15 did).** That mode runs 4 to 6% more instructions,
  follows no build and is not explained. On 2026-10-22, with two cralloc placements in it, the job
  was level with jemalloc, tcmalloc and mimalloc (-0.6%, +1.5%, +2.5%, inside the range). tcmalloc
  ran it at pads 0, 16 and 32, and pad 16's median is in its +4.6%.
- **Three more servers barely move it.** Over thirteen jobs: jemalloc 1.053 (x1.037), tcmalloc 1.042
  (x1.034), mimalloc 1.068 (x1.050), glibc 1.206 (x1.126). The three:

| Job | cralloc | jemalloc | tcmalloc | mimalloc | glibc | Evidence |
|---|---:|---:|---:|---:|---:|---|
| Apollo Router | 2.72M | +6.1% | +1.3% | +3.0% | +37.7% | five placements, `1b0acf0`, 2026-10-24 |
| SurrealDB, 1.7 GiB | 2.02M | +13.6% | +7.0% | +12.6% | +27.2% | cralloc and tcmalloc three placements, others single builds, `1b0acf0`, 2026-10-25 |
| Meilisearch | 16.76M | +4.0% | +1.8% | +7.2% | +7.1% | single builds, `e3753b5` |

- **Qdrant's cycles do not rank allocators:** allocation is 2% of them, and runs differ by 5% in
  work.
- **Closed-loop RPC**, five placements at `e3753b5`: `rpc log c64` jemalloc +3.0%, tcmalloc -0.1%*,
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
78, mini-redis 23; glibc's reach 9 ms on two jobs. Exceptions: on actix jemalloc's p99 and p99.9 are
0.4 and 0.7 us lower, tcmalloc's 0.3 and 0.4. On Sozu glibc's p50 and p99 are lower (30.8 and 48.1
against 31.8 and 54.2 us, 11% at p99), mimalloc's p50 and p99.9 (31.5 and 77.8 against 78.1), and
tcmalloc's p99.9 (75.5). glibc's p50 on the order service over HTTP/2 is 28.4 against 28.5.
Meilisearch's p99.9 is 44.6 ms for cralloc and jemalloc. [servers.md](servers.md) says what sets
these tails.

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

Servers `e3753b5`; batch programs `e3753b5` and, for ruff and boa, `5097226`; kernel jobs `c7f5c21`.

The three large-heap servers, anonymous MiB, peak through 70% load and a minute after close (three
rounds, cralloc six runs; Meilisearch's heap sits beside a mapped store, 3.5 to 4.6 GiB resident):

| | cralloc | jemalloc | tcmalloc | mimalloc | glibc |
|---|---:|---:|---:|---:|---:|
| Log store | 1,432 / 1,432 | 1,740 / 1,740 | 1,520 / 1,520 | 1,541 / 1,541 | 2,066 / 2,071 |
| Order service, 2 GiB | 2,186 / 2,186 | 2,220 / 2,220 | 2,222 / 2,222 | 2,341 / 2,341 | 3,141 / 3,141 |
| Meilisearch | 732 / 652 | 787 / 655 | 995 / 933 | 1,801 / 1,339 | 1,206 / 1,195 |

Meilisearch reaches at most 710 to 739 MiB through 70% load against jemalloc's 778 to 790. About 587
MiB of that is the same under both (glibc arenas its C code fills, stacks, the binary), so the row
dilutes the allocator's part. Just before the first request cralloc is still falling from indexing,
at 885 to 896, and a second in holds 677 to 705; jemalloc is already down at 655 to 658, and a
second in has risen to 720 to 744. While indexing, before any load, cralloc peaks at 1,388 to 1,433
against jemalloc's 1,230 to 1,251 (the watcher's cgroup figures, on the first-request clock,
computed from the raw records). Page faults a request: 42 for cralloc and jemalloc, 23 for mimalloc,
1 for glibc, 0.1 for tcmalloc.

Long runs, resident heap over the bytes the program holds:

| Job | cralloc | jemalloc | tcmalloc | mimalloc | glibc | Evidence |
|---|---:|---:|---:|---:|---:|---|
| Steady cache, 2 GiB, 15G operations | 1.105 | 1.148 | 1.127 | 2.97 | 1.27 | `9a11f70`, 2026-10-24 |
| Order service, 10 GiB, under load | 1.073 | 1.095 | 1.095 | 1.149 | | `536dbe8`, 2026-10-26 |
| Lifetime mix, 2 GiB, peaks | 1.10 | 1.12 | 1.13 | 1.15 | 1.09 | `536dbe8`, 2026-10-27 |
| Size shift, median 384 B | 1.36 | 1.19 | | | 1.83 (1.33 in its first cycle) | `1fde89f`, 2026-10-25; glibc's last cycle `9a11f70` |
| mini-redis rewritten, wide phases | 1.15 to 1.18 | 1.16 | | | 1.16 to 1.29 | tail rule, 2026-10-25 to 27 |

A 10.6-hour soak of Qdrant beside jemalloc drifted in neither: 155 MiB at the quiet points against
178 (`967317c`, 2026-10-28). SurrealDB's 750k records peak at 1,723 MiB against 1,800 to 1,931, and
its 1.5M at 3,333 against 3,505 to 3,782 (`1b0acf0`). [memory.md](memory.md) has the mechanisms.

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

DataFusion and tpchgen `e3753b5`; boa and ruff `5097226`, 2026-10-20, six rounds, wall time.

**All of the loss is the kernel.** On DataFusion cralloc's user cycles equal tcmalloc's and beat the
others; its kernel cycles are 3.07 G over tcmalloc's, against a total gap of 3.04 G. On tpchgen it
has the fewest user cycles and the most kernel cycles.

**The screening** of 34 program configurations, preloaded, three rounds (`5097226`, 2026-10-19):
cycles against cralloc's, jemalloc 1.019 (x1.044), mimalloc 0.995 (x1.067), glibc 1.055 (x1.075). It
predates most of the memory rules, and a preloaded free carries no size. Its worst gaps, not
measured again: 3.6 to 10% behind mimalloc in cycles on rust-analyzer, rhai, egglog, typst and
cargo.

**The kernel suite** is a regression check, not a target (`c7f5c21`, 2026-10-22 to 23, ten rounds of
native jobs, seven of preloaded, medians). Against the best incumbent on each job cralloc is ahead
on 38, level on 2 and behind on 15, and the geomean of its time over the best's is 0.803. snmalloc was
among the incumbents only in its Debug configuration ([measuring.md](measuring.md), "An incumbent may
not be what you think"), so a Release build may lower some of those bests. The others against
cralloc, by group (the RPC jobs are above):

| Group (jobs) | jemalloc | tcmalloc | mimalloc | glibc |
|---|---:|---:|---:|---:|
| Cross-thread (23) | 2.23 (x1.60) | 2.05 (x1.62) | 3.27 (x1.61) | 17.4 (x1.72) |
| Single-thread (14) | 1.66 (x1.59) | 1.61 (x1.51) | 1.51 (x1.57) | 1.68 (x1.98) |
| Work stealing (4) | 1.09 (x1.17) | 1.16 (x1.16) | 1.37 (x1.37) | 1.98 (x2.00) |
| Rust programs preloaded (3) | 1.56 (x1.23) | 1.55 (x1.21) | 1.59 (x1.85) | 4.29 (x4.40) |
| C programs preloaded (8) | 1.16 (x1.19) | 1.26 (x1.70) | 0.94 (x1.07) | 1.36 (x1.53) |

## On the M1 Max

**On the kernel suite cralloc leads here as it does on the Ryzen: ahead on 35 jobs, level on 9, behind
on 11 of 55, and its time is 0.826 of the best incumbent's (geomean).** Everything else in this note
is the Ryzen's. The servers and programs have not run here yet, so this says nothing about them.

An Apple M1 Max under Asahi Linux 7.2.12 with 16 KiB pages, at `66dfb90`, 2026-11-08: the generated
routines, the rules in bytes. Three rounds of every job under eight builds, 1,506 runs, 4 left out for
other work. The best incumbent is the best of six: mimalloc v3.5.3 and the crate's, per-CPU tcmalloc,
jemalloc, glibc and snmalloc in its Release configuration. The control is 0.33% from mimalloc at the median
of 63 jobs and 3.1% at the 90th percentile; its largest are modes of mimalloc's, up to 21%. The others
against cralloc, geomean with the geometric standard deviation across jobs:

| Group (jobs) | jemalloc | tcmalloc | mimalloc | glibc | snmalloc |
|---|---:|---:|---:|---:|---:|
| Cross-thread (23) | 2.27 (x1.65) | 1.73 (x1.59) | 3.69 (x1.84) | 20.6 (x1.90) | 2.30 (x1.32) |
| Work stealing (4) | 1.11 (x1.13) | 1.12 (x1.12) | 1.43 (x1.54) | 2.01 (x2.11) | 1.17 (x1.19) |
| RPC (3) | 1.01 (x1.02) | 0.99 (x1.00) | 1.00 (x1.01) | 1.13 (x1.05) | 0.99 (x1.01) |
| Single-thread (14) | 1.40 (x1.37) | 1.30 (x1.30) | 1.46 (x1.44) | 2.24 (x1.85) | 1.18 (x1.46) |
| Rust programs preloaded (3) | 1.60 (x1.31) | 1.35 (x1.14) | 1.38 (x1.54) | 4.39 (x3.69) | |
| C programs preloaded (8) | 1.17 (x1.29) | 1.36 (x1.96) | 1.09 (x1.23) | 1.35 (x1.55) | |
| **Memory**, cgroup peak of the jobs where cralloc reaches 30 MiB (14; snmalloc 9) | 1.04 (x1.15) | 1.11 (x1.31) | 1.56 (x1.47) | 1.10 (x1.15) | 1.15 (x1.32) |

**The cross-thread lead is what the Ryzen showed.** Moving a line between clusters costs what crossing
L3s does ([machine-aarch64.md](machine-aarch64.md)). On one pipeline cralloc takes 4.6 ns an allocation
against per-CPU tcmalloc's 8.6, and across clusters 12.2 against jemalloc's 20.5.

**Where it is behind,** each against its job's best incumbent. "Instructions" is the share of the gap in
cycles, and above 1 means more instructions and fewer stalls; "kernel" is the share in kernel cycles.
Placement is the mean over five pads (0 to 128 bytes), three rounds, two for the RPC jobs.

| Job | Against | Gap | Instructions | Kernel | Faults | Over placements |
|---|---|---:|---:|---:|---:|---|
| One pipeline of 4 to 256 KiB blocks | mimalloc | 148% | 1.55 | 0.0 | level | Not swept: far outside any placement |
| `largepair` | snmalloc | 102% | 0.93 | 0.0 | level | Not swept. glibc takes 17.6 ns where it stays in its top, and 1,611 in the two runs of three where it faults on every pair |
| `json 32Mi` | mimalloc | 8% | 1.31 | 0.26 | 20,129 / 13 | 7.3%, resolved (11.95, against 11.14) |
| `map hash 1M` | the mimalloc crate | 6% | 0.85 | 0.31 | 13,236 / 636 | 5.7%, resolved. Ahead of mimalloc v3.5.3 by 1.1% |
| `sh8bench`, `mstress` | mimalloc | 6%, 4% | over 1 | | 8,714 / 120, 9,146 / 234 | Preloaded, not swept |

- **`json` and `map hash` are mimalloc's huge pages and our back end, not the fast path.** Its heap is
  on 32 MiB pages, so it takes almost no faults, and a quarter to a third of the gap is the kernel's.
  [firestorm.md](firestorm.md) puts the rest of JSON's in the launder and the fill.
- **Level once swept:** `map btree 1M`, 0.4%, its ranges overlapping, and `rpc log c64`, `rpc echo c64` and `spawn t4`, within 0.7%. The Box tree is ahead,
  2.1% over placements.

## Where cralloc is behind, and why

Each cause is the one its own study found; none was profiled again at the latest commit.

- **The first requests after a long silence, on the log store.** The slowest request due in the
  first 5 ms of a burst after 30 s of silence: 581 us (538 to 638) against jemalloc 279, tcmalloc
  359, mimalloc 542, glibc 725. The background thread drained the per-CPU caches in the silence and
  the first requests refill them, about 0.2 ms. Left as it is ([servers.md](servers.md)). The 2 GiB
  order service, where the thread never starts, is level within its range: 151 (136 to 188) against
  jemalloc's 135 (104 to 204), six and three runs, three of the nine flagged for other work and
  kept.
- **Troughs.** At a 2 GiB trough cralloc holds 0.95 GiB, jemalloc 0.66 (`b622714`, `1d73192`,
  2026-10-28), on the same live slots. Each allocation takes the slot the last free pushed, so
  survivors stay spread over the peak's pages: free slots beside live ones are 273 MiB against 40.
  The rule of 2026-10-28 took about 15% off for 0.6% of cycles on that job. Classes of 128 bytes and
  down do not move ([memory.md](memory.md)).
- **A size mix that shifts.** The old classes' tails stay: 1.36 against jemalloc's 1.19 at a
  384-byte median, with the tail rule (`1fde89f`, 2026-10-25). Rewritten mini-redis is no longer
  behind (table above).
- **A mix of large buffers** ends its phases at 1.30, 1.25, 1.45, 1.48 against 1.42, 1.89, 1.29,
  1.36 (`26199ba`; a phase moves up to 0.3 between runs), with page tables three times the
  incumbents' ([large-blocks.md](large-blocks.md)).
- **Qdrant's heap.** Peak 483 and a minute after close 139 MiB (`536dbe8`, 2026-10-27) against
  jemalloc's 449 and 131 (`1b0acf0`, 2026-10-24), across sessions; tcmalloc's peak is 474.
  Small-class pages stranded in huge-page units made most of the first gap, and the tail rule
  returned them.
- **Small heaps at rest.** 2 to 11 MiB over jemalloc 10 s after close (Connect log 10 to 12 against
  7 to 8; Sozu 25 against 18; mini-redis 48 against 37), and the Router 96 to 101 against 78 to 81.
  Fixed footprint, and accepted: small-heap peaks do not lead what we optimize.
- **A quiet program with one thread keeps its memory after a peak.** When its light work stays
  inside the magazines: 215 MiB at 12 s against jemalloc's 4.6, and glibc's 208 and tcmalloc's 211
  stay too. When its bursts cross magazines it falls to 33. A documented limit (`docs/design.md`,
  "The background thread").
- **Page faults on batch programs**, as above. Large slots go back and are faulted again. The trim
  of a slot's slack cost about 0.8% on DataFusion and 2.0% on tpchgen for lower peaks. Huge pages
  for large blocks allocated whole would give DataFusion 2.9%, but no server gains and a sparse
  block would pay many times its size ([large-blocks.md](large-blocks.md)).
- **Kernel jobs** (`c7f5c21`). `largepair`, 520% behind glibc, which serves both from the top of its
  heap, and ahead of everyone else. Large objects crossing threads, 71% behind per-thread tcmalloc:
  a leaf word's line moves between cores each time, and no profiled server makes such a flow.
  `vecgrow 64x4Mi`, 23% behind mimalloc: faulting back outgrown slots; keeping them within a count
  (`ab2cf3d`) later took 15% off, for a peak of 327 MiB against 265. `map hash 1M`, 21% behind
  mimalloc's crate: in its build a quarter first-touch faults and the rest dTLB misses on a table
  allocated whole, and scattered launders on the drop add about a quarter of the job's gap. `map
  btree 1M`, 8%: a locked OR per scattered slot on the drop. tokio `spawn`, 1% and 5% behind
  jemalloc: the sibling stride closed most of it. `rpc log c64` and `c256`, about 4% behind mimalloc
  as single builds: c64 is level over placement, c256 was not swept.
- **C programs preloaded** (`c7f5c21`), regression checks: larson 9 to 12% behind mimalloc, through
  libstdc++'s operators, whose export is deferred; mstress 7%; JSON 2%; cfrac 1%; sh8bench 20 to
  56%, 0.387 s against 0.323 and 0.248 for mimalloc's second copy. There the size is not resolved;
  the sign is.

## What the latest session did not run

The latest result of each:

| What | Commit | Date |
|---|---|---|
| Router, SurrealDB and Qdrant's jemalloc, with deep idle on | `1b0acf0` | 2026-10-24 to 25 |
| Qdrant's cralloc, deep idle off | `536dbe8` | 2026-10-27 |
| Meilisearch's saturated rate: cralloc 1,413, jemalloc 1,372, tcmalloc 1,423, mimalloc 1,328 req/s | `5e06b9b` | 2026-10-21 |
| Long-run memory, all five: steady, shift, lifetime, mini-redis; before the tail rule | `9a11f70` | 2026-10-24 to 25 |
| The large-block mix; heaps of 0.5 to 10 GiB; troughs | `26199ba`; `536dbe8`; `1d73192` | 2026-10-25 to 28 |
| Kernel suite, with the 64-Vec, 256 MiB and spawn jobs; memory after a peak | `c7f5c21` | 2026-10-22 to 23 |
| ruff and boa; the screening | `5097226` | 2026-10-20; 2026-10-19 |

## Open questions

- **Tails with deep idle off** are not measured for the Router, SurrealDB or jemalloc on Qdrant, all
  in milliseconds.
- **The changes since `e3753b5`.** The refresh rule, measured alone, leaves the 2 GiB order service
  level; the pass's goal has no such measure, and the full server table has not been run since.
- **One DataFusion run** peaked at 1,454 MiB against 852 to 862 for cralloc's other five, at the
  same faults and cycles. It was the first of its session. Not explained.
- **Placement on the kernel suite.** Its spawn jobs and `rpc log c256` are single builds, so gaps
  under about 4% there are not findings.
