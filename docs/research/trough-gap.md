# The trough gap: where the memory is when the tide goes out

2026-10-27 and 28, allocator code at main `d36e0cb`, on the reference host (Ryzen 9950X, Linux
7.2.9, transparent huge pages on `madvise`). The job is `large-heap-constants.md`'s
`shape=lifetime`: short-lived values whose total alternates between 0.79 and 0.066 of the heap
size beside a long-lived cache of 0.2 of it, six phases, then a quiet minute. At the end of each
low phase (a trough) asmalloc held more than jemalloc at every size. Does that hold on main,
where is the memory in each allocator, and why does jemalloc have less? A last section answers
a second question from the lead: how much the clocked pass should return in one interval.

## Summary

- **The gap stands on main.** Trough heap (resident less the workload's own books), median of
  three troughs: 0.26 against jemalloc's 0.18 GiB at 0.5 GiB, 1.08 against 0.66 at 2 GiB, 3.64
  against 3.04 at 5 GiB. Since `bf837db` the 2 GiB trough came down from 2.21 to 2.02 times
  live; the rest is not huge pages.
- **It is placement.** At the 2 GiB trough both allocators hold the same slots (590 MiB with
  class rounding, jemalloc 593). asmalloc's sit on 1,006 MiB of pages, jemalloc's on 632 MiB of
  slabs. Free slots on pages that also hold a live object are 408 MiB in asmalloc and 40 MiB in
  jemalloc: that is the gap. Everything the huge-page rules keep is 90 MiB, against which
  jemalloc spends 33 MiB more on metadata; parked objects are 9 MiB.
- **Why.** In a trough a class's frees and allocations balance on each CPU, so the loaded
  magazine's level wanders and each allocation pops the slot the last free pushed: a new value,
  short-lived or long, takes a dying value's slot, and the dying values are spread over the whole
  peak footprint. Fills take the lowest free slots, but few allocations reach one. jemalloc's
  thread caches hold at most 200 objects a bin and are flushed by low-water mark every 64 KiB
  allocated, so most of its allocations come from the lowest non-full slab, and its bins are 85
  to 96% full at the trough. In asmalloc, classes of 320 B and up pack as well as that; classes
  of 256 B and down are 25 to 50% full on every page, worse the smaller the class, and so the
  more objects its magazine holds.
- **What it is not:** the stranded huge-page units of the earlier notes (the tail rule took
  them), parked objects, the floor, metadata.

Ranked proposals, each with its measured price:

1. **Launder one retiring full magazine in four, not one in 32** (`depot/slow.rs`,
   `REFRESH_EVERY`). The 2 GiB trough falls by 16% (1.12 to 0.94 GiB, three timed rounds), every
   class from 160 B up packs; cycles +1.1% (median of three rounds on the 18-second phases),
   +3.3% on 1.4-second phases, where faults rise by four fifths. On the order service at 2 GiB,
   no change in memory and +0.3 to 1.2% cycles a request, within what separate builds of a
   server move by placement alone. Classes of 128 B and down do not move.
2. **The clocked pass's goal: everything returnable above the floor** (C below). It is what the
   code already does on a large heap, by a longer road. A quarter of what is returnable (B) keeps
   0.15 to 0.43 GiB more at 2 GiB troughs for a third fewer faults, which cost no measurable
   cycles; on a small heap the three cannot be told apart.
3. **The smallest classes need a rule for the loaded magazine.** Not built. Retirement rules do
   not reach them: magazines of at most 256 slots took 67 MiB off the trough for 2.4% of cycles,
   laundering every retiring magazine 190 MiB for 10%. Ideas under "What would pack the smallest
   classes".
4. **Keep huge pages and the split cut as they are.** No huge pages at all saved 35 MiB at the
   2 GiB trough and cost 2 to 4% of cycles and 1.4 to 2.5 times the faults; splitting huge pages
   from an eighth free instead of a quarter changed nothing.

Not measured: the variants at 5 and 10 GiB; a real server in a trough with the variants (the
order service has none; below); more than one round of most timed runs; `r4`'s server price
inside one binary.

## The gap on main

Memory runs (`large-heap-constants/ladder.sh`, heavy slice, one run each); medians of the three
troughs, the heap after the quiet minute, seconds for a trough to come within a tenth of its end
(the heavy slice was frozen for minutes by other measurements, so phases ran 50 to 150 s):

| Live | asmalloc trough, quiet (GiB) | jemalloc trough, quiet | asmalloc over jemalloc | `large-heap-constants`, at `bf837db` |
|---|---|---|---|---|
| 0.5 GiB | 0.258, 0.253 | 0.177, 0.175 | +0.08 | 1.96 vs 1.37 times live (+0.08) |
| 2 GiB | 1.077, 1.036 | 0.659, 0.653 | +0.42 | 2.21 vs 1.25 (+0.5) |
| 5 GiB | 3.641, 3.538 | 3.042, 2.985 | +0.60 | 2.89 vs 2.27 (+0.8) |

Peaks: asmalloc 1.156, 1.102, 1.087 times live, jemalloc 1.171, 1.124, 1.111. On the
measurement CPUs (18-second phases, three rounds) asmalloc's 2 GiB trough is 1.10 to 1.14 GiB,
within 14 to 16 s of its fall; jemalloc's is 0.66.

## Where the memory is

At the end of each phase every thread stops at a barrier and the instrumented build takes an
account: asmalloc's walk of its heap (`Asmalloc::inspect`, extended to report each region's aged
frontier, tail boundary and clean bits) with `mincore` over every region, and every resident page
attributed by what the rules would do with it; jemalloc's own statistics (built with
`--enable-stats`): totals, merged arenas' active, dirty and muzzy pages, every bin's regions and
slabs. Means of the three 2 GiB troughs, MiB:

| | asmalloc | jemalloc |
|---|---|---|
| Heap (RssAnon less books) | 1,099 | 677 |
| Program's live bytes | 546 | 546 |
| Class rounding | 44 | 47 |
| Free slots on pages that also hold a live object | **408** | 40 (free regions of slabs) |
| Parked (magazines, spares, depot; thread caches) | 8.7, and 0.7 free beside them | 0.9 |
| Free pages of the small-page part: below the frontier; waiting above it | 0.4; 1.3 | 7.4 dirty |
| Free pages in huge pages that start below the frontier | 4.3 | |
| Free pages in huge pages above it with under a quarter free | 5.7 | |
| Free pages of split huge pages not yet returned (all in runs under four pages) | 61 | |
| Resident past the watermarks | 17.6 | |
| Metadata | 4.6 | 37.8 |
| Not accounted | -2.8 | -1.3 |
| Pages holding a live object, or slabs | 1,006 | 632 |

So jemalloc's live regions fill its slabs to 94%, and asmalloc's live slots fill the pages they
are on to 59%. The huge-page rules' share (rows 7 to 10) is 89 MiB. The split pages wait because
they are short runs (one pass in eight takes those) and the dying short-lived values keep freeing
more of them through the trough; nothing there is a rule failing. The allocator's count of
retained memory at the trough is 490 to 510 MiB, almost all of it fragmentation.

By class (2 GiB, first trough; MiB):

| Class | live slots | free beside them | pages with live | jemalloc bin full |
|---|---|---|---|---|
| 16 to 64 B | 28.9 | 79.2 | 109 | 86 to 94% |
| 80 to 128 B | 63.1 | 171.1 | 235 | 89 to 95% |
| 160 to 256 B | 126.3 | 125.5 | 253 | 90 to 96% |
| 320 to 512 B | 149.3 | 27.3 | 178 | 94 to 96% |
| 640 B to 8 KiB | 221 | 5 | 230 | 85 to 100% |

The unit maps say where. A class of 640 B: 16 MiB of small pages and the first 16 huge pages
90% live, the frontier at 47.5 MiB, then 64 huge pages of a split tail with a few objects each,
their free pages returned. A class of 64 B: 24.6 MiB carved, 6.2 MiB live spread over every page
of it, the frontier at 4.7 MiB. Fills of the small classes in a trough reach only the bottom
fifth of their memory; their live objects are everywhere.

The account at the peak, for scale: 2,187 MiB of slots on 2,209 MiB of pages, 15 MiB free
beside them; jemalloc 2,198 on 2,242.

## Why asmalloc places objects where the last ones died

When the short-lived total falls from 0.79 to 0.066 of the heap, each thread frees two values
for each it allocates until its FIFO is at the low target, a fraction of a second. The excess
frees go to the depot and are laundered (its targets are a fraction of a megabyte a class).
After that, each class's frees and allocations balance on each CPU, and the magazine loaded for
the class does the work: a free pushes a dying value's slot, the next allocation of that class
pops it. The level random-walks and reaches an end, which is when anything is exchanged,
laundered or filled, about once in the square of the magazine's size in operations of that
class. Magazines hold up to 1,022 objects for classes up to 64 B, 512 for 128 B, 102 for 640 B.
The long-lived cache (three quarters of the trough's live bytes) turns over about twelve times a
phase, each new value taking a slot the magazine just got back. So the trough's live objects
are a sample of the peak's slots, and the smaller the class the closer to uniform.

jemalloc has the same LIFO cache per bin but bounds it at twice a slab's regions, clamped to
20 to 200 (`tcache.c:20`, `opt_tcache_nslots_small_max`), and every 64 KiB a thread allocates
it visits one bin and flushes three quarters of what stayed unused (`opt_tcache_gc_incr_bytes`).
A bin's level therefore reaches its ends often, flushes return regions to their slabs, and fills
come from the bin's current slab, which jemalloc keeps the oldest, lowest non-full one
(`arena.c`, `arena_bin_slabs_nonfull_tryget` from a heap ordered by age and address). Its
placement is lowest-first most of the time; asmalloc's is lowest-first only at a fill.

The variants test this. Each is the diagnostic build with one constant changed, accounted at
2 GiB (one memory run each, means of three troughs, MiB):

| Build | heap | free beside live | pages with live | split pages waiting |
|---|---|---|---|---|
| main | 1,099 | 408 | 1,006 | 61 |
| refresh one retiring magazine in 4 (`r4`) | 934 | 271 | 868 | 26 |
| refresh every retiring magazine (`r1`) | 926 | 249 | 844 | 35 |
| magazines of at most 256 slots (`m256`) | 1,032 | 322 | 919 | 72 |
| no huge pages (`nh`) | 1,064 | 408 | 1,005 | (44 free small pages waiting) |
| split from an eighth free (`q8`) | 1,087 | 407 | 1,004 | 49 |

The refresh packs every class from 160 B up (256 B: 25.9 free beside 27.5 live with main, 6.3
with `r4`, 3.3 with `r1`) and leaves 64 and 128 B where they were (17 and 37 MiB free beside 6
and 14 live). Those classes' magazines rarely retire at all; their loaded magazine recycles in
place. Smaller magazines reach them (128 B: 20 MiB free beside 14 with `m256`) and nothing else.

On the small job (live 21 MiB at the troughs, 0.08 of a GiB rung) the same holds at a smaller
scale: asmalloc 95 MiB against jemalloc's 53, free beside live 25 against 6, and 34 MiB of free
whole pages held under the 32 MiB floor; `r4` 89, `r1` 90 (free beside live 15).

## What each proposal costs

Timed runs on the measurement CPUs (0-3 and 8-11) under `scripts/measure --both-l3`, the
diagnostic build of each variant, `life2` with its 18-second phases and `short2` with 1.4-second
ones (the same heap, 24 phases). Cycles an operation over the load, faults a thousand operations,
trough heap in GiB:

| Build | life2 trough | life2 cycles | life2 faults | short2 cycles | short2 faults |
|---|---|---|---|---|---|
| main (3 rounds) | 1.095, 1.136, 1.128 | 635.6, 636.8, 646.4 | 0.12 to 0.16 | 606.3 | 0.111 |
| `r4` (3 rounds) | 0.931, 0.956, 0.929 | 647.0, 643.5, 653.8 | 0.12 to 0.17 | 626.2 | 0.198 |
| `r1` | 0.904 | 698.3 | 0.117 | 657.6 | 0.262 |
| `m256` | 1.082 | 650.6 | 0.168 | | |
| `nh` | 1.141 | 648.8 | 0.168 | 631.6 | 0.273 |

`r4` costs 1.1 to 1.8% on the long phases, round by round, and 3.3% on the short ones, where it
launders magazines whose objects the next phase takes straight back and the fills to replace
them search fragmented bitmaps. `r1` costs two to five times as much for a little more memory.
These are allocation-bound jobs at about 600 cycles an operation; a server's share of time in
the allocator is a few percent, so its price there is a fraction of that.

The order service at 2 GiB (`servers-large`, `orders axum cache 2G`: fill, a minute at 70%,
bursts after silences), the variants built into it with the same counters, three sessions with
the builds in a random order under `scripts/measure --both-l3`:

| Build | Mcycles a request, by round | p99 / p99.9 us | resident over live, end of the minute / after close |
|---|---|---|---|
| main | 67.51, 67.42, 68.04 | 22.1 / 24.2 | 1.080 / 1.081 |
| `r4` | 67.70, 68.21, 68.25 | 22.0 / 24.0 | 1.081 / 1.081 |
| `m256` | 68.23, 67.74, 68.40 | 22.0 / 23.8 | 1.080 / 1.081 |

The store frees an order for each it stores, so no class shrinks and nothing changes in memory.
`r4` is 0.3, 1.2 and 0.3% slower round by round; separate builds of a server move by more than
that with where their code lands (`grpc-placement.md`), so this bounds its price here and does
not measure it. A switch inside one binary would.

## What would pack the smallest classes

Not built; for the next study. The loaded magazine of a small class is the recycler, and it
leaves a CPU's slab only at an end of its walk or at the age limit (an interval). Three shapes,
in order of how directly they act:

- A fill-side rule: when an allocation's swap finds a full spare or a depot magazine, take it
  only if the class is not shrinking on this CPU, else launder it and fill from the bitmaps.
  Needs a signal of shrinking that a slow path can read (the swap direction record exists).
- An age limit in exchanges as well as time: a loaded magazine of a class whose slots are dense
  in their pages is retired after it has turned over its own size some number of times. The fast
  path counts nothing, so the count would be the slow path's estimate from the class's traffic.
- Smaller magazines for the smallest classes only (64 KiB of objects is 1,022 slots at 64 B).
  This undoes measured speed (`launder-and-fill.md`) and `m256` took only 67 MiB.

## Do real servers show the pattern?

The pattern needs a long-lived population allocated during a fall in the short-lived one, in the
same classes. Of the servers under `bench/apps`:

- **Qdrant** shows it: the optimizers fill the 64 and 128 B classes in bursts beside long-lived
  payload objects. Before the tail rule its 64 B class held 43 MiB a minute after close with
  nothing live (`servers-qdrant.md`); with the rule asmalloc's charge a minute after close was
  139 MiB, where jemalloc's had been 131 in an earlier session. Whether the rest is in the small
  classes this note finds unpacked was not measured.
- **mini-redis** with keys rewritten at changing sizes (`heap-drift.md`): after the tail rule its
  wide phases are level with jemalloc (1.15 to 1.17 against 1.16), its small-value phases below.
- **The order service** has no trough: every request stores one order and drops the oldest, so
  its classes never shrink. asmalloc is the leanest there (1.08 times live at 2 GiB against
  jemalloc's 1.10, `large-heap-constants.md`). A server whose request concurrency, and with it
  in-flight memory, falls by an order of magnitude while it keeps writing to a cache would show
  it; none of ours is driven that way.

So the synthetic gap is larger than any measured server's. The mechanism is general, though: any
server whose small-class population falls by a factor of several while it keeps allocating
long-lived small objects keeps its peak's pages at the density of its trough.

## How much a pass returns

The code (`back/purge.rs`, `start_pass`) sets a pass's goal to the larger of a quarter of the
retained count and what is over the 64 MiB cap, held to what the last lap could return less the
32 MiB floor. On a large heap the retained count is mostly fragmentation and far over the cap, so
the pass may take everything the last lap counted. Three arms (`decay_edit.py`):

- **A**, as built.
- **B**, a quarter of what the last lap could return (at least 2 MiB), down to the floor; the cap
  plays no part.
- **C**, everything the last lap could return, down to the floor.

Timed runs as above, one each; trough heap in GiB (median of the troughs), seconds for a trough
to come within a tenth of its end, cycles an operation, faults a thousand operations,
`MADV_DONTNEED` calls (thousands) and GiB the clocked pass returned:

| Job | A | B | C |
|---|---|---|---|
| life2, 18 s phases | 1.095, 15.6 s, 636, 0.120, 284, 3.47 | 1.530, 15.7 s, 640, 0.085, 76, 2.34 | 1.127, 13.9 s, 641, 0.118, 290, 3.46 |
| 2 GiB, 4.4 s phases | 1.887, 4.4 s, 635, 0.196, 207, 3.91 | 2.039, 4.4 s, 634, 0.126, 86, 1.91 | 1.853, 4.5 s, 639, 0.203, 235, 4.11 |
| 2 GiB, 1.4 s phases | 2.176, 606, 0.111 | 2.181, 609, 0.110 | 2.176, 608, 0.110 |
| small heap, 10 s phases (MiB) | 85, 7.6 s, 407.4, 0.0050, 0.7 | 85, 7.6 s, 407.8, 0.0052, 0.9 | 85, 7.6 s, 407.6, 0.0052, 0.7 |

The memory runs on the heavy slice agree: at 2 GiB with long phases B's troughs are 0.09 GiB
above A's and C's, with 4-second phases 0.07 GiB above, with a third fewer faults.

- **A and C are the same rule on a large heap**, as expected: retained memory is far over the
  cap, so A's goal is "everything over the cap", which the last lap's count then holds. Their
  differences are within a run's noise.
- **B keeps memory for faults that cost nothing measurable.** With 18-second troughs it leaves
  0.43 GiB more at the trough's end (a quarter an interval of a falling count does not keep up
  with what a phase frees); with 4-second phases 0.15 GiB more. It halves to quarters the
  `madvise` calls and cuts faults by a third, and cycles do not move (within 0.7%). The faults
  are refaults of pages the next phase takes back, at 0.1 to 0.2 a thousand operations: tens of
  microseconds a second on this job.
- **On the small heap nothing separates them.** It retained 54 to 78 MiB (over the cap in 16 of
  62 passes), and two passes in three found no more than the floor returnable, so every arm
  returned the same 70 to 80 MiB in a run. Where the cap is not exceeded, A and B differ only in
  what the quarter is taken of (the retained count or the returnable part), and the floor then
  holds both.

**Recommendation: C.** It is the simplest statement of what the code does on large heaps, it
does not depend on the cap (whose job is growth's payment), and the clock alone decides when.
If the lead wants the design's quarter, B is the rule to write, and its price is the troughs
above. The cap's term in A matters only on a heap retained between the floor and about four
times the cap with little fragmentation, which no job here was.

## Method

- **Builds** (`bench/probes/trough-gap/`): `build.sh` extracts a commit, applies edit scripts and
  builds the drift workload or the order service. `tg_edit.py` extends the introspection walk
  (each region's aged frontier, tail word and clean bits) and replaces bench-alloc's memstat
  sampler with this probe's `memstat.rs`, which attributes every resident page (`Decomp`) and
  does not count requests (the original's counting made the lifetime job nine times slower,
  which changes what the clock returns per operation); it also adds `je-stats`, jemalloc with its
  statistics, and `je.rs`, which writes jemalloc's account at the same moments. `const_edit.py`
  changes one constant for a variant; `decay_edit.py` the pass's goal. Every variant and the
  timed base carry `large-heap-constants/diag_edit.py`'s counters.
- **Accounts** at each phase end with every thread stopped (`account_every`), the quiet
  minute's last account where the run took one. The accounted build's troughs match the plain
  build's (asmalloc 1.06 to 1.08 GiB against 1.05 to 1.08; jemalloc 0.661 against 0.659).
- **Runs** one at a time (`runs.sh`, `queue.sh`): memory runs on the heavy slice under a 16 GiB
  cap, timed runs on CPUs 0-3 and 8-11 under `scripts/measure --both-l3`. The heavy slice was
  frozen for minutes by other measurements; counted in operations, a frozen run does the same
  work and its accounts stand, but its seconds do not, and only timed runs give seconds and
  cycles.
- **Tables:** `phases.py` (phase ends), `decomp.py` (accounts, `--classes`, `--regions`),
  `runsum.py` (troughs, settle times, cycles, faults, calls).

## Reproduce

From the repository root, on this branch:

```
scripts/heavy bench/probes/heap-drift/build.sh asmalloc je
scripts/heavy bench/probes/trough-gap/build.sh tg-stat HEAD asmalloc-memstat \
  bench/probes/large-heap-constants/diag_edit.py tg_edit.py
scripts/heavy bench/probes/trough-gap/build.sh je-stat HEAD je-stats tg_edit.py
scripts/heavy bench/probes/trough-gap/build-variants.sh "r4=depot/slow.rs:REFRESH_EVERY=4" \
  "r1=depot/slow.rs:REFRESH_EVERY=1" "nh=back/policy.rs:HUGE_MIN_SLOTS_PER_PAGE=1048576" \
  "q8=back/policy.rs:TAIL_SPLIT_SHARE=8" "m256=classes/magazine.rs:MAX_LOG2=8"
scripts/heavy bench/probes/trough-gap/build-decay.sh
scripts/heavy bench/probes/large-heap-constants/ladder.sh OUT/repro lifetime "0.5 2 5" "asmalloc je"
bench/probes/trough-gap/queue.sh LIST     # lines: heavy|timed OUT JOB BUILD...; server OUT ROUND
python3 -I bench/probes/trough-gap/decomp.py OUT/acct/lifetime-2.tg-stat-acct.txt --classes 2
python3 -I bench/probes/trough-gap/runsum.py OUT/t-life2
```

Raw data: `docs/research/data/trough-gap-2026-10-28.tar.gz`: the ladder (`repro/`), the
accounted runs (`acct/`, `var-small/`, `var-life2/`), the decay memory runs (`decay-*`), the timed
runs (`t-*`), the server sessions (`srv/`) and the queue lists and logs.
