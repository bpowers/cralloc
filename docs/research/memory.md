# Where asmalloc's memory is, and what would bring it under jemalloc

2026-10-17. An adversarial review of the allocator's memory footprint, by measurement and by
reading. Speed was not the subject; where a proposal has a speed cost, the note says whether
it was measured.

The mechanisms it proposed have since been built. What they achieve is the first section
after the summary; the rest is the review as it was written, with its measurements of the
allocator before the change and of prototypes.

## Summary

- **With the mechanisms built**, asmalloc peaks at or under jemalloc on every benchmark
  workload of 10 MiB or more (JSON is 0.4% over), and after a 218 MiB burst it is at 4.3 MiB
  in twelve seconds where jemalloc is at 4.5 and it used to hold 147 for ever. It is still
  0.5 to 1 MiB over in the smallest processes and 2 MiB over on the mixed-size pipeline,
  which is the 128 KiB magazine budget. The budget is now 64 KiB, which puts the pipeline at
  jemalloc's memory ("Magazine budget", below).

What the review found, before the change:

- **At the peak of the benchmark workloads asmalloc is already close to jemalloc** now that
  the idle check is fixed: within 1 MiB on most, under on fork-join, hash map, tree and mixed
  churn, over by 11 MiB on Vec growth and by about 2 MiB on the mixed-size pipeline. Most of
  the first smoke test's excess was the idle-drain bug (fork-join confined to four CPUs: 32.7
  MiB with the bug, 15.9 with a wall-clock idle check, jemalloc 22.2).
- **After the peak asmalloc is far worse.** A program that frees 218 MiB and keeps working
  lightly holds 149 MiB indefinitely; jemalloc is at 4.5 MiB after ten seconds and mimalloc at
  16. Four independent defects cause it, and each hides the next:
  1. The 64 MiB retained cap is not a bound. A purge pass that finds nothing wholly free
     resets the count to zero, and pages that become free later are never counted again
     (measured: 136 MiB free and resident, count 0.01 MiB).
  2. Objects parked in magazines pin the pages they lie on, and nothing moves parked objects
     back to the bitmaps unless their own class is being served. Half a megabyte of parked
     objects pinned 50 MiB.
  3. Huge pages make each stray object pin 2 MiB instead of 4 KiB, keep up to 2 MiB resident
     above each region's watermark, and make a sparsely touched large block resident in
     full (256 one-megabyte blocks with one page written each: 246 MiB against jemalloc's 2.9).
  4. All maintenance hangs off slow paths, and a thread can allocate and free millions of
     objects a second without entering one. Then nothing is laundered, purged or drained.
- **Prototypes of five mechanisms, together, come close to jemalloc** on the after-peak
  test (4.5 to 9.6 MiB without huge pages, 15 with them for classes up to 1 KiB, against 4.5)
  and put the peaks at or under jemalloc's on every workload measured except JSON and the
  1M-object churn, which stay about 1 MiB over because of huge pages.
- **Where per-CPU caches already win.** Sixteen threads on two CPUs that each touch every
  class: asmalloc keeps 12 MiB, jemalloc 52, mimalloc 28.

## What the finished mechanisms achieve

Measured 2026-10-17 on main at 6803b5b, after the mechanisms this note proposed were built
properly (sections 1 to 5 below are the review that led to them, measured earlier the same
day on prototypes, and are kept as written). Same method. The machine was busy throughout:
load average 18 to 29 on 32 CPUs. That does not move memory figures much, but times mean
nothing and are not given.

What was built: the retained counts that are never reset, the 8 MiB cap for freed large
slots, and the clocked top-down purge of a quarter an interval; huge pages only for classes
up to 1 KiB, after 64 MiB carved, with the watermark's huge page purgeable; a loaded magazine
aged out after an interval, each CPU's full spares swept once an interval, a depot stack
shedding the depth it did not need, and one retiring magazine in 32 laundered; `collect` and
`malloc_trim` doing all of it at once; a large block that shrinks in place returning its
tail, and one that moves copied in 2 MiB pieces with the old pages returned behind the copy.
Magazines still hold up to 128 KiB of objects; that question was deferred.

### Peaks

Cgroup peak, MiB. asmalloc and jemalloc are three runs each, lowest to highest; mimalloc is
one run.

| Workload | asmalloc | jemalloc | mimalloc |
|---|---:|---:|---:|
| pipeline, 2 threads, small | 3.0 to 3.2 | 2.0 | 7.5 |
| pipeline, 2 threads, mixed sizes | 7.5 to 7.7 | 5.2 to 5.7 | 9.5 |
| exchange, 2 threads | 3.0 to 3.1 | 2.3 to 2.5 | 7.2 |
| mpmc 3+1 | 4.5 to 5.0 | 4.0 to 4.2 | 8.5 |
| fork-join, 4 threads | 15.7 to 16.9 | 16.5 to 17.3 | 25.4 |
| spawn, 4 threads | 3.4 | 2.7 to 2.9 | 10.5 |
| churn, 1k live | 1.1 to 1.4 | 0.7 to 1.2 | 4.7 |
| churn, 1M live | 107.9 to 108.0 | 110.1 to 110.6 | 117.2 |
| churn, 100k live, mixed sizes | 34.5 to 34.8 | 36.8 | 52.7 |
| BTreeMap 200k | 25.9 to 26.0 | 25.8 to 26.0 | 26.8 |
| HashMap 200k | 32.5 to 32.8 | 34.6 to 34.9 | 36.8 |
| tree, depth 18 | 13.1 to 13.2 | 17.3 to 17.6 | 19.0 |
| Vec growth, 1 x 64 MiB | 67.5 to 67.9 | 72.0 to 72.4 | 139.0 |
| Vec growth, 64 x 4 MiB | 269.7 to 270.0 | 305.4 to 305.5 | 263.3 |
| JSON 8 MiB | 89.3 to 89.5 | 88.9 to 89.2 | 97.0 |

Every workload of 10 MiB or more is at or under jemalloc, except JSON, which is 0.4 MiB
(0.4%) over. Vec growth went from 11 MiB over to 4.5 under, the 1M-object churn from 1 over
to 2.4 under.

### What is still above jemalloc, and why

- **The smallest processes: 0.5 to 1 MiB over** (pipeline 3.0 against 2.0, exchange 3.0
  against 2.4, mpmc 4.7 against 4.0, spawn 3.4 against 2.8). The sampler on the small
  pipeline at its peak: 1.6 MiB of heap, of which the program holds 0.5, parked objects are
  0.5, and metadata, magazine area and slabs are 0.55. Page tables are about 0.1 MiB more
  than jemalloc's. None of the new rules acts inside a run of a tenth of a second. This is a
  fixed cost of a few hundred kilobytes, not something that grows.
- **The mixed-size pipeline: 2.1 MiB over** (7.6 against 5.5), unchanged by the rules. Yes,
  it is still parked objects in 128 KiB magazines: at the peak the program holds 0.5 MiB,
  loaded magazines 2.4, the depot 2.2 and spares 0.1, out of 5.8 MiB of heap. The classes
  from 384 bytes to 8 KiB each have about 0.1 MiB loaded and as much in the depot. The run
  lasts 60 ms, so nothing ages out. The 32 KiB budget took this workload to 5.5 in the
  prototype (section 5, mechanism 5); that is the deferred question, and this is its cost.
- **The same thing at a phase change's first moment.** Eight CPUs using every class peak at
  30 to 50 MiB against jemalloc's 28, and two CPUs at 13.9 against 7.2. A second later the
  rules have begun to return it (below), but the peak itself is the magazine budget.
- **JSON, 0.4 MiB over**: the bookkeeping above on a heap whose live data is the same.

### After the peak

Free 218 MiB of small objects in random order, then allocate and free 2000 objects of 64
bytes every millisecond. Anonymous resident MiB:

| | at peak | 2 s | 4 s | 6 s | 8 s | 10 s | 12 s |
|---|---:|---:|---:|---:|---:|---:|---:|
| asmalloc | 211 | 52 | 30 | 23 | 13.5 | 8.2 | 4.3 |
| jemalloc | 218 | 142 | 126 | 59 | 12.5 | 4.4 | 4.5 |
| mimalloc | 226 | 226 | 226 | 226 | 14.8 | 14.8 | 14.8 |

Before the change asmalloc held 147 MiB for as long as the run lasted. Now it returns
memory sooner than jemalloc for the first six seconds (a quarter of what is free each
second, against jemalloc's curve that starts slowly), is level with it at eight, and ends
at the same place, 4.3 against 4.5. Its tail is slower: from 30 MiB down it loses about a
third every two seconds where jemalloc's curve finishes steeply, so between 9 and 11 s it
is a few megabytes above. The fraction and the interval are two constants.

### The worst cases

Anonymous resident MiB, eight seconds after the event unless a time is given.

| Case | asmalloc before | asmalloc | jemalloc | mimalloc |
|---|---:|---:|---:|---:|
| Burst then light work, at 12 s | 147 | 4.3 | 4.5 | 14.8 |
| Phase change: 8 CPUs use every class, then one CPU uses one | 30 to 49 | 10.0 | 24.7 | 15.8 |
| Many CPUs, few classes: 8 CPUs, 4 classes | 4.6 | 1.1 | 4.3 | 8.3 |
| Many classes, few CPUs: 2 CPUs, 44 classes | 10.9 | 1.6 | 4.1 | 6.2 |
| Many threads, few CPUs: 16 threads on 2 CPUs, 44 classes | 11.9 | 1.6 | 52.2 | 28.3 |
| Sparse: 256 blocks of 1 MiB, one page written each | 246 | 1.3 | 2.9 | 276 |
| Sparse: 4096 blocks of 64 KiB, one page written each | 247 | 16.4 | 33.2 | 344 |
| Three small objects | 0.2 | 0.2 | 0.3 | 2.2 |

The phase change was still falling at eight seconds (38, 21, 16, 10 at 2, 4, 6 and 8 s); it
is the slowest of these because seven idle slabs are drained one interval after they go
quiet, their magazines then wait in the depot for its low-water rule, and the purge takes a
quarter a pass.

### Large blocks that shrink or move

The soak's `bigmem` through the preload library, peak resident MiB, one run each
(`crates/asmalloc-preload/tests/programs/bigmem.c`; `chain` grows a block to 256 MiB by
`realloc` and shrinks it back, `grow` frees at the top; rounds, then threads):

| Mode | glibc | asmalloc before | asmalloc |
|---|---:|---:|---:|
| `chain 10 1` | 257 | 392 | 263 |
| `chain 10 4` | 1008 | 1178 | 1011 |
| `grow 10 4` | 1008 | 859 | 871 |

Not measured: a block written sparsely and then moved, which the piecewise copy makes
dense; the fix chosen is a copy that skips all-zero source pages (see the roofline note,
section 7).

### Still open

- The magazine budget, which is all of the mixed pipeline's gap and of the phase-change
  peak.
- A process in which no thread reaches a slow path returns nothing until it calls `collect`.
- Speed: none of this was timed, on this machine or any other.
- More than 8 CPUs, the RPC server, and runs longer than 12 s were not measured.

## Method

**Machine.** The reference machine (`zen5-roofline.md`): 32 CPUs, Linux 7.2, transparent huge
pages set to `madvise`, so an allocator gets huge pages only where it asks. The machine was
shared and busy; times from these runs are not reliable and are not reported.

**Peak memory** is the cgroup `memory.peak` of the measured process alone, as the benchmark
runner records it: `bench/probes/memory/peaks.sh` runs each workload under `scripts/memlimit`
and moves the process into a child cgroup of its own before it execs. The figure includes
page tables and about 0.5 MiB of the launching shell, the same for every allocator. Workloads
are the quick suite's (`bench/runner/src/suite.rs`), threads pinned to the suite's CPUs, the
process not confined unless stated. Each figure is one run. Repeats of the small workloads
differ by up to 1.5 MiB (jemalloc's mixed pipeline: 3.7, 5.1, 5.2), so differences under
1 MiB mean nothing.

**Where resident memory is.** `Asmalloc::inspect` (`crates/asmalloc/src/stats.rs`, with the
walks in `back/inspect.rs` and `depot/inspect.rs`) is a read-only walk of the regions and
bitmaps, the slabs and the depot. It reports every region with its watermark, every run of
free slots and every parked object with the place of its magazine. It writes nothing, never
allocates, contains no `unsafe`, and is a racy snapshot beside running threads. A sampler
thread in the benchmark's allocator crate (`bench/alloc/src/memstat.rs`, feature
`asmalloc-memstat`) calls it every 10 ms, asks the kernel which pages of each range are
resident with `mincore`, and attributes every resident byte to one use: held by the program,
parked in a loaded magazine, a spare or the depot, free in a bitmap, above a watermark,
metadata, magazine area, slabs. The sampler never calls the global allocator.

**After the peak.** New workloads in `bench/workloads/src/mem.rs` print the process's
anonymous resident memory, page-table memory and huge-page-backed memory from `/proc/self`
(`bench/probes/memory/memrun.sh` runs them):

- `quiet`: allocate objects, free them all in random order, then run light work for some
  seconds;
- `touch`: every thread allocates and frees objects of every small class, then all but one
  thread block;
- `sparse`: allocate large blocks and write the first page of each;
- `tiny`: allocate three objects.

**Baseline.** "asmalloc" is main with the wall-clock idle check (c5de4fd and later). The
peak table, the worst-case table in section 5 and the prototype comparison were measured on
it. The breakdowns by use and the step-by-step prototype ladder were measured just before
that fix was merged, with a stand-in for it (a slab is drained after about a second of the
cycle counter without activity); totals agree with the re-runs. "Before the fix" means main
at bfd2a80.

**Prototypes** are cargo features of the allocator crate (`x-*`, in `crates/asmalloc/src/xp.rs`),
each built far enough to measure. They are not on main: they live on the branch
`worktree-agent-a12def658c2e358ac` (tip e861620) for reference, and the builds named `lean`,
`age` and `age-thpoff` below need that branch.

## 1. Where the memory is

### At the peak

Cgroup peak, MiB.

| Workload | asmalloc | all prototypes | jemalloc | mimalloc |
|---|---:|---:|---:|---:|
| pipeline, 2 threads, small | 2.7 | 2.5 | 2.0 to 2.5 | 5.7 |
| pipeline, 2 threads, mixed sizes | 7.2 | 4.7 | 3.7 to 5.5 | 9.7 |
| exchange, 2 threads | 2.7 | 3.2 | 2.8 to 3.0 | 5.2 |
| mpmc 3+1 | 4.2 | 4.0 | 3.2 to 4.0 | 7.7 |
| fork-join, 4 threads | 16.5 | 15.4 | 22.0 to 27.3 | 22.5 |
| spawn, 4 threads | 3.2 | 2.9 | 2.5 to 3.1 | 12.0 |
| churn, 1k live | 1.1 | 1.4 | 1.0 to 1.2 | 4.7 |
| churn, 1M live | 111.3 | 111.0 | 110.5 | 117.0 |
| churn, 100k live, mixed sizes | 34.4 | 33.5 | 36.9 | 52.7 |
| BTreeMap 200k | 25.9 | 25.7 | 25.8 | 26.8 |
| HashMap 200k | 32.6 | 32.4 | 34.6 | 37.1 |
| tree, depth 18 | 15.1 | 14.9 | 17.2 | 19.0 |
| Vec growth, 1 x 64 MiB | 83.5 | 67.7 | 72.1 | 139.3 |
| Vec growth, 64 x 4 MiB | 304.9 | 260.2 | 305.5 | 265.0 |
| JSON 8 MiB | 90.1 | 89.8 | 88.9 | 96.8 |

"All prototypes" is the `lean` build of section 5. jemalloc's ranges are over three to five
runs of the session; the mimalloc column is from one earlier run. The process confined to
its CPUs, as the runner does it: fork-join 32.7 before the fix, 15.9 after, jemalloc 22.2.
The RPC server was not measured here.

No single-threaded job is far from jemalloc except Vec growth. The largest relative gaps
are in the smallest processes, where 0.3 MiB is 15% and is inside the noise.

Resident heap by use at the moment the program held the most, MiB (sampler):

| Workload | held by program | parked | free in bitmaps | above watermark | metadata, magazines, slabs |
|---|---:|---:|---:|---:|---:|
| churn, 1M live | 84.2 | 3.2 | 18.9 | 3.3 | 0.9 |
| Vec growth, 1 x 64 MiB | 51.8 | 0.1 | 18.3 | 0.0 | 0.3 |
| JSON | 74.3 | 1.4 | 11.6 | 1.1 | 1.0 |

What the excess over jemalloc is made of:

- **Vec growth (+11 MiB): freed large slots.** A buffer that outgrows its slot moves to one
  four times larger, and the slot it leaves stays resident until 64 MiB of them have
  accumulated. A freed 16 MiB slot can serve only a 16 MiB request, because a class owns its
  region; jemalloc splits and joins freed extents and grows in place.
- **Churn 1M, JSON (+1 MiB): the huge page at the watermark.** Once a class has carved
  8 MiB its region asks for huge pages, and the first touch of a huge page makes all 2 MiB
  resident, including the part above the watermark. 3.3 MiB in churn, 1.1 in JSON. Without
  huge pages churn is 107.9 against jemalloc's 110.2.
- **Mixed pipeline (+2 to 3 MiB): parked objects.** A magazine holds up to 128 KiB of
  objects, and for classes of 640 bytes and more that is what it usually holds.

### At the end of a run

Resident memory almost never falls, so the end of a run shows what the peak turns into.
Resident heap by use after the workload has freed everything, MiB:

| Workload | loaded | spares | depot | free in bitmaps | above | metadata | magazine area |
|---|---:|---:|---:|---:|---:|---:|---:|
| pipeline, small | 0.52 | 0.11 | 0.45 | 0 | 0.02 | 0.14 | 0.25 |
| pipeline, mixed | 2.86 | 0.13 | 1.94 | 0 | 0.03 | 0.32 | 0.48 |
| fork-join | 0.74 | 0.46 | 2.55 | 9.54 | 0.04 | 0.21 | 0.74 |
| churn, 1M live | 0.26 | 0.12 | 2.91 | 102.9 | 3.29 | 0.30 | 0.56 |
| churn, mixed | 2.23 | 0.13 | 6.92 | 22.8 | 0.04 | 0.45 | 0.59 |
| BTreeMap | 0.43 | 0.13 | 3.05 | 20.8 | 0.03 | 0.23 | 0.58 |

Metadata is 0.15 to 0.5 MiB and the slabs 4 KiB per CPU number used; neither is a problem.

### A process that allocates almost nothing

`tiny`: 0.2 MiB anonymous and 0.11 MiB of page tables, against jemalloc's 0.3 and 0.07. One
region per class, 4 GiB apart, costs about three page-table pages per class in use; a
program using 30 classes pays about 0.4 MiB more page tables than jemalloc. Start-up is not a
problem.

## 2. After the peak

`quiet live=2M sizes=small`, then 2000 objects of 64 bytes allocated and freed every
millisecond. Anonymous resident memory, MiB:

| | at peak | 2 s | 6 s | 10 s | 14 s |
|---|---:|---:|---:|---:|---:|
| asmalloc | 218 | 147 | 147 | 147 | 149 |
| jemalloc | 218 | 142 | 56 | 4.4 | 4.5 |
| mimalloc | 226 | 226 | 226 | 15 | 16 |

By use at 14 s: 136 MiB free in the bitmaps, 9 above watermarks, 3 parked. The back end's
retained count read 0.01 MiB.

### The cap is not a bound

`back/purge.rs` counts freed bytes and purges when the count passes 64 MiB. A slot's bytes
are counted once, when it is freed. When a whole pass purges nothing, the count is set to
zero, on the reasoning that what it held is fragmentation. During a storm of frees in random
order that is true of each pass: every page still has a live neighbour. The pages become
wholly free a moment later, and by then their bytes are no longer in the count. The count
then stays under the cap however much is free. Free resident memory is unbounded; this run
ended with 136 MiB against a cap of 64.

### Parked objects pin pages

With the count kept honest and a purge pass run once a second (`x-purge-decay`), the same
run still ended at 149 MiB. The pass found almost nothing to purge: the last objects freed
sit in magazines, their addresses are a random sample of the heap, and each one holds its
page. In this run 0.5 MiB of parked objects pinned 50 MiB; one stack of 234 objects of 256
bytes pinned 31 MiB of huge pages.

Nothing moves parked objects on:

- The heartbeat launders only the depot stack of the class its slow path just served, and
  only when that stack is over its target. A stack that is under target, or whose class no
  slow path is serving, stays for ever. So does everything a drain of an idle slab sends to
  the depot.
- The idle check is per slab. A class that fell out of use on a CPU that is busy with
  another is never reclaimed. (The lead's reading is right.)
- Objects in steady circulation between a program and its magazines never return to the
  bitmaps either, so they stay wherever they were when the heap was at its largest. Fills
  prefer the lowest addresses, but a magazine that is exchanged is never refilled.

`touch threads=8` shows the first two. Eight threads use every small class once, seven
block, and one keeps allocating 64-byte objects:

| | after touching | 1 s | 8 s |
|---|---:|---:|---:|
| asmalloc | 28.9 | 29.1 | 29.1 |
| jemalloc | 27.8 | 26.5 | 24.7 |
| mimalloc | 16.3 | 16.3 | 15.7 |

In asmalloc 23 MiB sat in loaded magazines, 2.9 MiB per CPU. After a second the idle check
drained the seven idle slabs into the depot, which then held 24.7 MiB, far over its nominal
11 MiB, and kept it. The busy CPU kept 2.85 MiB in classes it no longer used. jemalloc is no
better here: an idle thread's cache is collected only by that thread's own allocation, and
it kept 24.7 MiB until the threads exited.

### Huge pages

- **Granularity.** In the huge-page part of a region purging works in whole 2 MiB units, so
  one parked or live object holds 2 MiB. Without huge pages the after-peak run above ends at
  16.7 MiB where with them it ends at 69 (both with the depot and class prototypes below).
- **Above the watermark.** Up to 2 MiB per class that has carved 8 MiB, resident and never
  purgeable, because purging looks only at units wholly below the watermark: 7 to 9 MiB in
  the after-peak run, 3.3 MiB in churn.
- **Sparse large blocks.** A large class also switches to huge pages after 8 MiB.
  `sparse n=256 size=1Mi touch=4096`: asmalloc 246 MiB, jemalloc 2.9, mimalloc 276. With
  4096 blocks of 64 KiB: 247, 33, 344. Buffers allocated with room to spare are common, and
  jemalloc does not ask for huge pages, so this is a regression against it by up to the
  ratio of huge page to touched bytes.

### No slow path, no maintenance

Every maintenance step runs on a slow path. With the after-peak run's light work spread
over twelve sizes, each burst stays inside the loaded magazines: two million allocations a
second and no slow path at all. Memory stayed at 221 MiB for the whole run with every
prototype on. jemalloc counts allocated bytes on its fast path and runs an event every
64 KiB; mimalloc's frees go to a list that only its slow path collects, so it reaches the
slow path about once a page. asmalloc's rule that nothing is on the fast path leaves it
with no tick of its own.

## 3. Worst cases by design

From the constants, checked against the runs above where a run exists. Figures are for the
64 KiB magazine budget and 128 KiB depot target chosen later ("Magazine budget"); the
128 KiB budget's are in brackets.

| What | Bound | Notes |
|---|---|---|
| One CPU's loaded magazines, all 44 classes full | 1.63 MiB (2.89) | 2.9 measured at 128 KiB |
| One CPU's full spares | 0.14 MiB | the 1 MiB budget is split 44 ways, so only classes up to 80 bytes get any |
| Magazine memory per CPU | 0.27 MiB (0.28) | never unmapped; a magazine of 16-byte objects is 2 KiB for 4 KiB of objects |
| All CPUs, every class touched | 57 MiB at 32 CPUs, 454 MiB at 256 (97, 775) | reaches the depot after a second idle, and stays there |
| Depot | no bound | 128 KiB a class (5.5 MiB) is a target only for classes being served (256 KiB, 11 MiB) |
| Free and resident in the back end | no bound | see above |
| Pages pinned by one full magazine of 16-byte objects | 1 MiB, or 508 MiB of huge pages | 254 objects, one per page |
| Above watermarks | 2 MiB per class with 8 MiB carved | never purged |
| Large block, sparsely touched | 2 MiB per page touched | 512 times the small-page cost at worst |
| Freed large slots before any purge | 64 MiB | Vec growth |
| Growth headroom | address space only without huge pages | with them, the touched part rounded up to 2 MiB |
| Partly filled magazines | 2 KiB of magazine for any number of objects | with the drain bug one run made 1900 magazines, 3.7 MiB, to hold 0.56 MiB of objects |

The depot's target counts objects, not magazines, so drains that leave many nearly empty
magazines under the target cost more in magazines than in objects.

## 4. How the incumbents bound the same things

Read from the sources by a sub-agent (jemalloc 5.3 as `tikv-jemalloc-sys` bundles it,
tcmalloc and mimalloc 3.5 from `third_party/`); the constants were not re-checked. The
measured effects are in sections 1 and 2.

**jemalloc.**
- A thread-cache bin holds twice a slab's objects, clamped to 20..200: 3.2 KiB for 16-byte
  objects, 10 KiB for 512, 80 KiB for 4 KiB (`tcache.c:19-31`). It starts refills at half
  that.
- Collection is paced by bytes, not time: one event per 64 KiB allocated and per 64 KiB
  freed, each visiting one bin. A bin whose low-water mark since the last visit is above
  zero flushes three quarters of that mark and halves its next refill; a bin that ran dry
  doubles it (`tcache.c:125-208`). Only the owning thread's activity drives it.
- Freed pages decay over 10 s on a 200-step curve, read from the monotonic clock, advanced
  by a ticker on slow paths (about one tick per thousand slow-path events). Purging is
  `MADV_DONTNEED` (`arena_types.h:7-10`, `decay.c`, `pac.c`). No background thread by default
  in `tikv-jemallocator`, so an idle process never purges.
- It does not ask for huge pages. Freed extents are joined and split, and `realloc` grows in
  place.

**tcmalloc, per-CPU.**
- One byte budget per CPU, 1.5 MiB, shared by all classes. A class's capacity starts at zero
  and grows on misses, taking bytes from the CPU's pool (`cpu_cache.h:1240-1371`).
- Everything that shrinks or moves capacity runs on a background thread the program must
  start: drain a CPU whose cache did not change and had no misses (every 30 s), move 5% of
  the budget to the CPUs that miss most (5 s), resize classes (2 s), resize the slab (29 s)
  (`background.cc`). Without that thread per-CPU caches never shrink.

**mimalloc.**
- A page that becomes empty is kept for 16 visits of the slow path's collector if its bin
  has few pages, then freed (`page.c:595-693`).
- The slow path counts its own calls: a small collect every 1000, a full one, which runs
  the purge, every 10 000 (`page.c:1213-1241`).
- Freed arena memory is purged 4 s after it is freed, by clock, with `MADV_DONTNEED`, but
  only from that full collect (`arena.c:2266-2459`). It asks for huge pages and then purges
  in 2 MiB units, which is why it sits at 226 MiB for ten seconds above.

What matters for us: jemalloc bounds the cache per bin and collects by low-water mark, and
its decay is clocked. tcmalloc has the tightest per-CPU bound but needs a thread. None of
them reclaims an idle cache without help: jemalloc's needs the owning thread to allocate,
tcmalloc's the background thread.

## 5. What should change

Ranked by memory saved against complexity and cost. Each fits the concurrency rules: no
lock, no thread-local state, no thread, one atomic instruction or one rseq commit per
change, nothing on the fast path. All need a clock that costs no system call; the cycle
counter the idle-check fix reads serves.

After-peak run, anonymous resident MiB at 14 s, as prototypes are added:

| Build | with huge pages | without |
|---|---:|---:|
| asmalloc (with the stand-in idle check) | 149 | |
| + clocked purge (1) | 149 | |
| + depot collection and per-class idle reclaim (2) | 69 | 16.7 |
| + refresh (2) | 61 | 8.6 |
| + magazine age limit in place of idle reclaim (2) | 15.6 | 4.5 |
| jemalloc | | 4.5 |

**1. Purge on a clock, and never reset the count.** Once an interval (about a second), a
pass starts at the top of the heap and purges up to a quarter of the retained count; each
heartbeat does one bounded step of it. The count is free bytes believed resident and is
never set to zero, so it bounds a pass even in a fragmented heap, whose cost becomes one walk
of its bitmaps a second in steps, not one per heartbeat. Purging from the top down while
fills take the lowest addresses is the aging: what is reused is at the bottom. Two words of
state. Prototype: `x-purge-decay`. On its own it saved nothing in the test, because of (2);
it is the precondition for everything else. Cost: a clock read per heartbeat. Refaults
after a purge were not measured.

**2. Stop parked objects pinning the heap.** Three rules, all on slow paths:

- *A magazine does not stay loaded for more than an interval.* A slow path looks at one
  class of its own CPU's slab in turn. If the same magazine has been loaded for an interval,
  it takes it with the store-if-equal routine, as it would to replace it, and launders it and
  that class's spares. No fence, since it is the thread's own slab. A class in use pays one
  refill a second; a class that fell out of use is emptied. One word per class in the slab's
  cold part. Prototype: `x-class-age` (and `x-class-idle`, which takes only magazines whose
  slot has not moved and is weaker: lightly used magazines keep their scattered objects).
- *A depot stack nothing has touched for an interval is laundered,* one magazine per
  heartbeat, whichever class the slow path served. The stack head's version counter already
  says whether it was touched. One word per class. Prototype: `x-depot-gc`. The finished
  form should use a low-water mark as jemalloc does, so that a stack in use sheds its
  unused depth too.
- *One retiring full magazine in eight is laundered instead of kept,* so the objects in
  circulation are replaced by the lowest free ones within a few dozen exchanges. Prototype:
  `x-refresh`. Cost: one launder and one fill more per eight exchanges on the free side,
  about four atomic instructions each. Not measured for speed.

**3. Huge pages only where objects are dense, and none above the watermark.** Measured:
with huge pages only for classes up to 1 KiB (`x-thp-small`) the sparse test cannot occur;
with none (`x-thp-off`) churn peaks at 107.9 MiB (jemalloc 110.2), the tree at 13.0 (17.3),
and the after-peak run ends at 4.5 instead of 15.6. Argued, not built: keep huge pages for
classes up to about 1 KiB; raise the threshold from 8 MiB carved to something like 64, so
the page at the watermark is under 3% of the class; let a purge take the partly carved unit
at the top. The design's fault-cost argument for huge pages stands for dense small classes;
their cost in speed if removed was not measured here.

**4. A small cap for freed large slots.** With the retained cap at 8 MiB (`x-cap-8m`) Vec
growth peaks at 67.5 MiB (jemalloc 72.0) and 64 Vecs at 263 (305). The finished form is a
separate cap for large classes, checked at the free, leaving small classes to (1). Cost: a
program that frees and reallocates one large size in a loop faults it in each time once it
is over the cap; one run each showed 10.2 against 10.9 ns per pushed element, inside the
noise of this session.

**5. Smaller magazines for large small-classes.** With a 32 KiB magazine budget and a
64 KiB depot target (`x-mag-32k`) the mixed pipeline peaks at 5.5 MiB instead of 7.4, and a
CPU's worst case falls from 2.9 MiB to 1.0. Cost: four times the exchanges for classes of
256 bytes and up; not measured. With the mask read from a table, capacity could instead
start small and grow with a class's miss rate, as tcmalloc's does. That is argued only, and
needs care: a mask must match every magazine of the class that is loaded anywhere.

**6. A tick for threads that never miss.** Argued only. Two candidates: let a routine's
abort handler enter the slow path once (an abort is rare, costs nothing on the fast path,
and comes at about the scheduler's rate for a thread that is allocating); and an exported
`collect` a program can call from an idle hook. Without one of them, (1) and (2) do nothing
for a program whose allocations stay inside its magazines.

All of 1 to 5 together are the `lean` build: its peaks are the "all prototypes" column of
section 1. What is left over jemalloc at the peak is the huge page at each watermark
(churn 1M, JSON) and, in the smallest processes, a few hundred kilobytes of parked objects,
magazine area and page tables.

### The worst cases, measured

`bench/probes/memory/worst.sh`, on main after the idle-check fix. Anonymous resident MiB;
"all" is the `lean` build, "no THP" is mechanisms 1 and 2 with huge pages off.

| Case | when | asmalloc | all | no THP | jemalloc | mimalloc |
|---|---|---:|---:|---:|---:|---:|
| Burst then light work: free 218 MiB, then 2000 objects a millisecond | 4 s | 147 | 72 | 68 | 122 | 226 |
| | 12 s | 147 | 15.1 | 9.6 | 4.5 | 14.8 |
| Phase change: 8 CPUs use every class, then one CPU uses one | at once | 30 to 49 | 34 | 31 | 27.8 | 16.3 |
| | 8 s | 30 to 49 | 5.9 | 6.2 | 24.7 | 16.3 |
| Many CPUs, few classes: 8 CPUs, 4 classes | 8 s | 4.6 | 1.0 | 1.1 | 4.4 | 8.3 |
| Many classes, few CPUs: 2 CPUs, 44 classes | 8 s | 10.9 | 1.2 | 2.2 | 4.1 | 6.0 |
| Many threads, few CPUs: 16 threads on 2 CPUs, 44 classes | 8 s | 11.9 | 1.6 | 3.0 | 52.2 | 28.4 |
| Sparse: 256 blocks of 1 MiB, one page written each | | 246 | 1.3 | 1.3 | 2.9 | 276 |
| Sparse: 4096 blocks of 64 KiB, one page written each | | 247 | 16.4 | 16.4 | 33.2 | 344 |
| Three small objects | | 0.2 | 0.2 | 0.2 | 0.3 | 2.2 |

The phase-change peak varies from run to run with how the eight threads' bursts overlap.
In the burst case the last 10 MiB with huge pages on is the page at each class's watermark.
The quarter-a-second decay of (1) is slower than jemalloc's curve at first and faster than
mimalloc's delay; the fraction and interval are free choices.

### `malloc_trim`

It returns only the back end's wholly free pages today, so it inherits the pinning above: it
cannot purge a page that a parked object lies on. With the rules of (2) built, trim is the
same steps without waiting for the clock:

1. drain every slab a slow path has run on (the existing drain: claim, markers, one fence per
   CPU), which sends loaded magazines and spares to the depot;
2. launder every depot stack to the bitmaps, magazine by magazine;
3. purge every wholly free unit, as now, including the partly carved unit at each watermark
   once (3) allows it.

Step 1 costs an interrupt per CPU in use and step 2 an atomic instruction per 64 objects, so
trim stays a call a program makes rarely. The Rust equivalent is the same function, and it
is also the `collect` of (6) if given a cheaper mode that skips step 1 and only looks at
the caller's own CPU.

## The parked-object rules, built

2026-10-17, after the review above, measuring the finished forms of mechanism 2 (the magazine
age limit with its sweep of spare stacks, the depot's low-water rule, and the refresh) and
`collect`, on top of main with the back end's clocked purge. Same machine and method as above;
"main" is main without the rules, "rules" the branch, both built with
`bench/probes/memory/build.sh`. Each figure is one run.

Anonymous resident MiB, `bench/probes/memory/worst.sh`:

| Case | when | main | rules |
|---|---|---:|---:|
| Burst then light work: free 218 MiB, then 2000 objects a millisecond | 4 s | 73.3 | 38.9 |
| | 12 s | 73.3 | 4.3 |
| Phase change: 8 CPUs use every class, then one CPU uses one | 8 s | 29.0 | 5.6 |
| Many classes, few CPUs: 2 CPUs, 44 classes | 8 s | 10.6 | 1.3 |

jemalloc's figures in the table of section 5 are 4.5, 24.7 and 4.1.

**The refresh fraction.** Instructions, from the hardware counters of the measured phase,
median of eight interleaved runs; the times of these runs were too noisy to use:

| Refresh | churn, 100k live, mixed sizes | pipeline, 2 threads, small |
|---|---:|---:|
| none | +0.7% | +1.9% |
| one in 32 | +5.4% | +6.3% |
| one in 8 | +17% | +21% |

After the burst, at 12 s: none 7.1 MiB, one in 32 3.3, one in 8 3.9. Most of the cost is the
fill that replaces a laundered magazine, whose search of the bitmaps in a fragmented heap
(`tree::claim`) dominates the profile. The rules use one in 32.

**Speed of the finished rules against main**, interleaved, pinned (pipeline CPUs 0 and 1,
churn CPU 2), sixteen runs each: churn with 1k live 24.84 and 24.95 cycles an operation (no
slow paths, so no difference); churn with 100k live, mixed sizes, 87.9 and 89.8 (+2.3%,
instructions +5.4%); the pipeline was too noisy for medians (7.6 and 7.6 ns an allocation),
its fastest runs 3.81 and 4.10 (+8%), instructions +5.3%. Reading the cycle counter on every
heartbeat had looked costly in an earlier, busier session; a direct measurement put one read
at 7 ns, too little to matter, and the heartbeat now reads it only when a decision needs it.

**One launder.** A magazine of 254 objects scattered over a region, laundered by the back end
of a simulated heap (the same code): 0.9 us on average, 5.5 us at worst over 2000 tries. A
slow path launders at most four magazines beyond its exchange (heartbeat.rs, "Bounded work"),
so the rules add at most about 4 us to one slow path on average and about 20 us in the worst
case seen, plus a sweep step's pushes of a spare stack to the depot.

## Faults against memory: huge pages for grown blocks, the threshold, populate-ahead

2026-10-17, on main with the parked-object rules (8e3c0c4) and the branch that adds huge
pages for large blocks that grow (`back/region.rs`, `advise_grown`), populate-ahead for dense
classes, and a build-time override for both and for the huge-page threshold
(`ASMALLOC_<NAME>` in `back/policy.rs`). Since then populate-ahead and the overrides have
been removed, and a grown block counts as dense only if it at most doubles at once (it was
four times when these runs were made; Vec growth doubles, so its figures are the same).
The kernel primitives behind it are in
`zen5-roofline.md`, section 7. Workloads are the quick suite's single-threaded jobs, each
build interleaved in a new order each round, the process confined and pinned to CPU 13
(`taskset`; its CCD was idle apart from short bursts), in a cgroup of its own under
`scripts/memlimit`. Time is the job's metric with user and kernel cycles; peak is the
cgroup's `memory.peak`.

**Before and after**, ten rounds, medians. "main" is 8e3c0c4; "proposed" is huge pages for
grown blocks, the small-class threshold at 16 MiB, populate-ahead off. Kernel is cycles per
unit spent in the kernel; free 2 MiB blocks were available throughout (see the roofline note
for what happens when they are not).

| Job | | main | proposed | mimalloc | jemalloc |
|---|---|---:|---:|---:|---:|
| Vec growth to 64 MiB | cycles/element | 10.95 | **4.06** | 5.82 | 16.16 |
| | kernel | 7.11 | 1.64 | 1.27 | 10.73 |
| | faults | 41,609 | 1,245 | 73 | 63,291 |
| | peak MiB | 67.7 | 65.6 | 138.9 | 72.2 |
| JSON 8 MiB | cycles/byte | 29.84 | **25.79** | 16.91 | 25.64 |
| | kernel | 4.45 | 2.39 | 0.83 | 3.65 |
| | faults | 23,727 | 10,134 | 46 | 23,226 |
| | peak MiB | 89.2 | 90.3 | 96.7 | 89.0 |
| BTreeMap 200k | cycles/entry | 924 | 912 | 675 | 755 |
| | kernel | 45.8 | 44.9 | 10.4 | 43.2 |
| | peak MiB | 26.2 | 26.1 | 26.8 | 25.9 |
| churn, 1M live | cycles/op | 197 | 199 | 302 | 213 |
| | peak MiB | 108.0 | 109.7 | 116.9 | 110.4 |
| Burst then light work | peak MiB | 213.3 | 218.3 | 227.2 | 218.5 |
| | after 12 s | 4.3 to 6.1 | 4.6 | 16.2 | 4.5 |
| 64 Vecs to 4 MiB (3 rounds) | cycles/element | 46.4 | 41.0 | 39.6 | 14.4 |
| | peak MiB | 269.7 | 270.1 | 265.3 | 305.4 |

- Vec growth is now faster than mimalloc at half its memory. What is left in the kernel is
  zeroing, which every allocator pays.
- On JSON and BTreeMap the gap to mimalloc is mostly not faults: asmalloc's user cycles are
  23.4 and 867 against mimalloc's 16.1 and 665. In a profile of the BTreeMap job `release_all`
  (launder) is 9.5% of asmalloc's user instructions and the depot and tree another 4%;
  whole-process user instructions are 6.1e8 against mimalloc's 4.2e8 and jemalloc's 4.7e8. On
  JSON, launder (`release_all`, its sort and `validate`) is about 9% of all cycles. The
  BTreeMap's classes each stay under 8 MiB, so no threshold reaches them.
- 64 Vecs: all three of asmalloc, mimalloc and jemalloc fault little relative to their time;
  asmalloc and mimalloc spend 35 to 39 user cycles an element where jemalloc spends 7.
  Inference, not checked: 64 buffers in power-of-two slots, written round-robin at the same
  offset, alias in the caches.

**The threshold and populate-ahead**, eight rounds, medians. The 64 MiB column is main;
the others have huge pages for grown blocks too, which no block in these jobs qualifies for
(main and the 64 MiB build with them took the same faults, 23,727 and 23,729 on JSON). Kernel cycles per unit (the times of these runs were too noisy to use; the
kernel share was stable), cgroup peak, and for churn the resident set once everything is
freed. "+pop" is populate-ahead with stretches of an eighth of what the class has carved, up
to 64 pages. The burst row is the after-peak test above, one run each.

| Threshold | 64 MiB | 64 +pop | 32 | 32 +pop | 16 | 16 +pop | 8 | 8 +pop |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| JSON kernel cycles/byte | 4.82 | 4.87 | 3.86 | 3.28 | 2.96 | 2.67 | 2.34 | 2.39 |
| JSON faults | 23,700 | 4,600 | 14,200 | 3,000 | 10,100 | 3,000 | 8,100 | 3,000 |
| JSON peak MiB | 89.2 | 89.8 | 90.3 | 90.8 | 90.4 | 90.8 | 90.5 | 90.8 |
| BTreeMap kernel cycles/entry | 49 | 51 | 55 | 53 | 55 | 57 | 53 | 48 |
| BTreeMap peak MiB | 26.1 | 26.9 | 25.9 | 26.9 | 25.9 | 27.0 | 26.1 | 26.9 |
| churn 1M kernel cycles/op | 11.6 | 13.0 | 11.7 | 11.1 | 11.4 | 10.7 | 10.5 | 9.5 |
| churn 1M peak MiB | 108.0 | 109.4 | 108.0 | 109.5 | 109.6 | 110.9 | 113.2 | 113.9 |
| churn 1M after freeing all | 104.5 | 105.8 | 104.3 | 105.9 | 106.6 | 107.8 | 112.5 | 113.5 |
| burst peak MiB | 213.5 | 215.5 | 214.5 | 216.5 | 218.3 | 220.0 | 222.1 | 223.1 |
| burst after 12 s | 4.3 | 6.3 | 4.2 | 6.0 | 4.6 | 6.1 | 5.3 | 6.2 |

jemalloc: JSON 89.0, BTreeMap 25.9, churn 110.4 (after freeing 112.9), burst 218.5 and 4.5.

- With the parked-object rules a lower threshold no longer costs anything after the peak:
  every build is at 4 to 6 MiB twelve seconds after the burst. What it costs is the huge page
  above each watermark and the coarser purging at the peak: at 16 MiB, 1 MiB on JSON, 1.6 on
  churn and 5 on the burst, all at or under jemalloc; at 8 MiB churn goes 3 MiB over it.
- Populate-ahead cut faults three to five times and kernel time by at most a tenth, and
  cost about 1 MiB at every peak and 1 to 2 MiB after the burst (the stretch above each
  watermark, which is never purged). Off in the proposed policy.
- A program that grows a buffer and then uses it sparsely pays for the size it grew to,
  rounded up to a huge page: 64 buffers written 1 MiB densely, grown to 4 MiB by `realloc`
  and then written one byte every 64 KiB were 160 MiB resident with huge pages for grown
  blocks and 77 without (a scratch probe; the THP allocations partly failed in a first run,
  which then showed 78). That run grew by four times at once; under the rule as built, which
  allows at most a doubling, such a buffer gets no huge pages.
  The sparse tests of section 5 are unchanged (1.3 and 16.4 MiB), since a block allocated
  at its size never gets them.
- The soak's `bigmem` through the preload library, three interleaved runs, seconds and peak
  MiB, with huge pages for grown blocks ("on") and without them at the old 64 MiB threshold
  ("off"): `chain 10 1` 0.64 against 1.2 s (glibc 0.66), peak 264 both (glibc 257);
  `chain 10 4` 2.5 against 4.9 s (glibc 2.5), peak 914 to 1038 against 857 to 984 (glibc
  1000 to 1008); `grow 10 4` 1.15 to 1.97 against 3.5 to 3.7 s (glibc 2.5 to 2.7), peak 886
  to 952 against 835 to 849 (glibc 997 to 1014). Four threads each at 256 MiB is 1024 MiB;
  how close a run comes depends on how far the threads' growth overlaps, which the faster
  build makes more likely. The first "on" run of each of the first two modes took 5.4 and
  7.0 s: huge-page faults compacting memory (roofline note, section 7).

### Moves that keep sparse blocks sparse, and the density test

2026-10-17, later. A move now skips source pages that are all zero, and a grown block gets
huge pages only when the last 2 MiB the program wrote hold data in at least half their
pages, and then only up to the size asked for. Through the preload library, pinned, capped,
one run each unless a range is given; "main" is main before these changes.

`crates/asmalloc-preload/tests/programs/sparse_realloc.c` (malloc N, one byte a MiB, realloc
to 4N), resident MiB after the grow and cgroup peak:

| N | main | now | glibc |
|---|---:|---:|---:|
| 16 MiB | 17, peak 19 | 2, peak 3 | 1, peak 1 |
| 64 MiB | 65, peak 67 | 2, peak 2 | 2, peak 1 |
| 256 MiB | 257, peak 259 | 2, peak 3 | 2, peak 2 |

The soak's `realloc secs=60 threads=4 maxsize=268435456 budget=1024`, peak of its scope,
two runs each: main 1.0 GiB; with the sparse copy alone 896 to 1013 MiB; with the density
test as well 430 to 454 MiB; with no huge pages for grown blocks 267 to 283 MiB; glibc 310 to
345 MiB. Its blocks double but are written a stripe a MiB past 64 MiB, and huge pages on
the whole slot made them resident in full; what is left over glibc is the one growth step
the rule lets the advice run ahead, per thread. Vec growth keeps its gain (4.3 cycles an
element against 11 without huge pages for grown blocks), and `bigmem` is unchanged.

## Measured and argued

Measured: every figure in a table; the cap's failure; pinning; the depot and the busy CPU
keeping idle classes; the three huge-page effects; the absence of maintenance without slow
paths; the effect of each prototype on memory.

Argued: the speed cost of every proposal (the machine was too busy for times to mean
anything); the raised huge-page threshold and purging the unit at the watermark; adaptive
capacity; the abort-handler tick; the 256-CPU figures, which are arithmetic.

Not done: the RPC server; repeats and confidence intervals; workloads longer than a few
seconds apart from the worst-case tests, so the clocked mechanisms were not exercised by the
benchmark workloads themselves; more than 8 CPUs.

The experiments contain two `unsafe` blocks (a cycle-counter read and a store-if-equal of
zero) with no ledger entry. They are prototypes and must not be merged as they are.

## Magazine budget

2026-10-17, at main `6111f39`. The magazine byte budget (`MAGAZINE_BUDGET`) swept at 128 (the
value then), 64, 32 and 16 KiB, with the depot's per-class target (`DEPOT_TARGET`) at twice
the budget, as it was. Chosen: **64 KiB, depot target 128 KiB.**

**Method.** Each setting was a separate build of the workloads (build-time overrides, not
merged), run as an allocator entry of its own (`build` in `bench/allocators.toml`) against
jemalloc and mimalloc: ten full-size jobs of the suite, five rounds, every allocator once per
round in a fresh random order, under the runner's cap and CPU confinement, 330 runs, none
invalid. The machine was shared: SMT siblings of the measurement CPUs busy at a median of
1.0 CPU (90th percentile 5.7), 4.5 CPUs' worth elsewhere, nothing foreign on the job CPUs.
Time differences are per-round ratios against the 128 KiB build with a 95% bootstrap
interval of the median; they resolve 2 to 3% on the pipelines and mpmc and only 10 to 40% on
fork-join, the RPC server, churn at 1M live and JSON. Slow-path entries were counted with an
instrumented build (a relaxed counter per class on the allocation and free slow paths), also
not merged.

Time, median of five, and the cost against 128 KiB:

| Job | jemalloc | mimalloc | 128 KiB | 64 KiB | 32 KiB | 16 KiB |
|---|---:|---:|---:|---:|---:|---:|
| pipeline, mixed sizes (ns/alloc) | 18.5 | 21.5 | 12.3 | +5.1% [+3.6, +6.8] | +11.7% [+10.1, +15.9] | +23.3% [+21.2, +27.8] |
| pipeline, small sizes (ns/alloc) | 8.0 | 16.4 | 3.9 | −0.3% [−15, +7] | +5.0% [−8, +9] | +18.4% [+12, +68] |
| mpmc 3 to 1 (ns/alloc) | 10.8 | 8.0 | 5.3 | +0.4% [−1.0, +3.0] | +6.1% [+3.3, +7.4] | +12.9% [+10.7, +19.0] |
| fork-join, 4 threads (ns/alloc) | 5.2 | 6.3 | 3.8 | +8.5% [−5, +17] | +3.1% [−4, +23] | +10.6% [−4, +20] |
| RPC log ingest (server cycles/req) | 143k | 137k | 138k | +0.8% [−14, +35] | +9.4% [−3, +33] | +1.1% [−13, +17] |
| churn, 1k live (cycles/op) | 36.3 | 46.2 | 26.7 | +1.8% [−0.1, +2.6] | +3.9% [−5, +21] | +0.4% [−8, +7] |
| churn, 1M live (cycles/op) | 433 | 537 | 372 | +2.1% [−16, +27] | −0.7% [−25, +6] | +0.3% [−21, +14] |
| churn, 100k live, mixed (cycles/op) | 131 | 113 | 101 | +2.6% [−3, +18] | +10.3% [+2, +50] | +26.8% [+11, +53] |
| BTreeMap, 1M (cycles/entry) | 1814 | 1681 | 1777 | +1.8% [−5, +9] | −1.3% [−5, +9] | −0.5% [−8, +14] |
| JSON, 32 MiB (cycles/byte) | 41.5 | 30.8 | 32.5 | +4.1% [−3, +47] | +5.0% [+3.3, +6.5] | +34% [+14, +62] |

Cgroup peak, MiB, median of five (churn at 1M, BTreeMap and JSON moved by under 0.5 MiB at
every setting: 109, 124 and 352 against jemalloc's 110, 125 and 358):

| Job | jemalloc | mimalloc | 128 KiB | 64 KiB | 32 KiB | 16 KiB |
|---|---:|---:|---:|---:|---:|---:|
| pipeline, mixed | 5.24 | 8.75 | 7.07 | 5.12 | 4.00 | 3.54 |
| pipeline, small | 1.82 | 4.75 | 2.44 | 2.52 | 2.24 | 2.14 |
| mpmc 3 to 1 | 3.25 | 7.25 | 3.69 | 3.68 | 3.25 | 2.99 |
| fork-join | 30.8 | 21.3 | 15.9 | 15.7 | 15.5 | 15.2 |
| RPC log ingest | 8.9 | 24.6 | 9.4 | 8.3 | 7.6 | 8.1 |
| churn, 100k mixed | 42.7 | 54.6 | 35.2 | 34.6 | 34.2 | 34.1 |

Slow-path entries (allocation and free) per thousand allocations:

| Job | 128 KiB | 64 KiB | 32 KiB | 16 KiB |
|---|---:|---:|---:|---:|
| pipeline, mixed | 16.6 | 28.2 | 44.3 | 65.3 |
| churn, 100k mixed | 7.6 | 13.1 | 20.5 | 30.0 |
| pipeline, small; mpmc; churn 1M | 4.1 to 8.7 | the same | +25 to 26% | about double |

At 128 KiB every class up to 512 bytes already has the largest magazine (254 objects), so a
smaller budget reaches only larger classes: 64 KiB halves the magazines of 512 bytes and up
and leaves classes up to 256 bytes untouched, which is why the small-size jobs exchange
exactly as often; 32 KiB starts on the 256-byte class. The mixed pipeline's cost at 64 KiB,
5.1% for 11.7 more entries per thousand allocations, is about 55 ns per extra slow-path
entry.

The phase-change workload, two seconds after the change, holds 21 to 23 MiB at 64 KiB
against 27 at 128 KiB (jemalloc 26.5, mimalloc 16.3); its peaks are too noisy between rounds
to rank. Many classes on few CPUs peaks at 10 to 16 MiB at every budget (jemalloc 7.7), so
that excess is not parked magazines.

So 64 KiB is the one setting at or under jemalloc's memory on the mixed pipeline, for a cost
that is measurable on that job (still 30% faster than jemalloc there) and not resolvable on
any other.

## The background thread

2026-10-18, on the branch that adds `maintain` and the opt-in thread that calls it twice a
second (`Asmalloc::start_background` behind the `background` feature; `ASMALLOC_BACKGROUND`
for the preload library). Same machine; it was shared, with CPUs 0 to 3 busy throughout and
a load average of 15 to 20, and the runs were pinned to idle cores (8, 9 and 10).

**Which run has no slow path.** The after-peak run with its light work in bursts over twelve
sizes (`after=burst after_sizes=small`, section 2) no longer stays off the slow paths: the
age limit takes each class's magazine once an interval, and each take's refill is a slow path
whose heartbeat takes the next, so the heartbeats never stop. Without the thread it is at 4.5
MiB at 12 s, jemalloc 4.6 (one run each). The run with no slow path is the same light work one
object at a time (`after=pairs after_sizes=small`): two million allocations a second that
never leave the loaded magazines.

Anonymous resident MiB, `quiet live=2M sizes=small secs=16 after=pairs after_sizes=small`,
median of three interleaved runs. With the thread on, the runs differ by where the first
drain falls against the thread's half-second sleep, about half a second: at 2 s they read 68,
95 and 95 MiB (Rust) and 91, 70 and 70 (preload); from 14 s on they agree within 1 MiB. With
it off, and for jemalloc, they agree within 0.1 MiB.

| | freed | 2 s | 4 s | 6 s | 8 s | 12 s | 16 s |
|---|---:|---:|---:|---:|---:|---:|---:|
| asmalloc, Rust, thread off | 162 | 162 | 162 | 162 | 162 | 162 | 162 |
| asmalloc, Rust, thread on | 166 | 95 | 53 | 30 | 17 | 5.9 | 1.1 |
| jemalloc, Rust | 187 | 187 | 187 | 187 | 187 | 187 | 187 |
| asmalloc, preload, thread off | 177 | 177 | 177 | 177 | 177 | 177 | 177 |
| asmalloc, preload, thread on | 177 | 70 | 39 | 23 | 13 | 3.9 | 1.0 |
| jemalloc, preload | 188 | 188 | 188 | 188 | 188 | 188 | 188 |

The peak is 216 to 219 MiB for all. jemalloc's decay is driven by its own allocation events,
and one-at-a-time reuse of its thread cache raises none, so it keeps everything too. With the
thread, the first call that finds the slab quiet for an interval drains it; the clocked purge
then takes the retained memory down to the 64 MiB cap in one pass and by a quarter an
interval after that, which is the curve above.

A drain from `maintain` launders what it takes. Parking it in the depot instead, as the
heartbeat's idle check does, leaves the simulated version of this run at 73% of its quiet
memory after fourteen intervals (`maintain_brings_a_quiet_heap_down_to_what_collect_leaves`
fails that way): the slab, busy on its fast paths, pops its own magazines back from the depot
at once, and the scattered objects in them pin their pages.

**Cost to busy programs.** Ten interleaved runs each, order alternating, pipeline on CPUs 8
and 9, churn on CPU 10, long enough for seven calls of `maintain` during the measured phase:

| | thread off | thread on |
|---|---:|---:|
| pipeline, 2 threads, mixed sizes, 300M objects: ns per allocation, median (range) | 12.17 (12.10 to 12.22) | 12.10 (12.00 to 12.20) |
| the same, instructions | 77.377 G | 77.392 G |
| churn, 100k live, mixed sizes, 200M operations: cycles per operation | 88.78 (88.50 to 89.15) | 88.66 (88.47 to 89.09) |
| the same, instructions | 48.464 G | 48.471 G |

No difference beyond the noise; the instructions differ by 0.02%. In a busy process every
slab's counter moves between calls, so a call drains nothing and only records sightings.

**Cost when idle.** The thread's time on CPU from `/proc/<pid>/task/<tid>/schedstat`:

| Process | Window | On CPU | Calls | Per call |
|---|---:|---:|---:|---:|
| Rust, allocates almost nothing, sleeps | 60 s | 4.2 ms | 120 | 35 us |
| `sleep` under the preload library | 60 s | 4.5 ms | 122 | 37 us |
| Rust, after freeing 220 MiB, from 1 s | 60 s | 43 ms | 127 | 340 us |
| the same, from 20 s, everything returned | 40 s | 18 ms | 82 | 224 us |

With nothing to do a wakeup costs about 35 us, most of it the sleep and the wakeup
themselves. After the burst, a profile of the thread is 90% the purge walk: the retained
count stays above zero (free slots on units the walk cannot return whole), so each
interval's pass walks the bitmaps once round without meeting its goal, which is the clocked
purge's rule for a fragmented heap. That is 0.05% of a CPU for a heap of two million small
objects, and it grows with the bitmaps.

## The cap charged to growth

2026-10-18. The cap on retained memory used to act on frees: a launder that took the
small-class count past 64 MiB purged toward it, and a large free past 8 MiB of retained large
slots purged its own slot. A free cannot tell idle memory from memory about to be reused,
and the quiet-machine session (`baselines.md`, cause A) found programs that free and
reallocate the same classes faulting their memory straight back. Now one cap covers small
and large classes together and is charged when the heap carves: each 2 MiB of carving, while
retained memory is over the cap, purges up to as much as was carved (`back/purge.rs`). Frees
purge nothing except a large slot advised for huge pages. The clocked decay is unchanged,
including its term for everything over the cap.

A first version also charged claims that made purged pages resident again. That made the
allocator chase its own tail: a `HashMap` rebuilt five times made 24,500 `madvise` calls
(5,600 under the old rule), every one a growth purge of pages the same build was about to
reuse, which then refaulted and purged more. Charging only carving: 43.

**Method.** "Before" is main at 867aadf; "after" is the branch merged with it (af7fa45 plus
the carve-only change); "cap 16" is the same with the cap at 16 MiB. Times, kernel share and
faults from `benchrun run --suite full --only <job> --rounds 5`, the three builds interleaved
in a fresh order each round, on the runner's CPUs, with the machine otherwise quiet but for
two other implementers' builds; medians. Kernel share is kernel cycles over all cycles of the
measured phases. `madvise` calls from `strace -c -f` of a separate run of the same command on
CPUs 4-7 (the count, not the time). Memory from `bench/probes/memory/memrun.sh` on CPUs
4-7 and 12-15, two rounds, medians: cgroup peaks, and anonymous resident memory after the
peak.

| Job | | before | after | cap 16 |
|---|---|---:|---:|---:|
| pipeline, 2 threads, 4-256 KiB objects | ns/alloc | 318.9 | **119.2** | 98.9 |
| | kernel share | 54% | 0.2% | 0.2% |
| | faults | 241,000 | 676 | 659 |
| | `madvise` | 298,000 | 53 | 52 |
| JSON 32 MiB | cycles/byte | 20.89 | **18.32** | 18.35 |
| | kernel share | 14.7% | 3.2% | 3.2% |
| | faults | 93,500 | 28,300 | 28,300 |
| | `madvise` | 81,400 | 62 | 62 |
| mstress, preloaded | s | 0.396 | **0.270** | 0.288 |
| | faults | 684,000 | 35,900 | 40,600 |
| | `madvise` | 10,100 | 301 | 340 |
| | peak MiB | 128.7 | 139.9 | 140.1 |
| HashMap 1M | cycles/entry | 671.1 | **579.0** | 567.5 |
| | kernel share | 16.2% | 5.3% | 6.0% |
| | faults | 190,000 | 60,700 | 69,100 |
| | `madvise` | 5,590 | 43 | 50 |
| | peak MiB | 175.1 | **204.3** | 204.2 |
| BTreeMap 1M | cycles/entry | 962.6 | 935.9 | 950.1 |
| | faults | 38,400 | 24,900 | 24,900 |
| | `madvise` | 16,400 | 22 | 22 |
| churn, 1M live | cycles/op | 112.6 | 100.3 | 113.2 |
| | `madvise` | 741 | 21 | 20 |
| Vec growth to 256 MiB | cycles/element | 4.80 | 4.95 | 4.94 |
| | `madvise` | 378 | 378 | 377 |

The back-end smoke set (`--suite backend --rounds 3`): the large pipeline 432 to 128 ns/alloc,
JSON 8 MiB 20.5 to 18.7 cycles/byte, sh8bench 1.28 to 1.18 s; the tree and the small
pipeline unchanged.

- The large pipeline is still three times tcmalloc's 36 ns; none of what is left is the
  kernel. Churn's 11% is larger than anything this change does to it (it makes 741 purges
  in 20M operations before and 21 after) and is probably the session's noise; it is not
  slower. Vec growth's 3% is within its run-to-run spread; its `madvise` calls are the
  moves', unchanged.
- **HashMap peaks 29 MiB higher**, now 11% over jemalloc's 183.6. A growing map frees each
  table it outgrows: about 30 MiB of large slots, one per class, all under the cap. The 8 MiB
  large cap returned them at each free; now they stay resident, and the next build reuses
  them. The 16 MiB cap does not change it, since carving is done in the first build and later
  builds refault what decay returned without charge. mstress keeps its large blocks the same
  way, 11 MiB more, level with jemalloc's 139.6.

Cgroup peaks, MiB, before / after / cap 16: Vec growth 258.1 / 258.1 / 258.0, JSON 352.7 /
352.5 / 352.3, phase change 41 / 36 / 37, many classes on 2 CPUs 14.5 / 16.1 / 13.5, 16
threads on 2 CPUs 14.9 / 16.2 / 14.6, sparse 1 MiB blocks 3.6 / 3.8 / 4.0, sparse 64 KiB
blocks 33.4 / 33.8 / 34.0. The phase-change and few-CPU peaks move by 5 MiB between rounds
of one build, so none of these differences is resolved.

After the peak (free 218 MiB, then light work), anonymous resident MiB:

| | freed | 1 s | 2 s | 3 s | 4 s | 6 s | 8 s | 10 s | 12 s |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| before | 162 | 94 | 70 | 64 | 53 | 30 | 17.6 | 8.9 | 4.9 |
| after | 217 | 93 | 74 | 70 | 53 | 30 | 17.6 | 8.9 | 4.9 |
| cap 16 | 217 | 93 | 79 | 22 | 17.4 | 9.0 | 5.0 | 1.2 | 1.2 |
| after, no over-cap term in decay | 217 | 151 | 115 | 107 | 86 | 50 | 29 | 16 | 9.6 |

Before, the free itself purged the burst down to the cap, so the process held 55 MiB less
until the first decay pass; within a second that pass's over-the-cap term does the same, and
the curves agree from 4 s. Without that term decay takes a quarter an interval from 217
MiB and is twice as high at 12 s. At 16 MiB the over-cap pass takes far more at once (at 3 s,
22 against 70).

Ends of the worst cases, eight seconds after the event, before / after / cap 16: phase change
7.5 / 6.8 / 6.0, many classes on 2 CPUs 1.2 / 1.2 / 1.2, 16 threads on 2 CPUs 1.6 / 1.5 / 1.4.

## Build phases against mimalloc: huge pages and the threshold

2026-10-18, at main `016877c`. The question: on single-threaded build-and-drop jobs asmalloc
runs about mimalloc's instruction count in more cycles. Which memory effect is it?

- **In the build phases it is huge pages and nothing else.** The extra cycles are 4 KiB page
  faults and L1 dTLB misses on objects that sit on 4 KiB pages: the first 16 to 18 MiB of
  each class, and all of every class that never carves 16 MiB. Cache fills from L2, L3 and
  DRAM in the build phases are the same as mimalloc's or fewer. With transparent huge pages
  disabled for every process, asmalloc's build phases are level with mimalloc's or faster on
  all six jobs.
- **Classes far apart cost nothing beyond their TLB entries.** With huge pages from a class's
  first carve, every build phase is faster than mimalloc's (BTreeMap 3,687 against 3,799M
  cycles). What separate regions cost is that each class needs TLB entries of its own,
  which 4 KiB pages make expensive. Nothing points at the order or spacing of fills.
- **Two of the four gaps are mostly in the drop, not the build.** Box tree at depth 18 and
  JSON 8 MiB build or parse level with mimalloc; their deficit is the drop, which runs 1.5
  times mimalloc's instructions because every object is laundered. BTreeMap's and HashMap's
  drops are behind the same way (the scattered-launder section of `launder-and-fill.md`).
- **Recommended: switch a class to huge pages after 4 MiB carved instead of 16, at exactly
  that point.** Today the huge-page part starts at the first 2 MiB boundary above the
  commit, so a threshold of N leaves up to N + 2 MiB on small pages. At 4 MiB: BTreeMap 1M
  942 to 901 cycles/entry (mimalloc 859), tree 18 39.5 to 35.4 (33.8), JSON 8 MiB 16.97 to
  16.05 (16.09), churn 1M 112.8 to 110.0. Memory: at most 6 MiB more at any peak measured
  (churn 1M, 115.6 against 109.4; jemalloc 110.2), 1 to 2 MiB on the other heaps of 10 MiB
  or more, unchanged on small heaps and the worst cases, and lower after a burst. 2 MiB buys
  another 1 to 3% for up to 6 MiB more than 4 MiB costs (BTreeMap 200k 32.7 against 27.0).
  Decided otherwise: the threshold stays at 16 MiB, where asmalloc is under jemalloc's memory
  on churn and the small maps; 4 MiB would give that up and still leave BTreeMap behind
  mimalloc, whose remaining lead there is the drop. The exact switch was built ("The switch
  at the threshold, built", below).
- **`MADV_COLLAPSE` is not worth adding.** Collapsing a class's carved small pages when it
  switches at 16 MiB buys most of the 4 MiB threshold's gain on BTreeMap (943 to 911) but
  little on JSON 8 MiB and nothing on tree 18, whose classes never reach 16 MiB; at 2 or
  4 MiB it adds nothing over the exact switch. It costs 0.1 to 0.4 ms of synchronous kernel
  work per 2 MiB in a slow path, and no memory, which is its one merit.
- **HashMap's build gap is its large tables**, which no class threshold reaches: mimalloc
  with THP off builds as slowly as asmalloc. Advising every large block of 2 MiB or more for
  huge pages at allocation (an upper bound, not a proposal: it breaks the sparse-block rule)
  makes the build level with mimalloc's (2,027 against 2,049M cycles) and the job 474
  against 459 cycles/entry, the rest being the drop.

### Method

Each job's own phase counters (`bench/workloads`, which already reports build or parse,
walk and drop), over every repetition of the job, from a workload run by hand: CPU 2,
sibling idle, under `scripts/memlimit`, the configurations of a job interleaved in a fresh
random order each round under the shared measurement lock, five rounds, medians. Other work
ran on the second L3 throughout. Extra counters came from new `BENCH_EVENTS` support in the
harness (user mode only, in place of user cycles; `bench/README.md`), one set per run: L1
dTLB misses (`0xff45`) and page walks for 4 KiB and 2 MiB pages (`0x1045`, `0x4045`); L1
fills from L2, from L3 and from DRAM (`0x0144`, `0x0244`, `0x4844`). Zen 5 has no event for
page-walk cycles. Kernel cycles are cycles less user cycles. "THP off" is
`bench/probes/memory/thp-off.py` (`PR_SET_THP_DISABLE` before `exec`), which disables huge
pages even where a process advises them; mimalloc's huge-page memory went from 18 MiB to
none under it, and jemalloc, which asks for none, did not move. Scratch builds, not merged,
changed only `policy::HUGE_AFTER_HUGE_PAGES`, where the huge-page part starts, a
`MADV_COLLAPSE` call at the switch, or advice for large blocks. No huge-page fault fell back
to small pages during the sessions (`thp_fault_fallback` unchanged). Memory: cgroup peaks
from `peaks.sh`, the worst cases and the after-peak curve from `memrun.sh`, two rounds,
which agreed within 0.4 MiB; the eight-thread cases ran on CPUs 0 to 7.

### Where the cycles are

Millions over the job's phase, all repetitions (BTreeMap and HashMap 1M entries five times;
tree 18 four times; JSON 8 MiB three times). "First carve" is huge pages from a class's
first carve.

| | asmalloc | mimalloc | asmalloc, THP off | mimalloc, THP off | first carve |
|---|---:|---:|---:|---:|---:|
| **BTreeMap build**, cycles | 4,036 | 3,799 | 4,110 | 4,153 | 3,687 |
| instructions | 5,185 | 5,172 | 5,211 | 5,296 | 5,087 |
| kernel cycles | 76 | 25 | 90 | 88 | 26 |
| page faults (thousands) | 24.6 | 0.1 | 31.0 | 32.6 | 0.1 |
| L1 dTLB misses | 34.6 | 0.0 | 36.8 | 33.4 | 0.3 |
| page walks | 2.9 | 0.0 | 5.3 | 5.3 | 0.0 |
| L1 fills from L2 / L3 / DRAM | 81 / 83 / 11.3 | 81 / 80 / 12.6 | 82 / 83 / 11.4 | 83 / 83 / 13.1 | 81 / 81 / 10.5 |
| BTreeMap drop, cycles / instructions | 685 / 993 | 504 / 281 | 707 / 993 | 554 / 281 | 671 / 990 |
| **HashMap build**, cycles | 2,452 | 2,061 | 2,471 | 2,424 | 2,401 |
| kernel cycles | 161 | 47 | 161 | 190 | 123 |
| page faults (thousands) | 60.4 | 0.2 | 60.4 | 74.6 | 42.5 |
| L1 dTLB misses / page walks | 10.9 / 2.6 | 0.0 / 0.0 | 10.9 / 2.6 | 10.2 / 2.5 | 10.3 / 2.3 |
| HashMap drop, cycles / instructions | 373 / 820 | 247 / 201 | 374 / 820 | 235 / 201 | 375 / 820 |
| **Tree 18 build**, cycles | 33.5 | 34.4 | 34.5 | 42.4 | 28.1 |
| kernel cycles / faults (thousands) | 5.9 / 3.1 | 1.6 / 0.0 | 6.8 / 3.1 | 7.6 / 4.1 | 2.2 / 0.0 |
| walk | 11.4 | 11.7 | 12.3 | 14.7 | 10.3 |
| drop, cycles / instructions | 34.5 / 121 | 24.9 / 83 | 34.6 / 121 | 28.0 / 83 | 33.1 / 120 |
| **JSON 8 MiB parse**, cycles | 306 | 306 | 350 | 367 | 284 |
| kernel cycles / faults (thousands) | 27 / 8.4 | 11 / 0.0 | 50 / 20.4 | 53 / 22.0 | 12 / 0.5 |
| drop, cycles / instructions | 118 / 282 | 98 / 194 | 151 / 282 | 126 / 194 | 115 / 282 |

- **BTreeMap.** The build is 237M cycles behind at equal instructions: 51M in the kernel
  (24.6k faults against 0.1k), and the rest in user mode with 34.6M L1 dTLB misses and 2.9M
  page walks against none, about 9 cycles a miss. IBS samples of the misses (in a build
  with a 2 MiB threshold) put half of them in one class that spans 7 MiB, by its size the
  internal nodes, which never reach 16 MiB and so lie entirely on 4 KiB pages under main,
  and most of the rest in the small-page prefix of the leaf class. A lookup touches an internal node and a leaf on
  pages far apart; with 4 KiB pages the 96-entry L1 dTLB cannot hold the path. The drop is
  180M behind, at 3.5 times the instructions: one locked OR a scattered slot.
- **HashMap.** The build is 391M behind: 114M in the kernel (60k faults against 0.2k) and
  the rest with 10.9M dTLB misses and 2.6M walks, all on the large tables (thresholds move
  none of it). mimalloc with THP off builds in 2,424M, level with asmalloc. The drop is 126M
  behind, at four times the instructions: scattered launders of the values.
- **Tree 18.** The build is level (33.5 against 34.4) despite 3k faults; the walk is level.
  The gap, 3.9 cycles a node, is the drop: 34.5 against 24.9M cycles at 1.5 times the
  instructions, every node laundered in dense magazines (about 4.6 cycles a node). The whole
  tree is 12 MiB, under the threshold, which is why depth 20 (48 MiB) is level and depth 18
  is not: with huge pages its build is faster than mimalloc's and makes up the drop.
- **JSON 8 MiB.** The parse is level (306 both), with 16M more kernel cycles than mimalloc
  offset by fewer user cycles. The gap is the drop, 20M cycles at 88M more instructions.
- **THP off.** Every build phase of asmalloc is level with mimalloc's or faster (tree 20:
  48.3 against 68.1 cycles/node overall), and mimalloc keeps its lead only through its
  drops. jemalloc is unchanged by THP off, as it should be.

### The threshold

Exact switches (the huge-page part starts at the threshold) at 2, 4 and 8 MiB against main
(16 MiB, with the part starting at the next 2 MiB boundary above the commit). Time, five
interleaved rounds, medians; most rounds lay within 2% of their median, with single slow
rounds up to 6% above it.

| Job | main | exact 8 | exact 4 | exact 2 | first carve | mimalloc | jemalloc |
|---|---:|---:|---:|---:|---:|---:|---:|
| BTreeMap 1M, cycles/entry | 942 | 917 | 901 | 890 | 871 | 859 | 989 |
| HashMap 1M, cycles/entry | 563 | 561 | 563 | 563 | 555 | 462 | 634 |
| tree 18, cycles/node | 39.5 | 36.7 | 35.4 | 34.5 | 34.1 | 33.8 | 65.1 |
| tree 20, cycles/node | 44.0 | 41.4 | 40.7 | 40.4 | 39.8 | 43.7 | 100.5 |
| JSON 8 MiB, cycles/byte | 16.97 | 16.64 | 16.05 | 15.62 | 15.55 | 16.09 | 23.42 |
| JSON 32 MiB, cycles/byte | 16.25 | 16.31 | 15.92 | 15.88 | 16.11 | 16.54 | 23.43 |
| churn 1M live, cycles/op | 112.8 | 111.9 | 110.0 | 108.1 | | 187.0 | 127.1 |
| churn 1k live, cycles/op | 25.33 | 25.42 | 25.39 | 25.34 | | 45.79 | 36.57 |

"First carve" is from a separate session of the same design, which had no churn jobs. Tree
18 under main ranged from 38.0 to 39.9 across rounds, and main's median on it was 37.9 and
39.5 in two sessions; its differences under about 4% are not resolved.

Cgroup peak, MiB, two rounds (they agreed within 0.4 MiB), and anonymous resident memory 12
seconds after freeing 218 MiB (`burst-idle`):

| Job | main | exact 8 | exact 4 | exact 2 | first carve | jemalloc | mimalloc |
|---|---:|---:|---:|---:|---:|---:|---:|
| churn 1M live | 109.4 | 112.9 | 115.6 | 118.0 | 133.0 | 110.2 | 116.9 |
| churn 100k mixed | 33.9 | 33.7 | 33.8 | 33.7 | | 36.7 | |
| BTreeMap 200k | 25.7 | 27.0 | 27.0 | 32.7 | 61.5 | 25.9 | 26.6 |
| HashMap 200k | 32.4 | 32.5 | 32.5 | 38.1 | 68.6 | 34.7 | 36.6 |
| tree 18 | 13.1 | 15.0 | 15.0 | 15.0 | 45.0 | 17.1 | 19.0 |
| JSON 8 MiB | 90.0 | 90.0 | 91.7 | 91.8 | 123.0 | 89.0 | 96.6 |
| pipeline, mixed sizes | 5.7 | 5.6 | 5.6 | 5.5 | 52.0 | 5.7 | 9.2 |
| churn 1k live | 1.2 | 1.1 | 1.2 | 1.2 | 39.0 | 0.9 | 4.7 |
| burst, peak | 218.3 | 222.2 | 224.9 | 226.4 | 236.7 | 218.6 | 227.0 |
| burst, after 12 s | 4.9 | 4.0 | 3.6 | 3.4 | 17.1 | 4.5 | 16.2 |

The other worst cases (`worst.sh`: phase change, few classes on many CPUs, many classes on
few CPUs, 16 threads on 2 CPUs, sparse blocks, a tiny process) were within their run-to-run
spread of main at exact 2 MiB, the most aggressive of the three.

- **Huge pages from the first carve cost 2 MiB for every class touched**, since the first
  touch of a class faults a whole huge page: a three-object process holds 20.7 MiB, churn
  with 1k live objects 39 MiB. It is the fastest setting and not usable.
- **What a threshold costs is the huge page at each watermark**, up to 2 MiB per class past
  the threshold, and coarser purging there. Lower thresholds put more classes past it:
  churn 1M has a dozen classes of 4 to 16 MiB, BTreeMap 200k several of 2 to 4 MiB.
- **The after-peak curve improves** with a lower threshold: huge pages are purged whole, and
  more of the heap is in them.
- **4 MiB is the knee.** It keeps nearly all of the speed of 2 MiB, and its memory cost is
  confined to heaps with many classes of a few MiB each, at most about 6% of the heap.

### `MADV_COLLAPSE` when a class switches

Built as: when a class switches, advise the whole region for huge pages and collapse every
whole huge page of what it has carved, so the class is entirely on huge pages from then on.
Time, from the first two sessions (main's column is the first's):

| Job | main | collapse at 16 | exact 2 | collapse at 2 | collapse at 4 |
|---|---:|---:|---:|---:|---:|
| BTreeMap 1M, cycles/entry | 943 | 911 | 892 | 891 | 896 |
| tree 18, cycles/node | 37.9 | 39.6 (never switches) | 34.7 | 35.5 | 36.9 |
| JSON 8 MiB, cycles/byte | 16.83 | 16.64 | 15.69 | 16.15 | 16.43 |

It buys back dTLB misses (BTreeMap at 2 MiB: 9.5M against 15.5M with the exact switch) and
pays them in kernel time (60M against 40M cycles): 0.1 to 0.4 ms of synchronous work per
2 MiB, in a slow path. At 16 MiB it is free in memory and worth 3% on BTreeMap and 1% on
JSON; with a lower threshold it adds nothing. Not recommended.

### HashMap

hashbrown allocates each larger table fresh, at its size, so the tables never qualify as
grown blocks, and a large block allocated at its size gets no huge pages by design (a
sparsely used one would cost 2 MiB per page touched). As an upper bound only, every large
block of 2 MiB or more advised at allocation (with the exact 2 MiB switch): build 2,027
against mimalloc's 2,049M cycles, faults 8.8k against 47.7k, walks 0.1M against 2.4M,
567 to 474 cycles/entry; peak resident set lower, 188 against 219 MiB, since a freed slot
with huge pages is purged at its free. A rule that could earn this without the sparse risk needs
evidence of density at allocation, which a fresh table does not give; one candidate, not
built, is to learn it per class at the free (a `mincore` of a freed block of 2 MiB or more,
or the dense check the grown rule already uses), so that later tables of a class that were
dense get huge pages.

### The switch at the threshold, built

A region that will get huge pages now commits small pages only up to the threshold, and the
first carve that needs memory past it switches the region, so the huge-page part starts at
16 MiB exactly instead of at the next 2 MiB boundary above the last commit. "Before" is main
at `a763ad7`, "after" the change; CPU 2, interleaved, medians. The machine was busier than in
the sessions above (another job used about five CPUs of the same L3), so every figure is a
few percent higher than in them.

| Job | before | after | |
|---|---:|---:|---|
| BTreeMap 1M, cycles/entry | 973.4 | 967.6 | ten rounds, -0.4% |
| JSON 32 MiB, cycles/byte | 17.10 | 17.05 | ten rounds, 0.0% |
| churn 1M live, cycles/op | 126.2 | 125.1 | ten rounds, -0.5% |
| tree 18, cycles/node | 38.18 | 37.88 | five rounds, -1.0% |
| JSON 8 MiB, cycles/byte | 17.68 | 17.40 | five rounds, -1.6% |
| BTreeMap build faults (thousands) | 24.6 | 24.0 | |
| JSON 32 MiB parse faults (thousands) | 19.6 | 18.1 | |

Memory, two rounds each, cgroup peak MiB before / after: churn 1M 109.4 / 109.4, BTreeMap
200k 25.8 / 25.7, HashMap 200k 32.4 / 32.4, tree 18 13.2 / 13.1, JSON 8 MiB 90.0 / 90.1,
churn 1k 1.1 / 1.2, mixed churn 33.9 / 33.8, mixed pipeline 5.6 / 5.7 (jemalloc 110.3, 25.9,
34.8, 17.3, 88.9, 1.0, 36.8, 5.5). After freeing 218 MiB, anonymous resident MiB at 2, 4, 8
and 12 s: before 78, 52.9, 17.5, 4.9; after 79, 54.1, 16.8, 4.8. It costs nothing measurable
and saves a few faults; the jobs barely move because their classes that pass 16 MiB were
already mostly on huge pages.

### What is not explained

- Why identical builds of main read 37.9 and 39.5 cycles a node on tree 18 in two sessions
  an hour apart, with every other job steady. Occasional slow runs (up to 44.9) appear in
  every configuration of this short job.
- With the exact 2 MiB switch BTreeMap still takes 15.5M L1 dTLB misses, all on 4 KiB
  pages (the 2 MiB prefixes and classes under 2 MiB), and builds 100M cycles slower than
  with huge pages from the first carve. That is expected from the counts; it is noted
  because a cheaper way to cover the prefix than collapsing it was not found.

## Lines between CPUs, and the cost of a large block

2026-10-18, later. The large-object pipeline (two threads, 4-256 KiB objects) ran at
119 ns/alloc once the cap stopped purging, all of it user time, against tcmalloc's 36 and
mimalloc's 40. This section is what that time was made of and what was done about it.

**Method.** Cache lines one core took from the other are `ls_dmnd_fills_from_sys.local_ccx`,
counted with `perf stat` and sampled with `perf record` (IBS, and so `perf c2c`, is throttled
to a handful of samples on this kernel); a sample lands one instruction past its cause, so
each was read off the disassembly. Instructions per function are callgrind's (no rseq under
valgrind, so its heartbeat figures are not the real ones) and `perf stat`'s. Times are
`benchrun`, interleaved, five rounds, on the first L3 under the shared measurement lock, the
machine otherwise busy with builds on the second; medians.

**False sharing.** Six of every ten lines the producer's core took from the consumer's came
from words that only shared a line by accident: each class's layout memo sat on a line with
the next class's summary words, so every lookup of a layout missed after any large claim or
free; the depot packed eight classes' stack heads to a line beside the start-up status and
the CPU count; a region's watermark shared a line with summary words. `crate::lined` puts
each group of shared words on lines of its own and asserts the statics' sizes in lines. Fills
per allocation went from 6.4 to 5.6. The padding grows the depot from 800 bytes to 54 lines
(3.4 KiB) and each class's state in the back end from 600 bytes to 12 lines (19 KiB more over
112 classes), all of it zero, and untouched, until used.

**What else the time was.** Removing levers one at a time in throwaway builds, fills per
allocation and ns/alloc (three runs each): the heartbeat on every large operation, 5.5 and
104; the retained count's atomic add on every large free and claim, 4.45 and 84 to 99;
leaving a large class's summary bits set, no change; magazines of at least 8 slots for the
4-32 KiB classes, 3.8 and 64 to 88. With all of them the job is at 65 to 88 ns, and the
leaf-word compare-and-swap, which moves the line between cores on every allocation and free
of a large object, is the largest cost left.

**Not done: batching large objects across CPUs.** tcmalloc gets 36 ns because it caches
objects up to 256 KiB per CPU and moves them between CPUs in batches through its transfer
cache; our 32-256 KiB objects go to the bitmap one at a time on both sides. Making them
magazine classes, or adding a per-CPU cache of large slots with a shared batch stack, would
park hundreds of KiB per CPU per class, and the first moves the small/large boundary that the
C tables, the generated routines, `realloc` and the stagger depend on. An object of that size
costs microseconds to use, so 60-80 ns to allocate and free it is a percent or two of a real
program's time; the benchmark shows more because it barely touches its objects.

**Instructions for one large block.** A single-threaded loop that allocates, writes the first
word of, and frees a 64 KiB and a 1 MiB block (`largepair`) took 3,400 instructions per
allocation and free: the allocation looked its block up twice more to report its usable size;
the free looked it up, checked it again through the batch checker, and released one bit
through the batch machinery; both ran the depot's whole heartbeat and the back end's decay
step; every large claim cleared the summary bits to the top and the next free set them back;
and every metadata word was reached by checking the region and summing offsets afresh. Each
is gone (`back/class.rs`, `tree::release_one`, `tree::claim` with `keep`, `LONE_EVERY`,
`ClassHost::block`).

| per allocation and free | ns | instructions |
|---|---:|---:|
| `largepair`, before | 178.8 | 6,825 per 2 pairs (3,412) |
| `largepair`, after | 71.7 | 2,796 per 2 pairs (1,398) |
| glibc | 8.1 | 555 (278) |
| jemalloc | 133.9 | 6,830 (3,415) |
| mimalloc | 92.4 | 3,604 (1,802) |
| tcmalloc, per CPU | 61.2 | 2,025 (1,012) |

| large pipeline, 2 threads, ns/alloc | before | after | jemalloc | mimalloc | tcmalloc |
|---|---:|---:|---:|---:|---:|
| `objects=1M` (full suite) | 99.9 | 69.0 | 311 | 45.1 | 41.9 |
| the smoke set's | 100.0 | 69.5 | | | |

Elsewhere, before and after: sh8bench 0.773 and 0.724 s; the tree 40.2 and 38.0 cycles a
node; JSON 8 MiB 16.9 and 17.2 cycles a byte, the small pipeline 11.1 and 11.2 ns, mstress
0.286 and 0.286 s, HashMap 575 and 571 cycles an entry, Vec growth 5.47 and 5.46 cycles an
element: the same. In `backbench` fills from a fragmented heap are 8 to 13% faster and
nothing is slower. Peaks, the after-peak curve and the worst cases of `worst.sh` are within a
few tenths of a MiB of before.

**Magazine floor for the 4-32 KiB classes** (asked, not changed). Today the classes from
12 KiB to 32 KiB get the smallest magazine, 4 slots, 2 objects. A floor of 8 slots (6
objects) changes those seven classes only, and lets each CPU park four more objects in each:
48 KiB more for 12 KiB objects, up to 128 KiB for 32 KiB, 584 KiB more for a CPU loaded in all
seven. Measured against the floor of 4: the large pipeline 85 to 75 ns/alloc; the mixed
pipeline, the mixed exchange, mpmc and the mixed churn the same within 2%, and their peaks
the same within 0.2 MiB; `worst.sh` the same, its phase change 0.9 MiB higher eight seconds
after.

## Moving a dense block in whole pieces

2026-10-18, at main `a763ad7`. A move of a large block (`Back::move_large_with`) tested and
copied each 4 KiB page of a piece on its own, so that all-zero pages could be left out, and
into a destination not known to read as zero it first purged the whole destination, which the
copy then faulted back. Now each 2 MiB piece's pages are tested first (a page whose first word
is not zero costs one load); a piece with no all-zero page is one copy and leaves the
destination as it is. Only a piece with zero pages purges the destination, in the whole pages
under the span from its first zero page to its last, and leaves those pages out.

**What it changes.** Only moves into a destination that an earlier block left resident. A
dense 64 MiB block moved by `realloc` to 256 MiB, twenty times, each into the slot the
previous one freed (a scratch program, CPU 2, TSC ticks for the `realloc` alone, median of
five interleaved runs of each build):

| Move | before | after |
|---|---:|---:|
| Dense, into a slot left resident | 53.9 M ticks, 655,500 faults | 27.6 M, 344,200 faults |
| Dense, into a fresh slot | 42.9 M | 43.6 M |
| One byte a MiB, into a slot left resident | 22.1 M | 21.2 M |

The faults are the whole run's; the difference is the destination's 16,384 pages a move,
purged and faulted back before, written in place now.

**Vec growth does not move.** In both vecgrow jobs every move lands in a slot that reads as
zero, so the old code never purged a destination, and the copy is a few percent of the run.
`benchrun`, five interleaved rounds, CPU 2, medians; user and kernel cycles and faults are
the measured phase's, peak is the cgroup's:

| vecgrow 1x256Mi | cycles/element | user Mcycles | kernel Mcycles | faults | peak MiB |
|---|---:|---:|---:|---:|---:|
| asmalloc before | 5.664 | 500 | 261 | 3,007 | 257.8 |
| asmalloc after | 5.677 | 512 | 258 | 3,008 | 257.8 |
| glibc | 9.388 | 434 | 825 | 262,167 | 257.0 |
| jemalloc | 17.78 | 870 | 1,536 | 517,968 | 264.4 |
| mimalloc crate | 5.740 | 673 | 91 | 267 | 523.5 |

| vecgrow 64x4Mi | cycles/element | user Mcycles | kernel Mcycles | faults | peak MiB |
|---|---:|---:|---:|---:|---:|
| asmalloc before | 8.740 | 721 | 454 | 79,736 | 374.8 |
| asmalloc after | 8.797 | 725 | 454 | 79,736 | 374.8 |
| glibc | 19.37 | 1,648 | 944 | 272,626 | 269.2 |
| jemalloc | 11.49 | 753 | 782 | 235,113 | 307.2 |
| mimalloc crate | 44.28 | 5,880 | 64 | 153 | 289.0 |

After against before, paired by round: -0.4% (rounds from -7.5% to +4.7%) and +0.7% (-0.4%
to +1.0%). `madvise` calls, counted with `strace`, are the same in both builds: 376 and
1,257 (304 and 594 of them `MADV_DONTNEED`), one more `MADV_NOHUGEPAGE` after, from where
the reservation landed.

Section 10 of `baselines.md` found the zero-page rule cost 8.5% on the first job and 4.9% on
the second. That does not reproduce: a build with the rule switched off the same way
(every page copied, a page at a time, no test) measured the same as before and after,
within 2%, over six interleaved rounds. Its instruction counts there differed by 0.2%; the
difference was in cycles, and the rule-off build was the steadier of the two, which points
at layout or the machine rather than the test.

**The 64-Vec peak.** 374.8 MiB in every run of both builds, against 270.3 in `baselines.md`
(`b5d6e8f`) and 269 for glibc. That is the trail of outgrown blocks the next part of this
section removes.

**The gap to the mimalloc crate on one Vec to 256 MiB.** None on this day: 5.68 against 5.74
cycles an element (its three good rounds 5.64 to 5.74; two rounds at 7.4 and 8.9 paid
compaction). The two spend their time differently. mimalloc keeps what is freed, so after the
first repetition it takes no faults (267 in all; 91 M kernel cycles), at twice the peak. We
return the old block behind each move and a grown block's pages at its free, so every
repetition faults its memory again: 3,008 faults, most of them huge pages the kernel clears,
258 M kernel cycles. Our user time is a quarter lower than mimalloc's. What is left is that
policy, not the copy.

**The copy itself.** `copy_to_nonoverlapping` in a `no_std` crate becomes a call to
`memcpy`, which in a glibc process is glibc's (`__memmove_avx512_unaligned_erms` on this
machine). For 2 MiB it runs its vector loop with ordinary stores: glibc's thresholds for
`rep movsb` and for non-temporal stores are both 24 MiB here (`ld.so --list-diagnostics`).
A scratch program copied 256 MiB from a source pushed out of the caches, in 2 MiB pieces,
into huge pages, TSC ticks a MiB, median of five:

| Copy of a 2 MiB piece | into fresh pages (faulted by the copy) | into resident pages |
|---|---:|---:|
| glibc `memcpy`, one call | 283 k | 267 k |
| glibc `memcpy`, a call a 4 KiB page | 327 k | 280 k |
| 16-byte non-temporal stores (SSE2) | 283 k | 190 k |
| 32-byte non-temporal stores (AVX) | 278 k | 185 k |

One call for the piece is 15% faster than a call a page when the copy faults the
destination in, which is the common move. Non-temporal stores gain nothing there, because
the kernel has just cleared the page and its lines are in the cache, and are 30% faster into
resident pages, where ordinary stores must first read each line. Only a move into a slot
left resident would gain, and its data would arrive out of the cache; `rep movsb` would need
hand-written assembly. Not adopted.

**Sparse blocks stay sparse.** `sparse_realloc.c` through the preload library (malloc N, one
byte a MiB, realloc to 4N), resident MiB after the grow and cgroup peak, median of five
interleaved runs; mimalloc is the preload build of `third_party`'s:

| N | asmalloc before | asmalloc after | glibc | jemalloc | mimalloc |
|---|---:|---:|---:|---:|---:|
| 16 MiB | 2, peak 3.3 | 2, peak 3.5 | 1, peak 1.0 | 19, peak 17.3 | 36, peak 34.7 |
| 64 MiB | 2, peak 1.6 | 2, peak 1.8 | 2, peak 1.3 | 67, peak 65.3 | 132, peak 138.9 |
| 256 MiB | 3, peak 3.0 | 2, peak 3.3 | 2, peak 2.6 | 259, peak 258.7 | 260, peak 291.7 |

The sparse worst cases of `worst.sh` (allocation, no move) are unchanged: 256 blocks of
1 MiB at 2.3 MiB resident, peak 3.5, both builds; 4096 of 64 KiB at 32.2, peak 33.2 and
33.5.

**Cost of a hole.** Into a destination left resident, the old move made one purge of the
whole destination; the new one makes one for each piece that has zero pages, and faults back
the data pages between a piece's first and last zero page, and nothing for a piece without.
A block with one byte written a MiB has zero pages in every piece, so its move into a slot
left resident now makes 32 purges of the destination where it made one; it measured no
slower (the third row of the first table).

**Method.** The machine was shared: builds of other work ran on the second L3 throughout,
and free memory was under 1 GiB with 52 GiB in the page cache, so huge-page faults sometimes
compacted memory. Every timed run was on CPU 2 under the measurement lock, interleaved with
the runs it is compared to. "before" is `a763ad7`, "after" this change.

### A move returns the old block at any size

A block of at most one piece (2 MiB) was copied whole and then freed the ordinary way. Since
frees stopped returning memory because of a level (main `3426f7a` removed the 8 MiB cap on
freed large slots), such a free keeps the slot resident until the clock or the growth cap
reaches it. Sixty-four vectors growing to 4 MiB each leave a trail of 32 KiB to 2 MiB
blocks, about 100 MiB, which is the 374.8 MiB above. A free keeps a slot because the next
block of its class may want it; a move is evidence against that, since the block has just
been outgrown (or shrunk far below its slot), and that holds whatever its size. So every
move now takes the piecewise path: the old slot's pages go back after the copy and the slot
is released as purged memory, uncounted. A run's tail is no longer purged before `free_run`,
which purges the whole run.

`benchrun`, five interleaved rounds, medians; "before" is `a763ad7`, "after" both changes of
this section:

| Job | before | after | glibc | jemalloc | mimalloc crate |
|---|---:|---:|---:|---:|---:|
| vecgrow 64x4Mi, cycles/element | 7.77 | 8.96 | 18.56 | 10.71 | 43.69 |
| the same, peak MiB | 374.8 | 263.4 | 269.2 | 307.2 | 289.1 |
| the same, faults | 79,736 | 149,850 | 272,626 | 234,766 | 153 |
| vecgrow 1x256Mi, cycles/element | 5.38 | 5.30 | 8.97 | 16.56 | 5.35 |
| the same, peak MiB | 257.8 | 257.6 | 256.9 | 264.4 | 523.5 |
| largepair 64Ki+1Mi, ns | 71.8 | 72.6 | 8.3 | 137.6 | 110.4 |
| large pipeline (t2, 1M objects), ns/alloc | 69.6 | 69.0 | 409.6 | 311.2 | 42.1 |
| map hash 1M, peak MiB | 204.3 | 204.3 | 208.2 | 183.9 | 198.7 |
| mstress 8 (preload), s | 0.283 | 0.281 | 0.386 | 0.473 | |
| the same, peak MiB | 140.1 | 141.2 | 247.2 | 140.9 | |

The 64 Vecs give back 111 MiB of peak, under glibc's, and pay for it in faults: each
repetition faults its outgrown blocks again where it used to find them resident, 15% slower
than before (+15.1 to +16.3% by round), still 16% ahead of jemalloc and 6% ahead of what
`baselines.md` measured with the old cap (9.51). The rest is unchanged within noise.

The case that pays most: one block grown from nothing to 4 MiB by doubling and freed, a
thousand times (`vecgrow vecs=1 max=4Mi reps=1000`, `perf stat`, five interleaved runs,
cycles an element; 512 Ki elements a repetition):

| | before | after | glibc | jemalloc | mimalloc crate |
|---|---:|---:|---:|---:|---:|
| cycles/element | 4.87 | 5.08 | 1.72 | 2.91 | 2.45 |
| page faults | 514,358 | 578,230 | 2,148 | 2,000 | 122 |

The move adds 4%: 64 faults a repetition, the outgrown blocks below 2 MiB. Both builds are
about twice as slow as the incumbents here, and that is not the move: each repetition already
faulted 514 pages before, most likely because the grown block is advised for huge pages and
its free returns it (`purge_before_release`), so the next repetition faults it back (an
inference from the counts, not traced). A block resized back and forth across a class boundary does not move at all (the
headroom and shrink rules; `a_block_resized_back_and_forth_across_a_class_boundary_settles`
passes unchanged).

`sparse_realloc.c` now also returns the block it shrinks to an eighth, which moves: N=16 MiB
peaks at 1.3 MiB against 3.5 (64 and 256 MiB: 1.5 and 3.1 against 1.6 and 2.9; resident
after the grow 2 MiB in every case). The sparse worst cases are unchanged (2.3 and 32.2 MiB
resident).

## Reproducing

```
# The cap charged to growth: variants built as benchrun `build` entries in allocators.toml
# (bench/target/asmalloc-<name>), then for each targeted job:
bench/target/release/benchrun run --suite full --rounds 5 --only "json 32Mi" --allocators before,after,cap16
W=$(pwd)   # the checkout
bench/probes/memory/build.sh $W/bench asmalloc je mi base-ms=asmalloc-memstat
bench/probes/memory/peaks.sh $W asmalloc,je,mi pipe2 pipemix churn1M vecgrow json
bench/probes/memory/worst.sh $W asmalloc,je,mi
# The prototype builds (`lean`, `age-thpoff`) need the review branch and its x-* features.
bench/probes/memory/memrun.sh $W base-ms 0,1,2,3,8,9,10,11 phase touch threads=8 secs=8 sizes=s64
python3 bench/probes/memory/summ.py -c -s bench/target/memory-out/phase.base-ms.memstat
# The background thread: off, on, and jemalloc, natively and preloaded.
bench/probes/memory/build.sh $W/bench asmalloc bg=asmalloc-background je sys=
cargo build --release -p asmalloc-preload
bench/probes/memory/background.sh $W 8 3
# Build phases against mimalloc: one job's phase counters by hand, with extra events, then
# the same with huge pages disabled for the process. The threshold variants were scratch
# builds (HUGE_AFTER_HUGE_PAGES changed, the huge-page part starting at the threshold).
BENCH_CPUS=2 BENCH_EVENTS=dtlb_miss=0xff45,walk_4k=0x1045,walk_2m=0x4045 scripts/memlimit -m 4G -- \
  taskset -c 2 bench/target/asmalloc/release/workload map kind=btree n=1M reps=5
BENCH_CPUS=2 scripts/memlimit -m 4G -- taskset -c 2 \
  python3 -I bench/probes/memory/thp-off.py bench/target/asmalloc/release/workload tree depth=18 reps=4
```
