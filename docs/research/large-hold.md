# A hold on free large slots

2026-10-20, branch `large-hold` (on `burst-start` and main `0483439`). Free large slots go back
to the OS once their region has had no release for a short hold, about 125 ms, counted in fine
steps of the counted clock; nothing is returned while they total 32 MiB or less. On DataFusion
(TPC-H SF3, four threads) the peak falls from about 1,215 MiB to about 845, under jemalloc's
860, for 13 to 15% more page faults and time within a percent of the base.

Why: memory freed in one large class serves another only by way of the OS, and a program whose
phases use different large classes held every phase's free slots on top of the next phase's
live ones until the clocked pass, a second's step, returned them (`memory.md`, "DataFusion's
peak against jemalloc's").

## The rule as built

- **What.** The free slots of a large class's region, once no large slot has been released into
  that region for [`HOLD_STEPS`] steps of the hold's fine count (64 steps, about 125 ms), go back
  to the OS, down to a floor: nothing is returned while the free large slots believed resident
  (the retained count of large classes) total at most 32 MiB, and above it only what is over it.
  Small classes are left to the clocked pass.
- **Time.** A fine step is 2^23 ticks of the cycle counter, about 2 ms. The fine count steps by
  at most one at a reading of the counted clock (`crate::clock`), so a silence ages a hold by
  one step, not by the silence. The readings are the heartbeat's own; while a hold is under way
  and the heartbeat's clocked pass made none, the hold asks the heartbeat's clock, which answers
  from the depot's reading when there was one. With nothing held no clock is read for it.
- **Words.** The region's header in the back end's state, unused for a large class until now
  (its real header is in its metadata block), holds a mark or a stamp. A release of a large
  slot that stays resident marks its region (a load, and two stores when it was not marked)
  and makes a pass due at once; it reads no clock word. A `due` word says when a pass should
  run, zero for never. A heartbeat compares `due` with the fine count (two loads); a due pass
  walks the large classes' regions within the growth payment's bound of 16,384 bitmap words:
  it stamps marked regions with the fine count, purges the free units of regions whose hold
  has ended (by runs of pages, `Scope::All`), down to the floor, and sets `due` to the next
  hold to end. All of it is hints: a purge is a claim, so a wrong mark, stamp or `due` returns
  a slot early or late, never one in use.

## How long a hold

DataFusion, four threads, cgroup peak MiB, faults and time, five rounds interleaved (one set of
runs on the prototype clock, the stamps at the free; the base the same code with the hold off):

| hold | peak MiB | faults | time s |
|---|---:|---:|---:|
| none | 1,204 to 1,252, one run 913 | 0.90M to 0.96M | 3.44 to 3.50 |
| 16 ms | 828 to 883 | 1.14M | 3.51 to 3.63 |
| 60 ms | 845 to 862 | 1.08M to 1.10M | 3.50 to 3.68 |
| 125 ms | 841 to 843 | 1.04M to 1.06M | 3.46 to 3.53 |
| 250 ms | 1,070 to 1,081 | 0.95M to 0.98M | 3.46 to 3.57 |
| per class, adapting (below) | 819 to 871 | 1.18M to 1.19M | 3.56 to 3.59 |
| jemalloc | 849 to 879 | 1.02M to 1.04M | 3.61 to 3.64 |

The best constant is about 125 ms: shorter returns slots the next query takes back, longer
holds a query's free slots into the next. **An adapting hold did not beat it** and was not
kept: per class, the hold doubled when a slot was claimed within four holds of the hold's
purging its region, and halved after four holds without such a claim, between 4 ms and a
second. On DataFusion it held the peak at the same level for 9% more faults than the constant
of 60 ms it started from and 1.5% more time; the classes whose slots came back late kept their
holds short because the claims came after the window, and the shrink took back what the growth
gave. The last commit with it is `50246f7` (squashed away; `bench/probes/app-faults/edits`
keeps the experiment scripts).

**Populating a moved block's destination** (`MADV_POPULATE_WRITE` over what the move copies,
before the copy) cut tpchgen's faults by 6% and left kernel cycles where they were, and changed
nothing on gix-hours and DataFusion: most of the faults are the first touch and refault of
slots, not the copies. Dropped.

## Results

On the branch merged with main `0483439` (the hold's base is the same code with the hold off):

| | base | hold | jemalloc |
|---|---:|---:|---:|
| DataFusion SF3, cgroup peak MiB (5 runs, by hand) | 1,194 to 1,229 | 843 to 859 | 855 to 875 |
| DataFusion, faults | 920k to 935k | 1.046M to 1.059M | 1.025M to 1.038M |
| DataFusion, Gcycles (apps suite, 5 rounds, median) | 69.63 | 69.77 | |
| ruff, Gcycles; peak MiB (apps suite) | 23.18; 41.0 | 23.24; 41.6 | |
| boa, Gcycles; peak MiB (apps suite) | 44.45; 265 | 44.17; 266 | |
| tpchgen preloaded: Gcycles, faults, peak MiB | 26.06, 711k, 577 | 26.38, 713k, 582 | |
| gix-hours preloaded: Gcycles, faults, peak MiB | 48.14, 991k, 2,931 | 48.48, 982k, 2,935 | |
| datafusion SF1 preloaded: Gcycles, faults, peak MiB | 22.98, 238k, 442 | 23.30, 263k, 455 | |

DataFusion's peak is 0.97 of jemalloc's (it was 1.39), for 13% more faults and 0.2% more
cycles. The apps suite's own report gave the base a median peak of 934 MiB over its five
runs; by hand, each run in a cgroup of its own, every base run peaked at 1,194 or more.

Backend suite, five rounds: level but `pipeline t2 sizes=large`, 60.01 against 60.53 ns an
allocation (+0.9%, noise 0.2%), which runs 0.7% more instructions: the mark on every large free
(a load of the region's header word) and the hold's check on every heartbeat. Its heap is under
the floor, so nothing is purged there.

**Memory after a peak.** Before the merge with main the hold took a peak of large blocks
(`quiet live=4000 sizes=large`) from 248 to 62 MiB within half a second, where the base held
248 for two seconds and then fell to 66 and 37. On the merged code neither the base nor the
hold returns anything in either probe, large or small blocks: the light work after the free
moves too few bytes between tiers to beat under main's pacing by bytes, so no heartbeat runs.
That is not the hold's to fix (the background thread and the tokio task are).

## Method

Builds from the branch with the hold, and the same source with the hold switched off
(`bench/probes/app-faults/edits/nohold.py`) as the base, each built whole (preload library,
the suite's workloads and the apps) by one script, as `build` entries of `bench/allocators.toml`
added locally. The apps suite (`benchrun run --suite apps`) and the backend suite five rounds,
interleaved, under `scripts/measure --both-l3`; DataFusion also run by hand five rounds with
jemalloc beside it, each run in a cgroup of its own for its peak
(`bench/probes/memory/incg.sh`). tpchgen, gix-hours and datafusion SF1 preloaded from the
screening's tree (`bench/probes/app-faults/bench.py`), five rounds. Memory after a peak:
`workload quiet live=4000 sizes=large secs=4 after=burst after_sizes=small` on CPU 2, sampled
every 250 ms.
