# Probes

Small standalone programs that check one assumption each about the kernel, glibc, or the Rust
standard library. The research documents in `docs/research/` cite their results. They have no
dependencies; build them outside the repository tree so no binaries land here.

All of them need x86-64 Linux with glibc 2.35 or later. Results quoted below are from the
reference host (Ryzen 9950X, Linux 7.2.9, glibc 2.43) on 2026-10-16.

| Probe | Build | What it shows |
|---|---|---|
| `rseq_abort.rs` | `rustc --edition 2021 -O rseq_abort.rs -o /tmp/rseq_abort` | An rseq abort can be forced before every instruction of a critical section, unprivileged, with a perf hardware breakpoint that raises SIGTRAP. It aborts a six-instruction push at each boundary and asserts that nothing was committed and the retry succeeds. Works at `perf_event_paranoid=2`. |
| `rseq_readonly.rs` | `rustc --edition 2021 -O rseq_readonly.rs -o /tmp/rseq_readonly` | Writing the top bit of `cpu_id_start` (tcmalloc's cached-slab trick) is not undone by the kernel on `yield` or `spin`, and `migrate` kills the process with SIGSEGV. The kernel no longer rewrites the ID fields on every context switch and enforces them as read-only. |
| `rseq_auxv.rs` | `rustc --edition 2021 -O rseq_auxv.rs -o /tmp/rseq_auxv` | `AT_RSEQ_FEATURE_SIZE=33`, `__rseq_size=33`: glibc registers the full feature size. `__rseq_offset` differs between binaries (it follows the static TLS layout). |
| `vec_excess.rs` | `rustc +nightly --edition 2021 -O vec_excess.rs -o /tmp/vec_excess` | `Vec<u8, A>` ignores the excess length an `Allocator` returns: capacity stays 10 for a 64-byte block, and `deallocate` receives the requested size. On stable 1.99 the file does not compile (E0658), which shows the trait is still gated there. |
| `miri-os/` | `cargo +nightly miri run -- <op>` in that directory | A global allocator over `mmap` runs under Miri, including with threads. Miri rejects `madvise`, `partial_munmap`, `mprotect`, `prot_none`, `noreserve` and `asm`. |
