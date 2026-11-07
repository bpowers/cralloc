# What the allocator needs of the memory order

No route a block takes from a free to the next allocation leaned on total store order. Every one
carries a release and an acquire, or, where a routine's plain store is one end, a barrier the kernel
promises. The words several threads race on are sequentially consistent on both sides, so the TLA+
models and the schedule explorers, which know only sequential consistency, say something true of
them. What x86-64 hid was smaller: one published pair a routine reads in any order, one word of a
store-then-load pattern left out of the sequentially consistent order, a kernel path with no barrier
in it, and above all that our own tests could not have shown a mistake.

Arguments here rest on Rust's memory model and on the Arm architecture's, not on what one CPU does:
a CPU that never reorders some pair says nothing about another. Two rules rest on Arm's model and
not on Rust's, which says nothing of a routine's plain store or of an address dependency: a slab word
read with no protection against the committed mark, and a block passed with no order. Kernel facts
were read at tag v7.2.

## What was wrong

| What | Effect | On x86-64 |
|---|---|---|
| The twins' field loads were acquire and their commits release (`front/twin.rs`), as a `mov` is, and an `ldr` or `str` is not | Every test on a simulated heap had a release/acquire chain through each slab word that the real routines lack, so no test could show a protocol leaning on one | The same blindness, no effect on the allocator |
| The simulated fence of every CPU had no barrier (`os/sim`) | The parking protocol's fence was only a count under Miri | As above |
| The C free reads the mirror's shift and base index in any order (`c/mirror.rs`) | A new shift with a zero base index gives `ptr >> shift` for an index. For a heap with base index under 4,096, that is in range and names another region's class: the block goes on the wrong class's magazine. It takes a heap that low, as a small reservation on a small address space may be, and a block freed from C during the first publish. Such a heap is no longer mirrored, so every mix on every target is out of range or right | Not reachable: the routine reads the shift first and the stores stay in order |
| `membarrier`'s shortcut (one user of the address space, or one CPU online) returns with no barrier (`kernel/sched/membarrier.c:357-359`) | A drain's re-read and its reads of what it took rest on the exited thread's full barrier at its decrement of `mm_users`, and on the caller's load of that count staying before them. Arm orders that if the kernel's return is a context synchronization event (`SCTLR_EL1.EOS`, `IFB-ob`), and the language does not. An acquire fence after the call settles it | Not reachable; no instruction emitted |
| The clocked pass's start was a release compare-and-swap, the thread's look before it sleeps a sequentially consistent read (`back/purge.rs`) | In Rust's model the look could miss the start and the starter miss the thread's mark, and the thread would sleep out its half second. Arm's mapping orders it anyway: the `casl`, the `stlr` of the pass word and the `ldar` of the thread's word cannot pass each other. No block is at stake | No: the same instruction |

Nothing here was seen to fail on a real CPU. Every comment that argued from x86-64 now gives the
real argument.

## What stands in for a release

A routine's commit is a plain store and its loads are plain, so a routine orders nothing against
another thread. Three kernel facts do instead.

**A context switch is a full barrier.** `__schedule` takes the runqueue lock and then
`smp_mb__after_spinlock()` (`kernel/sched/core.c:7108-7109`), a `dmb ish` on arm64
(`arch/arm64/include/asm/spinlock.h`), before it stores `rq->curr`, and `__switch_to` has a
`dsb(ish)` after that (`arch/arm64/kernel/process.c:756`), "required by the membarrier system call".
So all a thread stored before it left a CPU the next thread on that CPU sees, and a migrated thread
keeps its own order. This is what makes the routines correct between threads of one CPU, with no
fence of ours.

**The fence is a barrier on the target CPU, before and after.** `membarrier` with
`PRIVATE_EXPEDITED_RSEQ` does `smp_mb()` on the caller (`guard(mb)`, `:373`), reads the CPU's
`rq->curr`, sends an interrupt if a thread of this address space runs there and waits for it
(`smp_call_function_single(.., 1)`, `:396`, whose `csd_lock_wait` is an acquire of the handler's
`smp_store_release`, `kernel/smp.c:342,394`), and does `smp_mb()` again on its way out. The handler,
`ipi_rseq` (`:210-230`), does `smp_mb()` and then raises the rseq event, so that the thread aborts
any sequence on its return. On arm64 every exception from user mode but a system call marks the entry
(`arch/arm64/kernel/entry-common.c:77-83`, `rseq_note_user_irq_entry`), so a sequence stopped in a
page fault when the interrupt arrives aborts too. A store the thread had executed is before the
handler's barrier, whatever store buffer it was in; an exception is precise, so a later one has not
happened. Then:

- what the CPU's threads stored before that point, the caller's loads after the call see;
- what the caller stored before the call, every load of that CPU's threads after that point sees.

Where no thread of the process runs on the CPU there is no interrupt, and the scheduler's barriers
pair with the caller's: if the caller read a `rq->curr` that has left, all the last thread stored is
before that store of `rq->curr`, and the caller's barrier after the read brings it into view; if the
thread is about to come back, the caller's first barrier and its read of `rq->curr` against the
thread's store of `rq->curr` and its `dsb` mean one sees the other. The kernel's own comment lays
out these scenarios (`membarrier.c:10-136`).

**The shortcut.** With one user of the address space or one CPU online the call returns before its
first barrier. What another thread stored came before its exit's decrement of `mm_users`, a full
barrier, and `LinuxOs` puts an acquire fence after the call so that the caller's loads stay behind
the kernel's read of that count (no instruction on x86-64).

The form without a CPU, which the background thread uses, is the same for every CPU.

## From a free to the next allocation

Each route keeps the block's bytes and the magazine's contents in one happens-before chain.

| Route | The chain |
|---|---|
| Same thread, or a signal handler | Program order |
| Same CPU, through the routines | Plain stores and loads, then a context switch |
| A magazine through a depot stack | The push's compare-and-swap releases the magazine's record, link and contents, and the pop's load of the head and its compare-and-swap acquire them. A head is only ever changed by a read-modify-write, so an acquire of a later head still synchronizes with every earlier push under it, and a pop need not release |
| A drain | The fence, to the drainer; then the depot's or a launder's release. Nothing reads a slot or link before the fence returns, because the exchange in step 2 can read a commit before the slots it covers are visible |
| Launder, then fill | The launderer has the chain above; its atomic OR of the leaf word releases, and the fill's compare-and-swap acquires, through any other ORs of that word |
| Large block, single slot | The holder's stores of the size word (release), the free's OR and the next claim's compare-and-swap |
| Purge | The claim acquires the last owner's stores, `madvise` runs under the claim, and the release of the claim carries that to the next owner, who then reads zero |
| Run of regions | `free_run` purges and then gives each tag back by compare-and-swap; the next `claim_run`'s compare-and-swap acquires it |

**A stale link is harmless and cannot be taken.** A pop that reads a head `(M, v)` acquires its
pusher, so the link it reads is that pusher's or later. A later write to `M`'s link needs `M` to have
been popped, which moved the version, and the compare-and-swap then fails (`Stack::pop_noting`).

**The slots a routine wrote.** A push stores the slot and then the commit, and those may reach memory
in the other order; a pop's commit may reach memory before its load of the slot has run. Neither
matters: another thread reads the slots only after a context switch or a fence, and an exception is
precise, so both instructions are complete or neither has happened.

**The fork.** A child is made while other threads run, so it is a cut through them. Copying
write-protects each page and shares it, and the parent's stale translations are flushed by
`flush_tlb_mm` under the address space's write lock: `tlbi` and then `dsb`
(`arch/arm64/include/asm/tlbflush.h:376-386`). A store with a stale translation has completed by
then, into the page the child shares, and one that misses it faults, with the later stores of its
thread not yet retired. So each lost thread's stores reach the child as a prefix of its program
order, as the TLA+ `Fork` model has it. That is an argument from Arm's rule for that `dsb`, not
something we tested.

## Publication

| What | Who writes, who reads | Why a reader's view is enough |
|---|---|---|
| The heap (`SpaceCell`) | Compare-and-swap with release; acquire load | Reads of what the reservation holds follow the acquire |
| Region tag | Compare-and-swap with release, after its metadata is committed; acquire load | A reader holds a pointer into the region, so it is ordered after the naming, or reads through an address dependency (below) |
| Published byte | Compare-and-swap with release after the naming; acquire load by the walks | The walks then read the region's words, all sequentially consistent, each committed before the watermark or tag that leads to it |
| Carving word, watermark and commit (`Grow`) | Sequentially consistent; watermark and commit share a word, set after the commit's call returns | A reader of a watermark sees the commit behind it |
| Large region's header at assignment | Release store after the tag and before the naming, whose compare-and-swap carries it | Nothing reads it until the carving word names it. A loser's undoing is released by giving the tag back, and acquired by the next taker's compare-and-swap |
| Layout (`Memo`) | Relaxed stores, the last a release; the last read with acquire | Every writer writes the same words |
| Domains | Relaxed bytes, then a release of the count; acquire load | As above |
| Magazine area's committed mark | `fetch_max` with release after `mprotect`; acquire load | See below |
| Generation | Compare-and-swap; any reader that finds zero draws and tries it, and a compare-and-swap reads the latest | All threads agree on one value at once |
| Masks and the region bytes the C routines read | Relaxed, value or zero | Any mix is safe: a zero sends the routine to its slow path, and a value is right |
| Mirror's shift and base index | Base index, then the shift with release; the routine reads both plainly | See "What was wrong": any mix is out of range now |

**The committed mark and a slab word read without protection.** A slow path reads a slab word to
name what it will compare, and checks that the word names an admitted magazine, aborting if not. The
thread may have moved, so that word can be another CPU's, written by a plain store, with no
happens-before edge to the reader, and in Rust's model the reader could see the word and an older
mark. Arm orders it. Whoever installs a magazine has made a handle for it, so it read the mark with
acquire before its store, and that read's source is an atomic of some thread: `aob` orders an atomic's
write before a later acquire that reads it, and `obs` a store before a read of it on another CPU. So
mark's write, installer's read, installer's store, reader's acquire load of the word, reader's
acquire load of the mark are in order, and the reader's mark is at least the one the installer read.
On x86-64 total store order gives the same. The acquire on those slab-word reads is what this needs
(checked below; without it the outcome is allowed).

**A block passed with no order.** C asks a `free` to be ordered after the block's last use, not after
its allocation, so a program may pass a block it never touched with a relaxed store. Everything the
free reads about the block's region it reads through the pointer, and an address dependency keeps
those loads behind the load of the pointer. The tag, watermark and metadata were written before the
store that passed it, because a slow path's sequentially consistent operations follow them, and
nothing passes those. The exception is the word that names the heap, which has no such dependency:
a program that hands the process's very first block on in this way could find no heap, and abort. We
accept that; such a program is a data race by C's rules if anyone wrote to the block. Rust's model
has no address dependencies, so there the free has no order at all: under Miri `lookup` reads an old
tag and finds no block, and the test is not run there. So this rule rests on the hardware's order
and on the compiler keeping the dependency, and on nothing we can check in the language.

## A store and then a load, on both sides

Each of these needs sequential consistency, or a kernel barrier, since nothing weaker rules out both
loads missing. `SeqCst` here means every operation in the pattern, in Rust's sense: a failed
compare-and-swap has only its failure ordering, and it is only the read of what it found.

| Pattern | The two sides | Holds by |
|---|---|---|
| Summary bits | A free's OR of the word and read of the bit; a clear's AND of the bit and re-read of the word | `Word`: every operation `SeqCst` |
| A kept bit, a batched clear, a look before an OR | As above | `Word` |
| Hint of which stacks hold magazines | The compare-and-swap of a push onto an empty stack and its read of the bit; a look's AND and re-read of the head | `SeqCst`. Other pushes and pops are not: a re-read that misses a pop sees a stack holding one too long, the safe way, and one that finds an empty stack misses first a push onto that empty stack, which sets the bit |
| Debts' bits (`shed.rs`), tail marks | A debt or mark and read of the bit; a clear and re-read | `SeqCst` |
| Background thread's sleep | The thread's mark and its reads of the work; a waker's write of the work and read of the word | `SeqCst`, the pass's start now included. A lost wake costs half a second |
| Background thread's parking | A slow path's plain store into a slab and its load of the word; the thread's store of `PARKED`, the fence and its second look | The fence of every CPU. The barrier falls somewhere in the slow path's instructions, and everything of it stays on its side, so the thread sees the store or the slow path sees `PARKED` |
| The drain's re-read | A routine's commit; the drainer's exchange, fence and re-read | The fence |

The last two are why no store of a routine needs to be atomic with its load, and why a slow path
needs no fence of its own.

Every operation in `back::word::Atomic` being `SeqCst` is also why the TLA+ models and the explorers
apply to those words as they stand: an execution of only sequentially consistent operations on
them is an interleaving. The exceptions are `Word::store` (a holder's size word; a loser's header) and
the tag bytes, which have no such pattern.

## Hints

The design says a hint's wrong value moves work and does nothing else. On weak memory a read can
also be older than anything the thread has reason to expect; checked one by one, the claim holds.
Whatever a hint says, memory goes back only through a claim, a magazine leaves a slab only through a
commit or the drain, and a magazine handle is only made for an address the committed mark admits.

- **Slab counters and stamps** (`activity`, `beats`, `retired`, `sweep`, `stocked`, stock words,
  stamps, `seen`): relaxed. A stale value moves a beat, a refresh, a take or a drain. A take is a
  compare with the word, and a drain is safe at any time. The parking protocol's second look reads
  `activity` after the fence, and rests on the slabs anyway.
- **Depot words** (`water`, `owing`, `held`, `looked`, a stack's count from a relaxed head): a stale
  depth launders a magazine early or late. A capped push's count is read after an acquire of the head
  and checked by its compare-and-swap.
- **The clock and the walk** (`Counted`, `epoch`, `decay`, `pass`, `purge_at`, `laps`, `returnable`,
  `Hold`'s words): hints. `Counted::now` reads the word and then the clock with no order between
  them, and a load can pass the clock's read. A reading a moment old looks like a clock that went back
  by less than a unit, and re-stamps the step within that unit.
- **Frontiers, a region's cursor, tail words, a free slot's stamp**: `Word`, and hints. A stale one
  makes a purge look at the wrong unit; the purge's claim is what is safe.

## The routines

The routines hold plain `ldr`, `ldp` and `str` and no barrier (`front/provisional/`, read line by
line against this section; `generated/x86_64` is the same in `mov`s). Any mix of old and new values
has to be safe, since their loads have no order: the hot word, then the slot through an address
dependency, and for the C entry points the shift and base index in one `ldp`, the region byte and
the mask. For the hot word and the slot, the thread either wrote them, or follows the thread that
did on its CPU, or follows the fence. A word another CPU wrote is a zero or a marker from a drain,
and neither is dereferenced; or an old value put back by a drain that backed out, whose magazine
this CPU's threads wrote. The C tables are constants, and the rest are above.

The twins use relaxed loads and stores, as the routines' are. What orders them is the machine: a
simulated thread's state word takes a read-modify-write at each step, which a fence and a thread
starting on the same simulated CPU also write. The barrier falls only at steps, which is less than
the kernel gives, but the simulator has other atomics that may give more (see "What is still
unproved"). So the twins no longer hand a protocol the release and acquire that hid the question, and
a pass on them is still no proof.

## What fooled us

- **A core that never reorders.** On an M1 Max, one thread storing two words 256 bytes apart and
  another loading the second with acquire and then the first, the stores showed out of order in 0.7%
  to 4.8% of rounds with the writer on a Firestorm core and a Firestorm reader, 0.2% to 0.4% with an
  Icestorm reader, and in none of 340 million rounds with the writer on an Icestorm core (2 s a
  pair, 2026-11-07). A first version of our test wrote on the Icestorm core and read that as "the
  M1 does not reorder stores". A test here that passes says little of Neoverse V2.
- **"A locked instruction is a full fence, and `ldset` is not."** Half right. Arm's current model
  orders the write of an atomic with both acquire and release before everything after it
  (`aarch64hwreqs.cat`, `bob`, `[range([R & A]; amo; [W & L])]; po`), which is why Linux uses `swpal`
  as a full barrier, and store buffering with `ldsetal` and a plain `ldr` is forbidden. That is LSE
  only: an LL/SC build (`ldaxr`, `stlxr`, which outlined atomics may fall back to) does not have the
  rule, and neither does a failed `casal`, which writes nothing. We rest on none of it, only on
  Rust's sequentially consistent operations on both sides.
- **An atomic that lets nothing pass.** On that M1 Max a plain store never passed a later
  read-modify-write of any ordering, relaxed included: none in 120 million rounds or more of `str`
  followed by each of `swp`, `ldadd`, `cas`, `ldaddl` and `ldaddal`, against 0.2% for `str`, `str` in
  the same run. So a push or an OR with its release left off passes here. Arm allows the
  reordering, and a core that performs atomics far from its store buffer may show it.
- **An address that orders the loads.** A pop reaches a magazine through a number in the head, and a
  claim a block through bits in its word, so an address dependency keeps the loads through them behind
  the read, with or without an acquire. Leaving the acquire off passes on any CPU that obeys Arm's
  rule, and it is still a bug in Rust's model, where the compiler may break the dependency.
- **A lock in the test.** The first pipeline passed blocks by a mutex, whose unlock after a consumer's
  frees orders them before a producer's next lock: the test supplied most of the order it was
  looking for.
- **The explorers' atomics.** They all behave as `SeqCst` in one interleaving whatever they say, so
  an explorer cannot tell a right ordering from a wrong one.

## What the orders cost

On Firestorm the orderings have different prices, where on x86-64 they are one
([firestorm.md](firestorm.md), "Atomics and barriers"), so a slow path asks for no more than its
argument needs. The depot's stacks use `casa` to pop and `casl` to push, 26 cycles against 42, and a
relaxed link and record, since a reload through `stlr` or `ldapr` of a word just stored is 19 cycles
against 6. A push onto an empty stack keeps `casal`, for the hint. `Word` stays sequentially
consistent everywhere: `ldsetal` costs what `ldsetl` does, `casal` a cycle more than the `casa` a
claim would need, and `ldar` what `ldapr` does, and in return the models apply to those words as they
stand. The acquire on a read of a slab word with no protection can pay the 19 cycles, once a slow
path; a relaxed read, with a second look at the mark behind a fence before an abort, would be
correct and would save it, and is not worth the code until a measurement says so.

## How it was tested

**Tests on real threads** (`crate::stress`, and `ordering` in `depot`, `back::tests`, `heap::tests`)
run the production code on threads that go round the CPUs the process may use, on the twins and on
the real routines and kernel. A ledger of two counters on lines apart in each block catches a new
owner that missed part of its last owner's writes, and a count of all takes one that missed all of
them. Rounds of a few instructions try each store-then-load pattern. The pipeline's only way from a
consumer to a producer is the allocator. They run a third of a second each in the gate, and twenty
seconds with `--all`: 34 passed in 62 s at `91c2e7e`, 2026-11-07, on an M1 Max.

**Weakened on purpose, in a throwaway copy, this CPU shows only part of it, and Miri the rest.** Each
row is one mutation against the tests that should see it, release builds, 4 s a test; Miri on 8 seeds
(`c72ea16` to `91c2e7e`, 2026-11-07).

| Weakened | M1 Max | Miri |
|---|---|---|
| Summary words, hint or sleep to release writes and relaxed reads | caught, each | not run |
| Every bitmap word relaxed | caught, by the summary test only | not run |
| `LinuxOs::fence` does nothing | caught in 3 s, twice of twice (`a_drain_leaves_each_thread_its_own_blocks`, "a spare stack names something that is not a magazine"); not in 0.3 s, and not by `collect`'s drains in 4 s | cannot run |
| A push's release, or a pop's acquire | not caught | caught |
| A free's OR with no release, or a claim with no acquire | not caught | caught |
| Summary words, hint or sleep to acquire and release | not caught | caught |
| A kept layout, or the domains' count, with no order | not caught, at thousands of rounds a second | caught |
| Tags with no order | not caught | not caught on 4 seeds: this test's readers have the carving word's order, which covers the tag |
| The simulator's barrier relaxed | not caught | not caught on 16 seeds |

Why the hardware misses so much is under "What fooled us". On the unweakened code Miri passes all 11
ordering tests it can run, on 16 seeds each (`91c2e7e` to `a99a5fb`, 2026-11-07, 1.5 CPU hours).

**The kernel's fence**, on the real kernel with no allocator: with plain stores and loads on one side,
store-then-load never failed, and a CPU's stores were never seen out of order after a fence. Left
out, the same 5 s showed both loads missing in 22,865 rounds and a later store first in 384,403
(`without_the_fence_the_hardware_shows_what_it_hides`, `91c2e7e`). `bench/probes/rseq_drain.rs` on
this kernel, 20 s at `335f834`: 525,132 calls and no late commit, and 120,393 late commits with the
call left out. Both are evidence for one kernel on one CPU. The argument is above.

**Arm's model, for the claims that rest on it**: `python3 tla/armcheck.py`, a small enumerator
transcribed from Arm's `aarch64hwreqs.cat`, because herd7 could not be installed here. It lists every
candidate execution. 14 published litmus tests get the catalogue's answers, which is the check on the
transcription, and each "allowed" row shows it can see the outcome. A transcription is not herd.

| Test | Bad outcome |
|---|---|
| `ldsetal; ldar` on both sides (the summary bits) | forbidden |
| `ldsetl; ldapr` on both sides (`AcqRel` and `Acquire`) | allowed |
| `stlr; ldapr` on both sides | allowed |
| A plain store and `ldar` on both sides (a failed compare-and-swap writes nothing) | allowed |
| The pass's start: `casl`, `stlr`, `ldar`, against `casal`, `ldar` | forbidden |
| Parking: plain store, `ldar` with a `dmb` anywhere in them, against atomic, `dmb`, interrupt, `dmb`, wait, `dmb`, read | forbidden |
| Slab word: `ldumaxal`, `ldapr` of the mark, `str` of the word, against `ldapr`, `ldapr` | forbidden |
| The same with the mark from another thread | forbidden |
| The same with a plain `ldr` of the word | allowed |
| Drain: plain slot and commit, `dmb`, handshake, against `swpal`, wait, read | forbidden |
| Drain with no barrier | allowed |
| Fence with the thread gone: stores, `dmb`, store of `rq->curr`, against `swpal`, `dmb`, its read, `dmb`, read | forbidden |
| Fence with the thread returning: store of `rq->curr`, `dmb`, read of the word, against `swpal`, `dmb`, its read | forbidden |
| Shortcut: store, atomic decrement, against `ldr` of the count, read | allowed |
| Shortcut with the acquire fence | forbidden |
| Relaxed hand-off, tag read through the pointer | forbidden |
| Relaxed hand-off, heap word | allowed |

## What is still unproved

- **No tool has explored the Rust under weak memory exhaustively.** GenMC "does not support re-using
  freed memory", and Miri tries one interleaving a seed. The chains above are an argument on paper.
- **Another core.** This M1 Max cannot show a missing release on an atomic or a missing acquire before
  a dependent load. Those rest on Miri, which does not run the real routines, and on the argument.
  `scripts/gate --all` has not been run on Neoverse V2, where atomics may behave differently.
- **The routines and Miri.** Miri sees the twins, whose order is the simulator's, and relaxing the
  simulator's barrier went unseen, so some other atomic of the simulator may order what the barrier
  should. So Miri does not show that the route through a routine holds. The real routines have only
  the tests on real threads and the reading above, and those are provisional, by hand, until Lean
  emits them.
- **The kernel's barriers are read, not proved.** The no-interrupt paths have a litmus test in Arm's
  model and no test on the kernel that isolates them.
- **The fork's cut** rests on Arm's rule for `tlbi` and `dsb`, as we read it.
- **Compiler mapping.** We trust that rustc and LLVM map Rust's orderings correctly, on LSE and on
  outlined atomics.
- **A block passed with no order** rests on address dependencies surviving the compiler in our own
  code between the load of the pointer and the loads through it, which nothing checks.
