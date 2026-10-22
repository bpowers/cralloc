# The tokio spawn gap

2026-10-21, branch `spawn-gap` on main `f9ee162`. Why asmalloc trails on the kernel suite's
`spawn t4 local` and `spawn t8 both` (29% and 41% behind glibc in `standings.md`), and whether
real servers show the same thing.

## Summary

- **The whole gap is where two of tokio's runtime structures land, not task memory.** Both are
  allocated once, by the thread that builds the runtime, and then written by every worker.
  1. **The scheduler handle** (tokio 1.53.3, 416 bytes): the owned-task list's counter, written
     by every spawn and every completion, shares a line with the idle state that every spawn
     reads and parked workers write. An allocator that starts the block on a line boundary puts
     them together. asmalloc, jemalloc, mimalloc and tcmalloc all do. glibc's 16-byte chunk
     header shifts the block by an amount the program's earlier allocations decide, which
     splits the line in some runs and not in others: glibc's spawn t8 runs at 192 to 227 ns a
     task depending on the length of the binary's path alone.
  2. **The workers' run-queue headers** (`queue::Inner`, 40 bytes, four or eight allocated in a
     loop): asmalloc and mimalloc have a 40-byte class, so the headers are packed 40 bytes apart
     and each worker's `head` and `tail` share lines with its neighbours'. jemalloc and tcmalloc
     have no 40-byte class; jemalloc serves them from its 48-byte class, interleaved with the
     48-byte park structures allocated in the same loop, which keeps them apart by luck.
- **Moving just those two blocks closes the gap.** A diagnostic wrapper that serves 33 to 40
  bytes from the 48-byte class and starts 416-byte blocks 16 bytes in takes asmalloc from
  130.6 to 98.6 ns a task at t4 (glibc 100.2, jemalloc 113.0) and from 249.8 to 189.6 at t8
  (glibc 192 to 227 by path, jemalloc 222.9), with the fewest instructions of any allocator.
  Each fix alone is worth about 15 ns at t4.
- **The hypotheses about magazines are refuted.** Task cells (256 bytes, align 128) are served
  from the 256-byte class, 256-aligned, one per four lines, as in jemalloc. They draw about the
  same share of L1 misses under asmalloc and jemalloc. Magazines of a 4 KiB budget (14 task
  cells instead of 254) change neither the misses nor the time.
- **tokio 1.54.0 (2026-10-19) already fixes the first:** it cache-pads the idle state and the
  injection queue (tokio #8622). The run-queue header is still unpadded. Built against 1.54,
  asmalloc is 15% (t4) and 14% (t8) behind jemalloc, and serving 33 to 40 bytes as 48 closes
  that.
- **Servers:** level with jemalloc and well ahead of glibc on both order-service jobs. The two
  structures are about 1% of L1 misses on HTTP/1.1 and 4% on HTTP/2, where hyper spawns a task
  per stream. Serving 40 bytes as 48 removed 9 of 79 fills from L3 per request on HTTP/2 and
  moved cycles by 2%, within the runs' spread.
- **Remedy: none in the allocator.** See the end.

## Method

The workload binaries built as `benchrun build` builds them, run by hand under
`scripts/measure` (`--both-l3` at t8), every run in a `scripts/memlimit` scope, builds
interleaved in a new random order each round, three or four rounds, medians. The binaries are
hard links at paths of equal length, because path length moves glibc (below). Counters come
from the workload's own measured-phase counters with three raw events
(`BENCH_EVENTS`): demand L1 fills (`ls_dmnd_fills_from_sys.all`, 0xff43), of which from L2
(0x143), from L3 or another core of the CCX (0x243), from the other CCX (0x443), and L2
requests for a line in exclusive state (`l2_cache_req_stat.ls_rd_blk_x`, 0x1064). Per task
below; a run has 5.12M tasks at t4 and 10.24M at t8.

Attribution: IBS op sampling (`perf c2c record -- -a -C cpus -c 4000`) of builds with a
diagnostic `trace` feature in `bench-alloc` that records every allocation's layout, address,
CPU and time stamp; each sampled load that missed L1 is put to the block containing its
address. Experiments: a diagnostic `shape` feature that wraps asmalloc and, from the
environment, serves a size range as a larger size (`SHAPE_BUMP=33-40:48`) or starts blocks of
one size some bytes into a larger block (`SHAPE_SHIFT=416:16`). The wrapper is out of line and
costs about 260 instructions a task, so its runs are compared with its own plain run (`S`).
Neither feature is merged; they, the runner patch below and the scripts used are in
`bench/target/spawn-gap-diagnostics.patch` of this worktree, with the scripts in `bench/target/spawn-gap-scripts/` (not committed).

Servers: `benchrun run --suite servers` for `orders axum` and `orders axum h2` (tokio 1.54.0,
the suite's day, s70), with a runner patched to count the same raw events over each segment.

## Spawn, t4 (CPUs 0-3, tokio 1.53.3)

| Build | ns/task | cycles | instructions | L1 fills | from L3/core | exclusive requests |
|---|---:|---:|---:|---:|---:|---:|
| asmalloc | 130.6 | 2,913 | 1,671 | 59.4 | 42.6 | 15.5 |
| jemalloc | 113.0 | 2,524 | 2,012 | 40.7 | 30.9 | 8.2 |
| glibc | 100.2 | 2,226 | 2,059 | 33.0 | 25.8 | 4.8 |
| asmalloc in the wrapper (S) | 127.5 | 2,827 | 1,932 | 54.9 | 40.7 | 12.9 |
| S, 33-40 bytes as 48 | 114.2 | 2,538 | 1,958 | 43.7 | 32.7 | 9.8 |
| S, 416-byte blocks 16 in | 111.9 | 2,488 | 1,940 | 41.8 | 32.6 | 8.0 |
| S, 416-byte blocks 8 in | 112.2 | 2,491 | 1,949 | 41.7 | 32.5 | 8.0 |
| S, 416-byte blocks 64 in (control) | 127.0 | 2,838 | 1,939 | 54.7 | 40.6 | 12.9 |
| S, both | 98.6 | 2,196 | 1,978 | 32.7 | 25.9 | 5.6 |

A shift of a whole line changes nothing; any shift within a line moves the handle's counter
away from the idle state at offset 0xf8. Rows from two sessions; S and jemalloc agreed across
them within 1%.

Where the L1-missing loads land (IBS samples in one run of each traced build, t4):

| Block | asmalloc | jemalloc | glibc |
|---|---:|---:|---:|
| scheduler handle, 416 B | 2,387 (line at +0xc0: 1,992) | 2,031 (1,578) | 858 |
| task cells, 256 B align 128 | 238 | 225 | 160 |
| owned-task shard locks, 384 B | 139 | 140 | 13 |
| run-queue headers, 40 B | 54 | 8 | 15 |
| all L1-missing user loads | 2,975 | 2,615 | 1,392 |

Most misses on the handle are HITM: the line is modified in another core. The run-queue
headers show few load misses but cost about 280 cycles a task, in stores and locked
operations that wait for the line (exclusive requests 12.9 to 9.8).

Magazines (t4, 4 KiB magazine budget, a diagnostic build): S 127.7 ns, small magazines 128.5;
with both placements fixed 97.5 and 99.2. Fills per task within 1% of each other, 200 more
instructions a task for the extra exchanges.

## Spawn, t8 (CPUs 0-3 and 8-11)

| Build | ns/task | cycles | L1 fills | from L3/core | from the other CCX |
|---|---:|---:|---:|---:|---:|
| asmalloc | 249.8 | 11,028 | 88.2 | 35.5 | 18.7 |
| jemalloc | 222.9 | 9,824 | 59.4 | 26.3 | 13.2 |
| glibc, this path | 225.2 | 10,001 | 62.3 | 26.8 | 14.6 |
| S | 243.7 | 10,769 | 81.7 | 36.0 | 16.6 |
| S, 33-40 as 48 | 227.5 | 10,085 | 66.1 | 28.0 | 13.9 |
| S, 416 16 in | 202.6 | 8,982 | 59.8 | 27.4 | 13.9 |
| S, both | 189.6 | 8,430 | 48.0 | 21.5 | 12.0 |

glibc by the length of the binary's path (same binary, hard links; 2 rounds): 227.0, 226.4,
194.1, 198.6, 191.7 and 192.7 ns for paths 8 to 40 bytes longer. The standings session's
glibc (179 at t8, 100 at t4) was a good draw; `baselines.md` section 15 saw the bad one.

## tokio 1.54

The same workloads built against tokio 1.54.0 (the bench's lock file moved for these builds
only), which pads the idle state. Medians of four rounds (t4) and three (t8):

| Build | t4 ns/task | t4 from L3/core | t8 ns/task | t8 from L3/core |
|---|---:|---:|---:|---:|
| asmalloc | 113.3 | 33.7 | 208.3 | 25.6 |
| jemalloc | 98.2 | 23.8 | 182.5 | 19.3 |
| glibc, two paths | 101.0, 106.6 | 25.2, 28.5 | 186.2, 190.6 | 19.8, 23.5 |
| S | 112.6 | 31.9 | 208.0 | 25.0 |
| S, 33-40 bytes as 48 | 98.9 | 25.3 | 189.6 | 21.2 |

Every allocator gains, and glibc's draw matters less. What is left between asmalloc and
jemalloc, 15% and 14%, is the run-queue headers: serving them as 48 bytes brings asmalloc to
jemalloc at t4 and within 4% at t8, inside a wrapper that costs about 260 instructions a task.
mimalloc, which also has a 40-byte class, was 6% behind jemalloc at t4 in the standings.

## Servers

`orders axum` (HTTP/1.1) and `orders axum h2`, s70, server cycles and fills per request:

| Job, build | rounds | cycles/req | instructions/req | L1 fills | beyond L2 |
|---|---:|---:|---:|---:|---:|
| axum, asmalloc | 1 | 67.7k | 105.0k | 612 | 94 |
| axum, jemalloc | 1 | 67.3k | 107.1k | 626 | 90 |
| axum, glibc | 1 | 85.0k | 112.6k | 810 | 231 |
| axum, S | 1 | 70.3k | 117.5k | 627 | 93 |
| axum, S 33-40 as 48 | 1 | 70.8k | 119.8k | 627 | 92 |
| h2, asmalloc | 2 | 61.1k | 107.7k | 912 | 88 |
| h2, jemalloc | 2 | 61.5k | 109.6k | 932 | 92 |
| h2, glibc | 1 | 85.9k | 123.8k | 1,244 | 250 |
| h2, S | 3 | 64.2k (62.5 to 65.2) | 120.4k | 908 | 89 |
| h2, S 33-40 as 48 | 3 | 62.9k (62.9 to 64.0) | 121.4k | 893 | 84 |

IBS on the asmalloc server at s70: on HTTP/1.1 the misses are thread stacks, system calls,
the store's eviction of old orders and hyper's connection state; the run-queue headers are 40
of 4,950 L1-missing samples and the handle does not show. On HTTP/2 tokio's handle (592 bytes
in 1.54) and the headers are 254 of about 5,900. So the effect is there, at a few percent of
the misses, and below what these rounds resolve in cycles.

## Remedy

Nothing in the allocator. Both placements are properties of tokio's structures, and the
allocator cannot tell, when a block is made, that different threads will write its parts.

- The handle's line is shared by every allocator that aligns blocks, and tokio 1.54 pads it.
  Moving blocks off line boundaries, as glibc's header does, is a lottery that helps one
  layout and hurts another.
- The run-queue header is the general hazard of size-segregated allocation: instances of one
  type made in a loop by one thread sit side by side, and if each is then written by its own
  thread they share lines unless the type is padded. jemalloc escapes it here only because two
  of tokio's types share its 48-byte class. Dropping asmalloc's 40-byte class (16-byte spacing
  above 32 bytes) would buy this one case and cost 20% on every 33 to 40-byte object
  everywhere, and move the hazard to other sizes; `baselines.md` section 15 rejected it for
  the same reason. A heuristic that spreads the first objects of each class one to a line
  would catch structures built at start-up, but would put later churn beside them; not
  measured, not recommended.

What would change the number: tokio padding `queue::Inner` (as #8622 did for the idle state),
which helps every allocator, and the suite moving to tokio 1.54. With both, the spawn jobs
would measure the allocator; until then they rank where these two blocks land.

## The bench on tokio 1.54 (2026-10-22)

Branch `spawn-gap-2` on main `b21e998`. The bench's lock file now takes tokio 1.54.0 for the
workloads and connect-rust's server; the apps keep their own lock files (the order service was
already on 1.54.0). Same method as above; server jobs by `benchrun`, the builds interleaved.

**The bump costs asmalloc 3.6% on a gRPC server, and the run-queue headers are why.**
`connect log grpc h2`, server cycles per request at s70, k (runs from four sessions; "padded"
is tokio 1.54.0 with `queue::Inner` in tokio's own `CachePadded`, the diff in
`bench/probes/spawn-gap/tokio-pad-queue.diff`):

| Build | tokio 1.53.3 | tokio 1.54.0 | 1.54.0 padded |
|---|---|---|---|
| asmalloc | 129.1, 128.5, 128.0, 126.7, 128.0, 128.5, 129.6, 128.5, 126.6 | 134.2, 132.2, 132.8, 133.0, 133.0, 134.3, 133.0, 134.0, 131.4 | 130.2, 129.3, 129.2, 128.6, 128.6 |
| jemalloc | 136.6, 134.9, 133.4, 134.6 | 136.7, 137.9, 136.3, 135.8, 136.3, 137.6 | 141.2, 140.5 |

- Medians: asmalloc 128.5k on 1.53.3, 133.0k on 1.54.0, 129.2k padded. Padding the header
  takes back about 85% of the loss; asmalloc then leads jemalloc's unpadded 1.54 by 5%.
- Instructions per request are level across the versions (asmalloc 298.5k, 298.7k, 298.9k;
  jemalloc 310.4k, 310.8k), so the loss is stalls, not added work in tokio. jemalloc moved
  1.3% and padding costs it 3%.
- The headers are placed identically in both versions (the same four 40-byte slots, from the
  allocation trace). Under IBS the hand-run servers show no more L1-missing user loads on 1.54,
  and the counted user-mode L1 fills and exclusive requests per request do not separate the
  builds either: the cost is in the stores and locked operations on the headers that IBS load
  sampling and those counters do not isolate. Why 1.54 exposes what 1.53.3 did not is not
  established; the likeliest reading, unverified, is that 1.53.3's contended idle line throttled
  the workers' searching and stealing, and with it padded they reach each other's queue
  headers more often.

**Spawn on 1.54**, all the standings' allocators, three interleaved rounds, ns per task
(medians):

| Allocator | t4 | t8 |
|---|---:|---:|
| asmalloc | 111.6 | 203.0 |
| jemalloc | 94.5 | 176.7 |
| tcmalloc per-thread | 95.9 | 177.2 |
| tcmalloc per-CPU | 97.0 | 187.5 |
| mimalloc / crate / sized | 97.0 / 97.4 / 97.7 | 188.5 / 187.4 / 188.3 |
| glibc | 98.7 | 186.3 |
| snmalloc | 139.9 | 191.0 |

asmalloc is 18% and 15% behind the best. glibc no longer depends on its draw: across eight path
lengths it ran 99.2 to 102.2 ns at t4 and 180.9 to 183.9 at t8 (two rounds each); on 1.53.3 the
same sweep gave 192 to 227 at t8. The 1.53.3 standings' glibc figures (100 and 179) were one draw
of a range that wide; any glibc figure for these jobs on 1.53.3 should be read as that range.

**With the run-queue header padded** (spawn, three interleaved rounds, ns per task):

| Allocator | t4 1.54 | t4 padded | t8 1.54 | t8 padded |
|---|---:|---:|---:|---:|
| asmalloc | 111.8 | 95.7 | 201.3 | 188.4 |
| jemalloc | 94.6 | 94.3 | 178.6 | 175.9 |
| mimalloc | 97.3 | 99.0 | 188.9 | 194.5 |
| glibc | 99.0 | 107.2 | 185.8 | 191.3 |

The padding helps asmalloc only; it costs glibc 3 to 8%, mimalloc 2 to 3%, and jemalloc 3% on
the gRPC server. A patch to tokio would need to show more than this, and it is no fix for us
to rely on. mimalloc, which also has a 40-byte class, does not lose on the unpadded header;
its placement was not traced.

**What a general rule would have to be.** The cost comes from four 40-byte objects made one
after another by one thread and then written by four. A rule that kept them apart would have to
put objects of a class made back to back on different lines, which spends a line per object on
the classes below 64 bytes, or drop the classes that do not divide 64 (40 and 56), which costs
up to 20% on objects of those sizes and leaves the 48-byte class, where two consecutive objects
still share a line. Neither is a rule I would build on this evidence; the narrower facts are
that the loss is 3.6% on one gRPC job and 15 to 18% on the spawn microbenchmark, and that
tokio padding its header would remove it.
