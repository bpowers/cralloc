# Qdrant

Qdrant 1.20.0, the vector search engine, as the `qdrant search` and `qdrant mixed` jobs of
the large-heap suite (`--suite servers-large`, run with `--mem-cap 12G`). Its release binary
ships with jemalloc.

- **Build** (`build.rs`, `src/main.rs`): Qdrant's server is its binary crate, not a library,
  so this package compiles Qdrant's own `src/main.rs` from the checkout cargo makes of its
  repository at the release's commit (where its library crates come from too), with three
  mechanical edits that must each match once: jemalloc's global-allocator items left out,
  `main` renamed, and a `#[path]` on each top-level module. The dependency table is Qdrant's
  binary's, written out. `no-jemalloc-sys/` takes the place of tikv-jemalloc-sys 0.7, which
  Qdrant's telemetry and its Pyroscope profiler link: no second jemalloc, and every jemalloc
  control call fails, so Qdrant's allocator statistics and memory checks are off for every
  allocator alike. Qdrant's release profile is fat LTO with one codegen unit: each allocator's
  build is a long single-core link.
- **Driver** (`src/driver.rs`): runs Qdrant in a child process (it reads its own command line)
  on a copy of the collection (a reflink where the file system has them), waits until the
  collection is green with every point, and checks it again after the load. Configuration:
  Qdrant's defaults but for loopback ports, warnings only, no telemetry, no cluster, and idle
  client connections kept for 10 minutes instead of 5 s (the load generator counts a closed
  connection as a broken run).
- **Inputs** (`fetch.sh`): the collection, 1,000,000 vectors of 384 floats around 256
  centres with a small payload, made by the driver's `build` job with Qdrant itself (about
  4 minutes on the heavy cores), and two request files: nearest-neighbour queries (10 results
  with payloads, one in five filtered by an indexed category), and the same with every 20th
  request an upsert of 20 existing points. The vectors live in memory-mapped files (Qdrant
  1.20 keeps "in RAM" vectors that way), so the page cache holds them; the heap is the HNSW
  graphs, the payload indices, the request work and, under upserts, the optimizers'.

`docs/research/servers.md`, "What each program taught us", has the measurements.
