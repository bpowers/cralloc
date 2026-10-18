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
  which is the 128 KiB magazine budget.

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
dense; moving large blocks by remapping is planned for that.

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

From the constants, checked against the runs above where a run exists.

| What | Bound | Notes |
|---|---|---|
| One CPU's loaded magazines, all 44 classes full | 2.89 MiB | measured 2.9 |
| One CPU's full spares | 0.14 MiB | the 1 MiB budget is split 44 ways, so only classes up to 80 bytes get any |
| Magazine memory per CPU | 0.28 MiB | never unmapped; a magazine of 16-byte objects is 2 KiB for 4 KiB of objects |
| All CPUs, every class touched | 97 MiB at 32 CPUs, 775 MiB at 256 | reaches the depot after a second idle, and stays there |
| Depot | no bound | 256 KiB a class (11 MiB) is a target only for classes being served |
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

## Reproducing

```
W=$(pwd)   # the checkout
bench/probes/memory/build.sh $W/bench asmalloc je mi base-ms=asmalloc-memstat
bench/probes/memory/peaks.sh $W asmalloc,je,mi pipe2 pipemix churn1M vecgrow json
bench/probes/memory/worst.sh $W asmalloc,je,mi
# The prototype builds (`lean`, `age-thpoff`) need the review branch and its x-* features.
bench/probes/memory/memrun.sh $W base-ms 0,1,2,3,8,9,10,11 phase touch threads=8 secs=8 sizes=s64
python3 bench/probes/memory/summ.py -c -s bench/target/memory-out/phase.base-ms.memstat
```
