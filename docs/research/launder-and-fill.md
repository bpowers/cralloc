# What launder and fill cost, and what made them cheaper

2026-10-17. The back end's launder (return a magazine's objects to the bitmaps) and fill (take
free objects from them) on their own and in the build-and-drop workloads where asmalloc trailed
jemalloc. "main" is ed43060, "head" the branch that made the changes below.

## Method

`bench/backend` (`backbench run`) drives `Back::launder` and `Back::fill` on a heap of its own,
in chunks of 64 as the depot does, over a pool of up to 64Ki objects (at most 16 MiB, so no
purge starts), and reports cycles and instructions per object from hardware counters, the
lowest of seven repetitions. Pointer orders: `asc` and `desc` (a magazine filled from the
bitmaps, used in order), `two` (two descending runs interleaved, as JSON's magazines come back),
`block` (shuffled within 256), `scatter` (shuffled over the pool, as a BTreeMap dropped in key
order frees), `stride` (64 words apart, the worst for grouping). Fills from full words, and from
words with one free slot in 8 and in 64. Pinned to CPU 2, sibling idle, three interleaved
rounds; the machine was otherwise quiet for the table below. Workloads: `bench/workloads`,
native, pinned to CPU 2, seven interleaved rounds, medians.

## The microbenchmark

Cycles and instructions per object, 256-byte class (16 and 4096 bytes behave alike):

| Case | main cyc | main ins | head cyc | head ins |
|---|---:|---:|---:|---:|
| launder asc | 12.1 | 76 | 10.9 | 52 |
| launder two | 21.4 | 131 | 14.5 | 66 |
| launder block | 34.0 | 140 | 26.1 | 76 |
| launder scatter | 85.6 | 489 | 58.6 | 294 |
| launder stride | 75.7 | 500 | 57.9 | 300 |
| fill, full words | 12.1 | 34 | 6.6 | 27 |
| fill, 1 free in 8 | 35.2 | 166 | 20.5 | 79 |
| fill, 1 free in 64 | 123.6 | 525 | 119.8 | 497 |

What each change bought, in the order made:

- **No sort; four open words.** The launder sorted its pointers; the input is either dense runs,
  which four open words group as well, or scattered, which no grouping helps. Most of the
  scatter gain is the next item.
- **One walk up the summary tree per batch** (`tree::Release`) instead of six levels per word.
- **Validation once per region met, with the layout's divisor**, not per object with the class
  arithmetic recomputed.
- **Fill writes a run of slots from one base**, one bounds check per word: halved a dense fill.
- **Settling looks at the clean bits once per claim** and goes run by run only if a page is
  clean: halved a fill from fragmented words.
- **A fill resumes at its last word** (`tree::claim_next`): a few cycles a claim, more for deep
  trees (16-byte slots, one free in 64: 135 to 119).
- **No copies between the depot and the back end** (`back::SlotPointer`).
- **Each class's layout worked out once per heap**: 4 instructions a slot on a dense fill.
- **Open words updated in place, inline**: `block` 45 to 28 cycles.
- **A short list of closed words.** Clearing a 64-entry list cost a dense launder over a cycle a
  slot; at 16 entries, with a part ending when the list fills, the dense case is faster than the
  sort it replaced.

Found on the way: on x86-64 an atomic OR whose old value is used (for the double-free check)
compiles to a load and a `lock cmpxchg` loop; it costs what one locked instruction costs, and
dominates a dense launder. Sampling on instructions or cycles puts its cost, and that of the
code before it, on the instruction after the lock; IBS (`-e ibs_op/cnt_ctl=1/`) does not.

Not helped: a launder's one walk to the top is the healing free and stays. A fill from words
with one free slot each costs two atomics a slot (the claim and the clear of the emptied word)
and is about where it started.

## Workloads

| | main | head | jemalloc |
|---|---:|---:|---:|
| BTreeMap 200k, cycles per entry | 783 | 748 | 669 |
| the same, drop phase, M cycles / M instructions | 42.2 / 207 | 30.6 / 120 | 19.5 / 61 |
| the same, build phase, M cycles / M instructions | 270.8 / 432 | 268.7 / 424 | 248.1 / 436 |
| JSON 8 MiB, cycles per byte | 25.4 | 22.6 | 25.6 |
| the same, drop phase, M cycles / M instructions | 188.5 / 463 | 152.0 / 277 | 138.6 / 281 |

The BTreeMap gap that is left is mostly the build phase, at equal instructions and equal page
faults.

The refresh rule (one retiring full magazine in 32 laundered), against the same build with it
off, run-phase instructions, three interleaved rounds: mixed churn with 100k live, main +4.7%,
head +2.3%; two-thread pipeline of small objects, main +4.9%, head +2.9%. Cycles moved less on
churn (+3.5% to +2.9%).

## Ascending magazines, and whole magazines in place (2026-10-18)

"before" is 91d97b9 (ascending magazines in, whole magazines not), "after" the branch that
followed. Same machine; CPU 2, sibling idle, other implementers building on other cores.

### Ascending order explained the walk

A fill wrote the lowest address into slot 1 and the routines pop from the top, so a magazine
handed out its 254 objects in descending address order, and successive magazines ascended: a
sawtooth. Filling from the top (`back::Descending`) made pops ascend. Box tree, depth 20, ten
builds, five interleaved rounds, medians:

| | descending magazines | ascending | mimalloc |
|---|---:|---:|---:|
| walk, M cycles | 531 | 211 | 229 |
| drop, M cycles | 1079 | 627 | 350 |
| build, M cycles | 529 | 448 | 355 |
| DRAM fills, M (whole process) | 9.9 | 3.6 | 4.0 |
| L2 fills, M | 9.6 | 20.1 | 29.8 |

Why, from a probe (`rustc -O`, a 2M-node tree of 24-byte nodes in an array, built and walked
in preorder, caches flushed before the walk, TSC ticks a node): nodes in ascending addresses
5.0, in descending addresses 5.0, in the sawtooth 14.1. The prefetchers follow a stream either
way; what defeats them is the reversal every 254 nodes (6 KiB), each of which starts a new
stream in the opposite direction.

### Whole magazines

The depot copied a magazine through a 64-entry buffer and called the back end once per 64:
nothing else needed the chunk, and group parts were sized to match it. The back end now reads
and writes the magazine's own slots, one call a magazine, so the host lookup, the group
setup, the walk up the summary tree (several locked ORs after a fill emptied the words) and
the retained-count add are paid once a magazine.

### The per-slot check

A launder must reject a pointer outside the class's regions, one that is not the start of a
slot, and one at or above the watermark. With a slot size `q << e`, `q` odd, and `n` the
pointer's offset from the region's base, `k = rotr(n * q⁻¹ mod 2^64, e)` is the slot index
when `n` is a multiple of the size, and above `(2^64 - 1) / size` otherwise (Hacker's Delight
10-17). So `k < watermark` is the whole check, one multiply and one rotate; a pointer below or
past the region wraps to a huge `n` and fails it too. Anything that fails goes to the full
check, which still aborts as before or, for a large block at its stagger, frees it. Kani
proves the check for every class, base, offset and watermark of a 4 GiB region
(`back::verify_slow`, seven minutes); a unit test covers the edges.

The dense loop then compares `k - first` against the held word's slot range, which covers the
word and the watermark at once, and sets the bit. Duplicates in the held word are caught by
counting slots against set bits when the word leaves the caller's hands, not by testing each
bit. The loop is 12 instructions a slot; it runs at about 2.5 cycles. Variants with fewer
instructions (walking by pointer, ten; two bit accumulators, to break the two-cycle `bts`
dependency) measured slower or no faster, and are not kept.

### The microbenchmark

`backbench`, now laundering and filling a whole magazine at a time into atomic slots as the
depot does (14 objects for 4096 bytes, 254 below 512); before rebuilt with the same harness.
Cycles and instructions per object, lowest of three interleaved rounds of seven:

| class | case | before cyc | before ins | after cyc | after ins |
|---|---|---:|---:|---:|---:|
| 16 | asc | 9.1 | 44.8 | 5.1 | 21.5 |
| 16 | desc | 9.1 | 44.8 | 5.1 | 21.5 |
| 16 | two | 11.7 | 58.8 | 9.8 | 52.0 |
| 16 | block | 22.3 | 68.1 | 20.2 | 67.5 |
| 16 | scatter | 54.7 | 285.0 | 57.1 | 315.3 |
| 16 | stride | 54.3 | 291.9 | 56.8 | 322.0 |
| 16 | fill | 7.1 | 29.1 | 4.7 | 16.1 |
| 16 | fill/8 | 20.8 | 81.0 | 19.5 | 77.7 |
| 16 | fill/64 | 118.9 | 498.6 | 123.6 | 502.5 |
| 256 | asc | 9.0 | 44.4 | 4.9 | 21.2 |
| 256 | desc | 8.9 | 44.4 | 4.9 | 21.2 |
| 256 | two | 11.5 | 58.5 | 9.6 | 51.6 |
| 256 | block | 22.2 | 67.7 | 20.0 | 67.1 |
| 256 | scatter | 54.7 | 284.7 | 57.0 | 314.8 |
| 256 | stride | 54.3 | 291.6 | 56.6 | 321.7 |
| 256 | fill | 7.1 | 29.0 | 4.6 | 16.0 |
| 256 | fill/8 | 20.8 | 80.9 | 19.4 | 77.6 |
| 256 | fill/64 | 118.7 | 498.5 | 123.6 | 502.3 |
| 4096 | asc | 32.4 | 112.2 | 29.6 | 93.0 |
| 4096 | desc | 32.4 | 112.2 | 29.5 | 93.0 |
| 4096 | two | 37.6 | 132.4 | 36.9 | 132.3 |
| 4096 | block | 56.5 | 152.2 | 56.9 | 162.5 |
| 4096 | scatter | 68.7 | 244.2 | 70.9 | 273.4 |
| 4096 | stride | 66.6 | 249.2 | 68.7 | 278.9 |
| 4096 | fill | 27.6 | 78.8 | 25.7 | 65.5 |
| 4096 | fill/8 | 48.9 | 141.2 | 48.1 | 136.5 |
| 4096 | fill/64 | 225.4 | 678.5 | 214.0 | 617.0 |

What is left on the dense paths, at 254 a magazine:

- Launder, 4.9 cycles: the loop about 2.5; about 300 cycles a call (the walk to the top of
  the tree, a locked OR at each level when the fill had emptied the words; the retained-count
  add; the host lookup, which unpacks the layout memo), 1.2 a slot; one locked OR and the
  group's bookkeeping per bitmap word, under one a slot. With 8192 a call the launder is 3.4.
- Fill, 4.6 cycles, 16 instructions: the store loop is 4 instructions a slot. Per claim of 64
  there are two atomics (the claim and the clear of the emptied word) and the descent, whose
  `lowest` and the count check in the claim run `count_ones` as a dozen instructions because
  the target has no `popcnt`. Built with `+popcnt,+bmi1,+bmi2,+lzcnt`, a dense fill is 4.1
  cycles, a fill from 1-in-64 words 98 instead of 124, a dense launder 4.9 unchanged.
- The 4096-byte class moves 14 objects a call, so the per-call costs dominate it (93
  instructions a slot).

Scattered launders (`scatter`, `stride`) are 4% slower and 10% more instructions: each slot
now pays the fast check's run set-up and exit before the group, about 30 instructions, of the
300 the group, the locked OR and the walk cost it. `two` and `block` are faster.

### Workloads

Five interleaved rounds, medians, CPU 2. "main" is 867aadf (ascending magazines, no whole
magazines); M cycles / M instructions per phase.

| | main | after | mimalloc | jemalloc |
|---|---:|---:|---:|---:|
| Box tree depth 20, cycles/node | 61.7 | 44.3 | 44.9 | 103.2 |
| build | 446 / 2030 | 296 / 1281 | 356 / 1593 | 506 / 1947 |
| walk | 213 / 441 | 210 / 441 | 230 / 441 | 333 / 441 |
| drop | 632 / 2269 | 420 / 1243 | 357 / 830 | 1326 / 2457 |
| BTreeMap 1M, cycles/entry | 1138 | 995 | 854 | 971 |
| build | 4781 / 5414 | 4185 / 5241 | 3790 / 5173 | 4232 / 5454 |
| drop | 863 / 2180 | 789 / 2251 | 480 / 281 | 626 / 791 |
| JSON 32 MiB, cycles/byte | 25.0 | 22.7 | 20.3 | 28.3 |
| parse | 2703 / 7997 | 2553 / 7384 | 2631 / 7129 | 3146 / 7634 |
| drop | 1491 / 3419 | 1256 / 2627 | 752 / 1286 | 1604 / 2770 |

The back-end smoke set (`benchrun run --suite backend --rounds 3`) agreed: tree depth 18 55.3
to 37.6 cycles/node (mimalloc 33.4), JSON 8 MiB 19.3 to 17.4 cycles/byte (mimalloc 16.0); the
two pipelines and sh8bench did not move beyond their noise.

What is left is mostly the drops. BTreeMap's drop frees its values in key order, which is
unrelated to the order they were allocated in, so the magazines it launders are scattered,
at about 300 instructions a slot (2251M instructions against mimalloc's 281M). JSON's drop
also launders scattered magazines and, past the small-class cap, purges memory the next parse
faults back (`baselines.md`, cause A). BTreeMap's build runs the same instructions as
mimalloc's in 10% more cycles; not analysed.
