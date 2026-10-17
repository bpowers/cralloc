# bench-rpc

A port of connect-rust's `echo_bench` and `log_bench` (github.com/connectrpc/connect-rust,
`benches/rpc` at `c312bb9`, Apache-2.0) with the allocator under test as the server's
`#[global_allocator]`.

- `rpc-server <echo|log>`: `echo_server.rs` and `log_server.rs` in one binary, on a tokio
  runtime whose workers are pinned to `BENCH_CPUS`. The log handler walks every field of a
  50-record batch through buffa's zero-copy views; the echo handler copies one string.
- `rpc-client <echo|log> addr=...`: the load loops of `echo_bench.rs` (one pooled
  `HttpClient`) and `log_bench.rs` (`conns` HTTP/2 connections), against one server, at one
  concurrency. It marks the measurement window on stdout so the runner can count the
  server's cycles over exactly that window.

The runner puts the server on one L3 domain (CPUs 0-3) and the client on the other (8-11),
and always runs the client built with glibc, so only the server's allocator changes.

Copied: `proto/bench.proto`, and the code buffa-codegen and connectrpc-codegen generated from
it (`src/generated/`). The module files are reduced to the `bench.v1` package, and the
echo_bloat types that share that package are left out. The generated client impls needed one
added bound to compile against the released connectrpc 0.9.1 (noted at the top of
`bench.__connect.rs`). The services and `log_request` are copied into `src/lib.rs`.
