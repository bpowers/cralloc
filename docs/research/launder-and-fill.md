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

## A direct path for scattered launders (2026-10-18, later)

"before" is main at 1868dd0, "after" this branch merged with it. Timed runs on CPU 2 under the
shared measuring lock, builds interleaved in random order, medians of five rounds (backbench:
of five runs each the lowest of seven repetitions). Builds and model checks of other work ran on
the second L3.

### Method: exact instruction counts of a drop phase

- Callgrind, with the drop phases of `bench/workloads` routed through one out-of-line function
  (`drop_phase`, named with `--toggle-collect`). Two things had to be fixed for it to see what
  runs natively. Stock valgrind caps the client at 64 GiB of address space, so asmalloc halves
  its regions to 8 MiB, magazines span regions and every slot takes the launder's slow check
  (BTreeMap 1M: 337M instructions a drop under callgrind against 189M natively). A valgrind
  3.27.1 built with `aspacem_maxAddr` raised from 128 GiB to 64 TiB
  (`coregrind/m_aspacemgr/aspacemgr-linux.c`) takes the full reservation. And under valgrind
  glibc's rseq registration fails, so every call takes the no-rseq path; a preloaded
  constructor that writes 0 into `cpu_id` of the failed (so unregistered) area makes the
  single-threaded workloads run the routines as natively. With both, callgrind's counts for a
  drop match `perf stat`'s within a few percent, except `memset`: under valgrind glibc picks the
  AVX2 variant, whose `rep stosb` callgrind counts per byte.
- Frees per drop counted with a counting `GlobalAlloc`: BTreeMap 1M, 1.134M; JSON 32 MiB, 3.574M;
  Box tree depth 20, 2.097M.
- The workloads also accept `BENCH_PERF_CTL=CTL,ACK` to switch a `perf record -D -1 --control`
  on for drops only. Sampling on instructions piles samples onto the instruction after a
  locked one (25% of one profile at a single address), so it was not used for attribution.

### Where the drop's instructions went, per freed object

| | asmalloc before | asmalloc after | mimalloc | jemalloc |
|---|---:|---:|---:|---:|
| BTreeMap 1M drop, native, all | 165 | 112 | 49.5 | 137 |
| the program's own drop code | 22 | 22 | 22 | 22 |
| free (routine, class from layout; `mi_free`; `sdallocx` fast path) | 23 | 23 | 27 | 27 |
| launder | ~112 | ~60 | | |
| jemalloc's tcache flush (bin lookups, bitmap) | | | | ~60 |
| slow path's exchange, depot, heartbeat | ~7 | ~7 | | |

Before, the launder of a scattered magazine was the whole of the excess over mimalloc: grouping
(window, map and groups) 50, the walk up the summary tree 11, the per-slot check and loop 17,
host lookups 7, the ORs 8, the rest inlined helpers. JSON is different: its launders are dense
(two interleaved runs), about 25 instructions an object, and the rest of its excess over
mimalloc (106 against 72 an object) is the slow path's exchange and depot (about 5), the
heartbeat (2 to 4; clock-driven, so more under valgrind) and a few instructions of inlined free
glue; the program's own drop is the same for all.

### What changed

- **The judgement.** As before, after eight single slots in a row a batch is judged scattered if
  most of its words fell outside the window. Words outside the window go first to a list of
  eight (`Gather::outside`), so a batch judged scattered never makes the hash map.
- **The direct path** (`group::Direct`). The words gathered so far are ORed, and each later
  slot is ORed on its own with `lock bts` into its leaf word, found by index in the region's
  committed leaf words (`Host::leaves`, ledger BACK-HOST-03). The OR's old bit is the double-free
  check. What the leaf words owe their summary words is merged in a table of 64 places
  (`Debts`); a summary word is paid when its place is taken or at the end, by reading it and
  setting only the bits it lacks; what that owes the level above is merged in a second table,
  and only then handed to `Release`.
- **Reading before setting.** A free whose word was empty used to set the summary bit unread
  with a locked OR. The direct path's debts read it first (`Look` in `tla/Summary.tla`), which in
  a scattered batch, whose summary words mostly have the bit set already, is a plain load. TLC
  finds no violation with it, with runs of claims and the healing free, on one and two summary
  levels and in a forked child (`Summary_look*.cfg`); the explorer runs the production code
  against claims and forks (`direct_frees_*`).
- **A double free no longer caught.** A slot given twice in one scattered batch, which another
  CPU claims between the two ORs, is the same double free that already goes unseen across two
  batches. Without a claim between, the second OR catches it.

### backbench

Cycles and instructions per object, a magazine a call:

| class | case | before cyc | before ins | after cyc | after ins |
|---|---|---:|---:|---:|---:|
| 16 | asc | 5.0 | 19.7 | 4.9 | 19.8 |
| 16 | desc | 5.0 | 19.7 | 4.9 | 19.8 |
| 16 | two | 10.0 | 42.7 | 8.4 | 38.8 |
| 16 | block | 11.7 | 46.6 | 10.3 | 43.1 |
| 16 | scatter | 51.1 | 106.9 | 27.5 | 56.0 |
| 16 | stride | 34.3 | 108.5 | 26.9 | 55.5 |
| 16 | fill | 4.5 | 16.0 | 4.5 | 16.0 |
| 256 | asc | 4.7 | 19.3 | 4.6 | 19.4 |
| 256 | desc | 4.7 | 19.3 | 4.6 | 19.4 |
| 256 | two | 9.8 | 42.3 | 8.2 | 38.4 |
| 256 | block | 11.6 | 46.4 | 10.2 | 43.0 |
| 256 | scatter | 50.8 | 106.5 | 27.3 | 55.5 |
| 256 | stride | 34.1 | 108.1 | 26.7 | 55.1 |
| 256 | fill | 4.3 | 15.9 | 4.3 | 15.9 |
| 4096 | asc | 29.9 | 85.8 | 28.7 | 88.1 |
| 4096 | desc | 29.8 | 85.8 | 28.7 | 88.1 |
| 4096 | two | 37.4 | 129.2 | 36.4 | 129.6 |
| 4096 | block | 47.0 | 135.5 | 45.5 | 136.9 |
| 4096 | scatter | 52.8 | 151.1 | 52.0 | 151.6 |
| 4096 | stride | 54.1 | 152.8 | 53.3 | 153.2 |
| 4096 | fill | 21.2 | 75.5 | 21.3 | 75.5 |

The 4096-byte class launders 14 objects a call, too few to be judged scattered, and is
unchanged. Of a scattered slot's 56 instructions about 40 are the loop (the check, the
`lock bts`, the debt); the rest is the batch's first slots, taken as runs, and paying the
summary words.

### The size of the bitmap, prefetching, and the cycles random words cost

`backbench run classes=8 bitmaps=SIZE`: carve as many 8-byte slots as a leaf bitmap of SIZE
covers (16 KiB fits L1, 512 KiB L2, 8 MiB L3, 64 MiB does not), touch each bitmap page once,
then time launders of 64 magazines of slots in random (`scatter`) or stride order and fill them
back untimed. Per object, three rounds, medians; misses are per object from `BENCH_EVENTS`
(`ls_l1_d_tlb_miss.all`, `.all_l2_miss` as walks, `l2_cache_req_stat.ic_dc_miss_in_l2`, demand
fills from DRAM). `pf1` decodes 16 slots ahead and prefetches each one's leaf word, `pf2` loads
it instead (experiments, not kept).

| bitmap | order | build | cyc | ins | dTLB miss | walk | L2 miss | from DRAM |
|---|---|---|---:|---:|---:|---:|---:|---:|
| 16K | scatter | before | 55.8 | 120.7 | 0 | 0 | 0 | 0 |
| 16K | scatter | after | 30.5 | 61.7 | 0 | 0 | 0 | 0 |
| 16K | scatter | after, pf1 | 32.3 | 77.7 | 0 | 0 | 0 | 0 |
| 16K | stride | before | 37.0 | 121.3 | 0 | 0 | 0 | 0 |
| 16K | stride | after | 29.2 | 61.3 | 0 | 0 | 0 | 0 |
| 512K | scatter | before | 105.2 | 337.6 | 0.49 | 0 | 0.04 | 0 |
| 512K | scatter | after | 73.6 | 178.2 | 0.48 | 0 | 0.05 | 0 |
| 512K | scatter | after, pf1 | 75.9 | 193.1 | 0.46 | 0 | 0.03 | 0 |
| 8M | scatter | before | 158.3 | 428.4 | 1.24 | 0.01 | 1.02 | 0.04 |
| 8M | scatter | after | 135.2 | 357.2 | 1.25 | 0.01 | 1.02 | 0.02 |
| 8M | scatter | after, pf1 | 129.1 | 372.4 | 1.27 | 0 | 1.01 | 0 |
| 64M | scatter | before | 436.1 | 453.4 | 1.90 | 0.78 | 1.93 | 0.58 |
| 64M | scatter | after | 340.0 | 417.4 | 1.93 | 0.78 | 1.90 | 0.73 |
| 64M | scatter | after, pf1 | 169.2 | 432.5 | 1.94 | 0.78 | 1.89 | 0.03 |
| 64M | scatter | after, pf2 | 196.8 | 434.2 | 1.94 | 0.78 | 1.84 | 0.74 |
| 64M | stride | before | 75.5 | 256.2 | 0.14 | 0.12 | 0.08 | 0 |
| 64M | stride | after | 208.4 | 179.6 | 0.14 | 0.12 | 1.03 | 0.84 |
| 64M | stride | after, pf1 | 82.6 | 194.4 | 0.14 | 0.12 | 0.23 | 0 |

- **The 20 cycles were not misses.** With the bitmap in L1, before, scattered slots cost 19
  cycles more than strided ones at the same instructions and no misses at all; after, 1.3. They
  were the map's and groups' work on random keys, and went with them. (`perf stat` of the whole
  process, the earlier way, showed no dTLB, L2 or store-forwarding difference either.)
- **Prefetching pays only past L3.** It costs 16 instructions and about 2 cycles a slot while
  the bitmap is in L1 or L2, a few percent at 8 MiB, and halves the cost at 64 MiB, where each
  slot is a DRAM miss. A plain early load does less. Left out. One case got worse: strided
  slots over 64 MiB of bitmap, 75 to 208 cycles, with a DRAM fill a slot where there were none.
  The old path ORed a batch's words at its end, grouped by summary word, which the hardware
  prefetchers followed; the direct path ORs them in the batch's order. Prefetching would
  recover it (83).
- **The TLB.** Page walks appear only at 64 MiB of bitmap (0.78 a slot): 512M live slots of one
  class. At 8 MiB there are L1 dTLB misses that the L2 TLB serves. Backing a region's leaf
  bitmap with huge pages once it passes 2 MiB (16M slots of the class) would cost at most the
  rest of one 2 MiB page per such region, under 2% of the objects' memory for 8-byte slots and
  less for larger ones; below that size it is not worth anything. Not built.
- **A batch spread over more summary words than it has slots** (512 KiB of bitmap and up: 4M
  live slots of the class) pays a summary word for nearly every slot, about 120 instructions a
  slot more than the 16 KiB case. It is half what it was; a cheaper payment is the next thing
  to try if such heaps matter.

### Workloads

M cycles / M instructions per phase, five interleaved rounds, medians:

| | before | after | mimalloc | jemalloc |
|---|---:|---:|---:|---:|
| BTreeMap 1M, cycles/entry | 934.5 | 911.8 | 868.5 | 989.4 |
| build | 3995 / 5194 | 4005 / 5193 | 3844 / 5166 | 4348 / 5465 |
| drop | 670 / 932 | 554 / 632 | 498 / 280 | 605 / 776 |
| JSON 32 MiB, cycles/byte | 16.6 | 16.3 | 16.6 | 24.9 |
| parse | 1986 / 7119 | 1957 / 7119 | 2117 / 7129 | 2589 / 7640 |
| drop | 791 / 1887 | 773 / 1891 | 659 / 1286 | 1596 / 2770 |
| Box tree depth 20, cycles/node | 43.3 | 44.1 | 42.7 | 101.6 |
| build | 291 / 1291 | 291 / 1291 | 354 / 1593 | 509 / 1946 |
| drop | 410 / 1208 | 412 / 1211 | 344 / 830 | 1283 / 2457 |

The BTreeMap drop runs 32% fewer instructions and 17% fewer cycles, ahead of jemalloc on both;
mimalloc is 56M cycles ahead, at a quarter of the instructions. JSON's and the tree's drops are
dense and unchanged (the tree's walk, which runs no allocator code, moved 206 to 223 between
builds, so its total is within layout noise).

### Wide batches

"before" is main at d56e3bf, "after" the branch that followed; same method.

A class with a few million live objects has more summary words than a magazine has slots, so
a scattered batch's slots almost never share one, the debt table merges nothing, and each slot
paid for a place taken and another summary word's debt through `Release`'s general walk: 174
instructions a slot at 512 KiB of bitmap (4M slots) against 62 at 16 KiB.

**Paying each debt at once does not beat the table where the table merges.** Built as the
simplest form (each slot pays its summary word by looking, and hands what that owes upward to
the second table), it took the 16 KiB case from 64 to 107 instructions and 31 to 47 cycles, and
the 512 KiB case from 173 to 123. It cannot stop where it finds a bit set: that is the design
without the healing free, which `Summary_crash.cfg` shows leaves a bit cleared by a thread lost
in a fork unrepaired for good. So the walk above the first level still has to be paid, merged
or not.

**So a batch is judged wide or narrow once**, as scattered or dense is: its first 64 direct
slots go through the table, and if 56 or more of them took places of their own, the rest of the
batch is wide. Strided and clustered slots come back to summary words they met and stay narrow;
a judgement after 32 slots misjudged a strided batch over 32 summary words (16 KiB) and cost it
16 cycles a slot. A wide batch:

- pays the table's debts by walking up at once;
- for each later slot, looks at its summary word, found by index in the region's level-1
  words (`Host::tree_words`, BACK-HOST-03, now any level), and merges what that owes level 2,
  which has 64 times fewer words;
- pays a level-2 debt whose place is taken by walking the rest of the way up through the
  region's levels held the same way (`Release::walk_upward`), not through `Release`'s one debt
  per level, which at 8 MiB and beyond paid nearly every slot generically.

Every payment reads before it sets, as modelled (`Look` at level 1, the model's `Up` above it),
and nothing is reordered across the leaf bit it answers for. In the model each free whose word
was empty now chooses whether to look, as the code mixes them on one tree (scattered launders
look; grouped launders and single frees do not): `Summary_look`, `_look_deep` and
`_look_crash` pass with the mix, in about 23 minutes together. An explorer runs the wide path
against claims and forks (`wide_direct_frees_*`), and a unit test frees a batch over 1024
summary words and takes it back.

Bitmap sweep, 8-byte class, per object, three rounds, medians (after the stack changes below):

| bitmap | order | before cyc | before ins | after cyc | after ins |
|---|---|---:|---:|---:|---:|
| 16K | scatter | 30.6 | 64.0 | 30.6 | 61.7 |
| 16K | stride | 29.4 | 63.6 | 29.2 | 61.4 |
| 512K | scatter | 74.3 | 173.4 | 54.4 | 124.0 |
| 512K | stride | 76.8 | 173.8 | 53.4 | 122.0 |
| 8M | scatter | 133.1 | 352.6 | 101.8 | 267.2 |
| 8M | stride | 77.6 | 174.3 | 58.7 | 123.7 |
| 64M | scatter | 329.3 | 413.1 | 268.4 | 287.9 |
| 64M | stride | 216.6 | 174.7 | 139.4 | 123.7 |

dTLB and cache misses per object are unchanged between the builds at every size; in this bench
a fill empties the words between rounds, so many slots find their summary word empty and pay a
locked OR, as the first frees into a full heap do. Walking from level 1 for every wide slot,
without the level-2 table, measured 164, 204 and 210 instructions at 512 KiB, 8 MiB and 64 MiB:
better past 8 MiB, worse at the size a program with a few million objects of a class has, so
not kept.

backbench, a magazine a call, five rounds: unchanged or slightly better in every case and class
(16-byte scatter 27.2 to 26.9 cycles, 55.5 to 52.8 instructions; dense cases within 0.2 cycles;
the 4096-byte class 2 instructions fewer).

**A scattered head and a dense tail** (`headtail`: each magazine's first 24 slots swapped with
slots anywhere in the pool) is judged scattered and takes a locked OR per slot of its tail.
Against the grouped path before the direct path (550d70c, the same harness), 16-byte class:
22.4 cycles and 70.9 instructions there, 27.1 and 52.7 here: 4.7 cycles a slot more. Not worth
a second judgement.

**Stack.** The slow paths run on whatever stack called `free`. The direct path had grown the
launder's frame: its tables beside a map sized for a whole magazine, and the map moved onto a
frame of its own when the batch turned direct. `workload stack` (`bench/workloads/src/stack.rs`,
release build) paints a thread's stack, frees a million objects in random order and then in
order, and reports the deepest byte written below the loop's frame; `bound=BYTES` makes it fail
above a bound. Peak: 16223 bytes at d56e3bf; 6151 now, with the map and the direct path in one
place (`Spill`), the map flushed where it lies and holding at most 63 words, and the upward
table at 32 places. The deepest frames: `Cache::launder_held` with the launder inlined, 4280
bytes (the window, 1 KiB; the map or the direct path's tables, about 2.3 KiB), `Words::flush`
856 when the map is used, `Bound::free_slow` 296. A batch that meets more than 63 words without
being judged scattered now flushes its map, and a slot on both sides of that flush is caught by
its second OR, as for a scattered batch.

Workloads, M cycles / M instructions, five interleaved rounds, medians:

| | before | after | mimalloc | jemalloc |
|---|---:|---:|---:|---:|
| BTreeMap 1M drop | 570 / 632 | 563 / 630 | 525 / 280 | 630 / 776 |
| JSON 32 MiB drop | 834 / 1891 | 826 / 1889 | 698 / 1286 | 1663 / 2770 |
| Box tree depth 20 drop | 421 / 1211 | 419 / 1209 | 371 / 830 | 1324 / 2457 |
| BTreeMap 8M (reps=2) drop | 2910 / 3346 | 2622 / 2931 | 2234 / 896 | 2947 / 2555 |
| BTreeMap 8M, cycles/entry | 1817.3 | 1730.2 | 1701.4 | 1963.9 |

The narrow drops did not move. BTreeMap at 8M entries (about a million live objects in each of
its busiest classes, a gigabyte resident) is the wide case in a workload: its drop runs 12%
fewer instructions and 10% fewer cycles, ahead of jemalloc in cycles and behind it in
instructions; mimalloc is 15% faster at the drop and 2% over the whole job.

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

# Domain stacks and a shared stack (2026-10-18)

A review of the depot per cache domain found three faults that need three or more domains,
which is every server: with a third domain holding a class, the heartbeat kept each stack at
or just under the take threshold, so a flow between two other domains went through the
bitmaps; N domains in use parked N/2 targets; and a miss in a domain's stack read the stack
head of every other domain on the machine, as did the look for an empty magazine and the
heartbeat's look at a class. This section replaces the take-across rule. "before" is main at
ee4c29b; "half" and "quarter" are the change at 0f1d653 with the domain cap at half of the
class's target and at a quarter, the default from 1dbe734.

## The change

Per class, one stack per domain and one shared stack; per magazine size, the same for empty
magazines. A slow path pushes on its domain's stack while that holds less than the cap, else
on the shared stack, and pops its domain's stack, else the shared one, else fills. The cap is
exact: the push checks the count it reads from the top's link, and its compare-and-swap
succeeds only if the head has not changed since. The shared stack's target is the class's
target less the cap, so a process on one domain parks what it did with one stack. The
heartbeat launders the shared stack while it is over its target; every stack sheds the depth
it did not need. Nobody reads another domain's stack on a slow path.

A domain that goes quiet sheds through the other domains' looks. Each domain keeps a word of
the classes whose stacks may hold magazines, and the depot a word of the domains whose word is
not zero: a push onto an empty stack sets the bits, a look that finds a stack empty clears
them, and a clear rereads the head so that a racing push is never lost (`depot/held.rs`). Each
periodic look visits its own domain's stack and the shared stack of one class, the classes in
turn, and every held stack of one other domain, the domains chosen from those words in
rotation (at first it was one stack of one other domain; see "Corrected" below). A CPU's looks
visit every held stack of the `n` other domains that hold anything once in `n` looks.

**Corrected (2026-10-19).** The figures first given here, everything back after 3.0 intervals
with 2 and 8 domains and 3.125 with 64, came from a test that looked on every slow path,
sixteen times the allocator's rate. A review measured the first version at the allocator's
rate (`LOOK_EVERY`, one busy CPU at 640 slow paths an interval, stacks parked with no slabs to
drain): quiet stacks went back in 2 intervals for 7 domains of 5 classes, 7 for 63 of 2, and
16 for 63 of 5, against 3 in every case at ee4c29b. A stack owes nothing until a second look,
in a later interval than the first, and each look visited one stack. With every held class of
the domain a look visits, the same cases take 2, 4 and 4 intervals; the look's work grows with
the classes one domain holds, not with the domains. Counting the quiet CPUs' slabs too, which
the idle check drains one slab a look, everything went back in 2, 4 and 6.4 intervals with 2,
8 and 64 domains of two classes (`depot/rules.rs`, both tests at the allocator's rate).

Visiting a whole domain is not free when that domain is in use: three independent pipelines
over two L3s (x3) ran 5 to 7% slower in every round (5.22-5.40 ns before, 5.57-5.78 after),
because each look read and sometimes wrote the other L3's busiest stack lines. A domain in use
sheds through its own looks, so each domain now records the last interval one of its CPUs
looked (one store an interval), and a look from elsewhere skips a domain that looked in this
one; the quiet cases above did not move. Against 23def4e, five interleaved rounds, the
per-round ratio's median and range: x3 0.970 (0.93-1.13), exchange t2 split 1.004
(0.98-1.03), larson-sized 8 0.994 (0.98-1.03), xmalloc-test 4+4 bimodal as before (one
fast run each), sh8bench on two L3s 0.593 against 0.599 s and on one 0.325 against 0.333 s.

Also from the review: the heartbeat's check of a shared stack's target ran before its own
sweep and drain, so what they parked there stayed over the target until the low-water rule
shed it (fifteen drained CPUs left 62 objects against a target of 2 for an interval). The
check now runs last, for every class the call pushed, laundering at most as many magazines as
it pushed.

Two things the first version got wrong. Every heartbeat read the served class's shared head and
top link to check the target; in mpmc p4 c4 every domain's overflow writes those lines, and the
read cost 6% (4.59 to 4.89 ns, five interleaved rounds). The shared stack grows only by pushes,
so the check now runs only after the slow path pushed there: 4.53. And a pop that tries its
domain's stack and then the shared one now notes both depths; the empty domain stack's note
starts its interval at zero, so a stack that was empty during an interval sheds one interval
later than before (two `maintain` calls became three in one test).

## Method

As in the section above: the runner, one round per invocation under the measurement lock, the
three builds rotating within each round, five rounds; preloaded C programs on CPUs 0-7 (one
L3) and 0-3 and 8-11 (two), sh8bench 16, larson `3 8 1000 5000 100 4141 8`, xmalloc-test
`-w 4 -t 3 -s 64`, mstress `8 50 25`, each in a cgroup of its own for its peak. The machine's
speed shifted between rounds of this session (most jobs show a fast and a slow cluster, the
same in every build), so the suite table gives each build's per-round ratio to before, paired
within the round, beside the medians.

## Suite jobs

ns per allocation (cycles per op for churn), medians, and the median and range of the
per-round ratio to before; cgroup peak MiB:

| Job | before | half | quarter | half / before | quarter / before | peak before | half | quarter |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| pipeline t2 local all-alloc | 3.75 | 3.72 | 3.88 | 0.995 (0.96-1.05) | 1.003 (1.00-1.08) | 2.5 | 2.2 | 2.5 |
| pipeline t2 local all-alloc, preload | 4.01 | 4.06 | 4.09 | 1.018 (1.00-1.04) | 1.027 (0.98-1.03) | 2.2 | 2.3 | 2.2 |
| pipeline t2 split all-alloc | 12.44 | 12.58 | 12.26 | 1.014 (0.99-1.03) | 0.987 (0.98-1.00) | 3.0 | 3.2 | 2.7 |
| pipeline t2 split free-only | 12.19 | 12.51 | 12.12 | 0.999 (0.99-1.03) | 0.986 (0.97-0.99) | 2.7 | 3.5 | 3.0 |
| pipeline t2 x2 local | 4.07 | 4.01 | 4.02 | 1.002 (0.99-1.01) | 1.002 (0.99-1.01) | 3.7 | 3.7 | 3.7 |
| pipeline t2 x2 split | 4.73 | 4.86 | 4.85 | 1.010 (0.97-1.20) | 1.019 (0.99-1.07) | 3.7 | 3.5 | 3.7 |
| pipeline t2 x3 both | 5.58 | 5.59 | 5.73 | 1.002 (0.99-1.06) | 1.033 (1.03-1.07) | 5.1 | 4.9 | 4.9 |
| pipeline t2 x4 both | 6.34 | 5.53 | 5.55 | 0.989 (0.85-1.01) | 1.002 (0.84-1.03) | 6.3 | 6.5 | 6.3 |
| exchange t2 local | 4.87 | 4.92 | 4.85 | 1.015 (1.00-1.02) | 0.993 (0.99-1.00) | 2.5 | 2.5 | 2.2 |
| exchange t2 split | 13.54 | 14.33 | 14.10 | 1.046 (1.03-1.18) | 1.023 (1.02-1.18) | 3.2 | 3.5 | 3.0 |
| exchange t8 both | 2.89 | 3.11 | 2.84 | 1.007 (0.98-1.17) | 0.977 (0.82-1.20) | 7.4 | 7.5 | 6.9 |
| mpmc p1 c3 local all-alloc | 6.01 | 5.23 | 5.25 | 0.983 (0.77-1.01) | 0.972 (0.79-1.00) | 3.3 | 3.5 | 3.3 |
| mpmc p3 c1 local all-alloc | 5.51 | 5.46 | 5.47 | 0.989 (0.98-0.99) | 0.990 (0.97-1.00) | 3.7 | 3.7 | 3.7 |
| mpmc p4 c4 both | 4.76 | 4.87 | 4.60 | 1.022 (1.01-1.04) | 1.006 (0.91-1.03) | 6.2 | 6.7 | 6.2 |
| churn live=100k | 47.39 | 47.38 | 47.52 | 0.999 (1.00-1.00) | 1.002 (1.00-1.01) | 12.2 | 12.2 | 12.2 |
| churn live=100k, preload | 52.56 | 52.57 | 52.36 | 1.002 (0.99-1.01) | 0.997 (0.99-1.00) | 12.2 | 12.2 | 12.2 |

The paired ratios put every job within 2% of before but exchange t2 split, slower in all three
sessions this change was measured in (+2 to +5% at half, +2% at a quarter), x3 at a quarter
(+3%), and the preloaded local pipeline at a quarter (+2.7%, but 0.98 to 1.03 and faster than
before in the two earlier sessions). x2 split stayed where it was (4.73 before, 4.85 at a quarter); the 4.8 of the
section above was a fast session. A quarter was faster than half on every job that crosses
L3s but x3, and kept the peaks of jobs across both L3s at before's level, where half parked up
to 0.7 MiB more (mpmc p4 c4 6.16 MiB before, 6.70 half, 6.18 a quarter). The default is a
quarter.

## C programs

Medians of five, and cgroup peak MiB:

| Program | Placement | before | half | quarter | peak before | half | quarter |
|---|---|---:|---:|---:|---:|---:|---:|
| sh8bench, s | one L3 | 0.339 | 0.331 | 0.334 | 116.9 | 118.3 | 117.9 |
| | two L3s | 0.606 | 0.595 | 0.594 | 121.4 | 124.6 | 122.4 |
| larson, Mops/s | one L3 | 581 | 591 | 589 | 100.5 | 101.3 | 101.7 |
| | two L3s | 418 | 431 | 421 | 80.7 | 82.7 | 81.1 |
| xmalloc-test, M frees/s | one L3 | 1039 | 1076 | 1081 | 7.2 | 15.2 | 34.4 |
| mstress, s | one L3 | 0.266 | 0.255 | 0.267 | 142.9 | 142.5 | 142.8 |
| | two L3s | 0.270 | 0.263 | 0.269 | 143.4 | 141.5 | 141.0 |

xmalloc-test on two L3s is bimodal as before, and so are its peaks (5 to 37 MiB, in every
build, and on one L3 too, which is why that row's peaks differ); one run of five per build was
fast (741, 753, 755) and the slow runs were 401 to 519 before, 403 to 422 at half and 380 to
435 at a quarter. xmalloc-test on one L3 gained 4% in both sessions; larson on two L3s 1% at a
quarter and 3% at half; nothing else moved beyond its spread. The memory worst cases
(`bench/probes/memory/worst.sh`, three rounds, medians of cgroup peaks, before and a quarter)
did not move: phase 51.8 and 51.1 MiB, many CPUs and few classes 7.2 and 7.4, many threads on
few CPUs 15.0 and 15.5, the rest identical.

## More domains than L3s

Under the hidden `internals` feature a benchmark build can split the domains by CPU number
(bench-alloc's `asmalloc-domains`, `ASMALLOC_DOMAIN_WIDTH`), so cases with three or more
domains run on real threads. Width 2 makes eight domains of the suite's CPUs, one per pair (0-1,
2-3, 8-9, 10-11): each two-thread pipeline has a domain of its own. Width 1 makes every CPU a
domain: every pipeline is a flow between two domains. Three rounds, quarter cap, medians, ns
per allocation and cgroup peak MiB:

| Job | L3s (2 domains) | width 2 | width 1 | peak L3s | width 2 | width 1 |
|---|---:|---:|---:|---:|---:|---:|
| pipeline t2 split all-alloc | 11.94 | 11.99 | 12.04 | 3.0 | 3.0 | 2.7 |
| pipeline t2 x2 local | 4.14 | 3.95 | 4.15 | 3.7 | 3.7 | 4.9 |
| pipeline t2 x2 split | 4.94 | 4.77 | 11.36 | 3.7 | 3.7 | 4.7 |
| pipeline t2 x3 both | 5.36 | 4.90 | 9.64 | 5.2 | 4.9 | 6.6 |
| pipeline t2 x4 both | 5.72 | 4.87 | 10.42 | 6.2 | 6.3 | 8.3 |
| exchange t8 both | 2.87 | 2.87 | 3.31 | 6.9 | 7.9 | 9.5 |
| mpmc p4 c4 both | 4.64 | 4.72 | 4.61 | 6.1 | 7.0 | 8.2 |

Balanced domains keep to themselves however many there are: with four or eight of them the
independent pipelines run faster than with two (x4 4.87 against 5.72). Flows between domains
share one stack per class, though, and so trade magazines with each other: at width 1 two
pipelines in different L3s (x2 split) took each other's magazines and ran at 11.4 ns, what one
depot for the machine cost before domains. On real hardware a domain is an L3, so a flow between
two domains crosses an L3 whichever flow's magazine it takes; the cost here comes from domains
finer than the L3s, which only this override makes. Where it would show on hardware is a
machine with more than one memory node, where a flow on one node could take a magazine from a
flow on another; a shared stack per node would keep them apart.

## One spare magazine per stack

With the depot as above (quarter cap), `MAX_SPARES` at 1 against 4, five interleaved rounds: mpmc
p4 c4 peaked at 5.61 MiB against 6.07 (4.63 against 4.67 ns), and sh8bench took 0.332 against
0.329 s on one L3 and 0.614 against 0.593 s on two (+3.5%; 0.603 to 0.635 against 0.575 to
0.613). One spare still costs sh8bench across L3s.

# sh8bench, accounted (2026-10-19)

Why sh8bench (preloaded, 16 threads) takes 1.6 times mimalloc's time on one L3 and 2.2 to 2.5
times on two, at a945951. No allocator code changed; the options at the end were built only to
be measured.

## The program

The main thread starts 16 workers, then mallocs 10M 8-byte blocks, writes a link into each, and
publishes them in an 80 MB array. Each worker frees its sixteenth of that array as the main
thread fills it, then runs 43,750 iterations over an array of 62,500 pointers. Each iteration
mallocs a run of one size, cycling through 1000 x 8, 5000 x 16, 1000 x 48, 100 each of 72, 148
and 200, 10 x 520, 5 x 1056, 3 x 4096, and one each of 9162, 34562 and 168524 bytes (two large
blocks per 7321), writing the first and last byte of each. When its array wraps, the worker
frees four fifths of it in one burst (the bottom part in allocation order, the top in reverse)
and keeps the next fifth for one more pass. So every worker allocates and frees its own blocks,
in bursts of 50,000 (about 1.4 MB, 200 magazines, two thirds of them 16-byte); a block lives
one or two passes; only the main thread's 10M blocks cross threads. 437M mallocs and as many
frees in all; 17 threads on 8 CPUs.

## Method

Preloaded, `taskset` to CPUs 0-7 (one L3) or 0-3 and 8-11 (two), under the measurement lock,
libraries and placements interleaved, five rounds, medians. Cycles are user plus kernel,
summed over CPUs, per call (874M `malloc` and `free` calls). Fills from `perf stat`
(`ls_dmnd_fills_from_sys.*`). The program's own touches are separated by a copy of the
program built the same way without `p[0] = 0; p[size-1] = 0` (`notouch`). Where cycles go
inside the allocator comes from `cycles:u` profiles of the `notouch` program, grouped by symbol
and scaled to the counted total; sampled fill events say which code waits for lines from where.
Slow-path traffic was counted with temporary per-CPU counters in a build not kept. Interventions
are builds that change one constant, to see what a cause is worth.

## One L3

14.5 cycles a call against mimalloc's 9.2: 5.3 more.

| Cause | Cycles a call |
|---|---:|
| Depot stack pushes and pops: a contended compare-and-swap on the domain or shared head, after reading the top magazine's link, both lines last written by another core | +2.2 |
| The rest of each exchange: spares, records, heartbeat (most of it waiting for stores still in flight at its first locked or mis-forwarded access) | +1.6 |
| Launder and fill, 51M objects (12% of allocations), less mimalloc's page work | +1.2 |
| Program touching handed-out objects that are not in this core's L2 (140M more L3 fills; 0.5 to 1.2 G cycles between sessions) | +0.6 |
| Fast paths and PLT | -0.6 |
| Kernel: 31K faults and 2,200 `mprotect`s against 390 faults (both about 0.17 a call) | 0 |
| Measured gap | 5.3 |

Every 250 calls is a slow path (3.5M), and each costs about 1,600 cycles: 560 in stack
operations (35% of compare-and-swaps fail), 400 of glue, 570 laundering and filling. Of the
3.5M exchanges, 74% pop the domain's stack and 9% the shared one; the per-CPU spares serve 5%,
because a burst is 50 times what four spares hold. Of the 51M objects through the bitmaps,
13.6M are the refresh rule's (one retiring magazine in 32) and 37M overflow the shared stack's
target. The fault count is not a cost: kernel time is 1% for both allocators. Removing the
`mprotect` sleeps (a build committing 16 MiB at a time: 400 voluntary switches down to 70)
changed nothing.

What each cause is worth when removed, cycles (G) and wall (ms), four to six rounds:

| Build | One L3 | Two L3s (suite runner, s) |
|---|---|---|
| a945951 | 12.8, 341-360 | 0.597 |
| refresh off | 12.3, 328 | |
| depot target 64 MiB (no overflow launders) | 12.3, 338 | |
| every spare kept on its CPU (256 spares, 1 GiB) | 9.1, 262 | 0.423 |
| magazines up to 1024 slots | 9.5, 265-270 | 0.431 |
| up to 4096 slots | 8.6, 247 | |
| mimalloc | 8.2, 213-225 | 0.27 |

So almost all of the one-L3 gap is the number of magazine exchanges that reach shared
structures. Laundering is a smaller part than it looks: without it, the same objects come
back through the depot from other cores' caches instead. A `lock xadd` per slow path (the
activity counter) and two struct copies that defeat store forwarding are where the profile
shows the waits, but making the counter a load and a store moved the wait to the next locked
instruction and changed nothing measurable (six rounds); the waits are the misses of the
exchange, not the instructions that expose them.

## Two L3s

22.7 cycles a call against 10.0: 12.7 more. It adds to the one-L3 causes (4.6, the notouch
part above) two of its own:

| Cause | Cycles a call |
|---|---:|
| One-L3 causes, allocator part | +4.6 |
| Objects last written in the other L3: 30M more fills from its cache and 21M more from DRAM | +6.3 |
| Allocator lines crossing L3s | +1.8 |
| Measured gap | 12.7 |

Objects reach the other L3 two ways: 67% of shared-stack pops (197K of 293K magazines, about
40M objects) take a magazine the other domain pushed, because a 50,000-object burst overflows a
domain's cap (eight 16-byte magazines) at once; and 76M objects (17% of allocations, against 12%
on one L3) are filled from bitmap words the other domain laundered, since fills take the
lowest address whoever freed it. Threads rarely move between L3s (16 changes in a run).
mimalloc keeps each object on its thread's page, so a block freed and reallocated by its own
thread never leaves that core; its two-L3 traffic is the main thread's 10M blocks.

The allocator's own lines that cross, by the code that waits for them (notouch, 6.9M fills
from the other L3): magazine slots read by `malloc` and written by `free` (29%; empty
magazines come from the shared empty stack too), depot heads and links (20%), bitmap leaf words
(17%), summary words (4%), the exchange's glue (12%), the program (7%).

Wall time hides part of this. In every asmalloc run one L3's four CPUs are busy throughout and
the other's partly idle (12.3 against 2.6 to 12 G cycles); runs where the kernel packs nearly
all threads into one L3 take the same 600 ms as spread ones, because spreading costs as many
cycles as the second L3 adds. mimalloc's runs spread evenly (250 ms) or not (327 ms). The kernel
here has `CONFIG_SCHED_CACHE`, which prefers one LLC per process; asmalloc's voluntary sleeps
(400 against 5) give it more wakeups to place, but removing most of them did not change the
placement or the time.

## Options

Neither is built; each was measured as a constant change.

**Larger magazines for small classes.** Today a magazine holds at most 254 objects. With the
byte budget alone deciding (1024 or 4096 slots for classes up to 64 or 16 bytes), exchanges fall
4 to 16 times and compare-and-swap failures 20 times. sh8bench: one L3 360 to 270 ms (1024) or
247 (4096, mimalloc 225); two L3s 0.597 to 0.431 s; peak unchanged (118 MiB). Elsewhere, 1024
slots, three rounds: tree depth 18 -9%, pipelines -9 to -15%, JSON 8 MiB 0 to -6%, larson-sized
+4%, the rest level. The cost is memory on small cross-thread heaps, from bigger magazines in
flight: pipeline t2 2.2 to 3.0 MiB (3.2 at 4096; jemalloc 1.9), x3 4.9 to 6.7 (8.2; jemalloc
4.7), mpmc p3 c1 3.5 to 4.9 (6.0), exchange split 2.7 to 3.5 (4.2). That breaks the jemalloc bar
on those jobs. The fast path is unchanged (the routines are proved to 2^16 slots); the back
end's launder batches, the drain's bounded work and the spare budget would need review.

**Keep bursts on their CPU.** Letting every spare stay on its CPU gives the same sh8bench speed
(262 ms, 0.423 s) at 142 MiB peak (jemalloc 162), but unbounded it ruins one-way flows: a
consumer CPU keeps everything it frees (pipeline t2 2 to 87 MiB and twice as slow, x2 split 176
MiB). A version that grows a class's spare limit only while the CPU both pops and pushes that
class's magazines with the depot in an interval, within the per-CPU byte budget given to the
classes in use rather than split evenly over 44, would keep one-way flows as they are. Not
measured; its peak on sh8bench would sit between 118 and 142 MiB.

Neither touches the two-L3 object crossing directly. That needs fills to prefer slots a
domain laundered itself (per-domain regions for a class), and an overflow that reaches the
other domain only when that domain is short; both are design changes to the depot and back
end.

# Stock: a CPU keeps what it sends away and takes back, when the depot is contended (2026-10-19)

The second option above, built in its bounded form (`depot/stock.rs`). "main" is 6b7d72d,
"stock" the branch, "room x4" the branch with four times the room, measured once to show what
the cap costs.

## The rule

- **Contended round trips.** When a slow path sends a magazine of a class to the depot because
  its CPU's spare stack of that kind is full, it marks the class's stock word: sent, and
  contended if a compare-and-swap of that push failed. When the same CPU later takes a magazine
  of that kind and class back from the depot, the round trip is complete and the marks are
  cleared; if the push or the pop found its stack contended, the class earns one credit for
  that kind of spare. Contention is what makes an exchange expensive (sh8bench, above: 1,600
  cycles, 35% of compare-and-swaps failing); on a stack nobody else is using an exchange is
  cheap, and stock would only park memory. Stock relieves the contention, the failures stop,
  and the credit stops growing. A CPU whose traffic in a class goes one way never takes back a
  kind it sent, and earns nothing. Empty magazines earn credit the same way, and a class keeps
  no more empty stock than full.
- **Depth.** A spare stack holds its fixed limit plus its credit, while the CPU's stock (spares
  past their fixed limits, counted in the objects a full magazine holds) stays within the spare
  budget less what the fixed limits could park: 1 MiB less 139 KiB, 885 KiB a kind. The fixed
  limits split the 1 MiB evenly over 44 classes, and most classes' magazines are bigger than
  their share, so they reach 139 KiB of it. When the room is spent, magazines go to the depot as
  before.
- **Disuse.** While a class has credit, its spare pops keep a low-water mark; the sweep that
  takes each class's full spares once an interval first takes the unused depth off the credit
  and returns empty spares above the new limit. The idle drain takes stock with the slab.
- **State.** A word per class in the unused quarter of its slab chunk, beside the spare heads,
  and a per-CPU count of stock bytes on the activity counter's line: no line a slow path did not
  already touch. Plain loads and stores, hints; all zero is no stock. The stack protocol, the
  drain and the fast paths are unchanged; a push and a pop now report whether a
  compare-and-swap failed.

Two versions came before. Marking the magazine itself (a tag in its record, credit when the CPU
popped that very magazine back) earned too little: other CPUs take a CPU's magazines first, and
1.2% of depot pops returned the popper's own, 0.06% under contention. Without the contention
condition, stock went to every round-tripping CPU: sh8bench 297 ms on one L3, but the RPC
servers held 2 to 4 MiB more for no speed, and `exchange t2 split` 1.2 MiB more.

## sh8bench

Five interleaved rounds, wall ms and user G-cycles:

| | main | stock | room x4 |
|---|---:|---:|---:|
| one L3 | 347, 12.9 | 291, 10.1 | 280, 9.8 |
| two L3s | 599, 20.8 | 524, 18.9 | 502, 18.3 |
| suite runner, two L3s, s, and cgroup peak MiB | 0.577, 121 | 0.523, 132 | 0.509, 129 |

## Neighbours

Suite runner, three to six rounds; C programs on one L3 by hand, three rounds:

| Job | main | stock | room x4 | peak main / stock / x4, MiB |
|---|---:|---:|---:|---|
| larson 8, one L3, M ops/s | 567 to 588 | 598 to 616 | 606 to 613 | |
| larson 8, two L3s | 438 | 445 to 462 | 425 | 72 / 75 / 73 |
| larson-sized 8 | 429 to 431 | 445 to 446 | 442 to 452 | 72 / 74 / 74 |
| xmalloc-test, one L3, M frees/s | 739 to 753 | 1137 to 1166 | 1162 to 1181 | |
| mstress, one L3, ms | 269 to 275 | 256 to 270 | 256 to 268 | |
| mstress, two L3s, s | 0.258 | 0.273 to 0.283 | 0.262 | 139 to 140 / 140 / 143 |
| mpmc p4 c4, ns | 5.15 | 5.09 | 5.08 | 5.9 / 5.9 / 5.9 |
| exchange t2 split, ns | 15.25 to 15.48 | 15.36 to 15.51 | 15.30 to 15.68 | 2.8 to 2.9 / 2.8 to 2.9 / 2.7 to 2.9 |
| rpc echo c64 | 46659 | 46472 | 46212 | 5.7 / 5.7 / 5.7 |
| rpc log c64 | 124163 to 124687 | 123257 to 124359 | 125154 to 125706 | 7.5 to 8.6 / 8.4 to 8.5 / 7.5 to 8.3 |
| rpc log c256 | 120480 | 120430 | 121055 | runs 10.9 to 14.2 / 10.4 to 14.6 (medians 12.8 / 12.6) |

The pipelines, mpmc p3 c1, forkjoin, spawn, churn, BTreeMap, vecgrow, JSON, tree depth 20 and
the smoke set's other jobs are level within their spread; pipeline x3 and mpmc p3 c1 peaks 4.9
and 3.5 against 4.6 and 3.7 MiB. Memory probes (`worst.sh`, two runs) are level: burst-idle
218, phase 46 to 54 for both, cpus-few-classes 6.0 to 8.1, threads-few-cpus 15.1 to 15.6,
sparse 3.5 and 33.2, tiny 0.5 and 0.5.

The tiny probe's 0.5 against 0.8 MiB in the first version was noise: its anonymous resident
memory is the same 244 KiB in both builds, and its cgroup peak runs 0.5 to 0.8 MiB for either,
by which run reads the binary's pages first. A process that never sends a magazine to the
depot never writes a stock word.

mstress on two L3s looked slower with stock in two sessions of hand-started runner rounds
(0.258 against 0.273 and 0.283 s) at cycles within 2%. It is not real: under the runner with
its clock warmed, ten interleaved rounds of main (c379026) against the same build with stock
off give 0.261 against 0.259 s (0.257 to 0.267 and 0.252 to 0.267), with the same clock (5.52
GHz), CPU time (1.07 s), voluntary and involuntary context switches (389 against 391, 262
against 300) and minor faults (36.0k against 37.3k).

On current main (c379026, which adds small classes starting on pages of their own and an exact
read of the retained count), sh8bench is 0.505 s against 0.504 for the stock commit before those
changes on two L3s (runner, eight rounds; 0.580 with stock off), and 291 against 284 ms on one L3
(six rounds by hand; 2% more cycles at the same instructions).

## What is left

On one L3, 291 against mimalloc's 225 ms: the room, the refresh rule's launders and the
exchanges the stock does not cover. On two L3s, 524 against 250 to 330: objects that still
cross through the shared stack and the bitmaps when a burst outruns the room. Four times the
room buys another 11 ms on one L3 and 22 ms on two, at the same peak within the spread (129
against 132 MiB), and larson, mstress and RPC no differently.

## After the review

An adversarial review found five faults. The fixes:

- **Disuse returns only what was not needed, a magazine a heartbeat.** The sweep took a class's
  whole full-spare stack, stock and all, and the same heartbeat's launder over target sent most
  of it to the bitmaps: up to about 370 magazines of the 8-byte class inside one `malloc` or
  `free`, once an interval per stocked class, used or not. Now the sweep's visit to a class
  with stock takes the unused depth off the credit once, then returns one magazine above the new
  limit a heartbeat (a full one to the source, then empty ones to the depot), staying on the
  class until none is left. Stock in use is not taken; nor are the fixed spares of a class
  whose stock is in use, since the pops that use the stock reach them, or the low-water mark
  shrinks the stock until they do. What replaces objects in circulation through stock is the
  refresh rule, one retiring full magazine in 32 laundered before it reaches the spares, the
  same rate as for everything else.
- **`collect` empties the caller's stacks whatever their depth**, bounded by the depth each had
  when its emptying began, in place of 64 pops a stack.
- **No count to drift.** The per-CPU byte count could be misplaced by a thread moved between a
  stack routine and the count's update, or by a lost update, and an excess never healed on a
  busy CPU. The room check now reads what the stacks hold: over a per-CPU set of the classes
  that may keep stock, how far each stack's head lies past its fixed limit, from its record. The
  set is a hint too, but one that heals: the check that admits a class's first magazine past its
  limit puts the class in, and the class's sweep puts it back if a race took it out. This also
  removes the drain's store that could wipe stock pushed meanwhile.
- **One CPU per decision.** The sweep's stock step acts only when the thread is still on the CPU
  whose slab it sweeps (otherwise that CPU's own next heartbeat carries on), and each decision
  reads the CPU once and acts on that CPU's words and stacks.
- Credit survives a drain until the CPU's next sweep, which finds it unused and takes it: credit
  is a permission and costs nothing until stock is pushed under it.

Also: the fixed limits come from a table worked out at compile time (a full spare pop had paid a
division); an interleaving scenario goes from real contention to credit to stock and back to
nothing after an interval of disuse; a test holds that a round trip credits only its own kind
and no more than the room.

A first version of the room check scanned all 44 classes' words on every push past a limit:
sh8bench's instructions rose 8.6% and its time 4.6% on one L3 (282 to 295 ms). The set of stocked
classes brought it back.

Against main (eafb661), five runner rounds on two L3s and six by hand on one: sh8bench 285.5 and
285 ms on one L3 (1% more cycles, 3.4% more instructions), 0.509 and 0.506 s on two (peaks 125
and 127 MiB); xmalloc-test, exchange split, rpc echo and both rpc log jobs level in time and peak;
larson 2 to 4% lower in both placements, within the ranges (one L3 618 to 641 against 616 to
625 M ops/s); the phase probe 46.7 and 45.4 against 47.7 and 52.8 MiB, tiny 0.5 MiB.

## The tree and JSON jobs between `a92d51a` and `e96086d`

2026-10-19. Between the full sessions at `a92d51a` and `e96086d`, `tree depth=20` went from
43.25 to 44.56 cycles a node (+3.0%), `json 32Mi` from 16.19 to 16.63 cycles a byte (+2.7%),
and most small-object jobs retired 0.3 to 1.2% more instructions. Neither job got slower
because of the allocator's added work; each moved because of where code landed.

**Method.** Ten commits on main's first-parent line, each built from `git archive` with the
same `bench/` sources (`e96086d`'s), as `build = ...` entries in `bench/allocators.toml`, run
interleaved with `benchrun run` under `scripts/measure`, three rounds, medians. Instructions
per phase are deterministic to 0.01%. Where they went: call counts from gdb breakpoints, and
gdb single-stepping of runs of consecutive slow paths with the cycle counter frozen (the slow
paths act on the clock, and stepping is thousands of times slower than running), each inlined
routine stepped over as one step. Function placement from `nm`; `align64` builds add
`-C llvm-args=-align-all-functions=6`.

**Instructions: stock and the frontier, as designed.** Per phase, in millions over the job's
ten repetitions (two million nodes each):

| Commit | build | walk | drop | whole process |
|---|---:|---:|---:|---:|
| `a92d51a` | 1301.0 | 440.9 | 1223.2 | 2969.7 |
| `6d95572` (origins, exact retained read) | 1304.0 | 440.9 | 1221.0 | 2970.6 |
| `4a77350` (stock) | 1309.3 | 440.9 | 1236.3 | 2991.1 |
| `74df709` (frontier) | 1322.9 | 440.9 | 1236.6 | 3004.9 |
| `e3e93ab` (hot words moved) | 1320.4 | 440.9 | 1237.3 | 3003.2 |

Stock adds about 60 to 80 instructions to each magazine exchange, about 1.0 a node: the stock
word's checks (`note_return`, `note_sent`, `stock_admits`), the contention flags, and the depot
push no longer inlined into `to_shared` (it now returns whether it was contended, and
`push_under` stays out of line; marking it `#[inline]` saves nothing). The frontier adds about
100 instructions and one compare-and-swap to each fill, about 0.65 a node: a fill always
reaches higher than the last in a job that only grows, so `region::reach` writes the region's
word every time. JSON's 33M are the same two (drop +18M at stock; parse +6M at stock, +11M at
the frontier). The class origins, the exact read, the lookup tables and the hot-word move add
none. Churn that never leaves the magazines runs no slow path, which is why it is level.

**Cycles: the tree's walk.** The extra instructions cost the build and drop phases about 0.4
cycles a node between the two sessions (+3.4M and +4.4M cycles). The rest, +27M of +27M in
`run`, is the walk, which runs no allocator code and retires the same instructions in every
build. The walk is a recursive function of 72 bytes; where the allocator's code size puts it
decides its time:

| Build | walk's offset in its 64-byte line | walk, M cycles |
|---|---:|---:|
| `6b7d72d`, `a76b222`, `4a77350`, `74df709` | 0x20 | 194 to 208 |
| `aeb7451` | 0x30 | 205 to 220 |
| `e3e93ab` | 0x10 | 202 to 216 |
| `a92d51a`, `6d95572`, `94b5b44`, `c3c4676` | 0x00 | 220 to 226 |
| the session's own `e96086d` build | 0x00 | 225 |

At offset 0 its last four instructions (three pops and the return) sit alone in a second line,
which costs a cycle on each of its two million returns (`zen5-roofline.md`, section 15). The
same builds of `a92d51a` and `e3e93ab` with every function 64-aligned put the walk at 0x00 in
both, and run 44.21 and 44.25 cycles a node: the regression is gone, and the 33M instructions
cost 0.1%.

**Cycles: JSON's parse.** Parse cycles rise 3% at `c3c4676` with level instructions. That
commit put `BY_SIZE` and `MASKS` 2048 bytes into page-aligned statics (`zen5-roofline.md`,
section 16); every inlined runtime-size allocation and free now loads them with a 32-bit
displacement, and LLVM inlines `__rust_dealloc` at more of serde_json's call sites:
`Value::deserialize` grows from 0x1001 to 0x1189 bytes. The same commit with the padding taken
out has the old code and the old time (five rounds: 1,993 against 2,029 Mcycles parse; padded
but not page-aligned, 2,050; page-aligned but not padded, 1,947). Function alignment does not
remove it (2,067 against 1,932). Reaching the table through a static reference or a constant
pointer gives the same code as the padded field. The padding is what keeps the inlined lookups
off the hot words' page offsets, which was worth up to 34 cycles an object in `fragchurn`, so it
stays; JSON compared across builds of the allocator carries this as it carries serde's own
layout (`baselines.md`, section 14).

**The split pipelines are another cause.** `pipeline t2 sizes=small split`, all-alloc and
free-only, CPUs 0 and 8: 11.45 and 11.42 ns before the class origins (`6b7d72d`), 12.00 and
12.08 after (`a76b222`), level from there to `e3e93ab` (six rounds), at the same instructions.
Stock, the frontier and the GOT slot are not it: the session's own binary, whose GOT slot for
`__rseq_offset` lies at 0x218 (the 256-byte class's hot word), ran 11.98 against 12.14 for a
build of the same commit with the slot at 0x2b0. With origins the job takes 50 times fewer 4 KiB
page walks (3,600 against 130k to 200k a run) and no fewer cycles; why spreading the classes'
first pages costs this job across L3s is not found.

**Nothing changed.** The instructions are stock's and the frontier's bookkeeping, at the
measured price of each; the cycles are code placement in the workload (tree) and an inlining
decision LLVM makes in serde_json (JSON). Building the workloads with every function 64-aligned
would stop the allocator's code size from moving workload functions, so sessions and
allocators compare on the same workload layout; it changes every job's numbers, so it is a
decision for the suite, not made here.

# Objects kept in the cache domain that last used them (2026-10-19)

The back end does not know about cache domains: a fill takes the class's lowest free slots,
whoever freed them. "sh8bench, accounted" found that this, with magazines popped from the
shared depot stack that the other domain pushed, is most of what two L3s add to sh8bench. This
section measures what keeping objects in their domain is worth and what it costs, with a
throwaway prototype on branch `exp-domain-regions`. No allocator code on main changed.

## Summary

Judged by the Rust workloads (the suite's jobs that span both L3s, eight to twelve
interleaved rounds): the gain is not confined to sh8bench, but it is confined to one shape.
Independent work spread over the L3s gains: three two-thread pipelines 13%, four 9%, two 2.6%.
Flows between the L3s gain 2 to 4% (split pipelines, exchange). mpmc 4+4, exchange t8,
pipeline t8, forkjoin and spawn are level. Peaks rise 0.3 to 1.6 MiB on those jobs (exchange
t2 split 3.5 to 4.7 MiB, exchange t8 9.2 to 10.8, pipeline t8 10.6 to 12.2), which matters
against the jemalloc bar on small heaps. On one L3 nothing moves. The C-program figures below
(sh8bench 24%) are larger but are not the measure to steer by.

- **Domain-affine fills alone buy little.** A carving region per class and domain, with fills
  that prefer their own domain's free slots and carve only when the class has none anywhere,
  takes sh8bench on two L3s from 0.390 to 0.373 s (cycles a call 15.6 to 14.8) and leaves the
  objects crossing between L3s nearly where they were. A domain without free slots of its own
  reuses the other's, and the shared stack still hands it the other domain's magazines.
- **A pair of rules buys a quarter.** Carving in its own domain before reusing another
  domain's free slots, and refilling from its own domain's free slots before popping a
  magazine another domain pushed (unless that magazine's objects live in its domain's
  memory), takes sh8bench to 0.297 s and 10.8 cycles a call (mimalloc 0.256 s, 9.8). Fills
  from the other L3's cache fall from 0.022 a call to 0.003, from DRAM from 0.029 to 0.008.
  Three independent pipelines over two L3s run 13% faster, four 9%; flows between the L3s
  2 to 4%. On one L3 nothing moves.
- **The price is memory.** Peaks rise 4 to 9% on the C programs that use both L3s, and by 1
  to 1.6 MiB on the suite's jobs across them. Each domain in
  use pays for a region per class, so the small-heap floor grows with domains: mpmc 4+4
  peaks at 8.7, 10.8 and 13.4 MiB with two, four and eight domains, against 8.3, 8.9 and
  10.5. And load that moves from one domain to another holds both domains' memory: 402 MiB
  against 247 (jemalloc 249) when the first domain's holes are pinned by fragmentation.
  Carving first only while retained memory is within the cap bounds that by the cap: 308 MiB,
  at the same speed elsewhere.

## Method

Builds, each from `git archive` of its commit, with the same workload sources:

- **base**: main (`515412e`) with magazines of up to 1,024 slots (`MAX_LOG2` 10), as the branch
  in progress will have them.
- **dom**: base plus the direction proposed. Each small class has a carving region per
  domain (a carving word per domain, and one bit per region in a mask per domain, set when the
  region is assigned). A fill searches the class's summary tree for its own domain's regions
  first (the class-level word masked by its domain's bits), then any region, and carves in
  its own domain's region only when nothing is free anywhere.
- **domcph** ("strict"): a fill takes its own domain's free slots, then carves in its own
  region, and takes other domains' free slots only if carving fails. And in the depot, a slow
  path whose domain stack is empty fills from its domain's own free slots before it pops the
  shared stack, unless the magazine on top of the shared stack was pushed by its own domain or
  its top object lies in a region of its domain; this applies only with more than one domain.
- **domrh** ("cap"): as strict, but carving comes before other domains' free slots only while
  retained memory is within the cap (64 MiB); past it a fill takes them first, as dom does.
- Intermediate builds are named where used: **domp** is dom's fills with the depot rule
  judged by the pusher alone.

Preloaded C programs under `scripts/measure --both-l3`, the builds and placements in a new
random order each round, each run in a capped cgroup of its own. Placements: CPUs 0-3 and
8-11 (two L3s), 0-7 (one). Counters are system-wide on the run's CPUs, per CPU
(`perf stat -a -C -A`): cycles, instructions, `ls_dmnd_fills_from_sys.near_cache` (fills from
the other L3's cache) and `.dram_io_near`; per call for sh8bench is over its 874M `malloc` and
`free` calls. Wall time on two L3s depends on how the scheduler spreads the threads over the
L3s, so the share of cycles each L3 ran was recorded and cycles a call is the steadier
figure. Software counts come from builds with `x-count`, which count where fills take
objects and which shared-stack pops took another domain's magazine (by a tag the pusher
writes), written at exit. Rust jobs with `benchrun run`, rounds interleaved, medians and the
per-round ratio to base. Memory probes with `bench/probes/memory/worst.sh` and `acct.sh`;
more domains than L3s with `ASMALLOC_DOMAIN_WIDTH` (width 2 makes four domains of CPUs 0-3
and 8-11, width 1 eight). The moved-load case is a new workload, `workload shift` (on the
experiment branch): threads on CPUs 0-3 each allocate 500k objects of the small mix (218 MiB
in all), free 90% of them (or all) in random order and wait; then threads on 8-11 each
allocate and free 500k objects of the same mix, over and over, for 8 s.

## sh8bench

Medians, 12 runs (base, strict, mimalloc), 18 (dom), 6 (the rest):

| | base | dom | domp | strict | cap | mimalloc |
|---|---:|---:|---:|---:|---:|---:|
| two L3s, s | 0.390 | 0.373 | 0.324 | 0.297 | 0.295 | 0.256 |
| cycles a call | 15.6 | 14.8 | 12.3 | 10.8 | 10.7 | 9.8 |
| fills from the other L3's cache a call | 0.022 | 0.019 | 0.008 | 0.003 | 0.003 | 0.006 |
| fills from DRAM a call | 0.029 | 0.025 | 0.014 | 0.008 | 0.007 | 0.004 |
| peak MiB | 137 | 137 | 139 | 147 | 143 | 221 |
| one L3, s | 0.230 | 0.236 | 0.237 | 0.232 | 0.238 | 0.200 |
| cycles a call | 9.94 | 10.02 | 10.10 | 9.95 | 10.19 | 9.12 |

Instructions are 40.6 to 40.9 a call in every build. Where the objects come from, millions per
run on two L3s (medians of three, `x-count`):

| | base | dom | strict |
|---|---:|---:|---:|
| objects filled from free slots / carved | 31.9 / 2.2 | 31.9 / 2.2 | 41.9 / 2.8 |
| of which from another domain's regions | (all mixed) | 5.8 | 0 |
| magazines popped from the shared stack | 39.7k | 39.6k | 25.6k |
| of which another domain pushed, magazines (objects) | 24.1k (17.7) | 22.2k (16.5) | 2.9k (2.7) |
| objects laundered | 32.8 | 34.3 | 43.9 |

- In dom, the second domain takes a third of its fills from the first domain's regions: the
  main thread's ten million 8-byte blocks are carved in its domain, and a domain with no free
  slots of its own takes the other's before it carves. Those objects then travel both ways
  through one region. The shared stack goes on handing each domain the other's bursts.
- Strict carving without the depot rule (14.3 cycles a call, one session of six rounds) and the
  depot rule on dom's fills (domp, 12.3) each
  take a part; together they remove nearly all crossing. What is left, 2.7M objects in
  magazines the other domain pushed, is magazines popped after the domain's own free slots
  ran out.
- The depot rule costs launders: a burst that overflows to the shared stack is laundered there
  and filled back from the bitmaps (44M objects against 33M), which on one L3 would cost 3%,
  so it is off with one domain. Judged by the pusher alone it also cost the packed runs of
  xmalloc-test 7% and the split pipelines 3 to 4%, where the magazines are the popper's own
  objects coming back from another domain; the peek at the top magazine's pusher and its top
  object's home fixes both.
- Judging a magazine by its objects' home needs strict carving. On dom's fills a domain's
  objects lie in both domains' regions, the home says nothing, and mpmc 4+4 ran 14% slower.

## Other programs

C programs, medians (four to six rounds; xmalloc-test by placement mode, since its threads
land all in one L3 or spread, for every build):

| Program | Placement | base | dom | strict | cap | peak MiB base / dom / strict / cap |
|---|---|---:|---:|---:|---:|---|
| larson 8, M ops/s | two L3s | 446 | 454 | 451 | 457 | 86 / 87 / 94 / 93 |
| | one L3 | 608 | 605 | 609 | | 102 / 102 / 101 / |
| mstress 8, s | two L3s | 0.256 | 0.253 | 0.249 | 0.250 | 150 / 151 / 156 / 155 |
| | one L3 | 0.250 | 0.251 | 0.251 | 0.250 | 152 / 152 / 152 / 152 |
| xmalloc-test 4+4, M frees/s | two L3s, packed | 935-1004 | 949-1001 | 931-953 | | |
| | two L3s, spread | 563-579 | 561-638 | 582-693 | | |
| | one L3 | 1270 | 1259 | 1266 | 1272 | 46 / 46 / 39 / 45 |

Suite jobs that span both L3s, ns per allocation (per task for spawn), median of 8 to 12
rounds; per-round ratio to base, median and range; cgroup peak MiB:

| Job | base | dom / base | strict / base | cap / base | peak base / dom / strict / cap |
|---|---:|---:|---:|---:|---|
| pipeline t2 split all-alloc | 10.71 | 1.000 (0.99-1.03) | 0.979 (0.96-1.01) | 0.983 (0.95-0.99) | 3.5 / 3.5 / 3.5 / 3.5 |
| pipeline t2 split free-only | 10.82 | 0.995 (0.97-1.02) | 0.985 (0.97-1.00) | 0.973 (0.96-0.98) | 3.5 / 3.5 / 3.5 / 3.6 |
| pipeline t8 both | 4.188 | 1.002 (0.99-1.01) | 0.999 (0.99-1.00) | 0.999 (1.00-1.00) | 10.6 / 10.7 / 12.2 / 12.3 |
| pipeline t2 x2 split | 3.869 | 0.972 (0.92-1.03) | 0.974 (0.93-0.98) | 0.974 (0.93-1.00) | 4.9 / 5.2 / 5.2 / 5.2 |
| pipeline t2 x3 both | 4.355 | 0.971 (0.96-0.99) | 0.870 (0.86-0.89) | 0.875 (0.86-0.89) | 6.8 / 7.0 / 7.5 / 7.5 |
| pipeline t2 x4 both | 4.535 | 0.986 (0.98-1.07) | 0.905 (0.89-0.96) | 0.910 (0.89-0.93) | 8.5 / 8.7 / 9.7 / 9.7 |
| exchange t2 split | 12.99 | 0.999 (0.98-1.01) | 0.961 (0.95-1.02) | 0.956 (0.93-0.97) | 3.5 / 3.9 / 4.7 / 4.8 |
| exchange t8 both | 2.622 | 1.000 (0.99-1.01) | 1.005 (0.99-1.01) | 1.001 (0.99-1.01) | 9.2 / 9.5 / 10.8 / 10.8 |
| mpmc p4 c4 both | 4.306 | 1.003 (0.96-1.03) | 0.992 (0.95-1.01) | 0.986 (0.97-1.02) | 8.5 / 8.4 / 8.6 / 8.7 |
| forkjoin t8 both | 3.665 | 1.012 (0.91-1.03) | 1.011 (0.91-1.07) | 1.016 (1.01-1.02) | 19.6 / 20.0 / 21.1 / 21.1 |
| spawn t8 both | 246.0 | 0.996 (0.99-1.03) | 0.988 (0.96-1.02) | 0.996 (0.97-1.01) | 6.4 / 7.1 / 6.7 / 6.7 |

On one L3 (CPUs 0-3 or one CPU, four rounds), strict is within 1% of base on every local
pipeline, exchange, mpmc, forkjoin, spawn, churn (1k and 100k live), BTreeMap and tree job,
at the same peaks. Cap costs the local pipelines 1 to 2% and sh8bench on one L3 2.5% of
cycles: the prototype reads the retained tallies, two lines other threads write, on every
fill.

## Memory

`worst.sh`, cgroup peak MiB, median of three (base / dom / strict): burst-idle 216.0 / 216.0
/ 216.0, phase 44.1 / 47.8 / 51.1, many CPUs few classes 7.0 / 6.5 / 7.2, many classes few
CPUs 15.9 / 15.9 / 15.9, many threads few CPUs 14.2 / 14.7 / 15.0, sparse 3.5 / 3.5 / 3.7 and
33.1 / 33.2 / 33.2, tiny 0.7. The phase case uses every small class on eight CPUs over two
L3s, so each class has a region per domain: page tables 0.73 MiB against 1.23, and resident
memory 2, 4, 6 and 8 s after the peak 31.0, 17.4, 13.1, 8.6 MiB against 37.5, 26.2, 16.0,
10.2.

The small-heap jobs of "Small heaps" (`memory.md`), cgroup peak MiB, median of three, base /
dom / strict, with the machine's two domains, four and eight:

| Job | two domains | four | eight |
|---|---|---|---|
| mpmc 4+4 | 8.30 / 8.46 / 8.66 | 8.87 / 9.86 / 10.79 | 10.48 / 11.80 / 13.41 |
| spawn t4 | 3.38 / 3.21 / 3.16 | 3.06 / 3.48 / 3.57 | 2.92 / 4.19 / 3.77 |
| pipeline t2 small | 2.98 / 3.04 / 3.19 | 3.13 / 2.93 / 2.88 | 3.61 / 3.68 / 3.68 |
| pipeline t2 mixed | 5.66 / 5.99 / 5.86 | 5.93 / 5.93 / 6.09 | 7.71 / 7.54 / 7.64 |
| churn 1k, tiny | level | level | level |

Cap, in a session of its own, matched strict on mpmc 4+4 (8.60, 10.77, 13.34) and was within
0.5 MiB of it on the rest: small heaps are under the cap. Base also grows with
domains, from the depot's stacks per domain; a domain-affine back end adds a region per class
and domain in use, about 16 KiB of page tables and metadata each, and the objects each domain
parks in its own regions.

## The moved load

`workload shift`, cgroup peak MiB, anonymous resident memory at the end of the second
domain's eight seconds, and the second domain's nanoseconds per object after its first round;
medians of two or three runs:

| Kept by the first domain | | base | dom | strict | cap | jemalloc | mimalloc |
|---|---|---:|---:|---:|---:|---:|---:|
| 10% | peak | 247 | 243 | 402 | 308 | 249 | 279 |
| | at 8 s | 244 | 241 | 399 | 306 | 183 | 254 |
| | ns per object | 27.5 | 31.5 | 26.5 | 39.5 | 31.8 | 49.4 |
| none | peak | 223 | 223 | 334 | 288 | 222 | 262 |
| | at 8 s | 220 | 219 | 253 | 261 | 5 | 228 |
| | ns per object | 26.5 | 27.5 | 26.5 | 35.6 | 31.9 | 36.7 |

- **Strict** carves the second domain's heap beside the first domain's holes. With 10% kept at
  random every page of the first heap keeps a live object, so nothing can be purged and the
  extra 155 MiB stays as long as those objects live. With none kept, the purge takes most of
  it back within two intervals (328 MiB at 1 s, 268 at 2 s, 253 from 3 s).
- **Cap** reuses the first domain's holes until retained memory is down to the cap, then
  carves: the counters show 0.9M objects carved where strict carved 2.0M and base 0.25M. What
  it holds beyond base is about the cap.
- **dom and cap make the second domain slower**, by up to 15% and 45% an object. Its magazines
  then mix objects of two regions of a class (its own and the first domain's), and a launder's
  fast check covers one region at a time: `launder_held` went from 17% to 36% of the
  process's instructions (18% more in all). Strict keeps each domain's objects in its own
  regions and is as fast as base.
- **More domains** (four and eight, by the width override) change none of the peaks, which
  depend on the split between the two groups of threads, but slow dom's second domain further
  (41.9 and 45.8 ns per object), as its threads spread over more regions.

## What it would take

What the prototype leaves out: large classes and the single-slot path of a thread without a
CPU stay domain-blind (domain 0); domains fold into 16 for regions; the pusher's tag is a side
table by magazine number, not the magazine's record; cap reads the retained tallies on every
fill; the launder is unchanged; and no proof, model or ledger entry was touched. A design
would touch:

- **The mapping from class and domain to regions.** A carving word per class and domain
  (`ClassState::carving`), and which domain a region belongs to: a mask per domain over the
  regions, written once at assignment by the thread that took the region from the table
  (before the carving word names it; the first prototype set it before the table's
  compare-and-swap and a loser cleared the winner's bit). Domains need a bound for regions:
  44 small classes times the 64 domains the depot keeps apart is 2,816 of the 3,940 regions,
  and every domain in use raises the floor. Folding to eight would leave larger machines
  sharing regions between domains, as the depot folds beyond 64.
- **How a fill finds another domain's free slots.** No new tree: the class summary words
  masked by the domain's bits pick the region, and the fallback is today's search.
  `tla/Summary.tla` lets a search follow any set bit, so the order changes no protocol. The
  design's rule that a fill carves only when the class has nothing free anywhere becomes:
  nothing free in its own domain, and, within the cap, before another domain's.
- **The launder.** A magazine can hold objects of several regions of its class; the fast
  check must keep two at hand, or a domain that reuses another's memory pays 15 to 45% an
  object.
- **The depot.** A magazine pushed to the shared stack records its pusher's domain (the record
  slot has room beside the level and depth), and a slow path reads the shared head and the top
  magazine's record and top object before choosing. These are reads of lines a pop reads
  anyway; `tla/Depot.tla` is unchanged, since the choice is a hint. The rule applies only with
  more than one domain.
- **The purge walk and frontier words** are per region already; more regions only raise the
  walk's bound. Growth past the cap pays as now, and in a domain that carves beside another's
  free memory it now pays from that memory, which is why the cap bounds the moved load.
- **Origins** stay per class; a class's regions in several domains share an origin, which is
  harmless because each domain's cores touch mostly their own.
- **The huge-page threshold** is per region, and a class that has filled a region before gets
  huge pages at once; with a region per domain, a dense class split over N domains reaches the
  threshold N times later and spends that much longer on small pages.
- **Proofs and models.** Kani's size checks of the back end's state; the assignment explorer
  (`assign_review`) and the back end's model test, which assume one carving region per class;
  the design's text on fills and on the depot. `Summary.tla`, `Purge.tla` and `Depot.tla` need
  no change.
- **The cap check** must be cheap, for example a flag the heartbeat sets once an interval.

## Recommendation

Do not build the direction as first stated: on the Rust jobs across two L3s it is level
(within 3% everywhere), and it makes a moved load slower. With both rules and the cap the
Rust jobs gain 9 to 13% where independent work is spread over the L3s and 2 to 4% on flows
between them, nothing elsewhere and nothing on one L3. The costs: peaks 0.3 to 1.6 MiB higher
on those jobs, a floor that grows with the domains in use, and up to the cap of memory held
when load moves between domains. That is a narrow gain for a design change that touches the
back end, the launder and the depot. Build it only if work spread over many L3s is to be a
target; otherwise leave it. If it is built, the sentence for the design:

> Each small class carves from a region of its own in each cache domain. A fill takes free
> slots from its own domain's regions first; then, while retained memory is within the cap, it
> carves in its own domain's region before it takes another domain's free slots, and past the
> cap it takes them first. A slow path whose domain stack is empty refills from its own
> domain's free slots before it pops a magazine another domain pushed onto the shared stack,
> unless that magazine's objects live in its domain's memory.

Least certain: whether the depot rule's tests (the pusher, the top object's home) hold up
beyond these programs, for instance flows among three or more domains, which were not timed;
and the wall times on two L3s, which depend on how the scheduler spreads threads (the cycles
a call agree with them).

# Magazines of up to 1,024 slots (2026-10-19)

A magazine held at most 254 objects whatever the class, so the classes up to 64 bytes held 2 to
16 KiB of objects in one while the byte budget allowed 64 KiB. "main" is e96086d (code identical
to 36dd48b); "branch" is the change below.

## The rule

A class's magazine is the most slots whose objects fit in 64 KiB, at most 1,024 slots and at
least 4 (`classes::magazine`, `MAX_LOG2`): 1,022 objects for every class up to 64 bytes, 510 for
80 to 128, 254 for 160 to 256 bytes, as before from there. Every class up to 64 bytes now holds
what a 256-byte class's magazine held already.

**Sizes measured first**, each a constant change at e96086d run against main in the runner
(three to five rounds; sh8bench seven): 512, 1,024, 2,048, 4,096 slots and 8,192 (the byte
budget alone). Change against main, and cgroup peak MiB:

| Job | main | 512 | 1,024 | 2,048 | 4,096 | 8,192 |
|---|---|---|---|---|---|---|
| sh8bench, two L3s (main 0.51 s) | 128 | -15% | -24% | -28% | -32% | -33% |
| tree depth 20 | 50 | -5% | -6% | -9% | -10% | -11% |
| pipeline t2 local; peak | 2.0 | -7.5%; 2.5 | -10%; 3.0 | -10%; 3.1 | -11%; 3.2 | -10%; 3.3 |
| pipeline t2 x3 both; peak | 4.7 | -12%; 5.7 | -18%; 7.0 | -19%; 7.5 | -20%; 8.1 | -19%; 8.2 |
| forkjoin t8; peak | 17.5 | -2%; 18.5 | -4%; 19.6 | -4%; 19.9 | -4%; 20.1 | -4%; 20.1 |
| pipeline t8; peak (time level) | 7.8 | 9.5 | 10.8 | 11.5 | 12.0 | 12.2 |
| rpc log c64, time | | | +1.3% | +3.2% | +2.4% | +1.6% |

Cross-thread jobs take nearly all their gain by 1,024 slots; sh8bench, the tree and JSON go on
gaining to 4,096 and beyond, while the memory parked in loaded magazines, and the work one slow
path does with one, double with each step. 1,024 was chosen: it gives the smallest classes what
the 256-byte class already had, so it adds no new kind of cost.

## Bounded work per call

The bounds on a slow path were counted in magazines: a fill filled one, and a heartbeat
laundered up to about four. Four times the objects a magazine would have made them four times
the work. They are now counted in work.

- **A fill claims from at most 256 bitmap words** (`back::policy::FILL_CLAIMS`,
  `Back::fill_within`) and hands out what it has. From words with one free slot each that is
  256 objects, what a 254-object magazine cost; denser words fill a whole magazine (dense ones in
  16 claims). A fill that runs out of free slots still carves the rest.
- **A slow path launders at most 1,022 objects by its own choice** (`depot::LAUNDER_BUDGET`):
  the refresh, the age limit or sweep, the shed, and the launder over target of what the
  heartbeat's own sweep and drain pushed each launder a magazine only while a full one of its
  class fits in what is left, after setting aside the objects the exchange pushed past a shared
  stack's target. Those are laundered last in the heartbeat, one magazine per push, whatever the
  rules did, so a shared stack still sheds magazines as fast as they arrive. A rule turned away
  waits for a later slow path; what a drain parks over a target and the budget leaves, the shed
  takes back. For classes of 254 objects or fewer that is four magazines, as before.

**Rates.** The launder over target stays one magazine per push past a target. The refresh
counts every retirement and is skipped when the budget is already spent in the same call or a
rule is waiting for it (below). The shed acts on debts any CPU can pay. The age limit and the
sweep are per slab: only their own CPU's slow paths run them, and the idle drain skips a busy
CPU. So each looks for work before it asks for room, and when it finds work and no room it
records that in the slab's sweep word; the CPU's next slow path gives it the whole budget first
and then still launders its own pushes over target (`depot/age.rs`, `Sweep`). Only the objects
of a push that left a shared stack over its target are set aside from the budget. A refresh due
on a call that cannot launder is postponed to the next retirement.

*Corrected after review (2026-10-19).* The first version set aside every object an exchange
pushed on a shared stack, over target or not, and gated the age limit and the sweep before they
looked for work, so a CPU whose every slow path pushed on a shared stack never ran either: in a
two-domain pipeline of a two-object class, an idle magazine of the smallest class stayed loaded
on the freeing CPU for 60 intervals and its sweep reached 4 classes of 44 (on main, emptied in
1.1 intervals). An earlier version of this note said such a CPU leaves those rules to other
CPUs; it cannot. Two tests now run that pipeline for three intervals, with the shared stack
under its target and with every push over it, and require the idle magazine taken back and the
sweep round every class (`depot/rules.rs`).

*Corrected after the second review.* That version let a call that carried a turned-away rule
skip its own launder over target. On a CPU that only frees, where every exchange pushes past
the target, the launder takes one magazine a push and never caught up: the shared stack
levelled at 19 to 25 magazines against a target of 6 for the 16-byte class and 36 to 58
against 12 for the 8-byte class, for hundreds of intervals, where main holds it at target. A
call now launders its own pushes whatever the carried rule spent. The 16-byte stack stays at its
target; the 8-byte one sits at 16 to 29 magazines against 12, the rest being what the sweep
sends on from the CPU's two full spares, until the shed takes it back. A test runs that phase for both classes and bounds
the stack at two and a half times its target (`a_free_only_phase_keeps_the_shared_stack_at_its_target`).
The price is a worst call of two of the largest magazines, on a call that carries a rule, which
happens at most once an interval per class and rule (below).

Fewer exchanges do mean fewer heartbeats for the smallest classes: the age look's rotation over
44 classes takes four times as many of that class's operations as before. The interval-paced
rules depend on there being slow paths at all, as they did before; `maintain` covers a program
that has none.

**The worst single call**, from `backbench` per-object costs on CPU 2 (8-byte class, medians of
three rounds), times the most one call can do: main, a fill of 254 and three launders of 254;
branch, a fill of 256 claims and 1,022 laundered.

| Case, cycles per object (main / branch) | main, per call | branch, per call | ratio |
|---|---:|---:|---:|
| fill, one free slot a word: 98 / 103 | 25K | 26K | 1.05 |
| scattered launder, small heap (16 KiB bitmap): 27 / 23.5 | 21K a magazine | 24K | |
| scattered launder, 8 MiB bitmap (64M slots): 112 / 74 | 28K a magazine | 76K | |
| scattered launder, 32 MiB bitmap (256M slots): 199 / 197 | 51K a magazine | 201K | |
| worst call, small heap: fill and launders | 88K | 50K | 0.57 |
| worst call, 8 MiB bitmap | 110K | 102K | 0.93 |
| worst call, 32 MiB bitmap | 177K | 228K | 1.29 |

So the worst call is at most 1.3 times main's, about 41 us at 5.5 GHz, on a class with a quarter
of a billion live objects. A call that carries a rule the budget turned away and also launders
its own push launders two of the largest magazines: 48K cycles on the small heap, 151K on the
8 MiB bitmap and 388K on the 32 MiB one (rechecked after the second review: 23.5, 74 and 190
cycles an object), 0.55, 1.4 and 2.2 times main's worst, at most once an interval for each class
and rule. With the bounds counted in magazines it would have been about 911K
cycles there (a fill of 1,022 and four launders), five times. A batch of 1,022 launders faster
per object than one of 254 where the bitmap fits the caches (the per-call costs are spread
further), which is why the middle row went down.

## Method

The runner, interleaved, main against the branch (and against the plain constant change, m1k,
while the bounds were developed): C programs seven rounds, cross-thread and single-threaded jobs
five, the RPC jobs ten (two sessions of five). sh8bench on one L3 by hand, six rounds,
`taskset 0-7`, a 400 ms spin on those CPUs before each run. Memory probes `worst.sh`, one run
each. `backbench` on CPU 2 under the measurement lock.

## Time and memory

Change against main (median of per-round ratios), and cgroup peak MiB, main to branch; jemalloc's
peak from the e96086d full session:

| Job | main | change | peak main / branch | jemalloc peak |
|---|---:|---:|---|---:|
| sh8bench, two L3s, s | 0.512 | -22.4% | 128.9 / 128.5 | 162.6 |
| sh8bench, one L3, s | 0.279 | -14% | | |
| tree depth 20, cycles/node | 43.5 | -7.0% | 50.2 / 50.2 | 66.7 |
| JSON 32 MiB native / preloaded | 17.8 / 18.5 | -5.4% / -3.9% | 349.5 / 349.7 | 361.2 |
| BTreeMap 1M | 916 | -1.0% | 123.2 / 123.5 | 125.2 |
| churn 100k native / preloaded | 44.1 / 49.9 | -3.7% / -3.9% | 12.0 / 12.2 | 11.7 |
| churn 1M | 104.8 | -1.7% | 110.2 / 110.2 | 110.2 |
| churn 1k | 23.2 | +0.2% | 0.8 / 1.0 | 0.7 |
| pipeline t2 s64 | 3.76 | -19.9% | 1.2 / 1.5 | 1.4 |
| pipeline t2 small, all-alloc / free-only | 3.77 / 3.77 | -9.8% / -9.6% | 2.0 / 3.0 | 1.8 |
| pipeline t2 small, preloaded | 3.97 | -6.8% | 2.2 / 2.4 | 1.9 |
| pipeline t2 mixed | 11.32 | -3.1% | 4.7 / 5.7 | 5.1 |
| pipeline t2 split, all-alloc / free-only | 12.14 / 11.98 | -12.0% / -10.4% | 2.5 / 3.5 | 1.8 |
| pipeline t2 x2 local / split | 4.06 / 4.59 | -11.4% / -14.8% | 3.4 / 4.5, 3.5 / 5.0 | 3.2 |
| pipeline t2 x3 / x4 | 5.29 / 5.42 | -17.7% / -16.0% | 4.7 / 6.8, 5.8 / 8.5 | 4.7, 6.0 |
| pipeline t4 / t8 | 3.02 / 4.18 | -0.1% / +0.1% | 4.0 / 5.2, 7.8 / 10.8 | 3.7, 6.6 |
| exchange t2 local / split | 4.16 / 13.78 | -8.2% / -5.4% | 2.2 / 2.9, 2.8 / 3.5 | 2.4, 2.3 |
| exchange t8 | 2.64 | -1.3% | 6.8 / 9.4 | 8.9 |
| mpmc p1 c3, all-alloc / free-only | 4.91 / 4.94 | -5.6% / -4.5% | 3.2 / 4.2 | 2.2 |
| mpmc p3 c1, all-alloc / free-only | 4.62 / 4.67 | -2.7% / -2.6% | 3.5 / 5.0 | 3.2 |
| mpmc p4 c4 (see below) | 5.06 | -3.6% | 6.1 / 8.2 | 4.7 |
| forkjoin t4 / t8 | 3.46 / 3.83 | -1.4% / -5.8% | 15.4 / 16.5, 17.7 / 19.5 | 31.0, 22.6 |
| larson / larson-sized, M ops/s | 464 / 460 | +1.9% / -0.8% (ranges -6 to +5) | 76 / 78 | 90, 88 |
| mstress, s | 0.263 | -0.7% | 139.0 / 140.9 | 145.0 |
| xmalloc-test, M frees/s, fast mode | 725 to 735 | 924 to 976 (+28%) | 36.6 / 35.5 | 53.6 |
| rpc echo c64, server cycles/req | 45.7k | -0.2% [-2, +1] | 5.8 / 6.3 | 5.8 |
| rpc log c64 | 124.4k | -0.7% [-2, +2] | 8.8 / 9.3 | 9.6 |
| rpc log c256 | 120.6k | +0.2% [-2, +2] | 14.2 / 14.5 | 14.1 |

xmalloc-test is bimodal for every build by where the scheduler puts its threads: main ran 407
and 431 twice and about 730 five times, the branch 562 once and 924 to 976 six times. The RPC
jobs, which looked 1 to 3% slower at three rounds with the larger constant changes, are level at
ten.

The memory probes are level: burst-idle 215.8 / 216.3, phase 49.0 / 49.0, cpus-few-classes
6.7 / 7.2, classes-few-cpus 15.7 / 16.1, threads-few-cpus 15.7 / 16.0, sparse 3.5 and 33.2 both,
tiny 0.7 both.

**mpmc p4 c4 is bimodal by code layout.** One version of the branch, with the same source in
every build, gave 4.98 ns (default), 4.20 (loops aligned), 4.21 and 4.94 (two link orders) under
`benchrun layout`, and two builds of it whose sources differed only in a doc comment ran 4.2 and
4.9 in every session, the slow one at 1.2% more instructions. Under `benchrun layout` the final
branch runs 4.19 to 4.40 and main 4.58 to 4.63, while the runner's builds above gave 4.92 and
5.06. So the larger magazines are worth about 8% on this job, and its speed otherwise depends on
where the code lands. The cause is not found: the profile puts 92% of cycles in the workload's
loop with the inlined fast paths, the same instructions moved by 32 bytes. Variants of the budget
compared on this job while the bounds were developed (where in the heartbeat the launder over
target runs, whether it brings stacks down to their targets, one or two magazines of budget)
moved with the layout rather than the variant; the choices above are the ones that keep the
heartbeat as it was.

## What it costs

- **Memory in flight.** Loaded magazines of the smallest classes hold up to 64 KiB of objects
  each, per CPU and class in use, and their slots 8 KiB. Small cross-thread heaps grow by 0.5 to
  3 MiB and go over jemalloc's peak (pipeline t2 3.0 against 1.8, x4 8.5 against 6.0, pipeline
  t8 10.8 against 6.6, mpmc p4 c4 8.2 against 4.7); forkjoin grows 1.1 to 1.8 MiB and stays well
  under jemalloc's 31 and 23 MiB. Every other heap over 10 MiB moves by under 2% (mstress 1.9
  MiB of 139, the rest 0.3 MiB or less).
- **Fixed full spares.** A CPU's spare budget is split evenly over the 44 classes, about 23 KiB
  each, and a class keeps a full spare only if its magazine fits that share. With 1,022-object
  magazines the 8-byte class keeps two (was four), the 16-byte one (was four), and 24 to 128
  bytes none (24 to 80 kept one to three). Every full magazine those classes retire goes to the
  depot unless stock covers it, and stock is earned only by round trips that met contention. A
  thread that oscillates around a magazine boundary in those classes now reaches the depot for
  each full magazine it takes back, at a quarter of the frequency it reached it before. Nothing
  measured shows a cost; one guaranteed full spare per class in use (64 KiB of objects per class
  and CPU at most) is the follow-up to try if a job does.
- **Memory after a peak: level, once the probe's marks are off the purge's steps.** The
  recheck after review first read burst-idle as 90.6, 30.2 and 4.9 MiB at 2, 6 and 12 s against
  main's 68.0, 21.3 and 1.3. Those marks fall a few milliseconds from the clocked purge's steps,
  which come once an interval at the same times in every run, so a run reads a step early or
  late by chance of a few milliseconds. Sampled every 250 ms, three runs each, medians, MiB:

  | burst-idle, s after the free | 1.5 | 2.5 | 3.5 | 4.5 | 6.5 | 8.5 | 10.5 | 11.9 | mean |
  |---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
  | main | 214.9 | 68.0 | 50.2 | 38.3 | 21.3 | 11.3 | 5.3 | 3.3 | 52.1 |
  | branch | 215.1 | 68.2 | 51.4 | 39.1 | 22.7 | 12.7 | 6.7 | 4.7 | 55.2 |

  | phase change, s after | 1.5 | 2.5 | 4.5 | 6.5 | 7.9 | mean |
  |---|---:|---:|---:|---:|---:|---:|
  | main | 50.6 | 38.4 | 22.0 | 12.7 | 8.0 | 27.4 |
  | branch | 49.9 | 40.5 | 23.8 | 14.2 | 9.7 | 28.8 |

  What is left is 1 to 1.7 MiB, from about 4 s on, in the class the idle phase uses: a
  sampler counting pages (a scratch probe) found 3.4 MiB of its free pages unpurged and 1.6 MiB
  pinned by its magazines at 12 s, against 0.1 and 0.2 on main. Fills of 1,022 objects reach
  further into the region, so its frontier keeps more pages, and its magazines are larger. The
  depot's parked objects were the same on both, 33,000 to 34,000 pinning 49 MiB just after the
  free. In one probe run the depot shed them an interval late on the branch (40 MiB pinned
  until 2.3 s): the debt is set at a look in a new interval, and looks come every sixteen slow
  paths, a quarter as many per allocation as on main. Looking every four slow paths removed
  that lag in its run, but it also looks four times as often on paths whose magazines did not
  grow: the large-object pipeline +3.5% against +1.4%, largepair +1.2% against -0.3% (three
  rounds), so it is not kept. Counting the look period in objects moved rather than in slow
  paths would remove the lag without that cost. jemalloc holds 141.5, 55.9 and 4.5 MiB at 2, 6
  and 12 s on this probe; mimalloc 226.2 until 6 s, then 14.8 to 16.2.
- **rpc log c64**, rechecked after review over twenty rounds: +1.3% [-1.8, +3.6] server
  cycles a request, at the same instructions (82.69 against 82.73 G); `benchrun layout` puts
  four layouts of the branch 1.7% apart (noise 0.7%), and the first build measured -0.7% at ten
  rounds. Not resolved; the colder objects of larger fills are untested as a cause.
- **Fills from sparse bitmaps are partial.** A fill stops after 256 claims, so in a fragmented
  heap a magazine may hold as few objects as before, and the exchange rate there is what it was.

## Also found

- **The geometry did not require a region to hold the largest magazine.** The magazine area is
  aligned only to the region size, and with 8 KiB magazines the test rig's 4 KiB regions put
  magazines out of alignment, which the slot tests then misread ("a slab word names something
  that is not a magazine"). `Geometry::try_new` now requires the smallest region to be at least
  the largest magazine, and the rig uses 8 KiB regions. The design's regions are 2 MiB at the
  floor.
- **backbench's 64 MiB bitmap case aborted on main.** The sweep names its slots as
  `base + index * size`, which holds only within one region; since small classes start at an
  origin a few pages in, 2^29 slots of 8 bytes no longer fit one region, the carve spilled into
  the next, and the sweep laundered addresses past the first region's last slot, which the back
  end rightly refused. The bench now checks that its slots are contiguous and says which size
  does not fit; 32 MiB is the largest for the 8-byte class.
- **The test shape ran out of magazines.** `Standard`, which the stress tests, Miri and the
  fuzzer use, had room for 127 of the largest magazines, and two one-owner stress tests served
  1,520 and 69,673 calls from the back end because none could be made. It now has 16 magazine
  regions (255), and those tests require that no magazine was refused.
- The routines and their proofs needed nothing: the masks are 32 bits, the Lean specification and
  the generated constants already cover 4 to 2^16 slots, and Kani's marker harness checks every
  mask from 2^2 to 2^16. No generated file changed.
