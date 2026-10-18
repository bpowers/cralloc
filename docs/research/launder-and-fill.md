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

## Fillers of one class on many CPUs (2026-10-18)

sh8bench preloaded (16 threads on 8 CPUs, refilling the 8- and 16-byte classes together)
spent a quarter of its cycles in `tree::clear` and `tree::descend` under `Back::fill`. The
cause was fillers of one class contending for the same words of the bitmap tree. "main" is
867aadf, "branch" the change below.

## Method

`bench/backend/src/contend.rs` (`backcontend run threads=N every=E`): a heap of its own, a
pool of 64Ki 16-byte objects with one in `E` laundered (1: dense words; 8: one free slot in
8), then N threads each fill 254 objects (a 16-byte class's magazine) and launder them,
2000 times. Each thread counts its own user cycles and instructions over the loop; the
time-stamp counter splits them between fill and launder. Thread `i` is pinned to the `i`-th
of CPUs 0-3 (one L3) and 8-11 (the other), siblings idle; 16 threads put two on each.
Figures are cycles per object to each thread, two interleaved rounds (range where they
differed). Single-thread figures are `backbench` on CPU 6, two interleaved rounds. sh8bench
is the runner's `backend` suite, three rounds. Other implementers were building on the
machine at the time.

## What it was

Counted with eight threads and one free slot in 8, per successful claim: 2.7 restarts from
the class's top word, 2.1 leaf words found empty under a set summary bit, 0.56 failed
compare-and-swaps, and 3.1 atomic clears of a level-1 summary word that every filler
shares. Every filler walked to the same lowest word; the winner emptied it and cleared its
bit, while the others descended into it before the clear, cleared the bit again (a
redundant locked operation on the shared line) and restarted from the top. The failed
compare-and-swap, the obvious suspect, was the smaller part.

## What changed

- A claim retries its compare-and-swap against the value it found instead of restarting.
- A run of claims (a fill) moves past the group of 64 words where it meets another claimer
  (it lost bits to one, or found a word empty), so after a meeting or two each filler works
  in cache lines of its own, still near the bottom of the heap.
- A run leaves a word it finds empty to the thread that emptied it. A search from the top
  still clears what it finds empty, so a bit left by a run lost in a fork is cleaned by the
  next fill, and a fill reports nothing free only after a search from the top.
- A run clears the bits of the words it empties in one group with one atomic AND, then reads
  each word again and sets back the bit of any that is no longer empty.
  `tla/Summary.tla` (`Run`) checks this, and that without the re-reads a free is lost.

## Contended fill and launder

Fill, cycles per object per thread:

| Threads | dense, main | dense, branch | 1 in 8, main | 1 in 8, branch |
|---|---:|---:|---:|---:|
| 1 | 6.4 | 6.0 | 19.2 to 19.6 | 15.9 to 16.0 |
| 2 | 8.0 to 8.1 | 7.7 to 8.0 | 27 to 29 | 18.2 to 18.3 |
| 4 (one L3) | 15 to 18 | 6.7 to 8.7 | 56 to 123 | 20.7 to 21.0 |
| 8 (two L3s) | 60 to 85 | 23 to 26 | 664 to 691 | 33 to 51 |
| 16 on 8 | 91 to 93 | 24 to 25 | 604 to 702 | 41 to 50 |

Launder, cycles per object per thread:

| Threads | dense, main | dense, branch | 1 in 8, main | 1 in 8, branch |
|---|---:|---:|---:|---:|
| 1 | 8.2 | 8.3 | 11.2 to 11.4 | 11.4 |
| 2 | 10.3 to 10.4 | 9.9 to 10.4 | 12.9 | 12.4 to 12.8 |
| 4 | 7.6 to 12.7 | 9.9 to 10.7 | 18.3 to 19.3 | 13.0 to 13.2 |
| 8 | 17 to 37 | 13 to 20 | 47.5 to 58.6 | 18.0 to 20.9 |
| 16 on 8 | 18.1 to 18.4 | 13.4 to 19.1 | 58.8 to 64.4 | 22.7 to 24.1 |

Eight threads on CPUs 0-7, one L3: fill 15.5 dense and 32 one in 8, within about twice the
one-thread cost. Across two L3s it is 4 times (dense) and 2 to 3 times (one in 8). What is
left is cache lines crossing between L3s: in this benchmark every thread recycles the same
few hundred slots at the bottom of the heap, so each fill takes leaf words last written by a
launder or a claim on another CPU, and two fillers can share a leaf cache line (eight words)
without ever meeting on a word. The profile is the leaf word's load and compare-and-swap in
`tree::take` and the launder's OR in `Release::leaf`; the summary clears are gone from it.

## Single thread

`backbench`, cycles per object, 16-byte class (256 bytes alike; 4096 bytes moves the same
way):

| Case | main | branch |
|---|---:|---:|
| fill, full words | 6.7 to 6.8 | 7.2 to 7.3 |
| fill, 1 free in 8 | 20.7 to 21.2 | 18.5 |
| fill, 1 free in 64 | 119.4 to 119.5 | 103.5 to 103.6 |
| launders | unchanged within 0.5 | |

The fragmented fills are cheaper by the batched clear. A dense fill costs 0.4 to 0.5 cycles
an object more (25 to 30 cycles a fill of 64), with 1.5 more instructions an object; the
cause was not found. Inlined into `Back::fill`, `claim_next` had cost the fill's loop over its
slots two reloads from the stack a slot (3.3 instructions an object); it is now out of line.

## sh8bench

| | main | branch | mimalloc |
|---|---:|---:|---:|
| sh8bench 16, preloaded, s | 1.163 | 0.782 | 0.246 to 0.260 |

The other jobs of the `backend` suite were unchanged (the two-pipeline job measured 12.15 ns
an allocation on both, five rounds each). In the branch's profile the tree is about 9% of
the cycles (`tree::take` 5.3, `Release::leaf` 3.6), the depot's stack push and pop 13%, and
the `malloc` and `free` routines 30%.

## The routines in sh8bench

The generated `malloc` and `free` cost about 12 and 8.5 cycles a call in the two-L3 run
(CPUs 4-7 and 12-15), plus 2 for the PLT, against 8 to 10 a pair measured in isolation. They
retire 12 and 17 instructions a call, their straight-line length, so rseq aborts are rare
(2,248 context switches in the whole run), and branch mispredictions are about 0.01 a call.

Corrected 2026-10-18 (later the same day). An earlier version of this note blamed L1 misses on
allocator metadata, from per-symbol counts with the counters in one perf group. That was wrong:
a group's events are all sampled at the leader's (cycles) instruction pointer, so the L1-miss
column was cycles-weighted, not where misses happen. IBS op sampling (`perf record -d -e
ibs_op/cnt_ctl=1/`, `perf mem report`) on CPUs 0-7 puts 2,120 of 2,148 load samples in `malloc`
and `free` in L1 (5 in L2, 3 in L3, 20 in the fill buffer). On one L3 the pair costs
about 11 cycles plus the PLT, near the isolated figure, and a hand-edited experiment that cut
the metadata lines a pair touches from about seven to five changed nothing measurable in
sh8bench, larson, mstress or xmalloc-test. On one L3 about 40% of the routines' cycles
are dispatch slots the front end left empty (`de_no_dispatch_per_slot.no_ops_from_frontend`):
the call and return through the PLT. What makes the two-L3 run slower is not established; magazines
crossing between L3s through the depot (their slots and the objects they name written on the
other L3) are the likely cause and were not measured.

## Scattered launders (2026-10-18)

"main" is c9784ab, "after" this branch. CPU 2 under the shared measuring lock, the other L3
busy with other work; `backbench`, a magazine a call, lowest of three interleaved rounds of
seven; workloads five interleaved rounds, medians.

### Where a scattered slot's 315 instructions went

A scattered magazine puts each slot in a word of its own. Per slot, by `callgrind`: about 90
instructions in the group of open words, closed words and its filter (`group.rs`), about 120
in the walk up (`tree::Release`: each word owed its summary bit separately, and with the words
in arbitrary order nearly every debt was paid at once, with a region lookup and an address
computed from the plan each time), and about 50 entering and leaving the dense loop.

### What changed

- **Runs, a window and a map.** A run of slots of one word still gathers in the dense loop. A
  run that ends goes into a window of one or two summary words of leaf words around the
  batch's first slot (a plain array indexed by word), or, outside it, into a hash map of the
  batch's words. Every word met gets one OR at the end. Dense magazines, two interleaved runs
  and slots shuffled among a few words fall in the window; scattered ones go to the map.
- **Scattered batches stop making runs.** After eight runs of one slot in a row, the rest of
  the batch goes a slot at a time, and if most of it has fallen outside the window, the
  window's words move to the map and every slot goes there without asking which of the two it
  falls in: for scattered slots that branch is a coin toss.
- **One debt per summary word.** The window's words, and the map's grouped by summary word,
  are ORed with `Release::leaf_unowed` and owe their summary word one merged debt
  (`Release::owe_leaves`). The release protocol's steps are unchanged; only their batching is.
- **Duplicates are caught exactly** within a batch of up to 254 distinct words (the window
  and the map hold every word met until the end, each word in one of them); beyond that, a
  slot on both sides of a map flush is caught by its word's OR, as before.

Tried and not kept: a counting sort of the map's words in place of following the groups'
links (no change); a plain load, the double-free check, and an OR whose result is unused,
which compiles to a single `lock or` in place of a `lock cmpxchg` loop (scatter 52.2 to 51.2
cycles, stride 35.3 to 34.4, dense unchanged: not worth a double-free check that two threads
freeing one slot at once could both pass).

### The microbenchmark

| class | case | main cyc | main ins | after cyc | after ins |
|---|---|---:|---:|---:|---:|
| 16 | asc | 5.1 | 21.5 | 5.0 | 20.2 |
| 16 | desc | 5.1 | 21.5 | 4.9 | 20.2 |
| 16 | two | 9.9 | 52.9 | 9.8 | 42.3 |
| 16 | block | 20.5 | 69.1 | 11.6 | 46.1 |
| 16 | scatter | 57.2 | 316.1 | 52.2 | 115.5 |
| 16 | stride | 57.1 | 323.1 | 35.2 | 118.5 |
| 256 | asc | 4.9 | 21.1 | 4.8 | 19.8 |
| 256 | desc | 4.9 | 21.1 | 4.8 | 19.8 |
| 256 | two | 9.7 | 52.6 | 9.6 | 41.8 |
| 256 | block | 20.4 | 68.5 | 11.6 | 45.9 |
| 256 | scatter | 57.0 | 315.6 | 52.0 | 115.1 |
| 256 | stride | 56.9 | 322.7 | 35.1 | 118.1 |
| 4096 | asc | 28.8 | 92.1 | 30.1 | 94.2 |
| 4096 | desc | 28.8 | 92.1 | 30.1 | 94.2 |
| 4096 | two | 36.4 | 132.4 | 37.4 | 138.7 |
| 4096 | block | 57.4 | 163.0 | 47.8 | 146.1 |
| 4096 | scatter | 71.2 | 272.6 | 53.4 | 167.4 |
| 4096 | stride | 69.2 | 278.9 | 54.8 | 169.9 |

Fills are unchanged (dense 4.4 cycles, 16.6 instructions). The 4096-byte class launders 14
objects a call, and pays a cycle more on its dense cases for the window's set-up.

### The floor for a scattered slot

A scattered slot needs its own locked OR, about 20 cycles that do not overlap the next one's.
Built with a plain load and store in place of every leaf OR (measurement only), scatter runs at
41 cycles and stride at 21, at the same instruction count: the locks cost about 11 and 14
cycles a slot here, and the rest is ordinary work. Why random words cost scatter some 20
cycles a slot more than stride's regular ones, with the same instructions, is not found. Ruled
out: branch mispredictions (under one a slot in either, from `perf stat` differences), L1
misses (fewer in scatter than in stride), the order within a magazine (the same slots sorted
within each magazine, the bench's `spread`, cost the same as `scatter`), the dependent loads of
the flush's linked groups (a counting sort into a plain array changed nothing), and the locks
(the gap stays without them).

### Workloads

| | main | after | mimalloc | jemalloc |
|---|---:|---:|---:|---:|
| BTreeMap 1M, cycles/entry | 943 | 941 | 859 | 985 |
| build, M cycles / M instructions | 4005 / 5197 | 4023 / 5198 | 3797 / 5172 | 4289 / 5456 |
| drop | 709 / 2086 | 680 / 992 | 500 / 281 | 641 / 791 |
| JSON 32 MiB, cycles/byte | 16.2 | 16.6 | 16.4 | 23.1 |
| parse | 1956 / 7131 | 2010 / 7130 | 2108 / 7129 | 2548 / 7661 |
| drop | 764 / 1966 | 770 / 1882 | 642 / 1286 | 1337 / 2779 |
| Box tree depth 20, cycles/node | 43.8 | 43.9 | 44.9 | 102.4 |

The smoke set's single-thread jobs (three rounds; its two-thread jobs span both L3s and were
not run): tree depth 18, 40.6 to 39.3 cycles/node (mimalloc 33.7); JSON 8 MiB, 17.25 both
(mimalloc 16.0).

BTreeMap's drop halves its instructions and loses 4% of its cycles: a scattered slot's cost
is its locked OR and the work that random words cost besides. What is left against mimalloc is
the drop (180M cycles: one locked OR a scattered slot against mimalloc's plain pushes onto a
page's list) and the build (226M, at the same instructions). JSON is level with mimalloc
within the noise of these runs.

# A depot per cache domain (2026-10-18)

With one depot shared by every CPU, magazines that one L3 freed were popped by the other, and
each slot of the magazine and most of the objects it named were then a fill from the other
L3. This section measures that, and the depot that keeps magazines in the L3 that freed them.
"before" is main at 69bcd42; "after" is the change at f023136.

## Method

C programs preloaded, each with the same thread count in two placements: eight CPUs of one L3
(0-7) and four on each (0-3 and 8-11, the suite's placement). Each run under the shared
measurement lock, the libraries interleaved and rotated every round, medians. Cache fills from
the other L3 are `ls_dmnd_fills_from_sys.near_cache` per operation (437M mallocs for sh8bench,
6.75M for mstress, the reported rate times 3 s for larson and xmalloc-test). Suite jobs: the
runner, one round per invocation under the lock, the two builds alternating, five rounds,
medians and cgroup peaks. Builds and proofs by others ran on CPUs 12-15 and 28-31, which share
an L3 with 8-11.

## The cost of one depot

Before the change, on main b784f1a, five rounds:

| Program | Allocator | One L3 | Two L3s | Two / one | Fills from the other L3 per op |
|---|---|---:|---:|---:|---:|
| sh8bench, s | asmalloc | 0.345 | 0.756 | 2.19 | 0.137 |
| | mimalloc | 0.218 | 0.242 | 1.11 | 0.016 |
| | jemalloc | 0.486 | 0.599 | 1.23 | 0.002 |
| | tcmalloc per-CPU | 0.723 | 2.017 | 2.79 | 0.326 |
| larson, Mops/s | asmalloc | 584 | 395 | 0.68 | 0.077 |
| | mimalloc | 668 | 489 | 0.73 | 0.050 |
| xmalloc-test, M frees/s | asmalloc | 1074 | 373 | 0.35 | 0.271 |
| | mimalloc | 355 | 297 | 0.84 | 0.228 |
| | tcmalloc per-CPU | 509 | 188 | 0.37 | 0.275 |
| mstress, s | asmalloc | 0.277 | 0.290 | 1.05 | 0.63 |

Where the other L3's fills land, asmalloc on 0-3 and 8-11: in `malloc`, popping the slots of a
magazine the other L3 wrote (29% for sh8bench, 46% for xmalloc-test); in the program, touching
objects the other L3 freed (36%, 32%); in the depot's stack push and pop (15%, 10%). Putting
each stack head on its own line (a151233) changed nothing measurable: 0.752 against 0.727 s
for sh8bench, within the spread. About 81% of sh8bench's mallocs are served from magazines
that came through the depot, and the depot's traffic per malloc is the same in both
placements: about half of it crosses.

## The change

Full and empty stacks per cache domain, up to 64, found once at start-up from sysfs. A slow
path uses its CPU's domain; one whose stack of a class is empty takes from another domain's
stack only when that holds at least the take threshold, else fills from the bitmaps. A class's
target is shared among the domains whose stacks of it hold anything.

**The take threshold.** Measured at the whole target, half and a quarter (ns per allocation,
lower is better; the x2 to x4 jobs are independent pipelines placed in both L3s):

| Job | before | whole | half | quarter |
|---|---:|---:|---:|---:|
| pipeline t2 split all-alloc | 12.22-12.46 | 13.39-13.60 | 12.80 | 12.38 |
| pipeline t2 split free-only | 12.25-12.47 | 13.45-13.58 | 12.94 | 12.43 |
| exchange t2 split | 14.44-14.66 | 15.68-15.69 | 14.24 | 14.55 |
| pipeline t2 x2 split | 11.63-11.70 | 4.87-4.91 | 4.87 | 8.00 |
| pipeline t2 x3 both | 9.78-9.90 | 5.51-5.58 | 5.67 | 6.71 |
| mpmc p4 c4 both | 5.70-6.00 | 5.01-5.19 | 5.71 | 5.57 |

The whole target sends half of a split pipeline's flow through the bitmaps; a quarter lets the
independent pipelines take each other's magazines whenever a stack holds a little. Half keeps
both: the default. On xmalloc-test the threshold made no measurable difference.

## After

C programs, before and after, seven rounds:

| Program | Placement | before | after |
|---|---|---:|---:|
| sh8bench, s | one L3 | 0.339 | 0.337 |
| | two L3s | 0.720 | 0.606 |
| larson, Mops/s | one L3 | 585 | 588 |
| | two L3s | 402 | 411 |
| mstress, s | one L3 | 0.261 | 0.258 |
| | two L3s | 0.275 | 0.270 |
| xmalloc-test, M frees/s | one L3 | 1077 | 1058 |
| | two L3s, slow runs | 377-390 | 399-441 |
| | two L3s, fast runs | 732-741 | 725-756 |

xmalloc-test on two L3s is bimodal for every library, by where the scheduler puts each pair of
threads: about half the runs of each library land near 735 and half near 390, so its median
says nothing; the table gives the two groups. Fills from the other L3 per op fell by 40% for
sh8bench (0.113 to 0.068) and 17% for mstress. Peak resident memory of the C programs did not
move beyond the run-to-run spread.

Suite jobs, ns per allocation (ns per task for spawn), medians of five, and cgroup peak MiB:

| Job | before | after | peak before | peak after |
|---|---:|---:|---:|---:|
| pipeline t2 x2 split | 11.16 | 4.78 | 3.5 | 3.9 |
| pipeline t2 x3 both | 9.53 | 5.54 | 4.9 | 5.1 |
| pipeline t2 x4 both | 10.23 | 5.60 | 5.8 | 6.3 |
| exchange t8 both | 3.27 | 2.96 | 6.6 | 7.3 |
| mpmc p4 c4 both | 5.03 | 4.87 | 5.6 | 6.4 |
| exchange t2 split | 14.11 | 13.81 | 2.5 | 3.0 |
| pipeline t2 split all-alloc | 11.57 | 11.84 | 2.2 | 3.0 |
| pipeline t2 split free-only | 11.59 | 11.79 | 2.5 | 3.2 |
| pipeline t8 both | 4.36 | 4.38 | 7.2 | 8.0 |
| forkjoin t8 both | 3.92 | 3.93 | 17.3 | 17.6 |
| spawn t8 both | 262.3 | 259.8 | 4.9 | 4.9 |
| pipeline t2 local | 3.80 | 3.85 | 2.2 | 2.5 |
| exchange t2 local | 4.27 | 4.35 | 2.5 | 2.2 |
| mpmc p1 c3 local free-only | 5.26 | 5.40 | 3.4 | 3.5 |
| churn live=100k, native | 47.46 | 47.55 | 12.2 | 12.2 |

The independent pipelines in two L3s are 1.7 to 2.3 times faster, ahead of jemalloc's 8.01
for x2 split in the session where we trailed it by 35%. The split pipeline, where the shared
depot is why we lead (jemalloc 18.35), gives up 2%. Jobs in one L3 moved by -1 to +3%, within
the spread of the earlier run of the same comparison (-1 to +2%).

**Parked memory.** Jobs that use both L3s peak 0.3 to 0.8 MiB higher. Sharing a class's target
among its domains did not change that; it comes from the take threshold: a domain's frees wait
in its stack up to the threshold before the other domain takes them, and meanwhile the other
fills from the bitmaps, carving new slots for each class in use. Single-L3 jobs and the
memory worst cases (`bench/probes/memory/worst.sh`: phase change, many CPUs and few classes,
many threads on few CPUs) end where they did; the many-CPUs case peaked at 6.1 to 7.0 MiB
against 4.5 to 5.7.
