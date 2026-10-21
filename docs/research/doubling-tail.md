# The doubling tail

2026-10-21, branch `doubling-tail` on main `65d55ac`. The order service's 64 shard tables
double under load (`--suite servers-doubling`, `servers-large.md`, "Pre-sized tables, and the
doubling as a job"). asmalloc's slowest doublings came from purge steps on the doubling's own
large allocation and free: a heartbeat that ran a step of the clocked pass and a hold look,
140 to 180 us of `madvise` on top of a copy of about 0.8 ms. A build with no purge work on
any path matched jemalloc's p90 and maximum. The fix, now in the design ("Heartbeat"), moves
returning memory off slow paths whenever the background thread runs. It waits on the
thread's rework and is not built yet.

## Method

`bench/probes/doubling-tail/probe.py` runs the job's plan (`_fill` at 248k req/s for 44 s,
then `dbl`) with the suite's placement: server on CPUs 0-3 with idle spinners on their SMT
siblings, loadgen on 8-11, under `scripts/measure --both-l3`, the server capped at 12 GiB by
`scripts/memlimit`. The order service now prints a `REHASH` line per rehash: its start on the
monotonic clock and the cycle counter, its thread, wall and CPU time, faults and context
switches. A 5 ms sampler records the machine's vmstat counters. `analyze.py` lines these up.

For one session a probe build (a temporary patch, kept out of the tree) timed every large
allocation and free, their heartbeats, each `mprotect` and `madvise`, each growth payment and
each hold look over a microsecond into a ring of cycle-counter events, which the probe reads
from the server's memory before it stops. Events on the doubling's CPU inside its window are
the doubling's own: no doubling was preempted or migrated. One session took `perf record`
(cycles every 50k, kernel call chains) over the doublings.

The diagnostic build `asmnopurge` returned from every heartbeat at once and never paid for
growth: no purge anywhere, for comparison only.

Raw data: `docs/research/data/doubling-tail-2026-10-21.tar.gz` (the `REHASH` lines, clocks,
memory maps and the probe build's ring; `analyze.py` reads each directory).

## Results

Two rounds, builds in random order each round, 128 doublings a build:

| Build | median ms | p90 ms | max ms |
|---|---:|---:|---:|
| asmalloc | 0.803 | 0.901 | 0.984 |
| asmalloc, no purge (diagnostic) | 0.796 | 0.842 | 0.860 |
| jemalloc | 0.774 | 0.841 | 0.857 |

- **Every doubling is on its CPU throughout**, for every build: CPU time equals wall time to
  a few microseconds, with no context switches, and 545 faults each. So nothing waits on a
  lock in the kernel or behind another thread's `mprotect`.
- **The slow ones are our purge work.** In the probe build's session, 13 of 64 doublings had
  more than 20 us of purge work on their own calls, 36 to 180 us. Those 13 had a median of
  0.905 ms against 0.784 for the rest. In each, one of the doubling's two large calls was the
  one in sixteen that beats. That heartbeat took a step of the clocked pass (one or two
  `madvise`, about 0.5 MiB) and then a hold look, returning 2 MiB in four to six `madvise`
  calls of 20 to 30 us. What the look returns is mostly the old tables other shards had
  just freed: 64 tables of 1.1 MB are over the 32 MiB floor once the 125 ms hold expires. A
  growth payment on the carve added up to 80 us in some doublings.
- **jemalloc does none of this in the window**: it keeps the freed tables dirty and returns
  nothing during the minute.

## What was wrong

- **`mprotect` is not the cause.** A doubling run makes 8 commits in the class's region, all
  in the doublings, of 9 to 14 us each. The 174 and 337 us measured earlier were under
  strace, which stops the process at every traced call. Committing further ahead would save
  under 100 us over the run.
- **Direct compaction is not needed to explain the gap.** None of these sessions had a direct
  compaction within 5 ms of any doubling, and the gap was there. The 1.43 ms worst case of the
  earlier runs, which had 298 compactions, did not recur: the worst here was 0.98 ms. Whether
  compaction caused it then is not known.
- **Not each fault costing more, at least not measurably.** Under perf, kernel fault-path
  samples were 15 a doubling under asmalloc against 11 under jemalloc, but user-space rehash
  samples were 15 against 18, and the totals differ by 6%. One run each is too few samples to
  tell real from noise.

## What is open

The median. Here asmalloc is 2 to 4% over jemalloc, not the 10% of the earlier runs. Every
build was 0.15 to 0.2 ms slower in these sessions than in those, so the machine's state
probably counts; that is unverified. Removing purge work closes about a third of the median
gap (0.803 to 0.796 against 0.774). The rest is inside the copy and its faults, not our
code: the allocator's own calls take about 2 us at the median. Run-to-run spread is as large
as the gap (jemalloc's two runs: 0.762 and 0.781; the no-purge build's: 0.807 and 0.788).
Untested candidates: the process has 236 memory mappings under asmalloc against 57 under
jemalloc, which a fault's lookup walks, and the old table's pages being returned by other
threads during the copy. Left for now: 2 to 4% on a rare event, inside the spread.

## The remedy, as approved

In the design under "Heartbeat": while the background thread runs, returning memory is its
work. A slow path that finds a matured hold, growth to pay for or an interval's pass due
records it and wakes the thread if it sleeps. The thread takes the same bounded steps until
nothing is due, and sleeps no longer than the next piece of work has to wait, so a hold
still matures within milliseconds. What goes back, and when, is unchanged. Without the
thread (one thread, turned off, not created) slow paths keep the work, which is why the steps
stay bounded. A carve still pays a step itself when growth has gone unpaid past a limit, so
the bound on retained memory does not rest on the scheduler.

It removes what the no-purge build shows is the whole tail, for any request that lands on a
heartbeat, not only a doubling. Costs: a futex wake on a slow path when work becomes due; the
purge's CPU time moves to the thread, which shares the workers' cores; growth is paid a few
milliseconds later, which DataFusion's peak must be checked against (`bounded-purge.md`
found it sensitive to late payment). Peak memory during a doubling should not change, since
the old and new tables are both live during a rehash under any allocator. Not measured until
it is built.

## Rejected

- **Huge pages for the new table.** Two faults instead of 545, but the machine runs THP
  `defrag=madvise`, so a fault in a region advised for huge pages compacts memory directly
  when none is free. That is mimalloc's doubling here: 22 ms at the worst
  (`servers-large.md`). Whether a large block will be written densely is also unknown when
  it is allocated in one call (design: "Huge pages only where objects are dense").
- **Populating the new block** (`MADV_POPULATE_WRITE`). It saves the trap per fault, not the
  work. On tpchgen it cut faults by 6% and left kernel cycles where they were
  (`large-hold.md`). It also cannot know the block will be written.
- **Committing further ahead.** 8 calls of about 10 us a run; nothing to win.

## A trap on the way

The first session ran with the server's environment stripped before `scripts/memlimit`,
which then could not reach systemd and fell back to `prlimit --as`. Under a 12 GiB address
space limit asmalloc halved its regions to 2 MiB, and each 2.2 MB table became a run of two
regions, committed with a 4 MiB `mprotect` and beating on every allocation. A different
allocator was measured and nothing said so. That session is discarded. `scripts/memlimit` now
warns loudly when it falls back, and `benchrun` refuses to measure under a finite
`RLIMIT_AS`.
