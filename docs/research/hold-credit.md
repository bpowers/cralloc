# Free large memory held on credit: a diagnostic

2026-10-22, branch `adaptive-hold`, on main `f9ee162`. A hand-over note. Nothing here is for
merge. The second diagnostic, which the first one's "Next" asked for, is at the end ("The
second diagnostic"); read its summary first.

## The model

Holding aged free memory raises the peak only if resident memory grows somewhere else while
it is held. So let the hold be long for everyone, and make growth evict:

- free memory younger than the base hold (125 ms) is left alone by the hold, as now;
- free memory between the base and a ceiling (2 s to start) is held on credit: any resident
  growth (a carve of fresh pages, or a claim that faults back returned pages) owes as much,
  paid from free memory elsewhere;
- past the ceiling, the hold returns it unconditionally, as it does now at 125 ms.

If that holds, DataFusion keeps main's peak (growth evicts what the last query left) with fewer
faults, and Meilisearch, whose indexer takes its blocks back within 2 s, stops refaulting.

## The diagnostic

`bench/probes/app-faults/edits/credit.py`, applied to a copy of main
(`bench/probes/app-faults/variant.sh`, or the `appvariant.sh` used for the apps). `HOLD_CEIL`
in the environment sets the ceiling in fine steps (default 1024, about 2 s; 256 for 0.5 s). It:

- holds every region to the ceiling instead of 64 steps (`hold_region`);
- counts the pages under a large claim that were returned (clean) as growth (`settle_slot`
  returns them, `alloc_at` charges them to `Back::grow`);
- pays a growth payment in full, not 128 pages a step (`pay_growth`);
- skips, in the payment's walk, any large-class region whose stamp is younger than the base
  hold (`purge_step` asks `hold_young`);
- carries what a payment could not take on the debt, whatever its walk found (main drops it
  when the walk returns nothing), so that heartbeats pay it as the memory ages.

`edits/growthwalk.py`, applied after it, counts each payment's walk (words, regions, bytes,
what was owed, dry walks) and prints them at exit.

## Results

DataFusion SF3, interleaved, cgroup peak (main 857 to 866 MiB, jemalloc 845 to 876):

| variant | peak MiB | faults | Gcycles |
|---|---:|---:|---:|
| main | 857 to 866 | 1.04M | 69.9 |
| ceiling 2 s, unpaid debt dropped | 1,187 to 1,251 | 807k to 840k | 68.9 (−1.4%) |
| ceiling 2 s, unpaid debt carried | 1,007 to 1,185 | 895k to 908k | 72.5 (+3.7%) |
| ceiling 0.5 s, debt carried | 1,158 to 1,182 | 901k to 912k | 73.0 (+4%) |

Meilisearch at 930 req/s, two rounds each: faults a request 45 to 48 at both ceilings (main 70
to 80, a constant 2 s hold 36 to 37, jemalloc 37 to 41), swing between ten-second pieces 23 to
41 MiB, p99.9 47 to 50 ms (level).

**Why DataFusion's peak does not hold.** In one run with debt carried, 17,300 payments walked,
16,300 of them a whole lap (about 80 regions, 2,000 to 4,000 bitmap words) that found nothing:
0.65 s of CPU, the 3.7%. The hold's age is per region: any free into a region restamps it, so
every free slot in it counts as young. DataFusion's queries run back to back, so at the moment
the next one grows, the regions that hold the last one's free memory have been freed into
within 125 ms, and the skip left the payment nothing to take. Main's payment has no such
rule, which is what holds main's peak. The retained cap was not the limit: retained memory
over the cap was hundreds of MiB.

## Where `pay_growth` differs from the model

- It walks from the shared walk position (`Walk::purge_at`), region by region downward and
  round the heap, with `Scope::All`, within `PURGE_CAP_WORDS` (16,384) words and one lap. It
  has no order by age or coldness.
- It takes small and large classes alike, and ignores the hold's stamps.
- It pays `min(debt, retained − cap)`, and nothing until the debt reaches a growth step (2 MiB).
  The cap is 64 MiB of retained memory, small and large together; under it, growth pays nothing.
- On main a payment returns at most 128 pages; the rest of the debt stays owed for heartbeats
  and later carves, but only if the walk returned something; otherwise the debt is dropped.
- Only carves count as growth on main. Even in the diagnostic, small classes' refaults (a fill
  claiming purged pages, counted as `revived`) are not growth.

## A claim's choice of slot

A claim takes the lowest free slot of its class (`tree::claim`); it does not look at whether a
slot is resident or returned. Purges walk each region from the top down, so returned slots are
usually above resident ones, and lowest-first takes resident slots first within a region. It
does not across regions, and a region whose top was returned and whose bottom slots are live
has only returned slots free. How much of DataFusion's refaulting is a class taking clean
slots while it has dirty ones free was not measured.

## Next

Remove the skip, so the payment takes free memory wherever its walk finds it, as on main, and
keep the rest (ceiling 2 s, refaults count as growth, unbounded payment, debt carried);
better, a first pass over regions older than the base hold and a second over any region while
debt remains. The prediction: the peak returns to main's apart from the 64 MiB cap, and faults
stay below main's. The regret-driven hold (`hold.rs` levels on this branch) is superseded by
this; its measurements are in the commits and in `meilisearch-memory.md`.

## The second diagnostic

2026-10-22, branch `hold-credit`, on main `3fdec1b`, where the background thread is on by
default. Nothing here is for merge.

**Summary.**

- **The peak holds only once every kind of growth pays.** Without the skip, and with refaults
  charged, DataFusion still peaked 80 MiB over main (878 to 957 MiB): a large block that grows
  in place over returned or never-written pages (`realloc`, through `region::cover`) was not
  charged, about 500 MiB a run. Charging it too gives 846 to 930 MiB against main's 845 to 868,
  within the 64 MiB cap of it, as predicted.
- **DataFusion's faults hardly fall** (2%, 1.03M against 1.04M). Its queries run back to back,
  so little free memory ages past the base hold, and whatever pays for growth is memory it takes
  again soon. It faulted back 1.5 to 1.7 GB of large slots a run against 1.7 to 1.9 on main.
- **Meilisearch is at 45 to 46 faults a request** (main 76, jemalloc 38), its swing 20 to 48
  MiB (main 66 to 225). Charging refaults is what keeps it above the constant 2 s hold's 36 to
  37: each charged refault evicts another block the indexer is about to take back. Not charging
  them gives 33 to 40, and DataFusion's peak 1,044 to 1,073 MiB. Both programs cannot be served
  by the same choice here; DataFusion's peak needs the charge.
- **The retained count overcounts large free memory.** A freed large block is counted resident
  for every page its size covered, written or not, so payments partly return pages that were
  never resident. At one point in a DataFusion run the count said 1,133 MiB of free large memory
  was resident in a process with 682 MiB of anonymous memory in all.
- **Choosing resident slots first would save little.** Of the bytes large claims faulted back on
  main, a free slot of the same class with fewer returned pages existed for 10 to 13%
  (DataFusion) and 16% (Meilisearch); one wholly resident, for 3 to 5% and 11%.
- The background thread starts in both programs (DataFusion 0.2 s in, Meilisearch 0.4 s) and
  is kept on. In DataFusion it runs four rounds and does no work; in Meilisearch, 120 rounds.

### The variant

`bench/probes/app-faults/edits/credit2.py`, applied to a copy of main:

- the hold returns a large region's free slots only after `HOLD_CEIL` fine steps (1,024, about
  2 s) without a free into it, instead of 64;
- a claim of a large slot whose pages under the block were returned (or never written) charges
  those pages to the growth debt, as a carve is charged;
- a payment returns all it owes, with no page bound (a stand-in for the background thread), in
  two passes from the shared walk, each at most once round and `PURGE_CAP_WORDS` words: the
  first takes only large-class regions whose stamp is older than the base hold (or that have
  none), the second, only if debt remains, any region, as main's payment does;
- what a payment could not take stays owed for the heartbeats; debt is owed only over the cap,
  as on main (`growth_goal`), so growth under the cap is dropped.

`covergrow.py`, after it, charges growth in place: `region::cover` returns the pages under a
block's new extent whose clean bits it cleared, and its four callers charge them. `norefault.py`
takes the claim's charge out again, to see what it costs. `creditdiag.py` counts (carves,
refaults, growth in place, payments by pass, dry payments, debt dropped, what the hold and the
clocked pass returned, the thread's start and rounds) and writes a series every 2 ms of
anonymous memory against the debt and the retained counts; `slotpref.py` scans, at each large
claim that faults pages back, every free slot of the class for the fewest it would have faulted
back for the same block. Builds: `cr` (credit2), `cr2` (+ covergrow), `cr3` (+ norefault), and
`crd2`, `mnd` (main), `mns` (main + slotpref) with the counters.

### DataFusion

SF3, CPUs 0 to 3, six rounds interleaved in a random order each round, cgroup peak; counts
below are medians of four runs of the counting builds.

| build | peak MiB | faults | Gcycles |
|---|---:|---:|---:|
| main | 845 to 868 | 1.03M to 1.04M | 69.6 to 70.0 |
| cr: claims charged | 878 to 957 | 0.99M | 69.5 |
| cr2: claims and growth in place charged | 846 to 930 | 1.02M to 1.03M | 69.4 to 69.5 |
| cr3: growth in place charged, claims not | 1,044 to 1,073 | 0.97M | 69.2 |
| jemalloc | 851 to 881 (one run 1,457) | 1.03M | 72.1 |

Main and jemalloc ran in three sessions of six rounds, cr2 in two; the ranges cover all of them.

What the counters say (cr and cr2 against main):

- **No payment found nothing.** Dry payments: 0 of about 500 a run. The skip was the whole of
  the first diagnostic's failure to pay.
- **Nothing was owed at the peak.** The debt was 0 to 2 MiB at the sample of most anonymous
  memory in every run, and never above 2 MiB in the variant (main: up to 100 MiB, from its
  bounded payment).
- **Free memory under the cap at the peak** was the cap's worth or more: the large count read
  270 to 330 MiB at the peak in seven variant runs of eight (893 in the eighth; main 250), with small classes' 1 to 10 MiB.
- **Uncharged growth set cr's peak.** In one run anonymous memory rose 444 MiB in 130 ms while
  the payments returned 668 MiB against 658 MiB charged. Growth in place was 495 to 545 MiB a
  run (main and variant alike); charging it took 40 MiB off the median peak.
- **What a payment returns is partly not resident.** In the window before cr2's peak (70 ms
  without a heartbeat), anonymous memory rose 387 MiB while payments returned 494 MiB against
  493 MiB charged. The count of resident free large memory reached 1,133 MiB with 682 MiB of
  anonymous memory in the process. A block's pages are counted from its requested size, so a
  `Vec` with spare capacity, or zeroed memory the program never wrote, is counted resident when
  it is freed, and returning it frees nothing.
- **Small classes' refaults are not material**: 21 to 28 MiB a run, against 2.0 to 2.2 GB
  carved and 1.5 to 1.9 GB faulted back under large claims.
- **The hold and the clocked pass returned nothing in the variant** (main's hold: 1.7 GB a run).
  Everything went back through payments: 2.5 to 2.6 GB a run, 0.5 of it from the first pass
  (regions aged past the base hold) and 2.0 from the second.
- Growth under the cap, dropped: 1.4 to 2.1 GB a run, mostly in the first query and between
  queries while the count was at the cap: growth with nothing to evict.

### Meilisearch

`meilisearch hn` at 930 req/s, the minute at 70%; two rounds each for main, cr2 and jemalloc,
two more for cr2 beside cr3.

| build | faults/req | heap at 70%, MiB | swing | p99.9 ms | Mcycles/req |
|---|---:|---:|---:|---:|---:|
| main | 76.3, 76.5 | 703 to 928 | 66, 225 | 40, 50 | 15.6, 17.1 |
| cr2 | 44.8, 45.1, 46.0, 46.4 | 899 to 957 | 20 to 48 | 46 to 56 | 16.6 to 17.1 |
| cr3 | 39.9, 39.9 | 926 to 969 | 30, 43 | 42, 43 | 16.9 |
| jemalloc | 38.0, 38.2 | 748 to 788 | 22, 28 | 59, 66 | 17.7 |

The counting builds, one run each (GB over the run):

| | main | cr2 | cr3 |
|---|---:|---:|---:|
| faults/req (that run) | 88 | 46 | 33 |
| large slots faulted back under claims | 27.1 | 13.8 | 10.1 |
| returned by the hold | 20.9 | 0.7 | 0.7 |
| returned by the clocked pass | 4.5 | 4.5 | 6.9 |
| returned by growth payments (first pass, second) | 0.2 | 4.3, 2.4 | 0.2, 0.3 |
| growth in place | 0.25 | 0.16 | 0.14 |

With refaults charged the payment takes over the hold's part, mostly from regions aged past the
base hold, but the indexer takes its blocks back within 2 s, so what it returns comes back.

### A claim's choice of slot (`slotpref.py`, main)

| | DataFusion (three runs) | Meilisearch (one run) |
|---|---:|---:|
| large claims, of them faulting pages back | 606k, 4.2k to 4.3k | 340k, 43k |
| bytes faulted back under them | 1.69 to 1.87 GB | 27.1 GB |
| of which a less-returned free slot of the class would have saved | 179 to 237 MiB (10 to 13%) | 4.3 GB (16%) |
| of which a wholly resident free slot existed | 55 to 83 MiB (3 to 5%) | 3.0 GB (11%) |

Most refaulting is a class with no resident free slot, not a wrong choice among its slots. A
preference would need a second bit per large slot ("free and resident", set by a free that
leaves it resident, cleared by a claim or a purge) and a summary over it, searched before the
free bitmap: one more descent of a summary tree per large claim, and a bit write on each large
free and purge.

### Method

The scripts are in `bench/probes/app-faults/credit/`, with this worktree's paths written in.
Each build is a copy of the worktree with the edits applied (`hcbuild.sh`), built into
`bench/target/<tag>/apps/<program>`.
DataFusion runs were made by hand in a cgroup of their own (`scripts/memlimit`, then
`bench/probes/memory/incg.sh` for the cgroup's peak), interleaved, each after 300 ms of
spinning CPUs 0 to 3, under `scripts/measure --both-l3`. Meilisearch ran through `benchrun`
with local `build` entries in `bench/allocators.toml` (not committed), one round of three
builds a session. `HOLD_CEIL=1024` throughout.

## DataFusion after the clocked pass's new floor

2026-10-22. Main `b8ae757` (merged at `d8416c2`) changed the clocked pass: its goal is a
quarter of what the last lap counted as returnable, down to the floor, with no term for what is
over the cap. DataFusion's peak did not move: on main `69e00b7` against `3fdec1b`, six rounds
interleaved with jemalloc, by the method above.

| build | peak MiB | faults | Gcycles |
|---|---:|---:|---:|
| main `3fdec1b` | 854 to 871 | 1.03M to 1.05M | 70.2 to 71.3 |
| main `69e00b7` | 853 to 867 | 1.02M to 1.05M | 70.3 to 70.7 |
| jemalloc | 851 to 881 | 1.02M to 1.03M | 72.2 to 73.4 |

The clocked pass returns nothing in DataFusion either way: its run is three and a half seconds of
queries whose reuse keeps the frontiers high. With `creditdiag.py`, three runs of `69e00b7` and
four of `3fdec1b`: the hold returned 1.66 to 1.77 GB a run (1.68 to 1.79 before), growth
payments 1.03 to 1.16 GB (1.04 to 1.09), the clocked pass nothing in both. Cycles in this session
were about 0.7 G higher for every build than in the sessions above.
