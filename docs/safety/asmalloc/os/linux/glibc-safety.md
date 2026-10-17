# Safety ledger: `crates/asmalloc/src/os/linux/glibc.rs`

The only C-library items the allocator touches. Kani and Miri do not build or run this file
(Kani builds without the Linux module; Miri never calls it).

## GLIBC-01: the `unsafe extern "C"` block declaring `__rseq_offset` and `__rseq_size`

**Block.** `unsafe extern "C" { safe static __rseq_offset: isize; safe static __rseq_size: u32; }`.
Marking the items `safe` makes reading them safe, so the whole obligation sits on the
declaration.

**Preconditions.** For a `safe static` the declaration must be exact. The symbol has the
declared type and layout, every bit pattern of it is valid, and it does not change while Rust
code can read it.

**Proof.**

1. *Existence and type.* glibc 2.35 and later export both from the dynamic loader, declared in
   `<sys/rseq.h>` as `extern const ptrdiff_t __rseq_offset;` and
   `extern const unsigned int __rseq_size;`. On x86-64 Linux `ptrdiff_t` is a signed 64-bit
   integer, so `isize`, and `unsigned int` is a 32-bit unsigned integer, so `u32`, with the same
   alignment. Every bit pattern is valid for both. A binary linked against an older glibc fails
   to load: an error, not undefined behaviour.
2. *Immutability.* glibc sets both while initializing the thread pointer, before relocation
   processing finishes and before any constructor or `main` runs. That is `__tls_init_tp` in the
   loader for dynamic programs and `__libc_setup_tls` for static ones. Afterwards they are in
   read-only (RELRO) memory. So from the first moment Rust code can run, they hold one value
   forever, which is what an immutable `static` requires.
3. *Meaning.* `__rseq_size` is zero when glibc did not register (valgrind,
   `glibc.pthread.rseq=0`, a kernel without rseq). Otherwise it is the registered feature size,
   which `registration()` passes to `RseqArea::new`. The values describe the process, not a
   thread.

**What this does not cover.** Whether the calling thread is registered. glibc registers each
thread as it starts. If a later registration fails, for example under a seccomp filter
installed after start-up, that thread's `cpu_id` is -2 while `__rseq_size` is unchanged. The
caches are indexed by `cpu_id`, which every registration has, and every routine compares it
with the slab count (unsigned, so a negative value is out of range) before touching a slab.
So an unregistered thread never reaches a slab whatever these facts say.

**Machine checks.** `os::linux::kernel::tests::glibc_registers_rseq` reads both values on the
reference platform.

**Review.** 2026-10-17, fresh adversarial reviewer: points 1 and 2 accepted. Point 3 was not:
it claimed registration was the same for every thread. It is corrected above, and the
correction awaits the same reviewer's check.
