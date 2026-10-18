# Allocation-cost roofline on Zen 5

Measured 2026-10-16 on one machine (section 2) with the harness in `bench/roofline`;
`bench/roofline/README.md` has the commands. A roofline here means the cost of the cheapest
code that could do a job, measured rather than assumed, so that a real allocator can be
compared against a floor instead of against other allocators.

Numbers are cycles unless a unit is given. One cycle was 0.177 ns (the core ran at about
5.65 GHz). Everything in a table was measured. Explanations of why a number comes out the way
it does are inference and are marked as such.

Contents

1. Summary
2. Machine and method
3. Fast-path shapes
4. Store-to-load forwarding
5. Atomics, calls and core widths
6. The front end: twelve-uop windows
7. Kernel memory primitives
8. Moving cache lines between cores
9. What rustc generates
10. Existing allocators
11. Design rules that follow
12. Caveats
13. The proved routines: encoding and placement
14. The C entry points

## 1. Summary

- An inlined allocate+free pair costs 2.0 to 2.4 cycles with a thread-local cache and about 4.0
  with a per-CPU cache under restartable sequences (rseq). tcmalloc's own per-CPU sequence
  costs 4.5 to 4.8. Through `call`, every shape costs 8.0, because a call and return cost 4
  cycles whatever the function does.
- Real allocators through Rust's `GlobalAlloc` cost 10 to 18 cycles per pair (mimalloc 3.5.3,
  jemalloc 5.3.0) and 11 to 26 (glibc 2.43).
- A value stored and loaded again costs nothing if it is 32 or 64 bits wide and addressed
  through a register. It costs 5 to 7 cycles if it is 8 or 16 bits loaded with `movzx`, or if it
  is addressed absolutely, which is how rustc addresses `thread_local!` statics.
- One uncontended atomic read-modify-write costs 20 cycles. Handing a cache line to another
  core costs 16 to 22 ns inside one L3 and 76 to 79 ns across L3s.
- A first-touch page fault costs 535 to 566 ns per 4 KiB page: as much as 750 inlined pairs.
  Inside a 2 MiB transparent huge page it is 117 to 214 ns per 4 KiB, bound by zeroing.
  `MADV_POPULATE_WRITE` saves only the exception, a fifth to a quarter (section 7).
- The front end sets a cliff by instruction count, not by alignment. A straight-line run of up
  to 12 fused uops flows at full speed; 13 to 24 takes 3 cycles; 25 or more takes 5.
- Without link-time optimization, rustc inlines the fast path only in the crate that declares
  the global allocator, and every allocation also calls an empty function. Thin LTO fixes both,
  with one hazard for a fast path written in `asm!` (section 9).
- When threads free what other threads allocated, mimalloc costs 1.5 to 2 times jemalloc in a
  symmetric exchange, and 2 to 2.7 times in a pipeline that crosses L3s. A floor allocator does
  the symmetric exchange 3 to 4.5 times cheaper than jemalloc.

## 2. Machine and method

- AMD Ryzen 9 9950X (Zen 5, family 26 model 68): 16 cores, SMT on, 48 KiB L1d and 1 MiB L2 per
  core, two L3 domains of 32 MiB (CPUs 0-7 with siblings 16-23, and 8-15 with 24-31). 60 GiB.
- Linux 7.2.9-201.fc44, glibc 2.43, rustc 1.99.1 (LLVM 23.1.1). Driver `amd-pstate-epp`,
  governor `powersave`, boost on. Transparent huge pages in `madvise` mode; every smaller
  multi-size THP setting off. The kernel zeroes pages on allocation (`INIT_ON_ALLOC`) and was
  built without the rseq time-slice extension.
- glibc registers rseq for every thread with the full feature size (33 bytes), which includes
  the `mm_cid` field: a small per-process id for the thread or CPU, used below to index
  per-CPU state.

Fast-path candidates are hand-written assembly loops (`bench/roofline/kernels`), so the
compiler cannot change them. Each is pinned to CPU 2 with its SMT sibling idle and run for 40
million iterations four times; the first run is a warm-up and the fastest of the rest is
reported. Cycles, instructions, retired uops and store-to-load forwards come from user-space
hardware counters. Cycle counts do not depend on frequency.

Three loop forms are used throughout:

- pair: allocate one object, free it at once. The tightest dependency chain an allocator has.
- call pair: the same, but each operation is an out-of-line function reached by `call`.
- batch: 32 allocations, then 32 frees in the same order. Figures are per allocate+free and
  include about 6 uops of loop overhead.

A "magazine" is an array of object pointers used as a stack. A "fused uop" is the unit the
core's front end counts in: one instruction, with a `cmp` or `test` and the conditional jump
after it counted together.

## 3. Fast-path shapes

Cycles per allocate+free; fused uops per pair in parentheses.

| Shape | Pair | Call pair | Batch |
|---|---:|---:|---:|
| Thread-local intrusive free list, reached through a base register | 2.28 (10) | 8.00 | 4.7 |
| Thread-local magazine, reached through a base register | 2.36 (13) | 8.00 | 3.0 |
| Per-CPU magazine under rseq, indexed by `mm_cid`, inputs in registers | 4.00 (21) | 8.00 | 4.8 |
| The same as it would be inlined: static slab, rseq offset loaded, class constant | 3.93 (23) | 8.00 | 4.7 |
| The same with the class in a register | 4.02 (25) | 8.00 | 5.5 |
| Per-CPU intrusive free list under rseq | 2.85 (18) | 8.00 | 5.1 |
| tcmalloc's `Pop` and `Push`, instruction for instruction, with its prefetches | 4.82 (26) | 8.00 | 5.8 |
| tcmalloc's without the prefetches | 4.50 (23) | | 5.6 |
| mimalloc-shaped memory traffic | 5.9 to 7.6 (20) | 8.7 | 6.6 to 7.5 |
| Thread-local free list with its head at `fs:[constant]` | 7.0 to 7.2 (8) | 8.00 | 8.7 to 8.9 |
| Thread-local magazine with its top at `fs:[constant]` | 15.9 (11) | 15.9 | 8.3 to 11.5 |

Reading it:

- The two cheapest shapes are within 0.4 cycles of the 2.00 that a bare magazine with
  everything hoisted achieves. That is the floor: two dependent arithmetic operations.
- rseq adds about 1.6 cycles to an inlined pair. Dissecting it: arming the critical section
  (one `lea`, one store) and finding the per-CPU slab (one load, a shift, an add) each take the
  loop from 2.00 to 4.00 alone, and both together still cost 4.00. The cause is the uop count
  (section 6), not either step.
- Through calls, rseq costs nothing extra: every shape that has no slow forwarding costs 8.00.
- In bursts, an array beats a list by 1.7 cycles per pair even with everything in L1, because
  popping a list is a pointer chase (4 cycles per load) and popping an array is an add. A list
  also makes cache misses on cold objects serial; that case was not measured.
- The last two rows are the same code as the first two with one change: the hot word is
  addressed as `fs:[constant]` instead of through a register. Section 4 explains the 3 to 7
  times slowdown.
- The mimalloc-shaped row is an approximation of mimalloc's loads and stores (heap, direct
  page table, page free list, a used counter, a page-map lookup and owner check on free). Real
  mimalloc is in section 10.

## 4. Store-to-load forwarding

A fast path stores its state and the next operation loads it back. Each row is the loop
`load; add 1; store` to one location. 1.00 means the store-to-load hop is free (the add is the
whole cycle).

| Location | 64-bit | 32-bit | 16-bit `movzx` | 8-bit `movzx` |
|---|---:|---:|---:|---:|
| `[reg + disp]` | 1.00 | 1.00 | 8.00 | 8.00 |
| `[rsp + disp]` | 1.00 | | 8.00 | |
| `[rip + symbol]` | 1.00 | | 8.00 | |
| `[reg + index*8 + disp]` | 1.00 | | | |
| `fs:[reg + disp]` | 1.00 | | | |
| `fs:[disp]`, the form rustc and C compilers emit for thread-locals | 6.5 to 7.1 | 6.7 to 7.2 | 8.00 | 8.00 |
| `ds:[absolute]`, no segment | 5.9 to 7.1 | | | |

Other cases:

| Case | Cycles |
|---|---:|
| `add [reg + disp], 1` as one instruction, 64, 32 or 16-bit | 1.00 |
| `add qword fs:[disp], 1` | 6.4 |
| Store 32 bits, load the low 16 with `movzx` | 8.00 |
| Store 64 bits, load the low 32 | 1.00 |
| 16-bit load with `mov ax, [mem]` (merging, no extension) | 3.00 |
| Base register copied, recomputed, or reloaded from memory every iteration | 1.00 to 1.13 |
| Load through one register, store through another holding the same address | 1.00 |
| An unrelated store between the load and the store | 1.00 |
| Pointer chase `mov rax, [rax]` / with an index register | 4.0 / 5.0 |

So the free hop needs two things: a 32 or 64-bit access (or a read-modify-write instruction of
any width), and an address with a base register. An address made only of a displacement gets
the ordinary forward of 5 to 7 cycles, with or without a segment prefix. Loading the thread
pointer into a register first (`mov r, fs:[0]`, then `[r + offset]`) restores the free hop.

Inference: this is the core's memory renaming, which forwards the stored register to the load
at rename time; it appears to be keyed on addresses with a base register and on loads that do
not extend.

## 5. Atomics, calls and core widths

| Operation | Cycles |
|---|---:|
| `lock cmpxchg`, `lock xadd`, `xchg [mem]`, `lock add`, `lock bts`, uncontended | 20.0 each |
| Two `lock add` to different cache lines | 40.0 |
| `lock add` plus 16 dependent adds (the adds alone: 16.0) | 20.0 |
| Lock-free stack pop then push (two compare-and-swap) | 41.0 |
| Queue push with one `xchg` and one store | 24.0 |
| `call` + `ret`, direct or indirect | 4.0 |
| Two calls | 8.0 |
| `call` + `ret` plus 8 dependent adds (the adds alone: 8.0) | 8.0 |

An atomic is a 20-cycle operation that serializes against other atomics but overlaps
unrelated work. A call and return take 4 cycles of the front end and also overlap unrelated
work, so the cost shows fully only when calls come back to back.

Widths, measured as independent work per loop iteration:

| Resource | Measured | Evidence (operations per iteration: cycles) |
|---|---|---|
| Loads | 4 per cycle | 4: 1.01, 6: 1.50, 8: 2.04 |
| Stores | 2 per cycle | 2: 1.00, 4: 2.00, 6: 3.00 |
| Taken branches | 2 per cycle | 2: 1.00, 3: 1.50 |
| Mixed uops | about 8 per cycle | 2 loads + 2 stores + 3 adds + branch: 1.08 |
| Register arithmetic alone | about 5 per cycle | 6 uops: 1.26, 8: 1.61, 12: 2.39 |

## 6. The front end: twelve-uop windows

Loops and functions near the floor turned out to be sensitive to small changes: one added
instruction could cost a whole cycle. The cause is the number of fused uops in a straight-line
run (from a branch target to the next taken branch), and to a smaller degree whether the run
crosses a 64-byte line. Byte alignment by itself does nothing.

A loop of N fused uops with no dependency between iterations:

| N | 2 to 8 | 9 | 10 | 11 | 12 | 13 to 24 | 25 to 30 |
|---|---:|---:|---:|---:|---:|---:|---:|
| Cycles per iteration | 1.00 | 1.13 | 1.25 | 1.38 | 1.50 | 3.00 | 5.00 |

The filler was one-byte `nop`s in one sweep and two-byte register moves in another; the
results are identical. With a 2-cycle dependency chain in the loop the figures are 2.00 up to
12 uops, 3.00 for 13 to 24, and 5.00 from 25. With 4-byte `lea`s as filler, where the body
crosses a 64-byte line at 16 uops, they are 3.00 for 13 to 15 and 4.00 for 16 to 22.

A function of N fused uops (including its `ret`) called from a loop:

| Body | Cycles per call |
|---|---|
| nops or 2-byte moves, all inside one 64-byte line | 4.00 for N up to 24, 6.00 for 25 to 30 |
| 4-byte `lea`s | 4.00 for N up to 16 (61 bytes), 5.00 from N = 17 (65 bytes), rising slowly after 27 |

Where a function sits inside its line:

| Function | Entry at 64n + offset | Cycles per call |
|---|---|---:|
| 6 uops, 21 bytes | any offset, including straddling a line | 4.00 |
| 11 uops, 41 bytes | any offset, including straddling a line | 4.00 |
| 16 uops, 61 bytes | 0, or 16 to 60 | 4.00 |
| 16 uops, 61 bytes | 4, 8 or 12 (only the last one to three instructions spill over) | 5.00 |

The magazine pair of section 3 (11 fused uops, 51 bytes) under perturbation:

| Change | Cycles |
|---|---:|
| Loop start moved to every byte offset 0 to 63 | 2.00 at all 64 |
| One multi-byte nop (one uop, 1 to 9 bytes) at the top | 2.00 to 2.06 |
| One nop before any instruction (12 uops) | 2.00 to 2.14 |
| One nop between a `test` and its `jz`, or between `dec` and `jnz` (breaks the fusion: 13 uops) | 3.00 |
| Two to twelve nops at the top, in the middle, or at the end (13 to 23 uops) | 3.00 |

Inference: the op cache hands over two entries of up to six fused uops per cycle. A run that
fits in two entries is delivered at once; each further pair of entries costs two more cycles;
and a line boundary forces an extra entry, which matters only when it pushes the count past
what the cycle budget covers. A call and return give a budget of 4 cycles, which hides runs of
up to 24 uops.

## 7. Kernel memory primitives

Wall-clock, single-threaded, two sessions; ranges are the two results.

| Operation | Cost |
|---|---|
| First touch of fresh memory, 4 KiB pages | 535 to 566 ns per page |
| Touch again after `MADV_DONTNEED` | 484 to 490 ns per page |
| `MADV_DONTNEED` then touch, one page | 586 to 588 ns round trip |
| `MADV_POPULATE_WRITE` (fault in a range with one call) | 346 to 418 ns per page |
| First touch in a `MADV_HUGEPAGE` region | 60 to 61 us per 2 MiB, that is 117 to 119 ns per 4 KiB |
| `MADV_DONTNEED`, 4 KiB pages | 81 to 96 ns per page |
| `MADV_FREE`, 4 KiB pages | 79 to 80 ns per page; touching again without reclaim 85 to 88 ns per page |
| `MADV_DONTNEED` of whole huge pages | 2.0 to 6.4 us per 2 MiB |
| `MADV_DONTNEED` of 64 KiB inside a huge page (splits it) | 5.2 us |
| `MADV_COLLAPSE` of 4 KiB pages into a huge page | 223 to 398 us per 2 MiB |
| `mmap` + `munmap` of 2 MiB, untouched | 0.79 us |
| `mprotect` to none and back, 2 MiB | 0.83 to 0.92 us |
| `MADV_GUARD_INSTALL` + `MADV_GUARD_REMOVE`, one page | 0.20 us |
| Zeroing memory that fits L1 / L2 / L3 / none | 316 to 336 / 243 / 141 to 146 / 30 to 33 GiB/s |

A kernel profile of the 4 KiB fault path shows about 2,100 kernel cycles per fault, of which
30 percent is spinlock acquisition, 10 percent list checking, and 7 percent clearing the page.
The cost is bookkeeping, not zeroing, which is why faulting 512 pages at once as one huge page
is about 4.6 times cheaper per byte.

Fresh memory at 4 KiB granularity costs 0.13 to 0.14 ns per byte: about 8.6 ns, or 49 cycles,
for each new 64-byte object. That is more than ten times the fast path.

### Making memory resident: faults, populate and huge pages

Measured 2026-10-17 with `bench/roofline/oscost`'s `faults` binary (`cargo build --release`,
then `RF_CPUS=13 target/release/faults single|threads|advice|split|khuge`), under
`scripts/memlimit`. Every variant writes one byte to every page of a fresh 64 MiB mapping, so
all end with the same memory resident; variants run interleaved in a new order each round,
pinned to CPU 13 with its sibling 29 idle, and each figure is the median of 3 to 9 rounds,
from per-thread counters. Two sessions; ranges are the two medians. The machine was shared:
CPUs 0 to 7 and their siblings were mostly busy with other work, CPUs 8 to 15 mostly idle.

| One thread | ns per 4 KiB | kernel cycles per 4 KiB |
|---|---:|---:|
| Demand fault: write each page | 523 to 582 | 2,310 to 2,530 |
| `MADV_POPULATE_WRITE` 1 page at a time, then write | 524 to 549 | 2,610 to 2,710 |
| populate 4 pages at a time | 441 to 486 | 2,220 to 2,440 |
| populate 16 pages | 418 to 457 | 2,130 to 2,330 |
| populate 64 pages | 395 to 440 | 2,060 to 2,260 |
| populate 512 or 4096 pages | 409 to 464 | 2,120 to 2,360 |
| Huge-page fault (`MADV_HUGEPAGE`), free 2 MiB blocks available | 151 to 214 | 730 to 1,050 |
| Huge-page fault, no free 2 MiB block (below) | 660 to 780, single runs 130 to 2,000 | |
| Write to a page already resident | 8 to 9 | 1 |

- **Populate saves the exception and nothing else.** Of a 4 KiB fault's 520 to 580 ns,
  populating saves 105 to 140 ns, a fifth to a quarter, and 16 pages a call already save all
  of it. A populate runs the same fault handler once per page from inside the kernel
  (`faultin_page_range`, `handle_mm_fault`): it skips the exception entry and the VMA lookup
  (in the demand-fault profile `asm_exc_page_fault` 7%, `mas_walk` 11%, `lock_vma_under_rcu`
  4%), and still allocates, zeroes, locks the page table and adds the page to the LRU for each
  page. In its profile the page-table lock is 28% and clearing the page 9%.
- **A huge page is bound by zeroing.** Clearing is 85% of its profile; 151 ns per 4 KiB is
  about 27 GB/s, which is what one core writes to DRAM. So 4 KiB pages cost about 400 ns of
  bookkeeping per page on top of the zeroing that every way of getting fresh memory pays.
- **Huge pages need free 2 MiB blocks.** THP defrag is `madvise` here, so a fault in an
  advised range with no free 2 MiB block compacts memory in the faulting thread and falls back
  to 4 KiB if that fails. Late in the session page cache filled most of memory and
  `/proc/buddyinfo` showed no free block of order 9; huge-page faults then cost 660 to 780 ns
  per 4 KiB (more than a demand fault) and one in five fell back. mimalloc's Vec growth went
  from 5.8 to 28 cycles an element in that state, asmalloc's with huge pages for grown
  blocks from 4.1 to a median of 9.6 (3.6 to 21.6 over five runs), against 10.3 with none.

With several threads at once, each on its own mapping (CPUs 13, 24, 25, 26, one L3; builds
started on CPUs 4 to 7 during the 4-thread row):

| ns per 4 KiB | 1 thread | 2 threads | 4 threads |
|---|---:|---:|---:|
| demand fault | 539 | 604 | 761 |
| populate 64 pages | 425 | 492 | 700 |
| huge-page fault | 151 | 290 | 581 |

Faults scale reasonably: the page-table locks are per table and the page allocator's lists
are per CPU. Huge pages scale worst because they are bound by bandwidth: the four threads
together zero about 28 GB/s, which is roughly what one Zen 5 core complex can write. At four
threads a huge page is only 1.3 times cheaper than a 4 KiB fault.

**Advice around a slot** (`faults advice`): 4000 cycles over a committed region advised
against huge pages, with 4 KiB pages resident in every other 16 MiB slot, of advising one
slot's whole huge pages for huge pages, filling it, purging it and taking the advice back, up
to five slots advised at once, half of them starting off a huge-page boundary.
`MADV_HUGEPAGE` cost 0.95 us and `MADV_NOHUGEPAGE` 1.29 us; filling cost 61 us per 2 MiB and
`MADV_DONTNEED` 2 us per 2 MiB. `/proc/self/maps` had 31 lines before, at most 41, and 31
after: the kernel merges the mapping back completely. In asmalloc itself, 2000 vectors grown
to 2 to 32 MiB with up to five alive left 6 more lines than before, all from regions assigned
and commits extended during the run, none from advice.

**Purging part of a huge page returns nothing** (`faults split`): 256 MiB faulted as huge
pages, then the first half of each purged with `MADV_DONTNEED` (7.6 us a call): the resident
set fell to 130 MiB but the cgroup stayed charged 257 MiB, and still was two seconds later.
The kernel only queues the page to be split later, under memory pressure. Purging the second
halves returned it all; purging whole huge pages returns them at once, 2.8 us per 2 MiB.

**khugepaged collapses sparse memory** (`faults khuge`): 64 MiB with one page written per
2 MiB, then advised for huge pages, became 64 MiB resident within 40 seconds
(`max_ptes_none` is 511, the default). Advice must never cover memory used sparsely, even
memory that was faulted in 4 KiB pages before the advice.

Multi-size THP (16 KiB to 1 MiB folios, which would fault 4 to 256 pages at a time without
the 2 MiB granularity) is off on this machine at every size, and an unprivileged process
cannot turn it on.

### Moving a large block: `mremap` against copying (2026-10-17)

The question: a large block that outgrows its slot is copied, which makes a sparsely written
block fully resident and is slow; glibc moves such blocks with `mremap`. Can asmalloc move
pages with `mremap(old, len, len, MREMAP_MAYMOVE | MREMAP_FIXED | MREMAP_DONTUNMAP, new)` and
later put a fresh `mmap(MAP_FIXED)` over the destination ("heal") so that the heap's mappings
merge again?

**Answer.** It works, with a stricter recipe than first proposed, and it is fast. But a copy
that skips all-zero source pages gets all of the memory benefit with no new mechanism.
Remapping buys speed only: 1.6 to 2.9 times the reallocs per second on blocks of tens of
megabytes and up.

Method. Programs in `bench/roofline/oscost/src/bin/` (`remap_heal`, `remap_cost`,
`remap_model`) build a heap like the allocator's: one `PROT_NONE`, `MAP_NORESERVE`
reservation, regions advised `MADV_NOHUGEPAGE` and made writable from their start. All ran
under `scripts/memlimit` with 8 GiB or less, pinned to CPU 13 (second thread on CPU 12).
Those two cores and their siblings were idle when sampled; the machine's load average was 12
to 17 from other work, and repeats of the workload model differ by up to 15%. Times are the
fastest of 3 to 40 repeats. Kernel behaviour was checked against the v7.2 sources
(`mm/mremap.c`, `mm/vma.c`, `mm/mmap.c`, `mm/madvise.c`, `mm/util.c`).

#### What the kernel does

- **The move.** The destination's pages in the moved range are freed and replaced; the rest
  of the destination slot keeps its old bytes. The source stays mapped, is not split, and
  reads as zero, so it can be recorded as zero. Resident memory does not rise. The moved
  range arrives as a mapping of its own that cannot merge with its neighbours: two more
  mappings per moved-in block, never more.
- **What blocks the merge.** The moved pages keep their original file offset, and the moved
  mapping carries the source region's `anon_vma`. `MADV_DONTNEED` does not help. A fresh
  `mmap` merges only once it has the neighbours' protection, `MAP_NORESERVE` and the
  `MADV_NOHUGEPAGE` advice; `PROT_NONE` then `mprotect` also works. The wipe-on-fork page is
  not in an object region and plays no part. The source needs no fresh `mmap` unless it was
  itself moved into.
- **The heal does not always return the count to where it was.** A healed range that is
  written while it has a moved-in mapping on both sides gets an `anon_vma` of its own, and
  the split between it and the region's mapping is then permanent. With whole-slot heals,
  random cycles settled at 57 to 69 mappings against a baseline of 13 (bounded by the number
  of slots, not creeping). Three rules make the return exact, 13 after every round of 28,000
  moves: heal exactly the moved-in range; never remap the last page of a destination slot,
  so every slot keeps a page of the region's own mapping; write one page of a region before
  the first move into it.
- **A block that grows in place after arriving lies in two mappings.** Linux 7.2 will move
  several mappings in one call, but not atomically: four below `vm.max_map_count`, a block in
  three mappings came back `ENOMEM` with two pieces moved, the third not, and the third's
  destination unmapped. A call on one mapping is refused cleanly within three of the limit.
  So a move must be one call on a range known to be one mapping. The kernel can be asked:
  `mremap(p, len, len + 4096, 0)` fails with `EFAULT` if the range crosses a mapping and
  `ENOMEM` if it does not, changing nothing (it must not be used on a range that ends where
  unmapped space begins, where it succeeds and grows the mapping). The program can split a
  block's mapping itself with `madvise` or `mlock`, so this cannot be known without asking.
- **A refused move can leave the destination unmapped.** The kernel unmaps the destination
  first. A failure after that (commit charge under strict overcommit, or no kernel memory)
  returns `ENOMEM` with the source intact and a hole where the destination was. Shown with an
  accounted mapping larger than RAM. The hole has to be mapped again before anything else
  lands in it.
- **`mlock`.** The move clears `VM_LOCKED` on the whole source mapping, not just the moved
  range. In a process that called `mlockall`, one move would unlock a region. `MADV_COLD` on
  one source page fails with `EINVAL` if the mapping is locked, which is a cheap test.
- **Fork.** Parent and child each keep a correct view when the other moves the block. In
  the parent, heals still merge. In a forked child they never merge with inherited mappings
  (the kernel refuses to merge with a mapping copied by fork), so a child settles at up to
  two extra mappings for each slot it has moved into: 490 against 13 with 288 slots, stable
  over 5 rounds. In a child, blocks also lie in several mappings without having been moved.
  Growing an inherited region by `mprotect` in a child, which the allocator already does,
  costs one extra mapping per region, not one per step.
- **Huge pages.** With both ends aligned to 2 MiB, huge pages move whole. Otherwise they are
  split. A destination holding huge pages is simply replaced.
- **Memory cap.** A 505 MiB block moves under a 512 MiB cgroup cap; a copy needs room for
  one piece. An unaligned move allocates page tables (about 1 MiB per 512 MiB), and at the
  very edge of the cap that is an OOM kill, as any page fault would be.
- **Overcommit** (from the source). In modes 0 and 1 the heap is `VM_NORESERVE` and a move
  charges nothing. In mode 2 the kernel ignores `MAP_NORESERVE`, writable parts are
  accounted, and the move charges the full length after uncharging the destination: no net
  change, but it can fail, leaving the hole above.
- **Mapping count.** Up to two extra mappings per live moved-in block. The default
  `vm.max_map_count` is 65530 (1048576 on this machine), and the allocator's own `mprotect`
  needs headroom, so the number of live moved-in blocks needs a cap in the low thousands.

#### Cost

Microseconds, fastest of the repeats. "Copy" is the present move: 2 MiB pieces, the source's
pages returned behind each. "Sparse copy" is the same, skipping source pages that are all
zero. Dense means every page written; sparse means one byte per MiB.

| Block | `mremap`, ends at 2 MiB | `mremap`, ends at different offsets | Copy, destination not resident | Copy, destination resident | Sparse copy |
|---|---:|---:|---:|---:|---:|
| 64 KiB dense | 1.5 | 1.5 | 5.9 | 0.4 | 5.9 |
| 256 KiB dense | 1.8 | 1.8 | 21 | 1.6 | 21 |
| 1 MiB dense | 3.0 | 3.0 | 90 | 13 | 90 |
| 16 MiB dense | 10 | 31 | 2,100 | 1,100 | 2,300 |
| 256 MiB dense | 32 | 390 | 43,000 | 26,000 | 46,000 |
| 1 GiB dense | 94 | 1,500 | 177,000 | | 186,000 |
| 4 GiB dense | 180 | 6,100 | 681,000 | | 696,000 |
| 1 MiB sparse | 1.9 | 2.0 | 128 | 47 | 49 |
| 256 MiB sparse | 29 | 131 | 48,000 | 23,000 | 13,600 |
| 4 GiB sparse | 176 | 2,400 | 775,000 | | 217,000 |

- Around the `mremap`: the one-mapping probe 0.1 to 2, the lock probe 0.6 to 4, and at free
  the fresh `mmap` and advice 0.8 to 15 on top of the purge any freed block may pay.
- With both ends on a 1 GiB boundary the kernel moves whole upper-level tables: 11 for
  1 GiB or 3 GiB. Ends at the same odd offset cost the same as aligned ends. Classes of
  8 MiB and up are multiples of 2 MiB, so only blocks under 8 MiB move page by page.
- A destination with resident pages costs the `mremap` their release under the lock: 38 ms
  for 1 GiB. Purge it first.
- Resident after moving a 256 MiB sparse block: 1 MiB by `mremap`, 256 by copy, 1 by sparse
  copy. The sparse copy pays a read fault per untouched source page, 0.19 us, which maps the
  shared zero page and adds no resident memory.
- **Crossover.** Against a destination whose pages are not resident, `mremap` wins at every
  large size. Against a resident one (a retained slot), the copy wins up to about 256 KiB.
  The workload below is insensitive to the threshold from 64 KiB to 16 MiB.

**What it does to other threads.** A second thread's longest pause during one move, middle
of 7 to 25 repeats:

| Second thread | `mremap` 64 MiB unaligned | `mremap` 1 GiB aligned | `mremap` 1 GiB unaligned | Copy, 1 GiB |
|---|---:|---:|---:|---:|
| Computing only (sees the TLB shootdowns) | 1.3 us | 2.6 us | 2.9 us | 5.3 us |
| Page faults in a mapping of its own | 2.6 us | 10 us | 5.7 us | 18 us |
| Page faults in the source's region | 114 us | 100 us | 1,550 us | 13 us |
| Page faults in the destination's region | 113 us | 100 us | 1,530 us | 12 us |
| `mmap` and `munmap` | 157 us | 396 us | 2,090 us | 760 us |

A thread that faults in either region, or needs the address-space lock, waits for the whole
call: about 0.1 ms per GiB aligned, 1.5 ms per GiB page by page (which the class sizes limit
to blocks under 8 MiB, about 12 us). Other threads are not held. The copy never holds anyone
for long. The `mmap` row is noisy in every column.

#### The soak's realloc workload

`remap_model` replays the soak's `realloc` mode (growth chains to a random top and back
down, written and checked as the soak does) against a model of the large-object rules, one
thread. Three runs at 256 MiB; ranges are the runs.

| Way of moving | Reallocs per second, blocks to 256 MiB | Peak resident | Reallocs per second, to 1 GiB | Peak resident |
|---|---:|---:|---:|---:|
| glibc | 2,460 to 2,490 | 65 MiB | 2,170 | 69 MiB |
| copy (now) | 1,260 to 1,280 | 136 MiB | 480 | 506 MiB |
| sparse copy | 1,220 to 1,380 | 72 MiB | 810 | 77 MiB |
| one `mremap`, sparse copy for the rest | 1,820 to 2,120 | 71 MiB | 1,420 | 75 MiB |

With blocks to 16 MiB all four are within 10% (4,440 to 5,000 per second, 18 to 27 MiB).
The model reproduces the soak's shape (glibc 1.9 times the copy's rate and half its peak),
not its numbers.

#### What follows

1. **Sparse copy fixes the memory problem by itself**: peak within 12% of glibc's, against
   2 to 7 times. It needs the destination to read as zero (purge it first unless the back
   end already knows it is zero) and no other state, no new system call and no new failure
   path. It does not speed up dense blocks.
2. **Remapping adds only speed**, 1.6 times at 256 MiB and 2.9 times at 1 GiB over the copy,
   and still short of glibc. Its price in mechanism: three new OS calls and their simulated
   model (mappings, not just pages), a word per large slot, a cap on live moved-in blocks,
   and these rules, each of which the experiments showed is needed:
   - one `mremap` per move, of the largest piece of the block that the probe says is one
     mapping; sparse copy for the rest, after the `mremap`, so a refusal leaves the source
     whole;
   - never the last page of the destination slot; a region written once before its first
     move in;
   - purge a destination that is not known zero before the call;
   - skip the remap if the lock probe fails;
   - on refusal, map the destination range again; if that fails too, never use the slot;
   - at free, purge, then a fresh `mmap` and advice over exactly the moved-in range;
   - no guarantee in a forked child, where the probe will often say no and heals do not
     merge.

## 8. Moving cache lines between cores

Wall-clock, single run each in two sessions.

| | SMT sibling | Same L3 | Other L3 |
|---|---:|---:|---:|
| Strict ping-pong of one line, per hand-off | 16 to 21 ns | 16 to 22 ns | 76 to 79 ns |
| Contended `fetch_add` on one line, per operation | 12 to 15 ns | 10 to 13 ns | 14 ns |
| Extra cost to a producer per 64-byte object when a consumer on the other core read and wrote it first | 0.8 to 0.9 ns | 7 to 8 ns | 27 ns |

Uncontended `fetch_add`: 3.5 ns.

The last row is what an allocator pays when a free on another core writes into the object (an
intrusive free list does) and the object is then reused by the core that allocated it.

## 9. What rustc generates

### Thread-locals

A `thread_local!` with a `const` initializer and no destructor compiles to direct `fs:[constant]`
accesses, the slow form of section 4. Two floor allocators that differ only in where the
free-list heads live show the effect in compiled Rust, on a `Box` whose pointer escapes:

| Floor (fat LTO, everything inlined) | Cycles per pair |
|---|---:|
| Heads in a `thread_local!` array | 7.3 to 7.4 |
| Heads in a heap struct behind a thread-local pointer | 4.0 |

When the compiler can see an allocation and its free together with nothing escaping, it folds
the list traffic away and the pair costs 1.3 to 2.1 cycles for either floor. An `asm!` fast
path is opaque to that folding.

### Inline assembly

- A `label` operand (a jump from the assembly to a Rust block) cannot be combined with a real
  output on stable 1.99.1 (rust-lang/rust issue 119364). It can be combined with clobbered
  registers. So a push sequence can branch straight to its Rust slow path, but a pop sequence
  must report a miss through its result, at the cost of one extra fused test and branch, or
  call the slow path from inside the assembly.
- A full rseq critical section fits in one `asm!` block: the descriptor goes in a
  `.pushsection`, the abort landing pad with its signature in another, and numeric labels tie
  them together. Each inlined copy gets its own descriptor.
- An operand that the optimizer knows to be constant (the size class of a `Box<T>`) still
  arrives in a register; `const` operands accept only Rust constant expressions.
- Around an inlined rseq sequence the compiler emitted one load of `__rseq_offset` per
  operation. It hoists the address of that variable out of loops but reloads the value, since
  the assembly block may write memory. The slab address was loaded from the GOT once per
  function.
- Compiled this way a pop is 13 fused uops and a push 11. A `Box<[u64; 4]>` allocate,
  initialize, free loop costs 5.8 cycles per iteration, against 4.1 for the plain-Rust
  thread-local list. The bare assembly pair, which initializes nothing, costs 4.0.

### When the fast path is inlined

`__rust_alloc` and its siblings are generated in the crate that declares `#[global_allocator]`.
Whether a call site gets the fast path inlined depends on whether the optimizer can see that
crate from the call site. The experiment (`bench/roofline/inline`) has an allocator crate with
`#[inline]` fast paths, a dependency crate, and a binary crate that declares the allocator and
is large enough to be split into two codegen units.

| Profile | Binary crate, same unit as the allocator shim | Binary crate, other unit | Generic code instantiated in the binary crate | Dependency crate | Inside std (`format!`) | Extra call per allocation |
|---|---|---|---|---|---|---|
| `lto = "off"` | inlined | call | call | call | call | yes |
| default `release` | inlined | inlined | inlined | call | call | yes |
| `codegen-units = 1` | inlined | inlined | inlined | call | call | yes |
| `lto = "thin"`, 16 units or 1 | inlined | inlined | inlined | inlined | call | no |
| `lto = "fat"` | inlined | inlined | inlined | inlined | partly | no |

The table is the same for a plain-Rust fast path and an `asm!` one. Details:

- The extra call is to `__rust_no_alloc_shim_is_unstable_v2`, an empty function that std calls
  before every allocation. Without cross-crate LTO it is a real call through the GOT.
- Default release builds already run thin LTO across one crate's own codegen units, which is
  why the binary crate's other unit is covered. They do not look across crates.
- Thin LTO reaches dependency crates and removes the extra call. Of the 73 standard-library
  functions in the binary that call the allocator out of line without LTO, 34 to 38 still do
  under thin LTO and 30 under fat LTO. For `format!` under fat LTO the free of the string is
  inlined at the call site and the allocation inside the formatting code is not.
- The number of codegen units does not matter under thin LTO.
- A library cannot set any of this for its users: profiles are read only from the workspace
  being built.

Cycles per `Box` allocate+free pair in that experiment:

| Site | Plain-Rust list | `asm!` rseq magazine |
|---|---:|---:|
| Inlined, no extra call (thin or fat LTO) | 4.1 | 5.8 |
| Inlined, extra call (default release, binary crate) | 9.2 | 12.3 |
| Out of line (default release, dependency crate) | 12.7 | 15.3 |

The empty call alone costs 5 to 6.5 cycles per pair, more than the allocation.

### A thin-LTO hazard for assembly fast paths

LLVM's thin LTO will not import a function that contains inline assembly out of a module that
also has a private symbol in `llvm.used` or in module-level assembly. `__rust_alloc` contains
the inlined fast path, so with an `asm!` fast path the rule bites whenever the crate declaring
the allocator has a private `#[used]` static (as crates that register constructors or
inventories do) or a `global_asm!` block with a local symbol:

| Crate that declares the allocator | Profile | Binary, other unit | Generic | Dependency |
|---|---|---|---|---|
| Binary crate with a `#[used]` static or `global_asm!` | default `release` | call | call | call |
| | `lto = "thin"` | call | call | call |
| | `lto = "thin"`, 1 unit | inlined | inlined | call |
| | `lto = "fat"` | inlined | inlined | inlined |
| A separate crate containing only the declaration | `lto = "thin"` or `"fat"` | inlined | inlined | inlined |
| | no LTO | call | call | call |

A plain-Rust fast path is not affected (measured with the same `#[used]` static). Declaring
the allocator in a crate of its own makes thin LTO reliable, and gives up inlining in builds
without LTO, where the binary crate's own sites would otherwise have had it.

## 10. Existing allocators

Through Rust's `GlobalAlloc`, 32-byte objects unless noted. mimalloc 3.5.3 is compiled from
source and called as the `mimalloc` crate calls it (`mi_malloc_aligned`, `mi_free`), or with
the size passed to free. jemalloc is Fedora's 5.3.0, called as `tikv-jemallocator` calls it
(`malloc` or `mallocx`, and the sized `sdallocx`). The C libraries are never part of LTO;
"LTO" changes only the Rust side.

Cycles per allocate+free; each cell is default release / fat LTO:

| | Pair, constant size | Pair, `Box` | Batch of 32 | Churn over 10,000 objects of 16 to 512 bytes |
|---|---|---|---|---|
| glibc | 22 / 11 | 26 / 11 | 39 to 41 / 33 | 51 to 53 / 47 |
| mimalloc | 15.5 to 18.2 / 12.4 to 13.7 | 17.5 / 12.3 to 13.3 | 14.6 to 15.6 / 11.6 | 38 / 35 |
| mimalloc, sized free | 14.4 to 15.5 / 10.3 | 14.6 / 10.3 | 13.6 / 9.5 | 32 to 34 / 30 to 31 |
| jemalloc | 15.1 / 12.1 | 15.8 / 12.1 | 18.6 / 15.5 | 43 / 40 |
| Floor, list behind a thread-local pointer | 7.5 / 2.1 | 9.1 / 4.0 | 16 / 8.7 | 19 / 16 |

The incumbents execute 57 to 95 instructions per pair. The floors execute 12 to 28.

### Threads freeing each other's objects

Nanoseconds per object that the two sides spent working (allocate plus free; time spent
waiting for the other side is excluded). Fat-LTO builds, bursts of 32 objects, two cores.
Same-L3 ranges are over two or three sessions; the other-L3 columns are one session.

| | One thread, no hand-off | Pipeline, same L3 | Exchange, same L3 | Pipeline, other L3 | Exchange, other L3 |
|---|---:|---:|---:|---:|---:|
| glibc | 6.6 to 6.7 | 110 to 135 | 115 to 124 | 149 to 153 | 140 |
| mimalloc | 2.6 to 2.7 | 8.6 to 9.0 | 15.7 to 19.4 | 64 to 67 | 32 |
| mimalloc, sized free | 1.9 to 2.1 | 9.1 to 9.6 | 15.3 to 16.2 | 65 to 66 | 30 |
| jemalloc | 3.6 | 10.2 to 11.0 | 8.5 to 11.3 | 24 to 31 | 17 |
| Floor | 0.8 | not valid | 2.5 to 2.6 | not valid | 9.0 |

- Pipeline: one thread allocates, the other frees. Exchange: both threads allocate for the
  other and free what they receive, so each allocates and frees at the same rate but never
  frees its own objects. A ring of four threads gives the same picture as two (mimalloc 15 to
  19, jemalloc 11, floor 2.7). With bursts of 256 instead of 32 the exchange costs mimalloc 9
  to 15 and jemalloc 10 to 11.
- The floor keeps whatever it is given on a thread-local list. In the exchange that is a fair
  bound. In the pipeline it grows without limit, so it is not a bound there.
- Within one L3, mimalloc and jemalloc are close on the pipeline, and mimalloc costs 1.5 to 2
  times jemalloc on the exchange. Across L3s mimalloc costs 2.1 to 2.7 times jemalloc on the
  pipeline and about 1.8 times on the exchange.
- Both leave a wide gap to the floor on the exchange: 3 to 4.5 times for jemalloc, 6 to 7.5
  times for mimalloc.

Counters for the same-L3 exchange, per object, over the whole process (both threads and the
hand-off ring):

| | Instructions | Line fills from beyond L2 | Line fills from L2 |
|---|---:|---:|---:|
| Floor | 47 | 0.93 | 0.30 |
| jemalloc | 173 | 1.64 | 0.64 |
| mimalloc, sized free | 89 | 1.50 | 1.16 |

Inference: mimalloc returns every object to the page it came from, with one locked instruction
per free (3.5 ns) and extra line movement on both sides; about a quarter of its cost above the
floor is the atomic and the rest is cache lines. jemalloc keeps received objects in the
freeing thread's cache and reuses them there, so it moves fewer lines, but it runs twice the
instructions. Across L3s, where a line costs four times as much to move, line movement decides
the ranking.

## 11. Design rules that follow

1. Keep hot mutable state in 32 or 64-bit words reached through a base register. No 8 or
   16-bit hot fields, and no state addressed as `fs:[constant]`. Compilers choose that form
   for thread-locals on their own, which is one reason to write the fast path in assembly.
2. No atomic and no shared cache line per operation. One atomic costs five inlined pairs;
   amortize it over a magazine.
3. Do not write to freed memory. On a cross-core free it costs the next user of the object 7
   to 27 ns.
4. Prefer arrays of pointers to intrusive lists in the per-thread or per-CPU cache.
5. Lay out each out-of-line entry point at a 64-byte boundary with its hot path inside that
   line, at most 12 fused uops to the `ret` where possible and never more than 24. Keep each
   compare adjacent to its jump. Put slow paths out of line. These are checkable on the emitted
   bytes.
6. Inline the fast path for builds with LTO, recommend thin or fat LTO, and offer the
   allocator declaration in a crate of its own. Out of line, the floor is the call itself, 4
   cycles per operation.
7. A pop written in `asm!` reports a miss through its result; a push can jump to its slow
   path.
8. In the back end, the fault is the cost. Reuse memory before touching fresh pages, grow
   with huge pages once a region is large, release in whole 2 MiB units where huge pages are in
   use, and do not collapse or split huge pages on a hot path.
9. Per-CPU state under rseq is affordable: about 1.6 cycles per inlined pair and nothing out
   of line.

## 12. Caveats

- One CPU model, one kernel build, one machine. Other cores forward, fetch and lock
  differently; the rules in section 11 are safe elsewhere, but the numbers are not.
- These are tight loops. Real programs put other work between an allocation and its free, so
  the dependency chains and front-end windows measured here are worst cases, not averages.
- Kernel and cross-core figures are single runs, repeated in two sessions; the tables give
  both. Several vary by 20 percent or more (`MADV_COLLAPSE` by nearly two times).
- Frequency was not pinned. Cycle counts are immune; nanosecond figures assume about 5.65 GHz.
- The tcmalloc row is its instruction sequence in isolation, with the cached slab pointer in
  an ordinary thread-local. google/tcmalloc itself was not built or run.
- The mimalloc-shaped row is a hand approximation and varied between sessions.
- Floors never return memory and serve only small sizes.
- The cross-thread workloads use one object size and two to four threads. They show a
  mechanism; they are not a model of a server.
- The front-end and forwarding explanations are inferred from the measurements, not taken
  from documentation.

## 13. The proved routines: encoding and placement

Measured 2026-10-17 on the pop and push in `lean/generated/` (unchanged from commit e225460 to
4eff6e5), with `bench/roofline/kernels` (`routines` mode, kernels emitted by
`gen_routines.py`), `bench/roofline/rust-alloc/run_slab.sh` and
`bench/roofline/toolchain/check.sh`. The question: which encoding and placement the generator
should pin.

Method. Each kernel copies a generated sequence instruction for instruction and changes one
thing. All run real restartable sequences against glibc's registered area; no run saw an
abort. Out-of-line routines are reached by direct calls with the class in `edi`, as a caller
would. Each routine was swept over the 64 entry offsets of its line while the other routine was
placed so that a line boundary splits it in half, a placement that was fast for every size.
Kernels ran on CPU 2, the Rust stand-in on CPU 3, with their SMT siblings 18 and 19 at 0 to 2.5
percent busy and CPU 3 at 0 to 4 percent during kernel runs; teammates were building on CPUs 5,
7, 12 and 20. Load average 2 to 5 on 32 CPUs. Each cell is the fastest of three repetitions in
each of three to five interleaved rounds, 10 or 20 million iterations. Pair and batch are the
loops of section 2 with one call per operation, so 8.00 per pair is the floor (two calls).

Noise. Two identical copies of the routines, at different addresses, gave the same value in
every pair cell; two identical copies of the inline kernels agreed to 0.01. Rounds differed by
0.00 to 0.05 in almost every cell. The exception is the batch loop: at a few offsets per routine
where the pair loop is fast (up to 13 of about 40), a batch costs about one cycle more per
pair. These cells move when the other routine moves or the binary is rebuilt, so they depend
on addresses, not on the offset alone; the rules below ignore them. None fell at offset 0 for a routine of 76
bytes or more.

### Placement

Generated out-of-line pop (89 bytes, 16 fused uops) and push (88 bytes, 16), cycles per call
pair, in two sweeps (two builds) with two copies each:

| Entry at 64n + k | Pop | Push |
|---|---:|---:|
| k = 0 to 39 (pop), 0 to 40 (push): the hot path stays in two lines | 8.00 | 8.00 |
| k = 40 to 63 (pop), 41 to 63 (push): it reaches a third line | 9.00 | 9.00 |

- Crossing one line boundary is free wherever it falls. Reaching a third line costs a cycle per
  call, even when only the `ret` is there.
- The offsets at which the routine ends exactly on a line boundary (39 and 40) are just the last
  fast ones. Offset 0, where the generator puts them today, is fast in every loop and build.
- Pop and push behave alike, and the boundary between fast and slow was identical in every
  build and copy.
- A short tail is different. At offset 0 the 67 to 71-byte variants below leave only their last
  one to three instructions (3 to 7 bytes) in the second line, and cost 9.00 per pair and 10.00
  per batch when both routines do. With only one of them placed so, the pair stays at 8.00 and
  the batch costs 8.8 to 9.9. A 75-byte pop (11-byte tail) was slow at offset 0 in one batch
  sweep; 74 bytes and up were not. This is the effect section 6 saw for a 61-byte function at
  offsets 4 to 12.

### Byte savings

Each change alone and combined, at offset 0 (both routines) and split (both placed so a line
boundary halves them), cycles per pair:

| Variant | Pop B / uops | Push B / uops | Pair, at 0 | Batch, at 0 | Pair, split | Batch, split |
|---|---:|---:|---:|---:|---:|---:|
| As generated | 89 / 16 | 88 / 16 | 8.00 | 8.07 | 8.00 | 8.07 |
| Short jumps to a stub after the `ret` | 81 / 16 | 80 / 16 | 8.00 | 8.07 | 8.00 | 8.07 |
| Mask from the slab base, 16-bit table below `SLABS` | 87 / 15 | 86 / 15 | 8.00 | 8.07 | 8.00 | 8.07 |
| Mask from the slab base, 8-byte entries below `SLABS` | 85 / 15 | 84 / 15 | 8.00 | 8.07 | 8.00 | 8.07 |
| Registers without REX | 86 / 16 | 86 / 16 | 8.00 | 8.07 | 8.00 | 8.07 |
| Merged address arithmetic | 88 / 16 | 87 / 16 | 8.00 | 8.07 | 8.00 | 8.07 |
| All four (16-bit table) | 75 / 15 | 75 / 15 | 8.00 | 8.07 | 8.00 | 8.07 |
| Short jumps, no REX, 8-byte table | 74 / 15 | 74 / 15 | 8.00 | 8.07 | 8.00 | 8.07 |
| The same with the sign test | 70 / 15 | 70 / 15 | 9.00 | 10.00 | 8.00 | 8.07 |
| The same with the direct `__rseq_offset` load | 71 / 14 | 71 / 14 | 9.00 | 10.00 | 8.00 | 8.07 |
| Both | 67 / 14 | 67 / 14 | 9.00 | 10.00 | 8.00 | 8.07 |
| All four, sign test and direct load | 68 / 14 | 68 / 14 | 9.00 | 10.00 | 8.00 | 8.07 |

- No byte saving buys a cycle at a fast placement: through calls the call is the cost.
  Several of them, combined, land in the 65 to 75-byte range where offset 0 is slow.
- Taking the mask from the slab's base register removes a `lea` (one uop), which did not show
  either. A 16-bit table needs a 66 prefix, so it saves 2 bytes, not more, and cannot hold the
  masks of magazines of more than 8192 slots, which the specification allows (`MAG_LOG_MAX` 16).
  Merged arithmetic puts an `add` on the path to the slab load.
- Inference, not confirmed: a table of 8-byte entries exactly 4096 bytes below `SLABS` gives
  each entry the page offset of its class's slab words. A copy 6144 bytes below, which cannot
  alias, measured the same.

### The sign test

`test cpu, cpu; js` in place of `cmp cpu, 8192; jae`: 4 bytes shorter, the same uops. Out of
line it measured the same as the compare at every fast placement (table above). Inline:

| Inline pop and push | Uops | Pair | Batch |
|---|---:|---:|---:|
| As generated (pop counts the caller's null test) | 14 + 13 | 5.27 | 5.81 |
| Sign test | 14 + 13 | 5.24 | 5.79 |
| Direct `__rseq_offset` load | 13 + 12 | 4.23 | 5.18 |
| Direct load and sign test | 13 + 12 | 4.32 | 5.53 |

It buys nothing (with the direct load, the batch was 0.35 slower), and it gives up what the
compare is for: with it, a CPU number of 8192 or more indexes beyond `SLABS`, so memory safety
would rest on the kernel's CPU numbers, which the design avoids.

### Reading `__rseq_offset` directly

What the toolchain does (`toolchain/check.sh`): rustc 1.99.1 links with `cc -fuse-ld=lld` (LLD
23.1.1), `-pie`, `-z relro -z now`. A `mov reg, [rip + __rseq_offset]` in an executable becomes
an `R_X86_64_COPY` relocation: the linker reserves 8 bytes in `.bss.rel.ro`, and every
reference, the GOT entry included, binds to that copy. GNU ld 2.46 does the same. In the
running program the copy held the right value already in a `.preinit_array` function, the
first user code that runs, and the `cpu_id` it locates matched `getcpu` in the main thread and
a new one. That follows from glibc's start-up order (glibc source, `elf/rtld.c`): ld.so sets the
offset in `init_tls`, before it relocates the program, and relocates the program before it runs
any constructor or routes `malloc` to the program. In a shared library the same instruction
does not link, with either linker: `relocation R_X86_64_PC32 cannot be used against symbol
'__rseq_offset'; recompile with -fPIC`. A statically linked program was not tested (no static
glibc on this machine); there the symbol is local and should need no relocation at all.

Cost: one load and 3 bytes fewer, bringing the inline pop to 12 uops plus the caller's test and
the push to 12. Inline, a pair costs 1.0 cycle less and a batch 0.6 (table above); under LTO a
`Box` pair 0.85 less and a batch 0.4 to 0.5 less (below). Out of line it changes nothing.

A crate cannot tell whether it will end up in an executable or a shared library, so the direct
load has to be chosen by whoever builds the executable. If it is chosen wrongly, linking fails;
it does not misbehave at run time.

### Per-class out-of-line entries

The inline sequences plus a `ret`, class and mask as immediates, no class argument: pop 80
bytes and push 78, 14 uops each, against 89 and 88 bytes and 16 uops, and the caller no longer
sets `edi`. In the kernels they cost the same as the generic routines (8.00 per pair, 8.07 per
batch). Under LTO a `Box` pair through them costs 8.00 against 8.54 through the generic ones,
and 6.80 inlined (5.95 with the direct load).

### An inline pop that calls its slow path

The pop jumps on a miss to a cold stub inside the same `asm!` block that calls the slow path
and returns to the end of the sequence, so the caller has no null test.

| | Pair | Batch |
|---|---:|---:|
| Kernels: as generated | 5.27 | 5.81 |
| Kernels: slow path called from the block | 5.21 | 5.85 |
| Kernels: the same, with the direct load | 4.20 | 4.98 |

Under LTO it changes nothing for a `Box`, because `Box` and `Vec` test the pointer for null
whatever the allocator does; that test is the one the inline pop shares today. Only telling the
compiler the result is never null (`assert_unchecked`, so an allocator that aborts on
exhaustion) removes it, for 0.02 to 0.08 cycles. And with no branch left in the pop, LLVM
unrolled the 32-allocation loop completely, which made the batch slower (6.85 against 5.96) and,
with the direct load, much slower (13.6 against 5.5). Inference: the unrolled code had 20
percent more ops dispatched than retired and three times the cycles waiting on loads, which
looks like memory-dependence mispredictions between the 32 copies.

What `asm!` requires for a `call` in a block:

- No `nostack`. The reference then promises a stack pointer aligned for a call and lets the
  block use the stack below it; rustc reserves the red zone around such a block, and makes a
  leaf function that contains one push a word to align the stack.
- That promise was not kept everywhere. When the function had a path needing a frame and
  another holding the block, shrink-wrapping put the prologue only on the first, and the block
  ran with the stack misaligned by 8 (`toolchain/asmcall.rs`, function `shrinkwrap`; also seen in
  the stand-in's out-of-line `alloc` without LTO). A slow-path stub has to align the stack itself.
- Clobbers: `clobber_abi("C")` declares what any call clobbers, including vector and mask
  registers the target features enable. A stub that saves registers instead must save vector
  state too, which in general means `xsave`, since Rust code may use vector registers.

### Under LTO

The stand-in allocator `rust-alloc/src/slab.rs` wraps the same sequences (64-slot magazines,
slow paths that are plain Rust and not restartable; not a real allocator). Fat LTO, CPU 3,
cycles per allocate and free, three runs that agreed to within 0.1 but for one batch run of
the in-block call (7.28):

| Variant | `Box` pair | Constant-layout pair | Batch of 32 |
|---|---:|---:|---:|
| Inline, as generated | 6.80 | 5.55 | 5.96 |
| Inline, direct `__rseq_offset` load | 5.95 | 5.14 | 5.47 to 5.55 |
| Inline, slow path called from the pop | 6.80 | 5.43 | 6.85 |
| The same, with the direct load | 5.95 | 4.70 | 13.6 |
| Out of line, as generated (entry on a line boundary) | 8.54 | 8.00 | 8.00 |
| Out of line, one entry per class | 8.00 | 8.00 | 8.00 |

Section 9's stand-in cost 5.8 per `Box` pair; it kept the rseq offset in a register and had a
shorter pop.

### Recommendations for the generator

Measured and inferred as stated above.

1. Placement: keep the entry on a 64-byte boundary, as naked functions give today, and require
   on the assembled bytes that the hot path (entry to `ret`) be at most 128 bytes and either at
   most 64 or at least 76. The pop and push (89 and 88) pass with 39 and 40 bytes to spare.
   Ending on a line boundary is not needed. Mid-line entries are never faster, and a naked
   function cannot have one.
2. Byte savings: take none for speed; none gains a cycle. Short jumps (8 bytes) and the mask
   from the slab base (one uop, up to 4 bytes) are safe if the bytes are ever needed, but stay
   out of 65 to 75 bytes. Skip the 16-bit table: it saves 2 bytes and narrows the magazines the
   specification allows.
3. Keep the range compare. The sign test gains nothing and weakens what the proof can claim.
4. The direct `__rseq_offset` load is correct in an executable, from its first instruction,
   and cannot link in a shared library. It is worth 0.4 to 1 cycle per inlined pair, nothing
   out of line. Offer it as an opt-in for executables (for example a cargo feature the binary
   enables); the default, the C entry points' library and the preload library keep the GOT.
5. Per-class entries: not worth generating for speed. They beat the generic out-of-line form
   by 0.5 per `Box` pair and lose to inlining by 1.2 to 2. Worth having only if inlining's code
   size becomes a problem.
6. Do not generate the in-block slow call. It removes a test that Rust's callers repeat anyway,
   invites full unrolling that ran more than twice as slow, and needs a stub that realigns the stack
   and preserves vector state.

### What this changes in earlier sections

- Section 6 and the design say a run that crosses a line near its end pays a cycle. For these
  routines a crossing anywhere is free; the costs are reaching a third line, and a tail of one
  to three instructions.
- Section 5 has a call through a register costing the same as a direct call. Calling these
  routines through registers gave 8, 10 or 12 cycles per pair, changing between repetitions;
  direct calls gave 8.00 every time.
- Section 9 and rule 7 offer calling the slow path from inside the `asm!` block as the way to
  drop the pop's extra test. In Rust the test stays, because the caller makes it.

## 14. The C entry points

Measured 2026-10-17 with `bench/roofline/centry` (its `gen.py` docstring names every variant).
The question: what `malloc(size)` and `free(ptr)` cost when the size class must be found at run
time, how callers reach them, and which routines the generator should emit for them.

Method. `gen.py` emits candidate routines in hand-written assembly. Each is a real restartable
sequence on glibc's registered area, with the generated pop or push as its back half, and with
slow paths that trap. Each routine is assembled twice: into a shared library (`L_`), and into the
driver executable (`X_`) for direct calls. The driver's loop kernels call them in three ways:
directly, through the PLT, and with `call [GOT]` (gcc's `-fno-plt`). Loops: pair (`malloc(32)`
then free), mixed (64 sizes from 1 to 512, cycled), and bursts of 8 and 32. A second build links
the IBT PLT (`endbr64` stubs). Pinned to CPU 13, with its sibling 29 at 0 percent busy in every
run quoted. Each cell is the best of 3 repetitions over 2 or 3 interleaved rounds, 10 million
operations. Round-to-round spread was 0.00 to 0.05 in pair and burst cells unless noted. For
the incumbents, a preloadable stand-in allocator (`standin.c`) exports the chosen routines as
`malloc`, `free` and `calloc`, with single-threaded C slow paths. Plain C loops (`cloop.c`, gcc
16.2 `-O2 -fPIE -pie -z now`) run it next to glibc 2.43, jemalloc 5.3.0, mimalloc 3.5.3 and
tcmalloc, all preloaded, in interleaved processes. The incumbents are bench-prep's builds in
`bench/baselines/out`.

The floor through calls is 4 cycles per call, as in section 5, so 8.00 per pair direct. Every
cycle above that has a cause given here, or is marked unexplained.

### Reaching the library

Pair of empty functions (`mov rax, rdi; ret` and `ret`), cycles per pair, and retired ops:

| Call | Cycles | Ops |
|---|---:|---:|
| `call sym`, direct | 8.00 | 9 |
| `call reg`, target loaded from the GOT just before | 9.00 | 11 |
| `call reg`, target hoisted out of the loop | 9.00 | 9 |
| `call sym@PLT` (stub `jmp [GOT]`) | 10.00 | 11 |
| the same through the IBT PLT (`endbr64; jmp [GOT]`) | 10.00 | 13 |
| `call [GOT]` (`-fno-plt`) | 10.00 | 17 |

The PLT costs exactly one cycle per call (the stub is one more taken branch to fetch); the
IBT stub's `endbr64` costs nothing more. `call [mem]` retires 5 ops where `call sym` retires
1. Empty, it costs the same as the PLT. With a real callee it costs much more:

| Callees | Direct | PLT | `call [GOT]` | `call reg` |
|---|---:|---:|---:|---:|
| empty | 8.00 | 10.00 | 10.00 | 9.00 |
| a counter in memory, loaded, incremented and stored, in each callee | 8.00 | 10.00 | 16.01 | 9.00 |
| the same, twice in each callee | 8.00 | 10.00 | 18.01 | 9.00 |
| a magazine pop and push, no rseq | 8.00 | 10.00 | 16.06 | |
| per-class rseq pop and push (Rust's routines) | 8.00 | 10.00 | 16.11 | 9.00 |
| the C malloc and free below | 8.00 | 10.00 | 26 to 33 | 9.00 |

Every store-to-load hop that crosses a `call [mem]` costs 7 cycles; a hop inside one call is
still free (the second row adds a hop inside each callee and only one cycle per call
appears). So `call [mem]` defeats the zero-latency forwarding of section 4 across the call.
Callees with long ALU chains, load chains, late store addresses, `fs:` loads or stores, or up to
20 uops showed nothing. Inference: `call [mem]` is microcoded on Zen 5, and microcode ends memory
renaming. The C routines suffer most, because their state is in memory twice (slab word and
slot) and free's word address hangs off the pointer the previous pop loaded. Who calls this
way: gcc `-fno-plt`, and rustc, which emits `call [GOT]` for calls into other shared objects by
default (seen in its output). So a Rust program on the System allocator with our library preloaded
pays this, and so does tcmalloc's shared library, which reaches its thread-locals through TLSDESC
`call [rax]` twice per malloc (below).

Own data inside the shared library: hidden symbols reached with `lea` cost nothing over the
executable's copy. Reaching the same data through the GOT adds one load per symbol (1 to 2 uops)
and measured the same in pairs and bursts (8.00/10.00). In the mixed loop it measured 0.8 cycles
better direct and 3.3 worse through the PLT; that is within the build-to-build spread of that
loop (below). `__rseq_offset`, a libc symbol, must go through the GOT in a shared library (section
13): two loads. A copy in our own data saves one uop and measured the same (8.00/10.00 pairs,
within noise in bursts). It would also need initialising before the first call, and a zero copy
makes the arm store to the TCB's `dtv` field, so it is not worth having.

### Load latency by addressing mode

The mixed loop below turned on this, so it was measured directly: a chase through zeroed memory,
cycles per load.

| Address | `mov r64` | `movzx` byte |
|---|---:|---:|
| `[base]`, `[base + disp8]`, `[base + disp32]` | 4 | |
| `[base + index]` | 4 | 4 |
| `[base + index + disp]`, disp8 or disp32 | 5 | 5 |
| `[base + index*8]`, with or without disp | 5 | |

Section 4's "with an index register 5.0" was a scaled index. An unscaled index without
displacement is as fast as a plain base.

### malloc: size to class

All variants have the same back half: `r9 = SLABS + 8 * class` (the class's word in slab 0),
then the generated pop with the mask read at a fixed displacement from r9. That is the shape the
pop and push are being changed to. The tables sit at fixed offsets below `SLABS` unless noted.
The miss branches go to `malloc_slow(size)` with `rdi` intact. Free is the recommended one below.

| Variant | Sizes | Bytes / uops | Pair direct / PLT | Burst 32 direct / PLT | Mixed direct |
|---|---|---:|---:|---:|---:|
| byte table at `(n + 7) >> 3`, 4097 entries (`mLj`) | 0 to 32 KiB | 101 / 19 | 8.00 / 10.00 | 8.31 / 10.07 | 21.9 |
| the same table, 129 entries (`mSj`) | 0 to 1 KiB | 101 / 19 | 8.00 / 10.00 | 8.32 / 10.07 | 21.9 |
| byte table indexed by `n`, 1025 entries (`mDj`) | 0 to 1 KiB | 95 / 17 | 8.00 / 10.00 | 8.13 / 10.07 | 23.2 to 23.8 |
| entries hold `SLABS + 8 * class` itself, 8 bytes each (`mQj`) | 0 to 1 KiB | 92 / 18 | 8.00 / 10.00 | 8.28 / 10.07 | 22.1 |
| packed entries: class offset and mask from one index (`mP`) | 0 to 1 KiB | 112 / 21 | 8.00 / 10.00 | 8.85 / 10.07 | 23.2 |
| tcmalloc's two ranges, `cmova` (`mTj`) | 0 to 32 KiB | 120 / 23 | 8.55 / 10.00 | 8.96 / 10.07 | 21.1 |
| computed: round to C's rule, `bsr`, shifts (`mK`) | 0 to 32 KiB | 151 / 32 | 9.69 / 11.00 | 10.88 / 11.54 | 21.2 |
| tables as separate symbols, each with its own `lea` (`mA`) | 0 to 1 KiB | 119 / 21 | 9.00 / 11.00 with its free | | 21.5 |
| Rust's `extern "C" fn malloc`: lookup, then `jmp` to the generic pop (`mR`) | 0 to 1 KiB | 37 + 89 / 6 + 16 | 10.00 / 12.00 with its free | 10.07 / 13.97 | 20.9 |

- The byte table at `(n + 7) >> 3` covering every small size is the best shape. It needs no
  second range and no computed class. C's 16-byte rule is built into the table: indexes 2k-1
  and 2k map to the same 16-byte multiple. It measured 8.00/10.00 at every size from 0 to 32768.
  4097 bytes is 64 lines, but only the lines of the sizes a program uses are touched.
- A table that yields the register value directly saves a uop and buys no cycle. Its 8-byte
  entries make it 8 times larger. A packed size-to-mask table buys nothing either: the mask load
  is off the critical path, since its branch is predicted. The fixed-displacement mask load is
  right for C.
- Two-range and computed classes cost uops, and that shows in direct calls and bursts. The
  direct-indexed table saves the shift but loses about 1.5 cycles in the mixed loop, unexplained
  (its table load is the 5-cycle form, but that load is not on the loop-carried path).
- The Rust structure (a): the extra `jmp` puts the hot path in two routines, at 2 cycles per pair
  and 4 per burst-of-32 element through the PLT. It is the best in the mixed loop by about 1.
- Cycles above the floor for a direct pair follow section 13's rules exactly: more than 128
  bytes (`mT` at 132, `fB` at 131) or more than 24 uops (`fA` at 25) costs a cycle. Through the
  PLT a call has a 5-cycle budget, and the 23-uop routines measured at the floor there; the
  byte and uop limits still decide the direct-call cells.

### free: address to class

The region index is `(p >> s) - (base >> s)` from the one published word `base | s`. A single
unsigned compare against 4096 then rejects null, pointers below the heap and pointers above it.
The class byte comes from the region table. Every byte that is not a small class (0, large
classes, run head and tail) has a zero mask in a 256-entry mask table. So the push's own mask
test sends those to the slow path, with no separate small-class test.

| Variant | Bytes / uops | Pair direct / PLT | Burst 32 direct | Mixed direct |
|---|---:|---:|---:|---:|
| `shrx` twice from the published word, region table by its own `lea` read as `[rcx + rax]` (`fUj`) | 115 / 22 | 8.00 / 10.00 | 8.31 | 21.9 |
| the same with `test rdi, rdi; jz ret` first (`fUjn`) | 120 / 23 | 8.00 / 10.00 | | |
| region table read slab-relative as `[r9 + rax - 12288]` (`fS`) | 125 / 21 | 8.00 / 10.00 | 8.21 | 23.1 |
| the same with an explicit small-class test (`fSx`) | 137 / 23 | 9.00 / 11.00 | 9.07 | 22.9 |
| explicit test and separate tables (`fA`) | 143 / 25 | 9.00 / 11.00 | 9.30 | 21.5 |
| `shr r, cl` in place of `shrx` (no BMI2) (`fC`) | 124 / 22 | 8.00 / 10.00 | 8.33 | 23.2 |
| shift and `base >> s` as two statics, `sub` from memory (`fW`) | 123 / 20 | 8.00 / 10.00 | 8.30 | 25.1 |
| region byte through a pointer biased by `base >> s` (`fVj`) | 119 / 23 | 9.07 / 10.01 | | 22.6 |
| class as a scaled index, `[r8 + rdx*8]` (`fY`) | 125 / 21 | 8.00 / 10.00 | | 23.4 |
| Rust's `extern "C" fn free`, then `jmp` to the generic push (`fR`) | 58 + 88 / 11 + 16 | 10.00 / 12.00 with `mR` | | 20.9 |

- Folding the small-class test into the mask table is what lets the free fit: an explicit test
  is 2 uops and 12 bytes, and pushes the routine past 128 bytes or 24 uops.
- The two statics (`fW`) look cheaper (one uop fewer). But the shift and `base >> s` sat at page
  offsets 8 and 16, the same as the slab words of classes 1 and 2, and the pair cost 9.1 at
  sizes up to 16 and 8.00 above (inference: 4K aliasing). The published word, at page offset 0,
  has no such case: the w2 free measured 8.00 at every size.
- `shr r, cl` costs a uop and no cycle, so BMI2 is not needed for speed.
- free(NULL): with the explicit null test it costs only the call (4.00 direct, 5.00 PLT).
  Without it, the null pointer fails the range check and reaches the slow path through the
  short-jump stub, at 5.00/6.00. With a direct long jump it is 4.00/5.00.
- The biased pointer takes the subtraction off the path to the region load, but cost uops and
  measured no better. The scaled-index word address trades a `lea` for a 5-cycle load: no gain.

### What the missing size costs

| Free | Pair direct / PLT | Burst 32 direct | Mixed direct / PLT | `call [GOT]` pair |
|---|---:|---:|---:|---:|
| C free, class from the address (`fUj`) | 8.00 / 10.00 | 8.31 | 21.9 / 23.6 | 26 to 27 |
| the same pop, free told the size (Rust's `dealloc`, `fZ`) | 8.00 / 10.00 | | 9.6 / 10.1 | |
| Rust's per-class pop and push | 8.00 / 10.00 | 8.07 | | 16.11 |
| Rust's generic pop and push, class in `edi` | 8.00 / 10.00 | 8.07 | | 16.10 |

In pairs and bursts the size costs nothing: the call is the cost. In a tight loop of mixed sizes
it costs about 12 cycles a pair. In that loop the next malloc's slab-word load is held behind the
free's commit store: `ls_bad_status2.stli_other` fires 0.5 to 1 times per pair, with no
mispredicts or resyncs. And that store's address comes at the end of a chain from the pointer:
word load (4), slot load (4), shift and subtract (2), region table (4), `lea` (1). One size class
runs at the floor (8.00). Two classes alternating already cost 17.8 to 20.7. A free whose class
comes from the size has an early store address, and the same loop costs 9.6. Inference: the
memory-dependence predictor learns the true same-class dependence and then makes every pop wait
for the youngest push. The shapes differ by up to 2 cycles here. The 5-cycle region load
explains 1 to 1.5 of that; the rest is unexplained. The mixed cells also moved by up to 0.8 cycles
between builds of identical code, so sub-cycle differences there are not findings.

### calloc and realloc

| C loop, PLT | `calloc(1, 16)` | `calloc(1, 64)` | `calloc(1, 256)` | realloc in place (60 to 61) |
|---|---:|---:|---:|---:|
| glibc | 13.1 | 12.6 to 14.0 | 17.0 | 17.0 |
| mimalloc | 14.1 | 14.5 | 20.1 to 20.3 | 25.4 to 26.4 |
| jemalloc | 21.1 | 20.7 to 22.2 | 23.8 | 60.2 |
| ours: generated calloc (product, pop, `jmp` to a zeroing tail) | 11.0 | 13.0 | 16.8 | |
| ours: C calloc calling the malloc routine directly, 16-byte stores | 15.0 | 16.0 | 21.0 | |
| the same with `rep stosb` | 17.6 | 17.8 | 50.1 | |
| the same calling `malloc` and `memset` through the PLT | 21.0 | 22.0 | 23.0 | |
| ours: realloc in C (size and region lookups, no rseq) | | | | 16.0 to 18.0 |

A calloc routine pays: the internal call costs 4 cycles, and without it we beat glibc. It fits
only with the 1025-byte direct size table (116 bytes, 23 uops). With the shifted index it is 25
uops, and with two ranges 29 uops and 145 bytes, 1 to 1.4 cycles slower. So it serves sizes up to
1 KiB and sends larger ones to its slow path, where zeroing dominates. `rep stosb` is slow for
small blocks on this core (50 cycles at 256 bytes). realloc needs no routine: the in-place case is
two table lookups and a compare, with no per-CPU state. The 2-cycle spread between our builds of
identical realloc code is layout.

### Against the incumbents

C loops through the PLT, cycles per allocate and free (per element for bursts), best of three
interleaved rounds:

| | Pair 32 B | Pair 256 B | Burst 8 | Burst 32 | Burst 100 (64 B) | Mixed 1 to 512 B | `-fno-plt` pair | `-fno-plt` burst 32 |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| ours (`mLj`, `fUjn`, generated calloc) | 10.00 | 10.00 | 10.25 to 10.88 | 10.07 to 11.00 | 10.82 to 11.70 | 20.6 to 22.4 | 25.6 to 25.8 | 16.1 to 16.6 |
| ours, Rust-structured (`mR`, `fR`) | 12.00 | 12.00 | 12.25 | 13.94 to 13.97 | 12.82 to 12.92 | 19.7 to 20.1 | 25.1 to 26.1 | 16.1 to 16.7 |
| glibc 2.43 | 12.00 | 12.00 | 12.26 | 35.6 to 36.5 | 46.8 to 48.0 | 14.5 to 15.1 | 22.4 | 41.1 to 42.1 |
| mimalloc 3.5.3 | 11.3 to 12.2 | 12.8 to 14.0 | 11.7 to 11.8 | 11.5 to 12.7 | 12.2 to 13.3 | 18.7 to 19.0 | 16.1 to 16.2 | 16.7 |
| jemalloc 5.3.0 | 14.1 | 14.5 | 13.4 | 13.2 to 13.4 | 14.1 to 14.2 | 31.6 | 39.7 | 17.7 |
| tcmalloc, per-CPU (glibc rseq off) | 41 to 43 | 41 | 25 to 26 | 30 | 28 to 29 | 31 to 34 | 39 | 31 |
| tcmalloc, per-thread | 56 to 57 | | 66 to 67 | 68 to 69 | 67 to 68 | 59 | 60 | 70 |

The IBT build measured the same as the PLT within these ranges. Ranges are over two sessions of
the comparison (stand-in builds differing only in the free's null test and the calloc).

- Through the PLT we are at the floor of two PLT calls, 10.00, for every pair and within 0.1 to
  1.0 of it in bursts. That is 2 cycles under glibc and mimalloc in pairs and 1 to 2 under
  mimalloc in bursts, the best of the incumbents there. glibc's tcache holds 7 objects per class,
  so its bursts of 32 and more fall to its arenas.
- Mixed sizes: we lose to glibc by 6 to 8 and to mimalloc by 1 to 3. glibc's pop gets the
  pointer with one dependent load (the tcache head), where a magazine needs two (word, then
  slot). That is about 4 cycles of the chain above, paid in this loop only.
- `call [GOT]`: we lose the pair to mimalloc by 9.5 and to glibc by 3.4, for the reason in the
  first subsection. In bursts the hops are not on a loop-carried path, and we are the fastest.
- tcmalloc as built here is not its per-CPU design at its best: its shared library reaches
  `__rseq_abi` and the sampler through TLSDESC calls (`call [rax]`) on every malloc, and
  executes 107 to 280 instructions a pair.
- In burst loops through the PLT, two builds of identical routines and kernels differed by a
  cycle per element (10.07 against 11.23). The slow one showed branch-predictor redirects and op
  cache misses at about one per 32 operations; the IBT build of the same objects did not. This is
  op-cache placement of caller, PLT stubs and routines, which the routines do not control.

### A checked sized free

Measured 2026-10-17, same harness and method (CPU 13, sibling 29 at 1 percent busy). The
question: can `free_sized(ptr, size)` and C++'s sized `operator delete(void*, size_t)` win back
the mixed-loop and `call [GOT]` losses above without trusting the size? The size serves as a
prediction and the address stays the truth. The class register comes from the size exactly as
in `c_malloc`, so the push's store address no longer waits for the pointer. In parallel the class
byte comes from the address as in `c_free`. A mismatch, or a pointer outside the heap, goes to
`free_slow(ptr)`, which uses only the address. That compare is a predicted branch off the
dependency chain.

| Free | Bytes / uops | Pair direct / PLT | Burst 8 direct / PLT | Burst 32 direct / PLT | Mixed direct / PLT | `call [GOT]` pair / mixed |
|---|---:|---:|---:|---:|---:|---:|
| `c_free`, unsized (`fUj`) | 115 / 22 | 8.00 / 10.01 | 8.50 / 10.25 | 8.32 / 10.07 | 21.9 / 23.6 | 25.8 / 26.1 |
| size trusted (`fZ`, not safe) | 112 / 19 | 8.00 / 10.00 | 8.25 / 10.25 | 8.07 / 11.05 | 9.6 / 10.1 | 16.2 / 13.4 |
| checked, compact (`fKx`) | 128 / 24 | 8.06 / 10.00 | 8.85 / 10.25 | 9.74 / 10.10 | 11.2 / 11.5 | 16.1 / 13.7 |
| checked, straightforward (`fKw`) | 136 / 24 | 9.00 / 11.00 | 9.25 / 11.25 | 9.07 / 11.07 | 11.1 / 11.4 | 16.1 / 13.7 |
| checked, with a bound check and the published word (`fK`) | 142 / 25 | 9.00 / 11.00 | 9.25 / 11.25 | 9.07 / 11.07 | 11.6 / 11.5 | 16.1 / 13.9 |

- The check works as intended. In the mixed loop the checked free runs within 1.4 to 1.6 of
  the size-trusting one, and 10.7 to 12.1 cycles under the unsized free. Under `call [GOT]` it
  matches the size-trusting free: 16.1 a pair against 25.8. The remaining 1.5 in the mixed loop
  is the compare's region load and the extra uops; the store address no longer depends on it.
- The straightforward encoding is 136 bytes and pays section 13's third-line cycle on every call.
  Two changes bring it to exactly 128 bytes and 24 uops:
  - No bound check on the size. A wrong prediction can only cause a mismatch, so `movzx eax,
    si` bounds the table index instead: an 8193-byte table, class 0 above 32 KiB.
  - Register roles that drop REX prefixes in the back half: rsi for the rseq offset, rcx for the
    descriptor, edx for the CPU number, all free once the class is known.
- Through the PLT the compact form costs nothing over the unsized free in pairs and bursts. With
  direct calls it costs 0.06 a pair, 0.35 per element in bursts of 8 and 1.4 in bursts of 32.
  Inference: 24 uops plus the caller's loop overhead exceed the 4-cycle call budget when calls
  come back to back.
- Placing the address check inside the sequence (`fKi`, `fKwi`) or the region table behind its
  own `lea` (`fKs`, 26 uops) measured no better. The slab-relative region read is a 5-cycle load,
  which no longer matters because the compare is off the chain.
- Shift and `base >> shift` as two statics save a uop over the published word, and that uop is
  what fits the 24. Here they sit at page offset 2048, clear of the slab hot words. `fW`'s
  class-1 and class-2 loss came from the same statics at offsets 8 and 16; the checked free was
  measured only at the sizes of the loops above.

Aligned sized delete (`operator delete(void*, size_t, align_val_t)`) gets no routine of its own.
Its prediction would need the size rounded up to the alignment first, 3 uops on a routine already
at 24; predicting from the bare size mismatches whenever the alignment changes the class. It
should be the unsized `c_free`.

Size of the prize from the region table alone: an unsized free that computes the class from the
region index by arithmetic (`fAr`). This pretends each small class owns a fixed block of
regions; here the shift is 0, so it is one `mov` and one `shr`.

| | Pair direct / PLT | Mixed direct / PLT | `call [GOT]` pair / mixed |
|---|---:|---:|---:|
| unsized, region table (`fUj`) | 8.00 / 10.01 | 21.9 / 23.6 | 25.8 / 26.1 |
| unsized, class by arithmetic (`fAr`) | 8.00 / 10.00 | 19.0 / 19.7 | 24.5 / 22.5 |
| checked sized (`fKx`) | 8.06 / 10.00 | 11.2 / 11.5 | 16.1 / 13.7 |

The table load accounts for 2.9 to 3.9 of the 12 to 13.5 cycles the unsized free loses in the
mixed loop, and 1.3 of the 9.6 it loses under `call [GOT]`. The rest is the pointer chain itself:
two loads to get the pointer in malloc, then the shifts. Changing how regions are assigned would
not close the gap; a size from the caller does.

### Recommendations for the generator

The pop and push (back half) as being changed: `r9 = SLABS + 8 * class` in, mask at a fixed
displacement from r9. The C entries add a front half in the same routine; a separate front half
that jumps to the shared routine costs 2 cycles a pair through the PLT. Labels, the descriptor
and the abort handler as in `lean/generated`; `8:` stubs follow the `ret`. All registers but
`rdi` and `rsi` are clobbered (`rax rcx rdx r8 r9 r10 r11`, flags). Entry 64-byte aligned.

1. `c_malloc(size: rdi) -> rax` (`mLj`): 101 bytes, 19 fused uops to the `ret`. A miss
   tail-calls `malloc_slow(size)` with `rdi` intact (above 32 KiB, no CPU number, no magazine,
   empty magazine).

   ```asm
       cmp   rdi, 32768
       ja    8f
       lea   r9, [rip + SLABS]
       lea   eax, [rdi + 7]
       shr   eax, 3
       movzx edx, byte ptr [r9 + rax + SIZE_OFF]  ; C class of every size, 4097 bytes
       lea   r9, [r9 + rdx*8]                     ; the class's word in slab 0
   5:  ...the pop, with the class register r9 and its fixed-displacement mask...
       ret
   8:  jmp   malloc_slow
   ```

   The table is constant and must be valid from load time. Index 0 (size 0) holds class 1. Before
   start-up, a zero mask sends every call to the slow path.
2. `c_free(ptr: rdi)` (`fUjn`): 120 bytes, 23 uops. Null returns at once. Anything outside the
   heap, unassigned, large or part of a run tail-calls `free_slow(ptr)` with `rdi` intact.

   ```asm
       test  rdi, rdi
       jz    9f
       mov   rcx, [rip + HEAP_WORD]                ; base | shift, 0 before start-up
       shrx  rax, rdi, rcx
       shrx  rcx, rcx, rcx
       sub   rax, rcx
       cmp   rax, 4096                             ; null, below and above the heap
       jae   8f
       lea   rcx, [rip + REGION_TABLE]
       movzx edx, byte ptr [rcx + rax]             ; no displacement: a 4-cycle load
       lea   r9, [rip + SLABS]
       lea   r9, [r9 + rdx*8]
   5:  ...the push...
   9:  ret
   8:  jmp   free_slow
   ```

   With the heap word zero, the index is the pointer itself, which is 4096 or more, so an
   unpublished heap also takes the slow path. Without BMI2, `mov rax, rdi; shr rax, cl; shr
   rcx, cl` is one more uop (24, at the limit) and measured the same.
3. The mask table has 256 entries of 8 bytes (2 KiB), one per possible region byte, zero for
   every byte that is not a small class. This replaces a separate small-class test. The 45 to
   256 entries are the price.
4. `c_calloc(n: rdi, size: rsi) -> rax`: `mov rax, rdi; mul rsi; jo` to a stub that calls
   `calloc_slow(SIZE_MAX)`; `mov rdi, rax`; then the malloc front half with the 1025-byte table
   indexed by the size itself, for sizes up to 1024 (above, `calloc_slow(total)`); the pop; then
   `mov rsi, rdi; mov rdi, rax; jmp calloc_zero` (a Rust function zeroing `rsi` bytes and
   returning the block). 116 bytes, 23 uops (`cD`).
5. `c_free_sized(ptr: rdi, size: rsi)` (`fKx`), for C23's `free_sized` and C++'s sized
   `operator delete` and `delete[]`. 128 bytes, 24 uops; at the
   limit of both rules, so the generator must not let it grow. The size is a prediction that the
   address checks. Anything that disagrees or lies outside the heap tail-calls `free_slow(ptr)`
   with `rdi` intact, so it is exactly as safe as `c_free`. Clobbers `rsi` as well.

   ```asm
       lea   r9, [rip + SLABS]
       movzx eax, si                               ; bounds the index; no size check needed
       add   eax, 7
       shr   eax, 3
       movzx edx, byte ptr [r9 + rax + SIZE_X_OFF] ; 8193 bytes; 0 for sizes above 32 KiB
       mov   ecx, dword ptr [rip + SHIFT]          ; the shift and base >> shift, published with
       shrx  rax, rdi, rcx                         ; the heap word, at a page offset no hot slab
       sub   rax, qword ptr [rip + BASE_INDEX]     ; word has
       cmp   rax, 4096
       jae   8f
       cmp   dl, byte ptr [r9 + rax + REGION_OFF]  ; the address's class must agree
       jne   8f
       lea   r9, [r9 + rdx*8]
   5:  mov   rsi, [rip + __rseq_offset@GOTPCREL]
       mov   rsi, [rsi]
       lea   rcx, [rip + 3f]
       mov   fs:[rsi + 8], rcx
   6:  mov   edx, fs:[rsi + 4]
       cmp   edx, 8192
       jae   8f
       shl   rdx, 12
       mov   rcx, [r9 + rdx]
       lea   rax, [rcx + 8]
       test  eax, dword ptr [r9 + MASK_OFF]
       je    8f
       mov   [rcx], rdi
       mov   [r9 + rdx], rax                       ; commit
   7:  ret
   8:  jmp   free_slow
   ```

   Predicted class 0 (size above 32 KiB) agrees only with an unassigned region's 0, and class 0's
   zero mask then sends the push to the slow path. Null fails the range check. The two statics
   must be written before the heap word is published: a free that sees a pointer into the heap
   got it from an allocation that happened after publication. The aligned sized delete is
   `c_free`.
6. No routine for realloc, `malloc_usable_size` or Rust's run-time sizes. Rust computes the
   class with the same `(n + 7) >> 3` byte lookup (its own table: no 16-byte rule, alignment
   applied first) and calls the class-register routine. Generic routines measured at the floor.
7. Placement: the 128-byte and 24-uop limits of section 13 hold for these routines and decide
   them. Keep the published heap word off the page offsets of the slab hot words (8 to 360).
   Anything read on every free that shares a page offset with a slab word of a hot class costs a
   cycle.

### The generated routines in the real library (2026-10-17)

The routines the Lean emitter now generates for `malloc`, `calloc`, `free`, `free_sized` and the
`delete` forms, measured in the real library, not the stand-in. `compare.sh` ran the C loops
(`cloop.c`) with `libasmalloc_preload.so` preloaded through the PLT, the IBT PLT and `call
[GOT]`. A fourth build links the same crate as a static library (`static/`), so the loops call
the routines directly. Pinned to CPU 12. The machine was shared with a soak and proof jobs, and
CPU 12's sibling, 28, was about 10 percent busy over the run. Each cell is the best of 3
interleaved rounds. Some rounds of ours and of the incumbents were up to 10 cycles slower; the
best rounds repeated to within 0.05 in pairs.

| C loop | static (direct) | PLT | IBT PLT | `call [GOT]` | stand-in, PLT | glibc, PLT | mimalloc, PLT |
|---|---:|---:|---:|---:|---:|---:|---:|
| pair 32 B | 8.03 | 10.00 | 10.00 | 28.20 | 10.00 | 12.00 | 12.24 |
| pair 256 B | 8.03 | 10.00 | 10.00 | 28.20 | 10.00 | 12.00 | 14.25 |
| burst 8 | 8.88 | 10.26 | 10.25 | 16.17 | 10.50 | 12.26 | 11.77 |
| burst 32 | 8.87 | 11.19 | 10.07 | 16.54 | 10.43 | 37.07 | 11.52 |
| burst 100 (64 B) | 9.33 | 10.83 | 10.83 | 16.77 | 10.96 | 47.81 | 12.31 |
| mixed 1 to 512 B | 21.06 | 23.61 | 22.67 | 25.92 | 22.48 | 14.23 | 19.02 |

jemalloc measured 13.4 to 14.6 and per-CPU tcmalloc 14.0 to 15.2 in the same PLT cells, and both
were over 30 in mixed.

- Both targets are met in pairs: 8.03 direct (the floor is 8.00) and 10.00 through the PLT.
  That is 2 cycles under glibc, and 2.2 to 4.3 under mimalloc.
- Burst 32 through the PLT is 1.1 slower than the IBT build of the same library. The stand-in
  showed the same split (above, against the incumbents): op-cache placement of the caller,
  stubs and routines, which the routines do not control.
- Mixed is about a cycle over the stand-in through the PLT. Our free reads the shift and `base
  >> s` as two words (the `fW` shape, 25.1 against `fUj`'s 21.9 in the free table above), not
  the one packed word.
- `call [GOT]`: the pair is 28.2, 2.4 over the stand-in's 25.8. A stand-in whose pop alone
  is changed to our generated order, `lea rcx, [rcx - 8]; mov rax, [rcx]`, in place of `mov
  rax, [rcx - 8]; lea rcx, [rcx - 8]`, measured 27.6 to 28.4 against 26.4 to 27.0 for the
  original, interleaved on the loaded machine. So about 1.3 cycles of the gap is that order: the
  popped pointer waits for the `lea`, and that pointer leads to the free's slab-word address,
  where `call [mem]` has disabled memory renaming (first subsection). Direct and PLT calls hide it.
  The order comes from the shared Lean pop steps. This is inference from one interleaved
  comparison.

calloc, PLT, cycles per `calloc(1, n)` and free:

| n | 16 | 32 | 64 | 128 | 256 |
|---|---:|---:|---:|---:|---:|
| ours | 11.05 | 12.08 | 12.28 | 12.40 | 13.54 |
| stand-in (`cD`, 16-byte stores up to 512) | 11.01 | 12.02 | 13.01 | | 16.85 |
| glibc | 14.04 | 13.07 | 14.03 | | 17.13 |
| mimalloc | 14.14 | 14.25 | 15.48 | | 20.62 |

The routine is 110 bytes and 22 uops. It tail-calls a Rust zeroing tail, which zeroes the
block's whole class, as `malloc_usable_size` reports it. It does that with its own 16-byte stores
up to 32 bytes and with glibc's `memset` above that. The tail started as one `memset` for every
size, which cost a cycle over the stand-in at 16 and 32 bytes (a call through the GOT and
glibc's dispatch). Then 16-byte stores up to 512 bytes, as the stand-in uses, which were 1 to 8
cycles slower than `memset` from 64 bytes up (21.4 against 13.5 at 256). Only the 32-byte
threshold was faster than both at every size.

### Caveats

- One process size and one CPU, single-threaded. The stand-in's slow paths are not the
  allocator's, so the comparison covers fast paths only. Incumbents ran in their production
  builds, ours as a stand-in.
- The mixed loop is a worst case: allocation directly after a free of another class, with
  nothing in between. Its cells move by up to a cycle with layout.
- The `call [mem]` explanation and the 4K-aliasing reading of the two-statics free are inferences
  from the measurements listed.
