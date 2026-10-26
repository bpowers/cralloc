# Large blocks under churn: slack in occupied slots, and the hold

2026-10-25, main `47e88d4`, on the reference host (Ryzen 9950X, Linux 7.2.9, transparent huge
pages on `madvise`). A follow-up to `heap-drift.md`, whose large mix (a 1 GiB cache of buffers
from 40 KiB to 32 MiB) left asmalloc near twice live in three of four phases, against
jemalloc's 1.3 to 1.5. Measured with a run-time switch in one binary; the switch and the
counters are a probe, not product code (`bench/probes/large-churn/`).

## What matters

- **Main is no better than the commit the drift study measured.** On main `47e88d4`, with
  grown blocks' frees kept for the hold and outgrown slots kept within 64 MiB, the large mix
  ends its phases at 2.32, 1.55, 1.91 and 2.09 times live (5536610: 2.08, 1.45, 2.07, 2.12),
  and 1.19 after a quiet minute (1.11).
- **Two mechanisms hold the memory, and both are traced in the code and counted.**
  - *Slack inside occupied slots.* A large claim settles only the pages under its new block
    (`region::settle_slot`) and takes the slot's whole dirty count off the retained tally. The
    pages past the block that earlier tenants wrote stay resident, counted nowhere, out of
    every purge's reach, until the slot is freed again. At the end of the first phase that is
    787 MiB in 7,179 occupied slots, all resident by `mincore`.
  - *Free slots in regions that are never quiet.* The hold's clock is per region
    (`hold::hold_region`): any free that leaves a slot resident restamps the region, and its
    free slots go back only after a hold with no such free. Under steady churn that never
    happens: 960 to 1,090 MiB of free large slots stay resident in the third and fourth
    phases.
- **Most slack is the last tenants' room, taken over by the next.** 76% of it is a block that
  had moved in with room (16 times its size) freed, its slot taken by a smaller block moving
  in with room; 95% of it is under 125 ms old. A moved tenant grows into 12 to 14% of the
  slack it finds, a whole allocation into almost none (9 MB of 108 GB). Inside a hot slot the
  dirty extent is the high-water mark of every tenant since the slot was last purged: 64% of
  whole tenants' slack and 23% of moved tenants' lies past the previous tenant's own size,
  and one buffer in a hundred ten times larger takes the first phase's slack from 797 to
  1,300 MiB and the phase's ratio, cycle after cycle, from 2.48 to 3.04.
- **The rules, on the large mix** (end of each phase in the last cycle; main 2.34, 1.60, 2.26,
  2.23; jemalloc 1.42, 1.89, 1.29, 1.36):
  - *Trim to the previous tenant at the claim* (return what lies past both the new block and
    the previous tenant's final size): 1.32, 1.49, 1.97, 2.14. It needs no number, stops the
    ratchet inside a slot (the big-buffer variant's first phase 1.40 to 1.55 against main's
    2.48 to 3.04), and costs a third more faults than main here and no cycles.
  - *Age each free slot on its own* (the stamp per slot, written by the holder at its free;
    the hold also walks young regions and returns their old free slots): 2.22, 1.45, 1.74,
    1.79. Nothing in the first phase, where tenants turn over inside a hold, as predicted. In
    the third and fourth it halves the free memory kept, to within a hold's worth of churn.
  - *Both together*: 1.33, 1.40, 1.53, 1.77, and 1.11 after the quiet minute. Level with
    main on every regression job but tpchgen (+1.6% cycles) and DataFusion (+0.6%), below.
  - *A bound on slack* (keep it while it totals at most a bound; past it, a moved block's
    claim returns what lies past twice its size): at 64 MiB, 1.34, 1.47, 2.06, 1.77, level on
    the 64-Vec job. At an eighth of live large memory, 1.28, 1.44, 2.10, 1.77, but it costs the
    64-Vec job 18%: that job holds 256 MiB of large blocks, so the bound is 32 MiB, under the
    room its 64 regrowing buffers use. The bound that passes is a fixed number.
  - *Trims past k times the size* for every claim: k = 1 gives the first phase 1.15, under
    jemalloc's; k = 2, 1.27; k = 4, 1.53; each triples the faults and costs no cycles.
- **Time is not the price on this job.** Every arm takes 8.2 to 8.4 M cycles per operation,
  within 1.3% of main, with up to 3.8 times main's faults. jemalloc takes 1.66 times main's
  cycles with 44 times its faults, glibc 1.22 times.
- **What I would build, ranked:**
  1. The trim to the previous tenant and per-slot ageing together. No new number. Large mix
     2.34, 1.60, 2.26, 2.23 to 1.33, 1.40, 1.53, 1.77; the in-slot climb stopped; level on
     the 64-Vec job, Meilisearch and the order service; DataFusion +0.6% cycles; tpchgen +1.6%
     cycles and about 60% more faults for 70 MiB less peak.
  2. Per-slot ageing alone, if tpchgen's cost decides: level or better on every job, and it
     halves what the third and fourth phases keep, but leaves the first phase's slack.
  - Not proposed: the bound at an eighth of live large memory (64-Vec +18%), the bound at
    64 MiB (works, but a fixed number), the trims past k times the size (a number, and three
    times the faults).
  - jemalloc stays ahead in the phases of 4 to 32 MiB blocks (1.29 against 1.53): the hold's
    worth of churn, which jemalloc does not keep for blocks of 8 MiB or more.

## Method

- **The workload.** `workload drift shape=large` (`bench/workloads/src/drift.rs`), as in
  `heap-drift.md`: eight threads, a 1 GiB cache of buffers, sizes log-uniform in 40 to
  256 KiB, 256 KiB to 4 MiB, 4 to 32 MiB, then 40 KiB to 32 MiB, four cycles, 2.4M
  operations. Half the new buffers are built by doubling from 4 KiB with `realloc`, a fifth
  of operations grow a cached one by half. `big_permille=10` makes one new buffer in a
  hundred ten times larger. Memory runs on the eight CPUs of `scripts/heavy` under
  `scripts/memlimit -m 8G`, one at a time; jemalloc's and glibc's are the drift study's runs
  (their code did not change).
- **The trace** (`lc.rs`, `lc_edit.py`). At every large claim, before it settles, the probe
  counts the dirty pages past the new block's extent, and records in a side table where they
  came from (the slot was an outgrown one kept; the last tenant grew in place; the last tenant
  was a larger whole allocation, or a larger block moved in with room; anything else, which
  includes entries the 1M-entry hash lost) and what took the slot (a moved block: a slot
  three times its size or more; else a whole allocation). At the free it counts how much of
  the slack past one, two, four and eight times the size the tenant had grown over. A walk,
  run while every workload thread is stopped, sums the slack of every occupied slot by those,
  by age, and by `mincore`.
- **The switch** (`lcs.rs`, `lcs_edit.py`), `ASMALLOC_LC=t<k>p<0|1>b<NNNN>a<0|1>`, every arm
  of a comparison in one binary with an environment of the same length:
  - `t` k, a claim returns the slack past k times its block's size for a moved-in block, past
    its extent for a whole one;
  - `p` 1, a claim returns the slack past both its extent and the previous tenant's final
    size, which the size word still holds;
  - `b` a bound in MiB on the slack in occupied slots, or 9999 for an eighth of live large
    memory; past it, a claim returns what lies past twice a moved-in block's size, past a
    whole one's extent;
  - `a` 1, per-slot ageing: the free stamps the slot with the fine count, and the hold's look
    at a region whose stamp is young walks its slots from the top, within its bound of bitmap
    words, and returns those whose own stamp is a hold old.
  Every rule acts at the holder's own calls: the claim, a resize in place, the free. The hold's
  walk claims each slot it returns, as every purge does. A trim is one `madvise` of the
  returned range, then the clean bits; a slot that kept huge-page advice is not trimmed.
- **Timed runs.** The large mix shortened to 240k operations in four phases, on CPUs 0-3 and
  8-11 under `scripts/measure --both-l3`, two rounds of every arm and of jemalloc and glibc in
  a random order, a session each.
- **Regression jobs**, in one binary with the switch, as the hold's and grown-slots' studies
  ran them: the 64-Vec and 256 MiB kernel jobs (`kern.py`, CPU 2, three rounds), tpchgen
  (`sessions/tpch.sh`, three rounds), DataFusion SF3 (benchrun's apps job, six rounds),
  Meilisearch under load (`meili-tail`, two rounds) and the order service at 2 GiB
  (`servers-thread`, one round).

## 1. The large mix on main

| | 40-256 KiB | 256 KiB-4 MiB | 4-32 MiB | mixed | quiet |
|---|---|---|---|---|---|
| asmalloc 5536610 | 2.08 | 1.45 | 2.07 | 2.12 | 1.11 |
| asmalloc main 47e88d4 | 2.32 | 1.55 | 1.91 | 2.09 | 1.19 |
| jemalloc | 1.42 | 1.89 | 1.29 | 1.36 | 1.09 |
| glibc | 1.50 | 1.83 | 2.59 | 2.45 | 2.39 |

At the ends of the four phases of the memstat build (MiB): slack in occupied large slots 802,
149, 55, 105 (5536610: 795, 100, 46, 105); free large slots resident 135, 400, 959, 1,091
(116, 360, 961, 825). The grown block's free kept for the hold adds about 270 MiB in the
fourth phase, through the second mechanism.

## 2. Slack inside occupied slots

**In the code.** `Back::alloc_at` claims a free slot and calls `region::settle_slot(.., size)`,
which counts every page of the slot whose clean bit is clear and clears the clean bits only
under the new block's extent. `alloc_at` then takes that whole count off the retained tally
(`unretain(settled.resident)`). So the pages past the extent that an earlier tenant wrote are
resident, in no tally, and beyond every purge, because a purge claims the slot first. The
slot's next free counts them again (`region::charge`). A block that shrinks in place returns
its tail at once ("nothing else can use it while the block holds the slot"); a smaller tenant
in a slot with a resident tail is the same situation, and nothing returns it.

**Where it comes from**, at the end of the first phase (787 MiB, every page resident):

| last tenant, then the new tenant | MiB | share |
|---|---|---|
| a larger block moved in with room, then a block moved in with room | 601 | 76% |
| anything else (the last tenant not larger, or lost by the side table), mostly then a moved block | 156 | 20% |
| a larger whole allocation, then a whole allocation | 14 | 2% |
| an outgrown slot kept | 7 | 1% |
| a tenant that grew in place | 1 | 0% |

95% of it is under 125 ms old: tenants turn over quickly, and the 800 MiB stands because claims
keep making it (10 GiB of slack found over the first phase, by 47.6k claims).

**What it buys.** Over the run, the slack claims found and how much of it the tenant grew over
before its free:

| tenant | slack past | found | grown into | share |
|---|---|---|---|---|
| whole allocation | its extent | 108 GB | 9 MB | 0.01% |
| moved with room | its extent | 265 GB | 36 GB | 13.7% |
| moved with room | twice its size | 243 GB | 30 GB | 12.2% |
| moved with room | four times | 207 GB | 21 GB | 10.1% |
| moved with room | eight times | 143 GB | 10 GB | 7.2% |

So the slack is warm room the next tenant mostly does not use: a return past twice the size at
the claim would have refaulted an eighth of what it returned.

**The ratchet inside a slot.** The dirty extent is the high-water mark of every tenant since the
slot was last purged. Of the slack claims found, 69 GB of the whole tenants' 108 and 60 GB of
the moved tenants' 265 lay past the previous tenant's own final size: left by tenants before
it. With one new buffer in a hundred ten times larger (`big_permille=10`), that grows to 101 GB,
the first phase's slack to 1,300 MiB, and main's first-phase ratio climbs across the cycles,
2.48, 2.80, 2.97, 3.04, and stays at 2.15 after the quiet minute, since nothing returns slack
in an occupied slot even in a silence.

## 3. Free slots in regions that are never quiet

**In the code.** A free that leaves a large slot resident stamps its region
(`Back::hold_stamp`). The look returns a region's free slots only if its stamp is a hold old
(`hold::hold_region`, `held_for`). Under steady churn every region is freed into within any
hold, so no region qualifies, and a slot idle for minutes stays resident beside one freed a
millisecond ago.

**Was a per-slot clock tried?** No. `large-hold.md` tried hold lengths and one adaptive hold
per class (doubling on a claim soon after the hold purged, halving on the clock), both with the
region's stamp. `hold-credit.md` held every region longer and made growth evict, and found its
first version failing for exactly this reason ("any free into a region restamps it, so every
free slot in it counts as young"); it did not age slots.

**Per-slot ageing.** The memstat breakdowns, free large slots resident at the ends of the phases
(MiB):

| | 1 | 2 | 3 | 4 | after the quiet minute |
|---|---|---|---|---|---|
| main | 135 | 400 | 959 | 1,091 | 22 |
| per-slot ageing | 101 | 317 | 499 | 535 | 24 |

What stays is within a hold's worth of churn: in the third phase the cache frees about
15 GB/s, about 1.9 GB in a 125 ms hold (and the hold is longer when heartbeats are rarer than a
fine step), and ageing leaves 500 MiB: slots freed less than a hold ago, many taken again
within it. That is what the hold was built to keep; jemalloc returns every free of 8 MiB or
more at once, which is why it ends the third phase at 1.29. With ageing, the grown block's
free costs a hold's worth of churn and no more. In the first phase ageing does nothing
(2.22 against 2.34): its tenants turn over inside a hold.

**What the walk costs.** In one run, 70k looks at young regions examined 1.9M slots, 27 a look:
a leaf-bitmap word read for each 64 slots and one load of the slot's stamp for each free one.
A look is bounded by the hold's bound of bitmap words; it starts at the region's top, where
idle free slots are, since claims take the lowest.

## 4. The rules

**Memory**, the large mix (memory runs; ratio at the end of each phase of the last cycle, and
after the quiet minute):

| arm | 40-256 KiB | 256 KiB-4 MiB | 4-32 MiB | mixed | quiet |
|---|---|---|---|---|---|
| main | 2.34 | 1.60 | 2.26 | 2.23 | 1.14 |
| trim past 1x | 1.15 | 1.41 | 1.95 | 2.05 | 1.06 |
| trim past 2x | 1.27 | 1.45 | 2.09 | 1.84 | 1.06 |
| trim past 4x | 1.53 | 1.46 | 1.78 | 1.89 | 1.05 |
| trim to the previous tenant | 1.32 | 1.49 | 1.97 | 2.14 | 1.07 |
| bound 64 MiB | 1.34 | 1.47 | 2.06 | 1.77 | 1.06 |
| bound an eighth of live large | 1.28 | 1.44 | 2.10 | 1.77 | 1.10 |
| bound an eighth + ageing | 1.29 | 1.42 | 1.65 | 1.62 | 1.12 |
| per-slot ageing | 2.22 | 1.45 | 1.74 | 1.79 | 1.21 |
| previous tenant + ageing | 1.33 | 1.40 | 1.53 | 1.77 | 1.11 |
| previous tenant + bound 64 MiB + ageing | 1.19 | 1.26 | 1.52 | 1.64 | 1.06 |
| jemalloc | 1.42 | 1.89 | 1.29 | 1.36 | 1.09 |
| glibc | 1.50 | 1.83 | 2.59 | 2.45 | 2.39 |

The big-buffer variant (one new buffer in a hundred ten times larger), first phase across the
four cycles: main 2.48, 2.80, 2.97, 3.04 (quiet 2.15); previous tenant 1.40, 1.54, 1.55, 1.45
(1.18); previous tenant + ageing 1.39, 1.37, 1.38, 1.41 (1.15); bound 64 MiB 1.26, 1.31, 1.36,
1.32 (1.04); bound an eighth of live large 1.30, 1.31, 1.33, 1.35 (1.08). Every rule that
returns slack at the claim stops the climb; the trim to the previous tenant does it with no
bound at all.

**Time**, the large mix, timed (M cycles per operation, all threads, two rounds; page faults
over the run):

| arm | Mcycles/op | against main | faults |
|---|---|---|---|
| main | 8.22, 8.26 | 1.000 | 9.4M, 8.6M |
| per-slot ageing | 8.20, 8.18 | 0.995 | 14.6M, 14.8M |
| trim to the previous tenant | 8.18, 8.26 | 0.998 | 13.1M, 12.8M |
| trim past 4x | 8.24, 8.27 | 1.003 | 27.8M, 27.2M |
| trim past 2x | 8.25, 8.26 | 1.003 | 30.4M, 30.4M |
| trim past 1x | 8.21, 8.31 | 1.003 | 32.2M, 32.2M |
| bound 64 MiB | 8.26, 8.28 | 1.004 | 29.8M, 29.8M |
| previous tenant + bound 64 MiB + ageing | 8.25, 8.28 | 1.004 | 34.9M, 34.5M |
| jemalloc | 13.71, 13.58 | 1.656 | 410M, 407M |
| glibc | 9.99, 10.17 | 1.224 | 62M, 61M |

The job writes every buffer it makes, so its time is the writing, and the faults the trims
add (up to 26M over the run) cost under 1%.

**What a trim costs on a request's path.** One `madvise` of the slot's pages past the cut, and a
pass over the slot's clean words to count them, on the claim. In the combination's run, 329k of
about 700k large claims returned pages, 680 KB each on average. The trim's size is the slot's
dirty tail, which can be the whole slot: a product rule would cut it to the same bounded steps
as every other purge (`policy::hold_beat`), the rest of the tail going back at the heartbeats
that follow, or bound it by the block's own size, as the shrink's tail is today.

**The regression jobs**, in one binary (medians; ranges in the raw data):

| job | main | previous tenant | ageing | previous tenant + ageing |
|---|---|---|---|---|
| 64-Vec to 4 MiB: Mcycles / faults / peak MiB | 731-758 / 19.4k / 326 | 758 / 19.4k / 326 | 754 / 19.6k / 327 | 740 / 19.6k / 327 |
| one buffer to 256 MiB: Mcycles | 367 | 362 | 359 | |
| tpchgen: Gcycles / faults / peak RSS MiB | 24.93 / 221k / 626-634 | 25.13 / 345k / 547-576 | 24.78 / 225k / 592-609 | 25.33 / 354k / 531-561 |
| DataFusion SF3: Gcycles / faults / peak MiB | 69.64 / 1,024k / 864 | 70.06 / 1,056k / 865 | 69.71 / 1,032k / 873 | 70.08 / 1,065k / 873 |
| Meilisearch: Mcycles/req, faults/req, heap mean/peak MiB (two rounds) | 16.78, 16.76; 39.0, 39.6; 713-716 / 748-761 | 16.77, 16.84; 43.2, 43.9; 698-706 / 746 | 16.86, 16.80; 37.4, 35.7; 711-722 / 757-761 | 16.81, 16.11; 44.5, 39.5; 701-707 / 737-753 |
| order service 2 GiB: kcycles/req, heap MiB | 67.2 / 2,178 | 67.3 / 2,178 | 67.5 / 2,178 | 67.5 / 2,178 |

glibc on tpchgen in the same session: 24.97 Gcycles, 292k faults, 515-519 MiB.

- **The 64-Vec job** is level under every arm here. The 64 MiB bound is level too (780 against
  783 Mcycles). The bound at an eighth of live large memory costs 18% (924 Mcycles, three times
  the faults): 64 buffers regrow over and over in slots their predecessors left, a bound of
  32 MiB trims their room at each claim, and they fault it back.
- **tpchgen** is the one job where the trim to the previous tenant costs: about 60% more faults
  and 0.8% cycles (1.6% with ageing), for 50 to 90 MiB less peak, now between glibc's and
  main's. Its buffers grow back into what the trim took.
- **DataFusion**: the trim to the previous tenant costs 0.6% cycles and 3% more faults; ageing
  is level. Peaks within 10 MiB, but for one previous-tenant run at 1,468 MiB, the kind of
  outlier jemalloc has also shown on this job.
- **Meilisearch** and the **order service** are level within run-to-run noise.

## 5. The swing job's cycles against tcmalloc

tcmalloc's 17% fewer cycles on the drift study's swing job was one round. Three interleaved
rounds (CPUs 0-3 and 8-11): tcmalloc 3,207, 3,536, 3,231 cycles per operation against
asmalloc's 3,338, 3,567, 3,907; wall time 66, 70, 66 s against 65, 68, 71 s. The gap is 1 to
17% by round, asmalloc's own rounds spread by 17%, and the wall times are level. The job's
consumers spin on an empty queue while producers work, so its cycles count waiting, which
moves with how the threads interleave. It is not a stable difference in the allocator, and I
did not profile it.

## Ranking

1. **Trim to the previous tenant at the claim, and age each free large slot on its own.**
   - *The trim.* A claim of a free large slot returns the pages past both its new block's
     extent and the extent of the slot's previous tenant (the size word still holds its final
     size), as the shrink returns a tail. Its numbers are the previous tenant's size and the
     purge's existing step bound (`policy::hold_beat`): a tail longer than a step goes back a
     step on the claim and the rest at the heartbeats that follow, as other purges do. It
     returns only what tenants before the last one wrote, so it does not cut the room the last
     tenant shows the next one needs.
   - *The age.* The free stamps the slot with the fine count, in its size word, written by the
     holder. The hold's look at a region whose stamp is young walks its free slots from the
     top within the look's existing bound of bitmap words and returns those a hold old. Its
     numbers are the hold's (`HOLD_STEPS`) and the look's bounds, unchanged.
   - Gains: the large mix 2.34, 1.60, 2.26, 2.23 to 1.33, 1.40, 1.53, 1.77, and the quiet tail
     1.14 to 1.11; the climb inside hot slots stopped (big-buffer variant 2.48 to 3.04 against
     1.37 to 1.41); a grown block's free kept for the hold costs a hold's worth of churn.
   - Costs: tpchgen +1.6% cycles and about 60% more faults, with 70 MiB less peak; DataFusion
     +0.6% cycles; level elsewhere. A `madvise` on about half the large claims of the mix, each
     bounded by a step.
2. **Per-slot ageing alone.** Level or better on every regression job, the third and fourth
   phases at 1.74 and 1.79, the first unchanged at 2.22. Choose it if tpchgen's cycles decide.
3. **Not proposed.** A bound on the slack at an eighth of live large memory (the 64-Vec job
   +18%). At 64 MiB it works (1.34 in the first phase, level on the 64-Vec job) but is a
   fixed number, and the trim to the previous tenant does as well without one. Trims past k
   times the size for every claim: a number, three times the faults, and the first phase no
   better than the previous-tenant trim except at k = 1.

Unresolved: whether tpchgen's extra faults come from tenants that are smaller than their
predecessors and then regrow (the trim's one way to cut needed room), which a count of
refaults by the trim's own returns would say; and jemalloc's lead in the 4 to 32 MiB phases,
which is the hold's design, not a defect.

## Reproduce

```
scripts/heavy bench/probes/large-churn/build.sh lc-diag HEAD asmalloc-memstat lc_edit.py
scripts/heavy bench/probes/large-churn/build.sh lcs HEAD asmalloc lcs_edit.py -- workload bench preload datafusion meilisearch orders
HEAP_DRIFT_MEMSTAT=1 scripts/heavy bench/probes/heap-drift/matrix.sh OUT/diag large lc-diag ops=600k phases=4 samples=32 account_every=8
scripts/heavy bench/probes/large-churn/arms.sh OUT/arms lcs large "t0p0b0000a0 t0p1b0000a0 t0p0b0000a1 t0p1b0000a1 ..."
scripts/heavy bench/probes/large-churn/arms.sh OUT/arms-big lcs big "..."
scripts/measure --both-l3 bench/probes/large-churn/timed.sh OUT/timed 1 "ARMS"
scripts/measure python3 -I bench/probes/large-churn/kern.py OUT/kern.jsonl --rounds 3 --jobs 'vecgrow vecs=64 max=4Mi reps=4' --arms 'main=lcs:t0p0b0000a0,...'
scripts/measure --both-l3 bench/probes/large-churn/sessions/tpch.sh OUT/tpch.jsonl 3
```

The app and server sessions need local `bench/allocators.toml` entries (not committed) with
`build = "lcs"` and `env = { ASMALLOC_LC = "<arm>" }`. Raw data:
`docs/research/data/large-churn-2026-10-25.tar.gz`.
