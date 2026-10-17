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

The code in this repository is written by Claude.

## License

Licensed under either of the Apache License, Version 2.0 ([LICENSE.APACHE](LICENSE.APACHE))
or the BSD 3-Clause License ([LICENSE.BSD](LICENSE.BSD)), at your option.
