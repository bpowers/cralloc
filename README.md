cralloc
========

cralloc is a memory allocator for multithreaded Rust programs on modern Linux. It serves
`GlobalAlloc`, `Allocator` and the C `malloc` family from one heap.

- The fast paths are assembly, chosen by measurement and proved correct in Lean.
- Caches are per CPU, built on restartable sequences, with no atomics and no locks.
- A free never touches the freed object or shared metadata, so freeing on another thread
  costs the same as freeing locally.

The design is in [docs/design.md](docs/design.md), and what we have learned is in
[docs/research/README.md](docs/research/README.md).

cralloc was called asmalloc until November 2026, and commits from before then use that name.

## Using it

As the global allocator, with fat LTO so that the fast path is inlined into its callers:

```toml
[dependencies]
cralloc = { git = "https://github.com/bpowers/cralloc" }

[profile.release]
lto = "fat"
```

```rust
#[global_allocator]
static ALLOCATOR: cralloc::Cralloc = cralloc::Cralloc;
```

A binary can add the `executable` feature, which saves a load on the fast path and does not
link into a shared library. The `allocator-api` feature implements the `Allocator` trait for
`Cralloc`, on toolchains that have the trait (nightly, until it is stable).
`Cralloc::collect()` returns to the OS everything the heap can give back now, for a program
that knows it is idle, and `Cralloc::inspect` reports where the heap's memory is.

The heap returns idle memory and empties idle caches on its slow paths, so a process whose
threads all stay inside their caches would keep what it held when it went quiet. A
background thread covers that. It starts by itself once the heap holds tens of megabytes of
free memory and the program has created a thread of its own, and it parks while the process
is quiet. `CRALLOC_BACKGROUND=0` in the environment the program is started with turns it
off, and `Cralloc::start_background()` starts it at once. A later `setenv` does not change
the setting, and a setuid, setgid or file-capability program, which cannot read its own
`/proc/self/environ`, ignores the variable, as `secure_getenv` would. A program that would rather drive the work
itself calls `Cralloc::maintain()` from a timer of its own, as the `cralloc-tokio` crate
does from a task, and then gets no thread. A program with one thread is never given a
second: it calls `maintain` or `collect`, or starts the thread itself. Creating the thread
is the one thing the allocator asks of the C library, and a build without default features
leaves it out.

With the preload library the thread is created when the program creates its first thread.
`CRALLOC_BACKGROUND=0` turns it off, and any other value starts it before `main`.

As a preload library, which replaces the C `malloc` family in any dynamically linked
program; C++ reaches it through its runtime's calls to `malloc` and `free`:

```sh
cargo build --release -p cralloc-preload
LD_PRELOAD=$PWD/target/release/libcralloc_preload.so some-program
```

Limits worth knowing:

- x86-64 Linux, with glibc (dynamically or statically linked) or musl. glibc has registered
  restartable sequences for its threads since 2.35; cralloc is developed and tested on
  Linux 7.2 and glibc 2.43. On musl the allocator registers its own, in executables only: a
  musl shared library or preload library is not supported. A thread without a registration,
  as under valgrind, is served correctly but slowly.
- A shared object that contains the allocator must be linked `-z nodelete`, as the preload
  library is: unloading it kills the process.
- The heap reserves address space, not memory, and makes do with less under a limit. Its
  smallest reservation is about 8 GiB of address space; under a limit (`ulimit -v`,
  `RLIMIT_AS`) that leaves less than that, every allocation fails.
- Under strict overcommit, or a data-segment limit (`RLIMIT_DATA`), a buffer that grows is
  charged for the room it was moved into to grow in place, which can be many times its size,
  whether or not it ever writes there. Such a system fits far fewer large growing buffers
  than its memory would suggest.
- A program that locks its memory with `mlockall` needs an unlimited `memlock` limit or
  the capability that lifts it: the heap's address-space reservation counts against the
  limit even though nothing in it is resident. With memory locked the allocator returns
  nothing to the OS, since the kernel refuses to take locked memory back: what the program
  frees stays resident for reuse, and a freed block larger than a region (4 GiB, or less
  where an address-space limit made the regions smaller) is not reused at all.
- A C program that links cralloc's static library against static glibc with lld needs
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
