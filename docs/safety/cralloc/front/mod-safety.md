# Safety ledger: `crates/cralloc/src/front/mod.rs`

## FRONT-01: the declaration of `__rseq_offset`: glibc's, or on musl the crate's own word

```rust
unsafe extern "C" {
    pub safe static __rseq_offset: isize;
}
```

**Obligations.** The declared type is the symbol's, every bit pattern of it is valid, and no
one writes the symbol while Rust may read it (a `safe` static is read without `unsafe`).

**Proof.**

1. *Existence and type.* glibc 2.35 and later export it from the dynamic loader, declared in
   `<sys/rseq.h>` as `extern const ptrdiff_t __rseq_offset;`. On x86-64 and aarch64 Linux
   `ptrdiff_t` is a signed 64-bit integer, so `isize`, with the same alignment, and every bit
   pattern is valid. A binary linked against an older glibc fails to load: an error, not undefined
   behaviour.
2. *Immutability.* glibc sets it as it lays out static thread-local storage (`_dl_determine_tlsoffset`,
   through `_dl_extra_tls_set_offset`), from `init_tls` in the loader for dynamic programs and
   `__libc_setup_tls` for static ones, before relocation processing finishes and before any
   constructor or `main` runs. Afterwards it is in read-only (RELRO) memory. So from the first moment
   Rust code can run it holds one value for ever, which is what an immutable `static` requires.
3. *On x86-64 musl*, which has no `__rseq_offset`, the declaration links to `cralloc_rseq_offset`
   (`link_name`), a word the crate defines in the data generated from Lean
   (`front/generated/x86_64/area.rs`): 8 bytes, 16-aligned, in `.rodata`, holding
   `cralloc_rseq_area@tpoff`, which the linker resolves to a constant in an executable (a
   static or static-PIE musl executable has no dynamic relocation for it, nor would a dynamic
   musl executable, whose own TLS offsets are link-time constants too). So the same type, every bit
   pattern valid, and one value for ever. The name is the crate's own, so a C library that one
   day defines `__rseq_offset` cannot be confused with it (the crate would then find that
   library's registration in place and be refused, AREA-04, which is safe).
4. *On aarch64 musl* nothing is declared: no assembler for it can write the offset as data, and the
   routines hold it in their instructions (`front/area-safety.md`).

It says nothing about whether the calling thread is registered. glibc registers each thread as
it starts; if a registration fails later (a seccomp filter installed after start-up, say), that
thread's `cpu_id` reads -2, and every routine's unsigned compare with the slab count sends it
to the miss path before it touches a slab.

It is public, and hidden, only so that the C routines the export macros define in another crate
can name it as an operand; what any code can do with it is read a constant.

Reviewed: 5a8ade1
