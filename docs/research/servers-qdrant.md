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

**The candidate rule** (`ht10`: the advice comes off only where pages go back, switch
`t1r2`; one binary linked into 0afff09's Qdrant, three rounds interleaved, all six runs on
search-cpu, medians in MiB):

| ASMALLOC_HT | heap mean / peak / +1 min (RSS anon) | cgroup anon, same | AnonHugePages peak | faults/req | Mcycles/req | p99 / p99.9 ms |
|---|---|---|---|---|---|---|
| t0 | 448 / 542 / 201 | 446 / 543 / 202 | 170 | 13.8 | 12.74 to 12.79 | 189 / 255 |
| t1r2 | 423 / 525 / 161 | 424 / 524 / 161 | 140 | 14.0 | 12.71 to 12.80 | 195 / 266 |

It takes 6% off the mean, 3% off the peak and 20% off the memory a minute after close:
about two-thirds of the first version's effect on the mean and after close, less than half
on the peak. Cycles are level; tails are within the run-to-run spread (p99 134 to 258 ms
across runs of both). A run made 14 to 19 calls taking the advice off and 16 to 18 putting it
back, 130 to 140 huge-page splits, and returned 59k to 64k pages (230 to 250 MiB). One
put-back call in one run took 53M cycles, about 10 ms; the others' longest was 46k to 55k.
**That one raise is a flag**: a 10 ms call on a request's path would be a tail of its own.

Side by side, each against the `t0` of its own rounds (MiB, medians):

| | first version `t1` (ht5) | candidate `t1r2` (ht10) |
|---|---|---|
| heap mean | 447 -> 409 (-38) | 448 -> 423 (-25) |
| heap peak | 535 -> 496 (-39) | 542 -> 525 (-17) |
| a minute after close | 195 -> 148 (-47) | 201 -> 161 (-40) |
| AnonHugePages peak | 170 -> 118 | 170 -> 140 |
| faults/req | 13.6 -> 14.0 | 13.8 -> 14.0 |
| counters a run | not recorded (no `d1`) | 14-19 advice off, 16-18 back, 130-140 splits, 59k-64k pages returned |

Where the files are, for why `t1r2` returns less than `t1`: the candidate's `HT_DIAG`
counters (`qht/htdiag` is ht10's, three runs appended, a run's last line is its total) and
the unit-map dumps of the ht5 `t0` and `t1` runs (`qht/t0`, `qht/t1`; read with
`bench/probes/heap-drift/account.py` of commit 4522abd, `--label peakrss` or `final`) are in
`docs/research/data/servers-qdrant-2026-10-24.tar.gz`, with every run's records. No unit-map
dump was taken of the ht10 runs.

Whether the extra is a constant or a share: doubling Qdrant's `indexing_threshold`, meant to
double the optimizers' segments, lowered the peak (405 against 509 in one run), since fewer
optimizations finish in the window; the load could not be scaled that way, so this is not
measured. The mechanism, stranded pages of classes that grow in bursts, scales with the
burst rather than being fixed.

Raw results: `docs/research/data/servers-qdrant-2026-10-24.tar.gz` (benchrun's records,
the watcher's and the thread-CPU probe's, the context-switch analyses, the accounts).

## The tail rule as product code (2026-10-26)

Branch `huge-tail-rule` at `3c2265e` (allocator as at `0cd82ec`) against main at its base
`3b5a3f5`, each linked into 0afff09's Qdrant as it is (no switch, so build against build);
beside them ht10's `t1` again, as the first version stand-in (ht5's build is gone).
`qdrant mixed thread`, three rounds interleaved, medians in MiB. Cycles are not read here:
they rank Qdrant's search routing.

| build | heap mean / peak / +1 min (RSS anon) | cgroup anon, same | AnonHugePages peak | faults/req | p99 / p99.9 ms |
|---|---|---|---|---|---|
| main | 449 / 542 / 185 | 447 / 543 / 185 | 168 | 13.7 | 191 / 256 |
| product | 414 / 515 / 155 | 415 / 517 / 155 | 120 | 14.5 | 192 / 226 |
| ht10 `t1` | 405 / 482 / 144 | 405 / 495 / 148 | 126 | 14.0 | 170 / 220 |

Against main, beside the earlier prototypes against their own `t0`:

| | `t1` (ht5) | `t1r2` (ht10) | product | ht10 `t1`, this session |
|---|---|---|---|---|
| heap mean | -38 | -25 | -35 | -44 |
| heap peak | -39 | -17 | -27 | -60 |
| a minute after close | -47 | -40 | -30 | -41 |

The product's peak runs spread from 488 to 539 and `t1`'s from 476 to 495; in two more
rounds with the memstat sampler (below) the two were level on mean (404 and 410) and peak
(499 and 501). What separates them reliably is after close: about 10 MiB.

Tails: p99 146 to 267 ms and p99.9 205 to 345 across all nine runs, with no order between
builds. Qdrant's tail is its search routing (see Method), not the allocator.

**The after-close gap is huge pages that khugepaged rebuilt.** The run cgroup's
`thp_collapse_alloc` (`memory.stat`, the run's own cgroup; `thpstat.py`) per run: main 0 to
1, product 19 to 29, `t1` 10 to 19. With the sampler (`asmalloc-memstat` once a second; free
slots on pages `mincore` calls resident, against the back end's own `retained`, the free
bytes it believes resident; `msgap.py`), free-and-resident minus retained, MiB:

| build | collapses | gap during the load | gap a minute after close |
|---|---|---|---|
| main | 0, 0 | within -1 to 1 outside transients | -3.9, -3.3 |
| product | 19 (one run unrecorded) | rises in steps with the collapses: +7 after 7, +10 after 14, +12.5 after 19 | +8.9, +9.5 |
| ht10 `t1` | 18, 15 | +2 to +3 after the last collapses | -1.2, -0.2 |

So the product holds about 10 MiB resident that it counts as returned (13 more than main's
baseline), and most of it stays for the minute after close. That is the whole after-close difference to `t1` (heap 148 against 137,
and 148 against 143, in the same rounds). It confirms the reviewer's account on a real
server. A raise puts the advice back over split, sparse huge pages. khugepaged rebuilds them.
The rebuilt free pages still read as returned, so neither the count nor the look sees them.
`t1` also collapses, but its walk reads every huge page page by page, so the memory goes back.

The look's other limit, walking only huge pages with 2 of 8 sampled pages free and resident,
leaves nothing measurable here: after close the product's extra over `t1` is no larger than
its collapse gap (heap 11 more against a gap 10.1 more, and 5 against 9.7 MiB).

**Advice calls** (strace of every `madvise` for 30 s at the 70% rate, separate untimed runs,
one each): HUGEPAGE main 134, product 153, `t1` 161; NOHUGEPAGE 57, 87, 74; COLD (the split)
0, 1397, 108; DONTNEED 3881, 5304, 4454. These include the large-block and to-huge advice.
The tail rule's own calls, from the trace below: in the 30 s at 70%, 14 to 16 raises and
12 to 16 lowers a run, over 18 MiB or the rest of the region (about 4 GiB).

**What a waiting advice change does to request threads.** The product was traced with
first-touch's `wl_edit.py`, which applied cleanly at `3c2265e`. The trace times every
address-space call with its thread's CPU time and context switches. Every request was
recorded; there were two runs (`advov.py`). Of 106 background advice changes, the median
took 5 to 7 us. Most never slept. One raise took 146 us (slept 11 times). One lower took
208 us. One pass's lowers over about 4 GiB and 18 MiB took 4.8, 0.6 and 5.2 ms, sleeping 8,
8 and 4 times: they waited for the lock, holding the CPU for 0.05 to 0.8 ms. Between 2 and 8
requests were in flight across those calls, and none of them reached the run's p99.
No request at or above p99 overlapped any advice change in the measured segments of either
run. The longest calls on Qdrant's own threads were the optimizer's large-block advice and
moves (up to 7.4 ms, sleeping), as on main.

**What it says about the rule:** the look should count a collapsed huge page as resident.
For example, it could treat a huge page as resident whole when `mincore` or AnonHugePages
says so, or clear the clean bits of a range the raise covers. Qdrant would then also get
the last ~10 MiB that `t1` returns after close. The 4 to 5 ms lowers over 4 GiB ranges did no
measured harm here, but they waited for the lock with requests in flight. Lowering only the
part that was raised would bound them.

**The fix (`huge-tail-accounting` at `ae24bca`).** This build adds the review's fixes. A raise
also now counts the free pages of non-empty huge pages in its range as resident again, aged
like a fill. It was measured with the same method on the same day: three rounds interleaved
with main and the product, then two sampler rounds. Medians in MiB, cgroup anon a minute
after close:

| build | heap mean / peak (RSS anon) | +1 min (cgroup anon) | AnonHugePages peak | p99 / p99.9 ms |
|---|---|---|---|---|
| main | 446 / 535 | 188 | 164 | 212 / 293 |
| product `3c2265e` | 408 / 504 | 157 | 122 | 196 / 223 |
| `ae24bca` | 410 / 488 | 144 | 116 | 220 / 272 |

`ae24bca` takes 36 off the mean, 47 off the peak and 44 off the memory after close. That is
the first version's effect; the earlier ht10 `t1` reached -44 / -60 / -41. It ran without
error. Tails again show no order (p99 134 to 263 ms across the nine runs).

The sampler rounds confirm the mechanism. `ae24bca` collapsed as often as the product
(27 and 19 huge pages). Its gap after close was -3.6 and -3.3 MiB, which is main's baseline
(-2.9, -3.2). Its heap a minute after close was 134 and 137, against main's 176 and 192.
The product ran once with collapses and once without:

- With 24 collapses, its gap was +10.8 and its heap 153.
- With none, its gap was -3.6 and its heap 136.

That pair isolates the effect. Nothing is left of the after-close distance to `t1`: 134 to
137 against `t1`'s 137 to 143 in the earlier sampler rounds.

The SurrealDB comparison was not run: its asmalloc arm would need two full rebuilds, and the
prototype had no effect on that job.

Files: `docs/research/data/servers-qdrant-2026-10-26.tar.gz` holds the rounds (`qprod`),
the sampler rounds with each process's sampler output (`qms`), the `ae24bca` rounds
(`qacct`: `a1` to `a3` plain, `s1` and `s2` with the sampler), the traced runs (`qwl`), the
strace logs and counts (`madvise`), and the scripts that built and ran them. The probes are
`bench/probes/server-profile/thpstat.py` and `msgap.py`.

## Unresolved

- The measured Qdrant binaries were built before a lint-only edit of the driver (a hex
  literal's grouping and `is_multiple_of`), which changes nothing it does.
- Qdrant writes `.qdrant-initialized` in its working directory (the runner's `bench/`) on
  every start; it is harmless and untracked.

- What makes a run settle on one search runtime or the other, and the third level of work
  within runs on search-cpu.
- `qdrant search` was measured at saturation only.
