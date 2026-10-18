# Safety ledger: `crates/asmalloc/src/c/errno.rs`

How the C layer sets `errno`: through glibc's accessor, the only reference to a C library in
the allocator.

## ERRNO-01: the declaration of `__errno_location`

**Obligation.** The declaration matches the definition the linker binds it to, in name, ABI,
argument types and return type, and the `link` attribute names a library that defines it.

**Proof.** glibc declares `extern int *__errno_location (void) __THROW __attribute_const__;`
(`errno.h`, via `bits/errno.h`) and exports it from `libc.so.6` as
`__errno_location@@GLIBC_2.2.5`. The Rust declaration is `extern "C" fn() -> *mut c_int`: no
arguments, the C calling convention, and a pointer to `int` returned in `rax`, which is how
`int *` is returned on x86-64 System V. `c_int` is `i32`, the size and alignment of C's `int`
on this target. `#[link(name = "c")]` adds `-lc`, the library that defines it. glibc defines
it in every configuration the design supports (glibc 2.43, dynamically or statically linked);
a program that links the crate with std links the same library already.

musl declares the same (`include/errno.h`: `int *__errno_location(void);`) and defines it in
`libc.a` (`src/errno/__errno_location.c`), returning the calling thread's `errno` slot; the
same declaration and argument hold.

## ERRNO-02: `__errno_location().write(value)` in `set`

**Preconditions.** The call is sound to make on the calling thread; the pointer it returns is
valid for a write of an `i32`, aligned, and nothing else accesses that location during the
write in a way that races with it.

**Invariants.**

- I1. glibc's `__errno_location` returns the address of the calling thread's `errno`, a
  thread-local `int` in the thread's static TLS block (`csu/errno-loc.c`: `return &errno;`).
  It has no preconditions and touches no other state. It is async-signal-safe.
- I2. The C layer runs only on threads glibc made or initialized (the main thread, threads of
  `pthread_create`, and forked children of either), so the thread pointer and its static TLS
  exist. The design's non-goal excludes `vfork`/`CLONE_VM` children and raw `clone` threads,
  which have no allocator support anyway; glibc's own `posix_spawn` child never allocates
  (`docs/research/linux-rseq-contract.md`, section 5).

**Proof.** By I2 the thread has glibc's TLS, so by I1 the call returns a valid, aligned pointer
to this thread's `errno`. `errno` is thread-local, so only this thread reaches it: the only
possible concurrent access is a signal handler on this same thread, which is not a data race
(the handler runs between whole instructions of this one, and a 4-byte aligned store is one
instruction). A handler that runs between the store and the C caller's read can change
`errno`, which is the ordinary C rule for errno and handlers, not a memory-safety question.
The write is a plain `i32` store into memory glibc owns on the program's behalf and documents
as writable by any code (`errno` is a modifiable lvalue).
