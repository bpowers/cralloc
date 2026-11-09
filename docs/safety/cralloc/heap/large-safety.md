# Safety ledger: `crates/cralloc/src/heap/large.rs`

HEAP-03 (the clear in `alloc_large_using`) is in `heap/mod-safety.md`.

## Where a large block starts, and what it may use

The entries on large blocks rest on three facts.

- **L1: a large slot is whole pages, on a page boundary.** `place` is the only way to a large
  class in code that ships, and it takes only a class that is a multiple of the page:
  `roomy_class` classes a size it has rounded up to a page, and the fallback classes a size rounded
  up to the larger of the page and the layout's alignment. The class of a multiple of a power of
  two is a multiple of it (Kani: `every_layout_gets_an_aligned_class_that_covers_it`; test
  `a_large_block_gets_a_slot_of_whole_pages_on_every_kernel`). A large class's origin is zero and
  regions are aligned to their size, so a slot starts at a multiple of its class size. The classes
  that are not whole pages on a kernel (56 KiB on 16 KiB pages, 160 KiB on 64 KiB) are not used
  there.
- **L2: a block starts at its slot's start or at its stagger.** `Back::alloc_at` starts it at
  `Layout::stagger(slot)` only when `alloc_large_using` asked for a stagger, which it does for an
  alignment of at most `STAGGER_STEP`, and only if the size fits the class from there; else at
  the slot's start. That check is the bound, not `policy::stagger_room`, which is policy: `place`
  uses it to choose a class in which every stagger fits, and in the slot it falls back to where no
  such class fits a region, more blocks start at their slots' starts. A stagger is a multiple of
  64 and less than the slot, so with L1 a staggered block has the alignment it asked for. A block
  aligned past 64 starts at its slot's start, and `place` made its class a multiple of the
  alignment. `Layout::block_at` takes the slot and the offset back from the address and accepts
  only those two offsets.
- **L3: what a block may use stays in its slot.** `Back::room` is the class size less the
  offset. The usable size (`usable` in `back/class.rs`) runs to the end of the page that holds the
  block's last byte. By L2 that byte is inside the slot, and by L1 the slot ends on a page
  boundary, so the usable bytes end inside the slot, whose memory `region::carve` committed whole.
  `region::extent_of` counts a size from the stagger, rounded up to a page, which is at or past the
  usable end of a block at either offset, so a claim or a resize clears the clean bits under all of
  it. A run's block starts at the run, and a run is whole regions, each a multiple of a page.

## HEAP-04: the copy and the free in `Bound::resize`

**Block.** `ptr.copy_to_nonoverlapping(grant.ptr, min(old.size(), new.size()))`, then
`self.dealloc(ptr, old)`; or, when the old block is large, the one call
`back.move_large(ptr, grant.ptr, min(old.size(), new.size()), grant.zeroed, why)`, which copies the
same bytes in pieces, returns the old block's pages behind the copy and frees it
(BACK-CLASS-03 in `back/class-safety.md`); `why` says whether the block outgrew its slot,
which decides only what becomes of the freed slot. Its contract is the same three facts proved below: the old block is
live, owned and long enough; the new one is writable for as much; they do not overlap.

**Preconditions.** The source is valid for reads and the destination for writes of that many
bytes, and they do not overlap; `dealloc`'s contract (HEAP-01's) for `ptr` and `old`.

**Proof.** `resize`'s contract is `dealloc`'s for `ptr` and `old`, so the old block is live,
owned by the caller, at least `old.size()` long: a layout that fits says at most the length
returned, which L3 keeps in the slot. The new block was just allocated for `new`, or for a larger
layout with headroom that records `new.size()`, so it is at least `new.size()` long from where it
starts (L3) and owned by this thread. Two live blocks of the heap are disjoint (the back end hands
each slot or run to one owner), so the ranges do not overlap. The free happens once, after the
copy, and only on the path that moved; on the in-place path and on failure the caller keeps the
block.

`move_large` is also told whether the new block reads as zero (`grant.zeroed`). That is true
only when the back end said the slot was fresh or purged (`Block::zeroed`) or the block is a
run, which is always fresh or purged, and `Grant::zeroed` is set nowhere else. More than a
value rests on it. A wrong `true` would let a skipped zero page show the new block's old
bytes, and it would also make `Bound::grow_block` skip the clear of a zeroed grow (HEAP-08), so
that `grow_zeroed` returned bytes past the old size that are not zero. That breaks the
`Allocator` contract, and a caller that relies on it, as one that takes the zero bytes for
valid values of a type may, has undefined behaviour. So the two sources of `true` are
obligations of this entry, not conveniences: the back end's zero bit for a large slot says the
whole slot reads as zero (`region::settle_slot`, kept true by the rule that a tenant writes
nothing past its recorded extent, stated on `back::Block`, and nothing before its own start),
and a run is purged whole at its free (`back/run.rs`).

The general resize clears nothing, in place or moved. A zeroed grow is cleared by its one
caller that asks for it, `Bound::grow_block` (HEAP-08 in `heap/resize-safety.md`), from the
old layout's size to the length returned, unless the grant says the bytes past those carried
over read as zero: `grant.zeroed`, which a moved block keeps from its allocation and an
in-place one never has. The copy and `move_large` write only `[0, kept)` of the new block,
and `move_large` purges only whole pages of it under that range, so that promise still holds
when the resize returns.

## HEAP-14: `back.resize_in_place(ptr, new.size())` in `resize_large_in_place`

**Preconditions** (`Back::resize_in_place`'s). `ptr` is a live large block of this heap that
the caller owns, nothing reads or writes it during the call, and if the size shrinks the
caller gives up every byte past it.

**Proof.** `resize_large_in_place`'s contract: `ptr` is a live large block of this heap that
the caller owns and is resizing to `new`, which fits it. A block the caller owns is reached by
no other thread (it was handed to this call to be resized), and this thread does nothing else
with it during the call. Resizing to `new` is giving up what lies past `new.size()`: the call
returns the block with that size, and what the caller may use afterwards is the usable size
(`resize_large_in_place` returns it from `Heap::usable_size`), which the back end's purge of the
tail leaves alone, since it starts at the end of the page that holds the new last byte. The back
end is the heap's own (`self.back()`), the one that handed the block out.

## HEAP-15: `self.heap().free_large(ptr)` in `Bound::free_large_slow`

**Proof.** `free_large_slow`'s contract is "As for `Heap::free_large`" and it passes `ptr`
unchanged to that function of its own heap: the caller's promise (a large block of this heap
that the caller owns and gives up) is the callee's, word for word. The heartbeat after it
does not touch the block.

## HEAP-16: `heap.resize_large_in_place(ptr, new)` in `Bound::resize`

**Preconditions** (HEAP-14's caller, `resize_large_in_place`'s). `ptr` is a live large block
of this heap that the caller owns and is resizing to `new`, which fits it.

**Proof.** `resize`'s contract is `dealloc`'s with `ptr` and `old`: a live block of this heap
that the caller owns, described by `old`, given up only if the resize succeeds; here it
succeeds, since an in-place resize cannot fail. The call is made only when `in_place` holds
and `small(old)` is `None`. A layout with no small class describes a large block (GLOBAL-03),
so the block is large. `in_place`, for a large `old`, checked that `new` fits it: `new` has no
small class, its size rounded up to its alignment is at most `len`, which `block_len` took
from `Back::room` (the bytes from the block's start to its slot's or run's end, so the block's
stagger is already off it, and `Back::resize_in_place` finds the new size within the slot), the
block stays after a shrink (`stays_after_shrink`), and `ptr` is aligned as `new` asks.

Reviewed: 0a92f0b
