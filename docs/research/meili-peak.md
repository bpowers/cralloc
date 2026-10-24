# Meilisearch's peak: zeroed buffers we make resident

2026-10-24, main `169b5c7`, on the reference host (Ryzen 9950X, Linux 7.2.9, transparent huge
pages on `madvise`). The standings put asmalloc's Meilisearch heap under load at 949 MiB
against jemalloc's 781 (peak), with about twice jemalloc's page faults. This note accounts for
both.

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
