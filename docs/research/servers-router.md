# The Apollo Router: a real GraphQL server

Measured 2026-10-24 on branch `real-servers` (allocator code as at main `46126f0`), with the
servers suite (`bench/apps/README.md`, "Servers"; `bench/apps/router/README.md`). Apollo
Router 2.19.0 runs on CPUs 0-3 with its cores held out of deep idle. Its four subgraphs
(Apollo's federation demo, 10,000 products) run in one glibc process on CPUs 12-14 and
answer from memory. The load generator runs on CPUs 8-11, with 64 connections sending
GraphQL queries from a fixed list of 20,000: product pages with inventory, reviews and
authors (three fetches deep), and two queries that start from the accounts subgraph.
Answers are 10 to 40 KB of JSON. Raw results:
`docs/research/data/servers-router-2026-10-24.jsonl.gz`.

## Summary

- **asmalloc does the least work a request, ahead of every incumbent and resolved over
  placement.** The figure is cycles a request at 70% of the slowest allocator's saturated
  rate. tcmalloc is 1.2 to 1.3% behind, mimalloc 2.9 to 3.0%, jemalloc 5.9 to 6.1%, and
  glibc 37.7%. That holds at both rates: the standings' rule (70% of glibc's rate), and 70%
  of the slowest of the other four. jemalloc is what the Router ships with.
- **The job is not placement-sensitive.** Each allocator's range over five code placements
  is 0.3 to 0.9%, against gaps of 1.2% and more. Every difference is resolved, with ranges
  apart (exact permutation test, p = 0.008).
- **The allocator matters here.** jemalloc is about 8.4% of the server's cycles, plus 2.7% in
  Rust's allocation shims. A request costs 2.6 to 2.7M cycles, mostly building, merging and
  serialising JSON trees (`serde_json_bytes` maps, `apply_selection_set`, `format_value`).
  Some blocks are freed on another worker thread: 1.25% of cycles go to jemalloc's
  `tcache_bin_flush_small`.
- **Tails:** at the standings' rate every p99.9 is confounded by idle states. At the faster
  rate they are not confounded: asmalloc 1,209 us, tcmalloc 1,202, mimalloc 1,472, jemalloc
  1,744. The ranges over runs are wide (asmalloc 1,045 to 5,545), so only the order
  "asmalloc and tcmalloc ahead of mimalloc and jemalloc" is worth reading.
- **Memory is a small heap and does not rank anything.** It peaks around 100 MiB.
  jemalloc ends at 78 MiB, asmalloc at 96 to 101, tcmalloc at 101, mimalloc at 143.

## Method

- Saturated rates: `--suite servers-sat`, two rounds of single builds. glibc 4,790 req/s,
  jemalloc 8,444 to 8,545, mimalloc 8,543 to 8,609, asmalloc 8,972 to 9,048, tcmalloc
  8,974 to 9,024. glibc's CPUs sleep at saturation (1,273 deep-idle entries a thousand
  requests, against under 1 for the others): it waits on its arena locks.
- Two jobs. `router graphql` runs at 1,440 and 3,350 req/s (30% and 70% of glibc's rate);
  for the other four allocators that is about 37% of capacity, where tails rank idle states.
  `router graphql fast` runs at 2,530 and 5,910 req/s (30% and 70% of jemalloc's rate),
  without glibc.
- Placement sweep as in `standings.md`: every allocator built at bench-alloc's five layout
  pads (`bench/probes/standings/pad-build.sh router`). Two runs per pad for the four
  incumbents and asmalloc, and one per pad for glibc, in eight interleaved sessions of one
  round each (`queue.sh`), compared with `servers.py`. 85 runs, none invalid.
- Router builds use Apollo's release profile, which is cargo's default (no LTO), so an extra
  allocator or pad is a link of seconds.
- Profiles: `bench/probes/server-profile/record.py --job "router graphql"`, 5 s of the 70%
  segment, single builds of jemalloc and asmalloc. The jemalloc share is its symbols' share
  of the server's cycles. asmalloc's inlined fast path is not separable in this build
  (no line tables), so only its out-of-line code (3.7%) is counted.

## Results

Server cycles a request at 70% (k), the mean over pads (range); the others relative to
asmalloc's mean, all resolved:

| Job | asmalloc | jemalloc | tcmalloc per-CPU | mimalloc | glibc |
|---|---|---|---|---|---|
| router graphql (3,350 req/s) | 2,716 (2,708 to 2,727) | +6.1% | +1.3% | +3.0% | +37.7% |
| router graphql fast (5,910 req/s) | 2,610 (2,606 to 2,614) | +5.9% | +1.2% | +2.9% | - |

At 70%: p50 / p99 / p99.9 in us (medians over 10 runs, glibc 5), deep-idle entries a
thousand requests, and resident MiB at peak and at the end of the run:

| Job | Allocator | p50 / p99 / p99.9 | deep/k | Peak / end MiB |
|---|---|---|---|---|
| router graphql | asmalloc | 556 / 1,002 / 1,108 | 2,362 | 101 / 101 |
| | jemalloc | 582 / 1,001 / 1,144 | 2,085 | 135 / 78 |
| | tcmalloc | 562 / 1,021 / 1,127 | 2,376 | 101 / 101 |
| | mimalloc | 563 / 986 / 1,115 | 2,227 | 147 / 143 |
| | glibc | 823 / 1,630 / 1,920 (confounded) | 4,451 | 90 / 87 |
| router graphql fast | asmalloc | 553 / 963 / 1,209 | 195 | 101 / 96 |
| | jemalloc | 596 / 1,049 / 1,744 | 178 | 135 / 81 |
| | tcmalloc | 563 / 990 / 1,202 | 210 | 101 / 101 |
| | mimalloc | 573 / 1,002 / 1,472 | 203 | 147 / 145 |

At 3,350 req/s the deep-idle rates of asmalloc, jemalloc, tcmalloc and mimalloc are within
the confound test's factor of each other, and glibc's is not, so its p99.9 is flagged.
At 5,910 req/s nothing is flagged.

## What was surprising

- **glibc at half the others' rate.** It also has the widest gap of any server in the
  standings (37.7%). Its CPUs sleep at saturation, which suggests lock waits in its arenas,
  likely from cross-thread frees (the Router builds JSON on one worker and drops it on
  another). glibc was not profiled, so this is not confirmed.
- **tcmalloc is the nearest incumbent, not jemalloc,** as on the order service. The Router
  ships with jemalloc and gives up 6% of its CPU to it on this load.

## Unresolved

- How asmalloc's 1.2% lead over tcmalloc splits between the fast path and `realloc`. Line
  tables for the Router build would be needed, as `server-profile.md` did.
- The Router's query-plan cache was not exercised with many distinct operations (three
  operations here). Production graphs with many operations would hold a larger plan cache.
  No round was run that way, since the lead judged it not worth contriving.
- The driver turns the Router's 10 s idle connection timeout up to 10 minutes. The
  generator leaves some of its 64 connections idle that long at low rates and counts a
  reconnect as a broken run. It does not touch the request path.
