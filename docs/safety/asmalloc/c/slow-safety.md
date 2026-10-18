# Safety ledger: `crates/asmalloc/src/c/slow.rs`

The slow paths of the C functions, and `calloc`'s zeroing tail.

Shared facts used below and in `mod-safety.md` and `routines-safety.md`.

- F1 (the address table). `process::find(p)` returns `Some(Found::Small(class))` only when the
  heap has started, `p` lies in the heap's object regions, the region table tags `p`'s region
  with the small class `class`, and `p` is the start of a slot of that region
  (`Back::lookup`: `Space::locate`, `RegionTable::get`, `Plan::slot_at`). A region holds slots
  of exactly the class it is tagged with, and a tag never changes while the region holds a
  handed-out slot (`back/class.rs`: a region is tagged before its first slot is carved, and
  only an untouched region is ever untagged).
- F2 (where blocks live). Every block the heap hands out, through any interface, is either a
  slot in a region of its class or a run. A small block (from the Rust layout, from a C size,
  from the back end directly when a thread has no CPU) is a slot of a small class in a region
  tagged with that class. So for a block the heap handed out, F1's class is the class it was
  handed out as.

## SLOW-01: `object.write_bytes(0, class.size())` in `allocate`

**Preconditions.** `object` is valid for writes of `class.size()` bytes and nothing else
accesses them during the write.

**Proof.** `object` is non-null and came from `heap::c_alloc_small(class)` with `class` the small
class of `Class::of_c(layout)`, where `layout` is `size` bytes at alignment `align`. A class is
at least the size it is computed for, after rounding to the alignment (`classes::verify`), so
`class.size() >= size`; the write covers the whole slot, no more. The heap hands a slot to one caller at a time (GLOBAL-01 in
`heap/global-safety.md`), and this call has not returned it yet, so no other code holds it. A
handed-out slot is committed memory (the back end carves only from a region's writable
prefix), so all `class.size()` bytes are writable.

## SLOW-02: the zeroing stores in `zero_block`, `zero_small` and `zero16`

`block.write_bytes(0, len)`, and for a length up to 512 bytes, `zero_small`'s volatile stores:
one 8-byte store for a length of 8, else one 16-byte store at each multiple of 16 below `len`.

**Preconditions.** `block` is valid for writes of `len` bytes and nothing else accesses them
during the write; for `zero_small`'s stores, `len` is 8, or a multiple of 16 with `block`
aligned to 16 (each store is aligned and inside the `len` bytes).

**Proof.** `len` is `table::C_CALLOC_LENS[total]`, the size of `table::calloc_class(total)`
(the table's tests check every entry), when the table covers `total`, else `total`. C classes
are 8 bytes or a multiple of 16 (the malloc table's test checks the 16-byte rule for every small
size), and a slot of a class of 16 bytes or more is aligned to 16: slots are carved at multiples
of the class size from a region start, which is page-aligned. `zero_block`'s contract is that
`block` is a slot of that class (so exactly `len` bytes of committed memory, SLOW-01), or, beyond
the table, a block of at least `total` bytes, and that the caller owns it; a block the caller
owns is one no other code accesses. Its one caller is `calloc_zero_tail` (SLOW-04).

`free_sized_slow` and `free_aligned_sized_slow` each call `free_slow(ptr)` with their own
contract, which is `free_slow`'s (the size and alignment are not used).

## SLOW-04: `zero_block(block, n.wrapping_mul(size))` in `calloc_zero_tail`

**Preconditions.** `zero_block`'s: `block` is a slot of the C class of the total, which the
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

## SLOW-03: `heap::c_free_small(class, object)` in `free_slow`

**Precondition** (`c_free_small`). `object` is a block of `class` the heap handed out, which the
caller owns and gives up.

**Proof.** `free_slow`'s contract: `ptr` is null or a block of the heap that the caller owns and
gives up (C's contract for `free`, which every C entry point that frees passes on). `object` is
`ptr`, non-null. `find` returned `Small(class)`, so by F1 `object` is the start of a slot of
`class`; it is a block the heap handed out by the contract; by F2 it was handed out as a block
of `class`. The caller owns it and gives it up, by the contract. The sized frees' slow paths are
`free_slow` with the size ignored, so a wrong size from the caller cannot reach the class. The
other arms make no unsafe call: `free_large` lets the back end find the class or run from the
address and abort if the slot is not handed out (`Back::free_by_address`, `Back::launder`'s
validation), and `None` aborts.

A pointer that breaks the contract is outside what the proof covers. Two kinds still fail
safely: one outside the heap, interior to a slot, in an untagged region, or before the heap
started makes `find` return `None` or `Large`, and aborts or is validated by the back end. A
slot start that is not live (a double free, or a slot never handed out) is pushed and caught
when its magazine is laundered (`Back::launder` aborts on a slot already free or beyond the
watermark), which is the design's stated double-free detection.
