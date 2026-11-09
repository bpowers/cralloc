# Safety ledger: `crates/cralloc/src/depot/age.rs`

## AGE-01: `store_if_equal(Field::hot(class), seen.word, null)` in `Cache::reclaim`

Used by the age limit's look (`age_look`) and by `collect`'s emptying of the caller's own slab
(`collect.rs`, `empty_own`). Both pass a `Loaded` made by `Cache::hot_magazine`.

**Precondition** (`Routines::store_if_equal`). `new` keeps the slab invariant for the field:
for a hot word, zero, a marker, or a slot address of a magazine the caller holds.

**Proof.** `new` is null, which is zero, the "no magazine" value every routine and slow path
accepts for a hot word: the pop and push routines fail on it (`w & mask == 0` and
`(w + 8) & mask == 0`), and the slow paths classify it as `Seen::Zero`. Nothing the routines
later dereference comes from it.

**What the thread then owns, and why the slots it reads are its own.** This is not part of the
routine's precondition, but the block after the commit relies on it: it records the level in
the magazine's last slot, reads slots `1..=level` and hands their objects to the source.

- `seen.word` was read from a hot word of `class` and is neither zero nor a marker
  (`hot_magazine` returns `None` for both), so by the slab invariant it was then the address of
  slot `level + 1` of a magazine of the class's size that nothing else referred to, with slots
  `1..=level` holding free objects of the class that appear nowhere else. `Area::loaded`
  admitted it (MAGAZINE-01), so the handle's slot accesses are in bounds whatever happens.
- The routine commits only if the field of the CPU it runs on holds exactly `seen.word` at the
  commit, inside one restartable sequence (REAL-03). By the slab invariant at that
  moment, a hot word equal to `seen.word` is the only reference to that magazine and encodes the
  same level. The commit replaces it with zero, so afterwards nothing in any slab, spare stack
  or depot stack refers to the magazine: it is this thread's alone, with slots `1..=level`
  holding objects nothing else holds. This holds whichever CPU the routine ran on (the thread
  may have moved after reading the word), and whatever the magazine went through between the
  read and the commit, since only the state at the commit matters.
- The thread sees what the slots hold, though the routines order nothing. Whoever filled the
  magazine installed it on this CPU, and the pushes and pops since were made here, so each of
  those threads' stores is behind a context switch's full barrier, or in this thread's program
  order, by the time this commit runs (`docs/research/memory-ordering.md`, "What stands in for a
  release"). A drain that backs out puts back a value this CPU's threads wrote, and changes no
  slot.
- A drain racing the take is the case `Drain.tla` already covers for a slow path's install. A
  sequence that starts after the drain's fence reads the marker and does not commit. One in flight
  across the exchange is aborted by the fence, or commits over the marker, and then the drain's
  step 4 finds a non-marker and takes nothing. If the fence fails instead, step 3 puts each old
  value back with a compare-and-swap against its marker; a take that committed zero over the marker
  makes that compare-and-swap fail, so the drain writes nothing back, which is right: the magazine
  is the taker's. Either way the magazine has one owner.
- A fork at any instruction gives the child a slab that keeps the invariant, each lost thread's
  stores being a prefix of its order (`Fork.tla`; `docs/research/memory-ordering.md`, "The fork").
  A take the fork cut off before its commit did nothing. One cut off after it leaves a magazine
  nothing refers to, lost to the child.
- If the routine does not commit, the thread never touches the magazine's slots.

`interleave.rs` checks the take against a mutator (with migrations) and against a drain under
every schedule within its bounds, with the census after each execution.

## AGE-02: `store_if_equal(Field::spare(class, Spare::Full), head, null)` in `Cache::take_spares_keeping`

The age limit's sweep of a CPU's full spare stacks.

**Precondition** (`Routines::store_if_equal`). `new` keeps the slab invariant for the field:
for a spare head, zero, a marker, or the slot-0 address of a magazine on that stack whose link
keeps the invariant.

**Proof.** `new` is null, the empty stack, which the stack routines accept: the pop fails with
`Empty` on it and the push links a magazine above it.

**What the thread then owns.** The block after the commit walks the chain, reading each link
before it writes the magazine's link or sends the magazine on. It relinks the top `keep`
magazines into a private list through their own link words and pushes them back on the stack
afterwards with the stack-push routine (by `spare_push`, SPARE-01), and launders the first of
the rest and sends them to the depot. Writing a link is the thread's to do only because the
chain is its own, which the points below establish; a link is read before it is written, so
the walk follows the chain the commit took.

- `head` was read from the full spare head of `class` and is neither zero nor a marker, so by
  the slab invariant it was then the slot-0 address of the top magazine of that stack.
- The routine commits only if the head of the CPU it runs on is exactly `head` at the commit,
  inside one restartable sequence. By the invariant at that moment that head is the only
  reference to the top magazine, and each magazine's link is the only reference to the one below
  it; stack pushes and pops change a head only by routines, so nothing else refers into the
  chain. The commit replaces the head with zero, so the whole chain, as its links stand at the
  commit, is this thread's. Each magazine is turned into a handle by `Area::magazine`, which
  admits only committed, aligned magazine memory (MAGAZINE-01), so a corrupt link aborts and
  never reads outside the area.
- The head may have been popped and pushed back between the read and the commit, with another
  chain below it: the walk reads the links after the commit, so it follows the chain the commit
  took, not the one the read saw. A thread that moved takes the stack of the CPU it is on; the
  argument does not depend on which CPU that is.
- The walk sees the links and records as the stack's pushes left them, for the reason AGE-01 gives
  for slots: each push ran on this CPU, so its stores are behind a context switch or in this
  thread's program order.
- A drain racing the take is the same case as AGE-01's, for a spare head. A commit that lands,
  before the drain's exchange or over its marker, takes the chain, and the drain's steps 3 and 4
  take nothing and write nothing over it. Otherwise the routine finds the marker or is aborted by
  the fence, and the chain is the drain's, or goes back on the CPU if the fence failed.
- A fork is as in AGE-01: a chain the commit took and the fork cut off is lost to the child.
- If the routine does not commit, nothing is read from or written to the chain.

Reviewed: 5a8ade1
