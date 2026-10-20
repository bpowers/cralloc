# Sizing buffers on evidence: our constants, prior art, and what carries over

2026-10-19, at `4cefa63` (magazines of up to 1,024 slots). Reading and arithmetic only: no
code changed and nothing was timed. Prior art was read from the sources in `third_party/`
(tcmalloc f3c6669a, mimalloc 3.5.3, Go) and from fresh checkouts and papers (jemalloc `dev`
975f3b0, illumos `kmem.c`, FreeBSD `uma_core.c`, Linux 7.2.9 and master, snmalloc, glibc
2.43, scudo, rpmalloc). Appendix A lists our constants, Appendix B the prior art in detail,
Appendix C the sources.

The question: raising one constant, the largest magazine, from 254 to 1,024 slots was worth 5
to 20% on many jobs for 1 to 3 MiB on small heaps (`launder-and-fill.md`, "Magazines of up to
1,024 slots"). Could such constants size themselves, by one general mechanism? The working
hypothesis: every such constant sizes a buffer between a cheap tier and a dearer one; every
buffer can grow on regret and shrink on idleness, with thresholds derived from measured costs
and one exchange rate between memory held and time spent.

## Summary

1. **The magazine size can adapt without a new instruction on the fast path.** Give a class
   the mask of a small magazine, `m` slots, and magazines of `n` slots, `n` a multiple of `m`,
   aligned to `n`. The two slot tests then fail at every `m`-slot boundary inside the
   magazine, not only at its ends, and never let a routine leave it. Each boundary becomes a
   checkpoint where a cheap slow path, with no shared state, decides whether to go on into the
   next segment or to exchange. The effective capacity of each CPU's magazine of each class is
   then a slow-path decision in steps of `m`. This is new as far as we found: tcmalloc pays for
   adjustable capacity with a capacity field the fast path reads on every call; Bonwick, UMA
   and Linux sheaves change a size for the whole cache under a lock. It needs the routines'
   precondition restated and its checkpoint cost measured ("Segmented magazines", below).
2. **One exchange rate works only if a parked object is priced by the page it pins, and it
   cannot bound a peak.** The rate our purge implies (a 4 KiB page kept about 1.5 s to save
   its 566 ns refault, about 400 cycles per KiB-second) prescribes, by the economic order
   quantity, magazines of tens of thousands of 8-byte objects if an object costs its own 8
   bytes, and about 1,400 if it costs the page it can pin. 1,022 sits there. But our memory
   goal is the peak (at or under jemalloc), and a price on byte-seconds says nothing about a
   peak that lasts a tenth of a second. Every system with one knob that works (Go's GOGC and
   memory limit, the JVM's time ratio, tcmalloc's per-CPU budget) states a bound on space and
   lets the mechanism spend it; none prices space. So the knob should be a budget, divided
   among buffers by their regret, with the exchange rate used only where time is the point:
   how long idle memory is kept.
3. **The tiers are in series, so sizing them one at a time moves the others.** The 1,024-slot
   change cut the depot stacks and spares of the classes up to 64 bytes, counted in magazines,
   by four: a domain stack now holds about one magazine of a 64-byte class, and no class from
   24 to 128 bytes keeps a fixed full spare (`launder-and-fill.md`, "What it costs"). A buffer's size
   should be stated in units of the batch of the tier above it, and adapted with it.

**Verdict on the hypothesis.** "Grow on regret, shrink on idleness" is what everyone does:
tcmalloc, jemalloc, Bonwick's kmem, UMA, Linux's per-CPU page lists and scudo, and our own
stock, depot shed and frontier. Nobody derives the thresholds from measured costs (tcmalloc's
source calls its scores "intuition", UMA's "somewhat arbitrary", Linux's "kind of arbitrary").
Deriving them is the new part, and so are segmented magazines and doing all of it on slow paths
with no background thread or stop of a CPU. It breaks in four places: a peak needs a budget,
not a price; the cost of a parked object is not local or visible when it is parked; tiers in
series; and the worst single call, which must stay a hard bound, not a third currency.

**What to adapt first:** a CPU's spares, by extending stock's rule to every class and
dropping the even split; the magazine's effective capacity, by segments, with the same signal;
the depot's shared target, by ghost hits. Not worth adapting: the interval, the purge's quarter
and the cap, the huge-page thresholds, headroom, the work bounds, the look and heartbeat
pacing, commit-ahead. The refresh rule should be judged for removal rather than tuned.

## What the prior art does

Every adaptive mechanism found has the same shape: a regret signal grows the buffer by a step,
idleness measured as a low-water mark over a window shrinks it, and a fixed bound clamps it.
What differs is the signal, the window, and whether a total is shared.

| System | Buffer | Grows on | Shrinks on | Bound |
|---|---|---|---|---|
| tcmalloc per-CPU | capacity of each class on each CPU | a miss; past two batches, an overflow followed by an underflow | quiescent ticks, by stealing one slot at a time | 1.5 MiB per CPU, zero-sum between classes (2 s) and CPUs (5 s) |
| tcmalloc HugeCache | free huge pages kept | a miss: limit = 1.1 x the demand dip over 1 s | a minimum over 2 s above a fifth of the limit; slower than growth on purpose | none but the floor of 10 |
| tcmalloc filler | pages not subreleased | demand peak over 10 s and trend over 120 s | release above that | the peak over the window |
| jemalloc `dev` tcache | fill count and retention per bin | a refill of an empty bin doubles the fill | a GC visit with low water above zero halves it; retention = used + a quarter | 2 x slab objects, 20 to 200 |
| jemalloc decay | dirty pages | none (reuse) | a 10 s smootherstep curve over 200 epochs | none |
| Bonwick kmem | magazine size for the whole cache | more than 3 failed `tryenter`s on the depot lock in 15 s | never; the depot's working set (min over 15 s) is reaped under memory pressure | a table of 9 sizes |
| FreeBSD UMA | bucket size | +1 each time the zone lock is found held | -1 on trim, halved on severe shortage | 254 |
| Linux per-CPU pages | `high` | +batch on each refill | -1/8 a second | `high_min` to managed/8 per CPU; memory pressure overrides |
| scudo | release threshold | half the pending delta | halves when a release frees more than last time | half a group |
| snmalloc (unmerged) | large-object cache per size | consecutive misses exceed flushes in an epoch | flushes exceed misses: by half the difference | none |
| Go scavenger | retained pages | heap goal x 1.1 | the heap goal; 1% of CPU | GOMEMLIMIT |
| asmalloc stock | spares past the fixed limit | a round trip to the depot that met contention | the low-water mark of an interval | the CPU's spare budget less the fixed limits |
| asmalloc depot shed | depot stack | a push | the low-water mark of an interval | the domain cap; the shared target |
| asmalloc frontier | free pages kept | a fill that reaches higher | pages above two intervals' frontiers, a quarter a pass | the retained cap, charged to growth |

Six things the prior art teaches.

- **The shrink must be slower than the growth, or a buffer that is exactly right looks idle.**
  tcmalloc says so of its per-CPU caches (a list with no misses is shrunk and regrows) and of
  its HugeCache ("require stronger evidence to reverse an expansion than to make it"); Linux
  grows by a batch per refill and decays an eighth a second; snmalloc's branch grows by the
  whole difference and shrinks by half.
- **The useful regret signal is a round trip, not a miss.** A miss on a one-way flow says
  nothing a bigger buffer would fix. tcmalloc grows past two batches only on an overflow
  followed by an underflow, and calls it "the only case when we could benefit from larger
  capacity". Linux's per-CPU lists learned this the hard way: a CPU that only allocates grows
  `high` to its maximum, and two follow-up fixes, one a 30% regression, came from counting
  frees. Our stock already requires a round trip.
- **Contention is the cost signal everyone uses for a shared tier**, because it is free to
  observe: Bonwick and UMA on their locks, our stock on a failed compare-and-swap.
- **One-way ratchets fail.** glibc's mmap threshold and rpmalloc 1.3's thread cache only
  grow; rpmalloc dropped its rule in 1.4, and Bonwick's code notes that shrinking "might be
  useful".
- **Learned models did not survive; static proxies and simple feedback did.** LLAMA's learned
  lifetimes (ASPLOS 2020) became a fixed span-capacity threshold in tcmalloc (ASPLOS 2024:
  +1.0% throughput, -0.8% memory). jemalloc's `dev` branch just replaced six tcache options
  with doubling and halving rules. No published allocator autotuner runs in production; Google
  tunes by fleet A/B experiments and Meta by hand per service.
- **One knob works when it is a ratio with a meaning, measured costs fill in the model, and a
  hard cap stands behind it.** GOGC is a heap ratio, and the pacer measures allocation and scan
  rates to turn it into CPU; the memory limit was added because a ratio cannot stop a spike,
  and a 50% CPU cap was added so the limit cannot spiral. G1's time ratio, calibrated while the
  heap never shrank, shrank it too far once it could (JDK-8238687 doubled the default). ZGC's
  new intensity knob leaves its mapping to CPU "intentionally unspecified". Go keeps two
  separate implicit rates (retain 10% over the goal; spend 1% of CPU scavenging) and never
  unifies them.

What tcmalloc's per-CPU stealing bought, at fleet scale: the default cache halved from 3 to
1.5 MiB per CPU with no loss, and fleet memory fell 1.9% (Zhou et al., ASPLOS 2024, section
4.1). It needs a background thread and stops a CPU to move capacity.

## What the theory prescribes

Write `lambda` for the exchange rate in cycles per byte-second, `K` for the cycles a miss
costs, `b` for the bytes a buffer holds.

- **Ski rental** (Karlin et al. 1988): keep an idle thing until holding it has cost what
  replacing it would, `T = K / (lambda * b)`, then let it go; this is within twice the best
  possible. For a retained page, `K` is a fault and a zeroing; for a parked magazine, a launder
  and a fill; for a depot magazine, the same. Bigger things are kept for less time.
- **Economic order quantity** (Harris 1913): a batch of `Q = sqrt(2 D K / h)` objects, `D`
  the rate at which they are used and `h` the holding cost of one per second. For a magazine:
  larger for a busy class and a dear exchange, smaller for big objects, and growing only with
  the square root of each. A depot stack is the safety stock of an (s, S) policy and depends
  on the variance of the net flow, which a low-water mark measures directly.
- **Working set and page-fault frequency** (Denning 1968; Chu and Opderbeck 1972): keep what
  was used in a window, and grow when misses come faster than a threshold. Our shed, sweep and
  frontier are working-set rules with a window of an interval or two.
- **ARC's ghost lists** (Megiddo and Modha 2003): remember what was evicted, not its contents,
  and grow when a request would have hit it. A ghost hit is regret made observable. Our stock's
  "sent" mark is a ghost of one magazine. For the depot: count what a stack laundered over its
  target in an interval, and treat fills of the class in the next interval, up to that count,
  as ghost hits. For a purged page: a fill that reaches a page purged in the last few
  intervals.
- **Slow start and AIMD** (Jacobson 1988; Chiu and Jain 1989): when many consumers share one
  budget, additive growth and multiplicative shrink converge to a fair and full share. Spares
  of 44 classes sharing a CPU's budget, and CPUs sharing a heap's, are that case.

### The numbers, and what they say about one exchange rate

From the design's measurements (4.3 GHz; first touch of a 4 KiB page 566 ns, about 2,400
cycles; inside a huge page 119 ns, about 510): our purge returns a page after one to two
intervals unused, about 1.5 s. Read as ski rental, that is `lambda = 2,400 / (4 KiB * 1.5 s)`,
about **400 cycles per KiB-second** for small pages and 85 inside huge pages.

At that price, the order quantity for an 8-byte class at a million objects a second per CPU,
with sh8bench's contended exchange of 1,600 cycles:

- priced by its own bytes, `h = 400 * 8 / 1024`, about 3 cycles per object-second, and
  `Q = sqrt(2 * 10^6 * 1,600 / 3)`, about **32,000 objects**;
- priced by the 4 KiB page a scattered object can pin, `h` about 1,600, and `Q` about
  **1,400 objects**.

We chose 1,022. So one rate can explain both the purge and the magazine, but only if a parked
object's cost is the memory it keeps resident, which lies between its own size (a fresh fill,
which `memory.md`, "Small heaps", found costs nothing) and a page (a scattered one). That cost
is not visible when the object is parked. The launder sees it afterwards: it judges each batch
dense or scattered (`back::class`, `SCATTERED`; `back::group`, `JUDGE`), which is a cheap
measure of the pinning factor after the fact. The same arithmetic for a parked magazine of 64
KiB whose refill costs about 50,000 cycles gives `T` of about 2 s, close to the one interval
we use; the interval is roughly what ski rental would give and is not worth adapting.

What one rate cannot do is bound the peak. A pipeline that runs for a tenth of a second has
almost no byte-seconds to weigh against its exchanges, so a price says to hold everything,
while our goal is jemalloc's peak. That needs a budget.

## Segmented magazines

A routine's tests are `word & mask == 0` (allocation fails at slot 0 or 1) and
`(word + 8) & mask == 0` (free fails at slot `n - 1` or 0), with `mask = 8n - 16`. Both look
only at the slot number modulo `n`. Give a class the mask of `m` slots and magazines of `n`
slots, both powers of two, `m` dividing `n`, each magazine aligned to `n`:

- allocation fails when the slot number modulo `m` is 0 or 1, free when it is `m - 1` or 0, so
  the routines never read or write the two slots on either side of an `m`-boundary;
- slot 0 and slot `n - 1` of the magazine are still such slots, so no routine can run off
  either end;
- the slots the routines never touch, two at each interior boundary, hold nothing: a magazine
  of 1,024 slots with segments of 256 holds 1,016 objects instead of 1,022.

When a free fails at an interior boundary (next free slot `k*m - 1`), the slow path writes the
object at slot `k*m + 1` and moves the word to `k*m + 2` with the store-if-equal routine it
already has, or it exchanges as today. When an allocation fails at an interior boundary (next
free slot `k*m + 1`), it takes the object at `k*m - 2` and moves the word to `k*m - 2`. The
write before the commit is the free routine's own pattern, so a drain that takes the magazine
in between leaves the thread holding its object, as a failed push does now. No shared state is
touched and no atomic instruction is executed.

So each CPU's magazine of each class has an effective capacity, a multiple of `m`, that the
slow path chooses at every checkpoint: go on, or retire the magazine as it is. The fast path's
instructions do not change; its constant does. What changes elsewhere:

- The routines' precondition says "a magazine of the class's size"; it would say "a magazine of
  a multiple of the mask's size". The Lean proofs bound every access within the `m`-aligned
  window of the word, which lies inside the magazine, so they should carry over, but they must
  be redone.
- Every slow-path count of a magazine's objects (the level in the word, fills, launders, drains,
  the age limit, the depot's depth) must skip the empty slots. That is arithmetic on fixed
  positions.
- A thread that oscillates across an interior boundary takes a slow path every second or so
  call, as one does at a magazine's ends today without spares. The checkpoint should treat a
  reversal there as a round trip and do what an exchange does now (swap with a spare), which
  bounds the cost by today's.
- A checkpoint costs a call to the slow path and one routine, perhaps 30 to 100 cycles, against
  about 1,600 for a contended exchange. That estimate is the first thing to measure. It also
  restores the heartbeat's frequency for the smallest classes, which the larger magazines cut
  by four.

With `m` = 256 and `n` = 1,024 (or 4,096, where sh8bench, the tree and JSON kept gaining),
the slow path could keep the full size where magazines round-trip under contention and stop at
one segment where a CPU's flow of a class goes one way, which is what `memory.md` ("Small
heaps", "What is left") proposed and could not then build: "a magazine size that adapts per
class and CPU (short on a CPU whose frees of a class go one way, full where it both allocates
and frees)". The physical magazine costs 8 KiB of slots whatever its effective size; the
magazine area is never returned, so that is a fixed cost per magazine made.

Two other ways were considered and are worse. A capacity field the fast path reads, as
tcmalloc packs `current` and `remaining_capacity` into one 32-bit header, costs a load or more
instructions on every call. Changing a class's mask at run time (the C path reads it from a
table) needs every magazine of the class on every CPU to match it, which takes a drain of every
slab, and the Rust path's mask is an immediate.

## Where the hypothesis breaks

- **A peak is not an integral.** See above. Our memory goal and the cgroup limits programs run
  under are peaks. Keep a space budget (the CPU's spare budget, the depot targets, the retained
  cap), possibly as a fraction of live memory with a floor, as GOGC is; divide it by regret; use
  a time constant only for how long idle memory is kept.
- **Regret on the memory side is not local.** A parked object costs the page it pins, which
  depends on where its neighbours are, and is known only when it is laundered or purged. A
  refault after a purge is visible only as a fill reaching a page purged earlier. Both can be
  observed late and cheaply; neither can be priced when the decision is made.
- **Tiers in series.** A larger magazine fills the next tier's byte budget in fewer magazines
  and hides the misses that tier would have seen; a larger domain stack hides the shared
  stack's. Sizes should be stated in the units of the batch above (magazines, not bytes, for
  spares and depot caps), and a class's budget should cover its loaded magazine, spares and
  stacks together, or each tier's adaptation will chase the others.
- **The worst single call is a third currency.** The bounds on fill claims (`FILL_CLAIMS`) and
  laundered objects (`LAUNDER_BUDGET`) exist so that one `malloc` is never long, and Linux set
  its batch scale (5) from a latency budget of 100 us. Pricing that in cycles would let one
  call do a thousand times the work of an average one. Keep these as hard bounds outside the
  knob.
- **Oscillation.** Every adaptive allocator damps it the same way: grow by a larger step than
  it shrinks, and require more evidence to shrink (tcmalloc's multi-tick scores and 2 s window,
  Linux's eighth a second, snmalloc's half). Our low-water rules shrink by exactly the depth not
  used, which is stable for a stack; a capacity that can be shrunk to zero by one quiet
  interval needs a floor or a slower decay.
- **Signals that disappear.** Our heartbeats come from slow paths, and the larger magazines
  made them four times rarer for the smallest classes; a perfectly sized buffer produces no
  misses at all. Linux's decay stopped on NOHZ-full CPUs for the same reason. Segment
  checkpoints and `maintain` both help.

## What to adapt first

1. **A CPU's spares.** Extend stock's rule to every class and drop the even split of
   `SPARE_BUDGET` over 44 classes and the `MAX_SPARES` cap: credit on a round trip that met
   contention, the interval's low-water mark off the credit, the CPU's budget as the bound.
   Everything needed is built, and the even split is now wrong for the classes that matter
   (none of 24 to 128 bytes keeps a fixed full spare). Signal: stock's. Fast path: nothing.
2. **The magazine's effective capacity, by segments.** Same signal: a checkpoint goes on
   while the class's exchanges on this CPU round-trip under contention, and stops at one
   segment while its flow goes one way. Expected: the 1,024-slot (or 4,096) gains on sh8bench,
   the tree, JSON and the pipelines that round-trip, and main's peak on the one-way pipelines
   and mpmc. First measure a checkpoint's cost with a constant change (`m` = 256 masks on
   1,024-slot magazines, always going on); then the proof change; then the rule.
3. **The depot's shared target, by ghost hits.** Count the objects a class's shared stack
   launders over its target in an interval; fills of the class in the next interval, up to that
   count, raise the target by what they took; the shed lowers it as now. In sh8bench 37M of 51M
   laundered objects overflowed the shared target and were filled again; a 64 MiB target took
   its cycles from 12.8 to 12.3 G (`launder-and-fill.md`, "sh8bench, accounted"). The gain is
   modest, so this is third.

Not worth adapting, for now:

- **The interval** (`IDLE_TICKS`, `DECAY_TICKS`): it is the time base of every rule, it is
  roughly what ski rental gives for both pages and magazines, and making it move would move
  everything at once.
- **The purge's quarter, its floor and the retained cap**: the after-peak curve already
  matches jemalloc's (`memory.md`, "After the peak"), the frontier is already a working-set
  rule, and the cap is a bound, which is what a budget should be. A refault-driven rate (a fill
  reaching a page purged in the last few intervals) is the version to build if a server shows
  refaults.
- **Huge-page thresholds** (`HUGE_AFTER_HUGE_PAGES`, `HUGE_MIN_SLOTS_PER_PAGE`, the grown-block
  rules): each is a one-time decision on evidence already, and its mistake is bounded by one
  huge page per class.
- **Headroom for growing blocks**: it spends address space, and the copying it saves falls only
  with its logarithm (`memory.md`, "Headroom for growing blocks").
- **The kept grown-block budget** (`KEPT_GROWN_HUGE_PAGES`): tcmalloc's HugeCache is exactly this
  buffer with grow-on-miss and slow shrink, so it could adapt; but the 8 to 32 MiB sweep moved
  only one loop, so wait for a workload that asks.
- **Work bounds** (`FILL_CLAIMS`, `LAUNDER_BUDGET`, `PURGE_STEP_WORDS`, `PURGE_CAP_WORDS`,
  `MOVE_PIECE_HUGE_PAGES`): worst-call constraints, not buffers.
- **Pacing** (`LOOK_EVERY`, `LONE_EVERY`, `MAINTAIN_EVERY`) and **commit-ahead**: overhead
  amortization with no regret signal; committing 16 MiB at a time changed nothing on sh8bench.
- **The refresh rule** (`REFRESH_EVERY`): with it off, the mixed pipeline's peak was 8.8% lower
  and mpmc's 6.4% lower, and the after-peak run unchanged (`baselines.md`, section 10). Measure
  whether it still earns its place before tuning it.

## What we are least sure of

- The cost of a segment checkpoint and of the oscillation guard; nothing here was timed.
- Whether the routines' proofs carry over to `m < n` as cleanly as argued.
- The implied exchange rate rests on two measured costs and an assumed rate of use; it is an
  order of magnitude, not a value.

## Appendix A: our constants

Unit, the buffer each sizes and between which tiers, what adapts it now, and what
`docs/research/` says. "Fixed" means a compile-time constant with no evidence acting on it.
Paths are under `crates/asmalloc/src/`.

### Front end and spares

| Constant | Where | Value | Sizes | Adapts now | Research |
|---|---|---|---|---|---|
| `MAGAZINE_BUDGET` | `classes/magazine.rs` | 64 KiB of objects | loaded magazine: fast path / slow-path exchange | fixed (mask is a fast-path constant) | `memory.md`, "Magazine budget": 128 to 64 KiB took the mixed pipeline from 7.07 to 5.12 MiB for +5.1% |
| `MAX_LOG2` | `classes/magazine.rs` | 1,024 slots | the same, for classes up to 64 B | fixed | `launder-and-fill.md`, "Magazines of up to 1,024 slots": sh8bench -22%, pipelines -10 to -18%, +0.5 to 3 MiB on small cross-thread heaps; sh8bench, tree, JSON keep gaining to 4,096 |
| `SPARE_BUDGET` | `depot/spare.rs` | 1 MiB of objects per CPU | full spares and stock: loaded magazine / depot | stock grows within it | `launder-and-fill.md`, "Stock": four times the room bought 11 ms on one L3, 22 on two |
| `MAX_SPARES` | `depot/spare.rs` | 4 magazines a stack | the same, by count; the whole limit for empty spares | stock's credit adds to it | `launder-and-fill.md`, "One spare magazine per stack": 1 against 4, sh8bench +3.5% on two L3s, mpmc peak 5.61 against 6.07 MiB |
| even split (`spare_limit`) | `depot/spare.rs` | `SPARE_BUDGET` / 44 per class | fixed full spares per class | fixed; stock covers it under contention | `launder-and-fill.md`, "What it costs": with 1,022-object magazines, 24 to 128 B keep none |
| stock room (`room`) | `depot/stock.rs` | budget less fixed shares, about 885 to 992 KiB | spares past the fixed limit | credit +1 per contended round trip; low-water mark off per interval | as above |

### Depot

| Constant | Where | Value | Sizes | Adapts now | Research |
|---|---|---|---|---|---|
| `DEPOT_TARGET` | `depot/mod.rs` | 128 KiB of objects per class | domain cap plus shared target: depot / bitmaps | shrinks only (shed by low water); laundered over target | `memory.md`, "Magazine budget" (set at twice the budget); `launder-and-fill.md`, "sh8bench, accounted": 64 MiB took 12.8 to 12.3 G cycles |
| `DOMAIN_CAP` | `depot/mod.rs` | a quarter of the target, 32 KiB | domain stack / shared stack | shed; at least one magazine | `launder-and-fill.md`, "Domain stacks and a shared stack": half was no faster and parked up to 0.7 MiB more. Now under one magazine for classes of 40 B and up |
| shared target | `depot/mod.rs` `shared_target` | the rest, at least a magazine | shared stack / bitmaps | heartbeat launders over it; shed | as above |
| empty caps | `depot/mod.rs` `empty_cap` | the domain cap in magazines | empty magazines | derived | none |

### Laundering, pacing and the interval

| Constant | Where | Value | What it is | Adapts now | Research |
|---|---|---|---|---|---|
| `REFRESH_EVERY` | `depot/slow.rs` | 1 in 32 | rate of replacing circulating objects | fixed | `memory.md`, "The parked-object rules, built" (1 in 32 vs 8); `baselines.md` section 10: moves no number it is meant to |
| `LAUNDER_BUDGET` | `depot/mod.rs` | 1,022 objects | worst call | fixed bound | `launder-and-fill.md`, "Bounded work per call" |
| `FILL_CLAIMS` | `back/policy.rs` | 256 bitmap words | worst call | fixed bound | the same |
| `IDLE_TICKS` | `depot/heartbeat.rs` | 2^32 ticks, about 1 s | the interval: idle drain, age limit, sweep, shed, stock disuse | fixed | `heartbeat.rs` comment; 0.7 to 4.3 s by machine |
| `DECAY_TICKS` | `back/policy.rs` | 2^32 ticks | purge pass period | fixed | `memory.md`, "After the peak" |
| `LOOK_EVERY` | `depot/heartbeat.rs` | 16 slow paths | pacing of looks at other slabs and stacks | fixed | none |
| `LONE_EVERY` | `depot/heartbeat.rs` | 16 large calls | heartbeat on large calls | fixed | none |
| `MAINTAIN_EVERY` | `heap/global.rs` | 500 ms | background thread period | fixed | `memory.md`, "The background thread" |

### Back end: retained memory and huge pages

| Constant | Where | Value | Sizes | Adapts now | Research |
|---|---|---|---|---|---|
| `RETAINED_CAP_HUGE_PAGES` | `back/policy.rs` | 32, 64 MiB | free resident memory: bitmaps / OS | a bound charged to growth | `memory.md`, "The cap charged to growth" |
| `DECAY_SHIFT` | `back/policy.rs` | a quarter a pass | rate of return | fixed | `memory.md`, "After the peak": "The fraction and the interval are two constants" |
| `DECAY_FLOOR_HUGE_PAGES` | `back/policy.rs` | 2 MiB | tail of the decay | fixed | none |
| the frontier window | `back/layout.rs` `Frontier` | this interval and the last | pages fills reached | evidence (fills) | `memory.md`, "The clocked pass spares what fills reach" |
| `GROWTH_STEP_HUGE_PAGES`, `PURGE_STEP_WORDS`, `PURGE_CAP_WORDS` | `back/policy.rs` | 2 MiB; 256; 16,384 words | work per step | fixed bounds | `memory.md`, "The cap charged to growth" |
| `COMMIT_AHEAD_PAGES`, `_HUGE_PAGES`; magazine `COMMIT_AHEAD` | `back/policy.rs`, `depot/magazine.rs` | 64 KiB; 2 MiB; 64 KiB | `mprotect` amortization | fixed | `launder-and-fill.md`, "One L3": 16 MiB at a time changed nothing |
| `HUGE_AFTER_HUGE_PAGES` | `back/policy.rs` | 8, 16 MiB carved | when a class switches to huge pages | one-time, on evidence | `memory.md`, "The threshold": 4 MiB the knee |
| `HUGE_MIN_SLOTS_PER_PAGE` | `back/policy.rs` | 4 (classes to 1 KiB) | which classes may | fixed | `memory.md`, section 5 item 3 |
| `GROWN_HUGE_MIN_HUGE_PAGES`, `GROWN_HUGE_MAX_FACTOR`, density half | `back/policy.rs` | 2 MiB; doubling; half | huge pages for growing blocks | one-time, on evidence | `memory.md`, "Faults against memory"; `baselines.md` section 10 |

### Large objects

| Constant | Where | Value | Sizes | Adapts now | Research |
|---|---|---|---|---|---|
| `KEPT_GROWN_HUGE_PAGES` | `back/policy.rs` | 8, 16 MiB | freed grown slots: kept / OS | fixed budget | `memory.md`, "Grown blocks kept up to a budget" (8, 16, 32 MiB) |
| `HEADROOM`, `FALLBACK_HEADROOM`, `HEADROOM_SHARE`, shrink at twice the headroom | `heap/large.rs` | 16; 4; a sixteenth of a region | address space for growth / copies | fixed | `memory.md`, "Headroom for growing blocks", "Headroom only while it is a small part of a region" |
| `MOVE_PIECE_HUGE_PAGES`, `WINDOW` | `back/policy.rs`, `back/class.rs` | 2 MiB; 512 pages | both copies resident during a move | fixed bound | `memory.md`, "A move returns the old block at any size" |

Not buffers, and out of scope: the stagger (`STAGGER_STEP`, `STAGGER_SLOTS`), class origins
(`ORIGIN_SPAN_PAGES`, `ORIGIN_STEP`, `ORIGIN_SHARE`), the launder's grouping (`SCATTERED`,
`JUDGE`, `WIDE`, `DEBTS`, `UPWARD`, `PLACES`, `EARLY`), `EXACT_TRIES`, start-up's `PATIENCE` and
`SPINS`, `MAX_DOMAINS`, `MIN_LOG2`.

## Appendix B: prior art in detail

### tcmalloc (local, f3c6669a)

All resizing runs in one background loop, `MallocExtension_Internal_ProcessBackgroundActions`
(`background.cc`), every second by default; nothing adapts its periods.

- **Per-CPU capacity** (`cpu_cache.h`). Misses are counted per CPU with relaxed stores
  (`RecordCacheMissStat`), one snapshot per consumer. On a miss, `UpdateCapacity` grows a class
  by one below two batches (batch 32 objects up to about 2 KiB), and past that by a batch only
  when an overflow follows an underflow (`PerClassResizeInfo::Update`). Bytes come from the
  CPU's budget, 1.5 MiB (`kMaxCpuCacheSize`). Refills ramp by `batch << successive`, up to half
  the capacity. Shrinking is by stealing one slot at a time once a class's quiescent ticks
  reach a size-dependent score, "based solely on intuition, common sense and benchmarking".
  Every 2 s `ResizeSizeClasses` grows the 5 most-missing classes of up to 10 CPUs at the
  others' expense, with the CPU stopped. Every 5 s `ShuffleCpuCaches` lets the 5 most-missing
  CPUs steal 5% of the limit from CPUs with under 80% of their misses and over 20% of the limit.
  Every 30 s `TryDrainingCaches` empties a CPU unchanged and missless since the last look. Every
  29 s the per-class maximums and the slab size are resized, which stops and drains every CPU;
  the slab grows when overflows exceed 0.9 of underflows and shrinks below 0.4, and its resize
  "erases any learned information".
- **Transfer cache** (`transfer_cache_internals.h`): a locked array, 16 batches growing to 64
  (and 1 MiB / size); every 2 s the most-missing classes grow by a batch paid by the least, a
  zero-sum; every 5 s `TryPlunder` returns the low-water mark of the period. The per-L3 sharded
  version is an experiment at 1%.
- **HugeCache** (`huge_cache.cc`): on a miss the limit rises to 1.1 times the demand dip over
  1 s; it falls by half the cache's 2 s minimum only when that minimum is over a fifth of the
  limit; floor 10 huge pages. The asymmetry is deliberate (`huge_cache.h`). The dip cannot order
  events and overestimates by design.
- **Skip-subrelease** (`huge_page_filler.h`): keep `min(peak over 10 min, short-term range over
  10 s + long-term minimum over 120 s)`; a tracker scores skipped releases but does not feed
  back. ISMM 2021: about 1% fleet throughput.
- **Release rate**: `background_release_rate` defaults to 0 in this checkout.
- **Papers.** OSDI 2021 (Temeraire): teams had turned periodic release off to keep huge pages;
  the huge-page-aware design raised coverage and cut overhead (15.0 to 11.2% in the A/B).
  ASPLOS 2024: per-CPU miss share is skewed to low CPU numbers, which motivated the stealing;
  the default cache halved with no loss and fleet memory fell 1.94%; all changes together about
  1.4% throughput and 3.4% memory over two years.
- **Failure modes**: a well-sized list looks idle and is shrunk; capacity-1 classes thrash;
  stop-the-world steps must stay rare; the idle drain can fire on a busy CPU whose LIFO churn
  never misses; several comments are stale against the code.

### jemalloc (`dev` 975f3b0, and 5.3.0)

- **tcache, `dev`** (`tcache_ncached_target.h`, commit "Adapt tcache fill and retention",
  2026): capacity fixed at 2 x slab objects in 20 to 200 (large bins 20). A refill of an empty
  bin doubles the fill, to half the capacity; a GC visit where the bin never emptied halves it,
  to 2. After GC a bin keeps `used + max(1, used/4)` where `used = ncached - low_water`, so an
  idle bin drops to 2 at once. Overflow halves retention. GC is an event every 64 KiB of the
  thread's allocation or free, at most every 10 ms. It replaced six options to simplify the
  configuration.
- **tcache, 5.3**: `lg_fill_div` raised at a visit with low water above zero (flushing three
  quarters of it), lowered after an underflow; one bin per 64 KiB event.
- **Decay** (`decay.c`, `pac.c`): pages dirtied in each of 200 epochs of `dirty_decay_ms` (10
  s) are allowed to stay with a smootherstep weight; jitter keeps arenas out of lockstep.
  Without background threads the clock is read every thousand or so arena events; with them,
  the thread sleeps until 1,024 pages would be purged, at least 100 ms. It replaced ratio-based
  purging. Fails on phases longer than the window; an idle process never purges.
- **HPA**: experimental and off. A peak-demand window modelled on tcmalloc was added in 2025
  and reverted three months later without a stated reason.
- **Failure modes**: an idle thread keeps its cache; a consumer thread converges to flushing
  nearly the whole bin, under the bin lock, at every overflow.

### Bonwick and Adams 2001 (illumos `kmem.c`)

- **Magazine resize**: every 15 s, if the depot lock's failed `tryenter`s rose by more than 3
  (`kmem_depot_contention`), `kmem_cache_magazine_resize` purges every CPU's magazines and the
  depot and steps to the next row of `kmem_magtype[]` (1, 3, 7, 15, 31, 47, 63, 95, 143
  rounds), while the object is under the row's limit. Grows only; only allocation contention
  is counted; each resize discards every cached object.
- **Depot working set**: each list keeps its minimum; every 15 s the old minimum becomes the
  reap limit, and a reap frees what was unused for the whole previous interval. It runs only
  under memory pressure, so idle depot memory is never returned otherwise.

### FreeBSD UMA (`uma_core.c`)

- **Bucket size**: from a table by item size (up to 254 entries); `+1` each time
  `zone_domain_lock` finds the lock held (2013: "only grow the bucket size when we face
  contention ... grow more slowly"); `-1` on trim, halfway to the minimum on severe shortage.
- **Working set** (`zone_domain_update_wss`, every 20 s, "constants ... somewhat arbitrary"):
  `wss = max(3/4 wss, imax - bimin)`, the largest allocation batch; a long-term minimum by a
  1/256 moving average. Trim under pressure to the working set; after 15 minutes with no
  near-miss, release a quarter of the long-idle surplus every 20 s. Added because repeated
  low-memory events squeezed real consumers (the ZFS ARC halved) into memory stuck in UMA.
  Per-CPU buckets are outside this.

### Linux (`mm/page_alloc.c`, `mm/slub.c`)

- **Per-CPU page lists** (Huang Ying, 2023): `high` from `high_min` (the low watermark per CPU)
  to `high_max` (managed pages / 8 per CPU). Each refill adds a batch (`nr_pcp_alloc`); the
  refill grows by `batch << alloc_factor` on consecutive refills, to `CONFIG_PCP_BATCH_SCALE_MAX`
  = 5, chosen from a 100 us latency budget; every free halves the factor. `decay_pcp_high`
  takes an eighth a second ("kind of arbitrary"). Below the high watermark, growth stops and
  frees lower `high`. Kbuild: 3.5 to 5% faster, zone-lock contention from 11 to 14% down to
  0.5%, up to 18.4% more memory. Failures: a CPU that only allocates grows to the maximum; the
  fix for it made high-order frees fire too eagerly (iperf3 -30% at 64 pairs, fixed in 2025);
  decay rides on the vmstat worker and stopped on NOHZ-full CPUs.
- **Sheaves** (2025, default in 7.0): capacity by object size (4, 12, 28 or 60), a main and a
  spare per CPU, a barn of 10 full and 10 empty per node. Nothing adapts; no timed drain.

### Others

- **mimalloc 3.5**: all fixed. Retire an emptied page for 16 collect cycles if its queue has
  at most 3; purge 4 s after the first range is scheduled in an arena (one expiry per arena, so
  a range freed just before it is purged at once). v2 extended a segment's purge by 1 ms per
  free; no doubling anywhere.
- **snmalloc**: main is static (16 KiB remote cache, 1 MiB per queue drain, a 2 MiB per-thread
  buddy cache decommitted on leaving). An unmerged branch (2026) has a decay range of 4 epochs of
  500 ms and a large-object cache whose budget grows by the excess of misses over stale flushes
  and shrinks by half the excess of flushes: the closest existing model of the rule we want.
- **glibc 2.43**: the mmap threshold rises to the size of any mmapped chunk freed (to 32 MiB),
  and the trim threshold to twice it; it never falls, and any `mallopt` freezes it. tcache
  count is 16 (the manual still says 7), static.
- **scudo**: per-class cache `2 x min(14, 1 KiB / size)` (Android 13, 8 KiB), static. Release
  every 5 s, attempted only after a free drains the thread cache; the release threshold adapts
  (halves when a release frees more than the last, grows by half the pending delta).
- **rpmalloc**: 1.3 had an adaptive thread cache keyed to a high-water mark that never decayed;
  1.4 turned it off by default in favour of fixed limits.

### Managed runtimes

- **Go pacer**: `goal = marked + (marked + stacks + globals) * GOGC / 100`. The 2021 redesign
  modelled GC as a split of CPU with a stable ratio of allocation to scan rate; the PI controller
  it proposed was not shipped, in favour of a filtered direct estimate. Leftovers: trigger
  clamps at 0.7 and 0.95 of the runway, a 25% background target, credit slack. GOGC works
  because GC CPU per byte allocated is about scan work over headroom, and headroom is GOGC times
  live: it is an exchange rate users can reason about.
- **Go memory limit**: added because a ratio has no absolute bound (OOMs, heap ballasts); 3%
  headroom, and a leaky bucket that caps GC at 50% of CPU so the limit cannot spiral.
- **Go scavenger** (`mgcscavenge.go`): retain the heap goal plus 10% ("each allocation out of
  scavenged memory incurs a potentially expensive page fault"), or 95% of the memory limit; work
  at 1% of CPU, paced by a PI controller on its own CPU time, the one place Go kept one; skip
  chunks that were dense in the current or last GC cycle (a working set of one cycle).
- **JVM**: Parallel GC's goals in order (pause, then `GCTimeRatio` 99, then footprint) with
  growth of 20% and shrink of 5%; the footprint goal is rarely reached and heaps drift to the
  maximum. G1 resizing at every GC (JDK-8238687, 2025) doubled `GCTimeRatio` from 12 to 24
  because the old value shrank heaps too far once shrinking was real. ZGC's adaptive sizing (JEP
  546) targets GC CPU, rejects a residency knob because CPU cost varies widely at a residency,
  and leaves its mapping unspecified.
- **TMO / Senpai** (Meta, ASPLOS 2022): reclaim `current * 0.0005 * max(0, 1 - PSI /
  0.1%)` every 6 s, so shrinking takes minutes and growth is immediate; one setting for all
  services, "not very sensitive"; 20 to 32% memory saved.

## Appendix C: sources

Local: `third_party/tcmalloc/tcmalloc/{cpu_cache.h, background.cc, parameters.cc,
transfer_cache_internals.h, huge_cache.cc, huge_page_filler.h, huge_page_subrelease.h,
internal/percpu_tcmalloc.h}`; `third_party/mimalloc/src/{page.c, arena.c, free.c}`;
`third_party/go/src/runtime/{mgcpacer.go, mgcscavenge.go, mgclimit.go}`.

Fetched: jemalloc `dev` (`tcache.c`, `tcache_ncached_target.h`, `decay.c`, `pac.c`, `hpa.c`);
illumos `uts/common/os/kmem.c`; FreeBSD `sys/vm/uma_core.c`; Linux `mm/page_alloc.c`,
`mm/slub.c` and commits 90b41691b988, 51a755c56dc0, 52166607ecc9, 6ccdcb6d3a74, c544a952ba61,
2d517aa09bbc; snmalloc main and `generalisedbackend`; glibc 2.43 `malloc/malloc.c`; LLVM scudo
`standalone/`; rpmalloc 1.3.2, 1.4.5.

Papers and documents: Hunter et al., "Beyond malloc efficiency to fleet efficiency", OSDI 2021;
Zhou et al., "Characterizing a memory allocator at warehouse scale", ASPLOS 2024; Maas et al.,
"Adaptive huge-page subrelease", ISMM 2021; Maas et al., LLAMA, ASPLOS 2020; Bonwick and Adams,
"Magazines and vmem", USENIX 2001; Weiner et al., TMO, ASPLOS 2022; Go design documents 44167
(pacer) and 48409 (memory limit); JEP 546, draft JEP 8359211, JDK-8238687; Karlin et al.,
"Competitive snoopy caching", Algorithmica 1988; Megiddo and Modha, ARC, FAST 2003; Denning,
"The working set model", CACM 1968; Chiu and Jain 1989.
