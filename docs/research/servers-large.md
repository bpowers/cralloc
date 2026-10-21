# Servers on large heaps

Measured 2026-10-20 on branch `servers-large`, allocator code as at `e0baed4` (main, with the
lean slow path and the reversal rule), with the large-heap suite (`bench/apps/README.md`,
"Large heaps"): server on CPUs 0-3 with its cores held out of deep idle, `bench/probes/loadgen`
on 8-11, three rounds, allocators in a new random order every round, one job and one round
per piece under `scripts/measure --both-l3` with two-minute gaps, each run capped at 3, 7 or
12 GiB by size. No run invalid. Cycles per request vary under 1.5% between rounds for every
allocator and job. Raw results, saturated-rate calibration included:
`docs/research/data/servers-large-2026-10-20.jsonl.gz`.

The jobs:

- **orders axum cache**: the order service over HTTP/1.1, 64 connections of POSTs, keeping
  its last 280k, 1.12M or 2.25M orders (about 1.9 KiB live each), so the live set holds still
  while every request allocates a new order and frees the oldest.
- **miniredis churn**: mini-redis preloaded with 450k, 1.8M or 3.6M keys of log-uniform 64 B
  to 4 KiB values before it listens, then half GETs and half SETs of random keys with new
  values of new random sizes, one SET in ten with an expiry of 10 to 60 s.

The rate is 70% of the slowest allocator's saturated rate after the heap is filled: mimalloc's
on orders (243k to 248k req/s), tcmalloc's or asmalloc's on mini-redis (462k to 470k).

Resident over live is the server's resident set over the bytes the program has allocated and
not freed, by requested size, counted by a build of the same server (`live-bytes`) at the
same marks. The program's allocations do not depend on the allocator, so one count serves
every allocator's runs.

## Results

Cycles per request at 70%, p99.9 at 70% in us, then resident over live at the start and the
end of the minute at 70%, and 9.75 s after the connections closed (medians of three):

| Job | asmalloc | jemalloc | tcmalloc per-CPU | mimalloc |
|---|---|---|---|---|
| orders 0.5 GiB | 67.5k, 26; 1.129, 1.129, 1.118 | 68.4k, 26; 1.114, 1.114, 1.114 | 69.8k, 27; 1.133, 1.133, 1.133 | 73.0k, 39; 1.199, 1.203, 1.207 |
| orders 2 GiB | 68.0k, 26; 1.092, 1.098, 1.086 | 68.7k, 26; 1.099, 1.104, 1.104 | 69.9k, 28; 1.104, 1.108, 1.108 | 72.6k, 40; 1.156, 1.158, 1.158 |
| orders 4 GiB | 68.0k, 165; 1.080, 1.089, 1.075 | 68.7k, 95; 1.098, 1.104, 1.104 | 69.6k, 117; 1.099, 1.115, 1.115 | 72.2k, 758; 1.153, 1.154, 1.155 |
| miniredis 0.5 GiB | 38.6k, 24; 1.104, 1.104, 1.106 | 38.6k, 24; 1.121, 1.123, 1.124 | 38.4k, 23; 1.137, 1.137, 1.138 | 38.2k, 25; 1.176, 1.176, 1.177 |
| miniredis 2 GiB | 38.8k, 24; 1.093, 1.093, 1.093 | 38.8k, 24; 1.104, 1.104, 1.104 | 38.8k, 24; 1.120, 1.120, 1.121 | 38.2k, 24; 1.158, 1.159, 1.160 |
| miniredis 4 GiB | 39.0k, 24; 1.090, 1.090, 1.090 | 39.2k, 24; 1.101, 1.101, 1.101 | 39.2k, 24; 1.117, 1.117, 1.118 | 38.6k, 24; 1.137, 1.137, 1.137 |

## What it shows

- **Memory.** asmalloc holds the least over live on five of the six jobs: 8 to 10% over live
  from 2 GiB up, against jemalloc's 10%, tcmalloc's 11 to 12% and mimalloc's 14 to 16%. On the
  0.5 GiB order service it is 13%, beside tcmalloc and above jemalloc's 11%. The ratio falls as
  the heap grows for every allocator: fixed costs (the program, its runtime, per-CPU caches)
  are a smaller part of a larger heap.
- **No drift.** Over a minute of churn at 70% no ratio moves by more than 0.016 (tcmalloc on
  orders 4 GiB); asmalloc's moves by at most 0.009. Mini-redis moves by under 0.002 for
  everyone.
- **After the load stops**, ten seconds into idleness, asmalloc has given back about 1% of live
  on the order service (1.098 to 1.086 at 2 GiB); the others hold what they held under load.
  On mini-redis nobody gives anything back.
- **A peak after preload.** On mini-redis 0.5 GiB asmalloc's peak is 1.19 times live, right
  after the preload (16 connections writing 450k keys); it returns that within the first
  seconds of churn and settles at 1.10. The others peak later, at what they hold in steady
  state. The cgroup peak, not the steady state, is what a memory limit has to cover.
- **Cycles per request.** asmalloc is fastest on the order service at every size (1% under
  jemalloc, 7% under mimalloc) and level on mini-redis, where all four are within 2% and
  mimalloc is ahead by 1 to 1.5%. Heap size barely moves anyone's cycles.
- **Tails.** p99.9 is 23 to 28 us for every allocator except mimalloc on the order service,
  until the order service reaches 4 GiB, where everyone's tail grows (95 to 165 us; mimalloc
  758 us) and requests miss their slot (7k to 28k of about 15M). asmalloc's is second-worst
  there. Faults per request grow with the heap on the order service (0.0008 to 0.0024) for
  everyone alike; the 4 GiB tail is not explained here and is the next thing to look at.
- **Memory system calls** are rare for every allocator. From `run.py syscalls` (strace -c
  over 8 s at the 2 GiB size, outside the timed runs), per 1,000 requests: asmalloc 0.48
  madvise on the order service and 0.05 on mini-redis; jemalloc none and 0.06; mimalloc and
  tcmalloc none. mprotect, mmap, munmap, mremap and brk are zero for everyone.

## Before the reversal rule landed

A first session the same day measured main before the lean slow path (`c4a9c17`) beside the
lean slow path at `d35a919`, two rounds. The lean build's numbers match main's above within
3%, at that session's lower rates. The old main was 15 to 17% slower on the order service at 0.5 and 2 GiB and 4% on mini-redis. At 70% of
its own saturated rate it fell behind on mini-redis 2 and 4 GiB and on the order service at
4 GiB: hundreds of thousands to millions of requests missed their slot, and p99.9 rose to
2.5 to 9 ms on mini-redis and to 708 ms on the order service at 4 GiB. That was the
magazine-end lock (`servers-partial-load.md`). Resident over live was the same for both
builds.

## Method notes

- The live count is asmalloc with an atomic add per call, into 64 cache-line counters picked
  by the thread's stack address (`bench/alloc/src/live.rs`). The runner reads it from
  `/proc/PID/mem` at each mark. The counting build is up to 6% slower than asmalloc
  (shown as `live-bytes` in largesum's output) and its resident set matches asmalloc's to
  0.001, so the counter changes nothing it measures.
- Each allocator's resident set is read from the run's 250 ms curve at the same mark. The
  "end of the minute" value is 9.9 s into the last piece.
- The load generator keeps a 20-byte record of every request, plus the command list (up to
  hundreds of MiB here); its cap is now 3 GiB (`bench/runner/src/cap.rs`).
- Summaries: `bench/probes/partial-load/largesum.py` (this table), `syssum.py` (the system
  calls), `timeline.py` (latency over time from a `--raw` record).
