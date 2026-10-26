# Large slots under churn: the trim at the claim and per-slot ageing, as built

2026-10-25, branch `large-slack-rule` on main `0eaf879`, on the reference host (Ryzen 9950X,
Linux 7.2.9, transparent huge pages on `madvise`). This follows `large-churn.md`, which
measured the two rules with a switch in a probe. This note covers the product code: the trim
at the claim, which turned out to need a bound, and per-slot ageing. The switch and counters
used here are a probe of their own (`bench/probes/large-slack/`).

## What matters

- **Both rules are built, and they keep the study's gains.** On the large mix (a 1 GiB cache
  of buffers from 40 KiB to 32 MiB), heap over live at the ends of the last cycle's four
  phases:
  - main: 2.20, 1.55, 1.94, 1.99;
  - the product: 1.30, 1.25, 1.45, 1.48;
  - the study's two rules: 1.33, 1.40, 1.53, 1.77.

  In the big-buffer variant, the first phase no longer climbs across the cycles: 1.36, 1.39,
  1.37, 1.41, against main's 2.50, 2.72, 2.74, 2.75. One run per arm: a phase's ratio moves
  by up to about 0.3 between runs of the same code (main's third phase, and the switch off
  against main on one binary, 1.94 against 2.30), so differences of a tenth or two between
  trimming arms, and "better than the study's", are within noise; main against the product is
  not. Resident memory in free large slots, read from the heap's own breakdowns, is the steadier
  measure (section 4).
- **The trim at once stalls a claim for milliseconds, so the product trims a step.** A claim's
  slack can be most of a slot of tens of megabytes:
  - Returned at once, the 1024-page and larger trims took 0.8 ms on average on pinned cores,
    up to 3 ms. The p99.9 trim was about 1 ms.
  - The product returns the top `PURGE_STEP_PAGES` (128 pages) of the slack at most, the
    bound of every purge on a request's path. Its p99.9 trim is 250 to 500 µs.
  - What it leaves stays in the occupied slot, to the next claim's trim or the free.
  - Against the trim at once, the step cost 0.02 to 0.06 of a phase's ratio, one run each:
    within the noise above.
- **Time is level** on the large mix (8.27 against main's 8.26 Mcycles per op, three rounds),
  the 64-Vec job and the 256 MiB job.
- **Faults rise wherever large slots turn over**: tpchgen +60% (about +2.0% cycles, for
  about 97 MiB less peak, 539 against 636 MiB), DataFusion +8% (+0.8% cycles, 13 MiB less
  peak), Meilisearch +12% a request (+0.6% cycles, level tails and heap). On tpchgen only 12%
  of the pages the trim returned were faulted back by the tenant that trimmed them; the rest of
  the extra faults are later, larger tenants of the same slot faulting back the room an
  earlier, smaller one let go. On the large mix the trimming tenant faulted back 32 to 33% of
  what its trim returned (28 to 29% with the trim at once).
- **A young region is walked once a hold at most** (after the review). Walking whenever some
  slot came of age was a walk at nearly every fine step under steady churn. Once a hold, the
  free large slots resident over the large mix's phases are within a few percent of walking
  at every chance, on the mean, and a tenth more at the peak (section 4).
- **The quiet minute is level with main** over three runs each (main 1.13, 1.15, 1.19; the
  product 1.14, 1.10, 1.15). In both, what stays is the floor's 30 MiB of free large slots and
  about 100 MiB of slack inside occupied slots, which nothing returns in a silence. The 1.19
  against 1.08 of the first runs was run-to-run spread (section 4).
- **Even a 128-page `madvise` is sometimes stalled for 2 to 6 ms** on this job and the order
  service (eight threads
  and the background thread on eight CPUs), whatever its size. That is the call waiting, not
  the trim's work, and other purges on request paths presumably meet it too; it was not
  measured for them.

## Method

- **The rules** are on the branch: the claim's pass over the slot's clean words also finds
  the slack, the pages past both the new block's extent and the last tenant's, from the size
  word the free left (`region::settle_slot`, `keep_from`). The claimer returns the top step of
  it (`Back::trim_slack`). The free stamps the slot with the hold's fine count in its size word
  (`Recorded::freed`, 28 bits). The hold's look at a region whose own stamp is young walks its
  free slots by their own ages (`hold::walk_aged`).
- **The probe** (`lsr.rs`, `lsr_edit.py`) puts each rule behind `ASMALLOC_LSR=t<0-3>a<0|1>`,
  so that every arm of a comparison runs in one binary with an environment of the same length:
  - `t0`: no trim;
  - `t1`: at once;
  - `t2`: a step;
  - `t3`: at most the larger of a step and the block's pages;
  - `a`: ageing off or on.

  The arms were measured before the product took the step; the probe's `t1` now gives the
  product's step. The probe also counts each trim's pages and time (the cycle counter around
  the `madvise`), and at the tenant's departure how many of the trimmed pages it faulted back
  (`mincore`). Those counts are written at exit to the file `ASMALLOC_LSR_OUT` names.
- **A build artifact, avoided.** The first probe build compiled the workload together with
  the RPC servers. Cargo then unified features into a different binary, which took 4% more
  cycles and 35% more faults than main with both rules off. Built alone, as main's reference
  build is (`lsr3`), the switch-off arm matches main: 9.3M against 9.9M faults, 8.25 against
  8.26 Mcycles per op. Every number here is from builds made the same way, except the app and
  server sessions, whose two arms share one binary.
- **Memory runs**: the drift workload's large mix (`shape=large live=1Gi ops=2400k phases=16`,
  a quiet minute), and its big-buffer variant (`big_permille=10`). One run per arm under
  `scripts/heavy` and `scripts/memlimit -m 8G`, one at a time (`arms.sh`); the ratio is the
  last sample of each phase.
- **Timed runs**: the large mix shortened to 240k operations on CPUs 0-3 and 8-11 under
  `scripts/measure --both-l3`, every arm once a round in a random order, three rounds
  (`timed.sh`).
- **Regression jobs**, interleaved under `scripts/measure`:
  - the kernel jobs on CPU 2, three rounds (`kern.py`);
  - tpchgen under the preload library, five rounds (`tpch.py`);
  - the app and server jobs through `benchrun`, with two local allocator entries naming one
    build and the switch off (`t0a0`) or on (`t1a1`).

## 1. The large mix

Memory, one run per arm, ageing on in every trimming arm:

| arm | 40-256 KiB | 256 KiB-4 MiB | 4-32 MiB | mixed | quiet |
|---|---|---|---|---|---|
| main | 2.20 | 1.55 | 1.94 | 1.99 | 1.08 |
| switch off | 2.15 | 1.53 | 2.30 | 2.21 | 1.16 |
| trim at once | 1.28 | 1.20 | 1.39 | 1.42 | 1.12 |
| trim a step (the product) | 1.30 | 1.25 | 1.45 | 1.48 | 1.19 |
| trim max(step, block) | 1.29 | 1.23 | 1.37 | 1.47 | 1.15 |

The big-buffer variant, first phase across the four cycles (then quiet):

| arm | cycle 1 | 2 | 3 | 4 | quiet |
|---|---|---|---|---|---|
| main | 2.50 | 2.72 | 2.74 | 2.75 | 1.16 |
| trim at once | 1.36 | 1.34 | 1.34 | 1.35 | 1.11 |
| trim a step | 1.36 | 1.39 | 1.37 | 1.41 | 1.09 |
| trim max(step, block) | 1.38 | 1.39 | 1.36 | 1.38 | 1.13 |

Every trimming arm does better than the study's combination (1.33, 1.40, 1.53, 1.77), most in
the third and fourth phases, where ageing is what acts. Why was not established. The likely
difference is the walk: the study's walked a young region from its top within one look's
bound and could not reach slots below it. The product's carries on across looks from where it
stopped (`AgeWalk`), and a walk under way has the next caller carry on at once. Ageing alone
was not run on this build, so this is a guess.

Time, three rounds (M cycles per operation, all threads; page faults over the run):

| arm | Mcycles/op | mean | faults (M) |
|---|---|---|---|
| main | 8.30, 8.30, 8.20 | 8.26 | 9.9, 9.8, 9.1 |
| switch off | 8.27, 8.27, 8.21 | 8.25 | 9.3, 9.5, 9.3 |
| trim at once | 8.22, 8.27, 8.19 | 8.23 | 24.2, 24.1, 23.5 |
| trim a step | 8.29, 8.30, 8.21 | 8.27 | 22.9, 24.2, 22.6 |
| trim max(step, block) | 8.17, 8.25, 8.23 | 8.21 | 23.2, 22.9, 23.4 |

The job writes every buffer it makes, so the faults the rules add cost under 1%.

## 2. How long a trim takes

The probe's counts on the timed runs (pinned cores), per run of 240k operations, over the
three rounds:

| arm | trims | mean pages | p99 pages | p99.9 pages | largest | mean time | p99.9 time | 1024+ pages |
|---|---|---|---|---|---|---|---|---|
| at once | 48k | 86 | < 1024 | < 4096 to 8192 | 7,338 to 7,626 pages | 26 to 32 µs | ~1 to 2 ms | 74 to 83 a run, 0.74 to 1.5 ms mean |
| a step | 51k | 59 | < 256 | < 256 | 128 pages | 20 to 23 µs | ~250 to 500 µs | none |
| max(step, block) | 49k | 71 | < 512 | < 1024 | 974 to 993 pages | 23 to 27 µs | ~500 µs | none |

The longest trim of each run took 1.9 to 4.1 ms in every arm. In the step arm it was a
128-page call, which usually takes about 20 µs, so the tail of single calls is the call
waiting, not its size: preemption or contention on the address space's lock, with nine
threads on eight CPUs. On the unpinned memory runs (heavy cores shared with builds), single
calls of under 100 pages took up to 15 to 31 ms in every arm.

The trim at once is what the size explains: its 1024-page and larger trims averaged 0.8 ms
pinned and 3.3 ms unpinned. That is the stall the product removes. A step is what every other
purge on a request's path may do: a heartbeat's step of the clocked pass or of the hold, and a
growth payment. In classes of a huge page or more those pieces are whole huge pages, 512
pages, so that a free slot that kept its advice is not left with a huge page partly returned.
The claim's step stays at 128 pages: clean bits are per page, every large slot but one whose
advice could not be taken back is advised against huge pages, and for such a slot
`purge_part` cuts the range inward to whole huge pages, which leaves nothing of a step.

## 3. The regression jobs

| job | main | switch off | trim a step + ageing (the product) |
|---|---|---|---|
| tpchgen: Gcycles / faults / peak RSS MiB (5 rounds, medians) | 24.80 / 217k / 636 | 24.83 / 212k / 628 | 25.33 / 339k / 539 |
| 64-Vec to 4 MiB: Mcycles, median of 3 / faults / peak MiB | 930 / 19.4k / 328 | 916 / 19.6k / 328 | 933 / 19.6k / 328 |
| one buffer to 256 MiB: Mcycles, median of 3 | 436 | 433 | 425 |

The trim at once on tpchgen: 25.37 Gcycles, 356k faults, 531 MiB. tpchgen's buffers grow back into what the trim took, but mostly not the
tenant's own: of the 600k pages the step's trims returned over five runs, 71k (12%) were
resident again when the tenant that trimmed them left its slot. The rest of the extra faults
are later tenants, larger than the one that let the room go. The kernel jobs ran about 12%
slower in this session than in an earlier one, for every arm alike: compare within the table,
not with other sessions.

The apps and servers, the switch off and on in one binary (`lsr4`, built from the branch with
the step), under `scripts/measure --both-l3`:

| job | rounds | switch off | the product | |
|---|---|---|---|---|
| DataFusion SF3: Gcycles / faults / cgroup peak MiB | 6 | 69.99 / 1,028k / 873 | 70.54 / 1,113k / 860 | +0.8% cycles |
| Meilisearch under load: Mcycles/req / faults/req | 4 | 16.72 / 38.6 | 16.82 / 43.2 | +0.6% |
| Meilisearch: p99 / p99.9 at s70, ms; loaded peak MiB | 4 | 28.7 / 44.2; 3,584 | 28.1 / 43.9; 3,577 | |
| order service, 2 GiB cache: kcycles/req / peak MiB / p99.9 at s70, µs | 2 and 4 | 68.7 / 2,192 / 24 | 68.5 / 2,188 / 24 | |
| `connect log h2`: kcycles/req at s70 / p99.9 µs | 3 | 109.1 / 725 | 108.7 / 724 | no large blocks |

Two of the order service's switch-off runs were marked invalid for other work on the
server's CPUs, so its switch-off arm has two runs. The product's trims on these jobs:
- DataFusion: about 14,700 trims a run, 2 pages on average;
- Meilisearch: about 340 a run, 28 pages on average, the longest 78 µs;
- the order service: about 40,500 a run, 40 pages on average. Its slowest single calls took
  2 to 5.6 ms, all of 128 pages or fewer: the call waiting, as on the large mix. About 1% of
  its trims took over half a millisecond, which the order service's latency percentiles
  measured from s70 do not show.

Of the pages trimmed, DataFusion's tenants faulted back 3 to 4%, Meilisearch's 7 to 8%, and
the order service's 17%.

## 4. After the review: the walk once a hold, free resident memory, and the quiet minute

The review found that the walk planned by when its soonest young slot came of age walked a
young region at nearly every fine step under churn (a third of the steps on a region of 3,000
slots under drifting demand, 864 metadata words a step), and woke the background thread at
every step when slots were freed a step apart. A walk now starts at most once a hold (a step
short, so that the thread's look, which the region's stamp schedules, lands on it), and the
thread plans from region stamps alone, as before the rules. A slot goes back between one hold
and two after its free.

**Free large slots resident**, summed over the large classes from the memstat build's
breakdowns (every other sample, 64 a run) while the workload ran, in MiB:

| build | large mix: mean | max | big-buffer variant: mean | max |
|---|---|---|---|---|
| main | 602, 568, 564 | 1,128, 1,112, 1,157 | 669 | 1,118 |
| the product as reviewed (walk when due) | 277 | 593 | 269 | 643 |
| the product (walk once a hold) | 281, 300, 279 | 673, 664, 657 | 319 | 678 |
| walk twice a hold | 293, 293 | 733, 647 | 351 | 714 |

Once a hold keeps 1 to 8% more on the mean and 11 to 13% more at the peak on the large mix,
19% more on the mean of the big-buffer variant (one run), against walking whenever a slot is
due. A walk twice a hold keeps no less than once a hold (293 on the large mix's mean, 351 on
the big variant's), so the differences between the three ways of walking are the runs' own
spread, about a tenth and up to a fifth on one run. Main keeps twice as much.

Heap over live at every phase end, four cycles (the large mix):

| build | cycle 1 | cycle 2 | cycle 3 | cycle 4 | quiet |
|---|---|---|---|---|---|
| main r1 | 1.94 1.56 1.91 1.95 | 2.17 1.53 1.87 2.01 | 2.18 1.54 1.89 2.08 | 2.34 1.48 2.00 2.16 | 1.13 |
| main r2 | 1.95 1.53 1.88 2.01 | 2.23 1.60 2.01 2.00 | 2.20 1.50 2.02 1.86 | 2.16 1.54 1.98 2.15 | 1.15 |
| main r3 | 1.96 1.42 1.98 2.16 | 2.20 1.47 1.91 2.05 | 2.19 1.51 1.73 2.21 | 2.19 1.55 1.96 1.78 | 1.19 |
| as reviewed | 1.29 1.24 1.42 1.47 | 1.31 1.26 1.47 1.56 | 1.32 1.27 1.54 1.50 | 1.31 1.23 1.42 1.43 | 1.14 |
| once a hold r1 | 1.29 1.20 1.51 1.50 | 1.31 1.31 1.59 1.72 | 1.30 1.27 1.46 1.50 | 1.30 1.20 1.43 1.43 | 1.14 |
| once a hold r2 | 1.30 1.29 1.51 1.77 | 1.32 1.31 1.37 1.66 | 1.30 1.35 1.60 1.57 | 1.30 1.28 1.65 1.59 | 1.10 |
| once a hold r3 | 1.30 1.27 1.62 1.34 | 1.29 1.19 1.48 1.65 | 1.30 1.27 1.68 1.59 | 1.33 1.26 1.45 1.46 | 1.15 |

The big-buffer variant's first phase across the cycles: main 2.47, 2.68, 2.87, 2.69; as
reviewed 1.36, 1.38, 1.38, 1.37; once a hold 1.35, 1.40, 1.39, 1.41.

**The quiet minute.** The first runs' quiet column had the product at 1.19 against main's
1.08. Three runs each of the memstat builds give main 1.13, 1.15, 1.19 and the product 1.14,
1.10, 1.15: the same, within a spread of 0.06 for each. The breakdown at the minute's end says
where the memory over live sits, in both: the free large slots the hold keeps at its floor
(25 to 31 MiB; once 0 for main), a few MiB of free small slots, and about 100 to 120 MiB more
resident in occupied slots than their tenants asked for, which is the slack the trim takes
only at a claim, and a silence has none.

## Reproduce

```
scripts/heavy bench/probes/large-slack/build.sh lsr3 HEAD asmalloc lsr_edit.py -- workload preload
scripts/heavy bench/probes/large-slack/build.sh lsr-main main asmalloc -- workload preload
scripts/heavy bench/probes/large-slack/arms.sh OUT/arms lsr3 large "t0a0 t1a1"
scripts/heavy bench/probes/large-slack/arms.sh OUT/arms lsr-main large "-"
scripts/heavy bench/probes/large-slack/arms.sh OUT/arms-big lsr3 big "t1a1"
scripts/measure --both-l3 bench/probes/large-slack/timed.sh OUT/timed 1 "t0a0 t1a1" lsr3 lsr-main
scripts/measure python3 -I bench/probes/large-slack/kern.py OUT/kern.jsonl --rounds 3 \
  --jobs 'vecgrow vecs=64 max=4Mi reps=4;vecgrow vecs=1 max=256Mi reps=4' \
  --arms 'ref=lsr-main:t1a1,off=lsr3:t0a0,on=lsr3:t1a1'
python3 -I bench/probes/large-slack/summary.py memory OUT/arms
python3 -I bench/probes/large-slack/summary.py timed OUT/timed/r*
python3 -I bench/probes/large-slack/summary.py trims OUT/timed/r*/*.lsr
```

Arm 2 is what the product does now, so the probe built from the branch has `t1` and `t2` give
the step. To measure the trim at once again, drop the step from `Back::trim_slack` in a scratch tree. The app and server
sessions need local `bench/allocators.toml` entries (not committed) with `build = "lsr4"` and
`env = { ASMALLOC_LSR = "t0a0" }` or `"t1a1"`. Raw data: `docs/research/data/large-slack-2026-10-25.tar.gz`.
