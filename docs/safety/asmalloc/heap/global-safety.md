# Safety ledger: `crates/asmalloc/src/heap/global.rs`

The process's heap, `HEAP`, over Linux, run with `Real` on `front::SLABS` through the one
`Bound` that `process()` makes. That construction is HEAP-02's (`heap/mod-safety.md`).

## GLOBAL-01: `unsafe impl GlobalAlloc for Asmalloc`

**Obligations** (`GlobalAlloc`'s `# Safety`). Blocks are valid for their layout, aligned, and
not handed out twice while live; the methods do not unwind; `realloc` keeps the old block's
contents up to the smaller size and leaves the old block untouched when it returns null.

**Proof.** Each method is the heap's. Alignment and size: a small block is a slot of the class
of the layout, which by alignment by construction (`classes::verify`) is a multiple of the
alignment and at least the size, and slots are aligned to their class's alignment; a large one
is a slot of the layout's class or a run aligned as `alloc_run` was asked. Exclusivity: the
back end hands each slot once until it is freed, and magazines hold each free object once (the
slab invariant, `front/real-safety.md`). No unwinding: the allocator's paths never panic
(aborts go through `Os::abort`, which does not return). `realloc` is `Bound::realloc`
(`heap/resize.rs`): its lean path keeps a small block whose new size has its class, or copies
it into a block of the new class and frees it only after the copy (HEAP-07); everything else,
and a lean path whose pop missed, which has changed nothing, goes to the general path, which
builds the new layout with `Layout::from_size_align`, returning null on an invalid size
without touching the block, and frees the old block only after copying (HEAP-04).

## GLOBAL-03: `unsafe impl Allocator for Asmalloc`

**Obligations** (`Allocator`'s `# Safety`). Blocks stay valid until deallocated or resized;
any handle (all are the same ZST) may free any block; `deallocate` accepts any layout that
fits; grow and shrink keep contents and leave the old block alone on error.

**Proof.** As GLOBAL-01, with: the returned length is the class size for a small block, by the
fit rule every size from the request to it has the same class (`classes::verify`), so a
deallocation with any fitting size finds the class; for a large block the length is
`usable_size`, at most the slot or run, and the class comes from the address. That last needs
`dealloc` to treat every fitting layout of a large block as large, or a large slot would be
pushed into a magazine. It does: a block is large because its layout's class is not small
(`small(layout)` is `None`: `Bound::alloc_other`, `Bound::resize`), a fitting layout has the
same alignment and a size at least the one requested, and the class of a layout is monotone in
its size (the size is rounded up to a multiple of the alignment and the class is the smallest
at least that, both non-decreasing; `classes::verify`). So a fitting layout's class is at least
the requested one's, above every small class or none at all, and `dealloc` takes the large
path, which finds the slot or run from the address. Zero-size
layouts get a dangling, aligned pointer and are never freed (`heap/allocator.rs`). Every handle
reaches `HEAP`, so blocks may move between handles.

## GLOBAL-04: `free_slow` in `c_free_small`

**Precondition** (`Bound::free_slow`). `object` is a block of `class` this heap handed out,
which the caller owns and gives up.

**Proof.** `c_free_small`'s own contract is that, word for word; its one caller,
`c::slow::free_slow`, discharges it (SLOW-03 in `c/slow-safety.md`). `c_alloc_small` and
`c_free_small` run the heap's slow paths directly, without a pop or push in front: their
callers reach them after a generated C routine missed, and `Cache::alloc_slow` and
`Cache::free_slow` begin with the pop or push themselves. Both also write the masks the
generated C routines read (`publish_masks`): plain atomic stores of constants, which a race
repeats harmlessly. A C routine that missed left the thread's `rseq_cs` naming its descriptor,
but the instruction pointer has left the sequence, so an interruption only clears it, and every
routine the slow path runs arms its own.

## GLOBAL-05: the frees and resizes of the `GlobalAlloc` and `Allocator` methods

**Blocks.** `process().dealloc`, `.realloc`, `.deallocate`, `.grow` and `.shrink`.

**Preconditions.** For `dealloc` and `deallocate`: the block is one this heap handed out, which
the caller owns and gives up, and the layout fits it (its own, or one whose size lies between
that and the length returned, with the same alignment). For `realloc`, `grow` and `shrink`: the
same of the old block and layout, except that the caller gives the block up only if the call
succeeds; for `realloc`, also that the new size is not zero and, rounded up to the layout's
alignment, at most `isize::MAX`.

**Proof.** These are the trait methods' own contracts (`GlobalAlloc::dealloc` and `realloc`,
whose `new_size` is not zero and does not overflow `isize` when rounded up to the alignment;
`Allocator::deallocate`, `grow`, `grow_zeroed` and `shrink`),
for a block this allocator returned. Every `Asmalloc` reaches `HEAP` through `process()`, so a
block any handle returned is a block of the heap `process()` names.

Reviewed: f41dbf9
