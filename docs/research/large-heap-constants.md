# The fixed numbers at heaps of 2 to 10 GiB

2026-10-26 and 27, allocator at main `bf837db`, on the reference host (Ryzen 9950X, Linux
7.2.9, transparent huge pages on `madvise`). The question: do the allocator's fixed numbers
hold at heaps of 2 to 10 GiB, and which should follow evidence the allocator already has? The
hold on free large slots and the tail rule's headroom are out of scope.

## Summary

- **They hold.** On a ladder of 0.5, 2, 4 or 5, and 10 GiB live, nothing asmalloc does gets
  worse faster than the heap. The order service at 10 GiB costs the same cycles a request as at
  0.5 GiB (68.0k to 68.4k), with the same p99.9 (24 us), the lowest resident over live of all
  allocators (1.073, jemalloc 1.095), and it falls with the heap. On a steady cache of small
  values the ratio falls from 1.15 to 1.086 and asmalloc stays the cheapest and the leanest.
  No proposal below is worth building before the work it would displace.
- **The floor and the cap are not engaged where it matters most.** Through the order service's
  day at every size, free memory never passed the 32 MiB floor while it served, so no pass,
  hold look or growth payment ran and the background thread started only at shutdown. On the
  steady cache the count passes the floor (by fragmentation) but what a pass could return
  stays under it, so each interval costs one counting lap: at most 13 ms of the thread's time
  at 10 GiB. They bite only on the large-buffer job, at every size, not more at large ones:
  half of its growth steps are over the 64 MiB cap at 0.5 GiB and at 5 GiB alike. Eight times
  the cap, or eight times the bound on outgrown slots, changed its faults by -7 to +8% at 5 GiB
  and did not lower its peak.
- **What does grow with the heap is bounded, and in the right places.** The clocked pass's
  length grows with what it returns: with the thread, the longest pass took 0.24 s at 2 GiB
  and 0.78 s at 10 GiB, against the one-second interval, returning about 1.3 GiB in one pass.
  A step on a request's path stays within its pages: without the thread the longest purge step
  of a heartbeat was 0.38 ms at 2 GiB and 1.5 ms at 10 GiB, the difference the kernel's (a huge
  page's split, with eight CPUs to shoot down). `mprotect` commits grow linearly past 64 MiB a
  region (the order service: 284 calls at 0.5 GiB, 627 at 10 GiB), and two of the six 4 and
  10 GiB fills had calls of 3 to 7 ms.
- **Two things to know that are not constants.** On the large-buffer job asmalloc's page
  tables are 1.5 to 2% of the heap, three times the incumbents' (77 MiB at 5 GiB against 21 to
  23), with 2,200 mappings; it is proportional, not fixed, and comes from where grown blocks
  sit, not from a number. And the design's "Sizing from evidence" describes an allowance for
  parked memory (a fraction of the heap with a floor) and depot targets that rise on evidence;
  the code has neither. Parked memory is bounded per CPU and per class (`SPARE_BUDGET`,
  `DEPOT_TARGET`), so it does not grow with the heap, which at large heaps is what matters.

Ranked proposals, none urgent:

1. **Let commits stay logarithmic.** `COMMIT_AHEAD_MAX_HUGE_PAGES` makes the number of
   `mprotect` calls linear in a region's use past 64 MiB. Rule: a commit goes as far ahead as
   the region has committed already, with no fixed cap (or a cap of a fraction of the heap).
   Buys: most of the calls the heap adds past a few GiB (about 280 of the order service's 627
   at 10 GiB), each a write hold of the address space's lock that stalls every faulting
   thread; the 3 to 7 ms ones fall in fills. Costs a small heap nothing (it only acts past
   64 MiB a region); under strict overcommit, commit charge up to a region's use. Small, and
   not measured as a time.
2. **Document the return rate as a limit.** One pass returns at about 1.5 GiB a second of the
   thread's time; a drop of more than that in an interval takes more than one. It is the
   kernel's rate, not a constant's, and memory still comes back within seconds (below).
3. **Correct the design.** Either build the parked-memory allowance or describe the fixed
   per-CPU and per-class bounds the code has.

Not measured: anything above 10 GiB; mini-redis and the log store as servers on the ladder;
a burst after a silence at 10 GiB with free memory to fault back (the order service has
none); NUMA. Comparisons with mimalloc stop at 5 GiB on the steady cache (below).

## The numbers

Every absolute number in the allocator crate that the heap's size could bear on. "Work bound"
and "per block" numbers bound one call or one block, which the heap's size does not change.

| Number | Where | Value | What it is for | Kind |
|---|---|---|---|---|
| Floor of the allowance | `back/policy.rs` `ALLOWANCE_FLOOR_HUGE_PAGES` | 32 MiB | the clocked pass returns nothing while what it could return is under it, and stops there; the hold returns large slots down to it; the background thread starts above it | absolute |
| Retained cap | `RETAINED_CAP_HUGE_PAGES` | 64 MiB | while free resident memory is over it, carving pays an equal purge | absolute |
| Growth step | `GROWTH_STEP_HUGE_PAGES` | 2 MiB | each step of carving pays one bounded walk | work bound |
| Unpaid growth | `GROWTH_UNPAID_HUGE_PAGES` | 16 MiB | debt the thread may leave before slow paths pay it | absolute |
| Interval | `DECAY_TICKS`, `IDLE_TICKS` | 2^32 TSC ticks, 1.0 s here | one pass, one idle look, one age limit | time |
| Share of a pass | `DECAY_SHIFT` | a quarter of retained, or all over the cap, held to what the last lap counted less the floor | how fast free memory goes back | proportional |
| Least a pass takes | `DECAY_FLOOR_HUGE_PAGES` | 2 MiB | ends a decay's tail | absolute |
| Fill claims | `FILL_CLAIMS` | 256 bitmap words | worst fill | work bound |
| Runs | `PURGE_MAX_RUN_PAGES`, `_WORDS`; `PURGE_MIN_RUN_PAGES`; `PURGE_ANY_RUN_EVERY` | 256 KiB, 64 words; 4 pages; 1 pass in 8 | one `madvise`; which runs a pass takes | per call |
| Frontier | `FRONTIER_INTERVALS` | 2 intervals | what fills reached is not purged | time |
| Heartbeat step | `PURGE_STEP_WORDS`, `PURGE_STEP_PAGES` | 256 words, 0.5 MiB | a step of the pass on a request's path, and the thread's step | work bound |
| `maintain` step | `PURGE_MAINTAIN_STEP_WORDS`, `_PAGES` | 64 words, 0.5 MiB | a step of `maintain` | work bound |
| Growth walk | `PURGE_CAP_WORDS` | 16,384 words | bitmap one growth payment reads | work bound |
| Hold look | `HOLD_BEAT_HUGE_PAGES` | 2 MiB | what one heartbeat's look at the hold returns | work bound |
| Huge pages after | `HUGE_AFTER_HUGE_PAGES` | 16 MiB carved | when a small class's region turns to huge pages | absolute, per class |
| Huge-page density | `HUGE_MIN_SLOTS_PER_PAGE`, `TAIL_SAMPLE_PAGES`, `TAIL_SPARSE_PAGES`, `TAIL_SPLIT_SHARE` | classes to 1 KiB; 8 samples; a quarter free | per huge page | ratio |
| Grown blocks' huge pages | `GROWN_HUGE_*` | 2 MiB, doubling, half dense | per block | per block |
| Move piece | `MOVE_PIECE_HUGE_PAGES`, `back/class.rs` `WINDOW` | 2 MiB | both copies resident for one piece | work bound |
| Kept outgrown slots | `KEPT_OUTGROWN_HUGE_PAGES` | 64 MiB | slots growing buffers left, kept resident | absolute |
| Unwritten zeroed block | `UNWRITTEN_FLOOR_HUGE_PAGES`, `_SAMPLES`, `_RESIDENT_ONE_IN` | 16 MiB, 16, one in 16 | the look at a zeroed block's free | per block |
| Commit ahead | `COMMIT_AHEAD_PAGES`, `_HUGE_PAGES`, `_MAX_HUGE_PAGES` | 64 KiB; 2 MiB; the region's use, at most 64 MiB | `mprotect` calls as a region grows | proportional, capped |
| Magazine | `classes/magazine.rs` `MAGAZINE_BUDGET` | 64 KiB of objects, at most 1,024 slots | a CPU's loaded magazine of a class | per CPU and class |
| Spares | `depot/spare.rs` `SPARE_BUDGET`, `MAX_SPARES` | 1 MiB a CPU, 4 a stack | spares and stock | per CPU |
| Depot | `depot/mod.rs` `DEPOT_TARGET`, `DOMAIN_CAP` | 128 KiB a class, a quarter of it a domain | magazines parked between CPUs | per class |
| Launder budget | `LAUNDER_BUDGET` | 1,022 objects | worst call | work bound |
| Refresh | `depot/slow.rs` `REFRESH_EVERY` | 1 retirement in 32 | turnover of circulating objects | rate |
| Pacing | `depot/heartbeat.rs` `BEAT_BYTES`, `LOOK_EVERY`, `LONE_EVERY` | 32 KiB moved a beat; 1 in 16 | how often slow paths do chores | rate |
| The thread's round | `heap/global.rs` `MAINTAIN_EVERY`; `depot/maintain.rs` `SHED_STEPS`; `depot/thread.rs` `QUIET_ROUNDS` | 500 ms; 4,096 magazines; 10 rounds | its period and bound | time, work bound |
| Headroom for growing blocks | `heap/large.rs` `HEADROOM`, `FALLBACK_HEADROOM`, `HEADROOM_SHARE` | 16x, 4x, a sixteenth of a region | address space per block | per block |
| Reservation | `space/` | 4,096 regions of 4 GiB; slabs for 8,192 CPUs | address space | absolute |
| Hold | `FINE_TICKS`, `HOLD_STEPS` | 125 ms | out of scope (settled, `hold-credit.md`) | time |
| Tail headroom | `TAIL_HEADROOM_HUGE_PAGES` | 16 MiB | out of scope (another study) | absolute |

Not in the code, though "Sizing from evidence" in the design describes them: an allowance for
everything parked, a fraction of the heap in use with a floor of tens of megabytes, under which
buffers grow; fills that start small and double; spare room earned beyond stock's; a depot
target that rises when a fill follows a launder. What the code bounds parked memory by is the
per-CPU and per-class rows above, which do not grow with the heap.

## Results

### The order service, 0.5 to 10 GiB

`orders axum cache` from `servers-large` (axum on tokio, 64 connections, every request stores
a new order of about 1.9 KiB and drops the oldest), at 0.5, 2, 4 and 10 GiB live, the
large-heap day: fill, a minute at 70% of the slowest allocator's saturated rate, three bursts
after silences of 3 s, close. Three rounds, medians. Cycles a request and p99.9 at 70%,
resident over live at the end of the minute and 10 s after close:

| Live | asmalloc | jemalloc | tcmalloc per-CPU | mimalloc |
|---|---|---|---|---|
| 0.5 GiB | 68.0k, 24 us; 1.099, 1.103 | 69.4k, 25; 1.106, 1.109 | 69.9k, 24; 1.114, 1.117 | 72.9k, 55; 1.199, 1.202 |
| 2 GiB | 68.2k, 24; 1.080, 1.081 | 69.6k, 24; 1.097, 1.097 | 70.1k, 25; 1.098, 1.099 | 73.2k, 39; 1.156, 1.157 |
| 4 GiB | 68.2k, 24; 1.076, 1.076 | 69.6k, 24; 1.095, 1.096 | 70.4k, 25; 1.096, 1.096 | 72.7k, 39; 1.151, 1.152 |
| 10 GiB | 68.4k, 24; 1.073, 1.074 | 69.9k, 24; 1.095, 1.095 | 70.2k, 24; 1.095, 1.095 | 72.5k, 47; 1.149, 1.149 |

- Nothing moves with the heap: asmalloc's cycles stay within 0.6%, p50 and p99 within a
  tenth of a microsecond, and it leads at every size by the same 2%.
- The diagnostic build's counters: no pass, no hold look and no growth payment while it
  served, at any size. The store frees one order for every one it stores, so free memory
  stays far under the floor; the background thread starts at shutdown, when the store is
  dropped. The cap is never over at a carve.
- Its heartbeats' longest chores took 19, 73, 84 and 172 us at the four sizes, none over
  0.2 ms.
- Page tables are the same for everyone at each size (21.7 to 22.8 MiB at 10 GiB). asmalloc
  has about 200 mappings against 55 to 68, at every size.
- `mprotect` commits: 280 to 294 at 0.5 GiB, 340 to 348 at 2, 403 to 416 at 4, 612 to 627 at
  10. On the measurement CPUs they take 12 to 20 us at the longest, except in two fills (4 GiB
  round 1, 10 GiB rounds 1 and 2): 3 to 6 calls of 3 to 7 ms, while four threads fault in the
  same address space.
- The first requests of the bursts after silences vary from run to run for every allocator
  (asmalloc's p99.9 there 47 to 860 us across rounds and builds, with no trend in size);
  nothing is returned in the 3 s silences, so they do not test the floor.

### A steady cache of small values

`workload drift shape=steady` (eight threads, a shared cache of values with a log-normal size
around 128 B, most frees another thread's object), 0.4G operations a GiB, then a quiet minute.
Resident heap over live at the end, cycles an operation (from runs on shared cores, so only
ratios within a row compare), page faults a thousand operations:

| Live | asmalloc | jemalloc | tcmalloc | glibc | mimalloc |
|---|---|---|---|---|---|
| 0.5 GiB | 1.149, 1421, 0.54 | 1.174, 1562, 0.83 | 1.146, 1470, 0.81 | 1.132, 2220, 0.79 | 1.688, 2270, 0.05 |
| 2 GiB | 1.104, 1542, 0.28 | 1.135, 1770, 0.81 | 1.125, 1560, 0.79 | 1.150, 2490, 0.80 | 1.568, 2685, 0.05 |
| 5 GiB | 1.091, 1560, 0.22 | 1.117, 1820, 0.80 | 1.119, 1655, 0.79 | 1.089, 2541, 0.76 | 1.156, 2392, 0.05 |
| 10 GiB | 1.086, 1578, 0.20 | 1.113, 1930, 0.80 | 1.116, 1786, 0.80 | 1.110, 2641, 0.78 | |

On the measurement CPUs at 10 GiB: asmalloc 1,321 cycles an operation, jemalloc 1,592.
Nothing is returned in the quiet minute (the live set stays). The count of retained memory is
over the floor by fragmentation, and what a pass could return is under it, so once an interval
the thread walks a lap that only counts: 154 laps, the longest pass 12.6 ms, the thread 0.5 s of
CPU in two minutes. Without the thread the heartbeats walk those laps; their longest purge
step took 41 us. mimalloc's ratio at 2 GiB (1.57) would have taken it over the 16 GiB cap at
10 GiB, so the comparison with it stops at 5 GiB; the others fit.

### Phases: peaks and troughs

`shape=lifetime`: short-lived values whose total alternates between 0.79 and 0.066 of the rung,
beside a long-lived cache of 0.2 of it, six phases of 1.2G operations. Ratio at the end of
each high phase (peak) and low phase (trough), the trough's heap in GiB, and after the quiet
minute (medians of the three phases):

| Live | asmalloc | asmalloc, no thread | jemalloc | tcmalloc | glibc | mimalloc |
|---|---|---|---|---|---|---|
| 0.5 GiB | 1.15, 1.96 (0.26), 1.94 | 1.16, 1.89 (0.25), 1.92 | 1.18, 1.37 (0.18), 1.35 | 1.16, 4.29 | 1.14, 4.09 | 1.25, 4.66 |
| 2 GiB | 1.10, 2.21 (1.18), 2.04 | 1.10, 2.21 (1.18), 2.25 | 1.12, 1.25 (0.66), 1.22 | 1.13, 4.19 | 1.09, 4.00 | 1.15, 4.27 |
| 5 GiB | 1.09, 2.89 (3.85), 2.83 | 1.09, 2.93 (3.91), 2.94 | 1.11, 2.27 (3.03), 2.24 | 1.12, 4.16 | 1.07, 3.97 | 1.13, 4.20 |
| 10 GiB | 1.08, 3.40 (9.07), 3.21 | 1.08, 3.34 (8.91), 3.41 | 1.11, 3.19 (8.51), 3.14 | 1.12, 4.14 | 1.07, 3.96 | 1.12, 4.17 |

- asmalloc's troughs are above jemalloc's at every size, by 0.08, 0.5, 0.8 and 0.5 GiB: not
  growing with the heap. The excess is the stranded huge-page units `heap-drift.md` found,
  not a fixed number.
- **How long memory takes to come back.** On the measurement CPUs (phases of about 18 s), the
  heap in a trough is within a tenth of the fall of where the phase ends 13 to 16 s after the
  fall begins at 2 GiB and 15 to 17 s at 10 GiB, with the thread or without it; jemalloc 13 to
  14 s and 22 s. The fall in live itself is spread over the phase's first seconds. In the quiet
  minute after the last trough, the thread returns 0.16 GiB more at 2 GiB and 0.52 GiB at 10
  GiB, within 19 s; without it nothing (a quiet program's documented limit).
- **The pass against the interval.** With the thread, the longest clocked pass (start to end)
  took 0.24 s at 2 GiB, 0.41 s at 5 (on shared cores) and 0.78 s at 10, its longest round the same; 119 of 137
  passes at 10 GiB took over a millisecond. The thread used 4.0 s of CPU in 118 s at 10 GiB
  (1.6 s at 2 GiB). A pass that outlasts the interval only delays the next one; nothing is lost.
- **Steps on a request's path (no thread).** 260k heartbeat purge steps returned memory at
  2 GiB and 496k at 10 GiB. The longest took 0.38 ms and 1.52 ms, p99.9 30 and 61 us; 741 of
  the 496k took over 0.1 ms. The step is bounded in pages; what grows is the kernel's cost
  of the calls in it: the longest split of a huge page (`MADV_COLD`) took 0.38 ms at 2 GiB and
  1.52 ms at 10.
- The cap: no growth step was over it, at any size. Troughs are freed memory, refilled by
  reuse, which the cap does not charge.

### Large buffers

`shape=large`: a cache of buffers of 40 KiB to 32 MiB by phase, half built by doubling with
`realloc`, two cycles of four phases. Every allocator's peak at 0.5 GiB was 2 to 3.7 times
live, so the top rung is 5 GiB for all (mimalloc's 3.7 would pass the cap at 10). Ratio at the
end, at its most, after the quiet minute; cycles an operation in millions; page faults an
operation:

| Live | asmalloc | asmalloc, no thread | jemalloc | tcmalloc | glibc | mimalloc |
|---|---|---|---|---|---|---|
| 0.5 GiB | 2.03, 2.53, 1.13; 5.6, 92 | 2.01, 2.53, 2.10; 6.2, 95 | 1.51, 2.13, 1.08; 14.0, 1706 | 2.97, 3.26, 2.75; 10.3, 14 | 2.76, 3.21, 2.53; 8.8, 371 | 3.31, 3.70, 2.85; 8.5, 0.7 |
| 2 GiB | 1.31, 1.39, 1.08; 6.7, 115 | 1.32, 1.59, 1.43; 7.1, 110 | 1.19, 1.82, 1.04; 14.8, 1713 | 1.69, 2.24, 1.73; 10.3, 23 | 2.13, 2.45, 1.89; 9.1, 196 | 1.99, 2.27, 2.02; 9.6, 1.4 |
| 5 GiB | 1.21, 1.53, 1.12; 8.3, 103 | 1.20, 1.42, 1.26; 8.4, 105 | 1.11, 1.72, 1.05; 13.9, 1718 | 1.39, 2.05, 1.40; 14.7, 24 | 1.96, 2.15, 1.78; 11.4, 222 | 1.76, 2.15, 1.75; 13.2, 2.2 |

(0.5 and 2 GiB ran on shared cores, 5 GiB on the measurement CPUs. The peak depends on the
  speed of the cores: asmalloc at 2 GiB on the measurement CPUs peaked at 2.03 times live,
  against 1.39 on shared cores, since the hold counts time and the work goes faster.)

- This is the job where the cap bites, at every size. Over the cap at 288 of 476 growth steps
  at 0.5 GiB and 1,738 of 3,457 at 5 GiB. The payments returned 2.9 GiB in the 0.5 GiB run and
  10.7 GiB in the 5 GiB run, 1.9 and 1.3 times what was carved.
- **Eight times the cap, and eight times the kept outgrown slots** (512 MiB each, scratch
  builds, `variant_edit.py`), at 5 GiB: payments fell by a third with the cap, faults rose 8%
  and cycles 4%; with the kept slots, faults fell 7% and cycles rose 1%; peaks 7.18 and 7.09
  GiB against 7.11 to 7.64 for the build as it is. At 2 GiB, three runs side by side on the
  measurement CPUs: faults 2.6% (cap) and 9% (kept slots) fewer, cycles within 1%, peaks 4.01
  to 4.06 GiB for all three. Neither number is what the faults follow.
- Steps on a request's path, without the thread, at 5 GiB on the measurement CPUs: purge steps
  of heartbeats at most 1.15 ms (p99.9 about 0.5 ms, 11% over 0.1 ms), growth payments by a
  carve at most 0.62 ms. With the thread these are its work: 16.4 s of its CPU in 158 s (10%).
- **Page tables and mappings**, the one fixed overhead that is not level with the others: 10,
  33 and 77 MiB of page tables at 0.5, 2 and 5 GiB (1.5 to 2% of live) against 3, 10 and 21
  to 23 for every incumbent; 615, 1,147 and 2,196 mappings against under 460. Each grown block
  sits alone in a slot with room to grow and gets huge-page advice of its own: a page-table
  page for the 2 MiB around each smaller block, and two mappings for each advised one (an
  inference from the counts, not traced). It grows with the number of grown blocks, not with any number above; with 65,530 mappings a
  process allowed, a heap of some 30,000 advised blocks would meet the limit (the design's
  refused-call rule covers that).

## Each number

- **Floor (32 MiB).** Holds. It keeps what a pass could return from going back below it; at
  10 GiB that is 0.3% of the heap, and no job here showed a cost from returning the rest
  (the faults it saves are the burst's after a silence, which a heap of gigabytes of reuse
  does not take from the last 32 MiB). It also starts the thread, and the order service never
  reaches it: the thread wakes only at shutdown there, at every size. Scaling it would keep
  more of a trough resident for nothing measured.
- **Retained cap (64 MiB).** Holds. Large heaps are not always over it: the order service, the
  steady cache and the phases never were at a carve, at any size. The large-buffer job is over
  it half the time at 0.5 GiB as at 5, and eight times the cap did not help it. Every carve over
  it pays one walk of at most 16,384 words and half a megabyte, under 1 ms on a request.
- **Kept outgrown slots (64 MiB).** Holds, by the same experiment.
- **Unpaid growth (16 MiB).** Holds; the slow paths paid what the thread left 1,383 times in
  the 5 GiB large run, each step under 0.85 ms.
- **The share of a pass (a quarter) and its floor (2 MiB).** Hold. Over the cap a pass takes
  everything it counted, so the share does not slow a large drop; memory came back in 15 to
  17 s at 10 GiB as at 2. Documented limit: the kernel returns about 1.5 GiB a second of the
  thread's time, so one pass at 10 GiB took 0.78 s of the 1 s interval.
- **Step sizes on a request's path** (256 words and 0.5 MiB; the hold's 2 MiB look; the growth
  walk). Hold: bounded at every size, longest 1.5 ms at 10 GiB, the kernel's.
- **The thread's round** (500 ms, `SHED_STEPS`, bounded `maintain` steps). Holds: its longest
  `maintain` step 5.4 ms at 10 GiB, its CPU 0.4 to 10% of one CPU by job.
- **Huge pages after 16 MiB, unwritten zeroed blocks from 16 MiB, move pieces, run lengths,
  headroom.** Per class or per block; the heap's size does not bear on them.
- **Commit ahead (at most 64 MiB a call).** Should scale: see proposal 1.
- **Magazines, spares, depot targets.** Hold: fixed per CPU and class, so parked memory is a
  smaller share of a larger heap; cycles a request did not move with the heap.

## Method

- **Builds.** The plain builds of the drift workload (`bench/probes/heap-drift/build.sh`) and
  of the order service, and a diagnostic build of asmalloc (`diag_edit.py`, scratch, never
  merged) that exports counters (`ASMALLOC_DIAG`) and times, with the cycle counter at 4.3
  GHz, every heartbeat's chores and its purge part, every growth payment by who paid it, the
  thread's steps, `maintain`'s steps and rounds, each clocked pass from start to end, and the
  `mprotect` and `madvise` calls. Memory figures for asmalloc are the diagnostic build's (it
  adds a few atomic counters and no memory); the order service's cycles are the plain build's
  and agree with the diagnostic one's to 0.5%. Variants of one constant (`variant_edit.py`)
  are the diagnostic build with that constant changed.
- **The order service** ran through `benchrun` with a suite added for this,
  `servers-ladder` (the three `servers-large` sizes and one of 5.6M orders, 10 GiB), one
  session per size and round under `scripts/measure --both-l3`, server on CPUs 0-3 with C2
  and C3 off, generator on 8-11, the five builds and a `live-bytes` build in a random order,
  three rounds. Live bytes are the `live-bytes` build's count at the same mark
  (`srvsum.py`). `watch_servers.py` sampled every server's page tables, mappings, background
  thread and counters every 0.5 s. The entries of `allocators.local.toml` were appended to
  `bench/allocators.toml` for the sessions.
- **The drift jobs** ran one at a time under a 16 GiB cap (`drift.py`, sampling the process
  every 0.25 s), through `ladder.sh` under `scripts/heavy` on CPUs 12-15 and 28-31, with
  nothing else holding more than a few GiB. The heavy slice was frozen for minutes at a time by
  other teammates' measurements. A frozen run does the same operations, so its ratios and
  faults stand, but its times do not; every figure in seconds, and every timing of a step,
  comes from runs on the measurement CPUs 0-3 and 8-11 under `scripts/measure --both-l3`
  (`timed.sh`): the steady cache and the phases at 2 and 10 GiB, the large job at 5 GiB and its
  variants. The 10 GiB phase runs and the 5 GiB large runs in the tables are those. One run
  per allocator and size.
- **Seconds to come back** (`summary.py`): from the last sample before a fall in live to the
  first at which the heap is within a tenth of the fall of its value at the end of the phase
  (or of the quiet minute).

## Reproduce

From the repository root, on this branch:

```
scripts/heavy bench/probes/large-heap-constants/build-all.sh
LHC_VARIANT=RETAINED_CAP_HUGE_PAGES=256 scripts/heavy bench/probes/large-heap-constants/build.sh \
  asmalloc-diag-cap512 HEAD asmalloc diag_edit.py variant_edit.py -- workload
LHC_VARIANT=KEPT_OUTGROWN_HUGE_PAGES=256 scripts/heavy bench/probes/large-heap-constants/build.sh \
  asmalloc-diag-kept512 HEAD asmalloc diag_edit.py variant_edit.py -- workload
(cd bench && cargo build --release -p bench-runner)
cat bench/probes/large-heap-constants/allocators.local.toml >> bench/allocators.toml   # local only
scripts/heavy bench/probes/large-heap-constants/ladder.sh OUT/mem "steady lifetime large" "0.5 2" \
  "asmalloc-diag asmalloc-diag:off je tc mi glibc"                  # then 5 and 10, as above
bench/probes/large-heap-constants/queue.sh LIST    # timed sessions: "drift OUT JOB RUNG BUILD..."
                                                   # and "orders OUT RUNG ROUND"
python3 -I bench/probes/large-heap-constants/summary.py OUT/mem --diag
python3 -I bench/probes/large-heap-constants/srvsum.py OUT/srv --diag
```

Raw data: `docs/research/data/large-heap-constants-2026-10-27.tar.gz`: `mem/` (the ladder's
runs and watch files), `timed/` (the runs on the measurement CPUs), `srv/` (benchrun's records
and the watcher's samples), and the queue logs.
