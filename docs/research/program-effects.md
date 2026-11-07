# How the allocator changes the program's performance

An allocator costs a program twice: the cycles in its own code, and what its choices do to every
later access to the objects. This note is about the second.

- **On servers, the gap between good allocators is mostly instructions, not stalls.** Against
  cralloc, jemalloc's and tcmalloc's cycles rise with their instructions while cycles per
  instruction stay level or fall. The program's own code runs about the same under cralloc and
  jemalloc, with level misses on heap memory.
- **Stalls appear when an allocator gets it badly wrong.** glibc's cycles per instruction are 14 to
  20% higher on the order service: it fills a third more lines a request, and 2.5 to 3 times as many
  from beyond L2. Some of that may be lock waits, which we have not separated from misses.
- **Where placement matters, it is a few lines or one shape of program.** False sharing of a few
  sibling objects cost a tokio spawn loop 8 to 14%. A magazine order that reversed every 254
  objects made a tree walk 2.5 times slower. Neither moved a server.
- **The fix is usually order, not spacing.** Handing objects out apart everywhere costs what
  adjacency was buying (a tree's build and walk lost 47%). Our rules act on a small, bounded part
  of what is handed out and keep the rest dense and ascending.
- **Where the program's code lands moves a server by 1 to 7%, by job**, as much as most differences
  between good allocators, and the allocator's own size decides where it lands.
- **We have not measured** what share of a server's cross-core misses is false sharing, or how
  many cache lines a request touches under each allocator. See
  ["Not yet measured"](#not-yet-measured).

Job names are defined in [workloads.md](workloads.md). The hardware costs behind this note are in
[hardware.md](hardware.md).

## On servers the gap is instructions

A ratio of cycles is exactly the ratio of instructions times the ratio of cycles per instruction
(CPI), so a gap can be split. Against cralloc, server cycles a request at 70% load, geometric mean
over eight server jobs, geometric standard deviation across jobs in brackets:

| Allocator | Cycles | Instructions | CPI |
|---|---:|---:|---:|
| jemalloc | 1.057 (x1.03) | 1.064 (x1.02) | 0.993 (x1.01) |
| tcmalloc, per CPU | 1.056 (x1.03) | 1.075 (x1.04) | 0.982 (x1.03) |
| mimalloc | 1.083 (x1.05) | 1.057 (x1.02) | 1.024 (x1.03) |
| glibc | 1.254 (x1.09) | 1.175 (x1.09) | 1.067 (x1.08) |

*2026-10-28, cralloc at `2ff7322`. Each job is the mean over five code placements of per-placement
medians. Computed from the standings' raw results. The jobs are the three Connect log jobs, the log
store, and four order service jobs. miniredis and the sozu proxy are left out, since every allocator
is within 1.2% of cralloc on them; with them in, the cycles column reads 1.046, 1.045, 1.065, 1.197.*

Read it with one caution. A CPI ratio under 1 does not show better placement: instructions an
allocator adds can run faster than the program's average, which pulls its CPI down. What the table
does show is that no stall penalty is visible for jemalloc or tcmalloc on top of their extra work:
their extra instructions cost 0.43 and 0.45 cycles each, no worse than the code around them
([servers.md](servers.md)).

The gaps have different shapes by program. CPI against cralloc, per job:

| Job | jemalloc | tcmalloc | mimalloc | glibc |
|---|---:|---:|---:|---:|
| Connect log (HTTP/1.1) | 0.998 | 1.023 | 1.017 | 0.999 |
| Log store, 1M records | 1.002 | 0.951 | 1.088 | 1.026 |
| Order service, axum | 0.989 | 0.963 | 1.026 | 1.156 |
| Order service, axum, HTTP/2 | 0.991 | 0.965 | 0.998 | 1.203 |
| Order service, actix | 0.978 | 0.968 | 1.036 | 1.135 |

- **glibc on the order service is where stalls show.** On axum it fills 810 lines a request against
  cralloc's 612, 231 of them from beyond L2 against 94; on HTTP/2, 1,244 against 912, and 250 against
  88. The order service caches orders that another worker later evicts, so 20% of the blocks followed
  by address are freed on another CPU. We have not split glibc's fills between its own chunk headers,
  the program's objects and lines that move while threads contend for its locks: at 70% load its CPUs
  are busier than cralloc's and at saturation it has many times the context switches.
  (2026-10-21, `f9ee162`, one to three rounds.)
- **mimalloc on the store** runs 11.6% more instructions and 21.4% more cycles. Its stalls were not
  attributed.
- **On the Connect log jobs glibc's gap is all instructions.** On the gRPC job, the one followed by
  address, objects die within 35 us on the CPU that made them, which leaves placement little to get
  wrong.

**The program's own code runs level under cralloc and jemalloc.** Take the allocator's paths out of
each server's cycles. What is left, the program's code and the kernel, is 1.2% higher under jemalloc
(geometric mean over seven jobs, x1.015, range 0.993 to 1.038). That is inside what code placement
alone moves one build. Allocator paths themselves take a geometric mean of 5.0% of the server's
cycles under cralloc (3.6 to 11.9%) and 7.2% under jemalloc (4.8 to 15.2%). (2026-10-22, `040033a`;
two runs each, one build each; profile in [servers.md](servers.md).)

**Misses agree.** Program loads and stores on heap memory, per request, sampled with IBS (AMD's
instruction-based sampling) at 70% load, cralloc / jemalloc:

| Job | L1-missing loads | From another core | From DRAM | Page walks, all user memory |
|---|---:|---:|---:|---:|
| Order service, axum | 258 / 257 | 94 / 92 | 19 / 20 | 2.7 / 4.0 |
| Order service, axum, HTTP/2 | 358 / 355 | 71 / 76 | 31 / 23 | 12.1 / 13.5 |
| Connect log gRPC, HTTP/2 | 1,689 / 1,702 | 41 / 49 | 0 / 0 | 7.3 / 6.1 |
| Log store, 1M records | 3,018 / 2,890 | 714 / 596 | 237 / 255 | 33 / 90 |

The differences sit within the runs' spread. The largest possible exception is DRAM loads on the
order service over HTTP/2 (36, 25 and 32 against 17, 24 and 28 over three runs), worth at most a few
hundred cycles of 65,000. jemalloc's own metadata misses more than ours: 246 L1-missing loads a
request on the store against 55. (2026-10-22, `040033a`.)

**Frees on another CPU cost nothing visible.** A free is a push on the freeing CPU and never writes
into the object. An allocator with an intrusive free list pays for that write when the object comes
back: 7 to 8 ns per 64-byte object within an L3 and 27 ns across. These servers free up to 25% of the
blocks followed by address on another CPU, and cross-core misses stay level with jemalloc's.

## Where placement matters

Each row is a behaviour we found, what the hardware did with it, and what we did.

| Allocator behaviour | Hardware effect | Size | What we do |
|---|---|---|---|
| Magazines popped in a sawtooth of addresses | Each reversal restarts the prefetcher's stream | Tree walk 2.5 times slower | Built: pops ascend |
| Siblings packed, then each written by its own thread | False sharing in the program | Spawn job 8 to 14% slower; servers level | Built: stride the first objects of a run |
| Every fill strided | Allocation-order walks lose adjacency | Tree +47%, JSON +6% | Rejected |
| Allocator's shared words on one line | False sharing in metadata | 6 of 10 lines moved between two cores | Built: `lined` |
| Large blocks at slot starts | 64 streams in one 12-way L1 set | 40.9 against 12.0 cycles an element | Built: stagger by slot index |
| Every class's first pages at the same page numbers | L2 TLB set conflicts | Churn at 10k live, -11.6% | Built: a page origin per class |
| Magazines traded across L3s | Lines fetched from the other L3 | Independent pipelines 1.7 to 2.3 times slower | Built: depot stacks per cache domain |
| Fills that ignore the cache domain | Objects reused across L3s | Independent pipelines 2.6 to 13% | Prototype only: not built |
| Small pages on a dense class | TLB misses, faults | BTreeMap build 6% | Built: huge pages past 16 MiB |
| Small pages on a dense large block | TLB misses, faults | DataFusion 2.9%, HashMap 10%, servers nothing | Documented limit |

The sections below give the evidence.

## False sharing

**What we have found is a hazard of size-segregated allocation, in the program's structures.**
Instances of one type made in a loop by one thread sit side by side. If each is then written by its
own thread, they share lines unless the type is padded. Whether they do depends on which sizes
share a class.

**tokio's run-queue headers are the measured case.** tokio allocates one 40-byte header per worker at
start-up. cralloc and mimalloc have a 40-byte class, so the headers land 40 bytes apart, and each
worker's `head` and `tail` share lines with its neighbours'. jemalloc serves them from its 48-byte
class, interleaved with another of tokio's types, which keeps them apart by luck. Switching the rule
at run time in one binary:

| Job | Stride off | Stride on |
|---|---:|---:|
| Spawn, 4 workers, ns a task | 111.3 | 95.8 (-14%) |
| Spawn, 8 workers, ns a task | 202.4 | 186.3 to 187.6 (-8%) |
| Connect log gRPC, k cycles a request | 129.3 | 129.0 to 130.1 |
| RPC log, 64 connections, k cycles a request | 121.9, 122.9 | 121.9 to 122.7 |

*2026-10-22, on `7a82ea4`, 4 to 6 interleaved rounds, medians. "On" is the settings of 16 and 64
siblings and whole magazines where run. A setting of 4 separates four workers' headers but not
eight's, so a stride must cover at least as many siblings as workers.*

**On a server these structures are few of the misses.** On the order service under cralloc, tokio's
headers are 40 of 4,950 L1-missing load samples over HTTP/1.1, and its handle and headers are 254 of
about 5,900 over HTTP/2, where hyper spawns a task per stream. Separating the headers moved no server
by more than the runs' spread, about 1%. An earlier reading that they cost a gRPC job 3.6% came from
comparing two builds, and was mostly code placement ([measuring.md](measuring.md)).

**Load samples can miss the cost.** On the spawn job the headers show few load misses but cost about
280 cycles a task, in stores and locked operations waiting for the line. A false-sharing measure built
only from load misses would undercount.

**A second structure shows how little it takes.** tokio's scheduler handle put the owned-task
counter, written by every spawn and completion, on a line with idle state that every spawn reads.
Any allocator that starts blocks on a line boundary keeps them together. glibc's 16-byte chunk header
shifts the block by an amount the program's earlier allocations decide: the same glibc binary ran
spawn at eight workers at 192 to 227 ns a task depending on the length of its own path. tokio 1.54
pads this one (tokio #8622), and glibc's lottery shrank to 181 to 184 ns.

**Our own metadata had it too.** In a two-thread pipeline of large objects, six of every ten lines the
producer took from the consumer came from words that only shared a line by accident: a class's layout
memo beside the next class's summary words, and eight classes' stack heads on one line. `crate::lined`
puts each group of shared words on lines of its own and asserts the statics' sizes in lines. Fills
an allocation fell from 6.4 to 5.6, for about 22 KiB more statics, zero and untouched until used.
(2026-10-18.)

**The rule, and what it costs.** Objects of a class allocated one after another are the allocator's
best evidence of siblings. A fill that opens a run hands out its first 64 strides of objects a stride
apart, in classes smaller than a line, and the rest in the fill's order. No slot is skipped; only the
order changes, and only in the first magazine of a run. A stride is the fewest slots at which two
objects are sure to share no line: `64 / size` where the size divides 64, else `ceil(64 / size) + 1`.
A fill opens a run when the first frontier word it records shows no claim this interval, so the rule
adds no state, write or clock read. Details are in `docs/design.md`.

**Rejected:**

- *Dropping the classes that do not divide 64 (40 and 56).* Costs up to 20% of memory on objects of those
  sizes, and two 48-byte objects can still share a line.
- *A line for each object below 64 bytes.* Far too much memory.
- *Moving blocks off line boundaries, as glibc's header does.* A lottery that helps one layout and hurts
  another.
- *Relying on tokio to pad the header.* Padding helped cralloc only: across builds it cost glibc 3 to 8%
  and mimalloc 2 to 3% on spawn.

## Spreading objects over lines and pages

**Order matters more than spacing.** A magazine used to hand out its objects highest address first,
while successive magazines ascended. Preorder walk of a 2M-node tree of 24-byte nodes, TSC ticks a
node, caches flushed: ascending 5.0, descending 5.0, sawtooth reversing every 254 nodes 14.1. The
prefetchers follow a stream in either direction. What defeats them is the reversal.

| Box tree, depth 20 | Sawtooth | Ascending | mimalloc |
|---|---:|---:|---:|
| Walk, M cycles | 531 | 211 | 229 |
| Build, M cycles | 529 | 448 | 355 |
| Drop, M cycles | 1,079 | 627 | 350 |
| DRAM fills, M | 9.9 | 3.6 | 4.0 |

*2026-10-18, `91d97b9`, the tree built ten times a run, 5 interleaved rounds, medians.*

**Handing out every fill apart is what breaks a walk.** A strided order on every fill cost a tree's build
and walk 47% and a JSON parse 6%, BTreeMap and HashMap about 1%. Units of eight strides cut the tree's
loss to 12% and left JSON's. Only the first fill of a run costs nothing measured on either. Whole
magazines strided at run time did not move the RPC servers, whose small classes circulate.

**Set conflicts need many streams or a shared offset.**

- *Large blocks in one L1 set.* Large slots are spaced by a power of two, so 64 buffers written in turn
  put every store in one set of the 12-way L1. A probe ran 7.9 ns an element at slot starts and 0.77
  with blocks moved by a different multiple of 64 bytes. On the suite's 64 vectors grown to 4 MiB:
  40.9 cycles an element at slot starts, 12.0 staggered (jemalloc 14.5, which offsets for the same
  reason; mimalloc 40.0), same peak. Three streams fit the ways and show nothing. The stagger's price: 4 KiB
  of room in a slot, and a block of `k` huge pages then spans `k + 1`. (2026-10-17.)
- *First pages in one L2 TLB set.* Every class region starts on a 4 GiB boundary, so the first pages of
  all classes shared low page numbers. A small heap walked page tables though it used a few hundred KiB.
  Each small class's first slot now lies at its own page origin (`i * 159` pages modulo 256). Origins
  spread over 256 pages removed all the walks, over 16 pages half to five sixths, and offsets of lines
  within a page none. It costs address space only.

| Job | Time | 4 KiB walks an operation |
|---|---:|---:|
| Churn, 10k live | -11.6% | 0.133 to 0.0001 |
| espresso, preloaded | -3.3% | 443k to 6.9k a run |
| RPC echo, 64 connections | -2.3% | 24.1 to 5.5 |
| BTreeMap, forkjoin, mstress | within 1% | level |

*2026-10-19, `700c8ce` against `35b00fa`, 3 or 4 rounds, medians.*

- *L1 and L2 sets on a small heap: real, and too small to matter.* Five in six of cralloc's L1 misses
  hitting L2 fell in the first eight of 64 L1 sets on the spawn job. Spreading them removed those L2 hits
  and changed neither time nor line transfers. (2026-10-19.)

## TLB and huge pages

**On servers, walks are not where time goes.** At 30 to 60 cycles a walk, they are under 0.1% of cycles
on the order service and RPC jobs, 0.3 to 0.5% on the store and 0.5 to 0.9% on Meilisearch. The servers'
walks are on small classes or on blocks under 2 MiB, which no per-block huge page can cover. On the store
cralloc takes about 60% fewer than jemalloc (33 against 90 a request).

**On batch programs, huge pages are most of a build's gap.** Single-threaded build phases that ran
mimalloc's instructions in more cycles were explained by page faults and L1 dTLB misses on objects on 4 KiB
pages. BTreeMap's build is 4,036 M cycles against mimalloc's 3,799: 34.6M L1 dTLB misses against none,
and L1 fills level. With huge pages disabled for every process, cralloc's build phases are level or
faster on all six jobs; with huge pages from a class's first carve, BTreeMap's build is 3,687 M cycles,
ahead of mimalloc. So classes far apart cost nothing beyond their TLB entries, and fills' order and spacing
are not the cause. (2026-10-18, `016877c`.)

**A class switches to huge pages after 16 MiB carved.** 4 MiB would take BTreeMap from 942 to 901 cycles an
entry and tree depth 18 from 39.5 to 35.4, for up to 6 MiB more at a peak. We chose the memory: 16 MiB keeps
cralloc under jemalloc on churn and small maps. `MADV_COLLAPSE` at the switch gains little and costs 0.1 to
0.4 ms a huge page on a slow path, so it is not used.

**Large blocks allocated whole get no huge pages: a documented limit.** A run-time switch in one binary
that advised every whole block of 2 MiB or more:

| Job | Cycles | What moved |
|---|---:|---|
| HashMap, 1M entries | -10.5% | walks 2.63M to 0.22M, faults 53k to 21k |
| DataFusion | -2.9% | half the walks and faults, same peak |
| tpchgen | -2.0% (9 rounds, ranges overlap) | walks halved |
| Meilisearch, a request | -1.1% (inside its spread) | 3% of its walks land in such blocks |
| Order service at 2 GiB, log store, Connect log gRPC | switch never acts | no block of 2 MiB or more |

*2026-10-24, `46126f0`, 1 to 10 rounds by job, medians.*

The cost of being wrong is large and has no ceiling but the block. A block touched in one page in 64
holds 11 to 16 times its memory. Written whole and reused, each life costs 25 times what it does today,
because an advised block's slot is returned at its free. On a host short of free 2 MiB blocks, every
advised fault can stall in direct compaction: lives slowed by up to 17 times in one session. Random reads
over a resident 256 MiB gain only 1 to 3%, since the walk overlaps a DRAM miss each read pays anyway. See
[large-blocks.md](large-blocks.md).

tcmalloc gets no huge pages on this host: zero `AnonHugePages` in every run, preloaded or linked.

## Cache domains

**Keeping magazines in their L3 is worth a lot to independent work.** With one depot, magazines one L3
freed were popped by the other, and every slot and most objects were then a fill from the other L3. A stack
per domain took independent pipelines over two L3s from 11.16 to 4.78 ns an allocation (two pipelines), 9.53
to 5.54 (three) and 10.23 to 5.60 (four), with fills from the other L3 down 40% on sh8bench. Jobs on one L3
moved -1 to +3%. Peaks rose 0.3 to 0.8 MiB on jobs using both L3s. (2026-10-18, `f023136` against `69bcd42`,
5 rounds.)

**Keeping fills in their L3 is not built.** The back end still takes a class's lowest free slots, whoever
freed them. A prototype that carved a region per class and domain, and refilled from a domain's own free
slots before popping another domain's magazine, gave independent pipelines 2.6 to 13% and flows between L3s
2 to 4%, and left everything else level. It added 0.3 to 1.6 MiB at peak, a small-heap floor that grows with
domains, and up to the retained cap held when load moves between domains. That is a narrow gain for a change
across the back end, the launder and the depot, so we left it. Build it only if work spread over many L3s
becomes a target. (2026-10-19, `515412e`; tag `repro/exp-domain-regions`.)

## The allocator moves the program's code

**A server's time moves with where its code lands, and the allocator's size decides that.** Builds identical
except for a padding function of N bytes ahead of everything moved all later code by exactly N. On the Connect
log gRPC job: N = 0, 16, 32, 48 ran 126.9, 127.1, 130.5 and 132.2 k cycles a request, instructions level. Over 90%
of the difference in sampled op-cache misses and decoder-delivered ops falls in the service's protobuf decoder, not
in allocator code. A whole-line shift is as bad as the worst sub-line one by another route, so no single alignment
decides it. The sensitive code is not ours and there is nothing to pin. (2026-10-22.)

Over five placements on 2026-10-28, the spread is 5.0 to 6.4% on the Connect log jobs and 1.7 to 3.1% on the order
service, median of five allocators, and it differs by allocator: cralloc's own is 3.1 to 3.7% and 1.2 to 2.7%.

This changes how to read every number here: see [measuring.md](measuring.md).

## Not yet measured

We want this measured, and nothing above answers it: does an allocator spread a program's objects across
more cache lines, and how does the share of false sharing change between allocators?

1. **Lines a request touches, by allocator.** We have L1 fills a request by source for cralloc, jemalloc and glibc
   on the order service, at one placement and one to three rounds, and for cralloc and jemalloc on the store.
   tcmalloc and mimalloc have none. Fills are also a proxy: they count misses, not distinct lines.
2. **False sharing's share of cross-core misses, by allocator.** We have the size of two known structures on the
   order service under cralloc, not a share of all cross-core traffic, and fills by source cannot tell false sharing
   from true sharing.
3. **The same at more CPUs.** Every server run pins the server to 4 CPUs on one L3. False sharing should grow with
   workers and cost more across L3s. "No server shows it" is a statement about 4 CPUs.

**How we would measure it:**

- *Fills.* Add `ls_dmnd_fills_from_sys` per source (`0xff43` all, `0x143` L2, `0x243` L3 or another core of the
  CCX, `0x443` the other CCX) and exclusive requests (`0x1064`) to the runner's server segments, over the usual
  placement sweep for all five allocators. This is a small runner change. Zen 5 gives five counters with the NMI
  watchdog on and the runner uses two, so three raw events a session. Worth doing first.
- *False sharing.* IBS op samples with data addresses, stores and locked operations as well as loads, joined to an
  allocation trace so each sample lands on a block and byte offset. A line is falsely shared when two CPUs touch it,
  one writes, and their bytes are disjoint. Then confirm causally for any structure that matters by moving it with a
  switch in one binary, as we did for tokio's headers. The trace and shape wrappers exist as a patch at tag
  `research-logbooks` (`bench/probes/spawn-gap/diagnostics.patch`).
- *Distinct lines.* The closest cheap estimate is distinct cache lines among IBS samples in a window, which undercounts.
  A full count needs a memory trace of one request under each allocator, with the allocator's rseq path intact.

**What would change the design:** false sharing or spread worth over about 1% of a server's cycles under cralloc that
another allocator avoids.

**Also open:**

- `SIBLINGS` = 64 rests on nothing measured. 16 is enough for 4 and 8 workers, whole first magazines cost nothing on
  servers, and fewer than the worker count misses headers.
- Classes of one to two lines (80, 96, 112 bytes) also put neighbours on a shared boundary line. No measured case.
- Why tokio 1.54 made the headers visible on some builds was never established.
- A 12% churn slowdown (25.4 against 22.6 cycles an operation) with 22.8M blocked loads appeared in one build that
  differed only in pass order. Its op-cache misses point at code layout, and no address was shown to alias a slab word.
  The run-time switch never showed it.
- A region-level huge-page mark for dense large classes under 2 MiB, such as the order service's 1.25 MiB table shards,
  and whether it moves anything at 3.8 walks a request.
- Everything here is one CPU model. See [hardware.md](hardware.md).
