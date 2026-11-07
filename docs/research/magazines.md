# Magazines, the depot and the back end

How objects flow between per-CPU caches, the depot and the bitmaps, and what launder, fill and
magazine size cost and buy. `docs/design.md` describes the mechanisms; this note has the evidence
for them. One machine throughout: Ryzen 9 9950X, two L3s, Linux 7.2.9. [workloads.md](workloads.md)
defines the jobs named here.

## What we know

- **On a server almost nothing leaves the loaded magazine.** The order service and the log RPC jobs
  take no slow path in hundreds of thousands of requests (the order service over HTTP/2, one in
  1,700). The log store takes about one slow path a request, a swap, and Meilisearch 80 slow paths;
  with maintenance they are 0.33% and 0.58% of those jobs' cycles. A free on another CPU is not by
  itself a flow: on the order service the worker that evicts what another made also allocates its
  replacement.
- **The price rises in steps.** A swap with a spare is 271 cycles in a tight loop and about 640 on a
  real server. A slow path through a contended depot is about 1,600. Per object, a launder costs 5
  cycles for a dense magazine, 23.5 for a scattered one in today's largest batches and 197 when the
  class has a quarter of a billion live slots; a fill costs 4.3 from dense bitmap words and about
  100 from words with one free slot.
- **A magazine's size is worth what it saves at the depot.** Going from 254 to 1,024 slots took 10
  to 18% off two to four pipelines of small objects and 22% off the worst C stress job, for 0.5 to 3
  MiB on small cross-thread heaps and under 2% on other heaps over 10 MiB. Entering the slow path
  hardly matters: a build that entered it as often as 256-slot magazines, but reached the depot as
  often as 1,024-slot ones, kept 85 to 100% of the gain on the pipelines and the tree.
- **The worst failure was a lock at a magazine's ends.** A swap installs a magazine at one end, and
  a swing across that end then crosses the far end every request. It cost the order service 15 to
  17% of its cycles and put p99.9 as high as 6 ms, and at 70% of its own saturated rate the server
  fell behind its load, to 708 ms at 4 GiB. Leaving the magazine half full after two reversals in a
  row removes it, from 8.7 slow paths a request each way to under 0.0001. More spares and larger
  batches do nothing for it.
- **Depot stacks per L3 pay where work is independent.** Two to four separate pipelines across the
  L3s run 1.7 to 2.3 times faster, and a real flow between L3s gives up 2%.
- **Stock, the spares a CPU earns under contention, buys C stress jobs and costs the RPC jobs
  nothing.** Connect, gRPC, the order service on axum and mini-redis give it nothing to act on once
  the half-full rule is in. The log store and Meilisearch do reach the depot, and no build counted
  stock on them.
- **Not built:** a back end that keeps objects in their L3 (9 to 13% on one job shape, for memory
  and a change everywhere), segmented magazines, and the fill-size rule. The fill-size rule is a
  proposal; "Sizing on evidence" says what it would have to show.

**For the design.** Magazines stay fixed per class, so the fast path carries no sizing state.
Slow-path work goes to avoiding shared traffic, not to shaving the entry. Every call keeps a hard
bound on what it does. What could change this is a server on many more CPUs: every server
measurement here ran on four, and what magazines park scales with CPUs, not with the heap.

## The tiers and what crossing each costs

| Tier | Bound (design, "Sizing the caches") | Crossing it costs |
|---|---|---|
| Loaded magazine | 64 KiB of objects, 4 to 1,024 slots | a pair 4 cycles in a bench, 11 to 18 on servers |
| CPU's spares | 1 MiB of objects a CPU; fixed full spares for 8 B (two) and 16 B (one) only; four empties; the rest earned as stock | 271 cycles in a loop, about 640 on a server |
| Domain's depot stack | 32 KiB a class, at least one magazine | two atomics; when contended about 560 cycles of stack operations, in a slow path of about 1,600 |
| Shared depot stack | 96 KiB a class, at least one magazine | the same, plus lines from the other L3 |
| Bitmaps | a fill claims at most 256 words; a slow path launders at most 1,022 objects by choice | per object, below |

## What launder and fill cost

Cycles and instructions per object, a magazine a call (254 objects, the size then), 8 to 256-byte
classes alike; lowest of seven repetitions, medians of five interleaved rounds, pinned; 2026-10-18
at 1868dd0 and d56e3bf.

| Case | Cycles | Instructions |
|---|---:|---:|
| Fill, dense words | 4.3 to 4.5 | 16 |
| Fill, one free slot in 8 | 18.5 | about 78 |
| Fill, one free slot a word | about 100 | about 500 |
| Launder, one run up or down | 4.6 to 4.9 | 20 |
| Launder, two interleaved runs (how JSON's magazines come back) | 8.2 to 8.4 | 39 |
| Launder, shuffled within 256 objects | 10.2 | 43 |
| Launder, scattered (a BTreeMap dropped in key order) | 27 | 53 |
| Scattered, 4M live slots of the class (512 KiB of bitmap) | 54 | 124 |
| Scattered, 64M live slots (8 MiB) | 102 | 267 |

A batch of 1,022, what the smallest classes launder today, is cheaper per object where the bitmap
fits the caches. Scattered, 254 against 1,022 objects a batch: 27 and 23.5 on a small heap, 112 and
74 with 64M live slots, 199 and 197 with 256M. The bench stops there, since 2^29 slots of 8 bytes no
longer fit a region.

A 4 KiB class moves 14 objects a call, so its per-call costs dominate: fill 21 cycles, dense launder
29, scattered 52. On Meilisearch fill and launder together average about 20 cycles an object
(computed from the 2026-10-22 profile and counts), between dense and scattered.

- **What a dense launder is made of.** The loop is 12 instructions and about 2.5 cycles a slot. A
  call pays about 300 cycles once, for the walk up the summary tree, the retained-count add and the
  host lookup: 1.2 a slot. One locked OR per bitmap word adds under one a slot. So the depot and
  back end deal in whole magazines: copying through a 64-entry buffer, with a back-end call per 64,
  cost a dense launder 9.0 cycles against 4.9 and a dense fill 7.1 against 4.6.
- **The per-slot check is one multiply.** With a slot size `q << e`, `q` odd, and `n` the pointer's
  offset from the region, `k = rotr(n * q⁻¹ mod 2^64, e)` is the slot index if `n` is a multiple of
  the size and huge otherwise (Hacker's Delight 10-17). So `k < watermark` checks start, alignment
  and range at once. Kani proves it for every class and offset of a 4 GiB region.
- **A dense fill is a store loop:** four instructions a slot, and two atomics a claim of 64.
  `count_ones` compiles to a dozen instructions for lack of `popcnt`; built with
  `+popcnt,+bmi1,+bmi2,+lzcnt` a dense fill measured 4.1 against 4.6 and a one-in-64 fill 98 against
  124, before the batched clear took that fill to about 100, so the gain today is not known. A fill
  from single free slots costs about a hundred cycles a claim, two atomics a slot, and no change
  helped it much.
- **A scattered slot needs its own locked operation.** Each has a word to itself, so nothing groups:
  a `lock bts`, about 20 cycles that do not overlap the next. After eight single-slot runs in a row
  a batch is judged scattered and goes direct, with no map. That took scattered from 51 to 27 cycles
  and 107 to 56 instructions.
- **The direct path has one regression.** Strided slots over 64 MiB of bitmap went from 75 to 208
  cycles, 139 after the wide path, with a DRAM fill a slot where there were none: the old path's
  ORs, grouped at the batch's end, were what the prefetchers followed. Prefetching would recover it
  (83), and was not kept.
- **A class with millions of live objects has more summary words than a magazine has slots.** Then
  nothing merges, and each slot paid a table place and a general walk. So after 64 direct slots a
  batch that took 56 or more places of its own is judged wide, once, pays each summary word by
  looking, and merges only what that owes the level above. That took 4M live slots from 173 to 124
  instructions and BTreeMap at 8M entries 10% off its drop's cycles.
- **The old value of an atomic OR is not free.** Used, as the double-free check uses it, the OR
  compiles to a load and a `lock cmpxchg` loop, and costs what one locked instruction costs.
- **The launder runs on whatever stack called `free`.** Its deepest stack is 6,151 bytes (`workload
  stack`, 2026-10-18).

**Drops in programs.** M cycles / M instructions over the job's repetitions, five interleaved
rounds, medians, 2026-10-18:

| Drop phase | cralloc | mimalloc | jemalloc |
|---|---:|---:|---:|
| BTreeMap 1M entries, scattered | 563 / 630 | 525 / 280 | 630 / 776 |
| BTreeMap 8M entries, wide | 2622 / 2931 | 2234 / 896 | 2947 / 2555 |
| JSON 32 MiB, two runs | 826 / 1889 | 698 / 1286 | 1663 / 2770 |
| Box tree depth 20, dense | 419 / 1209 | 371 / 830 | 1324 / 2457 |

On the BTreeMap a freed object costs 112 instructions under cralloc: the program's drop 22, `free`
23, the launder about 60, the exchange about 7. mimalloc costs 49.5, pushing onto a page's list with
no atomic; jemalloc 137, with about 60 in its cache flush. So 2.25 times mimalloc's instructions
cost 7% more cycles, and what is left is one locked OR a slot against a plain push.

## Fillers of one class part quickly

Eight threads refilling one class fought over a summary word every filler shares. Per successful
claim, at one free slot in 8: 3.1 atomic clears of it, 2.7 restarts from the top, 2.1 leaf words
found empty under a set bit, 0.56 failed compare-and-swaps. The failed compare-and-swap, the obvious
suspect, was the smaller part.

So a claim retries against the value it found, a run of claims moves past the group of 64 words
where it meets another claimer, and a run clears its own emptied words with one AND, then rereads
each and sets back any no longer empty (`tla/Summary.tla`, `Run`, shows the rereads are needed).
Fill, cycles per object per thread, two interleaved rounds, 867aadf against the change:

| Threads | Dense, before | Dense, after | One in 8, before | One in 8, after |
|---|---:|---:|---:|---:|
| 1 | 6.4 | 6.0 | 19.2 to 19.6 | 16 |
| 4 on one L3 | 15 to 18 | 6.7 to 8.7 | 56 to 123 | 21 |
| 8 on two L3s | 60 to 85 | 23 to 26 | 664 to 691 | 33 to 51 |

Laundering at eight threads on two L3s went from 17 to 37 cycles to 13 to 20 dense, and 48 to 59 to
18 to 21 at one in 8. What is left across L3s is 2 to 4 times one thread's cost: leaf lines handed
over as every thread recycles the same few hundred slots at the bottom of the heap.

## What magazine size buys

A class's magazine is the most slots whose objects fit in 64 KiB, at most 1,024 and at least 4:
1,022 objects up to 64 bytes, 510 for 80 to 128, 254 for 160 to 256, and on down to 2.

**Slots.** Each size a constant change against 254 slots, three to seven rounds; change in the job's
metric, then cgroup peak in MiB (2026-10-19 at e96086d). A *pipeline* has threads that allocate and
threads that free what those allocated; an *exchange* job has threads that trade objects; *mpmc* has
many producers and consumers; *sh8bench* is a C stress program (below).

| Job | 254: peak | 512 | 1,024 | 2,048 | 4,096 | 8,192 |
|---|---:|---:|---:|---:|---:|---:|
| sh8bench on two L3s | 128 | -15% | -24% | -28% | -32% | -33% |
| Box tree, depth 20 | 50 | -5% | -6% | -9% | -10% | -11% |
| Pipeline, two threads, one L3; peak | 2.0 | -7.5%; 2.5 | -10%; 3.0 | -10%; 3.1 | -11%; 3.2 | -10%; 3.3 |
| Three pipelines over two L3s; peak | 4.7 | -12%; 5.7 | -18%; 7.0 | -19%; 7.5 | -20%; 8.1 | -19%; 8.2 |
| Pipeline, eight threads; peak (time level) | 7.8 | 9.5 | 10.8 | 11.5 | 12.0 | 12.2 |

Cross-thread jobs take nearly all their gain by 1,024 slots. sh8bench and the tree go on gaining,
while the memory in loaded magazines and the work of one exchange double with each step. 1,024 gives
the smallest classes what a 256-byte class already held, so it adds no new kind of cost; the
routines and their proofs needed no change.

**At 1,024** (median of per-round ratios, five rounds, ten for RPC): pipelines of small objects -10
to -20%, split across L3s -10 to -12%, independent pipelines across L3s -11 to -18%, two-thread
exchange -5 to -8%, tree -7%, sh8bench -22%, RPC level within 2%, and JSON about -5%, not resolved,
since its control moved five points between sessions. On sh8bench across L3s 1,024 slots halve the
gap to mimalloc (0.597 to 0.431 s at a945951, mimalloc 0.27). Small cross-thread heaps grow 0.5 to 3
MiB (two-thread pipeline 2.0 to 3.0, eight-thread 7.8 to 10.8, four-by-four mpmc 6.1 to 8.2),
fork-join 1.1 to 1.8, and every other heap over 10 MiB under 2%.

**Bytes.** 64 KiB was chosen on 2026-10-17 ([memory.md](memory.md), "Magazine budget"): the
mixed-size pipeline's peak fell 7.07 to 5.12 MiB at +5.1% time, and 32 KiB cost +11.7%. Magazines of
128 slots reach jemalloc's peak on small cross-thread heaps and cost the pipelines 16 to 20%.

**Other costs of 1,024 slots.** The spare budget split evenly gives each class 23 KiB, so only 8 B
and 16 B keep a full spare that way; a thread oscillating at a boundary in 24 to 128 B reaches the
depot for each full magazine, though nothing measured shows a cost. A fill stops after 256 claims,
so in a fragmented heap a magazine may hold as few objects as before. Memory after a peak is level
but for 1 to 1.7 MiB from about 4 s on: fills of 1,022 reach further into a region.

## One call's work is bounded in work, not magazines

Larger magazines would have made every "one magazine a slow path" bound four times the work. So a
fill claims from at most 256 bitmap words (`FILL_CLAIMS`) and a slow path launders at most 1,022
objects by its own choice (`LAUNDER_BUDGET`). A magazine a call's own exchange pushed past a shared
stack's target is laundered anyway, or the stack would gain on arrivals. The worst single call,
per-object costs from the bench times the most a call can do (8-byte class, three rounds), is 50K
cycles on a small heap (88K with 254-slot magazines bounded by magazine), 102K with 64M live slots
of the class (110K), and 228K with 256M (177K), about 41 us. A call that carries a rule the budget
turned away and also launders its own push does two of the largest magazines: 48K, 151K and 388K
cycles, at most once an interval per class and rule. Counted in magazines the bound would have been
about 911K.

## The gain of large magazines is depot traffic, not fewer entries

A throwaway build gave a class magazines of 1,024 slots and the routine mask of 256. The two slot
tests then fail at every 256-slot boundary inside the magazine, and a cheap slow path, a
*checkpoint*, moves the word across. It enters the slow path as often as 256-slot magazines do and
reaches the depot as often as 1,024-slot ones. Five interleaved rounds, change against 256-slot
magazines; differences of two points are within what code placement moves. JSON is left out: its
control moved five points between sessions.

| Job | 1,024 slots | Checkpoints at 256 on 1,024 |
|---|---:|---:|
| Pipeline, small objects | -9.8% | -8.6% |
| Three pipelines across L3s | -18.1% | -16.2% |
| Box tree, depth 20 | -7.5% | -7.4% |

| sh8bench | -25.6% | -18.7% |

The tree enters the slow path 165,000 times at 254 slots and 41,000 at 1,024. The 124,000 extra
exchanges cost about 65M cycles, some 500 each, against 8M for as many checkpoints. What larger
magazines saved is the exchange's work, not the entry.

**Segments are not built, and this is why.**

- A checkpoint costs 62 to 67 cycles and one branch miss. Every 256 slots on 1,024 that is 1 to 3%
  on cross-thread pipelines against plain 1,024. Only 1,024 on 4,096 is free, adding 0.5 to 1.7 MiB
  on the same heaps for 3% on the tree and JSON and 11% on sh8bench.
- The first design, which wrote the slot and then committed the word outside a sequence, can hand
  one object out twice: store-if-equal never guarded a slot's contents. A sound one moves the word
  between the two names of one level. The Lean statements hold as they are; the work is on the Rust
  side.
- The signal does not fire. A contended round trip never occurs on the one-way pipelines or the
  tree, JSON and BTreeMap, where the size pays. A pipeline's magazine is a transfer batch as well as
  a buffer, and round trips size a buffer.

## What a slow path costs

`workload burst` allocates a burst of one class and frees it in reverse, on CPU 2. A burst of 1,000
makes no slow path and one of 1,100 makes exactly two, so the difference over the slow paths is the
price (seven interleaved rounds; rows from three sessions, up to 20 cycles run to run):

| Build, cycles a slow path | Swap with a spare, 8 B | Half through the depot, 64 B |
|---|---:|---:|
| As first measured, 5c9881f | 338 | 450 |
| Handles of whole words | 288 to 304 | 331 to 345 |
| And heartbeats paced by bytes | 271 | 352 |

- **Store forwarding was most of the entry.** Two thirds of its samples sat on one load. The back
  end's handle was written with narrow stores and copied with eight-byte loads, and a three-field
  magazine handle came back through memory into one sixteen-byte load. A load spanning several
  stores waits for them. A handle is now one word, in registers, with compile-time size assertions.
- **The heartbeat is paced by bytes moved.** It runs when the CPU's count passes a multiple of 32
  KiB (`BEAT_BYTES`). Most classes beat on every exchange. The 8, 16, 24 and 32-byte classes, which
  swap the most, beat on one in four, one in two, three in four and all but one in 512; the 5,120,
  12,288 and 14,336-byte classes, with magazines of six and two, on 94%, 75% and 88%. The bound is
  the rules' times: at 64 KiB a quiet domain's stacks went back in 8.4 intervals rather than within
  7, and at 256 KiB twelve tests fail.
- **What is left** in 271 cycles: about a quarter is the refresh rule's launder and refill (one
  retiring full magazine in 32, raised to one in 4 on evidence of fragmentation;
  [memory.md](memory.md)), a memory rule. The rest is about 70 instructions a side of general
  exchange. A path for the local swap alone was not built.

On whole jobs that is a few percent on flows that reach the depot (over four code placements, the
split pipeline 10.2 to 10.4 ns against 10.6 to 10.8, about 3.7%, and the 64-byte pipeline about
1.5%) and nothing elsewhere.

**A real server's slow path costs about three times the loop's.** On the log store the exchange
takes about 640 cycles a slow path, nine in ten of them swaps with spares; on Meilisearch about 750;
and maintenance on request threads adds 480 and 340. (Computed from the 2026-10-22 profile's cycles
by path over the counting build's slow paths.) Those figures leave out laundering, and the loop's
271 includes a quarter of refresh launders, so like for like it is about 200. The bench repeats one
class on one CPU, so its lines stay in L1; a server's do not, which was not measured further.

## One depot for the machine ran independent work at half speed

With one depot, magazines one L3 freed were popped by the other, and each slot and most objects were
then a fill from the other L3. C stress programs, the same thread counts on one L3 and spread over
two, five rounds, at b784f1a:

| Program | cralloc, one L3 / two | Fills from the other L3, per op | mimalloc, one L3 / two |
|---|---:|---:|---:|
| sh8bench, s | 0.345 / 0.756 | 0.137 | 0.218 / 0.242 |
| xmalloc-test, M frees/s | 1074 / 373 | 0.271 | 355 / 297 |
| larson, M ops/s | 584 / 395 | 0.077 | 668 / 489 |

xmalloc-test across L3s is bimodal for every library by where the scheduler puts its threads, about
half the runs near 735 and half near 390, so its row mixes a mode with the depot's effect. tcmalloc
per-CPU has the same failure (sh8bench 2.79 times as long across L3s, xmalloc-test at 0.37, 0.326
fills from the other L3 per op), so it belongs to per-CPU caches over a shared tier, not to us
alone. Stack heads on their own lines changed nothing measurable (0.752 against 0.727 s).

**Stacks per L3.** Independent pipelines, ns an allocation, medians of five, per-L3 stacks against
one depot: two across the L3s 11.16 to 4.78, three 9.53 to 5.54, four 10.23 to 5.60. A real flow
across L3s, where a shared depot is why we lead (jemalloc 18.35), gives up 2%. Jobs on one L3 moved
-1 to +3%, within the spread of repeat runs. Jobs on both L3s peak 0.3 to 0.8 MiB higher.

**One shared stack per class.** A take-across rule between domains had three faults that need three
or more domains, which is every server: a third domain's stack sat at the threshold and sent flows
through the bitmaps, N domains parked N/2 targets, and a miss read every other domain's head. So a
slow path pushes on its domain's stack below a cap, else the shared stack, and pops its domain's,
else the shared, else fills. No exchange reads another domain's stack.

**The cap is a quarter.** At half, jobs across L3s were no faster and peaked up to 0.7 MiB higher
(mpmc 4+4: 6.16 MiB before, 6.70 at half, 6.18 at a quarter). Five interleaved rounds at ee4c29b.

**Looks cost where the domain is busy.** A look visiting a whole other domain made three pipelines 5
to 7% slower, so a look skips a domain whose own CPUs looked this interval. A heartbeat reading the
shared head cost mpmc 4+4 6%, so it checks only after a push there.

**More domains than L3s.** Forced to a domain per CPU, flows between domains trade magazines through
one shared stack: two pipelines in different L3s ran at 11.4 ns, what one depot cost. On one socket
such a flow crosses anyway. On several memory nodes a flow could take a magazine from a flow on
another node; a shared stack per node is not built.

**One spare a stack against four:** mpmc 4+4 peaks at 5.61 against 6.07 MiB, sh8bench on two L3s
+3.5%.

## Contention is what makes an exchange dear

sh8bench is a C stress program: 16 threads on 8 CPUs, each allocating and freeing its own blocks in
bursts of 50,000, two thirds of them 16-byte. We do not steer by C programs, but it isolates
contention well. With 254-slot magazines, at a945951, five rounds, on one L3 it took 14.5 cycles a
call against mimalloc's 9.2: +2.2 in depot pushes and pops on a contended head, +1.6 in the rest of
each exchange, +1.2 in launder and fill, +0.6 in the program touching objects not in this core's L2,
and -0.6 in fast paths and PLT.

Every 250 calls is a slow path, about 1,600 cycles: 560 in stack operations with 35% of
compare-and-swaps failing, 400 for the rest of the exchange, 570 laundering and filling. So the gap
is the number of exchanges reaching shared structures. Keeping every spare on its CPU took 12.8 to
9.1 G cycles; a depot target of 64 MiB only to 12.3. Laundering is smaller than it looks: without it
the same objects come back from other cores' caches. Making the activity counter a load and store
instead of a `lock xadd` moved the wait to the next locked instruction. Committing 16 MiB at a time
removed the `mprotect` sleeps and changed nothing.

On two L3s it is 22.7 against 10.0 cycles a call: 4.6 of the same, +6.3 for objects last written in
the other L3, +1.8 for allocator lines crossing. A burst overflows a domain's cap at once, so 67% of
shared-stack pops take the other domain's magazine. Wall time hides part of this: the kernel's
`CONFIG_SCHED_CACHE` prefers one LLC per process, and runs packed into one L3 take the same 600 ms
as spread ones, because spreading costs as many cycles as the second L3 adds. Cycles a call is the
steadier figure.

## Stock pays under contention and nowhere else

The rule is in the design ("Sizing the caches"): a CPU that sends a magazine away and takes one
back, with a failed compare-and-swap on either trip, may keep one more. Five interleaved rounds,
against before and against four times the room: sh8bench on one L3 291 ms (347, 280), on two 524 ms
(599, 502), peak 132 MiB (121, 129); xmalloc-test on one L3, three rounds by hand, 1137 to 1166 M
frees/s (739 to 753, 1162 to 1181). Pipelines, mpmc, fork-join, spawn, churn, BTreeMap, JSON, tree
and the RPC jobs are level. Its price is 60 to 80 instructions an exchange. With the purge
frontier's 100 instructions and one compare-and-swap a fill, that costs the tree about 0.4 cycles a
node.

Tried first: crediting when a CPU popped that very magazine back earned too little (1.2% of depot
pops returned the popper's own). Without the contention condition stock went to every round trip,
and the RPC servers held 2 to 4 MiB more for no speed. A running byte count drifted and never
healed, so the room is read from the stacks' depths.

## A back end that keeps objects in their L3 is not built

The back end knows nothing of cache domains: a fill takes the class's lowest free slots, whoever
freed them. A throwaway build tried a carving region per class and domain. Fills preferring their
own domain's free slots alone buy little, because a domain with none reuses the other's, and the
shared stack goes on trading bursts. A pair of rules buys the rest: carve in one's own domain before
reusing another's slots, and refill from one's own free slots before popping a magazine another
domain pushed, unless its objects live here. On sh8bench across L3s (2026-10-19, 515412e): 0.390 s
and 15.6 cycles a call at base, 0.373 and 14.8 with fills preferring their own, 0.297 and 10.8 with
both rules, against mimalloc's 0.256 and 9.8; fills from the other L3 fall from 0.022 a call to
0.003, and the peak rises from 137 to 147 MiB.

Rust jobs, eight to twelve rounds: three independent pipelines -13%, four -9.5%, two -2.6%; flows
between L3s -2 to -4%; mpmc, fork-join, spawn and the eight-thread jobs level; nothing on one L3.
The price: 0.3 to 1.6 MiB on those jobs; a floor that grows with domains (mpmc 4+4 at eight domains
13.4 against 10.5 MiB); and load that moves between domains holds both domains' memory, 402 MiB
against 247 (jemalloc 249). Carving first only under the retained cap bounds that to 308, and then
costs the second domain up to 45% an object, because its magazines mix two regions and the launder's
fast check covers one.

Traps for whoever builds it: judging a magazine by its pusher alone cost xmalloc-test 7% and the
split pipelines 3 to 4%; judging by its objects' home needs the strict carving, or mpmc 4+4 runs 14%
slower; and the depot rule costs a third more launders, which is why it is off with one domain. It
would touch the class-to-region mapping, the fill's search, the launder, the depot and the
assignment proofs, for a narrow gain. Build it only if work spread over many L3s becomes a target.

## The magazine-end lock

A swap installs a magazine at one end: full for an allocation, empty for a free. A request whose
allocations and frees of a class go first below where they started and then above it then crosses
the far end of the new magazine, every request, and each crossing puts the next magazine at an end
again. Which (CPU, class) pairs lock is decided early, by where the first magazines land, and lasts
for as long as the worker stays put. A pair started mid-magazine never takes a slow path.

Counted per CPU and class in connect-rust's log server at saturation, 2026-10-20 at db31497, 15 of
24 processes had locked pairs: those with none ran at 246k to 262k req/s, those with 20 to 29 at
228k to 236k. A locked pair takes one allocation and one free slow path per request, in classes of 8
to 48 bytes and a few larger (640, 5,120). On CPUs with spare room they are swaps; on CPUs without,
depot trips, the only depot traffic of any size.

**Workers that stay on their CPU run locked.** On the order service at 280k req/s: with the idle
states free, 0.33 to 0.47 slow paths a request each way; held out of deep idle, 6 to 7; with idle
states free but workers pinned, 5.2 to 6.1. An unpinned free run has 0.001 to 0.002 migrations a
request, each moving a worker's swing onto magazines where it starts away from an end, so the free
runs that first looked level were being rescued by the scheduler. Locked, slow paths and heartbeats
were 17% of the server's cycles, about 1,000 cycles a slow path, some 13 a request, all of it user
mode on the workers: kernel cycles are level and jemalloc's do not move. Locked runs reach a p99.9
of 0.4 to 6 ms and miss up to 22k of 2.2M slots, even with cores held. At 70% of its own saturated
rate the server fell behind its load: hundreds of thousands to millions of missed slots, p99.9 of
2.5 to 9 ms on mini-redis and 708 ms on the order service at 4 GiB.

**More room and larger batches cannot help.** Eight times the spare budget: 233k req/s, 5 of 8
processes locked. A larger magazine still has ends. An earlier sizing rule, room for a round trip
and a larger batch for trips one way, recovers depot trips on CPUs with no room and never stops the
crossing; its batch rule never fires, since trips alternate.

**The rule.** Two reversals in a row, a swap reversing a swap which itself reversed the one before,
are evidence of a lock. The slow path then takes the loaded magazine out of the slab and copies half
of one magazine's objects into the other while it holds both. One reversal is not enough: it centred
the turn of every swing longer than a magazine, which half a magazine cannot absorb. A centred swap
that any other swap of the class follows within an interval has failed, and after the j-th failure
in a row the next 2^(j-1) lock swaps stay plain, at most 64; only an interval of silence clears the
count. Slow paths per 100 repetitions of a swing on one CPU, the same for 8, 64 and 1,024 bytes
(`depot/rules.rs`):

| Swing, magazines | 0.25 | 0.5 to 0.75 | 1 to 1.5 | 2 to 2.5 | 4 |
|---|---:|---:|---:|---:|---:|
| Rule on | 0 | 203 | 400 | 600 | 1000 |
| Rule off | 200 | 200 | 400 | 600 | 1000 |

Over two-size swings and all starting levels the rule costs +0.1 to +0.9%; of 192 patterns, 185 are
within 2% and seven at 2.1 to 3.4%.

**What it did to servers.** Server cycles a request at 70%, three rounds, cores held; 2026-10-20,
the lean slow path and the first form of the rule, one reversal, against main before them. These are
single builds: the Connect and gRPC rows move 4 to 5%, the size of that job's placement range, so
only the order service and mini-redis rows stand. The lean path alone measured the same as main on
the order service: the gain is the rule's, by taking swaps away rather than making them cheaper. The
rule as it is now, two reversals, read 68.5k on axum over HTTP/1.1 against 79.4k before and
jemalloc's 70.0k, and on actix 56.3k against 64.3k and 57.6k; the next section's counts are its slow
paths.

| Job | Before | After | jemalloc | Slow paths a request each way, before / after |
|---|---:|---:|---:|---:|
| Order service, axum, HTTP/1.1 | 80.4k | 68.8k | 69.1k | 8.7 / under 0.00001 |
| Order service, axum, HTTP/2 | 64.0k | 60.5k | 61.1k | 1.2 / 0.0001 |
| Connect log, HTTP/2 | 135.3k | 128.6k | 137.1k | 4.4 / under 0.0002 |
| gRPC log, HTTP/2 | 136.1k | 130.4k | 140.2k | 5.7 / under 0.0001 |
| mini-redis | 38.3k | 36.1k | 36.2k | 1.9 / 0.0001 |

One-way flows never reverse and never pay, and memory after a peak does not move.

## When a server's load reaches the slow path

Counts from a scratch build, never timed, and cycles from `perf` in separate runs at the same rate
(70%), 2026-10-22 at 040033a. Percentages are of the server's cycles.

| Job | Allocations a request | Slow paths a request | Mostly | Slow paths and maintenance | jemalloc's slow paths |
|---|---:|---:|---|---:|---:|
| Order service, HTTP/1.1 and at 2 GiB | 138 | none in 940k and 2.4M requests | | 0.00% | 0.20% |
| Order service, HTTP/2 | 140 | 0.0006 | depot pops, spare swaps | 0.02% | 0.22% |
| gRPC and Connect log, HTTP/2 | 135 | none in 250k requests | | 0.00% | 1.25 to 1.44% |
| Log store | 2,610 | 1.06 | swaps with spares | 0.33% | 1.11% |
| Meilisearch | 49,600 | 80 | three in four spare swaps, 17 depot pops | 0.58% | 1.16% |

**A balanced request stays in its magazine.** Once a magazine starts half full, a class whose swing
per request is under half a magazine never leaves it. On gRPC over HTTP/2: 640 B five a request (62
in a magazine), 5,120 B two (6), 20 KiB one (2), 24 KiB two (2), 256 B 50 (254), 128 B 51 (510), 48
B 50 (1,022). Even the 24 KiB class, as many a request as its magazine holds, stays, since its
objects are freed before the next are allocated, so its swing is one or two.

**A free on another CPU is not a flow.** Of blocks followed to their end, 20 to 28% on the order
service are freed on another CPU, up to 84% in the classes of a cached order (96, 128, 160, 48 B),
which live until a later request evicts them: 0.42 s on the small service, 4.6 s at 2 GiB. No slow
path follows, because the worker that evicts has just allocated the replacement, so each frees as
much as it allocates whoever made the blocks. Measured on the Connect and order jobs over HTTP/2,
which crosses workers more: what CPUs took from the depot beyond what they sent is 0.006 to 0.022
magazines a request.

**A long swing costs one swap a request.** The log store's requests make about 2,600 allocations,
80% of them 16 and 24 bytes, in bursts that often outrun half a magazine (inferred from the swaps;
the swing was not measured). It pays about one swap a request, a depot trip every 12 requests and a
fill every 50. Meilisearch looks the same at a larger scale: 0.3% of its blocks cross CPUs, so its
80 slow paths a request are probably long swings on one CPU (also inferred).

**No server job shows a strong one-way flow, as far as it was measured.** Under 1% of blocks cross
CPUs on the RPC jobs and Meilisearch, and the balance above was measured on the Connect and order
jobs. The store crosses 15%, and its depot traffic is one-sided in the counts (0.08 full pops
against 0.0045 empty pops a request), which may be a flow and was not measured. None pays for large
objects crossing threads or grows a table in steady state ([servers.md](servers.md)).

**A burst after a silence refills everything.** The background thread drains idle slabs in a
silence, so every CPU's first calls of every class reach the bitmaps. [servers.md](servers.md), "The
first requests after a silence", has what that costs: it is the one slow-path price a server still
sees.

## Sizing on evidence

Every constant here sizes a buffer between a cheap tier and a dearer one. The question was whether
one rule could size them all: grow on regret, shrink on idleness, thresholds from measured costs,
one exchange rate between memory and time. **It remains a proposal. The code has fixed bounds and
one earned rule, stock** (design, "Sizing the caches").

**Fills that start small are not built, and neither is an allowance for parked memory.** The
proposal: a fill asks for little, doubles while the class's refills on that CPU run one way, and
halves on disuse. Today a fill asks for a whole magazine. Why it might matter: a loaded magazine
holds up to 64 KiB, so a CPU with all 44 classes warm holds up to 2.8 MiB, and its spares up to 1
MiB more: about 120 MiB on 32 workers. Nothing has measured it. Every server run was pinned to four
CPUs, and where parked objects were counted, on four to eight CPUs, they came to 5 to 9 MiB.

**What the evidence would have to show before building it:**

1. On a server of 32 or more CPUs at a heap of 500 MiB or more, parked objects are a share of the
   heap that matters, more than the tens of MiB of constants the project already accepts at that
   size.
2. Those objects arrived by fills. A fill rule cannot shrink what frees left: a swing of frees fills
   a magazine whatever the fill size. So the count must split loaded magazines' objects by how they
   got there.
3. It costs no cycles where the size pays. The gain of 1,024 slots is depot traffic, so a rule must
   reach full size quickly on pipelines, the tree and JSON. At about 640 cycles a real swap, that
   bounds how many extra exchanges a ramp may take.
4. It does not make the first requests after a silence worse. Those already refill every class on
   every CPU once, and a ramp makes that several times.

**Why one exchange rate does not work.** A 4 KiB page kept about 1.5 s to save a refault of about
2,400 cycles reads as about 400 cycles per KiB-second. By the economic order quantity, `Q = sqrt(2DK/h)`, an
8-byte class priced by the page it can pin gets 1,400 objects, which looks like 1,022. A review
found the input wrong: it assumed a million objects a second, and real jobs run a hundred times that
(14,000), so the match is an accident, and the page was held nearer 5 s than 1.5, which puts the
price near 120. At 400 cycles, 1 MiB costs a hundredth of a percent of a core, yet 1 MiB for 10% was
rejected: the memory bar was a ratio to jemalloc, not a price.

**Where the hypothesis breaks.** A parked object's cost is the page it pins, known only when it is
laundered or purged. Tiers are in series: moving to 1,024 slots cut the spares and depot stacks of
classes up to 64 bytes, counted in magazines, by four, so sizes should be stated in the batch of the
tier above. The worst single call is a third currency; keep it a hard bound. And a perfectly sized
buffer has no misses, so its signal disappears.

**What others do.** All have one shape: a regret signal grows a buffer, a low-water mark shrinks it,
and a fixed bound clamps it. Nobody derives thresholds from costs (tcmalloc's source calls its
scores "intuition", UMA's "somewhat arbitrary", Linux's "kind of arbitrary").
[prior-art.md](prior-art.md) has the detail.

Lessons: a shrink must be slower than a growth, or a buffer that is exactly right looks idle.
Contention is free to observe. One-way ratchets fail (glibc's mmap threshold). A knob works when it
is a ratio with a meaning and a hard cap stands behind it, as GOGC and the memory limit do.

**Cheaper rules, if a measurement ever asks.** A review proposed parked memory as a set fraction of
live memory, with a floor, and argued that most of the value comes from asking only "is parked
memory under the fraction?". Cliffhanger (Cidon et al., NSDI 2016, read through a summary) is the
worked form of dividing a budget by regret: a miss that hits a queue's shadow queue moves a fixed
credit to that queue. A rival to segments is 256-slot magazines and a depot moving four linked
magazines with one compare-and-swap. It reaches the checkpoint's ceiling only if a local swap costs
what a checkpoint does. Today's is 271 against 62; an estimate puts a lean swap of three sequences
at 80 to 100, which would bring the rival within a point. That path is not built, so the rival is
unmeasured, not ruled out.

## Tried and rejected

- **Prefetching a scattered launder's leaf words** (16 slots ahead): 16 instructions and 2 cycles a
  slot while the bitmap is in L1 or L2; halves the cost only at 64 MiB of bitmap, 512M live slots, a
  size that no longer fits a region. A plain early load does less.
- **A map or a sort of a scattered batch's words:** a counting sort changed nothing, and the map's
  work on random keys was the 19 cycles a slot the direct path removed.
- **A plain load then `lock or`** in place of the OR whose old value is used: 52.2 to 51.2 cycles
  scattered, dense unchanged, for a double-free check two threads could both pass.
- **A double free caught on every path:** a slot given twice in one scattered batch, which another
  CPU claims between the ORs, goes unseen, as across batches. Accepted.
- **A second judgement of a scattered batch:** a scattered head with a dense tail costs 4.7 cycles a
  slot over the grouped path; not worth it. Judging wide after 32 slots rather than 64 misjudged a
  strided batch and cost it 16 cycles a slot.
- **Paying each scattered slot's debt at once:** a 16 KiB bitmap 64 to 107 instructions and 31 to 47
  cycles; and it cannot stop at a set bit without losing the free that repairs a summary bit a
  thread lost in a fork left clear (`Summary_crash.cfg`).
- **Walking from level 1 for every wide slot:** 164, 204 and 210 instructions at 512 KiB, 8 MiB and
  64 MiB of bitmap; better past 8 MiB, worse at the size a program with a few million objects of a
  class has.
- **Scanning all 44 classes' stock words on each push past a limit:** sh8bench +4.6%. A per-CPU set
  of stocked classes brought it back.
- **Looking at other stacks every four slow paths, not sixteen:** removed a one-interval lag after a
  peak, cost the large-object pipeline 3.5%.
- **Huge pages for a leaf bitmap past 2 MiB:** page walks appear only at 64 MiB of bitmap. Under 2%
  of the objects' memory; not built.

## Open questions

- What a server on 32 or more CPUs parks, and whether it arrives by fills.
- **The 64 KiB budget and the 1,024-slot cap were chosen against jemalloc's peak on small heaps.**
  The project has since stopped steering by those peaks. At 4,096 slots the tree and sh8bench kept
  gaining (-4.2%, -9.8%) for 0.5 to 1.7 MiB and four times the worst launder; a larger byte budget
  has not been measured on large-heap servers.
- The log RPC job at 1,024 slots: +1.3% [-1.8, +3.6] server cycles over twenty rounds at equal
  instructions. Four placements span 1.7%, so unresolved; colder objects from larger fills are
  untested.
- Between centred processes and ones that never locked, 2 to 3% of saturated rate on one server
  (244k to 250k against mostly over 250k). No slow paths are left to explain it.
- Flows among three or more domains, and machines with more than one memory node, were not timed.
