# What is in the suite, and why

Servers lead, real batch programs follow, and the kernels of `bench/workloads` and the C programs of
mimalloc-bench stay only as regression checks. A real program earns a place if allocators differ on
it by clearly more than its noise and it adds an axis the others lack: thread count, frees on
another thread, size mix, heap size, growth by `realloc`, async, or a short life
([`bench/apps/README.md`](../../bench/apps/README.md), "What belongs here"). What we learned
choosing:

- **The allocator matters in most real Rust programs.** Of 34 configurations of 32 programs, 19
  differ by 8% or more in cycles between best and worst allocator. The median spread is 9.4%, or
  4.8% without glibc (2026-10-19, preloaded).
- **A profile under glibc predicts it.** The rank correlation of a program's spread with its share
  of cycles in glibc's allocator functions is 0.74, and 0.85 with the share under page faults added
  (computed over the 34). Under about 1% in both makes a control or a reject.
- **A third of them ship another allocator.** Ten of the 32 carry jemalloc or mimalloc, so each
  needs a recorded one-line patch, or preloading measures nothing.
- **Kernel gaps do not predict servers.** Large objects crossing threads, large blocks faulted back
  and table growth lead the kernel suite's gaps. Six of seven profiled servers make no large
  allocation, and none grows a table in steady state ([servers.md](servers.md)). A kernel gap is a
  lead to check on a real program, not a target.

Results are in [standings.md](standings.md). This note defines the job names the other notes use.

## Servers

Each runs on CPUs 0-3 with its load generator on 8-11, the allocator as `#[global_allocator]`.

- **The Connect log service** (`connect log`, `connect log h2`, `connect log grpc h2`):
  connect-rust's log-ingest benchmark. A 50-record protobuf batch a request: small blocks freed
  where they were made, 55 `realloc`s a request over HTTP/2. HTTP/1.1 at 85k requests a second,
  HTTP/2 at 63k.
- **The log store** (`connect store 1M`): the same server keeping a million records, 1.06 GiB live
  in a 1.4 GiB heap. Every other request replaces 50 records and every other streams back 100. 36.3k
  a second; 2,610 allocations a request.
- **The order service** (`orders axum`, `orders actix`, `orders axum h2`): written for the suite.
  Parse a JSON order, price it, keep the last 100k in a sharded map, about 180 MiB live; blocks are
  freed by whichever worker evicts them. 235k, 280k and 166k a second.
- **The order service with a large heap** (`orders axum cache 0.5G`, `2G`, `4G`, `10G`): tables
  pre-sized. `orders axum doubling 4G` leaves them unsized, to time one doubling.
- **Sozu** (`sozu proxy`): Sozu 2.3.3 forwarding the order's POST to an upstream; four worker
  processes. **mini-redis** (`miniredis`): SET and GET over 100k keys. Both are near-controls.
- **Apollo Router** (`router graphql`, `router graphql fast`): Router 2.19.0 over Apollo's
  four-subgraph demo: 10 to 40 KB JSON trees built and merged per request. `fast` runs at rates set
  without glibc, whose half rate leaves the others at 37% of capacity.
- **Meilisearch** (`meilisearch hn`): 400k Hacker News items indexed before it listens, then
  searches from 128 connections and a batch of updates every 200th request. About 1 GiB of heap
  beside a 3 GiB mapped store.
- **Qdrant** (`qdrant search`, `qdrant mixed`): a million vectors of 384 floats; in `mixed` every
  20th request upserts 20 points. A control on time, the memory axis on heap. Storage on `/dev/shm`.
- **SurrealDB** (`surrealdb mem`, `surrealdb mem 1.5M`): the in-memory engine with 750k or 1.5M
  records, 1.7 and 3.8 GiB; reads by id and index, one in five updates.
- **RPC jobs of the kernel suite** (`rpc echo c64`, `rpc log c64`, `rpc log c256`): the same server
  under a closed-loop client, which is the bottleneck, so they rank the server's cycles a request.

### A server's day

The generator is open loop: requests come due on a fixed schedule, and latency runs from when one
was due. **`s70`**, the segment that ranks a job, runs at 70% of the slowest allocator's saturated
rate, the same rate for everyone. HTTP/2 jobs use eight connections of eight streams.

- **`servers`:** 2 s closed-loop fill, 4 s to measure saturation, 6 s at 30%, 6 s at 70%, three 2 s
  bursts after 3 s silences, then 10 s after the connections close.
- **`servers-large`:** the heap filled by closed-loop load, a minute at 70%, the bursts, the close.
  **`servers-ladder`** does this at 0.5, 2, 4 and 10 GiB.
- **`servers-thread`:** 30 s at 70%, 30 s of silence, one burst marked in pieces (`b5`, its first 5
  ms, then `b50`, `b500`, `b2`), and a minute after close. "A burst after a silence" means this.

## Batch programs

The `apps` suite, each with its own release profile and its output checked, ranked by cycles of all
threads.

- **ruff** 0.16.9: `ruff check` of Home Assistant core, 19k files, 4 threads; 1.1 s, 40 MB. Ships
  jemalloc.
- **DataFusion** 55.2.0: the 22 TPC-H queries at scale factor 3, 4 threads on one L3; 3.6 s, 1 GiB.
  Large buffers and growing hash tables. Ships mimalloc.
- **boa** 0.22.0: the V8 suite, one thread; 8 s, 250 MB. A tracing GC over many small objects.
- **tpchgen** 4.0.0: TPC-H SF1 to Parquet, 8 threads, preloaded (outside the suite). The only
  program dominated by blocks of 1 to 32 KiB, and the one where cralloc's faults stand out.

At `5097226` (2026-10-20) the allocator moved them by 9% (ruff), 7% (boa) and 6% (DataFusion) in
time, against under 1% between runs, and up to 3.6 times in memory. **mimalloc buys time with
memory:** huge pages for its whole heap, so ruff's peak is 3.4 times glibc's (119 against 35 MiB).
The kernels' heaps hide this.

**DataFusion on two L3s is bimodal.** On eight threads across both domains every allocator's runs
fall in two modes, about 2.4 and 3.5 s, nearly every query 30 to 50% slower in the slow one. On four
threads of one domain eight glibc runs agree within 1%, so the job runs there. The cause is not
known.

## Long-run memory jobs

`workload drift`: stand-ins for hours of service, driven and sampled by operation count, on 8
threads ([memory.md](memory.md)).

- **`steady`:** a cache shared by all threads, sizes around 128 B, most frees another thread's.
  **`shift`:** the same, the median moving 128, 384, 1024 B by phase ("the size shift").
- **`lifetime`, "the lifetime mix":** short-lived FIFOs alternating between a high and a low total
  beside a long-lived cache. Its low phases are "troughs". With a size table per phase it "wanders".
- **`swing`:** the steady cache filled by producers and drained by consumers, busy threads 8, 2, 8,
  1, 8, 4 by phase. Its consumers spin, so its cycles count waiting.
- **`large`, "the large mix"** (also the large-buffer job or cache, the large-block mix): a 1 GiB
  cache of buffers from 40 KiB to 32 MiB by phase, half built by doubling with `realloc`. The
  big-buffer variant makes one buffer in a hundred ten times larger.
- **mini-redis rewritten:** 1.2M keys overwritten through phases of wide and small values. **The
  soak:** Qdrant on disk beside jemalloc for 10.6 hours of varied phases.
- **Short probes:** `quiet` frees most of a heap and watches what stays (`burst-small`,
  `burst-idle`). `touch` is the "phase change": eight CPUs use every class, then one CPU uses one.
  `workload burst` allocates a burst of one class and frees it in reverse. `regrow` times one
  `realloc` a class up.

## The kernel suite

`benchrun run --suite full`, 55 jobs plus xmalloc-test. Figures are `c7f5c21`, 2026-10-22, medians,
in the order cralloc, per-CPU tcmalloc, jemalloc, mimalloc, glibc. `local` is one L3, `split`
alternates L3s between neighbours, `both` fills one then the other.

- **`pipeline`:** a chain of threads, the first allocating and the last freeing. Small objects, one
  L3: 3.3, 6.6, 7.6, 15.2, 127 ns an allocation. Across L3s: 10.5, 24, 18.4, 51, 125. `x2` to `x4`
  are independent chains, which should cost what one does alone: `x4` 4.5, 24, 8.1, 17.6, 142 per
  chain. This is what found the need for a depot stack per cache domain
  ([magazines.md](magazines.md)).
- **`all-alloc`, `free-only`:** whether threads that only free first allocate once, as a service's
  do. jemalloc gives a free-only thread no thread cache: 22 ns against 7.6.
- **`pipeline t2 sizes=large`, "the large pipeline":** 4 to 256 KiB crossing threads: 60.6, 40, 299,
  41.7, 338 (per-thread tcmalloc 35.5). The one cross-thread job cralloc loses.
- **`exchange`:** a ring, each thread freeing what the last sent. **`mpmc`:** a shared queue.
  **`forkjoin`:** rayon stages. All three as `pipeline`: 3.7, 6.9, 7.6, 11.9, 45 ns on `exchange
  t2`.
- **`spawn`:** tokio tasks: 95.8, 96.1, 94.2, 97.2, 97.9 ns. It ranks where a few of tokio's lines
  land ([program-effects.md](program-effects.md)).
- **`churn live=N`:** free a random object and allocate another. `live=100` sits in L1: 21.6, 37.3,
  33.9, 45.3, 34.8 cycles an operation. `live=10M` reaches DRAM: 208, 299, 245, 426, 457, and the
  suite's largest peak, 1.0 to 1.2 GiB by allocator.
- **`map btree 1M` ("BTreeMap"), `map hash 1M` ("HashMap"), `tree depth=18` and `20` ("the Box
  tree"), `json`:** build and drop, where mimalloc's huge pages tell. `map hash` 533, 609, 636, 462,
  1,108 cycles an entry; `json 32Mi` 16.0, 28.8, 23.4, 16.8, 22.8 cycles a byte.
- **`vecgrow`:** vectors doubled by `realloc`, filled by `memset` so that the job measures growth
  and not a push loop. `vecgrow 1x256Mi` ("one Vec to 256 MiB"): 2.18, 11.8, 14.5, 9.65, 6.66 cycles
  an element. `vecgrow 64x4Mi` ("64 Vecs", "the 64-Vec job"): 4.89, 6.42, 8.63, 3.95, 8.72.
- **`largepair`:** one 64 KiB and one 1 MiB block, allocated, written once and freed: 49.9, 59.1,
  131, 90.5, 8.07 ns a pair. glibc serves both from the top of its heap.
- **C programs, preloaded:** `larson` (C++ through libstdc++'s operators), `xmalloc-test` (two modes
  by where the scheduler puts each pair, so its medians say little), `sh8bench`, `mstress`,
  `cache-scratch`, `cfrac`, `espresso`. `cache-scratch` tests nothing as built: `-O3` removes its
  worker's `new`, `delete` and heap store, so the loop touches only the stack.

Among incumbents, jemalloc and tcmalloc lead cross-thread work (and tcmalloc large objects crossing
threads), mimalloc build-and-drop, and jemalloc is the memory floor on large heaps.

### The smoke set for the back end

`benchrun run --suite backend --rounds 3`, on every change to the back end, about a minute a round.
Each job exposed a cost once: large objects crossing threads, two pipelines on different L3s, a tree
of depth 18, JSON of 8 MiB, sh8bench preloaded, and one buffer doubled to 1 and to 4 MiB and freed.

## The screening: what each program showed

2026-10-19, cralloc at `5097226` preloaded against glibc 2.43, Fedora's jemalloc and mimalloc 3.5.3:
three rounds, a fresh pinned process a run. No abort, hang or wrong output in over 400 runs. Spread
is best to worst in cycles. On three runs programs rank by sensitivity; allocators within a program
do not rank.

**Recommended**, each for an axis:

- **jaq** (20%): the most allocation-bound, 37% of glibc's cycles, 38M live blocks; moves with THP
  state.
- **boa** (23%): cralloc 10% ahead of jemalloc, its best case. **egglog** (16%): median lifetime
  under 100 ns; mimalloc 7% ahead in cycles. **rhai** (9%): a heap under 4 MB isolates the fast
  path; mimalloc 7% ahead.
- **pulldown-cmark** (13%): one buffer grown to 1 GiB. The allocator's code is 0.1% of cycles, yet
  allocators differ through faults and how the block grows.
- **ruff check** (15% on one thread, 12% on eight, 27% cross-thread frees). **oxlint** (10%): 41%
  cross-thread. **rust-analyzer** (13%): salsa's graph, 11M live blocks, steady; mimalloc 4% ahead.
  **typst** (14%): a steady 770 MB; mimalloc 7% ahead.
- **datafusion** (10%): the widest size mix. **gix-hours** (11%): 30% of calls are `realloc`, 2.6
  GB. **tpchgen** (7%). **uv-loop** (17%): short processes, 43% of cycles in the kernel. **axum**
  (15%) and **actix** (24%): the order service; under actix half its blocks are freed on another
  thread.
- **Controls:** ripgrep and rav1e, zero spread in cycles. **cargo-resolve** (25%; mimalloc 10%
  ahead) needs a private, pinned `CARGO_HOME` first.

The suite has built boa, ruff, DataFusion and the order service, with tpchgen beside it; the rest
are candidates. Sozu, the Router, Meilisearch, Qdrant and SurrealDB came later, as real servers.

**What real programs' allocations look like.** In almost every program 75 to 100% of blocks are 256
bytes or less; tpchgen, oxipng, datafusion and pulldown-cmark are the exceptions. `realloc` is 30%
of gix's calls and 28% of tokei's, 90 to 99% of them doubling. The median block lives under 1 us in
egglog, rhai, starlark, axum, mini-redis and rust-analyzer, and about as long as the process in jaq,
boa and actix.

**Rejected:**

- rustpython, wasmi, parquet-rewrite (about 4% or less): rustpython spends 5% in the allocator at
  23M calls a second and still does not move. mini-redis (3%) stays as a server control.
- tokei, tantivy, oxipng, wasmtime (1 to 5%), with noisy wall times from their own scheduling.
- ty (6%): salsa again, with wall times of 2.0 to 3.6 s. difftastic (18%): a 0.6 s run with 30% wall
  noise; worth a larger input later.
- xsv, bat (6 to 7%), starlark (8%, mostly faults), fd-loop (13%, but 0.4 s): covered by others.
- ruff format (3%), though cralloc's peak there, 1.51 times glibc's, was its worst memory result.

**Not screened:** swc, pingora and wild (not reached); polars (links jemalloc); tonic (connect-rust
covers it); iggy, fjall, redb, simd-json, syn, regex, egg, bevy_ecs, rapier (no program to run);
1BRC (avoids allocating); mdbook, zola (pulldown-cmark is their parser).

**A lead still open:** on ruff check cralloc's wall time was 0.79 to 0.83 s against 0.64 to 0.78, at
the same cycles and the lowest CPU time, with about 1,400 context switches against 150 to 370.
Threads waiting on the mmap lock behind `mprotect` fit, but it is not established.

**Where a mistake could hide:** a dependency with its own allocator or C pools still looks
insensitive; a preloaded run never sees Rust's sized free; only standard output was compared; shell
loops report the shell's peak, so memory for fd, uv, cargo and egglog is missing.

## Open questions

- **Native against preloaded.** The screening's ranking may change with each allocator as the global
  allocator. Only ruff, boa, DataFusion and the order service have both.
- **The programs not built:** jaq, rust-analyzer, typst, oxlint and uv-loop carry axes the suite
  lacks.
