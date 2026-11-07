# How we know cralloc is correct, and which tools we chose

No single tool covers the allocator, so each layer gets the tool that fits it. Lean proves the
assembly, Kani the arithmetic, TLA+ the protocols as designed, and our own schedule explorers run
the Rust that implements them. Everything else rests on tests, on written proofs that a fresh
reviewer attacks, and on a short list of trusted assumptions about the kernel and compiler.

**No proof covers a run of operations on a whole heap.** The seams between tools are tests and
arguments on paper. The strongest statement we can make is the design's: every operation behaves as
one atomic step of an abstract allocator (`docs/design.md`, "Testing and Verification"). The
routines meet it by proof. The slow paths meet it by bounded checks, bounded schedule exploration
and heavy testing.

| Tool | What it establishes | What it cannot see |
|---|---|---|
| Lean, on the routines | every input, every interruption point, memory safety, clobbers, against the instruction and rseq models | the Rust slow paths, the kernel, the assembler |
| Kani | arithmetic and packed words for every input; twins on every state of one small slab | concurrency, assembly, nested retry loops |
| TLA+ with TLC | every interleaving of a protocol's model, a few threads and objects | the code, weak memory, flaws needing four or more views of a word |
| Schedule explorers | the production Rust under every schedule within a bound, with forks, in scenarios that follow the models, on small heaps | weak memory, anything past the preemption bound |
| Model tester and fuzzers | what a program can see, through all three interfaces | interleavings it does not happen to hit |
| Miri | undefined behaviour and data races on a simulated heap, under weak-memory emulation, one seed a test | the real routines and kernel; tests over its time limit; a weak-memory flaw its one seed misses |
| Soak (a stress harness that checks every block), breakpoint sweep, real programs | the shipped library on the real kernel, aborted at every instruction | how much of the state space they reached |
| Safety ledger | a written proof of each `unsafe` block, adversarially reviewed | anything a reviewer misses; a review lags the code |

Counts at `85cb393`, 2026-11-06: Lean 4.34.1, 10 routines; Kani 0.67.0, 44 quick and 24 slow
harnesses; TLC, 12 specs and 74 configurations, 30 of them expected to fail and 4 run only with
`SLOW=1`; about 1,020 tests, 104 marked slow; 111 ledger entries in 32 files.

## Where the defects were found

This is what the records keep, not a census. Kani, the explorers, the model tester and the fuzzers
are absent because what they catch during a change is fixed in that change and leaves little record.

- **TLA+:** two drainers sharing one marker value; a child's sweep racing new drains; a fork from a
  handler that interrupted a drain, which sank the sweep and led to generations; a summary bit left
  clear in a fork child; a too-small heap chosen when start-up racers share a limited address space;
  a walk acting on a region whose tag a losing assigner gave back. `tla/models.sh` keeps each as a
  configuration expected to fail. Most came before the code, which is the point.
- **Review of the ledger:** `Allocator::grow_zeroed` left a block's tail uncleared, which the trait
  as stabilised forbids; the thread's start waiting for ever in a fork child on a loader lock; a
  reading of the environment that did not hold on musl.
- **Miri:** no undefined behaviour in the allocator. It found a data race in the simulator's page
  check, and that the simulated fence gave too little ordering under weak memory (5 of 11 seeds),
  both in scaffolding.
- **Real programs:** redis's test suite found a moving `realloc` copying only the recorded size,
  while `malloc_usable_size` promised callers room to the end of the page.
- **The kernel audit:** a garbage `mm_cid` after a failed `execve`
  ([linux-rseq-contract.md](linux-rseq-contract.md)).

A sweep of every check on main on 2026-10-27 (the full gate, Kani, Lean, the fuzzers, all TLA+
models, the preload checks and soaks, thirty real programs, 200 runs of the unit suite) found no
allocator defect.

## Lean: what the routines' proofs say

Each routine is a Lean term, printed as Rust, as an `asm!` block in an inline function or a naked
function. For each we prove termination without a fault, hence memory safety; the specified step on
the slab; and that wherever the OS may abort it, nothing the specification sees has changed and the
registers the restart needs are intact. Any number of aborts is covered. Non-vacuity is proved too:
concrete witness states take every branch, so a contradictory precondition cannot pass.

The proved artifact is the shipped artifact. `lean/scripts/check.sh` fails if the generated files
differ from what Lean emits, runs golden tests of the printers, replays every module through the
kernel, audits axioms, and assembles each naked function to check its size and alignment. It takes
minutes, and on success records checksums the merge gate compares. A negative test injects a hidden
axiom and two forged proofs and requires the check to reject them. The emitter refuses an inline
routine over its micro-op limit (`Rust.checkUops`) and a sequence whose last step is not a plain
store (`Rust.checkRseq`).

What must be trusted is `lean/Cralloc/TCB/` (instruction semantics, the rseq interruption model, the
printers, the meaning of `asm!` operands), each routine's contract in `lean/Cralloc/Spec/`, Lean's
kernel, and the assembler. Nobody outside this project has reviewed the `%fs`, rseq and `asm!` parts
of that base, and a wrong model proves nothing. The breakpoint sweep is what tests the rseq axiom
against the real kernel. The axiom is plain: before any step of a sequence's body, control may go to
the abort handler with registers, flags and FS base kept, and memory changed as the contract's
environment allows. Termination rests on a fairness assumption: of a thread's attempts, one is at
last left to finish.

### What the verified-garbage fork taught us

We forked the Lean framework of verified-garbage, a project proving assembly cryptography, at
`bbeedea2` (Apache-2.0 OR BSD-3-Clause, the same as ours; attribution in `lean/LICENSES/`).

- **Author assembly as terms and print it; parse nothing.** Memory safety follows from the model:
  every load outside the permitted regions faults, so proving no fault proves the footprint.
- **Dropping the constant-time machinery dropped Mathlib.** The framework depends on Lean core
  alone.
- **The gaps it had were real, and we filled them:** `%fs`, a `Code.rseq` constructor with abort
  states, inline rendering, exits to unverified Rust by tail call. Each became new trusted text.
- **A call and return cost 4 cycles whatever the body,** so naked functions alone would have cost
  8.0 cycles a pair against about 5 inlined. Inline `asm!` needed a proof obligation about declared
  clobbers in place of a calling convention.
- **Keep cross-thread atomics out of the proved routines.** A model of x86-TSO with store buffers is
  a research project. The routines hold no locked instruction. Cross-thread paths are Rust atomics,
  checked by interleaving.
- **State the shared invariant once.** The layout constants the proofs assume are emitted with the
  routines, and a test holds the Rust to them.
- **Never set a resource limit to make a proof pass.** A proof that hits one is too slow, so make it
  faster.
- **Proofs freeze design.** Prove a routine once its shape has survived measurement, and keep Kani
  and the model tester as the always-on net meanwhile.

**Cost, against the estimate.** The first estimate was 1,000 to 3,000 lines of proof and 1,000 to
2,000 of new trusted base. At `85cb393`, 335 lines of routines carry 11,233 lines of routine proofs
and 3,404 of framework, about 34 to 1 before the framework, beside 4,748 lines of trusted base and
1,613 of specifications. verified-garbage runs about 12 to 1 over a far larger body. Our guess at
the difference is that aborts add obligations at every step.

## Kani: arithmetic for every input

The class function, division by a class size, the slot tests, the address space's parts, bitmap word
operations, every packed word and marker, and the decisions of the clocked rules (`docs/design.md`,
"What is proved, and how"). The twins too, on a slab of one magazine with an abort before any one step.

- Kani 0.67.0 pins `nightly-2025-11-21`, so the crate keeps `rust-version = "1.92"` and the
  `Allocator` trait stays behind a feature in a thin outer layer.
- It treats concurrent code as sequential and does not run assembly. Hence the twins.
- Bounded model checking does not get through the back end's nested retry loops, so no harness
  covers a run of operations on a heap. The explorers and model tests hold that ground.
- CBMC's memory accumulates across harnesses in one invocation, and it has taken a machine down.
  `scripts/kani` caps it at 6 GiB, and `scripts/kani-all` runs one harness per capped scope.

## TLA+: the protocols as designed

Twelve specs: the routines under rseq, the magazine exchange, the drain, the depot, summary bits,
the purge as a claim, start-up, fork, the assignment of regions and the boundary of huge-page
advice. The OS is an actor that stops, moves and restarts threads, runs handlers, fences and forks.

- A configuration expected to fail is kept as a test. It shows the model can see the flaw, and that
  the rule which removes it is needed. `tla/check` fails if any result differs from what
  `tla/models.sh` records.
- Two or three threads and three or four objects suffice, because a lost magazine needs one thread
  per conflicting view of a word. Every recorded counterexample lies within these bounds, most
  within twenty steps. `Depot_wrap` reaches a counter overflow by shrinking the counter.
- The other words more than one thread writes have no model: the clocked pass's words, the debt
  growth owes, the hold's looks, the hints about depot stacks. A flaw in them can cost memory
  returned late or work done twice, and cannot cost a block, because memory goes back only through a
  claim, which is modelled.
- The background thread's parking handshake has no model. The explorer covers it.

The specs still say "cid" for the index of a slab. The argument does not depend on which number it
is.

## Schedule explorers instead of loom and shuttle

The plan was loom per protocol and shuttle for the whole. The allocator crate takes no dependencies,
not even for tests, so we wrote two small checkers in the style of CHESS (`back/explore.rs`,
`front/check.rs`).

- Threads take turns, one running at a time, and switch only before an atomic operation. A
  depth-first search covers every schedule within a preemption bound. The production code runs
  unchanged.
- A schedule may fork at any step, after which all threads but one are gone. In the middle tier a
  thread may also migrate or have its sequence aborted. The gate runs the back end's scenarios one
  preemption lower than `--all` does.
- Scenarios follow the TLA+ models and check the same invariants, with a census of every magazine
  and object after each execution (`depot/interleave.rs`, `back/models.rs`).
- They explore sequentially consistent interleavings only. Weaker orders are argued from the
  orderings the code uses and from x86-64 being TSO. Miri's weak-memory emulation checks them only
  on the one seed each test gets, and the one such flaw it found showed on 5 of 11 seeds.

## Twins

Kani, Miri and the explorers cannot execute assembly, so each routine has a Rust twin with the same
specification, stepping on a simulated processor that the test controls. The routine is proved in
Lean, the twin is checked by Kani against the specification restated in Rust, and
`front/differential.rs` runs each from the same scripted slab states and requires the same result
and memory. That test cannot reach a thread with no valid CPU number, or an interruption. Twins are
test infrastructure. There is no second allocator for environments without rseq.

## Tests that run the allocator

- **Model-based testing** (`crates/cralloc-testkit`). A shadow model of live blocks checks
  alignment, overlap, usable size, zero fill, contents across resizes, and no write into live
  memory, through `GlobalAlloc`, `Allocator` and C, on one thread, on many with hand-off and thread
  exit, and across a fork. Failures minimize to a replayable script.
- **Eight mutant allocators** keep the model honest. Each kind of bug must be caught through every
  interface, within a bounded number of operations.
- **Fault injection.** The simulated OS fails any call on demand, wholly or after acting on half its
  range. Failure to obtain memory must surface as null; failure to release must be survivable.
- **Interruption of slow paths,** at each step, by a handler that allocates, by a migration, and by
  a fork the thread does or does not survive.
- **C conformance.** A table of edge cases runs against us, the C library, and broken
  implementations, which show the cases bite.
- **Fuzzing** (`fuzz/`) drives the same model from raw bytes, one target per interface, against the
  process heap. It does not steer a scheduler: the deterministic simulation first planned was
  replaced by the explorers.
- **The breakpoint sweep** (`scripts/preload-soak`). A perf hardware breakpoint with `sigtrap` stops
  the library at each instruction of each restartable sequence in turn; the kernel aborts the
  sequence, and the handler allocates and frees before it restarts. It fails if a sequence never
  fires unless it is listed with a reason. `rseq_abort.rs` is the prototype, unprivileged at
  `perf_event_paranoid=2`.
- **Real programs under the preload library** must print and exit as they do without it: shell
  pipelines, git, compressors, interpreters and a cargo build (`scripts/preload-check`), and the
  test suites of git, redis, CPython, SQLite and others (`scripts/preload-soak --suites`).
- **Miri** runs the allocator's unit tests on the simulated heap, one process and one seed a test,
  with a CPU limit ([portability.md](portability.md), "Miri").

## The safety ledger

Every `unsafe` block in shipping code has a Kani harness, a Lean routine, or an entry in
`docs/safety/<crate>/<file>-safety.md` with preconditions, invariants and a proof on paper.
`scripts/ledger-check` fails on a block that cites neither, or an entry nothing cites. A fresh agent
that did not write the code reviews entries and records the commit. Reviews lag the code by design:
at `85cb393` the check lists all 31 reviewed files as changed since review, the rename to cralloc
among the causes, and one file, `cells`, has had no review. That list is cleared before a release.

## What runs when

`scripts/gate` runs in well under a minute: formatting, the generated-file checksums, the ledger
check, lints in three feature sets and on musl, the workspace's tests, the quick Kani harnesses, and
the musl front-end tests. `scripts/gate --all` adds every ignored test, the slow Kani harnesses and
the release-only explorations, in many minutes, before a change to shared types or protocols.

On demand: `lean/scripts/check.sh` (minutes), `tla/check` (about a quarter of an hour on three
cores), `scripts/preload-check`, `scripts/preload-soak` (about 25 minutes, an hour more with
suites), the fuzzers, and `scripts/miri` (about twelve CPU hours). Everything that runs the
allocator is memory-capped, after an uncapped test once took 57 GiB and the machine with it.
Benchmarks are [measuring.md](measuring.md)'s subject.

## Tools we did not choose

- **Verus.** The only practical tool for unbounded proofs about unsafe, concurrent Rust, at the
  price of a dialect, a toolchain pin, and 4 to 13 lines of proof per line of code to redo whenever
  the design moves. verus-mimalloc (SOSP 2024) has 13,703 lines of proof for 3,178 of code, lacks
  `realloc` and aligned allocation, and completes 8 of 19 mimalloc-bench programs, 2 to 14 times
  slower than mimalloc. Worth revisiting once the design has stopped moving.
- **Aeneas.** Safe Rust only, with no concurrency. Our slow paths are neither.
- **Miri-GenMC.** It "does not support re-using freed memory", which rules it out for an allocator.
- **loom, shuttle.** A dependency, and they see only code routed through their types.
- **ThreadSanitizer.** It does not understand rseq (from memory). Miri's race detector plays that
  part on the twins.
- **`cargo-mutants`.** Not adopted; the mutants are written by hand.

StarMalloc (OOPSLA 2024) is the other verified precedent: F* and Steel extracted to C, 26K lines of
F* for 4.3K of C and two person-years, with a lock per size class and no thread caches. As far as we
found, nobody has verified a per-CPU allocator, or an allocator's fast path at the machine-code
level.

## From wasmalloc

Carried over: research before design, the floor and mimic benchmarks, an OS trait so all tests run
on a simulated OS, the model tester with mutants, fuzz targets, the ledger with fresh reviewers (its
first round found a real memory-safety bug), the fast gate, the Kani wrappers. What did not
transfer: its one-thread rule, since Kani says nothing about interleavings; one growing linear
memory, where native needs holes, partial failure and concurrent callers.

## Open questions

- **Nobody proved the link between the Rust and the TLA+ models.** The explorers follow the models'
  scenarios, a reviewed correspondence and not a refinement.
- **A run with `rseq_debug=1` has not been done.** The kernel's stricter validation would check
  every descriptor. It needs a boot parameter or root.
- **Weak memory is argued, and checked thinly.** `scripts/miri` runs each test once, on one seed,
  and many-seeds mode was part of the first plan. No tool here explores x86-TSO over the production
  code.
- **Reach is unknown.** Coverage is not measured or gated; `cargo-llvm-cov` is installed and unused.
- **Whether `LD_PRELOAD` reaches rustc's bundled jemalloc** was never settled, so the cargo build
  among the real programs may test less than it seems to.
- **Miri is not green.** 26 tests were over its limit and four failed on policy assertions on
  2026-10-28 ([portability.md](portability.md), "Miri").
- **Not proved at all:** liveness beyond the stated fairness, fragmentation bounds, performance,
  kernel behaviour, the compiler.
