# Safety ledger: `crates/asmalloc/src/c/seam.rs`

The C layer's fast path: the out-of-line pop and push behind `alloc_small` and `free_small`,
and the front halves of `malloc`, `free` and `calloc`.

Shared facts used below.

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

## SEAM-01: `Asmalloc::free_small(class, object)` in `free_small`

**Precondition** (`Asmalloc::free_small`). `object` is a block of `class` the heap handed out,
which the caller owns and gives up.

**Proof.** `free_small`'s own contract is that precondition, word for word, and its callers
discharge it (SEAM-02, SLOW-03).

## SEAM-02: `free_small(class, object)` in `free`, `free_sized` and `free_aligned_sized`

**Precondition.** As SEAM-01: `object` starts a slot of `class` that the heap handed out, owned
by the caller and given up.

The three front halves are the same code; the sized two receive a size (and an alignment) and
do not use it to choose the class, so the proof below is the same for each. A wrong size from
the caller (C's `free_sized`, or C++'s sized `delete` of an object through a base pointer with
no virtual destructor) cannot reach the class: the class comes from the address alone. Their
slow paths, `slow::free_sized_slow` and `slow::free_aligned_sized_slow`, are `free_slow`
(SLOW-03) with the size ignored.

**Proof.** `free`'s contract: `ptr` is null or a block of the heap that the caller owns and
gives up (C's contract for `free`, which every C entry point that frees passes on). `object`
is `ptr`, non-null. `find` returned `Small(class)`, so by F1 `object` is the start of a slot
of `class`; it is a block the heap handed out by `free`'s contract; by F2 it was handed out as
a block of `class`. The caller owns it and gives it up, by the contract.

A pointer that breaks the contract is outside what the proof covers. What each kind does:

- One outside the heap, in an untagged region, or before the heap started makes `find` return
  `None` or `Large`, and goes to `free_slow`, which aborts or lets the back end validate it.
  So does an interior pointer here; the generated C `free`, which does not divide, caches one
  as if it were a block (design: "Interfaces").
- A slot start that is not live is pushed like a live one, and nothing checks it until it
  reaches the bitmaps. A double free, `free(p); free(p)`, leaves `p` twice in the CPU's caches,
  so the next two `malloc`s of the class there both return `p`: two owners of one block. It is
  caught only if both copies reach the bitmaps while the slot is free (`Back::launder` aborts on
  a bit already set, or on the same slot twice in one batch); if one copy is handed out first,
  it is never caught. A slot that was never handed out, above its region's watermark, is
  handed out by the next `malloc` of the class, and its memory may not be committed, so the
  program faults when it writes it; if it reaches the bitmaps first, `launder` aborts on a slot
  beyond the watermark. The design accepts this: a double free of a small object is detected
  only when both copies reach the bitmaps.

## SEAM-03: `slow::calloc_zero(object, total)` in `calloc`

**Precondition** (`calloc_zero`). `object` is a slot of the class `table::calloc_class(total)`
that the caller owns.

**Proof.** `object` is non-null and came from `alloc_small(class)`, so it is a fresh slot of
`class` that the heap handed to this call alone (the heap never hands out a live block twice:
`global-safety.md`, GLOBAL-01), and the whole slot, `class.size()` bytes, is writable. `class`
is `table::calloc_class(total)`, the class of `Class::of_c` of `total` bytes (the table's tests
check every entry), and a class is at least the size it was computed for
(`classes::verify`), so `class.size() >= total`. It is exactly the slot `calloc_zero` asks for.
