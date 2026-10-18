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

# Fillers of one class on many CPUs (2026-10-18)

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

The generated `malloc` and `free` cost about 12 and 8.5 cycles a call here, plus 2 for the
PLT, against 8 to 10 a pair measured in isolation. Counted per symbol with grouped hardware
counters (branch build, CPUs 4-7 and 12-15): they retire 12 and 17 instructions a call, their
straight-line length, so rseq aborts are rare (2,248 context switches in the whole run);
branch mispredictions are about 0.01 a call. The difference is L1 misses: 0.32 a `malloc`
and 0.18 a `free`, landing on the slab word load, the magazine slot load and `free`'s chain
of the rseq offset, the published heap words and the region table. sh8bench writes every
object it allocates and streams through more memory than L1 holds (its own loop takes 46% of
all L1 misses), and each pair touches about six lines of allocator metadata besides the
object, which get evicted between calls; mimalloc's free list lives in the objects the
program is about to touch anyway. This is inference from where the misses land.
