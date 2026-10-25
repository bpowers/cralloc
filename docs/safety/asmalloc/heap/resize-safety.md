# Safety ledger: `crates/asmalloc/src/heap/resize.rs`

The lean path of a resize: a small block resized to a small class, by the pop and push
routines, with the general resize (`large.rs`, HEAP-04) for everything else. The calls into the
general resize and into the free's slow paths pass their callers' contracts on unchanged, and
say so where they are made.

## HEAP-06: `NonNull::new_unchecked(ptr)` in `Bound::realloc`

**Precondition.** `ptr` is not null.

**Proof.** `Bound::realloc`'s contract is `GlobalAlloc::realloc`'s, which its two callers
(`Asmalloc` and the simulated heaps) pass on: `ptr` is a block this allocator handed out, and
a block is the non-null pointer of a slot or run (HEAP-01). The new size is looked up by
itself (`SmallClass::of_nonzero`), with no `Layout`, so a size that breaks `realloc`'s rule
builds nothing invalid: it has no small class, and the general path checks it with
`Layout::from_size_align` and returns null, as before.

## HEAP-07: the copy and the push in `Bound::resize_small`

```rust
ptr.as_ptr().copy_to_nonoverlapping(moved.as_ptr(), kept);
if !self.routines.push(from, ptr) { /* the free's slow path for `from` */ }
```

**Preconditions.** `ptr` is readable and `moved` writable for `kept` bytes, and the ranges do
not overlap. The push, and the slow path after a push that misses, take a block of `from`
that this heap handed out and that the caller owns and gives up.

**Invariants and facts.**

- `resize_small`'s contract: `ptr` is a live block of class `from` that this heap handed out,
  owned by the caller, who gives it up if the call returns another block; `kept` is at most
  the size of `from` and of `to`.
- `moved` is the pointer the pop routine just returned for `to`, exactly as an allocation of
  `to` gets it (`Bound::alloc_small`): a free object of `to`'s class that the pop handed to
  this thread alone (GLOBAL-01's exclusivity: magazines hold each free object once, and a pop
  commits its removal), a whole slot of that class in committed memory (`back/region.rs`,
  `carve`), with the reservation's provenance taken back from its exposed address (REAL-03 in
  `front/real-safety.md`). Nothing else holds it until this thread gives it away.
- Each caller discharges the contract, and its `SAFETY` comment cites this entry.
  `Bound::realloc`, `grow_block` and `shrink_block`: the
  caller's block is described by `old` as `dealloc` takes it, so it is a slot of the small
  class `of_nonzero_layout(old)` gives, `from` (the fit rule, as in `Bound::dealloc`); `kept`
  is `min(old.size(), new size)`, at most `old.size() <= from.size()` and at most the new
  size, which is at most `to.size()` (a class is at least the size it is found for). C's
  `realloc` (MOD-05 in `c/mod-safety.md`): `from` is the class the region table gives for
  the slot `ptr` starts (`Back::small_slot`, F1 and F2 in `c/slow-safety.md`), and `kept` is
  `min(from.size(), size)` with `to` the class of `size`.

**Proof.** The copy runs only when `from != to`. Readable: `kept <= from.size()`, and the
block is a whole slot of `from`, committed. Writable: `kept <= to.size()`, and `moved` is a
whole slot of `to`, committed and this thread's. Not overlapping: `ptr` is live and `moved`
was free, and the heap never holds a free object that overlaps a live block (GLOBAL-01's
exclusivity; slots of one region do not overlap, and the two classes differ, so the two slots
are in different regions altogether). The bytes are copied as bytes; no typed value is read.

The old block is given up only after the copy, and only on this path, where the call returns
`moved`, another block, so the caller has given it up by the contract. The push stores the
pointer in a magazine of `from`'s class, which keeps the slab invariant because the block is
a block of `from` that nothing else refers to now. A push that misses has stored nothing,
and the block goes to `rust_free_slow` or `free_slow`, whose contract is the same (GLOBAL-04's
for `free_slow`). On the in-place path (`from == to`) and when the pop misses, nothing is
copied or freed, and the caller keeps the block.

## HEAP-08: the clear of a zeroed grow in `Bound::grow_block`

```rust
grant.ptr.as_ptr().add(old.size()).write_bytes(0, grant.len - old.size())
```

**What it is for.** `Allocator::grow_zeroed` as stabilised in Rust 1.100: after the call,
bytes `0..old.size()` hold what the old block held, and bytes `old.size()..len` are zero,
where `len` is the length returned, which may be more than `new.size()`. The block the caller
was given may be longer than `old.size()` (a small class's slack, a large block's size rounded
up to a page, or a fitting layout smaller than the length returned), and the caller may have
written all of it. So a byte past `old.size()` is known to be zero only if the block moved
into memory known to read as zero. The earlier, unstable wording let the old block's slack be
kept; the stable one does not.

**Preconditions.** `[grant.ptr + old.size(), grant.ptr + grant.len)` is valid for writes and
nobody else uses it.

**Invariants and facts.**

- This is the only place a grow clears. The lean path (`resize_small`) and the general resize
  (`Bound::resize`, `resize_large_in_place`) copy and record but clear nothing, and every
  `grow_zeroed` of a non-empty block, from `Asmalloc`, the simulated heaps and the tests'
  hooks, is `grow_block`. A grow of an empty block is an allocation with `Zero::Whole`
  (`heap/allocator.rs`), which clears, or knows to be zero, all `len` bytes (HEAP-03).
- `Allocator::grow`'s contract: `old` fits the live block at `ptr`, which the caller gives up
  if the call succeeds, and `new.size() >= old.size()`. Every path returns a grant whose `len`
  is at least `new.size()`, so `old.size() <= len` and the length written does not wrap.
- `Grant::zeroed` on a resize's result means that the bytes past those carried over read as
  zero. It is true only for a block the general resize moved into an allocation that was fresh
  or purged (`Block::zeroed`) or a run; the lean path and both in-place paths set it `false`.

**Proof, by path.**

- *Small, same class* (the lean path, where `resize_small` returns `ptr`; or the general
  resize's in-place branch for a small block, which the lean path leaves unreached): the grant
  is the caller's own slot, `len` its class size, `zeroed` false. The slot is committed whole
  and is the caller's alone for the call, and `old.size() <= len` by the fit rule. The clear
  covers the slack the caller may have written.
- *Small to another small class* (the lean path, moved): the grant is `moved` of HEAP-07, a
  whole committed slot of `to`, `len = to.size()`, held by this thread alone until it returns.
  The copy wrote `[0, kept)` with `kept = min(old.size(), new.size()) = old.size()`; the clear
  starts there.
- *Large, in place* (`resize_large_in_place`): the caller's own block, `len` the usable size
  after `Back::resize_in_place` recorded the new size, `zeroed` false. That range lies inside
  the slot or run and is committed: the block's start plus the new size fits `Back::room`,
  and a large slot's size and a run's length are multiples of the page, so the size rounded up
  to a page fits too (large classes are `q << e` with `2^e` at least 8 KiB, pages are at most
  4 KiB wherever a large class's slot fits, runs are whole regions). A grow purges nothing
  (`resize_in_place` returns pages only when a block shrinks), and purged memory would stay
  writable and read as zero anyway. The clear covers `old.size()` to the old usable size,
  which the caller may have written, and the pages past it, which an earlier tenant may have.
- *Moved by the general resize* (to or from a large block, to a new alignment, or after a
  lean pop that missed): the grant is a block just allocated for `new` (or for a layout with
  headroom), this thread's alone, `len` its usable size, and the copy or `move_large` wrote
  `[0, kept)` of it with `kept = old.size()`, as above (HEAP-04). When `zeroed`, the bytes
  `[kept, len)` read as zero before the copy, which wrote none of them, and `move_large`
  purges only whole pages of the destination under the copied range; nothing needs clearing
  and none is done. Otherwise the clear covers `[old.size(), len)`.

In every path the bytes `0..old.size()` are the old block's: in place they were never
written, and moved they were copied before the clear, which starts at `old.size()`.
