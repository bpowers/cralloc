# How the allocator changes the program's performance

An allocator costs a program twice: the cycles in its own code, and what its choices do to every
later access to the objects. This note is about the second.

- **Does the allocator change the program's misses? Against jemalloc, hardly.** L1 fills a request
  are within 1% on the order service and the log store. glibc fills a third more on the order
  service, and 2.5 to 3 times as many from beyond L2.
- **Most of jemalloc's and tcmalloc's cycle gap is instructions.** No stall shows in cycles per
  instruction, except tcmalloc's 1 to 2% on the Connect jobs, which is unresolved. That split cannot
  exclude one: a quarter to two fifths of jemalloc's gap is not explained by its extra code at its
  own speed, and no stall counter has been run. Stalls do show for mimalloc and glibc on some jobs.
- **False sharing has one measured case in a program and one in our own metadata.** Keeping tokio's
  run-queue headers apart takes 8 to 14% off a spawn loop, and moves no server measured, at four
  CPUs on one L3. For scale, all of the program's cross-core heap loads could be worth 10 to 18% of
  the order service's cycles and 3 to 4% of the RPC jobs'. How much of that is false sharing is not
  known.
- **Spreading objects over more lines has barely been tested.** Nearly every experiment reorders the
  same slots, and there order decides: a magazine that reversed direction every 254 objects made a
  tree walk 2.5 times slower. Lines per object has been varied on a server once, for one class, and
  fills did not move. It has not been varied across a heap.
- **Where the program's code lands moves a server by 1 to 7%, by job**, as much as most differences
  between good allocators, and the allocator's own size decides where it lands.

**For the design.** Keep objects dense and ascending. Act on a small, bounded part of what is handed
out (the first objects of a run), so the rest keeps its adjacency. Keep allocator metadata off
shared lines. A free writes nothing into the object. None of this has been tested against a heap
that spends more lines per object; ["Not yet measured"](#not-yet-measured) says how to.

Job names are defined in [workloads.md](workloads.md), and terms such as *run* and *frontier* in
[memory.md](memory.md). The hardware costs behind this note are in [hardware.md](hardware.md).

## Does the allocator change the program's misses?

**L1 fills a request, cralloc against jemalloc.** These are exact counters, so they carry "level"
better than the samples below. User-mode demand fills, at 70% load, one to three runs of one build:

| Job | cralloc | jemalloc | Ratio |
|---|---:|---:|---:|
| Order service, HTTP/1.1 | 624 | 627 | 1.004 |
| Order service, HTTP/2 | 916 | 921 | 1.006 |
| Order service, 2 GiB | 618 | 612 | 0.990 |
| Log store, 1M records | 4,291 | 4,327 | 1.008 |
| Connect log gRPC, HTTP/2 | 2,716 | 2,967 | 1.093 |
| Connect log, HTTP/2 | 2,208 | 2,444 | 1.107 |
| Meilisearch | 163,479 | 157,707 | 0.965 |

*2026-10-22, `7619eef`.* The excess on the Connect jobs is jemalloc's own accesses: on gRPC the
allocator's own L1 misses are 186 a request against 446, and the program's heap misses are level.
Demand fills miss what a prefetcher fetched first, so a fill count cannot tell a dense layout from a
sparse one by itself.

**The program's loads on heap memory**, sampled with IBS (AMD's instruction-based sampling), a
request, cralloc / jemalloc:

| Job | L1-missing | From another core | From DRAM | Page walks, all user memory |
|---|---:|---:|---:|---:|
| Order service, HTTP/1.1 | 258 / 257 | 94 / 92 | 19 / 20 | 2.7 / 4.0 |
| Order service, HTTP/2 | 358 / 355 | 71 / 76 | 31 / 23 | 12.1 / 13.5 |
| Connect log gRPC, HTTP/2 | 1,689 / 1,702 | 41 / 49 | 0 / 0 | 7.3 / 6.1 |
| Log store, 1M records | 3,018 / 2,890 | 714 / 596 | 237 / 255 | 33 / 90 |

*2026-10-22, `7619eef`, one op in 500,000, means over one to three runs. The samples are thin: 221
and 217 cross-core samples on the order service and 26 and 31 on gRPC, so one run reads about +-7%
and +-20%. "Level" here means within about 1% of the server's cycles.*

The largest differences are the store's cross-core loads, 723 against 576 in the medians (672 to 745
and 504 to 707 over three runs), with the same total miss latency, and DRAM on the order service
over HTTP/2 (36, 25 and 32 against 17, 24 and 28), whose summed latency is 3,400 cycles higher in
two runs of three; level one-build totals suggest little of it is stall. jemalloc's own metadata
misses more than ours: 246 L1-missing loads a request on the store against 55.

**glibc on the order service is where misses move.** On axum it fills 810 lines a request against
cralloc's 612, 231 of them from beyond L2 against 94; on HTTP/2, 1,244 against 912, and 250 against
88 (2026-10-21, `575760a`, one to three rounds). The order service caches orders that another worker
later evicts, so 20 to 28% of the blocks followed by address are freed on another CPU. We have not
split glibc's fills between its chunk headers, the program's objects and lines that move while
threads wait for its locks.

**The scale of cross-core traffic.** Much of it is in the kernel, not split further: 56 to 89% on
the order service and Connect, 42% on the store. Loads a request labelled "from another core" under
cralloc, with the program's heap loads priced at 90 to 125 cycles:

| Job | All | In the kernel | Program, heap | Program's heap loads, share of cycles |
|---|---:|---:|---:|---:|
| Order service, HTTP/1.1 | 304 | 66% | 94 | 13 to 18% |
| Order service, HTTP/2 | 177 | 56% | 70 | 10 to 14% |
| Connect log gRPC, HTTP/2 | 535 | 89% | 41 | 3 to 4% |
| Log store, 1M records | 1,304 | 42% | 723 | 18 to 25% |

*Computed from the same samples. It counts sharing of every kind, loads only, and ignores overlap,
so it is an order of magnitude, not a bound. The label itself is in doubt: it may include a core's
own dirty lines reloaded from the L3 (see "Not yet measured").* It still says false sharing has room
to matter: a tenth of this on the order service would be over 1% of its cycles.

**Frees on another CPU.** A free is a push on the freeing CPU and never writes into the object. An
allocator with an intrusive free list pays for that write when the object comes back: 7 to 8 ns per
64-byte object within an L3 and 27 ns across. Being level with jemalloc does not test this, because
jemalloc's thread cache also holds pointers and does not write into the object. See the hypothesis
at the end of the next section.

## How a gap splits into instructions and stalls

A ratio of cycles is exactly the ratio of instructions times the ratio of cycles per instruction
(CPI). Against cralloc, server cycles a request at 70% load, user and kernel, geometric mean over
eight server jobs, geometric standard deviation across jobs in brackets:

| Allocator | Cycles | Instructions | CPI |
|---|---:|---:|---:|
| jemalloc | 1.057 (x1.03) | 1.064 (x1.02) | 0.993 (x1.01) |
| tcmalloc, per CPU | 1.056 (x1.03) | 1.075 (x1.04) | 0.982 (x1.03) |
| mimalloc | 1.083 (x1.05) | 1.057 (x1.02) | 1.024 (x1.03) |
| glibc | 1.254 (x1.09) | 1.175 (x1.09) | 1.067 (x1.08) |

*2026-10-28, cralloc at `e5e69b1`. Each job is the mean over five code placements of per-placement
medians (glibc one run a placement). Computed from the standings' raw results. The jobs are the
three Connect log jobs, the log store and four order service jobs, so they are three programs.
miniredis and the sozu proxy barely allocate and are left out; with them in, the cycles column reads
1.046, 1.045, 1.065, 1.197.*

**A CPI ratio under 1 does not show better placement.** Instructions an allocator adds can run
faster than the program's average, which pulls its CPI down, and any added stall can hide behind
them. So the table says no stall penalty is *visible*. It cannot say there is none.

**How much of jemalloc's gap is unplaced.** Three estimates of the part not explained by its own
code at its own speed:

- One-build profile: 34% of the log gap lies outside its paths (gap 1.037, the rest 1.012).
- The same paths against placement-averaged totals of 2026-10-28, six jobs, with dates mixed: 43%
  (gap 1.056, the rest 1.024).
- Its extra instructions at its own measured 0.28 to 0.43 cycles each predict 1.042 over the eight
  jobs, against 1.057: 25%.

**Cycles per op in the program are level under jemalloc: 0.992 (x1.03)**, program user cycles over
IBS program ops, seven jobs of three binaries (2026-10-22). Outside jemalloc's symbols, cycles are
1.2% higher in one build and about 2% over placements, on 0 to 6% more ops in program-labelled code
(geometric mean 1.022). Some of that is probably jemalloc's work, since its share is counted by
symbol range and Rust's shims and the call are inlined into the program; the Router's profile shows
8.4% in jemalloc plus 2.7% in shims. So jemalloc's own symbols are about two thirds of its gap. The
rest is not placed: we have no evidence it is stalls, and none that it is not.

**tcmalloc's extra instructions** cost 0.22 to 0.35 cycles each on the order service and the store,
3 to 4.5 instructions a cycle, and 0.55 to 0.67 on Connect, where the server's own rate is 0.44. Its
CPI on Connect is 1 to 2% over cralloc's, inside the placement error of +-0.012 to 0.015 on one
binary, so that is unresolved. It has no profile, so no more can be said. jemalloc's extra
instructions cost 0.43, in the median of the ten jobs ([servers.md](servers.md)).

CPI against cralloc, per job:

| Job | jemalloc | tcmalloc | mimalloc | glibc | Blocks freed on another CPU |
|---|---:|---:|---:|---:|---:|
| Connect log (HTTP/1.1) | 0.998 | 1.023 | 1.017 | 0.999 | about 0% (measured on HTTP/2) |
| Connect log, HTTP/2 | 0.999 | 1.012 | 1.002 | 0.986 | 0% |
| Log store, 1M records | 1.002 | 0.951 | 1.088 | 1.026 | 15% |
| Order service, axum | 0.989 | 0.963 | 1.026 | 1.156 | 20% |
| Order service, axum, HTTP/2 | 0.991 | 0.965 | 0.998 | 1.203 | 25% |
| Order service, axum, 2 GiB | 0.991 | 0.967 | 1.025 | 1.068 | 28% |
| Order service, actix | 0.978 | 0.968 | 1.036 | 1.135 | not measured |

- **glibc's stalls are on the order service,** at 1.2 to 2.0 cycles per extra instruction. Some may
  be lock waits: at 70% load its CPUs are busier than cralloc's, and at saturation it has 8.5 times
  the context switches on axum and over 1,000 times on actix. That hint is weakest where the excess
  is largest, HTTP/2, at 1.5 times.
- **mimalloc pays stalls on the store** (+11.6% instructions, +21.4% cycles) and, at about 1 cycle
  per extra instruction, on three of the order service jobs. They were not attributed.
- **On the Connect log jobs glibc's gap is all instructions.** On the gRPC job objects die within 35
  us on the CPU that made them, which leaves placement little to get wrong.

**A hypothesis nobody has tested.** The two allocators whose fast paths write a link into a freed
object, mimalloc and glibc, show their CPI excess where blocks cross CPUs, and not where none do.
jemalloc and tcmalloc, which keep pointers in an array, show none where blocks cross. At 40 to 45
cycles a crossing this is the right size for mimalloc, HTTP/2 apart, and about a tenth of glibc's
excess on the small order-service jobs, so it cannot be glibc's main cost there. That price is for
reuse by the core that allocated the object, which is mimalloc's case; glibc's tcache keeps the
object on the freeing thread. Crossing also goes with a live heap that turns over, which three
programs cannot separate.

| Job | Crossing frees a request | The write, cycles | mimalloc's excess | glibc's excess |
|---|---:|---:|---:|---:|
| Order service, axum | 28 | 1.1 to 1.3k | 1.8k | 11.6k |
| Order service, HTTP/2 | 35 | 1.4 to 1.6k | none | 13.8k |
| Order service, 2 GiB | 39 | 1.5 to 1.7k | 1.7k | 5.0k |
| Log store | 392 | 15.7 to 17.6k | 36k | 14k |

*Excess is each allocator's extra cycles beyond its extra instructions at cralloc's CPI. Computed
from the 2026-10-22 counts and the 2026-10-28 standings.*

## Where placement matters

Each row is a behaviour we found, what the hardware did with it, and what we did.

| Allocator behaviour | Hardware effect | Size | What we do |
|---|---|---|---|
| Magazines popped in a sawtooth of addresses | Each reversal restarts the prefetcher's stream | Tree walk 2.5 times slower | Built: pops ascend |
| Siblings packed, then each written by its own thread | False sharing in the program | The stride takes 8 to 14% off a spawn job; no server measured moved | Built: stride the first objects of a run |
| Every fill strided | Allocation-order walks lose adjacency | Tree +47%, JSON +6% (across builds) | Rejected |
| Allocator's shared words on one line | False sharing in metadata | Fills an allocation 6.4 to 5.6 | Built: `lined` |
| Large blocks at slot starts | 64 streams in one 12-way L1 set | 40.9 against 12.0 cycles an element | Built: stagger by slot index |
| Every class's first pages at the same page numbers | L2 TLB set conflicts | Churn at 10k live, -11.6% | Built: a page origin per class |
| Magazines traded across L3s | Lines fetched from the other L3 | Independent pipelines 1.7 to 2.3 times slower | Built: depot stacks per cache domain |
| Fills that ignore the cache domain | Objects reused across L3s | Independent pipelines 2.6 to 13% | Prototype only: not built |
| Small pages on a dense class, below the threshold | TLB misses, faults | BTreeMap's build still 6% behind mimalloc | Built: huge pages past 16 MiB, threshold kept for memory |
| Small pages on a dense large block | TLB misses, faults | DataFusion 2.9%, HashMap 10%, servers nothing | Documented limit |

## False sharing

**What we have found is a hazard of size-segregated allocation, in the program's structures.**
Instances of one type made in a loop by one thread sit side by side. If each is then written by its
own thread, they share lines unless the type is padded. Whether they do depends on which sizes share
a class.

**tokio's run-queue headers are the measured case.** tokio allocates one 40-byte header per worker
at start-up. cralloc's 40-byte class puts them 40 bytes apart, so each worker's `head` and `tail`
share lines with its neighbours'. jemalloc serves them from its 48-byte class, interleaved with
another of tokio's types, which keeps them apart by luck. mimalloc also has a 40-byte class and does
not lose on the unpadded header; its placement was not traced. Switching the rule at run time in one
binary:

| Job | Stride off | Stride on |
|---|---:|---:|
| Spawn, 4 workers, ns a task | 111.3 | 95.8 (-14%) |
| Spawn, 8 workers, ns a task | 202.4 | 186.3 to 187.6 (-8%) |
| Connect log gRPC, k cycles a request | 129.3 | 129.0 to 130.1 |
| RPC log, 64 connections, k cycles a request | 121.9, 122.9 | 121.9 to 122.7 |

*2026-10-22, on `386a1f0`, 4 to 6 interleaved rounds, medians. "On" is the settings of 16 and 64
siblings and the whole first magazine of a run, where run. A setting of 4 separates four workers'
headers but not eight's, so a stride must cover at least as many siblings as workers.*

**On a server these structures are few of the misses.** On the order service under cralloc, tokio's
headers are 40 of 4,950 L1-missing load samples over HTTP/1.1, and its handle and headers are 254 of
about 5,900 over HTTP/2, where hyper spawns a task per stream. The stride moved no server measured:
in one binary on the RPC jobs (about 1% of run spread), and across builds on the order service (59.7
against 59.8 k cycles over HTTP/2). Serving 33 to 40 bytes as 48 on the order service, the one time
lines per object has been varied on a server, moved neither: axum 70.3 to 70.8 k cycles with fills
627 and 627, HTTP/2 64.2 to 62.9 k inside a range of 62.5 to 65.2 k with fills 908 and 893. That
class is 1.4% of allocations. An earlier reading that the headers cost a gRPC job 3.6% came from
comparing two builds, and was mostly code placement ([measuring.md](measuring.md)).

**The one share we have across allocators is the spawn job,** on tokio 1.53.3, before the handle's
line was padded. L1-missing load samples by block at four workers, cralloc / jemalloc / glibc:
scheduler handle 2,387 / 2,031 / 858, run-queue headers 54 / 8 / 15, task cells 238 / 225 / 160, all
2,975 / 2,615 / 1,392. Over all allocators, 75 to 95% of L1-miss latency sits on one line of the
handle.

**Task memory is not the cause.** Task cells draw the same share of misses under cralloc and
jemalloc, and magazines of 14 cells instead of 254 changed neither misses nor time (127.7 against
128.5 ns). This is the only evidence we have on per-request objects rather than start-up structures.

**Load samples can miss the cost.** On the spawn job the headers show few load misses but cost about
280 cycles a task, in stores and locked operations waiting for the line. A false-sharing measure
built only from load misses would undercount.

**A second structure shows how little it takes.** tokio's scheduler handle put the owned-task
counter, written by every spawn and completion, on a line with idle state that every spawn reads.
Any allocator that starts blocks on a line boundary keeps them together. glibc's 16-byte chunk
header shifts the block by an amount the program's earlier allocations decide: the same glibc binary
ran spawn at eight workers at 192 to 227 ns a task depending on the length of its own path. tokio
1.54 pads this one (tokio #8622), and glibc's lottery shrank to 181 to 184 ns.

**The suite has no working test of allocator-induced false sharing.** `cache-scratch` is meant to be
one: each thread frees an object another allocated, allocates its own and writes it repeatedly. As
we build it, `-O3` removes the worker's `new`, `delete` and heap store, and the disassembly shows a
loop touching only the stack, so its results across allocators mean nothing. The active form,
`cache-thrash`, is not in the suite.

**Our own metadata had it too.** In a two-thread pipeline of large objects, six of every ten lines
the producer took from the consumer came from words that only shared a line by accident: a class's
layout memo beside the next class's summary words, and eight classes' stack heads on one line.
`crate::lined` puts each group of shared words on lines of its own and asserts the statics' sizes in
lines. Fills an allocation fell from 6.4 to 5.6, a 12% change; nothing gives its time alone. It
takes about 22 KiB more statics, zero and untouched until used. (2026-10-18.)

**The rule, and what it costs.** Objects of a class allocated one after another are the allocator's
best evidence of siblings. A fill that opens a run hands out its first 64 strides of objects a
stride apart, in classes smaller than a line, and the rest in the fill's order. No slot is skipped;
only the order changes, and only in the first magazine of a run. A stride is the fewest slots at
which two objects are sure to share no line: `64 / size` where the size divides 64, else `ceil(64 /
size) + 1`. A fill opens a run when the first frontier word it records shows no claim this interval,
so the rule adds no state, write or clock read. Details are in `docs/design.md`.

**Rejected:**

- *Dropping the classes that do not divide 64 (40 and 56).* Costs up to 20% of memory on objects of
  those sizes, and two 48-byte objects can still share a line.
- *A line for each object below 64 bytes.* Judged far too much memory; never measured on a server
  (see "Not yet measured").
- *Moving blocks off line boundaries, as glibc's header does.* A lottery that helps one layout and
  hurts another.
- *Relying on tokio to pad the header.* Padding helped cralloc only: across builds it cost glibc 3
  to 8% and mimalloc 2 to 3% on spawn.

## Order, and lines and pages

**Order decides what a walk costs.** A magazine used to hand out its objects highest address first,
while successive magazines ascended. Preorder walk of a 2M-node tree of 24-byte nodes, TSC ticks a
node, caches flushed: ascending 5.0, descending 5.0, sawtooth reversing every 254 nodes 14.1. The
prefetchers follow a stream in either direction. What defeats them is the reversal. This was fixed
without a server measurement.

| Box tree, depth 20 | Sawtooth | Ascending | mimalloc |
|---|---:|---:|---:|
| Walk, M cycles | 531 | 211 | 229 |
| Build, M cycles | 529 | 448 | 355 |
| Drop, M cycles | 1,079 | 627 | 350 |
| DRAM fills, M | 9.9 | 3.6 | 4.0 |

*2026-10-18, `76e4533`, the tree built ten times a run, 5 interleaved rounds, medians.*

**Handing out every fill apart is what breaks a walk.** A strided order on every fill cost a tree's
build and walk 47% and, across builds, a JSON parse 6% and BTreeMap and HashMap about 1%. Units of
eight strides cut the tree's loss to 12% and left JSON's. Only the first fill of a run costs nothing
measured on either. The whole first magazine of a run, strided at run time, did not move the RPC
servers, whose small classes circulate. Striding every fill has never run on a server in one binary.

**Set conflicts need many streams or a shared offset.**

- *Large blocks in one L1 set.* Large slots are spaced by a power of two, so 64 buffers written in
  turn put every store in one set of the 12-way L1. A probe ran 7.9 ns an element at slot starts and
  0.74 to 0.80 with blocks moved by a different multiple of 64 bytes. On the suite's 64 vectors
  grown to 4 MiB: 40.9 cycles an element at slot starts, 12.0 staggered (jemalloc 14.5, which
  offsets for the same reason; mimalloc 40.0), same peak. Three streams fit the ways and show
  nothing. The stagger's price: 4 KiB of room in a slot, and a block of `k` huge pages then spans `k
  + 1`. (2026-10-17.)
- *First pages in one L2 TLB set.* Every class region starts on a 4 GiB boundary, so the first pages
  of all classes shared low page numbers. A small heap walked page tables though it used a few
  hundred KiB. Each small class's first slot now lies at its own page origin (`i * 159` pages modulo
  256). Origins spread over 256 pages removed all the walks, over 16 pages half to five sixths, and
  offsets of lines within a page none. It costs no memory. Two pipelines across L3s ran 5% slower
  with it at the same instructions and 50 times fewer walks; the cause was not found.

| Job | Time | 4 KiB walks an operation |
|---|---:|---:|
| Churn, 10k live | -11.6% | 0.133 to 0.0001 |
| espresso, preloaded | -3.3% | 443k to 6.9k a run |
| RPC echo, 64 connections | -2.3% (one cross-build number) | 24.1 to 5.5 |
| BTreeMap, forkjoin, mstress | within 1% | level |

*2026-10-19, `2c27ffe` against `5a3aa18`, 3 or 4 rounds, medians. sh8bench preloaded ran 2.2 to 3%
slower, not analysed.*

- *L1 and L2 sets on a small heap: real, and too small to matter.* Five in six of cralloc's L1
  misses hitting L2 fell in the first eight of 64 L1 sets on the spawn job. Spreading them removed
  those L2 hits and changed neither time nor line transfers. (2026-10-19.)

## TLB and huge pages

**On servers, walks are not where time goes.** At an assumed 30 to 60 cycles a walk (Zen 5 has no
event for page-walk cycles), they are under 0.1% of cycles on the order service and RPC jobs, 0.3 to
0.5% on the store and 0.5 to 0.9% on Meilisearch. The servers' walks are on small classes or on
blocks under 2 MiB, which no per-block huge page can cover. On the store cralloc takes about 60%
fewer than jemalloc (33 against 90 a request).

**On batch programs, huge pages are most of a build's gap.** Single-threaded build phases that ran
mimalloc's instructions in more cycles were explained by page faults and L1 dTLB misses on objects
on 4 KiB pages. BTreeMap's build is 4,036 M cycles against mimalloc's 3,799: 34.6M L1 dTLB misses
against none, and L1 fills level. With huge pages disabled for every process, cralloc's build phases
are level or faster on all six jobs; with huge pages from a class's first carve, BTreeMap's build is
3,687 M cycles, ahead of mimalloc. So classes far apart cost nothing beyond their TLB entries, and
fills' order and spacing are not the cause. Two of the four single-thread gaps, tree depth 18 and
JSON 8 MiB, are mostly the drop, as is what is left on BTreeMap ([magazines.md](magazines.md)). At
an exact 2 MiB switch BTreeMap still takes 15.5M L1 dTLB misses on 4 KiB pages. (2026-10-18,
`fca1d12`.)

**Classes of at least 4 slots a page, to 1 KiB, switch to huge pages after 16 MiB carved.** 4 MiB
would take BTreeMap from 942 to 901 cycles an entry and tree depth 18 from 39.5 to 35.4, for up to 6
MiB more at a peak. We chose the memory: 16 MiB kept cralloc under jemalloc on churn and small maps.
`MADV_COLLAPSE` at the switch buys most of the 4 MiB threshold's gain on BTreeMap and little
elsewhere, for 0.1 to 0.4 ms a huge page on a slow path, so it is not used.

**Large blocks allocated whole get no huge pages: a documented limit.** A run-time switch in one
binary that advised every whole block of 2 MiB or more:

| Job | Cycles | What moved |
|---|---:|---|
| HashMap, 1M entries | -10.5% | walks 2.63M to 0.22M, faults 53k to 21k |
| DataFusion | -2.9% | half the walks and faults, same peak |
| tpchgen | -2.0% (9 rounds, ranges overlap) | walks halved |
| Meilisearch, a request | -1.1% (inside its spread) | 3% of its walks land in such blocks |
| Order service at 2 GiB, log store, Connect log gRPC | switch never acts | no block of 2 MiB or more |

*2026-10-24, `875c19a`, 1 to 10 rounds by job, medians.*

The cost of being wrong is large and has no ceiling but the block. A block touched in one page in 64
holds 11 to 16 times its memory. Written whole and reused, each life costs 25 times what it does
today, because an advised block's slot is returned at its free. On a host short of free 2 MiB
blocks, every advised fault can stall in direct compaction: lives slowed by up to 17 times in one
session. Random reads over a resident 256 MiB gain only 1 to 3%, since the walk overlaps a DRAM miss
each read pays anyway. See [large-blocks.md](large-blocks.md).

tcmalloc gets no huge pages on this host: zero `AnonHugePages` in every run, preloaded or linked.

## Cache domains

**Keeping magazines in their L3 is worth a lot to independent work.** With one depot, magazines one
L3 freed were popped by the other, and every slot and most objects were then a fill from the other
L3. A stack per domain took independent pipelines over two L3s from 11.16 to 4.78 ns an allocation
(two pipelines), 9.53 to 5.54 (three) and 10.23 to 5.60 (four), with fills from the other L3 down
40% on sh8bench. Jobs on one L3 moved -1 to +3%. Peaks rose 0.3 to 0.8 MiB on jobs using both L3s.
(2026-10-18, `5e2350f` against `130887d`, 5 rounds.)

**Keeping fills in their L3 is not built.** The back end still takes a class's lowest free slots,
whoever freed them. A prototype that carved a region per class and domain, and refilled from a
domain's own free slots before popping another domain's magazine, gave independent pipelines 2.6 to
13% and flows between L3s 2 to 4%, and left everything else level. It added 0.3 to 1.6 MiB at peak,
a small-heap floor that grows with domains, and up to the retained cap held when load moves between
domains. That is a narrow gain for a change across the back end, the launder and the depot, so we
left it. Build it only if work spread over many L3s becomes a target. (2026-10-19, `5097226`; tag
`repro/exp-domain-regions`.)

## The allocator moves the program's code

**A server's time moves with where its code lands, and the allocator's size decides that.** Builds
identical except for a padding function of N bytes ahead of everything moved all later code by
exactly N. On the Connect log gRPC job: N = 0, 16, 32, 48 ran 126.9, 127.1, 130.5 and 132.2 k cycles
a request, instructions level. Over 90% of the difference in sampled op-cache misses and
decoder-delivered ops falls in the service's protobuf decoder. That is where the samples fall, not a
forced result, since the padding moves all later code at once. A whole-line shift is as bad as the
worst sub-line one by another route, so no single alignment decides it. The sensitive code is not
ours and there is nothing to pin. (2026-10-22.)

Over five placements on 2026-10-28, the spread is 5.0 to 6.4% on the Connect log jobs and 1.7 to
3.1% on the order service, median of five allocators, and it differs by allocator: cralloc's own is
3.1 to 3.7% and 1.2 to 2.7%.

**Batch jobs move too.** JSON spreads 3.5 to 6.7% across layouts, in serde_json's
`Value::deserialize`, half of whose time is one 16-byte reload after a recursive call, and the tree
3 to 9%. The tree's walk is 5 to 7% slower in the default layout than in the others; a loop over an
explicit stack instead put a store-to-load chain on every node and made the walk twice as slow, so
the recursion stays. (2026-10-18.)

**Physical memory moves a job too, and nothing controls it.** The 4 MiB grow job's runs fall in two
groups 9% apart in every layout, with ASLR off and the stack fixed, so it comes from the process's
physical pages. (2026-10-18.)

This changes how to read every number here: see [measuring.md](measuring.md).

## Not yet measured

We want this measured, and nothing above answers it: does an allocator spread a program's objects
across more cache lines, and how does the share of false sharing change between allocators?

**What we have.** L1 fills for cralloc and jemalloc on seven jobs and glibc on two, none for
tcmalloc or mimalloc, with no number for lines touched. The size of two known structures on the
order service, under cralloc. One share across allocators, on a spawn loop. Every server pinned to 4
CPUs on one L3. False sharing should grow with workers and cost more across L3s, so "no server shows
it" is a statement about 4 CPUs.

**How we would measure it, in this order:**

1. **Layout statistics from an allocation trace.** They need no counters and are exact: distinct
   lines covered per 100 allocations on a CPU, against bytes / 64; the share of consecutive
   allocations on the same or next line; address reversals per 1,000 allocations; and lines holding
   blocks allocated, or freed, by different CPUs. This answers "more cache lines" for lines
   allocated (step 6 answers for lines touched), and measures the opportunity for false sharing,
   including the case per-CPU magazines are exposed to by design: a slot freed on another CPU and
   reused there beside live neighbours. Level with jemalloc would not show that, since jemalloc does
   the same. The trace perturbs timing, so use it for placement and never for time. The trace and
   shape wrappers exist as a patch at tag `research-logbooks`
   (`bench/probes/spawn-gap/diagnostics.patch`), for cralloc, jemalloc and glibc; mimalloc and
   tcmalloc need adding, and it has no request marks.
2. **Stall counters, over the usual placement sweep.** `ex_no_retire.load_not_complete` (`0xa2d6`),
   `ex_no_retire.not_complete` (`0x2d6`) and `de_no_dispatch_per_slot.backend_stalls` (event
   `0x1a0`, umask `0x1e`, raw form `0x100001ea0`), then `no_ops_from_frontend` (event `0x1a0`, umask
   1) in a second session, since placement shows in the front end, with off-CPU time for lock waits.
   These show whether stalls rose for each allocator, not whose: they include an allocator's own
   misses, and they count user mode where the server's cycles are user and kernel. Zen 5 gives five
   counters with the NMI watchdog on and the runner uses two, so three raw events a session.
3. **A density switch in one binary.** Serve every class under a line at a line per object, and
   separately stride every fill. The first arm needs no new build: `SHAPE_BUMP=1-63:64` in the shape
   wrapper, one binary, its arms compared only with each other. Run on all servers, this gives the
   net of false sharing removed and spread added as one causal number with placement cancelled. It
   has never run across a heap.
4. **Fills by source and prefetch.** Demand fills `0xff43`, `0x143`, `0x243`; any fills `0xff44`,
   since demand fills miss what a prefetcher fetched into L1, which is the effect at issue;
   `ls_hw_pf_dc_fills` `0xdf5a`; `0x1064` (a store or state change that hits in L2) and `0x864` (a
   request that misses in L2). `0x443` (the other CCX) reads about zero on a server pinned to one
   L3. `0x243` is "L3 or a different L2 in this CCX", and no counter separates those. Raw events
   here are user mode, so the kernel's cross-core traffic is out.
5. **IBS, to name candidates, each confirmed by a switch in one binary.** It gives no source or
   latency for stores, and stores and locked operations are where the spawn job's cost was, so
   stores can add addresses but cannot be weighed. A definition by line, "two CPUs touch it, one
   writes, bytes disjoint", errs towards false both ways: sparse samples rarely see one byte twice,
   and over a long window different tenants of a line read as sharers. Classifying a miss needs the
   last writer's bytes before that transfer, and byte ranges need the op's width. At one op in
   500,000 a run gives 26 to 541 cross-core heap samples, so this proves long-lived hot structures,
   not a share. The `ldlat` filter would spend the budget on load misses. The "from another core"
   label may include a core's own dirty lines reloaded from the L3, by our reading of the kernel's
   IBS decode, unverified: a single-threaded job under IBS is the control.
6. **For a true share, an offline replay.** One trace of the program's calls and accesses in
   block-and-offset terms. Replay the traced calls through each allocator, threads pinned, to get
   its addresses, then count lines and set conflicts and classify each transfer, as PREDATOR does.
   The allocator's rseq path need not be intact, since its own accesses are not the question.
   Instrumentation changes interleaving, so a share from it is an estimate.
7. **Controls and more CPUs.** The method must find tokio's headers on spawn with the stride off,
   lose them with it on, and report nothing on a job with private heaps. A server across both L3s
   needs the load generator moved, and changes the price of a transfer from 90 to 125 cycles to
   about 440.

**What would change the design:** a switch in one binary, with enough rounds, showing that false
sharing or spread is worth over about 1% of a server's cycles. Across allocators three rounds of
five placements resolve 2.5% and not under 1%, so this needs the switch, not a comparison.

**Also open:**

- `SIBLINGS` = 64 rests on nothing measured. 16 is enough for 4 and 8 workers, the whole first
  magazine costs nothing on the RPC servers, and fewer than the worker count misses headers.
- Classes of one to two lines (80, 96, 112 bytes) also put neighbours on a shared boundary line. No
  measured case.
- mimalloc's and glibc's stalls where blocks cross CPUs, the hypothesis above.
- `cache-scratch` needs building so its store survives, to give the suite a test of this.
- A 12% churn slowdown (25.4 against 22.6 cycles an operation) with 22.8M blocked loads appeared in
  one build that differed only in pass order. Its op-cache misses point at code layout, and no
  address was shown to alias a slab word. The run-time switch never showed it.
- A region-level huge-page mark for dense large classes under 2 MiB, such as the order service's
  1.25 MiB table shards, and whether it moves anything at 3.8 walks a request.
- Everything here is one CPU model. See [hardware.md](hardware.md).
