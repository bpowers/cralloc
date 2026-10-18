# Safety ledger: `crates/asmalloc/src/heap/global.rs`

The process's heap, `HEAP`, over Linux, run with `Real` on `front::SLABS`.

## GLOBAL-01: `unsafe impl GlobalAlloc for Asmalloc`

**Obligations** (`GlobalAlloc`'s `# Safety`). Blocks are valid for their layout, aligned, and
not handed out twice while live; the methods do not unwind; `realloc` keeps the old block's
contents up to the smaller size and leaves the old block untouched when it returns null.

**Proof.** Each method is the heap's. Alignment and size: a small block is a slot of the class
of the layout, which by alignment by construction (`classes::verify`) is a multiple of the
alignment and at least the size, and slots are aligned to their class's alignment; a large one
is a slot of the layout's class or a run aligned as `alloc_run` was asked. Exclusivity: the
back end hands each slot once until it is freed, and magazines hold each free object once (the
slab invariant, `front/twin-safety.md`). No unwinding: the allocator's paths never panic
(aborts go through `Os::abort`, which does not return). `realloc` builds the new layout with
`Layout::from_size_align`, returning null on an invalid size without touching the block;
`Heap::resize` frees the old block only after copying (HEAP-04).

## GLOBAL-02: the calls to the heap's `unsafe fn`s with `Real`

**Precondition** (H, `heap/mod-safety.md`): the slab array of the routines is used only with
this heap; plus each method's own contract, passed on from the caller's.

**Proof.** `Real::slabs()` is `front::SLABS`. Both `Real` and `SLABS` are private to the crate,
so code outside it can neither run routines nor build a twin on these slabs. Inside the crate,
nothing writes `SLABS` except `Real`'s routines and the slow paths and drain of a `Cache`, and
the only `Cache`s made with `Real` are `HEAP`'s (`Heap::alloc_slow`, `Heap::free_slow`). `HEAP`
is the only `Heap` ever run with `Real`. The callers' contracts (`GlobalAlloc`,
`Allocator`) are what the heap's methods ask: a valid layout, and for frees and resizes a
block this heap returned with a layout that fits it. `pop_slow` and `push_slow` are entered from the routines (GLOBAL-04).

## GLOBAL-03: `unsafe impl Allocator for Asmalloc`

**Obligations** (`Allocator`'s `# Safety`). Blocks stay valid until deallocated or resized;
any handle (all are the same ZST) may free any block; `deallocate` accepts any layout that
fits; grow and shrink keep contents and leave the old block alone on error.

**Proof.** As GLOBAL-01, with: the returned length is the class size for a small block, by the
fit rule every size from the request to it has the same class (`classes::verify`), so a
deallocation with any fitting size finds the class; for a large block the length is
`usable_size`, at most the slot or run, and the class comes from the address. Zero-size
layouts get a dangling, aligned pointer and are never freed (`heap/allocator.rs`). Every handle
reaches `HEAP`, so blocks may move between handles.

## GLOBAL-04: `free_slow` from `push_slow`, and `alloc_slow` from `pop_slow`

**Preconditions.** As for `Heap::free_slow` and `Heap::alloc_slow`.

**Proof.** Both are `unsafe fn`s whose contract is that they are reached only as the tail of
the out-of-line routines; the crate never calls them, and only the generated routines name
them (`sym`). They run only as the tail of the out-of-line routines, whose callers
(`Asmalloc::alloc_small`, `Asmalloc::free_small`) pass the address of a small class's word in
slab 0 (`Front::word`) and, for a push, an object of that class they own (`free_small`'s
contract). The class is recovered from the address and checked (`Front::class_of_word` gives a
class only for the word of a small class; `small_class` aborts otherwise), and the object
address is non-null-checked. `small_class` also writes the masks the out-of-line routines read
(`publish_masks`): plain atomic stores of constants, which a race repeats harmlessly. The thread's
`rseq_cs` still names the routine's descriptor, but the instruction pointer has left the
sequence, so an interruption only clears it, and every routine the slow path runs arms its own.
