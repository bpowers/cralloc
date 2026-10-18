# Safety ledger: `crates/asmalloc/src/depot/age.rs`

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
  commit, inside one restartable sequence (TWIN-04 / REAL-03). By the slab invariant at that
  moment, a hot word equal to `seen.word` is the only reference to that magazine and encodes the
  same level. The commit replaces it with zero, so afterwards nothing in any slab, spare stack
  or depot stack refers to the magazine: it is this thread's alone, with slots `1..=level`
  holding objects nothing else holds. This holds whichever CPU the routine ran on (the thread
  may have moved after reading the word), and whatever the magazine went through between the
  read and the commit, since only the state at the commit matters.
- A drain racing the take is the case `Drain.tla` already covers for a slow path's install: if
  the drain's exchange put its marker in first, the routine reads the marker and does not
  commit; if the routine's sequence was in flight across the exchange and committed over the
  marker, the drain's step 4 finds a non-marker and takes nothing. If the drain's fence fails
  instead, its step 3 puts each old value back with a compare-and-swap against its marker; a take
  that committed zero over the marker makes that compare-and-swap fail, so the drain writes
  nothing back, which is right: the magazine is the taker's. Either way the magazine has one
  owner.
- If the routine does not commit, the thread never touches the magazine's slots.

`interleave.rs` checks the take against a mutator (with migrations) and against a drain under
every schedule within its bounds, with the census after each execution.

## AGE-02: `store_if_equal(Field::spare(class, Spare::Full), head, null)` in `Cache::take_spares`

The age limit's sweep of a CPU's full spare stacks.

**Precondition** (`Routines::store_if_equal`). `new` keeps the slab invariant for the field:
for a spare head, zero, a marker, or the slot-0 address of a magazine on that stack whose link
keeps the invariant.

**Proof.** `new` is null, the empty stack, which the stack routines accept: the pop fails with
`Empty` on it and the push links a magazine above it.

**What the thread then owns.** The block after the commit walks the chain, reading each link
before sending the magazine on, and launders the top one.

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
- A drain racing the take is the same case as AGE-01's, for a spare head: whichever of the
  drain's exchange and the routine's commit lands first wins the chain, and the drain's steps 3
  and 4 write nothing over a commit they did not make.
- If the routine does not commit, nothing is read from or written to the chain.
