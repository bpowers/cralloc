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
  Inside a 2 MiB transparent huge page it is 117 to 119 ns per 4 KiB.
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
