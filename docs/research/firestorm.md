# What gates the fast path on Firestorm

An allocate and free costs the recommended routines 13.0 cycles on an Apple Firestorm core (4.3 ns
at 3.03 GHz), inlined or through two calls, at a constant size or through the tables, and a batch of
32 costs 13.1. The floor for a per-CPU magazine under restartable sequences (rseq) is 12.8. A C
`free`, which finds its class through the pointer, brings the pair to 22.0; a sized free keeps it at
13.1.

**Nothing in the routines sets this.** Forwarding the hot word from one call's commit to the next
call's load takes 6 cycles, two hops a pair. The pair retires 41 instructions at 3.2 a cycle on a
core that can do 8, and sixteen variations of instruction selection all land between 12.8 and 13.1.
A caller's own work runs beside the hops: with 16 adds after each call, a loop that takes 12.5
cycles alone takes 13.3 with the pair. So the hop binds only where a program does under about 6
cycles of work between calls, and a chain, not op count or code layout, is what to think about here.

**Firestorm has memory renaming, and a per-CPU word cannot use it as the routines stand.** A hop is
free when the load and store use one base register with an immediate offset and nothing writes the
base between them. A per-CPU word's base is computed afresh by every call. Keeping the slab's
address in a scratch register would qualify, and costs 7.0 a pair, or 4.5 with the rseq area
hoisted, but it needs the class as an immediate, which stable Rust cannot hand an `asm!` block
("Held, not built").

Ranked by what one event costs:

| Access pattern | Cost on Firestorm | Rule |
|---|---|---|
| A compare-and-swap that fails | 47 cycles, with a flush | Slow paths should read and test first where a failure is likely |
| A read-modify-write with acquire, or `ldaddl` | 18 to 22 cycles a repeat, against 3 to 7 relaxed and for `casl`. A lock-free stack's pop and push: 9.1 relaxed, 26.3 acquire then release, 42.1 both on both | None on a fast path; the weakest ordering that is correct on a slow path |
| A load of a word just stored by `stlr`, or `ldapr`/`ldar` of a word just stored | 19 cycles a hop, against 6 | Do not reload through either on a path that repeats |
| A misprediction | 17 cycles alone, about 24 in a routine | No branch on what varies from call to call, such as the last call's class |
| A class from the pointer | 9 cycles a pair | Rust's `dealloc` has it from the layout; C's sized free uses the size as a prediction |
| A hop with an add on the chain | 6 cycles | What binds; see above |
| A hop with no add | 4 cycles, so a list's pair costs 9.0 | A list beats a magazine here, but writes into freed memory, so the design stays |
| The branch units: `b.cond`, `cbz`, `adrp`, `adr`, and `mrs` on one of them | Two a cycle, 1 for `mrs` | Share an `adrp`; matters only when nothing else binds |
| Decode | 8 instructions a cycle, a taken branch ends a group | No cliff, no alignment rule |

Figures are per allocate and free unless they say otherwise. Everything is from one M1 Max core
under Asahi Linux (16 KiB pages). Evidence is 2026-11-07 at `96227a0`, with `45c8717` where stated:
CPU 8, `performance` governor, three interleaved rounds of three repetitions after a warm-up, 1 to 4
million iterations, the fastest repetition, from user-mode counters (`bench/roofline/README.md`,
"aarch64"). Spreads, how far the slowest round's fastest sits above that, are under 0.07 cycles
unless shown.

## Which rules are whose

**From aarch64, on every core:**

- A routine reaches the rseq area with `mrs` and an add: no segment override. An `adrp` with an
  addend reaches anything within 4 GiB, while a displacement reaches 32 KiB, so `MASK_DISP` cannot
  be one.
- `x0` is both an argument and a result. A sequence writes neither its arguments nor its result's
  register before the commit unless the restart sets it again.
- `b.cond` reaches 1 MiB and `tbz` 32 KiB, and no linker can extend them, so a routine branches only
  within itself ("Reaching the routine's own code").
- A direct `adrp` to `__rseq_offset` is refused by GNU ld in a position-independent executable.
- A load or store with writeback costs an extra micro-op and buys nothing here; avoid it.

**From any out-of-order core:** forwarding is what a tight pair waits on unless the core renames
memory; a branch on what varies costs more than most things it could save; a pointer that decides
which word a store writes makes the next load wait.

**Firestorm's own:** the renaming and its rules, load latencies of 3 and 4, the branch units'
budget, decode with no cliff, and no page-offset hold. On this core alone, the routines' size,
alignment and instruction selection are free choices. Where they cost nothing here they follow what
Neoverse V2 wants.

**For Neoverse V2 (Graviton 4, Grace), reasoned from public data, not measured:**

| Choice | Firestorm, measured | Neoverse V2, public |
|---|---|---|
| Hop | 6 cycles with an add | About the same in cycles: forwarding 5, a failure 10 to 11; no report of renaming (Chips and Cheese). A pair near 12 |
| `adrp` beside `add` | No fusion, no harm | Fuses (LLVM `FeatureFuseAdrpAdd`), so the two stay adjacent |
| `cmp`, `tst` with `b.cond` | Fuse; `subs`, `ands` too | Fuse only when the result is discarded (LLVM `FeatureCmpBccFusion`), so the routines use `cmp` and `tst` |
| Writeback | One more micro-op, same cycles | A separate integer micro-op (LLVM `WriteAdr`): avoided |
| `add x, x, x, lsl #12` | 2 micro-ops, 2 cycles | 2 cycles on the two M units; `lsl` up to 4 is 1 cycle on any: use `lsl` then `add` or `[x, x]` |
| `[x, x, lsl #3]` | No cost beyond `[x, x]` | One 4-cycle load (LLVM `WriteLDIdx`) |
| `adrp` count | Half a cycle each when binding | Four units, no budget to mind |
| Routine alignment | Free | A micro-op cache of 1,536 entries and 6-wide decode behind it may have a cliff, so out-of-line routines stay on 64 bytes, which costs nothing here |
| A load at a store's page offset | No hold | Arm describes a hash of the virtual address for forwarding; unknown, so the layout rule stays |
| `mrs tpidr_el0` | 5 cycles to an ALU, 1 a cycle | Not found |

None of the recommended shapes is known to hurt on V2. LLVM's numbers come from its V2 scheduling
model, which cites Arm's guide; the guide itself was not read. The rest is [Chips and Cheese on
Graviton 4](https://chipsandcheese.com/p/arms-neoverse-v2-in-awss-graviton-4) and Arm's [Hot Chips
2023 talk](https://hc2023.hotchips.org/assets/program/conference/day1/CPU1/HC2023.Arm.MagnusBruce.v04.FINAL.pdf).

## The floor and where the routines stand

Hand-written loops, cycles per allocate and free. A pair allocates one object and frees it at once,
through two calls each is out of line, and a batch is 32 allocations and then 32 frees.

| Shape | Pair | Through two calls | Batch |
|---|---:|---:|---:|
| Magazine, the word's address held in a register that never changes | 9.04 | 5.00 | 6.36 |
| List, the head's address held | 8.38 | 5.00 | 4.91 |
| Thread-local list through a base register | 8.89 | 8.91 | 9.71 |
| Per-CPU list under rseq, area and slabs held | 9.02 | 9.02 | 9.91 |
| Per-CPU magazine under rseq, area and slabs held | 12.78 | 12.78 | 13.08 |
| Thread-local magazine through a base register | 13.43 | 13.43 | 13.10 |
| tcmalloc's `Pop` and `Push` for aarch64, instruction for instruction | 13.91 | 13.90 | 14.12 |
| Recommended routines, default form | 12.96 | 13.00 | 13.11 |
| Recommended routines, `executable` form | 13.05 | 13.05 | 13.06 |
| Recommended, a word and mask formed at each site | 12.99 | | |
| Recommended, the class from the tables each call | 13.16 | | |
| Held: the slab's address kept in a register | 7.00 | 9.00 | 7.84 (+0.17) |
| Held, with the rseq area hoisted | 4.50 | 7.00 | 5.06 |

- **Through two calls costs what inlined costs,** unlike Zen 5, where the call is the floor.
- **A list beats a magazine,** the reverse of Zen 5. A magazine's add costs its hop 2 cycles: the
  add and the slower path from a load to an ALU.
- **The held bare magazine's pair is 9.04 and not lower** because its store and next load sit next
  to each other, so one of its two hops does not rename. A taken branch between them gives 2.00 on
  the word alone (`r_two_branch`).
- **tcmalloc** has the same hop on a 32-bit word. Its prefetches cost 0.2 a batch element and
  nothing a pair.

**With a caller's work.** Nops are width-bound work and adds a chain, three chains here. Cycles a
pass, inlined pair against two calls against the caller alone:

| Caller's work a pass | Inline | Two calls | Alone |
|---|---:|---:|---:|
| 64 nops | 13.04 | 13.02 | 8.25 |
| 128 nops | 20.50 | 21.00 | 16.25 |
| 256 nops | 36.50 | 37.00 | 32.25 |
| 32 adds, 16 after each call | 13.27 | | 12.49 |
| 64 adds | 24.66 | | 24.28 |

A pair adds 4.25 cycles of decode to a width-bound caller, and two calls 4.75. Against a chain it
adds under 1. How long a lone call takes to produce its pointer is measured between two `isb`s: pop
and push together add 40 cycles to a frame of 63, or 44 in the form through the GOT, about 20 a
call.

## How a load gets a value a store just wrote

Each cell is the loop `load; add 1; store` to one location, so 1.00 means the hop is free.

| Location | 64-bit | 32-bit | 16-bit | 8-bit |
|---|---:|---:|---:|---:|
| `[x, #16]` or `[x]`, base never written | 1.00 (+0.45) | 1.00 (+0.10) with `add w` | 6.65 | 6.66 |
| `[sp, #16]`, `sp` unchanged | 1.00 (+0.36) | | | |
| `[x, x]`, `[x, x, lsl #3]`, `[x, w, uxtw]` | 6.72 to 6.73 | 6.72 | 6.73 | 6.72 |
| Base written each pass, by `add`, `mov`, `adrp` or a load | 6.71 to 6.89 | | | |
| Load and store through different registers or forms, one address | 6.71 to 6.74 | | | |
| Thread pointer and a register, with `mrs` each pass | 6.50 | | | |
| Load pre-index, store post-index | 6.74 | | | |

**The rule for a free hop.** Same base register, no write to it between the store and the next load,
an immediate offset, 64 or 32 bits with the store's register written at that width, and enough
instructions apart. A write between the load and the store of one pass does no harm (1.00), and a
write between the store and the next load always does, even `mov x8, x8` (6.74) or moving `sp` and
back (6.70). Misaligned words rename, even across a 128-byte line (1.00).

By inference, rename gives the load the store's source register, and the load's execution only
checks it.

**How far apart.** A renamed hop runs at the greater of the add chain and the loop's width. With at
least 7 instructions from the store to the next load, every chain tested renames. With fewer it
depends on the loop. Cycles a pass, K adds between load and store, and fill nops plus the loop's
compare and branch after the store:

| K, fill | 0 | 1 | 2 | 3 | 4 | 5 and up |
|---|---:|---:|---:|---:|---:|---:|
| 1 | 1.00 | 1.00 | 1.00 | 1.00 | 1.66 | 1.25 and up, by width |
| 2 | 4.69 | 4.78 | 2.00 | 3.42 | 2.00 | 2.00 |
| 3 | 6.36 | 3.00 | 5.02 | 4.36 | 3.65 | 3.00 |
| 4 | 4.00 | 6.83 | 5.45 | 5.38 | 4.00 | 4.00 |
| 5 | 8.59 | 7.84 | 7.12 | 5.00 | 5.71 | 5.00 |
| 6 | 8.89 | 8.89 | 6.00 | 7.44 | 6.00 | 6.00 |
| 7 | 10.63 | 9.91 | 9.17 | 8.44 | 7.72 | 7.00 |

Two hops on one word a pass, store next to load, cost 8.00; 8 nops between or a taken branch make it
2.00. The bare pair's 9.04 is this case. Routines are far past the limit, with 12 or more
instructions from a commit to the next call's load. A loop's position in a line and in the rename
groups does not change this: the held pair ran 6.84 to 7.05 over 64 placements of its two calls, and
7.00 over 32 placements of its loop. Independent words rename together (two chains 1.00, three 1.50
by the stores).

**Without renaming,** a hop costs K + 5.7 to 6.0 in one chain, at any K from 2 to 16 and whichever
form the base has (`[x, x]` or a base written each pass): a bare load and store 4.6, with an add
6.7. Two hops a pass are cleaner: 8.00 with no add and 12.00 with an add each, so 4 and 6 a hop.
Writeback in place of the add, on a load or a store, is worse: 13.49.

**Overlaps.** A load inside a store forwards at the same price (store 64, load 32 low or high, 16,
8: 6.72 to 6.75), and so does a load wider than its store, at 6.74, with no penalty for the bytes
that come from the cache. `stp` then `ldp` costs 6.75 and two `str w` under one `ldr` 6.96. Two
`str` under one `ldp` costs 11.13, and read 6.5 in one earlier session, so it may have two modes.

**A dependence that holds sometimes** costs less than the hop: a store to one of N words, then a
load of the first, 2.4 to 3.5 a pass; with the load feeding the store, 6.67 at one pass in two and
2.5 to 2.9 from one in four. Order violations stay under 0.07 a pass.

## Held, not built: keeping the slab's address in a scratch register

A routine computes the CPU's slab's address as before, but compares it with what a scratch register
still holds and loads through that register, which is then not written. Memory safety rests on the
compare, since the register can hold anything. A CPU number masked by `ubfiz` needs no branch of its
own if the top two slabs stay unwritten; with `cmp` the held pair costs 7.59 and not 7.00.

```asm
    ldr   w8, [x10, #4]
    adrp  x9, SLABS
    ubfiz x8, x8, #12, #13
    add   x9, x9, x8
    cmp   x18, x9
    b.ne  fix                  ; mov x18, x9; b back
back:
    ldr   x1, [x18, #WORD]     ; renamed from the last commit to this class's word
```

What it buys: the pair above at 7.00, or 4.49 with the area and the slabs' address hoisted, which
Rust could do by reading the thread pointer in a `pure` block. A lone pop and push add 19 cycles to
the `isb` frame in place of 40, since the word's load no longer waits on the CPU number. With the
area and slabs hoisted the two calls cost 5.50 and the batch 4.81. The word's position must be an
immediate. Through `[x18, x23]` the pair is 13.05 again.

**Why not now.** A class known only after inlining cannot reach `asm!` as an immediate on stable
Rust. `const` operands need a Rust constant, a `match` over the classes is a jump table at run-time
sites, and `is_val_statically_known` is unstable. So this waits for that intrinsic. A clobbered
register sends the call through `fix`: 13.05 when that is steady, and 14.13 when it clobbers at
random on half the pairs (0.46 mispredicts).

**How long a register survives.** A preloaded probe (`bench/roofline/aarch64/stale`) left a sentinel
in four registers as each `malloc` or `free` returned and checked them at the next call. Percent of
calls that found it, and the misprediction rate of two simple predictors, per call site and gshare,
which a real predictor beats:

| Program, calls | `x18` kept, missed | `x15` kept, missed | `x9` kept | Same class and `x18`, missed |
|---|---|---|---:|---|
| `git log --stat`, 170 k | 96.9%, 2.5/3.7% | 93.7%, 1.9/3.7% | 76.6% | 22.0%, 14.2/9.7% |
| `cargo metadata`, 98 k | 98.9%, 3.5/4.5% | 96.6%, 4.9/6.0% | 30.0% | 30.3%, 16.8/8.4% |
| `clang -c`, 111 k | 85.0%, 12.6/24.3% | 73.8%, 15.4/27.7% | 21.2% | 36.8%, 15.7/20.2% |
| `node`, 95 k | 91.1%, 6.0/10.7% | 85.5%, 7.3/15.9% | 59.0% | 38.9%, 8.6/11.4% |
| `jq`, 164 k | 100%, 0.1/0.1% | 100%, 0.1/0.2% | 72.9% | 60.5%, 33.8/7.6% |
| `perl`, 1.2 M | 100%, 0.0/0.0% | 100%, 0.0/0.0% | 25.5% | 0.0%, 0.0/0.0% |

`x17` keeps nothing (0.0 to 0.3%), since every PLT stub writes it. Short runs include cold sites.
`x18` is the register to use on Linux, except that a program built with a shadow call stack reserves
it. At 17 to 24 cycles a misprediction, a call pays from nothing to about 2 cycles for it by the
per-site figures, so whether it pays in a program needs a job, not a loop.

**Rejected: keeping the word's address instead,** which needs no immediate and so works on stable
Rust. It costs a branch on whether the last call had this class, and classes do not repeat
predictably. Cycles a pair, or a batch element, plain against each kept form, with the class from a
stream of known behaviour:

| Classes | Plain | Word kept | Slab kept, word by `[x, x]` |
|---|---:|---:|---:|
| One class | 12.97 | 6.61 | 12.97 |
| Same as the last 7 times in 8 | 12.96 | 9.79 | 12.97 |
| Cycle of 8 | 6.00 | 6.75 | 6.63 |
| Random of 2 | 11.53 | 18.79 | 11.45 |
| Random of 4 | 7.30 | 12.68 | 7.10 |
| Random of 8 | 6.20 | 9.89 | 6.66 |
| Random of 2, batch of 32 | 11.62 | 34.15 (+0.31) | 11.52 |
| Same as the last 7 in 8, batch | 11.87 | 13.98 | 11.64 |

Different classes share no word, so the plain pair falls to one hop and 6. That is the common case
in a program, and the word kept loses there: in the programs above a call found its class and
register intact 0 to 60% of the time, and the simple predictors missed 8 to 34% of those branches on
all but `perl`. **Also rejected, unbuilt: separate words for allocation and free,** so that a pair's
hops form two chains. By the single hop's 6.7 that would halve a pair, but a batch is no better, and
objects would stop being reused in stack order.

## The core

| Resource | Measured |
|---|---|
| Integer ALU | 6 a cycle (5 adds and the fused loop branch at 1.00; 12 adds at 2.17) |
| Decode and rename | 8 a cycle |
| Eliminated: `nop`, `mov` between registers, `mov x, #0` | Take no ALU. A chain of three `mov`s runs out of whatever tracks them: 2.75 a pass |
| Not eliminated: `mov x, xzr`, other small immediates | Each takes an ALU (6 `mov x, xzr` a pass 1.17). So a routine clears with `mov x, #0` |
| Loads, stores | 3 and 2 a cycle, 4 memory operations together. `ldp` takes one load slot (3 a cycle), `stp` one store slot |
| Branch units | 2 a cycle: `b.cond`, `cbz`, `tbz`, `adrp`, `adr`. `mrs` also 1 a cycle on one of the two: `mrs` and a branch beside the loop branch take 1.83 |
| Fused with `b.cond` | `cmp`, `tst`, `ands`, `subs`, `adds`, immediate or register; not `ccmp`, or with anything between |
| Also fused | `movz` and `movk`. Not `adrp` and `add` or `ldr`, `add` and `ldr`, `lsl` and `add`, `mrs` and `add` |

| Latency | Cycles |
|---|---:|
| ALU, `lsl`, `ubfiz`, `and` | 1 |
| `add x, x, x, lsl #n` | 2, as 2 micro-ops, for any `n` tried |
| `mul` | 3 |
| Load to a load's base register, any addressing mode, width or writeback (pre, post) | 3 |
| Load to a load's index register | 4 |
| Load to an ALU | 4 |
| An ALU to a load's index, on top of the ALU | 1 more |
| `ldp`, second register | 4 |
| Writeback register to the next access | 1 |
| `mrs tpidr_el0`, `mrs cntvct_el0` to an ALU, from between `isb`s | 5 |

So from a load through a shift to the next load's result is 9 cycles by either road: `lsl` and `[x,
x]`, or `add lsl` and `[x]`. By these figures, from nothing to the hot word takes about 15 cycles in
the `executable` form and 19 in the default form. `cntvct_el0` reads at 1 a cycle. `isb` costs 29.

The clock is fixed. A loop of 1 add a cycle read 1.000 at 3.03 GHz over 10 ms to 100 ms, and 1.002
over a millisecond right after a second of idle, so there is no ramp for cycles to hide (`./run.sh
clock`).

**No page-offset hold.** A loop that loads a table byte, forms a word's address from it and stores
there ran 1.0 cycle a pass at every distance from -32 to +64 bytes, in the low 12, 14 and 16 bits,
in 16 pairs of fresh mappings each, and the allocation-shaped chain (hop-bound) 4.7. The layout rule
in `front/mod.rs` costs nothing to keep, and another aarch64 core may need it.

## Atomics and barriers

Cycles for one, alone on an uncontended L1 line. "Beside" is the same with 16 unrelated dependent
adds (16 alone) or 4 chained loads (12 alone).

| Instruction | Alone | Beside adds, loads |
|---|---:|---|
| `swp` | 3.0 | 16.0, 12.0 |
| `ldadd`, `ldadd` with no result | 6.7 | 16.0, 12.3 |
| Load, then a `cas` that succeeds | 6.6 | 16.0, 12.5 |
| `ldxr`, add, `stxr` | 7.0 | 16.0, 13.1 |
| Load, `casl` | 7.0 | 16.0, 12.6 |
| `swpal` | 18.0 | 18.0, 25.7 |
| Load, `casa` | 19.0 | 19.0, 24.2 (+0.45) |
| `ldadda` | 20.0 | 20.0, 20.4 |
| `ldaxr`, add, `stlxr` | 20.0 | 20.0, 22.5 |
| Load, `casal` | 21.0 | 21.0, 21.2 |
| `ldaddl`, `ldaddal` | 22.0 | 22.0, 22.3 |
| A `cas` that fails, `casal` that fails | 47.0, 49.0, one flush each | 48, 47; 50, 49 |
| `ldar`, `ldapr` | 1.7 | 16.0, 12.1; 3.0 a pointer chase, as `ldr` |
| `stlr` | 1.0 | 16.0, 12.0 |
| `dmb ish`, `ishst`, `ishld` | 1.7 | 16.0, 12.0 |

Almost nothing here serializes unrelated work: a cell beside other work is that work's own time or
the atomic's, except `swpal`, `casa` and `ldaxr` beside chained loads, which wait on them a little,
as acquire says they may. Two `ldadd`s on different lines cost 6.9, and two `ldaddal`s 22.7, so an
acquire-release is not a fence on other atomics either. `isb` is: 42 beside either. A lock-free
stack's pop and push: `cas` 9.1, `casa` and `casl` 26.3, `casal` 42.1, `ldaxr` and `stlxr` 41.0. A
swap and a store, an MPSC push: 18.1.

**What ordered accesses cost is a store's value on its way to a load.** `stlr` then a plain `ldr` of
that word, or a plain `str` then `ldapr` or `ldar` of it: 19.0 a hop, against 6. By inference,
neither forwards, so the load waits for the store to complete.

## The front end

**There is no cliff.** A loop of N nops and a fused loop branch takes (N + 2) / 8 cycles at every N
from 6 to 512. Every start position in a 128-byte window gives the same 1.00 to 5.00 at N = 2 to 38.
A function of up to 12 adds, called twice a pass, costs 2.50 a call, and past that the ALUs set the
pace. A function's position in a line changes nothing at 12 adds. At 24 and 36, where the ALUs bind,
cells move by up to 0.4 and 1.1 with no pattern in the offset, about what repeating them moves them
(+0.35, +0.64).

**A taken branch ends a group, and that is all.** Runs of 1 to 7 instructions, each ended by a taken
branch, take 1 cycle each and past that N / 8 (1.13 a run with the loop's own). A never-taken branch
takes one of the 2 branch units, nothing more: `cbz` and `tbz` 2.50 at 4 a pass with the loop's,
`b.cond` 2.76.

**A taken branch is free only while the loop's code spans about 4 KiB.** One taken branch a cycle
holds to 4,096 bytes at any stride from 4 to 256 bytes (1,024 branches 4 bytes apart, 64 at 64, 32
at 128), starts to slip between 4.6 and 5.1 KiB and reaches 2.5 to 2.9 by 8 KiB. By inference the
fastest predictor's reach is by address. Calling a leaf from each of many sites, against the same
sites with no call: 1.0 a call to 4 KiB of sites, 2.1 to 2.4 from 8 to 64 KiB. A leaf 2 MiB away
costs the same (`45c8717`, 800 thousand iterations, +0.05 at most). So a call in a large program
costs about 2 cycles more than inlining, and inlining's other gains here are under a cycle.

**Calls.**

| A pass | Cycles |
|---:|---:|
| `bl`, `ret` | 3.00, the loop's branch included; each further call 2.00 |
| Beside 8 dependent adds | 8.00, as the adds alone |
| `blr`, `ret`, one target or two | 3.00, then 2.00 |
| A tail branch in the callee | 4.00 |
| Through a PLT-shaped stub | 4.00, then 3.00: the stub's `br` |
| `blr` through a GOT-shaped slot | 3.00, then 2.00: the same as a direct call |

## Reaching the routine's own code

Every combination built under fat link-time optimization (`bench/roofline/aarch64/reach`):

| Where the miss's stub is | Small | A function past 1 MiB | 300 MiB of text between |
|---|---|---|---|
| `.subsection 1` | Yes | **Does not assemble** (fixup out of range) | Yes |
| `.text.unlikely`, the x86-64 routines' place | Yes | Yes | **Does not link** |
| A `label` operand | Yes | Yes, this once: the compiler kept its block close | Yes |
| No stub: a miss leaves the sequence, result zero | Yes | Yes | Yes |

In `.subsection 1` a stub lands right after the function's `ret`, in its own section. **Rule:** a
conditional branch in a routine targets only the routine's own end, so an inlined pop sets its
result to zero first and a miss falls out of the sequence. The abort handler holds a signature and
an unconditional branch, which reaches 128 MiB with no veneer from `.subsection 1`. No placement
costs a cycle: a miss's branch is not taken, and the 13.0 above includes the zeroing and the status
(`p_fin`).

| A direct `adrp` to `__rseq_offset` | Links |
|---|---|
| GNU ld (what `cc` gives here), PIE, the default | **No:** "can not be used when making a shared object" |
| GNU ld, not PIE | Yes |
| `rust-lld`, PIE or not | Yes |

Since the `executable` form links only with `rust-lld` or without PIE, and buys nothing in a pair
(13.05 against 12.96) and 4 cycles a pair of lone calls, the default form serves everywhere on
aarch64. Jobs do not ask for it either. Over five placements, two rounds each, both forms linked with
`rust-lld`, the default form against `executable`, mean over placements: `churn live=100k` +1.0%, the
Box tree -1.1%, `rpc log c64` +0.5%, `rpc echo c64` +0.5%. Each pair of ranges overlaps. `rpc log` was
the only real server in the set (2026-11-07 at `4ed7804`).

## The C entry points

Cycles a pair through direct calls. Sizes are 32 bytes or 64 in a cycle up to 512. Every routine
reaches its tables with `adrp`, so **no layout has to change and `generated/constants.rs` stays
shared.**

| Pair | Direct | PLT | GOT | 64 sizes |
|---|---:|---:|---:|---:|
| `malloc`, `free` | 22.00 | 22.08 | 22.07 | 21.65 |
| `malloc`, sized free | 13.11 | 13.10 | 13.10 | 13.07 |
| `calloc(1, 16)`, `free` | 22.08 | | | |
| `calloc(1, 32)`, `free` | 22.01 | | | |
| `malloc`, `free`, batch of 32 | 13.15 | | | |

- **The chain through the pointer is 9 cycles of a `free`.** The slot's load feeds a shift, a
  subtract and a table byte, which address the commit, and the next `malloc`'s load waits for that
  address. Unlike Zen 5, where this showed only on mixed sizes, it shows at one size.
- **Index the CPU's slab by the class, with no ALU after the table.** `add x3, slabs, cpu; ldr x1,
  [x3, x5, lsl #3]` saves 0.6 on a `free` against `add x3, slabs, x5, lsl #3; ldr x1, [x3, x8]`:
  21.98 and 22.56.
- **Keep the mask's load off the word's address.** A mask at `[word + 2048]`, in slab 0's unused
  half, encodes well and costs 1.45: 24.01. A mask by `[masks, x5, lsl #3]` does not wait on the
  add. The same effect as Zen 5's: a predicted branch's operand still costs.
- **One table with word and mask together saves nothing:** 22.48, 0.08 better than the base, with no
  scaled form.
- **Share an `adrp` when nothing else binds.** The routine's word after its descriptor, published
  words and masks in one page, the region table `add`ed from the masks: 13.11 against 13.79 on the
  sized pair, 13.07 against 13.82 on sizes in a cycle, nothing on `free`. A sized pair has 22
  micro-ops for the branch units, 11 cycles alone. `Front` keeps these places by construction today;
  a compile-time assert should say so.
- **`ubfiz` for the CPU check** saves a fused micro-op and 0.5 cycles to a width-bound caller only:
  24.50 and 25.00 with 128 nops a pass. Rejected for the plain routines: memory safety would rest on
  the kernel's numbers for nothing a pair can see.
- **The slow path's routines:** `slab_cas` 4.03 a call when it succeeds and 5.00 when it fails,
  `slab_stack_pop` and `slab_stack_push` together 9.09, `current_cpu` 3.00 out of line and 1.00
  inlined with the offset held.

## What turned out not to matter

Each was measured against the design's shape on the pair, at 13.05: all within 0.05 unless shown.

- **Order of the slot's load and the new word** (13.05), **pre-index on the pop** (13.05),
  **post-index on the push** (13.06).
- **The CPU check:** `cmp` and `b.hs`, `tbnz`, `ubfiz`, none at all: 13.05.
- **`[x, x]` against an `add` and `[x]`** (13.05), `add lsl` (13.09).
- **Immediate or register mask** (13.05). **An `adrp` and `ldr` of a descriptor through the GOT**
  (12.97).
- **The offset through the GOT or a word of the routine's own, against a direct load:** 12.93 and
  12.94, against 13.05, better by nothing that means anything. Holding the area in a register,
  12.81.
- **Alignment and bytes,** by the front end's table.
- **A load at a store's page offset,** by the probe above. On Zen 5 this took a loop from 1 cycle to
  25.

Taken together: on a tight pair, none of these is a choice. Choose the shape that proves most simply
and suits Neoverse V2.

## The recommended shape

The default form of a pop, with the word's address in `x3` and the mask in `w2`. The push and the C
routines have the same prologue and tail. The descriptor is followed by a word holding the address
of `__rseq_offset`, so a routine needs one `adrp`:

```asm
    adrp  x11, 3f                ; descriptor, then __rseq_offset's address
    add   x11, x11, :lo12:3f
    ldr   x9, [x11, #32]
    ldr   x9, [x9]
    mrs   x10, tpidr_el0
    add   x10, x10, x9           ; the thread's rseq area
5:  str   x11, [x10, #8]         ; arm; the abort handler returns here
6:  mov   x0, #0                 ; a miss returns zero
    ldr   w8, [x10, #4]
    cmp   w8, #8192
    b.hs  7f
    lsl   x8, x8, #12
    ldr   x1, [x3, x8]
    tst   w1, w2
    b.eq  7f
    ldr   x0, [x1, #-8]
    sub   x1, x1, #8
    str   x1, [x3, x8]           ; commit
7:
```

## Raw events

Validated on `apple_firestorm_pmu` against a kernel with a known answer. An event is tied to a
subset of counters, so sets of four can multiplex with counters free; two or three at a time are
safe.

| Code | Event | Check |
|---|---|---|
| `0x02`, `0x8c` | Cycles, instructions | Named in sysfs |
| `0x01` | Micro-ops retired: fused pairs count 2, `ldp` 2, a load's writeback 1 more | 3 adds and a loop 5; `ldp` and a loop 4 |
| `0x52` | Micro-ops scheduled: fused pairs count 1, eliminated ones 0, shows replays | 8 nops and a loop 1; 4 adds and a loop 5; `cmp`, `b.eq` and a loop 2 |
| `0x8d`, `0x90`, `0x94` | Branches, taken, conditional. `0x94` counts `b.cond` only | 4 `cbnz` and a loop: 5, 1, 1 |
| `0x95`, `0x96` | Integer loads, stores, by instruction | 3 loads 3; 2 stores 2; `ldp` 1; `ldadd` 1 load |
| `0xa6`, `0xa7` | Load and store unit micro-ops | The same; `ldadd` 1 and 1 |
| `0x9b`, `0x9c` | Loads and stores; barriers, not `isb` | 3 loads 3; `dmb` 1, `isb` 0 |
| `0xa2`, `0xa3` | L1 store and load misses. Zero on a hit. The load count reads 0.10 a line with the prefetcher, the store count 1.51 | A line per pass over 4 MiB |
| `0xb3`, `0xb4` | Exclusive or CAS success, failure; `ldadd` and `swp` count 0 | `cas` 1, 0; a failing `cas` 0, 1 |
| `0xc4` | Order violations, non-speculative | 0 where nothing aliases; 0.07 on the sometimes-aliased loop |
| `0xcb` | Mispredictions, all | 0.48 a pass on a random bit from a table |
| `0x75`, `0x84` | Map rewinds, flushes | 0.26 on 7% mispredicts; a failing `cas` 1 flush |
| `0x76`, `0xd6` | Rename stalled, dispatch bubbles | Not validated: read only as more or less |

## Open questions

- **Neoverse V2's numbers.** The harness runs there: `bench/roofline/README.md`, "aarch64". Whether
  it renames, what `mrs` costs, what a failed CAS costs.
- **Whether a kept slab address pays in a job,** given stable Rust's limit: lone-call latency on the
  C routines and run-time sites, against the mispredictions the probe suggests. A cargo feature
  could answer this once the port runs.
- **What in the core makes the K + 5.7 hop's fraction,** and why a chain of 3 `mov`s runs out.
- **Whether a load wider than a store really forwards,** as 6.74 suggests, or waits by another road
  that happens to cost the same.
- **What limits the taken-branch span:** why about 4,600 bytes, and what a call costs with the
  routine and its callers more than 4 KiB apart in the same loop, which the site sweep only
  approaches.
- **What real programs do to the hop.** The caller-work table is of nops and adds; `pw_` should be
  rerun under the allocator as a job once the port runs.
