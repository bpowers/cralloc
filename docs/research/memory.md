# Where memory goes

What sets a process's resident memory against what the program holds, what each of cralloc's rules
buys and costs, and where it stands against jemalloc, tcmalloc, mimalloc and glibc. Blocks past the
small classes have their own note, [large-blocks.md](large-blocks.md). How placement changes the
program's speed is in [program-effects.md](program-effects.md). The rules are in `docs/design.md`,
"Returning memory and huge pages".

Measured on one machine (Ryzen 9 9950X, Linux 7.2.9, transparent huge pages `madvise`), 2026-10-17
to 28. *Resident over live* is anonymous resident memory, less the workload's own bookkeeping, over
the bytes the program holds at the sizes it asked for.

## The answer

- **On a steady heap cralloc holds the least, and it does not drift.** A 2 GiB cache churned for 15
  billion operations ends at 1.105 times live, flat from the first minute. The order service at 10
  GiB holds 1.073 against jemalloc's 1.095. A 10.6-hour Qdrant soak beside jemalloc drifted in
  neither, with cralloc 13 to 31 MiB lower at every quiet point.
- **Most of what is left on a steady heap is class rounding.** Of the 10.5 points over live on the
  steady cache, 7.8 are slots larger than the request, which jemalloc pays too. cralloc's lead at a
  peak is metadata (4.6 MiB against 38 at 2 GiB, because free memory is tracked in bitmaps) and free
  slots inside slabs (15 MiB against 45).
- **Its loss is troughs, to jemalloc alone, and the cause is reuse order.** When short-lived objects
  fall away while long-lived ones keep being made, each allocation takes the slot the last free
  pushed, so the survivors stay spread over the peak's footprint. At a 2 GiB trough cralloc holds
  0.95 GiB and jemalloc 0.66, on the same live slots: 273 MiB of free slots beside live objects
  against 40, 88% of it in classes of 128 bytes and down. tcmalloc, mimalloc and glibc do not pack
  at all there and hold more than 2.1 GiB.
- **When the size mix wanders, huge pages ratcheted.** A class whose demand fell kept every 2 MiB
  unit that still held one object: 1.10 climbed to 1.21 times live, then held. The tail rule returns
  that memory (1.14 at the peaks) for 1 to 2% of cycles on an allocation-bound job and nothing
  measurable on servers. After a large shift in sizes cralloc still holds more than jemalloc, 1.36
  against 1.19.
- **Most servers never engage any of this.** The order service's free memory stays under the 32 MiB
  floor at every size to 10 GiB, so no pass, hold or growth payment runs while it serves. The
  machinery earns its keep on programs with phases, and on large blocks.
- **Keeping memory on purpose has a visible price after a peak.** The floor leaves 33 MiB where
  jemalloc reaches 4.5, and a one-thread program that stays inside its magazines keeps all of a
  burst, as glibc and tcmalloc do. Both are chosen limits.

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
| Large-buffer cache, geomean of four phases | **1.41** | 1.52 | 2.02 | 2.11 | 2.05 |

*2026-10-24 to 28, one run a cell for the long jobs; cralloc at its latest measurement of each (from
`5536610` for the steady cache to `ad11370` for the wandering mix), the incumbents' code unchanged
over those days. Real servers at 70% load, resident peak, ten jobs at `2ff7322`, others against
cralloc, geometric mean (geometric standard deviation): jemalloc 1.01 (x1.17), tcmalloc 1.11
(x1.21), mimalloc 1.49 (x1.38), glibc 1.06 (x1.58). Computed from the standings' records, median of
5 to 15 runs a job.*

Terms. A *slow path* is any call the magazines cannot serve. A *heartbeat* is the bounded round of
maintenance slow paths run. To *launder* a magazine is to return its objects to the bitmaps.
*Retained* memory is the back end's count of free memory it believes resident. The *frontier* is the
highest slot a region's claims reached in the last two intervals; an *interval* is 2^32
cycle-counter ticks, about a second here. The *floor* is 32 MiB of returnable memory the clock
leaves alone. The *cap* is 64 MiB of retained memory, over which growth pays.

| Rule | Buys | Costs |
|---|---|---|
| Purge on a clock, above the frontiers | A 218 MiB burst down to the floor in 8 s, where it once stayed for good; the RPC jobs' faults under load down three quarters | A burst waits a second longer; 0.3 cycles a claim |
| Growth over the cap pays, frees never do | Large pipeline 319 to 119 ns an allocation, 298,000 `madvise` calls to 53 | HashMap's peak +29 MiB |
| Parked objects age out | Burst at 12 s, before the floor: 73 to 4.3 MiB | A refill per class per interval |
| Refresh one retiring magazine in 32, up to one in 4 on evidence | 2 GiB trough 1.10 to 0.95 GiB | +0.6% of cycles where it acts, nothing elsewhere |
| Tail rule | Wandering peaks 1.23 to 1.14, troughs 4.29 to 3.27 | 1 to 2% on the job; huge pages at peaks 1.7 to about 0.7 GiB |
| Huge pages for classes to 1 KiB after 16 MiB | Fewer faults and walks where objects are dense | Up to 2 MiB a class at the watermark |
| A 32 MiB floor | No refaults on the next burst | 33 MiB kept after a peak |
| Magazines of 64 KiB, not 128 | Mixed pipeline's peak 7.07 to 5.12 MiB | +5.1% on that job, nothing resolved elsewhere |
| Background thread, on by default | Memory returns in a silence; requests take no purge steps | 35 us a wake-up; 9.8 s of CPU in 10.6 hours of Qdrant |

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

*2026-10-27 and 28: cralloc at `ab6fe66` with the refresh rule, jemalloc built with statistics.*

- **Rounding is the floor of any allocator with size classes.** Both use four classes a doubling.
  glibc has none and the lowest peaks on this job (1.075), though it pays elsewhere.
- **The steady cache at 2 GiB has the same shape:** 2,048 live, 160 rounding, 13 to 15 free beside
  live, 7 to 8 parked, about 20 past watermarks, 4.5 metadata.
- **At a peak, pages are full and every allocator is close.** The gap opens when demand falls, and
  it is free slots beside live objects: nothing can return those, so they are placement, not policy.

## A steady heap does not drift

**The steady cache:** eight threads share a cache; each operation puts a value (log-normal, median
128 B, sigma 1) in a random slot and evicts at random while over budget, so most frees are another
thread's object, and makes two short-lived objects. Ratio's slope over the second half of 15 billion
operations, per billion: cralloc +0.00003, tcmalloc +0.00006, jemalloc -0.0006, glibc -0.012,
mimalloc +0.021 (least squares over 75 samples, computed from the runs). Memory is not bought with
time: 1,271 cycles an operation against tcmalloc's 1,326, jemalloc's 1,398, glibc's 2,051 and
mimalloc's 2,064 (2026-10-24 at `5536610`, one memory run each, two timed rounds).

**Threads that come and go.** The same cache filled by producers and drained by consumers, busy
threads 8, 2, 8, 1, 8, 4 by phase: cralloc settles at 1.22 (1.21 to 1.27 over the last half).
jemalloc swings between 1.24 and 2.5, tcmalloc is at 1.38 and still creeping, mimalloc 1.99, glibc
2.35. Its cycles count consumers spinning on an empty queue and move 17% between rounds, so its gap
to tcmalloc is not a stable difference.

**Qdrant for 10.6 hours** (2026-10-27 at `8e9d097`, jemalloc beside it on the same schedule; 35
cycles of searches, upserts, deletes and a collection built and dropped, each ending in 1 to 20
minutes of quiet). Anonymous memory at the quiet points rose over the first fifteen cycles in both
and then held: cralloc median 156 MiB (140 to 158), jemalloc 177 (175 to 187), both under 1 MiB an
hour over the last 5.9 hours (computed from the quiet points). Mappings stayed bounded, about 770
against 550. The allocator's count stayed between 0.6 MiB under and 6.8 over what `mincore` sees.
Huge pages did not drain. The thread took 0.07% of a CPU when Qdrant's own threads were active and
under 0.01% when it was idle. Minor faults were 2.7 million against 4.2. A mostly read workload on
shared cores, which cannot show a heap that grows for days or weeks of churn.

## The heap's fixed numbers hold to 10 GiB

Nothing gets worse faster than the heap (2026-10-27 at `bf837db`; three rounds of the order service,
medians; one run of the drift jobs). The order service stores an order of about 1.9 KiB and drops
the oldest on each request.

| Live | Order service: cycles a request, p99.9 | Over live: cralloc | jemalloc | tcmalloc | mimalloc | Steady cache: cralloc | jemalloc |
|---|---:|---:|---:|---:|---:|---:|---:|
| 0.5 GiB | 68.0k, 24 us | 1.099 | 1.106 | 1.114 | 1.199 | 1.149 | 1.174 |
| 2 GiB | 68.2k, 24 us | 1.080 | 1.097 | 1.098 | 1.156 | 1.104 | 1.135 |
| 4 or 5 GiB | 68.2k, 24 us | 1.076 | 1.095 | 1.096 | 1.151 | 1.091 | 1.117 |
| 10 GiB | 68.4k, 24 us | 1.073 | 1.095 | 1.095 | 1.149 | 1.086 | 1.113 |

On the order service no pass, hold look or growth payment runs while it serves, and the background
thread starts at shutdown. On the steady cache fragmentation holds the count over the floor while
what a pass could return stays under it, so each interval costs one lap that only counts: longest
pass 12.6 ms, 0.5 s of CPU in two minutes at 10 GiB.

What does grow with the heap, and stays bounded:

- **A pass's length:** longest 0.24 s at 2 GiB, 0.78 s at 10, against the one-second interval. The
  kernel returns about 1.5 GiB a second of the thread's time. Nothing is lost: a trough comes within
  a tenth of its end in 13 to 17 s at both sizes (jemalloc 13 to 22 s).
- **A step on a request's path,** without the thread: longest 0.38 ms at 2 GiB, 1.5 ms at 10; p99.9
  30 and 61 us. The step is bounded in pages; the growth is the kernel's split with eight CPUs to
  shoot down.
- **`mprotect` commits** are linear past 64 MiB a region: 284 calls at 0.5 GiB, 627 at 10, a few of
  3 to 7 ms in large fills. Letting a commit run as far ahead as the region has committed, with no
  cap, would remove about 280. Not built.
- **Page tables on the large-buffer job:** 1.5 to 2% of the heap, three times the incumbents' (77
  MiB at 5 GiB against 21 to 23), and 2,196 mappings against under 460, following the number of
  grown blocks (inferred from the counts).

The numbers themselves:

| Number | Value | Verdict |
|---|---:|---|
| Floor | 32 MiB | Holds. 0.3% of a 10 GiB heap; scaling it keeps more of a trough for nothing measured |
| Retained cap | 64 MiB | Holds. Only the large-buffer job is over it, half its growth steps at 0.5 GiB and at 5. Eight times: faults +8%, cycles +4%, peak the same |
| Kept outgrown slots | 64 MiB | Holds. Eight times: faults -7%, cycles +1%, peak the same |
| Unpaid growth | 16 MiB | Holds. Slow paths paid 1,383 steps at 5 GiB, each under 0.85 ms |
| The thread's round | 500 ms | Holds. Longest step 5.4 ms at 10 GiB, 0.4 to 10% of a CPU by job |
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
- `MADV_COLD` on part of the unit splits it first, and the return is then real. The split costs 20
  to 80 us a unit, up to 8 ms on busy CPUs.
- Under the advice khugepaged rebuilds any unit with one page present, within minutes. So advice
  must not stand over units that stay sparse.
- A unit returned whole faults a huge page at once, since `MADV_DONTNEED` frees its empty page
  table. A unit returned in part faults 4 KiB pages until khugepaged collapses it.
- Advice changes take `mmap_lock` for writing: 2 to 8 us at p50, up to 23 ms beside 2 MiB faults,
  which stall in compaction here (p99.9 9 ms).

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
| One size table: troughs | 3.77 | 3.2 | 3.66 | 4.03 |
| Size shift, median 384 B | 1.51 | 1.36 to 1.37 | 1.19 | 1.33 to 1.83 |
| Size shift, median 1,024 B | 1.17 to 1.23 | 1.14 to 1.15 | 1.25 to 1.28 | 1.75 to 1.85 |
| mini-redis wide values | 1.33 | 1.15 to 1.18 | 1.16 | 1.25 to 1.29 |

*2026-10-25 to 27, one run each; rule at `ad11370` for the wandering mix, `dee220a` and earlier for
the others. jemalloc's and glibc's ranges are between sessions and cycles.* On Qdrant (`3c2265e`,
three rounds, medians) heap mean 449 to 414 MiB, peak 542 to 515, a minute after close 185 to 155,
tails within their spread.

- **Time.** 1.0 to 1.5% of cycles on the 4-phase lifetime job inside one binary with plentiful huge
  pages (4 KiB walks +15 to 50%, 2 MiB walks -35 to -60%). Shipped builds read 601 to 609 cycles an
  operation across sessions against 593 to 595, which includes placement. Nothing measurable with 3
  s swings, with scarce huge pages, or on servers.
- **Most servers do not trigger it.** The store split 85 huge pages and lowered once, all after the
  load closed. The order service and Meilisearch did nothing. Qdrant, whose optimizers swing, raises
  and lowers 12 to 16 times in 30 s at 70% load.
- **System calls.** About 265,000 `madvise` and 10,000 `mincore` on the 4-phase job, on the thread,
  under 1% of a CPU. Without the thread a split on a slow path averages 40 to 60 us, at worst 1.7
  ms, and counts as a whole step.
- **Huge pages come back slowly.** A split unit that fills reach again faults 4 KiB pages until
  khugepaged rebuilds it, in minutes. `MADV_COLLAPSE` costs about 6 ms a unit, and telling dense
  units from sparse ones cheaply is not solved.

Tried and rejected:

- **Lowering the boundary whenever the frontier fell.** On the log store, whose small classes wobble
  by a few MiB, 50 advice changes a run returned nothing, and p99.9 went from 0.1 to 0.2 to 0.4 ms.
  A swing under about 128 MiB now changes no advice.
- **Reading every unit above the frontier page by page.** The thread woken by a burst spent 1.7 ms
  on a server CPU in its first 10 ms (0.46 before), four times the walk, and the slowest request
  took 1.2 to 2.7 ms in 6 of 8 runs (0.4 to 0.8). The sampled look brought it to 0.5 to 0.9.
- **Sampling words or aligned runs.** A leaf word of a 1 KiB class covers 16 pages, so sampling
  words missed nearly everything (troughs 3.81 against 3.25); aligned runs of four missed free runs
  not aligned (3.53).
- **Patience.** Waiting N intervals returns nothing from troughs shorter than about N to 2N
  intervals and most of longer ones. It did nothing on Qdrant at 30 and 120.
- **No split.** Returns RSS and not the charge (troughs 3.14 by RSS, 3.93 by the cgroup).
- **Counting pages resident when the advice goes back.** The count ran 78 to 89 MiB over `mincore`,
  enough to put a heap over the cap and purge memory in use. Instead the walk distrusts clean bits
  under the advice and asks `mincore` before a split.
- **Sizing the raise's headroom from evidence.** Two such rules did no better than the constant.
  None at all moved a growing class's boundary about ten times an interval (716 raises a run; 16 MiB
  145; 64 MiB 97).

The region's tail word is modelled in `tla/Tail.tla`. The model found two defects, both fixed: an
owner that cleared a repair request a refused call had left, and a fork child inheriting a word
whose owner's call never finished.

## In a trough, placement decides, and reuse order sets placement

**A trough's footprint is where the long-lived objects were born.** So it depends on how much of the
long-lived set turns over while the short-lived one is gone. Two studies that looked contradictory
are two ends of one line.

| Long-lived set replaced, times a phase | cralloc | jemalloc | tcmalloc | mimalloc | glibc |
|---|---:|---:|---:|---:|---:|
| 0.5 (wandering mix, 2 GiB) | **3.27** | 3.79 | 4.59 | 4.83 | 4.03 |
| 2.4 (10 GiB) | 3.40 | **3.19** | 4.14 | 4.17 | 3.96 |
| 4.7 (5 GiB) | 2.73 | **2.28** | 4.16 | 4.20 | 3.97 |
| 12 (2 GiB) | 1.77 | **1.24** | 4.19 | 4.27 | 4.01 |
| 47 (0.5 GiB) | 1.94 | **1.34** | 4.29 | 4.66 | 4.09 |

*Trough over live, median of the last three, one run each. Turnover computed from each run's
arguments (share of allocations long-lived, operations a phase, mean value 211 B, size of the
long-lived set). Only the 12-times row has the refresh rule below; cralloc's others are `ad11370`,
`bf837db` and `d36e0cb`.*

When little is replaced cralloc holds the least, with the tail rule (4.29 without it). When objects
are replaced it holds more than jemalloc, because of where the new ones go. Why tcmalloc, mimalloc
and glibc do not fall at any rate was not studied.

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

By class at the first 2 GiB trough, MiB, with the refresh rule:

| Class | Live slots | Free beside them | Filled |
|---|---:|---:|---:|
| 16 to 64 B | 29 | 79 | 27% |
| 80 to 128 B | 63 | 161 | 28% |
| 160 to 256 B | 127 | 27 | 82% |
| 320 to 512 B | 150 | 3 | 98% |
| 640 B to 8 KiB | 220 | 3 | 99% |

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

*One accounted memory run each, means of three troughs, at `d36e0cb`. Timed runs separately, one to
three rounds.*

A fixed one in 4 costs about 1% wherever it runs, so the rate follows evidence. Each lap of the
purge walk takes the small classes' free slots beside live objects as a share of what they carved.
Under 1 in 64 the rate stays one in 32; it halves at 1 in 64, 1 in 32 and 1 in 16, up at once and
down a step a lap. The share is 0.6% on the steady cache and at the peaks, 18% at the troughs and
12% once the rule has packed them, which keeps the most.

| In one binary, two rounds | Off | Rule | One in 4 always |
|---|---:|---:|---:|
| 2 GiB, 18 s phases: trough, GiB | 1.09, 1.13 | 0.95, 0.95 | 0.94, 0.94 |
| The same: cycles an operation | 638, 640 | 643, 643 | 646, 649 |
| 1.4 s phases: cycles | 608, 608 | 612, 613 | 615, 615 |
| Steady cache: cycles | 1,267, 1,277 | 1,275, 1,269 | |
| Order service 2 GiB: Mcycles a request, three sessions | 67.53, 67.77, 67.44 | 67.89, 67.51, 67.43 | |

*2026-10-28 at `ab6fe66`, a run-time switch.* The order service never walks a lap while it serves,
so the rule stays off there.

**What would pack the smallest classes.** Retirement rules do not reach classes of 128 B and down:
their magazines rarely retire. Not built, in order of how directly they act: a fill-side rule that
launders a full spare when the class is shrinking on this CPU; an age limit in exchanges as well as
time; smaller magazines for these classes alone, which undoes measured speed
([magazines.md](magazines.md)) and took only 67 MiB.

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
| Small heap, 10 s phases: trough, MiB | 85 | 85 |

*2026-10-28, one timed run each.* A quarter of a falling count does not keep up with what a phase
frees. It cuts faults by a third, and those are refaults at 0.1 to 0.2 per 1,000 operations, tens of
microseconds a second. On a small heap two passes in three found no more than the floor returnable,
so nothing separates them.

## Purge on a clock, never on a level

**A free cannot tell idle memory from memory about to be reused.** Everything here follows from
that.

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
| JSON 32 MiB: cycles a byte, kernel share | 20.9, 14.7% | 18.3, 3.2% |
| HashMap 1M: cycles an entry, faults | 671, 190,000 | 579, 60,700 |
| HashMap 1M: peak, MiB (jemalloc 184) | 175 | 204 |

*2026-10-18 at `867aadf` and after, five rounds interleaved, medians.* HashMap's price is the tables
it outgrew, under the cap. A first version also charged claims that made purged pages resident
again, and the allocator chased its own tail: 24,500 `madvise` calls on a `HashMap` rebuilt five
times, against 43. Refaults are reuse, not growth. A cap of 16 MiB returned a burst sooner and left
the peaks where they were.

**Only what fills did not reach.** Passes that started at the top left a quiet heap's lowest region
resident; passes that carried on round the heap took turns in a server's busiest regions and cost it
2 to 4%. The frontier fixes both.

- The RPC jobs' faults in the measured window fall by three quarters (1,050 to 250, 2,250 to 480,
  260 to 54), cycles level.
- Fragmentation over the cap stops costing the working set. A gigabyte of 256-byte objects with one
  in eight kept, then a 64-byte working set for 20 s: 38,330 `madvise` calls and 36k to 46k faults
  become 552 and 4,730.
- A working set that comes and goes every 1.5 s under the cap: faults a re-allocation 2,450 to 41.
  With gaps of 2.5 s or over the cap it helps less (3,820 to 2,140, and 35,180 to 31,730).
- Price: a burst that goes quiet keeps its memory one interval longer (215 MiB at 1 s against 102,
  level from 2 s), and a fill pays about 0.3 cycles a claim.

**Rejected: enforcing the cap at the free,** tried again as a cure for a quiet burst. gix-hours +46%
cycles and, called on every launder, fourteen times slower; DataFusion 78% more faults. Sparing
recent memory removes most of that and all of the benefit, since all of a burst is recent.

## Parked objects pin the heap

A parked object holds its page as surely as a live one, and parked objects are a random sample of
wherever the heap has been. Half a megabyte of them pinned 50 MiB; one stack of 234 objects of 256
bytes pinned 31 MiB of huge pages. So a magazine stays loaded for at most an interval, full spares
are swept, a depot stack sheds the depth it did not need, and retiring magazines are refreshed. Free
218 MiB, then light work, MiB at 4 s and 12 s: 73 and 73 without these, 39 and 4.3 with them
(2026-10-17, one run each).

**The refresh's base rate.** Instructions against no rule at all, median of eight interleaved runs:

| Refresh | Churn, 100k live, mixed sizes | Pipeline, small | Left after the burst at 12 s, MiB |
|---|---:|---:|---:|
| None | +0.7% | +1.9% | 7.1 |
| One in 32 | +5.4% | +6.3% | 3.3 |
| One in 8 | +17% | +21% | 3.9 |

Most of the cost is the fill that replaces a laundered magazine, whose search of a fragmented heap
dominates the profile. The finished rules cost churn +2.3% of cycles and nothing with 1k live. A
launder of 254 scattered objects takes 0.9 us on average, 5.5 at worst. A drain from `maintain`
launders what it takes: parked in the depot instead, a simulated quiet heap stays at 73% of its
memory after fourteen intervals, because the slab pops its magazines straight back.

**Worst cases by design.** Anonymous MiB, eight seconds after each event (2026-10-17 to 19, latest
figures, one run each). Here per-CPU caches win: jemalloc's are collected only by their own thread's
activity, so idle threads keep theirs. The last two rows are large blocks, which get no huge pages
when allocated whole.

| Case | cralloc | jemalloc | mimalloc |
|---|---:|---:|---:|
| Eight CPUs use every class, then one uses one | 7 to 9 | 24.7 | 15.8 |
| Two CPUs, 44 classes | 1.2 | 4.1 | 6.2 |
| 16 threads on two CPUs, 44 classes | 1.5 | 52.2 | 28.3 |
| 256 blocks of 1 MiB, one page written each | 2.3 | 2.9 | 276 |
| 4,096 blocks of 64 KiB, one page written each | 32.2 | 33.2 | 344 |

At the moment of a phase change the budget shows: eight CPUs using every class peak at 47 to 55 MiB
against jemalloc's 28.

**What is bounded, by arithmetic from today's constants.** Parked memory depends on CPUs and classes
in use, not on the heap.

| What | Bound |
|---|---|
| One CPU's loaded magazines, all 44 small classes full | 1.95 MiB of objects |
| One CPU's spares and stock | 1 MiB of objects |
| The depot, one domain | 128 KiB a class, 5.5 MiB |
| All CPUs, every class touched | about 94 MiB at 32 CPUs, 750 at 256 |
| Pages pinned by one full magazine of 1,022 objects | 4 MiB, one object a page; 2 GiB in a huge-page part |
| Free and resident | the floor while idle; the cap while growing; fragmentation has no bound |

## After a peak, and in a silence

Free 218 MiB of small objects in random order, then work lightly on one thread. Anonymous MiB
(2026-10-22 at `80643d2`, median of three):

| After freeing | 1.5 s | 4.5 s | 8.5 s | 12 s |
|---|---:|---:|---:|---:|
| cralloc, bursts crossing magazines | 215 | 93 | 33 | 33 |
| jemalloc | 142 | 113 | 4.5 | 4.5 |
| mimalloc | 226 | 226 | 15 | 16 |
| glibc, tcmalloc | 209, 211 | | | 209, 211 |
| cralloc, bursts inside its magazines | 215 | 215 | 215 | 215 |

The 33 MiB is the floor, paid on purpose: a few tens of MiB cost a server nothing that matters, and
returning them costs `madvise` calls and the refaults of its next burst.

**No slow path, no maintenance.** Nothing runs on the fast path. A thread whose steady work
allocates and frees fewer than a magazine's worth of each class per cycle settles into magazines
holding exactly one swing and never leaves them. In the last row's shape 39 of 66 runs held
everything: 207 MiB free in the bitmaps and counted, 1.5 parked. Heartbeats stopped for good after
about 16,000 calls, a second in; runs that returned memory made 110k to 125k. What tips a run was
not pinned down. With the background thread 0 of 6 held; preloaded, with C `free`, 0 of 10. Work
that frees on another thread, grows vectors or touches large blocks enters slow paths all the time
(the screened programs made 36k to 840k heartbeats a run). jemalloc has the same hole for
one-at-a-time reuse, which raises no event: it kept 187 MiB of the burst for good, where cralloc's
thread returned it.

**The background thread** is on by default once free memory is over the floor and the program has a
second thread. A busy process pays nothing measurable: seven `maintain` calls in a run, 12.17 and
12.10 ns an allocation, instructions +0.02%. Idle, a wake-up costs about 35 us; after a burst 340 us
a call, 90% the purge walk, 0.05% of a CPU, and it grows with the bitmaps. It parks when its rounds
find no work and no slow path has run. A program with one thread keeps its memory, as it does under
glibc.

## Returning memory off request paths

A request that returns memory makes every request queued behind it wait. At 25k requests a second on
a worker, a millisecond is about 25 requests, so once a second it is the p99.9.

**Every step on a request's path is bounded, whatever the unit.** A large slot used to be one unit.
A step now returns at most 0.5 MiB (a hold's look 2 MiB), a large slot going back in pieces from its
top, whole huge pages in a class of a huge page or more. DataFusion, five rounds, 2026-10-21:
longest heartbeat 0.98 to 3.0 ms becomes 0.29 to 0.51, longest growth payment 1.5 to 3.4 ms becomes
0.38 to 1.6, peak 842 to 864 MiB becomes 863 to 870 (jemalloc 851 to 883). The bound on growth is
what costs the peak: left for the next carve, peaks reach 872 to 940; owed and paid by heartbeats,
863 to 870. A hold look of 0.5 MiB let two runs in six over jemalloc, so it stays at 2 MiB.

**While the thread runs, the steps are its work.** A slow path records what is due and wakes it if
it sleeps. Total work does not grow (DataFusion's kernel cycles 5.98 G against 6.02). 2026-10-22
against `3fdec1b` and `040033a`, interleaved:

- The log store's slowest request in a burst's first 5 ms: 0.68 to 0.93 ms becomes 0.50 to 0.60.
  What is left is the idle drains. The order service's doublings:
  [large-blocks.md](large-blocks.md).
- DataFusion's peak holds, 0 runs of 20 over 880 MiB. The thread wakes about 440 times a run and
  runs 89 ms, waiting 87 ms for a CPU behind four workers. In the first build 3 runs of 14 peaked
  165 to 235 MiB higher, and 2 of 20 after two other changes. The cause was the hold's plan, which
  counted only stamps not yet held, so a region whose look stopped at its bound waited a whole hold.
  The bound on unpaid growth stays necessary: a thread waiting behind the threads that carve must
  not leave growth unpaid past 16 MiB.
- Rejected: a yield after each step. Each handed a worker a whole slice: the thread waited 462 ms a
  run, and 10 runs of 20 peaked 40 to 180 MiB over, while Meilisearch's tail and the workers' wait
  did not move. Lower priority fails the same way.

Meilisearch is not resolved. No p99.9 regression is shown: medians level (45.4 ms against 45.8),
every server thread's run-queue wait the same. But the spread over six runs is the widest (36 to 53
ms against 42 to 47), and its p99 is 0.7 to 2.8 ms above. The thread wakes 25 times a second at 70%
load instead of 2 and does about 10 ms of work a second, with cycles a request level.

## Commit calls grow with the logarithm of the heap

`mprotect` grew linearly with the heap when a region committed 64 KiB past each carve: 32,000 calls
a run in gix-hours, 2,300 in ruff, 1,600 in tpchgen. Each takes the memory-map lock for writing,
which every page fault in that region needs. In gix-hours the threads spent 2.6 s blocked inside it,
81 us a call against about 1 uncontended; in ruff about 190 of a typical run's 400 context switches
were queued on that lock. A commit now runs as far ahead as the region has committed, between 64 KiB
and 64 MiB. Calls fall three- to forty-fold (478, 740, 500, 510 and 117 on five programs), and no
memory moves. Mapping the whole reservation writable and never calling is level with this. The order
service on actix went from 1,670 calls to 300 and 222 context switches to 138, at level rate and
tail. The batch programs' wall times are bimodal under every allocator, so they rank by kernel
cycles, faults and switches.

## Magazine budget

Chosen: 64 KiB of objects a magazine, depot target 128 KiB. Ten jobs, five rounds, per-round ratios
with a 95% bootstrap interval, 2026-10-17 at `6111f39`.

| | jemalloc | 128 KiB | 64 KiB | 32 KiB | 16 KiB |
|---|---:|---:|---:|---:|---:|
| Mixed pipeline: time against 128 KiB | | | +5.1% [3.6, 6.8] | +11.7% [10.1, 15.9] | +23.3% [21.2, 27.8] |
| Its peak, MiB | 5.24 | 7.07 | 5.12 | 4.00 | 3.54 |
| Its slow paths per 1,000 allocations | | 16.6 | 28.2 | 44.3 | 65.3 |
| mpmc 3 to 1: time | | | +0.4% [-1.0, 3.0] | +6.1% [3.3, 7.4] | +12.9% [10.7, 19.0] |

64 KiB is the one setting at or under jemalloc's memory on the mixed pipeline, and it costs what it
costs only there, about 55 ns per extra slow path, still 30% faster than jemalloc on that job. Heaps
of 100 MiB or more moved under 0.5 MiB at every setting: this is a fixed cost, not a share.
Magazines were since allowed up to 1,024 slots ([magazines.md](magazines.md)), which reaches only
classes to 128 B.

## The huge-page threshold

A small class of at most 1 KiB switches to huge pages at 16 MiB carved. What it costs is the huge
page above each watermark and coarser purging. What it buys is fewer faults and dTLB misses, whose
effect on speed is in [program-effects.md](program-effects.md).

| Cgroup peak, MiB | 16 MiB | 8 | 4 | 2 | First carve | jemalloc |
|---|---:|---:|---:|---:|---:|---:|
| Churn, 1M live | 109.4 | 112.9 | 115.6 | 118.0 | 133.0 | 110.2 |
| BTreeMap 200k | 25.7 | 27.0 | 27.0 | 32.7 | 61.5 | 25.9 |
| Tree, depth 18 | 13.1 | 15.0 | 15.0 | 15.0 | 45.0 | 17.1 |
| JSON 8 MiB | 90.0 | 90.0 | 91.7 | 91.8 | 123.0 | 89.0 |

*2026-10-18 at `016877c`, two rounds that agreed within 0.4 MiB.* Huge pages from the first carve
cost 2 MiB for every class touched: a three-object process holds 20.7 MiB. 4 MiB is the knee, at
most 6 MiB more, confined to heaps with many classes of a few MiB each, and was turned down to stay
under jemalloc on churn and the small maps. The switch is exact: a threshold of N used to leave up
to N + 2 MiB on small pages.

Rejected: `MADV_COLLAPSE` at the switch, 0.1 to 0.4 ms of synchronous work per 2 MiB in a slow path,
adding nothing once the threshold is low. Populate-ahead: faults cut three to five times, kernel
time by at most a tenth, about 1 MiB at every peak.

## Small heaps

The goals say the peak of a heap of a few megabytes is not something to tune. This is what the fixed
cost is. Charged at exit (own pages, page tables, kernel slab objects), cralloc against jemalloc and
glibc, MiB, five rounds, 2026-10-18 at `779ba9b` and after: three allocations 0.47, 0.44, 0.22;
churn with 1k live 0.75, 0.61, 0.38; mpmc 4+4 4.52, 3.16, 0.98; RPC echo 3.42, 1.79, 0.95. Those had
one spare a stack and 256-slot magazines; both have since grown.

- **Page tables are the largest fixed cost.** Regions lie 4 GiB apart, so a class's first page needs
  a PTE and a PMD page of its own, about 8 KiB per class in use. A metadata block 128 MiB from the
  next cost as much again until the small classes' headers moved into the back end's state, with the
  leaf bitmap last. A class whose frees never reach the bitmaps now never touches its block, and one
  that does touches pages in one 2 MiB, under one page of page tables.
- **Parked objects are the whole story of mpmc:** 2.5 MiB loaded, in spares and in the depot against
  0.3 live. A fresh fill costs nothing, since a magazine filled by a carve names objects nobody has
  touched. That kills the idea of a short first magazine.
- **Spares.** A CPU whose traffic in a class goes one way fills a stack it never takes back: about
  0.5 MiB of mpmc 4+4's peak at four spares. One spare cost sh8bench 1.7 to 7.9% over three
  sessions, two 1.2 to 3.5%, four up to 0.9%, so it stays at four. Magazines of at most 128 slots
  reach jemalloc's charge on mpmc but cost 16 to 20% on the pipelines.
- **Moving the headers first cost a pipeline of large objects 4%,** not for the move but for what
  the compiler did: the class's layout was built twice and copied, then 19 more stores and 18 more
  loads a claim once the growth path was inlined. A cold call for the layout and an out-of-line
  growth payment brought it back: 76.6 to 72.3 ns an allocation.

## How the incumbents bound the same things

Read from their sources, not re-measured. jemalloc: one event per 64 KiB allocated or freed visits
one bin, driven only by the owning thread; freed pages decay over 10 s. tcmalloc per CPU: one budget
of 1.5 MiB a CPU, capacity grown on misses; everything that shrinks it runs on a thread the program
must start. mimalloc: a full collect with the purge every 10,000 slow paths; freed arena memory goes
after 4 s, in 2 MiB units. None reclaims an idle cache without help.

## How memory is accounted

- **Peak** is the cgroup's `memory.peak` of the measured process alone, page tables included. On a
  process this small it includes the kernel's per-CPU charge stock and moves by 0.4 MiB between
  identical runs.
- **Where resident memory is.** A sampler thread built with the allocator (feature
  `cralloc-memstat`) walks the regions, bitmaps, slabs and depot with `Cralloc::inspect`, a
  read-only racy walk that never allocates, asks `mincore` which pages are resident, and attributes
  every resident byte to one use. In a drift run it takes an account with every thread stopped, so
  the sums are exact.
- **jemalloc's account** comes from its own statistics at the same moments, in a build with them on.
  Its `stats.resident` counts active extents whether touched or not, so its heap is the process's
  anonymous memory less what lies outside it.
- **The whole process** at exit (feature `memacct`, any allocator): cgroup counters, smaps and
  `pagemap`, each page attributed to a structure.
- **Drift runs** (`workload drift`) count work and samples in operations, so a paused process does
  the same work; only runs on the measurement CPUs give seconds. **Memory shapes** (`workload
  quiet`, `touch`, `sparse`, `tiny`, `period`, `refill`) print the process's memory after a peak, in
  idle caches and behind sparse blocks, and are not ranked.

## Open questions

- **One machine, mostly single runs.** Clock rules act less often per operation in time-compressed
  jobs than in a server at a tenth of their rate, so returning memory is if anything
  under-represented. Memory runs used one L3.
- **Parked memory against CPU count.** Servers were pinned to four CPUs. The bounds above are
  arithmetic.
- **Packing the smallest classes** in a trough. No rule built, and it is the gap to jemalloc.
  Refresh on evidence at 5 and 10 GiB, and a real server in a trough with it, are not measured.
- **Why tcmalloc, mimalloc and glibc do not pack at any turnover,** what tips a quiet burst into
  having no heartbeat, and how a program that swings over hours fares.
- **Meilisearch's p99 under the thread** (above), and what is left of the size shift's gap with the
  tail rule on: it was not accounted again.
- **The threshold at 4 MiB.** It was refused to stay under jemalloc by a few MiB, before the goal
  said fixed MiB are spent freely. Worth measuring again, with its speed gain.
- **Meilisearch's fill.** In the 2026-10-28 standings its heap reaches 885 to 987 MiB in the first 5
  s against jemalloc's 772 to 1,006, while under load it is below jemalloc: at most 710 to 739
  against 778 to 790 (computed from the curves). The fill's excess is not attributed.
