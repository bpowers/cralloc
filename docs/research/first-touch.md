# First touches, and writers on the address space

2026-10-25, main `49986cf`, on the reference host (Ryzen 9950X, Linux 7.2.9, transparent huge
pages on `madvise`, `CONFIG_PER_VMA_LOCK` and `CONFIG_PT_RECLAIM` on).

- **Making pages resident in one call is not worth a mechanism.** A
  `MADV_POPULATE_WRITE` saves about a third of a fault on a run of pages and nothing on a
  single page. If every first touch were converted, the saving would be at most about 1% on
  the jobs with the most faults.
- **Under load, the allocator makes no calls that take the address space's lock for
  writing.** That held on the log store, the order service and the HTTP/2 RPC job, and on
  Meilisearch there were a handful in two minutes. No slow request lined up with one. A
  request that grows a region waiting behind a purge happens only while a program builds its
  heap. The largest seen was 2.8 ms, while Meilisearch indexed at startup.

## Part 1: first touches (closed)

**What matters.** One `MADV_POPULATE_WRITE` over a run of pages saves about a third of what a
fault per page costs, and nothing on a run of one page. A fault and a populate share most of
their work: allocating the page, charging it to the cgroup, clearing it, and putting it on
the LRU and in the reverse map. A populate saves only the entry into the kernel and back and
the lookup of the mapping. If every first touch in the heap were turned into a populate, the
most it could save is about 1% of DataFusion's time and of the HashMap job's. That is not
worth a mechanism that is sometimes wrong, so we do not batch first touches. The arms of the
original plan (populate a move's destination, a fill's pages, a large block allocated whole)
were not built.

**The probe** (`bench/probes/first-touch/populate.c`, `kernel.sh`). It maps a region as the
allocator does (reserved `PROT_NONE` and `MAP_NORESERVE`, made writable with `mprotect`,
advised `MADV_NOHUGEPAGE`). It counts the calling thread's core cycles, user and kernel
(`perf_event_open`), over batches of runs, and gives the median of 15 rounds. It ran on CPU 2
with heavy work paused (`scripts/measure --both-l3`) and its own memory cgroup
(`scripts/memlimit`). The CPU ran at 5.6 to 5.7 GHz (the TSC runs at 4.3 GHz). The machine had
1,270 to 1,350 free blocks of order 9 or more, and there were no THP fallbacks or compaction
stalls. The tables are from the first session (`k1`, and the small-batch run beside it); a
second session (`k2`) agrees within 3%.

**Cycles a page**, a fault per page against one populate call over the run, then the
program's write of one byte a page. The pages are fresh (a new mapping, with no page tables).
"Small batch" means 2 MiB of runs between counter reads, so the cleared pages stay in cache.
"Large batch" means 32 MiB, more than the L3 holds:

| run, pages | small batch: fault | populate | saved | large batch: fault | populate | saved |
|---:|---:|---:|---:|---:|---:|---:|
| 1 | 1,729 | 1,799 | -4% | 2,249 | 2,307 | -3% |
| 4 | 1,740 | 1,322 | 24% | 2,245 | 1,837 | 18% |
| 16 | 1,751 | 1,183 | 32% | 2,248 | 1,655 | 26% |
| 64 | 1,756 | 1,137 | 35% | 2,244 | 1,627 | 27% |
| 512 | 1,768 | 1,151 | 35% | 2,265 | 1,630 | 28% |

- **In time**, a fault is 305 to 315 ns with the batch in cache and about 400 ns past the
  L3. The 1.35k "cycles" of the earlier notes are TSC ticks of the in-cache case
  (`meili-peak.md`'s zbench).
- **Writing whole pages** instead of one byte each adds about 80 cycles a page to both, so
  the saving is the same.
- **Pages returned with `MADV_DONTNEED`** (page tables kept by a resident page in each
  2 MiB) cost the same as fresh ones, within 2%. A run of 512 pages frees its page table
  with its pages and costs no more.
- **The saving is about 600 cycles a page, whatever the batch.** That is the exception and
  the mapping lookup. A profile of the probe puts the rest in setting the PTE
  (`map_anon_folio_pte_nopf`, the largest single cost), the memcg charge, the LRU, clearing,
  and freeing pages later. A populate pays all of those.
- **A populate of pages already resident** costs 455 cycles for one page, and 110 to 145 a
  page on long runs. So populating a run that is resident is cheap, but not free.
- **On a busy machine** (heavy work running on the other L3) a fault rose to 3.1k cycles and
  a populate to 2.3k. The ratio stayed the same.

**How long one call holds the thread** (fresh pages, median of 9). The kernel holds the
address space's lock for reading for the whole call.

| size | 64 KiB | 256 KiB | 1 MiB | 2 MiB | 8 MiB | 32 MiB | 64 MiB | 256 MiB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| time, us | 6.7 | 12.3 | 49 | 100 | 413 | 2,439 | 5,521 | 25,586 |
| cycles a page | 1,240 | 1,114 | 1,092 | 1,114 | 1,137 | 1,668 | 1,901 | 2,211 |

Past 8 MiB the cost a page rises, because the clearing no longer fits in the cache, and the
program's own writes after the call would then miss too.

**Other threads** (`populate neigh`: four threads faulting their own pages in the same
mapping, a fifth populating or faulting chunks of its own, and a sixth calling `mprotect` on a
page of the mapping once a millisecond):

| fifth thread | fault p50 / p99, TSC ticks | faults a second, each | `mprotect` p50 / p99 / max, us |
|---|---|---:|---|
| idle | 2,107 / 7,568 | 1.68M | 3.1 / 88 / 173 |
| faults, 64 KiB at a time | 2,365 / 7,912 | 1.35M | 3.5 / 6,051 / 6,423 |
| populates 64 KiB | 2,494 / 7,998 | 1.25M | 9.2 / 8,221 / 11,631 |
| populates 2 MiB | 2,494 / 7,955 | 1.28M | 5,065 / 11,169 / 11,381 |
| populates 16 MiB | 2,365 / 7,826 | 1.30M | 7,937 / 14,982 / 15,733 |

- The faulting threads slow by the same amount whether the fifth thread faults or
  populates. A populate makes pages resident faster, and the threads compete for the page
  allocator either way.
- The `mprotect` is what suffers. It needs the lock for writing. Back-to-back populates hold
  it for reading, and new readers are let past a waiting writer for up to the rwsem's handoff
  timeout, about 4 ms here. So a 2 MiB populate, 100 us long, held the `mprotect` for a
  median of 5 ms. The fifth thread's `MADV_DONTNEED` of its own 256 MiB after each lap, a
  reader too, gave the 6 ms p99 even when that thread only faulted. Part 2 asks whether the
  allocator's own purges do this to its own commits.

**The bound on the jobs.** Removing a fault saves about 600 cycles at best:
- DataFusion takes about a million faults a run: at most 0.6 G cycles of 70 G, 0.9%;
- the HashMap job's 53k first-touch faults: 32 M cycles of 2.8 G, 1.1%;
- tpchgen's 218k (since the outgrown-slot rule): 0.13 G cycles of 24.7 G, 0.5%.

Every arm would reach less than that, because each covers only part of a job's faults.
- The move's copy is exact, but it covers only the moves' destinations. Since the
  outgrown-slot rule those are a minority of the faults on every job that has many.
- Fills cover a magazine's worth of small objects, usually less than a page or a few pages.
  Runs of one to four pages save nothing to a fifth.
- A large block allocated whole risks making resident what the program never touches. The
  probe puts that cost at the full price of a fault, 1.75k to 2.25k cycles, for each page made
  resident and never used.

**Recommendation.** Do not batch first touches. If the design wants a sentence, after "A
huge-page fault delivers 512 pages for about a fifth of the per-page price of small faults"
in "Returning memory and huge pages": *Making small pages resident in one call
(`MADV_POPULATE_WRITE`) instead saves only about a third of a fault, the entry into the kernel,
since allocating, charging and clearing each page is most of the price, and nothing on a single
page; turning every first touch of a heap into one would save about 1% of a query engine's time
(`docs/research/first-touch.md`).*

**Not done.** The fault attribution by cause and the switch's arms were not built. A third of
a fault bounds every arm below the run-to-run spread of the jobs that have the faults.

## Part 2: writers on the address space

**The question.** Part 1's probe showed an `mprotect` waiting a median of 5 ms behind
back-to-back readers of the address space. Two kinds of call are involved:
- `mprotect` is how a region's writable part grows, on the slow path of the request that
  carves.
- `MADV_DONTNEED` is a reader. The background thread and the heartbeats issue it all the
  time.

If the two met under load, a request would stall for milliseconds. That would be a tail that
nobody has attributed.

**What matters.**
- **It does not happen under load.** At 70% the allocator made:
  - no write-locked call at all on the log store at 1M, the order service at 2 GiB, or the
    gRPC log job over HTTP/2;
  - 3 to 6 in two minutes on Meilisearch, each 8 to 9 us.

  None of the slowest 0.1% of requests in any measured segment overlapped a write-locked
  call of ours of 100 us or more. A region's writable part runs ahead of its use by as much as
  it has (up to 64 MiB), so a heap that has stopped growing never grows it again.
- **It does happen while a heap grows fast.** It was seen only while Meilisearch indexed at
  startup, before the load began:
  - region commits on the indexing thread took 2.8 ms and 1.9 ms (two runs), and 0.34 ms
    twice;
  - a `MADV_NOHUGEPAGE` of 32 MiB at a free took 1.1 ms;
  - each of these slept, which means a lock wait, and each overlapped the background
    thread's 2 MiB growth or hold purges of 100 to 200 us.

  The log store's fill (40 to 50 commits a second beside 400 growth purges a second) had no
  writer longer than 16 us. Its purges were short (at most 53 us), and it had no other readers.
- **Meilisearch's long calls are preemption, not the lock.** 69 of its 76 calls of 300 us
  or more were preempted, and 5 slept. For example, a 4 KiB `MADV_DONTNEED` behind a move on a
  request worker took 4.4 ms of wall time and 0.1 ms of CPU. Its indexing threads oversubscribe
  the server's four CPUs. Over the whole run the calls spent 54% of their wall time on the
  CPU.
- **The servers' own writers outnumber ours.** strace was attached for 8 s at 70%.
  - Meilisearch made 94 `mprotect`, 17 `mmap` and 17 `munmap` calls a second of its own (the
    maps are reads of its store; the `mprotect` calls come from 31 threads). Beside them were
    175 `MADV_DONTNEED` and 110 `mincore` a second, ours. So about 130 writers and 290 readers a
    second share its lock under load, and none of the writers is ours.
  - The log store made no such call at all in the window.

**What this means for a rule that adds writers under load.** One example is changing
huge-page advice from the background thread. Each such call waits for every reader in
flight. While it waits, new readers are let past it for up to the rwsem's handoff timeout,
about 4 ms. Then every later reader, and every fault on the mapping it locks, waits behind
it. On a server like Meilisearch that is about 300 lock users a second, so budget for
milliseconds a call, as the 10 ms seen on Qdrant shows. A writer that has to exist should
be rare and off the request path, and it should not be timed against the purges.

**Remedies, not built.** There is nothing to build for the servers measured: under load
there are no write-locked calls to make cheaper. If the startup stalls ever mattered, there
are two levers:
- a commit that runs further ahead, so that a growing heap makes fewer calls (it already
  doubles, so this changes a logarithm's constant);
- a growth payment, or a background purge, that skips a step while a commit is waiting.

A background thread committing ahead of need would only move the wait onto the thread
that next needs the lock. A few milliseconds a commit, a handful of times while a program
builds its heap, is not worth any of these.

**Method.**
- **The trace** (`wl.rs`, `wl_edit.py`), on a scratch copy of main `49986cf`. It times every
  `mprotect`, `madvise`, `munmap`, `mmap` and `mincore` the allocator makes. Each record holds:
  - the start (CLOCK_MONOTONIC) and the duration;
  - the thread;
  - the length and address;
  - what the caller was doing: commit, switch to huge pages, clocked pass, hold, growth
    payment, trim, move, resize, free or claim. This is set by a guard on the caller's stack.
  - the thread's CPU time and its voluntary and involuntary context switches over the call
    (`getrusage(RUSAGE_THREAD)`).
- **Reading the trace.** The records go into an exported ring in the process's memory, which
  a watcher copies out once a second through `/proc/PID/mem` (`wlwatch.py`). So the server
  makes no system call to report. Each read takes the server's lock for reading once a second.
- **Cost.** The trace adds four calls (two clock reads, `gettid`, `getrusage`), about 0.5 us,
  to each allocator call it records. Those are a few hundred a second at most.
- **Runs.** One run per job under `scripts/measure --both-l3`, every request's record kept
  (`BENCH_RAW_DIR`). The jobs:
  - `servers-thread` for the log store at 1M and the order service at 2 GiB: the fill, 30 s at
    70%, a silence and a burst;
  - `meili-tail` for Meilisearch, twice: the fill, then two minutes at 70%;
  - `servers` for `connect log grpc h2`.
  - The order service's run was marked invalid for 0.10 CPUs of other work in the burst's
    first 5 ms; its load segments are used.
  - Free order-9 blocks and the THP counters are in each run's `.machine` file.
- **The census** (`census.sh`): strace attached to the server for 8 s, from 30 s into
  Meilisearch's load and 10 s into the log store's. It stops every thread at every call, so
  its timings are not used.

## Files

- `bench/probes/first-touch/populate.c`, `kernel.sh`: part 1's probe and session.
- `bench/probes/first-touch/wl.rs`, `wl_edit.py`, `build.sh`, `wlwatch.py`, `session.sh`,
  `wlsum.py`: part 2's trace.
- Raw data: `docs/research/data/first-touch-2026-10-25.tar.gz`.
