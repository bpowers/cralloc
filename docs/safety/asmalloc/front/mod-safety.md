# Safety ledger: `crates/asmalloc/src/front/mod.rs`

## FRONT-01: the declaration of glibc's `__rseq_offset`

```rust
unsafe extern "C" {
    pub safe static __rseq_offset: isize;
}
```

**Obligations.** The declared type is the symbol's, every bit pattern of it is valid, and no
one writes the symbol while Rust may read it (a `safe` static is read without `unsafe`).

**Proof.** The same declaration as GLIBC-01 (`os/linux/glibc-safety.md`), whose argument it
repeats: glibc declares `extern const ptrdiff_t __rseq_offset;` (`sys/rseq.h`); `isize` is
`ptrdiff_t` on x86-64 Linux, with every bit pattern valid; the dynamic loader sets it before any
user code runs and never again. The two declarations agree in type, so they name one symbol.
It is public, and hidden, only so that the C routines the export macros define in another crate
can name it as an operand; what any code can do with it is read a constant.
