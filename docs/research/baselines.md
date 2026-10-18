# Allocator baselines on workloads

Measured 2026-10-18 on a quiet machine, at commit `b5d6e8f`, with the suite in `bench/`
(commands and method in `bench/README.md`): asmalloc against glibc, mimalloc (the newest
release and the `mimalloc` crate as shipped), jemalloc (`tikv-jemallocator`) and tcmalloc
(per-CPU and per-thread), on every workload of the full suite, ten rounds. This replaces the
2026-10-17 session, which ran on a busy machine without asmalloc (section 12).

Raw results: `bench/results/full-2026-10-18-b5d6e8f.jsonl` in the main checkout, and
`docs/research/data/baselines-2026-10-18.jsonl.gz` (`benchrun report` reads it after
`gunzip`). Per-workload tables with every allocator, spread, memory and faults:
`docs/research/data/baselines-2026-10-18-detail.md`.

## 1. Summary

Time, asmalloc against the best incumbent on each job (a gap is the median of per-round
ratios; "level" means the 95% interval includes no difference):

- **Cross-thread, one L3 domain, 2 to 8 threads:** ahead on every job, 8 to 52%, except the
  eight-thread pipeline (level). Across L3 domains, ahead 27 to 52%.
- **Independent pipelines side by side across L3 domains:** behind 16 to 35% (10 against
  jemalloc's 8 ns per chain). Two in one domain: ahead 49%.
- **Large objects (4 to 256 KiB) crossing threads:** behind 8.3 times (297 against 36 ns).
- **Work stealing:** rayon ahead 15% at four threads, level at eight; tokio behind 10 to 18%.
- **RPC server:** behind 2% on server cycles per request.
- **Single-threaded churn, 100 to 10M live objects:** ahead 6 to 31%.
- **Build and drop:** behind mimalloc on BTreeMap (32%), HashMap (60%), Box tree (116%),
  JSON (45%) and one Vec grown to 256 MiB (15%); ahead 7% on 64 Vecs to 4 MiB.
- **C programs, preloaded:** behind on larson (29 to 30%), mstress (57%) and sh8bench (4.6
  times); level or within 2% on xmalloc-test, cache-scratch, cfrac and espresso.

Memory (cgroup peak, against the smallest incumbent):

- **Heaps of 10 MiB or more:** ahead, level or within 4% everywhere: churn, maps, tree,
  JSON, Vec growth, fork-join and mstress are ahead by up to 21% or level, churn at 100k
  live objects is 4% over; larson is behind 10 to 13% (60 against 53 MiB).
- **Small heaps (1 to 11 MiB):** behind glibc by 0.4 to 3 MiB of fixed footprint, which
  reads as 20 to 180%; against jemalloc on the same jobs, from 2 MiB under (the RPC
  server, fork-join) to 1.3 MiB over (mpmc with one producer).
- **After the peak:** lowest of the three measured. After freeing 218 MiB, asmalloc holds 1.2
  MiB at 16 seconds, jemalloc 4.5 and mimalloc 16.2. Behind only on the peak of a phase
  change (53 against jemalloc's 29 MiB) and of many classes on few CPUs (15 against 8).

Five causes explain most of the gaps (section 9): memory returned and faulted straight back
(large pipeline, HashMap, JSON, mstress), every object passing through the back end's bitmaps
on each build-and-drop cycle (tree, BTreeMap), contended refills (sh8bench), a global depot
that hands objects across L3 domains (independent pipelines), and C++ operators reached
through libstdc++ (larson). mimalloc's build-and-drop lead is also its huge pages: 100 to 600
times fewer page faults with transparent huge pages in `madvise` mode.

## 2. Machine, versions and method

AMD Ryzen 9 9950X (Zen 5), 16 cores with SMT, two L3 domains (CPUs 0-7 with siblings 16-23,
8-15 with 24-31), 60 GiB, Linux 7.2.9-201.fc44, glibc 2.43, rustc 1.99.1, GCC 16.2.1.
Governor `powersave`, energy preference `balance_performance`, boost on, transparent huge
pages `madvise` (defrag `madvise`), NMI watchdog on.

| Allocator | Version and build | Native (Rust `#[global_allocator]`) | Preload |
|---|---|---|---|
| asmalloc | `b5d6e8f` | `asmalloc::Asmalloc`, `executable` feature, fat LTO | `asmalloc-preload` (C names only) |
| glibc | 2.43 | `std::alloc::System` | none |
| mimalloc | v3.5.3 (`31d034d9`), `src/static.c` at `-O3` as libmimalloc-sys builds it | as the `mimalloc` crate calls it | cmake Release shared library |
| mimalloc-crate | the `mimalloc` crate 0.1.52 as shipped (bundles 3.3.2) | itself | not run |
| jemalloc | `tikv-jemallocator` 0.6.1 (jemalloc 5.3.0-1-ge13ca993, static, prefixed, no stats or profiling) | itself | Fedora 5.3.0-14 shared library |
| tcmalloc-percpu | google/tcmalloc `f3c6669a`, bazel 8.8.0 `-c opt`, initial-exec TLS | as `tcmalloc-better` 0.1.19 calls it | shared library; both with `GLIBC_TUNABLES=glibc.pthread.rseq=0` |
| tcmalloc-thread | the same | the same | the same, without the tunable |
| mimalloc-ctl | mimalloc built a second time: the control | | |

`bench/baselines/build.sh` builds the C libraries and records versions and flags in the
manifests, which every result file carries.

**Method.** One fresh process per run; each round runs every job under every allocator in a
new random order. Ten rounds, 55 jobs, 4,280 runs, none invalid. Threads are pinned one per
physical core on CPUs 0-3 and 8-11 (single-threaded jobs on CPU 2), the process confined to
its CPUs before `execve`, and every run under a per-run memory cap (4 GiB) inside a capped
session scope. Every run reported the allocator, mode, library and version it was meant to
have. Single-threaded jobs rank by cycles per operation (user and kernel), multithreaded ones
by wall time per allocation, C programs by their own throughput or wall time; memory ranks
by each run's cgroup peak. Cross-thread consumers check every object's header, so a block
handed out twice fails the run.

**Quiet.** Before the session, 0.03 CPUs' worth of work ran on the whole machine over ten
seconds, and the CPU temperature (Tctl 41°C, CCDs 30°C) held over half a minute. No other
work ran during it: the quiet gate (nothing on the measurement CPUs or their siblings, at most
one CPU's worth elsewhere) passed before every job without waiting, and no run was left out
as busy.

**Control.** mimalloc-ctl against mimalloc, paired by round: the largest median difference
is 9.1%; 7 of 55 intervals exclude zero where chance predicts 2.8, all under 1% except two
with wide intervals (exchange of mixed sizes, the preloaded pipeline). Single-threaded
differences of 1% and cross-thread differences of a few percent are resolved.

**Code layout.** The control does not catch a job whose time depends on where its code
landed, because every allocator's build places the workload's code somewhere else. When a job
moves after a change that should not have touched it, `benchrun layout` (`bench/README.md`)
runs it under builds that place the same code at other addresses; section 14 has the method
and the jobs that are sensitive.

## 3. asmalloc against the best incumbent, per workload

The best incumbent is the one with the lowest median cost; the gap is the median of
per-round ratios of asmalloc to it, with "level" where the 95% interval includes no
difference. Memory compares cgroup peaks with the smallest incumbent the same way.

| Workload | Metric | asmalloc | Best incumbent | Time | Cgroup peak MiB, asm / best | Memory |
|---|---|---:|---|---|---|---|
| pipeline t2 sizes=s64 local all-alloc (native) | ns/alloc | 3.845 | jemalloc 6.409 | ahead 40% | 1.4 / glibc 1.0 | behind 40% |
| pipeline t2 sizes=small local all-alloc (native) | ns/alloc | 3.753 | tcmalloc-percpu 6.6 | ahead 43% | 2.3 / glibc 1.2 | behind 90% |
| pipeline t2 sizes=mixed local all-alloc (native) | ns/alloc | 11.27 | tcmalloc-percpu 12.52 | ahead 10% | 5.1 / glibc 3.1 | behind 65% |
| pipeline t2 sizes=small local free-only (native) | ns/alloc | 3.717 | tcmalloc-percpu 6.602 | ahead 44% | 2.3 / glibc 1.2 | behind 92% |
| pipeline t2 sizes=large objects=1M inflight=512 local all-alloc (native) | ns/alloc | 296.6 | tcmalloc-thread 35.64 | behind 721% | 3.1 / tcmalloc-thread 3.8 | ahead 16% |
| pipeline t2 sizes=small split all-alloc (native) | ns/alloc | 12.09 | jemalloc 18.35 | ahead 38% | 2.2 / glibc 1.0 | behind 129% |
| pipeline t2 sizes=small split free-only (native) | ns/alloc | 11.05 | tcmalloc-percpu 23.47 | ahead 52% | 2.2 / glibc 0.8 | behind 182% |
| pipeline t4 sizes=small local all-alloc (native) | ns/alloc | 3.235 | jemalloc 3.602 | ahead 10% | 4.1 / glibc 2.8 | behind 46% |
| pipeline t8 sizes=small both all-alloc (native) | ns/alloc | 4.269 | jemalloc 4.396 | level (-3%) | 7.4 / glibc 6.0 | behind 24% |
| pipeline t2 x2 sizes=small local all-alloc (native) | ns/alloc per chain | 3.983 | tcmalloc-percpu 7.874 | ahead 49% | 3.6 / glibc 2.0 | behind 75% |
| pipeline t2 x2 sizes=small split all-alloc (native) | ns/alloc per chain | 10.81 | jemalloc 8.013 | behind 35% | 3.4 / glibc 2.0 | behind 70% |
| pipeline t2 x3 sizes=small both all-alloc (native) | ns/alloc per chain | 9.379 | jemalloc 8.081 | behind 16% | 4.7 / glibc 3.0 | behind 54% |
| pipeline t2 x4 sizes=small both all-alloc (native) | ns/alloc per chain | 10.16 | jemalloc 8.157 | behind 24% | 6.0 / glibc 4.0 | behind 53% |
| exchange t2 sizes=small local (native) | ns/alloc | 4.26 | tcmalloc-percpu 6.936 | ahead 39% | 2.5 / glibc 1.7 | behind 47% |
| exchange t2 sizes=mixed local (native) | ns/alloc | 8.082 | tcmalloc-percpu 8.791 | ahead 8% | 4.9 / tcmalloc-percpu 4.0 | behind 21% |
| exchange t2 sizes=small split (native) | ns/alloc | 13.79 | jemalloc 18.74 | ahead 27% | 2.4 / glibc 1.4 | behind 74% |
| exchange t4 sizes=small local (native) | ns/alloc | 2.363 | tcmalloc-percpu 3.688 | ahead 36% | 3.8 / glibc 3.2 | behind 18% |
| exchange t8 sizes=small both (native) | ns/alloc | 3.254 | jemalloc 3.991 | ahead 19% | 6.6 / tcmalloc-percpu 5.2 | behind 26% |
| mpmc p1 c3 sizes=small local all-alloc (native) | ns/alloc | 5.905 | jemalloc 7.522 | ahead 30% | 3.2 / glibc 1.2 | behind 154% |
| mpmc p1 c3 sizes=small local free-only (native) | ns/alloc | 5.087 | tcmalloc-percpu 8.559 | ahead 40% | 3.3 / glibc 1.4 | behind 132% |
| mpmc p3 c1 sizes=small local all-alloc (native) | ns/alloc | 5.16 | tcmalloc-percpu 7.126 | ahead 28% | 3.7 / glibc 2.1 | behind 76% |
| mpmc p3 c1 sizes=small local free-only (native) | ns/alloc | 5.876 | tcmalloc-percpu 7.245 | ahead 28% | 3.7 / glibc 2.0 | behind 84% |
| mpmc p4 c4 sizes=small both all-alloc (native) | ns/alloc | 4.921 | jemalloc 8.565 | ahead 43% | 5.7 / glibc 2.8 | behind 110% |
| forkjoin t4 local (native) | ns/alloc | 3.743 | tcmalloc-percpu 4.394 | ahead 15% | 15.7 / tcmalloc-percpu 16.4 | ahead 4% |
| spawn t4 local (native) | ns/task | 125.6 | glibc 105.9 | behind 18% | 2.6 / glibc 1.6 | behind 66% |
| forkjoin t8 both (native) | ns/alloc | 4.058 | jemalloc 3.963 | behind 3% | 17.9 / tcmalloc-percpu 17.8 | level (+0%) |
| spawn t8 both (native) | ns/task | 249.6 | jemalloc 224.9 | behind 10% | 4.8 / glibc 3.1 | behind 59% |
| rpc echo c64 (native) | server cycles/req | 4.62e+04 | tcmalloc-percpu 4.54e+04 | behind 2% | 5.8 / glibc 3.3 | behind 79% |
| rpc log c64 (native) | server cycles/req | 1.228e+05 | mimalloc 1.215e+05 | behind 2% | 7.7 / glibc 4.9 | behind 54% |
| rpc log c256 (native) | server cycles/req | 1.199e+05 | mimalloc 1.176e+05 | behind 2% | 11.0 / glibc 8.0 | behind 38% |
| churn live=100 small (native) | cycles/op | 24.21 | jemalloc 34.78 | ahead 31% | 0.9 / glibc 0.5 | behind 89% |
| churn live=1k small (native) | cycles/op | 25.13 | jemalloc 36 | ahead 30% | 1.1 / glibc 0.6 | behind 69% |
| churn live=10k small (native) | cycles/op | 32.16 | jemalloc 40.3 | ahead 21% | 2.3 / glibc 1.7 | behind 31% |
| churn live=100k small (native) | cycles/op | 47.79 | jemalloc 54.5 | ahead 12% | 12.2 / jemalloc 11.8 | behind 4% |
| churn live=1M small (native) | cycles/op | 113.6 | jemalloc 126.2 | ahead 10% | 109.3 / jemalloc 110.3 | ahead 1% |
| churn live=10M small (native) | cycles/op | 217.4 | jemalloc 250.3 | ahead 13% | 1069.7 / jemalloc 1091.5 | ahead 2% |
| churn live=100k mixed (native) | cycles/op | 88.83 | tcmalloc-percpu 94.38 | ahead 6% | 34.7 / tcmalloc-thread 34.6 | behind 0% |
| map btree 1M (native) | cycles/entry | 1089 | mimalloc-crate 826 | behind 32% | 123.4 / jemalloc 125.1 | ahead 1% |
| map hash 1M (native) | cycles/entry | 690.6 | mimalloc-crate 431.8 | behind 60% | 175.1 / jemalloc 183.6 | ahead 5% |
| tree depth=20 (native) | cycles/node | 99.26 | mimalloc 45.89 | behind 116% | 51.3 / glibc 64.6 | ahead 21% |
| vecgrow 1x256Mi (native) | cycles/element | 5.363 | mimalloc-crate 4.812 | behind 15% | 257.9 / glibc 257.0 | behind 0% |
| vecgrow 64x4Mi (native) | cycles/element | 9.38 | jemalloc 10.1 | ahead 7% | 270.3 / mimalloc 265.1 | behind 2% |
| json 32Mi (native) | cycles/byte | 23.62 | mimalloc-crate 16.34 | behind 45% | 352.2 / mimalloc-crate 355.0 | ahead 1% |
| churn live=100k small (preload) | cycles/op | 53.22 | mimalloc 52.13 | behind 2% | 12.3 / jemalloc 12.0 | behind 2% |
| pipeline t2 sizes=small local all-alloc (preload) | ns/alloc | 3.997 | tcmalloc-percpu 6.334 | ahead 37% | 2.0 / glibc 1.1 | behind 78% |
| json 32Mi (preload) | cycles/byte | 23.9 | mimalloc 16.63 | behind 44% | 353.1 / jemalloc 356.8 | ahead 1% |
| larson-sized 8 (preload) | Mops/s | 404.2 | mimalloc 522.7 | behind 30% | 60.3 / tcmalloc-thread 54.5 | behind 10% |
| larson 8 (preload) | Mops/s | 402.6 | mimalloc 514.6 | behind 29% | 60.0 / tcmalloc-thread 52.9 | behind 13% |
| xmalloc-test 4+4 (preload) | Mfrees/s | 325.8 | tcmalloc-percpu 545.6 | level (+23%) | 20.9 / tcmalloc-thread 34.0 | level (-39%) |
| sh8bench 16 (preload) | s | 1.19 | mimalloc 0.2559 | behind 364% | 119.5 / tcmalloc-percpu 119.8 | level (-0%) |
| mstress 8 (preload) | s | 0.3936 | mimalloc 0.2497 | behind 57% | 128.1 / jemalloc 139.6 | ahead 9% |
| cache-scratch 1 (preload) | s | 0.5136 | jemalloc 0.5001 | level (+2%) | 0.8 / glibc 0.8 | level (+2%) |
| cache-scratch 8 (preload) | s | 0.06737 | mimalloc 0.06764 | level (-0%) | 2.8 / glibc 2.5 | behind 10% |
| cfrac (preload) | s | 2.106 | mimalloc 2.07 | behind 2% | 1.7 / jemalloc 1.4 | behind 22% |
| espresso (preload) | s | 2.249 | mimalloc 2.231 | behind 1% | 2.8 / glibc 1.3 | behind 110% |

## 4. Cross-thread

Medians of ten.

Time:

| Job | Metric | asmalloc | glibc | mimalloc | mi crate | jemalloc | tc per-CPU | tc per-thread |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| pipeline t2 sizes=s64 local all-alloc | ns/alloc | 3.845 | 91.63 | 15.93 | 18.95 | 6.409 | 9.078 | 10.21 |
| pipeline t2 sizes=small local all-alloc | ns/alloc | 3.753 | 127.2 | 14.59 | 17.67 | 8.231 | 6.600 | 10.15 |
| pipeline t2 sizes=mixed local all-alloc | ns/alloc | 11.27 | 132.6 | 20.05 | 21.61 | 17.03 | 12.52 | 16.60 |
| pipeline t2 sizes=small local free-only | ns/alloc | 3.717 | 126.6 | 15.08 | 17.71 | 22.66 | 6.602 | 10.12 |
| pipeline t2 sizes=large objects=1M inflight=512 local all-alloc | ns/alloc | 296.6 | 370.1 | 41.74 | 40.01 | 300.2 | 40.46 | 35.64 |
| pipeline t2 sizes=small split all-alloc | ns/alloc | 12.09 | 123.2 | 53.01 | 67.01 | 18.35 | 23.35 | 29.83 |
| pipeline t2 sizes=small split free-only | ns/alloc | 11.05 | 122.3 | 53.52 | 64.48 | 50.61 | 23.47 | 29.07 |
| pipeline t4 sizes=small local all-alloc | ns/alloc | 3.235 | 33.31 | 10.18 | 10.06 | 3.602 | 4.136 | 6.854 |
| pipeline t8 sizes=small both all-alloc | ns/alloc | 4.269 | 49.05 | 11.37 | 11.91 | 4.396 | 5.931 | 7.865 |
| pipeline t2 x2 sizes=small local all-alloc | ns/alloc per chain | 3.983 | 131.3 | 17.00 | 18.35 | 7.908 | 7.874 | 12.63 |
| pipeline t2 x2 sizes=small split all-alloc | ns/alloc per chain | 10.81 | 139.0 | 16.99 | 20.16 | 8.013 | 22.49 | 29.88 |
| pipeline t2 x3 sizes=small both all-alloc | ns/alloc per chain | 9.379 | 139.8 | 17.41 | 20.59 | 8.081 | 20.89 | 28.62 |
| pipeline t2 x4 sizes=small both all-alloc | ns/alloc per chain | 10.16 | 147.1 | 17.54 | 21.41 | 8.157 | 25.62 | 33.88 |
| exchange t2 sizes=small local | ns/alloc | 4.260 | 45.54 | 12.01 | 12.78 | 7.629 | 6.936 | 11.86 |
| exchange t2 sizes=mixed local | ns/alloc | 8.082 | 71.58 | 15.17 | 15.54 | 10.11 | 8.791 | 12.74 |
| exchange t2 sizes=small split | ns/alloc | 13.79 | 129.8 | 42.66 | 46.48 | 18.74 | 27.22 | 32.96 |
| exchange t4 sizes=small local | ns/alloc | 2.363 | 25.38 | 6.181 | 6.792 | 4.535 | 3.688 | 6.954 |
| exchange t8 sizes=small both | ns/alloc | 3.254 | 23.55 | 8.535 | 9.619 | 3.991 | 6.037 | 9.082 |
| mpmc p1 c3 sizes=small local all-alloc | ns/alloc | 5.905 | 101.1 | 20.66 | 21.51 | 7.522 | 8.506 | 9.681 |
| mpmc p1 c3 sizes=small local free-only | ns/alloc | 5.087 | 97.78 | 19.98 | 21.12 | 20.15 | 8.559 | 9.710 |
| mpmc p3 c1 sizes=small local all-alloc | ns/alloc | 5.160 | 77.64 | 7.807 | 8.846 | 10.28 | 7.126 | 11.73 |
| mpmc p3 c1 sizes=small local free-only | ns/alloc | 5.876 | 75.69 | 7.678 | 8.852 | 20.99 | 7.245 | 12.97 |
| mpmc p4 c4 sizes=small both all-alloc | ns/alloc | 4.921 | 73.73 | 21.25 | 24.20 | 8.565 | 9.955 | 12.38 |

Cgroup peak, MiB:

| Job | asmalloc | glibc | mimalloc | mi crate | jemalloc | tc per-CPU | tc per-thread |
|---|---:|---:|---:|---:|---:|---:|---:|
| pipeline t2 sizes=s64 local all-alloc | 1.396 | 1.000 | 6.750 | 4.750 | 1.443 | 2.527 | 2.250 |
| pipeline t2 sizes=small local all-alloc | 2.299 | 1.250 | 4.750 | 6.627 | 1.877 | 2.799 | 2.590 |
| pipeline t2 sizes=mixed local all-alloc | 5.143 | 3.104 | 8.750 | 9.691 | 5.051 | 3.750 | 3.189 |
| pipeline t2 sizes=small local free-only | 2.283 | 1.250 | 4.750 | 4.750 | 1.744 | 2.787 | 2.582 |
| pipeline t2 sizes=large objects=1M inflight=512 local all-alloc | 3.143 | 33.86 | 65.88 | 58.73 | 8.609 | 4.381 | 3.752 |
| pipeline t2 sizes=small split all-alloc | 2.160 | 1.000 | 4.750 | 6.627 | 1.816 | 2.865 | 2.578 |
| pipeline t2 sizes=small split free-only | 2.230 | 0.762 | 4.750 | 4.750 | 1.746 | 2.734 | 2.578 |
| pipeline t4 sizes=small local all-alloc | 4.082 | 2.768 | 7.250 | 9.250 | 3.637 | 4.162 | 4.734 |
| pipeline t8 sizes=small both all-alloc | 7.383 | 5.982 | 12.50 | 14.50 | 6.828 | 6.564 | 9.158 |
| pipeline t2 x2 sizes=small local all-alloc | 3.594 | 2.012 | 7.250 | 9.248 | 3.246 | 3.994 | 3.602 |
| pipeline t2 x2 sizes=small split all-alloc | 3.441 | 2.012 | 7.250 | 9.250 | 3.246 | 3.230 | 3.723 |
| pipeline t2 x3 sizes=small both all-alloc | 4.672 | 3.000 | 8.000 | 9.750 | 4.746 | 3.990 | 4.846 |
| pipeline t2 x4 sizes=small both all-alloc | 6.035 | 4.000 | 10.25 | 12.00 | 6.062 | 4.482 | 5.941 |
| exchange t2 sizes=small local | 2.490 | 1.686 | 6.750 | 8.750 | 2.445 | 2.930 | 9.449 |
| exchange t2 sizes=mixed local | 4.902 | 4.537 | 12.75 | 14.65 | 6.270 | 4.006 | 9.244 |
| exchange t2 sizes=small split | 2.418 | 1.385 | 6.750 | 8.750 | 2.314 | 2.893 | 8.654 |
| exchange t4 sizes=small local | 3.787 | 3.230 | 8.250 | 9.250 | 4.277 | 3.691 | 16.06 |
| exchange t8 sizes=small both | 6.617 | 6.160 | 12.38 | 14.49 | 9.018 | 5.238 | 25.61 |
| mpmc p1 c3 sizes=small local all-alloc | 3.215 | 1.250 | 7.250 | 6.764 | 2.246 | 3.000 | 2.539 |
| mpmc p1 c3 sizes=small local free-only | 3.262 | 1.375 | 5.250 | 5.250 | 1.996 | 3.000 | 2.531 |
| mpmc p3 c1 sizes=small local all-alloc | 3.715 | 2.104 | 7.250 | 9.250 | 3.180 | 3.490 | 3.020 |
| mpmc p3 c1 sizes=small local free-only | 3.664 | 1.951 | 7.250 | 9.250 | 2.994 | 3.492 | 3.039 |
| mpmc p4 c4 sizes=small both all-alloc | 5.746 | 2.750 | 10.25 | 10.50 | 4.742 | 4.234 | 4.076 |

## 5. Work stealing and the RPC server

Time:

| Job | Metric | asmalloc | glibc | mimalloc | mi crate | jemalloc | tc per-CPU | tc per-thread |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| forkjoin t4 local | ns/alloc | 3.743 | 10.69 | 5.754 | 5.710 | 4.769 | 4.394 | 5.128 |
| spawn t4 local | ns/task | 125.6 | 105.9 | 119.4 | 118.2 | 113.2 | 113.8 | 113.8 |
| forkjoin t8 both | ns/alloc | 4.058 | 17.78 | 7.457 | 8.356 | 3.963 | 5.723 | 7.134 |
| spawn t8 both | ns/task | 249.6 | 228.1 | 244.9 | 237.1 | 224.9 | 230.7 | 227.3 |

Cgroup peak, MiB:

| Job | asmalloc | glibc | mimalloc | mi crate | jemalloc | tc per-CPU | tc per-thread |
|---|---:|---:|---:|---:|---:|---:|---:|
| forkjoin t4 local | 15.72 | 17.92 | 21.59 | 23.25 | 28.39 | 16.39 | 16.61 |
| spawn t4 local | 2.594 | 1.588 | 9.250 | 11.25 | 2.521 | 3.291 | 2.846 |
| forkjoin t8 both | 17.91 | 21.33 | 26.50 | 28.50 | 22.89 | 17.82 | 18.73 |
| spawn t8 both | 4.836 | 3.072 | 12.50 | 14.50 | 4.572 | 4.842 | 4.369 |

Time:

| Job | Metric | asmalloc | glibc | mimalloc | mi crate | jemalloc | tc per-CPU | tc per-thread |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| rpc echo c64 | server cycles/req | 46199 | 49130 | 46740 | 47001 | 46014 | 45404 | 47407 |
| rpc log c64 | server cycles/req | 122789 | 148744 | 121547 | 122308 | 125354 | 122973 | 128475 |
| rpc log c256 | server cycles/req | 119942 | 150747 | 117570 | 118996 | 122295 | 119292 | 125778 |

Cgroup peak, MiB:

| Job | asmalloc | glibc | mimalloc | mi crate | jemalloc | tc per-CPU | tc per-thread |
|---|---:|---:|---:|---:|---:|---:|---:|
| rpc echo c64 | 5.838 | 3.254 | 14.12 | 15.37 | 5.834 | 6.154 | 18.42 |
| rpc log c64 | 7.664 | 4.938 | 24.69 | 26.55 | 9.314 | 9.082 | 18.64 |
| rpc log c256 | 10.96 | 7.990 | 27.70 | 31.42 | 12.98 | 11.80 | 27.17 |

The RPC client is the bottleneck, so these rank the server's cycles per request.

## 6. Single-threaded

Time:

| Job | Metric | asmalloc | glibc | mimalloc | mi crate | jemalloc | tc per-CPU | tc per-thread |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| churn live=100 small | cycles/op | 24.21 | 35.09 | 45.71 | 47.03 | 34.78 | 37.65 | 73.96 |
| churn live=1k small | cycles/op | 25.13 | 45.61 | 45.77 | 46.33 | 36.00 | 38.49 | 73.98 |
| churn live=10k small | cycles/op | 32.16 | 56.43 | 50.24 | 54.41 | 40.30 | 41.96 | 75.42 |
| churn live=100k small | cycles/op | 47.79 | 78.93 | 65.71 | 72.82 | 54.50 | 58.84 | 95.11 |
| churn live=1M small | cycles/op | 113.6 | 305.7 | 184.2 | 217.3 | 126.2 | 140.3 | 282.7 |
| churn live=10M small | cycles/op | 217.4 | 459.1 | 433.8 | 563.1 | 250.3 | 300.1 | 520.6 |
| churn live=100k mixed | cycles/op | 88.83 | 166.3 | 98.33 | 108.7 | 107.8 | 94.38 | 149.6 |
| map btree 1M | cycles/entry | 1089 | 1322 | 837.6 | 826.0 | 963.5 | 1034 | 1221 |
| map hash 1M | cycles/entry | 690.6 | 1098 | 455.4 | 431.8 | 622.9 | 599.3 | 771.4 |
| tree depth=20 | cycles/node | 99.26 | 113.4 | 45.89 | 45.92 | 101.9 | 115.9 | 161.8 |
| vecgrow 1x256Mi | cycles/element | 5.363 | 8.636 | 10.86 | 4.812 | 16.69 | 13.14 | 13.15 |
| vecgrow 64x4Mi | cycles/element | 9.380 | 18.24 | 36.69 | 43.73 | 10.10 | 37.62 | 37.92 |
| json 32Mi | cycles/byte | 23.62 | 22.60 | 16.61 | 16.34 | 23.28 | 28.84 | 33.22 |

Cgroup peak, MiB:

| Job | asmalloc | glibc | mimalloc | mi crate | jemalloc | tc per-CPU | tc per-thread |
|---|---:|---:|---:|---:|---:|---:|---:|
| churn live=100 small | 0.938 | 0.500 | 2.537 | 4.521 | 0.711 | 2.117 | 1.707 |
| churn live=1k small | 1.053 | 0.621 | 4.539 | 4.527 | 0.830 | 2.250 | 1.832 |
| churn live=10k small | 2.266 | 1.738 | 6.547 | 6.527 | 1.867 | 3.332 | 2.836 |
| churn live=100k small | 12.23 | 12.60 | 14.56 | 16.54 | 11.78 | 13.44 | 13.01 |
| churn live=1M small | 109.3 | 120.7 | 116.8 | 118.8 | 110.3 | 113.9 | 113.5 |
| churn live=10M small | 1070 | 1199 | 1143 | 1141 | 1091 | 1114 | 1114 |
| churn live=100k mixed | 34.73 | 40.46 | 54.63 | 56.63 | 42.61 | 34.95 | 34.56 |
| map btree 1M | 123.4 | 130.5 | 128.6 | 128.6 | 125.1 | 134.1 | 133.7 |
| map hash 1M | 175.1 | 208.2 | 299.1 | 198.9 | 183.6 | 253.6 | 253.2 |
| tree depth=20 | 51.31 | 64.55 | 96.67 | 96.66 | 66.82 | 68.75 | 68.35 |
| vecgrow 1x256Mi | 257.9 | 257.0 | 525.5 | 523.5 | 264.4 | 683.4 | 682.9 |
| vecgrow 64x4Mi | 270.3 | 269.3 | 265.1 | 289.1 | 307.1 | 286.6 | 296.2 |
| json 32Mi | 352.2 | 414.4 | 387.3 | 355.0 | 360.1 | 398.7 | 398.3 |

## 7. Programs through LD_PRELOAD

The Rust workloads built with glibc, then preloaded:

Time:

| Job | Metric | asmalloc | glibc | mimalloc | jemalloc | tc per-CPU | tc per-thread |
|---|---|---:|---:|---:|---:|---:|---:|
| churn live=100k small (preload) | cycles/op | 53.22 | 78.74 | 52.13 | 61.19 | 56.91 | 93.92 |
| pipeline t2 sizes=small local all-alloc (preload) | ns/alloc | 3.997 | 125.3 | 14.43 | 7.640 | 6.334 | 8.925 |
| json 32Mi (preload) | cycles/byte | 23.90 | 22.62 | 16.63 | 23.94 | 29.30 | 32.94 |

Cgroup peak, MiB:

| Job | asmalloc | glibc | mimalloc | jemalloc | tc per-CPU | tc per-thread |
|---|---:|---:|---:|---:|---:|---:|
| churn live=100k small (preload) | 12.28 | 12.62 | 14.59 | 11.99 | 13.46 | 13.05 |
| pipeline t2 sizes=small local all-alloc (preload) | 1.994 | 1.121 | 4.750 | 1.949 | 2.514 | 2.357 |
| json 32Mi (preload) | 353.1 | 414.4 | 387.3 | 356.8 | 419.0 | 418.6 |

mimalloc-bench programs:

Time:

| Job | Metric | asmalloc | glibc | mimalloc | jemalloc | tc per-CPU | tc per-thread |
|---|---|---:|---:|---:|---:|---:|---:|
| larson-sized 8 | Mops/s | 404.2 | 355.3 | 522.7 | 391.7 | 496.1 | 334.3 |
| larson 8 | Mops/s | 402.6 | 347.6 | 514.6 | 401.2 | 435.3 | 316.0 |
| xmalloc-test 4+4 | Mfrees/s | 325.8 | 75.20 | 301.3 | 20.09 | 545.6 | 6.674 |
| sh8bench 16 | s | 1.190 | 1.528 | 0.256 | 0.568 | 1.977 | 5.099 |
| mstress 8 | s | 0.394 | 0.358 | 0.250 | 0.430 | 0.294 | 0.319 |
| cache-scratch 1 | s | 0.514 | 0.509 | 0.522 | 0.500 | 0.521 | 0.511 |
| cache-scratch 8 | s | 0.067 | 0.072 | 0.068 | 0.068 | 0.071 | 0.071 |
| cfrac | s | 2.106 | 2.101 | 2.070 | 2.144 | 2.184 | 2.705 |
| espresso | s | 2.249 | 2.485 | 2.231 | 2.293 | 2.266 | 2.436 |

Cgroup peak, MiB:

| Job | asmalloc | glibc | mimalloc | jemalloc | tc per-CPU | tc per-thread |
|---|---:|---:|---:|---:|---:|---:|
| larson-sized 8 | 60.30 | 88.92 | 103.9 | 88.70 | 65.25 | 54.49 |
| larson 8 | 60.02 | 89.93 | 103.3 | 89.46 | 60.65 | 52.93 |
| xmalloc-test 4+4 | 20.95 | 41.56 | 68.00 | 42.58 | 39.03 | 34.03 |
| sh8bench 16 | 119.5 | 165.4 | 213.4 | 161.9 | 119.8 | 121.9 |
| mstress 8 | 128.1 | 252.5 | 284.1 | 139.6 | 160.6 | 161.3 |
| cache-scratch 1 | 0.762 | 0.750 | 2.750 | 1.000 | 2.250 | 1.750 |
| cache-scratch 8 | 2.750 | 2.500 | 4.500 | 2.742 | 4.000 | 3.500 |
| cfrac | 1.734 | 1.445 | 4.527 | 1.410 | 2.812 | 2.377 |
| espresso | 2.797 | 1.334 | 8.537 | 1.721 | 3.070 | 2.750 |

asmalloc's preload library exports only the C names; C++ `new` and `delete` reach it through
libstdc++'s operators, where mimalloc's library defines them itself (section 9).

## 8. Memory after the peak

Measured on the same quiet machine with `bench/probes/memory/` (each process confined to its
CPUs and capped; medians of two or three rounds, which agreed to within 0.1 MiB). Anonymous
resident MiB.

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

For each job behind the best incumbent by more than about 10% on time, a profile (`perf
record`, one run, the same CPUs and confinement as the session) and counters per allocation:
slow-path entries from an instrumented copy of the allocator (not merged), system calls from
`strace -c`, page faults and cache fills from `perf stat`. Single runs; the session's medians
are in section 3.

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

- *tokio spawn* (10 to 18% behind glibc and jemalloc): no allocator symbol is among the top
  entries; the extra cycles (15.8e9 against 12.9e9) are in tokio's own code, with 40% more
  fills from other cores of the same L3, so task cells appear to be reused on a different
  worker from the one that freed them. Inference only.
- *One Vec grown to 256 MiB* (15% behind the mimalloc crate): mimalloc's huge pages; asmalloc
  takes the faults of 4 KiB pages up to the size where grown blocks get huge pages.
- *The same Rust programs preloaded* behave as natively: JSON is 44% behind for cause A.
- *Small heaps (memory)*: the 0.4 to 3 MiB over glibc on jobs of a few MiB is fixed
  footprint: the slab array, the first magazine-area unit, a loaded magazine per class in
  use and the metadata of each region a class has touched.

**What would fix them** (smallest changes, not implemented or measured): A, stop purging
large slots on the free path at 8 MiB and let the clocked decay return them, keeping a
per-free purge only above a much larger bound; the small-class cap similarly. B, let a
class's depot keep what was freed within an interval (shedding at the interval's end)
instead of laundering past a fixed 128 KiB, and fill magazines so they pop in ascending
address order. C, claim a whole bitmap word per atomic operation and continue the descent
instead of restarting it. D, export `operator new` and `delete` from the preload library. E,
prefer magazines from the same L3 domain in the depot.

## 10. Do the memory rules earn their place?

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
section 9 except D, in about 35 seconds a round: two threads passing 300k objects of 4 to
256 KiB (cause A for large blocks), two independent pipelines on different L3 domains (E), a
Box tree of depth 18 built and dropped four times (B), JSON of 8 MiB parsed three times (A
for small classes) and sh8bench preloaded (C). Two more were added later: one buffer grown
by doubling to 4 MiB and freed, a thousand times, and the same to 1 MiB four thousand times,
which time the large path's moves and kept slots (`memory.md`, "Grown blocks kept up to a
budget"). Run it with `--rounds 3` against the
incumbents on every change to the back end. One round on 2026-10-18, asmalloc against the
best incumbent: 351 against 46 ns/alloc, 11.1 against 8.2 ns per chain, 128 against 34
cycles/node, 23.5 against 17.5 cycles/byte, 1.36 against 0.39 s.

## 12. Caveats

- One machine, one kernel, transparent huge pages in `madvise` mode, which decides the
  build-and-drop jobs for mimalloc (the only allocator here that asks for huge pages). A
  session with THP `always` would show the other side.
- Single-threaded build jobs measure every repetition from a cold process, so allocators that
  keep freed memory gain over those that return it; jemalloc's time-based purging never fires
  in runs this short.
- Wall-time metrics of the C programs include process start-up, a few percent of the
  shortest (cache-scratch with eight threads, about 70 ms).
- The sampler reads `smaps_rollup` once a second, which holds a process's `mmap_lock`
  during a page-table walk.
- The RPC client is the bottleneck, so the RPC jobs rank server CPU per request.
- Peak memory on small jobs is mostly fixed footprint.
- The profiles in section 9 are single runs; their counts per allocation are exact, their
  attribution of cycles to symbols is sampled.

## 13. Earlier sessions and changes to the suite

The first session (2026-10-17, at `60ce673`, without asmalloc) ran on a busy machine; its
raw results are `docs/research/data/baselines-2026-10-17.jsonl.gz` and its tables
`docs/research/data/baselines-2026-10-17-detail.md`. Its findings about the incumbents hold
in this session (jemalloc and per-CPU tcmalloc lead cross-thread work among the incumbents,
mimalloc leads build-and-drop through huge pages, jemalloc is the memory floor on large
heaps), with these changes to the suite since, measured at the time:

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
  depth 20), at the `lea 0x8(%rcx)` that waits on the load of the per-CPU slab's top in
  asmalloc's free fast path, inlined into the recursive drop: each free loads the top that
  the previous free stored. That is the allocator's code and was not touched. The walk phase
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
2.2 times.
