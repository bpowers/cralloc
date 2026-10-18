# Safety ledger: `crates/asmalloc/src/c/routines.rs`, and the exported C routines

The generated C entry points (`front/generated/x86_64/centry.rs`): `c_malloc`, `c_calloc`,
`c_free` and `c_free_sized`. Lean proves each against its contract
(`lean/Asmalloc/Spec/CEntry.lean`) under any interruption; the obligations below are the
generated `# Safety` sections, discharged once here for every call: the wrappers in
`routines.rs` (ROUTINES-01) and the copies the export macros define under the C names
(C-EXPORT-02 in `mod-safety.md` refers here).

## The obligations

- (R) glibc registered the calling thread's rseq area.
- (L) `SLABS` (the slabs), the thread's `rseq_cs` and `cpu_id`, `__rseq_offset`, the masks'
  table (`MASK_DISP` above `SLABS`), each routine's tables (`C_SIZES`, `C_CALLOC_SIZES` or `C_SIZED_SIZES` 2048
  bytes in; the mirror of the heap words at `SHIFT_DISP` above `SLABS`, and the region table at `REGION_DISP` above `SLABS`) do not overlap the return address or wrap around the end of the address space;
  `rseq_cs` overlaps none of the others; none but the slabs overlaps the slabs.
- (I) For every class byte `k` below 256, the masks' table's entry `k` is zero or the mask of
  an allowed magazine size, and if it is not zero, every slab's word `8 k` is zero, a marker or
  a slot of a magazine of that size apart from all of the above, each slab's its own; every
  thread and the drainer keep this while the routine may run.

The contents of the size tables, the mirror and the region table may be anything:
the routines are proved for every content (their contracts state what they did in terms of the
values they read).

## Why they hold

- (R): as (R) in `front/real-safety.md`.
- (L): `SLABS` is the `Front` static; the masks, the published words and the region table are
  fields of it past every slab (compile-time assertions in `front/mod.rs` pin each to the
  generated `MASK_DISP`, `SHIFT_DISP`, `BASE_DISP`, `MIRROR_BYTES`, `MIRROR_COUNT`,
  and `REGION_DISP`, and the static's size to
  the slabs plus three pages). `C_SIZES`, `C_CALLOC_SIZES` and `C_SIZED_SIZES` are statics of their own, immutable,
  each table 2048 bytes in (assertions in `c/table.rs` against the generated `TABLE_OFF`,
  `MALLOC_LEN` and `SIZED_LEN`). Distinct statics do not overlap, and none overlaps glibc's
  thread control block, which holds the rseq area, or the thread's stack. None is at the top of
  the address space.
- (I): for a small class, (C) and (S) of `front/real-safety.md`: the mask entry is zero or the
  class's, which `MagazineSize` keeps allowed, and the slab invariant holds of its words. For
  any other byte the entry is zero: `publish_masks` writes `MASK_TABLE`, which is zero for every
  index that is not a small class's (`front::tests`), and nothing else writes the table. A zero
  mask carries no obligation on the words.

## What the routines' results mean

The contracts say what each routine did with the class it found; that this is the heap's
right class is the Rust side's to keep, and is what makes the results valid allocations and
frees:

- `c_malloc`: the class is `C_SIZES[(size + 7) / 8]`, `Class::of_c` of every size the entry
  covers (`table.rs` builds it from `Class::of_c` and its tests check every entry), so the block
  popped is a free block of the C class of `size`, which the C contract asks for.
- `c_calloc`: the class is `C_CALLOC_SIZES[n * size]` for a product that does not overflow
  and is at most 1024, `Class::of_c` of that total, and the routine leaves the block it popped
  to `slow::calloc_zero_tail`, which zeroes it (SLOW-04 in `slow-safety.md`).
- `c_free` and `c_free_sized` push the pointer to the class named by the region's byte in the
  copy beside the slabs (the sized free only when its prediction agrees), the region found
  from the mirror of the heap's words beside the slabs, which is zero or the heap's own words
  (`c/mirror.rs`); zero sends every pointer to the slow path. That byte is zero or
  the region's true class (`c/mirror.rs`): it is written only after a small block of the region
  has been handed out to C, and the back end never takes a region holding blocks from its
  class. By the free's contract the pointer is a block of the heap the caller owns and gives
  up; a block in a region of a small class is a slot of that class (F1, F2 in
  `seam-safety.md`). So the push hands the heap a valid free of the right class. A zero byte, a
  large class's or a run's has a zero mask, and the routine leaves for the slow path, which
  decides from the back end's own table. A pointer into the middle of a small block is outside
  the contract; the routine pushes it as a block of its region's class, and it is caught only
  if it reaches the bitmaps before being handed out again (docs/design.md, "Interfaces").
- On a miss each routine tail-calls its slow path (`malloc_slow`, `calloc_slow_args`, `free_slow`,
  `free_sized_slow`) with its
  arguments as they arrived, which completes the call from scratch.

## ROUTINES-01: the calls in `routines.rs`

**Proof.** The obligations hold as above for any calling thread of the process. `malloc` has no
precondition of its own; `free` and `free_sized` pass on their callers' promise, `free`'s
contract. Pointers cross the assembly as integers: `free` exposes the block's provenance and
`malloc` takes the exposed provenance back, as REAL-03 argues for the out-of-line routines.
