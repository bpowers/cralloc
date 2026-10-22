# The default background thread on large-heap servers

Measured 2026-10-21, allocator at `929b329` (branch `default-thread-fixes`, which starts the
thread by default once free memory passes the 32 MiB floor and the program has a thread of its
own), on the reference host (Ryzen 9950X, Linux 7.2.9). The same asmalloc binary was run with
the thread on (the default) and off (`ASMALLOC_BACKGROUND=0`), with jemalloc beside them for
scale. Follows `default-thread.md`, whose only server never reached the floor.

## Summary

- **Meilisearch is where the thread pays.** Ten seconds after the load stops its heap is 686
  MiB with the thread against 892 without, and 662 against 878 a minute after the clients
  leave: the thread returns the 216 MiB of free large-class pages that `meilisearch-memory.md`
  found left at rest, and asmalloc ends below jemalloc (669). Work, tails and peak under load
  do not change.
- **The cost is at the start of a burst after a silence.** The thread drains and returns
  memory while the server is quiet, and the burst faults it back. On the log store the first
  5 ms of the burst take about 2 page faults a request instead of 0.2, and the slowest request
  in them takes 0.86 to 0.97 ms against 0.31 to 0.64 with the thread off (0.22 to 0.43 for
  jemalloc); after 5 ms there is no difference. On Meilisearch the first half second of the
  burst takes about 160 faults a request against 75 to 100; its latency there overlaps with
  the thread off (p99 12 to 22 ms against 11 to 24).
- **The thread is cheap and parks.** Under steady load it wakes twice a second (its 500 ms
  interval) and uses under 0.1 s of CPU in a two-minute run. It parks 15 s (log store) and 22 s
  (Meilisearch) into a silence, and as long after the clients leave, and does not wake again
  until the next request.
- **The order service never starts it.** Its free memory never reaches the floor: each request
  frees the oldest order as it stores a new one. The thread starts only at shutdown, when the
  store is dropped. With the thread on or off the server is the same program, and measures the
  same.
- On the log store the thread returns 4 MiB of 1.4 GiB at rest; nothing else moves.

## Method

The suite `servers-thread` (`bench/runner/src/suite.rs`) runs three large-heap jobs:
`orders axum cache 2G` and `connect store 1M` from `servers-large`, and `meilisearch hn`. Each
is filled with closed-loop load, then held at its 70% rate for 30 s (`s70`, three ten-second
pieces), then silent for 30 s with the connections open, then one 2 s burst at the same rate,
then the connections close and the server is watched for 60 s. The burst is marked in pieces
(`b5`, `b50`, `b500`, `b2`) so that the requests due in its first 5 ms, 5 to 50 ms and 50 to
500 ms are summarised apart, with the server's cycles and page faults over each.

`bench/probes/default-thread/session.sh` runs one round of one job under `scripts/measure
--both-l3`: benchrun with `asmalloc`, `asmalloc-off` and `jemalloc` in a random order, capped
at 7, 8 and 12 GiB, and `watch.py` beside it on the runner's CPU. `asmalloc-off` is a local
entry in `bench/allocators.toml`, not committed: `feature = "asmalloc"`, `build = "asmalloc"`
(the same binary) and `env = { ASMALLOC_BACKGROUND = "0" }`. The watcher samples every 250 ms
the voluntary and involuntary context switches and CPU time of the thread named `asmalloc`,
and the server cgroup's anonymous memory; a wake-up is a voluntary switch. Its clock starts
at the load generator's start, so its times are early by the generator's start-up, well under
a sample. `summarize.py` makes the tables. Three rounds, as nine sessions with gaps; no run
invalid except that one jemalloc run crossed the runner's limit of other work on the server's
CPUs over the 5 ms piece (0.11 CPUs, half a millisecond), which is kept. The watcher missed the
server of one `asmalloc-off` Meilisearch run (fixed since), so that cell's cgroup memory is from
two runs.

Memory is the anonymous memory (the curve's rollup, once a second; the cgroup's anonymous
memory agrees to 1 MiB). Meilisearch's resident set also holds its mapped LMDB store, about
2.8 GiB, which is not the allocator's. Medians of three, ranges where they matter. Raw
results: `docs/research/data/default-thread-servers-2026-10-21.jsonl.gz` (benchrun) and
`default-thread-servers-watch-2026-10-21.jsonl.gz` (the watcher).

## Meilisearch

| | thread on | thread off | jemalloc |
|---|---|---|---|
| thread | starts during the start's indexing, 23 s before the load | none | |
| wake-ups a second at 70% | 2.0 | | |
| last wake-up after the silence starts / after close | 22 s / 22 s | | |
| server cycles a request at 70% | 17.08M | 17.08M | 17.48M |
| p50 / p99 / p99.9 at 70%, ms | 1.8 / 30 / 43 | 1.8 / 31 / 49 | 2.0 / 34 / 50 |
| page faults a request at 70% | 83 | 81 | 38 |
| heap at 70%, mean / peak, MiB | 857 / 951 | 872 / 956 | 750 / 782 |
| heap 10 s and 29.5 s into the silence, MiB | 686, 683 | 892, 889 | 753, 674 |
| heap 10, 30, 60 s after close, MiB | 663, 662, 662 | 878, 878, 878 | 722, 669, 669 |
| burst, 5 to 50 ms: slowest request, ms | 3.3 | 5.6 | 2.5 |
| burst, 50 to 500 ms: p99, ms | 17.5 (12.4 to 21.9) | 11.6 (10.8 to 24.0) | 3.3 (3.2 to 5.3) |
| burst, 50 to 500 ms: page faults a request | 156 to 164 | 75 to 103 | 12 to 22 |
| burst, first 500 ms: server cycles a request | 15.6M (13.8 to 16.7) | 14.0M (13.1 to 15.9) | 7.8M (7.8 to 8.2) |

The heap falls by about 200 MiB within the first 10 s of the silence, as the thread's rounds
return what the load left free; without the thread nothing returns it. Under load the thread
changes little (15 MiB lower on average): the hold returns large slots there either way, from
the slow paths without the thread and from the thread with it. The burst's first 5 ms hold five requests and are not compared. Every
burst of asmalloc's, with or without the thread, is slower than jemalloc's: the burst comes
with an indexing batch, and asmalloc faults back what it returned before (the hold's faults,
`meilisearch-memory.md`); the thread adds the pages it returned in the silence to those.

## connect store 1M

| | thread on | thread off | jemalloc |
|---|---|---|---|
| thread | starts 1.4 s into the fill | none | |
| wake-ups a second at 70% | 2.0 | | |
| last wake-up after the silence starts / after close | 15 s / 14 s | | |
| server cycles a request at 70% | 351k | 353k | 380k |
| p50 / p99 / p99.9 at 70%, us | 85 / 111 / 119 | 85 / 112 / 120 | 88 / 132 / 136 |
| heap at 70%, mean / peak, MiB | 1,415 / 1,453 | 1,413 / 1,453 | 1,733 / 1,763 |
| heap 10, 30, 60 s after close, MiB | 1,409, 1,409, 1,409 | 1,413, 1,413, 1,413 | 1,732, 1,732, 1,732 |
| burst, first 5 ms (182 requests): slowest, us | 863, 933, 971 | 311, 379, 637 | 222, 296, 431 |
| burst, first 5 ms: page faults a request | 2.1 to 2.2 | 0.17 to 0.33 | 0.09 to 0.57 |
| burst, 5 to 50 ms: p99 / slowest, us | 113 / 123 | 122 / 127 | 134 / 148 |

The store's heap is full and churns in place, so there is little to return (4 MiB). What the
thread drained and returned in the silence comes back as about 380 page faults in the burst's
first 5 ms, and the two or three slowest requests there wait about 0.5 ms longer.

## orders axum cache 2G

| | thread on | thread off | jemalloc |
|---|---|---|---|
| thread | starts at shutdown only | none | |
| server cycles a request at 70% | 68.3k | 68.0k | 70.3k |
| p50 / p99 / p99.9 at 70%, us | 16 / 22 / 26 | 16 / 22 / 25 | 16 / 22 / 27 |
| heap at 70% and at every point after, MiB | 2,186 | 2,186 | 2,220 |
| burst, first 5 ms (1,221 requests): slowest, us | 170, 200, 236 | 121, 132, 147 | 106, 119, 123 |
| burst, 5 to 50 ms: p99, us | 22 | 22 | 22 |

The service stores each new order and drops the oldest, so its free memory stays far under
the floor and the thread is never wanted until the process drops its store on SIGTERM. Both
asmalloc columns are therefore the same program until then. Even so, the slowest request of
the first 5 ms was 20 to 115 us longer in every "on" run than in every "off" run, with no page
faults in either and no thread. Most of that was the environment (next section).

## The burst after a silence, diagnosed

Measured 2026-10-21 on main at `af9eb38`, three rounds on the log store and two on
Meilisearch. Scratch builds of the allocator (`bench/probes/default-thread/diag_edit.py`, not
product code) export counters that the watcher reads from the server's memory every 250 ms and
every millisecond around the burst: pages each claim found returned (by class), bytes returned
(by class and by rule), drains, slow paths, and what each look at a due clocked pass read.
Two-letter switches turn off `maintain`'s drains or its purges, and a third letter every purge
a slow path's heartbeat takes. Every configuration ran with an environment of the same size: local entries of
`bench/allocators.toml` (not committed) on builds made with `build.sh`, setting
`ASMALLOC_DIAG` (`dp`, `-p`, `d-`, `dp-`, `d--`) and either `ASMALLOC_BACKGROUND=0` or a
dummy `ASMALLOC_BACKGROUNX=0`.
The load generator kept every request (`BENCH_RAW_DIR`). `burst.py` and `memsum.py` make the
numbers below.

**The penalty is page faults on what the drains handed to the purge.** On the log store:

| configuration | returned in the silence | faults, first 5 ms | slowest request, first 5 ms | extra waiting, first 5 ms |
|---|---|---|---|---|
| no drains, no purges anywhere | 0 | 3 to 10 | 0.27 to 0.51 ms | 2.8 to 6.4 request-ms |
| drains, no purges anywhere | 0 | 21 to 28 | 0.53 to 0.58 ms | 6.8 to 8.8 |
| purges, no drains | 0.1 to 0.3 MiB | 12 to 35 | 0.26 to 0.36 ms | 3.0 to 3.5 |
| the default (drains and purges) | 4.2 to 4.5 MiB | 362 to 392 | 0.90 to 1.04 ms | 19 to 36 |

(Extra waiting is the latency above the steady median, summed over the requests due in the
first 5 ms.) Of the extra waiting over the first row, about four fifths is the faults and the
rest the drains.
- The thread's clocked pass returns 4.3 MiB in every silence, as long runs. That memory comes
  from the drains. Each idle CPU's magazines are laundered into the bitmaps, which completes
  whole free pages, and the next pass returns them. Without drains the passes find almost
  nothing (0.1 to 0.3 MiB).
- The frontiers that should protect those pages do not, because the thread's rounds step the
  counted clock through the silence. Two intervals in, every frontier has expired.
- The burst takes it all back at once. In its first 2 ms, claims find about 1,130 returned
  pages (4.4 MiB), and the process takes about 380 faults. They fall over some twenty small
  classes, each CPU's first refill of each: 28 and 32 KiB most, then 256 B, 10 KiB, 224 B,
  896 B, 1,792 B and 3,584 B. No large buffer is involved. The slowest requests are the first
  dozen, each waiting about 0.9 ms behind those faults.

**The floor reads fragmentation.** Through the load and the silence the clocked pass's looks
read 185 to 189 MiB retained in small classes against the 32 MiB floor, and set goals of about
125 MiB. The memstat build (thread off) shows that of 185 MiB free in the bitmaps, 1.4 MiB lies
on wholly free pages. The other 183 MiB is free slots on pages with live neighbours, which no
purge can return. So the store always reads as over the floor, and each pass returns whatever
its scope admits. (The memstat build's thread never parked and never drained, so that build was
used only for the account with the thread off.)

**A slow path also purges at the burst's start.** At `af9eb38` every heartbeat still steps
the clocked pass. A pass due at the first slow path after a silence purges pages whose frontier
the silence let expire, on that request's path. With the thread off this returned 0.3 MiB on the
log store and 33 to 52 MiB on Meilisearch. With the thread on but its purges switched off it
returned 1.4 to 2.5 MiB on the store, which cost as much as the default, and 30 to 84 MiB on
Meilisearch. With the thread doing its work, the pass found nothing left to return.

**Meilisearch.** The floor's reading there is close to what can be returned. The large
classes' free memory lies on whole pages, and small classes hold 5 to 17 MiB retained, about
half of it returnable. The thread returned 224 to 283 MiB in the silence, through the hold and
the clocked pass in different proportions from run to run, mostly slots of the 56 MiB class.
The burst coincides with an indexing batch. In its first 50 ms every configuration's claims find
20,000 to 25,000 returned pages of the 3.5 MiB class, which the hold returned under load. Its
first five requests take 1 to 4 ms whatever the configuration, so the thread's silence costs
nothing visible there. Without the thread, the burst's first slow path pays the purge instead.

**The control for the order service.** Run with a dummy variable as long as
`ASMALLOC_BACKGROUND=0`, "on" and "off" overlap: the slowest request of the first 5 ms took 132
to 151 us on, 121 to 148 off. p99 there was still 10 to 25 us higher on (107 to 122 against 92
to 98). The log store's difference holds with environments of the same size: every
configuration above had one.

Raw results: `docs/research/data/default-thread-burst-2026-10-21.jsonl.gz` (benchrun). The
watcher's samples and the request records (about 860 MB) are under `bench/results/thread-burst/`
of the main checkout.

## The floor measured against what could be returned

Measured 2026-10-21/22 on branch `floor-returnable` at `b8ae757` (main `0d751b0`, whose design
states the rule), three rounds of the log store and Meilisearch and two of the order service,
thread on (with a dummy variable as long as `ASMALLOC_BACKGROUND=0`) against off and jemalloc.
The change: each lap of the purge walk counts the free, resident pages it sees, and a pass is
held to that count, stopping at the 32 MiB floor; only intervals in which a heartbeat ran age a
frontier; in a silence `maintain`'s passes may go below the frontiers while more than the floor
could be returned.

**The faults are gone; the burst start is still slower, now from the idle drains.** Log store,
the burst's first 5 ms:

| | thread on | thread off | jemalloc | thread on at `af9eb38` |
|---|---|---|---|---|
| faults | 21 to 27 | 0 to 9 | 18 to 98 | 362 to 392 |
| slowest request | 0.68 to 0.86 ms | 0.29 to 0.39 ms | 0.32 to 0.41 ms | 0.90 to 1.04 ms |
| extra waiting | 14 to 20 request-ms | 2.6 to 3.8 | 3.1 to 3.9 | 19 to 36 |

- The thread now returns nothing in the silence. The lap's count is a few MiB, under the floor.
- A diagnostic round on the same commit, with `maintain`'s drains switched off, matched the
  thread-off level: slowest 0.29 ms, 2.8 request-ms, 14 faults. With the drains, 0.93 ms and
  19 request-ms.
- With the drains, the burst's first 5 ms claim from the bitmaps 3,240 times, against 789 with
  the thread off. Every drained magazine is refilled from the bitmaps at once, on every CPU.
- So what is left of the penalty is the idle drains' own. It is larger than in the drains-only run
  before the change (0.53 to 0.58 ms), which I have not explained.

Under load nothing moved: 355k cycles a request against 359k off (jemalloc 381k); p99.9 120
against 127 us. The heap at rest is 1,419 MiB against 1,425 off. The thread parks 12.5 to 13 s
into the silence and 13.5 s after the clients leave.

**Meilisearch keeps its gain.** A minute after the clients leave its heap is 661 MiB with the
thread (662 at `af9eb38`), 888 without, 658 for jemalloc. Ten seconds into the silence it is
692 against 908. Its large free memory lies on whole pages, so the count is close to what is
retained, and the floor keeps nothing more. In the burst's 50 to 500 ms, p99 was 13.9 ms with
the thread, 19.6 without, 10.3 for jemalloc, all dominated by the indexing batch. Under load,
16.97M cycles a request against 17.04M off. The thread parks 23 to 25 s into the silence.

**The order service is unchanged.** 69.3k cycles a request against 69.0k off, p99.9 26 us, heap
2,179 MiB against 2,178. Its thread still starts only at shutdown.

**Silent processes** (`crates/asmalloc/tests/default_thread.rs`): all thirteen cases pass. A
worker's freed 128 MiB burst goes back to 35 MiB resident, and the thread parks 18 to 20 s
after the silence starts, then makes no wake-up. Before the change it parked at 17 to 20 s and
the process ended at 31 to 35 MiB. The fragmented heap parks at 18 s.

Raw results: `docs/research/data/floor-returnable-2026-10-22.jsonl.gz`.
