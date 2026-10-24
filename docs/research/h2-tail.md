# The Connect over HTTP/2 tail is the server CPUs waking from deep idle

2026-10-24, reference host (Ryzen 9950X, Linux 7.2.9, `acpi_idle` with the menu governor),
asmalloc at main `80643d2` (the standings' pad builds) against jemalloc. The question: why is
asmalloc's p99.9 on `connect log h2` at 70% load 45 us against 39 to 41 for every incumbent
(`standings.md`), while its p50 and p99 are the best?

## Summary

- **The slow 0.1% are requests that found the server CPU that would run them in a deep idle
  state.** This host's C2 has an 18 us exit latency (C3 declares 350 us). On a slow request,
  the first server thread to come on a CPU does so 22 to 28 us after the request came due,
  against 4 to 8 us on an ordinary one, and that CPU had been idle 20 to 60 us, long enough
  for the governor to pick C2 or C3. The extra 18 to 20 us is the exit.
- **The allocator does nothing during them.** Not one slow path, system call or page fault
  falls in this job (`server-profile.md`). It shows only through speed: the faster server
  idles longer between requests at the same rate, so its CPUs go deep more often.
- **Confirmed in one binary.** A build whose handler spins for a time set by an environment
  variable: with no spin, p99.9 is 47 to 48.5 us; with 1.5 us of spin (which puts its cycles
  a request at jemalloc's), it is 39.6 to 40.2, jemalloc's figure. Cycles a request differ,
  placement does not.
- **Confirmed at matched idleness.** asmalloc at 68k to 80k requests a second has a p99.9 of
  36.7 to 39.3 us, below jemalloc's at the ranked 63k. jemalloc at 52k gets the tail itself
  (53 to 56 us), and at 45k its CPUs go deep between almost every pair of requests (p99 61 us).
  The tail follows the idle gap, not the allocator.
- **Across allocators, how often a run lands in the slow mode follows speed.** In the
  standings, counting runs whose p99.9 is over 5 us above their p99: asmalloc (125.0k cycles
  a request) 10 of 15, mimalloc (130.4k) 2 of 15, tcmalloc (131.4k) 2 of 15, jemalloc (135.6k)
  0 of 15, glibc (143.6k) 0 of 5. At the last standings asmalloc was slower (127.6k) and had
  1 of 6. The lean `realloc` and the shorter class lookup made the server faster, and that is
  what moved this tail.
- **Only the server side.** The load generator's CPUs never enter an idle state (it
  busy-polls: zero entries on CPUs 8 to 11 in every run), so none of this is the client.
- **Nothing to change in the allocator.** The standings should measure tails with deep idle
  states off on the server's CPUs, or report idle entries beside each tail ("What the
  standings should do").

## What a slow request looks like

`bench/probes/h2-tail/trace.sh` runs the job under benchrun with every request's record kept
(`BENCH_RAW_DIR`), and beside it records every context switch on CPUs 0 to 3 and 8 to 11
(`perf record -e dummy --switch-events -k mono`, which needs no tracefs and takes no hardware
counter) and every CPU's idle-state counters every 5 ms (`cstate.py`, from
`/sys/devices/system/cpu/cpuN/cpuidle/stateK/usage`). Everything is on CLOCK_MONOTONIC, so
each request lines up with what the CPUs did.

For each request, the first server thread to come on a CPU that was idle when the request came
due (`idle.py`; asmalloc pad 48, 63k requests a second, 8 s traced, session 4):

| | first wake after due | idle time of that CPU by then |
|---|---|---|
| all requests that needed a wake (422k of 502k) | 4 to 10 us for 98% | under 20 us for 98% |
| slow requests, over 42 us (2,743) | 20 to 32 us for 74%; 42 to 48 us for 23% | 20 to 60 us for 81% |

The 42 to 48 us group pays the exit twice (two CPUs in turn). On jemalloc at the same rate,
idle periods before a server thread are under 15 us in all but about 300 of 500,000, so its
CPUs stay in C1 and POLL.

The slow mode comes and goes over seconds. Pieces of one second at the same rate (`cpieces.py`)
range from 0.06% to 1.1% of requests over 42 us within one run. Over 88 pieces of four runs,
C2 entries a second on the server CPUs correlate with the slow share at 0.71. In slow runs the server CPUs enter C1 1.4 to 1.7 times a request against 1.2 in clean ones
(tokio wakes a second worker that runs about 1 us and parks), and C2 entries rise from under
100 a second to 300 to 2,200. Cycles a request move by about
1% with the mode and instructions by less, so it is not more work.

## The confirmations

All runs: the `h2-tail` suite (the servers suite's day up to s70, then twenty one-second
pieces at the same rate; `runner-h2-tail.patch`), one benchrun run each, figures pooled over
s70 and the pieces (26 s, 1.6M requests at 63k), "slow" over 42 us, "deep" C2 and C3 entries
on CPUs 0 to 3 per thousand requests (`runsum.py`).

**One binary, handler spin** (`handler-spin.patch`, `BENCH_HANDLER_SPIN_NS` of equal length,
interleaved 0, 1500, 3000, 3000, 1500, 0):

| spin | k cycles a request | p99 | p99.9 | slow % | deep / k requests |
|---|---:|---:|---:|---:|---:|
| 0 | 126.1, 126.5 | 37.1, 37.0 | 48.5, 47.2 | 0.21, 0.16 | 29.1, 29.1 |
| 1.5 us | 135.1, 133.9 | 38.1, 37.7 | 40.2, 39.6 | 0.03, 0.04 | 21.1, 21.9 |
| 3 us | 142.9, 142.3 | 39.6, 39.0 | 41.7, 41.0 | 0.08, 0.03 | 21.2, 17.4 |
| jemalloc pad 0, no spin | 137.7 | 38.2 | 40.1 | 0.04 | 19.7 |

**Matched idleness: other rates** (`rates.sh`):

| | rate | p99 | p99.9 | slow % | deep / k requests |
|---|---:|---:|---:|---:|---:|
| asmalloc pad 48 | 63k | 36.9 to 37.2 | 45.9 to 49.9 | 0.14 to 0.38 | 29 to 44 |
| asmalloc pad 48 / 16 | 68k | 35.7 | 37.7 | 0.02 | 17.8 |
| | 73k | 35.9 / 36.6 | 38.0 / 39.3 | 0.03 / 0.07 | 17.2 / 16.9 |
| | 80k | 35.6 / 36.8 | 36.7 / 38.7 | 0.01 / 0.02 | 14.0 / 13.1 |
| jemalloc pad 0 | 63k | 38.2 | 40.1 | 0.04 | 19.7 |
| jemalloc pad 0 / 32 | 52k | 39.6 / 44.6 | 53.2 / 55.6 | 0.56 / 1.24 | 31 / 60 |
| jemalloc pad 0 | 45k | 60.7 | 65.0 | 45 | 235 |

The governor's choice is not monotone in the rate: at the suite's 27k (s30) every allocator's
CPUs stay in C1 and nobody has a tail. What matters is where a server's idle periods fall
against the 36 us residency C2 asks for, and that depends on the server's speed and on how
tokio spreads the requests over its workers.

**The direct switch, deep idle states off on CPUs 0 to 3, needs root** and was not run.
A SCHED_IDLE spinner on each server CPU, which would keep them out of idle without
privileges, is no substitute: woken workers waited behind it and 40 to 50% of every
allocator's requests went over 42 us (session 4).

## Ruled out on the way

- **HTTP/2 flow control.** connectrpc turns on hyper's adaptive window, which starts at 64 KiB
  against 8 streams of 15 KB bodies. The load generator now flags a request whose body waited
  for a window and logs every grant (`loadgen`, flags 8 and 16, `<raw>.win`). Windows grew to
  175 to 358 KB within 0.5 s on every connection, and 6 of 5.8 million requests ever waited.
- **The timer tick.** Requests due in the 25 to 50 us before each millisecond of
  CLOCK_MONOTONIC are about 1 us slower on every allocator: that is the tick, and it is
  small. The slow mode's requests spread over the first half of each millisecond.
- **Sampling profiles mislead here.** A cycles-sampling `perf record` beside benchrun's own
  counters is multiplexed with them every millisecond, and that rotation on every CPU's tick
  pushed 2.5 to 3.6% of jemalloc's requests over 42 us (session 2). Switch events with the
  dummy event cost nothing measurable.

## Why gRPC on the same server showed no tail

It does, less often. In this suite's longer runs two asmalloc builds had p99.9 of 45.9 and
41.9 us on gRPC over HTTP/2 (0.14 and 0.10% slow, deep entries 28.8 and 23.9 a thousand
requests), as Connect does in its slow mode. gRPC's extra trailers frame changes how the
work splits between the connection task and the stream task, and the standings' 15 runs
happened to land mostly in the fast mode. Not looked into further.

## What the standings should do

Tails at one common rate on this host mix idle-state exits into the comparison, and they
penalise the faster server, which idles longer. Proposed, for Bobby and the lead to decide:

1. **Measure tails with deep idle states off on the server's CPUs** (C2 and C3 disabled
   for CPUs 0 to 3 for the session, or a `/dev/cpu_dma_latency` request held by the runner).
   This takes the governor out of the comparison and needs root once per session. State the
   setting with the results. Server time would move too: in the spinner runs of session 4, where
   the CPUs never idled, the server ran 2 to 3% fewer cycles a request, so time and tails should come from the
   same setting.
2. **Without that, report idle entries beside each tail.** The counters are free to read
   (`cstate.py`); a run's deep entries a thousand requests says whether its p99.9 is an exit.
3. **Or compare tails at matched utilisation**, each allocator at the same fraction of its
   own saturated rate. This answers a different question (latency at equal headroom) and
   still leaves the governor in.

Rows of the current standings likely touched, not tested here: the gRPC h2 and HTTP/1.1
Connect rows (same server, same rates); the small servers' p99.9 (Sozu 77.6 against 68.6 to
74.3), whose loads leave CPUs idle between requests; and the log store's first requests
after a 30 s silence, where every server CPU has been in its deepest state (C3 declares
350 us). That row is put down to idle drains in `purge-off-path.md`, and exits may be part
of the 0.56 ms.

## The order service over HTTP/2's two modes

The standings saw asmalloc's builds at pads 48 and 64 run 63.5 to 66.3k cycles a request with
4 to 6% more instructions (and tcmalloc's at other pads). In this session three runs of the
same pad 64 binary and one of pad 48 all ran in the fast mode (60.3 to 60.8k, 105.6 to 106.2k
instructions, against pad 0's 60.5 to 60.9k and 105.4 to 105.5k). So the mode is not fixed
by the build. In these runs every allocator made 1.00 write-type system calls a request
(`/proc/PID/io`), its threads were scheduled in 0.50 to 0.53 times a request, and its CPUs
entered idle 0.27 to 0.40 times a request. In the standings' slow-mode runs, voluntary context
switches a request (0.52 to 0.55) sit inside the fast runs' range (0.49 to 0.55), and the
mode's extra instructions do not come with the idle-state mode above: on Connect h2 that mode
moves instructions by under 1%. I did not catch a slow-mode run, so what the extra
instructions are is not known. Nothing points at the allocator: the mode shows on tcmalloc
too, and the same asmalloc binary runs in either mode. Not related to the Connect tail.

## Method and files

Sessions of about ten minutes under `scripts/measure --both-l3`, with gaps as long; the
servers on CPUs 0 to 3 with spinners on their SMT siblings, as benchrun runs them; the
standings' pad builds at `80643d2`. Analyses under `scripts/heavy`.

`bench/probes/h2-tail/`: `trace.sh` (one traced run), `traces.sh` and `rates.sh` (a session of
them), `session.sh` (plain runs with records), `cstate.py` and `procs.py` (counter samplers),
`spin.py` (the SCHED_IDLE spinner that failed as a switch), `raw.py`, `runs.py`, `period.py`,
`phase.py`, `monophase.py` (the records), `flow.py` (flow control), `timeline.py`, `show.py`,
`idle.py` (switch events against requests), `cpieces.py`, `runsum.py`, `procsum.py`,
`modes.py` (tables), `ticks.py` (samples by phase of the millisecond), `runner-h2-tail.patch`
(the `h2-tail` suite: the longer day and the rate variants), `handler-spin.patch` (the
handler's spin). Raw results without the request records and perf data:
`docs/research/data/h2-tail-2026-10-24.tar.gz`.
