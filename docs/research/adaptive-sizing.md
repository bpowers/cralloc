# Sizing buffers on evidence: our constants, prior art, and what carries over

2026-10-19, at `4cefa63` (magazines of up to 1,024 slots). Reading and arithmetic only: no
code changed and nothing was timed. Prior art was read from the sources in `third_party/`
(tcmalloc f3c6669a, mimalloc 3.5.3, Go) and from fresh checkouts and papers (jemalloc `dev`
975f3b0, illumos `kmem.c`, FreeBSD `uma_core.c`, Linux 7.2.9 and master, snmalloc, glibc
2.43, scudo, rpmalloc). Appendix A lists our constants, Appendix B the prior art in detail,
Appendix C the sources. Reviewed the same day by an agent that did not write it ("Review",
after the summary): statements about other systems that the review found wrong or imprecise
are corrected in place, and passages whose reasoning it disputes are marked "Disputed".

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
   adjustable capacity with an index and a capacity packed into one word, an instruction or two
   on every push; Bonwick, UMA and Linux sheaves change a size for the whole cache under a lock.
   It needs the routines' precondition restated and its checkpoint cost measured ("Segmented
   magazines", below). *[Disputed: as described the checkpoint is unsound, and it needs new
   routines; "Review", findings 1, 4 and 5.]*
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
   how long idle memory is kept. *[Disputed: the assumed rate, and what follows from "it cannot
   bound a peak"; "Review", "The theory".]*
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
*[Disputed: the signal and the list; "Review", findings 2 and 3 and "The ranking".]*

## Review, 2026-10-19

An adversarial review by an agent that did not write the note, made the same day at `d6d79a6`.
Reading and reasoning only: nothing was built, run or timed. The statements about other
systems were checked against tcmalloc f3c6669a and Go in `third_party/` and against fresh
copies of jemalloc `dev` 975f3b0 and 5.3.0, illumos `kmem.c`, FreeBSD `uma_core.c`, Linux 7.0,
7.2 and master with the commit messages, and the Bonwick and Adams paper. The proposals were
checked against `docs/design.md`, `lean/Asmalloc/Spec/Slab.lean`, `classes/magazine.rs`,
`depot/{slow, heartbeat, stock, spare, stack, magazine}.rs`, `launder-and-fill.md` and
`memory.md`. "Verified" means read in the source; "inferred" is the reviewer's reasoning.
Statements of fact found wrong or imprecise are corrected in place and listed under "What was
checked"; the note's arguments stand as written, marked "Disputed" where a finding contests
them.

### Findings that change the design

1. **The segment checkpoint, as described, can hand one object out twice.** (Inferred from the
   code; sure.) It writes or reads a slot of the loaded magazine from the slow path, outside a
   sequence, and then moves the word with store-if-equal. That races with other threads on the
   same CPU.
   - Free: A reads the word at slot `km - 1` and is preempted. B crosses the same boundary and
     commits. A writes its object over B's in slot `km + 1`, fails the compare, retries and
     pushes again. A's object is cached twice and B's is lost.
   - Allocation: A reads the object in slot `km - 2` and is preempted. B pops it, frees two
     objects and pops one, so the word is `km + 1` again. A's compare succeeds and returns an
     object B holds.

   Every slow path today reads a magazine's contents only after it owns the magazine
   (`depot/slow.rs`: an install displaces a whole magazine, and `serve_free`, `fill` and the
   launders act on held ones), so store-if-equal has never had to guard contents. The
   checkpoint needs two new routines, a push and a pop across the gap inside a sequence: new
   Lean work, not reuse. The other way round, the pop and push proofs need no redoing: the
   specification states its invariant on the window of the mask (`SlotOk`, `Exclusive` and
   `Mag.ofWord` in `lean/Asmalloc/Spec/Slab.lean`, all in terms of `slotsOf mask`), and a
   segment inside a larger magazine satisfies it as it stands. The work moves to the Rust slow
   paths, the twins, Kani and TLA+.
2. **The proposed signal does not fire where the magazine's size pays.** (Verified from our own
   measurements.) The note has a checkpoint go on only while exchanges "round-trip under
   contention". Stock already uses that signal, and it left the pipelines, the tree and JSON
   level (`launder-and-fill.md`, "Neighbours"). Magazines of 1,024 slots moved those jobs by 4
   to 20% (the same note, "Time and memory"), and magazines of 128 slots cost the pipelines 16
   to 20% (`memory.md`, "What is left, and proposals"). A pipeline is a one-way flow on each of
   its CPUs, and the tree and JSON never contend. So rule 2 of "What to adapt first" keeps
   sh8bench and returns the pipelines to main's time as well as main's peak; its "Expected"
   line states only the peak. The cause: our magazine is the transfer batch as well as a
   buffer. Round trips size a buffer. The rate of flow sizes a batch, which is what the note's
   own order-quantity formula says.
3. **The prior art does not teach "a round trip, not a miss".** (Verified.)
   - Linux grows `high` on every refill from the zone, and still does in 7.2 (`nr_pcp_alloc`:
     "If we had larger pcp->high, we could avoid to allocate from zone"). Its author saw the
     case of a CPU that only allocates, judged that it "isn't a severe issue, because there are
     no idle pages in this case", and rejected growing on the free side instead, because idle
     pages would then be hard to free (commit 51a755c56dc0). 6ccdcb6d3a74 is the last patch of
     the same series and detects runs of high-order frees; the 30% regression was that
     detector firing early.
   - tcmalloc's sentence is quoted rightly (`UpdateCapacity`) but covers only capacity above
     two batches. Below that any miss grows a class, and the number of objects moved doubles
     on successive misses in one direction, to half the capacity
     (`TargetOverflowRefillCount`).
   - jemalloc `dev` doubles the fill on every refill of an empty bin
     (`tcache_ncached_fill_after_refill`) and never adapts capacity. Bonwick and UMA count
     contention in either direction, and `kmem.c` describes the magazine's size as "a higher
     transfer rate with the depot".

   Three systems adapt the size of a transfer apart from the capacity, on runs in one
   direction. The note records each and draws nothing from them.
4. **The checkpoint's cost and its heartbeat cannot both be as stated.** (Numbers verified;
   conclusion inferred.) The estimate of 30 to 100 cycles assumes no heartbeat; "restores the
   heartbeat's frequency" assumes one. sh8bench's exchange carries 400 cycles of glue,
   heartbeat included (`launder-and-fill.md`, "One L3"), and reading the clock on every
   heartbeat cost a two-thread pipeline 3% (`depot/heartbeat.rs`, `Clock`). From the pipeline
   rows (3.77 ns an allocation, 9.8% faster with a slow path every 1,022 calls instead of every
   254, for the allocation and for its free, at 4.3 to 5.5 GHz), an uncontended slow path
   there costs about 300 cycles all in. A checkpoint must be far under that to keep the gain.
5. **Other costs of segments.** (Inferred.)
   - Reach. Only classes up to 128 bytes have magazines of more than 256 slots. From 160 bytes
     up the 64 KiB byte budget stays a constant, and halving it is what took the mixed
     pipeline from 7.07 to 5.12 MiB.
   - A fill can already stop at any level (`Cache::fill`), so the allocating half of an
     adaptive capacity needs no segments.
   - Each interior boundary gives one level two words (the top of one segment, the bottom of
     the next), and an install must choose by the direction of the request. Fills and launders
     take one contiguous slice of slots today.
   - The byte limits of spares, stock and the depot count full magazines (`spare_limit`,
     `stock::weight`, the depot's caps) and overcount part-full ones by up to four.
   - A rival to measure against: magazines of 256 slots, adaptive spares, and a depot push and
     pop that move four linked magazines with one compare-and-swap. It has the same slow paths
     and the same atomic instructions per object, no new assembly and no empty slots, and it
     makes items 1 and 2 of "What to adapt first" one mechanism. What separates the two is
     whether the gain of 1,024 slots came from fewer entries to the slow path or from less
     traffic at the depot; that has not been measured.

### The theory

The arithmetic checks: 400 and 85 cycles per KiB-second, 32,000 and 1,400 objects, 2 s. The
inputs do not. (Inferred, from numbers verified in our notes.)

- The assumed rate is a million objects a second. The pipelines run near 2.6 x 10^8 (3.77 ns an
  allocation) and sh8bench over 10^8 on each CPU. At 10^8 the formula gives 14,000 objects at
  the price of a page and 320,000 at the object's own bytes. "1,022 sits there" comes from the
  assumed rate.
- The purge holds an idle page nearer 5 s than 1.5 (two intervals, then a quarter a pass),
  which puts the price near 120.
- At 400 cycles per KiB-second, 1 MiB costs a hundredth of a percent of a core. We reject 1
  MiB on a 2 MiB heap for a 10% gain. No price near the purge's explains our choices: the
  memory goal is a ratio to jemalloc.
- Ski rental's time falls with the class size, since a magazine's refill costs by the object
  and its bytes are the object's: about 16 s at 8 bytes, 2 s at 64, an eighth of a second at 1
  KiB. "The interval is roughly what ski rental would give" holds at one size.

That a price cannot bound a peak is right. "So the knob should be a budget" does not follow as
stated, because today's constants are budgets already. The cost we object to is relative: 0.5
to 3 MiB on heaps of 2 to 8 MiB, under 2% on every heap over 10 MiB (`launder-and-fill.md`,
"What it costs"). What follows is a budget in proportion to live memory, with a floor, which
the note mentions once as a possibility. Dividing a budget by regret is itself a price, the
one at which demand meets the budget, so the choice is not between the two. Of the knobs cited
in support, the JVM's time ratio is the counterexample: it bounds time, lets space float, and
drifts. GOGC is a proportion with a floor of 4 MiB (`mgcpacer.go`, `defaultHeapMinimum`).

### The ranking

- Item 1 removes the guaranteed spare that keeps an uncontended thread at a magazine's boundary
  from reaching the depot on every second call. Classes of 24 to 128 bytes already lack it
  (`launder-and-fill.md`, "What it costs"). Keep a floor of one full and one empty spare for a
  class in use.
- The capacity has no rule for shrinking. A signal that vanishes once the capacity suffices
  makes it saw once an interval. Checkpoints offer a better measure: the crossings counted at
  each boundary are the misses each smaller capacity would have had.
- Item 3 has no bound: phases of allocation and of freeing longer than an interval raise the
  target toward the whole swing.
- Wrong to leave constant: pacing counted in slow paths, which becomes a function of the
  adapted capacity; the retained cap, an absolute 64 MiB; the magazine's byte budget above 128
  bytes. A measurement made after the note, not the reviewer's, looks like a first case of the
  pacing: burst then idle held 90.6, 30.2 and 4.9 MiB at 2, 6 and 12 s with 1,024 slots against
  68.0, 21.3 and 1.3 with 254, at level peaks. Its cause was open when this was written.

### What was checked

- **Right, verified.** tcmalloc's resizing rules, periods, thresholds and 1.5 MiB; jemalloc
  `dev`'s fill and retention rules, 5.3's, and decay; illumos's table, its 3 failed `tryenter`s
  in 15 s, and that it only grows; UMA's bucket growth and working set; Linux's bounds, batch
  scaling and decay; sheaves; Go's constants; that G1's default doubled.
- **Corrected in place.** jemalloc removed eight options, not six, and 5.3 lowers its divisor
  at the bin's next visit, not straight after an underflow; its HPA is off by default, with no
  "experimental" label found. Linux: when a refill and a free raise `high`, which commit did
  what, and that the decay does not run on isolated CPUs (it never did; nothing stopped).
  tcmalloc's capacity shares the index's word, so it costs no load. tcmalloc's background
  resize grows the classes most often refused room, not the most-missing. illumos reaps on a
  few calls besides memory pressure. UMA's bucket size drops by one on every reclaim. A sheaf's
  capacity of 28 is 26 rounded up. The G1 default changed under two issue numbers.
- **Not checked.** The ASPLOS 2024 and OSDI 2021 numbers, LLAMA, snmalloc's branch, scudo,
  rpmalloc, glibc, mimalloc, TMO, ZGC, and whether segmented magazines are new.
- **Missing.** Cliffhanger (Cidon et al., NSDI 2016) divides a fixed memory budget among a
  cache's queues: a miss that hits a queue's shadow queue gives that queue a fixed credit taken
  from another queue chosen at random, which climbs the hit-rate curves by their local slopes
  with no curve ever built. It is the worked-out form of "a budget divided by regret". Checked
  against a published summary of the paper, not the paper's own text.

### The reviewer's rule, and the objection to it

Parked memory may be a set fraction of live memory, with a floor. A buffer grows while its
misses in the last interval cost more than its bytes at the going price, and returns what it
did not use; the price rises while the total is over.

The strongest objection: the 10 to 20% lives on heaps of 2 to 8 MiB, where a budget in
proportion is smallest. The rule then mostly yields small magazines on small heaps and large
ones on large heaps, which a checkpoint that asks only "is parked memory under the fraction?"
gives with no accounting of regret. Build that first, and add the division by regret only if a
measurement asks for it.

### Moving several magazines per compare-and-swap

What the rival of finding 5 asks of the depot stack (`depot/stack.rs`). (Inferred; not built.)
Today the head is the top magazine's number and a version that goes up once for each push and
each pop. Slot 0 of a magazine on the stack holds the number below it and the stack's count
through it. A pop follows only the head's number, and a compare-and-swap that succeeds means no
push or pop completed since the head was read, so the top's link is the one its pusher wrote.
There are two ways to move four at once, and the first is the one to take.

- **A bundle as one stack element.** Link the four through a second word and push the first as
  one element, weighted by the bundle's objects. The stack's protocol and its ABA argument do
  not change: a pop still reads one link. The second link has a home already. The last slot
  records the level in its low half and a spare stack's depth in its high half
  (`depot/magazine.rs`, `Record`), and the depth is unused while a magazine is off a spare
  stack (`set_level` writes it as zero). The link is written while the thread holds the bundle,
  never changes on the stack, and is read only after the pop, which is the rule finding 1
  turned on: contents are read after ownership. What it does need: caps and targets stated in
  bundles (a 64-byte class's domain cap is two 254-object magazines today, less than one
  bundle), and the rules that take one magazine (the shed, the launder over target) must pop a
  bundle, take one and push the rest back.
- **A chain of the top `k` through slot 0.** The ABA argument is the same in kind: one step of
  the version for each operation, and success still means every link walked was its pusher's.
  It adds two obligations. The pop now follows `k - 1` links before it owns anything, and a
  magazine another thread has taken may hold a spare stack's address in slot 0, so each number
  must pass `Area::by_id` before it is followed; a failure there means the head is stale and
  the pop tries again, never the abort that today's pop keeps for a head that names no
  magazine. And links carry cumulative counts, so a chain push must rewrite all `k` links on
  each try, which is harmless because they are still private.

### What matters for interactive servers

The owner's priority is interactive servers on tokio: requests of mixed sizes, tasks that move
between workers, bursts and idle gaps, heaps of hundreds of MiB. Which findings matter there.
(Inferred. None of it is measured at that heap size: the suite's RPC jobs have heaps of 6 to
14 MiB.)

- **The return side matters most** ("The ranking": pacing counted in slow paths, and the
  absolute retained cap). In an idle gap every worker parks, so there is no slow path, no
  heartbeat, and nothing goes back until traffic resumes (`docs/design.md`, "Heartbeat").
  tokio's `Builder::on_thread_park` is the idle hook that section mentions, provided `maintain`
  limits its own rate. After a burst, a heap of hundreds of MiB is over the 64 MiB cap
  (`back/policy.rs`, `RETAINED_CAP_HUGE_PAGES`), so the next burst's growth always pays for
  purging. A page is held about 5 s, so bursts a few seconds to tens of seconds apart fault
  back what was returned: 400 MiB is about 58 ms of small-page faults a burst. This is the one
  place where the note's price is the right tool, and the note defers it ("if a server shows
  refaults"). For this workload it belongs first.
- **Reach matters next** (finding 5). Mixed sizes live in classes above 128 bytes, where
  neither segments nor the rival adapts anything. A loaded magazine there holds up to 64 KiB
  of objects for each class and CPU: up to 2.8 MiB a CPU if every class is warm, about 90 MiB
  on 32 workers, which is not noise even at hundreds of MiB. The cheap lever is a fill that
  starts small and doubles, per class and CPU, which needs no new mechanism.
- **The capacity's signal and the small-heap memory bar stop mattering** (findings 2 and 3,
  "The theory"). Tasks that move between workers give each CPU both allocations and frees of a
  class, so the loaded magazine absorbs most of the traffic; the RPC jobs are level within 1%
  at 254 slots, at 1,024 and with stock (`launder-and-fill.md`, "Neighbours" and "Time and
  memory"). At hundreds of MiB the 0.5 to 3 MiB is under 1%, so segments against bundles is a
  question of throughput only, and a small one.
- Finding 1 matters only if segments are built. The work bounds matter as the note says: a
  server's tail latency is the worst single call.

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
| FreeBSD UMA | bucket size | +1 each time the zone lock is found held | -1 on each reclaim, halfway to the minimum when the per-CPU caches are drained | 254 |
| Linux per-CPU pages | `high` | +a scaled batch on each refill from the zone; a run of frees raises it to hold the run | -1/8 a second at most | `high_min` to managed/8 per CPU; memory pressure overrides |
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
  capacity". On Linux's per-CPU lists a CPU that only allocates grows `high` to its maximum,
  which its author saw and judged harmless; the series' last patch counted consecutive frees
  to detect runs of high-order frees sooner, and a 2025 fix raised its threshold after a 30%
  regression. Our stock already requires a round trip. *[Disputed: Linux grows on a plain
  miss by design, and tcmalloc's rule covers only capacity past two batches; "Review",
  finding 3.]*
- **Contention is the cost signal everyone uses for a shared tier**, because it is free to
  observe: Bonwick and UMA on their locks, our stock on a failed compare-and-swap.
- **One-way ratchets fail.** glibc's mmap threshold and rpmalloc 1.3's thread cache only
  grow; rpmalloc dropped its rule in 1.4, and Bonwick's code notes that shrinking "might be
  useful".
- **Learned models did not survive; static proxies and simple feedback did.** LLAMA's learned
  lifetimes (ASPLOS 2020) became a fixed span-capacity threshold in tcmalloc (ASPLOS 2024:
  +1.0% throughput, -0.8% memory). jemalloc's `dev` branch just replaced eight tcache options
  with doubling and halving rules. No published allocator autotuner runs in production; Google
  tunes by fleet A/B experiments and Meta by hand per service.
- **One knob works when it is a ratio with a meaning, measured costs fill in the model, and a
  hard cap stands behind it.** GOGC is a heap ratio, and the pacer measures allocation and scan
  rates to turn it into CPU; the memory limit was added because a ratio cannot stop a spike,
  and a 50% CPU cap was added so the limit cannot spiral. G1's time ratio, calibrated while the
  heap never shrank, shrank it too far once it could (JDK-8238687, with JDK-8247843, doubled
  the default). ZGC's
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
*[Disputed: the rate of a million objects a second is a hundredth of the measured jobs', and
the interval matches at one class size only; "Review", "The theory".]*

What one rate cannot do is bound the peak. A pipeline that runs for a tenth of a second has
almost no byte-seconds to weigh against its exchanges, so a price says to hold everything,
while our goal is jemalloc's peak. That needs a budget. *[Disputed: what follows is a budget
in proportion to live memory; "Review", "The theory".]*

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
touched and no atomic instruction is executed. *[Disputed: outside a sequence both steps race
with another thread on the same CPU, and store-if-equal does not guard a slot's contents;
"Review", finding 1.]*

So each CPU's magazine of each class has an effective capacity, a multiple of `m`, that the
slow path chooses at every checkpoint: go on, or retire the magazine as it is. The fast path's
instructions do not change; its constant does. What changes elsewhere:

- The routines' precondition says "a magazine of the class's size"; it would say "a magazine of
  a multiple of the mask's size". The Lean proofs bound every access within the `m`-aligned
  window of the word, which lies inside the magazine, so they should carry over, but they must
  be redone. *[Disputed: the specification is already stated on the mask's window, so these
  proofs stand; the new work is the checkpoint's routines; "Review", finding 1.]*
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
  by four. *[Disputed: the estimate leaves the heartbeat out; "Review", finding 4.]*

With `m` = 256 and `n` = 1,024 (or 4,096, where sh8bench, the tree and JSON kept gaining),
the slow path could keep the full size where magazines round-trip under contention and stop at
one segment where a CPU's flow of a class goes one way, which is what `memory.md` ("Small
heaps", "What is left") proposed and could not then build: "a magazine size that adapts per
class and CPU (short on a CPU whose frees of a class go one way, full where it both allocates
and frees)". The physical magazine costs 8 KiB of slots whatever its effective size; the
magazine area is never returned, so that is a fixed cost per magazine made.

Two other ways were considered and are worse. A capacity field the fast path reads, as
tcmalloc packs `current` and `remaining_capacity` into one 32-bit header, costs no load of its
own but an index in place of a pointer and an instruction or two on every push. Changing a
class's mask at run time (the C path reads it from a
table) needs every magazine of the class on every CPU to match it, which takes a drain of every
slab, and the Rust path's mask is an immediate.

## Where the hypothesis breaks

- **A peak is not an integral.** See above. Our memory goal and the cgroup limits programs run
  under are peaks. Keep a space budget (the CPU's spare budget, the depot targets, the retained
  cap), possibly as a fraction of live memory with a floor, as GOGC is; divide it by regret; use
  a time constant only for how long idle memory is kept. *[Disputed in part: "Review", "The
  theory".]*
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
  misses at all. Linux's decay does not run on isolated CPUs for the same reason. Segment
  checkpoints and `maintain` both help.

## What to adapt first

*[Disputed: the signal of item 2, the missing floor in item 1, the missing bound in item 3,
and three entries of the list that follows; "Review", findings 2 and 3 and "The ranking".]*

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

## Segmented magazines, measured (2026-10-19)

A throwaway prototype on branch `segments-proto` (c573841, from d6d79a6), timed against
d6d79a6 unmodified ("base", magazines of up to 1,024 slots). Answers first:

- **The gain of 1,024 slots is depot traffic, not slow-path entries.** Policy A, segments of
  256 slots always going on, enters a slow path as often as 254-slot magazines do but goes to
  the depot as often as 1,024-slot ones; it keeps most of the gain (table below).
- **A checkpoint costs 62 to 67 cycles** (136 instructions, one branch miss), against 262 to
  344 for a local spare swap today without its heartbeat and 365 to 425 with it.
- **m = 256 on 1,024 is not free:** against base it is 1 to 3% slower on the cross-thread
  pipelines, which crosses the stop line we set. **m = 1,024 on 4,096** has as many
  checkpoints as base has exchanges and kept 4,096's gains on the tree, JSON and sh8bench.
- **Policy B never goes on where the size pays:** no checkpoint of the tree, JSON, BTreeMap,
  the two-thread pipelines or mpmc went on. Policy C was built but not timed.
- **The Lean statements carry over unchanged**, and no routine is new.

### How the prototype does it

Every knob is a build-time variable; with none set the code behaves as on base.

- **Geometry.** A magazine of `n` slots gets the routine mask of `m = min(n, M)` slots; slots
  `km - 1` and `km` at each interior boundary hold nothing. Every class with more than `M`
  slots is segmented, so with `M = 256` the 80 to 128-byte classes (512 slots) are too.
  Counts of objects (`capacity`, levels, depot depths, spare limits, `LAUNDER_BUDGET`) count
  objects, so they fell into place once `capacity()` and the level-to-slot maps knew segments.
- **The checkpoint touches no slot outside a sequence.** At a free that finds the word at the
  top of a full segment (slot `km - 1`), the slow path moves the word to the first slot of the
  next (`km + 1`) with store-if-equal and retries the ordinary push; an allocation at `km + 1`
  moves it to `km - 1` and retries the pop. Both words name the same level, so a store-if-equal
  that succeeds after any interleaving only changes how the current state is written; a thread
  preempted between its read and its store either finds the word changed and retries, or
  re-expresses the state it finds. This is sound with any number of threads per CPU, unlike the
  write-then-commit version above (review finding on the checkpoint). It costs one sequence
  more than a fused "cross and pop" routine would, so 62 cycles is an upper bound for that
  routine.
- **It runs before the slow path builds anything.** The first version ran inside the slow path
  and cost 110 cycles and 330 instructions; two thirds of its samples sat on one load, where
  the entry builds the back end's handle on the stack byte by byte and reads it back eight bytes
  at a time, a store-forwarding stall. Every slow path pays that stall today (an aside worth
  fixing on its own).
- **No heartbeat, no clock** at a checkpoint under policy A. The heartbeat runs on exchanges
  only, as on base, so the rules that return memory (age limit, sweep, shed, refresh, idle
  drain, purge steps) are paced as on base: four times rarer for the smallest classes than at
  254 slots. Under policy C a checkpoint reads the clock: about 50 cycles more.
- **Fill and launder.** A fill writes objects straight into their segmented slots (a sink of its
  own in `back::class`). A launder takes slots 1 up to the top object, boundary slots included:
  nothing ever writes them, so they read null, and the back end skips nulls on the branch its
  dense loop already leaves by. A first version compacted the magazine before laundering; its
  division per object cost the tree 6%, a cost of the prototype and not of the idea.
- **Policy state.** B reads the class's stock word (credit of either kind). C keeps a stamp per
  CPU and class, written at checkpoints and installs, in a static of its own: the slab has no
  room below the masks' page offset (1,984 of 2,048 bytes used).

### The cost of a checkpoint

`workload burst`: 1,000 objects of one class allocated, then freed in reverse, 20,000 times,
on CPU 2, seven interleaved rounds. With `m = 256` that is exactly three checkpoints each way
per burst (counting build), and base never leaves its magazine.

| Build | 8 B, cycles/op | 32 B | slow paths per burst |
|---|---:|---:|---|
| base, 1,024 slots | 3.67 | 3.66 | none |
| policy A, m = 256 | 3.86 | 3.85 | 6 checkpoints |
| 254 slots | 4.80 | 4.96 | 6 local spare swaps |
| 254 slots, no heartbeat | 4.46 | 4.70 | 6 local spare swaps |

Per slow path: a checkpoint 62 (8 B) and 67 (32 B) cycles; a local spare swap 365 and 425,
of which the heartbeat is about 100 and 80. A swap without its heartbeat still pays the entry's
stall, the spare stack's pop and push with their depth and stock bookkeeping, and the
store-if-equal: three sequences and a few hundred instructions.

### Policy A against 254 and 1,024 slots

Five interleaved rounds; change against "u256" (the prototype with magazines of up to 256
slots, everything else as base), median of per-round ratios, and cgroup peak MiB.

| Job | u256 | base (1,024) | policy A (256 on 1,024) | peak u256 / base / A |
|---|---:|---:|---:|---|
| pipeline t2 s64 | 3.79 ns | -21.0% | -22.1% | 1.4 / 1.5 / 1.5 |
| pipeline t2 small | 3.79 ns | -9.8% | -8.6% | 2.0 / 2.9 / 3.0 |
| pipeline t2 mixed | 11.2 ns | -4.3% | -3.6% | 4.8 / 5.7 / 5.7 |
| pipeline t2 small, split | 12.0 ns | -9.9% | -12.0% | 2.5 / 3.5 / 3.5 |
| pipeline t2 x2 split / x3 / x4 | 4.54 / 5.36 / 5.44 | -14.4 / -18.1 / -16.5% | -12.4 / -16.2 / -15.1% | 6.1 / 8.4 / 8.3 (x4) |
| pipeline t8 | 4.18 | +0.1% | +0.1% | 8.0 / 10.7 / 10.7 |
| exchange t2 local / split / t8 | 4.16 / 13.8 / 2.64 | -8.1 / -4.8 / -0.9% | -6.5 / -5.0 / -0.6% | |
| mpmc p1c3 / p3c1 / p4c4 | 5.10 / 4.68 / 4.66 | -6.1 / -2.8 / -8.5% | +2.3 / -1.2 / -6.8% | 3.2 / 4.1 / 4.2 (p1c3) |
| tree depth 20, cycles/node | 44.0 | -7.5% | -7.4% | 50.2 all |
| JSON 32 MiB | 17.4 | -6.4% | -3.6% | 350 all |
| sh8bench (preload), s | 0.512 | -25.6% | -18.7% | 128 / 124 / 127 |

The prototype with 1,024 slots and its code changes (the control) ran -5.3% on JSON against
base's -6.4%, and within 1.5 points of base on the pipelines, so differences of two points
between columns are within what code placement moves; mpmc p1c3 ranged from -4.5 to +13.7% for
policy A. The pattern is not: policy A keeps 85 to 100% of the gain on the pipelines and the
tree, 55 to 75% on JSON and sh8bench, and every peak is base's, since the depot sees full
magazines.

Slow-path entries per job, from the counting builds: the tree enters 165,000 times at 254
slots, 41,000 at 1,024, and 41,000 plus 124,000 checkpoints under policy A; the two-thread
pipeline 87,000, 43,000, and 43,000 plus 43,000. On the tree the 124,000 extra exchanges of 254
slots cost about 65M cycles, some 500 each, against 8M for as many checkpoints: what larger
magazines saved is the exchange's work, not the entry.

**What this predicts for the rival** (254-slot magazines, adaptive spares, several magazines per
depot compare-and-swap): its local spare swap has the same shape as a checkpoint, a slow path
with no shared write, so policy A's column is its ceiling. It reaches it only if a local swap
costs about what a checkpoint does. Today's swap costs 260 to 345 cycles before its heartbeat;
at one per 254 operations each side, the 200 cycles over a checkpoint are about 0.8 cycles per
operation, roughly half the 1,024-slot gain on the two-thread pipelines (about 19 cycles per
object each side). The glue is most of it: the heartbeat (80 to 100 cycles), the entry's
store-forwarding stall (about 50), and the spare bookkeeping. A lean swap of three sequences
(stack pop, store-if-equal, stack push) and one branch miss should cost 80 to 100 cycles, which
would bring the rival within a point of policy A. That is an estimate; only the checkpoint and
today's swap were timed.

### Shapes (part 1 and 2)

Five interleaved rounds against base, median change; peak MiB. "u4096" is magazines of up to
4,096 slots unsegmented (8 and 16-byte classes; 24 and 32 bytes get 2,048). At 4,096 the
launder budget is the largest magazine's capacity, so the worst call launders four times what
it does on base; nothing else was changed.

| Job | m = 256 on 1,024 | u4096 | m = 256 on 4,096 | m = 1,024 on 4,096 | peak base / 4,096 |
|---|---:|---:|---:|---:|---|
| pipeline t2 small | +1.2% | -0.8% | +0.1% | -0.8% | 3.0 / 3.5 |
| pipeline t2 x2 split / x3 / x4 | +1.7 / +3.0 / +1.8% | -1.8 / -1.8 / 0.0% | +0.2 / +0.3 / +0.2% | -1.7 / +0.2 / -0.9% | 8.4 / 10.0 (x4) |
| pipeline t8 | 0.0% | -0.4% | -0.1% | -0.3% | 10.7 / 12.0 |
| exchange t2 local | +1.1% | -1.6% | -0.1% | -1.3% | 2.9 / 3.5 |
| mpmc p4c4 | +1.8% | -1.3% | -1.1% | -0.7% | 8.7 / 10.0 to 10.4 |
| tree depth 20 | 0.0% | -4.2% | -1.6% | -3.4% | 50 |
| JSON 32 MiB | +0.9% | -0.2% | +2.0% | -2.7% | 354 |
| BTreeMap / HashMap 1M | +0.5 / +0.8% | -0.1 / -0.3% | +0.2 / +0.6% | -0.6 / -0.3% | |
| churn 1k / 100k / 1M | 0.0 / 0.0 / +1.0% | within 1% | within 1% | within 1% | |
| forkjoin t4 / t8, spawn t4 / t8 | within ±1.1% | within ±1.6% | within ±1.6% | within ±1% | |
| rpc log c64, server cycles/req | -0.7% | -1.3% | +3.1% | 0.0% | 9.4 / 9.8 |
| sh8bench / larson (preload) | -1.6 / +0.7% | -9.8 / +1.2% | -9.8 / +3.2% | -11.3 / +0.7% | 127 / 124 |

Checkpoints per job (counting builds): `m = 256` makes about one per exchange on the
cross-thread jobs and three per 1,016 objects on one-way runs; `m = 1,024` on 4,096 makes 5 to
15 times fewer, as many as base's exchanges or fewer. Measured extra cycles of `m = 256` over
its control matched checkpoints times 62 on the single-threaded jobs (tree -7M against 8M
predicted) and exceeded it two to five times on the cross-thread jobs, which sum the cycles of
threads that wait on each other.

Memory probes (`worst.sh`, one run each): every shape within 0.5 MiB of base on burst-idle,
classes-few-cpus, sparse and tiny; `phase` 48.6 base, 50.6 (256 on 1,024), 47.5 (u4096), 53.9
and 53.5 (the segmented 4,096 shapes); cpus-few-classes 7.0, 7.5, 6.7, 7.5, 9.5.

### Policy B

How often each job's checkpoints went on under policy B (`m = 256`, counting builds):

| Job | went on | stopped |
|---|---:|---:|
| tree, JSON, BTreeMap | 0 | 82,560; 61,620; 19,659 |
| pipelines t2 (all sizes, local and split), x2, x3, x4 | 0 | all |
| mpmc p1c3, p3c1, p4c4 | 0 | all |
| pipeline t8 | 53,022 | 68,374 |
| exchange t2 local / split / t8 | 5,989 / 4,495 / 9,523 | 24,289 / 26,314 / 18,388 |

The reviewer was right: a contended round trip fires only where flows cross many CPUs, and
never on the one-way pipelines or on single-threaded jobs, where the size pays. Under B those
jobs behave as with 254-slot magazines. A rule that sizes the transfer by one-way runs (grow
on successive exchanges in the same direction) is what these jobs ask for. Policy C was built
(`pc1`, `pc10`) but not timed: the answer above decided the design first.

### What carries over, and what a real build must touch

- **Lean.** Nothing to redo. The routine's abstract magazine is the `m`-slot aligned window
  around its word (`slotsOf mask`, `magBase`, `Mag.ofWord` in `Spec/Slab.lean`); the
  precondition asks that window to be a magazine apart from the slabs and from every other slab
  word's window. A segment of an `n`-slot magazine is exactly such a window, so with `m` for `n`
  the statements and proofs hold as they are. What changes is the Rust side's slab invariant
  ("slots 1 to `s - 1` hold objects" becomes "every slot below `s` but the boundary pairs") and
  the contracts that cite it: `Routines::store_if_equal` and DEPOT-SLOW-01, AGE-01, SPARE-01,
  MAGAZINE-02 (the object slice now spans boundary slots). A fused cross-and-step routine would
  be new Lean work; the prototype does without one.
- **Geometry and counts.** `MagazineSize` (capacity, mask, slot-to-object and level-to-word
  maps), `front::MASK_TABLE`, `Magazine::{objects, object, set_object, word}`, `Area::loaded`,
  the record's clamp. Spare limits, stock weights, empty caps, shared targets and
  `LAUNDER_BUDGET` follow `capacity()`.
- **The exchange loop.** Without the checkpoint, `exchange_for_alloc` and `exchange_for_free`
  spin forever at an interior boundary (their "the routine should succeed now" retries see a
  level that is neither empty nor full), so the checkpoint is not optional anywhere a word can
  reach a boundary: the drain, the age limit and a stopped policy all leave words there.
- **Fill, launder, drain.** `Source::fill`'s contract (positions are not contiguous) and both
  implementations, the short-fill move, `launder_held`, and the back end's null skip or an
  equivalent. The drain, the age limit's take and the sweep go through `Area::loaded` and
  `launder_held` and needed nothing more.
- **Kani and tests.** `classes/verify.rs` (the slot tests mean empty and full),
  `depot/verify.rs` (words and levels round-trip), the twins (they take the mask from
  `MagazineSize`), three unit tests that pin the mask to the magazine size, one rules test that
  assumes a free fills a whole magazine, and the test rig's regions, which must hold the largest
  magazine (32 KiB at 4,096 slots).
- **Policy state.** No room in the slab's cold part for a per-class stamp; it must share the
  stock word or the age stamp.
- **Work bounds at 4,096.** With the budget at one magazine the worst call launders 4,094
  objects. Laundering one segment per slow path keeps today's bound and fits the code: the
  launder already takes a slot range, and the rules that wait for room would wait for a
  segment's room. The fill is already bounded in claims, not objects.

### A thread that oscillates across an interior boundary

The two words for a boundary level give the boundary hysteresis: a thread whose level moves
by one around it crosses once and then stays on the side it crossed to. A swing of two
straddling the boundary (level `L - 1` to `L + 1` and back) crosses on every swing, one
checkpoint per two operations, about 30 cycles an operation. Today's magazine ends have the
same worst case at the cost of a spare swap, at a quarter (or, at 4,096, a sixteenth) as many
levels. A guard: count reversals at one boundary in the slab, and on the second take the
magazine out with store-if-equal, move half a segment's objects into a spare magazine, and put
it back half a segment from the boundary; that bounds the rate to one move of 127 pointers per
127 operations. Not built: the `osc` probe that should have measured the worst case lands on a
boundary only in some runs (the runtime's own start-up allocations shift the level), and in
the runs that missed, all builds were equal.

### Where these numbers may mislead

- **Code placement.** `benchrun layout` refused every build of this prototype it made: in two
  of its four placements the GOT slot of `__rseq_offset`, which the out-of-line routines read
  (the store-if-equal the checkpoint uses among them), shares a hot word's page offset. `run`
  does not check hand-built entries, and in the shapes session the control, u4096 and the
  `m = 1,024` shape had that collision (base, `m = 256` and u256 did not; the question-2 runs
  used padded, clear builds of the control and u256). Their exchanges paid for it, so u4096's
  and `m = 1,024`'s gains are if anything understated. No layout check was completed.
- **Five rounds.** Several cross-thread jobs have per-round ranges wider than the differences
  (mpmc p1c3, sh8bench, larson); the control moved JSON by five points between sessions.
- **u256 is not main.** It is this branch with 256-slot magazines, so it carries the branch's
  work bounds and launder rules.
- **Counting runs** ran on the heavy cores, on one L3, so cross-L3 jobs' counts are for one
  domain.

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
  Every 2 s `ResizeSizeClasses` grows the 5 classes most often refused room, on up to 10 CPUs,
  at the others' expense, with the CPU stopped. Every 5 s `ShuffleCpuCaches` lets the 5
  most-missing CPUs steal 5% of the limit from CPUs with under 80% of their misses and over 20%
  of the limit.
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
  idle bin drops to 2 at once; a refill raises retention to at least twice what it filled.
  Overflow halves retention. GC is an event every 64 KiB of the thread's allocation or free, at
  most every 10 ms. It replaced eight options to simplify the configuration.
- **tcache, 5.3**: `lg_fill_div` raised at a visit with low water above zero (flushing three
  quarters of it), lowered at the next visit after a refill that found the bin empty; one bin
  per 64 KiB event.
- **Decay** (`decay.c`, `pac.c`): pages dirtied in each of 200 epochs of `dirty_decay_ms` (10
  s) are allowed to stay with a smootherstep weight; jitter keeps arenas out of lockstep.
  Without background threads the clock is read every thousand or so arena events; with them,
  the thread sleeps until 1,024 pages would be purged, at least 100 ms. It replaced ratio-based
  purging. Fails on phases longer than the window; an idle process never purges.
- **HPA**: off by default. A peak-demand window modelled on tcmalloc was added in 2025 and
  reverted about three months later without a stated reason.
- **Failure modes**: an idle thread keeps its cache; a consumer thread converges to flushing
  nearly the whole bin, under the bin lock, at every overflow.

### Bonwick and Adams 2001 (illumos `kmem.c`)

- **Magazine resize**: every 15 s, if the depot lock's failed `tryenter`s rose by more than 3
  (`kmem_depot_contention`), `kmem_cache_magazine_resize` purges every CPU's magazines and the
  depot and steps to the next row of `kmem_magtype[]` (1, 3, 7, 15, 31, 47, 63, 95, 143
  rounds), while the object is under the row's limit. Grows only; only allocation contention
  is counted; each resize discards every cached object.
- **Depot working set**: each list keeps its minimum; every 15 s the old minimum becomes the
  reap limit, and a reap frees what was unused for the whole previous interval. It runs under
  memory pressure and on a few explicit calls (`kmem_cache_reap_soon`, the identifier-space
  reap, the defragmentation scan), so idle depot memory is otherwise kept.

### FreeBSD UMA (`uma_core.c`)

- **Bucket size**: from a table by item size (up to 254 entries); `+1` each time
  `zone_domain_lock` finds the lock held (2013: "only grow the bucket size when we face
  contention ... grow more slowly"); `-1` on every reclaim, trim or drain, and halfway to the
  minimum when the per-CPU caches are drained.
- **Working set** (`zone_domain_update_wss`, every 20 s, "constants ... somewhat arbitrary"):
  `wss = max(3/4 wss, imax - bimin)`, the largest allocation batch; a long-term minimum by a
  1/256 moving average. Trim under pressure to the working set; after 15 minutes with no
  near-miss, release a quarter of the long-idle surplus every 20 s. Added because repeated
  low-memory events squeezed real consumers (the ZFS ARC halved) into memory stuck in UMA.
  Per-CPU buckets are outside this.

### Linux (`mm/page_alloc.c`, `mm/slub.c`)

- **Per-CPU page lists** (Huang Ying, 2023): `high` from `high_min` (the low watermark per CPU)
  to `high_max` (managed pages / 8 per CPU). Each refill from the zone adds the scaled batch
  (`nr_pcp_alloc`), and a free that finds the list at `high` raises `high` to hold the run of
  frees and a batch (`nr_pcp_high`); the refill grows by `batch << alloc_factor` on consecutive
  refills, to `CONFIG_PCP_BATCH_SCALE_MAX` = 5, chosen from a 100 us latency budget; every free
  halves the factor. `decay_pcp_high` takes up to an eighth a second ("kind of arbitrary").
  Below the high watermark, growth stops and frees lower `high`. Kbuild: 3.5 to 5% faster,
  zone-lock contention from 11 to 14% down to 0.5 to 1%, up to 18.4% more memory. A CPU that
  only allocates grows to the maximum, which the author saw and accepted because no pages sit
  idle there. The series' last patch (6ccdcb6d3a74) counts consecutive frees to drain
  high-order pages sooner; it fired too eagerly (iperf3 -30% from 32 to 64 pairs; c544a952ba61
  raised its threshold in 2025). Decay rides on the vmstat worker, which does not run on
  isolated CPUs.
- **Sheaves** (2025, default in 7.0): capacity by object size (4, 12, 28 or 60, the 28 being
  26 rounded up to an allocation size), a main and a spare per CPU, a barn of 10 full and 10
  empty per node. Nothing adapts; no timed drain.

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
  maximum. G1 resizing at every GC (JDK-8238687 with JDK-8247843, 2025) doubled `GCTimeRatio`
  from 12 to 24
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

Read for the review, besides the above. Local: `third_party/tcmalloc/tcmalloc/common.h`;
`third_party/go/src/runtime/mgcpacer.go` (`defaultHeapMinimum`). Fetched: jemalloc `dev`
975f3b0 (`include/jemalloc/internal/{tcache.h, tcache_inlines.h, tcache_ncached_target.h,
pac.h}`, `src/{tcache.c, jemalloc.c}`, `ChangeLog`) with commits d13fe91a, ad108d50 and
27d7960c, and tag 5.3.0 (`src/tcache.c`); illumos `uts/common/os/{kmem.c, vm_pageout.c}` and
`sys/kmem_impl.h`; FreeBSD `sys/vm/{uma_core.c, uma_int.h}` with commits fc03d22b177f and
2760658b211c; Linux `mm/{page_alloc.c, slub.c, vmstat.c, Kconfig}` at v7.0, v7.2 and master,
with the commits above and ca71fe1ad922 (the series' cover letter), be5e015d107d,
148f95f75c51 and e47c897a2949; Bonwick and Adams, sections 3.4, 3.7 and 6.1; JDK-8247843 and
the integration of 8238687 on the hotspot-dev list, July 2025; Cidon et al., "Cliffhanger:
scaling performance cliffs in web memory caches", NSDI 2016, through the summary at
blog.acolyer.org (24 March 2016).
