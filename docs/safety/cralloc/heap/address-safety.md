# Safety ledger: `crates/cralloc/src/heap/address.rs`

The C layer's operations on blocks known by their address. Each `unsafe` block either resizes a
block the caller holds after checking that it fits, or passes a caller's contract on to the
function on the process's heap whose contract it restates.

## ADDRESS-01: `back.resize_in_place(ptr, new)` in `Heap::resize_in_place_by_address`

**Preconditions** (`Back::resize_in_place`'s). `ptr` is a live large block of this heap that
the caller owns; nothing reads or writes it during the call; if `new` is smaller than its
recorded size, the caller gives up every byte past `new`.

**Proof.** The function's contract: `ptr` is a live large block of this heap that the caller
owns and is resizing. The function adds its own checks before the call, which only narrow
when it is made: the back end is this heap's (`self.back()`, and without one it returns
`false` and calls nothing); `back.lookup(ptr)` finds a slot of a class that is not small, or
a run, and not a small slot or nothing (both return `false` before the call); and `fits`
holds: `new` has no small C class, `new <= room` with `room` the bytes from the block to its
slot's or run's end (`Back::room`), and a shrink keeps a large part of the room
(`stays_after_shrink`). A block the caller owns is reached by no other thread, and this one
does nothing else with it meanwhile. Resizing to `new` gives up what lies past it: the C
caller (`realloc_other`, MOD-04) treats the block as `new` bytes from then on.

## ADDRESS-02: the pass-through calls in `process`

**Blocks.** `global::process().free_large_slow(ptr)` in `free_large`;
`global::process().resize_small(ptr, from, to, kept, Caller::C)` in `resize_small`;
`global::process().heap().resize_in_place_by_address(ptr, old, new)` in
`resize_in_place_by_address`.

**Proof.** Each function's `# Safety` is its callee's, and the arguments are passed unchanged,
to the process's heap, which is the heap the C layer's blocks come from (`global::process`,
HEAP-02: the one static heap):

- `free_large`: "`ptr` is a large block of the heap that the caller owns and gives up", which
  is `Heap::free_large`'s, the contract `Bound::free_large_slow` takes ("As for").
- `resize_small`: "As for `Bound::resize_small`". The added argument, `Caller::C`, chooses
  only how a missed push completes (HEAP-11) and carries no precondition.
- `resize_in_place_by_address`: "As for `Heap::resize_in_place_by_address`".

## ADDRESS-03: `back.move_large(from, to, len, false, why)` in `process::move_large`

**Preconditions** (`Back::move_large`'s). `from` is a large block of this heap that the caller
owns and gives up, readable for `len` bytes; `to` is a block of this heap the caller owns,
writable for `len` bytes, whose contents it does not need, and does not overlap `from`;
nothing else reads or writes either range during the call; if `zeroed`, `to` reads as zero.

**Proof.** The function's `# Safety` is "As for `Back::move_large`", followed by a summary of
it, so the caller has promised all of the above. The back end is the process heap's, which
both blocks belong to; if no heap exists the function aborts and calls nothing. It adds one
argument, `zeroed`, as `false`, which asks nothing of `to`: the back end then never leaves out
a page of `to` on the strength of it reading as zero (it purges the part under any zero source
page first, BACK-CLASS-03).

Reviewed: 502257d
