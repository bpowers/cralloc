# What we learned building cralloc

This directory says what we know about allocator performance, and why. `docs/design.md` says what the
allocator does. Each section here is a summary of the note it links, which holds the evidence.

Everything was measured on one machine unless stated: Ryzen 9 9950X (Zen 5, two L3s of 32 MiB), Linux
7.2.9, glibc 2.43. The rules should carry to other machines. The numbers will not.

## What gates the fast path

An inlined allocate and free costs 5.0 cycles at a constant size and 7.3 at a size known only at run
time. The floor for a per-CPU magazine under rseq is 4.0, and for any magazine 2.0. Through two calls
every shape costs 8.0. Incumbents cost mostly 10 to 15 through `GlobalAlloc` with fat LTO. On real
servers a pair costs about 14.

What sets these cycles is how each access gets its data. Ranked by what one event costs:

| Access pattern | Cost on Zen 5 | What we do |
|---|---|---|
| A cache line moved between cores | 90 to 125 cycles in an L3, about 440 across | Per-CPU slabs: no shared line per operation |
| A write into freed memory | 40 to 45 cycles in an L3, about 150 across, paid by the next user | A free writes only the CPU's own magazine |
| An atomic | 20 cycles uncontended | None on the fast path; the commit is a plain store |
| A load at the page offset of a store in flight | A loop goes from 1 cycle a step to 2, 10 or 18 to 25 | Hot words sit at offsets nothing loaded can overlap; the benchmark runner refuses a build that collides |
| A reload that loses memory renaming: `movzx`, an absolute address, a hop across `call [mem]` | 5 to 7 cycles a hop, against 0 | Hot state is 64 bits behind a base register, which the Lean model enforces |
| A store whose address arrives late | About 12 cycles in a loop of mixed sizes | Take the class from the size, not through the pointer |
| A straight run past the front end's windows | 1 cycle a call; 7 to 9 if the next load waits on memory | At most 13 fused micro-ops inlined, checked in Lean |

With those removed, what is left in a tight loop is retirement width: a pair retires 33.7 ops, and at 8 a
cycle that alone is 4.2 of its 4.96 cycles. In real programs it is misses and where the caller's code lands.
Byte alignment and byte savings did not matter. [roofline.md](roofline.md)

## What the hardware charges

| Operation | Cost | Whose price |
|---|---:|---|
| Uncontended atomic | 20 cycles | Zen 5 |
| `call` and `ret` | 4 cycles | Zen 5 |
| Cache line to another core, same L3 / other L3 | 16 to 22 / 76 to 79 ns | Topology |
| First touch of a 4 KiB page | 1,750 to 2,850 cycles, by how far the page falls out of cache | Kernel, cache, DRAM |
| The same memory inside a 2 MiB huge page | 117 to 214 ns per 4 KiB | DRAM bandwidth |
| `MADV_DONTNEED` of one page, alone / with other threads | 0.36 / 1.80 us | Kernel, core count |
| Clock after 1.5 s of idle | 4.38 GHz for 200 to 260 ms, then 5.7 | Platform |
| Exit from the C2 idle state | 18 us | Platform |

A page fault is the most expensive thing an allocator causes, as much as hundreds of inlined pairs. Most of it
is bookkeeping, so reuse comes before fresh pages, and `MADV_POPULATE_WRITE` saves only about 600 cycles a page.
Returning part of a huge page frees nothing until the kernel splits it. [hardware.md](hardware.md)

## What the allocator does to the program

**On servers the gap between good allocators is instructions, not stalls.** Against cralloc, geometric mean
over eight server jobs (geometric standard deviation), cycles = instructions x cycles per instruction:

| Allocator | Cycles | Instructions | Cycles per instruction |
|---|---:|---:|---:|
| jemalloc | 1.057 (x1.03) | 1.064 (x1.02) | 0.993 (x1.01) |
| tcmalloc, per CPU | 1.056 (x1.03) | 1.075 (x1.04) | 0.982 (x1.03) |
| mimalloc | 1.083 (x1.05) | 1.057 (x1.02) | 1.024 (x1.03) |
| glibc | 1.254 (x1.09) | 1.175 (x1.09) | 1.067 (x1.08) |

The program's own misses on heap memory are level under cralloc and jemalloc: 258 against 257 L1-missing loads
a request on the order service, 94 against 92 from another core. Stalls appear when an allocator gets it badly
wrong: glibc fills a third more lines a request on the order service, and 2.5 to 3 times as many from beyond L2.

**Where placement does matter, it is a few lines or one shape of program:**

| Allocator behaviour | Hardware effect | Size | What we do |
|---|---|---|---|
| Magazines popped in a sawtooth of addresses | Each reversal restarts the prefetcher's stream | Tree walk 2.5 times slower | Pops ascend |
| Siblings packed, then each written by its own thread | False sharing in the program (tokio's run-queue headers) | Spawn job 8 to 14% slower; servers level | Stride only the first objects of a run |
| Every fill strided | Allocation-order walks lose adjacency | Tree +47%, JSON +6% | Rejected |
| Allocator's shared words on one line | False sharing in metadata | 6 of 10 lines moved between two cores | Shared words on their own lines |
| Large blocks at slot starts | 64 streams in one 12-way L1 set | 40.9 against 12.0 cycles an element | Stagger by slot index |
| Every class's first pages at the same page numbers | L2 TLB set conflicts | Churn -11.6%, walks nearly gone | A page origin per class |
| Magazines traded across L3s | Lines fetched from the other L3 | Independent pipelines 1.7 to 2.3 times slower | Depot stacks per cache domain |
| Small pages on dense large blocks | TLB misses, faults | DataFusion 2.9%, HashMap 10%, servers nothing | Documented limit |

Page walks are under 1% of cycles on every server job. Where the program's code lands moves a server by 1 to 7%,
as much as most differences between good allocators, and the allocator's own size decides where it lands.
[program-effects.md](program-effects.md)

## Where memory goes

- **On a steady large heap we hold the least and do not drift.** A 2 GiB cache churned for 15 billion operations ends
  at 1.105 times live: tcmalloc 1.127, jemalloc 1.148, glibc 1.27, mimalloc 2.97. Most of what is left is class
  rounding, which jemalloc pays too. A 10.6-hour Qdrant soak beside jemalloc drifted in neither.
- **The heap's fixed numbers hold to 10 GiB.** The order service costs the same cycles a request at 10 GiB as at
  0.5 GiB, at 1.073 times live against jemalloc's 1.095.
- **Our loss is troughs, to jemalloc alone, and the cause is reuse order.** At a 2 GiB trough we hold 0.95 GiB to
  jemalloc's 0.66, on the same live slots. Each allocation takes the slot the last free pushed, so survivors stay
  spread over the peak's footprint: 273 MiB of free slots beside live neighbours, against 40. The other incumbents
  hold over 2.1 GiB.
- **When the size mix wanders, huge pages ratchet.** Free memory in a partly used 2 MiB unit could not return: 1.10
  climbed to 1.21 times live. The tail rule took that to 1.14 for 1 to 1.5% of cycles on an allocation-bound job and
  nothing measurable on servers.
- **Freed large blocks cross classes only through the OS, so no single hold time serves all programs.** A hold of at
  most about 125 ms keeps DataFusion's peak, and Meilisearch's blocks come back up to 2 s later. Faults turned out to be
  the cheap side of the trade: a return and refault costs 2.4 to 7 kcycles, and these rules move tens of MiB of peak for
  a percent or two of cycles. [large-blocks.md](large-blocks.md)
- **Servers differ in how much of this they engage.** The order service runs no clocked, hold or growth work at any size
  to 10 GiB, and the log store's tail rule acts only after close. Meilisearch and Qdrant do engage these rules.
  [memory.md](memory.md)

## Servers and tails

- **A server's calls are simple.** About 140 allocations a request on the order service and the RPC jobs, three
  quarters in two classes, with almost no slow path, system call or fault in steady state.
- **Our share is 5.0% of a server's cycles, jemalloc's 7.2%**, geometric mean over seven jobs (x1.53 and x1.47),
  profiled before a leaner `realloc`, which by subtraction brings ours to about 4%. What is left is the fast path
  and `realloc`. Spending on slow paths pays almost nowhere.
- **Objects flow rarely, and a cache's ends are the danger.** A swap with a spare costs about 640 cycles on a real
  server. The worst failure was a lock at a magazine's ends, which cost the order service 15 to 17% and reached a
  p99.9 of 6 ms. Leaving a magazine half full after two reversals removes it. [magazines.md](magazines.md)
- **At one fixed rate a tail can rank the CPU's idle states.** A faster server idles longer and sleeps deeper. With
  deep idle off our p99.9 on Connect over HTTP/2 fell from 45 to 37.8 us, the lowest, and nothing else moved. With it
  off, jemalloc, tcmalloc and cralloc sit within 4% at p50, p99 and p99.9 alike: what remains above p50 is the program's.
- **The tails we caused were maintenance on a request's path.** The magazine-end lock and purge work are gone. One
  remains: on the log store the first 5 ms after a silence reach 581 us against jemalloc's 279, from idle drains.
  [servers.md](servers.md)

## Where we stand

On servers cralloc does the least work: ahead or level on every job, behind on none. Over ten jobs the others'
cycles a request against cralloc's, geometric mean (geometric standard deviation): tcmalloc 1.045 (x1.04), jemalloc
1.046 (x1.03), mimalloc 1.065 (x1.05), glibc 1.197 (x1.13). On batch programs we are level, losing in the kernel to
page faults what we win in user cycles. On large heaps under load we hold the least. We are behind in troughs, on small
heaps at rest by a few MiB, and in the first requests after a long silence. [standings.md](standings.md)

## Measuring without fooling ourselves

Each of these produced a wrong conclusion that we wrote down before we found it:

- **Two builds compared by one number each.** A difference blamed on tokio's headers (3.6%) was mostly code placement.
  Switch behaviour at run time in one binary, or compare means over a placement sweep.
- **Tails at one fixed rate.** These can rank idle states. Check deep-idle entries first.
- **Short jobs after idle.** The first 250 ms run at 4.38 GHz, and runs of the same cycles spread by 30% in time. Warm
  the clock first.
- **glibc's "wins".** A spawn job ran 192 to 227 ns a task depending on the length of the binary's path.
- **Single runs on a shared machine.** Memory-bound jobs drifted by up to three times. Interleave, pin to idle cores, and
  do not multiplex counters.

[measuring.md](measuring.md) has these and the method. [workloads.md](workloads.md) defines every job named here.

## What we do not know

- **Whether an allocator spreads a program's objects over more cache lines, and how the share of false sharing changes
  between allocators.** We want this measured. We have L1 fills by source for two or three allocators and the size of two
  known structures, nothing across all five, and fills cannot separate false sharing from true sharing.
  [program-effects.md](program-effects.md), "Not yet measured", has how to measure it.
- **Anything beyond one CPU model.** A second machine would show which rules are Zen 5's.
- **Anything beyond four CPUs.** Every server run pinned the server to four, so we have not seen what magazines park on 32
  CPUs, or how false sharing grows.
- **Real traffic.** Our servers are real programs under synthetic load.

Each note ends with its own open questions.

## The notes

| Note | The question it answers |
|---|---|
| [roofline.md](roofline.md) | What gates the fast path? |
| [hardware.md](hardware.md) | What does the hardware and kernel charge, and which of it is Zen 5's? |
| [program-effects.md](program-effects.md) | How does the allocator change the program's performance? |
| [memory.md](memory.md) | Where does memory go? |
| [large-blocks.md](large-blocks.md) | What governs blocks past the slab classes? |
| [magazines.md](magazines.md) | How do objects flow between caches, depot and back end, and what does that cost? |
| [servers.md](servers.md) | What does a real server ask of an allocator, and what sets its tails? |
| [standings.md](standings.md) | Where do we stand against the incumbents, and where are we behind? |
| [measuring.md](measuring.md) | How do we measure, and what has fooled us? |
| [workloads.md](workloads.md) | What is in the suite and why? |
| [prior-art.md](prior-art.md) | What do other allocators do, and what did we take or reject? |
| [verification.md](verification.md) | How do we know it is correct? |
| [linux-rseq-contract.md](linux-rseq-contract.md) | What does the kernel promise about rseq? |
| [portability.md](portability.md) | What changes under musl, static linking, thread stacks and Miri? |

## Rules for this directory

**A note answers a question.** It leads with the answer and what it means for the design. Then it gives only the
evidence that carries it: a statistic with its spread (geometric mean, or median with a standard deviation or range),
the commit, the date, and the method in a line or two.

- **A finding goes into the note on its topic, by rewriting the claim.** Git keeps the old one. A new note is for a new
  question, and a note heading past about 700 lines is turning into a log.
- **Keep the negative results.** "Tried X, it cost Y, rejected because Z" saves the next person weeks.
- **Say what we do not know,** and where two measurements disagree.
- **No session logs, per-run tables or raw results here.** Raw results stay in `bench/results/`, which git ignores.

The session logs these notes were written from, up to 2026-11-06, are at the tag `research-logbooks`. The raw results they
cite are in the release `raw-results-2026-10-28`, with a map from the commit hashes of that time to today's.
