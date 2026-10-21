# A hold on free large slots

2026-10-20/21, branch `large-hold`, on main `cab8409` (burst-start merged). Free large slots go
back to the OS once their region has had no free that left a slot resident for a hold of 64
fine steps, about 125 ms in a program that beats every 2 ms, down to the allowance's floor of
32 MiB of free large memory. On DataFusion (TPC-H SF3, four threads) the cgroup peak falls from
about 1,200 MiB to about 845, under jemalloc's 870, for 12% more page faults and 0.4 to 0.9% more
cycles.

Why: memory freed in one large class serves another only by way of the OS, and a program whose
phases use different large classes held every phase's free slots on top of the next phase's
live ones until the clocked pass, a second's step, returned them (`memory.md`, "DataFusion's
peak against jemalloc's").

## The rule as built

- **Stamp.** A free of a large slot that leaves it resident (a plain free; the old slot of a
  move kept within the budget for grown blocks; a move whose purge failed) writes the fine
  count into its region's stamp, a word in the header line of the region's metadata block,
  only when the stamp differs. A free that returns the slot first (a grown block past its
  budget, a move's old slot) writes nothing. Small classes have no stamp.
- **Time.** A fine step is 2^23 ticks, about 2 ms. The fine count steps by one when a reading of
  the counted clock finds a step's ticks or more since its last step, however many more, so a
  silence ages a hold by one step. The readings are the heartbeat's own (the clocked pass's
  step), and `maintain`'s. A reading less than a step behind the count's time writes nothing.
- **Look.** The first heartbeat to see the fine count move takes the look (one compare-and-swap).
  It reads the large retained tally; at or under the floor (`policy::allowance_floor`, the one
  the clocked pass and the background thread use) it does nothing more. Over it, it walks the
  large classes' regions from where the last look stopped, on each region's own purge cursor,
  and purges the free units of regions whose stamp is at least 64 steps old (by wrapping
  difference; a stamp up to a hold ahead is fresh). It stops at the floor, or after 2 MiB
  (checked between units, and a large class's unit can be a whole slot) or 256 bitmap words. A
  look that stopped at a bound after returning something leaves the next heartbeat to carry on
  at once; one that returned nothing waits for the next step. A region walked from top to
  bottom loses its stamp.
- **Maintain.** Each round of `maintain` takes looks before its clocked pass, a step at a time
  within the clocked pass's step bounds (64 words, 128 pages). Its hold is the smaller of 64
  steps and the steps since its own last look plus one: in a silence, where its own readings
  step the count once a round, that is a stamp from before its last round; where heartbeats
  step the count, it is the hold.

No mark, no `due` word: nothing to strand, and nothing to stay armed. All of it is hints; a
purge is a claim, so a wrong stamp or position returns a slot early or late, never one in use.

## How long a hold

From the first version (stamps at the free on the prototype clock), DataFusion, cgroup peak,
five rounds interleaved:

| hold | peak MiB | faults | time s |
|---|---:|---:|---:|
| none | 1,204 to 1,252, one run 913 | 0.90M to 0.96M | 3.44 to 3.50 |
| 16 ms | 828 to 883 | 1.14M | 3.51 to 3.63 |
| 60 ms | 845 to 862 | 1.08M to 1.10M | 3.50 to 3.68 |
| 125 ms | 841 to 843 | 1.04M to 1.06M | 3.46 to 3.53 |
| 250 ms | 1,070 to 1,081 | 0.95M to 0.98M | 3.46 to 3.57 |
| per class, adapting | 819 to 871 | 1.18M to 1.19M | 3.56 to 3.59 |
| jemalloc | 849 to 879 | 1.02M to 1.04M | 3.61 to 3.64 |

The best constant is about 125 ms. **An adapting hold did not beat it** (per class, doubling
when a slot was claimed within four holds of the hold's purge of its region, halving after
four holds without): the same peak for 9% more faults than the 60 ms constant and 1.5% more
time. **Populating a moved block's destination** (`MADV_POPULATE_WRITE`) cut tpchgen's faults
by 6%, left kernel cycles where they were and changed nothing on gix-hours and DataFusion.
Both dropped; `bench/probes/app-faults/edits` keeps the scripts.

## Results

Builds from the branch before the merge with main (`large-hold` at `6b03523`, i.e. burst-start
`f50d725` plus the hold) against burst-start `f50d725` itself; DataFusion and ruff by hand,
each run in a cgroup of its own, five rounds interleaved with jemalloc; the first round of
each was disturbed and is left out of the ranges.

| | base | hold | jemalloc |
|---|---:|---:|---:|
| DataFusion SF3, cgroup peak MiB | 937 to 1,235 | 842 to 851 | 851 to 883 |
| DataFusion, faults | 924k to 953k | 1.031M to 1.046M | 1.015M to 1.034M |
| DataFusion, Gcycles (median) | 69.19 | 69.78 | |
| ruff, Gcycles (median); peak MiB | 22.97; 41 to 46 | 23.11; 41 to 46 | |
| `pipeline t2 sizes=large`, ns/alloc (9 rounds) | 60.09 | 60.35 | |

DataFusion's peak is 0.97 of jemalloc's (the base's is 1.08 to 1.42). Its extra cycles are
the faults that take returned memory back: the same build with the hold switched off
(`edits/nohold.py`, which keeps the code and the stamps) ran 69.09 Gcycles against the base's
69.06 and the hold's 69.76. ruff, whose free large memory stays under the floor, ran 22.86,
22.91 and 23.03 in the same session: within its noise of about 0.4%, the hold's slower. The byte bound did not
move it: the first version, which had none, peaked at 843 to 859.

**The longest look.** DataFusion with each heartbeat's look timed (`edits/holdtime.py`, three
runs): about 2,170 looks, 550 of which returned memory; of those, 60% took 8 to 30 us, 95%
under 250 us, and the longest took 0.95 to 1.4 ms. The longest returned one slot of 34 MiB:
the bound is checked between units, and a large slot is one unit. A look that meets a slot
bigger than the bound returns it whole on that request's slow path. The same holds for the
clocked pass's steps.

**The cost on the large pipeline** (+0.4%) is the free's stamp: a load of the fine count and of
the region's stamp. The first rework cost 1.0%, 0.6% of it from readings a little behind the
count's time re-stamping it, which concurrent heartbeats did all the time on a line every
large free reads; such readings now write nothing. With the stamp's load of the fine count
replaced by a constant the job was level with the base.

**Memory after a peak** (`workload quiet`, sampled every 250 ms): neither the base nor the
hold returns anything, since the light work after the free moves too few bytes to beat. The
hold's part of `maintain` covers that case for a program with the background thread or task;
the tests show it returning a quiet program's held slots at its second round.

## Method

`bench/probes/app-faults`: builds of the whole tree (preload library, the suite's workloads and
the apps) per variant, DataFusion and ruff run by hand under `scripts/measure --both-l3` with
`incg.sh` for the cgroup peak, and the backend suite's pipeline job through `benchrun`.
