# Safety ledger: `crates/asmalloc/src/heap/large.rs`

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
run, which is always fresh or purged; a wrong `true` would let a skipped zero page show the
new block's old bytes, which is a wrong value, not a memory error, and `Grant::zeroed` is set
nowhere else.

The general resize clears nothing, in place or moved. A zeroed grow is cleared by its one
caller that asks for it, `Bound::grow_block` (HEAP-08 in `heap/resize-safety.md`), from the
old layout's size to the length returned, unless the grant says the bytes past those carried
over read as zero: `grant.zeroed`, which a moved block keeps from its allocation and an
in-place one never has. The copy and `move_large` write only `[0, kept)` of the new block,
and `move_large` purges only whole pages of it under that range, so that promise still holds
when the resize returns.

Reviewed: f41dbf9
