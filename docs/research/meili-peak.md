# Meilisearch's peak: zeroed buffers we make resident

2026-10-24, main `169b5c7`, on the reference host (Ryzen 9950X, Linux 7.2.9, transparent huge
pages on `madvise`). The standings put asmalloc's Meilisearch heap under load at 949 MiB
against jemalloc's 781 (peak), with about twice jemalloc's page faults. This note accounts for
both.

**Update, the same day: the rule built** (branch `zero-by-evidence`, "Evidence at the free"
below). The lead chose evidence over the fixed threshold. A zeroed request of 16 MiB or more
that gets a slot known to be zero is marked. Its free samples sixteen pages and, if at most one
is resident, returns the slot known zero. It keeps nearly all of the fixed rule's gain on
Meilisearch: heap 718 to 733 MiB mean and 764 to 782 peak, against main's 853 to 860 and 958
to 961 and the fixed rule's 716 to 718 and 761 to 767. Faults are 51 to 53 a request, against
92 to 96. It costs a dense tenant nothing, since a block cleared with `memset` is never marked.

## Summary

- **The gap is not free memory we hold. It is zeroed memory we write.** Meilisearch's indexer
  asks for about four buffers of 50 MiB with `alloc_zeroed`, 7.7 a second, and writes about
  0.3% of each. asmalloc finds the slot not known to be zero four times in five and clears all
  50 MiB with a `memset`, which makes every page resident: 300 MiB/s. jemalloc returns every
  free of 8 MiB or more to the OS at once, so its next zeroed block of that size is fresh
  memory that it does not clear, and the pages the program never writes are never resident.
- **The account under load** (means over two minutes at 70%, MiB): asmalloc's own memory is
  298 against about 173 for jemalloc. The buffers alone hold about 129 resident, and with them
  not cleared asmalloc's part falls to 139, under jemalloc's. The rest of the process (about 587 MiB: glibc's arenas, which Meilisearch's C code
  fills, stacks and the binary) is the same under both. At asmalloc's peak sample the same
  buffers had just been freed and sat in the hold, 226 MiB of resident free slots.
- **The faults come from the same place, and from two others.** On main about 23 faults a
  request are the `memset` writing over pages the hold had returned. About 13 are outgrown slots
  that a move returns because the kept-grown budget is full, claimed again within 64 ms. About
  13 are slots of 5 to 20 MiB that the hold returns and the indexer takes back 0.1 to 1 s
  later, which jemalloc refaults too.
- **The fix is to return the pages instead of clearing them.** A prototype behind a switch:
  a zeroed large allocation with at least a threshold to clear, in a slot not known to be zero,
  returns the whole pages (`MADV_DONTNEED`) and writes zeros only over the part pages at either
  end. Measured with the threshold at 8 MiB, the result in this table:

| Meilisearch, 70% (930 req/s) | main | return at 8 MiB and up | jemalloc |
|---|---|---|---|
| heap mean / peak, two minutes, MiB (two rounds) | 858 to 877 / 958 to 974 | 717 to 720 / 767 to 768 | 753 to 762 / 806 to 820 |
| heap mean / peak, the standings' job (one run) | 855 / 926 | 719 / 753 | 744 / 796 |
| heap a minute after close (one run) | 660 | 641 | 663 |
| page faults a request | 78 to 89 | 52 to 54 | 37 to 42 |
| server Mcycles a request | 16.74 to 17.16 | 16.75 to 16.90 | 17.56 to 17.75 |
| p99 / p99.9, ms | 29.5 to 31.6 / 44.2 to 47.6 | 29.2 to 30.1 / 41.9 to 47.0 | 33.9 to 38.0 / 50.5 to 57.1 |

  DataFusion's peak and faults are unchanged at 8 MiB (860 to 866 MiB, 1.027M faults, against
  855 to 868 and 1.028M; cycles 69.45 G against 69.48). The 2 GiB order service and the 1M log
  store do not move (with the prototype's 1 MiB threshold, one run each).
- **The worst case is a zeroed block written in full and taken again while its slot is still
  resident.** There, returning the pages costs one fault for every page the program writes
  instead of a `memset` over resident pages. It is 12 to 14 times slower from 1 to 16 MiB and
  5 times at 64 MiB (a 16 MiB block's life: 6.7M cycles against 0.55M). Return pays when the
  program writes less than about 7% of the block (25% at 64 MiB). It is never slower when the
  slot's pages had been returned already. At 8 MiB and up jemalloc pays exactly this cost
  in that case, and so does glibc from 32 MiB.

The proposal, ranked, is at the end.

## The account

Two-minute runs of the `meili-tail` suite at 70% (`servers-thread`'s fill, then twelve
ten-second pieces), sampling builds. asmalloc's sampler walks the heap every 250 ms and asks
`mincore` which pages are resident (`bench/probes/meili-peak/memstat_peak.rs`). jemalloc's is
built with statistics and reads them every 250 ms (`je_peak.rs`). Both builds count the bytes the
program asks for and read glibc's `mallinfo2`. The sampling costs both builds about 15% in
cycles a request and raises their tails. Their memory matches the plain builds: anonymous
memory 884 against 858 to 877 on main, 760 against 753 to 762 for jemalloc.

Means over the window (session `s2`), MiB:

| | asmalloc main | asmalloc, return at 1 MiB | jemalloc |
|---|---:|---:|---:|
| anonymous memory of the process | 884 | 724 | 760 |
| outside the allocator: glibc's arenas, stacks, binary | 587 | 586 | about 587 [1] |
| the allocator's | 298 | 139 | about 173 [1] |
| live, small classes: asked for / rounding to the class | 7.5 / 2.4 | 7.5 / 2.5 | |
| live, large classes: asked for / resident | 226 / 203 | 223 / 75 | 261 asked, all classes |
| of which the 50 MiB zeroed buffers, resident | about 129 | about 0 | |
| free large slots, resident: within the hold / past it | 45 / 2 | 24 / 2 | dirty 65 |
| free small slots, resident: on wholly free pages / pinned by live ones | 23.5 / 4.8 | 14 / 4.4 | |
| parked in magazines and the depot | 7.5 | 7.4 | |
| metadata, magazine area, slabs | 2.2 | 2.2 | 20 |

[1] jemalloc's `stats.resident` counts its active extents' pages whether or not they were
touched, and reads 415 here, more than the whole process less glibc's arenas. glibc's
`mallinfo2` reads the same in all three runs (520 MiB in use, 68 to 75 free), so jemalloc's
part is the process's anonymous memory less asmalloc's outside part. Its 261 MiB asked for are
mostly untouched pages: when the indexer frees its four buffers, jemalloc's anonymous memory
does not move (`series.py`).

At asmalloc's peak sample (`s1`, 986 MiB): outside 639 (glibc in use 574, at its own high);
asmalloc 347, of which free large slots within the hold 226 (all four buffers, freed a moment
before), live large 74, small classes 35, parked 8. With the rule the buffers are not resident
when they are freed, so holding them costs nothing.

**Rounding and fragmentation are not the gap.** Rounding of small classes is 2.4 MiB. Small
free slots pinned by live neighbours are 5. jemalloc's own rounding, `allocated` less
`asked`, is 59 MiB, mostly untouched.

**What the buffers are.** Every zeroed large request in the window was in the 56 MiB class
(48 to 56 MiB). They are 383 MiB/s of `alloc_zeroed`, of which 82 MiB/s found the slot known
to be zero. With the rule, the slots of that class freed 1.1 MiB/s resident against 302 on
main. The program writes a few pages of each.

## Where the faults come from

The allocator's counters (`diag.rs`) count, for each claim of a large slot, the pages that
`mincore` says are not resident under the block, and the rule that last purged its region. For
each free, they count the slot's resident pages, which against the slot's pages resident at the
claim gives what the tenant faulted in. On main, MiB/s at 930 requests a second
(1 MiB/s is about 0.28 faults a request):

| source | MiB/s | faults a request |
|---|---:|---:|
| `memset` of the 50 MiB zeroed buffers over pages the hold returned | 82 | 23 |
| outgrown slots a move returned, claimed again within 64 ms | 49 | 13 |
| slots of 5 to 20 MiB the hold returned, claimed 0.1 to 1 s later | about 46 | 13 |
| everything else (first writes in other classes, moves' destinations) | | about 30 |
| total (kernel count of the sampling build) | | 72 |

The hold returned 182 MiB/s and the claims that faulted it back did so after 64 to 125 ms
(51 MiB/s), 125 to 250 ms (51), 0.25 to 0.5 s (49) and 0.5 to 1 s (15). The indexer takes its
blocks back after a batch, a fraction of a second to a second later. A hold long enough to
cover that keeps the peak, as `meilisearch-memory.md` and `hold-credit.md` found.

**Faults and memory high at once** is the `memset`: it faults pages the hold returned and makes
resident pages the program never asked to use. With the rule both go. What is left over
jemalloc, about 12 faults a request, is the moves' outgrown slots. The kept-grown budget
(16 MiB) is shared with every ordinary large free, and free large memory is above it nearly
all the time (option 2 below).

## The two inaccuracies

- **A freed large block counts as resident for every page of its requested size.** On main,
  frees of large slots were charged 1,106 MiB/s and had 911 resident. With the rule the
  difference is the whole of the 50 MiB buffers: charged 377 MiB/s, resident 1.1. The
  inflated count changes no decision on this job that matters for the peak. The hold stops at
  its floor by the count, and the extra bytes are pages it then "returns" that were not
  resident. But it keeps the kept-grown budget full, which is option 2's faults, and with the
  rule it would make every such free look like 50 MiB of retained memory (see "Bookkeeping").
- **Growth in place (`region::cover`) is not charged as growth.** It is 0.9 to 1.9 MiB/s here,
  and in steady state the heap does not grow at all: carves are 0 MiB/s and no growth payment
  was made. It does not move this job.

## Answers on the rule

**1. The worst case, measured.** `bench/probes/meili-peak/zbench`: a block's life (allocate
zeroed, store once to a fraction of its pages, free), the median of 30, on CPU 2, in kcycles.
"Resident" is the slot the last life left, still resident. "Returned" is a slot whose pages
were returned but which is not known to be zero, as the hold leaves a slot it purged in part.

| MiB | slot | main, none written | main, all written | return, none | return, 1/16 | return, 1/4 | return, all |
|---:|---|---:|---:|---:|---:|---:|---:|
| 1 | resident | 26 | 29 | 1.3 | 27 | 96 | 402 |
| 1 | returned | 344 | 343 | 1.8 | 23 | 82 | 339 |
| 4 | resident | 108 | 155 | 1.4 | 99 | 397 | 1,646 |
| 4 | returned | 1,424 | 1,416 | 1.5 | 82 | 334 | 1,385 |
| 16 | resident | 439 | 549 | 1.8 | 395 | 1,614 | 6,683 |
| 16 | returned | 5,923 | 5,838 | 1.8 | 340 | 1,396 | 5,746 |
| 64 | resident | 6,612 | 7,149 | 3.2 | 1,614 | 6,669 | 36,728 |
| 64 | returned | 31,368 | 32,292 | 3.3 | 1,420 | 5,748 | 31,497 |

A fault costs about 1.35 kcycles a page and returning a resident page about 0.25 more. A
`memset` costs 0.1 kcycles a page while the block fits in the cache and 0.4 at 64 MiB. So on a
resident slot, return pays below about 7% of the pages written (1 to 16 MiB) or 25% (64 MiB),
and costs 12 to 14 times (5 times at 64 MiB) when the program writes all of it. On a returned
slot it is never slower. A 1 MiB threshold is too low for the cost alone, though not by much in
time: a fully written 1 MiB block on a resident slot goes from 6 to 80 us. DataFusion shows it
(1 MiB: faults +1.2%, cycles +0.25%; 8 MiB: level). 8 MiB is the number: Meilisearch's
buffers are well above it, and at it and above jemalloc already pays this worst case.

**2. What the incumbents do.**
- **glibc.** A request at or above its `mmap` threshold gets a fresh mapping, which `calloc`
  does not clear and `free` unmaps. The threshold starts at 128 KiB, and rises to the size of
  each mapped block that is freed, up to 32 MiB. So 32 MiB and up is always lazy, and below that
  a size is lazy until a block of it is freed and then comes from the heap and is cleared.
- **jemalloc 5.3** (tikv-jemallocator's build: `dirty_decay_ms` 10,000, `muzzy_decay_ms` 0).
  Requests of 8 MiB and up go to its huge arena, whose decay is set to 0
  (`arena_choose_huge`), so every free is purged at once and a zeroed request gets zeroed
  memory without a `memset`. Below 8 MiB a zeroed request reuses dirty extents and clears them.
- **mimalloc** clears unless the page is known zero (`free_is_zero`). Freed memory is purged
  after `purge_delay`, 1 s, so it is lazy for memory more than a second idle.
- **tcmalloc** clears every `calloc` with `memset`. Its heap here (967 MiB mean, 1,039 peak) is
  asmalloc's.

So at 8 MiB and up a lazy zeroed block is what a jemalloc program already gets, and at 32 MiB
and up what a glibc one gets.

**3. Huge pages.** The 16 MiB threshold for huge pages is a small class's: its region switches
to huge pages after 16 MiB carved. Large classes get none, except a block that grows by
doubling past 2 MiB with dense contents, whose slot is advised for huge pages until it is freed
(`advise_grown`). A zeroed allocation is not grown, so its pages are small pages, and
returning them splits nothing. One case needs care. A slot whose advice could not be taken back
is claimed with its extent in whole huge pages (`alloc_at`), and a zeroed grow can be advised.
There the rule should return whole huge pages only, as `region::purge_part` already trims, and
clear the rest. A huge page faulted back costs about what its 512 small pages do in the kernel's
zeroing, in one fault. No different threshold is needed.

**4. Bookkeeping.** The prototype leaves the returned pages' clean bits clear, so the free
charges the whole extent: 50 MiB of retained large memory for a block with a few pages
resident. That inflates the count that the hold's floor, the kept-grown budget, the cap and
growth payments read. A payment or a hold would then "return" pages that are not there and call
the debt paid, so the cap would hold in the count and not in fact. The rule should therefore
also act at the free. Mark the slot in its size word (a bit beside `huge`) when the claim
returned its pages. At its free, return the slot whole and release it as purged
(`mark_purged`): clean, known zero, not counted. That costs one `madvise` of what the tenant
wrote, mostly page tables with nothing in them, and the block gives up the hold. That is
jemalloc's eager purge at 8 MiB, and the next zeroed claim of the slot then needs neither a
`memset` nor a return. Not built: the prototype measures only the claim side.

**5. Every entry point.** Zeroing of large blocks happens in three places, and the rule
belongs in one helper they call:
- `Heap::alloc_large_using`, which `GlobalAlloc::alloc_zeroed`, `Allocator::allocate_zeroed`
  and C `calloc` (through `process::alloc_large`) all reach. This is the only place the
  prototype changes.
- `resize_large_in_place` with `zero`: `grow_zeroed` in place clears the growth.
- `Bound::resize`'s move with `zero`: `grow_zeroed` that moves takes a block without
  zeroing and clears past the bytes kept.

C `realloc` and Rust `realloc` do not zero. Small classes are under any threshold worth
having.

**6. A fixed threshold or evidence.** A variant that asks `mincore` first and returns only a
block with fewer than an eighth of its pages resident (`ASMALLOC_ZRES=8`) bounds the worst case:
it is never more than about 1.35 times main. On Meilisearch it does as well (724/784 MiB,
52 faults), because the hold returns every buffer's slot at some point, and from then on the
slot stays lean. But `mincore` costs about 0.35 kcycles for each page that is not present,
a quarter of a fault, on every zeroed claim. That makes it 25 to 37% slower than main in
the cases it should win (1 MiB returned and fully written: 430 against 343). The fixed
threshold has a clear number, 8 MiB, at which the worst case is jemalloc's own. I agree with
the fixed threshold.

## Proposal, ranked

1. **Return instead of clearing, at 8 MiB and up** (above, with the free side of point 4).
   Meilisearch: heap mean 140 to 160 MiB lower and peak 170 to 200 lower, under jemalloc's on
   both. Faults 35% fewer, cycles and tails level. DataFusion, the order service and the log
   store level. Cost: a zeroed block of 8 MiB or more that the program writes in full and
   reallocates while its slot is still resident pays a fault a page instead of a `memset`,
   about 1.2 ms more a 16 MiB life, as it does under jemalloc. A design change: a zeroed
   request is answered by returning memory, and a free of such a block returns its slot.
2. **Keep outgrown slots within a budget of their own.** The kept-grown budget counts every
   free large slot, so a heap holding more than 16 MiB of free large memory, which this one
   always does, returns every outgrown slot, and the next growth faults it back: 13 faults a
   request here, about what is left over jemalloc. `memory.md`'s `keep` variant (a move keeps
   a slot that alone fits the budget) took tpchgen's faults from 704k to 219k for 10% of its
   peak. Not measured here.
3. **Charge freed blocks by what is resident.** With option 1's free side the main source of
   the over-count on this job goes. Elsewhere, the part a tenant never wrote is cheap to learn
   only at a claim of the same slot. Not worth a mechanism of its own until a job shows it
   matters.

Not proposed again: a longer or adapting hold (`large-hold.md`, `hold-credit.md`). The
indexer's reuse comes 0.1 to 1 s after its frees, and covering that keeps DataFusion's peak.

**Unresolved.** About 30 faults a request on main are not split by source. The burst after
the silence in the standings' job (one run each) took 45 faults a request in its first half
second with the rule, against 15 on main and 25 for jemalloc, and its p99 there was 9.4 ms
against 3.1 and 5.3. It is one run of a piece dominated by an indexing batch, which earlier
notes found spread from 3 to 22 ms. It needs rounds before the rule ships. Plain-build cycles
moved by 0.4M a request between rounds of the same build, so the rounds here show no cycle
change either way.

## Evidence at the free

Branch `zero-by-evidence` from main `226aede`. Measured on the rule's own code, with a
probe patch (`evidence_edit.py`) that reads its floor and cut from the environment, so that
main and the rule are one binary.

**The rule.**
- The claim clears what it cleared before: a zeroed large request is cleared with `memset`
  unless its slot is known to be zero.
- One addition: when a zeroed request of at least 16 MiB is answered from a slot known to be
  zero, nothing was cleared and none of its pages is resident, so the block is marked (a bit in
  the slot's size word, `Recorded::unwritten`).
- At the free of a marked block, `mincore` looks at the middle page of each sixteenth of its
  current extent. If at most one of the sixteen is resident, the slot is returned whole
  (`region::purge_held`) and released as purged: clean, known zero, not counted, no hold stamp.
  Otherwise it is an ordinary free.
- Every claim clears the mark. A resize in place keeps it, and the free samples the extent the
  block has then. A marked block that moves is not kept by the kept-grown budget, so its old
  slot goes back behind the copy. A slot with huge-page advice is never marked. A slot that
  cannot be returned whole (in a huge-page part, or the OS refuses) is an ordinary free.
- A request that is not zeroed is never marked: an over-sized scratch buffer that its tenant
  writes in small part and that is taken again within the hold costs nothing today, and
  returned at every free it would fault on every life.

**No new system call.** `mincore` was already in the OS interface (`Os::resident`, used by
`unspill`), with the simulated OS's counterpart and ledger entries (KERNEL-08,
RESERVATION-06). The rule calls it through `Space::resident` on single pages. That call costs
about 650 cycles, whether the page is present or absent (`zbench`, 1,024 calls, best of
five). Sixteen calls and the `madvise` cost about 12k cycles, 2.5 us, at a marked block's free.

**zbench, main against the rule** (the rule switched on at every size to choose the floor,
the cut at one in sixteen; medians over four interleaved pairs of runs, 30 lives a row, CPU 2,
kcycles). Every row starts from a heap that `collect` emptied, so its first slot is known to be
zero. "loop" takes the same slot life after life; "returned" is that slot after the program
returned its pages itself, so it is not known to be zero; "zero" is a slot known to be zero at
every life. Writes are one store a page, spread evenly. Selected rows; all rows are in
`docs/research/data/meili-peak-evidence-2026-10-24.tar.gz` (`zb/`), with the sessions below:

| MiB | slot | written | main | rule | rule / main |
|---:|---|---|---:|---:|---:|
| 1 | zero | 1/16 | 22.6 | 38.3 | 1.69 |
| 2 | zero | 1/16 | 43.3 | 55.2 | 1.27 |
| 4 | zero | 1/16 | 86.0 | 98.0 | 1.14 |
| 8 | zero | 1/16 | 174.1 | 184.2 | 1.06 |
| 8 | zero | 1/8 | 348.8 | 360.1 | 1.03 |
| 16 | zero | 1/16 | 353.9 | 360.8 | 1.02 |
| 16 | zero | 1/1 | 5,719.8 | 5,730.1 | 1.00 |
| 64 | zero | 1/16 | 1,493.3 | 1,499.3 | 1.00 |
| any | zero | none | 0.5 to 5.7 | 11.5 to 21.1 | the fixed 12k |
| 1 | loop | 1/16 | 25.5 | 38.8 | 1.52 |
| 16 | loop | 1/16 to 1/1 | 457 to 560 | 458 to 562 | 1.00 |
| 64 | loop | 1/16 to 1/1 | 6,997 to 7,450 | 7,083 to 7,685 | 0.99 to 1.03 |
| 16 | returned | 1/16 | 5,903.9 | 363.3 | 0.06 |
| 16 | returned | 1/1 | 6,030.1 | 5,721.5 | 0.95 |
| 64 | returned | 1/4 | 32,990.3 | 5,790.9 | 0.18 |
| 16 | alternate | none, then all | 448 / 560 | 449 / 563 | 1.00 |

- **The floor is 16 MiB.** The rule's only extra cost is the sample and the `madvise`, which
  fall on a block marked from a slot known to be zero (the "zero" rows). With a sixteenth of the
  block written, that is 6% of the life at 8 MiB, 2% at 16 MiB and 0.4% at 64 MiB. At 16 MiB
  and above no row is more than 3% slower than main except a block written not at all, which
  pays the fixed 12k cycles on a life that otherwise costs a few thousand. The 64 MiB loop rows,
  up to 3% either way, are rows where the rule takes main's path (no block is marked), so that
  is the spread between runs.
- **The cut is one in sixteen, not one in eight.** A fault costs about thirteen times what
  clearing a resident page does (1.35 against 0.1 kcycles at up to 16 MiB). So a tenant that
  writes more than about a thirteenth is cheaper on a slot kept resident and cleared. At one in
  eight, tenants that write between a thirteenth and an eighth would be returned and fault, at
  up to 1.5 times main's cost. The 1 MiB "loop 1/16" row shows that cost where the sample
  classifies the tenant sparse. At one in sixteen and 16 MiB, a sparse tenant's life on a slot
  known to be zero (360.8 kcycles) is under main's on its resident slot (456.6).
- **The sample count need not scale.** At 16 MiB and up the fixed cost is under 3% of a block a
  sixteenth written. A larger count would only buy precision the cut does not need.
- **Dense tenants pay nothing.** A block cleared with `memset` is never marked, so a tenant
  that writes all of a reused resident slot takes main's path (loop rows, 1.00). So does a
  dense tenant alternating with a sparse one on one slot: the dense life leaves the slot
  resident, the sparse life's `memset` keeps it so, and neither is marked. The returned rows
  are where the rule pays: a tenant's pages that were gone by the free leave the slot known
  zero, and the next tenant clears nothing.
- The rows that never mark (below the floor, or not from a known-zero slot) differ by up to 13%
  in either direction between the two modes at 1 to 4 MiB. They take the same code, so that
  spread is the heap's layout between the two processes, not the rule.

**What the look sees, and its limits** (from the review):
- **It sees what the tenant touched, not what it wrote.** A page that is only read maps the
  kernel's shared zero page, and `mincore` counts it resident. The reviewer showed it on the
  kernel: a 20 MiB zeroed block read in full, or read one page in eight, reads as all
  resident at the next request. So a block that is read widely and written sparsely is treated
  as dense and freed as on main. That is a limit of the rule, not a regression.
- **The allocator's own reads count too.** A grow in place reads the block's last piece to
  judge its density for huge pages (`mostly_data`), and those pages then read as resident.
- **The worst case is about an eighth, not a sixteenth.** The samples are one page in each
  sixteenth of the extent, so a dense run just under two sixteenths long can cover only one
  sample, and the block is returned. By the 13 to 1 cost of a fault against clearing a
  resident page, that tenant's next life costs about 1.6 times main's.
- **The simulated OS differs from the kernel here.** It counts a page resident only if the page
  holds a nonzero byte, so the tests see a page only read as absent.

**Meilisearch** (the rule at 16 MiB and one in sixteen, against main in the same binary and the
fixed 8 MiB prototype):

| | main | evidence rule | fixed rule | jemalloc |
|---|---|---|---|---|
| two minutes at 70% (3 rounds): heap mean / peak, MiB | 853–860 / 958–961 | 718–733 / 764–782 | 716–718 / 761–767 | 757 / 801 |
| faults a request | 92–96 | 51–53 | 51–54 | 42 |
| Mcycles a request | 16.85–17.16 | 16.37–17.01 | 16.85–16.88 | 17.79 |
| p99 / p99.9, ms | 28.4–31.8 / 43.0–47.2 | 28.8–31.5 / 46.1–47.1 | 26.8–29.4 / 41.2–44.7 | 36.0 / 52.1 |
| standings' job (3 rounds): heap mean / peak | 794–873 / 918–947 | 712–725 / 741–757 | 715–730 / 742–775 | |
| a minute after close | 651–658 | 657–664 | 642–657 | |
| burst after the silence, 50 to 500 ms: p99, ms | 3.1, 12.7, 3.0 | 11.3, 14.7, 3.1 | 3.1, 12.4, 3.1 | |
| the same: faults a request | 16, 89, 1 | 52, 42, 1 | 16, 74, 13 | |

The burst's half second is bimodal in every build: about 3 ms, or 12 to 15 ms with 40 to 90
faults a request, as an indexing batch lands in it or not. The rule was high in two of three
rounds, main in one of three, and the fixed rule in two of four (with the standings' run).
Three rounds do not separate them. The fixed rule's 9.4 ms in the first note was one of
these.

**The over-count and the outgrown slots** (counting builds, one run each):
- With the rule, every zeroed request found its slot known to be zero (7.7 a second), and
  nothing was cleared.
- Frees of large slots were charged 756 MiB/s against 646 resident, where main charged 1,080
  against 878. The 50 MiB buffers now leave uncounted.
- The outgrown slots did not change: a move refused the kept-grown budget 49.1 MiB/s against
  48.8 on main, and as much was faulted back within 64 ms. Free large memory within the hold is
  still 25 MiB on average (45 on main), and other classes' over-count is about 110 MiB/s. The
  16 MiB budget stays full, so those 13 faults a request remain.

**The other programs.**
- DataFusion, eight rounds each: peak 859 to 866 MiB with the rule against 862 to 868, 1.028M
  faults against 1.035M, 69.54 G cycles against 69.52.
- The 2 GiB order service: level, one run each (68.3k cycles a request in both, 2,186 MiB, p99
  22 us). Main's run was marked invalid for 0.12 CPUs of other work in the burst's first 5 ms;
  its load segment is used.
- The 1M log store: level, one run each (345.9k cycles a request against 347.9k, 1,428 MiB
  against 1,430, p99.9 122 us against 124).

**Proposed design text**, for "Large objects" after the paragraph on zeroed allocation in
"Returning memory": *A zeroed request whose slot is not known to be zero clears it, which
makes every page resident. One whose slot is known zero gets its pages untouched. When such a
request is 16 MiB or more, its block is marked, and its free looks at one page in each
sixteenth of the block's extent. If at most one of them is resident, the tenant left the block
mostly untouched, and the free returns the slot whole and releases it known to be zero and
uncounted, with no hold. The next zeroed request then clears nothing and pays only for the
pages it touches. Otherwise the free is an ordinary one. The look sees pages touched, not pages
written: a page only read maps the zero page and counts as resident, so a block read widely and
written sparsely is freed as before. A dense run just under an eighth of the block can meet one
sample, so the worst tenant the rule returns touched about an eighth of its block. A block cleared with a `memset` is
never marked, so a tenant that writes what it is given, on a slot that stayed resident, pays
what it always did. A request that is not zeroed is never marked either: a scratch buffer
written in small part and taken again within the hold costs nothing as it is. Every claim
clears the mark. What the look finds decides only which of two valid frees happens. A search
engine's indexer zeroed four 50 MiB buffers a batch and wrote a few pages of each. Clearing
them held 150 MiB more than jemalloc, which returns every free of 8 MiB or more at once
(`docs/research/meili-peak.md`).*

**Unresolved.**
- The burst after a silence: three rounds are bimodal in every build (above).
- The outgrown slots' 13 faults a request (proposal 2) are untouched.
- About 30 faults a request on main are still not split by source.

## Method

- **Builds.** `bench/probes/meili-peak/build.sh` extracts main `169b5c7`, applies the edits,
  and builds the app as `benchrun build-apps` does, sharing the main checkout's build
  directory. The builds were `mp-main` (none), `mp-diag` (`peak_edit.py` and
  `proto_edit.py`: counters, both samplers, the switch), `mp-je` (`peak_edit.py`; tikv-jemallocator
  with `stats`) and `mp-proto` (`proto_edit.py` alone, for the timed comparisons, Meilisearch
  and DataFusion). The order service and the RPC servers were built with the same `mp-proto`
  before the `ZRES` variant was added. Plain jemalloc is the standings' build.
- **The switch.** `ASMALLOC_ZPURGE=<bytes>` (0: main), `ASMALLOC_ZRES=<64ths>` (0: no
  `mincore`), read once. Every arm of one comparison is the same binary with environments of
  equal length (local `bench/allocators.toml` entries, listed in `session.sh`).
- **Sessions.** `session.sh` under `scripts/measure --both-l3`, three arms in a fixed order a
  session, default-thread's watcher beside. Two rounds of `meili-tail` for main, the 8 MiB
  rule and jemalloc, one for the 1 MiB rule and the `mincore` variant, and one of
  `servers-thread`'s Meilisearch job. DataFusion: two sessions of six rounds through
  `benchrun run --suite apps`, cgroup peak. The order service and the log store: one
  `servers-thread` run each, 1 MiB rule against main. The order service's runs were marked
  invalid for other work on its CPUs in the burst's first 5 ms (0.12 CPUs). Their load
  segment is used.
- **Tables.** `account.py DIR TAG [--at mean|peak] [--diag] [--classes]`, `series.py FILE`;
  `zbench.sh` builds the microbenchmark, run with `scripts/measure scripts/memlimit -- taskset
  -c 2`.
- Raw results: `docs/research/data/meili-peak-2026-10-24.tar.gz` (benchrun's records, the
  watcher's and the samplers' files, zbench's output).
- **The evidence rule.** `build.sh mp-ev <commit> asmalloc evidence_edit.py` (Meilisearch,
  DataFusion, the order service, the RPC servers) and `mp-evdiag` (with `peak_edit.py`), with
  local entries `ev-off` (`ASMALLOC_UNWRITTEN=00000000`), `ev-on` (`16777216`), both with
  `ASMALLOC_UNWRITTEN_ONE_IN=16`, and `evd-off`/`evd-on`. Sessions: `e1` to `e3`
  (`meili-tail`), `t1` to `t3` (`servers-thread`), `d1` (counting builds), `df3`, `orders2`,
  `store2`. In the counting build, the region of a slot returned at its free keeps the rule of
  its last walk, so the fault table's "hold, over 8 s" row there is the known-zero claims of the
  50 MiB class, which fault only what their tenants write. `zbench.sh evidence <commit>
  evidence_edit.py`, then four interleaved pairs, `zcompare.py`. `thread.py` and `apps.py`
  summarize the servers-thread and apps sessions.
