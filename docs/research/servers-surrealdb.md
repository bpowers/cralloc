# SurrealDB: an in-memory database as a server job

Measured 2026-10-25 on branch `real-servers` (allocator code as at main `46126f0`), with the
`servers-thread` plan of the large-heap suite (`bench/apps/surrealdb/README.md`). SurrealDB
3.3.4's in-memory engine runs on CPUs 0-3 with its cores held out of deep idle, holding
750,000 records (and 1,500,000 in a second job) with two indexes, filled through its HTTP
API as it starts. The load generator runs on CPUs 8-11 with 64 connections sending
SurrealQL: half reads by id, one in five index reads of 20 records, one in ten unique-index
reads, and one in five updates. Raw results:
`docs/research/data/servers-surrealdb-2026-10-25.tar.gz`.

## Summary

- **asmalloc does the least work a request and has the best tails.** Cycles a request at
  70% (6,110 req/s): asmalloc 2.02M; tcmalloc +7.0%, mimalloc +12.6%, jemalloc +13.6%
  (what SurrealDB ships with), glibc +27.2%. asmalloc against tcmalloc is resolved over three
  code placements each (ranges apart); the other arms are single builds. p99.9: asmalloc
  2.7 ms against 3.3 to 3.6 for the incumbents.
- **asmalloc holds the least memory**, 4 to 5% under jemalloc and tcmalloc at both sizes.
  At 750k the heap is 1,716 MiB against 1,792 to 1,924; at 1.5M it is 3,323 against 3,498
  to 3,774. The heap is the data: it stays resident after the load for every allocator.
- **The allocator matters here.** jemalloc's code is 11.5% of the server's cycles at 70%,
  plus 3.2% in the Rust allocation shims (profile of a triage build). The rest is the
  store's skip list, `memcmp` and value decoding.
- **The huge-tail rule does nothing here**, neither good nor harm: the heap of small objects
  only grows, so no class's region has a tail to return.

## Method

- Builds: SurrealDB's release profile (fat LTO, one codegen unit, `panic = "abort"`,
  stripped). Each allocator's build is a long single-core link here (17 to 126 minutes
  wall on a busy machine). asmalloc and tcmalloc were built at bench-alloc's pads 0, 32 and
  64 (`bench/probes/standings/pad-build.sh surrealdb`, `PADS="00 32 64"`). The pair ended
  more than 3% apart, so the sweep stopped at three pads, and jemalloc, mimalloc and glibc
  are single builds.
- Saturation, one round of single builds (req/s): glibc 8,728, jemalloc 9,672, mimalloc
  10,001, tcmalloc 10,460, asmalloc 11,186. The 70% rate is 6,110.
- `servers-thread`, six sessions: asmalloc and tcmalloc two runs at each pad, the others two
  runs each. The 1.5M job: one run per allocator. All runs valid.
- The huge-tail prototype (main 81437b0 plus a switch, linked into the asmalloc arm): `t0`,
  `t1`, `t1p30` and `t1p120` once each at 750k, and `t0` and `t1` at 1.5M.

## Results

At 70%, medians; memory is the anonymous heap in MiB (mean and peak under load, a minute
after close):

| Allocator | Mcycles/req | p50 / p99 / p99.9 us | 750k heap | 1.5M heap |
|---|---|---|---|---|
| asmalloc | 2.02 (2.01 to 2.04 over pads) | 69 / 1,949 / 2,682 | 1,716 / 1,723 / 1,725 | 3,323 / 3,333 / 3,333 |
| tcmalloc per-CPU | 2.16 (2.15 to 2.17) | 74 / 2,302 / 3,337 | 1,796 / 1,803 / 1,803 | 3,498 / 3,505 / 3,505 |
| mimalloc | 2.28 | 81 / 2,525 / 3,432 | 1,876 / 1,888 / 1,886 | 3,596 / 3,605 / 3,595 |
| jemalloc | 2.30 | 81 / 2,538 / 3,552 | 1,792 / 1,800 / 1,797 | 3,500 / 3,509 / 3,485 |
| glibc | 2.57 | 96 / 4,085 / 12,164 | 1,924 / 1,931 / 1,928 | 3,774 / 3,782 / 3,780 |

The runner flags glibc's p99.9 as confounded by idle (its CPUs' deep-idle rate differs
from asmalloc's by more than the test allows). The other four are not flagged.

The huge-tail prototype: every setting gave 1,706 / 1,721 / 1,721 MiB at 750k with 1,350 MiB
in huge pages, and `t0` and `t1` gave 3,323 / 3,333 / 3,333 at 1.5M.

## What was surprising

- The gap is wider than on any program server in the standings: 7% to tcmalloc and 14% to
  the allocator SurrealDB ships with. The work is many small allocations (decoded values,
  strings, query plans) made and freed per request, over a heap of gigabytes.
- jemalloc's profile (the triage build without LTO) shows `arena_cache_bin_fill_small`
  (2.7%) and `edata_heap_remove_first` (1.0%) among the top entries: its refills of small
  bins cost it here.

## Unresolved

- jemalloc, mimalloc and glibc are single builds of a fat-LTO program; placement could move
  any of them by a few percent, against gaps of 12 to 27%.
- Authentication is off; with it every request verifies the root password, which would
  dominate the server's work.
