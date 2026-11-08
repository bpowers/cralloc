# Large blocks

What governs blocks past the small classes, those above 32 KiB: how growth, slack, holds and huge
pages decide faults and peak. The rules are in `docs/design.md`, "Large objects" and "Returning
memory and huge pages". The whole heap, and terms such as heartbeat, floor, cap and frontier, are in
[memory.md](memory.md), and the jobs in [workloads.md](workloads.md). A *slot* is one block's place
in a class's region, and to *claim* one is to take its bit in the bitmap. *Advice* is
`MADV_HUGEPAGE`. Measured on a Ryzen 9 9950X (Linux 7.2.9, transparent huge pages `madvise`),
2026-10-17 to 28, but for what a section says is the M1 Max's.

## The answer

- **A large class's region serves one size, so freed memory crosses classes only through the OS.**
  Every incumbent lets memory freed at one size serve another. What jemalloc gets by coalescing,
  cralloc gets by returning and faulting back. So a program whose blocks cycle pays in faults, and a
  program with phases pays in peak.
- **Three things decide faults and peak, with one rule each.** How long free slots stay: a 125 ms
  hold, each slot ageing on its own. What an occupied slot keeps past its block: a trim at the
  claim. What growth leaves behind: a bound of 64 MiB, counted on its own.
- **No single hold time serves all programs.** DataFusion's blocks come back within 60 ms or not at
  all, and a hold of at most about 125 ms keeps its peak; at 250 ms the peak is 230 MiB higher.
  Meilisearch takes its blocks back up to 2 s later, and only half a second or more stopped those
  faults. A hold that adapted and a long hold paid for by growth were both built and lost. The price
  turned out small: most of Meilisearch's excess faults were zeroed buffers it barely wrote and
  outgrown slots returned on a level. With those fixed it takes 42 faults a request, level with
  jemalloc, at less heap under load (697 MiB mean against 749). While indexing, before any load, it
  peaks higher: 1,388 to 1,433 MiB against 1,230 to 1,251.
- **Faults are the cheap side of the trade.** A return and refault costs about 2.4 to 7 kcycles, and
  these rules move tens of MiB of peak for a percent or two of cycles.
- **Where it loses.** tpchgen takes the most faults (337k against 78k to 282k) for a middling peak,
  with cycles inside the runs' spread. HashMap's peak is 204 MiB against jemalloc's 182 to 184.
  Blocks of 4 to 32 MiB under churn hold 1.53 times live against jemalloc's 1.28, which returns
  every free of 8 MiB or more at once.
- **Past 32 KiB nothing is cached, and that is the kernel suite's two largest gaps on the M1 Max.**
  `largepair` is 64 KiB against snmalloc's free list, and the large pipeline four times mimalloc's
  instructions at half the Ryzen's clock. The port and some idle layers had added a sixth of the path,
  now gone. Magazines of at least six objects would take 16% off the pipeline for up to 584 KiB a CPU,
  and caching to 256 KiB would pass the incumbents on both, which the design stands against.
- **The large-buffer cache has no one winner.** Geomean of its phases, the rules beat tcmalloc,
  mimalloc and glibc at every size. Against jemalloc they lose at 0.5 GiB (1.82 against 1.46), lead
  at 1 GiB on one phase (1.41 against 1.52), and tie at 2 and 5 GiB (1.27, 1.19).

| Job | cralloc | jemalloc | tcmalloc | mimalloc | glibc |
|---|---:|---:|---:|---:|---:|
| DataFusion: peak MiB, M faults, Gcycles | 857, 1.09, 70.6 | 861, 1.03, 72.3 | 999, 0.45, 67.6 | 1,659, 0.12, 70.0 | 933, 0.80, 70.5 |
| tpchgen: peak MiB, k faults, Gcycles | 559, 337, 25.3 | 567, 182, 24.8 | 540, 156, 25.0 | 813, 78, 24.6 | 518, 282, 25.2 |
| Meilisearch at 70%: heap mean MiB, faults and Mcycles a request | 697, 42, 16.8 | 749, 42, 17.4 | 980, 0.1, 17.1 | 1,675, 23, 18.0 | 1,191, 1.0, 18.0 |
| Large-buffer cache at 1 GiB: geomean over live, Mcycles an operation | 1.41, 8.3 | 1.52, 13.6 | 2.02, 14.5 | 2.11, 12.8 | 2.05, 10.0 |
| 64 Vecs grown to 4 MiB: Mcycles, peak MiB | 655, 327 | 1,163, 305 | | 636, 266 | 1,169, 269 |
| HashMap 1M: cycles an entry, peak MiB | 561, 204 | 631, 182 | | 432, 199 | 1,103, 208 |

*First three 2026-10-28 at `e3753b5`, medians of three to six rounds. The cache: geomean of four
phases, computed from each phase's end over the cycles, 2026-10-24 and 25; the ladder above at
`536dbe8`. The hold counts time, so this job's peak follows core speed: at 2 GiB 2.03 times live on
the measurement CPUs, 1.39 on shared cores. Kernel jobs 2026-10-18 to 24, sessions that differ by up
to 10%.* *DataFusion* runs TPC-H SF3 on four threads, *tpchgen* writes TPC-H SF1 to Parquet on
eight, *Meilisearch* serves 930 requests a second. The *large-buffer cache* is 1 GiB of buffers on
eight threads, log-uniform in 40 to 256 KiB, 256 KiB to 4 MiB, 4 to 32 MiB, then all of that; half
are built by doubling with `realloc`.

What each rule trades:

| Rule | Job | Peak | Faults | Cycles |
|---|---|---:|---:|---:|
| The hold | DataFusion | about -350 MiB | +100k | +0.9% |
| Trim and ageing | tpchgen | -97 MiB | +122k | +2.0% |
| | DataFusion | -13 MiB | +85k | +0.8% |
| | Large-buffer cache | 1.89 to 1.41 times live | 2.4 times | level |
| Keep outgrown slots to 64 MiB | tpchgen | +55 MiB | -488k | -4.5% |
| | 64 Vecs | +49 MiB | -37k | -15% |
| Unwritten zeroed blocks go back | Meilisearch | -130 MiB mean | -42 a request | level |

*A fault's price is the change in cycles over the change in faults, on the rows where both resolve.*

## Free slots: a hold, and why no hold serves everyone

**The gap it closes.** At DataFusion's peak without a hold (2026-10-20 at `3ec3423`) the program
held 659 MiB and 421 more sat free, 420 of it on wholly free pages: 144 MiB in the 160 KiB class,
137 in 20 MiB, 69 in 10 MiB. Earlier queries freed those slots and the next query's blocks fell in
other classes. The clocked pass, a second to a query's 150 ms, was too slow. jemalloc holds a
similar 334 MiB dirty, but any size can reuse it: it wins by holding less, not by packing tighter.

**The rule.** A free that leaves a slot resident stamps its region with a fine count, a step per 2
ms that a heartbeat observes. Once a region has had no such free for 64 steps, about 125 ms, its
free slots go back in bounded looks while free large memory is over the 32 MiB floor, and only down
to it. A silence ages a hold by one step.

| Hold | DataFusion: peak MiB | DataFusion: faults | Meilisearch: faults a request | Meilisearch: heap swing, MiB |
|---|---:|---:|---:|---:|
| None | 1,204 to 1,252 | 0.90 to 0.96M | 34 to 39 | 14 to 41 |
| 60 ms | 845 to 862 | 1.08 to 1.10M | | |
| 125 ms | 841 to 843 | 1.04 to 1.06M | 71 to 88 | 49 to 172 |
| 250 ms | 1,070 to 1,081 | 0.95 to 0.98M | | |
| 0.5 s | 1,095 to 1,233 | | 40 to 43 | 57 to 60 |
| 2 s | 1,053 to 1,244 | | 36 to 37 | 9 to 27 |
| jemalloc | 849 to 879 | 1.02 to 1.04M | 38 | 31 |

*2026-10-20 and 21 at `3ed79ce` and `a37a3ac`, five and two rounds. Meilisearch's faults predate the
two fixes below.*

The programs differ in when reuse comes. In tpchgen, gix-hours and DataFusion, 86 to 100% of what
came back after a purge did so within 60 ms. In Meilisearch the indexer's 8 to 32 MiB buffers come
back a batch later: over one run 3.6 GB at 64 to 127 ms, 2.0 to 255 ms, 2.9 to 0.5 s, 3.6 to 1 s,
3.7 to 2 s. The free's stamp costs a pipeline of large objects 0.4%. A look once returned one slot
of 34 MiB in about a millisecond, which is why slots now go back in pieces ([memory.md](memory.md),
"Returning memory off request paths").

**Where Meilisearch's faults came from** (2026-10-24 at `4e55114`, counted by the rule that last
purged a slot): the `memset` of 50 MiB zeroed buffers over pages the hold returned, 23 a request;
outgrown slots a move returned and claimed again within 64 ms, 13; slots of 5 to 20 MiB the hold
returned and claimed 0.1 to 1 s later, 13, which jemalloc refaults too; about 30 others, not split.
The first two are gone (below).

**Tried and rejected.**

- **A hold that adapts.** Per class, doubling on a claim soon after the hold purged and halving on
  the clock: the same peak for 9% more faults than 60 ms. Per region it halved Meilisearch's faults,
  moved neither its time nor its tail, and cost DataFusion a tenth of its peak.
- **A long hold under which all growth pays** (2 s, growth evicts). With carves, refaults and growth
  in place all charged, DataFusion peaked at 846 to 930 MiB against 845 to 868, within the cap, but
  its faults fell only 2%, and Meilisearch's stopped at 45 (a plain 2 s hold 36), since each charged
  refault evicts a block the indexer is about to take back. Not charging refaults gives 33 to 40 and
  DataFusion 1,044 to 1,073. One choice cannot serve both. (Skipping young regions first left
  payments nothing to take, since any free restamps a region: 3.7% of cycles on empty walks.)
- **Growth over the cap purging only above the frontiers.** Sparing the slots climbing buffers take
  next cut DataFusion's faults from 249k to 162k, but pulldown-cmark went from 541 to 664 MiB
  against jemalloc's 543: nothing would take its input buffers.
- **Claims preferring resident slots:** a less-returned free slot of the class existed for only 10
  to 16% of the bytes faulted back. **Populating a moved block's destination:** tpchgen's faults
  -6%, kernel cycles level.

The retained count overcounts large free memory, since a block is charged what its extent covered,
written or not: 756 MiB/s against 646 resident on Meilisearch. A payment then partly returns pages
that are not there. It has not been worth a mechanism.

## Each free slot ages on its own

A region's clock cannot tell a slot idle for minutes from one freed a moment ago. Under steady churn
every region is freed into within any hold, and 960 to 1,090 MiB of free large slots stood resident
in the cache's third and fourth phases. So the free also stamps the slot, in its size word, and a
look at a young region walks its free slots and returns those a hold old, at most once a hold.

Free large slots resident over the run, three runs each: before, mean 564 to 602 MiB, maximum 1,112
to 1,157; with it, mean 279 to 300, maximum 657 to 673. What stays is a hold's worth of churn: the
third phase frees about 15 GB/s, 1.9 GB in a hold, and about 500 MiB stays, many slots taken again
within it. That is what the hold was built to keep, and why jemalloc ends that phase lower. Ageing
is level or better on the regression jobs.

## Slack: what earlier tenants left in an occupied slot

A purge claims a slot first, so it cannot reach an occupied one. A claim used to settle only the
pages under its new block, so the pages past it that earlier tenants wrote stayed resident, in no
count, until the next free. At the end of the cache's first phase that was 787 MiB in 7,179 slots,
76% of it a larger block moved in with room, then a smaller block moved in with room.

- **It is warm room the next tenant mostly does not use.** A moved tenant grows into 12 to 14% of
  it, a whole allocation into almost none (9 MB of 108 GB).
- **It ratchets inside a slot.** What is resident is the high-water mark of every tenant since the
  last purge. With one buffer in a hundred ten times larger the first phase climbed 2.48, 2.80,
  2.97, 3.04 over four cycles, and held 2.15 after a quiet minute.

**The rule.** The claim returns what lies past both its new block and the last tenant's final size,
which the free left in the size word: one step, from the top. The last tenant's room is kept, since
the next may need it. It takes no new number.

| Resident over live, by phase | 40 to 256 KiB | 256 KiB to 4 MiB | 4 to 32 MiB | Mixed | Faults, millions |
|---|---:|---:|---:|---:|---:|
| Neither rule | 2.14 | 1.52 | 1.93 | 2.04 | 9.1 to 9.9 |
| Prototype: ageing alone | 2.22 | 1.45 | 1.74 | 1.79 | 14.7 |
| Prototype: trim alone | 1.32 | 1.49 | 1.97 | 2.14 | 13 |
| Prototype: both | 1.33 | 1.40 | 1.53 | 1.77 | |
| Both, as built | 1.30 | 1.27 | 1.53 | 1.56 | 22.6 to 24.2 |
| jemalloc | 1.47 | 2.12 | 1.28 | 1.36 | 410 |
| glibc | 1.45 | 1.85 | 2.63 | 2.49 | 62 |

*2026-10-25. The prototype, at `5b316cb`, is one run each, and its ageing walk did not carry on
across looks; ageing alone was not run on what shipped. The other rows are means over phase ends of
three runs or four cycles at `26199ba`, computed (standard deviation as built 0.01, 0.05, 0.10,
0.13). A phase moves by up to 0.3 between runs of one code.* The big-buffer climb stops at 1.36 to
1.41. Every build takes 8.2 to 8.4 Mcycles an operation, since the job writes every buffer it makes.

**A step, not the whole.** Slack can be most of a slot of tens of megabytes. Returned at once, trims
of 1,024 pages or more stalled a claim 0.8 ms on average and up to 3 ms. The step is 128 pages, the
bound of a step on a request's path: p99.9 250 to 500 us, for 0.02 to 0.06 of a phase's ratio. Even
a 128-page `madvise` is sometimes stalled 2 to 6 ms, the call waiting whatever its size.

**The price is faults where a later tenant wants the room.** tpchgen +60%, +2.0% cycles, 97 MiB less
peak; only 12% of what the trim returned was faulted back by the tenant that trimmed it, and the
rest is later, larger tenants. DataFusion +8%, +0.8%, 13 MiB less. Meilisearch +12% a request,
+0.6%, tails and heap level.

Rejected: a bound on slack at an eighth of live large memory (the 64 Vecs +18%, since it trims the
room their regrowth uses); a bound at 64 MiB (works, but the trim does as well with no number);
trimming past the block's own size (first phase 1.15, three times the faults).

A silence does not trim. After a quiet minute 1.10 to 1.15 times live stays (no rule 1.13 to 1.19,
the same within spread): the floor's 30 MiB and 100 to 120 MiB of slack, waiting for a claim.

## Growth: moving with room

An object that outgrows its slot moves into one with room: sixteen times its size in a heap with its
full reservation where that slot is at most a sixteenth of a region, so to about 16 MiB; else four
times where that fits a region; else none.

**Bytes copied.** A doubling buffer lands in the class above H times its size plus the stagger's
room, 4 KiB on x86-64. That class is 1.25 H times the size, so for H = 2^k it moves once every k + 1 doublings and
copies about 1/(k + 1) of its final size, averaged over final sizes. Any one final size sees a
sawtooth: right after a move it has just copied half of itself.

| Room | Mean copied over final size | Loops of 1 to 64 MiB, geomean | 64 buffers of 4 MiB, committed |
|---:|---:|---:|---:|
| 4 times | 0.325 | | 1.46 GiB |
| 8 times | 0.274 | +2.7% | 0.68 GiB |
| 16 times | 0.208 | -10.7% | 2.64 GiB |
| 32 times | 0.166 | -14.5% | 10.4 GiB |

*2026-10-18 at `41987b6`, five interleaved rounds, medians.* Half the cycles of a buffer grown to 4
MiB and freed were `memmove`. Against 4 times, 16 takes 28 to 34% off the 1 and 4 MiB loops and one
vector to 256 MiB, and 6% off the 64 Vecs. It loses where it must: the 64 MiB loop copies 33 MiB a
repetition against 18, 35% slower. Gains fall with the logarithm of the factor and costs grow with
the factor, so sixteen and not more.

**Room costs address space and commit charge, not memory.** A slot is charged by its pages that are
not clean, and a clean bit is true: not written since carved or last purged. Room nobody wrote is
never charged, kept or purged. Under strict overcommit a buffer is charged the whole slot it ends
in, up to 20 times its size: under a 2 GiB `RLIMIT_DATA`, 789 buffers of 1 MiB where four times
fitted 396, but 28 of 4 MiB against 394. A failed commit falls back to less room.

**Regions decide the limits.** A region is never taken back from a class, so room past a region once
took regions until `malloc(256)` failed under an address-space limit. So sixteen times is given only
where a region holds sixteen or more such slots, and a heap short of address space does not trade
any for speed: at every limit from 16 to 256 GiB the buffers that fit equal four times' count. In
the full reservation it takes at least 770 GiB of live buffers to run out. Such a heap is a
different allocator: one session under `prlimit --as` got 2 MiB regions and measured that, with
nothing to say so, and the runner now refuses a finite limit.

**Shrinking.** A block that shrinks under a thirty-second of a small slot of a full heap, or an
eighth elsewhere, moves: half of what a block fills when it moves in, so one resized back and forth
across a slot's edge settles after one move.

**The grow loop's 40%.** A 4 MiB grow-and-free loop read 40% slower after an unrelated edit to the
benchmark, at the same instructions and the same allocator. It was where the benchmark's `Vec::push`
loop landed, and other allocators' builds moved too, both ways. So that job no longer pushes, and
grow-loop numbers from different builds do not compare ([measuring.md](measuring.md)).

## What growth leaves behind

A buffer grown and freed again and again should find its memory resident. Many buffers growing at
once should not leave a trail of what they outgrew.

**A budget set against all free large memory is always spent.** The hold keeps free large memory
above its floor by design: 251 MiB at tpchgen's peak, 391 on Meilisearch. So a 16 MiB budget
returned every outgrown slot and the program faulted it back within 32 ms: about 600k of tpchgen's
705k faults, 6.4 GiB a run on Meilisearch, all of the 64 Vecs' gap to mimalloc. The slots actually
kept and not yet taken again never passed 3 MiB on Meilisearch and 42 on tpchgen.

**The rule.** An outgrown slot stays resident while such slots, counted on their own, total at most
64 MiB, and goes back behind the copy past it. A free of a grown block is an ordinary free, unless
it carries huge-page advice, when it goes back whole.

| Against the budget it replaced | Before | Rule |
|---|---:|---:|
| 64 Vecs to 4 MiB: Mcycles, faults, peak MiB | 774, 56.4k, 278 | 655, 19.4k, 327 |
| tpchgen: Gcycles, faults, peak MiB (glibc 25.2, 291k, 515) | 25.84, 706k, 578 | 24.67, 218k, 633 |
| Meilisearch: faults a request | 48 to 53 | 36 to 40 |
| DataFusion: Gcycles, faults, peak MiB | 69.80, 1,030k, 863 | 70.09, 1,025k, 864 |

*2026-10-24 at `64a31f3` against `1b0acf0`, three to six rounds, medians.* The order service and the
log store are level. The 64 Vecs gain half of what a prototype did (-31%): the rest was keeping the
advised slots of its drops, which what shipped returns.

**The trail.** 256 buffers grown round-robin to 4 MiB beside 1 GiB of ballast, freed and collected
each round, so the next round's moves land in returned slots and pay nothing toward the cap. Peak
during growth less what is live: 25 MiB under the old budget; 540 keeping every slot; 71.5 with the
bound, which it never exceeds by more than a slot. The count does not drift, since every decrement
is causally after the increment it answers.

**A slot advised for huge pages is not kept.** Kept with its advice taken back, a buffer regrown to
256 MiB ran 10 to 12.6% slower, and to 64 MiB 31% slower with 32 MiB more peak. User cycles in the
program's own `memset` rose 55M while kernel cycles fell 22M: writing over a resident, cold slot
costs more than faulting a fresh huge page, whose clearing leaves the lines in cache.

Rejected: one count for slots and grown frees (the 64 Vecs' drops fill it, 660 against 534 Mcycles);
charging frees by `mincore` (the over-count is not what fills the budget, and a system call at every
free costs a 1 MiB loop 22%).

## Huge pages only for blocks that prove dense

**Grown blocks.** A block that at most doubles, past 2 MiB, with the last 2 MiB it wrote at least
half full of data, is being appended to, and is advised up to the size asked for. A Vec grown to 64
MiB went from 10.95 to 4.06 cycles an element and 41,609 to 1,245 faults, ahead of mimalloc's 5.82
at half its memory.

**Sparse blocks stay sparse.** A move skips all-zero source pages. malloc N, one byte a MiB, realloc
to 4N, resident MiB at N = 256 MiB: cralloc 2, glibc 2, jemalloc 259, mimalloc 260. Four threads
growing to 256 MiB, written a stripe a MiB past 64 MiB: 1.0 GiB with the whole slot advised, 430 to
454 MiB with the density test, 267 to 283 with no huge pages, glibc 310 to 345.

**The spilled huge page.** Large blocks start a little way into their slots, so that they do not
share cache sets ([program-effects.md](program-effects.md)), and a block of whole huge pages ends a
few bytes into the next one, or on aarch64 up to the stagger's pages. Advice rounds down, so those bytes fault a small page, and that page
table keeps the huge page small when the block grows over it: 504 of about 722 small faults a vector
per repetition on the 64 Vecs. So an in-place growth over a page the block only started asks
`mincore` whether the rest is resident, and if not copies the page aside, returns the huge page,
which frees its emptied page table, and copies back. Filling a huge page, in TSC ticks per 2 MiB:
small faults about 1,170k; `MADV_COLLAPSE` about 510k, and it queues work on every CPU; copy aside
and back 376k; a fresh page 357k to 389k. The 64 Vecs went from 5.69 to 3.97 cycles an element and
185k to 56.5k faults, peak level. Advising the spill page ahead instead is 24.5% faster but costs
126 MiB of peak.

**Blocks allocated whole get none: a documented limit.** A run-time switch advising every whole
block of 2 MiB or more (2026-10-24 at `1b0acf0`) showed gains on batch programs and none on servers
([program-effects.md](program-effects.md)). On memory a wrong guess has no ceiling but the block:
four blocks of 64 MiB with one store in 1/64 of the pages hold 256 MiB against 16, and each life
costs 12,373k TSC ticks against 631k. A block written whole and reused pays 25 times, mostly because an
advised slot goes back at its free. With 130 free 2 MiB blocks on the host a life slowed up to 17
times in direct compaction, and khugepaged fills in any advised range with one page present.
`MADV_COLLAPSE` on a sparse range fills every page. Evidence that could separate them, none built:
the request's shape (not zeroed), which misses a `Vec::with_capacity` far past its use; a per-class
hint learned at the free, which misses a class's first block; a look by the background thread, which
needs a second writer on a slot's size word.

**HashMap's peak** is its outgrown tables. hashbrown never reallocs, so no growth rule acts on it:
about 33 MiB of smaller tables are resident at the last resize, under the cap, where jemalloc reuses
that memory. Advising whole blocks closes both gaps (547 to 467 cycles an entry, 206 to 181 MiB), at
the cost above.

## Zeroed blocks: evidence at the free

Meilisearch's indexer asks for four buffers of about 50 MiB with `alloc_zeroed`, 7.7 a second, and
writes about 0.3% of each. cralloc cleared all of each one four times in five, which made every page
resident. jemalloc purges every free of 8 MiB or more at once, so its next such block is fresh and
uncleared; glibc is as lazy from 32 MiB and mimalloc after a second idle, and tcmalloc always
clears. (About 587 MiB of the process is glibc's arenas, which its C code fills, under either.)

**The rule.** A zeroed request of at least 16 MiB answered from a slot known to be zero is marked.
Its free asks `mincore` about the middle page of each sixteenth, and if at most one is resident the
slot goes back whole, known zero, uncounted, no hold. A block cleared with `memset` is never marked,
so a tenant that writes what it is given pays what it always did.

| Two minutes at 70%, three rounds | Before | Rule | Returned at 8 MiB and up | jemalloc |
|---|---:|---:|---:|---:|
| Heap mean, MiB | 853 to 860 | 718 to 733 | 716 to 718 | 757 |
| Faults a request | 92 to 96 | 51 to 53 | 51 to 54 | 42 |
| Mcycles a request | 16.85 to 17.16 | 16.37 to 17.01 | 16.85 to 16.88 | 17.79 |

*2026-10-24 at `ca08d3d`, in one binary.* DataFusion, the order service and the log store are level.

- **The floor is 16 MiB.** Sixteen `mincore` calls at about 650 TSC ticks and a `madvise` are about
  12k ticks: 6% of an 8 MiB life with a sixteenth written, 2% at 16 MiB, 0.4% at 64.
- **The cut is one in sixteen.** A fault costs about thirteen times clearing a resident page (1.35
  against 0.1 k TSC ticks), so a tenant that writes more than a thirteenth is cheaper kept and cleared.
  A dense run that meets one sample can reach about an eighth, and its next life costs about 1.6
  times.
- **The look sees pages touched, not written.** A page only read maps the shared zero page, and
  `mincore` counts it, so a block read widely and written sparsely is freed as before.
- **Rejected: returning the pages at the claim above a fixed size.** A tenant that writes all of its
  block pays a fault a page, 12 to 14 times slower from 1 to 16 MiB. Asking `mincore` first bounds
  that but costs 25 to 37% where it should win.

## A stagger of many pages

**A block on aarch64 also moves by whole pages (`docs/research/machine-aarch64.md`, "Blocks written in
turn"), and that costs address space and no memory, because the pages it passes over stay clean.** Up
to 516 KiB of a slot lies below its block on a 16 KiB kernel. Those pages were never resident, but a
free slot is charged the pages not marked clean, and the rule that cleared bits from the slot's start
would have charged them, against the floor, the cap and the hold. So a claim clears from the block's
first page, a carve marks those below clean, and only a tenant at the slot's start, at its claim and
whenever it grows back over them, or a slot under advice, whose huge fault does make them resident,
clears them. A block aligned past 64 bytes starts
at the slot's start, and its extent still counts from the stagger, so it may be charged up to the
stagger's pages it did not write; a slot whose tenants mix the two can hold that much resident
below a staggered block until its free. A move that returns a slot whole returns those pages too,
which an earlier tenant may have written, or the slot could not say it reads as zero.

26% of request sizes from 33 KiB to 2 GiB land one class higher for the room, and none two. Commit
charge rises only there.

## Moving a block

A moving block copies in 2 MiB pieces, each piece's pages returned behind it unless the slot is
kept, so both copies are resident for one piece. One block to 256 MiB by `realloc` and back, four
threads, peak resident: glibc 1,008 MiB, one whole copy 1,178, pieces 1,011. Only a piece with
all-zero pages purges the destination, under their span.

The copy is glibc's `memcpy`, one call a piece, 15% faster than a call a page when the copy faults
the destination in. Non-temporal stores gain nothing there, since the kernel has just cleared the
page, and `mremap` splits the heap's mapping in ways that take several more rules to undo. One
vector to 256 MiB is level with the mimalloc crate: it keeps what is freed, 267 faults at twice the
peak; cralloc returns it, 3,008 faults and a quarter less user time.

## What one large allocation and free cost

**On the M1 Max a large block costs about 85 ns and 1,340 instructions an allocation and free, at any
size from 40 KiB to 1 MiB, and the kernel suite's two largest gaps are that path against a cache. The
machine does not charge more for it: the path takes about the cycles it took on the Ryzen. The port and
some idle layers had added a sixth of them, and that is gone.** cralloc caches nothing past 32 KiB. By
their sources snmalloc caches to 64 KiB per thread, tcmalloc to 256 KiB per CPU and mimalloc to 512 KiB
per thread.

| Alone, ns an allocation and free | 40 KiB | 64 KiB | 128 KiB | 256 KiB | 1 MiB | `largepair`: 64 KiB and 1 MiB |
|---|---:|---:|---:|---:|---:|---:|
| cralloc | 81.7 | 83.8 | 84.1 | 86.2 | 87.1 | 85.6 |
| Before these fixes | 99.8 | 100.8 | 101.0 | 103.7 | 105.3 | 102.7 |
| snmalloc | 4.9 | 5.6 | 30.5 | 44.0 | 96.6 | 51.1 |
| tcmalloc, per CPU | 14.2 | 19.1 | 31.9 | 56.3 | 170 | 94.8 |
| mimalloc | 19.0 | 19.0 | 19.0 | 19.0 | 183 | 113 (109 to 120) |

*2026-11-08, `e926817` against `8e5acdf`, CPU 8, five interleaved rounds, medians, ranges under 1% but
where stated. `largepair` with `sizes=`.*

- **`largepair`'s gap is 64 KiB against a free list.** Of the 51.6 ns between cralloc and snmalloc
  before the fixes, 64 KiB held 92% and 1 MiB 8%. At 1 MiB, where snmalloc has no such cache, cralloc
  now leads everyone. The Ryzen's table could not show this, since its snmalloc was a Debug build
  ([measuring.md](measuring.md), "An incumbent may not be what you think").
- **In cycles it is the Ryzen's path.** There it took about 284 cycles (49.9 ns at 5.7 GHz, `c7f5c21`),
  here 312 before the fixes and 260 after. The Ryzen's last count of its instructions is 1,290 at
  `bff181b`, before the claim's trim and the free's stamp.
- **What the port had added.** The page size is found at run time on aarch64, so each look at a plan
  read it again: a relaxed load no compiler merges, a test for zero and a choice among three geometries.
  Single-stepping put 256 of the path's 1,780 instructions there, where x86-64 folds them to a constant,
  and a `Space` that keeps the page shift it read when it was made takes 12% off `largepair`. The timer's
  rate was read at each clock read, and `cntfrq_el0` takes 8.5 cycles to `cntvct_el0`'s one: keeping the
  shift takes 1% more. The page stagger stays, at about 4% (81.9 ns with the term compiled out).
- **16 KiB pages cost nothing here.** The same binary under a 4 KiB kernel, by the clock, takes 90.8 ns
  at 64 KiB against 88.8, 101.7 at 1 MiB against 92.3 and 155.7 at 4 MiB against 109.4, since a claim and
  a free walk a slot's clean words, and a page of 16 KiB means a quarter of them (`muvm`, no PMU, a build
  before the timer's shift, the fastest of six in each of two rounds; a third read 5% higher in the VM).
- **Layers that did nothing:** a plan built for the layout's memo, which needs one only at a class's
  first call; `place` out of line, with its plan written to memory a byte at a time; the class worked
  out of a layout already known to be large; registers saved for a beat's chores on the calls that only
  count; a slot's stagger worked out three times a claim.

| `largepair`, exact instructions a pair | Before | Now |
|---|---:|---:|
| From the request to its class (`alloc_other`, `place`) | 222 | 131 |
| The claim: layout, tree, clean words, frontier, size word | 684 | 576 |
| The free: layout, slot, charge, stamp, release, tally | 668 | 460 |
| The heartbeat's count, two calls | 104 | 65 |
| Chores of the one call in sixteen that beats | 67 | 71 |
| The job's own loop | 35 | 35 |

*Single-stepped with ptrace, the thread's `rseq_cs` cleared at each stop so that no sequence aborts
([measuring.md](measuring.md), "Exact instruction counts on aarch64"). A stepped thread meets the clock's
chores far more often, so that row is what the hardware counter's total leaves. Sampling on instructions
misattributes by up to three times at a function's level on this core: `slot_in` read 6.3% for 2.1%.*

Of what is left the clean words take 279 and the walk up the summary levels 120. The floor for this
design is about 300.

**Over five placements** (2026-11-08, three rounds, mean over pads, with the range): `largepair` went
from 103.3 (102.6 to 103.8) to 86.1 (85.7 to 86.7) ns and from 313 to 261 cycles; `pipeline t2 sizes=large`
from 112.9 (112.7 to 113.2) to 104.9 (104.2 to 105.5). As controls the 64-byte pipeline went from 4.30 to
4.25, overlapping, and `tree depth=18` from 28.81 to 28.53, the 1% the same loads cost its slow paths.

**Relaxing the retained tallies buys nothing** (85.6 against 85.5 ns), though `ldaddal` repeats at 22
cycles and `ldadd` at 7: at 60 cycles apart the repeat does not bind.

### Across two threads

**`pipeline t2 sizes=large` takes the cycles it took on the Ryzen, at half the clock, and mimalloc, which
waits on memory, did not slow down. Lines do not separate them; four times the instructions do. Caching
would close the gap, at a price in parked memory.** Over both threads cralloc took 684 cycles an object,
700 to 711 on the Ryzen, so 113 ns against 60.6 is the clock's 1.88. mimalloc took about 475 cycles
there and 276 here, 41.7 and 45.7 ns, at 1.6 instructions a cycle. L1 load and store misses an object are
6.6 and 3.2 against mimalloc's 6.3 and 2.8, and its instructions 440 against 1,829. The allocating thread
sets the pace: it spends 45% of its cycles on the large path, 25% on the job's own first write and random
sizes, 19% on exchanges and 11% on chores, and the freeing thread spins 36% of its own.

Half the job's objects are large, a quarter fall in classes of 12 to 32 KiB, whose magazines hold two
objects, and the rest in deeper ones. Exact instructions an object, both threads, at `8e5acdf`: the job's
own 173 (in mimalloc's 440 too), the large path 858, exchanges 471, chores 255. A magazine of two is
worse than no cache by this count: 24 KiB alone takes 88 ns and 1,960 instructions, every second object
paying an exchange and a beat on each thread, where 64 KiB takes 100 ns and 1,413. A magazine of six takes
31 ns and 723.

Each cause, one scratch build at a time. Single builds, five interleaved rounds, medians, ranges under 1%,
so a difference under about 2% says nothing:

| Against `e926817` | Pipeline, ns | `largepair`, ns | Instructions, pipeline | Job's peak, MiB |
|---|---:|---:|---:|---:|
| `8e5acdf` | 112.7 | 102.7 | 1,829 | 9.1 |
| `e926817` | 104.4 | 85.5 | 1,568 | 8.8 |
| No trim at the claim | 95.7 | | 1,484 | 9.0 |
| Chores on one heartbeat in eight | 99.5 | | 1,430 | 9.5 |
| No stamp at the free | 103.8 | 84.6 | 1,567 | 9.0 |
| No page stagger | 103.6 | 81.9 | 1,554 | 8.8 |
| Magazines of at least 8 slots, so six objects | 87.5 | 85.5 | 1,226 | 9.5 |
| A magazine budget of 256 KiB in place of 64 | 81.3 | 85.5 | 1,085 | 11.4 |
| At least 8 slots, and classes to 64 KiB | 72.7 | 44.8 | 1,100 | 10.7 |
| At least 8 slots, and classes to 256 KiB | 39.8 | 44.1 | 796 | 12.1 |
| mimalloc | 45.7 | 113 | 440 | 67 |
| tcmalloc, per CPU | 60.3 | 94.8 | 844 | 12.8 |
| snmalloc | 145 | 51.1 | 2,585 | 57 |

*Peak is the process's high-water mark, which moves by up to 0.5 MiB between runs.*

So of the 67 ns between cralloc and mimalloc before the fixes, the port's code and the layers held 8 ns,
12%; magazines of two objects 17, 25%; and the uncached path past 32 KiB 48, 71%, with the trim's 9 in it.
That passes the gap by 6 ns, since the cached build beats mimalloc.

- **The trim** is one `madvise` to 13 large allocations here, of pages this job never wrote. It stays: on
  the large-buffer cache's phase of these sizes it takes 2.14 times live to 1.32 ("Slack").
- **The free's stamp** moves the size word's line a second time: 14% of the job at 64 KiB alone (86 ns
  against 100), and under 1% in the mix, whose slots lie on more lines.
- **Crossing threads adds about 50 ns to the allocating thread at 64 KiB:** one thread takes 84 ns for
  both halves and the pipeline 100 for one, as the leaf word, the size word and the block's first line
  each change hands twice an object. This is reasoned from those figures and from sampled misses, which
  fall on those words and have a skid. On the Ryzen, taking the leaf's line exclusive first cost 16
  cycles alone and gained nothing; separate lines for a large class's leaf bitmap, and tallies 128 bytes
  apart, changed nothing.
- **The retained count is two tallies that only grow, on lines of their own.** As one word its line
  went to the other thread twice per large object, and every heartbeat took it to see if a pass
  could start: 0.7 of the 0.9 lines an allocation saved. A heartbeat now reads the clock first and
  the tallies only when a pass is due.

**What caching would cost, and why it is not built.** Magazines of at least 8 slots hold six objects where
the classes of 12 to 32 KiB hold two: up to 584 KiB more a CPU, and up to twice that in a domain's stacks.
Caching to 256 KiB is up to 8.7 MiB a CPU in loaded magazines and 17 MiB a domain in stacks, if every
class is warm and every block written, against 2.8 and 5.6 for all the classes today. The design stands
against the second: a large block's class comes from its address, because it may sit in a slot with room
to grow, and a magazine's comes from the layout; a block in a magazine has no stagger, which keeps buffers
written in turn apart; and the C routines' limit of 32 KiB and the slab's layout are in the Lean proofs. The
scratch builds took those limits off for the Rust path alone, so they give what it would buy and show
nothing about those costs. No profiled server makes such a flow. The floor of 8 slots needs one constant
and is the one to try, after the server suite and the memory jobs, since the budget it passes was halved for
memory ([memory.md](memory.md), "Magazine budget"). On the Ryzen it took this job from 656 cycles to 530.

## A doubling's tail is the purge that falls on it

The order service's 64 shard tables double under load, a copy of about 0.8 ms with 545 faults, on
one CPU throughout. Median, p90 and maximum in ms (2026-10-22): slow paths purge 0.766, 0.839,
0.905; the background thread purges 0.770, 0.794, 0.889; jemalloc 0.737, 0.828, 0.860. A heartbeat
on one of the doubling's two large calls ran a step of the pass and a hold look, 36 to 180 us of
`madvise`. So returning memory became the thread's work ([memory.md](memory.md), "Returning memory
off request paths").

Not the cause: `mprotect` (8 commits of 9 to 14 us a run; 174 and 337 us was strace stopping the
process) and direct compaction (none within 5 ms of a doubling). Rejected: huge pages for the new
table, two faults instead of 545, but mimalloc's doubling reaches 22 ms compacting; populating,
which saves the trap and not the work.

## How the incumbents grow

Read from their sources. jemalloc grows in place into a free neighbour; 8 MiB or more goes back at
once. mimalloc never grows in place, and freed pages go to an arena any size reuses, with huge pages
when free 2 MiB blocks exist. tcmalloc copies on every realloc, and got no huge pages on this host.
glibc remaps large chunks and unmaps them at free.

## Open questions

- **About 30 faults a request on Meilisearch** are not split by source, and its burst after a
  silence is bimodal in every build, as an indexing batch lands in it or not.
- **tpchgen's extra faults.** Whether a count of refaults by the trim's own returns would find some
  to save.
- **One class serving another** would remove the cause of the hold, and is a large change to the
  address space.
- **A whole block kept resident at its free** was not measured, and would remove most of the reuse
  cost of advising it. Nor was a region-level mark for dense large classes below a huge page, such
  as the order service's 1.25 MiB table shards.
- **The median of a doubling**, 2 to 4% over jemalloc, inside the run-to-run spread.
- **Meilisearch while indexing** peaks over jemalloc ([memory.md](memory.md)), not attributed.
- **Magazines of at least 8 slots on real programs.** Measured on the pipelines alone: neither the
  servers nor the memory jobs have run them.
- **The Ryzen at today's code.** Its instruction count and its gap to a Release snmalloc are not known;
  nothing here could run it.
