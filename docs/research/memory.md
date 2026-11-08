# Where memory goes

What sets a process's resident memory against what the program holds, what each of cralloc's rules
buys and costs, and where it stands against jemalloc, tcmalloc, mimalloc and glibc. Blocks past the
small classes have their own note, [large-blocks.md](large-blocks.md). How placement changes the
program's speed is in [program-effects.md](program-effects.md). The rules are in `docs/design.md`,
"Returning memory and huge pages", and the jobs in [workloads.md](workloads.md).

Measured on one machine (Ryzen 9 9950X, Linux 7.2.9, transparent huge pages `madvise`), 2026-10-17
to 28. *Resident over live* is anonymous resident memory, less the workload's own bookkeeping, over
the bytes the program holds at the sizes it asked for.

## The answer

- **On a steady large heap cralloc holds the least, and it does not drift.** A 2 GiB cache churned
  for 15 billion operations ends at 1.105 times live, flat from the first minute, the least at 2 GiB
  and up but for glibc by 0.002 at 5 GiB. The order service holds the least at every size, 1.073
  against jemalloc's 1.095 at 10 GiB. Across ten servers at load cralloc is level with jemalloc,
  which is lower on most of the small ones. A 10.6-hour Qdrant soak beside jemalloc drifted in
  neither, with cralloc 13 to 31 MiB lower in the median of each five cycles.
- **Most of what is left on a steady heap is class rounding.** Of the 10.5 points over live on the
  steady cache, 7.8 are slots larger than the request, which jemalloc pays too. cralloc's lead at a
  peak is metadata (4.6 MiB against 38 at 2 GiB) and free slots inside slabs (15 MiB against 45).
- **Its loss is troughs, to jemalloc alone, and the cause is reuse order.** When short-lived objects
  fall away while long-lived ones keep being made, each allocation takes the slot the last free
  pushed, so the survivors stay spread over the peak's footprint. At a 2 GiB trough cralloc holds
  0.95 GiB and jemalloc 0.66, on the same live slots: 273 MiB of free slots beside live objects
  against 40, 88% of it in classes of 128 bytes and down. tcmalloc, mimalloc and glibc do not pack
  at all there and hold more than 2.1 GiB.
- **When the size mix wanders, huge pages ratcheted.** A class whose demand fell kept every 2 MiB
  unit that still held one object: 1.10 climbed to 1.21 times live, then held. The tail rule returns
  that memory (1.14 at the peaks) for 1 to 1.5% of cycles on an allocation-bound job and nothing
  measurable on servers. After a large shift in sizes cralloc still holds more than jemalloc, 1.36
  against 1.19.
- **Servers engage these rules unevenly.** The order service's free memory stays under the 32 MiB
  floor at every size to 10 GiB, so no pass, hold look or growth payment runs while it serves, and
  the log store's tail rule acts only after close. Meilisearch and Qdrant do engage them. Large
  blocks engage a rule on the request's own path: about 40,500 trims a run on the order service, the
  slowest 2 to 5.6 ms, which its percentiles do not show.
- **Keeping memory on purpose has a visible price after a peak.** The floor leaves 33 MiB where
  jemalloc reaches 4.5, and a one-thread program that stays inside its magazines keeps all of a
  burst, as glibc and tcmalloc do and jemalloc and mimalloc do not. Both are chosen limits.

Resident over live, lower is better. Bold is the least.

| Job | cralloc | jemalloc | tcmalloc | mimalloc | glibc |
|---|---:|---:|---:|---:|---:|
| Steady cache, 2 GiB | **1.105** | 1.148 | 1.127 | 2.97 | 1.27 |
| Order service, 10 GiB, 70% load | **1.073** | 1.095 | 1.095 | 1.149 | not run |
| Wandering mix, peaks | 1.14 | 1.15 | 1.23 | 1.29 | **1.075** |
| Wandering mix, troughs (long-lived set turns over slowly) | **3.27** | 3.79 | 4.59 | 4.83 | 4.03 |
| Lifetime job, 2 GiB troughs (turns over 12 times a phase) | 1.77 | **1.24** | 4.19 | 4.27 | 4.01 |
| Size shift, median 384 bytes | 1.36 | **1.19** | 2.05 | 2.59 | 1.83 |
| mini-redis, wide values | 1.15 to 1.18 | 1.16 | 1.23 | 1.21 | 1.25 |
| Large-buffer cache, geomean of four phases, 0.5 to 5 GiB | 1.19 to 1.82 | 1.19 to 1.52 | 1.39 to 2.44 | 1.61 to 2.91 | 1.72 to 2.10 |

*2026-10-24 to 28, one run a cell for the long jobs; cralloc at its latest measurement of each (from
`9a11f70` for the steady cache to `b622714` for the 2 GiB troughs), the incumbents' code unchanged
over those days. Real servers at 70% load, resident peak, ten jobs at `e3753b5`, others against
cralloc, geometric mean (geometric standard deviation): jemalloc 1.01 (x1.17), tcmalloc 1.11
(x1.21), mimalloc 1.49 (x1.38), glibc 1.06 (x1.58). Computed from the standings' records, median of
5 to 15 runs a job.*

Terms. A *slow path* is any call the magazines cannot serve. A *heartbeat* is the bounded round of
maintenance slow paths run. To *launder* a magazine is to return its objects to the bitmaps.
*Retained* memory is the back end's count of free memory it believes resident. The *frontier* is the
highest slot a region's claims reached in the last two intervals; an *interval* is 2^32
cycle-counter ticks, about a second here. The *floor* is 32 MiB of returnable memory the clock
leaves alone. The *cap* is 64 MiB of retained memory, over which growth pays. The *log store* is a
Connect server holding 1M records.

| Rule | Buys | Costs |
|---|---|---|
| Purge on a clock, above the frontiers | A 218 MiB burst down to the floor in 8 s, where it once stayed for good; the RPC jobs' faults under load down three quarters | A burst waits a second longer; 0.3 cycles a claim |
| Growth over the cap pays, frees never do | Large pipeline 319 to 119 ns an allocation, 298,000 `madvise` calls to 53 | HashMap's peak +29 MiB |
| Parked objects age out | Burst at 12 s, before the floor: 73 to 4.3 MiB | A refill per class per interval |
| Refresh one retiring magazine in 32, up to one in 4 on evidence | 2 GiB trough 1.10 to 0.95 GiB | +0.6% of cycles where it acts, nothing elsewhere |
| Tail rule | Wandering peaks 1.23 to 1.14, troughs 4.29 to 3.27 | 1 to 1.5% on the job; huge pages at peaks 1.7 to about 0.7 GiB |
| A 32 MiB floor | Fewer `madvise` calls and refaults: with the rule that a silence ages no frontier, the log store's first 5 ms of a burst 370 to 25 faults, its remaining cost the idle drains | 33 MiB kept after a peak |
| Magazines of 64 KiB, not 128 | Mixed pipeline's peak 7.07 to 5.12 MiB | +5.1% on that job, nothing resolved elsewhere |
| Background thread, on by default | Memory returns in a silence; requests take no clocked or hold steps, and growth steps only when growth outruns it | 35 us a wake-up; 9.8 s of CPU in 10.6 hours of Qdrant |

## What resident memory is made of

The lifetime job at 2 GiB (defined below), MiB, means of three peaks and three troughs, every thread
stopped at each phase end and every resident page attributed.

| | Peak: cralloc | Peak: jemalloc | Trough: cralloc | Trough: jemalloc |
|---|---:|---:|---:|---:|
| Program's live bytes | 2,028 | 2,028 | 546 | 546 |
| Class rounding | 159 | 170 | 44 | 47 |
| Free slots beside live objects | 15 | 45 | 273 | 40 |
| Parked in caches | 8 | 1 | 8 | 1 |
| Free pages not yet returned | 10 | 4 | 44 | 7 |
| Past a region's watermark | 18 | | 13 | |
| Metadata | 4.6 | 38 | 4.6 | 38 |
| Heap, with up to 5 MiB unaccounted | 2,244 | 2,280 | 935 | 677 |

*2026-10-27 and 28: cralloc at `b622714` with the refresh rule, jemalloc built with statistics.*

- **Rounding is the floor of any allocator with size classes.** glibc has none and the lowest peaks
  on this job (1.075), though it pays elsewhere.
- **The steady cache at 2 GiB has the same shape:** 2,048 live, 160 rounding, 13 to 15 free beside
  live, 7 to 8 parked, about 20 past watermarks.
- **At a peak, pages are full and every allocator is close.** The gap opens when demand falls, and
  it is free slots beside live objects: nothing can return those, so they are placement, not policy.

## A steady heap does not drift

**The steady cache:** eight threads share a cache; each operation puts a value (log-normal, median
128 B, sigma 1) in a random slot and evicts at random while over budget, so most frees are another
thread's object, and makes two short-lived objects. Ratio's slope over the second half of 15 billion
operations, per billion: cralloc +0.00003, tcmalloc +0.00006, jemalloc -0.0006, glibc -0.012,
mimalloc +0.021 (least squares over 75 samples, computed from the runs). Memory is not bought with
time: 1,271 cycles an operation against tcmalloc's 1,326, jemalloc's 1,398, glibc's 2,051 and
mimalloc's 2,064; on the lifetime job below 595 against 619 to 888; on the size shift level with
tcmalloc (2026-10-24 at `9a11f70`, one memory run each, one or two timed rounds).

**Threads that come and go.** The same cache filled by producers and drained by consumers, busy
threads 8, 2, 8, 1, 8, 4 by phase: cralloc settles at 1.22 (1.21 to 1.27 over the last half).
jemalloc swings between 1.24 and 2.5, tcmalloc is at 1.38 and still creeping, mimalloc 1.99, glibc
2.35. Its cycles count consumers spinning, and move 17% between rounds.

**Qdrant for 10.6 hours** (2026-10-27 at `967317c`, jemalloc beside it on the same schedule; 35
cycles of searches, upserts, deletes and a collection built and dropped, each ending in 1 to 20
minutes of quiet). Anonymous memory at the quiet points rose over the first fifteen cycles in both
and then held: cralloc median 156 MiB (140 to 158), jemalloc 177 (175 to 187), both under 1 MiB an
hour over the last 5.9 hours (computed from the quiet points). Mappings stayed bounded, about 770
against 550. The allocator's count stayed between 0.6 MiB under and 6.8 over what `mincore` sees.
Huge pages did not drain. The thread took 0.07% of a CPU when Qdrant's own threads were active and
under 0.01% when it was idle. Minor faults were 2.7 million against 4.2; major faults 1.7 million
against 1.2, Qdrant rereading its files. A mostly read workload on shared cores, so its latencies
mean nothing, and it cannot show a heap that grows for days or weeks of churn. The RPC soak was not
run.

## The heap's fixed numbers hold to 10 GiB

Nothing gets worse faster than the heap (2026-10-27 at `536dbe8`; three rounds of the order service,
medians; one run of the drift jobs). The order service stores an order of about 1.9 KiB and drops
the oldest on each request.

| Live | Order service: cycles a request, p99.9 | Over live: cralloc | jemalloc | tcmalloc | mimalloc | Steady cache: cralloc | jemalloc |
|---|---:|---:|---:|---:|---:|---:|---:|
| 0.5 GiB | 68.0k, 24 us | 1.099 | 1.106 | 1.114 | 1.199 | 1.149 | 1.174 |
| 2 GiB | 68.2k, 24 us | 1.080 | 1.097 | 1.098 | 1.156 | 1.104 | 1.135 |
| 4 or 5 GiB | 68.2k, 24 us | 1.076 | 1.095 | 1.096 | 1.151 | 1.091 | 1.117 |
| 10 GiB | 68.4k, 24 us | 1.073 | 1.095 | 1.095 | 1.149 | 1.086 | 1.113 |

On the steady cache tcmalloc (1.146) and glibc (1.132) are under cralloc at 0.5 GiB, and glibc
(1.089) at 5. On the order service no pass, hold look or growth payment runs while it serves, and
the background thread starts at shutdown. On the steady cache fragmentation holds the count over the
floor while what a pass could return stays under it, so each interval costs one lap that only
counts: longest pass 12.6 ms, 0.5 s of CPU in two minutes at 10 GiB.

What does grow with the heap, and stays bounded:

- **A pass's length:** longest 0.24 s at 2 GiB, 0.78 s at 10, against the one-second interval. The
  kernel returns about 1.5 GiB a second of the thread's time. Nothing is lost: a trough comes within
  a tenth of its end in 13 to 17 s at both sizes (jemalloc 13 to 22 s).
- **A step on a request's path,** without the thread: longest 0.38 ms at 2 GiB, 1.5 ms at 10; p99.9
  30 and 61 us. The growth is the kernel's split, with eight CPUs to shoot down.
- **`mprotect` commits** are linear past 64 MiB a region: 284 calls at 0.5 GiB, 627 at 10, a few of
  3 to 7 ms in large fills. Removing the cap on how far a commit runs ahead would save about 280.
  Not built.
- **Page tables on the large-buffer job:** 1.5 to 2% of the heap, three times the incumbents', and
  2,196 mappings against under 460 at 5 GiB, following the number of grown blocks (inferred from the
  counts).

| Number | Value | Verdict |
|---|---:|---|
| Floor | 32 MiB | Holds. 0.3% of a 10 GiB heap; scaling it keeps more of a trough for nothing measured |
| Retained cap | 64 MiB | Holds. Of these jobs only the large-buffer one is over it, half its growth steps at 0.5 GiB and at 5. Eight times: faults +8%, cycles +4%, peak the same |
| Kept outgrown slots | 64 MiB | Holds. Eight times: faults -7%, cycles +1%, peak the same |
| Unpaid growth, the thread's round | 16 MiB, 500 ms | Hold. Paid steps under 0.85 ms; the longest thread step 5.4 ms at 10 GiB |
| Magazines, spares, depot | per CPU and class | Hold. Parked memory is a smaller share of a larger heap |

## When the size mix wanders, huge pages ratcheted

**The lifetime job:** each of eight threads keeps a FIFO of short-lived values whose total
alternates between a high and a low level, phase by phase, and a share of its allocations go to a
long-lived cache under random replacement. A *peak* is the end of a high phase, a *trough* the end
of a low one. In the *wandering mix* each phase draws its own table of 4,096 sizes, so a class
taking one draw in a hundred moves by 20 to 30% between phases; the total is 1.9 GiB at peaks and
0.5 in troughs.

**The ratchet.** Peaks climbed from 1.10 to about 1.21 over 24 billion operations and held between
1.19 and 1.23 for 48 billion more, about 6 points over jemalloc. With one size table for all phases
they stayed flat at 1.107.

- **Where the memory was:** the 212 MiB of growth at a late peak was all free memory no purge could
  return, 238 MiB in all, 176 of it whole empty 4 KiB pages inside 2 MiB units that held something.
- **Each class kept resident about 1.1 times the most it was ever asked for.** A 768-byte class
  asked for 78 to 130 MiB by phase and kept 145 for good. Classes above 1 KiB, on 4 KiB pages, did
  not ratchet.
- **The holes were not refilled.** A class carves only after its bitmaps have no free slot, the
  holes were in classes whose demand had fallen, and a region holds one class.
- **It was one tail.** At least 99% of the stranded pages lay at or above the region's frontier
  rounded up: dense units below, units at most a quarter full above.
- **glibc and jemalloc did not ratchet.** glibc merges freed chunks, which then serve any size.
  jemalloc's slabs are a few pages, and empty ones decay in 4 KiB pages. tcmalloc and mimalloc did,
  to 1.23 and 1.29.

The same mechanism held the old classes' tails after a size shift, and mini-redis's small-value
classes when its keys were rewritten at changing sizes.

### Returning part of a huge page frees nothing until it is split

Read against the kernel at `v7.2` and measured in cgroups of their own (2026-10-25).

- `MADV_DONTNEED` on part of a huge page takes the pages out of RSS at once, but not out of the
  cgroup's charge: the folio waits on the deferred-split queue. 64 units with all but 3 pages each
  returned: RSS 128 to 0.8 MiB, `memory.current` 129 to 129, still 128.9 a minute later.
- Reclaim frees it, and a thread pays: faulting into `memory.max` with 256 MiB stranded, the faults
  that met the limit stalled 40 to 49 ms. mimalloc's charge on mini-redis runs 10 to 15% over its
  resident set from this.
- `MADV_COLD` on part of the unit splits it first, and the return is then real (without it the
  troughs read 3.14 by RSS and 3.93 by the cgroup). The split costs 20 to 80 us a unit, up to 8 ms
  on busy CPUs.
- Under the advice khugepaged rebuilds any unit with one page present, within minutes. So advice
  must not stand over units that stay sparse.
- A unit returned whole faults a huge page at once, since its empty page table is freed. One
  returned in part faults 4 KiB pages until khugepaged collapses it.
- Advice changes take `mmap_lock` for writing: 2 to 8 us at p50, up to 23 ms beside 2 MiB faults.

### The tail rule, and what it cost

Above a region's frontier, a step of the clocked pass that may return memory treats huge pages as
small pages. One wholly free goes back whole. One at least a quarter free and resident is split and
its free runs returned; a sample of 8 pages, moving with each lap, picks which are counted. The
advice comes off, once per region per pass, over what went back, and a claim above that boundary has
the thread put it back up to the frontier plus 16 MiB.

| | Before | Rule | jemalloc | glibc |
|---|---:|---:|---:|---:|
| Wandering mix: peaks, last three | 1.226 | 1.138 | 1.150 | 1.075 |
| Wandering mix: troughs | 4.29 | 3.27 | 3.79 | 4.03 |
| Huge pages at peaks, MiB | 1,650 to 1,730 | about 650 | none | none |
| Size shift, median 384 B | 1.51 | 1.36 to 1.37 | 1.19 | 1.33 to 1.83 |
| mini-redis wide values | 1.33 | 1.15 to 1.18 | 1.16 | 1.25 to 1.29 |
| mini-redis small values | 2.3 to 3.0 | 1.8 to 2.3 | 2.4 to 2.7 | 4.7 to 5.3 |

*2026-10-25 to 27, one run each; rule at `7bf9ff8` for the wandering mix, `c6a7328` and earlier for
the others. jemalloc's and glibc's ranges are between sessions and cycles.* On Qdrant (`c09828f`,
three rounds, medians) heap mean 449 to 414 MiB, peak 542 to 515, a minute after close 185 to 155,
tails within their spread.

- **Time.** 1.0 to 1.5% of cycles on the lifetime job with four 18 s phases, inside one binary with
  plentiful huge pages (4 KiB walks +15 to 50%, 2 MiB walks -35 to -60%). Shipped builds read 601 to
  609 cycles an operation across sessions against 593 to 595, which includes placement. Nothing
  measurable with 3 s swings, with scarce huge pages, or on servers.
- **Serving, the log store, the order service and Meilisearch do not trigger it.** The store split
  85 huge pages and lowered once, all after the load closed. Qdrant, whose optimizers swing, raises
  and lowers 12 to 16 times in 30 s at 70% load.
- **System calls.** About 265,000 `madvise` and 10,000 `mincore` on that job, on the thread, under
  1% of a CPU. Without the thread a split on a slow path averages 40 to 60 us, at worst 1.7 ms, and
  counts as a whole step.
- **Huge pages come back slowly,** in minutes, through khugepaged. `MADV_COLLAPSE` costs about 6 ms
  a unit, and telling dense units from sparse ones cheaply is not solved.

Tried and rejected:

- **Lowering the boundary whenever the frontier fell.** On the log store, whose small classes wobble
  by a few MiB, 50 advice changes a run returned nothing, and p99.9 went from 0.1 to 0.2 to 0.4 ms.
  A swing under about 128 MiB now changes no advice.
- **Reading every unit above the frontier page by page.** The thread woken by a burst spent 1.7 ms
  on a server CPU in its first 10 ms (0.46 before), four times the walk, and the slowest request
  took 1.2 to 2.7 ms in 6 of 8 runs (0.4 to 0.8). The sampled look brought it to 0.5 to 0.9.
- **Sampling words or aligned runs.** Both missed free pages: troughs 3.81 and 3.53 against 3.25 for
  sampling pages.
- **Patience.** Waiting N intervals returns nothing from troughs shorter than about N to 2N
  intervals and most of longer ones. It did nothing on Qdrant at 30 and 120.
- **Counting pages resident when the advice goes back.** The count ran 78 to 89 MiB over `mincore`,
  enough to put a heap over the cap. The walk distrusts clean bits under the advice instead, and
  asks `mincore` before a split.
- **Sizing the raise's headroom from evidence.** Two such rules did no better than the constant.
  None at all moved a growing class's boundary about ten times an interval (716 raises a run; 16 MiB
  145; 64 MiB 97).

The region's tail word is modelled in `tla/Tail.tla`, which found two defects, both fixed: an owner
that cleared a repair request a refused call had left, and a fork child inheriting a word whose
owner's call never finished.

## In a trough, placement decides, and reuse order sets placement

**A hypothesis: a trough's footprint is where the long-lived objects were born,** so it falls with
how much of the long-lived set turns over while the short-lived one is gone. That would reconcile
two studies that look contradictory. These runs fit it and do not test it. Turnover was never varied
alone: the last four rows fix operations a phase, so turnover and heap size are one variable; the
first row is another job (a size table per phase, one long-lived value in 1,000 against 20); and the
series is not monotonic (12 to 47 rises for both allocators). A controlled run is under Open
questions.

| Long-lived set replaced, times a phase | cralloc | jemalloc | tcmalloc | mimalloc | glibc |
|---|---:|---:|---:|---:|---:|
| 0.5 (wandering mix, 2 GiB) | **3.27** | 3.79 | 4.59 | 4.83 | 4.03 |
| 2.4 (10 GiB) | 3.40 | **3.19** | 4.14 | 4.17 | 3.96 |
| 4.7 (5 GiB) | 2.73 | **2.28** | 4.16 | 4.20 | 3.97 |
| 12 (2 GiB) | 1.77 | **1.24** | 4.19 | 4.27 | 4.01 |
| 47 (0.5 GiB) | 1.94 | **1.34** | 4.29 | 4.66 | 4.09 |

*Trough over live, median of the last three, one run each. Turnover computed from each run's
arguments (share of allocations long-lived, operations a phase, mean value 211 B, size of the
long-lived set). Only the 12-times row has the refresh rule below; cralloc's others are `7bf9ff8`,
`536dbe8` and `1d73192`.*

In these runs cralloc holds the least where little is replaced, with the tail rule (4.29 without
it), and more than jemalloc where much is, because of where the new ones go. Why tcmalloc, mimalloc
and glibc do not fall was not studied.

**The gap is placement.** At the 2 GiB trough both hold the same slots (590 MiB with rounding,
jemalloc 593). cralloc's sit on 870 MiB of pages, jemalloc's on 632 MiB of slabs. What the huge-page
rules keep is under 100 MiB, against jemalloc's 33 MiB more metadata; parked objects are 8.

**Why cralloc places objects where the last ones died.** After the fall each class's frees and
allocations balance on each CPU, and the loaded magazine does the work: a free pushes a dying
value's slot, the next allocation pops it. The level random-walks and reaches an end, the only time
anything is exchanged, laundered or filled, about once in the square of the magazine's size in
operations of that class. Magazines hold up to 1,022 objects for classes to 64 B, 510 for 128 B, 62
for 640 B. So the trough's live objects are a sample of the peak's slots, closer to uniform the
smaller the class.

**Why jemalloc packs.** It has the same LIFO cache per bin, but bounded at twice a slab's regions,
clamped to 20 to 200, and every 64 KiB a thread allocates it visits one bin and flushes three
quarters of what stayed unused. A bin's level reaches its ends often, and fills come from the
oldest, lowest non-full slab. Its placement is lowest-first most of the time; cralloc's only at a
fill. Its bins are 85 to 96% full at the trough.

Slots filled on their pages, by class, at the first 2 GiB trough with the refresh rule: 16 to 64 B
27%, 80 to 128 B 28%, 160 to 256 B 82%, 320 to 512 B 98%, above that 99%.

### Refresh on evidence

Laundering a retiring full magazine replaces circulating objects with the lowest free ones. It is
the one thing that reached this, and it reached down to 160 B.

| Tried at 2 GiB | Heap, MiB | Free beside live | Cycles |
|---|---:|---:|---|
| One retiring magazine in 32 | 1,099 | 408 | |
| One in 4 | 934 | 271 | +1.1 to 1.8%; +3.3% on 1.4 s phases, faults +80% |
| Every one | 926 | 249 | +10% |
| Magazines of at most 256 slots | 1,032 | 322 | +2.4% |
| No huge pages at all | 1,064 | 408 | +2 to 4%, faults 1.4 to 2.5 times |
| Split from an eighth free, not a quarter | 1,087 | 407 | not timed |

*One accounted run each, means of three troughs, at `1d73192`; timed separately, one to three
rounds.*

A fixed one in 4 costs about 1% wherever it runs, so the rate follows evidence. Each lap of the
purge walk takes the small classes' free slots beside live objects as a share of what they carved.
Under 1 in 64 the rate stays one in 32; it doubles at 1 in 64, 1 in 32 and 1 in 16, up at once and
down a step a lap. The share is 0.6% on the steady cache and at the peaks, 18% at the troughs and
12% once the rule has packed them, which keeps the most.

| In one binary, two rounds | Off | Rule | One in 4 always |
|---|---:|---:|---:|
| 2 GiB, 18 s phases: trough, GiB | 1.09, 1.13 | 0.95, 0.95 | 0.94, 0.94 |
| The same: cycles an operation | 638, 640 | 643, 643 | 646, 649 |
| 1.4 s phases: cycles | 608, 608 | 612, 613 | 615, 615 |

*2026-10-28 at `b622714`, a run-time switch.* On the steady cache the rule stays off (1,272 cycles
an operation in both arms' means), and the order service never walks a lap while it serves (67.4k to
67.9k cycles a request in both).

**What would pack the smallest classes.** Their magazines rarely retire, so retirement rules do not
reach them. Not built: a fill-side rule that launders a full spare when the class is shrinking on
this CPU; an age limit in exchanges as well as time. Smaller magazines for these classes undo
measured speed ([magazines.md](magazines.md)) and took only 67 MiB.

**Do real servers show the pattern?** It needs a long-lived population allocated during a fall in
the short-lived one, in the same classes. Qdrant's optimizers do this beside long-lived payload
objects (a minute after close 139 MiB, jemalloc 131 in another session; not attributed). mini-redis
is level with jemalloc, and the order service has no trough. So the synthetic gap is larger than any
measured server's, though the mechanism is general.

## How much a clocked pass returns

Everything the last lap found returnable, down to the floor. The frontiers already keep it to what
two intervals did not reuse, so holding a share back only keeps memory the program has shown it does
not need.

| | All of it | A quarter |
|---|---:|---:|
| 2 GiB, 18 s phases: trough, GiB | 1.13 | 1.53 |
| The same: cycles an operation, faults per 1,000 operations | 641, 0.118 | 640, 0.085 |
| 2 GiB, 4.4 s phases: trough, GiB | 1.85 | 2.04 |
| The same: cycles, faults | 639, 0.203 | 634, 0.126 |

*2026-10-28, one timed run each.* A quarter of a falling count does not keep up with what a phase
frees. It cuts faults by a third, and those are refaults at 0.1 to 0.2 per 1,000 operations, tens of
microseconds a second. On a small heap two passes in three found no more than the floor returnable,
so nothing separates them.

## Purge on a clock, never on a level

A free cannot tell idle memory from memory about to be reused. Everything here follows from that.

**The count is never reset.** An early version set the retained count to zero when a pass purged
nothing, taking what was left for fragmentation. During a storm of frees in random order that is
true of each pass, the pages become wholly free a moment later, and their bytes are then in no
count: 136 MiB free and resident under a 64 MiB cap.

**Growth pays, frees do not.** While the count is over the cap, carving is matched by an equal
purge, from wherever the walk stands.

| | Purge at the free | Growth pays |
|---|---:|---:|
| Pipeline, 4 to 256 KiB objects: ns an allocation | 319 | 119 |
| The same: kernel share, faults, `madvise` | 54%, 241,000, 298,000 | 0.2%, 676, 53 |
| HashMap 1M: cycles an entry, faults, peak MiB (jemalloc 184) | 671, 190,000, 175 | 579, 60,700, 204 |

*2026-10-18 at `3acaa1d` and after, five rounds interleaved, medians.* HashMap's price is the tables
it outgrew, under the cap. Charging refaults as growth made the allocator chase its own tail: 24,500
`madvise` calls on a `HashMap` rebuilt five times, against 43. A cap of 16 MiB returned a burst
sooner and left the peaks where they were.

**Only what fills did not reach.** Passes that started at the top spent their goal re-purging the
pages a quiet program revived in the higher regions, and left the lowest region's resident
(simulated, 147 KiB over what `collect` leaves after 40 intervals). Passes that carried on round the
heap reached those, and took turns in a busy server's lowest regions: 2 to 4% of cycles on the RPC
jobs, 12% more faults. The frontier fixes both.

- The RPC jobs' faults in the measured window fall by three quarters (1,050 to 250, 2,250 to 480,
  260 to 54), cycles level. Part of that was given back when a fill that meets another stopped
  recording its leap past the other's words (rpc log's 268 to 363), which took a refilling heap from
  129 to 28 MiB.
- Fragmentation over the cap stops costing the working set: beside a fragmented gigabyte, 38,330
  `madvise` calls and 36k to 46k faults become 552 and 4,730.
- A working set that comes and goes every 1.5 s under the cap: faults a re-allocation 2,450 to 41.
  With gaps of 2.5 s, 3,820 to 2,140; over the cap, 35,180 to 31,730.
- Price: a burst that goes quiet keeps its memory one interval longer (215 MiB at 1 s against 102,
  level from 2 s), and a fill pays about 0.3 cycles a claim.

**Rejected: enforcing the cap at the free,** tried again as a cure for a quiet burst. gix-hours +46%
cycles and, called on every launder, fourteen times slower; DataFusion 78% more faults. Sparing
recent memory removes most of that and all of the benefit, since all of a burst is recent.

## Parked objects pin the heap

A parked object holds its page as surely as a live one, and parked objects are a random sample of
wherever the heap has been. Half a megabyte of them pinned 50 MiB; one stack of 234 objects of 256
bytes pinned 31 MiB of huge pages. So a magazine stays loaded for at most an interval, full spares
are swept, a depot stack sheds what it did not need, and retiring magazines are refreshed. A 218 MiB
burst at 4 s and 12 s: 73 and 73 MiB without these, 39 and 4.3 with them (2026-10-17).

**The refresh's base rate.** Instructions on mixed churn and a small pipeline, against no rule at
all, median of eight interleaved runs, then MiB left 12 s after the burst: no refresh +0.7%, +1.9%,
7.1; one in 32 +5.4%, +6.3%, 3.3; one in 8 +17%, +21%, 3.9. Most of the cost is the fill that
replaces a laundered magazine. The finished rules cost churn +2.3% of cycles and nothing with 1k
live. A launder of 254 scattered objects takes 0.9 us on average, 5.5 at worst. A drain from
`maintain` launders what it takes: parked in the depot, a simulated quiet heap stays at 73% of its
memory, because the slab pops its magazines straight back.

**Worst cases by design.** Anonymous MiB, eight seconds after each event (2026-10-17 to 19, latest
figures, one run each). Here per-CPU caches win: jemalloc's are collected only by their own thread's
activity, so idle threads keep theirs. Read from their sources, tcmalloc's per-CPU caches shrink
only on a thread the program must start, and mimalloc purges only on every 10,000th slow path, after
4 s. The last two rows are large blocks, which get no huge pages when allocated whole.

| Case | cralloc | jemalloc | mimalloc |
|---|---:|---:|---:|
| Eight CPUs use every class, then one uses one | 7 to 9 | 24.7 | 15.8 |
| Eight CPUs, four classes | 1.1 | 4.3 | 8.3 |
| Two CPUs, 44 classes | 1.2 | 4.1 | 6.2 |
| 16 threads on two CPUs, 44 classes | 1.5 | 52.2 | 28.3 |
| 256 blocks of 1 MiB, one page written each | 2.3 | 2.9 | 276 |
| 4,096 blocks of 64 KiB, one page written each | 32.2 | 33.2 | 344 |

Their peaks show the magazine budget at the moment of the change (cgroup, MiB, 2026-10-18 at
`39c2f54`, cralloc, jemalloc, mimalloc): eight CPUs using every class 52.7, 29.0, 18.6; two CPUs
with 44 classes 15.4, 7.8, 6.9; but 16 threads on two CPUs 13.0, 52.4, 31.4.

**What is bounded, by arithmetic from today's constants.** Parked memory depends on CPUs and classes
in use, not on the heap.

| What | Bound |
|---|---|
| One CPU's loaded magazines, all 44 small classes full | 1.95 MiB of objects |
| One CPU's spares and stock; the depot on one domain | 1 MiB; 128 KiB a class, 5.5 MiB |
| All CPUs, every class touched | about 94 MiB at 32 CPUs, 750 at 256 |
| Pages pinned by one full magazine of 1,022 objects | 4 MiB, one object a page; 2 GiB in a huge-page part |
| Free and resident | what a pass can return: the floor while idle; over the cap, carving is matched by a purge. Free slots beside live objects: no bound |

## After a peak, and in a silence

Free 218 MiB of small objects in random order, then work lightly on one thread. Anonymous MiB
(2026-10-22 at `c7f5c21`, median of three):

| After freeing | 1.5 s | 4.5 s | 8.5 s | 12 s |
|---|---:|---:|---:|---:|
| cralloc, bursts crossing magazines | 215 | 93 | 33 | 33 |
| jemalloc | 142 | 113 | 4.5 | 4.5 |
| mimalloc | 226 | 226 | 15 | 16 |
| glibc, tcmalloc | 209, 211 | | | 209, 211 |
| cralloc, bursts inside its magazines | 215 | 215 | 215 | 215 |
| jemalloc, the same | 183 | 87 | 4.6 | 4.6 |
| mimalloc, the same | 226 | 55 | 55 | 34 |

The 33 MiB is the floor, paid on purpose: returning those MiB costs `madvise` calls and refaults on
the next burst.

**No slow path, no maintenance.** A thread whose steady work allocates and frees fewer than a
magazine's worth of each class per cycle settles into magazines holding exactly one swing and never
leaves them. In the last row's shape 39 of 66 runs held everything: 207 MiB free in the bitmaps and
counted, 1.5 parked. Heartbeats stopped for good after about 16,000 calls, a second in; runs that
returned memory made 110k to 125k. What tips a run was not pinned down. With the background thread 0
of 6 held; preloaded, with C `free`, 0 of 10. Work that frees on another thread, grows vectors or
touches large blocks enters slow paths all the time (the screened programs made 36k to 840k
heartbeats a run, but single-threaded pulldown-cmark 282). Forcing a slow path once in N calls costs
the fast path, forcing one at a magazine's install only delays the state, a heartbeat from `realloc`
moves would not have helped, and a tick from a routine's abort was only argued. jemalloc has the
same hole for one-at-a-time reuse, which raises no event: it kept 187 MiB of the burst for good,
where cralloc's thread returned it.

**The background thread** is on by default once free memory is over the floor and the program has a
second thread. A busy process pays nothing measurable: seven `maintain` calls in a run, 12.17 and
12.10 ns an allocation, instructions +0.02%. Idle, a wake-up costs about 35 us; after a burst 340 us
a call, 90% the purge walk, 0.05% of a CPU, and it grows with the bitmaps. It parks when its rounds
find no work and no slow path has run. A program with one thread keeps its memory once quiet, as it
does under glibc.

## Returning memory off request paths

A request that returns memory makes every request queued behind it wait: at 25k requests a second on
a worker, a millisecond once a second is the p99.9.

**A step on a request's path returns a bounded amount, unless its unit is a whole huge page.** A
large slot used to be one unit. A step now returns at most 0.5 MiB (a hold's look 2 MiB), a large
slot going back in pieces from its top, whole huge pages in a class of a huge page or more.
DataFusion, five rounds, 2026-10-21: longest heartbeat 0.98 to 3.0 ms becomes 0.29 to 0.51, longest
growth payment 1.5 to 3.4 ms becomes 0.38 to 1.6, peak 842 to 864 MiB becomes 863 to 870 (jemalloc
851 to 883). The bound on growth is what costs the peak: left for the next carve, peaks reach 870 to
940; owed and paid by heartbeats, 863 to 870. A hold look of 0.5 MiB let two runs in six over
jemalloc, so it stays at 2 MiB. One to five steps a run still pass 0.25 ms, nearly all growth
payments reading up to 16,384 bitmap words.

**At 32 MiB, a step that reaches a wholly free huge page of small slots takes milliseconds.** A huge
page goes back whole, so the step claims and releases every leaf word under it, at 42.7 ns a word
against about 9 us for the `madvise`. On a Firestorm core it passes 100 us below slots of 256 bytes:

| Slot | Leaf words | A step, no thread | Slowest request, no thread | Thread, clocked | Thread, growth outruns it | A step, batched |
|---:|---:|---:|---:|---:|---:|---:|
| 8 bytes | 65,536 | 2.82 ms (2.78 to 3.18) | 5.6 ms | 6 to 12 us | 2.8 to 5.6 ms | 636 us |
| 16 bytes | 32,768 | 1.40 ms (1.40 to 1.78) | 2.8 ms | 7 to 11 us | 1.4 ms | 315 us |
| 64 bytes | 8,192 | 358 us (354 to 596) | 0.71 to 0.96 ms | 6 to 9 us | 0.36 to 0.39 ms | 86 us |
| 1 KiB | 512 | 29 us (28 to 50) | 56 to 196 us | 6 to 12 us | under 20 us | under 20 us |

*2026-11-08 at `938b26e`, 16 KiB pages, CPU 8, `rules step`: 641 MiB carved and freed, then requests
in another class, each timed, with what the thread spent off its CPU set aside. Median and 10th to
90th percentile of 27 to 43 clocked steps over 3 or 4 runs with the class wholly on huge pages; a
growth payment's median is within 1%. `strace` shows 13 `madvise` of 32 MiB for 13 huge pages, all on
the request's thread with no background thread, and all on the background thread when it runs the
clocked pass.*

- **The time is the words.** Every word empties and refills, so each pays a summary update both ways,
  and each makes a batch of its own to release. A profile at 8 bytes puts the `madvise` at about a
  tenth of a step and the release at most of the rest.
- **It comes in bursts.** One request for each huge page, on successive heartbeats. Growth does not
  wait for the frontier: its payment takes a huge page at once.
- **One request can take more than a step.** A carve's payment and its heartbeat's step make 5.6 ms
  at 8 bytes. `top_up` runs in the same step as the huge page it fills, and adds up to 2.2 ms there.
- **The thread takes the clocked pass off requests, but not growth that outruns it.** Past 16 MiB
  unpaid a grower pays, and at 8 bytes a thread's step pays 2 MiB of debt in 2.8 ms. A grower at 7.6
  MiB a ms (8 KiB blocks, written, nothing else), or carving 64 MiB at a time, took 6 such steps a run.
  Crediting a payment with the 32 MiB it returned left those 6 where they were.
- **A shortage of free blocks moves the cost to the kernel.** Mapped by small pages, the same unit is
  a `madvise` of 2,048 pages: 260 to 520 us at 1 KiB and 3.3 ms at 8 bytes. Mapped whole, some calls
  still took 170 to 400 us, not traced further.
- **Payments over small pages pass 100 us too.** At 128 pages a payment takes 250 us at 8 bytes, 140
  at 16 and under 65 at 64.
- **At 2 MiB** the same arithmetic gives 184 us for 8-byte slots on this core, **computed**. Zen 5 has
  not been measured.

The last column is a prototype on that commit, three interleaved rounds, with no test or model run on
it: a purge's claim leaves the summary bits set, as a large class's does, and one batch releases the
range, for 9.5 ns a word. At 8 bytes with the thread on, a writing grower then took 0 or 1 steps over
1 ms a run where it took 6, though a grower that writes nothing still took steps of 0.6 to 0.9 ms. By
that rate only slots of 8 to 48 bytes stay over 100 us. Passing their huge pages on a request, as a
split of 32 MiB is passed, would hold the bound, but with no thread they would stay resident until
`collect`: 641 MiB of 8-byte slots would end at about 459 MiB, not 43. Neither is built.

**While the thread runs, the clocked, hold and growth steps are its work.** A slow path records what
is due and wakes it if it sleeps. What a request still returns is what its own call decides: the
trim at a claim, a shrink's tail, the pieces behind a move, and a freed slot under advice or left
unwritten ([large-blocks.md](large-blocks.md)). Total work does not grow (DataFusion's kernel cycles
5.98 G against 6.02). The thread inherits the program's affinity, so a server pinned to four CPUs
gets the work on those four. 2026-10-22 against `d4f3b89` and `8ad5150`, interleaved:

- The log store's slowest request in a burst's first 5 ms: 0.68 to 0.93 ms becomes 0.50 to 0.60.
  What is left is the idle drains, and it is still behind jemalloc ([servers.md](servers.md)). The
  order service's doublings: [large-blocks.md](large-blocks.md).
- DataFusion's peak holds, 0 runs of 20 over 880 MiB, with the thread waiting 87 ms a run for a CPU
  behind four workers. In the first build 3 runs of 14 peaked at 1,030 to 1,040 MiB, about 180 over,
  and 2 of 20 after two other changes. The cause was the hold's plan, which counted only stamps not
  yet held, so a region whose look stopped at its bound waited a whole hold. The limit on unpaid
  growth is kept so that memory's bound does not rest on the scheduler; no DataFusion run needed it.
- Rejected: a yield after each step. Each handed a worker a whole slice: the thread waited 462 ms a
  run, and 10 runs of 20 peaked 40 to 180 MiB over, while Meilisearch's tail and the workers' wait
  did not move. Lower priority was not built, for the same reason.

Moving the steps to the thread costs Meilisearch's tail nothing shown. In one binary switching it at
run time, 14 paired rounds of two minutes at 70% (2026-10-22, 95% intervals): p99 -0.9 ms (-2.1 to
+0.5), p99.9 0.0 (-2.3 to +2.3), cycles -0.7% (-1.9 to +0.6). Separate builds had read +2.0 ms at
p99.9, but two builds of one behaviour differ by about 1 ms ([servers.md](servers.md), "Tails the
allocator caused"). Faults were 87 against 83 a request in that binary and level between builds, so
whether they rise is not settled. The thread does about 9 ms of work a second.

## Commit calls grow with the logarithm of a region's use, up to 64 MiB

`mprotect` grew linearly with the heap when a region committed 64 KiB past each carve: 32,000 calls
a run in gix-hours, 2,300 in ruff, 1,600 in tpchgen. Each takes the memory-map lock for writing,
which every page fault in that region needs. In gix-hours the threads spent 2.6 s blocked inside it,
81 us a call against about 1 uncontended; in ruff about 190 of a typical run's 400 context switches
were queued on that lock. A commit now runs as far ahead as the region has committed, between 64 KiB
and 64 MiB. Calls fall three- to forty-fold (478, 740, 500, 510 and 117 on five programs), and no
memory moves. Mapping the whole reservation writable is level with this. The order service on actix
went from 1,670 calls to 300 and 222 context switches to 138, at level rate and tail. The batch
programs' wall times are bimodal under every allocator ([measuring.md](measuring.md)).

## Magazine budget

Chosen: 64 KiB of objects a magazine, depot target 128 KiB. Ten jobs, five rounds, per-round ratios
with a 95% bootstrap interval, 2026-10-17 at `ae689e1`.

| | jemalloc | 128 KiB | 64 KiB | 32 KiB | 16 KiB |
|---|---:|---:|---:|---:|---:|
| Mixed pipeline: time against 128 KiB | | | +5.1% [3.6, 6.8] | +11.7% [10.1, 15.9] | +23.3% [21.2, 27.8] |
| Its peak, MiB | 5.24 | 7.07 | 5.12 | 4.00 | 3.54 |

64 KiB is the largest setting at or under jemalloc's memory on the mixed pipeline, and it costs what
it costs only there, about 55 ns per extra slow path, still 30% faster than jemalloc on that job.
Heaps of 100 MiB or more moved under 0.5 MiB at every setting: a fixed cost, not a share. Magazines
have since grown to 1,024 slots ([magazines.md](magazines.md)), which reaches only classes to 128 B.

## The huge-page threshold

A small class of at most 1 KiB switches to huge pages at 16 MiB carved. It costs the huge page above
each watermark, up to 2 MiB a class, and coarser purging. It buys fewer faults and dTLB misses
([program-effects.md](program-effects.md)).

| Cgroup peak, MiB | 16 MiB | 8 | 4 | 2 | First carve | jemalloc |
|---|---:|---:|---:|---:|---:|---:|
| Churn, 1M live | 109.4 | 112.9 | 115.6 | 118.0 | 133.0 | 110.2 |
| BTreeMap 200k | 25.7 | 27.0 | 27.0 | 32.7 | 61.5 | 25.9 |

*2026-10-18 at `fca1d12`, two rounds that agreed within 0.4 MiB.* Huge pages from the first carve
cost 2 MiB for every class touched: a three-object process holds 20.7 MiB. 4 MiB is the knee, at
most 6 MiB more, and was turned down to stay under jemalloc on churn and the small maps. The switch
is exact: a threshold of N used to leave up to N + 2 MiB on small pages.

Rejected: `MADV_COLLAPSE` at the switch, which at 16 MiB buys 3% on BTreeMap and 1% on JSON for 0.1
to 0.4 ms of synchronous work per 2 MiB in a slow path, and adds nothing at a lower threshold.
Populate-ahead: faults cut three to five times, kernel time by at most a tenth, about 1 MiB at every
peak.

**On 32 MiB huge pages the threshold stays at 8 huge pages, 256 MiB, and not because it counts huge
pages.** Fewer buy about 1% where they buy anything, and the worst case costs them far more. Every value
from one binary, switched at run time, so that placement cancels (`bench/roofline/aarch64/hopjob`,
variant `hugen`, which does not ship), interleaved, on an M1 Max with 16 KiB pages, at `e779fa0`,
2026-11-08. Change in cycles from 8, and the second allocator at 8 as the control, 7 rounds, 15 on JSON, 3
on the programs and 2 on the servers, at 70% of what this machine saturates at:

| Job | 1 | 2 | 4 | Control | Peak at 1, 2, 4, 8, MiB |
|---|---:|---:|---:|---:|---|
| Tree, depth 20 | −0.68% | 0.00% | +0.04% | −0.05% | 65, 52, 52, 52 |
| JSON, 32 MiB | −0.27% | −0.56% | −0.59% | +0.46% | 382 at each |
| B-tree map, 1 M | +0.13% | +0.73% | +0.29% | −0.04% | 146, 126, 126, 126 |
| Hash map, 1 M | +0.15% | +0.29% | −0.13% | +0.30% | 208 at each |
| Churn, 100 k live, mixed | −0.36% | −0.08% | +0.19% | +0.20% | 54 at each |
| Churn, 1 M live | +0.27% | +0.98% | +0.36% | +0.40% | 111 at each |
| Churn, 10 M live | −1.67% | −0.92% | −0.39% | −0.34% | 1,221, 1,157, 1,084, 1,063 |
| boa, the V8 suite | −0.25% | −0.43% | −0.09% | | 309, 309, 309, 276 |
| ruff, 4 threads | +0.09% | +0.09% | +0.09% | | 66, 65, 66, 62 |
| DataFusion, TPC-H at 3 | −0.09% | −0.14% | +0.01% | | 940, 942, 941, 942 |
| miniredis, 2 GiB heap | −0.04% | −0.23% | −0.29% | | 2,062, 2,045, 2,045, 2,044 |
| miniredis, 4 GiB heap | +0.17% | +0.22% | +0.50% | | 4,147, 4,090, 4,047, 4,046 |

Those are with the huge pages in hand: JSON held 256 MiB of them at 1 and 32 at 8. Its kernel time halves,
0.34 to 0.17 cycles a byte, and its own code gives back half of that. Churn at 10 M live has the most
to gain, with 736 MiB of them: its fill falls from 15.5 cycles an object to 11.8, and its steady state
1.7%. One build for each value read JSON as −2.3%, which was where its code landed. The servers' 99th
percentile is 2.2 to 2.4 ms at every value, with deep idle off.

**The worst case on purpose:** 24 dense classes, each holding just past a threshold, where its first huge
page is up to half of what it holds. Peak in MiB, with huge pages free:

| Each class holds | In objects | 1 | 2 | 4 | 8 | mimalloc | jemalloc |
|---:|---:|---:|---:|---:|---:|---:|---:|
| 33 MiB | 792 | 1,489 | 794 | 794 | 794 | 930 | 828 |
| 65 MiB | 1,560 | 2,257 | 2,257 | 1,562 | 1,562 | 1,794 | 1,628 |
| 129 MiB | 3,096 | 3,533 | 3,707 | 3,793 | 3,098 | 3,522 | 3,229 |
| 257 MiB | 6,168 | 6,865 | 6,865 | 6,865 | 6,865 | 7,012 | 6,431 |

Each value's worst is the same 695 MiB, so the share falls as the threshold rises: +87% at 1, +44% at 2,
+22% at 4 and +11% at 8, against the leanest incumbent +80%, +39%, +17% and +7%. That last is the eighth
of a class the constant allows, and 8 is the only value that stays near the goal on memory on a heap of
gigabytes. A real program pays much less: the most here is 158 MiB at 1, on churn at 10 M live.

**A shortage does not change the answer, and it adds to it.** With the free blocks used up first
(`hopjob/fragment.c`), each allocation and first write of 1 KiB blocks, three runs at each value:

| Class carved to | 1 | 2 | 4 | 8 |
|---:|---:|---:|---:|---:|
| 128 MiB, slowest request of each run | 2.6 to 4.1 ms | 0.38 to 3.1 ms | 0.38 to 0.92 ms | 44 to 178 us |
| 128 MiB, ns a request over all | 139 to 157 | 123 to 164 | 143 to 149 | 131 to 141 |
| 384 MiB, slowest request of each run | 0.64 to 3.3 ms | 0.88 to 40 ms | 1.7 to 3.5 ms | 0.38 to 41 ms |
| 384 MiB, ns a request over all | 111 to 127 | 116 to 244 | 123 to 153 | 129 to 249 |

A class past 256 MiB meets a stall at any value. A class under it meets one only at the lower values, once
for each huge page on the way, at most seven more a class in a process's life. With blocks free the same
growth costs 111, 110, 115 and 129 ns a request at 384 MiB: 15% off a phase of growth, and nothing after it.

## The rules are bytes, on a kernel of any page

**The cap and the floor act at 64 and 32 MiB on a kernel with 32 MiB huge pages, where stated in huge
pages they read 1 GiB and 512 MiB and never acted.** One thread, Linux 7.2 with 16 KiB pages:

| | Stated in huge pages | In bytes |
|---|---:|---:|
| 512 MiB freed, 512 MiB of another class grown: retained at the end, MiB | 512.6 | 64.0 |
| The same: resident at the end, MiB | 1,059.5 | 582.5 |
| 256 MiB freed, then 15 s of `maintain`: retained at the end, MiB | 256.6 | 31.9 |
| The same: resident at the end, MiB | 259.6 | 34.9 |

*2026-11-07, `09c0e19` against `335f834`, `rules cap`, `rules floor` in `bench/roofline/rules`, one
run each.* Retained memory falls by what is carved, 32 MiB for each 32 MiB, and stops at 64.0.

**A class past 256 MiB still gets 32 MiB huge pages, and only a class of at most 1 KiB does.** Stated
as slots to a page, the rule took classes up to 4 KiB in on 16 KiB pages, which it does not mean:
carved to 771 MiB, a class of 1 KiB has its huge-page part from 256 MiB and 544 MiB mapped by huge
pages; a class of 4 KiB had the same and now has none. The rule's own threshold stays eight huge
pages, since what it bounds is a share of the class.

**A split of 32 MiB is the background thread's or `collect`'s.** A heap with 416 MiB under huge
pages, one live block of 1 KiB left in each 3 pages' worth, then 10 s of requests:

| | `MADV_COLD` by a request | By the background thread | By `collect` | Resident after the requests, MiB |
|---|---:|---:|---:|---:|
| One thread | 0 | | 8,274 | 536 |
| With the background thread | 0 | 8,275 | 0 | 281 |

*Same commit and date, `strace -f` over `rules split`.* So a program with one thread keeps such a
heap's sparse huge pages whole until it calls `collect`, and growth over the cap cannot pay with
them. That is the price of the bound on a slow path: 1.0 to 1.8 ms a split, by
[machine-aarch64.md](machine-aarch64.md).

**A bound on the kernel's work stays in pages.** A step of the walk returns 128 pages and a look at
the hold 512 on every kernel, 2 MiB and 8 MiB on 16 KiB pages, since the kernel's cost is per page: 42
us here for 512, 41 to 49 on Zen 5.

## Which rules earn their place

Each rule switched off in a build of its own; the figures are the rule off against cralloc as built.

| Rule off | What changed | Verdict |
|---|---|---|
| Huge pages for blocks that grow by doubling | One Vec to 256 MiB 131%* slower, 64 Vecs 37%* slower; peaks within 1% | Earns its place |
| Staggered starts for large blocks | 64 Vecs walked together 313%* slower; sparse 64 KiB blocks peak at 17.8 MiB against 33.4; large objects crossing threads 6.7%* faster | Earns it on time; costs memory when a block is touched in its first page |
| Huge pages for small classes past 16 MiB | JSON 33.5%* slower, BTreeMap 1.7%*; peaks 0.6 to 1.4%* lower | Earns its place |
| The magazine age limit | A burst ends at 60.1 MiB against 4.9 | Earns its place |
| The depot's shedding | A burst ends at 57.1 MiB against 4.9; phase change 16.9 against 9.4 | Earns its place |
| The spare sweep | A burst ends at 7.6 MiB against 4.9 | Earns a little |
| The refresh at one in 32 | Mixed pipeline's peak 8.8%* lower; a burst's end unchanged | Not at this rate; its work is the trough |
| The density test on grown blocks | Nothing; no job here grows a block sparsely | Evidence is in [large-blocks.md](large-blocks.md) |
| The move that skips all-zero pages | Nothing that reproduced; no job here moves a sparse block | Evidence is in [large-blocks.md](large-blocks.md) |

*2026-10-18 at `39c2f54`, not repeated; several rules have changed since, and the bursts' ends
predate the floor. Eleven jobs, five interleaved rounds, the memory shapes two; `*` marks a 95%
interval that excludes zero.* Separate builds move placement-sensitive jobs: one Vec to 256 MiB
moved by up to 9% and the RPC log job by up to 4.7% under rules that barely reach them, so smaller
differences there are left out. The skip-zero build's 8.5% was one of these: six later rounds put it
within 2%. The refresh's two readings after a burst disagree (7.1 to 3.3 MiB on 2026-10-17,
unchanged here), not traced; both are under today's floor.

## Small heaps

The goals say the peak of a heap of a few megabytes is not something to tune. The fixed cost,
charged at exit (own pages, page tables, kernel slab objects), cralloc against jemalloc and glibc,
MiB, five rounds, 2026-10-18 at `a4ccb7d` and after: three allocations 0.47, 0.44, 0.22; churn with
1k live 0.75, 0.61, 0.38; mpmc with four producers and four consumers 4.52, 3.16, 0.98. An RPC
server's figures at exit are after the others gave memory back, so compare its peak: 5.50, 5.93,
3.35. Those had one spare a stack and 256-slot magazines; both have since grown.

- **Page tables are the largest fixed cost.** Regions lie 4 GiB apart, so a class's first page needs
  a PTE and a PMD page of its own, about 8 KiB per class in use. A metadata block 128 MiB from the
  next cost as much again until the small classes' headers moved into the back end's state, with the
  leaf bitmap last. A class whose frees never reach the bitmaps now never touches its block, and one
  that does touches pages in one 2 MiB, under one page of page tables.
- **Parked objects are the whole story of mpmc:** 2.5 MiB against 0.3 live. A fresh fill costs
  nothing, since its objects are untouched, which kills the idea of a short first magazine.
- **Spares.** A CPU whose traffic in a class goes one way fills a stack it never takes back: about
  0.5 MiB of mpmc 4+4's peak at four spares. One spare cost sh8bench 1.7 to 7.9%, two 1.2 to 3.5%,
  four up to 0.9%, so it stays at four. Magazines of at most 128 slots cost 16 to 20% on the
  pipelines.
- **Moving the headers first cost a pipeline of large objects 4%.** The class's layout was built
  twice and copied, with 19 more stores and 18 more loads a claim once the growth path was inlined,
  and the bitmaps of a large class were no longer near its summary (31 L1 misses in 45 accesses on
  one page). A cold call for the layout, an out-of-line growth payment and a large class's block
  starting with its header took it from 76.6 to 72.3 ns an allocation.

## How memory is accounted

- **Peak** is the cgroup's `memory.peak` of the measured process alone, page tables and the kernel's
  per-CPU charge stock included, so a small process moves by 0.4 MiB between identical runs.
- **Where resident memory is.** A sampler thread (feature `cralloc-memstat`) walks the heap with
  `Cralloc::inspect`, a read-only walk that never allocates, asks `mincore` which pages are
  resident, and attributes every resident byte to one use. In a drift run it takes an account with
  every thread stopped, so the sums are exact.
- **jemalloc's account** comes from its own statistics at the same moments. Its `stats.resident`
  counts extents whether touched or not, so its heap is the process's anonymous memory less what
  lies outside it. **The whole process** at exit (feature `memacct`, any allocator) is cgroup
  counters, smaps and `pagemap`.
- **Drift runs** (`workload drift`) count work in operations, so a paused process does the same
  work; only runs on the measurement CPUs give seconds. **Memory shapes** (`workload quiet`,
  `touch`, `sparse` and others) print memory after a peak, in idle caches and behind sparse blocks,
  and are not ranked.

## Open questions

- **One machine, mostly single runs.** Clock rules act less often per operation in time-compressed
  jobs than in a server at a tenth of their rate, so returning memory is if anything
  under-represented. Memory runs used one L3. The wandering mix's wander is sampling noise in its
  size tables: a real program's may be more or less, and how far demand rises sets the ratchet's
  bound.
- **Past the ladder.** Nothing above 10 GiB, no NUMA, the log store and mini-redis not on it, and no
  burst after a silence at 10 GiB with free memory to fault back.
- **Parked memory against CPU count.** Servers were pinned to four CPUs; the bounds are arithmetic.
- **Whether turnover sets a trough.** One heap at two values of the long-lived share would test what
  the table above only fits.
- **Packing the smallest classes** in a trough. No rule built, and it is the gap to jemalloc.
  Refresh on evidence at 5 and 10 GiB, and a real server in a trough with it, are not measured.
- **Not explained:** why tcmalloc, mimalloc and glibc do not pack, what tips a quiet burst into
  having no heartbeat, and what is left of the size shift's gap with the tail rule on. Swings of
  hours were not run.
- **The threshold at 4 MiB** was refused to stay under jemalloc by a few MiB, before the goal said
  fixed MiB are spent freely. Worth measuring again.
- **Meilisearch while indexing.** Before any load it peaks at 1,388 to 1,433 MiB against jemalloc's
  1,230 to 1,251. Its heap is still falling when load starts, where jemalloc's has already fallen
  and rises under load. Through 70% load it reaches at most 710 to 739 against 778 to 790
  ([standings.md](standings.md)). The indexing excess is not attributed.
