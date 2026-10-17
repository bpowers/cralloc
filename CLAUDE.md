# asmalloc

A memory allocator for multithreaded Rust programs on modern Linux: per-CPU caches under
restartable sequences, assembly fast paths generated from and proved in Lean, and lock-free
slow paths in Rust.

Read `docs/design.md` before changing anything. It says what we are building and why, and it
is the source of truth when code and intent disagree. `docs/research/` holds the measurements
and prior-art notes behind it.

## Platform

x86-64, Linux 7.2 or later, glibc 2.43 or later, Rust 1.99 or later on stable. Nightly is
only for tools that need it (Miri, Kani). Lean is pinned in `lean/`. `third_party/` holds
local reference checkouts, is gitignored, and is read-only.

## How we work

We are building the whole allocator before polishing any part of it. Move quickly and keep
process light.

- Trunk-based. Do each task on its own branch in its own git worktree; the lead merges to
  `main`.
- Merge early and fix forward. A semantic conflict or a regression on `main` is something to
  fix next, not a reason to hold work back.
- The gate is formatting, lints, unit tests and the quick proofs, about a minute in total.
  Run it before asking for a merge. Never add a slow step to it. Benchmarks and long proofs
  run on demand.
- Stay inside your component's directory. Do not edit shared files (the workspace manifest,
  this file, `README.md`, `docs/design.md`) unless that is your task. If you need a change in
  another component, tell the lead.
- A major piece gets one adversarial review from a fresh agent. Fix material findings and
  move on; a large finding gets its fix checked by the same reviewer, not a new review.
- Commit messages: imperative subject, body says why. No emoji anywhere. No generated-by or
  co-author trailers.

## Documentation

- `docs/design.md` changes when the design changes, and not otherwise. It has no plan or
  status section.
- Keep facts that go stale out of this file, `README.md` and the design: no file lists, no
  counts derived from the code, no benchmark numbers. Numbers belong in `docs/research/`
  with the date and method.
- Write plainly and briefly, for a reader who has none of your context. Lead with what
  matters most.
- Comments say why, or something non-obvious. They do not restate the next line.

## Correctness rules

- All assembly is generated from Lean. Never write `asm!`, `naked_asm!` or `global_asm!` by
  hand in the allocator. The one exception is the system-call instruction in the OS
  interface, which has a ledger entry.
- The allocator crate has no dependencies, not even on a C library: it makes its own system
  calls. Test, fuzz and benchmark crates may depend on what they need.
- Write good, clean Rust: small modules with one job, types that make wrong states hard to
  build, `unsafe` confined to a few well-named places.
- Every `unsafe` block has a machine-checked proof (Kani, or Lean for the routines) or an
  entry in the safety ledger at `docs/safety/<crate>/<file>-safety.md`, named after the
  source file without `.rs`. An entry gives the preconditions, the invariants that make them
  hold, and a proof on paper, and is reviewed adversarially by a fresh agent that did not
  write the code. Changing the block means updating the entry and getting it reviewed again.
  This rule covers the allocator and the preload library. Test, fuzz and benchmark crates
  keep `unsafe` small and commented but need no ledger.
- The concurrency rules in the design hold everywhere: no locks, no thread-local state,
  shared state changes by one atomic instruction or one rseq commit, and all-zero is a valid
  state.
- The allocator is `#![no_std]` and `#![deny(unsafe_op_in_unsafe_fn)]`. It never allocates,
  panics, unwinds or prints while serving a request.
- Use offset arithmetic and `ptr.map_addr`/`with_addr`, not integer-to-pointer casts, so
  Miri and Kani can follow the code.
- The core is generic over the OS interface so tests, fuzzing and proofs run against a
  simulated OS.
- Run Kani only through the memory-capped wrapper in `scripts/`. An uncapped CBMC run can
  take the machine down.

## Measuring

Count cycles and instructions with hardware counters, pin to idle cores, and interleave the
things being compared. `docs/research/zen5-roofline.md` describes the method. A difference of
a cycle or two on a fast path is a finding to explain, not noise: the cause is usually
instruction selection or layout, and we fix it by construction.
