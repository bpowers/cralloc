# Where the allocator's time goes on servers

2026-10-22, allocator at main `040033a` (the sibling stride; main's later `fb9b4ab` changes one
constant in `depot/siblings.rs`), jemalloc as tikv-jemallocator builds it. Seven server jobs,
each profiled inside one binary per allocator, so the placement trap (`grpc-placement.md`)
does not enter any number below except the totals, which are one build's.

## Summary

- **On every job asmalloc spends less of the server's time than jemalloc does, and little of
  it**: 3.6 to 5.0% of the server's cycles on the order service and the RPC jobs, 7.4% on
  Meilisearch, 11.9% on the streaming store, against 4.8 to 6.9%, 10.4% and 15.2% for
  jemalloc. Slow paths, maintenance, system calls, page faults and the background thread
  together are under 0.1% of the cycles on five jobs, 0.3% on the store and 1.2% on
  Meilisearch. What is left is the inlined fast path and `realloc`.
- **`realloc` is the largest single cost on the RPC jobs**: 2.5% of the gRPC and Connect h2
  servers' cycles (3,300 a request), about 0.9% elsewhere. Every `realloc` in these servers
  moves a small block one class up (Vec doubling: the gRPC service grows 50 buffers a request
  from 128 to 256 bytes). The path runs about 240 instructions a call besides the copy, 100 of
  them computing size classes the slow way (`Class::of_size`) several times over. A lean
  small-to-small path should take back about two thirds of it.
- **The fast path costs two to three times its roofline in real programs**: 4 to 12 cycles an
  operation against 2 to 2.5. On calls whose layout is not a constant (most of the store's and
  Meilisearch's) the class, slab word and mask take about 10 instructions beside the 11 of the
  routine. The fast path is 10.6% of the store's cycles and 5.3% of Meilisearch's, so a
  cheaper non-constant lookup is the second candidate, worth up to 2% on the store; it is an
  estimate, not measured.
- **Placement costs the program nothing measurable against jemalloc**: the program's misses on
  heap memory per request are level on every job (over HTTP/2 the order service may lose a few
  hundred cycles to DRAM; its cycles do not show it), and asmalloc takes about 60% fewer page
  walks on the store. Page walks are under 1% of every job's cycles.
- **What the kernel suite's gaps predict does not appear**: no server but Meilisearch makes a
  large allocation, and its large blocks are freed by the thread that made them 99% of the
  time; only Meilisearch returns large blocks and faults them back (the hold), at about 0.3%
  of its cycles over jemalloc's; no server grows a table in steady state.

## Method

**Jobs.** From the servers suite, `orders axum`, `orders axum h2`, `connect log grpc h2` and
`connect log h2`; from the large-heap suite, `orders axum cache 2G`, `connect store 1M` and
`meilisearch hn`. Each run is one `benchrun run --rounds 1` of the job, unchanged: server on
CPUs 0-3 with its cores held out of deep idle, load from 8-11, capped, under
`scripts/measure --both-l3`. `bench/probes/server-profile/record.py` watches for the load
generator and records CPUs 0-3 during the 70% segment: 5 s of the servers suite's six, 10 s
of the large-heap minute. Only the server's own samples are kept.

**Cycles** (`record.py --mode cycles`, `where.py`): `perf record -e cycles -c 500000 -g -j
any,u`. Each sample is placed by its instruction:

- Allocator code is named by line tables. rpc-server has them in the bench profile
  (`debug = 1`); the order service and Meilisearch were measured as line-table builds of the
  same source (`build-twin.sh`; for Meilisearch, line tables for asmalloc and bench-alloc
  only). `llvm-symbolizer --inlining` gives each sampled address its chain of inlined
  functions, and a chain that reaches `crates/asmalloc/src` is the allocator's, put in a path
  by its frames. This is how the inlined fast path is counted: the pop and push routines, the
  class lookup, and everything else LTO inlined into the program. As a check, each inlined
  routine's range is read from the binary's rseq descriptors (`elfinfo.py`); samples inside
  them all land in the fast path or in `realloc`, never in the program.
- jemalloc is C called out of line: its code is the address range between its first and last
  `_rjem_` symbols, static functions included, and named by function.
- A sample in libc's `memcpy`, `memmove` or `memset` is the allocator's when the last call
  from the binary in the sample's branch record (AMD's LBR) came from allocator code. The
  record held the call for 43% of such samples; in those, realloc's copies were a tenth or
  less of the copying (most is the program's), so the unattributed rest (0.8 to 2% of cycles,
  counted as the program's) hides at most a few tenths of a percent of the allocator's.
- Kernel samples are put by their kernel call chain: a page fault, a named system call, or
  interrupts. A fault is the allocator's when the faulting instruction is; a system call when
  the instruction or the last call before it is.
- The background thread (`comm` asmalloc) is apart.

Shares come from the recording; cycles a request are the share times the runner's cycles a
request over the whole 70% segment of the same run. Two runs per job and allocator, in two
sessions; the two agree to 0.3 points of share on every row that matters.

Skid: plain cycles samples land a few instructions late, which moves cost between the routine
and its neighbours. IBS op samples, which are exact, give the same picture in ops
(`opsby.py`), so a figure for the fast path is good to about a fifth of itself; a figure for a
path that is a function of its own (realloc, the slow paths, jemalloc's) is better.

**Misses** (`record.py --mode ibs`, `misses.py`): IBS op sampling, one op in 500,000, with
data addresses and miss latency, and every page fault with its address. A data address is
heap (asmalloc's reservation; for jemalloc every anonymous mapping that holds no thread's
stack), stack, file or other; the instruction is the allocator's or the program's as above.
Per request is samples times the period over the requests. The runner counts user-mode dTLB
misses, page walks and L1 fills per request beside it (`runner-events.patch`). One run per
job and allocator, three for the order service over HTTP/2 and the store, where the first run
differed. For Meilisearch, jemalloc's "heap" includes the C library's arenas (LMDB's), which
asmalloc's runs show apart, so its heap rows do not compare.

**Counts** (`count_edit.py`, `prof.rs`, `count.py`, `counts.py`): a scratch build of the
allocator with counters at the slow paths' decisions, the large path, resizes and its system
calls, behind a wrapper that counts calls and records a window of them (layout, address, CPU,
time, and whether the call took a slow path), then follows one block in 64 by address to its
free. Counters are read from the server's memory twice in the 70% segment, so their rates are
per request; the record gives shares and lifetimes. A different binary, never timed.

## The jobs

Cycles a request by path, mean of two runs each; % is of the server's cycles. "Allocator"
sums the paths. Server totals are one build's (on the rpc-server jobs, `grpc-placement.md`
puts any one build anywhere in a range of 2.5 to 5%). Calls are per request from the counting
build.

### orders axum (HTTP/1.1)

| | asmalloc | % | jemalloc | % |
|---|---:|---:|---:|---:|
| fast path, allocate | 643 | 1.00 | 688 | 1.06 |
| fast path, free | 1,207 | 1.88 | 1,497 | 2.30 |
| realloc | 581 | 0.91 | 941 | 1.44 |
| slow paths | 0 | 0.00 | 129 | 0.20 |
| kernel, background thread | 0 | 0.00 | 0 | 0.00 |
| **allocator** | **2,433** | **3.80** | **3,255** | **4.99** |
| server | 64,060 | | 65,200 | |

138 allocations and frees and 8 reallocs a request; 77% of allocations are 8 or 16 bytes.
Every realloc moves (8 to 16 up to 512 to 1,024 bytes), 1,290 bytes copied a request. No slow
path in 940,000 requests, no large call, no system call. 2% of all frees, and 20% of a sample
followed by address, happen on another CPU: the cached orders (96, 128, 160, 48 bytes), which
live 0.42 s and are freed by whichever worker evicts them, 66 to 84% elsewhere.

### orders axum h2

| | asmalloc | % | jemalloc | % |
|---|---:|---:|---:|---:|
| fast path, allocate | 966 | 1.50 | 926 | 1.42 |
| fast path, free | 1,617 | 2.50 | 1,863 | 2.86 |
| realloc | 636 | 0.99 | 881 | 1.35 |
| slow paths, maintenance | 11 | 0.02 | 145 | 0.22 |
| **allocator** | **3,235** | **5.01** | **3,817** | **5.86** |
| server | 64,591 | | 65,109 | |

140 allocations, 8 reallocs. A slow path every 1,700 requests (0.0006 a request), mostly depot
pops and spare swaps. 25% of followed frees are on another CPU.

### orders axum cache 2G

| | asmalloc | % | jemalloc | % |
|---|---:|---:|---:|---:|
| fast path, allocate | 620 | 0.92 | 668 | 0.98 |
| fast path, free | 1,181 | 1.76 | 1,545 | 2.26 |
| realloc | 606 | 0.90 | 935 | 1.37 |
| slow paths | 0 | 0.00 | 142 | 0.21 |
| **allocator** | **2,409** | **3.58** | **3,291** | **4.81** |
| server | 67,196 | | 68,479 | |

The same calls as the small order service; holding 2 GiB changes none of them. No slow path in
2.4 million requests, no fault, no system call.

### connect log grpc h2

| | asmalloc | % | jemalloc | % |
|---|---:|---:|---:|---:|
| fast path, allocate | 1,099 | 0.83 | 1,247 | 0.92 |
| fast path, free | 575 | 0.43 | 818 | 0.60 |
| realloc | 3,396 | 2.56 | 5,403 | 3.97 |
| slow paths | 1 | 0.00 | 1,970 | 1.44 |
| **allocator** | **5,072** | **3.82** | **9,441** | **6.93** |
| server | 132,663 | | 136,142 | |

136 allocations (128 and 48 bytes, 37% each) and 55 reallocs a request, 50 of them 128 to 256
bytes, 14.5 KB copied. Everything dies within 35 us on the CPU that made it. No slow path in
250,000 requests. jemalloc's slow paths are its thread event counters
(`te_recompute_fast_threshold`) and tcache flushes.

### connect log h2

| | asmalloc | % | jemalloc | % |
|---|---:|---:|---:|---:|
| fast path, allocate | 958 | 0.73 | 1,070 | 0.77 |
| fast path, free | 503 | 0.38 | 921 | 0.67 |
| realloc | 3,300 | 2.51 | 5,321 | 3.85 |
| slow paths | 0 | 0.00 | 1,723 | 1.25 |
| **allocator** | **4,761** | **3.63** | **9,035** | **6.54** |
| server | 131,294 | | 138,116 | |

As the gRPC job: 135 allocations and 55 reallocs.

### connect store 1M

| | asmalloc | % | jemalloc | % |
|---|---:|---:|---:|---:|
| fast path, allocate | 21,023 | 5.73 | 24,821 | 6.35 |
| fast path, free | 17,828 | 4.86 | 25,618 | 6.55 |
| realloc | 3,482 | 0.95 | 4,673 | 1.19 |
| slow paths | 714 | 0.19 | 4,343 | 1.11 |
| maintenance on request threads | 512 | 0.14 | 6 | 0.00 |
| kernel, background thread | 8 | 0.00 | 21 | 0.01 |
| **allocator** | **43,568** | **11.87** | **59,550** | **15.23** |
| server | 366,940 | | 391,037 | |

2,610 allocations a request, 80% of them 16 or 24 bytes, and 34 reallocs (52 KB copied).
About one slow path a request, nearly all swaps with a spare (0.98 a request), a depot trip
every 12 requests, a fill every 50; a heartbeat a request. 15% of followed frees are on
another CPU: the stored records (320 and 448 bytes, living a second), 42 to 76% elsewhere.

### meilisearch hn

| | asmalloc | % | jemalloc | % |
|---|---:|---:|---:|---:|
| fast path, allocate (out of line: no LTO) | 582,351 | 3.52 | 903,999 | 5.09 |
| fast path, free | 298,700 | 1.80 | 458,593 | 2.58 |
| zeroing for `alloc_zeroed` | 47,307 | 0.29 | 65,591 | 0.37 |
| realloc | 131,391 | 0.79 | 146,226 | 0.82 |
| slow paths | 68,449 | 0.41 | 206,437 | 1.16 |
| maintenance on request threads | 27,527 | 0.17 | 1,083 | 0.01 |
| large objects | 1,640 | 0.01 | 2,984 | 0.02 |
| kernel: madvise and the like | 64,798 | 0.39 | 55,023 | 0.31 |
| **allocator** | **1,222,529** | **7.38** | **1,840,339** | **10.36** |
| kernel: page faults in program code | 199,621 | 1.21 | 149,968 | 0.84 |
| server | 16,563,805 | | 17,767,448 | |

49,600 allocations (8,700 of them zeroed), 1,250 reallocs (330 KB copied) and 4 large
allocations a request. 0.08% of calls take a slow path (80 a request: half spare swaps, 17
depot pops, 3 fills, 3 launders). 0.76 madvise calls a request return 210 KB. Large blocks,
mostly 128 KiB to 16 MiB, live 0.1 ms at the median and 42 ms at p99, and 99% are freed by
the thread that made them. The page faults the program takes on asmalloc's heap, 39 a
request against about 18 on jemalloc's own (the rest of jemalloc's 40 are C arenas), are the
hold's returned slots faulted back (`meilisearch-memory.md`): about 50,000 cycles a request,
0.3%.

### The fast path, per call

| job | asmalloc alloc / free, cycles a call | jemalloc alloc / free |
|---|---|---|
| orders axum | 4.7 / 8.7 | 5.0 / 10.8 |
| connect log grpc h2 | 8.1 / 4.2 | 9.2 / 6.0 |
| connect store 1M | 8.1 / 6.8 | 9.5 / 9.8 |
| meilisearch hn (called out of line) | 11.7 / 6.0 | 18.2 / 9.2 |

Against 2 to 2.5 cycles an operation inlined in the roofline (`zen5-roofline.md`). The
alloc/free split moves with skid; the sum is firmer. IBS on the store: the pop and push
routines take 11 ops a call each, and the code around them, for calls whose layout is not a
constant, about 10 more: `SmallClass::of_nonzero_layout`'s table lookup, the slab word and the
mask, one load from each of two tables (`BY_SIZE`, `MASKS`) in a dependent chain. Cycles
inside the pop routine fall on the slab word's load (26%) and the magazine slot's (37%), and
20% on the arming instruction, which reads as skid from the code before. The allocator's own
L1 misses are about 180 a request on the store (magazines, slab words, tables), most served
from L2: a few thousand cycles of the 39,000.

## What placement costs the program

Loads and stores of the program on heap memory per request (IBS): L1 misses, those served by
another core's cache, from DRAM, and page walks. asmalloc / jemalloc.

| job | L1-missing loads | from another core | from DRAM | load miss latency, cycles | page walks (runner, all user memory) |
|---|---|---|---|---|---|
| orders axum | 258 / 257 | 94 / 92 | 19 / 20 | 18.1k / 17.7k | 2.7 / 4.0 |
| orders axum h2 | 358 / 355 | 71 / 76 | 31 / 23 | 21.6k / 18.2k | 12.1 / 13.5 |
| orders axum cache 2G | 265 / 251 | 96 / 87 | 20 / 18 | 19.1k / 17.7k | 5.0 / 6.1 |
| connect log grpc h2 | 1,689 / 1,702 | 41 / 49 | 0 / 0 | 31.5k / 31.6k | 7.3 / 6.1 |
| connect log h2 | 1,459 / 1,552 | 44 / 60 | 0 / 0 | 26.1k / 30.2k | 6.6 / 3.8 |
| connect store 1M | 3,018 / 2,890 | 714 / 596 | 237 / 255 | 202k / 207k | 33 / 90 |

- Level on every job within the runs' spread, with one small exception. Over HTTP/2 the order
  service's heap loads went to DRAM 36, 25 and 32 times a request under asmalloc against 17, 24
  and 28 under jemalloc, which puts its summed load-miss latency 3,400 cycles higher in two
  runs of three; its cycles a request are level with jemalloc's all the same (64.6k against
  65.1k), so I read it as at most a few hundred cycles of real stall. Not mapped to blocks. The store's
  loads from another core ranged 672 to 745 for asmalloc and 504 to 707 for jemalloc over three
  runs each, with the same total miss latency. User-mode L1 fills per request (runner) agree:
  624/626 on the order service, 916/921 over HTTP/2, 4,263/4,298 on the store.
- asmalloc's heap takes fewer TLB misses where the heap is large: on the store 440 dTLB misses
  and 33 walks a request against 697 and 90. On Meilisearch it takes more L1 dTLB misses
  (27,300 against 19,800 a request, nearly all served by the L2 TLB) and slightly more walks
  (2,530 against 2,250): about 0.3% of its cycles at the L2 TLB's few cycles.
- Page walks are a small share everywhere: at 30 to 60 cycles a walk, under 0.1% of the order
  service and RPC jobs, 0.3 to 0.5% of the store and 0.5 to 0.9% of Meilisearch.
- jemalloc's own metadata misses more: on the store its code takes 246 L1-missing loads a
  request on heap memory with about 150,000 cycles of summed latency, against asmalloc's 55
  and 1,000.

Since nothing here differs materially, the extra misses were not mapped to blocks.

## The order service and code placement

Asked in passing: does the order service move with where code lands, as the rpc-server jobs do?
`benchrun layout` builds the workloads only, so `pad-orders.sh` builds the order service with
bench-alloc's pad of 0, 16, 32, 48 and 64 bytes for each allocator, and `pad-sweep.sh` runs
`orders axum` over them, two rounds in four sessions (`probes/spawn-gap/padsweep.py`). Server
cycles a request at 70%, medians of two:

| | pad 0 | 16 | 32 | 48 | 64 | mean | range |
|---|---:|---:|---:|---:|---:|---:|---:|
| asmalloc | 67.9k | 68.8k | 69.6k | 68.7k | 67.4k | 68.5k | 3.3% |
| jemalloc | 70.4k | 70.1k | 70.2k | 69.9k | 69.5k | 70.0k | 1.2% |

The pairs of runs agree within 0.6%. So the order service is placement-sensitive too, about
as much as the rpc-server jobs for asmalloc's builds and less for jemalloc's; averaged over
the pads asmalloc leads by 2.3% (3.6% at pad 0 alone). Which code moves it was not looked for.

## Ranked: what is left to win

Sizes are what asmalloc spends now on the path; "to win" is my estimate of what the change
named would take back.

| # | Cost | Jobs, cycles a request (% of the job) | To win | Change |
|---|---|---|---|---|
| 1 | `realloc` of small blocks, which always move one class up | gRPC 3,400 (2.6%), Connect h2 3,300 (2.5%), store 3,500 (0.95%), orders 580 to 640 (0.9 to 1.0%), Meilisearch 131k (0.8%) | about two thirds: 1.7% on the RPC jobs, 0.6% on the order service, 0.5% on Meilisearch and the store | a lean small-to-small move: each class from the size table once, pop, copy, push; today about 240 instructions a call besides the copy, 100 of them in `Class::of_size` |
| 2 | The fast path's class, word and mask lookup when the layout is not a constant | store 38,900 for the whole fast path (10.6%), Meilisearch 881k (5.3%), orders 1,850 (2.9%), RPC 1,500 to 1,700 (1.1 to 1.3%) | up to 2% on the store, under 1% elsewhere; not measured | one table load giving the slab word's offset and the mask together, instead of a chain of two tables and arithmetic |
| 3 | The hold's returned large slots faulted back (Meilisearch only) | about 50k in faults over jemalloc's (0.3%), beside 65k of madvise against jemalloc's 55k | most of the 50k | the adaptive hold `meilisearch-memory.md` asks for |
| 4 | Slow paths and maintenance on request threads | Meilisearch 96k (0.6%), store 1,230 (0.3%), elsewhere under 0.03% | little | none needed |
| 5 | L1 dTLB misses on Meilisearch's heap | about 0.3% | little | huge pages for classes over 1 KiB, only if a large heap needs them for something else |
| 6 | The background thread | under 0.01% on every job | nothing | none |

### Not worth doing for servers

- **Large objects crossing threads** (the kernel suite's 4 to 256 KiB pipeline): six of the
  seven jobs make no large allocation at all; Meilisearch makes four a request and frees 99%
  of them on the thread that made them. A per-CPU cache or batched frees for large blocks
  would buy no server anything.
- **Large blocks returned and faulted back** (HashMap, vecgrow): only Meilisearch shows it, as
  item 3. The order service and the RPC jobs make no system call and take no fault in a
  minute at 70%; the store takes 0.003 faults a request.
- **Table growth** (doubling): none in steady state. The order service's one doubling is the
  separate `servers-doubling` job (`servers-large.md`).
- **Slow paths**: zero in a million requests on the order service and the RPC jobs, 0.02% of
  calls on the store and 0.08% on Meilisearch; asmalloc spends a fifth to a twentieth of what
  jemalloc spends there.
- **Frees on another CPU**: 2% of frees on the small jobs, up to 20 to 25% of blocks followed to
  their end on the order service, 15% on the store, under 1% on Meilisearch and the RPC jobs.
  They cost asmalloc nothing visible: a free is a push on the freeing CPU, and the program's
  cross-core misses are level with jemalloc's.
- **Placement of blocks**: the program's heap misses are level with jemalloc's on every job but
  a possible few hundred cycles on the order service over HTTP/2.

## Files

`bench/probes/server-profile/`: `record.py` (one profiled run), `campaign.sh` (runs of a plan,
one measure session each), `where.py` (cycles by path), `misses.py` (misses by memory and
code), `summarize.py` (the tables), `count_edit.py`, `prof.rs`, `build-count.sh`, `count.py`
and `counts.py` (the counting build), `elfinfo.py` (rseq descriptors), `rseqhist.py` and
`opsby.py` (inside the fast path), `hotsite.py`, `counters.py`, `textcmp.py`, `bytediff.py`,
`build-twin.sh`, `pad-orders.sh` and `pad-sweep.sh` (the placement sweep), `runner-events.patch` (raw events per request in the runner,
the spawn-gap hunks). Results (the analyses' JSON and the runner's records):
`docs/research/data/server-profile-2026-10-22.tar.gz`.
