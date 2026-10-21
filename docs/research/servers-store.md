# A streaming RPC server holding a large heap

Measured 2026-10-21 on branch `servers-stream` (allocator code as at `ea192e1`), with the
large-heap suite: `connect store 1M`, connect-rust's server (`bench/rpc`, `rpc-server store`)
as a log store of a million owned log records, about 1.06 GiB live, on CPUs 0-3 with its cores
held out of deep idle; the load generator on 8-11, 64 connections over HTTP/1.1 in the Connect
protocol, alternating an ingest of 50 new records (the oldest 50 go) with a tail of the 100
most recent, which the server streams back as 100 messages (about 84 KB). The large-heap day
at 36,300 req/s, 70% of mimalloc's saturated 51,840 (asmalloc 64,062, jemalloc 59,197,
tcmalloc 56,361). Three rounds as separate sessions. Two runs were invalid, with 0.55 and 3.4
CPUs of other work on the server's CPUs in one segment while another teammate's measurement
overlapped, and were replaced by a fourth round of those two allocators. Raw results:
`docs/research/data/servers-store-2026-10-21.jsonl.gz`.

## Results

Medians of three. Cycles per request and p99.9 at 70%; resident over the live count at the
start and end of the minute at 70% and ten seconds after the connections close:

| Allocator | cycles/req | p99.9 us | resident/live, 70% | resident/live, idle | huge-page MiB | direct compactions |
|---|---|---|---|---|---|---|
| asmalloc | 371k | 130 | 1.305, 1.305 | 1.308 | 1,200 to 1,320 | 0, 0, 59 |
| jemalloc | 395k | 137 | 1.417, 1.418 | 1.422 | 0 | 0 |
| tcmalloc per-CPU | 417k | 148 | 1.405, 1.405 | 1.408 | 0 | 0 |
| mimalloc | 446k | 185 | 1.416, 1.385 | 1.384 | 1,150 to 1,420 | 16 to 195 |

Page faults per request are 0.004 or fewer for every allocator. Memory system calls per 1,000
requests (`run.py syscalls`, strace over 10 s at the same rate, outside the timed runs):
madvise asmalloc 0.16, jemalloc 40, mimalloc 0.09, tcmalloc none; no mprotect, mmap, munmap
or brk for any.

## What it shows

- **asmalloc does the least work per request** by a clear margin: 6% under jemalloc, 11%
  under tcmalloc, 17% under mimalloc, with the best tail. Each request either turns 50
  decoded records into owned ones and drops 50 old ones (each record about 20 small blocks,
  freed on whichever worker ingests the record that displaces it) or builds 100 response
  messages from stored records and frees them as they are encoded.
- **It holds the least**: 31% over live against 38 to 42% for the others, with no drift over
  the minute and the same ten seconds after the load stops (nobody returns anything here: the
  heap is full and churns in place).
- **jemalloc makes 40 madvise calls per 1,000 requests**, returning pages of the freed records
  that it then faults back (0.004 faults a request, the most of the four).
- asmalloc and mimalloc both put most of the heap in huge pages. The runs with direct
  compactions do not stand apart from the others of the same allocator.

## The job

- The service is written for the suite (`bench/rpc/src/store.rs`); the protocol, the generated
  code and the records are connect-rust's. Ingest stores each record as connect-rust's owned
  message type; the tail turns each stored record into a `BenchResponse` (strings, a byte
  field, a nested message, a repeated string field and two maps copied from it), as a service
  turns its model into its API's messages. The store is 64 shards of a ring each.
- It starts full (the same records the generator writes, numbered from 0), so the run measures
  the whole heap from its first request.
- connectrpc closes a keep-alive HTTP/1.1 connection after 30 s without a request. At 70% most
  of the 64 connections sit idle that long, so every run had about 110 reconnects and was
  untrustworthy (only with the cores held out of deep idle, when the server answers fast
  enough to need few connections). `rpc-server store` sets the limit to 300 s, as a service
  behind a pooled client would.
- The load is `store.http` in the connect inputs (`probes/loadgen/connect_store_requests.py`,
  4,000 pairs, 96 MB), sent in turn; the load generator now takes a file of whole HTTP
  requests (added for Meilisearch).
