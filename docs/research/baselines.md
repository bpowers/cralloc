# Allocator baselines on workloads

Measured 2026-10-18 on a quiet machine, at commit `a92d51a`, with the suite in `bench/`
(commands and method in `bench/README.md`): asmalloc against glibc, mimalloc (the newest
release called as the `mimalloc` crate calls it, the same with the size passed back to free,
and the `mimalloc` crate as shipped), jemalloc (`tikv-jemallocator`) and tcmalloc (per-CPU
and per-thread), on every workload of the full suite, ten rounds. This replaces the session
at `b5d6e8f` earlier the same day; section 4 says what moved between the two and why.

| Session | Raw results | Per-workload tables (every allocator, spread, memory, faults) |
|---|---|---|
| `a92d51a`, 2026-10-18, this note | `docs/research/data/baselines-2026-10-18-a92d51a.jsonl.gz`; `bench/results/full-2026-10-18-a92d51a.jsonl` in the main checkout | `docs/research/data/baselines-2026-10-18-a92d51a-detail.md` |
| `b5d6e8f`, 2026-10-18, the previous report | `docs/research/data/baselines-2026-10-18.jsonl.gz`; `bench/results/full-2026-10-18-b5d6e8f.jsonl` | `docs/research/data/baselines-2026-10-18-detail.md` |
| `60ce673`, 2026-10-17, busy machine, no asmalloc | `docs/research/data/baselines-2026-10-17.jsonl.gz` | `docs/research/data/baselines-2026-10-17-detail.md` |

`benchrun report` reads the raw results after `gunzip`.

## 1. Summary

Time, asmalloc against the best incumbent on each job (a gap is the median of per-round
ratios; "level" means the 95% interval includes no difference). asmalloc is ahead on 33 of
the 56 jobs, level on 4 and behind on 19; at `b5d6e8f` it was 28, 4 and 23 of 55.

- **Cross-thread, small objects:** ahead on every job, 4% (eight threads in a pipeline) to
  51%, and 36 to 80% ahead of mimalloc. Independent pipelines on different L3 domains, 16 to
  35% behind jemalloc at `b5d6e8f`, are now 33 to 43% ahead.
- **Single-threaded churn, 100 to 10M live objects:** ahead 7 to 33%.
- **Box tree:** 3% ahead of mimalloc (116% behind at `b5d6e8f`). **One Vec grown to 256
  MiB:** 45% ahead (the job was redefined; section 4).
- **Work stealing:** rayon 22% ahead at four threads, level at eight.

Where asmalloc is behind (section 9 has the causes):

- **Large objects (4 to 256 KiB) crossing threads:** 62 ns/alloc against per-thread
  tcmalloc's 35 (76% behind) and mimalloc's 42 (49%); jemalloc takes 305. It was 297 ns.
- **One 64 KiB and one 1 MiB block allocated and freed (`largepair`, new):** glibc takes 8.1
  ns a pair, far ahead of everyone, because it serves both blocks from the top of its heap.
  asmalloc's 67 ns is fourth of eight: 9 and 14% behind the two tcmallocs, 25% ahead of
  mimalloc and 50% ahead of jemalloc.
- **64 Vecs grown to 4 MiB together:** 86% behind mimalloc (5.9 against 3.1 cycles/element),
  level with tcmalloc, 22% ahead of jemalloc.
- **Build and drop of maps:** HashMap 23% behind the mimalloc crate (16% behind mimalloc),
  BTreeMap 8%. JSON is level with mimalloc (1% behind the crate, 2% ahead of mimalloc); 5%
  behind preloaded.
- **tokio spawn:** 33% behind glibc at four threads (level with mimalloc), 12% behind jemalloc
  at eight (3% behind mimalloc). The time is false sharing in tokio's own structures, not
  work in the allocator, and glibc's lead is luck of placement (section 15).
- **RPC server:** 2 to 4% behind on server cycles per request; level to 1% against mimalloc.
- **C programs, preloaded:** sh8bench 2.2 times mimalloc (0.60 against 0.27 s) and 19%
  behind jemalloc; larson 16 to 25% behind mimalloc, a gap accepted for now (C++ programs
  reach the allocator through libstdc++, and exporting the C++ operators is deferred);
  mstress 6%, espresso 4%, cfrac 1%; cache-scratch level. xmalloc-test is bimodal (section 7).

Memory (cgroup peak):

- **Against jemalloc, the bar:** at or under it on every job of 10 MiB or more but two:
  HashMap, 12% over (204 against 182 MiB), because the tables a map outgrows now stay
  resident for its next build; and churn with 100k live objects, 1 to 2% (0.1 to 0.3 MiB,
  native and preloaded).
- **Small heaps (under 12 MiB):** 0.25 to 3.3 MiB over glibc where glibc is the smallest.
  Against jemalloc, from 1.5 MiB over (mpmc, four producers and four consumers) to 1.8 MiB
  under (exchange, eight threads) and 5.8 under (the large-object pipeline).
- **Against the smallest incumbent** the largest gap on a big heap is larson, 32 to 35% over
  per-thread tcmalloc (72 against 54 MiB, and 19% under jemalloc). It was 60 MiB at
  `b5d6e8f`; not attributed.
- **After the peak** (measured at `b5d6e8f`, section 8): the lowest of the three measured.

## 2. Machine, versions and method

AMD Ryzen 9 9950X (Zen 5), 16 cores with SMT, two L3 domains (CPUs 0-7 with siblings 16-23,
8-15 with 24-31), 60 GiB, Linux 7.2.9-201.fc44, glibc 2.43, rustc 1.99.1, GCC 16.2.1.
Governor `powersave`, energy preference `balance_performance`, boost on, transparent huge
pages `madvise` (defrag `madvise`), NMI watchdog on. The same as at `b5d6e8f`, with the same
baseline manifests.

| Allocator | Version and build | Native (Rust `#[global_allocator]`) | Preload |
|---|---|---|---|
| asmalloc | `a92d51a` | `asmalloc::Asmalloc`, `executable` feature, fat LTO | `asmalloc-preload` (C names only) |
| glibc | 2.43 | `std::alloc::System` | none |
| mimalloc | v3.5.3 (`31d034d9`), `src/static.c` at `-O3` as libmimalloc-sys builds it | as the `mimalloc` crate calls it | cmake Release shared library |
| mimalloc-sized | the same library | the same, freeing with `mi_free_size_aligned` (mimalloc v3 ignores the size in release builds, so only the call path differs) | not run |
| mimalloc-crate | the `mimalloc` crate 0.1.52 as shipped (bundles 3.3.2) | itself | not run |
| jemalloc | `tikv-jemallocator` 0.6.1 (jemalloc 5.3.0-1-ge13ca993, static, prefixed, no stats or profiling) | itself | Fedora 5.3.0-14 shared library |
| tcmalloc-percpu | google/tcmalloc `f3c6669a`, bazel 8.8.0 `-c opt`, initial-exec TLS | as `tcmalloc-better` 0.1.19 calls it | shared library; both with `GLIBC_TUNABLES=glibc.pthread.rseq=0` |
| tcmalloc-thread | the same | the same | the same, without the tunable |
| mimalloc-ctl | mimalloc built a second time: the control | | |

`bench/baselines/build.sh` builds the C libraries and records versions and flags in the
manifests, which every result file carries. mimalloc-sized was not in the `b5d6e8f` session.

**Method.** One fresh process per run; each round runs every job under every allocator in a
new random order. Ten rounds, 56 jobs, 4,800 runs, none invalid, over 95 minutes. Threads are
pinned one per physical core on CPUs 0-3 and 8-11 (single-threaded jobs on CPU 2), the
process confined to its CPUs before `execve`, and every run under a per-run memory cap (4 GiB)
inside a capped session scope; no run reached its cap. Every run reported the allocator,
mode, library and version it was meant to have, and no run's counters were multiplexed.
Single-threaded jobs rank by cycles per operation (user and kernel), multithreaded ones by
wall time per allocation, C programs by their own throughput or wall time; memory ranks by
each run's cgroup peak. Cross-thread consumers check every object's header, so a block handed
out twice fails the run.

**Quiet.** No other work ran during the session: the quiet gate (nothing on the measurement
CPUs or their siblings, at most one CPU's worth elsewhere) passed before every job without
waiting, its one-second look seeing at most 0.09 CPUs' worth on the measurement CPUs and 0.06
elsewhere, and no run was left out as busy.

**Control.** mimalloc-ctl against mimalloc, paired by round: the largest median difference
is 6.4% (the four-thread pipeline, interval +2.4% to +16.1%); 7 of 56 intervals exclude zero
where chance predicts 2.8. Of the other six, larson and larson-sized differ by +4.9% and
-4.8%, opposite ways, and the rest (the RPC log jobs, eight-thread spawn, `largepair`) by
under 2%. Single-threaded differences of 1% and cross-thread differences of a few percent are
resolved.

**Code layout.** The control does not catch a job whose time depends on where its code
landed, because every allocator's build places the workload's code somewhere else. When a job
moves after a change that should not have touched it, `benchrun layout` (`bench/README.md`)
runs it under builds that place the same code at other addresses; section 14 has the method
and the jobs that are sensitive.

## 3. asmalloc against the best incumbent, per workload

The best incumbent is the one with the lowest median cost; the gap is the median of
per-round ratios of asmalloc to it, with "level" where the 95% interval includes no
difference. "vs mimalloc" is the same against mimalloc (v3.5.3; the shared library when
preloaded). Memory compares cgroup peaks with the smallest incumbent the same way, and with
jemalloc.

| Workload | Metric | asmalloc | Best incumbent | Time | vs mimalloc | Cgroup peak MiB, asm / smallest | Memory | vs jemalloc |
|---|---|---:|---|---|---|---|---|---|
| pipeline t2 sizes=s64 local all-alloc (native) | ns/alloc | 3.741 | jemalloc 6.282 | ahead 40% | ahead 72% | 1.2 / glibc 1.0 | behind 25% | ahead 13% |
| pipeline t2 sizes=small local all-alloc (native) | ns/alloc | 3.745 | tcmalloc-percpu 6.708 | ahead 44% | ahead 76% | 2.2 / glibc 1.2 | behind 80% | behind 14% |
| pipeline t2 sizes=mixed local all-alloc (native) | ns/alloc | 11.29 | tcmalloc-percpu 12.53 | ahead 10% | ahead 44% | 5.0 / tcmalloc-thread 3.1 | behind 58% | ahead 5% |
| pipeline t2 sizes=small local free-only (native) | ns/alloc | 3.755 | tcmalloc-percpu 6.673 | ahead 44% | ahead 75% | 2.0 / glibc 1.1 | behind 80% | behind 26% |
| pipeline t2 sizes=large objects=1M inflight=512 local all-alloc (native) | ns/alloc | 62.21 | tcmalloc-thread 35.49 | behind 76% | behind 49% | 2.8 / tcmalloc-thread 3.7 | ahead 25% | ahead 68% |
| pipeline t2 sizes=small split all-alloc (native) | ns/alloc | 11.54 | jemalloc 18.49 | ahead 38% | ahead 77% | 2.6 / glibc 1.0 | behind 165% | behind 29% |
| pipeline t2 sizes=small split free-only (native) | ns/alloc | 12.54 | tcmalloc-percpu 24.59 | ahead 51% | ahead 77% | 2.6 / glibc 1.0 | behind 157% | behind 47% |
| pipeline t4 sizes=small local all-alloc (native) | ns/alloc | 3.172 | jemalloc 3.584 | ahead 12% | ahead 68% | 4.0 / glibc 2.8 | behind 42% | behind 10% |
| pipeline t8 sizes=small both all-alloc (native) | ns/alloc | 4.232 | jemalloc 4.383 | ahead 4% | ahead 63% | 7.7 / glibc 6.0 | behind 29% | behind 12% |
| pipeline t2 x2 sizes=small local all-alloc (native) | ns/alloc per chain | 4.134 | jemalloc 7.865 | ahead 47% | ahead 76% | 3.4 / glibc 2.0 | behind 69% | behind 5% |
| pipeline t2 x2 sizes=small split all-alloc (native) | ns/alloc per chain | 4.633 | jemalloc 8.037 | ahead 43% | ahead 73% | 3.4 / glibc 2.0 | behind 70% | behind 4% |
| pipeline t2 x3 sizes=small both all-alloc (native) | ns/alloc per chain | 5.313 | jemalloc 8.121 | ahead 34% | ahead 70% | 4.8 / glibc 3.0 | behind 57% | level (+1%) |
| pipeline t2 x4 sizes=small both all-alloc (native) | ns/alloc per chain | 5.498 | jemalloc 8.169 | ahead 33% | ahead 69% | 5.9 / glibc 4.0 | behind 50% | ahead 2% |
| exchange t2 sizes=small local (native) | ns/alloc | 4.438 | tcmalloc-percpu 6.912 | ahead 37% | ahead 65% | 2.2 / glibc 1.8 | behind 26% | ahead 7% |
| exchange t2 sizes=mixed local (native) | ns/alloc | 8.150 | tcmalloc-percpu 8.809 | ahead 7% | ahead 47% | 4.8 / tcmalloc-percpu 4.0 | behind 19% | ahead 22% |
| exchange t2 sizes=small split (native) | ns/alloc | 14.05 | jemalloc 18.63 | ahead 26% | ahead 65% | 2.9 / glibc 1.5 | behind 94% | behind 22% |
| exchange t4 sizes=small local (native) | ns/alloc | 2.350 | tcmalloc-percpu 3.715 | ahead 38% | ahead 62% | 3.7 / glibc 3.3 | behind 14% | ahead 13% |
| exchange t8 sizes=small both (native) | ns/alloc | 2.996 | jemalloc 3.969 | ahead 27% | ahead 60% | 6.8 / tcmalloc-percpu 5.2 | behind 29% | ahead 21% |
| mpmc p1 c3 sizes=small local all-alloc (native) | ns/alloc | 5.237 | jemalloc 7.583 | ahead 30% | ahead 80% | 3.1 / glibc 1.2 | behind 142% | behind 36% |
| mpmc p1 c3 sizes=small local free-only (native) | ns/alloc | 5.361 | tcmalloc-percpu 8.679 | ahead 37% | ahead 74% | 3.0 / glibc 1.2 | behind 140% | behind 50% |
| mpmc p3 c1 sizes=small local all-alloc (native) | ns/alloc | 4.842 | mimalloc 7.704 | ahead 37% | ahead 37% | 3.5 / glibc 2.0 | behind 68% | behind 7% |
| mpmc p3 c1 sizes=small local free-only (native) | ns/alloc | 4.876 | tcmalloc-percpu 7.152 | ahead 32% | ahead 36% | 3.5 / glibc 2.0 | behind 71% | behind 19% |
| mpmc p4 c4 sizes=small both all-alloc (native) | ns/alloc | 4.991 | jemalloc 8.579 | ahead 41% | ahead 76% | 6.0 / glibc 2.7 | behind 122% | behind 32% |
| forkjoin t4 local (native) | ns/alloc | 3.453 | tcmalloc-percpu 4.416 | ahead 22% | ahead 41% | 15.6 / tcmalloc-percpu 16.4 | ahead 5% | ahead 46% |
| spawn t4 local (native) | ns/task | 130.9 | glibc 99.11 | behind 33% | level (+9%) | 2.5 / glibc 1.7 | behind 49% | level (+1%) |
| forkjoin t8 both (native) | ns/alloc | 3.818 | jemalloc 3.947 | level (-4%) | ahead 50% | 17.8 / tcmalloc-percpu 17.8 | level (+1%) | ahead 25% |
| spawn t8 both (native) | ns/task | 254.7 | jemalloc 226.6 | behind 12% | behind 3% | 4.6 / glibc 3.1 | behind 53% | level (+1%) |
| rpc echo c64 (native) | server cycles/req | 46816 | tcmalloc-percpu 45614 | behind 3% | level (+0%) | 5.6 / glibc 3.3 | behind 71% | ahead 3% |
| rpc log c64 (native) | server cycles/req | 125168 | mimalloc-crate 120967 | behind 4% | behind 1% | 8.3 / glibc 5.3 | behind 57% | ahead 14% |
| rpc log c256 (native) | server cycles/req | 121083 | mimalloc-crate 118012 | behind 2% | level (+0%) | 11.7 / glibc 9.7 | behind 23% | ahead 20% |
| churn live=100 small (native) | cycles/op | 22.67 | jemalloc 33.86 | ahead 33% | ahead 49% | 0.8 / glibc 0.5 | behind 60% | behind 12% |
| churn live=1k small (native) | cycles/op | 23.97 | jemalloc 35.16 | ahead 32% | ahead 46% | 0.9 / glibc 0.6 | behind 45% | behind 11% |
| churn live=10k small (native) | cycles/op | 31.48 | jemalloc 39.65 | ahead 21% | ahead 36% | 2.1 / glibc 1.8 | behind 21% | behind 13% |
| churn live=100k small (native) | cycles/op | 46.51 | jemalloc 53.15 | ahead 12% | ahead 28% | 12.1 / jemalloc 11.8 | behind 2% | behind 2% |
| churn live=1M small (native) | cycles/op | 104.3 | jemalloc 119.5 | ahead 13% | ahead 42% | 109.2 / jemalloc 110.3 | ahead 1% | ahead 1% |
| churn live=10M small (native) | cycles/op | 214.1 | jemalloc 247.0 | ahead 13% | ahead 49% | 1069.7 / jemalloc 1091.4 | ahead 2% | ahead 2% |
| churn live=100k mixed (native) | cycles/op | 87.47 | tcmalloc-percpu 93.66 | ahead 7% | ahead 10% | 34.4 / tcmalloc-thread 34.5 | ahead <1% | ahead 19% |
| map btree 1M (native) | cycles/entry | 884.6 | mimalloc-crate 817.6 | behind 8% | behind 8% | 123.5 / jemalloc 125.2 | ahead 1% | ahead 1% |
| map hash 1M (native) | cycles/entry | 533.8 | mimalloc-crate 434.4 | behind 23% | behind 16% | 204.3 / jemalloc 181.7 | behind 12% | behind 12% |
| tree depth=20 (native) | cycles/node | 43.25 | mimalloc-crate 44.31 | ahead 3% | ahead 3% | 51.2 / glibc 64.5 | ahead 21% | ahead 24% |
| vecgrow 1x256Mi (native) | cycles/element | 2.203 | mimalloc-crate 4.150 | ahead 45% | ahead 77% | 258.4 / glibc 257.0 | behind 1% | ahead 2% |
| vecgrow 64x4Mi (native) | cycles/element | 5.891 | mimalloc-sized 3.055 | behind 86% | behind 77% | 277.4 / mimalloc-sized 265.0 | behind 5% | ahead 10% |
| json 32Mi (native) | cycles/byte | 16.19 | mimalloc-crate 16.13 | behind 1% | ahead 2% | 352.5 / mimalloc-crate 355.0 | ahead 1% | ahead 3% |
| largepair 64Ki+1Mi (native) | ns/pair | 67.46 | glibc 8.087 | behind 735% | ahead 25% | 0.8 / glibc 0.5 | behind 66% | behind 15% |
| churn live=100k small (preload) | cycles/op | 51.52 | mimalloc 50.87 | behind 1% | behind 1% | 12.1 / jemalloc 12.0 | behind 1% | behind 1% |
| pipeline t2 sizes=small local all-alloc (preload) | ns/alloc | 3.985 | tcmalloc-percpu 6.823 | ahead 42% | ahead 72% | 2.0 / glibc 1.2 | behind 60% | level (-0%) |
| json 32Mi (preload) | cycles/byte | 17.12 | mimalloc 16.30 | behind 5% | behind 5% | 353.3 / jemalloc 360.5 | ahead 2% | ahead 2% |
| larson-sized 8 (preload) | Mops/s | 426.9 | mimalloc 541.1 | behind 25% | behind 25% | 71.5 / tcmalloc-thread 54.3 | behind 32% | ahead 19% |
| larson 8 (preload) | Mops/s | 433.6 | mimalloc 500.8 | behind 16% | behind 16% | 72.2 / tcmalloc-thread 53.6 | behind 35% | ahead 19% |
| xmalloc-test 4+4 (preload) | Mfrees/s | 396.5 | tcmalloc-percpu 549.5 | bimodal (section 7) | ahead 25% | 4.4 / tcmalloc-thread 34.0 | bimodal | ahead 90% |
| sh8bench 16 (preload) | s | 0.5966 | mimalloc 0.2712 | behind 124% | behind 124% | 120.1 / tcmalloc-percpu 119.7 | level (+0%) | ahead 26% |
| mstress 8 (preload) | s | 0.2625 | mimalloc 0.2474 | behind 6% | behind 6% | 139.5 / jemalloc 139.1 | level (-1%) | level (-1%) |
| cache-scratch 1 (preload) | s | 0.5186 | mimalloc 0.5062 | level (+2%) | level (+2%) | 1.0 / glibc 0.8 | level (+16%) | ahead 20% |
| cache-scratch 8 (preload) | s | 0.0674 | mimalloc 0.0672 | level (+0%) | level (+0%) | 2.8 / glibc 2.5 | behind 10% | level (+0%) |
| cfrac (preload) | s | 2.107 | mimalloc 2.080 | behind 1% | behind 1% | 1.6 / jemalloc 1.4 | behind 17% | behind 17% |
| espresso (preload) | s | 2.316 | mimalloc 2.226 | behind 4% | behind 4% | 2.6 / glibc 1.3 | behind 92% | behind 51% |

## 4. What moved since `b5d6e8f`, and why

The incumbents held still between the two sessions: each one's median change over the jobs
the sessions share is under 1%. The jobs below moved by far more, and only for asmalloc,
except where said. The workloads were rebuilt between the sessions, and some jobs' times
depend on where their code landed (section 14), so moves of a few percent on JSON and the
tree are not resolved; none of those below is that small. Causes come from the notes that
measured each change, before and after, on its own.

| Job | `b5d6e8f` | `a92d51a` | Against the best, then and now | What moved it |
|---|---:|---:|---|---|
| Large objects crossing threads, ns/alloc | 296.6 | 62.21 | behind 721%, behind 76% | Frees no longer purge: the retained cap is charged to growth (`e618a42`, `c2b8a31`; 319 to 119 ns, `madvise` calls 298k to 53). Then shared words on cache lines of their own (`f5647f2`) and a large allocation and free in half the instructions (`fd8e607`): 99.9 to 69.0 ns. `memory.md`, "The cap charged to growth" and "Lines between CPUs, and the cost of a large block". |
| Independent pipelines across L3s (x2 split, x3, x4), ns/alloc per chain | 10.81, 9.379, 10.16 | 4.633, 5.313, 5.498 | behind 35, 16, 24%; ahead 43, 34, 33% | A depot per cache domain (`e4dcd16`, `f023136`), then a capped stack per domain and a shared stack per class (`d37faa7`, `1dbe734`): magazines stay in the L3 that freed them. x2 split 11.16 to 4.78 ns. `launder-and-fill.md`, "A depot per cache domain". |
| Box tree depth 20, cycles/node | 99.26 | 43.25 | behind 116%, ahead 3% | Magazines hand out objects lowest address first (`94c5784`; the walk 531 to 211 M cycles), and are filled and laundered whole, in place (`4b0af00`; 61.7 to 44.3 cycles/node). `launder-and-fill.md`, "Ascending magazines, and whole magazines in place". The job's own code also changed (build, walk and drop now timed apart, the drop out of line): glibc moved 113 to 86 with it and per-thread tcmalloc 162 to 144, the other incumbents under 5%. |
| JSON 32 MiB, cycles/byte, native; preloaded | 23.62; 23.90 | 16.19; 17.12 | behind 45, 44%; behind 1, 5% | The cap charged to growth (81k `madvise` calls to 62, kernel share 15% to 3%) and whole-magazine launders and fills. |
| BTreeMap 1M, cycles/entry | 1089 | 884.6 | behind 32%, 8% | Whole-magazine launders and fills (1138 to 995), scattered launders through a window and a map (`68eb74d`) and then a direct path (`1b49725`; drop instructions per object 165 to 112), and the cap charged to growth (faults 38k to 25k). |
| HashMap 1M, cycles/entry; cgroup peak | 690.6; 175.1 MiB | 533.8; 204.3 MiB | behind 60%, 23%; memory ahead 5%, behind 12% of jemalloc | The cap charged to growth: 671 to 579 cycles/entry, faults 190k to 61k, and 29 MiB more peak, since the tables a map outgrows now stay resident for its next build instead of being purged at their free. |
| mstress 8, s; cgroup peak | 0.394; 128.1 MiB | 0.263; 139.5 MiB | behind 57%, 6%; memory ahead 9%, level with jemalloc | The cap charged to growth (0.396 to 0.270 s, faults 684k to 36k, peak 128.7 to 139.9 MiB). |
| sh8bench 16, s | 1.190 | 0.597 | behind 364%, 124% | Fillers of one class stopped fighting over the same summary words (`4f6039c`; 1.163 to 0.782 s), and the depot per cache domain (0.720 to 0.606 s). `launder-and-fill.md`. |
| larson, larson-sized, Mops/s; cgroup peak | 402.6, 404.2; 60 MiB | 433.6, 426.9; 72 MiB | behind 29, 30%; behind 16, 25% | Speed: the depot per cache domain and its domain stacks measured +2% and +1 to 3% across two L3s (`launder-and-fill.md`); the rest not attributed. The 12 MiB more peak: not attributed. |
| Smallest heaps (churn 100 and 1k live), cgroup peak MiB | 0.94, 1.05 | 0.80, 0.91 | | Region headers moved out of the metadata blocks (`38ac817`) and one spare magazine per stack (`a790dfe`). `memory.md`, "Small heaps". |
| Pipelines and exchange split across L3s, cgroup peak MiB | 2.2 to 2.4 | 2.6 to 2.9 | | The depot per cache domain: a domain's frees wait in its own stack before the other domain takes them, and meanwhile the other carves (`launder-and-fill.md`, "Parked memory"). |
| Split pipeline, free-only consumer, ns/alloc | 11.05 | 12.54 | ahead 52%, ahead 51% | 14% slower. The depot per cache domain measured +2%; the rest not attributed. |
| tokio spawn, four threads, ns/task | 125.6 | 130.9 | behind 18%, 33% | Not attributed. asmalloc is 4% slower and glibc 6% faster (105.9 to 99.1); asmalloc's runs fall in two groups, 129 to 131 and 149 to 166 ns. |

**Redefined, not compared.** The `vecgrow` jobs fill each vector by `memset` steps instead of
one push per element (`f0690eb`; section 14), so their numbers before and after the change
are different jobs. Under the new definition, sixteen times the room for a growing block
(`c43b949`) made one Vec to 256 MiB 34% faster (`memory.md`, "Headroom for growing blocks").
`largepair 64Ki+1Mi` is new in the full suite.

**xmalloc-test is bimodal**, as `launder-and-fill.md` found for every library on two L3s, so
its medians do not compare. asmalloc's slow runs went from 260-346 to 374-401 Mfrees/s and
its fast runs from 650-664 to 739-750; the depot per cache domain measured the slow runs at
377-390 before and 399-441 after.

## 5. Multithreaded

Medians of ten.

Cross-thread, time:

| Job | Metric | asmalloc | glibc | mimalloc | mi sized | mi crate | jemalloc | tc per-CPU | tc per-thread |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| pipeline t2 sizes=s64 local all-alloc | ns/alloc | 3.741 | 92.23 | 13.69 | 14.16 | 15.89 | 6.282 | 9.734 | 10.19 |
| pipeline t2 sizes=small local all-alloc | ns/alloc | 3.745 | 126.3 | 15.52 | 15.01 | 17.85 | 7.712 | 6.708 | 10.12 |
| pipeline t2 sizes=mixed local all-alloc | ns/alloc | 11.29 | 134.8 | 20.05 | 20.30 | 21.61 | 17.00 | 12.53 | 16.63 |
| pipeline t2 sizes=small local free-only | ns/alloc | 3.755 | 127.5 | 15.01 | 15.04 | 18.86 | 22.31 | 6.673 | 10.09 |
| pipeline t2 sizes=large objects=1M inflight=512 local all-alloc | ns/alloc | 62.21 | 355.8 | 41.74 | 41.91 | 40.27 | 305.0 | 40.67 | 35.49 |
| pipeline t2 sizes=small split all-alloc | ns/alloc | 11.54 | 129.3 | 51.05 | 50.59 | 67.61 | 18.49 | 24.24 | 30.11 |
| pipeline t2 sizes=small split free-only | ns/alloc | 12.54 | 127.5 | 53.01 | 50.73 | 64.58 | 50.63 | 24.59 | 29.82 |
| pipeline t4 sizes=small local all-alloc | ns/alloc | 3.172 | 33.26 | 10.05 | 10.12 | 10.01 | 3.584 | 4.134 | 6.859 |
| pipeline t8 sizes=small both all-alloc | ns/alloc | 4.232 | 49.10 | 11.27 | 11.92 | 12.78 | 4.383 | 5.915 | 7.849 |
| pipeline t2 x2 sizes=small local all-alloc | ns/alloc per chain | 4.134 | 134.2 | 16.95 | 18.05 | 18.54 | 7.865 | 7.903 | 12.55 |
| pipeline t2 x2 sizes=small split all-alloc | ns/alloc per chain | 4.633 | 138.5 | 17.05 | 17.32 | 20.77 | 8.037 | 22.49 | 29.44 |
| pipeline t2 x3 sizes=small both all-alloc | ns/alloc per chain | 5.313 | 141.7 | 17.60 | 17.29 | 20.77 | 8.121 | 20.84 | 29.26 |
| pipeline t2 x4 sizes=small both all-alloc | ns/alloc per chain | 5.498 | 143.8 | 17.72 | 17.60 | 21.23 | 8.169 | 24.11 | 33.94 |
| exchange t2 sizes=small local | ns/alloc | 4.438 | 45.93 | 12.10 | 12.31 | 13.01 | 7.591 | 6.912 | 11.86 |
| exchange t2 sizes=mixed local | ns/alloc | 8.150 | 71.51 | 15.27 | 15.35 | 15.66 | 10.12 | 8.809 | 12.73 |
| exchange t2 sizes=small split | ns/alloc | 14.05 | 129.8 | 42.72 | 40.70 | 47.98 | 18.63 | 24.85 | 33.12 |
| exchange t4 sizes=small local | ns/alloc | 2.350 | 25.06 | 6.183 | 6.087 | 7.657 | 4.028 | 3.715 | 7.315 |
| exchange t8 sizes=small both | ns/alloc | 2.996 | 24.01 | 7.786 | 8.296 | 9.815 | 3.969 | 6.120 | 8.552 |
| mpmc p1 c3 sizes=small local all-alloc | ns/alloc | 5.237 | 101.2 | 26.47 | 23.43 | 21.72 | 7.583 | 8.508 | 9.643 |
| mpmc p1 c3 sizes=small local free-only | ns/alloc | 5.361 | 97.06 | 20.53 | 22.20 | 21.50 | 20.25 | 8.679 | 9.680 |
| mpmc p3 c1 sizes=small local all-alloc | ns/alloc | 4.842 | 79.30 | 7.704 | 9.346 | 9.056 | 10.73 | 8.182 | 11.73 |
| mpmc p3 c1 sizes=small local free-only | ns/alloc | 4.876 | 79.93 | 7.648 | 8.207 | 10.23 | 21.27 | 7.152 | 12.45 |
| mpmc p4 c4 sizes=small both all-alloc | ns/alloc | 4.991 | 73.89 | 21.31 | 21.36 | 24.37 | 8.579 | 9.399 | 12.26 |

Cross-thread, cgroup peak, MiB:

| Job | asmalloc | glibc | mimalloc | mi sized | mi crate | jemalloc | tc per-CPU | tc per-thread |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| pipeline t2 sizes=s64 local all-alloc | 1.250 | 1.000 | 4.750 | 4.750 | 4.750 | 1.443 | 2.500 | 2.250 |
| pipeline t2 sizes=small local all-alloc | 2.242 | 1.250 | 4.750 | 4.750 | 6.750 | 1.938 | 2.750 | 2.582 |
| pipeline t2 sizes=mixed local all-alloc | 4.951 | 3.311 | 8.750 | 8.750 | 10.64 | 5.070 | 3.750 | 3.070 |
| pipeline t2 sizes=small local free-only | 1.988 | 1.117 | 4.750 | 4.750 | 6.750 | 1.746 | 2.750 | 2.500 |
| pipeline t2 sizes=large objects=1M inflight=512 local all-alloc | 2.752 | 33.69 | 67.77 | 67.77 | 56.73 | 8.527 | 4.320 | 3.695 |
| pipeline t2 sizes=small split all-alloc | 2.574 | 0.973 | 4.750 | 4.750 | 6.750 | 1.965 | 2.658 | 2.574 |
| pipeline t2 sizes=small split free-only | 2.572 | 1.000 | 4.750 | 4.750 | 6.750 | 1.746 | 2.709 | 2.572 |
| pipeline t4 sizes=small local all-alloc | 3.996 | 2.809 | 7.250 | 8.250 | 9.248 | 3.639 | 3.998 | 4.590 |
| pipeline t8 sizes=small both all-alloc | 7.725 | 5.994 | 12.50 | 12.50 | 14.50 | 6.873 | 6.861 | 8.990 |
| pipeline t2 x2 sizes=small local all-alloc | 3.424 | 2.020 | 5.250 | 5.250 | 9.250 | 3.246 | 3.682 | 3.580 |
| pipeline t2 x2 sizes=small split all-alloc | 3.445 | 2.012 | 5.250 | 5.250 | 9.250 | 3.248 | 3.215 | 3.562 |
| pipeline t2 x3 sizes=small both all-alloc | 4.787 | 2.957 | 8.000 | 7.750 | 9.750 | 4.746 | 3.750 | 4.602 |
| pipeline t2 x4 sizes=small both all-alloc | 5.869 | 3.996 | 10.12 | 9.750 | 12.38 | 6.072 | 4.496 | 5.721 |
| exchange t2 sizes=small local | 2.246 | 1.752 | 4.750 | 4.750 | 8.750 | 2.432 | 2.912 | 9.270 |
| exchange t2 sizes=mixed local | 4.779 | 4.533 | 12.75 | 12.75 | 14.75 | 6.096 | 4.004 | 9.336 |
| exchange t2 sizes=small split | 2.895 | 1.498 | 4.750 | 4.750 | 8.750 | 2.381 | 2.889 | 8.043 |
| exchange t4 sizes=small local | 3.688 | 3.271 | 9.250 | 9.250 | 9.250 | 4.240 | 3.762 | 16.14 |
| exchange t8 sizes=small both | 6.838 | 6.197 | 12.50 | 14.25 | 14.38 | 8.682 | 5.240 | 25.72 |
| mpmc p1 c3 sizes=small local all-alloc | 3.059 | 1.250 | 5.250 | 5.250 | 9.250 | 2.246 | 3.027 | 2.701 |
| mpmc p1 c3 sizes=small local free-only | 3.023 | 1.250 | 5.250 | 5.250 | 6.756 | 1.996 | 3.094 | 2.578 |
| mpmc p3 c1 sizes=small local all-alloc | 3.467 | 2.039 | 7.250 | 7.250 | 9.250 | 3.246 | 3.496 | 3.027 |
| mpmc p3 c1 sizes=small local free-only | 3.471 | 2.025 | 7.250 | 7.250 | 9.250 | 2.963 | 3.254 | 3.002 |
| mpmc p4 c4 sizes=small both all-alloc | 6.023 | 2.732 | 10.25 | 10.38 | 12.25 | 4.516 | 4.219 | 3.961 |

The large-object pipeline is bimodal for asmalloc (seven rounds at 61.6 to 62.3 ns, three at
79 to 81) and, less often, for the tcmallocs and mimalloc.

Work stealing, time:

| Job | Metric | asmalloc | glibc | mimalloc | mi sized | mi crate | jemalloc | tc per-CPU | tc per-thread |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| forkjoin t4 local | ns/alloc | 3.453 | 10.69 | 5.786 | 5.846 | 5.934 | 4.803 | 4.416 | 5.144 |
| spawn t4 local | ns/task | 130.9 | 99.11 | 121.0 | 119.6 | 117.3 | 113.8 | 114.3 | 124.0 |
| forkjoin t8 both | ns/alloc | 3.818 | 17.75 | 7.998 | 7.549 | 8.564 | 3.947 | 5.134 | 7.632 |
| spawn t8 both | ns/task | 254.7 | 235.6 | 246.9 | 244.0 | 236.0 | 226.6 | 231.1 | 228.4 |

Work stealing, cgroup peak, MiB:

| Job | asmalloc | glibc | mimalloc | mi sized | mi crate | jemalloc | tc per-CPU | tc per-thread |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| forkjoin t4 local | 15.60 | 18.08 | 21.59 | 21.62 | 23.38 | 28.90 | 16.39 | 16.63 |
| spawn t4 local | 2.521 | 1.686 | 9.250 | 9.250 | 9.250 | 2.504 | 3.324 | 2.793 |
| forkjoin t8 both | 17.82 | 19.28 | 26.71 | 26.64 | 28.67 | 23.82 | 17.83 | 18.71 |
| spawn t8 both | 4.613 | 3.121 | 12.50 | 12.50 | 14.50 | 4.590 | 4.826 | 4.227 |

The RPC server, time:

| Job | Metric | asmalloc | glibc | mimalloc | mi sized | mi crate | jemalloc | tc per-CPU | tc per-thread |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| rpc echo c64 | server cycles/req | 46816 | 48862 | 46746 | 46590 | 47105 | 45799 | 45614 | 47409 |
| rpc log c64 | server cycles/req | 125168 | 149854 | 123607 | 121462 | 120967 | 127620 | 122863 | 128922 |
| rpc log c256 | server cycles/req | 121083 | 152528 | 119946 | 118420 | 118012 | 125013 | 119607 | 126574 |

The RPC server, cgroup peak, MiB:

| Job | asmalloc | glibc | mimalloc | mi sized | mi crate | jemalloc | tc per-CPU | tc per-thread |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| rpc echo c64 | 5.646 | 3.277 | 13.75 | 13.22 | 14.96 | 5.832 | 6.141 | 18.76 |
| rpc log c64 | 8.271 | 5.297 | 25.07 | 25.41 | 26.54 | 9.695 | 9.355 | 19.32 |
| rpc log c256 | 11.72 | 9.666 | 29.11 | 28.08 | 31.57 | 14.40 | 15.49 | 27.23 |

The RPC client is the bottleneck, so these rank the server's cycles per request.

## 6. Single-threaded

Time:

| Job | Metric | asmalloc | glibc | mimalloc | mi sized | mi crate | jemalloc | tc per-CPU | tc per-thread |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| churn live=100 small | cycles/op | 22.67 | 34.59 | 44.85 | 45.38 | 46.89 | 33.86 | 36.91 | 73.70 |
| churn live=1k small | cycles/op | 23.97 | 44.91 | 44.84 | 45.07 | 46.37 | 35.16 | 37.78 | 73.69 |
| churn live=10k small | cycles/op | 31.48 | 55.73 | 49.32 | 49.15 | 54.04 | 39.65 | 41.40 | 75.37 |
| churn live=100k small | cycles/op | 46.51 | 78.28 | 64.96 | 63.85 | 72.64 | 53.15 | 58.27 | 94.83 |
| churn live=1M small | cycles/op | 104.3 | 306.6 | 179.8 | 171.4 | 215.8 | 119.5 | 133.3 | 283.6 |
| churn live=10M small | cycles/op | 214.1 | 456.4 | 421.4 | 411.0 | 557.3 | 247.0 | 298.3 | 520.8 |
| churn live=100k mixed | cycles/op | 87.47 | 164.9 | 96.99 | 98.04 | 109.4 | 106.6 | 93.66 | 149.7 |
| map btree 1M | cycles/entry | 884.6 | 1324 | 820.6 | 826.1 | 817.6 | 947.9 | 1030 | 1224 |
| map hash 1M | cycles/entry | 533.8 | 1088 | 456.8 | 463.8 | 434.4 | 626.0 | 596.7 | 771.7 |
| tree depth=20 | cycles/node | 43.25 | 85.89 | 44.54 | 46.42 | 44.31 | 101.3 | 117.3 | 143.8 |
| vecgrow 1x256Mi | cycles/element | 2.203 | 6.718 | 9.653 | 9.636 | 4.150 | 14.37 | 11.82 | 11.79 |
| vecgrow 64x4Mi | cycles/element | 5.891 | 8.213 | 3.504 | 3.055 | 3.153 | 8.170 | 6.405 | 6.375 |
| json 32Mi | cycles/byte | 16.19 | 22.96 | 16.56 | 16.77 | 16.13 | 23.30 | 28.70 | 33.46 |
| largepair 64Ki+1Mi | ns/pair | 67.46 | 8.087 | 90.19 | 90.35 | 106.7 | 133.6 | 59.01 | 61.74 |

Cgroup peak, MiB:

| Job | asmalloc | glibc | mimalloc | mi sized | mi crate | jemalloc | tc per-CPU | tc per-thread |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| churn live=100 small | 0.801 | 0.500 | 2.539 | 2.539 | 4.531 | 0.713 | 2.113 | 1.707 |
| churn live=1k small | 0.910 | 0.629 | 4.543 | 4.547 | 4.527 | 0.820 | 2.244 | 1.834 |
| churn live=10k small | 2.117 | 1.750 | 6.549 | 6.547 | 6.533 | 1.869 | 3.322 | 2.834 |
| churn live=100k small | 12.07 | 12.60 | 14.57 | 14.56 | 16.55 | 11.77 | 13.47 | 13.00 |
| churn live=1M small | 109.2 | 120.6 | 116.8 | 116.8 | 118.8 | 110.3 | 113.9 | 113.5 |
| churn live=10M small | 1070 | 1199 | 1143 | 1143 | 1141 | 1091 | 1114 | 1114 |
| churn live=100k mixed | 34.40 | 40.64 | 54.64 | 54.64 | 56.63 | 42.68 | 34.92 | 34.49 |
| map btree 1M | 123.5 | 130.5 | 128.6 | 128.6 | 128.6 | 125.2 | 134.1 | 134.2 |
| map hash 1M | 204.3 | 208.2 | 299.1 | 299.1 | 260.8 | 181.7 | 253.6 | 253.2 |
| tree depth=20 | 51.16 | 64.50 | 96.68 | 96.67 | 96.66 | 66.93 | 68.72 | 68.45 |
| vecgrow 1x256Mi | 258.4 | 257.0 | 525.4 | 525.5 | 527.5 | 264.4 | 683.3 | 683.0 |
| vecgrow 64x4Mi | 277.4 | 265.4 | 265.0 | 265.0 | 291.0 | 307.1 | 286.7 | 296.3 |
| json 32Mi | 352.5 | 414.3 | 387.3 | 387.3 | 355.0 | 362.3 | 398.6 | 398.3 |
| largepair 64Ki+1Mi | 0.799 | 0.479 | 4.547 | 4.543 | 4.521 | 0.697 | 2.062 | 1.652 |

## 7. Programs through LD_PRELOAD

The Rust workloads built with glibc, then preloaded:

Time:

| Job | Metric | asmalloc | glibc | mimalloc | jemalloc | tc per-CPU | tc per-thread |
|---|---|---:|---:|---:|---:|---:|---:|
| churn live=100k small (preload) | cycles/op | 51.52 | 77.92 | 50.87 | 60.09 | 55.54 | 93.01 |
| pipeline t2 sizes=small local all-alloc (preload) | ns/alloc | 3.985 | 126.9 | 14.07 | 7.644 | 6.823 | 8.928 |
| json 32Mi (preload) | cycles/byte | 17.12 | 22.91 | 16.30 | 24.01 | 29.27 | 32.97 |

Cgroup peak, MiB:

| Job | asmalloc | glibc | mimalloc | jemalloc | tc per-CPU | tc per-thread |
|---|---:|---:|---:|---:|---:|---:|
| churn live=100k small (preload) | 12.11 | 12.61 | 14.59 | 12.00 | 13.47 | 13.04 |
| pipeline t2 sizes=small local all-alloc (preload) | 1.992 | 1.250 | 4.750 | 1.996 | 2.689 | 2.275 |
| json 32Mi (preload) | 353.3 | 414.4 | 387.3 | 360.5 | 418.8 | 418.6 |

mimalloc-bench programs:

Time:

| Job | Metric | asmalloc | glibc | mimalloc | jemalloc | tc per-CPU | tc per-thread |
|---|---|---:|---:|---:|---:|---:|---:|
| larson-sized 8 | Mops/s | 426.9 | 394.4 | 541.1 | 393.7 | 492.5 | 333.9 |
| larson 8 | Mops/s | 433.6 | 401.3 | 500.8 | 401.5 | 434.5 | 324.1 |
| xmalloc-test 4+4 | Mfrees/s | 396.5 | 95.03 | 301.4 | 21.72 | 549.5 | 6.488 |
| sh8bench 16 | s | 0.5966 | 1.570 | 0.2712 | 0.5034 | 1.980 | 5.118 |
| mstress 8 | s | 0.2625 | 0.3620 | 0.2474 | 0.4502 | 0.2907 | 0.3168 |
| cache-scratch 1 | s | 0.5186 | 0.5075 | 0.5062 | 0.5148 | 0.5084 | 0.5187 |
| cache-scratch 8 | s | 0.0674 | 0.0789 | 0.0672 | 0.0675 | 0.0711 | 0.0714 |
| cfrac | s | 2.107 | 2.102 | 2.080 | 2.206 | 2.183 | 2.720 |
| espresso | s | 2.316 | 2.479 | 2.226 | 2.290 | 2.270 | 2.434 |

Cgroup peak, MiB:

| Job | asmalloc | glibc | mimalloc | jemalloc | tc per-CPU | tc per-thread |
|---|---:|---:|---:|---:|---:|---:|
| larson-sized 8 | 71.47 | 89.11 | 105.0 | 88.77 | 65.05 | 54.33 |
| larson 8 | 72.24 | 92.03 | 102.1 | 89.76 | 60.70 | 53.60 |
| xmalloc-test 4+4 | 4.361 | 41.83 | 67.95 | 45.08 | 38.85 | 33.99 |
| sh8bench 16 | 120.1 | 164.7 | 210.7 | 162.2 | 119.7 | 121.9 |
| mstress 8 | 139.5 | 247.2 | 282.8 | 139.1 | 165.0 | 163.9 |
| cache-scratch 1 | 1.000 | 0.750 | 2.750 | 1.246 | 2.250 | 1.750 |
| cache-scratch 8 | 2.750 | 2.500 | 4.500 | 2.744 | 4.000 | 3.500 |
| cfrac | 1.641 | 1.447 | 4.527 | 1.404 | 2.809 | 2.381 |
| espresso | 2.570 | 1.334 | 8.535 | 1.707 | 3.105 | 2.838 |

**xmalloc-test** runs in one of two modes, by where the scheduler puts each pair of threads
(`launder-and-fill.md`, "A depot per cache domain"), so its medians say little. asmalloc: seven
runs at 374 to 401 Mfrees/s, peaking at 3.7 to 4.4 MiB, and three at 739 to 750, peaking at
34 to 36 MiB. Per-CPU tcmalloc: three runs at 189 to 194, seven at 547 to 557. The memory
median of 4.4 MiB is the slow mode's; at `b5d6e8f` five of ten runs were fast and the median
was 21 MiB.

asmalloc's preload library exports only the C names; C++ `new` and `delete` reach it through
libstdc++'s operators, where mimalloc's library defines them itself (section 9).

## 8. Memory after the peak

Measured 2026-10-18 with the allocator at `b5d6e8f`, on the same quiet machine as that
session, with `bench/probes/memory/` (each process confined to its CPUs and capped; medians of
two or three rounds, which agreed to within 0.1 MiB). Not repeated at `a92d51a`. The change
since that bears on it, the cap charged to growth, was measured on its own: its after-peak
curve agrees with the one below from 4 s on (`memory.md`, "The cap charged to growth").
Anonymous resident MiB.

After freeing 218 MiB in random order, with 2,000 64-byte objects allocated every millisecond
(`memrun.sh ... quiet live=2M sizes=small secs=16 after=burst after_sizes=s64`):

| | freed | 2 s | 4 s | 6 s | 8 s | 12 s | 16 s |
|---|---:|---:|---:|---:|---:|---:|---:|
| asmalloc | 162 | 70 | 53 | 30 | 17.5 | 4.9 | 1.2 |
| jemalloc | 187 | 142 | 142 | 56 | 12.5 | 4.5 | 4.5 |
| mimalloc | 226 | 226 | 226 | 226 | 14.8 | 16.2 | 16.2 |

The worst cases of `docs/research/memory.md` (`worst.sh`), MiB, asmalloc / jemalloc /
mimalloc:

| Case | cgroup peak | first sample after | last sample |
|---|---|---|---|
| burst, then idle (12 s) | 218 / 219 / 227 | 70 / 142 / 226 | 4.9 / 4.5 / 16.2 |
| phase change | 52.7 / 29.0 / 18.6 | 34.5 / 26.5 / 16.3 | 9.3 / 24.7 / 15.9 |
| many CPUs, few classes | 5.3 / 6.8 / 13.4 | 2.5 / 4.4 / 8.3 | 1.0 / 4.3 / 8.3 |
| many classes, few CPUs | 15.4 / 7.8 / 6.9 | 9.4 / 5.9 / 6.2 | 1.4 / 4.2 / 6.2 |
| 16 threads on 2 CPUs | 13.0 / 52.4 / 31.4 | 7.5 / 51.0 / 28.4 | 1.6 / 47.8 / 28.0 |
| sparse 1 MiB blocks | 3.4 / 3.9 / 277 | | |
| sparse 64 KiB blocks | 33.4 / 34.2 / 345 | | |
| tiny process | 1.1 / 0.8 / 2.5 | | |

When no allocation leaves the magazines (light work after the free), nothing in a program's
own calls runs the maintenance, and memory stays where the free left it until the opt-in
background thread is on (`background.sh`, three rounds):

| | freed | 2 s | 4 s | 8 s | 16 s |
|---|---:|---:|---:|---:|---:|
| native, background off | 162 | 162 | 162 | 162 | 162 |
| native, background on | 166 | 95 | 53 | 17 | 1.1 |
| preloaded, background off | 177 | 177 | 177 | 177 | 177 |
| preloaded, background on (`ASMALLOC_BACKGROUND=1`) | 177 | 70 | 39 | 13 | 1.0 |
| jemalloc, native or preloaded | 187 | 187 | 187 | 187 | 187 |

## 9. Where asmalloc is behind, and why

### At `a92d51a`

The gaps of the summary, with what the notes that measured them found. None was profiled
again for this session.

- **Large objects crossing threads** (62 against per-thread tcmalloc's 35 ns/alloc). With
  the kernel gone from it, the largest cost left is the leaf word's compare-and-swap, which
  moves a cache line between the two cores on every allocation and free of a large object.
  tcmalloc caches objects up to 256 KiB per CPU and moves them between CPUs in batches; doing
  the same here would park hundreds of KiB per CPU per class and move the small/large
  boundary, and was decided against (`memory.md`, "Lines between CPUs, and the cost of a
  large block").
- **`largepair`** (67 ns a pair against glibc's 8 and per-CPU tcmalloc's 59). glibc serves both
  blocks from the top of its heap and gives them back there. asmalloc runs about 1,400
  instructions a pair, after the change that halved them, against tcmalloc's 1,000 (same
  section).
- **64 Vecs grown to 4 MiB** (5.9 against mimalloc's 3.1 cycles/element; 185k faults against
  281). A block that grows in place loses a huge page at each doubling: its stagger makes a
  block of exactly 2 MiB end a few bytes into the next huge page, which the program then
  faults as a small page. Advising that page at the growth took the job to 4.5
  cycles/element but cost 126 MiB of peak, and was not adopted (`memory.md`, "Headroom for
  growing blocks").
- **HashMap** (23% behind the mimalloc crate; 61k faults against 277). hashbrown allocates
  each larger table fresh, at its size, and a large block allocated at its size gets no huge
  pages by design, since a sparse one would cost 2 MiB per page touched; so the build faults
  4 KiB pages, and the drop launders scattered values (`memory.md`, "HashMap"). Its 12% of
  memory over jemalloc is the outgrown tables kept for the next build (section 4).
- **BTreeMap** (8%). The drop takes one locked OR per scattered slot against mimalloc's plain
  pushes onto a page's list; the build runs mimalloc's instructions in more cycles, on 4 KiB
  pages for the internal nodes' class, which never reaches the 16 MiB at which a class gets
  huge pages (`launder-and-fill.md`, "A direct path for scattered launders"; `memory.md`,
  "Build phases against mimalloc"). A 4 MiB threshold would win about 4% here for up to 6 MiB
  of peak elsewhere; it stays at 16 MiB by decision.
- **sh8bench** (2.2 times mimalloc, 19% behind jemalloc). Over 40% of its time is the
  placement across two L3s: on one L3 it takes 0.33 s (`launder-and-fill.md`, "Domain stacks and a
  shared stack"). What the rest of the two-L3 cost is made of is not established.
- **larson** (16 to 25% behind mimalloc): cause D below, unchanged. Accepted for now: C++
  programs reach the allocator through libstdc++'s operators, and exporting `operator new`
  and `delete` is deferred.
- **tokio spawn** (33% behind glibc at four threads, 12% behind jemalloc at eight): false
  sharing between tokio's per-worker queue headers, which a 40-byte class places back to
  back; asmalloc retires the fewest instructions per task. The spread at four threads is
  clock frequency. Section 15.
- **RPC, mstress, preloaded JSON, espresso, cfrac** (1 to 6%): not analysed.
- **Small heaps (memory)**: fixed footprint, mostly page tables (about 8 KiB per class in use,
  since each class's region is 4 GiB from the next), parked objects in cross-thread flows, and
  the magazine area (`memory.md`, "Small heaps", "What is left, and proposals").

### At `b5d6e8f`

Profiles and counters for each job then behind the best incumbent by more than about 10% on
time, kept as they were written. Since then cause A was removed by charging the retained cap
to growth, most of B by ascending, whole-magazine fills and launders and a direct path for
scattered launders, about half of C by fillers that no longer meet on the same words and by
the depot per cache domain, and E by the depot per cache domain; D is unchanged (section 4).

For each job, a profile (`perf record`, one run, the same CPUs and confinement as the
session) and counters per allocation: slow-path entries from an instrumented copy of the
allocator (not merged), system calls from `strace -c`, page faults and cache fills from `perf
stat`. Single runs; that session's medians are in its detail file.

**A. Memory returned and faulted straight back.**

- *Large objects crossing threads* (297 against tcmalloc-thread's 35.6 ns/alloc). Half the
  cycles are in the kernel (1.63e9 against 7.7e6): TLB shootdown IPIs
  (`smp_call_function_many_cond` 14%, TLB flushes 13%). 295k `madvise` calls for 550k large
  frees, and 224k page faults against 1,001. Each free past the 8 MiB large-class retained
  cap purges its own slot, and about 36 MiB is always in flight (512 objects of about
  70 KiB), so every free purges and the next allocation faults the slot back; each purge
  interrupts the other thread's CPU. The 16 to 32 KiB classes' two-object magazines add
  about 0.35 slow-path entries per allocation, a small part.
- *HashMap* (60% behind mimalloc): kernel cycles 537M against 39M, 190k faults against 301.
  hashbrown allocates each larger table fresh, so it gets no huge pages and faults in 4 KiB
  pages; the old table, freed past the large cap, is purged.
- *JSON* (45%): 81k `madvise` calls, 93k faults against 383, nine times the kernel cycles.
  Dropping each 350 MiB document pushes the small-class count past its 64 MiB cap; the cap
  purges and the next parse faults the memory back.
- *mstress* (57%): kernel cycles 2.75e9 against 0.31e9, 680k faults against 663, 10k
  `madvise` and 15.5k large blocks.

**B. Every object through the back end's bitmaps on each build-and-drop cycle.**

- *Box tree* (116% behind mimalloc): 82.5k slow-path entries each way for 21M nodes, each
  moving a whole 254-object magazine, so every node is laundered to the bitmaps when the tree
  is dropped and filled from them when the next is built: the depot keeps 128 KiB of a
  class, 4k nodes, of a 2M-node tree. The depot and back-end symbols are about 23% of the
  cycles, some 20 cycles a node. Recycled nodes also come back in an order unrelated to their
  addresses: 9.6M DRAM fills against mimalloc's 4.5M and 9.9M L2 fills against 29.1M, and the
  walk, which allocates nothing, takes 2.3 times mimalloc's cycles.
- *BTreeMap* (32%): the extra time is user cycles (1.1e9 over 5M entries), with fewer DRAM
  fills than mimalloc (18.9M against 23.1M), so not placement: the launder-and-fill traffic
  (30k magazine exchanges) and 38k faults against 262.

**C. Contended refills: sh8bench** (4.6 times mimalloc). 437M mallocs (counted by a shim
preloaded ahead of the allocator), of which 0.3% take a slow path (1.42M) and 117k are large;
2.7k `madvise` and 40.8k faults, so not the kernel. The back end's fill, claiming objects
from the per-class bitmap tree that every CPU shares (`tree::clear` and `tree::descend`
under `fill`), is 25% of the cycles: about 7,000 cycles per magazine refill, spent on
`fetch_and` and on a failed compare-exchange in the descent, with 16 threads on 8 CPUs
refilling the same 8- and 16-byte classes. The malloc and free routines take 23.5%, about 21
cycles a pair against mimalloc's 9; not analysed further.

**D. C++ through libstdc++: larson** (29 to 30%). asmalloc's and mimalloc's runs use the same
user and kernel cycles over larson's fixed duration, and 0.16% of mallocs take a slow path.
The preload library exports only C names, so `new` goes through libstdc++'s `operator new`
and `malloc@plt`: 7.8%, 2.6% and 3.8% of the cycles, where mimalloc's library defines
`operator new` itself.

**E. A depot shared by both L3 domains: independent pipelines** (two split across domains
35% behind jemalloc, three 16%, four 24%). With the chains on different domains asmalloc
takes 8.4M fills from the other L3 against jemalloc's 22k, about 0.4 per allocation, while
the depot's own code is only 3 to 6% of the cycles. Magazines that one chain frees go to the
per-class depot stacks that every CPU shares, so the other chain allocates objects whose
cache lines live in the other domain; jemalloc's per-thread arenas keep each chain's memory
in its own. With both chains in one domain asmalloc is 49% ahead.

**Others.**

- *tokio spawn* (10 to 18% behind glibc and jemalloc): false sharing inside tokio's own
  structures, which depends on where the allocator happens to place them; task cells are not
  the cause. Section 15.
- *One Vec grown to 256 MiB* (15% behind the mimalloc crate, under the job's old definition):
  mimalloc's huge pages; asmalloc takes the faults of 4 KiB pages up to the size where grown
  blocks get huge pages.
- *The same Rust programs preloaded* behave as natively: JSON is 44% behind for cause A.
- *Small heaps (memory)*: the 0.4 to 3 MiB over glibc on jobs of a few MiB is fixed
  footprint: the slab array, the first magazine-area unit, a loaded magazine per class in
  use and the metadata of each region a class has touched.

**What would fix them**, as proposed then (smallest changes, not implemented or measured at
the time): A, stop purging large slots on the free path at 8 MiB and let the clocked decay
return them, keeping a per-free purge only above a much larger bound; the small-class cap
similarly. B, let a class's depot keep what was freed within an interval (shedding at the
interval's end) instead of laundering past a fixed 128 KiB, and fill magazines so they pop in
ascending address order. C, claim a whole bitmap word per atomic operation and continue the
descent instead of restarting it. D, export `operator new` and `delete` from the preload
library. E, prefer magazines from the same L3 domain in the depot. What was built instead or
as proposed: for A, the cap charged to growth; for B, ascending magazines, but not the
interval rule (whole-magazine and direct launders made the laundering cheaper instead); for
C, a claim that retries against the value it found and runs of claims that move apart; for
E, a depot per cache domain. D is deferred.

## 10. Do the memory rules earn their place?

Measured 2026-10-18 with the allocator at `b5d6e8f`. Several rules have changed since (the
cap is charged to growth, not to frees, and growing blocks get sixteen times their room;
`memory.md`), so this describes the allocator then.

Each rule switched off in a scratch copy (one edit each, not merged), built and run as an
allocator entry of its own against asmalloc as built: eleven jobs, five interleaved rounds
(no invalid runs), and the worst-case memory workloads, two rounds, on the same quiet
machine. Changes are the rule-off build relative to asmalloc as built, so a positive number
is what the rule saves; * marks a 95% interval that excludes zero. mpmc was bimodal in this
run (it moved by a third under rules it never reaches), so only its starred changes count.

| Rule switched off | What changed | Verdict |
|---|---|---|
| Huge pages for large blocks that grow by doubling | one Vec to 256 MiB 131%* slower, 64 Vecs 37%* slower; peaks within 1% | earns its place |
| The density test on that rule | nothing beyond noise; no workload grows a block sparsely (the soak's 1013 against 271 MiB is the only evidence) | moves no number here |
| Staggered starts for large blocks | 64 Vecs walked together 313%* slower; but sparse 64 KiB blocks peak at 17.8 MiB without it against 33.4 with it, and large objects crossing threads are 6.7%* faster without | earns its place on time; costs memory when blocks are touched in their first page, since the stagger makes that page straddle two |
| The move that skips all-zero pages | one Vec 8.5%* and 64 Vecs 4.9%* faster without it; no workload moves a sparse block | costs time here; its benefit is unmeasured |
| The cap's re-arm | JSON 2.6%* faster without it; memory unchanged everywhere, after-peak included | moves no number here |
| The one-in-32 refresh | mixed pipeline peak 8.8%* lower without it, mpmc 6.4%* lower, churn at 1M 2.3%* faster, the RPC server 4.2%* slower; after-peak unchanged | moves no number it is meant to; slightly worse memory with it |
| The magazine age limit | burst-then-idle ends at 60.1 MiB without it against 4.9; many classes on few CPUs 2.9 against 1.4, 16 threads on 2 CPUs 3.2 against 1.6 | earns its place |
| The spare sweep | burst-then-idle ends at 7.6 MiB without it against 4.9; phase-change peak lower without it (35.5 against 53.0, within this case's noise) | earns a little after the peak |
| The depot's low-water shedding | burst-then-idle ends at 57.1 MiB without it against 4.9; phase change 16.9 against 9.4, the other worst cases 2 to 4 times higher at the end | earns its place |
| Huge pages for small classes past 16 MiB | JSON 33.5%* slower without them, BTreeMap 1.7%*; peaks 0.6 to 1.4%* lower without them | earns its place |

Time, each rule off against asmalloc as built (* = 95% interval excludes zero)

| Job | asmalloc | grown-huge | density | stagger | skip-zero | rearm | refresh | age | sweep | shed | small-huge |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| vecgrow 1x256Mi | 5.604 cycles/element | +131.2%* | +1.2% | -4.8% | -8.5%* | -0.5% | -8.5% | -4.4%* | -9.1%* | -2.6% | -7.6%* |
| vecgrow 64x4Mi | 9.51 cycles/element | +36.8%* | -0.6% | +313.4%* | -4.9%* | +0.1% | -4.3%* | -3.8% | -3.8%* | -4.8%* | -4.6%* |
| map hash 1M | 698.4 cycles/entry | +0.2% | -0.1% | -0.1% | -0.2% | -0.5% | +0.6% | +0.5% | -0.4% | +0.1% | +0.2% |
| map btree 1M | 1097 cycles/entry | +0.2% | +0.1% | -0.5% | -0.8% | -0.3% | -0.5% | -0.5% | -0.5% | -0.5%* | +1.7%* |
| json 32Mi | 24.09 cycles/byte | -0.6% | -0.9% | -2.0% | -1.8%* | -2.6%* | +0.7% | -0.6%* | -0.2% | -0.7%* | +33.5%* |
| churn live=1M small | 114.3 cycles/op | +0.0% | +0.1% | -0.0% | -0.3% | +0.1% | -2.3%* | +0.0% | +0.2% | -0.3% | -0.3% |
| churn live=1k small | 25.11 cycles/op | -0.1% | -0.1% | +0.0% | -0.1% | -0.2% | +0.0% | +0.1% | -0.1% | +0.1% | +0.0% |
| pipeline t2 sizes=mixed local all-alloc | 11.21 ns/alloc | +0.6% | -0.2% | +0.2% | +0.3% | +0.1% | -1.3% | -0.1% | +0.9% | +0.6% | +0.3% |
| mpmc p3 c1 sizes=small local all-alloc | 5.148 ns/alloc | +2.9% | -0.3% | +2.1% | +34.4% | +1.4%* | -4.6% | +11.7% | +0.2% | +31.2%* | +1.2% |
| pipeline t2 sizes=large objects=1M inflight=512 local all-alloc | 265.5 ns/alloc | -1.0% | -3.4% | -6.7%* | +0.8% | -0.0% | +3.5% | -0.3% | -0.3% | -0.1% | -3.0% |
| rpc log c64 | 1.223e+05 server cycles/req | +0.8% | +3.2%* | +0.9% | +1.0% | -0.2% | +4.2%* | +4.7%* | +0.8%* | +1.6%* | +2.7% |

Cgroup peak, each rule off against asmalloc as built (* = 95% interval excludes zero)

| Job | asmalloc | grown-huge | density | stagger | skip-zero | rearm | refresh | age | sweep | shed | small-huge |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| vecgrow 1x256Mi | 257.9 MiB | +0.7%* | -0.0% | -0.0% | -0.0% | -0.0% | -0.0%* | -0.0%* | -0.0%* | -0.0% | -0.0% |
| vecgrow 64x4Mi | 270.3 MiB | -0.6%* | -0.0% | -0.8%* | -0.0% | -0.0% | +0.0% | +0.0% | +0.0% | -0.0% | -0.0% |
| map hash 1M | 175.1 MiB | -0.0% | -0.1% | -0.0%* | +0.0% | +0.1%* | -0.0% | -0.0% | +0.0% | +0.0% | -0.0% |
| map btree 1M | 123.4 MiB | -0.0% | +0.0% | +0.1%* | -0.1% | +0.0% | +0.0% | -0.1% | +0.0% | +0.0% | -0.6%* |
| json 32Mi | 353.1 MiB | +0.2%* | -0.3%* | -0.2%* | -0.3%* | -0.3%* | -0.3%* | -0.3%* | -0.3%* | -0.2%* | -1.1%* |
| churn live=1M small | 109.3 MiB | -0.0% | -0.0% | -0.0% | -0.0% | +0.0% | -0.0% | -0.1% | +0.0% | -0.0% | -1.4%* |
| churn live=1k small | 1.043 MiB | -1.9% | +0.7% | +0.4% | -1.9% | +0.4% | +0.0% | +0.0% | +0.7% | +1.9% | -3.4% |
| pipeline t2 sizes=mixed local all-alloc | 5.105 MiB | +0.9% | +0.2% | +0.3% | +0.8% | -0.6% | -8.8%* | -1.0% | +0.0% | +0.6% | -1.8% |
| mpmc p3 c1 sizes=small local all-alloc | 3.719 MiB | -6.6% | -0.4% | -0.2% | -0.2% | -6.5% | -6.4%* | -0.2% | +0.3% | -6.6% | +0.2% |
| pipeline t2 sizes=large objects=1M inflight=512 local all-alloc | 3.078 MiB | +0.3% | +0.5% | -1.0% | -1.4% | +0.4% | +6.8%* | +0.0% | -1.6% | +0.6% | -0.6% |
| rpc log c64 | 7.84 MiB | +2.4% | +2.2% | +0.1% | +0.2% | +4.1% | +2.7% | -0.5% | +0.7% | +3.0% | +10.1% |

## 11. Smoke set for the back end

`benchrun run --suite backend` runs five small jobs that between them expose every cause in
section 9's `b5d6e8f` list except D, in about 35 seconds a round: two threads passing 300k
objects of 4 to 256 KiB (cause A for large blocks), two independent pipelines on different L3
domains (E), a Box tree of depth 18 built and dropped four times (B), JSON of 8 MiB parsed
three times (A for small classes) and sh8bench preloaded (C). Two more were added later: one
buffer grown by doubling to 4 MiB and freed, a thousand times, and the same to 1 MiB four
thousand times, which time the large path's moves and kept slots (`memory.md`, "Grown blocks
kept up to a budget"). Run it with `--rounds 3` against the incumbents on every change to the
back end. One round on 2026-10-18 at `b5d6e8f`, asmalloc against the best incumbent: 351
against 46 ns/alloc, 11.1 against 8.2 ns per chain, 128 against 34 cycles/node, 23.5 against
17.5 cycles/byte, 1.36 against 0.39 s.

## 12. Caveats

- One machine, one kernel, transparent huge pages in `madvise` mode, which decides much of
  the build-and-drop jobs: mimalloc asks for huge pages for its whole heap, asmalloc for small
  classes past 16 MiB carved and for grown blocks. A session with THP `always` would show the
  other side.
- Single-threaded build jobs measure every repetition from a cold process, so allocators that
  keep freed memory gain over those that return it; jemalloc's time-based purging never fires
  in runs this short.
- Wall-time metrics of the C programs include process start-up, a few percent of the
  shortest (cache-scratch with eight threads, about 70 ms).
- The sampler reads `smaps_rollup` once a second, which holds a process's `mmap_lock`
  during a page-table walk.
- The RPC client is the bottleneck, so the RPC jobs rank server CPU per request.
- Peak memory on small jobs is mostly fixed footprint.
- Some jobs run in two modes: xmalloc-test, the large-object pipeline and tokio spawn. Their
  medians depend on how many runs fell in each.
- The workloads were rebuilt between the `b5d6e8f` and `a92d51a` sessions, and JSON and the
  tree carry a few percent of code layout (section 14), so moves of that size between the
  sessions are not resolved.
- The profiles in section 9 are single runs; their counts per allocation are exact, their
  attribution of cycles to symbols is sampled.

## 13. Earlier sessions and changes to the suite

**The `b5d6e8f` session** (2026-10-18, the same machine, quiet, ten rounds, 55 jobs, 4,280
runs, none invalid, none left out as busy) was the previous report; its data are listed at
the top. Its control's largest difference was 9.1%, with 7 of 55 intervals excluding zero.
Section 4 compares it with this one, and sections 8, 10 and 11 were measured with the
allocator at that commit. Changes to the suite between the two sessions:

- `largepair 64Ki+1Mi` was added to the full suite: one 64 KiB and one 1 MiB block
  allocated, written once and freed, two million times.
- The `vecgrow` jobs fill each vector by `memset` steps instead of one push per element
  (section 14), so their numbers before and after are different jobs.
- The tree job times its build, walk and drop apart, and the drops of the tree and the maps
  run out of line, so that a profiler can name them. The jobs' metrics are defined as before;
  the tree's incumbents moved with the change (glibc 24%), the maps' by 2% or less.
- mimalloc-sized was run; the `b5d6e8f` session left it out.

**The first session** (2026-10-17, at `60ce673`, without asmalloc) ran on a busy machine; its
data are listed at the top. Its findings about the incumbents hold in both later sessions
(jemalloc and per-CPU tcmalloc lead cross-thread work among the incumbents, mimalloc leads
build-and-drop through huge pages, jemalloc is the memory floor on large heaps), with these
changes to the suite since, measured at the time:

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
allocates, the tikv build is 1 to 4% faster than the 2026-10-17 session's numbers; on jobs
with free-only threads it is two to three times slower. The suite now runs both shapes under
their own names: `all-alloc`, where every thread allocates once first (the primary jobs), and
`free-only`.

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

Every tcmalloc preload figure of the 2026-10-17 session was measured with the descriptor build and
understates tcmalloc by about this much.

## 14. Code layout and the single-threaded jobs

2026-10-18, at `a2688b7` plus this change. The 4 MiB grow-and-free job ran 40% slower after
an unrelated edit moved its push loop (`memory.md`, "The grow loop's 40%"), so every
single-threaded job was checked for the same thing.

**Method.** `benchrun layout` builds the workloads four ways that compile the same work to
other addresses: the normal build; `-C llvm-args=-align-loops=64`; and lld's
`--shuffle-sections` with seeds 1 and 2, which reorders every function (16-byte aligned, so
each lands at another offset in its line; `grow_and_drop`, for one, is at offsets 32, 0, 0
and 48 of its 64-byte line, and the tree's drop at 0, 32, 16 and 48). The variants run
interleaved in random order, one fresh process each, single-threaded jobs on CPU 2, under
the measurement lock; medians over rounds. Spread is the largest layout median over the
smallest; noise is the largest half-IQR of one layout's runs. `loops64` retires up to 1 to 3%
more instructions in loop-heavy jobs (its padding); the shuffles retire the same. The
variants move the allocator's Rust code too, so a profile (`perf record -e cycles:u`,
`perf annotate`) decides whose code a sensitivity is in.

**The grow jobs.** Before, the 4 MiB loop (asmalloc, five rounds) spread 6.9%, and one build's
runs ranged from 2.38 to 3.11 cycles/element. `vecgrow` now fills each vector by `resize` in
steps of up to 4 KiB, each no larger than what the vector holds, so the vector reallocates at
the same sizes as when pushing (checked for every length to 5,000 and for the suite's); the
fill is a `memset` in libc, whose code no build moves, in an out-of-line `grow_and_drop`. A
first version copied a chunk from the stack with `extend_from_slice`; its loads aliased the
stores to the vector at 4 KiB strides, and the 1 MiB loop ran 0.445 or 0.472 cycles/element
by where the stack began (environment padded with ASLR off), so it was dropped for the fill,
which is flat across the same padding (0.415 to 0.431). Now 92 to 96% of the 4 MiB loop's
cycles are libc's `memset` and `memmove`, and under 1% the workload's.

Seven rounds, spread (noise) per allocator:

| Job | asmalloc | jemalloc | mimalloc crate |
|---|---:|---:|---:|
| vecgrow 1x1Mi grow-and-free | 1.7% (0.9%) | 1.4% (1.3%) | 1.3% (0.8%) |
| vecgrow 1x4Mi grow-and-free | 1.7% (4.3%) | 5.3% (2.8%); 15 rounds 4.0% (3.0%) | 1.7% (3.7%) |
| vecgrow 1x256Mi | 0.8% (2.1%) | 1.6% (0.6%) | 0.3% (0.4%) |
| vecgrow 64x4Mi | 0.8% (0.3%) | 0.6% (0.8%) | 0.1% (0.3%) |

The 4 MiB job's noise is not layout. Each build's runs fall in two groups about 9% apart
(asmalloc 0.627 or 0.684 cycles/element) in every layout, and with ASLR off and the stack
fixed too, so it comes from the process's physical memory, not its addresses. jemalloc's
`loops64` build spends its time in the same libc `memset` and `memmove` (96%) as its default
build, whose code is at the same addresses in both; its 4 to 5% is that bimodality over few
rounds, not a moved loop.

**The rest, asmalloc** (five rounds; spread, noise, instructions):

| Job | Spread | Noise | Instructions |
|---|---:|---:|---:|
| churn live=100 small | 0.6% | 0.6% | 0.01% |
| churn live=1k small | 0.3% | 0.4% | 0.00% |
| churn live=10k small | 0.6% | 0.7% | 0.01% |
| churn live=100k small | 0.4% | 0.5% | 0.03% |
| churn live=1M small | 1.2% | 1.2% | 0.11% |
| churn live=10M small | 1.1% | 1.6% | 1.98% |
| churn live=100k mixed | 0.5% | 0.1% | 0.03% |
| churn live=100k small (preload) | 0.8% | 0.2% | 0.01% |
| map btree 1M | 0.8% | 1.0% | 1.51% |
| map hash 1M | 0.8% | 0.3% | 0.23% |
| largepair 64Ki+1Mi | 0.9% | 1.6% | 0.55% |
| json 8Mi | 2.9% | 0.8% | 0.75% |
| **json 32Mi** | **3.5%** | 0.5% | 0.73% |
| **json 32Mi (preload)** | **6.7%** | 0.8% | 0.99% |
| **tree depth=18** | **5.1%; again 9.2%** | 2.8% | 0.14% |
| **tree depth=20** | **3.3%; again 2.9%** | 1.3% | 0.14% |

The C programs (cfrac, espresso, cache-scratch 1) are not built here and were not checked.

- **json**: the parse phase moves (preloaded, the allocator is the shared library, which no
  variant moves: 1,965 against 2,155 Mcycles), in serde_json's
  `Value::deserialize`, half of whose time is one 16-byte reload of a `Value` from the stack
  after the recursive call that wrote it. That is the parser the job exists to run; rewriting
  it would make it another job, so it is left as it is, and json results compared across
  builds of the workloads carry a few percent of layout.
- **tree**, two places. The drop phase is 7% slower in `loops64` (404 against 432 Mcycles at
  depth 20). The profile put it at the `lea 0x8(%rcx)` after the load of the per-CPU slab's
  top in asmalloc's free fast path, inlined into the recursive drop, and this audit took it
  for the store-to-load hop on that word. It is not: the hop stays free at every placement.
  The drop function's last straight run leaves a tail of one to three instructions in a new
  64-byte line at some placements, which misses the op cache on every call, and the profile
  blames whatever dispatches late behind it (`zen5-roofline.md`, section 15). The caller's
  layout decides it and no change to the routine removes it. The walk phase
  (a recursive sum, workload code) is 5 to 7% slower in the default build than in the others.
  A loop over an explicit stack instead put a store-to-load chain on every node and made the
  walk twice as slow, so the recursion stays.

**Grow-and-free under the new definition** (thousands of cycles a repetition, medians; 1 and
4 MiB: `benchrun --suite backend`, seven rounds; 64 MiB: `vecgrow vecs=1 max=64Mi reps=40`
by hand, seven interleaved runs):

| To | asmalloc | glibc | jemalloc | mimalloc | mimalloc crate | tcmalloc per-CPU |
|---|---:|---:|---:|---:|---:|---:|
| 1 MiB | 55.4 | 32.7 | 41.3 | 113.8 | 108.8 | 108.4 |
| 4 MiB | 334.6 | 169.7 | 395.2 | 440.2 | 430.7 | 434.1 |
| 4 MiB, faults | 0.6 | 2.1 | 1.8 | 0.0 | 0.0 | 2.4 |
| 64 MiB | 14,582 | 47,955 | 97,021 | 32,074 | 31,947 | 73,983 |
| 64 MiB, faults | 69 | 16,386 | 30,771 | 1.8 | 1.8 | 17,105 |

Without the push loop, the 1 and 4 MiB loops take a quarter to a sixth of what they did, and asmalloc
is 1.7 to 2 times glibc there: about half of its cycles are `memmove` in its moves, while
glibc's loop is 96% the fill itself, so it grows without copying. At 64 MiB asmalloc leads by
2.2 times. (Measured before growing blocks got sixteen times their room; `memory.md`,
"Headroom for growing blocks", has the loops since.)

## 15. The tokio spawn jobs: false sharing in tokio, placed by the allocator

2026-10-18 and 19, at `a92d51a`. asmalloc trailed on both spawn jobs in two full sessions
(t4: 130.9 ns/task against glibc 99.1 and jemalloc 113.8; t8: 254.7 against 235.6 and
226.6). The cause is not work in the allocator. Two of tokio's own structures have fields
that different workers write sitting on one cache line, and whether they do depends on where
the allocator puts those structures.

**Method.** The workload binaries as `benchrun build` makes them, run by hand on the
session's CPUs (0-3; 0-3 and 8-11) under the measurement lock, the allocators interleaved,
three or four rounds. `perf stat` for counters (no more than two events beside the
workload's own, so nothing multiplexed); `perf record` of cycles for profiles; IBS op
sampling (`perf c2c record -c 10000`) for the data address, source and latency of each
sampled load; hardware breakpoints (`perf stat -e mem:ADDR:x`, ASLR off) to count calls of
out-of-line functions. Experiments were one-line edits to a scratch build, not merged.

**What the allocator does.** Per task: three allocations and three frees (the child's
256-byte task cell, a request and a response of 16 to 256 bytes). asmalloc retires fewer
instructions per task than anyone (t4: 1,705 against jemalloc 2,036 and glibc 2,070). Over a
whole t4 run, 56k allocation and 56k free slow paths for 16.9M allocations (0.3%), 36k
pushes to the depot's domain stacks; slow paths and depot together are about 0.5% of the
cycles. Faults, system calls and kernel time match jemalloc's (553 against 475 faults a
process, 31 ms of system time each); the workers are pinned, about ten migrations a process,
all at start-up, and about 20 involuntary switches a run bound the rseq aborts. tokio makes
the same calls under asmalloc and jemalloc (5.5M `notify_parked_local`, 0.2M parks and 10.9M
`run_task` per t4 run, within 2%).

**Where the cycles go.** The extra cycles are in tokio's code, at locked instructions
(compare-and-swap on a task's state, the `Arc` count, the task list's counter) that wait for
earlier stores to lines that another core holds. asmalloc's t4 runs make 40 requests for a
line in exclusive state per task against jemalloc's 26 and glibc's 21, and fetch 42 lines
per task from the L3 or another core's cache against 30 and 25. IBS puts 75 to 95% of all
L1-miss latency, for every allocator, on one line of tokio's scheduler handle (item 2), and
in asmalloc's runs most of the rest on the workers' run-queue headers (item 1):

1. **The workers' run-queue headers.** tokio allocates each worker's queue header, an
   `Arc<queue::Inner>` of 40 bytes holding the queue's `head` (written by thieves) and
   `tail` (written by the owner), one after another on the thread that builds the runtime,
   with a 48-byte `Arc<park::Inner>` between each two. asmalloc has a 40-byte class, so the
   four headers land 40 bytes apart and share lines: worker 1's `head` shares a line with
   worker 0's `head` and `tail`, and worker 1's `tail` with worker 2's. jemalloc has no
   40-byte class; the headers go to its 48-byte class, interleaved with the park structures,
   96 bytes apart, and IBS sees no misses on them. glibc puts each worker's 2 KiB queue
   buffer between two headers. Serving 33 to 40 bytes from the 48-byte class instead (a
   scratch build, not a proposal) gave the same interleaving and removed the difference:

   | ns/task (cycles/task, all workers) | asmalloc | asmalloc, 40 served as 48 | jemalloc |
   |---|---:|---:|---:|
   | spawn t4 local | 130.2 (2,895) | 115.4 (2,573) | 114.3 (2,535) |
   | spawn t8 both | 251.5 (11,139) | 235.1 (10,440) | 226.7 (10,061) |

   The run-queue line costs asmalloc about 320 cycles a task at t4 and 700 at t8. The slow
   compare-and-swaps on task headers in `wake_by_val` and in the child's poll (twice
   jemalloc's cycles there, with few misses of their own) are this too, most likely waiting
   for an earlier store to the falsely shared `tail` to drain. With the headers apart, every
   tokio function costs what it does under jemalloc. The 4% left at t8 is on the task-list
   line below, whose cost under eight cores on two L3s varies; not analysed further.

2. **The scheduler handle's task-list line.** `Arc<Handle>` is about 640 bytes. On one line
   of it sit the owned-task list's counter, which every spawn increments and every
   completion decrements from any worker, and fields that the same calls only read (and, in
   some builds, the idle-worker state that `notify_parked_local` reads). asmalloc and
   jemalloc give a 640-byte block 64-byte alignment (asmalloc's class 640 gives 128), so
   these fields always share the line. glibc's block starts after a 16-byte header at whatever
   offset earlier allocations left, which the program's own arguments decide: the same glibc
   binary runs spawn t8 at 178 ns/task started from a 101-byte path and at 235 from the main
   checkout's 62-byte path (three interleaved runs each, 177 to 182 against 233 to 237),
   presumably because `std::env::args` allocates the path ahead of everything tokio
   allocates. IBS shows the hot line split in two in the fast layout, with a third of the
   miss latency. The session's glibc results are this lottery: the good layout at t4 (99.1),
   the bad one at t8 (235.6). asmalloc could only change this by giving up alignment by
   construction.

**The ±7% on t4.** Cycles per task are flat across the spread: asmalloc's ten session runs
took 2,880 to 2,935 cycles a task whether they took 129 or 166 ns, and jemalloc's and
tcmalloc's outliers (131 to 147 ns) also kept their cycles. The clock ran slower in those
runs (about 4.4 GHz against 5.6); it is not the allocator.

**Cache-set aliasing, checked: not the cause here.** Every class region starts on a 4 GiB
boundary and fills take the lowest addresses, so the few live objects of each class sit at
the same page offsets: five in six of asmalloc's L1 misses that hit L2 fall in the first
eight of the 64 L1 sets, and nearly all its misses in the first 64 of the 1,024 L2 sets,
where jemalloc's spread. Starting each region's first carve at a different offset (scratch
build) spread them and removed the L2 hits, and changed neither time nor transfers: a real
effect, too small to matter here.

**What would fix it.** Not the allocator, short of tuning its classes to tokio's structure
sizes: which types share a class decides whether these headers interleave, and a coarser
class would move the problem to other programs and cost memory everywhere. tokio could pad
`queue::Inner` and move the task-list counter away from the fields it reads. Until then the
spawn jobs rank where each allocator puts these few lines more than what it costs to
allocate, and glibc's result there changes with the program's path.
