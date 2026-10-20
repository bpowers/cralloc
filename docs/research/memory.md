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

## The cost of a large block, by cause

2026-10-19, from main `a92d51a`. Where the cycles of one large allocation and free go, alone
(`largepair 64Ki+1Mi`) and across two threads (`pipeline t2 sizes=large objects=1M
inflight=512`, 4 to 256 KiB objects, half of them 32 KiB or more and so large), what was cut,
and what is left.

**Method.** Cycles, instructions and lines taken from another core
(`ls_dmnd_fills_from_sys.local_ccx`) from `perf stat`; where they go from `perf record` of
cycles and of that event (a sample lands one instruction past its cause, read off the
disassembly); instruction counts by source line from callgrind with `--dump-instr=yes` (no
rseq under valgrind, so its heartbeat counts are not the real ones). Variants built by hand,
interleaved in random order, three to five runs each on CPUs 0-1 or 2 under the measurement
lock, each run after half a second of spinning on CPU 3 (below). The before/after rows are
`benchrun`, five rounds, against `a92d51a` built as a second allocator.

**The pipeline's spread is the clock, not the allocator.** Over the ten rounds of the
`a92d51a` session the pipeline took 62 ns in seven and 79 to 81 ns in three, at the same
cycles per allocation (700 to 711) in all ten: the slow rounds ran at 4.4 GHz instead of 5.7.
Every allocator shows it, at random rounds (tcmalloc per CPU in three, tcmalloc per thread in
two), and the longer jobs nearly always run at the lower clock. What picks the clock is
whether the package was busy just before the run. Run after a pause of 0.2 s or more, the
job ran at 4.39 GHz for all of its 80 ms, twelve runs out of twelve; run back to back, or after
half a second of spinning on any core, of either L3 (CPU 2, or CPU 14), it ran at 5.7 GHz,
nine out of nine; 50 ms of spinning was not enough. So a short job's nanoseconds depend on
what ran on the machine in the second before it, while its cycles do not. Comparisons below
are in cycles; a session that ranks short jobs by time should warm the package before each
run, or rank them by cycles.

### Alone: `largepair`

Per allocation and free, cycles and instructions from `perf stat`, atomic read-modify-write
instructions counted in the code path:

| | `a92d51a` | now | floor |
|---|---:|---:|---:|
| cycles | 379 | 271 | about 140 |
| instructions | 1,630 | 1,290 | about 300 |
| atomic read-modify-writes | 8 | 4 | 4 |
| lines from other cores, system calls, page faults | 0 | 0 | 0 |
| ns (`benchrun`, five rounds) | 66.6 | 48.2 | |
| cgroup peak, MiB | 0.8 | 0.8 | |

tcmalloc takes 61.6 ns (per thread) and 66.3 (per CPU) in the same session, glibc 8.

The floor is what this design must do: a claim is a walk down the class's summary words to a
leaf word and one compare-and-swap there, a read of the slot's zero and clean bits and a write
of its size word, and the retained count lowered by what the slot held; a free finds the slot
from the address (a region-table load, a multiply, the stagger and watermark checks), counts
its pages that are not clean, sets its bit with one atomic OR, looks at the summary words
above, and raises the retained count. Four atomic instructions, each about 20 cycles and
serialized with the others, and a few hundred instructions.

The four that were above the floor:

- The size word was stored sequentially consistently, which is an `xchg`; only the slot's
  holder writes or reads it, so a release store does.
- The activity counter that a large allocation or free bumps for the idle check was a locked
  add, twice per pair; it is a hint, and a load and a store do.
- The free of a slot whose leaf word was empty (in this loop, every free) set the summary bit
  above it without looking, an atomic OR of a bit that a large class's claims leave set. The
  single-slot release now looks first at every level, the one above a summary word its own OR
  found empty included, on a tree whose claims keep their bits; `tla/SummaryLookUp.tla`
  models that step.

The instructions that went: each metadata word was reached through a block address cached in
the class's host, with saturating arithmetic and a compare at every access (100 instructions a
claim); the claim counted the slot's clean pages in one pass and cleared those under the block
in a second, and counted bits without a population-count instruction, which the baseline
instruction set lacks, though a slot's clean pages are one run nearly always (100 more); and
the claim's tail worked out the class size and the slot's address again.

What is left above the floor is about a thousand instructions of layering, none of it large
alone: per allocation about 500 in the claim (the class's layout unpacked from its memo, the
walk's spot arithmetic, the stagger, the usable size), 200 in the heap above it (`place`, the
start check, the grant), and per free about 450. A large path written as one function over a
region's metadata block would come near the floor; it was not done here, since the job is
ahead of every incumbent already and the pipeline below is not bound by instructions.

### Across threads: the pipeline

Cycles per allocation over both threads (the faster thread spins), and lines one core took
from the other:

| | `a92d51a` | now | tcmalloc per thread | tcmalloc per CPU |
|---|---:|---:|---:|---:|
| cycles per allocation | 702 | 656 | 402 | 459 |
| lines from the other core, per allocation | 4.9 | 4.0 | | |
| cgroup peak, MiB | 2.9 | 2.8 | 3.8 | 4.2 |

Of the 0.9 lines cut, 0.7 came from the retained count. It was one word, raised by every free
and lowered by every claim, so in the pipeline its line went to the other thread twice per
large object, and every heartbeat read it to decide whether a decay pass could start, which
took the line from whichever thread had just written it. It is now two tallies that only grow,
on lines of their own, one for what is counted in (frees, launders) and one for what is
counted out (claims, fills); and a heartbeat reads the clock to decide whether a pass is due,
reading the tallies only when one is. A due pass that finds nothing retained ends at once, so
the next interval's heartbeats wait again. Separating the two tallies by 128 bytes as well, in
case the L2's adjacent-line prefetcher fetched one with the other, changed nothing measurable.

Where the 656 cycles go. The two threads take turns being the slower one (in a profile with
call stacks, a fifth of the producer's samples were waits on a full ring and an eighth of the
consumer's on an empty one), so a cut on one side alone shows only in part. The producer's
time, which is the larger: half in large claims, a third in the small classes' magazine
exchanges, a seventh in the workload's own work. Within a large claim, two thirds of the
samples are the leaf word's compare-and-swap, the load of the summary word on the leaf's line
before it, and the counted-out tally's add after it: the waits for a line the consumer has just
written. The consumer's large free is likewise mostly its `lock bts` on the same line.

The floor for this design is set by those lines. A large slot freed on one thread and claimed
on another sets a bit in a leaf word that the claimer must then read and change, and the
lowest-address-first claim keeps every live slot of a class in one or two leaf words, so each
large object moves its leaf word's line twice, once each way, about 100 cycles a move within
an L3, plus the two threads' tally adds. The small half of the mix, 4 to 32 KiB, goes through
magazines, and from 12 KiB up a magazine has four slots and holds two objects, so every
second object is an exchange with the depot: a full magazine popped from the stack the other
thread pushed it on, an empty one pushed back, the stack heads and the magazine's link moving
between the cores each time. Neither is bookkeeping that can be cut by construction; both are
the price of handing objects across threads one at a time, or two at a time. tcmalloc gets
402 cycles by caching objects up to 256 KiB per thread or per CPU and moving them in batches.

Changes that were tried and did not help: taking the leaf line exclusive before reading it (a
compare-and-swap of zero for zero ahead of the claim's load) cost the single-threaded job 16
cycles and gained nothing across threads; giving a large class's leaf bitmap a pair of lines of
its own, apart from its summary words, changed nothing, since the summary word and the leaf
come over together.

**Options that park memory** (not built; measured as throwaway builds on top of the cuts
above, pipeline cycles per allocation, three runs):

| Magazines of the 12 to 32 KiB classes | cycles | what more a CPU may park |
|---|---:|---|
| 4 slots, 2 objects (now) | 656 | |
| 8 slots, 6 objects | 530 | 4 more objects in each of the seven classes: up to 584 KiB with all loaded |
| 16 slots, 14 objects | 466 | 12 more in each, and 8 more in the 8 and 10 KiB classes: up to 1.9 MiB with all loaded |

The 8-slot floor is the one measured in "Lines between CPUs" above (85 to 75 ns then), with
the same memory cost. Reaching tcmalloc's 402 would also need the large objects batched: a
per-CPU cache of 32 to 256 KiB slots, or large classes as magazine classes, which the design
has turned down for their memory.

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

### Grown blocks kept up to a budget

2026-10-18, later, at main `630883b`. The loop above, one buffer grown to 4 MiB and freed
over and over, ran at twice the incumbents' time because both of our rules returned its
memory every repetition: the move returned the slot it outgrew, and the free of the grown
block, which has huge-page advice, returned that too. Both are right for a block grown once
and wrong for one grown again and again, and the two cases differ in how much is at stake: a
few MiB in the loop, a hundred in the 64-vector job.

Now retained large-slot memory is counted apart from small again (the decay and the cap use
the sum), and a move or the free of an advised block purges only if keeping the slot would
take that count past a budget (`policy::KEPT_GROWN_HUGE_PAGES`). Otherwise the slot is
released as a plain free leaves it: resident, counted, advice in place. An allocation that
lands on an advised free slot already took the advice back; a grown move into it advises it
again (`region::advise_grown`), which is two `madvise` calls a repetition in the loop. A
plain free still never purges.

This is not a level on frees. A purge here is charged only to a block the program copied or
wrote in full to reach its size, so it costs at most about what the program already spent on
it, and never falls on a block that was allocated, barely touched and freed.

**The loops.** One `Vec<u64>` grown from empty to the size by pushing, then dropped, repeated
(`vecgrow vecs=1`); `perf stat` on CPU 2, five interleaved runs, medians; thousands of cycles
and page faults per repetition, cgroup peak MiB. Budgets of 8, 16 and 32 MiB:

| To | main | 8 MiB | 16 MiB | 32 MiB | glibc | jemalloc | mimalloc crate |
|---|---:|---:|---:|---:|---:|---:|---:|
| 1 MiB, kcycles | 430 | 291 | 290 | 290 | 270 | 404 | 463 |
| 1 MiB, faults | 64.1 | 0.2 | 0.2 | 0.2 | 0.2 | 0.1 | 0.0 |
| 1 MiB, peak | 10.9 | 10.9 | 10.9 | 10.9 | 10.1 | 10.5 | 21.2 |
| 4 MiB, kcycles | 2,733 | 1,459 | 1,443 | 1,258 | 1,079 | 1,822 | 1,868 |
| 4 MiB, faults | 578 | 3.3 | 3.3 | 0.8 | 2.1 | 2.0 | 0.1 |
| 4 MiB, peak | 13.8 | 16.1 | 16.0 | 15.9 | 13.1 | 16.4 | 27.1 |
| 64 MiB, kcycles | 30,164 | 26,938 | 27,082 | 29,176 | 68,691 | 121,066 | 44,030 |
| 64 MiB, faults | 622 | 73 | 73 | 53 | 16,388 | 30,775 | 4.6 |
| 64 MiB, peak | 73.8 | 76.1 | 76.1 | 92.1 | 73.4 | 80.7 | 147.3 |

At 8 and 16 MiB the 4 MiB loop still returns one slot a repetition: the slot the buffer last
moved into is 20 MiB (four times the room, plus the stagger's), though only 4 MiB of it was
written, and the count charges a slot its whole size, as it does for every free. That slot
costs the 3.3 huge-page faults and 13% kernel time left; at 32 MiB it is kept, and the loop is
within 17% of glibc. The 1 MiB loop is within 8% of glibc at every budget; the 64 MiB one was
already ahead of all three and stays so.

**The rest** (`benchrun`, five interleaved rounds, medians):

| Job | main | 8 MiB | 16 MiB | 32 MiB | glibc | jemalloc | mimalloc crate |
|---|---:|---:|---:|---:|---:|---:|---:|
| vecgrow 64x4Mi, cycles/element | 8.99 | 8.97 | 8.94 | 8.82 | 18.48 | 10.69 | 43.78 |
| the same, peak MiB | 263.3 | 269.8 | 276.4 | 287.6 | 269.2 | 307.1 | 289.1 |
| vecgrow 1x256Mi, cycles/element | 4.95 | 4.60 | 4.60 | 4.84 | 9.03 | 16.87 | 5.33 |
| the same, peak MiB | 257.7 | 259.9 | 259.9 | 276.0 | 257.0 | 264.4 | 523.5 |
| the same, faults | 3,200 | 1,472 | 1,472 | 1,448 | 262,166 | 517,967 | 267 |
| largepair, ns | 69.9 | 69.8 | 69.9 | 69.8 | 8.1 | 135.8 | 106.9 |
| large pipeline, ns/alloc | 67.1 | 67.3 | 67.7 | 67.4 | 334.6 | 302.5 | 40.6 |
| map hash 1M, cycles/entry | 574 | 565 | 568 | 570 | 1,108 | 633 | 441 |
| the same, peak MiB | 204.4 | 204.4 | 204.3 | 204.4 | 208.1 | 183.7 | 260.8 |
| mstress 8 (preload), s | 0.343 | 0.338 | 0.314 | 0.326 | 0.390 | 0.509 | |
| the same, peak MiB | 135.3 | 135.7 | 138.1 | 135.3 | 276.3 | 140.2 | |

The 64 vectors keep up to the budget of their outgrown slots, which is all of the rise in
their peak. One vector to 256 MiB faults half as often (time: paired by round, -1%, within noise). mstress varied
by a fifth between rounds and is noise here; nothing else moved. `sparse_realloc.c` and the
sparse worst cases are unchanged (16 MiB budget: resident 2 MiB after every grow; 2.3 and
32.2 MiB).

**Chosen: 16 MiB.** 32 MiB would cover the 4 MiB loop's last slot, but adds 11 MiB to the
64-vector peak and 16 to the 256 MiB one, for one loop shape.

**After the peak.** One `Vec<u64>` grown to 256 MiB and dropped, then a 64 KiB allocation
and free every millisecond (a scratch program, resident MiB from `/proc/self/statm`): main
2.7 at the drop and after; 16 MiB budget 4.9 at the drop, 2.9 by 500 ms, 2.7 after. The
256 MiB block's 1 GiB slot is far past the budget and goes back at its free; what is kept is
its outgrown slots below, and the clock returns them within the first interval. The same
with a 4 MiB vector: 4.7 at the drop, 2.7 by 500 ms.

### A large slot charged its pages that are not clean

2026-10-18, later. The retained count charged a freed large slot its whole size. A block that
moved in with headroom fills a fifth to a quarter of its slot, so it was counted four or five
times what it could have touched, which inflated what the decay, the growth cap and the budget
for kept grown blocks act on. The 4 MiB loop above lost its last slot every repetition for it:
20 MiB charged for 4 MiB written.

**The first rule, and why it went.** As first merged (`d6e8841`), the free marked the slot's
pages past what its tenant could have written clean, in slots of a huge page or more whose
tenant wrote less than half, and charged the rest. An adversarial review found that the free
did not know whether an earlier tenant had written those pages, and that this broke two
things: a tenant that asked for nothing (a zero-size `aligned_alloc` at a huge alignment) left
an extent of no pages, so its free marked a resident slot wholly clean and no purge, not even
`malloc_trim`, ever returned it; and a smaller tenant after a larger one hid the larger one's
pages from the count without bound in total, ten MiB resident behind a 1.7 MiB count in its
test, out of sight of the cap and the budget. It also found that moves under way at once each
kept a slot past the budget, since the budget was checked before the copy and counted after.

**The rule now.** A clean bit is true: it says the page has not been written since it was
carved or last purged. Only a purge, a shrink's purge that went back whole, and the carve of a
slot (for the pages past the block it was carved for, which nothing has written) set one; a
claim, and anything that widens a held block (an in-place grow, huge-page advice, a resize),
clear the bits under what the block may write, writing only words that change. A free slot is
charged its pages that are not clean (`region::charge`), and the free writes no clean bit and
reads no size word. The free, the claim (`region::settle_slot`), the keep decision and the
purge's count agree on that, so the count is exact: the review's random runs now also check
that no clean bit, of a free slot or a held one, stands over a page that holds data, and that
holds with purges and advice failing and with twelve threads at once.

So that the carve knows which pages a block with headroom will write, the heap claims the slot
for the size the block uses, not for the room (`Heap::alloc_large_using`), where it used to
claim it for the room and record the smaller size after. The keep decision adds the slot's
charge to the count first and takes it back if the result is past the budget, so moves under
way at once pass it by one slot at most. There is no longer any condition on slot size or tail:
a slot of any size is charged what is true of it, and in the steady state the free and the
claim read the slot's clean words and write nothing.

**Measured** against main `1868dd0`, interleaved, budget 16 MiB. The loops, `perf stat`, five
runs, medians, thousands of cycles and faults per repetition, peak MiB:

| Grow to | main | after | glibc | jemalloc | mimalloc crate |
|---|---:|---:|---:|---:|---:|
| 1 MiB | 291 / 0.1 / 10.7 | 291 / 0.1 / 10.7 | 269 / 0.2 / 10.2 | 404 / 0.1 / 10.4 | 465 / 0.0 / 21.0 |
| 4 MiB | 1,262 / 0.8 / 15.8 | 1,259 / 0.8 / 15.8 | 1,073 / 2.1 / 13.1 | 1,828 / 2.0 / 16.4 | 1,867 / 0.1 / 27.1 |
| 64 MiB | 28,655 / 73 / 75.9 | 28,374 / 73 / 75.9 | 68,782 / 16,388 / 73.4 | 122,957 / 30,775 / 80.7 | 43,934 / 4.6 / 147.2 |

The 4 MiB loop is where the first rule had it, 17% behind glibc. What is left there is user
time: the buffer is copied at each move into a slot four times its size, where glibc most
likely grows the block in place or remaps it (not traced). The two `madvise` calls a
repetition (the claim takes back a kept slot's advice, the grown move gives it again) are in
its 1.6% kernel time; avoiding them would need the heap to tell the claim that the block is a
growing move's destination, so they stay.

A claim of a kept slot takes its advice back but not the huge pages already faulted, so that
tenant's shrink in place is not trimmed to whole huge pages and may purge part of one; on
Linux 7.2.9 a partly unmapped huge page stays charged whole to the cgroup until memory pressure
splits it (`thp_partial.c`: purging half of each huge page of 64 MiB took RSS from 67 to 35 MiB
but the charge only to 56, unchanged 2 s later). It is bounded at under two huge pages a
shrink, and those pages are marked clean, so the retained count does not see them; the
behaviour is left as it is.

`benchrun`, five interleaved rounds, medians; the change is paired by round:

| Job | main | after | change | glibc | jemalloc | mimalloc crate |
|---|---:|---:|---:|---:|---:|---:|
| vecgrow 64x4Mi, cycles/element | 9.00 | 8.97 | -0.9% | 18.47 | 10.58 | 43.80 |
| the same, peak MiB | 276.3 | 279.3 | | 269.2 | 307.1 | 289.1 |
| vecgrow 1x256Mi, cycles/element | 4.72 | 4.78 | -1.3% | 9.13 | 17.33 | 5.44 |
| the same, peak MiB | 259.8 | 259.7 | | 257.0 | 264.4 | 523.4 |
| large pipeline, ns/alloc | 71.6 | 65.5 | -8.5% | 391.5 | 297.2 | 49.2 |
| map hash 1M, cycles/entry | 560 | 561 | -0.0% | 1,103 | 631 | 432 |
| the same, peak MiB | 204.3 | 204.3 | | 208.2 | 181.6 | 198.9 |
| mstress 8 (preload), s | 0.262 | 0.271 | +2.5% | 0.363 | 0.441 | |
| the same, peak MiB | 139.7 | 141.5 | | 242.9 | 139.4 | |
| largepair, ns | 78.7 | 74.4 | -4.2% | 8.1 | 135.9 | 108.1 |

`largepair` is 4.2% faster than main (2.4 to 5.4% by round), since its free no longer reads the
size word. The pipeline's paired change is -8.5% but its rounds range from -29.5% to -7.6%,
and main's own pipeline moved 4% with the layout change of `1868dd0`; take it as no slower.
mstress ranges from -3.1% to +5.6%, noise. The 64 vectors' peak is 3 MiB higher, within the
budget for kept grown blocks, whose slots are now charged what they hold.

**After the peak**, as above, one vector grown and dropped, then light work, resident MiB: a
256 MiB vector 4.8 at the drop, 2.9 by 500 ms, 2.7 from 1.5 s (main 5.0, 3.0, 2.9); a 4 MiB
vector 8.9 at the drop, 4.9 at 500 ms, 3.0 at 1.5 s, 2.8 from 2.5 s (main 8.8, 4.8, 3.0, 2.7).
The worst cases of `worst.sh`, two runs each, are within 0.3 MiB of main except the phase
change, whose peak was 52 to 55 MiB against 46 to 49 and which ended at 9.0 to 9.2 against 8.2
to 8.7; that case moved as much between runs of one build in earlier sessions (50 to 56 MiB,
8.8 to 9.7 at the end) and touches no large slot. `sparse_realloc.c` stays at 2 to 3 MiB
resident after every grow, peaks within 0.3 MiB of main.

### The grow loop's 40%: where the benchmark's loop lands

2026-10-18, later. The 4 MiB grow-and-free loop measured 1,765k cycles a repetition on main
`fa4a4e0` against 1,262k on `1868dd0` the session before. It is not the allocator. Built
today from the same bench sources, every commit since measures the same, interleaved, five
runs: `1868dd0` 1,770k, `40876e1` 1,765k, `5eb615b` 1,766k, `dc12be3` 1,767k, `fa4a4e0`
1,765k, all with 0.8 faults, 4 `madvise` calls (two of each advice) and a 15.8 to 15.9 MiB
peak a repetition. The binary built last session from `1868dd0` still runs at 1,263k.

The two `1868dd0` binaries retire the same instructions (9.08e9 and 9.09e9) with the same
branch, cache, TLB, op-cache and store-forwarding counts; 85% of the time is the benchmark's
own `Vec::push` loop in `workload::main`, a chain of one store and one load of the length
per element, and that loop is byte for byte the same in both. What differs is where it lands:
a change to `bench/workloads/src/single.rs` (the perf control for drop phases) moved
`workload::main`. Rebuilding the incumbents shows the same thing for them: jemalloc's binary
went from 1.83e9 to 1.32e9 user cycles, the mimalloc crate's from 1.35e9 to 1.86e9, with the
same instructions. Padding the environment by up to 2 KiB, which moves the stack, changes
nothing; building with `-C llvm-args=-align-loops=64`, which keeps the loop's stack slots
as they were in the slow build and only moves the code, gives 1.24e9, the fast figure. So it
is the loop's code address, most likely its store and load relative to a 32 or 64-byte
boundary in some predictor of Zen 5's (not identified).

So grow-loop numbers from different builds of the workloads cannot be compared, and the
17% behind glibc above was measured with asmalloc's loop in a good place. Building the bench
workspace with `-C llvm-args=-align-loops=64` puts the loop in the same place in every
build (asmalloc 1.24e9, jemalloc 1.32e9, the mimalloc crate 1.36e9, glibc 1.08e9 user cycles
for the 4 MiB loop); it is not adopted here, since it moves every benchmark's code and so its
baselines. Instead the job no longer pushes (`baselines.md`, section 14), and its numbers
above are not comparable with later ones.

### Headroom for growing blocks

2026-10-18, later, at main `32eb3b0` (variants) and `e486bc4` (the confirming run). A block that
outgrows its slot moves to one with `HEADROOM` times the room it asked for. At four, half the
cycles of a buffer grown to 4 MiB and freed were `memmove` in its moves. Since a slot is
charged, kept and purged by its pages that are not clean, room nobody writes costs no memory,
so a larger factor costs only address space and commit charge. Now it is sixteen
(`heap/large.rs`), with a fallback to four and then none, and the shrink threshold moved with
it. A room past a region is now cut to a region, where it used to be dropped, which left a
block over a quarter of a region at 4 (a sixteenth at 16) to move at every doubling.

**Bytes copied.** A doubling buffer lands in the class above `H` times its size plus the
stagger's 4 KiB. That class is `1.25 H` times the size, so for `H = 2^k` the buffer moves once
every `k + 1` doublings. Averaged over final sizes it copies about `1/(k+1)` of its final size:
a third at 4, a quarter at 8, a fifth at 16, a sixth at 32. Any one final size sees a
sawtooth: right after a move it has just copied half of itself, whatever the factor. Counted
with `vecgrow ... moves=1`, MiB copied by blocks above 32 KiB, and moves, per repetition:

| Final | 4 (main) | 8 | 16 | 32 |
|---|---:|---:|---:|---:|
| 1 MiB | 0.25 / 1 | 0.50 / 1 | 0 / 0 | 0 / 0 |
| 2 MiB | 0.25 / 1 | 0.50 / 1 | 1.00 / 1 | 0 / 0 |
| 4 MiB | 2.25 / 2 | 0.50 / 1 | 1.00 / 1 | 2.00 / 1 |
| 8 MiB | 2.25 / 2 | 0.50 / 1 | 1.00 / 1 | 2.00 / 1 |
| 16 MiB | 2.25 / 2 | 8.50 / 2 | 1.00 / 1 | 2.00 / 1 |
| 32 MiB | 18.25 / 3 | 8.50 / 2 | 1.00 / 1 | 2.00 / 1 |
| 64 MiB | 18.25 / 3 | 8.50 / 2 | 33.00 / 2 | 2.00 / 1 |
| 128 MiB | 18.25 / 3 | 8.50 / 2 | 33.00 / 2 | 2.00 / 1 |
| 256 MiB | 146.25 / 4 | 136.50 / 3 | 33.00 / 2 | 130.00 / 2 |
| mean copied / final | 0.325 | 0.274 | 0.208 | 0.166 |

**Time.** The grow-and-free loop at each final size, thousands of cycles a repetition (CPU 2,
the builds interleaved in random order, five rounds, medians; `reps` 4000/MiB, at least 60):

| Final | 4 | 8 | 16 | 32 | glibc | jemalloc |
|---|---:|---:|---:|---:|---:|---:|
| 1 MiB | 55.2 | 73.3 | 40.6 | 40.0 | 33.4 | 42.0 |
| 2 MiB | 111.7 | 132.3 | 165.5 | 93.5 | 80.6 | 167.9 |
| 4 MiB | 341.6 | 218.2 | 252.0 | 347.1 | 172.4 | 390.9 |
| 8 MiB | 525.0 | 380.9 | 416.7 | 514.9 | 352.5 | 4,968 |
| 16 MiB | 1,528 | 2,045 | 1,417 | 1,530 | 689 | 15,154 |
| 32 MiB | 5,850 | 6,530 | 3,308 | 3,768 | 22,464 | 39,485 |
| 64 MiB | 14,741 | 16,261 | 19,932 | 12,567 | 50,861 | 100,341 |
| geomean vs 4 | | +2.7% | -10.7% | -14.5% | -1.7% | +244% |

Time follows the bytes copied, size by size. Faults a repetition are no higher at 16 than at
4 at any size. VmHWM is within 1.4 MiB of 4, and 8 is 6 MiB higher from 16 MiB up. Seven
sizes from 1 MiB do not cover whole periods, so the geomean favours 32 a little. The averages
of copied bytes are the fairer summary.

The other jobs (`benchrun`, five interleaved rounds, medians, change paired by round against
4; `r16` is 16 limited to rooms of 256 MiB, above which it takes 4):

| Job | 4 | 8 | 16 | 32 | r16 | glibc | jemalloc |
|---|---:|---:|---:|---:|---:|---:|---:|
| vecgrow 1x256Mi, cycles/element | 3.33 | 3.32 (-0.0%) | 2.13 (-35.6%) | 3.19 (-4.3%) | 2.10 (-35.3%) | 6.81 | 14.83 |
| the same, faults / peak MiB | 1,454 / 259.8 | 1,491 / 266.0 | 897 / 258.5 | 1,367 / 259.5 | 889 / 258.3 | 262,166 / 257.1 | 517,967 / 264.4 |
| vecgrow 64x4Mi, cycles/element | 6.90 | 7.93 (+15.1%) | 6.47 (-6.2%) | 6.50 (-5.7%) | 6.41 (-7.3%) | 8.88 | 8.80 |
| the same, faults / peak MiB | 137,557 / 279.4 | 282,480 / 277.6 | 184,881 / 277.3 | 122,497 / 277.7 | 184,881 / 277.2 | 272,625 / 269.2 | 246,703 / 303.1 |
| large pipeline, ns/alloc | 62.1 | 62.3 | 62.4 | 62.1 | 62.7 | 336.9 | 303.1 |
| largepair, ns | 67.4 | 67.6 | 67.4 | 67.5 | 67.8 | 8.1 | 132.1 |

`main` measured as 4 throughout, within 1%. The pipeline and `largepair` never resize, and
they moved by under 1%. `r16` is 16 on every job here; it differs only for blocks past 16 MiB
that grow on past 256 MiB, which it copies more.

**Why 16 loses the 64 MiB loop.** It is the copy, not faults. Per repetition (40
repetitions, three interleaved runs; huge faults from `thp_fault_alloc`, calls by `strace -c`,
user cycles split by `perf record -e cycles:u`):

| | 4 | 16 |
|---|---:|---:|
| MiB copied, moves | 18.25, 3 | 33.0, 2 |
| user kcycles | 5,000 to 5,700 | 8,700 to 10,400 |
| of them `memmove` / `memset` | 28% / 66% | 57% / 36% |
| kernel kcycles | 7,600 to 9,600 | 8,100 to 9,600 |
| faults, of them huge | 68.7, 40 | 56.0, 48 |
| `madvise`, `mprotect` | 19, 1.9 | 26, 1.9 |

At 4 the last move is to 32 MiB, copying 16, and the 160 MiB slot then holds 64 MiB. At 16 the
last move is to 64 MiB, copying 32. That copy also costs twice as much a byte (158k against
77k cycles a MiB), since a 32 MiB source no longer fits the L3. The `memset` is the same at
both. A single buffer grows in slot 0 of each class, which has no stagger, so it loses no huge
page in place (below).

**The 64 vectors and huge pages.** At 8 and 16 the 64-vector job faults more (twice and 1.35
times as often) because more of its growth is in place, and in-place growth loses huge pages.
Each slot starts its block at a different stagger, so a block of exactly 2 MiB ends a few
bytes into the next huge page. Its advice is rounded down (`region::advise_grown`), so writing
those bytes faults a small page there. At the next growth that huge page is advised but
already has a page table, and it stays small: one huge page of each in-place doubling. That
spill page is one the block can write, so advising it at the growth, before the program
writes it, is the fix at the cause. Measured (five interleaved rounds, medians):

| `vecgrow 64x4Mi` | cycles/element | faults | peak MiB |
|---|---:|---:|---:|
| 16 | 5.86 | 184,880 | 277.3 |
| 16, the spill page advised | 4.46 (-24.5%) | 56,108 | 403.0 |
| 4 | 5.90 | 137,557 | 279.2 |
| 4, the spill page advised | 6.38 (+8.4%) | 137,557 | 403.9 |

The 1 and 4 MiB loops and 1x256Mi are unchanged by it, within 0.3%. It costs 126 MiB of peak:
a buffer that stops at a multiple of a huge page holds one more whole huge page for the few
bytes it wrote there. It was not adopted. Starting a grown block unstaggered would remove the
spill, but this is the case the stagger is for: 64 buffers of 4 MiB walked together took 40.9
cycles an element at slot starts and 12.0 staggered (`zen5-roofline.md`, section 7). Any
stagger smaller than a huge page makes a block of `k` huge pages span `k + 1`, so the choice is
between that page's memory and its faults.

**Address space and commit charge.** What large classes hold at exit (memstat builds, one run
each): regions assigned, and committed MiB, which is the commit charge under strict
overcommit and equals what was carved:

| Job | 4 | 8 | 16 | 32 |
|---|---:|---:|---:|---:|
| 1 MiB loop | 2 / 2.9 | 2 / 10.8 | 1 / 1.3 | 1 / 2.6 |
| 4 MiB loop | 3 / 23.0 | 2 / 10.8 | 2 / 41.4 | 2 / 162.6 |
| 64 MiB loop | 4 / 183 | 3 / 171 | 3 / 1,321 | 2 / 163 |
| vecgrow 64x4Mi | 3 / 1,460 | 2 / 680 | 2 / 2,640 | 4 / 10,400 |
| vecgrow 1x256Mi | 5 / 1,463 | 4 / 2,731 | 3 / 1,321 | 3 / 4,259 |

A growing buffer is charged the whole slot it ends in. At factor `H` that is `1.25 H / 2^j`
times its final size, for a final size `j` doublings past its last move: at 4 between 1.25
and 5 times (a mean of 2.9 over sizes), at 16 between 1.25 and 20 (mean 7.75), and at 32 up to
40. Watermarks only rise, so the charge outlives the buffer, though the next buffer of the
class reuses the slot. `r16` holds the charge of a big buffer to 256 MiB of room: 361 MiB for
the 64 and 256 MiB jobs where 16 has 1,321. A failed commit is not a failed allocation: the growth falls back to four
times the room, then to the size alone. Nothing was built for strict overcommit.

**Running out of address space.** Each large class takes regions from one shared pool of
3,940. A region holds `region / class` slots of the class. Growing buffers of final size `F`
fit until about `3940 * (slots of the class they end in)` are live, which in the design's
geometry is hundreds of GiB even at 32. A buffer ends in a slot up to `1.25 H` times its
size, so the live data that fills the reservation is about 15.4 TiB over that: at least 3 TiB at
4, 770 GiB at 16, 385 GiB at 32. Under an address-space limit, regions shrink and that bound
shrinks with them. `bench/probes/memory/grow_exhaust.c`, preloaded under `prlimit --as`:
buffers grown from 64 KiB by doubling and kept (first byte written, so they cost address
space only), until an allocation fails:

| Limit (region) | Final | 4 | 16 | 32 |
|---|---|---:|---:|---:|
| 16 GiB (2 MiB) | 256 KiB | 11,769 | 3,923 | 3,923 |
| 16 GiB | 1 MiB | 3,922 | 3,923 | 3,923 |
| 16 GiB | 4 MiB | 1,960 | 1,961 | 1,961 |
| 64 GiB (8 MiB) | 256 KiB | 47,220 | 11,805 | 3,935 |
| 64 GiB | 1 MiB | 3,934 | 11,805 | 3,935 |
| 64 GiB | 4 MiB | 3,934 | 3,934 | 3,935 |

In every case, at the failure, a plain `malloc` of the size whose growth failed also fails,
and a 64 KiB one succeeds. The fallback (16, then 4, then the size) never refuses what an exact
allocation could serve (`a_growing_block_takes_less_headroom_once_the_regions_run_out`).
Capacity is not the same, though: what was used is gone, since a region is never unassigned.
Small regions are where it shows. A slot of a whole region holds one buffer, so at 16 and 32
every buffer of 256 KiB or more takes a region, and 4 fits three or four times more 256 KiB
buffers. Which factor fits more depends on how the final slot divides a region (16 fits three
times more 1 MiB buffers at 64 GiB). Below about 8 GiB the heap does not start at all, so this
is a limit to document, not a case to build for. The design geometry needs hundreds of GiB of
live buffers to get there.

**The shrink threshold** is twice the factor, 32: a block that moved in fills more than
`1/(1.25 H)` of its slot. `a_block_that_moved_in_with_headroom_stays_when_it_shrinks_back`
checks, for factors 4, 8, 16 and 32 and every large class, that a block that outgrew its
class by a byte and moved stays when cut back. `a_block_resized_back_and_forth_across_its_slot_boundary_settles`
and the C test of the same name check that twenty resizes across a slot's edge move once.
The cost: a large block may now shrink in place to a thirty-second of its slot before it moves.
The pages past it still go back at once, so it holds address space, not memory.

**An untouched tail is never charged, kept or purged**, at 4, 8, 16 and 32 (the crate's
tests, run with the constant set to each): `the_room_a_grown_block_never_wrote_is_never_charged_kept_or_purged`
grows a block by a move into a slot of a whole simulated region and frees it with its advice.
The block is charged its block's huge pages, not the slot, and is kept on that charge. A trim
then takes back exactly what was counted. The back end's charge tests
(`a_large_slot_is_charged_its_pages_that_are_not_clean`, the review's random runs) pass
unchanged.

**Memory after the peak**, as in "Grown blocks kept up to a budget" (preloaded scratch
program, resident MiB at the free and at 0.5, 1.5 and 2.5 s): 256 MiB at 4 4.3, 2.6, 2.4,
2.3; at 16 3.0, 2.3, 2.3, 2.3. 4 MiB at 4 8.2, 4.5, 2.6, 2.2; at 16 7.0, 3.2, 2.3, 2.2.
`sparse_realloc.c` is the same at both (1 to 3 MiB resident at every step for 16, 64 and 256
MiB). Every case of `worst.sh` ends within 0.5 MiB and peaks within 1 MiB of 4 (none grows a
block).

**Chosen: 16, the shrink threshold 32.** Against 4 it copies about a third fewer bytes on
average (0.21 of the final size against 0.33). That is 28 to 34% of the time of the 1 and
4 MiB loops and of one vector to 256 MiB, and 6% of the 64 vectors. Like any constant, it loses at some sizes: 2 and 64 MiB copy half the final
size, and the 64 MiB loop is 35% slower, since it copies 33 MiB a repetition where 4
copies 18. 32 would copy a fifth fewer bytes again for twice the
address space and commit charge (10.4 GiB committed for 64 buffers of 4 MiB). The gains
diminish with the logarithm of the factor while the costs grow with the factor, so sixteen,
not more. The confirming run, final build against main `e486bc4` (five rounds, change paired by round):
1 MiB loop -28.2%, 4 MiB loop -29.8%, 1x256Mi -34.2%, 64x4Mi -5.8%, the large pipeline
+0.5%, `largepair` -0.5%, tree +0.5%, JSON -1.2%. Against glibc, the loops are now
1.25 and 1.48 times its time where they were 1.74 and 2.11.

### Headroom only while it is a small part of a region

2026-10-18, later, after review. As first merged, a room past a region was cut to a whole
region. In the full reservation that only matters for buffers past 256 MiB, but regions shrink
under an address-space limit, and a region is never unassigned. Every buffer whose sixteen
times reached half a region then took a region of a class with one slot to a region. When the
regions were gone, nothing of any new class could be had, `malloc(256)` included. The rule now:
sixteen times the size where that is at most a sixteenth of a region, otherwise four times
where that fits a region, as before, otherwise none. The class sixteen times lands in has a
dozen slots or more to a region, and past that the rule is the earlier one.

**Buffers that fit** (the review's `grow_starve.c`: `malloc(FROM)`, `realloc(TO)`, kept; under
`prlimit --as`, preloaded; the base is four times, as at `e486bc4`):

| Limit (region) | 40 to 64 KiB | 128 to 256 KiB | 512 KiB to 1 MiB | 2 to 4 MiB |
|---|---|---|---|---|
| 16 GiB (2 MiB): base, as merged, now | 23,634, 3,939, 23,634 | 3,939, 3,939, 3,939 | 3,939, 3,939, 3,939 | 1,969, 1,969, 1,969 |
| 64 GiB (8 MiB): base, as merged, now | 98,475, 23,634, 98,475 | 23,634, 3,939, 23,634 | 3,939, 3,939, 3,939 | 3,939, 3,939, 3,939 |

Under these limits sixteen times would need a buffer of a 256th of a region, 8 or 32 KiB,
which is not a large block, so the rule there is the base's, buffer for buffer. In the full
reservation it gives sixteen times up to 16 MiB. There it fits fewer buffers than the base:
slots up to 320 MiB, twelve or more to a region, against the base's 5 times the size. That is
still hundreds of GiB of live buffers.

**Time** (`benchrun`, five interleaved rounds, medians, against the rule as merged): the 1 and
4 MiB loops -0.8% and -0.3%, `vecgrow 1x256Mi` -0.1%, `vecgrow 64x4Mi` -1.0%; the 64 MiB loop
copies the same 33 MiB. Bytes copied up to 256 MiB are unchanged. Past that the room is four
times, so a buffer grown by doubling copies, in MiB, against the base and the rule as merged:

| Final | base | as merged | now |
|---|---:|---:|---:|
| 512 MiB | 146 | 33 | 289 |
| 1 GiB | 146 | 33 | 289 |
| 2 GiB | 1,170 | 1,057 | 289 |
| 4 GiB (arithmetic) | 3,218 | 3,105 | 2,337 |

**Commit charge.** Under a 2 GiB `RLIMIT_DATA` (preloaded `grow_exhaust.c`, buffers grown from
64 KiB by doubling and kept), base against now: 396 and 789 buffers of 1 MiB, 394 and 28 of
4 MiB, 49 and 26 of 16 MiB. A buffer is charged the whole slot it moved into, up to twenty
times its size up to 16 MiB and five times above.

**A refused commit no longer costs the last region.** `Back::assign` took a region for a class
and then committed the slot. When the OS refused the commit (strict overcommit, a data limit),
the region stayed with that class, empty, and the smaller rooms after it had none. A large
class now commits its first slot before it takes the region (`region::commit_first`), and
records the commit in the region's header. A refusal leaves the region for the next attempt.
Small classes are unchanged: their first carve is a magazine, and they take a region once.

### Sixteen times only in a full heap, and the threshold of each slot

2026-10-18, later, after a second review. Two costs remained.

- The thirty-second shrink threshold applied to every large block, also where no block gets
  sixteen times the room. So a buffer allocated big and cut to between a thirty-second and an
  eighth of its slot kept the slot. Under a 16 GiB limit, 3,940 buffers allocated at 1 MiB and
  cut to 100 KiB used every region, where the base fits 70,902.
- At a 128 GiB limit (16 MiB regions), 64 KiB buffers got sixteen times again: 47,268 fit
  against the base's 200,889.

The rule now. A heap that did not get its full reservation lacks address space, so it does not
trade any for speed: it gives four times the room where that fits a region, and keeps a shrink
threshold of an eighth, as the base did. A heap with its full reservation gives sixteen times
where the slot that takes is at most a sixteenth of a region. A slot's threshold is twice the
factor a block growing into a slot of its size gets: a thirty-second for those small slots of
a full heap, an eighth for every other slot and for runs. A heap knows which it got from its
plan: its region size is the geometry's largest.

**Buffers that fit**, base against now (preloaded, `prlimit --as`; growing: the review's
`grow_starve.c`, `malloc(FROM)` then `realloc(TO)`; shrinking: `shrink_starve.c`, allocate,
cut, keep, at most 200,000):

| Limit | 40→64 KiB | 128→256 KiB | 512 KiB→1 MiB | 2→4 MiB | 512 KiB cut to 40 KiB | 1 MiB cut to 100 KiB | 64 MiB cut to 3 MiB |
|---|---:|---:|---:|---:|---:|---:|---:|
| 16 GiB | 23,634 | 3,939 | 3,939 | 1,969 | 165,438 | 70,902 | 1,954 |
| 64 GiB | 98,475 | 23,634 | 3,939 | 3,939 | 200,000 | 200,000 | 7,864 |
| 128 GiB | 200,889 | 47,268 | 11,817 | 3,939 | 200,000 | 200,000 | 15,744 |
| 256 GiB | 401,778 | 98,475 | 23,634 | 3,939 | 200,000 | 200,000 | 35,442 |

Every cell is the same for the base and now.

**In the full reservation** nothing that was measured changed. Bytes copied are as before up
to 2 GiB. Against the rule as first merged (five interleaved rounds): the 1 and 4 MiB loops
-0.3% and -0.6%, `vecgrow 1x256Mi` +0.0%, `vecgrow 64x4Mi` -3.2%. Under a 2 GiB
`RLIMIT_DATA`, which leaves the reservation full, the counts are still 789, 28 and 26 buffers
grown to 1, 4 and 16 MiB. The settling test now runs every plan of the design's geometry and
meets all three pairs of headroom and threshold that can occur: sixteen times with a
thirty-second, four times with a thirty-second (a small four-times slot of a full heap), and
four times with an eighth.

## Growing blocks fault their spilled huge page in small pages

2026-10-18, at main `a92d51a`. A full session had asmalloc well behind mimalloc on two build
jobs, with tens of thousands of faults to mimalloc's few hundred: `vecgrow 64x4Mi` 5.89
against 3.06 cycles an element, `map hash 1M` 534 against 434 cycles an entry.

**Method.** Workloads run by hand on CPU 2 under `scripts/memlimit` and the measurement lock,
builds interleaved in a fresh order each round, three to five rounds, medians; faults, user
and kernel cycles from the job's own counters; huge-page faults from `thp_fault_alloc` in
`/proc/vmstat` around each run. Fault addresses from `perf record -e page-faults -c 1 -d`,
grouped by region and by offset within a slot. The machine was shared and busier than in the
full session, so absolute times run a few to 25% higher; compare within a table. The neighbour
check is `benchrun run` (one round per hold of the lock, three rounds) with the old build as an
extra allocator entry.

**vecgrow 64x4Mi.** All of the gap is the kernel: 543M kernel cycles against mimalloc's 30M,
while asmalloc's user cycles are lower (228M against 380M). Per vector and repetition there
were about 722 small faults and one huge one:

- 504 in the 40 MiB slot's second huge page. A vector moves at 2 MiB into a slot with headroom,
  at its stagger, so its 2 MiB end a few bytes into the next huge page. Advice is rounded down
  (`region::advise_grown`), so the program writes those bytes on a small page, and the page
  table that creates keeps the whole huge page on small pages when the vector doubles in place
  to 4 MiB and the advice reaches it. This is the spill described under "Headroom for growing
  blocks"; there, advising the spill page ahead cost 126 MiB of peak.
- 226 in the 1.25 MiB slots of the outgrown 1 MiB blocks. A move returns the slot it outgrew
  once the budget for kept grown blocks is spent, and the next repetition faults it back.

mimalloc faults 132 huge pages in the first repetition and nothing after: it keeps what is
freed and reuses it at any size, so the 4 MiB blocks of one repetition become the small and
medium blocks of the next. Class segregation rules that out here; keeping either kind of block
instead costs peak (below).

**The change.** When an in-place growth extends a block's advice over the huge page that holds
its old end, and the block used only the first page of it, that page is copied aside into the
block's new room, the huge page is returned, and the page is copied back
(`Back::unspill`). `MADV_DONTNEED` frees a page table it empties (`reclaim_pt`, Linux 6.14 and
later with `CONFIG_PT_RECLAIM`, on by default on x86-64), so the copy back faults the huge
page whole. Where the page table stays, the page goes on in small pages as before. Costs of
the ways to fill such a page, measured with a scratch C program, TSC ticks per 2 MiB, one 4
KiB page written first, median of 64:

| Fill the rest of the huge page | ticks |
|---|---:|
| as small faults | 1,146k to 1,191k |
| `MADV_COLLAPSE`, then write | 503k to 518k |
| copy aside, `MADV_DONTNEED`, copy back, then write | 376k |
| a fresh advised huge page (reference) | 357k to 389k |

`MADV_COLLAPSE` breaks even with small faults at half the page written, and it calls
`lru_add_drain_all`, which queues and waits for work on every CPU, and takes the mmap lock for
writing; not chosen.

**Before and after**, `benchrun`, three rounds:

| Job | before | after | mimalloc | jemalloc |
|---|---:|---:|---:|---:|
| vecgrow 64x4Mi, cycles/element | 5.69 | **3.97** | 3.06 | 7.48 |
| the same, faults | 185,064 | 56,544 | 279 | 246,894 |
| the same, cgroup peak MiB | 277 | 278 | 265 | 303 |
| vecgrow 1x256Mi, cycles/element (five rounds by hand) | 1.778 | 1.780 | | |
| vecgrow 1x4Mi grow-and-free | 0.478 | 0.485 | 0.872 | 0.800 |
| vecgrow 1x1Mi grow-and-free (five rounds by hand) | 0.299 | 0.299 | | |
| map hash 1M, cycles/entry | 537 | 536 | 464 | 632 |
| map btree 1M | 920 | 916 | 838 | 962 |
| tree depth=20 | 44.1 | 44.0 | 46.1 | 97.3 |
| tree depth=18 | 40.1 | 38.4 | 34.6 | 64.9 |
| json 32Mi | 16.38 | 16.18 | 16.58 | 23.54 |
| json 8Mi | 16.73 | 17.01 | 15.98 | 23.41 |
| pipeline t2 sizes=large, ns/alloc | 80.3 | 80.6 | 55.6 | 386 |
| pipeline t2 x2 sizes=small | 5.46 | 5.36 | 20.6 | 9.80 |

In the `benchrun` session 1x256Mi read 1.84 and 2.18 with ±5% and identical faults, and
1x1Mi showed 470 and 727 faults; run again interleaved by hand, both are level (above). Neither
can reach the change: one vector lives in slot 0, which has no stagger, and a 1 MiB block gets
no advice. sh8bench through the preload library, which allocates no large blocks, ran 0.606 s
after (full session 0.60). Cgroup peaks from `bench/probes/memory/peaks.sh` agree within 0.2
MiB before and after on every job it runs (pipelines, churn, maps at 200k, tree 18, json 8 MiB,
both vecgrows).

**After review (2026-10-19).** As first merged, the change cost time and memory in two cases
the jobs above never reach, since a single vector sits in slot 0, which has no stagger:

- It returned the page on arithmetic alone. A staggered buffer grown and freed again and
  again lands in the slot it was kept in (the budget for kept grown blocks), where the huge
  page is resident already; the growth threw it away and faulted it back, one huge fault a
  doubling, and with transparent huge pages off for the process 2 MiB dropped and 511 small
  faults. Now the OS is asked (`Os::resident`, `mincore`) whether any page of the huge page
  past the first is resident, and the page is returned only if none is. The clean bits cannot
  answer this: an advised block's extent is rounded up to a huge page, so they are cleared
  over the page before the program writes it.
- It set the page aside one huge page on, which for any doubling from two huge pages up lies
  inside the range just advised, so the copy faulted a huge page the program had not reached.
  Now it goes in the first page of the huge page holding the new end, which the rounded-down
  advice never covers.

With transparent huge pages off, a growth in a fresh slot still returns one small page and
faults it back, for nothing; the allocator does not know the setting, and that costs one call
and one fault a doubling.

The review's probe (`crates/asmalloc-preload/tests/programs/unspill_probe.c`), preloaded,
CPU 2, the builds interleaved, three rounds that agreed within a few percent; one buffer in
slot 1 grown by doubling from 1 MiB and freed, eight repetitions; the sums over repetitions 2
to 8 of faults inside `realloc` (huge ones in brackets), resident memory added inside
`realloc`, and TSC ticks of `realloc` and fill together. "before unspill" is `a92d51a`, "as
merged" main `16bf1b1`, "fixed" this change.

| To, slot | THP | before unspill | as merged | fixed |
|---|---|---:|---:|---:|
| 8 MiB, kept slot | on | 0, +0 MiB, 100.5M | 14 [14], +0 MiB, 101.3M | 0, +0 MiB, 100.3M |
| 8 MiB, kept slot | off | 0, 100.4M | 14 and 7,154 in the fill, 111.4M | 0, 100.5M |
| 16 MiB, fresh slot | on | 0, 250.4M (10,780 in the fill) | 42 [35], +70 MiB, 237.0M | 42 [21], +42 MiB, 237.2M |
| 16 MiB, fresh slot | off | 0, 270.5M | 42, 269.6M | 42, 270.7M |

In a fresh slot every repetition now faults three huge pages inside `realloc`, each the one the
block had begun, instead of five; with the slot kept, nothing, as before the change. The
`mincore` call costs about 2.5k ticks a growth. No run lost a byte.

`vecgrow`, by hand as above, five interleaved rounds, cycles an element, faults and huge-page
faults; the 2-vector jobs keep vector 1 in staggered slots that are kept between repetitions:

| Job | before unspill | as merged | fixed |
|---|---:|---:|---:|
| vecgrow 64x4Mi | 6.70, 184,897 [260] | 5.02, 56,377 [512] | 4.98, 56,376 [512] |
| vecgrow vecs=2 max=4Mi reps=1000 | 0.508, 1,355 [3] | 0.589, 1,843 [1,003] | 0.506, 844 [4] |
| vecgrow vecs=2 max=8Mi reps=500 | 1.516, 514,342 [1,004] | 0.623, 4,341 [2,004] | 0.625, 4,342 [2,004] |

The machine was busier than in the session above, so 64x4Mi reads higher in every column;
peaks are the same in all three builds (278.8, 13.7 and 21.4 MiB).

**What is left on vecgrow 64x4Mi.** 56K small faults on the outgrown 1 MiB blocks, and 512
huge-page faults, two per vector and repetition, all of it faulting memory back that was
returned. Both are the price of the memory bar. A budget for kept grown blocks of 64 MiB
instead of 16 (a scratch build) runs 4.07 cycles an element with 19,896 faults, but peaks at
327 MiB, over jemalloc's 305.

**map hash 1M.** 52K of the 60.6K faults are in the first repetition (one repetition alone
takes 51,952), first touches of the tables and of the value classes, all on 4 KiB pages; later
repetitions find the tables resident. The tables are large blocks allocated at their size,
which get no huge pages by design, so they stay on small pages in every repetition: the build's
extra dTLB misses and walks in "Build phases against mimalloc". The peak is the outgrown
tables: at the last resize the four or five smaller tables freed earlier in the repetition,
about 33 MiB, are still resident and retained (under the 64 MiB cap, and a free never purges),
next to the old and new tables; jemalloc reuses that memory for the values. Huge pages for every
large block of 2 MiB or more at allocation (a scratch build, an upper bound; it breaks the
sparse-block rule) measured, three rounds by hand:

| map hash 1M | cycles/entry | faults | VmHWM MiB |
|---|---:|---:|---:|
| main | 547.5 | 60,558 | 206.5 |
| huge pages for at-size blocks of 2 MiB or more | 467.1 | 21,432 | 181.3 |
| mimalloc | 468.7 | 160 | 301.2 |
| jemalloc | 633.5 | 181,212 | 186.0 |

It closes both gaps: the tables fault whole, and freed tables carry advice, so past the budget
for kept grown blocks their free returns them. Its cost is the case the rule exists for: a
block of 2 MiB or more that is touched once every 2 MiB becomes wholly resident, up to 512
times what it touched (arithmetic). Not adopted; whether to take that trade, or to learn
density per class at the free (which would help the repetitions after the first, but not the
first, where most of the faults are), is the owner's decision.

## Small heaps

2026-10-18, at main `779ba9b` (before) and the branch's tip (after). Every page of an asmalloc
process on the small jobs, by structure, and what was trimmed.

**Method.** `bench/probes/memory/acct.sh` runs each job with the full suite's arguments, CPUs
and confinement, capped, in a cgroup of its own, interleaved by round, under the measurement
lock. Builds with the `memacct` feature (any allocator) hold the process at exit for 2.5 s so
the kernel flushes `memory.stat`, then write the cgroup's counters, smaps and, from `pagemap`,
each mapping's own pages and its pages mapped to the shared zero page. asmalloc builds with
`asmalloc-memstat` add the heap's walk (where every region, metadata block, the magazine area
and the slabs are, and at the sample where the program held the most, each class's objects by
use). `acct.py` attributes every own page to a structure (the binary's data by symbol) and
estimates page tables per structure (a PTE page per 2 MiB, a PMD page per 1 GiB touched,
charged to the structure that touched it first; a lower bound, 70 to 95% of VmPTE). The
probes' own buffers are left out. For the workloads the state at exit is the peak's:
memory.current at exit is within 0.3 MiB of memory.peak. The RPC servers' exit is up to 1 MiB
under their peak, so their breakdown is indicative.

Two findings about the measurement itself. The cgroup peak of a process this small includes
the kernel's per-CPU charge stock, up to 64 pages per CPU that charged, and moves by up to
0.4 MiB between identical runs; so "charged" below is what memory.stat says the process was
charged for its own pages, page tables and kernel slab objects, which repeats within a few
tens of KiB. And the parked objects of a fresh fill cost nothing: a magazine filled by a carve
names objects nobody has touched, so a first fill of 254 objects is as cheap as one of three.
Only objects that have been used and come back pin pages. That kills the idea of a short
first magazine.

### Where the pages are

KiB at exit, one median run each, before the trims; jemalloc for comparison.

| Structure | tiny | churn 1k | pipeline t2 | mpmc 4+4 | RPC echo | scales with |
|---|---:|---:|---:|---:|---:|---|
| binary data and BSS (jemalloc 48) | 56 | 56 | 56 | 56 | 104 | per process |
| slabs (BSS) | 4 | 4 | 8 | 32 | 16 | 4 KiB per CPU used |
| object pages | 44 | 216 | 828 | 3,272 | 1,732 | per class: at least a page; then live and parked objects |
| region metadata | 44 | 84 | 204 | 212 | 488 | 4 KiB per class (the header); 16 KiB per class whose frees reach the bitmaps |
| magazine area | 20 | 40 | 212 | 880 | 360 | 2 KiB per 256-slot magazine ever made: per CPU and class, about 1 loaded + up to 8 spares |
| glibc, stacks, libraries, TLS | 160 | 164 | 216 | 312 | 320 | per process and thread (jemalloc's heap is in this row for it) |
| user pages, total (jemalloc) | 336 (368) | 576 (540) | 1,524 (1,328) | 4,764 (3,000) | 3,020 (1,664) | |
| page tables (jemalloc) | 252 (72) | 404 (76) | 432 (112) | 492 (208) | 712 (144) | about 16 KiB per class in use: 8 for its object region, 8 for its metadata block |
| kernel slab objects (jemalloc) | 45 (17) | 59 (18) | 65 (21) | 75 (30) | 136 (27) | per class: the VMAs that commits and huge-page advice split off |

Objects at the peak sample, KiB of objects on resident pages (memstat):

| Job | live | loaded | spares | depot | free in bitmaps |
|---|---:|---:|---:|---:|---:|
| pipeline t2 | 134 to 302 | 252 to 334 | 109 to 117 | 87 to 216 | 32 to 210 |
| mpmc 4+4 | 319 to 356 | 1,010 to 1,100 | 468 | 897 to 1,014 | 318 to 587 |
| RPC echo | 423 to 441 | 576 to 779 | 95 to 132 | 205 to 328 | 357 to 541 |

What the table says, against the suspects:

- **Page tables are the largest fixed cost**: 180 to 570 KiB over jemalloc, about 16 KiB per
  class in use. Each class's region is 4 GiB from the next, so its first page needs a PTE and a
  PMD page of its own (8 KiB). Its metadata block, 128 MiB from the next, needed a PTE page for
  the header, which sat after the leaf bitmap (up to 64 MiB in), and another for the leaf
  bitmap's first page once frees reached the bitmaps.
- **Region metadata**: one page per class for the header alone, four (leaf, header with level
  1, level 2, levels 3 and 4) once the class's frees reach the bitmaps.
- **The magazine area**: spare stacks four deep. A CPU that only allocates a class keeps four
  empty magazines of it that it never fills, and the CPUs that free make new ones.
- **Parked objects** are the whole story of mpmc: 2.5 MiB loaded, in spares and in the depot
  against 0.3 MiB live. Spares were 468 KiB of it: four full magazines of the small classes on
  each consumer CPU, which only frees and never takes them back. The depot's 0.9 to 1 MiB is
  the second domain's stacks filling to the take-across threshold before the producers' domain
  takes from them.
- **Not suspects after all**: the slab array (4 KiB per CPU used), the magazine area's first
  unit and commit-ahead (address space only; nothing touched beyond use), eager
  initialisation (nothing found that writes a page it would not otherwise touch).

### What was trimmed

1. **Region headers out of the metadata blocks, leaf bitmap last** (`back/layout.rs`,
   `back/host.rs`). The `Grow` and `Hints` words of every region now sit side by side in the
   back end's state, 16 bytes each, four to a line, and a block runs top summary level first
   and the leaf bitmap last, starting a page. A small class whose frees never reach the bitmaps
   never touches its block; one that does touches two or three pages, all in its first 2 MiB,
   under one page of page tables. (A large class writes its slot's size word in the block on
   every allocation, so it touches its block as before.) The generated routines read neither. Kani's layout harnesses and
   the safety ledger entry BACK-HOST-01 are updated.
2. **One spare magazine per stack** (`MAX_SPARES`, 4 to 1). One covers a thread oscillating at
   a magazine boundary; deeper stacks filled only on CPUs whose traffic goes one way.

Charged at exit (own pages, page tables and kernel slab objects), MiB, median of five
interleaved rounds:

| Job | before | headers | + one spare | jemalloc | glibc |
|---|---:|---:|---:|---:|---:|
| tiny (three allocations) | 0.57 | 0.47 | 0.47 | 0.44 | 0.22 |
| churn, 1k live | 0.94 | 0.75 | 0.75 | 0.61 | 0.38 |
| pipeline t2 small | 1.92 | 1.79 | 1.71 | 1.42 | 0.46 |
| mpmc 4+4 | 5.19 | 5.00 | 4.52 | 3.16 | 0.98 |
| RPC echo c64 | 3.70 | 3.56 | 3.42 | 1.79 | 0.95 |
| RPC log c64 | 4.68 | | 4.50 | 1.75 | 0.89 |
| RPC log c256 | 6.96 | | 6.52 | 2.14 | 0.90 |

(The RPC servers' jemalloc and glibc figures at exit are after the server has stopped and
their allocators have given memory back; compare their peaks.) The "headers" column is from a
three-round session of its own.

Cgroup peak, MiB, median of five interleaved rounds:

| Job | before | after | jemalloc | glibc |
|---|---:|---:|---:|---:|
| tiny | 0.88 | 0.68 | 0.71 | 0.48 |
| churn, 1k live | 1.18 | 1.04 | 0.87 | 0.75 |
| pipeline t2 small | 2.39 | 2.31 | 2.05 | 1.38 |
| mpmc 4+4 | 6.30 | 5.55 | 4.81 | 3.34 |
| RPC echo c64 | 5.84 | 5.50 | 5.93 | 3.35 |
| RPC log c64 | 8.44 | 7.46 | 9.36 | 4.63 |
| RPC log c256 | 12.32 | 12.32 | 12.94 | 7.07 |

The tiny process and the RPC servers are now at or under jemalloc's peak. Churn is 0.17 MiB
over, the pipeline 0.26 and mpmc 0.74.

Time, the jobs that fit the measurement CPUs (one L3 domain, CPUs 0 to 7), five interleaved
rounds under the lock; change of the trimmed build against `779ba9b`, median of per-round
ratios [lowest, highest]:

| Job | before | change |
|---|---:|---:|
| pipeline t2 small, all-alloc (ns/alloc) | 3.714 | +0.5% [-17.3, +0.8] |
| the same, free-only | 3.719 | -0.1% [-2.9, +1.6] |
| exchange t2 small | 4.243 | -2.5% [-3.0, +1.3] |
| two pipelines t2, one domain (ns/alloc per chain) | 4.004 | +1.0% [-1.4, +6.2] |
| mpmc 3+1 | 4.809 | -0.2% [-0.8, +1.0] |
| mpmc 4+4 on CPUs 0 to 7 | 1.860 | +0.9% [-3.5, +2.6] |
| churn, 100 live (cycles/op) | 24.64 | +0.2% [-0.1, +1.2] |
| churn, 1k live | 25.47 | -0.2% [-0.8, +0.1] |
| churn, 100k live | 47.86 | -0.3% [-0.6, +0.0] |
| churn, 1M live | 116.6 | -2.0% [-10.6, -1.7] |

Nothing resolvably slower on these. The back end's smoke set (`benchrun run --suite backend`,
under the lock, CPUs 0-3 and 8-11) was then run at main `3882aee` against the branch merged
with it, with one, two and four spares (the four-spare build is the header move alone).
Median of five interleaved rounds, change against main:

| Job | main | one spare (tip) | two spares | four spares |
|---|---:|---:|---:|---:|
| pipeline t2 large objects (ns/alloc) | 71.98 | +3.8% | +5.4% | +4.4% |
| two pipelines across L3s (ns/alloc per chain) | 4.858 | -1.2% | -0.3% | -0.8% |
| tree depth 18 (cycles/node) | 37.22 | -0.6% | -0.9% | +3.7% |
| JSON 8 MiB (cycles/byte) | 17.82 | +0.4% | -2.0% | -0.5% |
| sh8bench 16, preloaded (s) | 0.604 | +3.0% | +1.7% | +0.9% |

sh8bench, which alternates in bursts of a few magazines, is where deeper spares earn
something. Over three sessions (5, 5 and 12 rounds) one spare cost it +7.9, +3.0 and +1.7%,
two +3.5, +1.7 and +1.2%, four (the header move alone) +0.9 and -0.1%. The tree and JSON jobs
do not resolve a difference. Memory, the other side: two spares instead of one leave mpmc 4+4
0.4 MiB higher at its peak (6.22 against 5.54 MiB in the candidate session above) and the
pipeline about 0.1 MiB.

The pipeline of large objects is 3.8 to 4.4% slower with every spare setting, so the header
move costs it. The first measurement was 6.8%: with the layout's slow path inline, `Back::host`
was no longer inlined and every large allocation and free copied the class's host through
`memmove`; working the layout out in a cold call of its own removed that. What remains is
2.5% more instructions and 14% more L1 data misses per run. These were ruled out, each as a
build of its own: spare depth; four headers to a line (a line each measured the same);
putting a large class's header back in its block; putting a large class's leaf bitmap first,
next to its summary and zero bitmap; and both together, which came to +3.5%. Its cause is not
found.

*Found, the same day, by attribution.* `callgrind` on `largepair` (single-threaded, the same
large allocate and free sequence; 200k operations) at `3882aee` and `1868dd0`: 3550 against
3677 instructions per operation, all of the difference in `Back::alloc_at` (+55 per call),
`tree::release_one` (+9) and `Heap::free_large` (+6). Per instruction, `alloc_at` executed
19 more stack stores and 18 more loads per call: the layout memo returned a layout from two
places, the unpacked words and the cold call that works one out, so `alloc_at` built the
class's host twice and copied it; and `Back::grow` had come to be inlined whole, adding spills.
`release_one`'s extra nine are the host's block cache, which a free used to fill by reading the
header and now fills in `release_one`. AMD IBS op sampling with data addresses (five runs a
build, about 18,000 user samples each) put the extra L1 misses of the pipeline in the large
classes' metadata blocks: 31 misses in 45 accesses on the page holding the zero bitmap and
the start of the sizes, which with the header moved out and the leaf bitmap after the sizes
were no longer near the summary. None of the extra misses were on lines both threads write.

Fixed (`dc2c720`): the cold call only fills the memo and the layout is always unpacked from
it; paying the growth debt is a cold call of its own; and a large class's block, which every
large allocation and free touches anyway, starts with its header (on its own line), then the
summary, leaf and zero bitmaps, then the clean bitmap and sizes, and is committed whole at
assignment. Small classes keep the layout that saves their pages. `perf stat`, five
interleaved rounds under the lock, medians:

| Job | `3882aee` | `1868dd0` | fixed |
|---|---:|---:|---:|
| pipeline t2, large objects (ns/alloc) | 72.61 | 76.61 | 72.28 |
| largepair (ns/op) | 75.36 | 80.65 | 74.86 |
| largepair, instructions | 6.061 G | 6.333 G | 6.061 G |

### What is left, and proposals

At the tip, against jemalloc:

- **Object regions' page tables, about 8 KiB per class in use.** Churn with 1k live objects
  uses 17 classes; its whole remaining gap (0.14 MiB charged) is this and the kernel's VMA
  objects. A region's first page needs a PTE page and a PMD page because regions are 4 GiB
  apart. Only a change to where classes start would share them, for example carving each
  class's first few pages from a shared small-start region, or smaller regions; both are design
  changes to the address scheme, and this is their price: 8 KiB times the classes in use.
- **Parked objects in cross-thread flows.** mpmc 4+4 at the tip parks 1.1 MiB loaded across
  eight CPUs, 0.3 MiB in spares, 0.9 MiB in the depot, against 0.3 MiB live, with 0.75 MiB
  free in the bitmaps on resident pages. Two proposals, each measured as a scratch build:
  - *Magazines of at most 128 slots* (`MAX_LOG2` 7) brings mpmc to jemalloc's charge (3.47
    against 3.29 MiB charged, peak 5.07 against 4.87) and the pipeline under it (1.34 against
    1.42, peak 1.70 against 1.86), but costs 16 to 20% on the pipelines, 7% on exchange and mpmc
    3+1, 5% on mpmc 4+4 and 3.4% on churn with 100k live. Not worth it as it stands; a magazine
    size that adapts per class and CPU (short on a CPU whose frees of a class go one way, full
    where it both allocates and frees) would be the version to try.
  - *The depot's cross-domain threshold.* The second domain's stacks fill to half the class's
    target before the first takes from them: 0.9 MiB in mpmc 4+4. The depot is being changed
    on another branch (a domain cap with a shared stack, `d37faa7`), which bounds this by one cap
    per domain; its cap is the number to choose with this job in view.
- **The magazine area, 2 KiB per 256-slot magazine ever made, never returned.** 128 KiB on the
  pipeline and 544 KiB on mpmc at the tip. Magazines are never unmapped by design (stale links
  read harmless memory); purging the pages of empty magazines on the depot's empty stacks would
  need their links kept elsewhere. Not proposed now.
- **Kernel slab objects**, 30 to 110 KiB over jemalloc: a VMA per piece the commits and the
  huge-page advice split off the reservation, a few per class.

## The clocked pass spares what fills reach: a frontier per region

2026-10-19. On main every clocked pass started at the top of the heap, which left a quiet
heap's lowest region resident for ever (`baselines.md`, "The clocked pass carries on round
the heap"). Carrying the pass on round the heap removed that residue but cost the busy RPC
server 2 to 4% and more faults, because the pass then took its turn in the server's busiest
regions. Neither order knew which free pages were being reused.

Now each region keeps a frontier: the page past the highest slot a fill or allocation claimed
or carved there, for an interval of the clock and the one before, under the interval's
number (`back/layout.rs`, `Frontier`). Fills take a region's lowest free slots, so free pages
above the frontier went a whole interval unwanted, and the clocked pass takes only those; its
quarter, its steps and the carried-on order are unchanged. It does so over the cap too.
Growth over the cap pays as on main, from where the walk stands, frontiers or not. A fill
reads its region's word once and writes it only when it reaches higher or is the region's
first claim of an interval. Nobody sweeps the words.

**What changed after review.** The first version rotated every region's frontier when a pass
started, and over the cap took a second lap below the frontiers. The review (`782e88b`)
found that a heap with more than the cap of fragmentation, which no purge can lower, is over
the cap for good, so that lap purged what the program reused every interval; and that
growth's walk above the frontiers could starve its walk below them for good. The rotation
also wrote a large class's block header on a bare table lookup. The frontier now carries its
interval number, holds over the cap, and lives in the back end's state for every class. The
walk also goes round only the regions below the highest ever given to a class, so a lap no
longer reads every entry of the reservation.

**Method.** Main (`94b5b44`) against the branch (`f004a01`, "frontier"), each from
`git archive` of its commit with the same workload sources, interleaved. RPC jobs with
`benchrun run` under `scripts/measure --both-l3`, ten rounds, medians, with the per-round
ratio to main and its interquartile range; the suite's memory jobs three rounds. Faults are
the process's, and for RPC also those inside the measured window. `madvise` calls from
`strace -f -c` of separate runs on CPUs 12-15 (counts only). Memory probes with
`bench/probes/memory/worst.sh` and `workload period`, three rounds, medians. `backbench` on
CPU 2, three runs each. `workload fragchurn` on CPU 2, three rounds, in the normal build and
with every loop aligned to 64 bytes (benchrun layout's `loops64`).

| | main | frontier |
|---|---:|---:|
| rpc log c64: server cycles/req; vs main | 122.0k | 122.6k; +0.9% [-1.4, +1.2] |
| rpc log c64: faults, process / window | 2,810 / 1,050 | 1,830 / 250 |
| rpc log c256: server cycles/req; vs main | 119.7k | 119.8k; +0.05% [-1.7, +2.2] |
| rpc log c256: faults, process / window | 5,310 / 2,250 | 3,060 / 480 |
| rpc echo c64: server cycles/req; vs main | 44.77k | 45.05k; +0.7% [+0.5, +0.9] |
| rpc echo c64: faults, process / window | 1,380 / 260 | 1,140 / 54 |
| fragchurn: faults over 20 s; `madvise` calls | 36k to 46k; 38,330 | 4,730; 552 |
| fragchurn: cycles per object, normal build / loops aligned | 23.1 / 23.1 | 31.0 / 22.8 |
| burst-idle: MiB resident 1 / 2 / 4 / 8 / 12 s after freeing 218 MiB | 102 / 68 / 38.5 / 11.9 / 3.2 | 215 / 68 / 38.3 / 11.3 / 1.3 |
| phase change: MiB resident 1 / 2 / 4 / 6 / 8 s after | 37.6 / 28.7 / 16.3 / 12.5 / 8.0 | 46.7 / 35.4 / 20.0 / 11.6 / 7.3 |
| phase change: cgroup peak MiB | 46.9 | 49.6 |
| maintain test (`Native` shape): purges an interval, steady; left over `collect` after 40 intervals | 28; 50 pages | 11; 0 |
| `backbench` fill, full words / one slot a word: cycles and instructions a slot | 4.2, 15.7 / 15.2, 70.1 | 4.3, 16.4 / 15.8, 73.4 |

The suite's memory jobs: cgroup peaks identical (json 32Mi 354.1 MiB, map hash 204.2, map
btree 123.2, churn 1M 110.2, mstress 139 to 141), faults identical, times within 0.3% but
mstress, +2.6% [+0.7, +4.7] at the same faults. The maintain row's main figures are from the
`baselines.md` note (origins forced to zero).

- **The busy server keeps what it reuses.** Window faults fall by three quarters on all three
  RPC jobs, and the cost purge-resume's order had is gone: cycles per request are level on
  rpc log, and echo's 0.7% was traced in a thirty-round run of the first version to kernel
  scheduling and network locks with user cycles, throughput and latency level (no page-fault
  or `madvise` work in the kernel profile).
- **Fragmentation over the cap no longer costs the working set.** `workload fragchurn` (a
  gigabyte of 256-byte objects with one in eight kept at random, so more than the cap stays
  fragmented for good; then 400k 64-byte objects allocated, written and freed for twenty
  seconds): main purges and faults the working set back every interval, 38,330 `madvise`
  calls and 36k to 46k faults; the frontier makes 552 calls and no fault after the working
  set's first round. Its cycles per object swung between builds, 31 in the normal build of
  the branch against 23 for main and 22.8 against 23.1 with every loop aligned, at equal
  instructions and misses: not the purge, but a load held behind a store to the same page
  offset, which the build's placement of the fast path's tables decided
  (`zen5-roofline.md`, section 16).
- **Quiet memory waits for its frontier, over the cap too.** A burst that goes quiet keeps
  its memory one interval longer: 215 MiB at one second against main's 102, then level from
  two seconds and lower at twelve. The phase change holds up to 9 MiB more for the first four
  seconds and is level from six. Memory a fill reached stays until no fill has reached it for
  an interval, whatever the count says; growth over the cap still pays at once.
- **The fill pays a load and a compare:** 0.1 cycle a slot from full words, 0.6 a claim from
  fragmented bitmaps.

**Follow-ups, the same day.** Three changes after the second review, measured against main at
`c3c4676` (which has the frontier), three rounds unless said:

- **A fill that meets another records only what it reached before.** A run that meets
  another filler moves past its group of 64 words and claims beyond it; those claims are not
  evidence that the words skipped are in use, and they raised the frontier by a group per
  meeting (16 MiB of 4 KiB slots, 128 MiB of 32 KiB ones). `workload refill` (thread 0
  writes and frees 256 MiB of one class, then four threads on CPUs 0-3 each write and free
  8 MiB of it every second for ten seconds): main holds 129 MiB at 32 KiB and 49 MiB at
  4 KiB from four seconds to the end; the branch comes down to 28 MiB both ways. The price
  is a few refaults where fills overlap: 6% more faults at 32 KiB, level at 4 KiB, and on rpc
  log c64 (eight rounds) 363 window faults against 268 at level cycles (-0.3%
  [-1.1, +0.3]).
- **The interval number moves only forward**, unless the clock is more than an interval
  behind it.
- **The fill keeps its reach in the run's cursor.** `backbench`, cycles and instructions a
  slot, main / branch / the branch with no frontier work at all: full words 4.3, 16.4 / 4.3,
  16.4 / 4.2, 15.8; one slot a word (`fill/8`) 15.9, 73.4 / 15.9, 72.2 / 15.6, 70.6; one a
  group (`fill/64`) 102, 466 / 98, 459 / 96, 445. What is left is about 0.3 cycles a claim:
  two stores into the cursor and the record at the end of a fill.

The period probe at 1.5 s is level (44 and 37 faults a re-allocation).

## Faults and system calls in real programs

2026-10-19, at main `6302b96` and the branch `app-faults`. The screening of real Rust programs
(`rust-bench-candidates.md`) left two leads: far more page faults, `madvise` and `mprotect`
calls than the incumbents where mid-size blocks grow (tpchgen, gix-hours), and lost wall time
with many context switches in parallel ruff check. Both are explained.

- **`mprotect` grew linearly with the heap.** A region committed a fixed 64 KiB past each
  carve, so a large class paid one call for nearly every new slot and a small class one per
  64 KiB: 32,000 calls a run in gix-hours, 2,300 in ruff, 1,600 in tpchgen. Each takes the
  memory-map lock for writing, and the region's mapping, which every page fault in that
  region needs. In ruff about 190 of a typical run's 400 context switches were page faults and
  `mprotect` calls queued on that lock; in gix-hours the threads spent 2.6 s in all blocked
  inside `mprotect` (81 µs a call, against about 1 µs uncontended). **Fixed**: a commit now
  runs ahead by as much as the region has committed, between 64 KiB and 64 MiB (`policy::
  commit_ahead`), so the calls grow with the logarithm of each region. The lock waits in
  ruff fall to about 30, gix-hours spends 40 ms in `mprotect`. No memory moves; the cost is
  commit charge under strict overcommit, at most a region's use or 64 MiB.
- **Large-slot memory is returned and claimed again within milliseconds.** Two rules do it,
  and they are what the `madvise` calls and the excess faults are. A large block that grows
  out of its slot moves and gives the old slot back, piece by piece, whenever free large
  memory is over the 16 MiB kept-grown budget; the next buffer growing through that class
  claims the same slot and faults it back (tpchgen: 6,900 of its 9,400 calls, and about 70%
  of its faults). And growth over the 64 MiB cap pays by purging free memory from wherever
  the walk stands, frontiers or not, which in a program whose buffers climb through the large
  classes is the slots the next buffers take (gix-hours 6,450 calls, datafusion 3,000). Of
  what came back, 81% did so within a millisecond in tpchgen, 52% in gix-hours and 17% in
  datafusion; 86 to 100% within 60 ms. **Not fixed**: each fix holds memory, and the rules say what to hold. Options and
  their costs are below.
- **The wall time of parallel ruff is the program's own.** Its runs are bimodal under every
  allocator (0.56 or 0.75 s; glibc 0.70 to 0.95), the slow ones with hundreds of rayon yields
  and preemptions more. Over 40 runs here asmalloc on main was mostly fast; the screening's
  five slow runs were mostly the program. The lock waits were real, and the fix removes them.
- **Reserving and committing costs nothing measurable once the calls are logarithmic.** A
  build that maps the whole reservation readable and writable and never calls `mprotect`
  is level with the fix on every program.
- **The servers show the pattern at a smaller scale.** actix's order service made 1,670
  `mprotect` calls under load, 300 with the fix, and its context switches fell from 222 to
  138 (jemalloc 143), at level throughput and tail latency. Neither service returns memory
  it reuses: about 50 `madvise` calls, and half jemalloc's faults.

### Method

The programs, inputs and pinning are the screening's (`bench/probes/rust-screen/candidates.py`),
run from its built tree with the library preloaded: CPUs 0-3 and 8-11 for parallel programs, CPU
2 for pulldown-cmark, a 300 ms spin before each run. `bench/probes/app-faults/` holds the tools.
`bench.py` interleaves libraries in a fresh order each round and records `perf stat`'s user and
kernel cycles, CPU time, faults and context switches, the wall time from inside `perf stat`, and
the peak resident set from `/usr/bin/time`. Each variant is the branch's source with one rule
changed by a script in `edits/`, built by `variant.sh`.

**Call sites.** `diag.patch` (against `6302b96`, never for main) marks the OS calls and the back
end's functions between a rule and the call `#[track_caller]`, so each `mprotect`, `madvise`
and `mincore` is counted by the source line of the rule that asked, with its bytes and the
cycle-counter time it took, and prints the table at exit. It also counts revivals: pages of a
free slot that a purge had marked clean and a claim then took again, with the time since the
region's last purge. For large slots this includes pages never written since their carve
(marked clean too), so it is an upper bound of refaults. `strace -k` gave the same split but
slows the programs fifty-fold, which multiplies the clocked passes, so its counts are not used.

**Context switches.** `perf record -e context-switches -c 1 -g` (tracepoints are root's here),
each switch classified by its kernel stack: a fault waiting for the memory-map lock
(`rwsem_down_read_slowpath` under `do_user_addr_fault`), `mprotect` waiting for it for
writing, `mprotect` waiting for a mapping's readers (`__vma_start_write`), futex, preemption,
`sched_yield` (`csall.py`).

### Where the calls come from

One run each of main with the diagnostic patch. Calls by the rule that made them:

| | tpchgen | gix-hours | datafusion | ruff check | pulldown-cmark |
|---|---:|---:|---:|---:|---:|
| `mprotect`: carve of a large slot | 922 | 18,667 | 2,057 | 279 | 2 |
| `mprotect`: carve for a fill | 594 | 13,228 | 359 | 1,846 | 62 |
| `mprotect`: a new region's metadata and first slot | 133 | 134 | 163 | 105 | 49 |
| `madvise`: a move gives back the outgrown slot | 6,881 | 3,811 | 244 | 8 | 57 |
| `madvise`: growth over the cap | 1,171 | 6,450 | 3,016 | 0 | 11 |
| `madvise`: a move purges the destination under zero pages | 1,055 | 9 | 6 | 0 | 0 |
| `madvise`: free of a slot advised for huge pages, past the budget | 59 | 9 | 48 | 0 | 1 |
| `madvise`: tail of a block that shrinks in place | 0 | 0 | 0 | 478 | 0 |
| `madvise`: huge-page advice (regions, grown blocks) | 310 | 112 | 190 | 76 | 42 |
| clocked decay pass | 0 | 0 | 0 | 0 | 0 |
| pages of purged large slots claimed again, MiB | 1,840 | 573 | 500 | 7 | 4 |

The clocked pass, the rule built for idle memory, made no calls: none of these runs is long or
quiet enough. What growth over the cap purged, by class, was 32 KiB to 4 MiB slots in
datafusion (450 MiB) and 64 KiB to 16 MiB in gix-hours (960 MiB); in pulldown-cmark it was the
128 to 512 MiB slots of its input buffers (1,000 MiB), which nothing claimed again.

### What each rule costs, by experiment

Variants of main, three rounds each (medians): every purge refused (`nopurge`), a move keeping
the slot it leaves (`keepmove`), growth never paying (`nogrow`):

| | faults | kernel Gcycles | peak RSS MiB |
|---|---:|---:|---:|
| tpchgen: main / nopurge / keepmove / nogrow | 709k / 186k / 221k / 700k | 2.25 / 1.07 / 1.21 / 2.22 | 573 / 718 / 654 / 618 |
| gix-hours: main / nopurge / keepmove / nogrow | 997k / 818k / 942k / 880k | 3.39 / 2.84 / 3.25 / 2.95 | 2,933 / 3,251 / 2,935 / 3,237 |
| datafusion: main / nopurge / keepmove / nogrow | 249k / 146k / 243k / 163k | 2.19 / 1.67 / 2.14 / 1.71 | 447 / 655 / 446 / 569 |

So the moves' give-back is tpchgen's problem and growth's purge is gix-hours' and
datafusion's. Raising the kept-grown budget to 64 MiB did nothing for tpchgen (645k faults);
256 MiB did (234k, 633 MiB): the budget counts every free large slot, and ordinary frees,
which are always kept, fill it.

### The fix, and the options, five rounds interleaved

Medians of five rounds. *fix* is the branch (commit-ahead); *keep* adds: a move keeps the slot
it leaves whenever that slot alone is within the kept-grown budget (16 MiB), resident and
counted; *idle* adds: growth over the cap purges only above the frontiers, as the clocked pass
does; *rw* maps the reservation readable and writable and never calls `mprotect`.

| | wall s | Gcycles | kernel G | CPU s | faults | switches | peak RSS MiB |
|---|---:|---:|---:|---:|---:|---:|---:|
| **tpchgen** main | 1.36 | 26.5 | 2.49 | 4.89 | 704k | 1,449 | 572 |
| fix | 1.47 | 26.0 | 2.36 | 4.69 | 709k | 857 | 577 |
| fix+keep | 1.52 | 25.0 | 1.23 | 4.52 | 219k | 1,151 | 630 |
| fix+idle | 1.62 | 26.3 | 2.27 | 4.84 | 701k | 1,258 | 628 |
| fix+keep+idle | 1.60 | 24.9 | 1.13 | 4.48 | 203k | 1,336 | 709 |
| rw | 1.61 | 26.2 | 2.29 | 4.73 | 709k | 1,443 | 584 |
| jemalloc / glibc | 1.61 / 1.37 | 25.2 / 24.6 | 1.00 / 1.27 | 4.56 / 4.49 | 182k / 273k | 1,287 / 1,012 | 558 / 525 |
| **gix-hours** main | 2.30 | 47.9 | 3.44 | 8.78 | 997k | 13,726 | 2,934 |
| fix | 1.96 | 48.5 | 3.36 | 8.95 | 993k | 14,386 | 2,931 |
| fix+keep | 2.01 | 49.0 | 3.26 | 9.14 | 926k | 12,843 | 2,935 |
| fix+idle | 1.42 | 49.0 | 3.01 | 9.13 | 879k | 10,881 | 3,228 |
| fix+keep+idle | 1.28 | 49.1 | 2.84 | 9.16 | 816k | 29,361 | 3,241 |
| rw | 1.46 | 49.2 | 3.55 | 9.16 | 981k | 28,161 | 2,936 |
| jemalloc / glibc | 2.15 / 1.47 | 50.0 / 54.4 | 3.46 / 2.69 | 9.22 / 10.04 | 819k / 688k | 12,633 / 22,542 | 3,169 / 2,671 |
| **datafusion** main | 1.08 | 23.3 | 2.18 | 4.30 | 249k | 16,583 | 450 |
| fix | 1.04 | 24.6 | 2.49 | 4.58 | 245k | 14,006 | 442 |
| fix+keep | 0.98 | 25.5 | 2.42 | 4.72 | 234k | 14,316 | 476 |
| fix+idle | 1.01 | 23.8 | 1.92 | 4.41 | 162k | 14,123 | 568 |
| fix+keep+idle | 1.03 | 23.7 | 1.78 | 4.40 | 149k | 13,471 | 572 |
| rw | 1.01 | 25.7 | 2.55 | 4.76 | 249k | 14,226 | 443 |
| jemalloc / glibc | 0.99 / 1.08 | 23.5 / 24.2 | 2.02 / 2.54 | 4.39 / 4.50 | 184k / 287k | 14,254 / 21,760 | 674 / 429 |
| **pulldown-cmark** main | 0.927 | 5.14 | 0.47 | 0.92 | 85.8k | 10 | 541 |
| fix | 0.918 | 5.09 | 0.46 | 0.92 | 85.8k | 9 | 541 |
| fix+keep | 0.926 | 5.14 | 0.54 | 0.92 | 85.8k | 11 | 541 |
| fix+idle | 0.927 | 5.09 | 0.44 | 0.92 | 84.9k | 11 | 664 |
| jemalloc / glibc | 1.035 / 0.964 | 5.75 / 5.34 | 0.96 / 0.71 | 1.04 / 0.96 | 269k / 171k | 11 / 13 | 543 / 541 |
| **ruff check** main | 0.627 | 11.19 | 0.44 | 2.06 | 51.3k | 685 | 382 |
| fix | 0.570 | 11.19 | 0.42 | 2.06 | 52.4k | 213 | 388 |
| fix+keep / fix+idle | 0.560 / 0.602 | 11.15 / 11.13 | 0.41 / 0.41 | 2.05 / 2.04 | 51.2k / 51.3k | 249 / 564 | 386 / 383 |
| rw | 0.594 | 11.13 | 0.40 | 2.05 | 51.5k | 1,059 | 383 |
| jemalloc / glibc | 0.600 / 0.725 | 11.37 / 12.43 | 0.51 / 0.53 | 2.09 / 2.29 | 96.6k / 102k | 307 / 522 | 380 / 400 |

`mprotect` calls with the fix (diagnostic builds, one run each): tpchgen 478, gix-hours 740,
datafusion 500, ruff 510, pulldown-cmark 117.

Reading it:

- **Wall time of the parallel programs is bimodal and mostly not ours.** tpchgen (1.25 or
  1.65 s) and gix-hours (1.3 or 2.3 s) have two modes under every allocator, and their fast
  mode uses more cycles and CPU time, not fewer; ruff likewise. Rank them by kernel cycles,
  faults and switches. With that caution: gix-hours on main was in its slow mode in 11 of 13
  runs (2.15 to 2.33 s), with the fix in 4 of 13 and between modes (1.4 to 1.8 s) in 4; it is the program whose threads
  were blocked longest in `mprotect`, so the fix plausibly changes its schedule. Not
  established.
- **The fix** cuts the calls three- to forty-fold, the lock waits, and in ruff context
  switches by two thirds. Cycles, faults and memory are level. The backend suite is level
  but for `pipeline t2 sizes=large`, 2.4% slower in two sessions (60.9 against 59.4 ns); the
  branch's own default layout under `benchrun layout` measured 59.45 with a 1% spread across
  layouts, so it is placement, not the change. Memory probes (the peaks of the suite's memory
  jobs, and after the peak) are identical.
- **keep** takes tpchgen's faults from 704k to 219k and its kernel cycles from 2.5 to 1.2 G,
  under jemalloc's and glibc's, for 58 MiB (10%) of peak: 630 MiB against jemalloc's 558.
  Elsewhere it is level, but 7% fewer faults in gix-hours.
- **idle** takes datafusion's faults from 249k to 162k and kernel cycles by 12%, and gix-hours'
  by 12% and 10%, for 118 MiB (26%) of datafusion's peak, still under jemalloc's 674, and
  300 MiB (10%) of gix-hours', 2% over jemalloc's. But pulldown-cmark, which its frees do not
  bring back, goes from 541 to 664 MiB against jemalloc's 543: its input buffers were claimed
  in the last interval, so the frontier protects them, and growth is no longer allowed to
  take them. As built it fails the memory bar.
- **rw** is level with the fix; it leaves the faults and the `madvise` calls where they were.

### What the rules could use to hold on

Both returns are of memory claimed again within milliseconds to tens of milliseconds, far below
the clock's interval of a second, and the frontier cannot tell it apart: a large class has few
slots to a region, and the slot a growing block leaves was claimed in this interval by that
very block, so the frontier always covers it. In pulldown-cmark the same evidence covers a
slot that nothing will claim for hundreds of milliseconds. Evidence that would separate them:

1. **A free slot's age.** A large slot's size word, written only by its holder, could take the
   time of the free; growth's purge would skip slots freed within a fraction of an interval
   (say a sixteenth, 60 ms, which covers 86 to 100% of the revivals above). pulldown-cmark's
   input buffers wait longer than that between free and reuse, so growth would still take
   them. A mechanism of moderate size, and a design decision: the cap's rule is that growth
   pays now.
2. **Reuse of freed slots, not carves.** A frontier written only by claims of freed slots
   would say "this class reuses what it frees"; carves would stop counting. A second word per
   region, and a change to what the frontier means.
3. **A larger kept budget for moves, counted apart from ordinary frees** (keep, above, is the
   simplest form): costs about 10% of tpchgen's peak, which is then 13% over jemalloc's.

### Smaller findings

- The order services (`bench/probes/app-faults/srv.py`, the screening's axum and actix
  servers on CPUs 0-3 under oha on 8-11, 400k requests, five rounds): actix 1,670 `mprotect`
  calls on main and 300 with the fix, context switches 222 and 138, throughput 468k and 471k
  requests a second, p99 0.175 and 0.174 ms, p99.9 0.248 and 0.255 ms (jemalloc 465k, 0.171,
  0.228). axum level likewise (347k and 330k against jemalloc's 341k, within its run-to-run
  spread; its process exits without running the diagnostic's report). asmalloc faults half as
  often as jemalloc in both and makes about 50 `madvise` calls a run. Closed-loop and
  saturated, so this says nothing about tail latency at a fixed load.
- A single slow call: on main an `mprotect` took 35 to 80 µs on average in the parallel
  programs, nearly all of it waiting for the lock, and held the lock against every faulting
  thread while it ran. With the fix there are a few hundred a run. Growth over the cap may
  walk up to 16,384 bitmap words and make many `madvise` calls in one allocation; its worst
  call was not measured.
- After the peak (`memrun.sh ... quiet live=2M sizes=small secs=12 after=burst
  after_sizes=small`): in 3 of 8 runs, on main and on the branch alike, asmalloc held 215 MiB
  to the end instead of returning to 3 MiB. Not investigated; it is not this change.

**Design text.** "Address space" says a region becomes writable "in steps, each a single
`mprotect` that extends the writable prefix well ahead of what is in use". It could say: by as
much as the region has made writable already, up to 64 MiB, so the number of calls grows with
the logarithm of the region's use; each call takes the memory-map lock for writing, which
page faults in the region wait for.

**A working set that comes and goes on a period longer than the interval.** `workload period
live=N sizes=small cycles=4 gap_ms=G after=burst after_sizes=s64`: allocate N objects and
write them, free them in random order, run burst-idle's light work for G ms, four times. MiB
resident one and two seconds into each gap and at its end, mean over cycles, and the faults
of each allocation after the first:

| | main | frontier |
|---|---:|---:|
| 300k objects (33 MiB, under the cap), 2.5 s gaps: MiB at 1 s / 2 s / end | 25.1 / 19.2 / 17.0 | 32.5 / 28.6 / 24.3 |
| the same: faults per re-allocation | 3,820 | 2,140 |
| 300k objects, 1.5 s gaps: MiB at 1 s / end | 25.1 / 22.2 | 32.5 / 32.5 |
| the same: faults per re-allocation | 2,450 | 41 |
| 2M objects (218 MiB, over the cap), 2.5 s gaps: MiB at 1 s / 2 s / end | 101.7 / 82.4 / 67.6 | 215.0 / 92.5 / 58.8 |
| the same: faults per re-allocation | 35,180 | 31,730 |

Under the cap the frontier keeps the working set through a gap of one and a half intervals
and faults almost nothing when it returns, and a little less than half as much after two and
a half. Over the cap it is spared for the first interval of the gap now, and faults a tenth
less when it comes back.

## A burst that is never given back: no slow path, no heartbeat

2026-10-20, at main `2c61347`. In the after-the-peak probe (`workload quiet live=2M
sizes=small secs=6 after=burst after_sizes=small`, native build, CPU 2), asmalloc sometimes
holds all 215 MiB to the end instead of returning to about 20 MiB by 6 s. It is not a purge
that fails; it is the design's stated limit, "a process in which every thread stays inside its
magazines has no heartbeat at all", and the probe falls into it in most runs.

- **How often.** 39 of 66 runs over two days held everything; the share drifted from 1 in 6 to
  18 in 20 between sessions. With the opt-in background thread, 0 of 6 (20 MiB at 6 s).
  Preloaded (C `free`), 0 of 10.
- **Where the memory is.** In a run that holds it: 207 MiB free in the bitmaps and counted as
  retained, 1.5 MiB parked in the depot, 0.25 loaded (`asmalloc-memstat`). Not a large slot,
  not a frontier: a counting build (`bench/probes/app-faults/edits/decaycount.py`) shows the
  heartbeats stop for good after the same 15,755 or 17,573 calls, about a second in, having
  started three clocked passes. Runs that return memory make 110k to 125k heartbeats and
  start one pass a second.
- **Why.** The light work allocates 2,000 objects of twelve classes up to 256 bytes and frees
  them, every millisecond: about 167 objects a class against magazines of 254. Once each
  class's loaded magazine holds more than a burst's worth, every call is served on the fast
  path, for ever. The rules that would move on (a magazine's age limit, the CPU's sweep, the
  idle drain) all run from a slow path, of this thread or another, and there is no other. What
  tips a run into that state I did not pin down: not the phase of the clock's interval (runs
  that held everything began their first heartbeat at every phase), nor any `realloc` (the
  light work makes none). The sizes here are all small; a 64 KiB allocation would be a large
  call and do maintenance one call in sixteen.
- **Is "never" realistic?** For a single thread, or a thread per core, whose steady work
  allocates and frees fewer than a magazine's worth of each class per cycle, yes: a request
  handler that allocates tens of objects a class does. Work that frees on another thread, grows
  vectors, or touches large blocks enters slow paths all the time: the screening's programs
  made 36k (tpchgen) to 840k (gix-hours) heartbeats a run; pulldown-cmark, single-threaded,
  282.

**Enforcing the cap where memory is freed (experiment, not for merging).** A free or launder
that adds to the retained count takes a debt; at each growth step (2 MiB) of it, or at every
call (*eager*), the freer purges what is over the 64 MiB cap from where the walk stands,
within the growth payment's bound of 16,384 bitmap words a call. *Spare recent* skips regions
released into within 2^28 ticks (about 60 ms). Probe, runs with no heartbeat, MiB held at the
end: main 215; per step 147; eager 108; spare recent 215. Neither reaches the cap: the frees
come in random order, so a page is wholly free only at the last frees, after most payments have
walked past it, and the parked objects left in magazines and the depot (under 2 MiB of
objects) pin a page each, a whole 2 MiB unit in the huge-page part. Those are moved only by the
heartbeat's rules. Sparing recent memory spares all of a burst, since all of it is recent.

What it costs where large memory is freed and claimed again within milliseconds (preloaded,
three rounds, medians; diagnostic builds):

| | faults | kernel Gcycles | Gcycles | `madvise` | peak RSS MiB |
|---|---:|---:|---:|---:|---:|
| tpchgen: main / per step / eager / spare recent | 712k / 853k / 857k / 718k | 2.57 / 3.14 / 2.80 / 2.27 | 26.6 / 27.3 / 26.6 / 26.3 | 9.5k / 21k / 24k / 10k | 575 / 517 / 504 / 565 |
| gix-hours | 990k / 1,023k / 1,006k / 1,017k | 3.51 / 6.93 / 8.09 / 4.17 | 49.4 / 72.1 / 691 / 59.9 | 9.6k / 138k / 137k / 29k | 2,928 / 2,925 / 2,923 / 2,937 |
| datafusion | 245k / 436k / 439k / 291k | 2.15 / 3.05 / 3.10 / 2.38 | 23.1 / 24.4 / 24.1 / 25.7 | 3.5k / 9.1k / 8.9k / 3.8k | 438 / 327 / 339 / 394 |
| pulldown-cmark | level | | | | |

Per step costs gix-hours 46% more cycles and datafusion 78% more faults; eager makes gix-hours
fourteen times slower (17.7 s), walking its fragmented bitmaps on every launder. Sparing recent
memory removes most of it (gix-hours +21% cycles, datafusion +19% faults) and also removes the
benefit on the probe.

**A heartbeat from `realloc` moves.** Counted per run (C `realloc` that moves a block):
tpchgen 45k small and 6.6k large, gix-hours 40.7M small, datafusion 210k, ruff check 693k,
pulldown-cmark 2.6k; the probe 420, all before the burst. The programs that move blocks already
make heartbeats by the tens of thousands, and the probe's light work makes no moves after the
drop, so a heartbeat from moves would not have helped. Not built.

**Options.** (1) Keep the limit and say so in the probe and the design: a program whose
steady work fits its magazines keeps what it held when it went quiet unless it starts the
background thread; the probe should report how many runs had no heartbeat. (2) A heartbeat
that cannot be skipped: a slow path forced once in N fast-path calls costs the fast path, which
the design rules out; forcing it instead when a slow path installs a magazine (a magazine
installed with fewer than a burst's worth of room either way) only delays the state. (3) The
background thread on by default, which the design opted against. (4) The cap enforced on the
free side: bounded by the parked objects, not by the cap, and it thrashes where memory is
reused unless it spares recent memory, which spares a burst too.

## Reproducing

```
# The frontier: three builds from `git archive <commit>` into src-<name>, each built with
#   (cd src-<name>/bench && cargo build --release -p bench-workloads -p bench-rpc \
#      --features bench-alloc/asmalloc --target-dir $W/bench/target/asmalloc-<name>)
# and listed in allocators.toml as `build = "asmalloc-<name>"` entries; then, from bench/:
scripts/measure --both-l3 target/release/benchrun run --suite full --only "rpc " \
  --allocators main,frontier --rounds 5          # twice
bench/probes/memory/worst.sh $W asmalloc-main,asmalloc-frontier burst-idle phase
bench/probes/memory/memrun.sh $W asmalloc-main,asmalloc-frontier 2 period-under \
  period live=300k sizes=small cycles=4 gap_ms=2500 after=burst after_sizes=s64
BENCH_CPUS=2 scripts/memlimit -m 4G -- taskset -c 2 bench/target/asmalloc-<name>/release/workload fragchurn
strace -f -qq -c -e trace=madvise <workload command>   # on CPUs 12-15, under scripts/memlimit
# Small heaps: accounting builds, then the jobs interleaved under the lock, then the tables.
W=$(pwd)   # the checkout
cd bench
cargo build --release -p bench-workloads -p bench-rpc --target-dir target/asm-ms --features bench-alloc/asmalloc-memstat
cargo build --release -p bench-workloads -p bench-rpc --target-dir target/asm-acct --features bench-alloc/asmalloc,bench-alloc/memacct
cargo build --release -p bench-workloads -p bench-rpc --target-dir target/je-acct --features bench-alloc/je,bench-alloc/memacct
cargo build --release -p bench-workloads -p bench-rpc --target-dir target/sys-acct --features bench-alloc/memacct
cd ..
flock /tmp/asmalloc-measure.lock bench/probes/memory/acct.sh $W asm-acct,asm-ms,je-acct,sys-acct 5 \
  tiny churn1k pipe2 mpmc44 echo64 log64 log256
python3 -I bench/probes/memory/peaks.py bench/target/memory-out/acct asm-acct je-acct sys-acct
python3 -I bench/probes/memory/acct.py -v bench/target/memory-out/acct/pipe2.asm-ms.1
# Time on CPUs 0-7 (plain builds; acct.sh prints each run's primary metric):
flock /tmp/asmalloc-measure.lock bench/probes/memory/acct.sh $W before,after 5 \
  pipe2 pipe2free exch2 pipex2 mpmc31 mpmc44a churn100 churn1k churn100k churn1M > times.txt
python3 -I bench/probes/memory/times.py times.txt before before after

# Headroom: variants built with HEADROOM edited, as benchrun `build` entries in allocators.toml
# (bench/target/asmalloc-<name>); bytes copied by one repetition, then the timed jobs.
bench/target/asmalloc-h16/release/workload vecgrow vecs=1 max=4Mi reps=1 moves=1
bench/target/release/benchrun run --suite full,backend --rounds 5 --allocators h4,h16,glibc,jemalloc \
  --only "vecgrow 1x256Mi,vecgrow 64x4Mi,largepair,sizes=large objects=1M,grow-and-free"
cc -O2 -o grow_exhaust bench/probes/memory/grow_exhaust.c
prlimit --as=$((64 << 30)) env LD_PRELOAD=target/release/libasmalloc_preload.so ./grow_exhaust 262144
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
# Faults and system calls in real programs: the screening's built tree, and a scratch directory.
export RUST_SCREEN_ROOT=<the screening's target/rust-screen> APPF_OUT=/tmp/app-faults
P=bench/probes/app-faults
scripts/heavy $P/variant.sh main-d --diag $P/edits/oldcommit.py $P/edits/diag2.py   # counts by call site
scripts/heavy $P/variant.sh fix                                                     # the branch
scripts/heavy $P/variant.sh keep $P/edits/keepsmall.py    # likewise idle (idlegrow.py), rw (rw.py), ...
scripts/measure --both-l3 scripts/memlimit -m 8G -- python3 -I $P/bench.py $APPF_OUT/runs.jsonl \
  --rounds 5 --progs tpchgen,gix-hours,datafusion,pulldown-cmark,ruff-check \
  --libs main=$APPF_OUT/lib/main-d.so,fix=$APPF_OUT/lib/fix.so,glibc=-
python3 -I $P/summ.py $APPF_OUT/runs.jsonl
scripts/measure --both-l3 $P/csmix.sh ruff-check 8 main=$APPF_OUT/lib/main-d.so fix=$APPF_OUT/lib/fix.so
python3 -I $P/csall.py main fix        # context switches by reason
scripts/measure --both-l3 scripts/memlimit -m 8G -- python3 -I $P/srv.py $APPF_OUT/srv.jsonl \
  --rounds 5 --libs main=$APPF_OUT/lib/main-d.so,fix=$APPF_OUT/lib/fix.so
# A burst that is never given back: counting builds (heartbeats, decay steps, realloc moves),
# preload and native (natbuild.sh: variant.sh, then the workloads against the variant's crates).
P=bench/probes/app-faults; E=$P/edits
scripts/heavy $P/variant.sh x-base --diag $E/decaycount.py
scripts/heavy $P/variant.sh x-fc --diag $E/decaycount.py $E/freecap.py    # + freeeager.py, freeold.py
BENCH_CPUS=2 bench/target/<label>/release/workload quiet live=2M sizes=small secs=6 \
  after=burst after_sizes=small            # the DIAG counts go to stderr at exit
```
