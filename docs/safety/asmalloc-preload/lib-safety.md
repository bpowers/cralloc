# Safety ledger: `crates/asmalloc-preload/src/lib.rs` (and `build.rs`)

The `LD_PRELOAD` library has no `unsafe` block of its own. What it does that the compiler
cannot check is define symbols for the whole process: the C allocator's, through
`asmalloc::export_c_abi!`, and the name the unwinder looks up for Rust frames.

## PRELOAD-01: `asmalloc::export_c_abi!()`

The expansion holds the `unsafe` forwarding blocks and `#[unsafe(no_mangle)]` definitions of
C-EXPORT-01 (`docs/safety/asmalloc/c/mod-safety.md`), which covers them.

What the preload setting adds:

- **The whole family is replaced.** The exported set is every function glibc 2.43's
  `libc.so.6` exports that takes or returns a heap block: `malloc`, `calloc`, `realloc`,
  `reallocarray`, `free`, `cfree`, `free_sized`, `free_aligned_sized`, `posix_memalign`,
  `aligned_alloc`, `memalign`, `valloc`, `pvalloc`, `malloc_usable_size`, `malloc_trim`
  (checked with `nm -D --defined-only /lib64/libc.so.6`). The rest of glibc's malloc
  interface (`mallinfo`, `mallinfo2`, `mallopt`, `malloc_stats`, `malloc_info`) takes no block
  and reads or tunes only glibc's own heap, which then stays empty. `__libc_malloc` and its
  siblings are glibc's internal aliases; no binary or library on the reference host imports
  them (census of `/usr/bin`, `/usr/lib64` and `/usr/libexec`), and glibc's own code calls the
  interposable names.
- **C++.** The library also exports 16 of the 20 replaceable C++ allocation operators: the
  nothrow `new` forms and every `delete` (`asmalloc::export_cxx_abi!`, CXX-EXPORT-01 in
  `docs/safety/asmalloc/c/cxx-safety.md`). libstdc++ keeps the four throwing forms of `new`,
  which allocate with the library's `malloc` and `aligned_alloc` and can throw
  `std::bad_alloc`, so every block either `new` returns is asmalloc's, and the library's
  `delete` frees it. The library has no dependency on a C++ runtime; it finds the
  new-handler with `dlsym` when an allocation fails, and loads into programs that have none.
- **Before the library can be called.** The dynamic loader uses its own minimal allocator until
  it has relocated every object, the preload library included, and only then switches to the
  interposed `malloc`; glibc has registered rseq for the main thread before that
  (`docs/research/linux-rseq-contract.md`, section 4). So the first call finds `__rseq_offset`
  and the library's GOT relocated. The switch looks up `malloc`, `calloc`, `realloc` and `free`
  in the global scope (`elf/dl-minimal.c`, `__rtld_malloc_init_real`), where the preload
  library comes before libc, and the unversioned definitions here satisfy its `GLIBC_2.0`
  lookup. Memory the minimal allocator handed out is not passed to the interposed `free`: the
  one block the loader later resizes or frees, the initial thread's DTV, is copied into a new
  block instead of reallocated and is never freed (`elf/dl-tls.c:562-578` and `741`). The
  real-program runs (`scripts/preload-check`) would abort on the first such free, since such a
  block is not in asmalloc's heap. A new thread registers rseq before it runs code that
  allocates.
- **Never unloaded.** A preloaded object is never unloaded, so the routines' rseq descriptors,
  which a thread's `rseq_cs` may name until its next preemption, stay mapped for the life of
  the process.
- **Unwinding.** The library is built with `panic = "abort"`. A panic reaches the panic handler,
  which ends the process through `Os::abort` (raw system calls, no allocation). The only
  function the library calls that may throw is a C++ new-handler (CXX-04); that call is
  `extern "C-unwind"`, so rustc gives it a landing pad, whose personality (PRELOAD-02) aborts.
  Everything else it calls cannot throw: the system call instruction, glibc's
  `__errno_location` and `dlsym`, `std::get_new_handler` (`noexcept`), and the compiler's
  `memcpy` and `memset`. So no exception ever unwinds out through the library's frames.

## PRELOAD-02: `#[unsafe(no_mangle)] extern "C" fn asmalloc_preload_no_unwinding`, and the binding of `rust_eh_personality` to it

**Obligations.** The exported name clashes with nothing else in a process. Binding
`rust_eh_personality` to it inside the library changes no other object's unwinding.

**Proof.** The name is specific to this library. `build.rs` passes the linker
`--defsym=rust_eh_personality=asmalloc_preload_no_unwinding`, which defines the name inside the
link only: rustc's version script for a `cdylib` exports the symbols rustc lists and makes
every other one local, and `rust_eh_personality` is not in the list, so it is not in the
library's dynamic symbol table (checked with `nm -D`). The unwind tables of the precompiled
`core` linked into the library name it, and they now resolve inside the library instead of
failing the load. Another object that looks up `rust_eh_personality` dynamically
(`librustc_driver` exports and imports its own) does not find this library's. The function is
only reachable when an exception unwinds into the library's own frames, which happens in one
case: a C++ new-handler that throws (PRELOAD-01). Then it aborts, which is the behaviour Rust
defines for that call under `panic = "abort"`, and the heap is left as it was: the handler runs
between attempts, when the allocator holds nothing.
