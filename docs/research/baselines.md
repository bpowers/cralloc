# Allocator baselines on workloads

Measured 2026-10-19 on a quiet machine, at commit `e96086d`, with the suite in `bench/`
(commands and method in `bench/README.md`): asmalloc against glibc, mimalloc (the newest
release called as the `mimalloc` crate calls it, the same with the size passed back to free,
and the `mimalloc` crate as shipped), jemalloc (`tikv-jemallocator`) and tcmalloc (per-CPU
and per-thread), on every workload of the full suite, ten rounds. This replaces the session
at `a92d51a` the day before; section 4 says what moved between the two and why. This session's
runner warmed the package's clock before every run (section 16), which the earlier sessions'
did not.

| Session | Raw results | Per-workload tables (every allocator, spread, memory, faults, clock) |
|---|---|---|
| `e96086d`, 2026-10-19, this note | `docs/research/data/baselines-2026-10-19-e96086d.jsonl.gz`; `bench/results/full-2026-10-19-e96086d.jsonl` in the main checkout | `docs/research/data/baselines-2026-10-19-e96086d-detail.md` |
| `a92d51a`, 2026-10-18, the previous report | `docs/research/data/baselines-2026-10-18-a92d51a.jsonl.gz`; `bench/results/full-2026-10-18-a92d51a.jsonl` | `docs/research/data/baselines-2026-10-18-a92d51a-detail.md` |
| `b5d6e8f`, 2026-10-18 | `docs/research/data/baselines-2026-10-18.jsonl.gz`; `bench/results/full-2026-10-18-b5d6e8f.jsonl` | `docs/research/data/baselines-2026-10-18-detail.md` |
| `60ce673`, 2026-10-17, busy machine, no asmalloc | `docs/research/data/baselines-2026-10-17.jsonl.gz` | `docs/research/data/baselines-2026-10-17-detail.md` |

`benchrun report` reads the raw results after `gunzip`.

## 1. Summary

Time, asmalloc against the best incumbent on each job (a gap is the median of per-round
ratios; "level" means the 95% interval includes no difference). asmalloc is ahead on 33 of
the 56 jobs, level on 5 and behind on 17, with xmalloc-test, which runs in two modes, apart; at
`a92d51a` it was 33, 3 and 19. Over those 55 jobs the geometric mean of asmalloc's time over
the best incumbent's fell from 0.889 to 0.865.

- **Cross-thread, small objects:** ahead on every job, 5% (eight threads in a pipeline) to
  49%, and 39 to 77% ahead of mimalloc. Independent pipelines on different L3 domains are 33
  to 48% ahead.
- **Single-threaded churn, 100 to 10M live objects:** ahead 9 to 34%.
- **One Vec grown to 256 MiB:** 48% ahead. **Box tree:** level with mimalloc (3% ahead at
  `a92d51a`; the difference is where the workload's own code landed, section 4).
- **Work stealing:** rayon 20% ahead at four threads, 2% at eight.

Where asmalloc is behind (section 9 has the causes):

- **Large objects (4 to 256 KiB) crossing threads:** 58.9 ns/alloc against per-thread
  tcmalloc's 39.0 (51% behind) and mimalloc's 41.7 (41%); jemalloc takes 300. It was 62.2
  against 35.5.
- **One 64 KiB and one 1 MiB block allocated and freed (`largepair`):** glibc takes 8.0 ns a
  pair from the top of its heap. asmalloc's 50.8 is now second of eight: 14 and 18% ahead of
  the two tcmallocs, 45% ahead of mimalloc.
- **64 Vecs grown to 4 MiB together:** 24% behind mimalloc (4.9 against 3.9 cycles/element),
  24% ahead of per-CPU tcmalloc and 42% ahead of jemalloc.
- **Build and drop of maps:** HashMap 24% behind the mimalloc crate (18% behind mimalloc),
  BTreeMap 6% (5%). JSON 2% behind the crate (1% behind mimalloc); 7% behind preloaded.
- **tokio spawn:** 31% behind glibc at four threads (9% behind mimalloc), 12% behind jemalloc
  at eight (4% behind mimalloc). The time is false sharing in tokio's own structures, not work
  in the allocator, and glibc's lead is luck of placement (section 15).
- **RPC log server:** 2 to 3% behind on server cycles per request, level with mimalloc; the
  control differs from mimalloc by 2% on these jobs (section 2). The echo server is level.
- **C programs, preloaded:** sh8bench 2.0 times mimalloc (0.51 against 0.26 s), now level with
  jemalloc; larson 14 to 16% behind mimalloc, a gap accepted for now (C++ programs reach the
  allocator through libstdc++, and exporting the C++ operators is deferred); mstress 5%, cfrac
  2%, espresso under 1%; cache-scratch level. xmalloc-test is bimodal (section 7).

Memory (cgroup peak):

- **Against jemalloc, the bar:** at or under it on every job of 10 MiB or more but two:
  HashMap, 11% over (204 against 184 MiB), because the tables a map outgrows stay resident for
  its next build; and churn with 100k live objects, 3% (0.3 MiB) natively, level preloaded.
- **Small heaps (under 14 MiB):** 0.2 to 4.2 MiB over glibc where glibc is the smallest, the
  most on `rpc log c256` (13.8 against 9.6 MiB, up from 11.7; section 4). Against jemalloc,
  from 1.4 MiB over (the eight-thread pipeline) to 2.0 under (exchange, eight threads) and 6.0
  under (the large-object pipeline).
- **Against the smallest incumbent** the largest gap on a big heap is larson, 40 to 42% over
  per-thread tcmalloc (75 to 76 against 53 to 54 MiB), and 14 to 17% under jemalloc. It was
  72 MiB at `a92d51a`; stock holds the difference (section 4).
- **After the peak** (measured at `b5d6e8f`, section 8): the lowest of the three measured.

## 2. Machine, versions and method

AMD Ryzen 9 9950X (Zen 5), 16 cores with SMT, two L3 domains (CPUs 0-7 with siblings 16-23,
8-15 with 24-31), 60 GiB, Linux 7.2.9-201.fc44, glibc 2.43, rustc 1.99.1, GCC 16.2.1.
Governor `powersave`, energy preference `balance_performance`, boost on, transparent huge
pages `madvise` (defrag `madvise`), NMI watchdog on. The same as at `a92d51a`, with the same
baseline manifests.

| Allocator | Version and build | Native (Rust `#[global_allocator]`) | Preload |
|---|---|---|---|
| asmalloc | `e96086d` | `asmalloc::Asmalloc`, `executable` feature, fat LTO | `asmalloc-preload` (C names only) |
| glibc | 2.43 | `std::alloc::System` | none |
| mimalloc | v3.5.3 (`31d034d9`), `src/static.c` at `-O3` as libmimalloc-sys builds it | as the `mimalloc` crate calls it | cmake Release shared library |
| mimalloc-sized | the same library | the same, freeing with `mi_free_size_aligned` (mimalloc v3 ignores the size in release builds, so only the call path differs) | not run |
| mimalloc-crate | the `mimalloc` crate 0.1.52 as shipped (bundles 3.3.2) | itself | not run |
| jemalloc | `tikv-jemallocator` 0.6.1 (jemalloc 5.3.0-1-ge13ca993, static, prefixed, no stats or profiling) | itself | Fedora 5.3.0-14 shared library |
| tcmalloc-percpu | google/tcmalloc `f3c6669a`, bazel 8.8.0 `-c opt`, initial-exec TLS | as `tcmalloc-better` 0.1.19 calls it | shared library; both with `GLIBC_TUNABLES=glibc.pthread.rseq=0` |
| tcmalloc-thread | the same | the same | the same, without the tunable |
| mimalloc-ctl | mimalloc built a second time: the control | | |

`bench/baselines/build.sh` builds the C libraries and records versions and flags in the
manifests, which every result file carries.

**Method.** One fresh process per run; each round runs every job under every allocator in a
new random order. Ten rounds, 56 jobs, 4,800 runs, none invalid, in 97 minutes. Threads are
pinned one per physical core on CPUs 0-3 and 8-11 (single-threaded jobs on CPU 2), the
process confined to its CPUs before `execve`, and every run under a per-run memory cap (4 GiB)
inside a capped session scope; no run reached its cap. Every run reported the allocator,
mode, library and version it was meant to have, and no run's counters were multiplexed.
Single-threaded jobs rank by cycles per operation (user and kernel), multithreaded ones by
wall time per allocation, C programs by their own throughput or wall time; memory ranks by
each run's cgroup peak. Cross-thread consumers check every object's header, so a block handed
out twice fails the run.

**Quiet.** No other work ran during the session: the quiet gate (nothing on the measurement
CPUs or their siblings, at most one CPU's worth elsewhere) passed before every run without
waiting, its one-second look seeing at most 0.06 CPUs' worth on the measurement CPUs, 0.05 on
their siblings and 0.05 elsewhere, and no run was left out as busy.

**Clock.** Before each run the runner spun the job's CPUs until every one held the CPU set's
warm clock (section 16). Every warm-up reached it, in 15 ms at the median and 24 ms at the
90th percentile, plus 0.8 s the seven times a CPU set was first used. Four of the 4,800 runs
(three of `mpmc p4 c4`, one of `mpmc p1 c3`) ran more than 5% below their allocator's median
clock on the job, against 665 at `a92d51a`, whose runner did not warm the clock.

**Control.** mimalloc-ctl against mimalloc, paired by round: the largest median difference
is 4.5% (larson-sized, interval -8.3% to -0.7%); 7 of 56 intervals exclude zero where chance
predicts 2.8. Of the other six, the RPC log jobs differ by -2.1% and -2.4%, as they did by
-1.8% and -1.4% at `a92d51a`, and the rest (`pipeline t2 x2` split, the mixed exchange,
eight-thread spawn, JSON) by under 2%. Single-threaded differences of 1% and
cross-thread differences of a few percent are resolved; on the RPC log jobs, differences of
2% are not.

**The rseq offset.** `benchrun run` records where each binary's copy of glibc's
`__rseq_offset` lies (`zen5-roofline.md`, section 16). The copies the routines load on every
call are clear of the slab hot words' page offsets in all three binaries; in the workload
binary, the GOT slot that only the out-of-line routines use is at the 256-byte class's.

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
| pipeline t2 sizes=s64 local all-alloc (native) | ns/alloc | 3.757 | jemalloc 6.223 | ahead 40% | ahead 73% | 1.2 / glibc 1.0 | behind 24% | level (-7%) |
| pipeline t2 sizes=small local all-alloc (native) | ns/alloc | 3.759 | tcmalloc-percpu 6.617 | ahead 43% | ahead 76% | 2.0 / glibc 1.2 | behind 60% | behind 12% |
| pipeline t2 sizes=mixed local all-alloc (native) | ns/alloc | 11.18 | tcmalloc-percpu 12.44 | ahead 10% | ahead 45% | 4.7 / tcmalloc-thread 3.1 | behind 58% | ahead 8% |
| pipeline t2 sizes=small local free-only (native) | ns/alloc | 3.752 | tcmalloc-percpu 6.639 | ahead 43% | ahead 75% | 2.0 / glibc 1.2 | behind 60% | behind 15% |
| pipeline t2 sizes=large objects=1M inflight=512 local all-alloc (native) | ns/alloc | 58.91 | tcmalloc-thread 38.99 | behind 51% | behind 41% | 2.8 / tcmalloc-thread 3.8 | ahead 27% | ahead 68% |
| pipeline t2 sizes=small split all-alloc (native) | ns/alloc | 12.03 | jemalloc 18.36 | ahead 34% | ahead 77% | 2.5 / glibc 1.0 | behind 154% | behind 39% |
| pipeline t2 sizes=small split free-only (native) | ns/alloc | 12.03 | tcmalloc-percpu 23.64 | ahead 49% | ahead 77% | 2.5 / glibc 1.0 | behind 160% | behind 54% |
| pipeline t4 sizes=small local all-alloc (native) | ns/alloc | 3.006 | jemalloc 3.587 | ahead 16% | ahead 70% | 4.2 / glibc 2.6 | behind 56% | behind 11% |
| pipeline t8 sizes=small both all-alloc (native) | ns/alloc | 4.181 | jemalloc 4.378 | ahead 5% | ahead 62% | 8.0 / glibc 5.8 | behind 39% | behind 21% |
| pipeline t2 x2 sizes=small local all-alloc (native) | ns/alloc per chain | 4.069 | jemalloc 7.774 | ahead 48% | ahead 76% | 3.5 / glibc 2.0 | behind 74% | behind 7% |
| pipeline t2 x2 sizes=small split all-alloc (native) | ns/alloc per chain | 4.585 | jemalloc 8.064 | ahead 43% | ahead 74% | 3.2 / glibc 2.0 | behind 62% | level (+0%) |
| pipeline t2 x3 sizes=small both all-alloc (native) | ns/alloc per chain | 5.305 | jemalloc 8.059 | ahead 34% | ahead 70% | 4.6 / glibc 3.1 | behind 52% | level (+0%) |
| pipeline t2 x4 sizes=small both all-alloc (native) | ns/alloc per chain | 5.405 | jemalloc 8.110 | ahead 33% | ahead 70% | 5.8 / glibc 3.9 | behind 48% | level (-2%) |
| exchange t2 sizes=small local (native) | ns/alloc | 4.152 | tcmalloc-percpu 6.918 | ahead 40% | ahead 66% | 2.2 / glibc 1.8 | behind 25% | ahead 9% |
| exchange t2 sizes=mixed local (native) | ns/alloc | 8.057 | tcmalloc-percpu 9.075 | ahead 11% | ahead 47% | 4.8 / tcmalloc-percpu 3.8 | behind 24% | ahead 20% |
| exchange t2 sizes=small split (native) | ns/alloc | 13.76 | jemalloc 18.65 | ahead 26% | ahead 66% | 2.8 / glibc 1.3 | behind 112% | behind 23% |
| exchange t4 sizes=small local (native) | ns/alloc | 2.175 | tcmalloc-percpu 3.677 | ahead 41% | ahead 65% | 4.0 / glibc 3.2 | behind 25% | ahead 8% |
| exchange t8 sizes=small both (native) | ns/alloc | 2.626 | jemalloc 3.962 | ahead 34% | ahead 66% | 6.9 / tcmalloc-percpu 5.3 | behind 31% | ahead 22% |
| mpmc p1 c3 sizes=small local all-alloc (native) | ns/alloc | 4.947 | jemalloc 7.549 | ahead 34% | ahead 76% | 3.5 / glibc 1.5 | behind 123% | behind 53% |
| mpmc p1 c3 sizes=small local free-only (native) | ns/alloc | 5.000 | tcmalloc-percpu 8.476 | ahead 41% | ahead 76% | 3.2 / glibc 1.5 | behind 103% | behind 42% |
| mpmc p3 c1 sizes=small local all-alloc (native) | ns/alloc | 4.665 | tcmalloc-percpu 7.097 | ahead 34% | ahead 39% | 3.5 / glibc 2.1 | behind 70% | behind 12% |
| mpmc p3 c1 sizes=small local free-only (native) | ns/alloc | 4.683 | tcmalloc-percpu 7.062 | ahead 34% | ahead 39% | 3.6 / glibc 2.0 | behind 78% | behind 23% |
| mpmc p4 c4 sizes=small both all-alloc (native) | ns/alloc | 4.635 | jemalloc 7.592 | ahead 39% | ahead 75% | 6.0 / glibc 3.1 | behind 96% | behind 28% |
| forkjoin t4 local (native) | ns/alloc | 3.497 | tcmalloc-percpu 4.393 | ahead 20% | ahead 39% | 15.5 / tcmalloc-percpu 16.1 | ahead 4% | ahead 50% |
| spawn t4 local (native) | ns/task | 130.2 | glibc 99.52 | behind 31% | behind 9% | 2.5 / glibc 1.7 | behind 57% | behind 10% |
| forkjoin t8 both (native) | ns/alloc | 3.869 | jemalloc 3.934 | ahead 2% | ahead 48% | 17.7 / tcmalloc-percpu 17.4 | behind 1% | ahead 22% |
| spawn t8 both (native) | ns/task | 254.2 | jemalloc 225.9 | behind 12% | behind 4% | 4.9 / glibc 3.0 | behind 59% | behind 11% |
| rpc echo c64 (native) | server cycles/req | 45699 | tcmalloc-percpu 45709 | level (-0%) | ahead 2% | 5.8 / glibc 3.3 | behind 75% | level (-1%) |
| rpc log c64 (native) | server cycles/req | 123907 | mimalloc-sized 121804 | behind 2% | level (+0%) | 8.4 / glibc 5.2 | behind 62% | ahead 11% |
| rpc log c256 (native) | server cycles/req | 121103 | mimalloc-sized 117984 | behind 3% | level (+1%) | 13.8 / glibc 9.6 | behind 47% | level (-4%) |
| churn live=100 small (native) | cycles/op | 22.64 | jemalloc 33.91 | ahead 33% | ahead 50% | 0.8 / glibc 0.5 | behind 64% | behind 32% |
| churn live=1k small (native) | cycles/op | 23.24 | jemalloc 35.18 | ahead 34% | ahead 49% | 0.8 / glibc 0.6 | behind 47% | level (+1%) |
| churn live=10k small (native) | cycles/op | 28.00 | jemalloc 39.65 | ahead 29% | ahead 44% | 2.2 / jemalloc 1.7 | behind 29% | behind 29% |
| churn live=100k small (native) | cycles/op | 44.09 | jemalloc 52.84 | ahead 17% | ahead 32% | 12.0 / jemalloc 11.7 | behind 3% | behind 3% |
| churn live=1M small (native) | cycles/op | 104.7 | jemalloc 119.1 | ahead 12% | ahead 42% | 110.2 / jemalloc 110.2 | level (+0%) | level (+0%) |
| churn live=10M small (native) | cycles/op | 214.3 | jemalloc 245.9 | ahead 13% | ahead 49% | 1069.5 / jemalloc 1091.5 | ahead 2% | ahead 2% |
| churn live=100k mixed (native) | cycles/op | 85.28 | tcmalloc-percpu 93.82 | ahead 9% | ahead 12% | 34.2 / tcmalloc-thread 34.5 | ahead 1% | ahead 19% |
| map btree 1M (native) | cycles/entry | 913.1 | mimalloc-crate 856.7 | behind 6% | behind 5% | 123.2 / jemalloc 125.2 | ahead 2% | ahead 2% |
| map hash 1M (native) | cycles/entry | 543.6 | mimalloc-crate 441.5 | behind 24% | behind 18% | 204.2 / jemalloc 183.6 | behind 11% | behind 11% |
| tree depth=20 (native) | cycles/node | 44.56 | mimalloc-crate 44.37 | level (+1%) | level (+0%) | 50.2 / glibc 64.5 | ahead 22% | ahead 25% |
| vecgrow 1x256Mi (native) | cycles/element | 2.158 | mimalloc-crate 4.149 | ahead 48% | ahead 78% | 258.5 / glibc 257.0 | behind 1% | ahead 2% |
| vecgrow 64x4Mi (native) | cycles/element | 4.882 | mimalloc-sized 3.944 | behind 24% | behind 24% | 277.5 / mimalloc 265.0 | behind 5% | ahead 10% |
| json 32Mi (native) | cycles/byte | 16.63 | mimalloc-crate 16.25 | behind 2% | behind 1% | 353.7 / mimalloc-crate 355.2 | ahead <1% | ahead 2% |
| largepair 64Ki+1Mi (native) | ns/pair | 50.78 | glibc 8.025 | behind 533% | ahead 45% | 0.8 / glibc 0.2 | behind 199% | behind 12% |
| churn live=100k small (preload) | cycles/op | 51.58 | mimalloc 51.21 | level (+0%) | level (+0%) | 12.0 / jemalloc 12.0 | level (+0%) | level (+0%) |
| pipeline t2 sizes=small local all-alloc (preload) | ns/alloc | 3.966 | tcmalloc-percpu 6.305 | ahead 37% | ahead 72% | 2.0 / glibc 1.2 | behind 60% | behind 5% |
| json 32Mi (preload) | cycles/byte | 17.29 | mimalloc 16.22 | behind 7% | behind 7% | 354.5 / jemalloc 359.6 | ahead 1% | ahead 1% |
| larson-sized 8 (preload) | Mops/s | 467.5 | mimalloc 536.7 | behind 16% | behind 16% | 76.5 / tcmalloc-thread 53.9 | behind 42% | ahead 14% |
| larson 8 (preload) | Mops/s | 453.4 | mimalloc 518.9 | behind 14% | behind 14% | 75.1 / tcmalloc-thread 53.3 | behind 40% | ahead 17% |
| xmalloc-test 4+4 (preload) | Mfrees/s | 727.7 | tcmalloc-percpu 556.6 | bimodal (section 7) | ahead 59% | 36.1 / tcmalloc-thread 33.9 | bimodal | ahead 38% |
| sh8bench 16 (preload) | s | 0.5123 | mimalloc 0.2573 | behind 102% | behind 102% | 128.7 / tcmalloc-percpu 119.7 | behind 7% | ahead 21% |
| mstress 8 (preload) | s | 0.2592 | mimalloc 0.2498 | behind 5% | behind 5% | 139.9 / jemalloc 145.0 | level (-3%) | level (-3%) |
| cache-scratch 1 (preload) | s | 0.5176 | glibc 0.5022 | level (+3%) | level (+1%) | 1.0 / glibc 0.8 | level (+33%) | ahead 20% |
| cache-scratch 8 (preload) | s | 0.0670 | mimalloc 0.0677 | level (-0%) | level (-0%) | 2.8 / glibc 2.5 | behind 10% | behind <1% |
| cfrac (preload) | s | 2.099 | mimalloc 2.063 | behind 2% | behind 2% | 1.7 / glibc 1.4 | behind 21% | behind 17% |
| espresso (preload) | s | 2.239 | mimalloc 2.229 | behind <1% | behind <1% | 2.5 / glibc 1.3 | behind 90% | behind 40% |

## 4. What moved since `a92d51a`, and why

Two things changed besides the allocator, and both move numbers:

- **The clock.** This session's runner warmed the package's clock before every run (section
  16). At `a92d51a`, 665 of the 4,800 runs ran more than 5% below their allocator's usual
  clock on the job, most at the 4.4 GHz nominal clock. That moved jobs ranked by wall time,
  and also jobs ranked by cycles that wait on memory, whose cycles grow with the clock: on
  `vecgrow 64x4Mi` mimalloc's runs at `a92d51a` took 3.05 cycles an element at 4.38 GHz and
  3.94 at 5.71, and every allocator ran `map btree` at 4.9 to 5.1 GHz then and at 5.67 now,
  and takes 3 to 6% more cycles an entry. Where the old session's runs were cold, the moves
  below are also given "at the same clock": against the old runs within 5% of the
  allocator's median clock on the job in this session.
- **The workloads were rebuilt**, so jobs whose time depends on where their code landed
  (section 14: JSON and the tree) can move by a few percent with no change to the allocator.

At the same clock, each incumbent's median change over the jobs is under 0.2%, and
asmalloc's -0.8%. The incumbents' largest moves on one job at the same clock: per-thread
tcmalloc 10% slower on the large-object pipeline, the mimalloc crate 8% slower on the 64-byte
pipeline, mimalloc-sized 6% slower on two exchanges, and on sh8bench mimalloc 5% faster and
jemalloc 5% slower; glibc's larson, eight-thread cache-scratch and xmalloc-test moved by 16 to
36% because it runs them in two modes, in other proportions this time. Causes of asmalloc's
moves come from the notes that measured each change, before and after, on its own; where none
did, the move is not attributed.

| Job | `a92d51a` | `e96086d` | Against the best, then and now | What moved it |
|---|---:|---:|---|---|
| `largepair`, ns/pair | 67.46 | 50.78 | behind 735%, behind 533% (glibc); now ahead of both tcmallocs | A shorter large-object path (`2cdccf4`): four atomic read-modify-writes a pair instead of eight, and a fifth fewer instructions. Measured 66.6 to 48.2 ns (`memory.md`, "The cost of a large block, by cause"). |
| Large objects crossing threads, ns/alloc | 62.21 | 58.91 | behind 76%, behind 51% | The same change, and the retained count split into two tallies on lines of their own: 702 to 656 cycles an allocation there; 5% here at the same clock. The best, per-thread tcmalloc, is 10% slower than at `a92d51a` at the same clock (35.4 to 38.9 ns), not explained; that is the rest of the narrower gap. |
| `vecgrow 64x4Mi`, cycles/element; faults | 5.891; 185k | 4.882; 57k | behind 86% (about 63% at the same clock), behind 24% | A block that grows over its own spilled huge page faults it whole (`3bf8818`, reviewed in `233b971`): 6.70 to 5.02 cycles and 184,897 to 56,377 faults in the note's runs (`memory.md`, "Growing blocks fault their spilled huge page in small pages"). At the same clock asmalloc went from about 6.4 to 4.9. The rest of the narrower gap is the clock: mimalloc-sized, the best, ran at 4.4 GHz in six of its ten runs at `a92d51a`. |
| sh8bench 16, s; cgroup peak | 0.597; 120 MiB | 0.512; 129 MiB | behind 124%, behind 102%; jemalloc 19% ahead then, level now | Stock (`4a77350`, with the review's fixes `83d20cb` and `94b5b44`): a CPU keeps, within a budget, the magazines it sends to a contended depot and takes back. Measured 0.577 to 0.523 s and 121 to 132 MiB (`launder-and-fill.md`, "Stock"). |
| larson, larson-sized, Mops/s; cgroup peak | 433.6, 426.9; 72 MiB | 453.4, 467.5; 75, 76 MiB | behind 16, 25%; behind 14, 16% | Stock: measured +2 to 5% on larson and +4% on larson-sized, peaks 72 to 74 or 75 MiB (same section). larson-sized's further 5% is not attributed. |
| `churn live=10k`, cycles/op; espresso, s; `rpc echo c64`, server cycles/req | 31.48; 2.316; 46816 | 28.00; 2.239; 45699 | ahead 21%, ahead 29%; behind 4%, behind under 1%; behind 3%, level | Small classes start on pages of their own (`a76b222`, section 15), measured at -11.6%, -3.3% and -2.3%: their first pages no longer share the data TLB's sets. |
| Cross-thread: pipeline t4, exchange t4 and t8, mpmc p1 c3 and p3 c1 (both shapes), ns/alloc | 3.172; 2.350, 2.996; 5.237, 5.361; 4.842, 4.876 | 3.006; 2.175, 2.626; 4.947, 5.000; 4.665, 4.683 | ahead in both sessions | 3.5 to 6% faster at the same clock (exchange t8 12% in all runs, half of it the clock). Not attributed. |
| Churn, 1k, 100k and 100k mixed sizes, cycles/op | 23.97, 46.51, 87.47 | 23.24, 44.09, 85.28 | ahead in both, by 2 to 5 points more now | 3.1, 5.2 and 2.5% fewer cycles. Not attributed. |
| Split pipeline, both shapes, ns/alloc | 11.54, 12.54 | 12.03, 12.03 | ahead 38, 51%; ahead 34, 49% | 4.6 and 4.4% slower at the same clock; all-alloc takes 5% more cycles and 0.9% more instructions. Six of asmalloc's ten free-only runs at `a92d51a` ran at 4.4 GHz, so its median then (12.54) was a cold run's, and part of the 14% that session reported for it (section 13) was the clock. It came with the classes' origins (`a76b222`): 11.45 and 11.42 ns before, 12.00 and 12.08 after, at the same instructions and with fifty times fewer page walks. The mechanism is not found (same note as the tree's row). |
| `tree depth=20`, cycles/node | 43.25 | 44.56 | ahead 3%, level | 3.0% more cycles, with 1.1% more instructions. Where the workload's code landed: all the added cycles are in the walk, which runs no allocator code; its 72-byte recursive function began a 64-byte line in this session's build, which leaves its return alone in the next line. With every function 64-aligned the two commits run 44.21 and 44.25. The instructions are stock's and the frontier's bookkeeping on each magazine exchange and fill, 0.1% of the job's cycles (`launder-and-fill.md`, "The tree and JSON jobs between `a92d51a` and `e96086d`"). |
| JSON 32 MiB, cycles/byte, native; preloaded | 16.19; 17.12 | 16.63; 17.29 | behind 1, 5%; behind 2, 7% | 2.7 and 1.0% more cycles at 0.4 and 0.5% more instructions. An inlining decision in the workload: with the lookup tables moved 2048 bytes into their pages (`c3c4676`), LLVM inlines the free at more of serde_json's call sites and `Value::deserialize` grows by a tenth; the same commit without the padding has the old code and the old time. The padding keeps fast-path loads off the hot words' page offsets and stays (same note). |
| `mpmc p4 c4`, ns/alloc | 4.991 | 4.635 | ahead 41%, ahead 39% | The clock: six of asmalloc's ten runs then ran at 4.4 to 4.8 GHz. Its three runs then at 5.59 GHz took 4.41 to 4.49 ns, against 4.55 to 4.65 now at the same clock: about 3% slower. Not attributed. |
| `rpc log c256`, cgroup peak; faults | 11.7 MiB; 5,188 | 13.8 MiB; 3,258 | over glibc 23%, now 47%; under jemalloc 20%, now level | The frontier (`74df709`, `memory.md`, "The clocked pass spares what fills reach"): the clocked purge takes only free pages that no fill reached for an interval. Its note measured the faults, 5,310 to 3,060; the peak is the memory it keeps, which the note did not measure. On `rpc log c64` and `rpc echo c64`, peaks rose 0.1 to 0.2 MiB and faults fell from 2,712 to 1,852 and 1,404 to 1,138. |
| Small cross-thread heaps: mpmc p1 c3, exchange t4, pipelines t4 and t8, cgroup peak MiB | 3.1, 3.7, 4.0, 7.7 | 3.5, 4.0, 4.2, 8.0 | | Not attributed. Others fell by as much: the pipelines of two threads (2.2 to 2.0 MiB for small objects; 2.6 to 2.5 split), churn with 1k live objects (0.91 to 0.75). |

Instructions: in the measured phases, most small-object jobs retire 0.2 to 1.5% more
instructions than at `a92d51a`, and the churn jobs with 10k or fewer live objects the same. The
added instructions are stock's (60 to 80 on each magazine exchange) and the frontier's (about
100 and a compare-and-swap on each fill that reaches higher than the last); jobs that never
leave the magazines run neither.

**xmalloc-test is still bimodal**, and its medians do not compare. asmalloc landed in its fast
mode in nine of ten runs (722 to 740 Mfrees/s) against three of ten at `a92d51a` (739 to
750); the fast mode is no faster. Per-CPU tcmalloc was fast in all ten (three slow before),
and per-thread tcmalloc and glibc still run in two modes. asmalloc's peak goes with its mode,
about 36 MiB fast and 4 MiB slow, with one slow run in each session peaking at 34 and 23 MiB,
so its median peak rose from 4.4 to 36 MiB because the median run's mode changed. Why it
landed fast more often is not attributed; stock measured this program at
1,137 to 1,166 against 739 to 753 Mfrees/s on one L3 (`launder-and-fill.md`, "Stock").

## 5. Multithreaded

Medians of ten.

Cross-thread, time:

| Job | Metric | asmalloc | glibc | mimalloc | mi sized | mi crate | jemalloc | tc per-CPU | tc per-thread |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| pipeline t2 sizes=s64 local all-alloc | ns/alloc | 3.757 | 94.49 | 13.94 | 14.25 | 16.75 | 6.223 | 9.584 | 10.18 |
| pipeline t2 sizes=small local all-alloc | ns/alloc | 3.759 | 125.2 | 15.62 | 15.04 | 17.47 | 7.658 | 6.617 | 10.16 |
| pipeline t2 sizes=mixed local all-alloc | ns/alloc | 11.18 | 133.3 | 20.18 | 20.26 | 21.68 | 16.94 | 12.44 | 16.52 |
| pipeline t2 sizes=small local free-only | ns/alloc | 3.752 | 126.5 | 15.32 | 15.27 | 17.95 | 22.24 | 6.639 | 10.11 |
| pipeline t2 sizes=large objects=1M inflight=512 local all-alloc | ns/alloc | 58.91 | 321.9 | 41.71 | 41.59 | 40.13 | 300.1 | 42.21 | 38.99 |
| pipeline t2 sizes=small split all-alloc | ns/alloc | 12.03 | 124.9 | 51.18 | 49.60 | 65.50 | 18.36 | 24.20 | 29.04 |
| pipeline t2 sizes=small split free-only | ns/alloc | 12.03 | 123.7 | 50.70 | 51.10 | 65.56 | 50.43 | 23.64 | 28.99 |
| pipeline t4 sizes=small local all-alloc | ns/alloc | 3.006 | 33.16 | 9.874 | 9.975 | 10.12 | 3.587 | 4.125 | 6.850 |
| pipeline t8 sizes=small both all-alloc | ns/alloc | 4.181 | 49.07 | 11.08 | 11.69 | 11.67 | 4.378 | 5.918 | 7.834 |
| pipeline t2 x2 sizes=small local all-alloc | ns/alloc per chain | 4.069 | 132.4 | 16.94 | 17.08 | 18.65 | 7.774 | 7.881 | 12.49 |
| pipeline t2 x2 sizes=small split all-alloc | ns/alloc per chain | 4.585 | 141.8 | 17.26 | 17.50 | 20.69 | 8.064 | 22.37 | 29.23 |
| pipeline t2 x3 sizes=small both all-alloc | ns/alloc per chain | 5.305 | 142.0 | 17.43 | 17.62 | 21.18 | 8.059 | 20.91 | 28.55 |
| pipeline t2 x4 sizes=small both all-alloc | ns/alloc per chain | 5.405 | 140.2 | 17.71 | 17.90 | 19.98 | 8.110 | 24.09 | 33.78 |
| exchange t2 sizes=small local | ns/alloc | 4.152 | 45.25 | 12.02 | 12.77 | 12.77 | 7.619 | 6.918 | 11.88 |
| exchange t2 sizes=mixed local | ns/alloc | 8.057 | 69.81 | 15.37 | 15.60 | 15.36 | 10.04 | 9.075 | 12.75 |
| exchange t2 sizes=small split | ns/alloc | 13.76 | 129.5 | 40.43 | 42.40 | 43.91 | 18.65 | 24.69 | 32.86 |
| exchange t4 sizes=small local | ns/alloc | 2.175 | 24.39 | 6.188 | 6.463 | 6.703 | 3.987 | 3.677 | 6.844 |
| exchange t8 sizes=small both | ns/alloc | 2.626 | 22.42 | 7.742 | 7.972 | 9.083 | 3.962 | 5.520 | 7.841 |
| mpmc p1 c3 sizes=small local all-alloc | ns/alloc | 4.947 | 100.1 | 20.41 | 20.59 | 21.47 | 7.549 | 8.488 | 9.625 |
| mpmc p1 c3 sizes=small local free-only | ns/alloc | 5.000 | 101.8 | 20.57 | 20.45 | 21.68 | 20.19 | 8.476 | 9.580 |
| mpmc p3 c1 sizes=small local all-alloc | ns/alloc | 4.665 | 77.54 | 7.689 | 8.028 | 8.945 | 10.29 | 7.097 | 11.73 |
| mpmc p3 c1 sizes=small local free-only | ns/alloc | 4.683 | 77.83 | 7.670 | 7.954 | 8.909 | 21.00 | 7.062 | 11.77 |
| mpmc p4 c4 sizes=small both all-alloc | ns/alloc | 4.635 | 70.39 | 18.50 | 19.11 | 22.24 | 7.592 | 9.092 | 11.11 |

Cross-thread, cgroup peak, MiB:

| Job | asmalloc | glibc | mimalloc | mi sized | mi crate | jemalloc | tc per-CPU | tc per-thread |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| pipeline t2 sizes=s64 local all-alloc | 1.250 | 1.031 | 4.750 | 4.750 | 4.750 | 1.381 | 2.500 | 2.250 |
| pipeline t2 sizes=small local all-alloc | 1.994 | 1.250 | 4.750 | 4.750 | 6.746 | 1.805 | 2.787 | 2.562 |
| pipeline t2 sizes=mixed local all-alloc | 4.748 | 3.297 | 8.750 | 8.750 | 10.39 | 5.088 | 3.750 | 3.055 |
| pipeline t2 sizes=small local free-only | 1.994 | 1.250 | 4.750 | 4.750 | 6.750 | 1.742 | 2.811 | 2.500 |
| pipeline t2 sizes=large objects=1M inflight=512 local all-alloc | 2.750 | 34.18 | 65.88 | 69.89 | 57.69 | 8.754 | 4.250 | 3.750 |
| pipeline t2 sizes=small split all-alloc | 2.500 | 1.000 | 4.750 | 4.750 | 8.750 | 1.803 | 2.779 | 2.561 |
| pipeline t2 sizes=small split free-only | 2.506 | 1.000 | 4.750 | 4.750 | 6.750 | 1.742 | 2.750 | 2.561 |
| pipeline t4 sizes=small local all-alloc | 4.166 | 2.592 | 9.250 | 9.250 | 9.248 | 3.746 | 4.129 | 4.695 |
| pipeline t8 sizes=small both all-alloc | 8.021 | 5.781 | 14.05 | 13.25 | 14.27 | 6.600 | 6.289 | 9.086 |
| pipeline t2 x2 sizes=small local all-alloc | 3.471 | 2.016 | 5.250 | 5.250 | 9.250 | 3.246 | 3.797 | 3.496 |
| pipeline t2 x2 sizes=small split all-alloc | 3.246 | 2.000 | 5.250 | 5.250 | 9.250 | 3.246 | 3.152 | 3.523 |
| pipeline t2 x3 sizes=small both all-alloc | 4.637 | 3.057 | 8.000 | 8.686 | 8.000 | 4.742 | 4.025 | 4.480 |
| pipeline t2 x4 sizes=small both all-alloc | 5.850 | 3.930 | 10.12 | 8.375 | 10.87 | 6.004 | 4.416 | 5.469 |
| exchange t2 sizes=small local | 2.242 | 1.785 | 4.750 | 4.750 | 8.748 | 2.387 | 2.807 | 9.279 |
| exchange t2 sizes=mixed local | 4.775 | 4.650 | 12.75 | 12.75 | 14.75 | 5.867 | 3.779 | 9.150 |
| exchange t2 sizes=small split | 2.777 | 1.311 | 4.750 | 4.750 | 8.750 | 2.312 | 2.777 | 8.646 |
| exchange t4 sizes=small local | 4.000 | 3.162 | 9.250 | 8.250 | 9.250 | 4.283 | 3.740 | 15.84 |
| exchange t8 sizes=small both | 6.912 | 6.068 | 13.88 | 14.38 | 14.38 | 8.873 | 5.266 | 24.64 |
| mpmc p1 c3 sizes=small local all-alloc | 3.451 | 1.500 | 5.250 | 5.250 | 9.250 | 2.242 | 3.000 | 2.568 |
| mpmc p1 c3 sizes=small local free-only | 3.176 | 1.500 | 5.250 | 5.250 | 6.629 | 2.246 | 3.057 | 2.750 |
| mpmc p3 c1 sizes=small local all-alloc | 3.492 | 2.064 | 7.004 | 7.135 | 9.250 | 3.162 | 3.479 | 2.959 |
| mpmc p3 c1 sizes=small local free-only | 3.598 | 2.002 | 7.250 | 7.250 | 9.250 | 2.982 | 3.262 | 2.947 |
| mpmc p4 c4 sizes=small both all-alloc | 6.018 | 3.072 | 10.25 | 10.50 | 12.37 | 4.746 | 4.240 | 4.021 |

The large-object pipeline was bimodal at `a92d51a` (asmalloc seven rounds at 61.6 to 62.3 ns,
three at 79 to 81), by the clock each run started with. With the clock warmed, asmalloc's runs
take 58.7 to 59.7 ns, per-thread tcmalloc's 38.2 to 39.2.

Work stealing, time:

| Job | Metric | asmalloc | glibc | mimalloc | mi sized | mi crate | jemalloc | tc per-CPU | tc per-thread |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| forkjoin t4 local | ns/alloc | 3.497 | 10.57 | 5.787 | 5.857 | 5.703 | 4.784 | 4.393 | 5.134 |
| spawn t4 local | ns/task | 130.2 | 99.52 | 119.1 | 119.7 | 116.4 | 113.1 | 113.5 | 112.7 |
| forkjoin t8 both | ns/alloc | 3.869 | 17.79 | 7.468 | 7.499 | 8.341 | 3.934 | 5.141 | 7.118 |
| spawn t8 both | ns/task | 254.2 | 235.6 | 242.3 | 242.1 | 235.3 | 225.9 | 230.0 | 226.0 |

Work stealing, cgroup peak, MiB:

| Job | asmalloc | glibc | mimalloc | mi sized | mi crate | jemalloc | tc per-CPU | tc per-thread |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| forkjoin t4 local | 15.45 | 16.70 | 21.50 | 21.50 | 23.25 | 30.97 | 16.09 | 16.57 |
| spawn t4 local | 2.529 | 1.650 | 9.250 | 8.250 | 9.250 | 2.434 | 2.998 | 2.789 |
| forkjoin t8 both | 17.71 | 19.42 | 26.52 | 26.67 | 28.75 | 22.60 | 17.43 | 18.55 |
| spawn t8 both | 4.893 | 3.023 | 12.60 | 12.50 | 14.50 | 4.453 | 4.396 | 4.211 |

The RPC server, time:

| Job | Metric | asmalloc | glibc | mimalloc | mi sized | mi crate | jemalloc | tc per-CPU | tc per-thread |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| rpc echo c64 | server cycles/req | 45699 | 49001 | 46762 | 46352 | 46726 | 46058 | 45709 | 47429 |
| rpc log c64 | server cycles/req | 123907 | 148324 | 123711 | 121804 | 125234 | 126935 | 121859 | 128346 |
| rpc log c256 | server cycles/req | 121103 | 150281 | 120102 | 117984 | 120272 | 123984 | 118518 | 125182 |

The RPC server, cgroup peak, MiB:

| Job | asmalloc | glibc | mimalloc | mi sized | mi crate | jemalloc | tc per-CPU | tc per-thread |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| rpc echo c64 | 5.789 | 3.318 | 13.27 | 13.44 | 14.85 | 5.811 | 6.188 | 18.52 |
| rpc log c64 | 8.381 | 5.188 | 25.22 | 25.09 | 26.77 | 9.561 | 9.807 | 19.46 |
| rpc log c256 | 13.84 | 9.629 | 28.69 | 28.75 | 31.10 | 14.11 | 15.28 | 26.85 |

The RPC client is the bottleneck, so these rank the server's cycles per request.

## 6. Single-threaded

Time:

| Job | Metric | asmalloc | glibc | mimalloc | mi sized | mi crate | jemalloc | tc per-CPU | tc per-thread |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| churn live=100 small | cycles/op | 22.64 | 34.57 | 45.38 | 45.04 | 46.72 | 33.91 | 37.00 | 73.70 |
| churn live=1k small | cycles/op | 23.24 | 44.94 | 45.25 | 44.79 | 46.49 | 35.18 | 37.82 | 73.75 |
| churn live=10k small | cycles/op | 28.00 | 55.58 | 49.79 | 48.71 | 54.21 | 39.65 | 41.36 | 75.40 |
| churn live=100k small | cycles/op | 44.09 | 77.95 | 64.62 | 63.47 | 72.17 | 52.84 | 58.30 | 94.60 |
| churn live=1M small | cycles/op | 104.7 | 306.3 | 179.5 | 171.3 | 216.2 | 119.1 | 132.9 | 283.4 |
| churn live=10M small | cycles/op | 214.3 | 456.7 | 421.8 | 410.1 | 558.4 | 245.9 | 298.0 | 520.9 |
| churn live=100k mixed | cycles/op | 85.28 | 164.9 | 97.17 | 97.88 | 109.1 | 106.9 | 93.82 | 150.0 |
| map btree 1M | cycles/entry | 913.1 | 1385 | 866.4 | 860.0 | 856.7 | 982.0 | 1059 | 1256 |
| map hash 1M | cycles/entry | 543.6 | 1106 | 461.7 | 465.9 | 441.5 | 634.4 | 608.4 | 780.2 |
| tree depth=20 | cycles/node | 44.56 | 86.65 | 44.48 | 46.43 | 44.37 | 100.4 | 116.8 | 142.9 |
| vecgrow 1x256Mi | cycles/element | 2.158 | 6.790 | 9.664 | 9.674 | 4.149 | 14.57 | 11.86 | 11.88 |
| vecgrow 64x4Mi | cycles/element | 4.882 | 8.747 | 3.946 | 3.944 | 4.050 | 8.335 | 6.451 | 6.589 |
| json 32Mi | cycles/byte | 16.63 | 23.07 | 16.55 | 16.49 | 16.25 | 23.69 | 28.88 | 33.67 |
| largepair 64Ki+1Mi | ns/pair | 50.78 | 8.025 | 91.58 | 90.33 | 106.6 | 132.6 | 59.12 | 61.78 |

Cgroup peak, MiB:

| Job | asmalloc | glibc | mimalloc | mi sized | mi crate | jemalloc | tc per-CPU | tc per-thread |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| churn live=100 small | 0.811 | 0.494 | 2.520 | 2.547 | 4.500 | 0.605 | 2.125 | 1.656 |
| churn live=1k small | 0.750 | 0.592 | 4.500 | 4.500 | 4.500 | 0.746 | 2.250 | 1.750 |
| churn live=10k small | 2.250 | 1.750 | 6.547 | 6.500 | 6.500 | 1.746 | 3.279 | 2.811 |
| churn live=100k small | 12.03 | 12.52 | 14.50 | 14.50 | 16.50 | 11.75 | 13.50 | 13.03 |
| churn live=1M small | 110.2 | 120.7 | 116.8 | 116.7 | 118.8 | 110.2 | 113.8 | 113.5 |
| churn live=10M small | 1070 | 1199 | 1143 | 1143 | 1141 | 1091 | 1114 | 1114 |
| churn live=100k mixed | 34.25 | 40.56 | 54.55 | 54.57 | 56.50 | 42.49 | 35.00 | 34.50 |
| map btree 1M | 123.2 | 130.5 | 128.8 | 128.8 | 128.8 | 125.2 | 135.2 | 133.8 |
| map hash 1M | 204.2 | 208.1 | 299.0 | 299.0 | 198.8 | 183.6 | 253.7 | 253.2 |
| tree depth=20 | 50.25 | 64.50 | 96.50 | 96.50 | 96.50 | 66.75 | 68.75 | 68.42 |
| vecgrow 1x256Mi | 258.5 | 257.0 | 525.5 | 525.5 | 527.5 | 264.4 | 683.2 | 682.9 |
| vecgrow 64x4Mi | 277.5 | 265.4 | 265.0 | 265.0 | 291.0 | 307.1 | 286.5 | 296.2 |
| json 32Mi | 353.7 | 414.3 | 387.2 | 387.2 | 355.2 | 361.2 | 398.7 | 398.3 |
| largepair 64Ki+1Mi | 0.793 | 0.250 | 4.500 | 4.500 | 4.500 | 0.707 | 2.000 | 1.500 |

## 7. Programs through LD_PRELOAD

The Rust workloads built with glibc, then preloaded:

Time:

| Job | Metric | asmalloc | glibc | mimalloc | jemalloc | tc per-CPU | tc per-thread |
|---|---|---:|---:|---:|---:|---:|---:|
| churn live=100k small (preload) | cycles/op | 51.58 | 77.88 | 51.21 | 60.00 | 55.68 | 93.25 |
| pipeline t2 sizes=small local all-alloc (preload) | ns/alloc | 3.966 | 127.6 | 13.95 | 7.650 | 6.305 | 8.954 |
| json 32Mi (preload) | cycles/byte | 17.29 | 23.00 | 16.22 | 23.86 | 29.16 | 32.92 |

Cgroup peak, MiB:

| Job | asmalloc | glibc | mimalloc | jemalloc | tc per-CPU | tc per-thread |
|---|---:|---:|---:|---:|---:|---:|
| churn live=100k small (preload) | 12.01 | 12.50 | 14.50 | 12.00 | 13.50 | 13.00 |
| pipeline t2 sizes=small local all-alloc (preload) | 1.998 | 1.250 | 4.750 | 1.895 | 2.586 | 2.318 |
| json 32Mi (preload) | 354.5 | 414.3 | 387.2 | 359.6 | 418.8 | 418.6 |

mimalloc-bench programs:

Time:

| Job | Metric | asmalloc | glibc | mimalloc | jemalloc | tc per-CPU | tc per-thread |
|---|---|---:|---:|---:|---:|---:|---:|
| larson-sized 8 | Mops/s | 467.5 | 330.2 | 536.7 | 389.2 | 495.6 | 327.4 |
| larson 8 | Mops/s | 453.4 | 334.6 | 518.9 | 403.5 | 435.3 | 320.5 |
| xmalloc-test 4+4 | Mfrees/s | 727.7 | 75.26 | 297.5 | 23.84 | 556.6 | 14.29 |
| sh8bench 16 | s | 0.5123 | 1.502 | 0.2573 | 0.5275 | 1.977 | 5.118 |
| mstress 8 | s | 0.2592 | 0.3557 | 0.2498 | 0.4448 | 0.2949 | 0.3193 |
| cache-scratch 1 | s | 0.5176 | 0.5022 | 0.5022 | 0.5144 | 0.5264 | 0.5261 |
| cache-scratch 8 | s | 0.0670 | 0.0968 | 0.0677 | 0.0691 | 0.0706 | 0.0717 |
| cfrac | s | 2.099 | 2.092 | 2.063 | 2.138 | 2.175 | 2.710 |
| espresso | s | 2.239 | 2.474 | 2.229 | 2.287 | 2.265 | 2.432 |

Cgroup peak, MiB:

| Job | asmalloc | glibc | mimalloc | jemalloc | tc per-CPU | tc per-thread |
|---|---:|---:|---:|---:|---:|---:|
| larson-sized 8 | 76.48 | 90.65 | 104.6 | 88.37 | 65.52 | 53.86 |
| larson 8 | 75.09 | 90.63 | 103.5 | 89.94 | 60.78 | 53.32 |
| xmalloc-test 4+4 | 36.08 | 41.97 | 68.25 | 53.57 | 39.54 | 33.92 |
| sh8bench 16 | 128.7 | 165.2 | 211.6 | 162.6 | 119.7 | 121.9 |
| mstress 8 | 139.9 | 250.7 | 282.0 | 145.0 | 163.5 | 169.0 |
| cache-scratch 1 | 0.994 | 0.750 | 2.934 | 1.145 | 2.250 | 1.750 |
| cache-scratch 8 | 2.750 | 2.500 | 4.500 | 2.738 | 4.000 | 3.500 |
| cfrac | 1.697 | 1.445 | 4.500 | 1.496 | 2.750 | 2.250 |
| espresso | 2.500 | 1.312 | 8.518 | 1.789 | 3.000 | 2.750 |

**xmalloc-test** runs in one of two modes, by where the scheduler puts each pair of threads
(`launder-and-fill.md`, "A depot per cache domain"), so its medians say little. asmalloc: nine
runs at 722 to 740 Mfrees/s, peaking at 35.0 to 37.5 MiB, and one at 414, peaking at 23 MiB.
Per-CPU tcmalloc: all ten at 548 to 567. Per-thread tcmalloc: two runs at 6 to 7, eight at 14
to 15; glibc five at 54 to 56, five at 95 to 97. The memory median of 36 MiB is the fast
mode's. At `a92d51a` asmalloc ran seven runs slow (374 to 401, peaking at 3.7 to 4.4 MiB but
one at 34) and three fast (739 to 750, 35 to 36 MiB), and its median peak was 4.4 MiB; at
`b5d6e8f`, five of ten fast and 21 MiB. The medians compare the mode counts, not the
allocators.

asmalloc's preload library exports only the C names; C++ `new` and `delete` reach it through
libstdc++'s operators, where mimalloc's library defines them itself (section 9).

## 8. Memory after the peak

Measured 2026-10-18 with the allocator at `b5d6e8f`, on the same quiet machine as that
session, with `bench/probes/memory/` (each process confined to its CPUs and capped; medians of
two or three rounds, which agreed to within 0.1 MiB). Not repeated at `a92d51a` or
`e96086d`. The changes since that bear on it were measured on their own: the cap charged to
growth, whose after-peak curve agrees with the one below from 4 s on (`memory.md`, "The cap
charged to growth"), and the frontier, after which a burst that goes quiet keeps its memory
one interval longer (215 MiB at 1 s against 102), then is level from 2 s and lower at 12 s
(`memory.md`, "The clocked pass spares what fills reach"). Anonymous resident MiB.

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

### At `e96086d`

The gaps of the summary, with what the notes that measured them found. None was profiled
again for this session.

- **Large objects crossing threads** (58.9 against per-thread tcmalloc's 39.0 ns/alloc; 656
  against 402 cycles an allocation in the note's runs). Half the producer's time is in large
  claims, mostly waiting for the leaf word's line, which the consumer's free has just written;
  a third is the small classes' magazine exchanges, since a magazine of 12 to 32 KiB objects
  holds two. Each large object moves its leaf line between the cores twice, and every second
  small one is an exchange with the depot. tcmalloc moves objects up to 256 KiB between
  threads in batches. Bigger magazines for those classes measured 530 cycles (8 slots) and
  466 (16) for up to 0.6 and 1.9 MiB more parked per CPU; not built (`memory.md`, "The cost
  of a large block, by cause").
- **`largepair`** (51 ns a pair against glibc's 8). glibc serves both blocks from the top of
  its heap. asmalloc's 271 cycles a pair (in the note's runs) against a floor of about 140 for
  this design are about a thousand instructions of layering above the claim and the free
  (same section); asmalloc is ahead of every other incumbent.
- **64 Vecs grown to 4 MiB** (4.9 against mimalloc's 3.9 cycles/element; 57k faults against
  286). What is left is faulting back what was returned: 56k small faults on the outgrown
  1 MiB blocks, which a move returns once the budget for kept grown blocks is spent, and two
  huge-page faults per vector and repetition. A budget of 64 MiB instead of 16 measured 4.07
  cycles an element and 327 MiB of peak, over jemalloc's 305 (`memory.md`, "Growing blocks
  fault their spilled huge page in small pages").
- **HashMap** (24% behind the mimalloc crate; 61k faults against 255), as at `a92d51a` below.
  Huge pages for every large block of 2 MiB or more at allocation closed both the time and the
  memory gap in a scratch build (467 cycles an entry, 181 MiB), at the cost of making a sparse
  block of that size wholly resident; not adopted (same section).
- **BTreeMap** (6%), as at `a92d51a` below.
- **sh8bench** (2.0 times mimalloc, level with jemalloc). On one L3, 291 against mimalloc's
  225 ms: exchanges that still reach the shared stacks, and the refresh rule's launders; on two,
  objects that cross through the shared stack and the bitmaps when a burst outruns a CPU's
  stock (`launder-and-fill.md`, "sh8bench, accounted" and "Stock").
- **larson** (14 to 16% behind mimalloc): cause D below, unchanged, and accepted for now.
- **tokio spawn** (31% behind glibc at four threads, 12% behind jemalloc at eight): false
  sharing in tokio's own structures, placed by asmalloc's size classes (section 15).
- **The split pipeline, the tree and JSON** (4.5%, 3% and 3% slower than at `a92d51a`, section
  4): not attributed.
- **RPC log, mstress, preloaded JSON, cfrac, espresso** (under 1 to 7%): not analysed. On the
  RPC log jobs the control differs from mimalloc by 2%.
- **Small heaps (memory)**: as at `a92d51a` below; `rpc log c256` also keeps what its fills
  reach (section 4).

### At `a92d51a`

The gaps of the summary, with what the notes that measured them found. None was profiled
again for that session.

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
  memory over jemalloc is the outgrown tables kept for the next build (section 13).
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
the depot per cache domain, and E by the depot per cache domain; D is unchanged (section 13).

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
- Some jobs run in two modes: xmalloc-test for most allocators, and glibc's larson and
  eight-thread cache-scratch. Their medians depend on how many runs fell in each. The
  large-object pipeline and tokio spawn ran in two modes in earlier sessions by the clock each
  run started with, which this session's runner controls (section 16).
- Earlier sessions' runs started from whatever clock the package had. Against them, short jobs
  ranked by time, and jobs ranked by cycles that wait on memory, move with the clock (section
  4).
- The workloads were rebuilt between each pair of sessions (between `a92d51a` and `e96086d`
  only to add three probe programs that no suite job runs, which moves the rest of the code),
  and JSON and the tree carry a few percent of code layout (section 14), so moves of that size
  between sessions are not resolved.
- The profiles in section 9 are single runs; their counts per allocation are exact, their
  attribution of cycles to symbols is sampled.

## 13. Earlier sessions and changes to the suite

**The `a92d51a` session** (2026-10-18, the same machine, quiet, ten rounds, the same 56 jobs,
4,800 runs, none invalid, none left out as busy, 95 minutes) was the previous report; its data
are listed at the top. Its runner did not warm the clock, and 665 of its runs ran more than 5%
below their allocator's usual clock on the job. Its control's largest difference was 6.4%,
with 7 of 56 intervals excluding zero. Section 4 compares it with this one. Its report said
what moved since the session before, as follows. Neither session warmed the clock, so the
moves below include it where runs started cold: the split pipeline's free-only consumer
(section 4) and tokio spawn at four threads (section 16) are two such cases.

**What moved between `b5d6e8f` and `a92d51a`, and why.**

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

**The `b5d6e8f` session** (2026-10-18, the same machine, quiet, ten rounds, 55 jobs, 4,280
runs, none invalid, none left out as busy) came before; its data are listed at the top. Its
control's largest difference was 9.1%, with 7 of 55 intervals excluding zero. Sections 8, 10
and 11 were measured with the allocator at that commit. Changes to the suite between it and
the `a92d51a` session:

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
effect, too small to matter here. The same alignment does matter to the data TLB on other jobs; see
"Small classes start on pages of their own" below.

**What would fix it.** Not the allocator, short of tuning its classes to tokio's structure
sizes: which types share a class decides whether these headers interleave, and a coarser
class would move the problem to other programs and cost memory everywhere. tokio could pad
`queue::Inner` and move the task-list counter away from the fields it reads. Until then the
spawn jobs rank where each allocator puts these few lines more than what it costs to
allocate, and glibc's result there changes with the program's path.

### Small classes start on pages of their own

2026-10-19, at `700c8ce` against main at `35b00fa`. Every region starts on a 4 GiB
boundary and fills take the lowest slots, so the first pages of all the classes in use had
the same low virtual page numbers, which index the second-level data TLB's sets. A small
heap spread over many classes then walked the page tables though it used a few hundred
kilobytes. Each small class's slot 0 now lies at an origin of its own
(`Plan::slot_origin`): class `i` at `i * 159` pages modulo 256, rounded down to the class's
alignment, never more than a 64th of a region. No two small classes start on the same page
in a full-size region, and the pages below an origin are never committed, so it costs
address space only. The class from an address is the region table's as before; only the
back end's slot arithmetic subtracts the origin, so the fast paths are unchanged.

**Method.** Eleven jobs where a small heap spans many classes, run by `benchrun` with main
and the change as two allocator builds, interleaved, under `scripts/measure` (`--both-l3`
for the RPC jobs and mstress). A scratch patch to the runner counted raw events: in the
workload's measured region for the Rust workloads, over the server's window for RPC, and for
the whole process for the C programs. Per operation, medians of three rounds (first L3) or
four (both L3s). Scratch builds first tested where the offset matters: whole pages over 16
pages removed half to five sixths of the walks, over 256 pages all of them, and offsets of
whole lines within one page none of them (espresso gained 2.7% from fewer L1 misses, which
whole pages over 256 also give); lines also left `spawn` alone (above).

| Job | Time | 4 KiB page walks per operation |
|---|---:|---:|
| churn live=10k small | −11.6% | 0.133 → 0.0001 |
| espresso (preload) | −3.3% | 443k → 6.9k a run |
| rpc echo c64 | −2.3% | 24.1 → 5.5 |
| rpc log c64 | −1.2% | 99 → 71 |
| cfrac (preload) | −1.0% | 8.9k → 1.5k a run |
| churn live=100, map btree 1M, forkjoin t4, json 32Mi preload, mstress 8 | within ±1% (mstress −3.5% on wall time, cycles not) | level |
| json 32Mi | +2.4% this session; −0.1 to +0.7% in three earlier ones | level |

churn live=100 under main ran at 34 cycles/op in one of three runs, and 22.6 in the others
and in every run of the change; L1 misses fell by more than half there too. rpc log, which
was +0.5 to +1.7% in earlier sessions on an earlier build, is −1.2% here: noise. The smoke
set (three rounds) is level but sh8bench preloaded, +2.2% here and +3% on the first version
of the change (which spread origins by bit reversal), not analysed further; tree depth=18
runs at 37.4 or 39 to 40 cycles/node in both builds from run to run (section 14). Cgroup
peaks of the small-process probes (`bench/probes/memory/peaks.sh`: spawn, churn1k, pipe2,
mpmc, forkjoin, btree, json, hmap) are within 0.5 MiB of main, the larger differences lower.

## 16. The clock a run starts with

2026-10-19, at `35b00fa` plus the runner change it describes. Short multithreaded jobs spread
by up to 30% in time at the same cycles (section 15; `memory.md`, "The cost of a large block,
by cause"). The cause is the package's boost, and the runner now controls it.

**What the clock depends on.** `bench/probes/clock_ramp.rs` spins threads on chosen CPUs and
reads each one's cycles over wall time in 5 to 20 ms windows, with the heavy-work slice
frozen and nothing else busy (amd-pstate-epp, governor `powersave`, EPP
`balance_performance`, as the machine comes):

- After 1.5 s of idle, every core runs at 4.38 GHz, the nominal clock (`acpi_cppc/
  nominal_freq` is 4300 MHz), for the first 200 to 260 ms of a spin, then reaches the boost
  within 40 ms: 5.71 GHz with one core busy, 5.67 with four, 5.56 with eight. The ramp takes
  the same time for one, four or eight cores.
- The state is the package's. A 500 ms spin on CPU 3 (same L3), 18 (CPU 2's SMT sibling) or
  10 (the other L3) put CPU 2 at 5.68 GHz from its first 10 ms; 250 ms did so in one of six
  trials, 100 ms never.
- It decays between 200 and 400 ms of idle. After an 800 ms spin, a spin started 5 to
  200 ms later ran at 5.70 from its second 5 ms window (the first, a core waking from idle,
  reads 5.0 to 5.5); after 400 ms it was back at 4.38 for 45 to 60 ms before ramping.
- Load from other people's work keeps the boost (a session's quiet gate allows one CPU's
  worth elsewhere), and lowers it a little: CPU 2 alone ran at 5.42 GHz with seven other CPUs
  busy.

So in a session, a run that starts after the gate's second of idle runs its first 250 ms at
4.38 GHz, and a job shorter than that runs all of it there. In the `b5d6e8f` full session,
61% of the first runs after a gate in the cross-thread, work-stealing and single-thread
groups ran below 4.6 GHz, 30% of the second and 15% of the third, against 5.5% later; 602 of
its 4,280 runs are more than 5% below their allocator's usual clock on the job.

**What the runner does** (`bench/runner/src/clock.rs`, `bench/README.md`, "Clock"). Before
each run it spins on the job's CPUs, reading each one's clock in 5 ms windows, until every one
has held the CPU set's warm clock (within 3%) for three windows, then starts the job: the
start is a few milliseconds, far inside the 200 ms the boost survives. The warm clock is
learned from an 800 ms spin the first time a session uses a CPU set, and relearned if a
warm-up cannot reach it within 800 ms. The gate spins on the runner's CPU while it measures,
which keeps the package boosted without touching any CPU the gate counts, so the warm-up
after it is a check, not a ramp. Every run records the clock over its measured phases
(`clock.ghz`) and its warm-up (`clock.warmup`); `benchrun report` gives each job's median
clock and counts runs more than 5% below their allocator's median on the job
(`--steady-only` leaves them out).

**Cost.** 15.4 ms a run (median of 340), 0.8 s for each CPU set's first warm-up: about a
minute and a half on a full session of 4,800 runs.

**Before and after.** The four jobs below, all allocators, ten rounds each way, in alternating
blocks of five, under `scripts/measure --both-l3` with the gate tightened to 0.3 CPUs busy
elsewhere so that every job started from a quiet package, as on an idle machine (all 680
runs passed the gate quiet). Before is `--no-warm` (the old behaviour: sleep through the
gate, no warm-up).

| Job, allocator | Before: median, ± (half IQR), range | Runs off clock | After: median, ±, range | Runs off |
|---|---|---:|---|---:|
| pipeline t2 large, asmalloc (ns/alloc) | 58.13, 9.9%, 57.7 to 74.9 | 3 | 58.43, 0.3%, 57.8 to 59.3 | 0 |
| pipeline t2 large, tcmalloc-percpu | 40.65, 14.2%, 39.9 to 52.3 | 4 | 40.34, 0.3%, 40.0 to 40.9 | 0 |
| pipeline t2 large, mimalloc-crate | 40.17, 8.3%, 39.9 to 52.2 | 4 | 40.02, 0.3%, 39.8 to 40.4 | 0 |
| spawn t4 local, asmalloc (ns/task) | 139.05, 7.2%, 128.8 to 166.1 | 5 | 129.90, 0.4%, 128.6 to 131.0 | 0 |
| spawn t4 local, tcmalloc-thread | 113.72, 6.7%, 112.1 to 147.7 | 3 | 113.04, 0.6%, 111.9 to 114.4 | 0 |
| spawn t4 local, jemalloc | 113.17, 0.6%, 111.1 to 131.1 | 1 | 113.41, 0.3%, 112.4 to 115.2 | 0 |

Before, 46 of 340 runs were off clock; after, none. Medians of the allocator-job pairs with
no run off clock before moved by 0.4% at most, except glibc's pipeline (-1.6%, a job that
spreads by ±5% either way); asmalloc's spawn t4 median fell 6.6%, because five of its ten
runs before had started cold. The single-threaded `churn live=1k
small`, ranked by cycles, had cold runs before (10 of 90 runs off) and its medians moved by
0.4% at most: cycles do not see the clock. The preloaded `cfrac` (2.1 s, ranked by time) had
no run off clock either way, since only its first 250 ms can be cold, and its medians moved by
0.2% at most.

glibc's large-object pipeline is the exception that shows why the report compares a run with
its own allocator's usual clock: its threads spend their time in `mmap`, `munmap` and page
faults and run at 4.4 to 4.5 GHz however warm the package is, before and after.

An earlier pair of runs, taken while another worktree's proofs kept one CPU busy outside the
frozen slice, had no cold runs in either arm: a busy machine hides the effect, which is why it
came and went between sessions.

### The clocked pass carries on round the heap

2026-10-19, at `186bf6a` against main at `233ffbf`. The maintain test (`heap/tests.rs`) passed
only by a slack of two huge pages. Run for 40 intervals, maintain left 50 pages (36 with
origins) that `collect` then returned: free, resident pages at the bottom of the lowest
region. Each interval the idle drain launders the loaded magazines of a quiet program's light
classes, and the light work refills them from the lowest free slots, reviving a few pages at
the bottom of each class's region. Every clocked pass began at the top of the heap and spent
its quarter of what is retained re-purging those pages in the higher regions, so it never
reached the lowest region's. "Fills take the lowest addresses" holds within a region; across
regions the order is only that in which classes were first used.

A pass now begins where the walk stands and carries on region by region, top-down within
each, wrapping from the lowest region to the highest; it ends when its goal is met or the walk
is back where it began a lap later. Its budget and steps are unchanged, and the cap's purge
already carried on from where the walk stood.

Measured on the simulated `Native` shape (the maintain scenario, 40 intervals) and the real
machine (`benchrun`, three to five interleaved rounds; the burst-idle and phase probes of
`bench/probes/memory/worst.sh`, three rounds, `madvise` counted with `strace -c` and minor
faults with `time`):

| | main | carried on |
|---|---:|---:|
| maintain: resident over what `collect` leaves, after 14 / 40 intervals | 328k / 147k | 4 KiB / 0 |
| maintain: purges an interval, steady | 28 | 20 |
| burst-idle probe: `madvise` calls, faults, MiB resident at 4, 8 and 12 s | 31.9k, 40.8k, 52/15/3 | 31.8k, 40.8k, 52/15/3 |
| phase probe: `madvise` calls, faults | 5.7k to 6.1k, 10.2k to 10.6k | 6.1k to 6.6k, 10.0k to 10.7k |
| rpc log c64: server cycles/req, process faults (five rounds) | 123.9k, 2,660 | 128.9k (+4.0%), 2,970 |
| json 32Mi native / preload, cycles/byte (five rounds) | 16.36 / 17.33 | 16.67 (+1.9%) / 17.45 (+0.7%) |

The rest of the smoke set, mstress 8, rpc echo c64 and the RPC jobs' peaks are level. json's
faults are identical in both builds, so its 1.9% is more likely code placement (section 14).
rpc log's are not: 12% more faults and 4% more cycles a request say the carried-on pass now
also purges free pages at the top of the busy server's lowest regions, the classes it used
first, which it then faults back. Before, those regions were only reached when the higher
ones had nothing left to give.

