# Safety ledger: `crates/asmalloc/src/c/mod.rs`

The C functions, and the `export_c_abi!` macro that exports them. F1 and F2 are stated in
`seam-safety.md`.

## MOD-01: the copy and the free in `realloc`

```rust
old.as_ptr().copy_to_nonoverlapping(new, kept.min(size));
free(ptr);
```

**Preconditions.** `old` is readable for `n = kept.min(size)` bytes and `new` writable for
`n`; the two ranges do not overlap; `free(ptr)`'s contract holds (a block of the heap that the
caller owns and gives up).

**Invariants and facts.**

- `realloc`'s contract: `ptr` is null or a block of the heap the caller owns; unless the call
  fails, the caller gives it up. Here `ptr` is not null (`old`).
- `kept` is what the old block holds. For `Found::Small(class)`, `class.size()`: by F1 and F2
  the block is a whole slot of `class`, all of it the block's and committed. For
  `Found::Large { size: used }`, `used`, the size the back end recorded when the block was
  last allocated or resized (`Back::alloc`, `Back::alloc_run`, `Back::resize`). It is within
  the slot or run: `Back::alloc` and `Back::resize` abort on a size larger than the slot or
  run, and `alloc_run` records the size it was asked to hold. All of the slot or run is
  committed: a carve hands out a slot only after the pages up to the slot's end are writable
  (`back/region.rs`, `carve`), `alloc_run` commits the whole run before returning it, and
  nothing makes committed memory inaccessible again (a purge is `MADV_DONTNEED`, which keeps
  the protection).
- `new` is non-null and was just allocated for this call: by `seam::alloc_small(class)` with
  `class` the small class of `Class::of_c(size)`, so at least `size` bytes; or by
  `process::alloc_large_grown(size)` or `process::alloc_large(size, ..)`, both at least `size`
  bytes (a slot of a class at least the rounded size, or a run of at least `size`).

**Proof.** Readable: `n <= kept`, and `kept` bytes from `old` are the block's (above). The
contents may include bytes the program never wrote; a byte copy carries them as they are and
reads no typed value. Writable: `n <= size <= ` the size of `new`. Not overlapping: `old` is
still live (it is freed only after the copy) and `new` is a different live block, and the heap
never hands out memory that overlaps a live block (GLOBAL-01). The free: on this path the call
succeeds, so the caller gives `ptr` up, and it is a block of the heap it owns by the contract;
`free`'s contract is exactly that.

## MOD-02: `out.write(block)` in `posix_memalign`

**Precondition.** `out` is valid for a write of a pointer.

**Proof.** `posix_memalign`'s contract, from POSIX: `memptr` points to a `void *` object the
caller provides. The write happens only after the allocation succeeded, so on failure `*out`
is untouched, as POSIX requires.

## MOD-03: the pass-through calls

`free`, `free_sized`, `free_aligned_sized` and `cfree` call `seam::free(ptr)`; `realloc` calls
`free(ptr)` for a zero size; `reallocarray` calls `realloc(ptr, total)`.

**Preconditions.** The callee's contract: `seam::free` and `free` take null or a block of the
heap the caller owns and gives up; `realloc` takes null or a block the caller owns.

**Proof.** Each caller's own contract is the callee's, passed on unchanged: the sized frees and
`cfree` free exactly what `free` frees (their size and alignment arguments are ignored, so a
wrong one from C cannot change which block or class is freed). `realloc(ptr, 0)` gives `ptr`
up, which C17 permits and glibc does; the caller of `realloc` may not use `ptr` afterwards
whatever the outcome of a zero-size call.

## C-EXPORT-01: the forwarding calls in `export_c_abi!`, and its `#[unsafe(no_mangle)]` items

**Obligations.**

- Each `unsafe { $crate::c::f(args) }` meets `f`'s contract.
- Each `#[unsafe(no_mangle)]` function is a sound definition of the C symbol of that name: its
  signature is the C declaration's, it keeps the C contract, and defining the symbol replaces
  no other definition the program relies on.

**Proof.** Each exported function is `unsafe extern "C"` with the C signature (`size_t` is
`usize`, `int` is `c_int`, `void *` is `*mut c_void`; rustc's
`suspicious_runtime_symbol_definitions` lint checks `malloc` and friends against the signatures
it expects), and its body passes its arguments to the function of the same name in
`asmalloc::c` unchanged. A C caller of `free(p)`, `realloc(p, n)`, `posix_memalign(&q, ..)` or
`malloc_usable_size(p)` has made the C promise for that call, which is the Rust function's
`# Safety` section: a pointer argument is null or a block the caller owns (for `free` and the
frees, given up), or for `posix_memalign` a writable `void *`. The functions without pointer
arguments have no preconditions.

The symbols are meant to replace the C library's allocator, which is the point of the macro,
and they replace it whole: every glibc 2.43 function that takes or returns a block is among
them (the list and the census behind it are in the macro's documentation and in the preload
crate's ledger). Replacing a subset would let glibc serve a block from its own heap that
asmalloc is then asked to free, or the reverse. glibc supports replacing its allocator by
interposition and calls `malloc` and the rest through the interposable symbols itself (glibc
manual, "Replacing malloc"). A program must invoke the macro at most once, or enable the `c-abi`
feature and not invoke it, since two definitions of one symbol fail to link; and it must not
also link another allocator that defines these names (the design's non-goal: one instance per
process).
