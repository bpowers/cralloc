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

One construction ships, and it discharges (B); the simulated heap's (`heap/sim.rs`) is test
scaffolding, outside the ledger.

**The process's heap** (`unsafe { Bound::new(&HEAP, Real) }` in `global::process`).
`Real::slabs()` is `front::SLABS`. `Real` is private to the crate, and `SLABS`'s type `Front`
has no public field or method, so code outside the crate can neither run the routines nor reach
the slab array to build a twin on it. Inside the crate, nothing writes `SLABS` except `Real`'s
routines (and the generated C routines, which are the same routines on the same array and
complete their misses through `c_alloc_small` and `c_free_small`, so through this `Bound`) and
the slow paths and drain of a `Cache` made from a `Bound`. The only `Bound` with `Real` is
`process()`'s, and its heap is always `HEAP`, the one `static` heap over Linux.

## HEAP-03: `write_bytes(0, n)` on a new block (`grant_small` here, `alloc_large` in `large.rs`)

**Preconditions.** The `n` bytes at the pointer are valid for writes, and nothing else reads or
writes them concurrently.

**Proof.** The block was just handed to this thread by the heap and nobody else has it. For a
small class `n` is at most the class size: the layout's size, or the class size itself. A
small block is a whole slot of its class in carved, committed memory (the back end commits
before it hands a slot out; magazines hold only slots that `fill` handed out). For a large slot
`n` is the layout's size or `len`, both at most the class size, the slot's length. Runs are not
written. The pointer carries the reservation's provenance (`Space::slot`).

## HEAP-11: the slow free after a push that missed (`free_small` here, `resize_small` in `resize.rs`)

**Blocks.** `self.rust_free_slow(class, object)` in `free_small`; `self.rust_free_slow(from,
ptr)` and `self.free_slow(from, ptr)` in `resize_small`.

**Preconditions** (`free_slow`'s, which `rust_free_slow` takes as its own). The block is of
the class passed, this heap handed it out, and the caller owns and gives it up.

**Proof.** The push routine's contract (REAL-03) is that a push that returns `false` has
stored nothing: the block is in no magazine, and no other thread can have it. So after a miss
the block is exactly as the caller held it before the push.

- In `free_small`, the contract is `free_small`'s own: `object` is a block of `class` this
  heap handed out, which the caller owns and gives up. That is `free_slow`'s, word for word.
- In `resize_small`, the block `ptr` is of `from` and this heap's (its contract), and the
  only use of it since was the read of `kept` bytes out of it (HEAP-07), which changes
  nothing. The function then returns `moved`, a block popped from a magazine of `to`, so
  another block than `ptr` (a magazine never holds a block its owner still holds), and the
  contract says the caller gives `ptr` up when another block is returned. So the caller
  gives `ptr` up here, which `free_slow` asks.

## HEAP-12: the pass-through calls in `rust_free_slow` and `dealloc`

**Blocks.** `self.free_slow(class, object)` in `rust_free_slow`; `self.dealloc_other(object,
layout)` in `dealloc`.

**Proof.** Each callee's contract is the caller's, and the arguments are passed unchanged.

- `rust_free_slow`'s `# Safety` is "As for `Bound::free_slow`", with the same `class` and
  `object`.
- `dealloc_other`'s is "As for `Bound::dealloc`". `dealloc` passes `object`, which is its
  `ptr` made `NonNull` (HEAP-01), and its own `layout`; it adds nothing and checks nothing
  further, since `dealloc_other` decides the class itself.

## HEAP-13: the class passed to `free_small` in `dealloc` and `dealloc_other`

**Precondition** (`free_small`'s). `object` is a block of `class` this heap handed out,
which the caller owns and gives up.

**Proof.** The caller's contract (`dealloc`'s) is that `object` is a block this heap handed
out, which the caller owns and gives up, described by `layout`: the layout it was allocated
with, or one with the same alignment and a size between that one's and the length the heap
returned. The class passed is the small class of `layout`: `SmallClass::of_nonzero_layout`
in `dealloc`, `small(layout)` in `dealloc_other` (the same class function for a layout the
inlined lookup turns away; `small` is `Class::of_layout` restricted to small classes). A
block allocated with a layout of a small class is a slot of that layout's class, since the
allocation took its class from the same function of the layout (`Bound::alloc`, `alloc_other`,
`alloc_grant`). And every layout that describes the block has that same class: the fit rule,
proved by Kani for every layout (`classes::verify`,
`every_layout_up_to_the_class_size_has_the_same_class`), says a layout with the same alignment
and any size from the request's up to the class size has the class of the request, and the
length the heap returns for a small block is its class size. So `object` is a block of
`class`.

Reviewed: 502257d