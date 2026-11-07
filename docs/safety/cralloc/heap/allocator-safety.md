# Safety ledger: `crates/cralloc/src/heap/allocator.rs`

The `Allocator` trait's operations on a `Bound`. Each `unsafe` block passes the trait's
contract on to the heap's free or resize, after a check that the block is not the empty one.

The trait's contract (`core::alloc::Allocator`): a block passed in is *currently allocated*
by this allocator and the layout passed with it *fits* it: the same alignment it was
allocated with, and a size from the one asked for up to the length `allocate` returned. Here
that length is the usable size (`Heap::usable_size`: the class size for a small block, the
size asked for rounded up to a page for a large one). A zero-size layout gets a dangling
pointer and no block (`dangling`), so a zero-size "block" is never passed on.

## ALLOCATOR-01: `self.dealloc(ptr.as_ptr(), layout)` in `deallocate`

**Preconditions** (`Bound::dealloc`'s). `ptr` is a block this heap handed out that the caller
owns and gives up, described by `layout`: the layout it was allocated with, or one with the
same alignment and a size between that one's and the length the heap returned.

**Proof.** `deallocate`'s contract is the trait's for `deallocate`: `ptr` is currently
allocated and `layout` fits it, and the caller gives it up. The call is made only for a
non-zero `layout.size()`; a layout that fits a block and is not empty describes a block that
was allocated with a non-zero layout (fitting keeps the size between the request's and the
returned length, and a zero-size request returned no length), so `ptr` came from
`alloc_grant`, a block this heap handed out. "Fits" is then `dealloc`'s description word for
word, with the length returned being the usable size.

## ALLOCATOR-02: `self.grow_block(ptr, old, new, zero)` in `grow`

**Preconditions** (`grow_block`'s). `Allocator::grow`'s contract for a block of this heap,
with `old` not empty.

**Proof.** `grow`'s contract is `Allocator::grow`'s for a block of this heap, passed with
unchanged arguments. The function checks the one thing `grow_block` adds: an empty `old`
returns before the call (it is no block, and a fresh allocation serves the grow).

## ALLOCATOR-03: the calls in `shrink`

**Blocks.** `self.deallocate(ptr, old)` when `new` is empty; `self.shrink_block(ptr, old,
new)` otherwise.

**Proof.** `shrink`'s contract is `Allocator::shrink`'s for a block of this heap: `ptr` is
currently allocated, `old` fits it, `new.size() <= old.size()`, and on success the caller
gives the old block up.

- `new` empty: the result is the empty block, so the old block is given up, and `ptr` and
  `old` are what `deallocate` asks (currently allocated, fitting). An `old` that is itself
  empty is no block; `deallocate` then frees nothing (ALLOCATOR-01's check).
- `new` not empty: `shrink_block`'s contract is `Allocator::shrink`'s with `new` not empty,
  which the branch has just checked.

Reviewed: c18c19d
