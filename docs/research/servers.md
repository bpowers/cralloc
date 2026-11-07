# What a real server asks of an allocator

What real Rust servers ask of an allocator, what share of their cycles is ours, and what sets their
tails. Real programs under synthetic load, on one machine (Ryzen 9 9950X, Linux 7.2.9): the server
on four CPUs, an open-loop load generator on four others, latency counted from when a request fell
due. "At 70%" means one fixed rate for every allocator: 70% of the slowest allocator's saturated
rate.

## What we know

- **A request asks for little, and mostly small.** About 140 allocations a request on the order
  service and the RPC jobs, three quarters of them in two classes. Half the blocks die within
  microseconds on the CPU that made them. In steady state there is almost no system call, page fault
  or large allocation, except on Meilisearch.
- **Our share is 5.0% of a server's cycles, jemalloc's 7.2%.** Geometric mean over seven jobs (x1.53
  and x1.47; ours ranges from 3.6 to 11.9%), 2026-10-22. Of user-mode cycles it is 7.2% and 10.2%.
  The kernel takes 20 to 46% on all but Meilisearch, level between allocators. A lean `realloc` has
  since taken our share to about 4.0% by subtraction, about 2% on the RPC jobs.
- **What is left is the fast path and `realloc`.** Slow paths and maintenance are under 0.6% of the
  cycles on every job, and our slow paths a third or less of what jemalloc spends on its own.
- **Good allocators differ in instructions, not stalls.** Across ten jobs (2026-10-28), against
  cralloc:

  | Allocator | Cycles a request (spread) | = Instructions | x Cycles per instruction | Cycles per extra instruction |
  |---|---:|---:|---:|---:|
  | jemalloc | 1.046 (x1.03) | 1.051 | 0.995 | 0.43 |
  | tcmalloc per-CPU | 1.045 (x1.04) | 1.060 | 0.986 | 0.45 |
  | mimalloc | 1.065 (x1.05) | 1.046 | 1.018 | 0.77 |
  | glibc | 1.197 (x1.13) | 1.137 | 1.053 | 1.16 |

  The eight jobs that allocate much run at 0.44 to 0.66 cycles an instruction, so jemalloc's and
  tcmalloc's extra code costs what any code costs. mimalloc adds stalls, and glibc's gap on the
  order service is mostly stalls (1.14 times the cycles per instruction).
- **Tails come in layers.** With deep idle states on, the tail at a fixed rate ranks the CPU's idle
  states, and a faster allocator gets the worse tail: it idles longer and sleeps deeper. With them
  off, jemalloc, tcmalloc and cralloc sit within 4% of each other at p50, p99 and p99.9 alike: a
  tail is service time plus an excess that belongs to the program. Past that come the program's own
  events, such as a hash table doubling under a lock.
- **The tails that were ours were maintenance on a request's path.** The magazine-end lock, purge
  work on the first requests after a silence and on a large call: all removed. One remains: on the
  log store the first 5 ms after a silence reach 581 us against jemalloc's 279, from idle drains.

**For the design.** Spending on the fast path and `realloc` pays on every server; spending on slow
paths pays almost nowhere. Returning memory belongs on the background thread, never on a request. A
per-CPU cache suits this load so well that the only way to lose is to tear it down: a lock at its
ends, or a drain before a burst.

## The jobs

- **Order service** (`orders`): axum or actix over HTTP/1.1 or HTTP/2, `POST /orders` of 835 bytes
  of JSON, 64 connections, keeping the last 100,000 orders (about 1.9 KiB live each), or up to 5.6M
  in the large-heap jobs, so each request allocates a new order and frees the oldest.
- **Connect log** (`connect log`): connect-rust's log-ingest service, 50 records and 22 KB of
  protobuf a request, Connect over HTTP/1.1, Connect or gRPC over HTTP/2.
- **Log store** (`connect store 1M`): the same server holding a million owned records (1.06 GiB
  live), alternating an ingest of 50 with a streamed tail of 100.
- **mini-redis**, **Sōzu** (a reverse proxy, four workers and a main process), **Meilisearch** (400k
  Hacker News items, searches with a batch of updates every 200th request), **Qdrant** (a million
  384-float vectors, in RAM), the **Apollo Router** (GraphQL over four subgraphs), **SurrealDB**
  (750k records in memory).

Each server's cores are held out of deep idle by an idle-priority spinner on each SMT sibling, and
since 2026-10-25 C2 and C3 are off as well. The spinner shares the core, which raises jemalloc's
cycles a request on the order service by 17%, alike for every allocator.
[measuring.md](measuring.md) has why.

## What a request asks

Counts from a scratch build at the same rate as the timed runs, never timed itself, 2026-10-22 at
040033a. "Crossing" is blocks freed on another CPU, 1 in 64 followed by address. Median life is from
a 50 to 100 ms window, which favours short lives.

| Job | Allocations a request | Commonest classes | Bytes | Reallocs | Crossing | Median life |
|---|---:|---|---:|---:|---:|---:|
| Order service, HTTP/1.1 | 138 | 8 B 59%, 16 B 19% | 13 KB | 8 | 20% | 0.2 us |
| Order service, HTTP/2 | 140 | 8 B 59%, 16 B 19% | 6.8 KB | 8 | 25% | 0.2 us |
| Order service, 2 GiB | 138 | 8 B 59%, 16 B 19% | 13 KB | 8 | 28% | 0.2 us |
| gRPC log, HTTP/2 | 136 | 128 B 38%, 48 B 37% | 87 KB | 55 | 0% | 9 us |
| Connect log, HTTP/2 | 135 | 128 B 38%, 48 B 37% | 64 KB | 55 | 0% | 10 us |
| Log store | 2,610 | 24 B 44%, 16 B 36% | 223 KB | 34 | 15% | 2 us |
| Meilisearch | 49,600 | 8 B 56%, 160 B 17% | 5.7 MB | 1,250 | 0.3% | 0.2 us |

- **Reallocs move a block one class up.** Every one on the order service and RPC jobs: `Vec`
  doubling, 50 buffers a request from 128 to 256 bytes on gRPC, 14.5 KB copied.
- **Only Meilisearch makes large allocations:** four a request, mostly 128 KiB to 16 MiB, living 0.1
  ms at the median and 42 ms at p99, 99% freed by the thread that made them.
- **Holding gigabytes changes no call.** The 2 GiB order service makes the same calls as the small
  one. Page faults are zero on the order service and RPC jobs, 0.003 a request on the store.

So several things the kernel benchmarks punish do not appear on a server, and are **not worth doing
for servers**: a per-CPU cache or batched frees for large blocks, avoiding refaults of large blocks
(Meilisearch only, 0.3% of cycles), table growth (the order service's one doubling is below). Frees
on another CPU cost us nothing visible: a free is a push on the freeing CPU.
[magazines.md](magazines.md) has how seldom load reaches a slow path.

## What share of the cycles is ours

Share of the server's cycles by path, two runs a job and allocator, 2026-10-22 at 040033a, jemalloc
as tikv-jemallocator builds it. `perf` samples are placed by instruction with line tables, so the
inlined fast path is counted. A fast-path figure is good to about a fifth of itself; realloc and
slow-path figures are better. Totals are one build's.

| Job | k cycles a request | Fast path | realloc | Slow paths, maintenance | cralloc, all | jemalloc, all | Kernel |
|---|---:|---:|---:|---:|---:|---:|---:|
| Order service, HTTP/1.1 | 64.1 | 2.88 | 0.91 | 0.00 | 3.80 | 4.99 | 45.6 |
| Order service, HTTP/2 | 64.6 | 4.00 | 0.99 | 0.02 | 5.01 | 5.86 | 27.7 |
| Order service, 2 GiB | 67.2 | 2.68 | 0.90 | 0.00 | 3.58 | 4.81 | 45.0 |
| gRPC log, HTTP/2 | 132.7 | 1.26 | 2.56 | 0.00 | 3.82 | 6.93 | 31.0 |
| Connect log, HTTP/2 | 131.3 | 1.11 | 2.51 | 0.00 | 3.63 | 6.54 | 31.3 |
| Log store | 366.9 | 10.59 | 0.95 | 0.33 | 11.87 | 15.23 | 19.6 |
| Meilisearch | 16,564 | 5.32 | 0.79 | 0.58 | 7.38 | 10.36 | 5.2 |
| Geometric mean (spread) | | | | | 5.0 (x1.53) | 7.2 (x1.47) | |

Meilisearch's `cralloc, all` also holds zeroing and `madvise`. Geometric means are over the seven
rows; of user-mode cycles they are 7.2% (x1.37) and 10.2% (x1.30). Three more servers were profiled
under jemalloc or glibc alone: the Router 8.4% plus 2.7% in Rust's allocation shims, SurrealDB 11.5%
plus 3.2%, Qdrant under glibc 1.8 to 2.5%.

- **jemalloc's share is 1.43 times ours (x1.17), and the program's and the kernel's cycles are
  nearly level.** So the gap between the two is their own code, not an effect on the program.
  [program-effects.md](program-effects.md) has the misses.
- **A pair costs 14 cycles on a server** (geometric mean over the seven jobs, x1.20; 11 to 18),
  against about 4 inlined in a bench ([roofline.md](roofline.md)) and 18 for jemalloc (x1.23). The
  alloc and free split moves with skid; the sum is firmer. On calls whose layout is not a constant,
  most of the store's and Meilisearch's, the class, slab word and mask took about 10 instructions
  beside the routine's 11.
- **`realloc` was the largest cost left**, 2.5% of the RPC jobs: 60 to 105 cycles a call and about
  240 instructions besides the copy, 100 of them computing size classes the slow way. It is now one
  lookup for each class, a pop, a copy and a push (`heap/resize.rs`): a `Vec` growing from 128 to
  256 bytes costs 126 instructions and 23 cycles against 354 and 63. That is 12,200 instructions a
  request on the RPC jobs and about 2% of their cycles over five code placements; the order service
  gains 0.7 to 1.2%, not resolved.
- **Today's share, by subtraction.** Taking 40 cycles a call off each job's reallocs gives 4.0%
  (x1.8) as the geometric mean and about 2.1% on the two RPC jobs. Not re-profiled.
- **The non-constant lookup is now 8 instructions, not 10** ([roofline.md](roofline.md)), 1.6% on
  the store at the mean over placements. The profile's other suggestion, one table load giving word
  and mask together, makes constant-size calls slower and was rejected.

## Whether a gap is instructions or stalls

Cycles and instructions a request, 2026-10-28 at 2ff7322, five code placements of every build, two
or three runs each (glibc one), medians per placement and the mean over placements. Cycles a request
= instructions x cycles per instruction, an exact split. Cycles and instructions count user and
kernel. Cycles per extra instruction is the extra cycles over the extra instructions.

| Against cralloc: cycles = instructions x CPI | Connect log (3 jobs) | Log store | Order service (4 jobs) |
|---|---:|---:|---:|
| jemalloc | 1.093 = 1.096 x 0.997 | 1.070 = 1.067 x 1.002 | 1.027 = 1.040 x 0.987 |
| tcmalloc per-CPU | 1.062 = 1.046 x 1.015 | 1.132 = 1.190 x 0.951 | 1.033 = 1.069 x 0.966 |
| mimalloc | 1.055 = 1.047 x 1.007 | 1.214 = 1.116 x 1.088 | 1.073 = 1.051 x 1.021 |
| glibc | 1.163 = 1.174 x 0.991 | 1.485 = 1.447 x 1.026 | 1.272 = 1.116 x 1.139 |
| cralloc's own instructions per cycle | 2.25 | 1.97 | 1.52 to 1.79 |

mini-redis and Sōzu are controls: every allocator within 1% on both axes.

- **jemalloc's gap is all instructions** on every job. Its extra code runs at 0.32 to 0.53 cycles an
  instruction, no worse than the server around it.
- **tcmalloc runs more instructions than its cycles show.** Its extra code costs 0.22 to 0.35 cycles
  an instruction on the order service and the store, 3 to 4.5 instructions a cycle, which is why the
  ratio of cycles per instruction is under 1. A naive "stall share" comes out negative here; the
  marginal figure is the honest one.
- **mimalloc pays stalls on the order service over HTTP/1.1 and the store**, about 1 cycle per extra
  instruction, and instructions only on Connect.
- **glibc is two different gaps.** On Connect and the store its 15 to 49% is instructions. On the
  order service it is 1.2 to 2.0 cycles per extra instruction: at 70% its CPUs are busy 3.6 to 3.7
  of four where cralloc's are 2.9 to 3.2, and at saturation it has many times the others' context
  switches, which points at lock waits (not separated from misses).
- **Saturation agrees.** Saturated rate against cralloc's, pad-0 builds, three rounds, geometric
  mean over the ten jobs: jemalloc 0.954 (x1.04), tcmalloc 0.949 (x1.05), mimalloc 0.941 (x1.07),
  glibc 0.806 (x1.14). cralloc has the highest rate, or is level with it, on every job that
  allocates much.

So placement barely separates good allocators; it matters where an allocator gets it badly wrong.
The misses by source are in [program-effects.md](program-effects.md).

## What sets the tails

### At one fixed rate a tail can rank idle states, not allocators

The machine's deep idle states take 18 us (C2) and 350 us (C3) to leave. At a fixed rate the faster
server idles longer between requests, so its CPUs go deeper, and the wake-up lands in the tail.

- **With every state free, the 70% tail is the machine and the runtime.** p99.9 is 2 to 4 ms for
  every allocator, and glibc, the slowest, had the best on actix. tokio parks all but one worker for
  milliseconds at a time, then all four catch up. Kept out of C3, p99.9 falls to 30 to 110 us for
  every allocator (2026-10-20, two rounds).
- **The fine version sits in tens of microseconds.** cralloc's p99.9 on Connect over HTTP/2 read 45
  us against 39 to 41 for every incumbent, with the best p50 and p99. The slow 0.1% found the CPU
  that would run them in C2: the first server thread comes on 22 to 28 us after the request fell
  due, against 4 to 8, on a CPU idle 20 to 60 us. The allocator does nothing during them. HTTP/2
  flow control is ruled out (6 of 5.8M requests waited for a window), and so is the timer tick
  (about 1 us).
- **Confirmed in one binary.** A handler spinning 1.5 us, which puts cralloc at jemalloc's cycles a
  request, takes p99.9 from 47 to 48.5 us to 39.6 to 40.2. At matched idleness the tail swaps
  owners: cralloc at 68k to 80k req/s has 36.7 to 39.3 us; jemalloc at 52k has 53 to 56.
- **How often a run lands there follows speed.** Runs with p99.9 over 5 us above p99: cralloc
  (125.0k cycles a request) 10 of 15, mimalloc (130.4k) 2 of 15, tcmalloc (131.4k) 2 of 15, jemalloc
  (135.6k) 0 of 15, glibc (143.6k) 0 of 5. C2 entries a second correlate with the slow share at 0.71
  over 88 one-second pieces.
- **The direct switch settled it.** With C2 and C3 off on the server's CPUs (2026-10-25, 12 runs
  over five placements) cralloc's p99.9 fell from 45.1 to 37.8 us, the lowest, and nothing else
  moved by more than 0.5 us. Cycles a request and saturated rates moved under 1%, so cycles from
  before and after compare. Numbers from that date on are with deep idle off, and the reports flag a
  tail comparison it confounds (`scripts/deep-idle`).

The confound has a second part: "70%" of the slowest allocator is 32 to 41% of cralloc's own
capacity on the Connect jobs and 61 to 63% on the order service over HTTP/1.1, and 81 to 86% of
glibc's on the order service over HTTP/1.1 and the store. (Computed: the rate over each allocator's
saturated rate, 2026-10-28.) So glibc's millisecond tails there are partly load.

### With deep idle off, a good allocator's tail follows its service time

Medians over runs at 70%, 2026-10-28 (490 runs, zero deep entries); ratio to cralloc, geometric mean
and spread over the ten jobs:

| Allocator | p50 | p99 | p99.9 |
|---|---:|---:|---:|
| jemalloc | 1.028 (x1.02) | 1.028 (x1.03) | 1.029 (x1.03) |
| tcmalloc per-CPU | 1.029 (x1.03) | 1.035 (x1.04) | 1.024 (x1.05) |
| mimalloc | 1.038 (x1.04) | 1.092 (x1.12) | 1.33 (x1.34) |
| glibc | 1.199 (x1.19) | 1.91 (x3.1) | 3.8 (x7.2) |

- **The excess of p99.9 over p50 is the program's.** On Connect over HTTP/2 it is 8.7, 8.9 and 8.6
  us for cralloc, jemalloc and tcmalloc; on the order service over HTTP/1.1 8.1, 7.8 and 8.1. So the
  allocator moves their tails only by moving service time: cycles a request and p50 rank the five
  allocators alike (rank correlation 1.0) on seven of the eight jobs that allocate much. On the
  order service over HTTP/2 every allocator has one p50.
- **mimalloc has a tail of its own** on the order service, 1.6 to 2.2 times cralloc's p99.9, and on
  the store 1.45 times. The cause was not studied; on the order service its runs show no compaction.
- **glibc's tail is lock waits and load.** p99.9 of 8.9 ms on the store and 9.2 ms on the 2 GiB
  order service, near its capacity on both.
- **Single runs still reach milliseconds** for every allocator (cralloc's highest orders run 487 us,
  jemalloc's 2,437). Unexplained, and the medians are robust to it.

### The program's own events

- **A hash table doubling under a lock: the 4 GiB tail.** Nine tenths of the slow requests in a
  minute at 4 GiB (93 to 96%) fall in 59 to 67 stalls of about 2 ms, for every allocator. The order
  service's 64 shards are each a `HashMap` behind a mutex, filling in step; once churn uses up its
  growth allowance a table more than half full doubles, rehashing 35k orders under the lock, and
  every connection on a worker that needs that shard waits. Pre-sizing takes p99.9 from 95 to 165 us
  to 26. As a job of its own, medians of five, with 545 faults a table under every allocator: p99.9
  jemalloc 219 us, tcmalloc 209, cralloc 282, mimalloc 18,707, whose doublings wait up to 22 ms in
  direct compaction. cralloc's excess was purge work, in the table below.
- **Meilisearch's 44 ms is its indexing**, spread over the searches: p99.9 44.6 ms for cralloc and
  jemalloc, 47 to 58 for the rest (2026-10-28). Its run-to-run spread is about 10 ms, and two builds
  of one behaviour differ by 1 ms, so a change needs a switch in one binary
  ([measuring.md](measuring.md)).
- **Qdrant's p99 of about 200 ms is upserts** waiting for their write behind the update queue, for
  every allocator.
- **Unexplained, for every allocator:** stalls of 0.2 to 1.2 ms every 1.024 s in the first 15 to 25
  s of a minute at 4 GiB.

### Tails the allocator caused

| Tail | Size | Cause | What removed it |
|---|---|---|---|
| Magazine-end lock ([magazines.md](magazines.md)) | p99.9 0.4 to 6 ms, up to 22k of 2.2M slots missed | a swing across a magazine's ends, once workers stay on their CPUs | half-full rule: 36 to 48 us, below jemalloc's 51 to 65 |
| Maintenance at a burst's start | first 0.5 ms 151 us at the median, jemalloc 35 | a heartbeat finds everything a minute old | pacing by bytes; counted intervals |
| Purge steps on a large call | up to 180 us on 13 of 64 doublings of 0.8 ms | one large call in 16 beats and runs a pass and a hold look | returning memory moved to the thread: p90 level with jemalloc's |
| Pages returned in a silence | 380 faults in a burst's first 5 ms, slowest 0.9 to 1.0 ms | the thread's own drains completed pages its pass then returned | the floor read against what could be returned; silence ages nothing |
| Idle drains | first 5 ms, slowest 581 us, jemalloc 279 | every CPU refills from the bitmaps | not removed (below) |

Moving purge to the thread has no tail cost: in one binary switching it at run time, on Meilisearch,
14 paired rounds of two minutes, p99 moves -0.9 ms (95% interval -2.1 to +0.5) and p99.9 0.0 (-2.3
to +2.3), with TLB shootdowns the same (2026-10-22). Within that binary page faults read 5% higher
(87 against 83 a request) at no cost in latency, but the separate builds of the two behaviours are
level at 82.5, so whether the rise is real is not settled. The thread then wakes 25 times a second
at 70% and does about 9 ms of work a second there.

### The first requests after a silence

A silence stops all slow paths, so maintenance waits, and the first requests pay if any rule lets
them. Three things had to change, and the measurements say which did the work.

**The first heartbeats after a silence used to do it all.** On the order service, 337 `madvise` and
14 `membarrier` calls in the first 3 ms of eight bursts (hundreds of one-page purges and a dozen
drains); 9.5 request-ms of extra waiting a burst against jemalloc's 3.5. Slowing those calls under
strace tripled the cost on connect-rust's server and left jemalloc's as it was, so they were on the
requests' path.

- **Pacing by bytes alone removes it on these servers** (3.4 against jemalloc's 3.2 request-ms),
  since a server in steady state then takes almost no heartbeats.
- **Counted intervals** then add nothing measurable there. They keep any heartbeat that does run
  from finding everything a minute old: a silence of any length ages things by one interval
  (`clock.rs`), and each rule has a test that fails on the raw clock.

**Then the thread had the same fault during the silence.** Log store, first 5 ms of a burst,
2026-10-21. The clocked pass returned 4.3 MiB each silence, pages its own drains had just completed,
and frontiers had expired because the thread's rounds stepped the clock:

| Configuration | Returned in the silence | Faults | Slowest request | Extra waiting |
|---|---:|---:|---:|---:|
| No drains, no purges | 0 | 3 to 10 | 0.27 to 0.51 ms | 2.8 to 6.4 request-ms |
| Drains, no purges | 0 | 21 to 28 | 0.53 to 0.58 ms | 6.8 to 8.8 |
| Purges, no drains | 0.1 to 0.3 MiB | 12 to 35 | 0.26 to 0.36 ms | 3.0 to 3.5 |
| Both | 4.2 to 4.5 MiB | 362 to 392 | 0.90 to 1.04 ms | 19 to 36 |

The pass read a floor against 185 MiB retained, of which 1.4 MiB lay on wholly free pages; the rest
is free slots beside live neighbours, which nothing can return. So now the floor is measured against
what a lap found returnable, and only intervals in which a heartbeat ran age a frontier
(`back/purge.rs`). That leaves 21 to 27 faults.

**Idle drains are what is left.** A drain empties each idle CPU's magazines into the bitmaps, and in
the burst's first 5 ms every drained magazine is refilled at once: 3,240 claims against 789 with the
thread off, slowest 0.68 to 0.86 ms against 0.29 to 0.39. On 2026-10-28, deep idle off, so this is
not an exit: 581 us (538 to 638), against jemalloc 279, tcmalloc 359, mimalloc 542, glibc 725, over
182 requests a burst. On this job the drain buys almost nothing: the thread returns nothing in the
silence and the heap at rest is 1,419 MiB against 1,425. The order service at 2 GiB never starts the
thread (151 us against 135); Meilisearch's first five requests reach 1.9 to 3.2 ms under every
allocator, so the drains do not show.

Not built: draining a quiet slab only after several counted intervals, or parking its magazines in
the depot rather than laundering them. Each needs measuring on both a job like this and one whose
parked objects pin pages.

## What returning memory costs a request

A `madvise(MADV_DONTNEED)` is cheap in pages and dear in calls. One thread faulting in a run and
returning it, 600 times per length, alone and beside three threads of the same process, 2026-10-20:

| Pages a call | Alone, us a call | Us a page | With other threads, us a call | Us a page |
|---|---:|---:|---:|---:|
| 1 | 0.36 | 0.36 | 1.80 | 1.80 |
| 4 | 0.36 | 0.09 | 1.80 | 0.45 |
| 16 | 1.12 | 0.07 | 2.83 | 0.18 |
| 64 | 3.27 | 0.05 | 5.56 | 0.09 |
| 512 | 27.4 | 0.05 | 30.3 | 0.06 |

In a multithreaded process a call costs about 1.7 us before it returns anything, so one page costs
what 35 do. Under a loaded server and strace the calls measured 10 to 13 us.

- **Free pages were adjacent.** Connect-rust's server made 1,281 calls for 289 runs. So the walk
  joins adjacent pages, up to 64 a call, and the clocked pass takes only runs of at least 4, with
  one pass in 8 taking any length, since a single page among live ones is the likeliest to be
  refilled. Calls: connect-rust 443 to 43 a run, DataFusion 3,900 to 1,900, tpchgen 7 to 10% down
  (its calls are large blocks). A first setting of runs of 8 and one pass in 16 saved few more calls
  and left twice the memory waiting after a peak, 27 MiB of 163 after 12 s against a bound of 20.
- **`maintain` in steps.** Timed a step at a time beside four busy threads: p50 1.2 us, p99 35 us,
  worst 316 us. Every step over 250 us is one `madvise` of a single free slot of a large class
  (2,560 pages, 10 MiB, 0.15 us a page). A drain takes up to about 190 us, a shed step 50 to 100 us.
- **The floor.** Tens of megabytes cost a server nothing that matters, and returning them costs
  calls and the next burst's refaults. burst-idle settles at the 32 MiB floor from 6 s on. Above it,
  passes follow [memory.md](memory.md).

## The background thread

It starts by itself once free memory passes the floor and the program has a thread of its own;
`docs/design.md`, "Heartbeat", has why. What it did on three large heaps, 2026-10-21 and 28, same
binary with it on and off:

| | Meilisearch | Log store | Order service, 2 GiB |
|---|---:|---:|---:|
| Starts | during indexing | 1.4 s into the fill | only at shutdown |
| Wake-ups a second at 70%; CPU | 20; 2.7 ms a second | 2; 0.3 ms a second | |
| Parks into a silence, after | 22 to 25 s | 13 to 14 s | |
| Heap a minute after close, on / off, MiB | 662 / 878 (jemalloc 669) | 1,409 / 1,413 | same program |
| Cycles a request, on / off | 17.08M / 17.08M | 351k / 353k | same program |

- **It pays on Meilisearch**: it returns 216 MiB of free large-class pages nothing else would. Work,
  tails and peak under load do not change.
- **A silent process is not woken by its allocator.** It parks after ten quiet rounds in a row and
  then makes no system call. A burst freed whole goes back to the floor in about five seconds. A
  heap keeping 200 MiB of free slots beside live neighbours parks too; before review its thread woke
  twice a second for ever. Cases by process, one test each (`tests/default_thread.rs`): a worker's
  frees, the main thread's frees of a worker's blocks, a fragmented heap, a fork child of a parked
  parent, a refused thread creation. On musl the evidence is a free by a thread other than the first
  ([portability.md](portability.md)).
- **It costs a large allocation and free 1.5 to 2 cycles a pair** (under 1%), and 7 in a loop
  alternating two large classes, which moves with code placement, not with what the code does.
- **`maintain` through a silence, from a task or the thread,** costs the order service's next burst
  about 2 request-ms (5.3 to 5.7 against 3.6) for 1 to 2 MiB of 200, in a build that started the
  thread there by hand. connect-rust's server goes from 5.4 to 1.5 MiB at no cost.

## What each program taught us

**The order service** found the magazine-end lock and shows the kernel at 45% of a request over
HTTP/1.1, 28% over HTTP/2. Heap size does not move it: 67.8 to 68.2k cycles a request from 0.5 to 4
GiB, and the same at 10 GiB ([memory.md](memory.md), "The heap's fixed numbers hold to 10 GiB").
Cached blocks cross CPUs freely and create no flow. Over HTTP/2 it has a second mode, 4 to 6% more
instructions, seen under cralloc and tcmalloc at some placements; the same binary runs in either, so
nothing points at the allocator, and no run was caught.

**The Connect log service** made `realloc` the largest cost and showed the tail above. A build's
number stands for any point in 2.5 to 5%, since its protobuf decoder is placement-sensitive
([program-effects.md](program-effects.md)). snmalloc takes 1.7 to 2 times every other allocator's
cycles here, in both idle modes, and is mid-field elsewhere.

**The log store** is our largest share and our clearest lead: 7% over jemalloc, 13% over tcmalloc,
21% over mimalloc, 49% over glibc, with the best tail (126 us against 136 to 184). Each record is
about 20 small blocks freed on whichever worker ingests its replacement. jemalloc makes 40 `madvise`
calls per 1,000 requests here and faults them back; cralloc 0.16. The fast path is 10.6% of its
cycles, so it is where a cheaper path would show first.

**mini-redis** is a control, all allocators within 1.5%, and it showed the lock in its purest form:
two classes, one slow path each way a request, twice jemalloc's user instructions (12.5k against
6.4k).

**Sōzu** parses through buffers from its own pool, so the allocator is hardly called: a control on
time, within run-to-run spread (up to 7%). It is a fixed-cost memory job, five processes each
paying: over glibc, cralloc 2.5 MiB a process, jemalloc 1.4, tcmalloc 4.8, snmalloc 7.9, mimalloc
8.1 (44 of its 52 MiB in huge pages).

**Meilisearch** is the job where most of the "heap" is not ours: of 3.5 to 4.6 GiB resident, 2.8 is
its LMDB store, and 560 to 590 MiB of the anonymous rest is glibc arenas filled by C code. Cycles a
request 16.76M, the least (jemalloc 17.42M, tcmalloc 17.07M, mimalloc 17.97M, glibc 17.96M), 42
faults a request, level with jemalloc after the hold changes ([large-blocks.md](large-blocks.md)).
tcmalloc's sized-delete check caught our suite's wrapper keeping a block in place on `realloc`, so a
sampled block was freed as 8 bytes against the 2 it was first asked for; the wrapper now copies, as
`tcmalloc-better` does.

**The Router** builds, merges and serialises JSON trees: 2.6 to 2.7M cycles a request. cralloc does
the least work, tcmalloc +1.2%, mimalloc +3%, jemalloc +6% (which it ships with), glibc +38%, over
five placements with ranges apart (2026-10-24). cralloc's out-of-line code is 3.7% of cycles; its
inlined path was not separable in that build. glibc saturates at half the others' rate with its CPUs
asleep (1,273 deep-idle entries a thousand requests against under 1), which suggests lock waits on
blocks built on one worker and dropped on another; not profiled. Not placement-sensitive: 0.3 to
0.9% a build. It peaks near 100 MiB, so memory ranks nothing.

**SurrealDB** has the widest gap of any program server: tcmalloc +7.0%, mimalloc +12.6%, jemalloc
+13.6% (which it ships with), glibc +27.2%, and p99.9 2.7 ms against 3.3 to 3.6, at 750k records
(2026-10-25). Many small allocations a request over a heap of gigabytes; jemalloc's refills of small
bins show as 3.7% of cycles. It holds 4 to 5% under jemalloc and tcmalloc, and the heap only grows,
so the tail rule has nothing to return. jemalloc, mimalloc and glibc are single builds of a fat-LTO
program, which a placement could move by a few percent.

**Qdrant** is a control on time: malloc is about 2% even with glibc, and dot products and graph
traversal 60 to 70%. Cycles a request rank the run's state, not the allocator. Qdrant routes
searches between two runtimes by observed CPU use, each run settles on one, and runs fall on three
levels of work (11.9, 12.5, 12.9M cycles) with instructions moving alike. On disk the job followed
the disk's load, in clusters that hit every allocator, and threads waited on page faults in its
payload store: a first 7% gap against cralloc was one such cluster, so its storage now lives in RAM.
Memory is its axis, and jemalloc is the smallest: heap mean / peak / a minute after close was
jemalloc 268 / 449 / 131, cralloc 337 / 528 / 195, tcmalloc 416 / 474 / 417. Most of our extra was
64 and 128-byte pages stranded in partly used 2 MiB units; the tail rule took cralloc to 394 / 483 /
139 ([memory.md](memory.md)). The rule's advice changes on a live server are mostly 5 to 7 us; the
longest, 4.8 to 5.2 ms over 4 GiB, waited on the address-space lock with 2 to 8 requests in flight,
and no request at p99 or above overlapped one.

## Memory on servers, briefly

[memory.md](memory.md) has the account. What servers add: on steady large heaps cralloc holds the
least over live, pre-sized order service at 2 and 4 GiB 1.080 and 1.074 against jemalloc's 1.097 and
1.095, tcmalloc's 1.098 and 1.096, mimalloc's 1.155 and 1.151. No ratio drifts by more than 0.016 in
a minute of churn, ours by at most 0.009. The log store holds 1.31 times live against 1.38 to 1.42.
A preload's peak (1.19 times live on mini-redis at 0.5 GiB) goes back within seconds, at about half
the excess a second, and the cgroup peak is what a limit must cover. Huge pages do not decide a
server's cycles: cralloc is fastest on the order service with less coverage than mimalloc (0.72 to
0.93 of resident against 0.91 to 1.00), and on mini-redis mimalloc's 0.8 to 1.0 against our 0 to
0.17 buys 1 to 1.5%.

## Open questions

- **Beyond four CPUs.** Every server was pinned to four. [magazines.md](magazines.md) says what that
  hides.
- **Real traffic.** These are real programs under synthetic load: fixed request mixes, no slow
  clients, no deploys.
- **Today's shares.** The profile predates the lean `realloc`, the shorter lookup and the thread's
  rework. 4.0% is a subtraction.
- **mimalloc's own tail** on the order service, and single-run stalls of milliseconds under every
  allocator.
- **Matched utilisation.** No tail here compares allocators at one fraction of their own capacity,
  which would separate glibc's load from its locks.
- **The Router, SurrealDB and Qdrant** were last measured on 2026-10-24 to 27, deep idle on. Their
  tails are in milliseconds, which an 18 us exit cannot move, and their cycles should stand.
- **Why Connect over HTTP/2 goes into its idle mode more than gRPC**, whose extra trailers frame
  changes how work splits between the connection and stream tasks. Not looked into.
- **The Router with many distinct operations.** Three operations here; a production graph would hold
  a larger query-plan cache.
- **The 2 to 3% of saturated rate** left between centred and never-locked processes
  ([magazines.md](magazines.md)).
- **One machine.** Everything above is one CPU model, kernel and idle driver.
