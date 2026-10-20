# Servers at partial load: the magazine-end lock, everywhere a worker stays put

Measured 2026-10-20 on the reference machine, asmalloc at `3847d9f` (allocator code as at
`fffde81`), with `bench/probes/partial-load/run.py`: the order service on actix (and on axum
to confirm), server on CPUs 0-3, `bench/probes/loadgen` on 8-11, 2 s closed-loop fill, then a
fixed rate (actix 280k req/s, axum 235k, the 70% rates of the servers suite) with 2 s of
warm-up and 8 s measured, one fresh server process per run, three rounds, allocators
interleaved, under `scripts/measure --both-l3`. Follows `servers.md`, which found that with
the server's cores held out of deep idle asmalloc spends about 10k cycles a request more than
jemalloc at 70% load.

## Summary

- **The extra work is the slow mode of `rpc-slow-mode.md`**: (CPU, class) pairs whose
  magazine is installed at an end and whose per-request swing crosses it, so they take one
  allocation and one free slow path every request. At 70% on actix with the cores held, 6 to
  7 allocation and as many free slow paths a request (about 25 locked pairs, each at one
  allocation and one free slow path per request on its CPU); with the idle states free, 0.33
  to 0.47. It is all in the worker threads and all in user mode: 17k to 33k user instructions
  a request more than with the cores free (jemalloc's do not move), and 8k to 14k user cycles a
  request more than jemalloc's in the same runs; by symbol, asmalloc's slow paths and heartbeat
  take 17% of the server's cycles held against 3% free (jemalloc's whole allocator 5%).
- **What decides it is whether a worker migrates, not the idle states.** With the idle states
  free but each actix worker pinned to its CPU, the pairs lock just as they do held: 5.2 to
  6.1 slow paths each way a request and 88k to 92k user instructions a request against
  jemalloc's 66k. The free, unpinned runs had 0.001 to 0.002 migrations a request (about
  4,000 in 8 s); held and pinned runs had none. A migration moves a worker's swing onto
  another CPU's magazines, where it starts away from an end, so occasional migrations keep
  breaking locks. Holding the cores stops the migrations, as pinning does. A server whose
  workers stay on their CPUs, the normal case under steady load, runs locked.
- **Centring the loaded magazine on a swap that reverses the last** (the `probe-center`
  experiment of `rpc-slow-mode.md`) **removes it entirely**: fewer than one slow path in
  2,000 requests on actix and axum, held. Its cost per request falls below jemalloc's:

| actix, 280k req/s, held | user cycles | kernel cycles | user instructions | p99.9 us |
|---|---:|---:|---:|---:|
| asmalloc | 41k to 48k | 27k | 85k to 99k | 65 to 445 |
| asmalloc, centring | 31.2k to 31.4k | 27k to 29k | 63.6k to 63.9k | 36 to 48 |
| jemalloc | 33.1k to 33.4k | 28k to 29k | 65.5k to 65.7k | 51 to 65 |
| asmalloc, `lean-slow` merged | 42k to 45k | 27k | 91k to 96k | 139 to 5,935 |

| axum, 235k req/s, held | user cycles | kernel cycles | user instructions |
|---|---:|---:|---:|
| asmalloc | 50k to 53k | 29k | 101k to 107k |
| asmalloc, centring | 39.8k to 39.9k | 31k | 77.1k |
| jemalloc | 41.3k | 31k | 79.4k |

- **How much it explains**: all of it. Centring takes asmalloc from 69k to 75k cycles a request
  (user and kernel) to 58.5k to 61k on actix, against jemalloc's 61k to 63k; on axum from 80k
  to 82k to 71k, against jemalloc's 72k. Each locked slow path costs about 1,000 cycles with its
  heartbeat (some 13 of them a request, 10k to 14k cycles).
- **The teammates' branches do not remove it.** `lean-slow` makes each slow path cheaper but
  leaves the lock: merged with main it measured 42k to 45k user cycles a request on actix, within
  plain asmalloc's range. `burst-start` changes how intervals are counted, which the lock does not
  depend on: no rule that runs once an interval ever installs a magazine in the middle.
  `rpc-slow-mode.md`'s centring is the remedy; it should land on top of `lean-slow`, whose cheaper
  slow path then pays for the swaps a centred magazine still takes when a swing really does run
  out.
- **The latency tail follows.** Locked runs miss slots (up to 22k of 2.2M) and reach a p99.9 of
  0.4 to 6 ms even held; centred runs miss none, with p99.9 36 to 48 us, below jemalloc's.

## Ruled out

- **Measurement.** Counted per thread: only the four `actix-rt|system` workers run during the
  segment, and the extra is in them. User against kernel: the extra is all user; asmalloc's
  kernel cycles are 1k to 2k lower than jemalloc's, from fewer context switches (its workers
  are busier). The generator held its rate (280,014 req/s) in every run; the spinners are the
  same processes for every allocator; the marks bound the same 8 s.
- **Migrations as a cost.** None in held runs; in free runs they are what keeps the cost away.
- **Faults and system calls.** Page faults are under 0.0003 a request in every mode; kernel
  time does not differ in asmalloc's disfavour.
- **The idle check and the clock.** The heartbeat's share (`beat`, `age_and_sweep`,
  `decay_step`, about 3.5% of cycles held) is paid per slow path; it is the price of the
  slow paths' number, not a separate cause.

## Method

- **Counters**: `run.py stat`, `perf stat --per-thread -e
  cycles:u,cycles:k,instructions:u,instructions:k,task-clock,context-switches,cpu-migrations,page-faults`
  attached to the server over the measured segment; `statsum.py` divides by the requests the
  generator completed in it.
- **Profiles**: `run.py record`, `perf record -g -F 4999`, 4 s; `perf report --no-children
  --sort sym`. Frame-pointer call graphs do not reach through the service (it is built without
  frame pointers), so the comparison is by symbol.
- **Slow-path counts**: the `probe-counts` build of `rpc-slow-mode.md` (commit `ce47dab`,
  cherry-picked onto a throwaway branch, with `asmalloc-probe` and `asmalloc-center` forwarded
  by `bench/apps/orders`), built by hand into `bench/target/asmalloc-probe` and
  `bench/target/asmalloc-center`; `run.py probe` reads `ASMALLOC_PROBE_COUNTS` from the
  server's memory at the segment's start and end, and `probesum.py 44` sums it per request and
  per pair. The centring numbers above are from that build, whose counters cost a relaxed add
  per slow path, which the centred build rarely takes.
- **Holding the cores**: as the suite does, a `SCHED_IDLE` spinner on each server CPU's SMT
  sibling (here a shell loop rather than the runner's `pause` loop). **Pinning**: each actix
  worker bound to one of CPUs 0-3 with `taskset -p` after the server starts (`--pin`).
- Raw files are in `bench/apps/target/partial/` of the main checkout (not kept in the
  repository).
