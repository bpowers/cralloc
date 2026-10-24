# Grown slots: keeping what growing buffers leave behind

2026-10-24, main `d3a28c8`, on the reference host (Ryzen 9950X, Linux 7.2.9, transparent huge
pages on `madvise`). Measured with a run-time switch in one binary; the switch is a probe, not
product code (`bench/probes/grown-slots/`).

## What matters

- **The budget for kept grown blocks is spent on every heap that matters, and two jobs pay
  for it.** It is checked against all free large-slot memory, which the hold keeps above
  32 MiB by design. So a move's outgrown slot, and the free of a grown block, return memory
  that the program faults back within 32 ms. On tpchgen that is about 600k of 705k page faults.
  On Meilisearch it is 6.4 GiB a run, about 17 of 52 faults a request. On 64 Vecs grown to
  4 MiB it is the whole of the gap to mimalloc.
- **HashMap growth's gap is not this.** hashbrown never reallocs: it allocates a new table
  and frees the old one, as plain frees that stay resident. Every rule gives the same faults
  and time. Of the gap to mimalloc, 24% is first-touch faults. The other 76% is dTLB misses on
  the table, a 64 MiB block allocated whole, which gets no huge pages. Details are below.
- **The rule I rank first:** a grown block's free keeps its slot, as any other free does. A
  move's outgrown slot is kept while the outgrown slots kept and not yet taken again total at
  most 64 MiB, counted on their own. It needs one bit in the slot's size word and one counter.
- **What it buys, against main:**
  - 64-Vec: 31% fewer cycles, now 16% ahead of mimalloc.
  - tpchgen: 5% fewer cycles and 69% fewer faults, level with glibc on cycles.
  - Meilisearch: 1% fewer cycles and a third fewer faults a request, at the same heap.
  - DataFusion: level, with an 11 MiB lower peak.
  - The two large-heap servers: level.
- **What it costs:**
  - Peaks rise where freed grown buffers are held for the hold and then reused: tpchgen
    570 to 650 MiB (glibc 522, jemalloc about 558), 64-Vec 278 to 327 (mimalloc 265).
  - The worst trail is bounded by the 64 MiB: 70 MiB over live on a 2 GiB heap, against 25
    on main. Keeping every outgrown slot instead leaves 540 MiB, which is why the count is
    needed.
- **Rejected:**
  - A budget scaled to the heap but set against all free large memory: free large memory
    exceeds it on these heaps too, so nothing changes.
  - Charging frees by what `mincore` says is resident: the over-count is not what fills the
    budget. It changed nothing and cost tpchgen 2%.
  - Keeping everything: the 540 MiB trail.

## Method

- **The switch** (`grown_edit.py`, `gs.rs`) is applied to a scratch copy of main by
  `build.sh`. `ASMALLOC_GROWN=m<mode>b<MiB>s<shift>r<0|1>d<0|1>` is read once, and every arm
  of a comparison uses the same binary with values of equal length. The modes:
  - 0: main.
  - 1: the budget counted over kept grown slots alone (move slots and grown frees).
  - 2: keep every one.
  - 3: keep none.
  - 4: a grown block's free kept as a plain free, and a move's slot by mode 1's count.
  - 5: as 4, but a move's slot by main's level.

  The other fields:
  - `s`: the budget becomes the larger of `b` and the heap in use shifted right by `s`. The
    heap is live large blocks plus small memory carved and not free.
  - `r`: a large free asks `mincore` and marks absent pages clean.
  - `d`: counters, written at exit to `GS_DIAG`.

  The own count is kept with a bit in the slot's size word. It is set when a slot is kept,
  and it is subtracted at the claim that takes the slot again and by the walk's purge of its
  pages.
- **Attributing faults.** Each large slot's size word also carries a tag for what last
  returned its pages (a move, a grown block's free, an unwritten block's free, a shrink, the
  hold, the clocked pass, growth over the cap) and the fine count at the time. A claim, and a
  widening in place, count the clean pages they clear under the block, by tag and by age.
  Those are the pages its tenant faults if it writes them: an upper bound, and exact for
  buffers that are filled.
- **Runs.**
  - Kernel jobs: `kern.py` on CPU 2 under `scripts/measure`, three rounds interleaved per
    session (`sessions/k1.sh`, `k2.sh`, `k3.sh`). Cycles are compared only within a session:
    k3 ran about 10% slower across the board than k1.
  - tpchgen: `tpch.py` with the preload library and the screening's tree, eight threads,
    four rounds in each of two sessions. Cycles are user plus kernel; the peak is the peak
    RSS.
  - DataFusion: benchrun's apps job, six rounds, cgroup peak.
  - Meilisearch: `meili-tail`, two rounds.
  - Order service at 2 GiB and log store at 1M: `servers-thread`, one round each.
  - Every server and app session ran under `scripts/measure --both-l3`.
  - Trail: `trail/` under `scripts/heavy` (`sessions/trail.sh`).
- Raw data: `docs/research/data/grown-slots-2026-10-24.tar.gz`.

## 1. How much of each gap is outgrown slots

Clean bytes taken back by claims and growth in place, by what returned them, main. MiB a run;
nearly all within 32 ms of the return.

| program | move's slot | grown free | hold | growth over cap | first touch | faults | what the rule changes |
|---|---:|---:|---:|---:|---:|---:|---|
| 64 Vecs to 4 MiB | 148 | 1,146 | 0 | 0 | 314 | 56k | all of the gap |
| tpchgen | 2,336 | 282 | 2 | 53 | 147 | 705k | about 85% of faults |
| Meilisearch, one run | 6,424 | 58 | 15,700 [1] | 860 | 74 | 52 a request | 17 a request |
| DataFusion | 71 | 528 | 928 | 526 | 289 | 1.03M | about 15% of faults, offset (below) |
| HashMap 1M | 0 | 0 | 0 | 3 | first touch only | 53k | nothing |
| json 32Mi | 0 (5 moves, all kept) | 0 | 0 | 0 | | 16k | nothing |

[1] Mostly the indexer's 5 to 20 MiB blocks, taken back 0.25 to 8 s after the hold returned
them (`meili-peak.md`). Not a growth rule's to fix.

Where the time goes against the best incumbent, main:

- **64-Vec** (k1): 675M cycles, of which 361M is kernel (faults and returns). mimalloc takes
  543M, with 140 faults. Keeping everything gives 455M, with 90M kernel. The copy itself is
  256 MiB a run, about 30M cycles. So the gap is the returns and refaults, not the copy.
- **tpchgen**: 26.08 G cycles, 2.10 G of it kernel. glibc takes 24.66 G with 1.16 G kernel.
  The rule gives 24.75 G with 1.01 G kernel, so the whole kernel gap is this.
- **HashMap** (k1, build phase): asmalloc 2,455M cycles (136M kernel), mimalloc 2,078M (47M
  kernel). Of the 377M gap, 89M (24%) is kernel: 52.7k first-touch faults against mimalloc's
  159, which come in 2 MiB pages. The other 288M (76%) is user time.
  - The dTLB counters (`sessions/tlb.sh`, quiet machine) give asmalloc 10.9M dTLB misses and
    2.55M 4 KiB page walks a run, against about none for mimalloc, which ends with 298 MiB in
    huge pages. jemalloc, with no huge pages, has the same 11M misses as asmalloc.
  - asmalloc's small classes, where the strings live, get huge pages after 16 MiB carved. So
    the walks are the hash table: a large block allocated whole, up to 64 MiB, accessed at
    random, on 4 KiB pages.
  - What would close it is huge pages for a large block allocated whole and then written
    densely. The design gives them only to blocks that grow.
  - One caution: in a session that overlapped a heavy build, mimalloc got no huge pages and
    took 3.5 G cycles with 74k faults. Its lead depends on free 2 MiB pages being available.
- **DataFusion**: faults and peak hardly move under any rule. Kept grown slots raise free
  memory, and the hold and the cap then return about as much of something else, which is
  faulted back in its place. Its gap to tcmalloc (3% in time, 0.46M faults against 1.03M) is
  the hold's and the cap's returns plus first touch.

**The incumbents at the same growth.** A subagent read the sources: jemalloc 5.3 as
tikv-jemallocator builds it, mimalloc 3.5.3 and 3.3.2, and google/tcmalloc. glibc is from
knowledge. All four let memory freed at one size serve another, which asmalloc's regions,
one class each, cannot.

- **jemalloc** grows in place into a free neighbouring extent, else copies. Dirty extents
  decay over 10 s, and anything of 8 MiB or more is purged at once.
  - Measured: 64-Vec 1,163M cycles with 247k faults; tpchgen about 558 MiB peak
    (`memory.md`); 1x256Mi 518k faults.
- **mimalloc** never grows in place. Freed pages go back to a 64 KiB-slice arena that any
  size reuses, purged after 1 s (3.3.2) or 4 s (3.5.3). Arenas get transparent huge pages.
  - Measured: 64-Vec 543M cycles with 140 faults and a 265 MiB peak.
- **tcmalloc** copies on every realloc. Freed large ranges go to a hugepage cache that
  coalesces and splits, with a limit set by recent demand and a 20 MiB floor.
- **glibc** grows mmapped chunks with `mremap` and munmaps them at free. Below its dynamic
  threshold, chunks coalesce in the heap.
  - Measured: 64-Vec 1,169M cycles with 273k faults; tpchgen 24.66 G cycles with 275k faults
    and a 522 MiB peak.

## 2. The candidate rules, measured

Medians; ranges in the raw data. Kernel jobs are k3 (64-Vec) and k1/k2 (the others).

| rule | 64-Vec Mcyc / faults / peak | tpchgen Gcyc / faults / peak | Meilisearch Mcyc, faults/req, heap mean/peak | DataFusion Gcyc / faults / peak |
|---|---|---|---|---|
| main | 769 / 56k / 278 | 26.08 / 705k / 570 | 16.88-17.01, 52-53, 716-725 / 748-770 | 69.79 / 1,025k / 865 |
| 4, 64 MiB (first) | 534 / 19k / 327 | 24.75 / 222k / 650 | 16.79-16.82, 35-39, 717-723 / 747-769 | 69.75 / 1,013k / 854 |
| 4, 16 MiB | 640 / 56k / 327 | 25.23 / 347k / 620 | 16.77-16.80, 35-39, 720-721 / 765-769 | 69.70 / 1,013k / 854 |
| 1, 64 MiB (both counted) | 660 / 19k / 326 | 24.67 / 221k / 640 | | 69.81 / 1,018k / 862 |
| 1, 16 MiB | as main | 25.25 / 357k / 612 | | 69.58 / 1,024k / 866 |
| 4, max(16 MiB, heap/16) | -13% (k2) / 62k / 327 | 24.91 / 262k / 616 | | |
| keep all (2) | 534 / 18k / 327 | 24.59 / 222k / 648 | | 69.57 / 1,023k / 857 |
| main, max(16 MiB, heap/8) | as main | | | |
| main, charged by `mincore` | +3% / 56k / 278 | 26.60 / 708k / 574 | | |
| mimalloc / glibc | 636 / 140 / 266 (k3) | glibc 24.66 / 275k / 522 | jemalloc 757 / 801 (`meili-peak.md`) | |

- **The order service at 2 GiB:** level under main, rule 4 at 16 and at 64 MiB: 68.1k
  cycles a request, 2,186 MiB, 2,189 a minute after close.
- **The log store at 1M:** heap level (1,428 to 1,434 MiB). Cycles were 337.6k on main,
  345.0k with 16 MiB and 340.9k with 64 MiB, one run each, single builds. The store makes no
  large moves, and the standings found this job varies by 1% across placements. I read it as
  noise; it is unresolved.
- **The other kernel jobs**, json 32Mi, the HashMap job and growing and freeing one 4 MiB
  buffer: within 1% under every rule.
- **Growing and freeing one 1 MiB buffer:** level under every rule but the `mincore` charge,
  which costs 22%: a system call at every free.
- **One buffer grown to 256 MiB:** 9 to 13% slower in k3 under the rules that keep a grown
  block's free. In k2 it was level, and main itself spans 300 to 330 between sessions, so
  this is unresolved.
  - When it is slow, the time is user time, not faults or the TLB: the dTLB counters are
    level, and the kept rules take half main's faults.
  - It is a cost only up to that point: every asmalloc arm is 4 times faster than mimalloc
    here.
- **Why the own count is needed.** The free large memory in tpchgen peaked at 251 MiB under
  main's count, and Meilisearch's at 391. A budget of a few tens of MiB set against that, or
  against an eighth of a 500 MiB heap, is always spent. The outgrown slots kept and not yet
  reused never went above 3 MiB on Meilisearch and 42 MiB on tpchgen under rule 4.
- **Why a grown block's free is kept apart from the count.** A grown block's free is
  ordinary free memory, and a plain free of a block of the same size is always kept. When
  both count together (rule 1), the 64-Vec job's drops fill the budget, and its outgrown
  slots go back: 660 against 534 Mcycles.

## 3. The trail

`trail` grows 256 buffers round-robin, 64 KiB at a time, to 4 MiB each, on a heap that holds
1 GiB of live ballast (1 KiB boxes, and 1 MiB blocks churned four at a step). 4 MiB is a
doubling past a move into a slot with headroom, so the outgrown slot is half the buffer: the
worst ratio. After each round it frees the buffers and calls `collect`. The next round's
moves then land in returned slots, which is reuse and pays nothing toward the cap, the
hardest case. The trail is the peak during growth less what is live, in MiB, the same in
rounds 1 and 2.

| rule | round 0 (destinations carved) | later rounds (destinations reused) | the freed buffers after the drop |
|---|---:|---:|---|
| main | 24 | 25 | returned at the free |
| 4, 16 MiB | 24 | 25 | held for the hold: about 0.55 s with the background thread, longer without |
| 4, 64 MiB | 68 | 70 | the same |
| 4, max(16 MiB, heap/16) | 69 | 133 | the same |
| main, max(16 MiB, heap/8) | 66 | 255 | |
| keep all | 68 | 540 | |

- **Round 0.** When the destinations are carved, the cap's growth payments hold every rule's
  trail to about the cap.
- **Later rounds.** When the moves reuse returned slots, nothing pays, and only a count of
  its own bounds the trail.
- **How long it lasts.** At most until the next growth takes the slots, or one hold, after
  which the hold returns them down to the 32 MiB floor. With the program here beating once
  every 16 ms, the hold lasted about half a second.
- **What a grown block's free costs.** Those slots are held for the hold, as any free is:
  here, 1 GiB of freed buffers for about 0.55 s with the background thread. On the 64-Vec
  job, where the drop is the whole heap and the next round reuses it, that is the 278 to
  327 MiB peak.

## 4. Proposal, ranked

1. **A grown block's free is an ordinary free. A move's outgrown slot is kept while kept
   outgrown slots total at most 64 MiB.**
   - Gains: tpchgen -5% cycles and -69% faults; 64-Vec -31%; Meilisearch -1% cycles and -33%
     faults; DataFusion, the order service and the log store level.
   - Costs: tpchgen's peak +80 MiB (+14%, over jemalloc's and glibc's); 64-Vec's +49 MiB; a
     worst trail of 70 MiB. One size-word bit, one counter, and a look at the size words of
     the slots a purge takes.
   - 64 MiB is the retained cap's value. It could be the same constant.
2. **The same with 16 MiB.**
   - Gains: half of tpchgen's faults are still faulted back (-3% cycles); 64-Vec -17%;
     Meilisearch the same as rank 1.
   - Costs: tpchgen +50 MiB of peak; no trail beyond main's.
   - Choose this if tpchgen's peak matters more than its time.
3. **One count for both kinds, 64 MiB.** tpchgen as rank 1, but 64-Vec only -14%, since the
   drops fill the budget.

Not proposed:
- Keeping every grown slot: the 540 MiB trail.
- Scaling a budget set against all free large memory: no effect.
- Charging frees by `mincore`: no effect, at a cost.

**Design text** (for "Large objects", replacing the paragraph that begins "A block small
enough to move in one piece"):

*A block small enough to move in one piece is left whole behind the move. A buffer grown and
freed again and again should find its memory resident each time; many buffers growing at once
should not leave a trail of the blocks they outgrew. These are two cases, and they get two
rules.*

*A grown block that is freed is an ordinary free: its slot stays resident, is stamped for the
hold, and is charged as any free is. A plain free of a block of the same size is kept, and the
program that grew one is likely to grow another.*

*A move's outgrown slot is kept resident while the outgrown slots kept and not yet taken
again total at most 64 MiB, and goes back at once beyond it. The count is of those slots
alone. Measured against all free large memory, which the hold keeps above its floor by design,
any such budget is always spent on a large heap: a TPC-H data generator faulted back 2.3 GiB
of outgrown slots a run within milliseconds, and a search engine a third of its faults
(`docs/research/grown-slots.md`). The bound is what makes the keep safe. When the moves land
in slots the heap reuses, nothing pays toward the cap, and without a count of their own, 256
buffers grown on a 2 GiB heap left 540 MiB behind them.*

*A kept slot carries a mark in its size word, written by its holder. The count rises when a
slot is kept, and falls when a claim takes a marked slot or a purge returns a marked slot's
pages. It is a hint: a mark lost or a purge missed moves the count, never a slot in use, and
a count read above the free large memory is taken as that memory.*

*The purge falls only on a block the program copied to reach its size, so it costs at most
about what the program already spent on that block.*

**Unresolved.**
- The log store's cycles, one run per arm: 337.6k on main against 345.0k with 16 MiB. The
  rule does not act on that job, but it needs a second round before it can be ruled out.
- The 256 MiB buffer's 9 to 13% in one session out of two.
- tpchgen's peak: whether keeping grown frees for the full hold is what raises it, or the kept
  outgrown slots. The two variants differ by 30 MiB and the runs spread by 30 to 40.
