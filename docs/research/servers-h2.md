# Servers over HTTP/2, and the mini-redis control

Measured 2026-10-20 at `apps-servers-2` (allocator code as at `fffde81`), with the servers
suite (`bench/apps/README.md`, "Servers"): server on CPUs 0-3 with its cores held out of deep
idle, `bench/probes/loadgen` on 8-11, the server's day at 30% and 70% of the slowest
allocator's saturated rate, three rounds, allocators in a new random order every round, one
job and one round per piece under `scripts/measure --both-l3`, with gaps. Seven allocators
(the mimalloc variants other than the control and per-thread tcmalloc left out to fit the
session). No run invalid. Raw results: `docs/research/data/servers-h2-2026-10-20.jsonl.gz`.

HTTP/2 is prior knowledge over cleartext, eight connections of eight streams each: the same
64 requests at once as the HTTP/1.1 jobs' 64 connections. gRPC is connect-rust's own
log_bench request framed for gRPC; a response succeeds only with `grpc-status: 0`.

## Results

Cycles per request at 70% (the ranking; ± under 3% for every allocator), p99.9 at 70% in us,
peak resident MiB under load, resident MiB 9.75 s after the connections closed:

| Server | jemalloc | tcmalloc per-CPU | mimalloc | asmalloc | glibc | snmalloc |
|---|---|---|---|---|---|---|
| connect log, gRPC over HTTP/2, 63k req/s | 138k, 40, 14.5, 6.6 | 134k, 40, 13.2, 13.2 | 134k, 40, 24.4, 24.4 | 136k, 41, 11.2, 8.0 | 149k, 42, 7.6, 7.3 | 253k, 63, 22.0, 22.0 |
| connect log, Connect over HTTP/2, 63k | 136k, 40, 15.3, 6.8 | 132k, 40, 13.4, 13.4 | 131k, 39, 24.2, 24.2 | 136k, 41, 11.2, 7.8 | 146k, 42, 7.6, 7.1 | 252k, 64, 19.9, 19.9 |
| connect log, Connect over HTTP/1.1, 85k | 132k, 34, 9.6, 8.1 | 126k, 33, 13.7, 13.7 | 126k, 34, 22.2, 20.4 | 128k, 35, 9.8, 8.8 | 144k, 36, 8.7, 6.8 | 227k, 56, 17.3, 17.3 |
| orders axum, HTTP/2, 166k | 60.0k, 43, 207, 205 | 61.9k, 43, 212, 212 | 63.0k, 77, 262, 260 | 64.2k, 63, 209, 204 | 82.6k, 61, 415, 415 | 61.4k, 44, 418, 418 |
| orders axum, HTTP/1.1, 235k | 69.0k, 24, 206, 206 | 70.3k, 25, 213, 213 | 73.8k, 32, 262, 262 | 81.4k, 34, 210, 205 | 84.5k, 107, 335, 334 | 72.9k, 27, 348, 348 |
| miniredis, 476k | 36.1k, 23, 42.4, 36.2 | 36.2k, 23, 43.2, 43.2 | 36.0k, 23, 54.8, 48.9 | 38.3k, 24, 45.8, 37.0 | 36.1k, 23, 34.5, 34.5 | 36.6k, 23, 45.9, 45.9 |

The control (mimalloc-ctl) is within 0.7% of mimalloc on every job.

## What HTTP/2 changes for the allocator

- **Magazines do not flow between CPUs.** HTTP/2 puts a connection task between the socket and
  the handlers, so a frame or a body made on one worker can be freed on another. With the
  slow-path counts (the `probe-counts` build), the full magazines a CPU takes from the depot
  and those it sends there balance per CPU and class: what CPUs took beyond what they sent is
  0.013 to 0.015 magazines a request on gRPC over HTTP/2, 0.006 to 0.022 on Connect over
  HTTP/2, 0.015 to 0.020 on axum over HTTP/2, and 0.02 to 0.08 on HTTP/1.1. The objects that
  cross workers are matched by objects crossing back in each class, at these rates.
- **The depot trips are the magazine-end lock again.** At 70% with the cores held, gRPC over
  HTTP/2 takes 6.3 to 6.5 allocation and as many free slow paths a request: 1.8 to 2.3 swaps
  with the CPU's own spares and 4.0 to 4.3 round trips to the depot each way, with the same CPU
  taking and sending in each class. The classes: 640 bytes (0.9 to 1.3 depot round trips a
  request), 24 KiB (0.25 to 0.75), 5,120 (0.5 to 0.8), 48, 256, 40, 32 and 24 bytes (0.2 to
  0.5 each); 8 and 16 bytes swap with spares only. A CPU trips to the depot where its spare
  room for the class is full, as `rpc-slow-mode.md` found. Connect over HTTP/2 locks fewer
  pairs (3.3 to 4.2 slow paths each way, 2.7 to 3.4 of them depot trips); axum over HTTP/2
  1.5 to 2.0 (0.9 to 1.2 depot); axum over HTTP/1.1 8.5 to 9.1 (2.5 to 3.2 depot).
- **What it costs.** Counted per thread over 8 s at 70% (cores held, two runs each): on the
  log service the slow paths hardly show against a request of 150k cycles: asmalloc's user
  cycles a request are level with jemalloc's over gRPC/HTTP/2 (117k against 116k), 2k below
  over Connect/HTTP/2, and 6k above over HTTP/1.1 (110k against 104k, with one process of two
  not locked at all); per-CPU tcmalloc is 5k to 7k below both. On axum, HTTP/2 locks fewer
  pairs and asmalloc's gap to jemalloc shrinks from 15k user cycles a request (57k against
  42k) over HTTP/1.1 to 5k (57k against 52k) over HTTP/2. Kernel cycles are level between
  allocators in every case.
- **So earned room and multi-magazine depot trips are worth little to these servers**: there
  is almost no flow for them to carry. What would remove the remaining gap is the centring
  remedy for the lock (`servers-partial-load.md`), which applies alike over both protocols.

## mini-redis, the control

The allocators agree within 1.5% except asmalloc, 6% over jemalloc (38.3k against 36.1k
cycles a request) and the lowest saturated rate (677k against 723k to 740k). Per thread, all of
the difference is user mode: 12.5k to 12.6k user instructions a request against jemalloc's
6.4k, kernel level. The slow-path counts show the lock once more, in the two smallest classes
only: 1.9 to 2.0 allocation and as many free slow paths a request, all in the 8- and 16-byte
classes, one each per request on every CPU, all swaps with spares. mini-redis's memory: glibc
holds the least (34.5 MiB), asmalloc and jemalloc give back the most when idle (8.8 and 6.2
MiB in 10 s).

## Not yet done

- **Streaming RPCs.** connect-rust's `BenchService` (`ServerStream`) is generated in
  `bench/rpc` but not implemented there: it needs an implementation of the service's six
  methods (only `ServerStream` doing work), a `BenchRequest` body with `response_count` and a
  payload, generated like `log50.bin`, and `rpc-server bench`. The generator already handles a
  streamed response over HTTP/2: it completes a request at the stream's end. Half a day.
- **The proxy and meilisearch**, next.

## lean-slow on these jobs

Measured 2026-10-20 afternoon: branch `lean-slow` at `d35a919` (the lean slow path, and the
loaded magazine left half full on a swap that reverses the last), merged into main `60e8d87`
on a throwaway branch and built as a second asmalloc beside main's, for the four jobs above
and axum over HTTP/1.1. The suite's day with cores held, three rounds, four allocators in a
new random order each round; then, at 70% and cores held, two runs each of the slow-path
counts (`probe-counts` cherry-picked onto both builds) and of per-thread counters. No run
invalid. Raw results: `docs/research/data/servers-lean-2026-10-20.jsonl.gz`.

Cycles per request at 70% (± under 2.5%), p99.9 at 70% in us, saturated rate in thousands:

| Job | asmalloc main | asmalloc lean-slow | jemalloc | tcmalloc per-CPU |
|---|---|---|---|---|
| connect log, gRPC/HTTP/2 | 136.1k, 41, 181 | **130.4k**, 41, 189 | 140.2k, 40, 174 | 132.2k, 39, 185 |
| connect log, Connect/HTTP/2 | 135.3k, 40, 187 | **128.6k**, 44, 191 | 137.1k, 40, 176 | 130.7k, 40, 188 |
| orders axum, HTTP/2 | 64.0k, 54, 306 | **60.5k**, 43, 303 | 61.1k, 42, 304 | 60.6k, 43, 304 |
| orders axum, HTTP/1.1 | 80.4k, 33, 328 | **68.8k**, 25, 383 | 69.1k, 25, 382 | 69.9k, 25, 375 |
| mini-redis | 38.3k, 24, 688 | 36.1k, 23, 723 | 36.2k, 23, 736 | **36.0k**, 23, 735 |

lean-slow is the best or level with the best on every job: 6 to 7% under jemalloc on the
log service, level with it on axum and mini-redis, and 14 to 15% under main on axum over
HTTP/1.1. Memory does not move (peaks within 0.2 MiB of main's).

**Slow paths per request at 70%** (two runs each):

| Job | main: swaps with spares | main: depot round trips | main: fill and launder | lean-slow: all slow paths |
|---|---|---|---|---|
| gRPC/HTTP/2 | 1.5 to 1.9 | 3.7 to 3.9 | 0.17 to 0.18 each | under 0.0001 |
| Connect/HTTP/2 | 0.9 to 1.2 | 3.0 to 3.4 | 0.13 to 0.15 | under 0.0002 |
| axum HTTP/2 | 0.39 to 0.40 | 0.73 | 0.04 | 0.0001 |
| axum HTTP/1.1 | 6.1 to 6.3 | 2.2 | 0.27 | under 0.00001 |
| mini-redis | 1.8 to 1.9 | 0.002 to 0.003 | 0.06 | 0.0001 |

(Counted each way; allocation and free slow paths are equal on main. On lean-slow the whole
8 s window holds 0 to 312 slow paths against 0.5 to 3.8 million requests.)

**The reversal rule removes the depot round trips too, not only the swaps with spares.** On
main, gRPC over HTTP/2 makes its depot round trips in 640 B (0.8 a request), 256 B (0.6),
32 B (0.5), 5,120 B (0.5), 24 KiB (0.5), 48 B (0.3), 24 B (0.3) and 40 B (0.2); on lean-slow
none of them takes a slow path more than once in 10,000 requests. So the swings fit in a
half-full magazine even where the magazine is small. Allocations per request by class (a
build that counts them, below), against the class's magazine capacity: 640 B 5 a request
(capacity 62), 5,120 B 2 (6), 20 KiB 1 (2), 24 KiB 2 (2), 256 B 50 (254), 128 B 51 (510),
48 B 50 (1,022). Even the 24 KiB class, which allocates as many objects a request as its
magazine holds, stays in its magazine once that starts half full: its objects are freed
before the next are allocated, so its swing is one or two, not the request's count. Earned
room has nothing to act on in these servers once the reversal rule is in.

**Per thread**, at 70% with cores held, user cycles a request (kernel within about 3k in every
case; the extra on main is all in the workers and all user mode): gRPC/HTTP/2 main 120.5k,
lean-slow 106.0k, jemalloc 116.3k, tcmalloc 108.5k; Connect/HTTP/2 113.3k, 104.0k, 114.0k,
106.7k; axum HTTP/2 57.6k, 52.6k, 51.6k, 52.8k; axum HTTP/1.1 58.1k, 41.2k, 40.4k, 42.2k;
mini-redis 8.7k, 6.4k, 6.4k, 6.4k. lean-slow's user instructions match jemalloc's to within
3% on axum and mini-redis and are 4 to 5% lower on the log service.

Method notes: the second build was `bench/target/asmalloc-lean`, a hand-built entry in
`allocators.toml` (`build = "asmalloc-lean"`, not committed). The size counts come from a
throwaway `asmalloc-sizes` feature of bench-alloc that wraps asmalloc and adds one relaxed
atomic per call into a table `BENCH_SIZE_COUNTS`, read by `run.py sizes` and summed by
`sizesum.py`; its counts are exact, its timing is not meant to be.
