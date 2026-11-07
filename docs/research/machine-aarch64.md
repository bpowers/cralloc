# What an aarch64 machine charges an allocator

This is the bill for everything around the core on an Apple M1 Max with a 16 KiB kernel, and for the
same core under a 4 KiB kernel in a virtual machine. `firestorm.md` has the core's own part, and
`hardware.md` has this bill for Zen 5. Three findings change what the allocator does.

- **Coherence has two units.** Within a cluster false sharing stops at 64 bytes, and across clusters
  at 128. So the bound on a line is 128, and no register or sysfs file says so.
- **Blocks at a power-of-two spacing share TLB sets as well as cache sets.** Moving them by a cache
  line fixes the caches. It still leaves 64 buffers written in turn 6 to 16 times slower than moving
  them by a page as well, so a block now moves by both.
- **A huge page's worth depends on the page it replaces.** Per byte it saves 3.3 times on faults over
  16 KiB pages, and by calculation about 9 times over 4 KiB and 1.5 over 64 KiB. Its stall and its
  unit of return grow sixteenfold at each step. So the huge-page rules hold for 2 and 32 MiB and are
  off at 512 MiB.

| Operation | Cost | Whose price |
|---|---:|---|
| Cache line to another core, same cluster / other cluster / P to E | 42.5 / 179 to 181 / 205 to 213 ns | Topology |
| Next reader of an object another core wrote, per load, same cluster / other cluster | 11 to 13 / 31 to 61 ns | Topology |
| Page walk, tables in cache | 28 to 34 cycles, 10 ns | Core |
| First touch of a 16 KiB page | 1,890 to 2,100 cycles, 627 to 692 ns | Kernel |
| The same memory inside a 32 MiB huge page | 189 to 196 ns per 16 KiB | Clearing rate |
| `MADV_DONTNEED` of one page, alone / with other threads running | 0.8 to 0.9 / 1.4 to 1.6 us | Kernel, core count |
| Splitting a 32 MiB huge page | 1.0 to 1.8 ms | Kernel |
| Clock after idle | 3.03 GHz from the first 2 ms: no ramp | Platform |
| Exit from `CPU PD` | about 7 us over a wake from `WFI` | Platform |

**The machine.** Apple M1 Max: two Icestorm efficiency cores (CPUs 0 and 1, 2,064 MHz) and eight
Firestorm performance cores in two clusters (CPUs 2 to 5 and 6 to 9, 3,036 MHz), no SMT, 62 GiB.
Asahi Linux 7.2.12 with 16 KiB pages and 32 MiB transparent huge pages on `madvise`, defrag on
`madvise`, multi-size huge pages off, `INIT_ON_ALLOC` on, governor `performance`, idle states `WFI`
and `CPU PD`. The generic timer runs at 24 MHz. The 4 KiB rows are CPUs 2 to 5 under `muvm`: Linux
6.12, no huge pages, no PMU, and page walks through a second stage of translation, so they read in
nanoseconds and their walks cost about twice bare metal's.

**The method.** Two sessions on 2026-11-07, the second at `3bfd7c8`, through `scripts/measure`. A
range is across the two sessions, each the fastest of three repetitions or the median of the rounds
stated. The first session left builds running on other clusters, except on runs that crossed clusters
or used an E core; the second paused them throughout. `bench/roofline/machine/run.sh` runs all of it
on any aarch64 Linux, and `bench/roofline/README.md`, "machine", has the commands.

**Computed marks what no probe measured.** All of the 64 KiB kernel is computed, and so are the 4 KiB
kernel's huge-page costs, since no 64 KiB kernel has run and the 4 KiB one has no huge pages.

## Where the two sessions disagree

Most rows moved under 2%, contended atomics across clusters by 10 to 20%, and no decision here
changed. The larger moves depend on what else the machine was doing:

- **The system cache.** A chase of 48 MiB on a P core read 100 ns with builds paused and 127 ns with
  them running, and a chase of 32 MiB read 74 against 123 ns. The clusters share that cache.
- **The first work after a long idle.** At 100 ms between requests a chase read 7 to 9 ns a load with
  builds paused and 23 to 55 ns with them running. That points to other work emptying the L2, not the
  idle state, which is the opposite of what the first session alone suggested.
- **Reading through an array of pointers.** With builds running a chase at 2 GiB gained 1 to 9%, and
  with them paused it ran 4.5 times as fast. At 512 MiB it ran about four times as fast either way. The
  first session had read this as nothing to speak of.
- **Returning a whole huge page:** 7.6 against 11.5 us, the widest move among the kernel's calls.

## Cache lines: 64 bytes within a cluster, 128 across

Two threads run `ldadd` on words at chosen offsets, nanoseconds an operation (alone 2.29):

| Words apart | Same cluster | Other cluster | P and E |
|---:|---:|---:|---:|
| 8 to 56 | 10.0 | 19.7 to 22.4 | 18.6 to 21.8 |
| 64 to 120 | 2.2 to 2.3 | 19.7 to 22.5 | 17.8 to 25.5 |
| 128 and up | 2.2 to 2.3 | 2.2 to 2.3 | 2.3 to 2.9 |

One word apart across a boundary says the same: 56 and 64 share only across clusters, 120 and 128
never do, and 248 and 256 never do, so there is no wider unit. Moving every second line of a set by a
small amount finds what each cache indexes by, and that agrees: 64-byte lines in the L1, 128-byte
lines in the L2.

| Cache | P core | E core |
|---|---|---|
| L1 data | 128 KiB, 8 ways, 256 sets, 64-byte lines | 64 KiB, 8 ways, 128 sets, 64-byte lines |
| L2, shared in a cluster | 12 MiB, 12 ways, 8,192 sets, 128-byte lines | 4 MiB, 16 ways, 2,048 sets, 128-byte lines |

`ctr_el0` and `dczid_el0` both say 64, and sysfs leaves the line blank, so only this test finds the
128. **The bound on `lined::LINE` is 128 on aarch64.** On a core with 64-byte lines it doubles the
shared statics, to about 44 KiB, zero until used, and it spaces a run's first siblings 128 bytes apart
instead of 64. Neither should cost anything measurable; neither has run on such a core.

A miss brings a neighbour, upward only. After a miss to memory the other half of the 128-byte line
costs 59 to 62 cycles more and the next line up 227 to 230, where any line further away costs 361 to
383. A line below costs 320 to 376.

## Moving a line costs more than twice what it does on Zen 5

| Per hand-off or per load | Same cluster | Other cluster | P and E | E and E |
|---|---:|---:|---:|---:|
| Strict ping-pong of one line | 42.5 ns, 129 cycles | 179 to 181 ns, 544 to 551 | 205 to 213 ns, 623 to 647 | 54 ns, 111 |
| Contended `ldadd`, each op overall | 5.3 ns | 9.2 to 10.2 ns | 10.9 to 12.9 ns | not run |
| Contended `cas`, each op overall | 3.7 to 3.8 ns | 6.4 to 6.7 ns | 9.1 to 9.8 ns | not run |
| Next reader, a chase through objects the other core wrote | 11 to 13 ns | 31 to 61 ns | 35 to 66 ns | not run |
| Next writer, a burst of stores, per line, shuffled | 0 | 7 to 17 ns | 7 to 18 ns | not run |
| Next writer, the same, ascending | 0 | 0 | 0 | not run |

Uncontended, `ldadd` is 2.2 ns and `cas` 1.8. Crossing clusters costs 4.3 times a hand-off inside one,
about what crossing L3s costs on Zen 5, so the depot's stacks per cache domain stand, with the three
clusters as the domains. E and E is one session, one run.

**A burst of stores hides the transfer.** Zen 5 charged the producer 7 to 8 ns per 64-byte object
within an L3 and 27 across. Here the same probe reads nothing within a cluster, and nothing across one
when the writes ascend. By inference, stores here may commit out of order, so one that waits for its
line does not hold up the rest and a burst overlaps its misses. A load in a chase cannot, so the
reader's row is the true price of a line. Which a program pays depends on how it uses an object.

## The hierarchy

| Random chase, huge pages | P core | E core |
|---|---:|---:|
| L1 | 3 cycles, 0.99 ns | 3 cycles, 1.46 ns |
| L2 | 18 cycles, 5.94 ns | 14 cycles, 6.8 ns |
| Evicted from L2, a set of few lines | 194 to 201 cycles, 65 ns | 144 to 148 cycles, 71 ns |
| DRAM | 380 to 410 cycles, 125 to 135 ns | 280 to 289 cycles, 135 to 140 ns |

On small pages DRAM reads 401 to 456 cycles, the rest being walks past the L2 TLB. The third row is
what the system cache gives a line. Its reach for one core depends on the other clusters (above): with
builds paused a P core's chase is still under DRAM's cost at 64 MiB, and with them running it reaches
DRAM's at 48 MiB.

| GiB/s past the L2, one pass | P core | P cluster, four cores | E core |
|---|---:|---:|---:|
| Read | 46 | 47 | 21 |
| Zero (`dc zva`) | 86 | 91 to 94 | 75 |
| Fill | 46 | 44 to 45 | 19 |
| Copy | 24 | 22 to 23 | 14 |

One core takes nearly all a cluster can have, which is why four threads faulting huge pages each get
a quarter of the rate (below).

### Blocks written in turn: the stagger needs pages as well as lines

The L1's 8 ways make 64 streams a problem, as Zen 5's 12 did. On huge pages, where no TLB limit
applies, 64 buffers written in turn take 11.2 to 11.4 ns an element at one offset, in slots of 2 and 4
MiB, and 0.65 to 0.75 with any stagger of 64 bytes over 16 or more offsets. **On small pages that stagger does little,** because
slots at a multiple of many pages also land in a few sets of each TLB (below), and 64 pages do not fit
12 ways.

So a block also moves by whole pages: `stagger(i) = 64 * (i % 64) + page * (i % P)`, with `P =
min(32, slot_pages / 8)` and no page term when P is under 2. Nanoseconds an element, no stagger /
lines alone / the rule:

| Slot, in pages | 16 KiB kernel, 64 buffers | 16 KiB kernel, 16 buffers | 4 KiB kernel, 64 buffers | 4 KiB kernel, 16 buffers |
|---|---:|---:|---:|---:|
| 32 | 4.7 / 1.5 / 0.77 | 4.7 / 0.59 / 0.37 | 8.2 / 0.58 / 0.58 | 8.3 / 0.37 / 0.37 |
| 64 | 5.4 / 3.2 / 0.53 | 4.7 / 0.60 / 0.37 | 9.2 / 1.2 / 0.70 | 8.2 / 0.37 / 0.37 |
| 128 | 5.4 / 5.4 / 0.46 | 4.7 / 0.60 / 0.37 | 9.3 / 9.0 / 0.78 | 8.2 / 0.61 / 0.37 |
| 256 | 5.4 / 6.0 / 0.46 | 4.0 / 0.71 / 0.37 | 10.3 / 15.5 / 0.80 | 8.4 / 0.87 / 0.44 |
| 1,024 | 6.2 / 7.5 / 0.46 | 4.0 / 0.70 to 0.75 / 0.37 | 15.6 / 21.5 / 0.81 | 8.4 / 0.84 / 0.38 |
| Between those: 80, 96, 112, 160, 224 | 4.8 / 0.95 to 1.6 / 0.46 to 0.48 | 4.7 / 0.37 to 0.64 / 0.37 | 8.3 / 0.63 to 0.90 / 0.60 to 0.76 | 8.3 / 0.37 to 0.40 / 0.37 |

The 4 KiB columns are one session, and nested walks inflate their slow cells. A joint cycle of the two
terms fails at small P (2.4 ns at 4 offsets), because it leaves only 4 cache offsets, so the terms are
independent. Stepping by 128 bytes is no better than 64, since the L1 indexes by 64.

The room is `P * page + 4 KiB`, at most an eighth of the slot plus 4 KiB: 132 KiB on a 4 KiB kernel,
516 KiB on 16 KiB and, **computed**, 2 MiB on 64 KiB. It costs address space and no memory, since
nothing touches the pages below the block. **Computed** over request sizes from 33 KiB to 2 GiB on all
three kernels, 26% land one class higher than with the lines' 4 KiB and none land two higher. A request
of 1 MiB stays in the 1.25 MiB class. Being wrong about a TLB costs only that address space.

### Order: pops ascend

| Walk, 512 MiB, huge pages, cycles an object | Ascending | Descending | Sawtooth, worst reversal |
|---|---:|---:|---:|
| 24 bytes | 3.1 | 3.5 | 10.1 to 10.3, at 254 objects |
| 64 bytes | 4.3 | 5.5 to 5.6 | 16.9 to 17.4, at 126 |
| 128 bytes | 33 to 34 | 34 to 35 | 94 to 101, at 14 |
| 256 bytes | 57 to 58 | 58 | 171 to 172, at 14 |

A random order costs 384 to 392. A magazine that reverses costs three to four times what one that
ascends does, as on Zen 5.

**What the prefetchers follow.** A chase at a stride over 2 GiB on huge pages, cycles a load, in either
direction, against about 400 at random:

| Stride | 64 | 128 | 192 | 256 | 512 | 1,024 | 2,048 | 4,096 | 8,192 |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Cycles | 4 to 6 | 34 to 36 | 52 | 58 to 63 | 127 to 137 | 30 to 34 | 31 to 35 | 32 to 46 | 36 to 52 |

Strides of 640, 768, 1,536, 3,072 and 16,384 get nothing, 384 to 400 cycles, and 320 and 384 get 266
to 273 ascending and nothing descending. So where a program touches one line per object it is
prefetched only if the class is a power of two. On 16 KiB pages a stride up to 1,024 crosses pages as
well as it does inside a huge page. From 2,048 up each page starts the stream again: 69 to 83 cycles
at 2,048 ascending, 118 to 128 at 4,096, and 108 to 127 at 8,192, against 132 to 144, 181 to 198 and
238 to 259 descending. A stream of 8 lines after a jump averages 108 to 128 cycles ascending and 163
to 193 descending, 16 lines 82 to 83 and 96 to 98.

**A magazine of raw pointers gets its objects prefetched, on a P core.** An array read in order whose
values are addresses has the targets of its later entries fetched ahead, as a data-memory-dependent
prefetcher does. Scattered objects over 512 MiB, popped from arrays of each size, cycles an object,
pointers / the same objects by 32-bit offsets:

| Entries a magazine | Each pop waits for the last object | 256 instructions between pops | 1,024 instructions between pops | No work between pops |
|---:|---:|---:|---:|---:|
| 14 | 193 / 403 | 50 / 56 | 156 / 179 | 15 / 15 |
| 62 | 279 / 403 | 53 / 56 | 166 / 178 | 15 / 15 |
| 254 | 104 / 403 | 49 / 55 | 145 / 178 | 15 / 15 |
| 1,022 | 66 / 403 | 42 / 56 | 138 / 179 | 15 / 15 |

Where the core can overlap the misses, which a program's independent pops let it do, the gain is 6 to
25% of the cycles an object at this much work, and none in a tight loop. Icestorm has none of it: 291 / 296 and
460 / 465. It weakens under load: at 2 GiB with builds running, 395 against 415. Nothing in the
allocator has to change to get this, but **magazines should keep raw addresses:** offsets would lose it
on a heap that has been running a while, where magazines are scattered. (One session, at the commit
that adds `streams magazine`.)

## The TLBs: sets by page number, and a large reach

| | P core, 16 KiB | E core, 16 KiB | P core, 4 KiB under `muvm` |
|---|---|---|---|
| L1 | 160 entries, 32 sets, 5 ways, reach 2.5 MiB | 128 entries, 32 sets, 4 ways, reach 2 MiB | Sets by every fourth page, fits 128 pages |
| L2 | 3,072 entries, 256 sets, 12 ways, reach 48 MiB | 1,024 entries, 256 sets, 4 ways, reach 16 MiB | 3,072 entries, 256 sets, 12 ways, reach 12 MiB |
| L1 miss, L2 hit | 6 cycles, 2 ns | 7 cycles, 3.4 ns | about 2 ns |
| Walk, tables in cache | 28 to 34 cycles, 10 ns | 31 cycles, 15 ns | 21 ns, through two stages |

**Origins stay 256 pages, stepping by 159, in pages.** The L2's 256 sets match Zen 5's, on the E core
and on the 4 KiB kernel too. For classes in regions at the same page number, cycles a load by span of
the origins: 16 classes of one page read 39 to 40 with none and 3.0 from a span of 16 up; 64 classes
of 16 pages read 62 to 63 with none, 40 at 64 and 17 from 128 up.

## First touch of a 16 KiB page: about 1,900 cycles, whatever is passing through

| Memory passing through | ns a page | Cycles a page |
|---:|---:|---:|
| 2 MiB at a time (one session) | 665 | 1,970 |
| 8 MiB | 627 to 628 | 1,892 to 1,894 |
| 64 MiB | 629 to 639 | 1,908 to 1,939 |
| 256 MiB | 629 to 636 | 1,907 to 1,929 |
| 1 GiB | 660 to 692 | 2,002 to 2,101 |

It is nearly flat, where Zen 5 rose from 1,750 to 2,850. Zeroing here is fast enough not to care: the
190 ns a huge page's clearing takes per 16 KiB is a third of the fault, and the rest is the kernel's
bookkeeping. Under the 4 KiB kernel a fault is 412 to 415 ns per 4 KiB, about 2.6 times as much per
byte. In a written-once run of 64-byte objects, what a fault adds to memory already resident:

| Kernel, page | ns an object |
|---|---:|
| 4 KiB, under `muvm` | 7.3 |
| 16 KiB | 2.9 to 3.1 |
| 32 MiB huge pages | 0.72 |

**A populate saves little.** At 64 pages or more it saves 126 to 136 ns of 627 to 692, about a fifth,
and 13% at 4 pages, and at one page it costs 70 to 75 ns more than the fault. Under the 4 KiB kernel it
saves 148 of 412. So we still do not batch first touches. Four threads faulting at once in a cluster
take 911 to 913 ns against 632 to 641 alone.

## Huge pages: worth having at 2 and 32 MiB, off at 512 MiB

**`MADV_HUGEPAGE` gets one** under `defrag=madvise` on this machine, in every run: 1,103 free 32 MiB
blocks at the start, and 6 fallbacks in 2,317 faults since boot. Nothing here measured a shortage of
free blocks, where the fault stalls in direct compaction.

**What 32 MiB is worth against 16 KiB.** Objects of 64 bytes, median of three rounds:

| Heap | Fill, ns an object, small / huge | Random churn, ns an operation, small / huge | Gain on churn |
|---:|---:|---:|---:|
| 8 MiB | 4.2 to 4.3 / 4.4 | 5.2 to 6.3 / 5.7 to 6.4 | 0.92 to 0.99 |
| 16 MiB | 4.8 to 4.9 / 4.8 | 12.4 to 13.0 / 12.6 to 13.0 | 0.99 to 1.00 |
| 32 MiB | 5.1 to 5.5 / 2.7 | 17.8 to 17.9 / 16.2 to 16.4 | 1.09 to 1.11 |
| 64 MiB | 5.1 to 5.3 / 2.7 | 27.2 to 27.6 / 26.8 | 1.02 to 1.03 |
| 256 MiB | 5.2 to 5.6 / 2.7 | 38.5 to 38.7 / 36.3 to 36.4 | 1.06 to 1.07 |
| 1 GiB | 5.3 to 5.5 / 2.4 to 2.5 | 48.0 to 48.2 / 44.6 to 44.7 | 1.08 |
| 4 GiB | 5.0 to 5.2 / 2.5 | 53.6 / 47.1 | 1.14 |

So on an established heap a huge page buys little up to about 64 MiB, apart from 9 to 11% at 32 MiB,
which this probe does not explain, and growth gains twice at any size past one huge page. A written-once build costs 18.7 to 19.4 cycles an object on small pages, 12.2
on huge and 10.0 on resident memory. The rule starts at 256 MiB in a class, or at a block that grows
past 32 MiB, which is where the gain is. This churn is a synthetic heap; the switch in one binary that
`measuring.md` asks for needs the port.

**How the worth falls with the page, while the stall and the unit grow:**

| Kernel | Huge page | L2 TLB reach | Small over huge fault, per byte | One huge fault, alone | Four at once, each | A split |
|---|---:|---:|---:|---:|---:|---:|
| 4 KiB | 2 MiB | 12 MiB | **computed** 8.7 | **computed** 24 us | **computed** 86 us | **computed** 60 us |
| 16 KiB | 32 MiB | 48 MiB | 3.3 | 365 to 389 us | 1.38 ms | 1.0 to 1.8 ms |
| 64 KiB | 512 MiB | **computed** 192 MiB | **computed** 1.5 | **computed** 6.2 ms | **computed** 22 ms | **computed** 16 ms |

The computed cells scale the 16 KiB row by size. The 8.7 sets the 4 KiB kernel's fault, in a virtual
machine, against this machine's bare clearing rate. The 1.5 assumes a fault's bookkeeping of about 370
ns, as on the 4 KiB kernel, plus clearing 64 KiB. The reach assumes the same 3,072 entries.

**The cutoff: huge-page rules hold only for a huge page of at most the retained cap, 64 MiB.** A
whole-page purge cannot be held to a cap smaller than the page. And by the table, at 512 MiB a fault
saves 1.5 times for a stall of 6 to 22 ms on a slow path, a class must carve 4 GiB before its first
huge page, and the churn table, scaled to the TLB's reach, puts the gain at about 8% at that size. Only
512 MiB is affected. `worth churn` and `worth cold` on a 64 KiB kernel should check this. With the rules
off a region still advises `MADV_NOHUGEPAGE`, or a kernel set to `always` would map 512 MiB pages anyway.

**Partial purges return nothing, as on Zen 5.** `MADV_DONTNEED` of the first half of each of 128 huge
pages took resident memory to 2,050 MiB and left the cgroup charged 4,100 MiB, two seconds later too,
at 13.7 to 13.9 us a call. A whole huge page returned is 7.6 to 11.5 us, against 166 us for its pages
at 81 ns each.

**`MADV_COLD` still splits, and it costs a millisecond.** Over a whole huge page it does nothing, 1.0
to 1.3 us. Over part of one it takes 1.0 to 1.1 ms where every page holds data, and 1.4 to 1.6 ms after
a partial purge, then about 100 us for each half returned. **A split also frees pages that read as
zero:** where only zeros had been written, 1,025 MiB resident fell to 1 MiB, at 1.7 to 1.8 ms a split.
So a split of a huge page over 2 MiB belongs off the request path, on the background thread or in
`collect`.

**khugepaged collapses sparse memory.** With `max_ptes_none` at 2047, 32 huge pages' worth of memory
with one page written in each, then advised, became 1,026 MiB resident in 50 seconds, in both sessions.
Advice must never cover memory used sparsely.

**The contiguous bit gives nothing by default.** This core honours it: over a huge page's memory mapped
by page-table entries, a chase reads 3.3 cycles at 8 MiB, like the huge page and against 9.3 on small
pages, and 33 at 128 MiB against 19 and 51. But an anonymous mapping gets such runs only from large
folios, and multi-size huge pages are off unless root turns them on.

## What other calls charge

| Call | P core, 16 KiB | E core, 16 KiB | P core, 4 KiB under `muvm` |
|---|---:|---:|---:|
| Entering the kernel (`getppid`) | 126 ns | 176 ns | 117 ns |
| `MADV_DONTNEED`, nothing resident | 181 ns | 345 ns | 177 ns |
| `MADV_DONTNEED`, 2 / 64 / 512 pages, alone | 0.50 to 0.58 / 5.4 to 5.5 / 41.5 us | not run | 0.71 / 3.7 / 26.2 us |
| The same, three threads in the cluster | 1.17 / 6.0 / 42.2 us | not run | 1.62 / 4.6 / 27.2 us |
| The same, nine threads on every other CPU | 1.2 to 1.4 / 6.0 to 6.1 / 42.3 us | not run | not run |
| `MADV_DONTNEED`, a page in a long call | 81 ns | not run | 51 ns |
| `MADV_FREE`, then rewrite | 285 to 288 ns a page | 484 to 485 ns a page | 405 ns a page |
| `MADV_COLD`, small pages | 26 ns a page, 241 ns a call | 45 ns a page, 504 ns a call | 53 ns a page, 255 ns a call |
| `mprotect` to none and back, 128 pages, untouched / resident | 1.2 to 1.3 / 3.6 to 3.7 us | 3.2 to 3.4 / 9.9 to 10 us | 1.8 / 3.6 us |
| `mmap` and `munmap`, 128 pages | 0.98 us | 2.4 us | 1.3 us |
| `mincore`, 1 / 256 pages | 0.52 / 1.28 us | 0.84 / 2.35 us | 0.20 / 0.46 us |
| `MADV_HUGEPAGE` and back | 1.2 us | 3.1 to 3.3 us | not run |
| `futex` wake, no waiter / a sleeper | 146 / 292 ns | 221 ns / not run | 128 ns / not run |
| `membarrier` rseq, no thread of ours there | 124 ns | not run | not run |
| The same, by CPU, a thread there: same cluster / other / E | 0.57 / 1.46 / 1.73 us | not run | not run |
| The same, for all: three CPUs / five | 2.12 / 2.59 us | not run | not run |

**A shootdown is cheap here, and small purges cost the others nothing.** Broadcast invalidation adds
0.5 to 0.9 us to a call however many other CPUs run the process, where Zen 5 paid 1.44 us. The other
threads lose no measurable laps at up to 16 pages, and up to about 3 laps at 64, a lap being about 0.6
us of one thread's time. **A kernel call's cost is per page,** 81 ns per 16 KiB against 81 to 96 per 4
KiB on Zen 5, so a bound on how long a purge runs stays in pages and a bound on memory is bytes.

## The clock and idle

**No ramp.** With the `performance` governor a P core reads 3.03 GHz from its first 2 ms after a pause
of any length up to 5 s, and an E core 2.06. With all ten CPUs busy a P core reads 2.73 to 3.04 GHz
over 500 ms, where cycles over wall time cannot tell a lower clock from other work taking the CPU. So a
job needs no warming here. A plain `cntvct_el0` read costs about a cycle, with an `isb` 10.6 ns, and
`clock_gettime` 22.5 ns. At 24 MHz a tick is 42 ns, so a timed call under a microsecond needs a loop.

**Leaving `CPU PD` costs about 7 us.** A futex wake reaches a thread in 1.8 to 2.1 us from `WFI` and 8.8
to 10.7 us from `CPU PD`. The waker's call takes 292 ns, and up to 3 us after 100 ms. `CPU PD` declares
a residency of 10 ms, but the governor goes deep at far shorter gaps: 0.2 to 0.5 entries a request at
200 to 500 us, and the 90th percentile is about 9 us from 200 us up. So a tail at a fixed rate has the shape
it has on Zen 5, and is smaller.

**The L2 survives idle.** With builds paused a chase of 64 KiB to 4 MiB runs at 5.5 to 6 ns a load up
to 20 ms between requests, 5.7 to 7 at 50 ms and 6.6 to 8.7 at 100 ms. What the first session lost at
100 ms was probably other work (above); this does not separate it further.

## Big and little

An E core's time over a P core's, on what an allocator does:

| Work | E over P |
|---|---:|
| Loads in L1 | 1.47, the clock |
| Loads in L2 | 1.15 |
| Loads in DRAM | 1.05 |
| First touch | 1.85 |
| Huge-page fault | 1.19 |
| Entering the kernel | 1.4 |
| `MADV_DONTNEED`, nothing resident | 1.9 |
| `mprotect`, `mmap` | 2.5 to 2.7 |

So memory is level and kernel work about twice. No constant follows from it. A CPU's slab serves
whichever kind it is, and the E cluster is one more domain.

## What the constants should be

Rules about memory are bytes. A bound on kernel work is pages, since the cost is per page. A rule about
the kernel's huge page uses its size. TLB rules are pages.

| Constant | Value | Unit, and why |
|---|---|---|
| `lined::LINE` | 128 | The wider of the two coherence units |
| `SIBLINGS` | 64 | Unchanged; below Graviton 4's worker count, so one to re-measure |
| `STAGGER_STEP`, `STAGGER_SLOTS` | 64 bytes, 64 | Unchanged: the L1 indexes by 64 |
| The stagger's page term | `min(32, slot_pages / 8)` | Pages: TLB sets |
| `ORIGIN_SPAN_PAGES`, step | 256, 159 | Pages: TLB sets |
| Least region | 2 MiB | Bytes: a reservation of about 8 GiB on every kernel; a region under a huge page has no huge pages |
| `RETAINED_CAP`, `KEPT_OUTGROWN` | 64 MiB | Bytes |
| `ALLOWANCE_FLOOR` | 32 MiB | Bytes |
| `GROWTH_STEP`, `GROWTH_UNPAID` | 2 MiB, 16 MiB | Bytes |
| `UNWRITTEN_FLOOR` | 16 MiB | Bytes |
| `COMMIT_AHEAD_PAGES` | 64 KiB, at least a page | Bytes |
| `COMMIT_AHEAD_MAX` | 64 MiB, at least a huge page in the huge part | Bytes |
| `PURGE_MIN_RUN` | 16 KiB, at least a page | Bytes: one page costs 50 to 57 ns per KiB, four 8 to 10 |
| `TAIL_HEADROOM` | 16 MiB, at least a huge page | Bytes: calls scale with bytes grown |
| `MOVE_PIECE` | 2 MiB; whole huge pages for a block under advice | Bytes: a partial purge returns nothing |
| `HUGE_MIN_SLOTS_PER_PAGE` | Classes to 1 KiB | Bytes: as a share of a page it would take classes to 4 KiB on 16 KiB |
| `PURGE_MAX_RUN`, `PURGE_STEP`, `PURGE_MAINTAIN_STEP` | 64, 128, 128 pages | Pages: 128 take 11 us |
| `HOLD_BEAT` | 512 pages | Pages: 42 us here, 41 to 49 on Zen 5 |
| `HUGE_AFTER` | 8 huge pages | The huge page: at most an eighth of the class is waste |
| `GROWN_HUGE_MIN`, `COMMIT_AHEAD_HUGE` | 1 huge page | The huge page |
| `TAIL_SPLIT_SHARE`, `TAIL_SAMPLE`, `TAIL_SPARSE` | 1 in 4, 8, 2 | A share and counts |
| `PURGE_*_WORDS`, `FILL_CLAIMS` | Unchanged | Bitmap reads |
| `UNWRITTEN_RESIDENT_ONE_IN` | 16 | Safe on every kernel; a fault is 3.3 clears here against 13, so it could rise |
| A huge page's rules | On up to 64 MiB | The retained cap |

**Ticks (computed).** The timer scales by a shift so that an interval is about a second. Rounding the
shift to the nearest power of two keeps an interval within 0.71 to 1.41 s: at 24 MHz it is 1.40 s and
the hold 175 ms, and at 1 GHz 1.07 s and 134 ms. Rounding down would make it 1.95 s at 1.1 GHz and the
hold 244 ms, where DataFusion's peak in `large-blocks.md` rose.

## What should carry to Neoverse V2

Graviton 4 and NVIDIA Grace have not run any of this, and `run.sh` runs all of it there. What follows is
judgement. Where a column cites a source, we read it there and did not measure it.

| Here | There | To re-measure |
|---|---|---|
| Two coherence units | One of 64 bytes. Arm's learning path lists Graviton 4 at L1d 64 KiB 4-way, L2 2 MiB 8-way, L3 36 MiB 12-way, all 64-byte lines. A bound of 128 costs padding | `lines granule` |
| TLB sets by page number | Chips and Cheese gives a 48-entry L1 dTLB, and Arm's Hot Chips slides make it fully associative, by a search's summary of them. It calls the L2 TLB's size unspecified, 2,048 on V1. We could not read Arm's manual, so the L2's sets are unknown. If they are not by low page bits the page term costs only address space | `hier tlbsets`, `streams rule` |
| Three domains | One L3 for the machine, so one domain and the depot's bound of 128 KiB a class | The mesh's distances, with `moves` |
| An array of pointers prefetched | Chips and Cheese says V2's L1 prefetcher crosses pages, and that a sampling indirect prefetcher dereferences pointers | `streams magazine`, `hier prefetch` |
| First touch flat with memory passing through | Clearing is probably slower, so a huge page may be worth more | `faults single`, `hier bandwidth` |
| Broadcast invalidation costs others little | Unknown on 96 or more CPUs | `kcost purge` with all CPUs |
| A 24 MHz timer | Near 1 GHz | `lines regs` |
| 512 MiB off, by calculation | The only way to know | `worth churn`, `worth cold` on a 64 KiB kernel |

Sources: [Chips and Cheese, Hot Chips 2023](https://chipsandcheese.com/p/hot-chips-2023-arms-neoverse-v2),
[Arm's slides](https://hc2023.hotchips.org/assets/program/conference/day1/CPU1/HC2023.Arm.MagnusBruce.v04.FINAL.pdf),
[Arm's learning path](https://learn.arm.com/learning-paths/servers-and-cloud-computing/memory-subsystem/cache-hierarchy/).

## Open questions

- **Everything beyond this machine,** above. Nothing here has run on a 64 KiB kernel, or on 4 KiB
  outside a virtual machine.
- **A huge page's worth on real jobs,** and the page stagger's. Written in turn is the stagger's best
  case, and fewer buffers than a TLB set's ways gain nothing.
- **Huge pages under fragmentation:** no run has met a shortage of free 32 MiB blocks.
- **Fragmentation on 16 KiB pages:** a live object pins four times the memory it does on a 4 KiB page, so
  `memory.md`'s numbers need the port.
- **What weakens the pointer prefetcher under load** at 2 GiB and not at 512 MiB. Its worth in a real
  program, where a pop has other work around it, needs the port too.
