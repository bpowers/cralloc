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

**What the 1.0 to 1.5 cycles over the 4.0 floor are.** An earlier reading of the disassembly
said register copies: the inline routines take their inputs in fixed registers (`rdi`, `esi`,
`rdx`), which the cold slow-path calls clobber, so the compiler copies the word address and
rematerializes the mask before each sequence. A measurement (below, "Inputs as `in(reg)`")
refuted that. Removing the copies saves 5 instructions a pair but only 0.04 to 0.25 cycle. The
pair is bound by 8-wide retirement of about 29 to 34 ops, plus 0.5 to 1.4 cycles in which
retirement waits on a load. The `Box` pair adds the caller's initialisation of 32 bytes.

**Default release, `Box`, GOT form: the 29.6 cycles did not reproduce** (re-measured
2026-10-17, evening). Today's code measured 13.7 cycles in 30 runs, ASLR on and off
interleaved, and the commit measured above (3c21b82) measured 13.7 to 14.5. The direct form
measured 12.1 to 12.3. Both builds call std's marker `__rust_no_alloc_shim_is_unstable_v2` (an
empty function) through `call [GOT]` on every `Box::new`; the linker did not relax it in either
build. So the call form does not separate the two builds, and the remaining difference of about
1.5 cycles is the GOT form's extra load. During a stretch of heavy load on other cores, the
same binary measured 16.7 to 22.0, so the 29.6 is most likely interference that the earlier
check of the sibling CPU missed. No build setting is needed. Fat LTO removes the marker call.

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

## Inputs as `in(reg)`, and what bounds the inline pair (2026-10-17)

The question: do the inline pop's and push's fixed input registers cost the last cycle of the
Rust fast path? `bench/roofline/inreg` copies the generated inline routines with their inputs
as `in(reg)` operands (`patch.py`) and builds `rust-alloc`'s `asm` and `asm_exe` variants with
fat LTO, both ways. Pinned to CPU 12, minimum of 8 interleaved rounds, cycles (instructions)
per allocate and free:

| | GOT, fixed | GOT, `in(reg)` | direct, fixed | direct, `in(reg)` |
|---|---:|---:|---:|---:|
| pair, constant | 5.46 (42) | 5.42 (37) | 4.96 (40) | 4.73 (35) |
| pair, `Box` | 6.93 (46) | 6.85 (42) | 5.94 (44) | 5.89 (40) |
| pair, run-time size | 7.74 (62) | 7.52 (57) | 7.38 (60) | 7.05 (55) |
| batch of 32 | 5.98 (45.3) | 6.27 (40.3) | 5.34 (43.3) | 5.08 (38.3) |

`in(reg)` removes exactly the predicted copies (`mov esi, 0x7f0`, `mov rdi, r15` before each
sequence, `mov rdx, r12` before the push) and adds one (`mov r14, rax`, since the popped
pointer can no longer share `rax`). The GOT batch loop got 0.3 slower, which is not explained.

Counters, per constant pair, direct form:

| | Retired ops | Cycles | Cycles retirement waits on a load |
|---|---:|---:|---:|
| fixed | 33.7 | 4.96 | 0.51 |
| `in(reg)` | 28.8 | 4.74 | 0.87 |
| `in(reg)`, word address in one register | 31.0 | 5.26 | 1.20 |

The GOT form with `in(reg)` retires 30.9 ops in 5.45 cycles, 1.41 of them waiting on a load.
Retirement is 8 ops a cycle, so the fixed pair is mostly op count (4.2 cycles of 4.96).
Removing 5 ops makes the wait on loads grow by about as much as it saves. The loop is then
bound by load latency, not by width.

The last row tests one guess at that latency: that the slab word's store-to-load hop is slow
because both accesses are addressed `[word + r8]`, with `r8` reloaded every sequence. The
variant forms the address once into `r9` (`lea r9, [word + r8]`) and uses `[r9]` for the load
and the commit. It is 0.5 slower, and the wait on loads grows again, by about the cycle the
`lea` adds to the address chain. So the address form is not the cause. The loads wait on the
per-sequence address chain: the `cpu_id` load, `shl`, then the word load, and in the GOT form
the two `__rseq_offset` loads before them. Lengthening that chain costs cycles one for one.
That chain is what the design's per-CPU addressing needs, so the inline pair stops at 4.7 to
5.0 cycles (direct form) and 5.4 to 5.5 (GOT form).

