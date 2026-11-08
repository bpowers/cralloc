# How we measure, and what has fooled us

Each trap below misled us, and most reached a note as a wrong conclusion. The rules that came from
them:

- **Never compare two builds by one number each.** Switch the behaviour at run time in one binary,
  or compare means over a sweep of code placements. One build stands for any point in a range of 1
  to 7%, by job.
- **A tail at one fixed rate can rank idle states.** Read the deep-idle entries beside it, or
  measure with deep idle off. A difference the report flags cannot be read as the allocator's.
- **Warm the clock before a run, keep other work off the job's cores and L3, interleave what you
  compare, and run a control.**
- **Do not multiplex counters or group sampled events.** Take data addresses from IBS.

The runner enforces most of this. [`bench/README.md`](../../bench/README.md) documents it; this note
says why. Everything was measured on the Ryzen below; "On the M1 Max" says what differs on the other
machine we have.

## The traps

### One number from each of two builds measures where the code landed

**What we concluded.** That cralloc was 17% behind glibc on the 4 MiB grow loop, a figure taken with
its loop in a good place. That moving to tokio 1.54 cost cralloc 3.6% on `connect log grpc h2`
through tokio's run-queue headers. That striding whole magazines cost the RPC log jobs 2 to 3% and
striding first fills 1 to 2%, which is how the sibling stride's bound of 64 objects was chosen. All
separate builds, interleaved, several runs each.

**How we found out.** A later build of the same source ran the grow loop 40% slower, 1,765k cycles a
repetition against 1,262k, at the same instructions and counters, with the loop byte for byte the
same; an unrelated edit had moved it. Builds of one stride rule that differed only in code ranged
from 3.9% below main to 1.5% above it, with instructions level. A function of N bytes that never
runs, ahead of all other code, moves it by exactly N: N = 0 to 64 ran 126.9 to 132.4k cycles a
request. With the stride switched at run time inside one binary: off 129.3k, 16 siblings 129.0, 64
siblings 130.1, whole magazines 129.3. The headers were worth nothing measurable there, and the
bound of 64 rests on nothing.

**Why.** The slow builds starve the front end: 38k more empty dispatch slots at eight a cycle make
4.8k cycles, the whole difference between a slow and a fast build. Over 90% of the difference in
sampled op-cache misses falls in the service's generated protobuf decoder. That is where the samples
fall, not a forced result: the pad moves all later code at once and the decoder was not moved alone,
and cralloc's own `realloc` is among the movers. The allocator's code comes first in the binary, so
any change to its size moves the decoder. The period is longer than a line, so no alignment fixes
it.

**How large, today** (2026-10-28 at `e3753b5`, five placements, computed from the raw records):

| Job | Cycles over placements, median of 5 allocators (cralloc) | Instructions |
|---|---:|---:|
| Connect log service, 3 jobs | 5.0 to 6.4% (3.1 to 3.7%) | under 1% |
| Order service, 4 jobs | 1.7 to 3.1% (1.2 to 2.7%) | mostly under 1.5% |
| Sozu proxy | 2.8% (2.3%) | 2 to 3%: partly a mode of its work |
| Log store; mini-redis | 1.4%; 1.4% (2.8%; 1.0%) | under 1% |
| `rpc log c64`; `rpc echo c64` | 1.8%; 0.8% (5.8%; 1.1%) | under 1% |

Noise within one placement is a median half-range of 0.1 to 0.7%, so more placements help more than
more rounds. One build of an allocator (the median of 1 to 3 runs) against one of cralloc on the ten
server jobs differs from the answer over placements by 1.9 points (standard deviation), and at worst
by 6.8; a single run of each, by 2.4 points, at worst 14.2. The sign is wrong in 1% of pairs where
the gap is 1.5% or more, and in 29% where it is smaller, all on Sozu and mini-redis, whose gaps are
mostly unresolved anyway. The Router is not sensitive (0.3 to 0.9%); JSON spreads 3.5 to 6.7% and
the tree 3 to 9% ([program-effects.md](program-effects.md)). Two builds of `mpmc p4 c4` whose
sources differed only in a doc comment ran 4.2 and 4.9 ns in every session. Scratch builds that
switched off rules a job barely reaches moved one Vec by up to 9% and rpc log by up to 4.7%
(2026-10-18). Tails do it too: two builds of Meilisearch read +2.0 ms at p99.9 where one binary with
a switch reads 0.0.

**The rule.**

- A change that can be a run-time choice: one binary, an environment variable read once before
  `main` (a constructor in `bench-alloc` can do it), values of equal length. Placement cancels
  exactly; about 1% of run spread is left.
- A change of code, or a comparison of allocators: `benchrun layout --layouts pads`, a function of N
  bytes in `bench-alloc`, and compare means over the pads. Three rounds of five pads separated 2.5%
  and up, not under 1%.
- A job whose spread over pads is within its noise needs neither. A job that is one short loop
  measures that loop's address, so rewrite it: `vecgrow` now fills by `memset`.

### A tail at one fixed rate can rank the CPU's idle states

**What we concluded.** That cralloc had a real p99.9 deficit on `connect log h2`: 45 us against 39
to 41 for every incumbent, with the best p50 and p99. Earlier the same effect gave glibc, the
slowest allocator, the best p99.9 on actix (2026-10-20).

**How we found out.** Every context switch and idle counter on the server's CPUs, on one clock
beside every request. On a slow request the first server thread came on a CPU 22 to 28 us after it
was due, against 4 to 8 on an ordinary one, and that CPU had been idle long enough to enter C2 (18
us to leave) or C3 (declares 350). Three checks:

- One binary with a handler that spins: p99.9 47 to 48.5 us with no spin, 39.6 to 40.2 with the 1.5
  us that puts its cycles at jemalloc's.
- Other rates: cralloc at 68k to 80k requests a second has 36.7 to 39.3 us; jemalloc at 52k gets the
  tail itself, 53 to 56.
- C2 and C3 off on the server's CPUs: cralloc 45.1 to 37.8 us, the lowest at every placement; the
  others within 0.4 us; cycles and saturated rates within 1%.

**Why.** At one rate the faster server idles longer between requests, so its CPUs go deep more
often. The allocator does nothing during these requests. The load generator's CPUs busy-poll and
never enter an idle state, so none of this is the client.

**The rule.** Every server run records its deep entries per thousand requests, and the report marks
two tails "confounded by idle" when those rates differ by 1.2 times and by at least 1 a thousand
requests; the test errs towards "cannot tell". `scripts/deep-idle` shows the setting and, with root,
switches the deep states off on the server's CPUs. The standings run that way.
[servers.md](servers.md) has what sets the tails that remain.

The default `SCHED_IDLE` spinner on each server CPU's SMT sibling removes C3's millisecond tails,
but shares the core: it raises jemalloc's cycles a request on the order service by 17%, alike for
every allocator, so absolute cycles here are not a quiet core's. (cralloc's rise was once a third,
and the rest was the magazine-end lock, since fixed: [servers.md](servers.md).) A spinner on the
server's own CPU is no substitute: 40 to 50% of requests went over 42 us. And p99.9 is noisy:
medians to p99 repeat within 1 to 5%, p99.9 within 10 to 50% in the millisecond regime.

### A short job after idle runs at the wrong clock

**What we reported.** That the large-object pipeline and tokio spawn ran in two modes. Moves between
sessions went out as slowdowns or "not attributed", such as 14% on the split pipeline, part of which
was a cold median. Runs spread by up to 30% in time, and every ranking by time carried it.

**How we found out.** Cycles were flat across the spread. The pipeline took 62 ns an allocation in
seven rounds and 79 to 81 in three, at 700 to 711 cycles in all ten. Run after a pause of 0.2 s it
ran at 4.39 GHz for all of its 80 ms, twelve times of twelve; after half a second of spinning on any
core, at 5.7, nine of nine. From idle a core runs at the nominal clock for 200 to 260 ms, then
boosts to 5.56 to 5.71. One busy core anywhere keeps the boost, so the quiet gate's second of idle
made the next runs cold, and a busy machine hid the effect ([hardware.md](hardware.md)).

**Cycles do not see the clock only if the job does not wait on memory.** Memory takes the same
nanoseconds at any clock, so it takes more cycles at a higher one. `vecgrow 64x4Mi` on mimalloc took
3.05 cycles an element at 4.38 GHz and 3.94 at 5.71. A tight loop on L1 is immune; a memory-bound
job ranked by cycles is not.

**The rule.** Before each run the runner spins on the job's CPUs until each has held the set's warm
clock, within 3%, for three 5 ms windows: 15 ms a run. Every run records its clock, and the report
counts runs over 5% below their allocator's usual clock on the job: 665 of 4,800 before, 4 after.

### glibc's spawn result moved with the length of the binary's path

**What we took it for.** glibc as the bar on tokio `spawn`: 99 ns a task at four threads, with
cralloc 31% behind. That figure was one draw from a range the path decided.

**How we found out.** Hard links to one glibc binary ran `spawn t8` at 192 to 227 ns a task, at
paths 8 to 40 bytes longer. `std::env::args` appears to allocate the path ahead of everything tokio
allocates, and glibc's 16-byte header then decides whether tokio's scheduler handle has its hot
counter and a neighbour on one line.

**What was left was real.** tokio 1.54 padded that line, and glibc then ran 180.9 to 183.9 ns at any
path length. It still beat cralloc, 98.7 and 186.3 ns at four and eight threads against 111.6 and
203.0, as jemalloc did. The cause was where cralloc's 40-byte class put tokio's run-queue headers,
and the sibling stride closed it: 96 and 186 ns ([program-effects.md](program-effects.md)).

**The rule.** Binaries at paths of equal length, environments of equal length. The runner's control
cannot catch this: it does not test effects that differ by allocator entry. On the order service the
slowest request of a burst's first 5 ms was 20 to 115 us longer in every run of one arm, the same
program as the other until shutdown, and most of that was an environment variable's length: with a
dummy of equal length the slowest requests overlap, though p99 stayed 10 to 25 us higher
(2026-10-21).

### Single runs on a shared machine drift by up to three times

**What we concluded.** That Qdrant had a 7% gap against cralloc: a cluster of slow runs that hit
every allocator in turn, because the job read a disk the machine shares. On `/dev/shm` it went away.
That tcmalloc took 17% fewer cycles on the drift study's swing job: one round. Three rounds gave 1
to 17%, and that job's consumers spin on an empty queue, so its cycles count waiting.

**How we found out.** Every context switch on the server's CPUs put Qdrant's searchers in page
faults on its payload store, the same in every arm. And memory-bound jobs drifted by up to three
times over half an hour with no load on their own CPU or its sibling (jemalloc on `churn live=1M`,
372 to 124 cycles an operation, 2026-10-17); builds on the same L3 were enough.

**The rule.** Timed work goes through `scripts/measure`, builds and tests through `scripts/heavy`.
The runner marks a run invalid over 0.1 CPUs of other work in a server segment, or 0.25 over any
other run. One fresh process a run, in a new random order each round. Do not compare sessions for
moves under about 2%.

### Multiplexed and grouped counters lie differently

- **Multiplexing.** A multiplexed counter is an estimate, and unscaled it looks fast, so the runner
  makes such a run invalid. It also hurts the program: a cycles `perf record` beside the runner's
  counters rotated them on every tick, and pushed 2.5 to 3.6% of jemalloc's requests over 42 us, a
  false tail. At most five counters a session (the NMI watchdog holds one), no groups.
- **Groups.** We blamed L1 misses on allocator metadata in sh8bench, from per-symbol counts with the
  events in one perf group. A group's events are all sampled at the leader's instruction pointer, so
  the L1-miss column was cycles-weighted. IBS put 2,120 of 2,148 load samples in `malloc` and `free`
  in L1, and cutting the metadata lines from seven to five changed nothing (2026-10-18).
- **Skid.** A profile put a 7% slowdown of the tree's drop on the `lea` after the slab word's load,
  and we took it for the store-to-load hop, which is free at every placement. The cost was a short
  tail in a new line missing the op cache on every call ([roofline.md](roofline.md)). A figure for
  an inlined path is good to about a fifth of itself; IBS op samples are exact.

### What else has gone wrong

- **A different allocator was measured and nothing said so.** With the environment stripped,
  `scripts/memlimit` fell back to an address-space limit, and under it cralloc halved its regions.
  `benchrun` now refuses to measure under a finite `RLIMIT_AS`.
- **A different binary was built.** Cargo unified features with the RPC servers built beside a
  workload: 4% more cycles and 35% more faults than the same source built alone, purely from the
  build. Build every arm the same way. And the link can place a load on a hot word's page offset;
  `build` and `run` refuse that ([roofline.md](roofline.md)).
- **Memory figures have edges.** A script's "after close" column took the last point of the memory
  curve, 0.3 to 6 s after the end mark, in shutdown: jemalloc read 1,466 MiB on the store where it
  held 1,542. Meilisearch's "peak under load" of 894 MiB was the first sample of its fill, left from
  indexing; through 70% load it reaches at most 710 to 739. Both found rereading the raw data,
  2026-11-06. Sozu's summed resident set once counted the 180 MB executable in each process, 53 to
  97 MiB, until shared pages were counted once.
- **An incumbent may not be what you think.** `snmalloc-sys` reads `debug` in the profile that
  builds it as asking for snmalloc's Debug configuration, and `cc` as asking for frame pointers. The
  suite's release profile has `debug = 1`, so snmalloc ran unoptimized with its assertions on, 158
  ns a pipeline allocation and 502 cycles a `churn live=1k` operation on the M1 Max against 13.8 and
  21.0 in Release, and the crates' jemalloc and mimalloc kept frame pointers and nothing else of a
  debug build (the mimalloc crate's debug mode needs a feature; the mimalloc built from third_party
  was never touched). All three now build as a default release profile does (2026-11-07 at
  `69dd363`), and every figure of them on the Ryzen is from before. A thread whose first jemalloc
  call is a free gets no thread cache: tikv's prefixed build ran 181% and 104% slower than Fedora's
  on jobs with free-only threads, and 1 to 4% faster elsewhere, hence the `all-alloc` and
  `free-only` forms. tcmalloc's shared library through TLS descriptors understated it by 14 to 26%.
  A program that compiles in its own allocator hides under preload: difftastic's spread was 2.7%
  until its jemalloc came out, then 18%.
- **Some jobs run in two modes, and their medians compare mode counts:** xmalloc-test, glibc on
  larson, DataFusion on two L3s, the order service over HTTP/2 (4 to 6% more instructions, not tied
  to a build), Qdrant's search routing. Look at the runs before reading a median.
- **Wall time misleads on parallel programs:** nine screened programs jump 20 to 70% run to run
  while their cycles stay within a few percent.
- **Huge pages tie results to the machine's state.** jaq ran 1.74 to 2.31 s, and 1.48 to 1.53 an
  hour later; gix-hours under mimalloc spent 27% of its cycles under page faults, in
  `isolate_freepages_block`. Every run records the `thp_*` and `compact_*` counters.
- **A rule that counts time follows core speed.** The hold counts time, so the large mix's peak at 2
  GiB is 2.03 times live on the measurement CPUs and 1.39 on shared cores.

## The method in brief

**The machine.** AMD Ryzen 9 9950X (Zen 5), 16 cores with SMT, 48 KiB L1d and 1 MiB L2 a core, two
L3s of 32 MiB (CPUs 0-7 with siblings 16-23, 8-15 with 24-31), 60 GiB. Linux 7.2.9-201.fc44, glibc
2.43, rustc 1.99.1. `amd-pstate-epp`, EPP `balance_performance`, boost on. Transparent huge pages
`madvise`, `INIT_ON_ALLOC` on, no rseq time-slice extension, NMI watchdog on.

**A run.** The allocators are what a Rust user gets ([`bench/alloc`](../../bench/alloc/src/lib.rs)):
mimalloc v3.5.3, `tikv-jemallocator` 0.6.1, google/tcmalloc `f3c6669a` (per-CPU unless a note says
otherwise), glibc through `System`. One fresh process, capped, threads pinned to physical cores with
siblings idle; hardware counters over the measured phase, all threads. The cap costs nothing: capped
over uncapped -0.85% (-1.30 to -0.39%, 224 pairs) against -1.18% between two uncapped sessions
(2026-10-17). A server is ranked by its own cycles a request under open load
([workloads.md](workloads.md), "A server's day").

**A fast-path number.** Hand-written assembly loops, so that the compiler cannot change them, on CPU
2 with its sibling idle: 40 million iterations four times, the first a warm-up, the fastest of the
rest. Three forms: a pair (allocate one, free it), a call pair (each out of line), a batch (32 then
32). A difference of a cycle or two here is a finding to explain, not noise: the cause is usually
instruction selection or layout, and we fix it by construction ([roofline.md](roofline.md)).

**Statistics.** Kernel jobs: the median of per-round ratios against the best incumbent, with a 95%
bootstrap interval; "level" when it includes 1. The best incumbent is the best of eight builds:
mimalloc's three forms (its crate bundles 3.3.2, not v3.5.3), both tcmallocs, jemalloc, glibc and
snmalloc. Servers: the mean over placements of the median at each; a difference is **resolved** when
the two ranges over placements do not overlap, or an exact permutation test of the placements'
medians gives p < 0.05, and otherwise it is inside the range. Across jobs: geometric mean, with the
geometric standard deviation over jobs (population form). Tails: the median over runs of each run's
percentile.

**The control.** `mimalloc-ctl` is mimalloc built a second time and run as another allocator. At
`c7f5c21` (2026-10-22, 56 jobs) the median absolute difference is 0.5% and the 90th percentile 2.1%.
Four intervals exclude no difference where chance predicts 2.8: JSON -1.1%, `rpc log c256` +1.0%,
`rpc log c64` +0.9%, BTreeMap -0.3%. The largest are jobs with modes, sh8bench 23%, none excluding
zero. So about 1% is resolved on single-threaded jobs and a few percent on cross-thread ones; on the
RPC log jobs 2% is not (-2.1 and -2.4% at `31b46f4`). It does not catch code placement, since both
copies place their code alike.

**Caveats that stay.** One CPU model and kernel: the rules should carry, the numbers will not. Why a
loop costs what it does is inferred, not documented. THP `madvise` decides much of the
build-and-drop jobs: mimalloc asks for huge pages for its whole heap, cralloc for small classes past
16 MiB and grown blocks. A server's heap is its anonymous memory, from `smaps_rollup` once a second,
so it can be a second stale. Peaks of a few MiB move by up to 1.5 MiB between runs, so differences
under 1 MiB mean nothing.

## On the M1 Max

Asahi Linux 7.2.12 with 16 KiB pages: two Icestorm cores, then two clusters of four Firestorm cores
at 3036 MHz, each sharing 12 MiB of L2, no L3 in sysfs, no SMT. `scripts/cpusets` gives jobs one
cluster and the other, and builds the rest of the machine with the second cluster, so a session
across clusters always runs with heavy work paused. It is a desktop with a browser open. Figures are
one session of `quick`, 405 runs, 2026-11-07 at `dff7993`.

- **The clock has no ramp to wait for.** The governor is `performance`, with boost off. All but 34 of
  399 warm-ups held the clock in their first three windows (15.5 ms; the longest took 208), and every
  run's measured clock lay within 2.98 to 3.04 GHz. The warm-up stays because it costs only that, and it would catch a clock
  that did move.
- **Which PMU.** One PMU a kind of core, each with a type of its own, and a generic hardware event
  opened for no CPU goes to Firestorm's. On an Icestorm core such an event counts nothing: its
  running time stops while its enabled time goes on, as it does for a task that leaves its PMU's
  CPUs, so the run reads as multiplexed and is invalid.
- **Ten counters, not interchangeable.** Nine can count cycles and two can count instructions: a third
  `instructions` event ran 67% of the time. A workload's three and the runner's two fit. There is
  no NMI watchdog to hold one.
- **Nothing holds a core out of deep idle without root.** There is no SMT sibling for a spinner, so
  `CPU PD` (10 us to leave, 10 ms residency) stays in a server's tails until `scripts/deep-idle off`
  disables it. Runs say `idle_states: free`. A tail the report flags cannot be read as the allocator's.
- **Placement moved jobs mostly through the routines.** Five pads moved `tree depth=20` 6.1% and `map
  btree 1M` 5.3%, at level instructions and 0.1 to 0.5% noise, when routines formed addresses inside
  their sequences (`ebe126a`). With every address formed before the arming store they moved 0.5 and
  2.0% (`62be25d`), and with the generated routines 0.2 and 0.8% (`66dfb90`), noise 1.3 and 1.5%,
  where mimalloc moves 0.1 and 0.4%. The cause is not traced; an `adrp` inside the sequence is the
  suspect. `rpc log c64` spreads 0.6% on tcmalloc, inside
  its noise, and 3.4% on jemalloc, 2.6% of it instructions. So fewer jobs measure where their code
  landed than on the Ryzen, but too few have been swept to drop the rule.
- **A kernel job's limit on other work stands; a server's reads high.** A kernel run's other work has a
  median of -0.01 CPUs: of 402 valid runs the highest was 0.15, and three went over the limit of 0.25.
  Server segments under load read -0.19 to 0.25, by job and segment and alike across allocators
  (`orders axum` at 30% has a median of 0.13), where the threads named on those CPUs add up to under
  0.08 at the same moments. So the clocks disagree by more than the Ryzen's limit of 0.1, which threw
  out a third of quiet runs, and the limit here is 0.3. Why they disagree is not traced. Three seconds of
  a busy CPU, 0.55 and up on the Ryzen, should still show.

## Reusable techniques

The placement sweep and the run-time switch are under the first trap. Beside them:

- **The IPC split of a gap:** cycles against instructions a request over one window. Share in
  instructions = log(instruction ratio) / log(cycle ratio). jemalloc on the Connect log service:
  +9.6% cycles, +9.8% instructions, so work. glibc on the order service: +25.3%, +8.3%, so mostly
  stalls. It shows what is visible and cannot exclude a stall hiding behind fast added code; stall
  counters would ([program-effects.md](program-effects.md)).
- **Attributing misses to heap blocks:** a diagnostic build records every allocation's layout,
  address, CPU and time; IBS op sampling (`perf c2c record -- -a -C cpus -c 4000`) gives each load's
  data address and latency; each load that missed L1 goes to the block holding its address. That
  named tokio's headers and scheduler handle. Load samples miss stores and locked operations that
  wait for a line: 54 samples on headers that cost about 280 cycles a task. To test a placement, a
  wrapper serves a size range as a larger size, or starts one size some bytes into a larger block.
- **Attributing cycles to inlined allocator code:** line tables for the allocator only, each sampled
  address's chain of inlined functions (`llvm-symbolizer --inlining`), checked against the routines'
  ranges in the binary's rseq descriptors. For a `memcpy` sample, AMD's branch record names the last
  call out of the binary, in 43% of samples.
- **A counting build, never timed:** counters at slow-path decisions, or requested bytes for a
  live-bytes denominator, read from the running server through `/proc/PID/mem` at each mark.
- **Exact instruction counts:** callgrind with a phase routed through one out-of-line function
  (`--toggle-collect`). Stock valgrind caps the client at 64 GiB, so cralloc halves its regions, and
  glibc's rseq registration fails; a valgrind with `aspacem_maxAddr` raised and a preloaded
  constructor that writes 0 into `cpu_id` fix both. Do not sample on instructions: 25% of one
  profile piled onto the instruction after a locked one.
- **Lining up requests with the CPUs:** `perf record -e dummy --switch-events -k mono` takes no
  hardware counter and costs nothing measurable; idle counters from sysfs every 5 ms; all on
  `CLOCK_MONOTONIC`.

Zen 5 raw events used here (`perf list --details` has the rest):

| Event | Code |
|---|---|
| Demand L1 fills: all; from L2; L3 or a core of this CCX; the other CCX | `0xff43`, `0x143`, `0x243`, `0x443` |
| Store or state change that hits in L2; request that misses in L2 | `0x1064`; `0x864` |
| Any fills, with prefetch; hardware prefetch fills | `0xff44`; `0xdf5a` |
| No retire, waiting on load data; waiting to execute | `0xa2d6`; `0x2d6` |
| L1 dTLB misses; L2 misses starting a walk for 4 KiB; for 2 MiB | `0xff45`, `0x1045`, `0x4045` |
| Loads blocked by an older store | `0x0224` |
| Dispatch slots: left empty by the front end; stalled by the back end | `0x1a0` umask 1; umask `0x1e` |
| Op-cache misses; ops from the decoder; instruction-cache misses | `0x28f` umask 4; `0xaa` umask 1; `0x18e` umask `0x18` |

## Open questions

- **Which of this is Zen 5's:** the front-end sensitivity, the clock's ramp, the idle exits.
- **What makes the order service over HTTP/2 change mode**, and what code moves it with placement.
  Only the gRPC decoder has been traced.
- **Why a by-hand `perf stat -p` on the server** counted the runner's instructions but 28% fewer
  cycles.
- **Older "after close" figures** taken from a run's last curve point may read shutdown. Only the
  standings are corrected.
