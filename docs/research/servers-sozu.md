# Sōzu: a reverse proxy as a server job

Measured 2026-10-21 on branch `apps-sozu` (allocator code as at `ade62be`), with the servers
suite (`bench/apps/README.md`, "Servers" and "A proxy"): Sōzu 2.3.3 on CPUs 0-3 with four
worker processes, its cores held out of deep idle; the upstream (`bench/probes/loadgen`'s
`upstream`, 4 KiB answers) on 12-14; the load generator on 8-11, 64 connections of the order
service's POST. The server's day at 30% and 70% of the slowest saturated rate, three rounds as
separate sessions, allocators in a new random order every round. No run invalid, and none with
other work on the server's CPUs. Raw results:
`docs/research/data/servers-sozu-2026-10-21.jsonl.gz`.

## Results

Every allocator saturates at 263k to 276k req/s (snmalloc slowest). At 70% (186k req/s), cycles
per request (median of three, ± half the range), p99.9 in us, and the cgroup's peak memory
(every process of the proxy, the executable's shared pages once):

| Allocator | cycles/req | p99.9 | cgroup peak MiB | over glibc | huge-page-backed MiB |
|---|---|---|---|---|---|
| glibc | 89.2k ± 1.8% | 95 | 11.4 | 1.00 | 0 |
| jemalloc | 93.8k ± 0.9% | 62 | 18.6 | 1.63 | 0 |
| asmalloc | 92.2k ± 3.4% | 75 | 24.1 | 2.11 | 0 |
| tcmalloc per-CPU | 92.4k ± 2.9% | 81 | 35.2 | 3.09 | 0 |
| snmalloc | 96.2k ± 0.6% | 76 | 50.7 | 4.45 | 0 |
| mimalloc | 92.5k ± 0.1% | 66 | 52.0 | 4.56 | 44 |

The control (mimalloc-ctl) is within 0.2% of mimalloc. No run had a direct compaction.

## What it shows

- **The allocator barely moves the work.** Apart from snmalloc (4% over the rest), the
  allocators are within the spread between runs of one allocator (up to 7%, glibc to asmalloc
  alike). Sōzu parses and rewrites each request through buffers from its own pool, so the
  allocator is called little on the request path. As a ranking job it is a control.
- **It is a memory job, and a fixed-cost one.** The heap is small and does not grow with load:
  every allocator holds the same from the first burst to ten seconds after the connections
  close. What differs is what each allocator costs a process, times five processes (the main
  process and four workers): asmalloc about 2.5 MiB a process over glibc, jemalloc 1.4,
  tcmalloc 4.8, snmalloc 7.9, mimalloc 8.1. On a heap this small the ratio to glibc says more
  about the fixed cost than about how an allocator manages memory; on the large-heap jobs
  asmalloc holds the least over live (`servers-large.md`).
- **mimalloc gives each process huge pages** (44 of its 52 MiB), which is most of its cost
  here; no other allocator has any.

## Method notes

- The runner gained three things for this job: an upstream it starts first and whose address
  it passes to the server (`{upstream}`); memory sampled over every process of the run's
  cgroup, with the processes' shared pages counted once; and page faults taken from the
  counter that follows forks. Before the shared pages were counted once, the summed resident
  set counted the 180 MB executable's text in every process (53 to 97 MiB); the cgroup peak,
  which ranks memory, was never affected.
- The other-work check takes the smaller of two per-CPU clocks and leaves out `ksoftirqd`
  (`bench/README.md`, "Other work on the job's CPUs"). A proxy defers a third of a CPU of
  network work to `ksoftirqd` at saturation, which the first version of the check counted as
  other work.
- Cycles per request at 70% moved by 1 to 3% between two sessions of the same allocators an
  hour apart (glibc 92.3k in the first, 89.2k in the second); the table is the second, after
  the memory fix.
