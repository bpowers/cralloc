# Returning memory off the slow paths while the thread runs

2026-10-22, branch `purge-off-path` (`423a7e4` and the fixes in `2327b7f`), measured against
main `3fdec1b` (whose allocator code is today's main's) and jemalloc on the reference host
(Ryzen 9950X, Linux 7.2.9).
The design is "Heartbeat", from "While the thread runs, returning memory is its work".

## Summary

Second round (after the review, main `040033a` merged; "Second round" below):

- **A yield after each of the thread's steps was tried and taken out.** With four workers on
  DataFusion's four CPUs every yield handed a worker a whole slice: the thread waited about
  460 ms a run behind them, its holds and payments came late, and 10 runs of 20 peaked 40 to
  180 MiB over (none without the yields, none on main). On Meilisearch it changed neither the
  tail nor the workers' run-queue wait. Do not try it again without a way past that.
- **Meilisearch's tail is level with main's**: p99.9 at 70% 44.7 ms against 45.8 (main's
  moved from 37 to 41 to 42 to 47 with its floor change), and every thread's run-queue wait
  is the same in every build. The first round's 6 ms did not reproduce.
- **The log store's burst after a silence** takes 0.50 to 0.60 ms for its slowest request in
  the first 5 ms against main's 0.68 to 0.93: the pass is off request paths. The idle drains
  are what is left, about 0.2 ms over the thread-off build.
- The doubling and DataFusion hold: 0.780 / 0.827 / 0.916 ms against main's 0.779 / 0.852 /
  1.103; cgroup peak 867 against 868 MiB.

First round:

- **The doubling tail is gone at p90 and mostly at the maximum.** A table doubling of the
  order service takes 0.770 / 0.794 / 0.889 ms (median, p90, max of 192) against main's
  0.766 / 0.839 / 0.905 and jemalloc's 0.737 / 0.828 / 0.860. The p90 is now under
  jemalloc's; the slowest doubling, 30 us over jemalloc's, had three faults more than the rest
  and no allocator work in it.
- **DataFusion's peak is unchanged, after one fix.** The cgroup peak is 864 to 866 MiB
  against main's 860 to 876 and jemalloc's 833 to 872, with 0.1% fewer cycles than main
  (69.66 against 69.76 G) and the same kernel cycles. The first build peaked 180 MiB higher
  in about one run in five; see "What DataFusion found".
- **Total work does not grow.** The same `madvise` calls move to the thread: DataFusion's
  kernel cycles are 5.98 G against main's 6.02, and the doubling server's CPU time over its
  run 1% less than main's.
- **On the large-heap servers the work moves and the tails do not improve.** The log store is
  unchanged (it returns almost nothing). On Meilisearch the thread now does about 9 ms of
  work a second at 70% load and wakes 21 times a second instead of 2; cycles a request are
  the same (17.04 M against 17.02), but page faults a request are 7% higher (90 against
  85, ranges overlapping) and p99.9 at 70% is 46 ms against 40 (39 to 52 against 37 to 41
  over six runs). The burst after the silence and memory at rest are unchanged. The thread
  runs on the server's own CPUs, and when it takes a pass back to back it holds one of them
  for milliseconds; see "Limits".

## What changed

While the thread is awake or parked (`thread_word::Duty`), a slow path's heartbeat takes no
purge step. It keeps the readings the clocked rules count time by (the interval's number,
the hold's fine count) and starts an interval's pass as before, and if a reading made work
due that the thread does, it looks at the thread's word afresh and, if the thread sleeps,
wakes it. The thread takes the same bounded steps a heartbeat takes (`Back::purge_steps`)
until none would do more, between the steps of its round and after it.

- **The word.** A new bit, `SLEEPING`, outside the slow path's mask. The thread sets it
  before a timed wait and then looks once more for work; a waker clears it with a
  compare-and-swap and makes the futex call. Each side writes before it reads, so either the
  thread finds the work or the waker finds the bit. A slow path under steady churn finds the
  bit clear and writes nothing; one load of the word, as before.
- **What the thread foresees, it publishes in counted time.** The hold's next look is a fine
  count in a new word on the hold's line; the heartbeat whose reading steps the count to it
  wakes the thread. A timer could not do this: the fine count moves only with readings, so in
  a silence no hold matures and nothing should wake the thread. A free that stamps a region
  when no look is planned sets one a hold ahead; a crossing of the floor by free large memory
  sets one at once. The next pass is the next counted interval: the heartbeat that starts it
  wakes the thread.
- **Growth** is a debt any slow path adds to. Once a step of growth, a grower wakes the
  thread if retained memory is over the cap, and otherwise drops the debt as a payment under
  the cap would. Past `GROWTH_UNPAID_HUGE_PAGES` (16 MiB) unpaid, growers and heartbeats pay a
  step each, as they do without a thread.
- **A fork child** that inherited a running thread's word treats it as running until work
  first comes due; the cold look then sees the word is another process's, makes it the
  child's, and the slow path does the work.
- Where there is no thread (one thread, `ASMALLOC_BACKGROUND=0`, a failed start, default
  features off) nothing changes.

## Method

Builds: main by `bench/probes/purge-off-path/build-at.sh asmalloc-main main` (main's tree
extracted and built with its own runner, sharing the apps data), the branch by `benchrun
build` and `build-apps` in this checkout, jemalloc likewise. Every session under
`scripts/measure --both-l3`, allocators in random order each round.

- Doubling: `bench/probes/doubling-tail/probe.py plain --builds asmalloc-main,asmalloc,je`,
  three sessions of one round; `analyze.py` for the table. The probe now also records the
  server's user and system time over its life (`*.cputime`).
- DataFusion: `benchrun run --suite apps --only "=datafusion tpch sf3 t4"` with the three
  allocators, six rounds; `bench/probes/purge-off-path/apps.py` for the table (cycles of all
  threads over the queries, from the run's counters, split into user and kernel).
- The diagnosis below: `bench/probes/purge-off-path/dftrace.py` runs DataFusion as the suite
  does and samples, every 5 ms, its RssAnon and the allocator thread's CPU time, run-queue
  wait and voluntary switches; `dfsum.py` and `dfshow.py` read the traces.
- Servers: `bench/probes/purge-off-path/servers.sh`, the `servers-thread` suite with
  default-thread's watcher, three rounds of each job; `probes/default-thread/summarize.py`.

Raw data: `docs/research/data/purge-off-path-2026-10-22.tar.gz` (the doubling probe's
directory, the server sessions, the DataFusion results `df-*.jsonl`, of which `df-final` is
the final build's, `df-1` to `df-3` the first build's and `df-4` the build with a step
between the round's, and the traces `dftrace-1` to `-3`: the
first build, with the two changes that did not help, and with the fix).

## Doubling

`orders axum doubling 4G`, three rounds, 64 doublings a run (192 a build):

| build | median ms | p90 ms | max ms | server user / system s over the run |
|---|---:|---:|---:|---|
| main | 0.766 | 0.839 | 0.905 | 105.9 / 94.0 |
| branch | 0.770 | 0.794 | 0.889 | 103.5 / 94.3 |
| jemalloc | 0.737 | 0.828 | 0.860 | 108.8 / 91.6 |

Every doubling stayed on its CPU (no context switch, CPU time equal to wall time) with 545
faults. Of the branch's 192 doublings two took over 0.84 ms: 0.859 ms, and 0.889 ms with 548
faults. The doubling note's builds on the same job: main 0.803 / 0.901 / 0.984, the no-purge
diagnostic 0.796 / 0.842 / 0.860, jemalloc 0.774 / 0.841 / 0.857; the machine was faster in
this session for every build. The server's CPU time over the run (all threads, 72 s of load)
is 1% under main's: the purge calls moved to the thread, and slow paths stopped paying for
the calls around them.

## DataFusion

TPC-H SF3, four threads on CPUs 0-3, six rounds, the final build (`2327b7f`):

| build | Gcycles | user Gcycles | kernel Gcycles | peak RSS MiB | cgroup peak MiB | faults k |
|---|---:|---:|---:|---:|---:|---:|
| main | 69.76 (69.56 to 69.84) | 63.73 | 6.02 | 914 (908 to 919) | 865 (860 to 876) | 1,050 |
| branch | 69.66 (69.47 to 69.97) | 63.63 | 5.98 | 922 (920 to 922) | 865 (864 to 866) | 1,029 |
| jemalloc | 72.02 (71.95 to 72.20) | 65.87 | 6.17 | 906 (876 to 915) | 863 (833 to 872) | 1,027 |

(Medians, ranges; the cgroup peak also holds the inputs' page cache, which the first run of a
session can add.) The cgroup peak is main's; the process's peak resident set is 8 MiB higher
in every run. The thread wakes about 440 times a run (3.5 s) and runs 89 ms of it. With the
thread, the unpaid limit of 16 MiB is not what holds the peak: the thread pays within its
wake-up, and no run reached the limit by a margin that showed.

## What DataFusion found

The first build peaked at 1,030 to 1,100 MiB in 3 runs of 14, the others at main's level.
`dftrace.py` showed where: in the bad runs, after the pass at the start of the fourth
interval, about 130 MiB more stayed resident and was never returned during the last
queries. Two changes did not remove it (20 runs, two bad): the thread taking a purge step
between the steps of its round, and heartbeats paying growth past the limit. The cause was
the hold's plan. A look stops at its bound of bitmap (256 words), and one that returned
nothing waits for the next fine step, where a heartbeat looks again; the plan counted only
stamps not yet held, so a held region the look had not reached waited a whole hold or more.
Held regions that keep their stamp now set the next look at the next step: 0 bad runs in 20,
with the thread's wake-ups and CPU unchanged (median 437 a run, 89 ms). The two other changes
were kept: the first is the design's "the same bounded steps" in a long round, the second
keeps the bound on retained memory when the thread waits for a CPU behind the threads that
carve (a grower's step alone pays half a megabyte while a carve may owe tens).

## Servers

`servers-thread`, three rounds of the log store and six of Meilisearch.

| | main | branch | jemalloc |
|---|---|---|---|
| Meilisearch: thread wake-ups a second at 70% | 2.02 | 21.2 | |
| Meilisearch: thread CPU ms a second at 70% | 0.0 | 9.2 | |
| Meilisearch: kcycles a request at 70% | 17,019 | 17,043 | 17,430 |
| Meilisearch: p50 / p99 / p99.9 at 70%, ms | 1.8 / 27.5 / 40.0 | 1.8 / 30.3 / 46.2 | 2.0 / 32.3 / 49.1 |
| Meilisearch: p99.9 range, ms | 37 to 41 | 39 to 52 | 46 to 58 |
| Meilisearch: page faults a request at 70% | 84.6 (75 to 100) | 90.2 (84 to 102) | 40.8 |
| Meilisearch: anon at 70%, mean / peak MiB | 868 / 948 | 848 / 939 | 753 / 796 |
| Meilisearch: anon 10 s into the silence, at close + 60 s, MiB | 686, 660 | 687, 658 | 745, 666 |
| Meilisearch: burst, 50 to 500 ms p99, ms | 7.0 | 6.5 | 7.5 |
| Meilisearch: burst first 500 ms, kcycles a request | 11,139 | 10,619 | 10,003 |
| Log store: kcycles a request at 70% | 355 | 351 | 380 |
| Log store: p50 / p99 / p99.9 at 70%, us | 84 / 114 / 123 | 83 / 121 / 124 | 88 / 132 / 137 |
| Log store: burst first 5 ms, p99 / max us | 940 / 943 | 599 / 609 | 387 / 414 |
| Log store: anon at rest, MiB | 1,409 | 1,409 | 1,734 |

The burst's cycles on Meilisearch are bimodal for every build (about 7 M or about 15 M a
request, three runs each way for the branch and for main), by whether the burst meets an
indexing batch. Both threads park as before: last wake-up 22 s into the silence and after
close, none in the last 10 s.

## Second round: after the review, the merge with main, and a yield after each step

2026-10-22, the branch at `7aaee31` (main `040033a` merged; the review of `26cc261` answered)
against main `040033a`, the branch without the yields (`asmalloc-noyield`, the same commit
with `yield_cpu` taken out of `depot/thread.rs` by `build-at.sh`), and jemalloc. The yield:
the thread calls `sched_yield` after each bounded step of its round and of the purges, so that
a program thread waiting for its CPU waits at most a step.

**DataFusion: the yields cost the peak, and were taken out** (they are gone from the
branch's last commit). `dftrace.py`, 20 runs each:

| build | peak RssAnon, runs over 880 MiB | thread run-queue wait a run, median | thread's involuntary switches a run |
|---|---:|---:|---:|
| main | 0 of 20 | 0.6 ms | 0 |
| branch, no yield | 0 of 20 (848 to 860) | 87 ms | 35 |
| branch, yield | 10 of 20 (902 to 1,039) | 462 ms | 390 |

With four workers on four CPUs every yield hands a worker a whole slice before the thread
comes back, and the holds and payments it owes wait behind them, about half a second a run.
benchrun's six rounds (where the branch had one run at 1,049 MiB and main one at 1,022)
agree in median: cgroup peak 867 against main's 868 and jemalloc's 863; Gcycles 69.71
against 69.73 (one build's; kernel 5.88 against 5.84).

**Meilisearch: no difference to explain this session.** Six rounds each, at 70%:

| | main | branch | branch, no yield | jemalloc |
|---|---|---|---|---|
| p50 / p99 / p99.9, ms | 1.76 / 28.6 / 45.8 | 1.78 / 30.9 / 44.7 | 1.75 / 29.3 / 45.4 | 2.03 / 35.7 / 54.3 |
| p99.9 range, ms | 42 to 47 | 44 to 48 | 36 to 53 | 50 to 59 |
| kcycles a request | 16,945 | 16,982 | 16,985 | 17,517 |
| page faults a request | 88.6 | 85.6 | 89.0 | 38.5 |
| thread wake-ups / CPU ms a second | 2.0 / 0.0 | 23.4 / 9.7 | 24.8 / 9.6 | |
| workers' run-queue wait in 2 ms, p99.9 (median of runs) | 41.4 ms | 42.2 ms | 43.0 ms | 44.5 ms |
| thread's CPU in 2 ms, p99 | 0 | 0.32 ms | 0.66 ms | |

Main's p99.9 was 37 to 41 ms in the first round and is 42 to 47 now, with main's floor
change merged; the branch's is level with it either way, and the yield halves the thread's
CPU in any 2 ms at p99 without moving the workers' wait (the per-2-ms wait summed over every
server thread, from `schedtrace.py`, every 2 ms; `schedsum.py`). So the first round's 6 ms
did not reproduce, and these traces do not show the thread delaying the workers.

**Doubling** (three rounds, 192 doublings): branch 0.780 / 0.827 / 0.916 ms (median, p90,
max), main 0.779 / 0.852 / 1.103, jemalloc 0.758 / 0.826 / 0.850.

**The log store's burst after a silence** (diagnostic builds, `default-thread/burst.py`,
three rounds; the slowest request of the first 5 ms): main 0.93, 0.68, 0.89 ms; the branch
0.57, 0.60, 0.50; the branch with the thread off 0.31, 0.98, 0.44. Main's request paths read
5.5k to 6.7k bitmap words of a pass in those 5 ms; the branch's none (the thread reads 20k to
30k). What is left over the thread-off build comes at the burst's first requests, after a
silence in which the thread drained the four slabs (4 drains, as on main): the slow paths
that refill them. The pass is off request paths; the idle drains are what remains.

## Limits

- **The thread shares the program's CPUs.** It is created by a program thread and inherits
  its affinity; a server pinned to four CPUs gets the purge work on those four. A heartbeat's
  step cost the request that took it up to half a millisecond; the thread's pass, taken in
  bounded steps back to back, holds one CPU for milliseconds, and every request queued on the
  worker it displaced waits. Meilisearch's p99.9 at 70% (requests of tens of ms, with
  indexing beside them) is 6 ms worse in median; the doubling's own time and DataFusion are
  better or unchanged. Running the thread at a lower priority, or yielding between its steps,
  would trade that against how promptly it returns memory under saturation: a design question,
  not built.
- **Page faults on Meilisearch are 7% higher in median**, within the runs' spread. A
  candidate: the thread takes an interval's pass to its end as soon as it starts, where
  heartbeats spread it over the interval, so the pass sees less of the interval's frontiers.
  Not measured apart.
- **Wake-ups.** A slow path makes the futex call when work it made due finds the thread
  asleep: once an interval for the pass, once a hold's end or a look the thread planned, once
  a step (2 MiB) of growth over the cap. Meilisearch's 21 a second cost its requests about
  that many system calls.
- **A fork child** of a process whose thread ran leaves its work to that thread until work
  first comes due, then takes it back; a grower in it wakes nobody (the back end cannot tell
  generations) and pays once 16 MiB is unpaid.
- **A program that keeps no heartbeat going** (all on fast paths) wakes the thread only by its
  rounds, as before.
