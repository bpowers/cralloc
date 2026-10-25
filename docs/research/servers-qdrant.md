# Qdrant: a vector database as a server job

Measured 2026-10-24 on branch `real-servers` (allocator code as at main `46126f0`), with the
large-heap suite (`bench/apps/qdrant/README.md`). Qdrant 1.20.0 runs on CPUs 0-3 with its
cores held out of deep idle, over a million vectors of 384 floats (1.5 GB) with HNSW graphs
and payloads. The load generator runs on CPUs 8-11 with 64 connections. `qdrant search`
sends nearest-neighbour queries (top 10 with payloads, one in five filtered). `qdrant mixed`
is the same with every 20th request an upsert of 20 points, which the optimizers re-index
in the background.

**Storage is in RAM.** Each run's copy of the collection lives on /dev/shm. That is not how
Qdrant is deployed; the job measures the server's CPU and memory with the disk taken out
(below, "Why the storage is in RAM").

## Summary

- **On time Qdrant is a control.** Malloc is about 2% of the server's cycles even with
  glibc: 1.8% on search, 2.5% on mixed. Dot products and HNSW traversal are 60 to 70%.
  Saturated rates are within 3% of each other on both jobs.
- **Cycles a request on mixed rank the run's state, not the allocator.** In every arm, runs
  fall on three levels of work a request (about 11.9, 12.5 and 12.9M cycles). Instructions
  move with cycles, so these are amounts of work, not stalls. Qdrant routes searches between
  two runtimes (`search-cpu`, `search-io`) by observed CPU use; each run settles on one, and
  runs on search-io do about 5% less work. Which one a run settles on looked like chance in
  every arm. Within runs on search-io asmalloc did the least work: 11.90 and 11.96M against
  jemalloc's 12.05 to 12.21M.
- **Memory is the axis, and jemalloc is the smallest.** On mixed at 70%, the anonymous heap
  in MiB (mean / peak under load / a minute after close) was jemalloc 268 / 449 / 131,
  asmalloc 337 / 528 / 195, tcmalloc 416 / 474 / 417, glibc 638 / 700 / 665 and mimalloc
  682 / 797 / 447. asmalloc's peak is 79 MiB (18%) above jemalloc's. Most of the difference
  is 64 B and 128 B pages stranded in partly used 2 MiB units (about 43 MiB at the peak and
  56 MiB after close); the huge-tail prototype's `t1` returns three-quarters of it ("The
  peak").
- **Tails at 70% do not separate the allocators** once the disk is out (p99 196 to 230 ms
  for every arm). They are upserts that wait for their write behind the update queue.

## Why the storage is in RAM

The first rounds ran on the disk, each run on a reflink copy of the collection. At saturation
the mixed job was bimodal. Normal runs kept 3.82 to 3.88 CPUs busy at 1,670 to 1,740 req/s;
slow runs kept 3.1 to 3.4 busy at 1,270 to 1,480. Slow runs came in clusters in time and hit
every allocator: in one session the last four runs in a row (asmalloc, tcmalloc, jemalloc,
asmalloc) were all slow. The 7% gap first reported against asmalloc was one of those
clusters.

What the threads waited on was measured from every context switch on the server's CPUs,
with its kernel call chain (`record.py --mode sched`, `offcpu.py`), and was the same in
every arm. The searchers waited on page faults in Qdrant's payload store (blobstore
decompression, then `filemap_read_folio`, then `io_schedule`): 1.7 to 2.0 s of 12
searcher-seconds per 3 s window. Address-space lock waits were under 0.01 s in every arm,
so nothing an allocator does with `mprotect` or `madvise` held them up. Two things put
those reads on the disk:

- A new inode starts with a cold page cache, so each run faulted the copy in: about 114k
  major faults a run. Reading the copy once before the load (tried in the runner) cut that
  to 83k.
- The optimizers write new segments at about 150 MB/s (the files grew from 1.9 to 4.5 GB
  in 16 s) and the searchers fault parts of them back from the disk. So the job reads the
  disk by design, and its time followed the disk's load, which the machine shares.

With the copy on /dev/shm (`RamDir`: the runner makes `benchrun-ram-<pid>-<n>`, removes it
however the run ends, sweeps those of dead runners, and will not start a run with less than
10 GiB of memory available) every arm keeps 3.94 to 3.97 CPUs busy at saturation and the
searchers have no file waits.

## Method

- Builds: Qdrant's release profile, fat LTO with one codegen unit, one single build per
  allocator (21 to 57 minutes each here). No placement sweep: with a 2% allocator share,
  and run states that move the work by 5%, a sweep could not resolve an allocator effect.
- Saturation, RAM storage, two rounds (req/s): search mimalloc 3,528, asmalloc 3,593,
  glibc 3,576, jemalloc 3,628, tcmalloc 3,635. Mixed: asmalloc 1,804, glibc 1,822,
  mimalloc 1,831, tcmalloc 1,855, jemalloc 1,866. The 70% rates are 2,470 and 1,260.
- Memory and tails: `servers-thread`: the fill, 30 s at 70%, a 30 s silence, a burst, then
  a minute after close; five rounds, all five allocators (jemalloc 3 valid: 2 runs were
  invalid for other work on the server's CPUs). Heap is anonymous memory from the rollups
  (`bench/probes/standings/heapsum.py`).
- Work by run: four more rounds of asmalloc, jemalloc and tcmalloc with every thread's CPU
  time by name over the window (`bench/probes/server-profile/threadcpu.py`).
- Memory system calls, `strace -c -f` for 10 s at saturation: asmalloc 776 madvise, 66
  munmap, 109 mmap, 22 mprotect; jemalloc 1,829 madvise; tcmalloc 256; mimalloc 602; glibc
  12,298 mprotect.

## Results

`qdrant mixed` at 1,260 req/s, medians over runs:

| Allocator | Mcycles/req | p50 / p99 / p99.9 ms | faults/req | heap mean / peak / +1 min, MiB |
|---|---|---|---|---|
| jemalloc | 12.51 | 0.8 / 230 / 282 | 18.6 | 268 / 449 / 131 |
| asmalloc | 12.83 | 1.3 / 205 / 238 | 14.0 | 337 / 528 / 195 |
| tcmalloc | 12.90 | 1.4 / 221 / 272 | 7.8 | 416 / 474 / 417 |
| glibc | 12.94 | 1.5 / 196 / 247 | 7.4 | 638 / 700 / 665 |
| mimalloc | 13.07 | 2.0 / 208 / 233 | 5.8 | 682 / 797 / 447 |

The cycles column is not a ranking: see the summary.

## The peak

**asmalloc's extra over jemalloc is mostly small-class pages stranded in partly used 2 MiB
units.** The 64 B and 128 B classes, which the optimizers fill in bursts, carve past the
16 MiB point and go onto huge pages, where the purge returns only wholly free 2 MiB units.
After a burst a few live objects per unit keep whole free 4 KiB pages resident, and they
stay there after the load stops.

Accounts with the memstat builds, one run each, MiB:

- meili-peak's sampler (0afff09, allocator code as at 46126f0), at the sample with the most
  anonymous memory (509): live 126 small and 280 large; free on wholly free small pages 76,
  almost all in the 64 B and 128 B classes; free large slots in the hold 27; parked 5;
  metadata 2. Averaged over the 70% window: live 53 small and 200 large, free small pages
  127, free large 27.
- jemalloc's statistics (`je_peak.rs`, the same commit), at its peak (427 anonymous): asked
  for 245, rounding 30, dirty 150, metadata 27. Averaged: dirty 72, metadata 30.
- heap-drift's sampler, which maps each huge-page unit by occupancy, with the huge-tail
  prototype's allocator (main 81437b0 plus a switch) and the switch off (`t0`). At the RSS
  peak (heap 533): returnable now 45, stuck 46, of which 42.5 is whole free 4 KiB pages in
  units that hold something. In the 64 B class: live 2.8, stuck 43.5, huge part 54. At the
  end of the run: stuck 56 (64 B 43, 128 B 11) with nothing live in either class, and 81
  returnable that the clock has not yet returned.
- The same with the switch on (`t1`: the tail above the frontier loses its huge-page advice
  and its free pages go back): stuck 11.7 at the peak.

**The prototype on this job** (one binary, the switch by environment, three rounds
interleaved, medians in MiB):

| ASMALLOC_HT | heap mean / peak / +1 min (RSS anon) | cgroup anon, same | AnonHugePages peak | faults/req |
|---|---|---|---|---|
| t0 (main 81437b0) | 447 / 535 / 195 | 446 / 539 / 195 | 170 | 13.6 |
| t1 | 409 / 496 / 148 | 411 / 502 / 148 | 118 | 14.0 |
| t1p30 | 445 / 542 / 170 | 448 / 543 / 170 | 176 | 13.7 |
| t1p120 | 449 / 544 / 202 | 450 / 545 / 202 | 170 | 13.7 |

t1 takes 8.5% off the mean, 7% off the peak and 24% off the memory a minute after close, for
3% more page faults; cycles a request are level within runs of the same routing state (all 8
runs of rounds 2 and 3 settled on search-cpu). The delayed settings do not act within a run
this long. Main 81437b0 itself (the rule for outgrown large slots) did not move this job:
t0's peak and after-close match the earlier arm's (528 and 195).

Whether the extra is a constant or a share: doubling Qdrant's `indexing_threshold`, meant to
double the optimizers' segments, lowered the peak (405 against 509 in one run), since fewer
optimizations finish in the window; the load could not be scaled that way, so this is not
measured. The mechanism, stranded pages of classes that grow in bursts, scales with the
burst rather than being fixed.

## Unresolved

- What makes a run settle on one search runtime or the other, and the third level of work
  within runs on search-cpu.
- `qdrant search` was measured at saturation only.
