# Safety ledger: `crates/cralloc/src/heap/global.rs`

The process's heap, `HEAP`, over Linux, run with `Real` on `front::SLABS` through the one
`Bound` that `process()` makes. That construction is HEAP-02's (`heap/mod-safety.md`).

## GLOBAL-01: `unsafe impl GlobalAlloc for Cralloc`

**Obligations** (`GlobalAlloc`'s `# Safety`). Blocks are valid for their layout, aligned, and
not handed out twice while live; the methods do not unwind; `alloc_zeroed`'s block reads as
zero for the layout's size; `realloc` keeps the old block's contents up to the smaller size
and leaves the old block untouched when it returns null.

**Proof.** Each method is the heap's. Alignment and size: a small block is a slot of the class
of the layout, which by alignment by construction (`classes::verify`) is a multiple of the
alignment and at least the size, and slots are aligned to their class's alignment; a large one
starts at its slot's start or stagger, with the alignment and the size asked for from there (L1
to L3 in `heap/large-safety.md`), or is a run aligned as `alloc_run` was asked. Exclusivity: the
back end hands each slot once until it is freed, and magazines hold each free object once (the
slab invariant, `front/real-safety.md`). No unwinding: the allocator's paths never panic
(aborts go through `Os::abort`, which does not return). `alloc_zeroed` is an allocation with
`Zero::Requested`, which clears the layout's `size()` bytes, or skips a large slot or run the
back end knows to be zero (HEAP-03). `realloc` is `Bound::realloc`
(`heap/resize.rs`): its lean path keeps a small block whose new size has its class, or copies
it into a block of the new class and frees it only after the copy (HEAP-07); everything else,
and a lean path whose pop missed, which has changed nothing, goes to the general path, which
builds the new layout with `Layout::from_size_align`, returning null on an invalid size
without touching the block, and frees the old block only after copying (HEAP-04).

## GLOBAL-03: `unsafe impl Allocator for Cralloc`

**Obligations** (`Allocator`'s `# Safety`). Blocks stay valid until deallocated or resized;
any handle (all are the same ZST) may free any block; `deallocate` accepts any layout that
fits; grow and shrink keep contents and leave the old block alone on error; and the methods
zero what their documentation says (the trait's `# Safety` asks that they be correct in this
too). As stabilised in Rust 1.100: `allocate_zeroed` returns a slice that is zero over its
whole length, past the layout's size included; `grow_zeroed` keeps bytes `0..old.size()` and
returns bytes `old.size()..len` zero, where `len` is the length it returns, so the old block's
slack, which the caller may have written, is zeroed too.

**Proof.** As GLOBAL-01, with: the returned length is the class size for a small block, by the
fit rule every size from the request to it has the same class (`classes::verify`), so a
deallocation with any fitting size finds the class; for a large block the length is
`usable_size`, which ends inside the slot or run (L3 in `heap/large-safety.md`), and the class
comes from the address. That last needs `dealloc` to treat every fitting layout of a large block
as large, or a large slot would be pushed into a magazine. It does: a block is large because its
layout's class is not small (`small(layout)` is `None`: `Bound::alloc_other`, `grant_other`,
`Bound::resize`), a fitting layout has the same alignment and a size at least the one requested,
and the class of a layout is monotone in its size (the size is rounded up to a multiple of the
alignment and the class is the smallest at least that, both non-decreasing; `classes::verify`).
So a fitting layout's class is at least the requested one's, above every small class or none at
all, and `dealloc` takes the large path, `self.heap.free_large(object)` in `dealloc_other`, whose
comment cites this entry. `Heap::free_large` asks for a large block of this heap that the caller
owns and gives up, which `dealloc`'s contract gives, for `GlobalAlloc::dealloc` as for this trait's
`deallocate`, and it finds the slot or run from the address. Zero-size
layouts get a dangling, aligned pointer and are never freed (`heap/allocator.rs`).
`allocate_zeroed` is an allocation with `Zero::Whole`, which clears all `len` bytes it
returns, or skips a large slot or run the back end knows to be zero (HEAP-03); a grow of an
empty block is the same allocation. `grow`, `grow_zeroed` and `shrink` of a non-empty block
are `Bound::grow_block` and `shrink_block` (`heap/resize.rs`): a small block whose new layout
has its class stays, with the class's length; one that moves to another small class is copied
into a block of that class before it is freed (HEAP-07); everything else is the general
resize. For `grow_zeroed`, whichever path served it, `grow_block` then clears
from `old.size()` to the length returned unless the block moved into memory known to read as
zero (HEAP-08), so the bytes the stable contract names are zero in place and moved alike.
Every handle reaches `HEAP`, so blocks may move between handles.

## GLOBAL-04: `free_slow` in `c_free_small`

**Precondition** (`Bound::free_slow`). `object` is a block of `class` this heap handed out,
which the caller owns and gives up.

**Proof.** `c_free_small`'s own contract is that, word for word; its one caller,
`c::slow::free_slow`, discharges it (SLOW-03 in `c/slow-safety.md`). `c_alloc_small` and
`c_free_small` run the heap's slow paths directly, without a pop or push in front, after a
generated C routine missed or, for an aligned request, with none before them; `Cache::alloc_slow`
and `Cache::free_slow` begin with the pop or push themselves. Both also write the masks the
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
for a block this allocator returned. Every `Cralloc` reaches `HEAP` through `process()`, so a
block any handle returned is a block of the heap `process()` names.

Reviewed: 0a92f0b
