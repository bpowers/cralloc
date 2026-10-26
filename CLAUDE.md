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
- The gate (`scripts/gate`) is formatting, lints, a check that generated files were not
  edited by hand, a check that safety comments and ledger entries name each other, and the
  tests and proofs that take a few seconds each. It runs in well under a minute. Run it
  before asking for a merge, and never add a slow step to it: a test or proof that takes
  more than a few seconds is marked slow and runs with `scripts/gate --all`, which you run
  before asking to merge a change to shared types or protocols. Benchmarks, the Lean check,
  TLA+, Miri, fuzzing and the preload checks run on demand. The slow steps have time limits
  that count the time heavy work spends paused for a measurement, so on a busy machine one
  can time out (exit 124, or a proof marked FAILED with no failing check): rerun that step
  alone with `ASMALLOC_TEST_TIMEOUT` or `KANI_TIMEOUT` raised before reading it as a failure.
- The machine is shared. Stop only processes you started, by process ID; never kill by
  name (`pkill`, `killall`), since another teammate may be running the same tool.
- Timed measurements do not share cores, or a cache, with anything else. Run every build,
  test, model check and proof through `scripts/heavy`: it runs the command at low priority
  on the cores the benchmark suite does not pin to. Run every timed measurement through
  `scripts/measure`, one teammate at a time, for minutes, not hours (split a long session
  into pieces); a measurement that also uses cores of the second L3 takes `--both-l3`,
  which pauses heavy work while it runs. Do not take the lock files by hand, and do not
  start heavy work any other way, or a measurement cannot pause it.
- Everything that runs the allocator runs under a memory cap. An uncapped test once took
  57 GiB and the machine with it. `cargo test`, `cargo run` and `cargo bench` are capped by
  the workspace's cargo runner (`scripts/run-capped`: 4 GiB unless `ASMALLOC_TEST_MEM` says
  otherwise). Anything started another way, such as a benchmark runner, a fuzzer, a preload
  run or a probe built with `rustc`, goes through `scripts/memlimit`. Never set a cap above
  16 GiB, and run one memory-heavy job at a time.
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
  calls. The exception is the default `background` feature: it creates the background
  thread through the C library, so that the library knows the thread exists. Nothing else
  may call a C library function, and a build without default features refers only to what
  it must share with the library (`errno`, the rseq offset) and what the compiler emits
  (`memcpy`, `memset`). Test, fuzz and benchmark crates may depend on what they need.
- Write good, clean Rust: small modules with one job, types that make wrong states hard to
  build, `unsafe` confined to a few well-named places.
- Every `unsafe` block in code that ships (the allocator and the preload library, not the
  simulator, the twins or other test scaffolding) has a machine-checked proof (Kani, or Lean
  for the routines) or an entry in the safety ledger at
  `docs/safety/<crate>/<file>-safety.md`, named after the source file without `.rs`. An
  entry gives the preconditions, the invariants that make them hold, and a proof on paper.
  The block's comment cites the entry's ID or names the Kani harness, and the gate fails on
  one that does neither; an `unsafe fn` or `unsafe trait` is a contract, and needs a
  `# Safety` section instead.
  A fresh agent that did not write the code reviews entries adversarially and records the
  commit it reviewed in the file (`Reviewed: <commit>`). Changing a block means updating its
  entry; `scripts/ledger-check` lists the files that have changed since their review, and
  that list is cleared before a release, not on every change. Test scaffolding, and the
  test, fuzz and benchmark crates, keep `unsafe` small and commented but need no ledger.
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

A server job can move by several percent with where the program's own code lands in the
binary, which any change to the allocator's size shifts. Two builds of such a job are not
compared by one number each: switch the behaviour at run time inside one binary, or compare
means over a sweep of placements (`benchrun layout`). `docs/research/grpc-placement.md` says
which jobs need this and why.

A server's tail latency at one fixed request rate can rank the CPU's idle states instead of
the allocator: the faster server idles longer, its CPUs sleep deeper, and the wake-up lands
in the tail. The runner records how often each server run's CPUs went into a deep idle
state, and the reports flag a tail comparison that this confounds; a flagged difference is
not the allocator's. `scripts/deep-idle` shows the setting and, with root, switches the
deep states off. `docs/research/h2-tail.md` has the evidence.
