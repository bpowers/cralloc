# Safety ledger: `crates/asmalloc/src/heap/miri.rs`

Compiled only under Miri.

## MIRI-01: the `extern "Rust"` declarations of `miri_alloc` and `miri_dealloc`

**Proof.** Miri provides these shims with exactly these signatures
(`fn miri_alloc(size: usize, align: usize) -> *mut u8` and
`fn miri_dealloc(ptr: *mut u8, size: usize, align: usize)`), documented in Miri's README for
`no_std` programs. `miri_alloc` requires a non-zero size and a power-of-two alignment, which a
`GlobalAlloc` caller guarantees and the `Allocator` impl checks (zero sizes never reach it).
`miri_dealloc` requires the block and the size and alignment it was allocated with, which
`GlobalAlloc::dealloc`'s and `Allocator::deallocate`'s contracts give (the `Allocator` impl
returns exactly the requested length, so a fitting layout is that one).

## MIRI-02: `unsafe impl GlobalAlloc` and `Allocator` for `Asmalloc` under Miri

**Proof.** Miri's allocator returns fresh, aligned, exclusive blocks and checks every misuse.
Grow and shrink use the trait's defaults (allocate, copy, deallocate). Nothing panics.
