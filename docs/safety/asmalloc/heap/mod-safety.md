# Safety ledger: `crates/asmalloc/src/heap/mod.rs`

`Heap` is a back end and a depot. `Bound` is a heap with the routines that write its slab array,
made by one `unsafe` constructor (HEAP-02). Everything that allocates or maintains the heap is a
safe method of `Bound`; its frees and resizes are `unsafe` only because they take a block.

## HEAP-01: `NonNull::new_unchecked(ptr)` in `Bound::dealloc`

**Precondition.** `ptr` is not null.

**Proof.** `dealloc`'s contract says `ptr` is a block this heap handed out. Every block is the
non-null pointer of a slot or run (`alloc` returns null only for "no memory", and a caller
cannot free that, since it was never a block). The `GlobalAlloc` and `Allocator` contracts
pass the same promise down: `ptr` was returned by `alloc`/`allocate` (a zero-size `Allocator`
block is never passed here; `Bound::deallocate` returns first).

## HEAP-02: `Bound::new(heap, routines)`, and `Cache::new` in `Bound::cache`

**Contract of `Bound::new`** (B). The slab array of `routines` belongs to `heap`: while either
is used, the array is used with no other heap, and nothing writes it but `routines` and
`heap`'s own slow paths and drains.

**`Cache::new` in `Bound::cache`.** Its preconditions: for as long as any of them is used,
`depot` is used only with this `space`, and the slab array of `routines` only with this depot
and space. `self.heap.depot` and the back end are fields of one `Heap`, and the back end's
`State` publishes exactly one space for its whole life (`SpaceCell`), so every `back.space()`
this heap ever sees is the same reservation, and the depot is only ever paired with it. `back`
is `self.heap`'s back end (every caller passes `self.heap.start()` or `.started()` or `.back()`).
The routines' slab array is used only with this heap and its depot by (B). The source is this
heap's back end. This is the only `Cache::new` outside the depot's own tests, so every slow
path, heartbeat, collect and drain of every heap rests on (B) and nothing else.

There are two constructions, and each discharges (B).

**The process's heap** (`unsafe { Bound::new(&HEAP, Real) }` in `global::process`).
`Real::slabs()` is `front::SLABS`. `Real` is private to the crate, and `SLABS`'s type `Front`
has no public field or method, so code outside the crate can neither run the routines nor reach
the slab array to build a twin on it. Inside the crate, nothing writes `SLABS` except `Real`'s
routines (and the generated C routines, which are the same routines on the same array and
complete their misses through `c_alloc_small` and `c_free_small`, so through this `Bound`) and
the slow paths and drain of a `Cache` made from a `Bound`. The only `Bound` with `Real` is
`process()`'s, and its heap is always `HEAP`, the one `static` heap over Linux.

**A simulated heap** (`unsafe { Bound::new(&self.heap, routines) }` in `SimHeap::with`). The
routines are twins (`SimRoutines`) on `self.slabs`, a box the `SimHeap` owns, never hands out
and pairs with `self.heap`, which it also owns, for the life of the value; every `Bound` with
those slabs is made here, with that heap. Which simulated thread the twins run on (the lease)
is SIM-01's concern, not (B)'s.

## HEAP-03: `write_bytes(0, n)` on a new block (`grant_small` here, `alloc_large` in `large.rs`)

**Preconditions.** The `n` bytes at the pointer are valid for writes, and nothing else reads or
writes them concurrently.

**Proof.** The block was just handed to this thread by the heap and nobody else has it. For a
small class `n` is at most the class size: the layout's size, or the class size itself. A
small block is a whole slot of its class in carved, committed memory (the back end commits
before it hands a slot out; magazines hold only slots that `fill` handed out). For a large slot
`n` is the layout's size or `len`, both at most the class size, the slot's length. Runs are not
written. The pointer carries the reservation's provenance (`Space::slot`).

Reviewed: f41dbf9
