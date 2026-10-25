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

**The clearing of a zeroed grow.** `grant.ptr.add(kept).write_bytes(0, grant.len - kept)`,
when the grow must zero and the new block is not known to read as zero. `grant.len` is the
new block's usable length, so `[kept, len)` is inside the block this thread was just given,
and the copy (or `move_large`) wrote only `[0, kept)` of it, before the clearing.

## HEAP-05: `ptr.add(before).write_bytes(0, len - before)` in `resize_large_in_place`

**Preconditions.** `[ptr + before, ptr + len)` is valid for writes and nobody else uses it.

**Proof.** The block is the caller's live large block (`resize`'s contract), resized in place
because the new size fits its slot or run. `len` is `Back::usable_size` after
`Back::resize_in_place` recorded the new size: the bytes from the block's start to the end
of the page the new size ends in. The write
happens only when `len > before`, a grow, and `resize_in_place` purges only when the block
shrinks (the pages between the new usable size and the old one), so a grow purges nothing and
`[ptr + before, ptr + len)` is still committed, and owned only by the caller, when it is
written. (A purge would not make the write unsound anyway: purged memory stays writable and
reads as zero.) The block starts at its slot's start or its stagger, and the stagger plus the
new size fits the slot (the in-place test compares against `Back::room`, the bytes from the
block to the slot's end), and a large slot's size and a run's length are
multiples of the page size (large classes are `q << e` with `2^e` at least 8 KiB, pages are at most 4 KiB in
every geometry that hosts a large class's slot, and runs are whole regions), so the rounded
size does too, and the range lies in the block, which is committed whole (a carved slot's
pages, or a committed run). `before < len`.

Reviewed: f41dbf9
