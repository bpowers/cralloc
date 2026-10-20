# Burst start: counted intervals, purging by runs, and maintenance from a timer

Measured 2026-10-20 and 21 on branch `burst-start` (final runs at `b2d9edc`) against main at `b17c5e2`, on the reference host
(Ryzen 9950X, Linux 7.2.9). Follows `server-latency.md`, which found that the first
milliseconds after a silence paid for maintenance the silence skipped.

## Summary

- **Counted intervals take the maintenance off the burst start.** In the first 3 ms of eight
  bursts of the axum order service, main made 337 `madvise` and 14 `membarrier` calls; the
  branch makes 8 and 8. The extra waiting a burst start adds falls from 9.5 to 3.9
  request-milliseconds per burst, against jemalloc's 3.5, and the median latency of the
  requests due in a burst's first half millisecond from 151 to 53 us (jemalloc 35).
- **Purging runs of pages cuts calls where the clocked pass makes them.** Free pages were
  being returned one `madvise` a page even when adjacent: 1,281 calls for 289 runs on the
  connect-rust server. A call costs about 2 us in a process whose other threads run, and
  0.05 us a page, so the walk now returns up to 64 adjacent pages a call, and leaves runs
  shorter than 4 pages to one pass in 8 (the call counts below were taken with 8 and 16, the
  first setting, which left too much memory waiting; see "Memory after a peak"). datafusion's calls fall by half (3,900 to 1,900);
  tpchgen's and gix-hours' by 7 to 10%, because nearly all of theirs return large blocks
  (hundreds of kilobytes to megabytes a call), which this does not touch.
- **The price is memory a pass or two late.** On burst-idle (218 MiB freed at random) the
  branch, as finally set, holds 109 MiB three seconds after the free where main holds 68, and
  3.7 MiB at twelve seconds where main holds 4.9. phase is level within 1.6 MiB throughout.
  Single free pages between live ones go back within about eight intervals instead of two.
- **Maintenance during silences gives memory back and, on the order service, costs the next
  burst.** With `maintain` called through the silences (the new `asmalloc-tokio` task, or the
  background thread) the connect-rust server goes from 8.9 to 6.2 MiB after its clients leave,
  at no cost to its bursts; the order service gives back 1 to 2 MiB of 202 (its live data is
  180 MiB) and its burst start costs 20 to 27 request-ms instead of 3.9, because the silence's
  maintenance empties the caches and returns the pages the next burst then refaults. While
  the servers are busy neither costs anything measurable: `maintain` now leaves a heap whose
  heartbeats are running to them.

## Counted intervals

Every rule that acted on the passage of time read it from the cycle counter: the age limit's
stamps, the sweep, the depot's low-water intervals, the idle check's sightings, the purge
frontiers' interval number and the clocked pass. They now read a counted time
(`crates/asmalloc/src/clock.rs`): one word in the back end's state holding the count of
intervals and the cycle counter at its last step. A reading steps the count when an interval
of the cycle counter has passed since the last step, however long ago that was, and returns
the count times the interval plus the ticks since the step, capped short of an interval. So
while readings keep coming the counted time follows the clock, and a silence of any length
moves it by at most one interval. All zero is a valid state; every change is one
compare-and-swap.

Each rule has a test that a long silence after its evidence ages it by at most one
interval, and each of those tests fails with the raw clock in place of the counted time; the
tests that the rules act within their intervals while heartbeats keep coming pass unchanged.
`maintain` now runs one round in each counted interval and returns at once otherwise, so a
timer or a hook may call it freely; its clock read is what counts a silence's intervals.

What the burst start still pays on the branch: the sweep and any age look or drain whose
interval falls due at the first heartbeat after the silence, as it would have one interval
on had the program kept running (8 `membarrier` calls in 8 bursts, one drain each).

## What a purge call costs

`bench/probes/madvise_cost.rs`: one thread faults in a run of pages and returns it with one
`madvise(MADV_DONTNEED)`, 600 times per length; with and without three threads of the same
process looping over their own memory on other CPUs, so that every call must shoot down
their TLBs.

| pages a call | alone: us a call | us a page | with 3 other threads: us a call | us a page | other threads' work lost a call |
|---|---|---|---|---|---|
| 1 | 0.36 | 0.36 | 1.80 | 1.80 | 1.3 laps |
| 2 | 0.27 | 0.14 | 1.63 | 0.82 | 1.4 |
| 4 | 0.36 | 0.09 | 1.80 | 0.45 | 1.7 |
| 8 | 0.65 | 0.08 | 2.15 | 0.27 | 2.1 |
| 16 | 1.12 | 0.07 | 2.83 | 0.18 | 3.3 |
| 64 | 3.27 | 0.05 | 5.56 | 0.09 | 1.8 |
| 512 | 27.4 | 0.05 | 30.3 | 0.06 | |

(A lap is 64 page touches, about 0.3 us.) In a multithreaded process a call costs about
1.7 us before it returns anything, the price of a page about 0.05 us after; a call of one
page costs what 35 pages do. Under a loaded server and strace the calls measured 10 to 13 us.

## Whether the free pages are adjacent

Grouping each burst's `madvise` calls by address (`server-latency.md`'s strace runs):

| | calls | runs of adjacent pages | runs of one page |
|---|---|---|---|
| connect-rust server | 1,281 | 289 | 95 |
| axum order service | 1,010 | 277 | 114 |

So most calls returned a page next to one another call returned: the walk went down a
region a unit at a time, and in the small-page part a unit is one page. A third of the runs
are single pages.

## Purging by runs

The walk now joins the purgeable units below the one it reaches into a run, up to 64 pages
(`PURGE_MAX_RUN_PAGES`), and returns it with one claim and one call; units of the huge-page
part are already whole huge pages and are not joined. The clocked pass takes only runs of at
least 4 pages (`PURGE_MIN_RUN_PAGES`), a quarter of a one-page call's cost a page; one pass in
8 (`PURGE_ANY_RUN_EVERY`) takes runs of any length, with a goal of everything it finds, so a
single free page goes back within about eight intervals and a long run within one. The first
setting, runs of 8 and one pass in 16, left the after-peak tail at twice main's and failed the
preload test that the background thread returns a quiet program's memory (27 MiB of 163 left
after 12 s, against a bound of 20); 4 and 8 pass it. Growth over the cap and `collect` take any run, as before. The pass word
carries which kind of pass it is.

Calls a run, main against the branch with runs of 8 and any-run every 16 (strace -c, two
runs each); with 4 and 8 the burst starts' counts are what the any-run passes add, at most one
pass in eight:

| program | main | branch |
|---|---|---|
| tpchgen | 9,722, 9,672 | 8,764, 8,711 |
| gix-hours | 10,594, 10,690 | 10,051, 9,710 |
| datafusion | 3,648, 4,164 | 1,938, 1,889 |
| axum order service, first 3 ms of 8 bursts | 337 | 8 |
| connect-rust server, a run of 8 bursts | 443 (58 runs) | 43 |

tpchgen's calls are nearly all large blocks: 1.3 MiB, 1 MiB, 640 KiB, 48 KiB at a time, from
the large path's frees and moves, which this does not change.

## Memory after a peak

Anonymous resident MiB after the peak, sampled every 250 ms, median of three runs per build
(five for burst-idle), main and branch interleaved; burst-idle watched for 24 s instead of
the usual 12. `var48` is a build with runs of 4 pages and any-run passes every 8th, for
comparison (two runs). var48 is what the branch now ships.

| burst-idle (218 MiB freed) | 1 s | 2 s | 3 s | 4 s | 6 s | 8 s | 12 s | 16 s | 20 s |
|---|---|---|---|---|---|---|---|---|---|
| main | 215 | 109 | 68 | 51 | 30 | 15.4 | 4.9 | 2.8 | 2.8 |
| branch, first setting (8, 16) | 215 | 140 | 122 | 68 | 36 | 19.5 | 9.0 | 9.0 | 2.8 |
| branch as shipped (4, 8) | 215 | 132 | 109 | 68 | 36 | 19.8 | 3.7 | 3.7 | 3.7 |

| phase (eight CPUs, then one) | 1 s | 2 s | 3 s | 4 s | 6 s | 8 s | 12 s |
|---|---|---|---|---|---|---|---|
| main | 51.6 | 41.9 | 31.4 | 27.4 | 17.6 | 11.3 | 11.3 |
| branch | 51.1 | 42.8 | 33.0 | 27.3 | 17.9 | 11.2 | 11.2 |

With the first setting the tail (9 MiB from 12 to 16 s) was the short runs waiting for the
any-run pass; with 4 and 8 it goes at the eighth interval. The lag at 2 to 4 s is not the short
runs (both settings show it) and is not explained; it is a pass late, and gone by 6 s. phase
was measured with the first setting only.

## Maintenance during silences: nothing, the thread, the task

`asmalloc-tokio::spawn_maintenance` (a task calling `maintain` every
`Asmalloc::MAINTAIN_EVERY`) and the background thread (`ORDERS_MAINTAIN` /
`BENCH_MAINTAIN` = `task`, `thread`) against nothing, on this branch with asmalloc linked
into the server; main's build and jemalloc beside. Steady 90k and 215k req/s (axum; 63k and
146k for the connect-rust server), then six bursts of 1 s at the lower rate after 5 s of
silence, then the clients leave for 10 s. Three rounds for axum, two for the RPC server,
interleaved.

| axum order service | burst start, extra request-ms (median [range]) | first 0.5 ms p50 us | kcycles/request at 90k | resident during silences / 10 s after the clients leave, MiB |
|---|---|---|---|---|
| main | 9.5 [7.8 to 16.8] | 151 | 70.4 | 202.2 / 202.1 |
| branch, nothing | 3.9 [3.8 to 6.3] | 53 | 69.4 | 202.2 / 202.3 |
| branch, thread | 27.2 [23.7 to 30.3] | 487 | 69.4 | 202.0 / 201.0 |
| branch, task | 20.4 [14.3 to 23.0] | 377 | 69.8 | 201.4 / 200.4 |
| jemalloc | 3.5 [3.4 to 3.6] | 35 | 69.2 | 204.0 / 204.0 |

| connect-rust server | burst start, extra request-ms | first 0.5 ms p50 us | kcycles/request at 63k | resident during silences / 10 s after |
|---|---|---|---|---|
| branch, nothing | 3.9 | 107 | 103.4 | 8.8 / 8.9 |
| branch, thread | 3.4 | 90 | 100.9 | 8.4 / 6.2 |
| branch, task | 3.0 | 64 | 101.4 | 8.4 / 6.2 |

Cycles per request at a fixed rate differ by under 1% on axum and 2% on the RPC server, inside
the rounds' spread: the task and the thread cost a busy server nothing measurable. That needed
a change: as first written, `maintain` ran one round every interval whatever the heartbeats
were doing, and a timer then cost the busy axum server 2.5% more cycles a request. Now a
heartbeat notes the counted interval it ran in, and `maintain` returns at once if one ran in
this interval or the last.

What the order service's bursts pay for: with the task its eight bursts took 480 page faults
against 100 with nothing, and strace shows the work moved from the bursts (none in their first
3 ms) to the silences (212 `madvise` and 32 `membarrier` calls in five). Those drains and purges
return the caches and pages of the burst before, and the next burst takes them back from the
bitmaps and the kernel. On the RPC server, with a heap of 9 MiB, that costs nothing visible;
on the order service, whose bursts allocate across many classes and CPUs, it costs more than the
silence saves (1 to 2 MiB). jemalloc, which does nothing during silences, keeps everything.

The thread and fork: the thread calls `maintain` every `MAINTAIN_EVERY`, which reads the
counted clock and so steps the count through a silence; the child of a fork has neither the
thread nor a runtime's task, and its count, a copy of the parent's word, goes on from where the
parent's stood (`crates/asmalloc/src/heap/background.rs`, `crates/asmalloc-tokio`).

## Options

- **Keep counted intervals and runs as built.** Together they bring the order service's burst
  start to jemalloc's within 0.4 request-ms and cut the clocked pass's calls by an order of
  magnitude where it makes them (axum 337 to 8 in the burst starts; the connect-rust server
  443 to 43 a run), for memory that comes back a pass or two later after a peak.
- **Find the pass that is late at 2 to 4 s after a peak** (109 against main's 68 MiB at 3 s
  on burst-idle) before tuning the run lengths further.
- **Make maintenance during silences gentler before recommending it for servers.** As built,
  the task and the thread empty a busy server's caches in the second interval of each
  silence, which on the order service makes the next burst start six times dearer for 1 to 2
  MiB. Options: have `maintain` drain a quiet slab only after several counted intervals of
  silence rather than one; or have its drains park in the depot instead of laundering, so a
  burst refills from magazines rather than the bitmaps, and leave the memory to the purge.
  Each needs measuring on both servers; neither changes the busy case.
- **The design's "Heartbeat" section** should say that `maintain` leaves a heap whose
  heartbeats ran in this interval or the last to them, as built.
- **tpchgen's and gix-hours' `madvise` calls** come from the large path (blocks of 48 KiB to
  1.5 MiB freed or moved), which this work does not touch; the large-slot hold being built
  elsewhere (`large-hold`) is where they would fall.
