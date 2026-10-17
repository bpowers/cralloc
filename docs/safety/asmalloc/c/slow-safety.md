# Safety ledger: `crates/asmalloc/src/c/slow.rs`

The slow paths of `malloc`, `free` and `calloc`, and `calloc`'s zeroing tail. F1 and F2 are
stated in `seam-safety.md`.

## SLOW-01: `object.write_bytes(0, size)` in `allocate`

**Preconditions.** `object` is valid for writes of `size` bytes and nothing else accesses them
during the write.

**Proof.** `object` is non-null and came from `seam::alloc_small(class)` with `class` the small
class of `Class::of_c(layout)`, where `layout` is `size` bytes at alignment `align`. A class is
at least the size it is computed for, after rounding to the alignment (`classes::verify`), so
`class.size() >= size`. The heap hands a slot to one caller at a time (GLOBAL-01 in
`heap/global-safety.md`), and this call has not returned it yet, so no other code holds it. A
handed-out slot is committed memory (the back end carves only from a region's writable
prefix), so all `class.size()` bytes are writable.

## SLOW-02: `block.write_bytes(0, total)` in `calloc_zero`

**Preconditions.** As SLOW-01, for `block` and `total`.

**Proof.** `calloc_zero`'s contract is that `block` is a block of the heap of at least `total`
bytes that the caller owns; a block the caller owns is one no other code accesses. Its one
caller today is `seam::calloc` (SEAM-03); a generated `c_calloc` routine that jumps here must
meet the same contract, which its specification will state.

## SLOW-03: `seam::free_small(class, object)` in `free_slow`

**Precondition** (SEAM-01). `object` starts a slot of `class` that the heap handed out, owned by
the caller and given up.

**Proof.** As SEAM-02, from `free_slow`'s contract (`ptr` is null or a block of the heap that
the caller owns and gives up), with `find` returning `Small(class)`. The other arms make no
unsafe call: `free_large` lets the back end find the class or run from the address and abort
if the slot is not handed out (`Back::free_by_address`, `Back::launder`'s validation), and
`None` aborts.
