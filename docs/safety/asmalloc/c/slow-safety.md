# Safety ledger: `crates/asmalloc/src/c/slow.rs`

The slow paths of `malloc`, `free` and `calloc`, and `calloc`'s zeroing tail. F1 and F2 are
stated in `seam-safety.md`.

## SLOW-01: `object.write_bytes(0, class.size())` in `allocate`

**Preconditions.** `object` is valid for writes of `class.size()` bytes and nothing else
accesses them during the write.

**Proof.** `object` is non-null and came from `seam::alloc_small(class)` with `class` the small
class of `Class::of_c(layout)`, where `layout` is `size` bytes at alignment `align`. A class is
at least the size it is computed for, after rounding to the alignment (`classes::verify`), so
`class.size() >= size`; the write covers the whole slot, no more. The heap hands a slot to one caller at a time (GLOBAL-01 in
`heap/global-safety.md`), and this call has not returned it yet, so no other code holds it. A
handed-out slot is committed memory (the back end carves only from a region's writable
prefix), so all `class.size()` bytes are writable.

## SLOW-02: `block.write_bytes(0, len)` in `calloc_zero`

**Preconditions.** `block` is valid for writes of `len` bytes and nothing else accesses them
during the write.

**Proof.** `len` is the size of `table::calloc_class(total)` when the table covers `total`,
else `total`. `calloc_zero`'s contract is that `block` is a slot of that class (so exactly
`len` bytes of committed memory, SLOW-01), or, beyond the table, a block of at least `total`
bytes, and that the caller owns it; a block the caller owns is one no other code accesses. Its callers
are `seam::calloc` (SEAM-03) and `calloc_zero_tail` (SLOW-04).

`free_sized_slow` and `free_aligned_sized_slow` each call `free_slow(ptr)` with their own
contract, which is `free_slow`'s (the size and alignment are not used).

## SLOW-04: `calloc_zero(block, n.wrapping_mul(size))` in `calloc_zero_tail`

**Preconditions.** `calloc_zero`'s: `block` is a slot of the C class of the total, which the
caller owns.

**Proof.** `calloc_zero_tail`'s contract gives that the product does not overflow, so the
wrapping product is the total, at most the calloc table's end, and that `block` is a slot of its
class, owned. Its one caller is the generated `c_calloc`, which jumps here only after a pop that
committed (`Spec.CEntry.callocContract`, proved in Lean): the product did not overflow and was
at most `CALLOC_MAX` (1024, `table::CALLOC_TABLE_MAX` by a compile-time assertion), and the block
popped is from the magazine of the class `C_CALLOC_SIZES[total]` gives, which is
`C_CALLOC_CLASSES`, the class of `Class::of_c` of the total (`table.rs`'s tests check every
entry). A popped block is a free block of that class that the pop handed to this call alone
(`routines-safety.md`). The routine passes `n` and `size` as they arrived and the block in
`rdx`, the third argument's register, and its tail call leaves the stack as the caller of
`calloc` left it, so the arguments arrive as the signature says.

## SLOW-03: `seam::free_small(class, object)` in `free_slow`

**Precondition** (SEAM-01). `object` starts a slot of `class` that the heap handed out, owned by
the caller and given up.

**Proof.** As SEAM-02, from `free_slow`'s contract (`ptr` is null or a block of the heap that
the caller owns and gives up), with `find` returning `Small(class)`. The other arms make no
unsafe call: `free_large` lets the back end find the class or run from the address and abort
if the slot is not handed out (`Back::free_by_address`, `Back::launder`'s validation), and
`None` aborts.
