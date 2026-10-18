# asmalloc's first numbers

Measured 2026-10-17 with `bench/roofline/rust-alloc` (features `asm` and `asm_exe`), on the
machine and with the method of `zen5-roofline.md` section 2, after the routines stopped baking
in the size class (below). Single-thread workloads pinned to CPU 12 with its sibling 28 idle;
cross-thread ones on CPUs 8 to 11 (CPUs 14 and 15 were busy with another job, and a teammate
was measuring on 13), so `x_cross_pairs` ran two pairs, not three. Two interleaved rounds of
every allocator; the tables give the second. The machine ran Kani and model checks on other
cores (load average 15 to 18). `asm` reads `__rseq_offset` through the GOT, `asm_exe` with one
direct load (`executable` feature).

Cycles per allocate+free (single thread), default release / fat LTO:

| | Pair, constant | Pair, `Box` | Pair, run-time size | Batch of 32 | Churn |
|---|---|---|---|---|---|
| asmalloc (`asm`) | 11.7 / 5.5 | 29.6 / 6.9 | 13.1 / 8.8 | 9.5 / 6.4 | 32.2 / 28.3 |
| asmalloc (`asm_exe`) | 10.9 / 5.0 | 12.1 / 5.9 | 12.8 / 7.5 | 9.4 / 5.3 | 37.9 / 32.4 |
| glibc | 22.2 / 13.0 | 26.2 / 11.0 | 23.1 / 21.9 | 40.1 / 32.4 | 52.8 / 46.9 |
| mimalloc | 17.7 / 12.3 | 20.0 / 13.6 | 20.0 / 12.9 | 14.8 / 10.7 | 39.3 / 37.4 |
| mimalloc, sized free | 15.4 / 10.2 | 14.7 / 10.3 | 14.4 / 10.2 | 13.7 / 9.6 | 41.2 / 30.3 |
| jemalloc | 15.1 / 12.1 | 15.9 / 12.1 | 15.7 / 14.8 | 16.9 / 11.9 | 42.8 / 39.7 |
| floor (list behind a thread-local pointer) | 7.6 / 2.2 | 9.2 / 4.0 | 9.2 / 3.7 | 16.4 / 8.8 | 19.2 / 16.4 |

Rounds agreed within 2% for asmalloc but in two cells: run-time size, `asm`, fat LTO (7.7 in
the first round) and churn, `asm_exe` (31.4 / 28.1 in the first). Under fat LTO asmalloc
executes 40 to 62 instructions per pair and 93 to 95 per churn step (before: 106 to 119 per
pair). The floor for a per-CPU rseq pair is 4.0 cycles inlined and 8.0 through two calls
(`zen5-roofline.md` section 3). Churn has 0.014 branch misses per step (before: 2.0; jemalloc
0.2).

Nanoseconds per object, both sides working (fat LTO, bursts of 32):

| | One thread | Pipeline | Pipeline, reader touches | Exchange, 2 | Ring of 4 | 2 pipelines |
|---|---:|---:|---:|---:|---:|---:|
| asmalloc (`asm`) | 1.4 | 2.9 | 4.7 | 3.9 | 4.7 | 6.1 |
| asmalloc (`asm_exe`) | 1.3 | 2.8 | 4.7 | 3.8 | 4.6 | 5.6 |
| glibc | 7.9 | 152 | 140 | 131 | 136 | 118 |
| mimalloc | 2.8 | 10.0 | 9.9 | 22.1 | 17.0 | 13.2 |
| mimalloc, sized free | 2.2 | 10.2 | 10.8 | 22.2 | 20.9 | 10.4 |
| jemalloc | 3.1 | 11.9 | 11.9 | 10.7 | 11.9 | 11.8 |
| floor | 0.9 | not valid | | 2.7 | 3.0 | |

asmalloc's rounds agreed within 7%; mimalloc's pipelines moved by up to 60% between rounds
(15.7 and 17.8 in the first).

## What changed, and what is left

**The fast path inlines.** Each routine is now given the address of its class's word in
slab 0, and inline its mask, in registers, so `GlobalAlloc::alloc` is one table lookup, one
routine and a cold call; there is no jump table. Under fat LTO a constant layout folds to `lea`
of the word, `mov` of the mask, and the routine, inlined into the caller
(`bench/roofline/fastpath` checks this). A run-time size adds `lea; cmp; jae; shr; movzx; lea;
or; mov` (the byte table indexed by `(size - 1) / 8`, alignment folded in).

**What the 1.0 to 1.5 cycles over the 4.0 floor are** (inference from the disassembly): the
register allocator keeps the word address in a callee-saved register, because the cold calls
to the slow paths clobber the argument registers, and copies it, and rematerializes the mask
immediate, before each sequence: four more micro-ops per pair than the kernels' best loop.
Inputs in callee-saved registers would remove them; that is a change to the inline target's
calling convention in Lean's trusted core, not made here. The `Box` pair adds the caller's
initialisation of 32 bytes.

**Default release, `Box`, GOT form: 29.6 cycles, unexplained.** The same code with the direct
`__rseq_offset` load costs 12.1, and both builds call std's allocation shim through
`call [GOT]`, which section 14 found ends zero-latency forwarding across the call. The GOT
build shows 2.4 million branch-predictor resyncs over 80 million pairs (the direct build
1,700) and two more store-to-load forwards per pair. Padding the environment or moving the
thread control block changes nothing. Fat LTO removes the shim call, and there the two forms
differ by one cycle, as section 13 measured.

**The run-time-size pair** under fat LTO (7.5 to 8.8) is under the two-call floor because it
is inlined; it is also faster than every incumbent's constant-size pair.

## Choosing the routine's shape, in the kernels

`bench/roofline/kernels` (`routines inline`, variants `w*` in `gen_routines.py`), CPU 12 with 28
idle, three interleaved rounds of 20 million iterations, rounds within 0.03 but where noted.
The class's word address arrives in `rdi`, set by the caller's `lea` before each operation or
once before the loop (`hoist`); cycles per inline allocate+free, pair and batch of 32:

| Mask from | GOT pair | GOT batch | direct pair | direct batch |
|---|---:|---:|---:|---:|
| an immediate (the old routine, class constant) | 5.25 | 5.80 | 4.23 | 5.18 |
| memory at `[rdi + D]`, `lea` per operation | 5.57 | 6.07 | 4.63 | 5.36 |
| the same, hoisted | 5.36 | 5.94 | 4.54 | 5.15 |
| a register, set per operation (`mov esi, imm`) | 5.20 | 5.90 | 4.47 | 5.51 |
| the same, hoisted | 5.08 | 5.67 | 4.16 | 5.51 |

The register mask won all four pair loops, by 0.16 to 0.38, and the GOT batch loops; it lost
the direct batch loops by 0.15 to 0.36. It also takes the load out of the sequence, and a
constant class folds it to an immediate, so the inline routines take it in a register.
Reading the mask 4096 bytes from the word (the same page offset as the class's slab word) cost
0.8 cycles more than 2048 or 6144 bytes: 4K aliasing, which is why the out-of-line routines'
table sits at page offset 2048.

## Code size per call site

Fat LTO, from the disassembly of the constant-layout loop (`bench/roofline/fastpath` prints
each site's loop):

| | Hot path, in place | Miss block, same function | `.text.unlikely` | Descriptor and its relocations |
|---|---:|---:|---:|---:|
| allocation, constant layout | about 85 bytes | 13 | 32 (abort stub, two miss stubs) | 32 + 72 |
| free, constant layout | about 70 | 13 | 12 (abort stub) | 32 + 72 |
| either, run-time size | 35 to 40 more | | | |

The descriptor (`__rseq_cs`, 32 bytes) holds three absolute addresses, which in a PIE are
three `R_X86_64_RELATIVE` relocations of 24 bytes each, applied once at load. A call to an
out-of-line routine instead costs 5 to 10 bytes and 4 cycles.
