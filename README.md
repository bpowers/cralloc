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

As a preload library, which replaces the C `malloc` family in any dynamically linked
program; C++ reaches it through its runtime's calls to `malloc` and `free`:

```sh
cargo build --release -p asmalloc-preload
LD_PRELOAD=$PWD/target/release/libasmalloc_preload.so some-program
```

Limits worth knowing:

- x86-64 Linux with a glibc that registers restartable sequences for its threads (2.35 or
  later). A thread without a registration, as under valgrind, is served correctly but
  slowly.
- A shared object that contains the allocator must be linked `-z nodelete`, as the preload
  library is: unloading it kills the process.
- The heap reserves address space, not memory, and makes do with less under a limit. Under
  a limit too small for its smallest reservation, every allocation fails.
- One copy per process: two copies have separate heaps and cannot free each other's blocks.
- Under Miri, declare the allocator `#[cfg(not(miri))]` and keep Miri's own.

The code in this repository is written by Claude.

## License

Licensed under either of the Apache License, Version 2.0 ([LICENSE.APACHE](LICENSE.APACHE))
or the BSD 3-Clause License ([LICENSE.BSD](LICENSE.BSD)), at your option.
