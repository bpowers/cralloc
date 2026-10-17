# asmalloc's first numbers, untuned

Measured 2026-10-17 with `bench/roofline/rust-alloc` (features `asm` and `asm_exe`), on the
machine and with the method of `zen5-roofline.md` section 2: single-thread workloads pinned to
CPU 12 with its sibling 28 idle, cross-thread ones on CPUs 12 to 17. Two interleaved rounds of
every allocator; the tables give the second. asmalloc's two rounds agreed within 3% everywhere
but the three pipelines (12.3 to 16.9 ns); glibc's and mimalloc's cross-thread figures moved
by up to 40% between rounds.
The machine ran one other job of ours (Miri, on CPUs 4-7 and 20-23); load average 3 to 4.
Nothing in asmalloc was tuned. `asm` reads `__rseq_offset` through the GOT, `asm_exe` with one
direct load (`executable` feature).

Cycles per allocate+free (single thread), default release / fat LTO:

| | Pair, constant | Pair, `Box` | Pair, run-time size | Batch of 32 | Churn |
|---|---|---|---|---|---|
| asmalloc (`asm`) | 18.4 / 16.9 | 19.3 / 17.8 | 19.0 / 16.6 | 19.6 / 17.2 | 150 / 141 |
| asmalloc (`asm_exe`) | 18.5 / 16.3 | 19.3 / 17.3 | 19.0 / 16.1 | 19.4 / 16.1 | 150 / 142 |
| glibc | 22.1 / 11.0 | 26.2 / 11.0 | 22.0 / 21.9 | 41.0 / 33.8 | 52.9 / 47.0 |
| mimalloc | 18.5 / 11.4 | 17.6 / 13.4 | 17.3 / 13.0 | 16.0 / 11.6 | 38.7 / 35.9 |
| mimalloc, sized free | 14.4 / 11.3 | 14.7 / 10.3 | 14.4 / 10.3 | 13.7 / 9.5 | 33.5 / 30.4 |
| jemalloc | 15.1 / 12.1 | 16.1 / 12.1 | 23.6 / 14.8 | 15.5 / 11.1 | 42.8 / 40.1 |
| floor (list behind a thread-local pointer) | 7.6 / 2.1 | 9.2 / 4.0 | 9.2 / 3.6 | 16.4 / 8.8 | 19.2 / 16.4 |

asmalloc executes 106 to 119 instructions per pair (incumbents 52 to 95, floor 18 to 38).
The floor for a per-CPU rseq pair is 4.0 cycles inlined and 8.0 through two calls
(`zen5-roofline.md` section 3).

Nanoseconds per object, both sides working (fat LTO, bursts of 32):

| | One thread | Pipeline | Pipeline, reader touches | Exchange, 2 | Ring of 4 | 3 pipelines |
|---|---:|---:|---:|---:|---:|---:|
| asmalloc (`asm`) | 3.6 | 4.8 | 5.1 | 5.6 | 6.6 | 16.7 |
| asmalloc (`asm_exe`) | 3.5 | 4.7 | 5.1 | 5.5 | 6.3 | 16.3 |
| glibc | 7.9 | 157 | 138 | 118 | 120 | 124 |
| mimalloc | 2.8 | 8.8 | 9.4 | 17.4 | 16.3 | 9.2 |
| mimalloc, sized free | 1.9 | 9.7 | 10.0 | 14.1 | 16.7 | 10.4 |
| jemalloc | 3.1 | 11.2 | 11.2 | 11.8 | 11.7 | 11.2 |
| floor | 0.9 | not valid | | 2.5 to 2.6 (study) | | |

What is slow, and why (inference from disassembly, `perf` and counters):

- **The fast path is not inlined, even with fat LTO.** `__rust_alloc` and `__rust_dealloc` each
  hold 44 copies of the inlined routine, one per small class, behind a jump table, because the
  routine takes the class as a constant and `__rust_alloc` sees the layout only at run time.
  That makes them about 3.8 KB each, and LLVM does not inline them into callers whose layout is
  constant. Every pair therefore pays two calls, the class computation from the layout on both
  sides, and an indirect jump each way: about 17 cycles against the 8.0 floor of two calls and
  4.0 of an inlined pair.
- **Churn is 3 to 4 times the incumbents.** With random classes the two indirect jumps
  mispredict: 2.0 branch misses per allocate+free against 0.2 for jemalloc, with the same L1
  misses. The slow paths are under 2% of the time (`perf`); the cost is in the dispatch.
- **Three concurrent pipelines** (16.7 ns) cost more than one (4.8 ns), unlike jemalloc;
  presumably the depot's per-class stacks, shared by every CPU, contend. Not yet profiled.
- The direct `__rseq_offset` load saves about 0.5 cycle per pair here, less than the cycle the
  routine saves in isolation, since the calls and the dispatch dominate.
