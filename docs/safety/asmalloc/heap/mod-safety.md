# Safety ledger: `crates/asmalloc/src/heap/mod.rs`

`Heap` joins a back end, a depot and the slab array of the routines it is run with. Its
methods that take routines are `unsafe fn`s whose contract is (H): **the slab array of
`routines` is used only with this heap.** The two callers meet it by construction: the
process heap (`global.rs`, GLOBAL-02) is run only with `Real` on `front::SLABS`, and a
`SimHeap` only with twins on its own boxed slabs (SIM-03).

## HEAP-01: `NonNull::new_unchecked(ptr)` in `Heap::dealloc`

**Precondition.** `ptr` is not null.

**Proof.** `dealloc`'s contract says `ptr` is a block this heap handed out. Every block is the
non-null pointer of a slot or run (`alloc` returns null only for "no memory", and a caller
cannot free that, since it was never a block). The `GlobalAlloc` and `Allocator` contracts
pass the same promise down: `ptr` was returned by `alloc`/`allocate` (a zero-size `Allocator`
block is never passed here; `Heap::deallocate` returns first).

## HEAP-02: `Cache::new(&self.depot, back.space(), routines, &source)` in the slow paths

**Preconditions** (`Cache::new`). For as long as any of them is used, `depot` is used only
with this `space`, and the slab array of `routines` only with this depot and space.

**Proof.** `self.depot` and `self.back` are fields of one `Heap`, and the back end's `State`
publishes exactly one space for its whole life (`SpaceCell`), so every `back.space()` this heap
ever sees is the same reservation, and the depot is only ever paired with it. The routines'
slab array is used only with this heap by (H). The source is this heap's back end.

## HEAP-03: `write_bytes(0, n)` on a new block (`alloc_grant` here, `alloc_large` in `large.rs`)

**Preconditions.** The `n` bytes at the pointer are valid for writes, and nothing else reads or
writes them concurrently.

**Proof.** The block was just handed to this thread by the heap and nobody else has it. For a
small class `n` is at most the class size: the layout's size, or the class size itself. A
small block is a whole slot of its class in carved, committed memory (the back end commits
before it hands a slot out; magazines hold only slots that `fill` handed out). For a large slot
`n` is the layout's size or `len`, both at most the class size, the slot's length. Runs are not
written. The pointer carries the reservation's provenance (`Space::slot`).
