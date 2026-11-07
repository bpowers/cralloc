# Safety ledger: `crates/cralloc-preload/src/lib.rs` (and `build.rs`)

`lib.rs` has no `unsafe` block of its own. What it does that the compiler cannot check is
define symbols for the whole process: the C allocator's, through `cralloc::export_c_abi!`,
and the name the unwinder looks up for Rust frames. The constructor that starts the background
thread, and its calls of the C library, are in `background.rs`, whose ledger is
`background-safety.md`.

## PRELOAD-01: `cralloc::export_c_abi!()`

The expansion holds the `unsafe` forwarding blocks and `#[unsafe(no_mangle)]` definitions of
C-EXPORT-01 (`docs/safety/cralloc/c/mod-safety.md`), which covers them.

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
  interposable names. For a static musl program, where no export list applies, the evidence
  is the experiments in `docs/research/portability.md`, "Static linking".
- **C++.** The library exports no C++ allocation operator (design: "Interfaces", C++), so
  every `operator new` and `delete` a program calls is its C++ runtime's or the program's own,
  and reaches the heap only through the C functions above: the runtime's `new` calls `malloc`
  and `aligned_alloc`, its `delete` calls `free`. That is sound in every arrangement of
  libraries. Exporting only some operators is not: a library that defines them and is looked
  up after the preload library (Boehm GC's `libgccpp`) would get the program's `new` and we its
  `delete`, and blocks that are not ours would reach us. cralloc has no C++ operators to
  export. `nm -D` of the built library shows no `_Zn` or `_Zd` symbol
  (`every_cxx_operator_form_meets_its_cases_and_binds_to_the_cxx_runtime`).
- **Before the library can be called.** The dynamic loader uses its own minimal allocator until
  it has relocated every object, the preload library included, and only then switches to the
  interposed `malloc`; glibc has registered rseq for the main thread before that
  (`docs/research/linux-rseq-contract.md`, "glibc"). So the first call finds `__rseq_offset`,
  the library's GOT and the routines' words holding the variable's address (`R_X86_64_64`
  relocations, applied in the same pass as the GOT's) relocated. The switch looks up `malloc`, `calloc`, `realloc` and `free`
  in the global scope (`elf/dl-minimal.c`, `__rtld_malloc_init_real`), where the preload
  library comes before libc, and the unversioned definitions here satisfy its `GLIBC_2.0`
  lookup. Memory the minimal allocator handed out is not passed to the interposed `free`: the
  one block the loader later resizes or frees, the initial thread's DTV, is copied into a new
  block instead of reallocated and is never freed (`elf/dl-tls.c:562-578` and `741`). The
  real-program runs (`scripts/preload-check`) would abort on the first such free, since such a
  block is not in cralloc's heap. A new thread registers rseq before it runs code that
  allocates.
- **Never unloaded.** The routines' rseq descriptors, which a thread's `rseq_cs` may name
  until its next preemption, must stay mapped for the life of the process: the kernel reads
  them then, and kills the process if they are gone. A preloaded object is not unloaded, and
  the library is linked `-z nodelete` as well (`build.rs`; the dynamic section's `FLAGS_1`
  says `NODELETE`, which `the_library_is_marked_never_to_be_unloaded` checks), so not even a
  `dlopen` and `dlclose` of it can unmap it. Any other shared object that contains the
  allocator must be linked the same way; unloading one kills the process.
- **Unwinding.** The library is built with `panic = "abort"`. A panic reaches the panic handler,
  which ends the process through `Os::abort` (raw system calls, no allocation). Nothing the
  library calls can throw: the system call instruction, glibc's `__errno_location`, the
  compiler's `memcpy` and `memset`, and the C functions the background thread's start uses
  (`getenv`, `sigfillset`, `pthread_sigmask`, `sigaltstack`, the `pthread_attr` functions,
  `pthread_create`, `pthread_setname_np`, `pthread_detach`, `dlsym`, `getpid`; BACKGROUND-01,
  PRELOAD-03). None is a cancellation point, at which a cancelled thread would unwind. The
  background thread itself waits with the `futex` system call, made directly, which is not
  one either, and nothing can cancel it: its identifier never leaves the function that creates
  it.
  So no exception ever unwinds out through the library's frames.

## PRELOAD-02: `#[unsafe(no_mangle)] extern "C" fn cralloc_preload_no_unwinding`, and the binding of `rust_eh_personality` to it

**Obligations.** The exported name clashes with nothing else in a process. Binding
`rust_eh_personality` to it inside the library changes no other object's unwinding.

**Proof.** The name is specific to this library. `build.rs` passes the linker
`--defsym=rust_eh_personality=cralloc_preload_no_unwinding`, which defines the name inside the
link only: rustc's version script for a `cdylib` exports the symbols rustc lists and makes
every other one local, and `rust_eh_personality` is not in the list, so it is not in the
library's dynamic symbol table (checked with `nm -D`). The unwind tables of the precompiled
`core` linked into the library name it, and they now resolve inside the library instead of
failing the load. Another object that looks up `rust_eh_personality` dynamically
(`librustc_driver` exports and imports its own) does not find this library's. The function is
only reachable when an exception unwinds into the library's own frames, which nothing it calls
can start (PRELOAD-01). If one did, it aborts, which is the behaviour Rust defines under
`panic = "abort"`.

Reviewed: cb8a219
