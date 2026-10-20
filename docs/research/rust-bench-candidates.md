# Real Rust programs as allocator benchmarks: screening

Screened 2026-10-19 with asmalloc at commit `515412e` (`libasmalloc_preload.so`, release
build, rseq offset check clear), against glibc 2.43, jemalloc 5.3.0 (Fedora's) and mimalloc
3.5.3 (`bench/baselines`), on the reference host (Ryzen 9950X, Linux 7.2.9, THP `madvise`
with defrag `madvise`). 32 programs screened in 34 configurations; 15 recommended plus two
controls.
Raw data: `data/rust-bench-candidates-2026-10-19.tar.gz` (every run, perf summary and
allocation profile, as JSON lines) and `data/rust-bench-candidates-2026-10-19-detail.txt`
(per-allocator medians). The tools are in `bench/probes/rust-screen/` and
`bench/probes/alloc_profile.rs`.

## Summary

- **The allocator matters in most real Rust programs that do real work.** 19 of 32 differ by
  8% or more in cycles between the best and worst allocator, well beyond their noise in
  cycles. glibc is slowest almost everywhere; between jemalloc, mimalloc and asmalloc the
  differences are mostly 3 to 15%.
- **Wall time is the wrong first metric for parallel programs.** Nine parallel candidates
  (ty, gix, oxlint, ruff check, tokei, tpchgen, wasmtime, oxipng, difftastic) have wall times
  that jump by 20 to 70% from run to run under every allocator, while their cycles and CPU
  time stay within a few percent. The jumps come from how the program schedules its own work
  (rayon, salsa, gitoxide's pipeline), and from the kernel's direct compaction when an
  allocator asks for huge pages (below). The suite should rank parallel programs by CPU time
  or cycles, run more rounds, or fix their thread count.
- **Ten of the 32 programs carry jemalloc or mimalloc.** ruff and ty, datafusion-cli,
  difftastic and egglog set it as the `#[global_allocator]` unconditionally (patched out
  here); oxlint, uv, boa and jaq turn it on by default or in release builds (built without);
  fd offers it as an option. Unpatched, preloading measures nothing: difftastic looked
  insensitive (2.7% spread) until its jemalloc was taken out (18% in cycles). That a third
  of these programs ship their own allocator is evidence in itself, and each will need the
  same patch in the suite.
- **asmalloc ran every program correctly.** No abort, hang or wrong output in over 400 runs:
  the
  standard output of every candidate matched across allocators (egglog and tantivy print
  their own timings).
- **asmalloc is fastest on boa (10% ahead of jemalloc), single-threaded ruff check and
  pulldown-cmark**, level on the servers and the controls, and **5 to 13% behind mimalloc**
  on rust-analyzer, rhai, egglog, typst and cargo, and 10% in cycles on datafusion. It takes
  many more page faults where blocks of 1 to 32 KiB cross threads (tpchgen: 713k faults
  against glibc's 275k to 316k and mimalloc's 78k, with 9,400 `madvise` and 2,400
  `mprotect` calls a run).

## Method

Each candidate is built from a pinned release with the system allocator (`build.sh` from
crates.io, `build-src.sh` from a release tarball, patched where the program picks an
allocator itself). Each run is a fresh process started under `perf stat` with the
allocator's library in `LD_PRELOAD` (glibc runs preload nothing but the suite's probe),
pinned like the suite: CPU 2 for single-threaded programs, CPUs 0-3 and 8-11 for parallel
ones, servers on 0-3 with a closed-loop client on 8-11. Every run is preceded by a 300 ms
spin on its CPUs so that it starts at the boost clock. Three rounds, the allocators in a new
random order each round, all under `scripts/measure --both-l3` and an 8 GiB cap. Recorded:
wall time (taken inside `perf stat`, which otherwise adds 10 to 50 ms), user and kernel
cycles, instructions, CPU time, page faults, peak resident set (the process's own `VmHWM`
from the suite's probe, which also checks that the preloaded library served `malloc`), and a
hash of standard output.

Separately, under glibc: `perf record` of all cycles for the share in glibc's allocator
functions, in the kernel, and under page faults; and one run under `alloc_profile.so`, a
preload recorder written for this (it wraps glibc's malloc with a 16-byte header), for
calls, sizes, cross-thread frees and reallocs, realloc growth, peak live bytes and blocks,
and lifetimes. For a few candidates, `syscalls.py` counted context switches with `perf` and
memory-map system calls with `strace`.

**Preloading measures the C entry points**: `malloc`, `free`, `realloc` and
`posix_memalign`, never Rust's sized `dealloc` or a native `#[global_allocator]`. A suite
built with each allocator as the global allocator may rank them differently, especially
where sized deallocation helps (mimalloc and asmalloc).

Columns: *Time* is the median wall time of each allocator over glibc's. *Noise* is the
largest half-range of one allocator's three runs, as a share of its median. *Cycles
spread* is the largest median user and kernel cycles over the smallest. *RSS* is peak
resident set over glibc's; a dash marks a shell loop, where the probe sees the shell.
*Alloc share* is the share of all cycles in glibc's allocator functions; glibc's
thread-cache fast path costs only a few cycles a call, so this understates what a slower
path would cost. *Kernel (faults)* is the share of cycles in the kernel and under page
faults. *Calls/s* is the profiled run's calls over glibc's wall time. *Cross-thread frees*
counts frees and reallocs by a thread other than the one that allocated. *Peak heap* is the
peak of requested bytes live, and blocks live, in the profiled run. *Build* is the wall time
of the build on 8 shared cores, often paused by others' measurements: a rough guide only.
Thread counts are from the profiled run, which had 8 CPUs.

| Candidate | Version | Input | CPUs (threads) | glibc time | Time je / mi / asm | Noise | Cycles spread | glibc peak RSS | RSS je / mi / asm | Alloc share | Kernel (faults) | Calls/s | Cross-thread frees | Reallocs | Peak heap (blocks) | Build |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| actix | actix-web 4.17.0 | POST JSON orders, 400k, 64 conns | 4 (10) | 1.09 s | 0.85 / 0.85 / 0.84 | 0.8% | 24% | 288 MiB | 0.72 / 0.76 / 0.72 | 14.4% | 39% (1%) | 45.6M | 48.81% | 6.41% | 181.4 MiB (4.50M) | - |
| axum | axum 0.8.9 | POST JSON orders, 400k, 64 conns | 4 (9) | 1.34 s | 0.90 / 0.95 / 0.93 | 3.4% | 15% | 285 MiB | 0.72 / 0.75 / 0.72 | 11.4% | 37% (1%) | 81.2M | 23.63% | 2.93% | 180.9 MiB (4.50M) | - |
| bat | 0.26.1 | highlight mm/*.c, 6 MB | 1 (2) | 2.18 s | 0.99 / 0.95 / 0.96 | 1.5% | 7% | 11 MiB | 1.10 / 1.72 / 1.09 | 7.6% | 2% (0%) | 25.5M | 0.0% | 9.32% | 2.8 MiB (0.01M) | 40 s |
| boa | 0.22.0 | Octane splay, 1/4 iterations | 1 (2) | 3.16 s | 0.91 / 1.01 / 0.82 | 1.6% | 23% | 270 MiB | 0.93 / 1.00 / 0.92 | 9.1% | 2% (1%) | 7.6M | 0.0% | 9.64% | 217.6 MiB (2.92M) | 338 s |
| cargo-resolve | cargo 1.99.1 | resolve ruff's workspace ×10 | 1 (1) | 1.88 s | 0.94 / 0.79 / 0.89 | 3.1% | 25% | - | - | 18.1% | 20% (11%) | 1.9M | 0.0% | 4.55% | 127.2 MiB (0.56M) | - |
| datafusion | 54.1.0 | TPC-H SF1, 22 queries | 8 (29) | 1.03 s | 0.86 / 0.97 / 0.99 | 8.4% | 10% | 447 MiB | 1.50 / 2.25 / 1.02 | 2.3% | 11% (5%) | 9.8M | 7.81% | 2.1% | 328.4 MiB (0.04M) | 400 s |
| difftastic | 0.71.0 | asyncio, CPython 3.12.7 vs 3.13.0 | 8 (10) | 0.56 s | 1.00 / 0.79 / 1.01 | 30.5% | 18% | 557 MiB | 1.00 / 1.59 / 0.91 | 4.4% | 19% (16%) | 10.0M | 7.55% | 3.22% | 435.5 MiB (0.32M) | 46 s |
| egglog | 3.0.0 | four of its larger tests | 1 (1) | 2.18 s | 0.96 / 0.86 / 0.93 | 0.6% | 16% | - | - | 12.7% | 4% (4%) | 12.6M | 0.0% | 0.17% | 36.1 MiB (0.23M) | 33 s |
| fd-loop | 10.5.0 | 30 walks of Linux 6.12 | 8 (10) | 0.43 s | 0.87 / 0.90 / 0.91 | 3.0% | 13% | - | - | 17.7% | 32% (1%) | 1.8M | 9.35% | 13.62% | 7.2 MiB (0.01M) | 230 s |
| gix-hours | 0.60.0 | estimate-hours, cargo repo, 20k commits | 8 (13) | 1.59 s | 1.37 / 0.87 / 1.41 | 44.8% | 11% | 2676 MiB | 1.18 / 1.25 / 1.10 | 13.0% | 7% (4%) | 84.8M | 0.02% | 30.39% | 2566.2 MiB (0.29M) | 144 s |
| jaq | 3.1.1 | group_by over citylots.json, 189 MB | 1 (1) | 2.34 s | 0.84 / 0.85 / 0.87 | 13.8% | 20% | 2030 MiB | 0.87 / 0.86 / 0.86 | 37.2% | 11% (10%) | 53.5M | 0.0% | 1.39% | 1325.9 MiB (38.11M) | 61 s |
| mini-redis | 0.4.1 | 1.6M GET/SET, 64 conns | 4 (9) | 2.36 s | 1.00 / 0.98 / 0.97 | 1.8% | 3% | 230 MiB | 1.08 / 1.13 / 1.07 | 1.3% | 65% (0%) | 9.7M | 7.93% | 0.0% | 210.2 MiB (0.25M) | 11 s |
| oxipng | 10.2.1 | -o 4 on 40 PNGs | 8 (9) | 2.16 s | 1.16 / 1.00 / 1.28 | 11.9% | 1% | 122 MiB | 0.91 / 2.19 / 1.11 | 0.1% | 1% (0%) | 0.2M | 0.01% | 0.56% | 129.6 MiB (0.00M) | 12 s |
| oxlint | oxlint_v1.88.0 | -W all, 2805 npm .js files | 8 (17) | 0.88 s | 0.98 / 0.80 / 0.96 | 18.1% | 10% | 1222 MiB | 1.02 / 1.17 / 0.97 | 10.1% | 16% (10%) | 37.9M | 41.47% | 10.41% | 1184.2 MiB (7.59M) | 524 s |
| parquet-rewrite | 60.0.0 | lineitem SF1 to zstd | 1 (1) | 1.98 s | 1.00 / 0.99 / 1.03 | 1.4% | 4% | 73 MiB | 1.01 / 1.70 / 0.98 | 0.6% | 4% (1%) | 0.9M | 0.0% | 1.93% | 51.2 MiB (0.01M) | 92 s |
| pulldown-cmark | 0.13.4 | 470 MB of reST read as Markdown | 1 (1) | 1.04 s | 1.08 / 1.04 / 0.95 | 0.7% | 13% | 541 MiB | 1.00 / 1.29 / 1.00 | 0.1% | 19% (10%) | 0.1M | 0.0% | 5.28% | 1133.4 MiB (0.03M) | 4 s |
| rav1e | 0.8.1 | 60 CIF frames, speed 6 | 8 (9) | 4.35 s | 0.99 / 0.99 / 0.99 | 1.5% | 0% | 78 MiB | 1.28 / 2.29 / 0.91 | 0.1% | 1% (0%) | 0.1M | 1.03% | 2.33% | 60.1 MiB (0.00M) | 41 s |
| rhai | 1.26.1 | maps, arrays, closures script | 1 (1) | 1.94 s | 1.02 / 0.95 / 1.01 | 0.8% | 9% | 9 MiB | 3.71 / 1.52 / 1.11 | 9.3% | 0% (0%) | 36.5M | 0.0% | 0.02% | 3.5 MiB (0.03M) | 56 s |
| ripgrep | 15.2.0 | regex over Linux 6.12 ×4 | 8 (9) | 1.30 s | 1.00 / 0.99 / 0.99 | 1.1% | 0% | 17 MiB | 1.29 / 6.31 / 1.10 | 0.6% | 22% (0%) | 2.7M | 0.35% | 11.5% | 7.5 MiB (0.01M) | 573 s |
| ruff-check | 0.17.1 | check --select ALL, CPython Lib | 8 (17) | 0.78 s | 0.94 / 0.87 / 1.07 | 14.5% | 12% | 400 MiB | 0.96 / 1.35 / 0.95 | 10.3% | 6% (3%) | 36.8M | 26.72% | 2.76% | 296.7 MiB (3.96M) | 1055 s |
| ruff-check-st | 0.17.1 | same, one thread | 1 (1) | 2.27 s | 0.91 / 0.88 / 0.87 | 0.8% | 15% | 372 MiB | 0.92 / 1.02 / 0.95 | 9.8% | 5% (3%) | see ruff-check | | | | 1055 s |
| ruff-format | 0.17.1 | format --diff, CPython Lib | 8 (17) | 0.72 s | 1.02 / 1.01 / 1.10 | 3.1% | 3% | 130 MiB | 1.16 / 2.60 / 1.51 | 4.8% | 11% (1%) | 10.5M | 0.22% | 3.85% | 97.5 MiB (0.10M) | 1055 s |
| rust-analyzer | 2026-10-12 | analysis-stats on ripgrep | 8 (12) | 4.71 s | 0.95 / 0.88 / 0.92 | 1.5% | 13% | 1271 MiB | 1.02 / 1.06 / 0.99 | 8.8% | 7% (4%) | 24.5M | 6.63% | 1.49% | 1079.9 MiB (10.66M) | 245 s |
| rustpython | 0.4.0 | richards, strings, trees, dict churn | 1 (1) | 3.74 s | 1.00 / 0.99 / 1.00 | 0.2% | 1% | 23 MiB | 1.14 / 1.44 / 1.19 | 5.3% | 0% (0%) | 22.7M | 0.0% | 1.36% | 8.9 MiB (0.06M) | 74 s |
| starlark | 0.14.2 | BUILD-like dicts and lists | 1 (1) | 1.30 s | 0.99 / 0.92 / 0.97 | 0.9% | 8% | 1552 MiB | 0.99 / 1.00 / 0.98 | 3.1% | 18% (16%) | 26.3M | 0.0% | 0.01% | 2349.0 MiB (1.80M) | 168 s |
| tantivy | 0.24.0 | index 300 MB of docs and C source | 8 (198) | 4.44 s | 0.94 / 1.00 / 1.02 | 8.0% | 4% | 625 MiB | 0.87 / 1.45 / 1.03 | 1.4% | 6% (2%) | 6.3M | 0.35% | 1.91% | 507.0 MiB (0.10M) | 64 s |
| tokei | 15.0.0 | Linux 6.12 + CPython 3.13 trees | 8 (11) | 1.02 s | 0.84 / 0.98 / 0.98 | 19.7% | 5% | 192 MiB | 1.12 / 1.87 / 0.84 | 1.0% | 9% (1%) | 3.5M | 4.22% | 27.63% | 144.0 MiB (0.18M) | 130 s |
| tpchgen | 4.0.0 | TPC-H SF1 to Parquet | 8 (13) | 1.27 s | 0.94 / 1.11 / 1.14 | 19.0% | 7% | 518 MiB | 1.09 / 1.56 / 1.12 | 0.7% | 6% (4%) | 0.9M | 7.4% | 5.49% | 470.7 MiB (0.03M) | 163 s |
| ty | 0.17.1 | check CPython Lib | 8 (18) | 3.46 s | 0.80 / 0.83 / 0.75 | 33.9% | 6% | 1388 MiB | 0.99 / 1.15 / 1.01 | 5.9% | 3% (2%) | 33.2M | 0.03% | 6.95% | 1113.8 MiB (9.88M) | 1055 s |
| typst | 0.15.1 | generated 480-section report | 8 (9) | 1.50 s | 0.97 / 0.86 / 0.94 | 1.9% | 14% | 775 MiB | 1.05 / 1.16 / 1.06 | 6.4% | 8% (7%) | 8.7M | 0.03% | 5.49% | 767.5 MiB (1.24M) | 201 s |
| uv-loop | 0.13.1 | resolve 397 packages offline ×20 | 8 (10) | 0.93 s | 0.79 / 0.75 / 0.84 | 3.7% | 17% | - | - | 13.9% | 43% (20%) | 0.7M | 21.42% | 6.33% | 96.0 MiB (0.13M) | 554 s |
| wasmi | 2.0.0 | pulldown-cmark.wasm over 240 MB | 1 (1) | 4.56 s | 1.02 / 1.02 / 0.98 | 2.4% | 4% | 461 MiB | 1.07 / 1.29 / 1.01 | 0.0% | 2% (1%) | 0.0M | 0.0% | 2.11% | 457.3 MiB (0.00M) | 363 s |
| wasmtime | 44.0.3 | compile esbuild.wasm, 12 MB | 8 (10) | 0.84 s | 1.03 / 0.92 / 1.05 | 35.7% | 5% | 558 MiB | 0.81 / 1.30 / 0.86 | 2.4% | 3% (2%) | 9.8M | 1.94% | 13.19% | 334.1 MiB (0.11M) | 202 s |
| xsv | 0.13.0 | frequency, lineitem SF1 CSV | 1 (1) | 2.17 s | 0.99 / 0.95 / 0.96 | 3.3% | 6% | 30 MiB | 0.94 / 1.76 / 1.01 | 4.6% | 2% (0%) | 71.9M | 0.0% | 0.0% | 32.4 MiB (0.21M) | 12 s |

ty and gix-hours have six and five runs per allocator; the rest three. Sizes, realloc growth
ratios and lifetimes per candidate are in the raw profiles. A few shapes stand out: in almost
every program 75 to 100% of blocks are 256 bytes or less; tpchgen (58% between 1 and
32 KiB), oxipng, datafusion (6% over 4 KiB) and pulldown-cmark (one buffer grown to 1 GiB)
are the exceptions. Reallocs are 30% of gix's calls and 28% of tokei's, 90 to 99% of them
doubling. The median block lives under 1 µs in egglog, rhai, starlark, axum, mini-redis
and rust-analyzer, and about as long as the process in jaq, boa and actix.

## Recommended set

Fifteen programs and two controls. "Signal" is the cycles spread against its noise.

| Pick | Threads | Cross-thread frees | Sizes | Peak heap | Realloc growth | Async | Kind | Signal |
|---|---|---|---|---|---|---|---|---|
| jaq | 1 | none | small | 1.3 GB, 38M blocks | low | no | batch | 20%, but time moves with THP state |
| boa | 1 | none | small | 220 MB | moderate | no | batch, GC | 23%, noise 1.6% |
| egglog | 1 | none | small, short-lived | 36 MB | low | no | batch | 16%, noise 0.6% |
| rhai | 1 | none | small | 3.5 MB | none | no | batch | 9%, noise 0.8% |
| pulldown-cmark | 1 | none | one huge buffer | 1.1 GB | high | no | batch | 13%, noise 0.7% |
| ruff check, one thread | 1 | none | small | 300 MB | low | no | batch | 15%, noise 0.8% |
| ruff check, parallel | 8 | 27% | small | 300 MB | low | no | batch | 12% in cycles; wall noisy |
| rust-analyzer | 8 | 7% | small | 1.1 GB, 11M blocks | low | no | batch | 13%, noise 1.5% |
| typst | 8 | ~0 | small to 1 KiB | 770 MB | moderate | no | batch | 14%, noise 1.9% |
| oxlint | 8 | 41% | small | 1.2 GB | moderate | no | batch | 10% in cycles; wall noisy |
| datafusion | 8 (29) | 8% | mixed, to 256 KiB | 330 MB | low | yes (tokio) | batch | 10% in cycles; wall noise 8% |
| gix-hours | 8 | ~0 | small | 2.6 GB | 30% of calls | no | batch | 11% in cycles; wall bimodal |
| tpchgen | 8 | 7% | 1 to 32 KiB | 470 MB | moderate | no | batch | 7% in cycles; asmalloc faults |
| axum | 4 | 24% | small | 180 MB | low | yes, work stealing | server | 15%, noise 3.4% |
| actix | 4 | 49% | small | 180 MB | moderate | yes, thread per core | server | 24%, noise 0.8% |
| uv-loop | 8 | 21% | small | 96 MB each | moderate | yes | 20 short processes | 17%, noise 3.7% |
| control: ripgrep | 8 | ~0 | small | 7 MB | moderate | no | batch, I/O | 0% |
| control: rav1e | 8 | 1% | frames | 60 MB | low | no | compute | 0% |

Why each:

- **jaq**: the most allocation-bound program found (37% of glibc's cycles, 53M calls/s) and
  the largest live set (38M blocks); what a data tool with a tree-shaped heap looks like.
- **boa**: a garbage-collected interpreter with 3M live objects; asmalloc's best case so far
  (10% ahead of jemalloc), and a check that it stays so.
- **egglog**: equality saturation; the shortest lifetimes (median under 100 ns) on a small
  heap. mimalloc leads by 8%.
- **rhai**: an interpreter whose heap stays under 4 MB, so it isolates the fast path from
  memory behaviour. mimalloc leads by 6%.
- **pulldown-cmark**: the one realloc-grown giant buffer: the allocator's code is 0.1% of
  cycles, yet allocators differ by 13% through page faults and how a 1 GiB block grows.
- **ruff check**, in both forms (the harness teammate already has ruff): one thread for a
  clean signal (asmalloc 1% ahead of mimalloc), eight threads for 27% cross-thread frees.
- **rust-analyzer**: salsa's memoised graph, 11M live blocks, steady and long enough (4 to
  5 s) to need no tricks. asmalloc is 5% behind mimalloc.
- **typst**: layout and PDF writing with a steady 770 MB heap; mimalloc leads by 9%.
- **oxlint**: the most cross-thread frees outside the servers (41%), with an arena-backed
  AST; the JavaScript toolchain entry.
- **datafusion** (already chosen): the widest size mix and the most threads (29).
- **gix-hours**: realloc-heavy (30% of calls) on a 2.6 GB heap. Its wall time is bimodal
  under every allocator; fix its thread count or rank it by CPU time.
- **tpchgen**: the only program dominated by 1 to 32 KiB blocks, and the one where asmalloc's
  page faults stand out (about 2.5 times glibc's).
- **axum** and **actix**: the same order service on the two runtime models; actix frees half
  its blocks on another thread. If only one, keep actix: lower noise, larger spread.
- **uv-loop**: short-lived processes, where start-up, first faults and the kernel (43% of
  cycles) dominate. uv ships jemalloc.
- **ripgrep** and **rav1e** as controls: zero spread in cycles, so a move there is the setup.

Conditional: **cargo-resolve** (25% spread, mimalloc 13% ahead of asmalloc) is a strong
short-lived candidate but resolves against `~/.cargo`'s index, which other builds on the
machine change; it needs a private, pinned `CARGO_HOME` first.

## Rejected and why

- **rustpython, wasmi, parquet-rewrite, mini-redis**: under 4% in cycles. RustPython spends
  5% in the allocator at 23M calls/s and still does not move; mini-redis spends 65% of its
  cycles in the network stack. Keep rustpython in mind as a third control.
- **tokei, tantivy, oxipng, wasmtime**: 1 to 5% in cycles, with noisy wall times from their
  own scheduling.
- **ty**: 6% in cycles, wall times spread from 2.0 to 3.6 s under every allocator.
  rust-analyzer covers the same salsa workload with steady times.
- **difftastic**: 18% in cycles, but a 0.6 s run with 30% wall noise; worth a larger input
  later (two whole CPython trees take over a minute).
- **xsv, bat**: 6 to 7% in cycles with small heaps; covered by rhai and egglog.
- **starlark**: 8% (mostly faults on a 2.3 GB heap); covered by jaq.
- **ruff format**: 3% in time, but note asmalloc's peak RSS there, 1.51 times glibc's
  (196 against 130 MiB), its worst memory result.
- **fd-loop**: 13% in cycles but 0.4 s; uv-loop covers short-lived processes better.

Not screened:

- **swc**: not reached in the time box; oxlint covers JavaScript tooling.
- **polars**: the Python wheel links jemalloc statically, so preloading cannot reach it; a
  Rust-only run needs a driver of our own and a long build.
- **pingora**: a proxy needs an upstream and a load generator wired together; not reached.
- **tonic gRPC**: the suite's RPC benchmarks (connect-rust) already cover a Rust RPC server.
- **iggy, fjall, redb, simd-json, syn, regex, egg as libraries**: no program to run without
  writing a driver, against the "real programs" criterion; ripgrep covers regex, egglog egg.
- **1BRC**: good solutions avoid allocation by design.
- **wild**: needs a captured link (object files and the linker line); not reached.
- **mdbook, zola**: built mdbook but did not screen; pulldown-cmark is their parser.
- **bevy_ecs, rapier**: no headless command-line program; rav1e and oxipng were the two
  parallel compute screens, and both confirm insensitivity.
- **boa on the rest of Octane**: richards, deltablue and raytrace take 25 to 50 s each in boa
  even at a quarter of their iterations.
- **mini-redis pipelined**: mini-redis writes each reply without `TCP_NODELAY`, so a
  pipelined batch waits 40 ms on Nagle and delayed ACK; screened unpipelined.

## What the screening says about asmalloc

These are leads for the lead, not conclusions; each rests on three runs.

- **Faults and `madvise` with mid-size blocks across threads.** tpchgen under asmalloc takes
  713k page faults and makes 9,400 `madvise` and 2,400 `mprotect` calls a run; glibc 275k
  faults and 101 `madvise`, mimalloc 78k and 25. gix-hours: 1.0M faults, 10,500 `madvise`
  and 40,000 `mprotect` (glibc 686k, 85 and 37,900; mimalloc 340k, 29 and 69). Its kernel
  cycles in tpchgen are 2.3G against glibc's 1.3G. Memory is returned and faulted back.
- **Parallel wall time.** In ruff check asmalloc's wall time was 0.79 to 0.83 s in all five
  runs against 0.64 to 0.78 for the others, at the same cycles and the lowest CPU time, with
  about 1,400 context switches a run against 150 to 370 for glibc and jemalloc (mimalloc
  varied, 130 to 1,000). One cause that fits is threads waiting on the mmap lock behind
  asmalloc's `mprotect` calls (2,500 to 3,500 a run), which take it for writing. Not
  established; in oxipng and oxlint the same signature was within those programs' own noise.
- **Huge pages make results depend on the machine's state.** With THP defrag `madvise`, an
  allocator that asks for huge pages (mimalloc, and asmalloc for dense classes) can stall in
  direct compaction. A perf profile of gix-hours under mimalloc had 27% of cycles under page
  faults, in `isolate_freepages_block` and a zone spinlock; under glibc 4%, asmalloc 7%.
  asmalloc's jaq
  times were 1.74 to 2.31 s in the screening and 1.48 to 1.53 s an hour later, with faults
  falling from 180k to 55k. A suite of large-heap programs should record
  `compact_stall` and `thp_fault_alloc` from `/proc/vmstat` around each run.
- **Memory.** asmalloc's peak RSS is within 12% of the smallest incumbent's on all but four
  programs: ruff format (1.51 times glibc), rustpython (1.19, 23 MB), tantivy (1.19 times
  jemalloc) and oxipng (1.22 times jemalloc). mimalloc's peaks are the largest almost
  everywhere (6.3 times glibc on ripgrep, 2.25 on datafusion).

## On the selection criteria

The evidence supports them, with three amendments:

- "More than noise" should be judged in cycles or CPU time for parallel programs, or on a
  fixed thread count; by wall time at three rounds, half the parallel candidates would fail
  or pass by luck.
- "Real programs at pinned releases" has to allow a recorded one-line patch: a third of the
  programs pick their own allocator, and the suite cannot otherwise put another one under
  them.
- "Seconds per run" holds, but several of the best signals come from runs of about one
  second (ruff, actix, typst); a fixed repeat count inside one process is not available for
  them, so they need more rounds instead.

## How far to trust this

The ranking of programs by sensitivity is sound where the cycles spread is several times the
noise; the ranking of allocators within a program is not, at three rounds. Single-threaded
wall times repeat within 1 to 2%. Parallel wall times do not: rank those by cycles or CPU
time. Runs of programs with large heaps also depend on how fragmented the machine's free
memory is, which other people's work changes.

Where a mistake could hide:

- A program whose allocator is compiled in passes every check here except the profile:
  `alloc_profile.so` then sees few calls. Each candidate's sources were searched for
  `#[global_allocator]` and the profiled call counts checked; a dependency that sets one, or
  a C library with its own pools, would still look insensitive.
- Preloaded runs see only C entry points; native Rust builds with sized deallocation could
  move mimalloc and asmalloc relative to the others.
- The allocator share is measured under glibc only, and glibc's fast path is cheap.
- The recorder adds a header and slows calls, so its cross-thread share and lifetimes are
  close but not exact for parallel programs; calls per second divide its count by glibc's
  timed wall time.
- Only standard output was compared across allocators; files written (PDF, Parquet, video)
  were not.
- Shell loops (fd, uv, cargo, egglog) report the shell's peak RSS, so memory for them is
  missing.
- For a fair fixed-load server comparison, drive axum and actix open-loop at about 70% of
  glibc's saturated throughput and measure server CPU per request and tail latency, as the
  suite's RPC driver already does; the closed-loop numbers here measure throughput only.

## Reproducing

`bench/probes/rust-screen/` holds everything: `inputs.sh` and `prep.py` fetch and derive the
inputs (sha256 printed), `sources.sh`, `build-all.sh` (with `builds-crates.txt`) and
`build-src.sh` build the candidates into `target/rust-screen`, `candidates.py` says how each
runs, and `screen-all.sh NAME...` screens one end to end. `screen.py table` prints the table
above from the raw results.
