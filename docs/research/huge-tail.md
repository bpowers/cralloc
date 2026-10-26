# Huge pages only where memory goes back: the tail rule

2026-10-24 and 25, main `81437b0` (the first runs on `ed0e1b7`), on the reference host (Ryzen
9950X, Linux 7.2.9, transparent huge pages `madvise`, defrag `madvise`, khugepaged
`max_ptes_none` 511, `pages_to_scan` 4096 every 10 s). Measured with a run-time switch in
one binary; the switch is a probe, not product code (`bench/probes/huge-tail/`). The drift
workload is heap-drift's (`heap-drift` at `4522abd`).

## What matters

- **Returning part of a huge page frees memory only if we make the kernel split it, and that
  is cheap.** `MADV_DONTNEED` on part of a PMD-mapped huge page takes the pages out of RSS
  at once but not out of the cgroup's charge: the folio waits on the deferred-split queue
  until reclaim, and the thread that then meets the cgroup's limit stalls 40 to 50 ms while
  the kernel splits it. `MADV_COLD` on part of the unit splits it first, 20 to 80 us a unit,
  and the return is then real.
- **The stranded memory is a tail above the frontier.** In every breakdown of the drifting
  run, at least 99% of the whole free 4 KiB pages stranded in 2 MiB units that hold something
  lay at or above the region's aged frontier rounded up: a dense bottom, then units at most a
  quarter full.
- **The rule stops the drift and cuts what the troughs keep by a quarter.** On the problem's
  run (lifetime mix, 24 phases of about 45 s, wandering size table), resident over live at
  the peaks went 1.10 to 1.23 with main and 1.10 to 1.15 with the rule (jemalloc 1.12 to
  1.15, glibc 1.07); in the troughs 4.29 with main and 3.21 with the rule (jemalloc 3.79,
  glibc 4.03). On Qdrant (`real-servers`, the first form of the rule) heap mean 447 to
  409 MiB, a minute after close 195 to 148, cycles level.
- **The price is huge pages on the way back up: 1 to 1.5% of time on the allocation-bound
  lifetime job, nothing on servers.** A split unit that fills reach again faults 4 KiB pages
  until khugepaged rebuilds it, which takes minutes, so at the peaks the job held 300 to
  400 MiB in huge pages instead of 1.4 to 1.6 GiB.
- **Taking the advice off must be part of returning memory, not a state that follows the
  frontier.** The first form lowered the boundary whenever the frontier fell; on the log
  store, whose small classes wobble by a few MiB, it changed the advice 50 times a run and
  returned nothing, and the tail moved with it (p99.9 0.1 to 0.2-0.4 ms). The candidate
  (`t1r2`) takes the advice off only over units it has just split and returned part of, once
  per region per pass, and puts it back only where it came off. A swing under about 128 MiB
  then returns nothing and changes no advice. Its walk above the frontier read those units
  page by page even in laps that only count, and on the store the first milliseconds of a
  burst were slower in 6 of 8 runs of the walking variants (1.2 to 2.7 ms against 0.4 to
  0.8). The product looks at a few pages of each unit first and walks it only in a step that
  may return memory; its burst start is main's ("The product").
- **Patience is a step function per trough length.** Waiting N intervals returns nothing from
  troughs shorter than about N to 2N intervals (main's behaviour, huge pages kept) and most of
  longer ones. On Qdrant, whose bursts are seconds to tens of seconds apart, patience of 30
  and 120 did nothing. Ranked behind the frontier's own two intervals.
- **Ranked first: `t1r2`.** Details, costs and design text below.

## The kernel

All from `bench/probes/huge-tail/kernel.c`, each run in its own cgroup (`scripts/memlimit`),
read against the kernel's source at `v7.2` (`mm/madvise.c`, `mm/huge_memory.c`, `mm/rmap.c`,
`mm/memory.c`, `mm/khugepaged.c`).

- **Partial `MADV_DONTNEED` defers the free.** `zap_pmd_range` splits the PMD mapping into
  page-table entries and zaps the range; `__folio_remove_rmap` sees a folio partly mapped and
  queues it with `deferred_split_folio`, on a memcg-aware list. 64 units of 2 MiB, all but 3
  pages each returned: RSS 128 to 0.8 MiB, `AnonHugePages` 128 to 0, `memory.stat` anon 128
  to 1.0 MiB, but `memory.current` 129.0 to 129.0 MiB, and 128.9 a minute later with the
  range advised against huge pages so that nothing else touched it.
- **Reclaim frees it.** `deferred_split_scan` is a memcg-aware shrinker. A write to
  `memory.reclaim` (writable in our scope) split all 64 at once (129 to 1.7 MiB, 24 ms; the
  write returns `EAGAIN`). Faulting into `memory.max` with 256 MiB stranded, the faults that
  met the limit stalled 40 ms (4 KiB faults) and 49 ms (2 MiB faults), and then the memory was
  gone. The memory is not lost, but a thread pays for it at the worst moment, and RSS says it
  is gone when the cgroup says it is not.
- **Forcing the split.** `madvise_cold_or_pageout_pte_range` and `madvise_free_huge_pmd` call
  `split_folio` on a range that covers part of a PMD-mapped huge page, or part of a large
  folio mapped by page tables. One `MADV_COLD` page before the `MADV_DONTNEED` makes the
  return real (`memory.current` 3 MiB after, against 112 MiB without). `MADV_FREE` and
  `MADV_PAGEOUT` do the same, as does `MADV_COLD` over the unit after the `MADV_DONTNEED`.
  The split call costs 20 to 80 us a huge unit, the `MADV_DONTNEED` of its free 2 MiB about
  35 us, and one `process_madvise` over many units 15 us a unit. `MADV_COLD` takes `mmap_lock`
  for reading; `MADV_DONTNEED` only the VMA's lock. In the allocator the split averaged 20 to
  60 us; its worst was 0.9 to 1.7 ms on the measurement CPUs and up to 8 ms on busy ones.
- **khugepaged** skips a range advised `MADV_NOHUGEPAGE`: split units stayed split for four
  minutes. With the advice on it collapses any unit with one page present, copying the live
  pages into a new huge page: 64 units returned in part were rebuilt within about two
  minutes (RSS 0.8 back to 96 MiB of 128), and after the advice went back on over a split
  tail, 15 sparse units within about 85 s. Advice must not stand over units that stay sparse.
  `MADV_COLLAPSE` of one unit took 5.8 ms.
- **Faults after the advice is back.** A unit returned whole faults a huge page at once:
  `MADV_DONTNEED` over a whole 2 MiB frees its empty page table (`CONFIG_PT_RECLAIM`). A unit
  returned in part keeps its page table and faults 4 KiB pages until khugepaged collapses it.
- **Advice changes** take `mmap_lock` for writing and write-lock the VMAs they split or
  merge: the call waits for faults and returns in progress in that VMA, and new faults there
  wait behind it; faults use per-VMA locks, so other mappings see nothing. Flipping the advice
  of a 1 GiB tail beside two faulting threads: with 4 KiB faults the call took p50 2 to 8 us,
  p99 0.15 to 0.7 ms, at most 1.7 ms, and faults in that VMA waited up to 2.5 ms once in 5 s;
  beside 2 MiB faults, which stall in compaction here (p99.9 9 ms with no flips), p99 7 ms and
  at most 23 ms. On an idle mapping a call costs 2 to 3 us whatever its size.
- **Mappings.** A boundary adds one `/proc/self/maps` line a region (four regions: 12 lines to
  16; `vm.max_map_count` is 1048576 here).
- **Huge pages are not always there.** At one reading on the first day this host had 36 free
  2 MiB blocks (orders 9 and up of `/proc/buddyinfo`) and every run saw 800 to 1800
  huge-page fault fallbacks; later 600 to 3000 free and no fallbacks. Every timed run records
  the free blocks at its start, and the tables say which conditions they had.

## The rule as prototyped

`ht.rs` documents the switch, `ASMALLOC_HT`. The candidate, `t1r2`:

- **Where.** Units of a region's huge-page part that start at or above its aged frontier
  rounded up to 2 MiB.
- **Lowering.** Only a walk step that may return memory (the clocked pass over the floor,
  growth's payment) walks those units as small pages, in runs within one 2 MiB unit. Before
  the first run it returns from a unit since the advice last changed it asks for the split
  (`MADV_COLD` over the run). A unit that is wholly free goes back in one call over its 2 MiB
  and needs no advice change (its huge page goes whole, or its page table with it). The
  region keeps the lowest unit returned in part; when the region's walk reaches its bottom
  the advice comes off from there to the old boundary, or the region's end, in one call.
- **Raise.** A claim that reaches above a lowered boundary marks the region; the background
  thread, or where there is none the next heartbeat, puts the advice back up to the frontier
  plus 16 MiB rounded up. Where the boundary never came down there is nothing to raise.
- **Steps.** All of it is the clocked purge's work, on the background thread where there is
  one. Where there is none, a split must count against the step's bound (below).

The variants measured: `t1` (the first form: boundary at the frontier, lowered whenever the
frontier fell at least 16 MiB below the carved top), `g1` (`t1` lowering only in a step that
may return), `r1` (`r2` lowering once a step), `p<N>` (patience), `s0` (no split), `w` (raise
headroom), `k<pages>` (a split counted against the step).

## Results

Resident over live: RSS less the workload's own bookkeeping; the cgroup's `memory.current`
agrees within 0.02 when the split is forced. Peaks are the ends of high phases, troughs the
ends of low ones, the mean of the last three. THP is `AnonHugePages` at the peaks.

### The problem's run

Lifetime, 24 phases of about 45 s, 144 samples, one run each, memory only (shared CPUs; their
cycles are not comparable). Free 2 MiB blocks at the starts 1050 to 1900, no fallbacks.

| arm | peaks first, last, last 3 | troughs | quiet minute | THP at peaks MiB | khugepaged collapses |
|---|---|---|---|---|---|
| main | 1.102, 1.215, 1.226 | 4.29 | 4.29 | 1653 | 0 |
| `t1` | 1.104, 1.129, 1.145 | 3.21 | 3.14 | 329 | 1134 |
| `t1p30` | 1.102, 1.157, 1.168 | 4.03 | 3.88 | 1047 | 812 |
| jemalloc | 1.115, 1.140, 1.150 | 3.79 | 3.80 | 0 | |
| glibc | 1.069, 1.074, 1.075 | 4.03 | 4.03 | 0 | |

With the fixed size table (no drift): main 1.100, 1.105, 1.105 at the peaks, 3.77 in the
troughs, 1414 MiB huge at the peaks; `t1` 1.100, 1.097, 1.097, 3.13, 238 MiB.

### Where the stranded memory lies

Half-length run, memstat breakdowns at each phase end, frontier read from the back end: whole
free 4 KiB pages in 2 MiB units that hold something, MiB.

| | main, troughs | main, peaks | `t1`, troughs | `t1`, peaks |
|---|---|---|---|---|
| at or above the frontier rounded up | 355, 408, 431, 473, 493, 505 | 0, 1, 37, 57, 81, 83 | 34, 37, 33, 43, 32 | 0, 1, 5, 8, 8 |
| below | 0 to 3 | 0 | 0 to 2 | 0 |

### The forms of the rule, 4-phase lifetime job

18 s phases, one binary, measurement CPUs `0-3,8-11`, plentiful huge pages.

| arm | troughs | THP at peaks MiB | cycles/op | 4 KiB walks/op | advice calls down + up | splits |
|---|---|---|---|---|---|---|
| main | 3.82 | 1526 | 593 | 0.43 | | |
| `t1` | 3.10 | 426 | 599 | 0.49 | 127 + 180 | 2893 |
| `t1r1` | 3.07 to 3.09 | 420 to 446 | 597 to 601 | 0.49 | 913 to 1071 + 112 to 116 | 2447 to 2489 |
| `t1r2` | 3.11 | 404 | 602 | 0.50 | 107 + 109 | 2442 |

`t1r2`'s window, from a unit's first return under the advice to the advice coming off: mean
2.2 ms, at most 16.5 ms; khugepaged collapsed 16 units in the run as with `r1` (22 with `t1`),
so none was refilled in it. Without the split (`s0`) RSS said 3.14 and the cgroup 3.93.
Headroom for the raise: 0 made a growing class step its boundary about ten times an interval
(716 raises), 16 MiB 145, 64 MiB 97, with no change in memory.

### Patience and the swing's period

Lifetime job with troughs of the length shown, one round each, interleaved; plentiful: 660
to 1250 free 2 MiB blocks and no fallbacks; scarce: 800 to 1800 fallbacks a run.

| phases, huge pages | arm | troughs | THP at peaks | cycles/op | advice calls |
|---|---|---|---|---|---|
| 3 s, plentiful | main | 4.65 | 1624 | 580 | |
| | `t1` | 4.43 | 1439 | 579 | 166 + 348 |
| 3 s, scarce | main / `t1` / `p8`, `p30` | 4.59 / 4.38 / = main | 382 / 317 | 599 / 599 | 0 / 443 / 0 |
| 10 s, scarce | main / `t1` / `p8` / `p30` | 4.11 / 3.74 / 4.05 / = main | 632 / 380 / 624 | 603 / 604 / 604 | 0 / 768 / 9 / 0 |
| 18 s, plentiful | main / `t1` / `p8` | 3.82 / 3.09 / = main | 1524 / 404 / 1522 | 595 / 603 / 597 | 0 / 322 / 0 |
| 30 s, plentiful | main / `t1` / `p8` | 3.95 / 2.95 / 3.65 | 1573 / 293 / 1228 | 600 / 609 / 601 | 0 / 993 / 104 |
| 30 s, scarce | main / `t1` / `p8` / `p30` | 3.92 / 3.00 / 3.83 / = main | 582 / 172 / 567 | 612 / 615 / 607 / 611 | 0 / 925 / 134 / 1 |

The climb out of each 30 s trough (`rampcost.py`, the first sample of each high phase, about
15 s): 14.4 ms a million operations with `t1` against 14.1 with main, with 3.3 times the
faults; the steady part 14.9 against 14.7.

### The floor

Lifetime job, 10 s phases, the short-lived set swinging S MiB above a 128 MiB trough beside
384 MiB of long-lived data, main against `t1`, plentiful:

| S, MiB | advice calls | units split | MiB returned | troughs, heap MiB, main / `t1` | first sample of each climb, ms/Mop |
|---|---|---|---|---|---|
| 32 | 3 | 0 | 0 | 646 / 646 | 13.7 / 13.7 |
| 64 | 32 | 0 | 0 | 669 / 670 | 13.9 / 13.8 |
| 128 | 129 | 2 | 0 | 733 / 727 | 14.0 / 14.0 |
| 256 | 222 | 49 | 11 | 866 / 848 | 14.2 / 14.2 |
| 512 | 362 | 331 | 41 | 1128 / 1096 | 14.1 / 13.9 |

Under about 128 MiB the floor keeps every pass from returning anything; `t1` still changed the
advice there, and `g1`/`r2` do not (below).

### Servers

One binary, the switch off and on, `servers-thread` (fill, 30 s at 70%, a 30 s silence, a
burst marked as its first 5 ms, 50 ms and 500 ms, a minute after close) and the `ht-bursts`
probe suite (four bursts after 6 s lulls at a fiftieth of the 70% rate, so slow paths run and
frontiers age; `runner_edit.py`). Huge pages sampled from `smaps_rollup`.

| job | arm | runs | Mcycles a request | s70 p99.9 ms | first 5 ms of the burst, max ms | heap / a minute after close MiB | advice calls, MiB returned |
|---|---|---|---|---|---|---|---|
| log store 1M | main | 12 | 0.34-0.36 | 0.1 | 0.4-0.8 | 1424-1442 / same | |
| | `t1` | 8 | 0.34-0.35 | 0.2-0.4 | 0.6-2.5 | 1427-1442 / same | 50-60 a run, 0-6 |
| | `t1g1` | 2 | 0.36 | 0.1 | 0.6-0.7 | 1432-1434 | 0, 0 |
| | `t1r1` | 2 | 0.35 | 0.2-0.8 | 1.9-2.7 | 1430 | 0, 0 |
| | `t1r2` | 6 | 0.34-0.35 | 0.1-0.3 | 0.5-2.0 | 1423-1438 / same | 0, 0 |
| | `t1p30` | 4 | 0.35-0.36 | 0.1 | 0.4-0.7 | 1422-1434 | 0, 0 |
| log store, 6 s lulls | main / `t1` | 2 each | 0.34 / 0.34 | 0.1 / 0.2-0.4 | 0.2-0.3 / 0.2 | 1428-1438 | ~55 a run, 0 |
| order service 2 GiB | main / `t1` | 4 each | 0.07 | 0.0 | 0.1-0.2 | 2169-2192, 47-91% huge | none |
| order service, 6 s lulls | main / `t1` | 2 each | 0.07 | 0.0-0.4 | 0.1 | 2186 | ~50 a run, 0 |
| orders HTTP/2 | main / `t1` | 3 each | 0.059-0.060 | 0.04 | | 203 | none |
| Meilisearch | main / `t1` / `p30` | 1 each | 16.4 / 16.7 / 16.5 | 45 / 40 / 47 | 2.0 / 3.0 / 2.8 | 715 / 659-666 | none |

The burst's first 5 ms holds a few hundred requests, so its maximum is one request. Two
things are clear and one is not. `t1`'s advice changes moved the store's s70 p99.9 in every
run. `t1r2` changes no advice there and returns nothing, and its cycles and p99.9 are main's
within a run's noise. But the burst maximum of the variants that walk the units above the
frontier as small pages (`r1`, `r2`) is 1.2 to 2.7 ms in 6 of their 8 runs, against 0.4 to
0.8 in all 12 of main's and in all 6 of `g1`'s and `p30`'s, which do the same work as main
there. The likely cause is that walk itself: in a step that may return memory it reads the
bitmap of every unit above the frontier page by page (a call took up to 0.6 ms on the store)
where main's look stops at a unit's first word in use, and the background thread spends
that time on the server's CPUs as a burst begins. Not confirmed; the fix is cheap (below).
The fast ramp (`tree depth=25`, 1.5 GiB of small nodes built, dropped and rebuilt three
times, two rounds): build 2.41 to 2.51 Gcycles main, 2.39 to 2.44 with `t1`, the same faults
and walks; the rule never acts within a ramp.

### Without the background thread

4-phase lifetime job with `ASMALLOC_BACKGROUND=0`: main 3.74 / 1524 MiB / 599 cycles; `t1`
2.98 to 3.01 / 322 to 340 / 605 to 607, with 3 times the advice calls (399 to 593 down, 370 to
544 up). On a request's slow path a split averaged 40 to 60 us and the worst was 0.9 to
1.7 ms; the longest walk call in a region with a tail 1.0 to 1.1 ms, whether a split counted as
nothing or as 128 pages (`k128`), since one slow split is the worst case either way.

## What it costs

- **Time on the job:** 1.0 to 1.5% cycles per operation on the lifetime job with huge pages
  plentiful and swings of tens of seconds, from the refilled range on 4 KiB pages (4 KiB walks
  +15 to 50%, 2 MiB walks -35 to -60%). Nothing measurable with 3 s swings, with scarce huge
  pages, or on the servers.
- **The background thread's time:** 0.5 to 0.6 s in a 70 s run (under 1% of a CPU), 5.6 s in
  the 20-minute run. A return in the tail averages 4 to 6 us and 1.5 pages: free runs in a tail
  are short while short-lived objects are still dying in it.
- **Worst calls**, on the thread: a split up to 4 to 5 ms and a raise up to 3 ms on the
  measurement CPUs (up to 8 and 6 ms on busy ones); `t1r2`'s raises averaged 3 us.
- **Raise lag**, from the claim's mark to the advice being back: mean 0.2 to 0.9 ms, at most 3
  to 15 ms; with 16 MiB of headroom no fast ramp met it.
- **khugepaged** rebuilt 16 to 69 units a timed run and 1134 in the 20-minute run under the
  advice (mostly units fills had made dense again, but the count does not separate them).

## Surprising

- Without the split RSS looked returned (troughs 3.14) while `memory.current` stayed (3.93).
- 2 MiB faults are slow on this host (p50 135 us, p99.9 9 ms, compaction under
  `defrag=madvise`), so an advice change can wait milliseconds behind them.
- No allocator here returns most of a 45 s trough: at 512 MiB live, main, jemalloc and glibc
  all keep over 1.9 GiB. The rule brings asmalloc under both.
- With the fixed table the rule acts as much as with the wandering one: every class's tail
  drops in a trough. The drift needs wandering demand; the trough's memory does not.
- The first form's cost on the store came from advice changes that returned nothing, not from
  refaults: no request faulted.

## Unresolved

- **Huge pages back sooner.** Nothing rebuilds a refilled unit faster than khugepaged. A
  `MADV_COLLAPSE` by the background thread of units fills have made dense again costs about
  6 ms a unit; telling dense units from sparse ones cheaply is not solved. Not built.
- **The zone walk's cost at a burst start** (above). Confirmed and fixed in the product: see
  "The product".
- **Server-scale swings** (hours, a day) were not run; the choice there is the same step
  function.
- **Qdrant and SurrealDB on `t1r2`** are with `real-servers` (tree `target/huge-tail-ht10`).

## Proposal, ranked

1. **`t1r2`: huge pages only where memory has not gone back.** The clocked purge, in a step
   that may return memory, walks the part of a huge-page part above the frontier as small
   pages, forces the split before it returns part of a unit, returns wholly free units whole,
   and takes the advice off once per region per pass over exactly the units it returned part
   of. A claim above a lowered boundary has the thread or a heartbeat put the advice back to
   the frontier and 16 MiB beyond. Before the product build, the walk above the frontier
   should take a cheap look at a unit before reading it page by page (Unresolved), and the
   store's burst start be measured again. No constant of its own beyond the headroom; the floor and
   the pass's pace bound its advice changes as they bound its returns. On a request's slow
   path a split counts as a whole step's pages (128), since it costs about what a step of
   returns does (40 to 60 us, 128 pages at 0.1 to 0.4 us), so a step makes at most one.
2. **The same with patience N** keeps huge pages through troughs shorter than N to 2N
   intervals. It did nothing on Qdrant at 30 and 120 and bought at most 1.5% on the synthetic
   swing; it stays a measured alternative.
3. **Not:** no split (returns RSS, not the charge), the advice back to the region's end
   (khugepaged refills the sparse tail), the claim changing the advice itself (a write lock on
   a request's path), lowering with the frontier (`t1`: advice changes that return nothing).

## Suggested design text

For "Returning memory and huge pages", replacing "Never split a huge page":

> - **Huge pages only where memory has not gone back.** A region's huge-page part above its
>   frontier holds what fills left behind, a few objects to a huge page, and returning only
>   whole huge pages keeps it all. So a step of the clocked pass that may return memory walks
>   that part as small pages. Before it returns part of a huge page it asks the kernel to
>   split it: pages returned from a huge page that is not split leave the resident set but
>   stay charged to the cgroup until reclaim splits it, and the thread that meets the limit
>   pays tens of milliseconds. A huge page wholly free goes back whole. Once the walk of the
>   region is done, the advice comes off, in one call, over the huge pages it returned part
>   of, so that khugepaged does not rebuild them; advice that comes off where nothing went
>   back is a lock on the address space for nothing, and a server whose classes wobble paid
>   for it in its tail. When a claim reaches above that boundary, the background thread, or a
>   heartbeat where there is none, puts the advice back up to the frontier and a few huge
>   pages beyond, never further: under the advice khugepaged rebuilds any huge page with one
>   page in use. A split huge page that fills reach again is small pages until khugepaged
>   rebuilds it, which costs a program that swings by gigabytes every few tens of seconds
>   about a percent of its time (`docs/research/huge-tail.md`). A split costs a slow path
>   what a step of returns does, and counts as one. Never split a huge page that fills still
>   reach.

In the table, the clocked purge's row gains "and, in a huge-page part above the frontier, runs
of free pages, splitting their huge pages"; the row "the part of a huge page above a region's
watermark" stays.

## The product

2026-10-25, branch `huge-tail-rule` (`0cd82ec`), against main `3b5a3f5`, each built as it is
(`build.sh` with `HT_EDIT=0 DRIFT=none`), no switch. The rule is `t1r2` with four changes,
each of which can move a server's memory against `t1r2`'s:

- **A cheap look first.** Every step looks at a huge page above the frontier once, at its
  top: wholly free, it goes back whole; otherwise 8 of its pages are sampled, one in each
  eighth, at a place the interval's number picks. It is walked page by page only if 2 of the
  8 are free and resident, and only by a step that may return memory. `t1r2` walked every one
  page by page, in counting laps too.
- **One split a step on a slow path only.** The background thread's steps split as many as
  their budget reaches.
- **The advice comes off when a pass or a growth payment ends in a region**, as well as when
  the walk reaches the region's bottom.
- **A raise that would end past the watermark's huge page goes to the region's end.**

### The burst start

A trace of every walk call with its thread, CPU and duration (`ht.rs` `HT_TRACE`,
`burst.py`) showed it: in the first 10 ms after the 30 s silence the woken background thread
ran a counting lap on a server CPU, 263 calls in 0.46 ms with main and 1050 to 1096 in 1.7 to
1.8 ms with `t1r2`, because the lap read every huge page above the frontier page by page. No
request's slow path walked. With the look first, on the store, 12 runs each (eight at
`02cfdcd`, whose look sampled pages at fixed places, three of those at code pads of 16 to
48 bytes): the burst's first 5 ms max 0.5 to 0.8 ms with main and 0.5 to
0.9 with the product, s70 p99.9 0.1 ms in all.

### How the look samples

Three forms were measured on the 4-phase lifetime job (troughs; main 3.82):

| look | troughs | THP at peaks MiB | cycles/op |
|---|---|---|---|
| 8 leaf words, half wholly free | 3.81 | 1524 | 595 |
| 8 runs of 4 pages, 1 free (moving places) | 3.53 | 1116 | 597 |
| 8 pages, 2 free (moving places) | 3.25 | 768 | 601 |
| `t1r2`, no look | 3.11 | 404 | 602 |

By words the look missed nearly everything: a leaf word of a 1 KiB class covers 16 pages,
so a huge page with a live slot every 16 pages has no free word, and the run made one split.
By aligned runs of four (the walk's shortest) it missed free runs that are not aligned. The
look that ships samples pages. What it leaves against `t1r2` is huge pages with less than
about a quarter of their pages free.

### Against main

| job | main | product |
|---|---|---|
| lifetime, 4 phases, plentiful huge pages: troughs, THP at peaks, cycles/op | 3.82, 1522 MiB, 595 | 3.25, 768 MiB, 601 |
| the same, fixed size table, scarce huge pages (`02cfdcd`) | 3.72, 584 MiB, 615 | 3.17, 314 MiB, 618 |
| lifetime, no background thread (`02cfdcd`) | 3.72, 580 MiB, 611 | 3.14, 382 MiB, 615 |
| fast ramp (`tree depth=25`), build / walk Gcycles, 2 rounds | 9.06, 8.81 / 7.98, 8.00 | 9.72, 9.02 / 8.56, 8.53 |
| log store 1M, Mcycles a request at s70, pad 0, 8 runs (4 at `02cfdcd`) | 0.336 to 0.345 | 0.347 to 0.353 |
| the same, mean over pads 0 to 48, 1 run each (`02cfdcd`) | 0.345 | 0.348 |
| order service 2 GiB (`02cfdcd`), 2 runs | 0.068, p99.9 0.0, heap 2178 MiB | the same |
| Meilisearch, 3 runs: cycles, peak / a minute after close MiB | 16.5-16.6 M, 746-758 / 655-660 | 16.6 M (one 13.4), 748-767 / 658-663 |

Memory only, one run each, under `scripts/heavy` on a busy machine (their cycles are not
comparable):

| job | main | product | others |
|---|---|---|---|
| lifetime, 24 phases, wandering table: peaks first, last 3; troughs; quiet; THP at peaks | 1.101, 1.226; 4.29; 4.29; 1652 MiB | 1.090, 1.146; 3.35; 3.26; 662 MiB | `t1` 1.104, 1.145; 3.21 |
| the same, fixed table | 1.101, 1.107; 3.77; 3.76 | 1.101, 1.099; 3.22; 3.12 | `t1` 1.100, 1.097; 3.13 |
| size shift, end of each median-384 phase | 1.506 to 1.515 | 1.359 to 1.373 | jemalloc 1.183 to 1.194, glibc 1.328 (first cycle; the run hit its time limit) |
| the same, median 1024; quiet minute | 1.173 to 1.227; 1.168 | 1.141 to 1.147; 1.134 | jemalloc 1.253 to 1.278; 1.228 |

mini-redis (heap-drift's run, under `scripts/measure --both-l3`, one run each), resident over
live at the end of each phase:

| build | wide phases | small phases | quiet |
|---|---|---|---|
| main | 1.11, 1.33, 1.32 | 2.32, 2.86, 2.45, 2.71 | 1.31 |
| product | 1.11, 1.17, 1.16 | 1.97, 2.29, 2.08, 2.02 | 1.20 |
| jemalloc | 1.11, 1.16, 1.16 | 2.55, 2.65, 2.44, 2.58 | 1.16 |
| glibc | 1.13, 1.16, 1.29 | 4.69, 4.74, 4.84, 5.27 | 1.28 |

khugepaged rebuilt 1091 huge pages in the product's wandering run (none with main), as it did
under `t1`: units fills had made dense again.

- **The ramp's walk phase** allocates nothing; the product's 7% is placement: the same
  instructions (14.11 G), the same TLB walks and faults, cycles only.
- **The store's cycles** are placement too: the same instructions a request (721.5 to 722.7
  k), and over the pads main alone spans 3.8%. One round a pad; the 0.8% between the means is
  within that.
- **Meilisearch's burst** has a slow first half second (b500 max 20 to 28 ms against 4.2) in
  some runs of both builds: main 4 of 12 and the product 5 of 12 over all its versions, the
  final product 0 of 3. Traced (first-touch's `wl_edit.py`), the product made no split and no
  advice change in Meilisearch, so the rule has no part in it.

### The raise and the address-space lock

On the store, the order service and Meilisearch the product never raised: on the store it
split 85 huge pages and lowered once, all after the load closed; on the others nothing.
On the lifetime job (strace, cores shared with heavy work) a raise took a median of 43 us and
at most 5.7 ms, a lowering 9 us and at most 5.7 ms, a split 8 us and at most 5.8 ms; these
are the caller's times, and the caller is the background thread, or a heartbeat where there
is none. Whether other threads' faults queue behind a waiting raise needs a server that
raises: Qdrant. `wl_edit.py` applies to the product's tree as it does to main's.

### The protocol

`tla/Tail.tla` models the region's tail word: the claim's mark, the walk's note, the owner
bit taken by one compare-and-swap (a loser skips), the kernel call that may be refused or
partly applied, the stale owner's takeover, the reconcile, and a fork that leaves a call
half made. TLC 2026.10.24 (rev `2ef2af8`): `Tail.cfg` holds (17.7 M distinct states, 6
minutes). Two findings on the way, both fixed: an owner that took the word cleared a repair
request that a refused call had left, so the kernel and the word could disagree for ever
(`Tail_norepair.cfg` shows it); and a fork child inherits a word whose owner's call never
finished in the child, so the child's advice could disagree with its word
(`Tail_orphan.cfg`, without the child's repair). The child now marks every tail word for
repair as its first heap call.

### The numbers the rule needs

- **Headroom of a raise, 8 huge pages (16 MiB).** From the measured raises: none made a
  growing class move its boundary about ten times an interval (716 a run), 16 MiB 145, 64
  MiB 97, with no change in memory. It bounds what khugepaged may rebuild; fixed, since it
  answers a class's growth in an interval, not the region's size.
- **8 samples, 2 free.** The samples choose which huge pages to count; fixed, since a look
  is per huge page.
- **A quarter free, by the walk's count** (`TAIL_SPLIT_SHARE`, from the review below): the
  cut below which a huge page is not split. Fixed, for the same reason.

### After the review (2026-10-26)

The adversarial review of `3c2265e` found no memory-safety defect and ten broken promises;
`d8c8fad` answers them. What changes what is returned: a counting step resumed inside a huge
page no longer returns its rest; the sample's places move with every lap, in a silence too;
and the split is decided by the walk's exact count of free, resident pages, a quarter of the
huge page or more, where before two of eight samples decided it, a rate that in time split
huge pages a twentieth free. The count reads what a walk of the whole huge page reads: its
slots' leaf words (32 for 1 KiB slots, 2048 for 16-byte ones) and a clean word a page, only
for huge pages the sample calls sparse. Refused calls wait for the next interval, a request's
heartbeat raises one region a step, an owner that does not run no longer keeps the background
thread awake, and the advice never comes off below the frontier as it stands.

Product before (`0cd82ec`) and after (`d8c8fad`) against main, one run each:

| job | main | before | after |
|---|---|---|---|
| lifetime, 4 phases: troughs, THP at peaks, cycles/op | 3.82, 1526 MiB, 593 | 3.28, 776 MiB, 601 | 3.31, 888 MiB, 600 |
| lifetime, 24 phases, wandering: peaks last 3, troughs, quiet | 1.227, 4.30, 4.29 | 1.145, 3.37, 3.28 | 1.155, 3.48, 3.45 |
| mini-redis, wide phases | 1.11, 1.33, 1.33 | 1.11, 1.17, 1.17 | 1.11, 1.17, 1.18 |
| mini-redis, small phases | 2.42 to 3.02 | 2.16 to 2.69 | 1.76 to 2.05 |

The cut keeps huge pages under a quarter free that the rate split in time: the 24-phase
troughs give back 0.11 of what the product had won (3.37 to 3.48, against main's 4.30). The
4-phase job and mini-redis's wide phases are the same. Its small phases ended lower after
the fixes; one run each, and not explained.

### The accounting and the headroom (2026-10-26)

Branch `huge-tail-accounting`. One run each; the 4-phase job, the store and mini-redis under
`scripts/measure`, the 24-phase job and the call counts under `scripts/heavy` on a busy
machine (their cycles are not comparable).

**The accounting** (`ae24bca`, measured alone against stage 1, `a16d05a`). A raise, or a
note the frontier has passed, now counts every free page of each non-empty huge page it
leaves under the advice as resident again, and a raise counts as a fill's reach over its
range. On the 4-phase job the troughs and cycles did not move (3.30 against 3.28; 601
against 597 cycles/op). On the 24-phase job the troughs went from 3.50 to 3.41, the peaks
from 1.156 to 1.139, and khugepaged rebuilt 645 huge pages against 1423. mini-redis's wide
phases were the same (1.16 to 1.18). The allocator's count against the resident free memory
mincore sees, at the end of the 4-phase job: stage 1 counted 4.4 MiB less than was
resident; the accounting counts 78.5 MiB more, pages it called resident that khugepaged had
not yet rebuilt in that run. An overcount costs a later pass a call on pages that are not
there and goes away with it; an undercount stayed. Advice calls on the 4-phase job: raises
96 to 170, lowerings 183 to 169. On Qdrant the gap the product left after close closed
(`docs/research/servers-qdrant.md`, dated section).

**The review's second round** (`dee220a`) changes what is returned too: above the
boundary a step that may return memory walks the split huge pages with neither the look nor
the cut, so the pass for short runs takes what is left. The 24-phase troughs went to 3.18
(peaks 1.136), the 4-phase troughs were 3.34. It doubled the system calls: on the 4-phase
job `MADV_DONTNEED` 108 thousand to 231 thousand and `MADV_COLD` 74 thousand to 214
thousand, at the same cycles/op (601). The split before each return is kept above the
boundary, since a lowering at the frontier's huge page may take the advice off over huge
pages never split.

**The headroom**, two arms against the constant 8, on that product (`7c06da3`): (i) the
frontier's rise over the last interval, at least one huge page; (ii) grow on regret, a raise
within 4 intervals of the region's last doubling the headroom from one huge page, a lowering
halving it, kept in a per-region word beside the tail words.

| | constant 8 | (i) rise | (ii) regret |
|---|---|---|---|
| 4-phase: troughs, cycles/op | 3.34, 601 | 3.24, 603 | 3.34, 599 |
| 4-phase: raises (`MADV_HUGEPAGE`, with main's own ~20) | 202 | 174 | 170 |
| 24-phase: peaks last 3, troughs | 1.136, 3.18 | 1.137, 3.24 | 1.136, 3.19 |
| mini-redis wide phases | 1.16 | 1.15 to 1.16 | 1.16 to 1.17 |
| store: Mcycles a request, burst max ms (2 runs) | 0.35, 0.6 | 0.35, 0.6 to 0.7 | 0.35, 0.6 |

Neither arm raised more often than the constant: the case of a class that has just started
growing, which the 4-phase job's high phases are, did not show the 716 raises a run that no
headroom at all made, since a raise follows a look for marks, at most a heartbeat's or a
thread's step apart, and a floor of one huge page is enough there. Neither did better on
memory beyond a run's noise ((i) better on the 4-phase troughs and worse on the 24-phase
ones). So the constant stays: the regret arm matches it with a word a region more, and the
rise arm is not consistently better.

## Method and data

- **Builds.** `build.sh` extracts main, applies heap-drift's `bench/` changes
  (`553661020a6d..4522abd`) and the switch (`ht_edit.py`, `ht_proto.py`, `ht.rs`), and builds
  the drift workload plain and with the memstat sampler, the RPC servers, the order service
  and Meilisearch, or jemalloc's and glibc's workload from the same tree. Builds: `ht0`, `ht1`
  (`ed0e1b7`, the frontier runs), `ht4` to `ht10` (`81437b0`; each adds a setting, none
  changes an earlier one's meaning). `KEEP=1` kept `ht5` (`t1`, `p`) and `ht10` (`r2`) for
  `real-servers`.
- **Sessions.** `sessions/kern.sh`, `swing.sh` and `swingsize.sh` under `scripts/measure
  --both-l3`, CPUs `0-3,8-11`, with 4 KiB and 2 MiB walks counted; `long.sh` under
  `scripts/heavy`; `servers-watch.sh` under `scripts/measure --both-l3`, with local
  `allocators.toml` entries (README). Each drift run records the free 2 MiB blocks, the
  cgroup's huge-page faults and collapses, and the machine's fallbacks.
- **The product.** `build.sh` with `HT_EDIT=0 DRIFT=none` builds a commit as it is (`PAD=N`
  for a code pad); `sessions/product-timed.sh`, `servers.sh` (local entries `b-main`,
  `b-prod`), `product-wl.sh` (first-touch's trace, entries `wl-main`, `wl-prod`) under
  `scripts/measure --both-l3`; `product-memory.sh` under `scripts/heavy`; `burstwl.py` lines a
  burst's requests up with the traced calls.
- **Counters.** `d1` writes the switch's counters to `HT_DIAG`; `htsumm.py` reads them (cycles
  at the 4.3 GHz TSC).
- **Data.** `docs/research/data/huge-tail-2026-10-25.tar.gz`: the kernel probe's outputs,
  every drift run with its counters, the memstat breakdowns, the server records with their
  huge-page samples. `docs/research/data/huge-tail-product-2026-10-25.tar.gz`: the product's
  runs (`prod/`), with the address-space traces but not the per-request records.
