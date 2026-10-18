# Safety ledger: `crates/asmalloc/src/heap/large.rs`

HEAP-03 (the clear in `alloc_large`) is in `heap/mod-safety.md`.

## HEAP-04: the copy and the free in `Heap::resize`

**Block.** `ptr.copy_to_nonoverlapping(grant.ptr, min(old.size(), new.size()))`, then
`self.dealloc(routines, ptr, old)`; or, when the old block is large, the one call
`back.move_large(ptr, grant.ptr, min(old.size(), new.size()))`, which copies the same bytes
in pieces, returns the old block's pages behind the copy and frees it (BACK-CLASS-03 in
`back/class-safety.md`). Its contract is the same three facts proved below: the old block is
live, owned and long enough; the new one is writable for as much; they do not overlap.

**Preconditions.** The source is valid for reads and the destination for writes of that many
bytes, and they do not overlap; `dealloc`'s contract (H and HEAP-01's) for `ptr` and `old`.

**Proof.** `resize`'s contract is `dealloc`'s for `ptr` and `old`, so the old block is live,
owned by the caller, at least `old.size()` long. The new block was just allocated for `new`
(or for a larger layout with headroom), so it is at least `new.size()` long and owned by this
thread. Two live blocks of the heap are disjoint (the back end hands each slot or run to one
owner), so the ranges do not overlap. The free happens once, after the copy, and only on the
path that moved; on the in-place path and on failure the caller keeps the block.

## HEAP-05: `ptr.add(before).write_bytes(0, len - before)` in `resize_large_in_place`

**Preconditions.** `[ptr + before, ptr + len)` is valid for writes and nobody else uses it.

**Proof.** The block is the caller's live large block (`resize`'s contract), resized in place
because the new size fits its slot or run. `len` is `Back::usable_size` after
`Back::resize` recorded the new size: the new size rounded up to a page. The new size fits the
slot or run (the in-place test), and a large slot's size and a run's length are multiples of
the page size (large classes are `q << e` with `2^e` at least 8 KiB, pages are at most 4 KiB in
every geometry that hosts a large class's slot, and runs are whole regions), so the rounded
size does too, and the range lies in the block, which is committed whole (a carved slot's
pages, or a committed run). `before < len`.
