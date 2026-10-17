# Allocator baselines on workloads

Measured 2026-10-17, 00:01 to 01:30, with the suite in `bench/` (commands and method in
`bench/README.md`), at commit `60ce673`. This is the bar asmalloc has to clear: for each
workload, which of glibc, mimalloc, jemalloc and tcmalloc is fastest and which uses the least
memory. `docs/research/zen5-roofline.md` has the fast-path microbenchmarks; this document
extends them to whole programs.

The machine was shared with other work for the whole session (section 2). The control came
out null at the rate chance predicts, and single-threaded results are tight, but
multithreaded differences under about 10% are not resolved here. A rerun on a quiet machine
should replace the numbers, not the method.

Raw results: `docs/research/data/baselines-2026-10-17.jsonl.gz` (one JSON line per run;
`benchrun report` reads it after `gunzip`). Per-workload tables with every allocator, spread,
memory and page faults: `docs/research/data/baselines-2026-10-17-detail.md`.

Contents

1. Summary
2. Machine, versions and method
3. Fastest and smallest, per workload
4. Cross-thread
5. Work stealing and the RPC server
6. Single-threaded
7. C programs through LD_PRELOAD
8. Memory
9. What contradicts the design's assumptions
10. Caveats
11. Changes to the suite after this session

## 1. Summary

- **No incumbent wins everywhere.** jemalloc is fastest on most cross-thread jobs and on
  single-threaded churn; tcmalloc in per-CPU mode is fastest or second on most cross-thread
  jobs; mimalloc is fastest on allocation-heavy builds (trees, maps, JSON), on the
  decode-heavy RPC server (tied with per-CPU tcmalloc) and on most mimalloc-bench programs. asmalloc has to beat a different incumbent on each.
- **mimalloc is 1.5 to 2.8 times slower than jemalloc on most cross-thread jobs**,
  including one-way pipelines within one L3 domain (16.4 against 7.8 ns per allocation).
  The exceptions are mixed sizes (1.15 times) and many producers feeding one consumer,
  where mimalloc beats jemalloc.
- **jemalloc collapses on cross-thread large objects** (4 to 256 KiB): 319 ns per
  allocation against 37 to 44 for everyone else.
- **mimalloc's build-heavy wins come from huge pages, not its fast path.** It takes a few
  hundred page faults where the others take tens of thousands. With random frees over a
  live set of a million objects or more (churn) it is 1.4 to 1.7 times slower than jemalloc.
- **mimalloc is not the memory floor.** It has the largest or nearly the largest peak
  resident set on most workloads, up to 8 times tcmalloc's (large objects) and 1.6 times
  jemalloc's (hash map); it keeps everything resident after a free. jemalloc or glibc is
  smallest almost everywhere that uses more than a few MiB.
- **tcmalloc without per-CPU caches is a different allocator**: 1.5 to 2 times slower than
  per-CPU mode on most cross-thread and churn jobs, 30 times on xmalloc-test. On glibc 2.35
  and later it falls into that mode silently.
- **Rust's sized free buys nothing on mimalloc** (it ignores the size). mimalloc is 13
  cycles per operation faster on churn when preloaded (plain `malloc` and `free`) than
  through the `mimalloc` crate's `mi_malloc_aligned`.

## 2. Machine, versions and method

Machine as in the roofline study: AMD Ryzen 9 9950X (Zen 5), 16 cores with SMT, two L3
domains (CPUs 0-7 with siblings 16-23, and 8-15 with 24-31), Linux 7.2.9-201.fc44, glibc
2.43, rustc 1.99.1, GCC 16.2.1. Governor `powersave`, energy preference
`balance_performance`, boost on, transparent huge pages in `madvise` mode (defrag
`madvise`), NMI watchdog on.

| Allocator | Version | Built | Rust `#[global_allocator]` calls | LD_PRELOAD |
|---|---|---|---|---|
| glibc | 2.43 | system | `std::alloc::System` | none |
| mimalloc | v3.5.3 (`31d034d9`) | `src/static.c`, `-O3`, `MI_DEBUG=0 MI_BUILD_RELEASE NDEBUG`, as `libmimalloc-sys` builds it; static | `mi_malloc_aligned`, `mi_zalloc_aligned`, `mi_realloc_aligned`, `mi_free`, as the `mimalloc` crate | cmake Release shared library with override |
| mimalloc-sized | the same | the same | the same, freeing with `mi_free_size_aligned` | not run |
| jemalloc | 5.3.0 (Fedora `5.3.0-14.fc44`, profiling compiled in) | system shared library | `malloc`/`mallocx`, `calloc`, `realloc`/`rallocx`, `sdallocx`, as `tikv-jemallocator` | the same library |
| tcmalloc-percpu | google/tcmalloc `f3c6669a` | bazel 8.8.0 `-c opt` (GCC, `-O2`); static archive of tcmalloc and abseil | aligned nothrow `new`, sized aligned `delete`, as `tcmalloc-better` 0.1.19; in-place `realloc` when the size class allows | the shared library; both with `GLIBC_TUNABLES=glibc.pthread.rseq=0` |
| tcmalloc-thread | the same | the same | the same | the same, without the tunable |
| mimalloc-ctl | mimalloc, built a second time | | | |

`bench/baselines/build.sh` rebuilds all of these and writes the exact flags to
`bench/baselines/out/manifest/`.

Each run is a fresh process. A round runs all 47 jobs; within each job the seven
allocators run in a new random order. Ten rounds, 3,170 runs, none invalid: every run
reported the allocator and, for tcmalloc, the mode it was meant to have (tcmalloc's own
`MallocExtension_Internal_GetPerCpuCachesActive`, through the binary or a preloaded probe),
and no run had multiplexed counters. Threads are pinned one per physical core on CPUs 0-3
and 8-11; single-threaded jobs use CPU 2. Single-threaded jobs rank by cycles (user and
kernel) per operation over the measured phase; multithreaded ones by wall time per
allocation, because spinning threads make cycles misleading; C programs by their own
throughput or by wall time. Peak resident set is `VmHWM` read by the process at exit.

**Load.** Other teammates' builds and tests ran throughout. The quiet gate was cut to 5
seconds so the session could finish, and records what it saw instead: across runs, the SMT
siblings of the measurement CPUs were busy (median 0, 90th percentile 2 CPUs' worth), and
other processes ran on the measurement CPUs themselves about one CPU's worth at times.

**Control.** mimalloc-ctl (mimalloc from a second build directory) against mimalloc, paired
by round: the 95% interval excluded zero in 3 of 47 jobs, where chance predicts 2.4, and
the largest median difference was 4.7%. The intervals are what matter for reading the
tables: within 1 to 2% on single-threaded and RPC jobs, but up to ±10% on several
cross-thread jobs and wider on three: forkjoin with 8 threads (up to +57%), sh8bench (−27%
to +44%) and cache-scratch 8 (up to +15%). Leaving out runs with measurable interference
(`report --quiet-only`, a third of the runs) changes only winners the table below already
marks as ties.

## 3. Fastest and smallest, per workload

"Fastest" is the lowest median of the job's metric (highest for rates). The margin is how
much worse the runner-up is; "tie" means the 95% interval of the per-round ratio includes no
difference. "Smallest" is the lowest median peak resident set. Controls (mimalloc-ctl) are
not ranked. All rows have 10 runs per allocator.

| Workload | Metric | Fastest | Margin | Smallest | Margin |
|---|---|---|---|---|---|
| pipeline, 2 threads, 64 B, one L3 | ns/alloc | jemalloc | 40% vs tcmalloc-percpu | glibc | 28% vs jemalloc |
| pipeline, 2 threads, 16-256 B, one L3 | ns/alloc | tcmalloc-percpu | 17% vs jemalloc | glibc | 46% vs jemalloc |
| pipeline, 2 threads, mixed sizes, one L3 | ns/alloc | tcmalloc-percpu | 30% vs tcmalloc-thread | glibc | 43% vs jemalloc |
| pipeline, 2 threads, 4-256 KiB, one L3 | ns/alloc | tcmalloc-thread | 13% vs tcmalloc-percpu (tie) | tcmalloc-thread | 5% vs tcmalloc-percpu |
| pipeline, 2 threads, small, across L3s | ns/alloc | jemalloc | 29% vs tcmalloc-percpu | glibc | 45% vs jemalloc |
| pipeline, 4 threads, small | ns/alloc | jemalloc | 4% vs tcmalloc-percpu (tie) | glibc | 39% vs jemalloc |
| pipeline, 8 threads, small, both L3s | ns/alloc | jemalloc | 5% vs tcmalloc-percpu | glibc | 34% vs jemalloc |
| exchange, 2 threads, small, one L3 | ns/alloc | tcmalloc-percpu | 32% vs jemalloc | glibc | 26% vs jemalloc |
| exchange, 2 threads, mixed, one L3 | ns/alloc | tcmalloc-percpu | 15% vs jemalloc | glibc | 20% vs jemalloc |
| exchange, 2 threads, small, across L3s | ns/alloc | jemalloc | 3% vs tcmalloc-percpu (tie) | glibc | 40% vs jemalloc |
| exchange, 4 threads, small | ns/alloc | tcmalloc-percpu | 28% vs jemalloc | glibc | 31% vs jemalloc |
| exchange, 8 threads, small, both L3s | ns/alloc | jemalloc | 43% vs tcmalloc-percpu | glibc | 31% vs tcmalloc-percpu |
| 1 producer, 3 consumers | ns/alloc | jemalloc | 13% vs tcmalloc-percpu | glibc | 49% vs jemalloc |
| 3 producers, 1 consumer | ns/alloc | tcmalloc-percpu | 7% vs mimalloc | glibc | 59% vs jemalloc |
| 4 producers, 4 consumers | ns/alloc | jemalloc | 13% vs tcmalloc-percpu (tie) | glibc | 74% vs jemalloc |
| rayon stages, 4 threads | ns/alloc | tcmalloc-percpu | 16% vs jemalloc | glibc | 12% vs tcmalloc-percpu |
| rayon stages, 8 threads | ns/alloc | jemalloc | 24% vs tcmalloc-percpu | tcmalloc-percpu | 2% vs glibc |
| tokio spawn/join, 4 workers | ns/task | glibc | 6% vs tcmalloc-thread (tie) | glibc | 31% vs jemalloc |
| tokio spawn/join, 8 workers | ns/task | jemalloc | 4% vs tcmalloc-percpu | glibc | 41% vs jemalloc |
| RPC echo, concurrency 64 | server cycles/req | tcmalloc-percpu | 2% vs mimalloc-sized | glibc | 55% vs jemalloc |
| RPC log ingest, concurrency 64 | server cycles/req | mimalloc | 0% vs mimalloc-sized (tie) | glibc | 80% vs jemalloc |
| RPC log ingest, concurrency 256 | server cycles/req | mimalloc-sized | 1% vs mimalloc (tie) | glibc | 63% vs jemalloc |
| churn, 100 live | cycles/op | glibc | 8% vs tcmalloc-percpu | glibc | 29% vs jemalloc |
| churn, 1k live | cycles/op | jemalloc | 2% vs tcmalloc-percpu | glibc | 24% vs jemalloc |
| churn, 10k live | cycles/op | tcmalloc-percpu | 0.1% vs jemalloc (tie) | glibc | 19% vs jemalloc |
| churn, 100k live | cycles/op | jemalloc | 5% vs tcmalloc-percpu | jemalloc | 1% vs glibc |
| churn, 1M live | cycles/op | jemalloc | 8% vs tcmalloc-percpu | jemalloc | 4% vs tcmalloc-thread |
| churn, 10M live | cycles/op | jemalloc | 20% vs tcmalloc-percpu | jemalloc | 2% vs tcmalloc-thread |
| churn, 100k live, mixed sizes | cycles/op | tcmalloc-percpu | 3% vs mimalloc | tcmalloc-thread | 1% vs tcmalloc-percpu |
| BTreeMap build and drop, 1M | cycles/entry | mimalloc | 0.1% vs mimalloc-sized (tie) | jemalloc | 2% vs mimalloc |
| HashMap build and drop, 1M | cycles/entry | mimalloc | 2% vs mimalloc-sized (tie) | jemalloc | 14% vs glibc |
| Box tree, depth 20 | cycles/node | mimalloc | 1% vs mimalloc-sized (tie) | glibc | 5% vs jemalloc |
| Vec doubling to 256 MiB | cycles/element | glibc | 25% vs mimalloc-sized | glibc | 3% vs jemalloc |
| 64 Vecs doubling to 4 MiB | cycles/element | jemalloc | 78% vs glibc | mimalloc | 0% vs mimalloc-sized (tie) |
| serde_json into Value, 32 MiB | cycles/byte | mimalloc | 0.3% vs mimalloc-sized (tie) | jemalloc | 8% vs mimalloc |
| churn, 100k live (preload) | cycles/op | mimalloc | 19% vs jemalloc | glibc | 1% vs jemalloc (tie) |
| pipeline, 2 threads (preload) | ns/alloc | jemalloc | 7% vs tcmalloc-percpu | glibc | 31% vs jemalloc |
| serde_json (preload) | cycles/byte | mimalloc | 37% vs glibc | jemalloc | 8% vs mimalloc |
| larson, sized delete, 8 threads | Mops/s | mimalloc | 30% vs tcmalloc-percpu | tcmalloc-thread | 8% vs tcmalloc-percpu |
| larson, 8 threads | Mops/s | mimalloc | 23% vs jemalloc | tcmalloc-thread | 7% vs tcmalloc-percpu |
| xmalloc-test, 4+4 threads | Mfrees/s | tcmalloc-percpu | 44% vs mimalloc (tie) | tcmalloc-thread | 11% vs glibc |
| sh8bench, 16 threads | s | mimalloc | 97% vs jemalloc | tcmalloc-percpu | 2% vs tcmalloc-thread |
| mstress, 8 threads | s | mimalloc | 20% vs tcmalloc-percpu | jemalloc | 17% vs tcmalloc-percpu |
| cache-scratch, 1 thread | s | jemalloc | 0% vs mimalloc (tie) | glibc | 19% vs jemalloc |
| cache-scratch, 8 threads | s | mimalloc | 1% vs glibc (tie) | glibc | 19% vs jemalloc |
| cfrac | s | mimalloc | 2% vs glibc | glibc | 19% vs jemalloc |
| espresso | s | mimalloc | 3% vs jemalloc | glibc | 42% vs jemalloc |

Where the smallest is glibc on a job of a few MiB, the difference is each allocator's fixed
footprint (glibc about 3 MiB, jemalloc 4 to 5, mimalloc 5 to 7, tcmalloc 7), not how it
manages the workload's memory.

## 4. Cross-thread

Nanoseconds of wall time per allocation (each allocation is also freed, on another thread);
medians of 10.

| Job | glibc | mimalloc | jemalloc | tcmalloc-percpu | tcmalloc-thread |
|---|---:|---:|---:|---:|---:|
| pipeline 2, small, one L3 | 126 | 16.4 | 7.8 | 6.7 | 10.0 |
| pipeline 2, small, across L3s | 121 | 51.2 | 19.2 | 24.7 | 29.2 |
| pipeline 2, 4-256 KiB | 358 | 43.5 | 319 | 41.8 | 36.9 |
| pipeline 8, small | 48 | 12.1 | 4.4 | 4.6 | 8.1 |
| exchange 2, small, one L3 | 44 | 11.8 | 7.6 | 5.8 | 11.4 |
| exchange 2, small, across L3s | 131 | 40.7 | 18.1 | 18.6 | 33.3 |
| exchange 8, small | 27 | 8.8 | 4.0 | 5.8 | 8.0 |
| 1 producer, 3 consumers | 95 | 21.8 | 7.7 | 8.7 | 9.7 |
| 3 producers, 1 consumer | 77 | 7.5 | 10.7 | 7.0 | 11.6 |

- jemalloc and per-CPU tcmalloc share the lead; mimalloc takes 1.5 to 2.8 times as long as
  jemalloc and glibc 6 to 16 times. Crossing L3 domains makes the fast allocators 2.4 to 3.7
  times slower.
- Many producers into one consumer is the shape mimalloc handles: the consumer's frees go
  back to pages of three owners, and mimalloc ties tcmalloc while jemalloc falls behind.
- Large objects: jemalloc's thread cache stops at 32 KiB (`opt.tcache_max`), and larger
  objects freed on another thread go through its arenas, nearly as slowly as glibc.
- tcmalloc's per-thread mode costs 10 to 100% on these (it is faster only with large
  objects), and far more where threads only allocate or only free (section 7).

## 5. Work stealing and the RPC server

- **rayon** (parallel-iterator stages; 53 to 79% of frees on a different worker from the
  allocation): per-CPU tcmalloc leads at 4 threads (4.4 ns per allocation against jemalloc
  5.1, mimalloc 5.8, glibc 10.8). At 8 threads the spread of every allocator is ±80 to
  110%, so that row ranks nothing.
- **tokio spawn and join**: all allocators within 13% (glibc 107 ns per task, mimalloc 123).
  Tokio's scheduler keeps most work on the spawning worker (3 to 7% of frees cross), so the
  allocator barely matters here.
- **RPC server** (connect-rust, server on one L3, client on the other): the client is the
  bottleneck (the server used 0.9 to 2.4 of its 4 CPUs), so requests per second are equal
  and the ranking is the server's cycles per request:

| Job | glibc | mimalloc | jemalloc | tcmalloc-percpu | tcmalloc-thread |
|---|---:|---:|---:|---:|---:|
| echo, c64 | 48,788 | 46,833 | 46,459 | 45,605 | 47,446 |
| log ingest, c64 | 146,789 | 122,263 | 130,666 | 123,813 | 128,971 |
| log ingest, c256 | 148,671 | 119,525 | 128,004 | 121,834 | 126,151 |

  On the decode-heavy log service, mimalloc and per-CPU tcmalloc are 5 to 7% cheaper than
  jemalloc and 17 to 20% cheaper than glibc; mimalloc's resident set is 2 to 4 times the
  others'.

## 6. Single-threaded

Cycles per operation (user and kernel), medians of 10:

| Job | glibc | mimalloc | jemalloc | tcmalloc-percpu |
|---|---:|---:|---:|---:|
| churn, 100 live | 34.3 | 44.6 | 37.2 | 37.0 |
| churn, 10k live | 55.6 | 49.3 | 41.7 | 41.6 |
| churn, 1M live | 305 | 180 | 126 | 135 |
| churn, 10M live | 457 | 418 | 249 | 299 |
| Box tree, per node | 115 | 44.6 | 110 | 118 |
| BTreeMap, per entry | 1,339 | 839 | 972 | 1,039 |
| HashMap, per entry | 1,099 | 456 | 640 | 603 |
| serde_json, per byte | 22.8 | 16.8 | 24.0 | 28.6 |
| Vec to 256 MiB, per element | 9.0 | 11.3 | 16.8 | 13.6 |
| 64 Vecs to 4 MiB, per element | 18.5 | 36.7 | 10.4 | 38.0 |

- **Random frees over a live set** (churn): jemalloc is fastest, or tied, from 1k live
  objects up;
  mimalloc is 1.2 to 1.7 times slower, most so when the set outgrows the caches. Inference:
  mimalloc writes a free-list link into every freed object, a cache miss when the victim is
  cold, which is roofline design rule 3 at work.
- **Build then drop** (tree, maps, JSON): mimalloc is 1.2 to 2.5 times faster than the
  runner-up. It takes 200 to 350 page faults where the others take 17,000 to 186,000; its arenas are advised for
  transparent huge pages, so each fault maps 2 MiB. Inference: most of its lead here is the
  fault count, which roofline section 7 prices at 535 ns per 4 KiB fault against 119 ns per
  4 KiB of huge page.
- **Vec growth**: glibc wins one big vector (`mremap` moves pages instead of copying);
  jemalloc wins many medium ones (3.5 times faster than mimalloc and tcmalloc). tcmalloc
  moves the vector whenever it outgrows its size class and peaks at 687 MiB for a 256 MiB
  vector.
- **Sized free**: mimalloc-sized and mimalloc agree within noise everywhere; mimalloc v3
  ignores the size in release builds (`src/free.c:334`).

## 7. C programs through LD_PRELOAD

- mimalloc wins larson (both forms), sh8bench, mstress, cfrac and espresso. On sh8bench it is
  twice as fast as jemalloc and 7 times as fast as per-CPU tcmalloc.
- xmalloc-test (pure producers and pure consumers) is per-CPU tcmalloc's: 425 million frees
  per second against mimalloc 295, glibc 95, jemalloc 22 and per-thread tcmalloc 14.
- The same Rust programs preloaded show what the `mimalloc` crate costs: churn at 100k live
  objects takes 50.6 cycles per operation with mimalloc preloaded (Rust's `System` calls
  `malloc`) against 64.1 natively (`mi_malloc_aligned`). jemalloc (55 native, 60 preloaded)
  and tcmalloc (58, 66) gain from native sized frees, as expected.

## 8. Memory

Peak resident set, MiB, medians of 10, on the jobs that use more than a few MiB:

| Job | Live data | glibc | mimalloc | jemalloc | tcmalloc-percpu |
|---|---:|---:|---:|---:|---:|
| pipeline, 4-256 KiB | | 36 | 74 | 11 | 9 |
| churn, 100k live, mixed | 36 | 43 | 57 | 46 | 40 |
| churn, 10M live | 903 | 1,199 | 1,143 | 1,094 | 1,117 |
| HashMap, 1M | | 210 | 301 | 185 | 258 |
| Box tree, depth 20 | | 67 | 99 | 70 | 73 |
| Vec to 256 MiB | 256 | 259 | 527 | 267 | 687 |
| serde_json, 32 MiB | | 416 | 389 | 359 | 403 |
| RPC log ingest, c64 | | 6 | 26 | 11 | 13 |
| larson sized | | 84 | 94 | 81 | 52 |
| mstress | | 242 | 282 | 141 | 165 |

- jemalloc is smallest or within a few percent of it on most large jobs; mimalloc is the
  largest or second largest on most. Part of mimalloc's excess is transparent huge pages:
  22 of its 26 MiB on the RPC server, 64 MiB on larson, all 1,140 MiB at 10M churn.
- Returning memory: after the HashMap is dropped and the process idles for 500 ms,
  jemalloc holds 86 MiB, glibc 111, tcmalloc 192 and mimalloc 301, all it ever had.

## 9. What contradicts the design's assumptions

- **"mimalloc, the incumbent to beat on memory"** (design, Goals). On these workloads it is
  rarely the smallest and often the largest; jemalloc is the memory floor, and glibc is on
  small heaps. Matching mimalloc's memory is a weak goal; matching jemalloc's is the real one.
- **"mimalloc keeps up when objects flow one way within one L3"** (design, Background, from
  roofline section 10). Here it does not: 16.4 ns per allocation against jemalloc's 7.8 on a
  same-L3 two-thread pipeline with mixed small sizes, 13.4 against 7.0 with 64-byte objects.
  The roofline measured 32-byte objects and only the time each side spent working, not
  wall time; the difference between the two setups is not explained yet.
- **"on par with mimalloc on single-threaded ones"** (Goals) understates the target.
  jemalloc beats mimalloc on churn by up to 1.7 times, and mimalloc beats everyone on
  build-and-drop by up to 2.5 times through huge pages. asmalloc needs both: no writes into
  freed objects, and huge pages early enough to win the fault count.
- **tcmalloc's per-CPU front end is as strong as the design expects** on cross-thread work,
  but its middle and back end are not: it is slowest on build-and-drop and on sh8bench, and
  its `realloc` growth keeps old and new copies resident.

## 10. Caveats

- A shared, busy machine (section 2). Single-threaded and RPC numbers are tight (controls
  within 2%); several multithreaded rows are not, and rayon with 8 threads ranks nothing.
- Wall-time rankings of multithreaded jobs depend on frequency, which boost and the other
  load changed; the per-allocation cycle counts are in the detail tables.
- jemalloc in native runs is Fedora's shared library, not `tikv-jemallocator`'s static
  build, which is what Rust services use and what the suite now measures. The two differ by
  more than expected on one-way cross-thread jobs; see section 11.
- tcmalloc is built with bazel's `-c opt` (`-O2`, frame pointers kept); mimalloc with `-O3`.
- Peak memory on small jobs is mostly fixed footprint.
- The RPC client is the bottleneck, so the RPC jobs rank server CPU per request, not
  throughput or latency.
- One machine, one kernel; the huge-page results depend on THP being in `madvise` mode.

## 11. Changes to the suite after this session

Measured 2026-10-17, 15:15 to 15:45, on the same machine, again shared with other work
(one or two CPUs' worth of load outside the measurement CPUs; single-threaded jobs on CPU 2,
which a teammate may also have used). Quick suite (`benchrun run --suite quick`).

**Native jemalloc is now `tikv-jemallocator` itself.** The `je` feature makes it the global
allocator unchanged, with default features, so the build is what a service depending on it
gets: tikv-jemalloc-sys 0.6.1 (jemalloc `5.3.0-1-ge13ca993`), static, symbols prefixed
`_rjem_` (C code keeps glibc's malloc), `--disable-stats`, no profiling, background-thread
support compiled in but off, compiled with cc's flags for the release profile (`-O3`, frame
pointers kept). Fedora's library (`5.3.0-14.fc44`) is `-O2` with the distribution's
hardening flags, profiling and statistics compiled in, shared, and replaces malloc for the
whole process. It stays the jemalloc used for LD_PRELOAD runs. Run-time options are
identical apart from profiling, which is off in both.

Five interleaved rounds, tikv build against Fedora's library called the same way (median
cost, tikv relative to Fedora; spreads under 1.5%):

| Job | tikv vs Fedora |
|---|---:|
| pipeline, 2 threads, small, one L3 | +181% (27.8 against 9.9 ns/alloc) |
| 3 producers, 1 consumer | +104% (27.0 against 13.2 ns/alloc) |
| exchange, 2 threads, across L3s | −2.1% |
| rayon stages, 4 threads | −4.3% |
| tokio spawn, RPC log ingest | 0.1% |
| churn 1k, churn 1M, BTreeMap, Vec growth, JSON | −1.0% to −4.0% |

The two large differences are one effect. A thread whose first call into jemalloc is a free
gets only minimal thread state, and with it no thread cache, until it allocates; each of its
frees then locks an arena bin (`arena_dalloc_small`, with mutex lock and unlock about 20% of
the profile). The pipeline's last stage and the mpmc consumers only free. With Fedora's
library, glibc and std allocate on every new thread through jemalloc, so those threads have a
cache; with the prefixed build those allocations go to glibc. Allocating one object on each
thread before the work makes the tikv build 7.4 ns/alloc on the pipeline and 10.0 on mpmc,
slightly faster than Fedora's (7.8 and 10.3). So, on the jobs where every thread
allocates, the tikv build is 1 to 4% faster than the numbers in sections 3 to 8; on jobs
with free-only threads it is two to three times slower. Whether free-only threads are a
fair shape is a choice the suite has to make (a thread that only drops what it receives does
occur in real services, but most also allocate).

**Every run is under a memory cap.** `benchrun run` runs in a systemd scope of its own and
each measured process in a capped cgroup of it (`bench/README.md`, Memory cap). To check
that the cap does not change what is measured, four pairs of quick-suite sessions with
glibc, mimalloc, jemalloc and per-CPU tcmalloc, alternating the previous runner (no cap) and
the new one (cap), the same allocator order within a pair: the median paired cost ratio,
capped over uncapped, is −0.85% (95% interval −1.30% to −0.39%, 224 pairs), against −1.18%
between consecutive uncapped sessions (the drift from one session to the next). The
tightest job, churn with 1k live objects, differs by −0.07% (−0.37% to +0.30%). The cap
costs nothing measurable. Memory-bound single-threaded jobs drifted by up to three times over
the half hour in both arms alike (jemalloc churn 1M from 372 to 124 cycles/op) with no load on
their own CPU or its sibling; interleaving cancels such drift, but single sessions on a
shared machine cannot be trusted for these jobs.

**tcmalloc's shared library now uses initial-exec TLS.** Built `-fPIC` by this GCC, whose
default is `-mtls-dialect=gnu2`, it reached its thread-locals (`tcmalloc_sampler`,
`__rseq_abi`) through TLS descriptor calls, two per `malloc`, an indirect call through memory
that on Zen 5 defeats zero-latency store forwarding (`zen5-roofline.md` section 14). Preloaded
allocators are normally built with initial-exec TLS, as mimalloc's and jemalloc's shared
builds are; `build.sh` now passes `--copt=-ftls-model=initial-exec`, the library has no
`R_X86_64_TLSDESC` relocations left (10 before), and `malloc`, `free` and the aligned `new`
read their thread-locals with plain `%fs:` loads. The static archive was unaffected (a static
link relaxes either model). Measured 2026-10-17 on the shared machine, preloaded, per-CPU
mode, five interleaved runs, medians, before and after:

| Job | TLS descriptors | Initial-exec | Change |
|---|---:|---:|---:|
| churn, 1k live (cycles/op) | 45.0 | 37.4 | −17% |
| churn, 100k live (cycles/op) | 68.1 | 58.7 | −14% |
| xmalloc-test 4+4, short (Mfrees/s) | 405 | 511 | +26% |
| larson-sized 8, short (Mops/s) | 308 | 369 | +20%, noisy |

Every tcmalloc preload figure in sections 3 to 8 was measured with the descriptor build and
understates tcmalloc by about this much.
