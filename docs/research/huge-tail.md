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
  then returns nothing and changes no advice. One cost is open: its walk above the frontier
  reads those units page by page, and on the store the first milliseconds of a burst were
  slower in 6 of 8 runs of the walking variants (1.2 to 2.7 ms against 0.4 to 0.8); a cheap
  look before the walk should remove it, and is not yet measured.
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
- **The zone walk's cost at a burst start** (above). A step should look at a unit above the
  frontier cheaply before walking it as small pages: a few of its leaf words, walking it only
  if most are wholly free, and not at all where less than the 16 MiB a lowering needs lies
  above the frontier. Not built or measured.
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
- **Counters.** `d1` writes the switch's counters to `HT_DIAG`; `htsumm.py` reads them (cycles
  at the 4.3 GHz TSC).
- **Data.** `docs/research/data/huge-tail-2026-10-25.tar.gz`: the kernel probe's outputs,
  every drift run with its counters, the memstat breakdowns, the server records with their
  huge-page samples.
