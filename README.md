asmalloc
========

asmalloc is a memory allocator for multithreaded Rust programs on modern Linux. It serves
`GlobalAlloc`, `Allocator` and the C `malloc` family from one heap.

- The fast paths are assembly, chosen by measurement and proved correct in Lean.
- Caches are per CPU, built on restartable sequences, with no atomics and no locks.
- A free never touches the freed object or shared metadata, so freeing on another thread
  costs the same as freeing locally.

The design is in [docs/design.md](docs/design.md), and the measurements behind it are in
[docs/research/](docs/research/).

## Using it

As the global allocator, with fat LTO so that the fast path is inlined into its callers:

```toml
[dependencies]
asmalloc = { git = "https://github.com/bpowers/asmalloc" }

[profile.release]
lto = "fat"
```

```rust
#[global_allocator]
static ALLOCATOR: asmalloc::Asmalloc = asmalloc::Asmalloc;
```

A binary can add the `executable` feature, which saves a load on the fast path and does not
link into a shared library. The `allocator-api` feature implements the `Allocator` trait for
`Asmalloc`, on toolchains that have the trait (nightly, until it is stable).
`Asmalloc::collect()` returns to the OS everything the heap can give back now, for a program
that knows it is idle, and `Asmalloc::inspect` reports where the heap's memory is.

The heap returns idle memory and empties idle caches on its slow paths, so a process whose
threads all stay inside their caches would keep what it held when it went quiet. A
background thread covers that. It starts by itself once the heap holds tens of megabytes of
free memory and the program has created a thread of its own, and it parks while the process
is quiet. `ASMALLOC_BACKGROUND=0` in the environment turns it off, and
`Asmalloc::start_background()` starts it at once. A program that would rather drive the work
itself calls `Asmalloc::maintain()` from a timer of its own, as the `asmalloc-tokio` crate
does from a task, and then gets no thread. A program with one thread is never given a
second: it calls `maintain` or `collect`, or starts the thread itself. Creating the thread
is the one thing the allocator asks of the C library, and a build without default features
leaves it out.

With the preload library the thread is created when the program creates its first thread.
`ASMALLOC_BACKGROUND=0` turns it off, and any other value starts it before `main`.

As a preload library, which replaces the C `malloc` family in any dynamically linked
program; C++ reaches it through its runtime's calls to `malloc` and `free`:

```sh
cargo build --release -p asmalloc-preload
LD_PRELOAD=$PWD/target/release/libasmalloc_preload.so some-program
```

Limits worth knowing:

- x86-64 Linux, with glibc (dynamically or statically linked) or musl. glibc has registered
  restartable sequences for its threads since 2.35; asmalloc is developed and tested on
  Linux 7.2 and glibc 2.43. On musl the allocator registers its own, in executables only: a
  musl shared library or preload library is not supported. A thread without a registration,
  as under valgrind, is served correctly but slowly.
- A shared object that contains the allocator must be linked `-z nodelete`, as the preload
  library is: unloading it kills the process.
- The heap reserves address space, not memory, and makes do with less under a limit. Under
  a limit too small for its smallest reservation, every allocation fails.
- Under strict overcommit, or a data-segment limit (`RLIMIT_DATA`), a buffer that grows is
  charged for the room it was moved into to grow in place, which can be many times its size,
  whether or not it ever writes there. Such a system fits far fewer large growing buffers
  than its memory would suggest.
- A program that locks its memory with `mlockall` needs an unlimited `memlock` limit or
  the capability that lifts it: the heap's address-space reservation counts against the
  limit even though nothing in it is resident.
- A C program that links asmalloc's static library against static glibc with lld needs
  `-lgcc` before the library on the link line. Otherwise lld takes glibc's 128-bit float
  helpers from the Rust runtime inside the library, whose unwind tables name a symbol that
  only a Rust program defines.
- One copy per process: two copies have separate heaps and cannot free each other's blocks.
- The first allocation that cannot be served from a cache reads a few files under
  `/sys/devices/system/cpu` to learn which CPUs share a cache, and never reads them again. A
  program that installs a seccomp filter before it first allocates must let `openat`, `read`
  and `close` fail or succeed, not kill; if they fail, the allocator treats the machine as
  one cache domain.
- Under Miri, declare the allocator `#[cfg(not(miri))]` and keep Miri's own.

The code in this repository is written by Claude.

## License

Licensed under either of the Apache License, Version 2.0 ([LICENSE.APACHE](LICENSE.APACHE))
or the BSD 3-Clause License ([LICENSE.BSD](LICENSE.BSD)), at your option.
