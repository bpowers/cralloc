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
  seconds of churn and settles at 1.10. The extra is memory the preload freed, retained until
  the clocked purge returns it: the resident set falls by about half the excess each second
  (553, 533, 523, 518, 516, 513 MiB), as designed. The others peak later, at what they hold in
  steady state. The cgroup peak, not the steady state, is what a memory limit has to cover.
- **Cycles per request.** asmalloc is fastest on the order service at every size (1% under
  jemalloc, 7% under mimalloc) and level on mini-redis, where all four are within 2% and
  mimalloc is ahead by 1 to 1.5%. Heap size barely moves anyone's cycles.
- **Tails.** p99.9 is 23 to 28 us for every allocator except mimalloc on the order service,
  until the order service reaches 4 GiB, where everyone's tail grows (95 to 165 us; mimalloc
  758 us) and requests miss their slot (7k to 28k of about 15M). asmalloc's is second-worst
  there. The cause is the order service's own hash tables doubling, once, during that minute
  ("The 4 GiB tail" below).
- **Memory system calls** are rare for every allocator. From `run.py syscalls` (strace -c
  over 8 s at the 2 GiB size, starting 2 s after the fill, outside the timed runs), per 1,000
  requests: asmalloc 0.48 madvise on the order service and 0.05 on mini-redis; jemalloc none
  and 0.06; mimalloc and tcmalloc none. mprotect, mmap, munmap, mremap and brk are zero for
  everyone. asmalloc's are the purge after the fill: traced with timestamps over the suite's
  plan at 4 GiB, it made 2,100 madvise calls in the fill and none in the minute at 70%
  except 55 while the tables doubled.

## The 4 GiB tail

Found with the open-loop generator's per-request records (`run.py plain --raw`,
`stalls.py`), the suite's plan up to its minute at 70% at 248k req/s, one run each.

- **Nine tenths of the slow requests are one event.** Of the requests over 60 us in the
  minute, 93 to 96% fall in a burst of 59 to 67 stalls of about 2 ms each, inside 2 to 3
  seconds, for every allocator (asmalloc 30k slow requests, jemalloc 27k, tcmalloc 37k). At
  the same moment the resident set rises by 130 MiB and the live count by 70 MiB.
- **The burst is the order service's 64 shard tables doubling.** Each shard is a `HashMap`
  behind a mutex holding 35k orders. Inserts and removes leave the table's growth allowance
  falling; once it runs out, a table more than half full doubles (65,536 to 131,072
  buckets), rehashing every entry while its shard's lock is held. The other workers that
  need that shard block on the mutex, and every connection they serve waits. The shards
  fill in step, so all 64 double within seconds, once, after several generations of
  orders, and then the tables sit at a quarter full. At 2 GiB the tables, half the size,
  double during the minute too, but each stall stays short enough that p99.9 does not move
  (29 us in the same probe).
- **What is ours in it.** A profile of the burst (perf record, cycles) puts about 0.56 ms of
  each table's doubling in the rehash under asmalloc against 0.39 ms under jemalloc; the
  difference is page faults (23 ms against 14 ms over the 64 tables): asmalloc places each
  2.2 MB table in fresh memory, touched 4 KiB at a time, where jemalloc reuses part of what
  it freed. That makes asmalloc's stalls a little longer and is why its p99.9 sits above
  jemalloc's in this job. The madvise and mprotect calls asmalloc makes in the burst take 3
  to 11 us each under strace, under 1% of the stall time. No slow requests point to a
  heartbeat: asmalloc's slow requests outside events of ten or more number 130 to 140 in the
  minute.
- **The rest** (3 to 7% of slow requests) is common to all allocators: stalls of 0.2 to 1.2
  ms every 1.024 s in the first 15 to 25 s of the minute, cause not found.
- **mimalloc** has the same burst with longer stalls (3.4 ms median) and stalls of 10 to 25
  ms beside it. Its runs have 1,000 to 1,600 direct compactions each against none for the
  others (bar one asmalloc run): its huge-page faults wait for the kernel to compact memory,
  on a machine whose free memory is about 2 GiB with the rest in page cache.

The two branches being reworked: purging by runs with bounded steps does not touch this (no
purge runs in the minute). A hold on large free slots is in the same path, the placement of
a large block, but it would help only if a free slot of the new size were held; here the
freed tables are half the size of the new ones.

The tail measures a one-off event of the program, timed by its hash state and the length of
the fill. Pre-sizing each shard to twice its share of `keep` would take it out of the minute
and leave the job measuring steady churn; that is a change of job, not made here.

## Huge pages

Huge-page-backed memory over resident at its peak under load (the anonymous memory in
transparent huge pages; the machine runs THP in `madvise` mode), three rounds:

| Job | asmalloc | mimalloc | jemalloc, tcmalloc |
|---|---|---|---|
| orders 0.5 GiB | 0.72 | 1.00 | 0 |
| orders 2 GiB | 0.89 | 0.91 to 1.00 | 0 |
| orders 4 GiB | 0.39, 0.92, 0.93 | 0.47, 0.96, 0.99 | 0 |
| miniredis 0.5 GiB | 0 | 0.99 | 0 |
| miniredis 2 GiB | 0.12 | 0.81 to 1.00 | 0 |
| miniredis 4 GiB | 0.17 | 0.66 to 0.92 | 0 |

- jemalloc and tcmalloc do not ask for huge pages, so in `madvise` mode they get none.
- asmalloc marks a class's region for huge pages only past its first 16 MiB and only for
  classes of 1 KiB and under (`back/policy.rs`). Mini-redis's values are log-uniform from 64
  B to 4 KiB, so most of its bytes are in classes that never get huge pages.
- Coverage varies between runs with the machine's free memory: the low runs at 4 GiB had
  hundreds of direct compactions and THP faults that fell back to small pages.
- It does not decide cycles per request here: asmalloc is fastest on the order service with
  less coverage than mimalloc, and on mini-redis, where mimalloc's coverage is far higher,
  mimalloc is ahead by 1 to 1.5%.

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
  calls), `timeline.py` (latency over time from a `--raw` record), `stalls.py` (slow
  requests grouped into stalls, lined up with timed system calls from `run.py madvise`).
