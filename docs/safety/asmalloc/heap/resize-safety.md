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

## HEAP-08: `moved.add(kept).write_bytes(0, to.size() - kept)` in `Bound::grow_block`

**Preconditions.** `[moved + kept, moved + to.size())` is valid for writes and nobody else
uses it.

**Proof.** The write happens only when `resize_small` returned a block other than `ptr`, which
is then `moved` of HEAP-07: a whole committed slot of `to`, `to.size()` bytes, held by this
thread alone until `grow_block` returns it. `kept <= to.size()` (HEAP-07), so the range lies
inside the slot, and the copy wrote only `[0, kept)` of it, before. This is the same clearing
the general resize does for a small block (HEAP-04, "The clearing of a zeroed grow"), whose
grant is never known to be zero and whose length is the class size.
