# Purge steps bounded whatever the unit

2026-10-21, branch `bounded-purge`, on main `8aec365`. No purge step a request's slow path
takes now returns more than a bound, whatever the slot sizes. On DataFusion (TPC-H SF3, four
threads) the longest heartbeat fell from 1.0 to 3.0 ms to 0.3 to 0.5 ms, and the longest growth
payment from 1.5 to 3.4 ms to 0.4 ms in most runs. The peak rose by about 20 MiB, to about
jemalloc's.

Why: a server at 25k requests a second on a worker delays about 25 requests for each
millisecond one of them spends returning memory, so a stall like that once a second is its
p99.9.

## What changed

- **A large slot goes back in pieces.** A unit of the small-page part is the pages of the slots
  it covers, so one large slot was one unit, and a step took it whole: a look of the hold
  returned one 34 MiB slot in about a millisecond (`large-hold.md`). A step that may return
  fewer pages than a unit holds now takes the unit's top part, claiming and releasing the slot
  as a whole purge does. The region's cursor then names the page the piece began at, and the
  next step's unit is the rest of the slot below it. The retained count drops by what each
  piece cleaned.
- **Zero.** A piece sets the slot's zero bit only if it leaves every page of the slot clean, so
  a slot taken while partly returned is not handed out as zero: it gets the returned pages as
  zero and the rest as its last tenant left them. A free slot whose pages are all clean reads
  as zero because every clean bit of a slot is true while it is held (`region::cover`), and
  nothing writes a free one.
- **The heartbeat's step of the clocked pass** returns at most `PURGE_STEP_PAGES`, 128 pages
  (0.5 MiB). It had no page cap.
- **The hold's look** keeps its 2 MiB, now true for every unit.
- **A growth payment** returns at most `PURGE_STEP_PAGES`. A carve of one large slot could owe,
  and pay at once, tens of megabytes. What a payment cannot take stays owed, and heartbeats pay
  it a step each, outside a clocked pass. A clocked pass under way counts what is over the cap
  already, and paying both took the count below it.
- `maintain`'s steps were bounded already (128 pages). `collect` and `malloc_trim` are not
  bounded, and take a slot whole.

## Results

DataFusion, five rounds interleaved, before (main `b23b252`) against after. Both builds time
every heartbeat and every growth payment (`bench/probes/app-faults/edits/steptime.py`). Counts
are for all five runs together.

| | before | after |
|---|---:|---:|
| cgroup peak MiB | 842 to 864 | 863 to 870 |
| Gcycles (median) | 69.90 | 70.17 |
| faults (median) | 1.047M | 1.035M |
| longest heartbeat, a run | 0.98 to 3.0 ms | 0.29 to 0.51 ms |
| heartbeats of 0.24 ms or more / of 0.49 ms or more / of 0.97 ms or more | 95 / 49 / 11 | 21 / 1 / 0 |
| longest growth payment, a run | 1.5 to 3.4 ms | 0.38 to 1.6 ms |
| growth payments of 0.49 ms or more / of 0.97 ms or more | 115 / 62 | 4 / 2 |

jemalloc's peak in the same week was 851 to 883 MiB, so the hold's 0.97 of it is now about
0.99. The bound on the growth payment is what costs the peak. Variants of the after build,
four rounds each:

| growth payment | peak MiB | longest growth payment |
|---|---:|---:|
| not bounded | 845 to 867 | 1.7 to 2.5 ms |
| 2 MiB, the rest left for the next carve | 872 to 909 | 0.2 to 0.5 ms |
| 0.5 MiB, the rest left for the next carve | 870 to 940 | 0.35 to 0.64 ms |
| 0.5 MiB, the rest paid by heartbeats (as built) | 863 to 870 | 0.38 to 1.6 ms |

The remaining long steps are the hold's 2 MiB looks at about 80 us of `madvise` each. The
growth payment's walk is still bounded by `PURGE_CAP_WORDS`, 16,384 bitmap words. Steps past
half a millisecond are rare enough that preemption, with four threads on four CPUs, may
account for them.

**The order service at 2 GiB** (`orders axum cache 2G`, servers-large, two rounds each): no
heartbeat or growth payment on a slow path took more than half a microsecond, before or after.
Its free memory never goes over the floors at s70, so nothing is purged on its slow paths.
Server cycles a request were 68.1k to 68.5k before and 67.9k to 68.0k after.

## Method

`edits/steptime.py` wraps `Back::heartbeat_with` and `Back::pay_growth` with the cycle counter
and prints a histogram at exit. DataFusion runs were made by hand in cgroups of their own
(`dfrun.sh`), under `scripts/measure --both-l3`. The order service ran through `benchrun` with
`build` entries for both, and the histogram was read from each run's stderr tail.
