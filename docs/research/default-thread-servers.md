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
faults in either and no thread; the cause is not known. Take that as the spread to allow when
reading the log store's first 5 ms, whose difference is several times larger and comes with
the faults.
