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
  marker, the drain's step 4 finds a non-marker and takes nothing. Either way the magazine has
  one owner.
- If the routine does not commit, the thread never touches the magazine's slots.

`interleave.rs` checks the take against a mutator (with migrations) and against a drain under
every schedule within its bounds, with the census after each execution.
