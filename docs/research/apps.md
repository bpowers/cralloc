# Real Rust programs: first session

Measured 2026-10-19 and 2026-10-20 at the `rust-apps` branch (on `515412e`), with the `apps`
suite (`bench/apps/README.md`): ruff, DataFusion and boa, each with every allocator of
`bench/allocators.toml` as its `#[global_allocator]`. Machine and allocator versions as in
`baselines.md` section 2, plus `snmalloc-rs` 0.7.6.

## Method

`benchrun run --suite apps`, one fresh process per run, allocators in a new random order for
each job in each round, under `scripts/measure --both-l3`, every run capped at 4 GiB. Six
rounds for ruff and boa in two pieces of three (seeds 1 and 2); DataFusion separately, below.
The quiet gate passed before every run without waiting; no run was invalid. The ruff and
boa pieces also ran DataFusion on eight threads across both L3 domains, which is left out
(below). Raw results: `docs/research/data/apps-2026-10-20.jsonl.gz`. Time is the
measured work's wall time; memory is the run's cgroup peak. "±" is half the interquartile
range.

## Results

ruff 0.16.9 checking Home Assistant core 2026.10.2, four threads on one L3 domain:

| Allocator | s | ± | vs mimalloc | Cgroup peak MiB | Faults |
|---|---:|---:|---:|---:|---:|
| glibc | 1.140 | 0.4% | +8.7% | 35.4 | 10.7k |
| mimalloc | 1.049 | 0.2% | 0 | 119 | 0.8k |
| mimalloc-sized | 1.071 | 0.2% | +2.1% | 128 | 0.8k |
| mimalloc-crate | 1.043 | 1.7% | -0.6% | 118 | 1.5k |
| asmalloc | 1.064 | 0.5% | +1.4% | 44.2 | 16.7k |
| jemalloc | 1.101 | 0.5% | +5.0% | 37.8 | 10.7k |
| tcmalloc-percpu | 1.097 | 0.2% | +4.5% | 33.2 | 8.9k |
| tcmalloc-thread | 1.113 | 0.5% | +6.1% | 39.5 | 10.1k |
| snmalloc | 1.064 | 0.5% | +1.4% | 42.2 | 11.1k |
| mimalloc-ctl | 1.051 | 0.8% | +0.2% | 130 | 0.8k |

boa 0.22.0 running the V8 suite (v7), one thread:

| Allocator | s | ± | vs mimalloc | Cgroup peak MiB | Faults |
|---|---:|---:|---:|---:|---:|
| glibc | 8.514 | 0.2% | +4.1% | 263 | 72.7k |
| mimalloc | 8.182 | 0.4% | 0 | 272 | 1.3k |
| mimalloc-sized | 8.250 | 0.2% | +0.8% | 272 | 1.3k |
| mimalloc-crate | 8.317 | 0.3% | +1.7% | 275 | 5.1k |
| asmalloc | 8.042 | 0.0% | -1.7% | 263 | 30.4k |
| jemalloc | 8.579 | 0.1% | +4.9% | 250 | 71.6k |
| tcmalloc-percpu | 8.361 | 0.1% | +2.2% | 253 | 65.3k |
| tcmalloc-thread | 8.503 | 0.3% | +3.9% | 254 | 65.5k |
| snmalloc | 8.274 | 0.1% | +1.1% | 275 | 70.5k |
| mimalloc-ctl | 8.169 | 0.2% | -0.2% | 272 | 1.3k |

DataFusion 55.2.0 running the 22 TPC-H queries at scale factor 3, four threads on one L3
domain (six rounds in two pieces, seeds 3 and 4, 2026-10-20):

| Allocator | s | ± | vs mimalloc | Cgroup peak MiB | Faults |
|---|---:|---:|---:|---:|---:|
| glibc | 3.578 | 0.6% | +2.6% | 881 | 837k |
| mimalloc | 3.488 | 0.3% | 0 | 1681 | 123k |
| mimalloc-sized | 3.504 | 0.5% | +0.4% | 1688 | 123k |
| mimalloc-crate | 3.503 | 0.3% | +0.4% | 1744 | 154k |
| asmalloc | 3.488 | 0.2% | 0.0% | 1204 | 927k |
| jemalloc | 3.614 | 0.2% | +3.6% | 866 | 1029k |
| tcmalloc-percpu | 3.422 | 0.5% | -1.9% | 994 | 453k |
| tcmalloc-thread | 3.485 | 0.2% | -0.1% | 1022 | 449k |
| snmalloc | 3.578 | 0.4% | +2.6% | 1427 | 485k |
| mimalloc-ctl | 3.485 | 0.2% | -0.1% | 1656 | 123k |

## What these show

- **The allocator matters in all three**, by 9% (ruff), 7% (boa) and 6% (DataFusion) in
  time, against a spread between runs of under 1%, and by up to 3.6 times in memory. Each ships
  with jemalloc or mimalloc; jemalloc is among the slowest on all three here.
- **mimalloc buys its time with memory.** It backs its heap with huge pages: ruff's peak is
  3.4 times glibc's (119 against 35 MiB) and DataFusion's 1.9 times (1681 against 881 MiB). The kernels' heaps
  hide this; a program whose heap is tens of megabytes shows it.
- **asmalloc** is the fastest on boa (1.7% ahead of mimalloc), 1.4% behind mimalloc on ruff
  at a third of its memory, and level with mimalloc on DataFusion (1.9% behind per-CPU tcmalloc). Against jemalloc, the memory bar, it is 6 MiB
  (17%) over on ruff, level on boa, and 39% over on DataFusion (1204 against 866 MiB), the
  largest gap to jemalloc on any job so far and the one to look at first. On ruff it takes 56% more page faults than jemalloc
  (16.7k against 10.7k), which the kernels did not show.
- **tcmalloc aborted** in ruff and boa until the bench wrapper stopped keeping a shrunk block
  in place on `realloc` (commit "keep a tcmalloc block in place on realloc only when it
  grows"): tcmalloc checks the size passed to a sized delete of a sampled allocation. No
  kernel shrank a block that way.
- **DataFusion on two L3 domains is bimodal.** On eight threads (CPUs 0-3 and 8-11) every
  allocator's runs fell into two modes, about 2.4 and 3.5 s, nearly every query 30 to 50%
  slower in the slow one, and with glibc the modes alternated run by run. On four threads
  of one domain, eight glibc runs agreed within 1%. The job runs on one domain; the cause is
  not known.

## Build cost

Each program compiles once into a shared build directory; each allocator then compiles
`bench-alloc`, the driver and its allocator and links with the program's own release profile
(`bench/apps/README.md`). On the shared, busy machine (heavy work paused by other
measurements), per program:

| Program | Profile | First build (wall) | Each further allocator (CPU) | Shared build directory | Binary |
|---|---|---:|---:|---:|---:|
| ruff | fat LTO, 16 units | 13 min | about 2 min | 2.8 GB | 29 MB |
| DataFusion | fat LTO, 1 unit | 25 min | about 6.5 min | 2.4 GB | 98 MB |
| boa | fat LTO, 1 unit | 16 min | about 50 s | 1.2 GB | 26 MB |

The control (mimalloc-ctl) costs nothing: its build is the same as mimalloc's. With ruff,
the `ruff` crate itself compiles again for jemalloc, because `bench-alloc` turns on
tikv-jemallocator's default features and ruff depends on that crate with fewer. Inputs take
1.1 GB (Home Assistant's checkout and TPC-H at scale factor 3).
