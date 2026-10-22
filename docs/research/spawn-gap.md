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
- **Later (2026-10-22): order, not spacing.** A fill that opens a run hands its first 64
  objects out a stride apart; it takes back the spawn gap and leaves the rest level. See "The
  rule, built", the last section.

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
tokio padding its header would remove it. (A third option, order rather than spacing, follows.)

## Siblings handed out a stride apart (2026-10-22, scratch)

The coordinator's idea: objects of one class allocated one after another are the allocator's
best evidence of siblings, and siblings handed to different threads are the systematic case of
this false sharing; so hand out a small class's slots in strided order instead of neighbour by
neighbour. Scratch builds only; the patches are in `bench/probes/spawn-gap/`.

**The premise holds.** A refill claims the lowest free bitmap words (a run of up to 64 slots
each) or carves a contiguous run, writes the slots into the magazine in ascending address
order, and the magazine pops lowest address first (`back::Descending`). The depot asks for a
whole magazine on every fill (1,022 objects for every class to 64 bytes), so there are no small
first batches on this path, and the next fill resumes at the next free word: consecutive
allocations of a class get consecutive slots across magazine boundaries too. tokio's four
run-queue headers were consecutive slots of the 40-byte class.

**What the scratch build does.** It leaves the bitmaps alone: after a fill, the depot reorders
the magazine so that objects popped one after another lie `stride` slots apart, within units of
64 strides (`interleave-scratch.patch`, `depot/slow.rs`). No slot is skipped and the frontier
is unchanged; only the order within a fill differs. Classes under 64 bytes.

**The stride must be larger than proposed for sizes that do not divide 64.** Objects `s` slots
apart are sure to share no line only if `(s - 1) * size >= 64`, unless the size divides 64.
With stride 2, 40-byte objects 80 bytes apart still share a line when they straddle (those at
160 and 240 share the line at 192). So 64 / size for 8, 16 and 32 bytes, and ceil(64 / size) + 1
for the others (24: 4; 40, 48 and 56: 3). The trace of the scratch build shows tokio's headers
120 bytes apart, each on lines of its own.

**Results**, interleaved with asmalloc, tokio 1.54, medians (runs per build in brackets):

| Job | asmalloc | strided, units of 64 strides | units of 8 strides | first fill of each class only |
|---|---:|---:|---:|---:|
| connect log grpc h2, k cycles/req (2) | 133.0 | 129.2 | 129.4 | 128.4 |
| spawn t4, ns/task (3) | 111.6 | 95.8 | 96.1 | 96.0 |
| spawn t8, ns/task (3) | 203.0 | 186.4 | 185.4 | 187.2 |
| tree depth=20, cycles/node (3) | 40.9 | 60.2 (+47%) | 46.4 (+12%) | 40.6 |
| json 32Mi, cycles/byte (3) | 16.4 | 17.4 (+6%) | 17.6 (+7%) | 16.5 |
| map btree 1M, cycles/entry (3) | 914 | 924 (+1%) | 924 (+1%) | not run |
| map hash 1M, cycles/entry (3) | 531 | 536 (+1%) | 530 | not run |
| churn small, 100 to 10M live, and mixed (1 to 3) | | level | | not run |
| rpc log c64 / c256, k cycles/req (3) | 123.9 / 118.5 | 125.4 / 121.7 | | c256: 120.7 against 119.1 |
| boa, Gcycles (3) | 44.3 | 45.1 (+1.7%) | | not run |

- Every variant takes back the gRPC loss and the header part of the spawn gap.
- Strided order on every fill costs whatever walks same-size objects in allocation order: the
  tree's build-then-walk loses 47%, JSON 6%, the RPC log jobs 1 to 3%, boa 2%. Units of eight
  strides cut the tree's loss to 12% and leave JSON's.
- Strided order on each class's first fill only (`interleave-first-fill-scratch.patch`, a flag
  per class for the process) keeps the whole gain and costs nothing measured on the tree, JSON
  or the RPC log job. The runtime's structures are made at start-up, from the class's first
  magazine. It misses sibling sets made later: a runtime built after the first fills, or
  per-connection structures made in steady state. The jobs marked "not run" were not run for it;
  it touches only the first 1,022 objects of each class, so it should not move them.

**Classes between one and two lines** (80, 96, 112 bytes): neighbouring objects share a
boundary line just as 40 to 56 do, and a stride of 2 would separate them. No measured case: IBS
samples on those classes are few in these jobs (the 96-byte class: 2 HITM samples on the
HTTP/2 order server). Not built.

## The rule, built (2026-10-22)

Branch `sibling-stride` on main `7a82ea4`. **A fill that does not continue a run of fills of
its class hands its first 64 objects out a stride apart, and the rest of the magazine in
address order.** It takes back the spawn gap (t4 111.8 to 95.2 ns a task, t8 207.1 to 185.2),
puts tokio's run-queue headers on lines of their own, and leaves every job that must be level
level. On the gRPC server it moves cycles by less than builds of one rule differ from each
other, so that job cannot confirm the gain there.

**Where the evidence of a run lives.** The depot keeps none: the design's batch sizing ("a run
of trips in the same direction") and its fill that doubles are not built, and the swing bits
in a class's stock word record direction without time. The back end has it. Every fill already
loads the frontier word of each region it claims in (`region::reach`), and that word names the
interval of the last claim recorded there. So a fill opens a run when the first frontier it
records showed no claim of the current counted interval; only the first, since its own record
makes any later one in the region read as claimed. No new state, no new write and no clock
read: `reach` returns what it saw, and `Source::fill` passes it up. Per region, so per class
for any class under 4 GiB, and across CPUs; two fills at the very start of an interval may both
open a run. A server's reorders under the c256 RPC log load, counted with a gdb breakpoint:
45 in the run, about one per class an interval.

**Why only 64 siblings.** Strided whole, the magazine of every run's first fill cost the RPC log
server (connect-rust over HTTP/1.1) 2 to 3%, and so did whole magazines on each class's first
fill only, 1 to 2% (the scratch's first-fill row already showed c256 +1.3%). A server's small
classes circulate through magazines and refill about once an interval (the age limit), so a
strided magazine's order stays in its working set, and a request's own objects then lie on
more lines. Strided over its first 64 strides, the reorder left the RPC log jobs level in both
forms. The first-fill form so limited lost what the gRPC server had shown, and the per-interval
form kept it in that session; neither difference survived more builds (below).

Variants, interleaved with main, three rounds, medians (one session; ns a task and k server
cycles a request):

| Job | main | per interval, whole | first fill, whole | first fill, 64 strides |
|---|---:|---:|---:|---:|
| spawn t4 | 111.9 | 95.6 | 95.5 | 94.2 |
| spawn t8 | 204.4 | 186.9 | 187.7 | 186.0 |
| rpc log c64 | 122.2 | 125.3 (+2.6%) | 123.5 (+1.0%) | 122.8 (+0.5%) |
| rpc log c256 | 118.7 | 122.0 (+2.8%) | 121.1 (+2.0%) | 118.8 |

A second session: rpc log c64 main 123.0, first fill 64 strides 122.5, per interval 64 strides
121.8; c256 118.8, 118.7, 118.5.

**Placement.** Traced builds (`diagnostics.patch`, `rpc-headers.sh`): in rpc-server, tokio's
four headers are the first four 40-byte blocks the process allocates, so every variant's first
fill covers them. On main they lie 40 bytes apart and four pairs share a line; on the branch
they lie 120 bytes apart, at 0x050, 0x0c8, 0x140 and 0x1b8 past the class's first page, and
none shares a line. The spawn workload's are the same four slots.

**The stride** is the briefed one: `64 / size` where the size divides 64, else
`ceil(64 / size) + 1` (24: 4; 40, 48, 56: 3), the fewest slots at which two objects share no
line wherever they start. At the classes' real alignment 48 and 56 bytes would need only 2.
Passes run last first, so the first pass, and with it the siblings, ends a stride below the
first object after them. The reorder is in place, one cycle of the permutation at a time with a
bit per position (64 bytes of stack), since the slow path runs on the caller's stack and the
scratch's copy took 8 KiB of it, more than the slow path's measured peak of about 6 KiB
(`launder-and-fill.md`, "Stack").

**Results**, the commit `741dc1d` (measured as `d0159ae`, which differs from it in one comment)
against main, interleaved, three rounds unless noted, medians:

| Job | Metric | main | branch |
|---|---|---:|---:|
| spawn t4 local | ns/task | 111.8 | 95.2 (-14.9%) |
| spawn t8 both | ns/task | 207.1 | 185.2 (-10.6%) |
| connect log grpc h2 (14 runs, 4 sessions) | k cycles/req | 131.6 | 131.0 |
| tree depth=20 | cycles/node | 40.76 | 40.69 |
| json 32Mi | cycles/byte | 16.42 | 16.35 |
| map btree 1M | cycles/entry | 915.0 | 912.2 |
| map hash 1M | cycles/entry | 526.6 | 529.2 |
| churn live=100 small (5 rounds) | cycles/op | 22.63 | 22.64 |
| churn live=1k / 10k / 100k small | cycles/op | 23.23 / 27.97 / 41.55 | 23.25 / 27.92 / 41.73 |
| churn live=1M small (5 rounds) | cycles/op | 101.4 | 100.9 |
| churn live=10M small | cycles/op | 210.3 | 211.6 |
| churn live=100k mixed | cycles/op | 83.47 | 82.82 |
| rpc log c64 (5 rounds) | k cycles/req | 122.5 | 122.8 |
| rpc log c256 | k cycles/req | 118.1 | 118.9 (+0.7%) |
| boa v8 | G cycles | 44.51 | 44.63 |
| orders axum, s70 | k cycles/req | 68.8 | 68.6 |
| orders axum h2, s70 | k cycles/req | 59.7 | 59.8 |
| connect store 1M, s70 | k cycles/req | 371.0 | 368.5 |

Instructions are level on every job (the churn jobs to 0.01%). A first three-round session had
churn live=100 at +1.7% and live=1M at +2.3%; five rounds each, above, put them level.

**The gRPC server.** Within each session the branch ran +1.5%, -0.3%, -0.5% and -1.4% against
main. Builds of the same rule that differ only in code ran from -3.9% to +1.5% against main in
the same sessions: one build of the per-interval 64-stride rule ran 1.5 to 3.9% below main in
four sessions, and the commit with only its passes' order changed back to match that build ran
like the commit. Instructions per request were level throughout (298k to 299k). So the job
moves with where the allocator's code lands by about as much as the headers were worth here
(padding them took 3.8k off 133.0k in the tokio 1.54 section), and these sessions cannot
resolve the headers' share. The trace shows them separated.

**Found on the way: data placement and the slab's hot words.** That same order-only build ran
churn live=100 12% slower than main and the commit (25.4 against 22.6 cycles an op), with the
same instructions and no more L1 fills, but 22.8M loads blocked by a non-forwardable older store
(`ls_bad_status2.stli_other`, 0x0224) against 1M, and twenty times the op-cache misses. The
blocked loads fall in the workload's own loop and its size sampler, not in the allocator. The
strided order hands out objects at regular page offsets, and which ones a program's hot data
lands on decides whether its loads alias a slab hot word's page offset, the hazard the design
keeps the allocator's own tables clear of ("Front end"). Any order can land on it; this one
moved the dice. Not pursued further.

**Method.** Builds from `git archive` of each commit with identical `bench/` sources
(`bench/probes/spawn-gap/ab-build.sh`), listed as `build = "asm-NAME"` entries in a local,
uncommitted `bench/allocators.toml`, names of equal length; `benchrun run` under
`scripts/measure --both-l3`, builds interleaved in a new random order each round. The variant
builds edited one line each of the branch: `region::reach` returning "the word is zero" for
the first-fill form, and the slice `spread` is given for the 64-stride form. The reorder count:
gdb on rpc-server with a breakpoint on `siblings::spread` under the c256 load. The blocked
loads: `BENCH_EVENTS` on the churn job, and `perf record` of the same event. Raw results:
`docs/research/data/sibling-stride-2026-10-22.jsonl.gz`.
