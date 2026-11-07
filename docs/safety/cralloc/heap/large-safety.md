# Safety ledger: `crates/cralloc/src/heap/large.rs`

HEAP-03 (the clear in `alloc_large`) is in `heap/mod-safety.md`.

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
owned by the caller, at least `old.size()` long. The new block was just allocated for `new`
(or for a larger layout with headroom), so it is at least `new.size()` long and owned by this
thread. Two live blocks of the heap are disjoint (the back end hands each slot or run to one
owner), so the ranges do not overlap. The free happens once, after the copy, and only on the
path that moved; on the in-place path and on failure the caller keeps the block.

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
nothing past its recorded extent, stated on `back::Block`), and a run is purged whole at its
free (`back/run.rs`).

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
returns the block with that size, which is all the caller may use afterwards
(`resize_large_in_place` returns `Back::usable_size` after the call). The back end is the
heap's own (`self.back()`), the one that handed the block out.

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
from `Back::room` (the bytes from the block's start to its slot's or run's end), the block
stays after a shrink (`stays_after_shrink`), and `ptr` is aligned as `new` asks.

Reviewed: c18c19d