# Safety ledger: `crates/asmalloc/src/c/mod.rs`

The C functions, and the `export_c_abi!` macro that exports them. F1 and F2 are stated in
`seam-safety.md`.

## MOD-01: the copy and the free in `realloc`

```rust
old.as_ptr().copy_to_nonoverlapping(new, kept.min(size));
free(ptr);
```

When the old block is large the two steps are one call, `process::move_large(old, to, n)`,
which copies the same `n` bytes in pieces, returns the old block's pages behind the copy and
frees it (BACK-CLASS-03 in `back/class-safety.md`). Its contract is the preconditions below.

**Preconditions.** `old` is readable for `n = kept.min(size)` bytes and `new` writable for
`n`; the two ranges do not overlap; `free(ptr)`'s contract holds (a block of the heap that the
caller owns and gives up).

**Invariants and facts.**

- `realloc`'s contract: `ptr` is null or a block of the heap the caller owns; unless the call
  fails, the caller gives it up. Here `ptr` is not null (`old`).
- `kept` is what the old block holds. For `Found::Small(class)`, `class.size()`: by F1 and F2
  the block is a whole slot of `class`, all of it the block's and committed. For
  `Found::Large { size: used }`, the block's usable size, `process::usable_size(old)`: `used`,
  the size the back end recorded when the block was last allocated or resized
  (`Back::alloc`, `Back::alloc_run`, `Back::resize`), rounded up to a page (or `used` itself
  if no block were found, which cannot happen for the address `find` just found). It is
  within the slot or run: `used` is (`Back::alloc` and `Back::resize` abort on a size larger
  than the slot or run, and `alloc_run` records the size it was asked to hold), and the slot
  or run is a whole number of pages (every class above the small ones is a quarter step of a
  power of two of at least 32 KiB, so a multiple of 8 KiB; a run is whole regions), so
  rounding `used` up to a page stays inside it. This is the same bound GLOBAL-03 relies on
  for the length `Allocator` returns. All of the slot or run is
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

## MOD-04: `process::resize_in_place_by_address(old, used, size)` in `realloc`

**Precondition.** `old` is a live large block of the heap that the caller owns and is resizing.

**Proof.** `realloc`'s contract: `ptr` is a block of the heap the caller owns. `find` returned
`Large`, so it is a large slot or run of the heap, live by that contract. `realloc` is resizing
it to `size`: if the call keeps it in place, the caller gives up what lies past `size`, which
C's `realloc` allows (the bytes past the new size are no longer the program's).

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
manual, "Replacing malloc"). The macro also defines `asmalloc_version`, a name no C library or other allocator defines; it
returns a pointer to a static NUL-terminated string, valid for the life of the program. A
program must invoke the macro at most once, or enable the `c-abi`
feature and not invoke it, since two definitions of one symbol fail to link; and it must not
also link another allocator that defines these names (the design's non-goal: one instance per
process).

## C-EXPORT-02: the generated routines `export_c_abi!` and `export_cxx_abi!` define

`$crate::asmalloc_exports_c_x86_64!()` defines `malloc`, `free` and `free_sized`, and
`$crate::asmalloc_exports_cxx_x86_64!()` defines `_ZdlPv`, `_ZdaPv`, `_ZdlPvm` and `_ZdaPvm`,
each a `#[unsafe(no_mangle)]` naked function whose body is a generated routine's code
(`c_malloc`, `c_free`, `c_free_sized`, `c_delete`, `c_delete_sized`), written by the Lean
emitter and checked against it (`Rust.exportsFile`).

**Obligations.** As C-EXPORT-01 for each symbol: its ABI is the C declaration's, it keeps the
C contract, and it replaces nothing the program relies on; and the routine's own obligations
hold whoever calls it.

**Proof.** The routines take their arguments in `rdi` and `rsi` and return in `rax` (System V,
which is C's on x86-64 Linux), and keep the callee-saved registers (proved). The routine's
obligations are discharged in `routines-safety.md` for any thread of the process, so for a C
caller as for Rust; a C caller of `free(p)`, `free_sized(p, n)` or a `delete` has made the free's
promise. The copies name the crate's statics and slow paths through `$crate`, so they are the
same memory and functions as the routines in the crate. The rest of C-EXPORT-01 (what the
symbols replace) is unchanged. `export_cxx_abi!` names a marker only `export_c_abi!` defines,
so the C++ deletes cannot be exported without the C allocator whose blocks they free.

