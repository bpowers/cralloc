# What the hardware and kernel charge

Against a fast path of 5 cycles, everything the machine charges around it is expensive. This bill
decides where an allocator's effort belongs: not faulting, not moving lines, no atomic per object.
It is one machine, so the rules should carry to another and the numbers will not ("What should carry
to another CPU").

| Operation | Cost | Whose price |
|---|---:|---|
| Uncontended atomic | 20 cycles | Zen 5 |
| `call` and `ret` | 4 cycles | Zen 5 |
| Cache line to another core, same L3 / other L3 | 16 to 22 / 76 to 79 ns | Topology |
| Next user of an object another core wrote, per 64 bytes, same L3 / other L3 | 7 to 8 / 27 ns | Topology |
| First touch of a 4 KiB page, clears in cache / past the L3 | 1,750 / 2,250 to 2,850 cycles, 310 / 400 to 505 ns | Kernel, then cache and DRAM |
| The same memory inside a 2 MiB huge page | 117 to 214 ns per 4 KiB | DRAM bandwidth |
| `MADV_DONTNEED` of one page, alone / with other threads running | 0.36 / 1.80 us | Kernel, core count |
| Clock after 1.5 s of idle | 4.38 GHz for 200 to 260 ms, then 5.7 | Platform |
| Exit from the C2 idle state | 18 us | Platform |

A page fault is the most expensive thing an allocator causes: 350 to 570 inlined pairs. Most of it
is bookkeeping, so reuse comes before fresh pages, a populate call saves only about 600 cycles a
page, and a huge page costs a fifth to a third as much per byte when a free 2 MiB block exists. And
quote cycles: the clock moves a job's time by 30% at the same cycles.

**The machine.** AMD Ryzen 9 9950X: 16 cores with SMT, 48 KiB L1d and 1 MiB L2 a core, two L3
domains of 32 MiB (CPUs 0 to 7 with siblings 16 to 23, and 8 to 15 with 24 to 31), 60 GiB. Linux
7.2.9, `amd-pstate-epp`, governor `powersave`, boost on. Transparent huge pages and defrag on
`madvise`, multi-size huge pages off. The time-stamp counter runs at 4.3 GHz, so 1,350 ticks are
about 1,770 core cycles.

## What belongs to Zen 5's core

Widths, as independent work per loop iteration: loads 4 a cycle, stores 2, taken branches 2, mixed
ops about 8, register arithmetic alone about 5. A load is 4 cycles through `[base]`, `[base + disp]`
or `[base + index]`, and 5 with a scaled index or with an index and a displacement. Zeroing is 316
to 336 GiB/s in memory that fits L1, 243 in L2, 141 to 146 in L3 and 30 to 33 past it. Memory
renaming, the page-offset hold, the op cache's windows and `call [mem]` are also this core's:
`roofline.md` has them, with the rules they gave.

## Moving lines between cores is the topology's price

| Per hand-off, or per 64-byte object | SMT sibling | Same L3 | Other L3 |
|---|---:|---:|---:|
| Strict ping-pong of one line | 16 to 21 ns | 16 to 22 ns | 76 to 79 ns |
| Contended `fetch_add` on one line | 12 to 15 ns | 10 to 13 ns | 14 ns |
| Extra cost to a producer when a consumer on the other core read and wrote the object first | 0.8 to 0.9 ns | 7 to 8 ns | 27 ns |

Uncontended `fetch_add` is 3.5 ns. The last row is what an allocator pays when a free on another
core writes into an object and the core that allocated it reuses it. Crossing L3s costs about four
times as much, which is what ranks allocators once objects cross them (`roofline.md`). (2026-10-16,
wall clock, single runs in two sessions.)

## First touch of a 4 KiB page: 1,750 to 2,850 cycles, by how far the page falls out of cache

The handler's work is fixed; what moves is whether the page it clears and the kernel data it touches
are still in cache. The two figures in circulation, 305 to 400 ns and 535 to 566 ns, are one curve
at different places:

| Memory passing through, machine | Cycles a page | ns a page |
|---|---:|---:|
| 2 MiB at a time, quiet | 1,730 to 1,830 | 305 to 320 |
| 32 MiB at a time, quiet | 2,245 to 2,290 | 398 to 405 |
| 64 MiB at a time, quiet (computed) | about 2,540 | about 450 |
| 256 MiB at a time, quiet (computed) | about 2,850 | about 505 |
| 64 MiB at a time, the other L3 busy | 2,310 to 2,530 in the kernel alone | 523 to 582 |
| 512 MiB in one pass, the clock not recorded | not counted | 535 to 566 |

**Trust the first four.** They come from one probe that counted core cycles and time together with
heavy work paused, 15 rounds, in two sessions within 3%, at a measured 5.62 to 5.69 GHz (2026-10-25
at `49986cf`). The computed rows are one `MADV_POPULATE_WRITE` over that much memory (1,901 and
2,211 cycles a page) plus the 640 cycles a fault costs over a populate at 2 and at 32 MiB. The last
two rows are earlier, less controlled runs. The 64 MiB one (2026-10-17) shared the machine with
other work, and its excess is what that should cost: 16 to 29% over the quiet estimate, with the
populate beside it 17 to 30% over the quiet populate, at about 5.2 GHz by its cycles. The first
probe on a busy machine read 3.1k cycles. The 512 MiB one (2026-10-16) is a single wall-clock pass
with no cycle count, a little past where the curve is heading. So 566 ns is not wrong: it is the top
of the curve, and it overstates a fault in a program that grows a little at a time.

**Most of it is bookkeeping.** With the clear in cache, clearing is 7 to 9% of the profile, and the
zeroing rates predict 90 to 150 cycles of 1,750. Past the caches, clearing alone is 650 to 720
cycles of 2,850, about a quarter, and by inference misses on kernel data make up the rest of the
rise. The largest items are setting the page-table entry under its lock, the memory-cgroup charge
and the LRU. Pages returned with `MADV_DONTNEED` cost the same as fresh ones within 2%. Each new
64-byte object carries 27 to 45 cycles of first touch, five to nine inlined pairs.

**A populate saves about 600 cycles a page, the exception and the mapping lookup,** whatever the
total: a third of an in-cache fault, 27% past the L3, a fifth to a quarter on the busy machine. It
saves nothing on a run of one page and 18 to 22% on four. So we do not batch first touches. Turning
every fault into a populate would save at most 0.9% of DataFusion's cycles (about a million faults),
1.1% of a HashMap job's and 0.5% of tpchgen's. Populate-ahead for dense classes, built and removed,
cut faults three to five times and kernel time by under a tenth, for about 1 MiB at each peak. One
call holds the thread 100 us at 2 MiB and 25.6 ms at 256 MiB.

Four threads faulting at once on their own mappings cost 761 ns against 539 alone, since page-table
locks are per table and page lists per CPU.

## Huge pages are bound by zeroing, and by free 2 MiB blocks

A huge-page fault costs 60 to 61 us per 2 MiB in one pass (117 to 119 ns per 4 KiB) and 151 to 214
ns per 4 KiB on the shared machine. Clearing is 85% of the profile, and 151 ns is about 27 GB/s,
what one core writes to DRAM. At four threads it is 581 ns, only 1.3 times cheaper than a small
fault, because together they zero about 28 GB/s, roughly what one core complex can write.

- **No free block, no bargain.** A fault in an advised range with no free 2 MiB block compacts in
  the faulting thread and falls back to 4 KiB if that fails: 660 to 780 ns per 4 KiB once page cache
  had filled memory. mimalloc's `Vec` growth went from 5.8 to 28 cycles an element in that state.
- **Purging part of a huge page returns nothing.** `MADV_DONTNEED` of the first half of each of 256
  MiB of huge pages took resident memory to 130 MiB, and the cgroup stayed charged 257 MiB two
  seconds later. The kernel only queues the page for a later split, and a thread that then meets the
  cgroup's limit stalls 40 to 50 ms. `MADV_COLD` splits it first, for 20 to 80 us (`memory.md`).
- **khugepaged collapses sparse memory.** 64 MiB with one page written per 2 MiB, then advised for
  huge pages, became 64 MiB resident within 40 seconds. Advice must never cover memory used
  sparsely. A call of advice itself is about 1 us. (2026-10-16 and 17, medians of 3 to 9 interleaved
  rounds on 64 MiB, ranges over two sessions.)

## What other calls charge

| Call | Cost |
|---|---|
| `MADV_DONTNEED`, one long call | 81 to 96 ns a page |
| `MADV_DONTNEED`, 1 / 64 pages a call, alone | 0.36 / 3.27 us |
| The same with three other threads running, whose TLBs it shoots down | 1.80 / 5.56 us |
| `MADV_FREE` | 79 to 80 ns a page; touching again without reclaim 85 to 88 |
| `MADV_DONTNEED` of whole huge pages | 2.0 to 6.4 us per 2 MiB |
| `MADV_COLLAPSE` of 4 KiB pages into a huge page | 223 to 398 us per 2 MiB |
| `mmap` and `munmap` of 2 MiB untouched; `mprotect` to none and back | 0.79 us; 0.83 to 0.92 us |
| `MADV_GUARD_INSTALL` and `REMOVE`, one page | 0.20 us |

So a purge is about 2 us for the shootdown plus 0.05 us a page, which is why the clocked pass
returns runs of adjacent pages with one call. (Calls of 1 to 64 pages 2026-10-20, 600 times per
length; the rest 2026-10-16, single runs in two sessions.)

## Writers on the address space wait behind readers, and under load we make none

A populate holds the address-space lock for reading for the whole call, and a purge holds its
mapping's lock the same way; `mprotect` and huge-page advice need them for writing. New readers pass
a waiting writer for up to the lock's handoff timeout, about 4 ms. So a 2 MiB populate, 100 us long,
held an `mprotect` for a median of 5 ms when populates ran back to back.

- **Under load it does not happen.** At 70% load the allocator made no write-locked call on the log
  store, the order service or the gRPC job, and 3 to 6 in two minutes on Meilisearch, each 8 to 9
  us. None of the slowest 0.1% of requests overlapped one of 100 us or more. A region's writable
  part runs up to 64 MiB ahead of its use, so a heap that has stopped growing never grows it again.
- **It happens while a heap grows fast:** region commits of 2.8 and 1.9 ms while Meilisearch indexed
  at startup, each overlapping purges of 100 to 200 us.
- **The servers' own writers outnumber ours:** about 130 write-locked calls a second of
  Meilisearch's own under load.

**Rule.** A writer on the address space has to be rare and off the request path. (2026-10-25 at
`49986cf`, every call timed through a ring read from outside the process, one run per job.)

## Moving a block with `mremap` is fast, and we copy

A move costs 32 us at 256 MiB and 180 us at 4 GiB with both ends on 2 MiB boundaries, or 390 us and
6.1 ms page by page, against a copy's 43 ms and 681 ms. But a copy that skips all-zero source pages
gets the memory benefit by itself (peak within 12% of glibc's against 2 to 7 times), and remapping
adds only speed, 1.6 times the reallocs at 256 MiB and 2.9 at 1 GiB. What decided it (2026-10-17,
probes last present at `666f143`):

- moved pages keep their file offset, so the range cannot merge with its neighbours until a fresh
  `mmap` over it, and each live moved-in block costs up to two mappings;
- a refused move can leave the destination unmapped, and a block in two mappings can come back
  `ENOMEM` half moved;
- a move clears `VM_LOCKED` on the whole source mapping, and in a forked child heals never merge;
- a thread faulting in either region waits for the whole call: about 0.1 ms per GiB aligned, 1.5 ms
  per GiB page by page.

## The clock a run starts with belongs to the package

After 1.5 s of idle every core runs at 4.38 GHz, the nominal clock, for the first 200 to 260 ms of a
spin, then reaches the boost within 40 ms: 5.71 GHz with one core busy, 5.67 with four, 5.56 with
eight. The state is the package's: a 500 ms spin on another CPU, even in the other L3, warms it all.
It decays between 200 and 400 ms of idle. Other work keeps the boost and lowers it a little (5.42
GHz with seven other CPUs busy).

So a job shorter than that runs entirely on the cold clock, and short multithreaded jobs spread by
up to 30% in time at the same cycles. Mined from the first-touch runs: an 8 MiB populate took 413 us
warm and 524 us cold, at 2.33 and 2.31 Mcycles. The runner warms each job's CPUs first, 15.4 ms a
run (`measuring.md`; 2026-10-19 at `35b00fa`).

## Idle states are the platform's, and they land in the tail

Through `acpi_idle` with the menu governor, the host declares exit latencies of 1 us for C1, 18 us
for C2 and 350 us for C3, with target residencies of 2, 36 and 700 us. A request that finds its CPU
in C2 gets its first server thread 22 to 28 us after it came due, against 4 to 8 us ordinarily: the
extra 18 to 20 us is the declared exit. Two exits in turn take 42 to 48 us. That CPU had been idle
20 to 60 us, long enough for the governor to go deep, so the charge falls where idle periods meet
the 36 us residency, and it is not monotone in load.

With C2 and C3 off, one server's p99.9 fell from 45.1 to 37.8 us, the others moved within 0.4 us,
and cycles a request and saturated rates moved under 1%. CPUs that never idle at all run 2 to 3%
fewer cycles a request. `servers.md` has what this does to comparing allocators, and
`scripts/deep-idle` shows and sets the states. (2026-10-24 and 25, 10 to 12 runs a cell over five
placements at 63k requests a second.)

## What should carry to another CPU

No second CPU model has been run, so this is judgement from the mechanisms, not a result.

| Rule | Expectation elsewhere | What to re-measure |
|---|---|---|
| No atomic and no shared line per operation; no writes into freed memory; arrays over lists | Carries: coherence and pointer chasing cost on every core | The sizes |
| Inline over call; a late store address holds up the next load | Should carry; memory-dependence prediction is common | The sizes; whether the hold appears |
| Reuse before fresh pages; populate does not pay; partial purges of huge pages return nothing | Carries: these are the kernel's, not the core's | Fault cycles, which follow the kernel's configuration as much as the CPU |
| Nothing at a hot word's page offset | Should carry: page-offset aliasing is a known x86 hazard, and the layout costs nothing if it is absent | Whether the 10 to 25-cycle mode exists |
| Hot state 64 bits through a base register | Harmless, may buy nothing: without this renaming every reload is an ordinary forward | The forwarding table |
| At most 13 and 24 micro-ops, 64 or 76 to 128 bytes, entry on a line | Will not carry as numbers: op caches differ. The limits are constants in Lean's printer and the size check | The window and line sweeps |
| Domain-aware magazines | Depends on topology: nothing to gain on one L3, more on many | Hand-off costs by distance |

Every cycle, nanosecond and clock number here will change, and the clock and idle numbers can change
on the same CPU with firmware or driver. `bench/roofline` runs the core and kernel probes on a new
machine, and the runner learns its warm clock.

## Open questions

- **Everything beyond one machine:** no second CPU model, kernel configuration or multi-socket host,
  and no quiet fault past 256 MiB.
- **Why glibc's large-object pipeline reads 4.4 to 4.5 GHz however warm the package,** when a thread
  faulting in a loop reads 5.64. Cycles over wall time undercount a thread that blocks in the
  kernel, which would explain it; that was not checked.
- **Multi-size huge pages,** which would fault 4 to 256 pages at a time, are off here and
  unmeasured.
