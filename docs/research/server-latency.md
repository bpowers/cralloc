# Server latency at a fixed request rate

Measured 2026-10-19 and 20 with asmalloc at commit `d31a26e`, against glibc 2.43, jemalloc
5.3.0 (Fedora's), mimalloc 3.5.3 and tcmalloc `f3c6669` in per-CPU mode (`bench/baselines`),
on the reference host (Ryzen 9950X, Linux 7.2.9, THP `madvise`). Raw records:
`data/server-latency-2026-10-20.tar.gz`. Tools: `bench/probes/loadgen/`.

## Summary

- **Where users feel it, asmalloc is level with the best allocator in steady service.** At
  30% of the slowest allocator's saturated rate its p50 to p99 are within 1 to 7% of the
  best on all three servers, its CPU per request within 1 to 3%, and it holds the least or
  near-least memory. It is fastest at saturation on actix, 2% behind jemalloc on axum.
- **At 70% load the tail belongs to the machine and the runtime, not the allocator.** Every
  allocator shows queueing episodes of 1 to 8 ms. They go away when the server's cores are
  kept out of the deepest idle state (C3, 350 us to wake): p99.9 falls from 2.2 to 4.1 ms to
  30 to 110 us. A faster allocator leaves more idle time, so it gets more C3 and a worse 70%
  tail; that is why glibc, the slowest, has the best p99.9 on actix at 70%.
- **asmalloc is worse in two places, both its own.**
  1. On the connect-rust server, some processes run in a slow mode: 2 to 4% of cycles in
     depot exchanges instead of 0.02%, 6% less throughput, and twice the latency at the start
     of each burst. Which mode a process gets varies from run to run and does not depend on
     address-space layout. The class involved is not yet identified.
  2. The first requests after a silence pay for the maintenance the silence skipped: the
     clocked purge (tens of one-page `madvise` calls) and idle-slab drains (`membarrier`),
     run by the first slow paths of the burst. On axum this doubles the latency the burst
     start adds (9.2 against jemalloc's 3.3 request-ms); the opt-in background thread halves
     it. The memory it returns is under 0.5 MiB.
- **Memory through silences and after load: no allocator returns anything that matters**
  on these servers. The order services hold 100,000 orders (about 180 MiB live), which
  dominates; the RPC server's heap is 8 MiB. asmalloc, without its background thread, returns
  nothing while the process is idle, as designed.

## Method

**Generator.** `bench/probes/loadgen` (Rust, mio): an open-loop HTTP/1.1 client. Requests
are due on a fixed, evenly spaced schedule at a set total rate, over 64 keep-alive
connections in 4 spinning threads (one per CPU); each connection carries one request at a
time, as a pooled client does. Latency runs from when a request was due to when its response
was complete, so a request that waited for a free connection is charged for the wait (a
request that found every connection busy is counted as having missed its slot). Percentiles
are exact (all latencies are kept). It samples the server's resident set every 10 ms from
`/proc/PID/statm`, and prints a mark at each segment so a script can count the server over
it. At saturation the server's four CPUs are fully busy (task-clock 19 to 20 s per 5 s), so
the generator is not the limit; its closed-loop rates match oha's from the screening within
3%.

**Servers.** The screening's order service on axum and on actix (`POST /orders`, 835-byte
JSON, store of the last 100,000 orders), each allocator preloaded; and connect-rust's
log-ingest server (`bench/rpc`, 50 records, 22 KB of protobuf per request) built with each
allocator as `#[global_allocator]`, driven with the Connect protocol over HTTP/1.1 rather
than the suite's gRPC over HTTP/2. Server on CPUs 0-3 (4 workers), generator on 8-11.

**A server's day**, one fresh server process per allocator and round: 3 s closed-loop fill,
4 s closed-loop saturation, 10 s at 30% and 10 s at 70% of the slowest allocator's saturated
rate (each after a 1 s warm-up), four bursts of 2 s at the 70% rate each after 5 s of
silence, then the connections close and the idle server is watched for 20 s. Rates: axum
90k and 215k req/s, actix 115k and 270k, RPC 63k and 146k (glibc's saturated rates were 308k,
385k and 209k). `perf stat` counts the server over each phase. Five rounds, allocators in a
new random order each round, under `scripts/measure --both-l3`; saturation has three more
rounds of its own.

**Bursts**, for the start of load after silence: ten bursts of 1 s, each after 3 s of
silence (three times asmalloc's clock interval), three rounds; axum at its 30% rate (so its
runtime's 70% episodes stay out of it), RPC at its 70% rate.

**Noise.** Given as half the range over the median of rounds. Medians to p99 repeat within
1 to 5%; p99.9 within 10 to 50% (one bad millisecond in ten seconds moves it); maxima are
anecdotes. asmalloc's saturated rate varies more than the others' (below).

## Results

Medians over five rounds (saturation: eight). Latency in microseconds.

| | axum | actix | connect-rust log |
|---|---|---|---|
| saturated req/s, asmalloc / best | 395k / 402k jemalloc | **498k** / 492k tcmalloc | 239k / 254k mimalloc |
| 30%: p50, asmalloc / best | 16.3 / 16.3 | 14.5 / 14.4 | 25.0 / 23.8 mimalloc |
| 30%: p99 | 20.6 / 20.0 tcmalloc | 17.9 / 17.4 tcmalloc | 29.3 / 27.4 mimalloc |
| 30%: p99.9 | **165** / 288 to 965 for the rest | 26.5 / 25.6 tcmalloc | 1,604 / 931 tcmalloc |
| 30%: CPU per request | 14.24 / 14.20 jemalloc | 10.17 / 10.01 tcmalloc | 20.27 / 19.61 mimalloc |
| 70%: p99.9 | 2,418 / 2,332 jemalloc | 4,127 / 1,931 glibc | 2,014 / 1,780 mimalloc |
| resident while loaded, MiB | **201** / 204 jemalloc | **204** / 207 jemalloc | 8.6 / 8.0 jemalloc |
| burst start, extra request-ms | 9.2 / 3.3 jemalloc | (see text) | 18.6 [18 to 52] / 21.8 mimalloc |

Full tables per allocator: `day-report.txt` in the data archive.

- **Saturation.** glibc is 20 to 25% behind the rest everywhere; it has many times the
  others' context switches on the order services (lock waits). asmalloc's own rate spreads
  more than anyone's: RPC 231k to 258k across 28 processes, against 2% for every other
  allocator; actix 457k to 502k.
- **30%.** Allocators differ by a few microseconds at the median and p99, in line with their
  CPU per request. Page faults, context switches and kernel cycles per request are the same
  for all but glibc (which faults far more often). The RPC server's p99.9 near 1 to
  1.6 ms under every allocator comes from rare wake-up stalls (next item), not allocation.
- **70%.** Every allocator queues. On axum, tokio parks all but one worker for several
  milliseconds at a time: per-thread run time sampled every 0.5 ms shows one worker running
  while three are parked and throughput halved, then all four catching up. On actix
  throughput falls a few percent below the arrival rate for tens of milliseconds. Kept out of
  C3 by a spinner on each SMT sibling (CPUs 16-19; two rounds, axum and actix, asmalloc,
  jemalloc and glibc):

  | s70 p99.9, us | asmalloc | jemalloc | glibc |
  |---|---|---|---|
  | axum, plain | 2,180 to 2,640 | 2,820 to 2,870 | 2,240 to 2,570 |
  | axum, siblings spinning | 30 to 68 | 30 to 32 | 47 to 48 |
  | actix, plain | 2,180 to 4,140 | 3,490 to 3,600 | 850 to 2,310 |
  | actix, siblings spinning | 85 to 2,118 | 44 to 93 | 108 to 956 |

  The server's CPUs spent 1.7 to 3.6 s (summed over four CPUs) of each 8 s window in C3 under
  asmalloc and jemalloc, 0.2 to 1.2 s under glibc. (The spinners share the cores, so medians rise by 3 to 8 us.)
  Two runs kept one millisecond-scale event with spinners (asmalloc 6.7 ms once, glibc
  1 ms once); not investigated.
- **Bursts after silence.** Every allocator's first millisecond or two after a silence is
  slow (the server wakes from C3 and its runtime unparks); what differs is how long it lasts.
  Per burst, as the median over bursts and then over runs:

  | | asmalloc | asmalloc + background thread | jemalloc | tcmalloc | mimalloc | glibc |
  |---|---|---|---|---|---|---|
  | axum 30%: extra request-ms in the first 20 ms | 9.2 | 4.8 | 3.3 | 3.3 | 5.2 | 8.2 |
  | axum 30%: p50 of requests due in the first 0.5 ms | 119 | 71 | 29 | 35 | 71 | 119 |
  | RPC 70%: extra request-ms | 18.6 (18 to 52) | | 23.9 | 23.1 | 21.8 | 53.1 |

  RPC runs fall in two groups by process (below): 18 to 21 request-ms, the best of any
  allocator, or 50 to 56, glibc's level.
- **Memory.** Flat through every silence and for 20 s after the clients leave, under every
  allocator, within 0.5 MiB; glibc gives back 2 MiB of the RPC server's 9 when the
  connections close. asmalloc holds the least on the order services (201 to 204 MiB against
  glibc's 310 to 400) and 0.6 MiB more than jemalloc on the RPC server.

## What is in asmalloc's tail, ranked

1. **Idle states and runtime parking (all allocators; up to several ms at 70%).** The
   spinner experiment removes 95 to 99% of the 70% p99.9 for every allocator, in all but two
   runs. The allocator
   enters only indirectly: less CPU per request means more idle time and more C3. Nothing in
   asmalloc can fix this; a server that cares sets a PM QoS latency limit or keeps a core
   busy.
2. **A per-process slow mode on the RPC server (6% throughput, about 30 extra request-ms per
   burst).** In `perf record` of five saturated processes, the one at 253k req/s spent 0.02%
   of cycles in asmalloc's slow paths (`Cache`, `alloc_slow`, `free_slow`, depot stacks);
   the four at 235k to 242k spent 2.0 to 3.4%, all in magazine exchanges with the depot, not
   in the back end. The realloc path costs 2.4 to 2.8% in both. Turning address-space
   randomization off (`setarch -R`) leaves the split as it was (239k to 255k and 238k to 258k in
   the two sets of six), so it is not where things are mapped; it is consistent with which worker allocates
   and which frees a class whose magazines hold two objects (the 16 to 32 KiB body buffers),
   which tokio decides per process. Not established: `perf --call-graph dwarf` cannot unwind
   through the generated routines (no CFI), so the class and caller are unknown.
3. **Maintenance at the start of a burst (about 5 request-ms per burst on axum; under 0.5 MiB
   of memory returned).** With no slow path during a silence, no heartbeat runs, so the
   first slow paths of the burst do what the silence skipped, and the clock says everything
   is idle. `strace -f --seccomp-bpf -T` of the memory system calls, over ten bursts: asmalloc
   on RPC made 594 `madvise(DONTNEED)` calls in the first 3 ms of the bursts (mostly one 4 KiB
   page each, 13 us apiece) and 92 in the rest; jemalloc 21. On axum, 248 `madvise` and 11
   `membarrier` in the first 3 ms. Slowing those calls (strace stops on each) tripled
   asmalloc's burst-start cost on RPC (to 187 request-ms) and left jemalloc's as it was, so
   they are on the requests' path. `perf record` at 20 kHz shows the purge step in 0.84% of
   samples in burst starts against 0.01% in steady load. With `ASMALLOC_BACKGROUND=1` the
   same work happens in the silences (3,484 `madvise` and 40 `membarrier` there, none in the
   burst starts) and axum's burst-start cost falls from 9.2 to 4.8; the price is that the
   pages it returns are faulted back (600 faults per ten bursts against 120).
4. **Not seen.** `mprotect` while serving (one call in all burst starts), page faults
   (at most 0.0008 per request), context switches (the same as jemalloc's), or any single
   slow path long enough to stand out in a request: the slowest memory call in all traces
   was 124 us.

## Options

- **Treat a silence as a pause, not as evidence of disuse.** When the first heartbeat after a
  gap of more than an interval finds the decay and idle clocks stale, restamp them and act an
  interval later, as the design already does for a clock that goes back. Expected: the burst
  start loses its purge and drains (axum about 9 to about 5 request-ms, by the background
  thread's measure); memory is returned an interval later in a busy period, and an idle
  process returns none either way. A server with idle periods that wants its memory back
  then needs the background thread, which it needs today anyway.
- **Purge runs of pages in one call.** Most purge calls were for one 4 KiB page. Coalescing
  adjacent free pages in a step cuts system calls and TLB shootdowns everywhere, not only at
  burst starts; it should be measured on `tpchgen`, whose 9,400 `madvise` calls a run the
  screening found.
- **Recommend the background thread for servers**, on the evidence above, and say so in the
  preload library's and the crate's documentation.
- **Find the slow mode's class first** (the largest asmalloc-specific cost here): a probe
  build that counts slow-path exchanges per class and CPU, run on the RPC server until a
  slow-mode process appears; and emit CFI for the generated routines so `perf` can unwind
  through them.
- **For the suite's server tier**: rank allocators at 30% load, or with the server's cores
  kept out of C3; at 70% the tail ranks runtimes and idle states.

## Reproducing

```
cd bench/probes/loadgen && cargo build --release --target-dir ../../../target/server-tails/loadgen
python3 connect_log_body.py 50 > ../../../target/server-tails/log50.bin
scripts/measure --both-l3 python3 bench/probes/loadgen/serve.py run axum --plan day \
    --rates 90k,215k --rounds 1 --out target/server-tails/day      # one session per round
python3 bench/probes/loadgen/serve.py report target/server-tails/day
python3 bench/probes/loadgen/serve.py memory target/server-tails/day
python3 bench/probes/loadgen/analysis/bursts.py target/server-tails/day
```

`serve.py` takes `--custom PLAN` for other plans, `--wrap` for a command around the server
(`strace`, `setarch -R`), `--record` for `perf record` of the server, and the allocator
`asmalloc-bg` (the background thread, preloaded servers only). `SCREEN_ROOT` names the
screening's build of the order services. `analysis/strace_phases.py` and
`analysis/perf_windows.py` split a run's system calls and samples by phase and burst start.
