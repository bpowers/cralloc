# Verification, testing and benchmarking: tools and precedents

What exists today for proving, testing and measuring an allocator whose fast paths are
assembly inside restartable sequences and whose slow paths are concurrent Rust. Researched
on 2026-10-16. `docs/design.md` states what asmalloc does; this document is the evidence and
the reasoning behind its testing and verification section.

## Summary

- **No single tool covers the allocator, so each layer gets the tool that fits it.**

  | Layer | Tool | Strength of result |
  |---|---|---|
  | assembly kernels (rseq pop, push, batch) | Lean, in a fork of verified-garbage's framework | machine-checked for all inputs and all abort points, against the instruction-set and rseq models |
  | size classes, geometry, bitmaps, C-shim arithmetic | Kani | machine-checked for all inputs, bit-precise |
  | back-end state machines (spans, bitmaps, depot) | Kani bounded harnesses on a simulated OS | bounded |
  | cross-thread protocols | a Lean or TLA+ model; loom per protocol; shuttle and Miri for the whole | model proved or exhaustively checked; implementation tested against it |
  | OS interface (mmap, madvise, rseq, membarrier, futex) | axioms | trusted; fault-injection tested |
  | everything else that is `unsafe` | safety ledger with adversarial review | reviewed, not proved |

- **Every prefix of an rseq critical section followed by an abort can be tested
  exhaustively**, unprivileged, in an ordinary test, with a hardware breakpoint that raises a
  signal. A prototype works.
- **Stable Rust `asm!` can express a whole rseq critical section**, descriptor and abort
  signature included.
- **Nobody has verified a per-CPU allocator or an allocator fast path at the machine-code
  level.** The proofs are small (5 to 10 instructions per section). The cost is in new
  trusted models that nobody has reviewed: `%fs`, rseq abort semantics, `asm!` operands.
- **Tools that cannot execute assembly or rseq (Kani, Miri, loom) need a Rust twin of each
  kernel.** The twin is test infrastructure. There is no second allocator for environments
  without rseq.
- **Benchmark baselines can silently run in the wrong mode.** google/tcmalloc falls back to
  per-thread caches under glibc 2.35 and later unless glibc's rseq registration is disabled
  (read from its source, not run), and the system `libtcmalloc.so` is gperftools, a
  different allocator.

### Decisions these findings fed

- Fork verified-garbage's Lean framework into this repository, with attribution. Both
  projects are licensed Apache-2.0 OR BSD-3-Clause. Drop its constant-time machinery.
- The primary configuration is LTO with the fast path inlined, so the emitter must render
  kernels as inline `asm!`, not only as the naked functions verified-garbage emits.
- Never write metadata into freed user memory. Frame conditions become nearly syntactic.
- No second implementation for environments without rseq (valgrind,
  `glibc.pthread.rseq=0`). There the slow path serves small objects straight from the
  back-end bitmaps, the path large objects always take. asmalloc never forwards to another
  allocator.
- Verus is not adopted now. Revisit when the design has stopped moving.
- An `unsafe` block that Kani or a model checker cannot reach gets a pen-and-paper entry in
  `docs/safety/<crate>/<file-without-.rs>-safety.md`, reviewed adversarially by a fresh
  agent, running the same model as the team lead, that did not write the code.
- Process stays light: one fast gate, merge early, fix forward. Long proofs and benchmarks
  run on demand.

## Sources and conventions

Paths are relative to `third_party/`, a directory of local reference checkouts that is not
part of this repository.

| Project | Upstream | Read at |
|---|---|---|
| verified-garbage | https://github.com/pyca/verified-garbage | `bbeedea23` (2026-10-17) |
| wasmalloc | https://github.com/bpowers/wasmalloc | `900981e` (v0.1.1 + 2, 2026-09-02) |
| tcmalloc | https://github.com/google/tcmalloc | `f3c6669a` (2026-10-16) |
| mimalloc | https://github.com/microsoft/mimalloc | v3.5.3 (`31d034d9`, 2026-09-16) |
| mimalloc-bench | https://github.com/daanx/mimalloc-bench | `69c41ed` (2026-10-02) |
| connect-rust | https://github.com/connectrpc/connect-rust | `c312bb9` (2026-10-16) |

Each claim carries a tag:

- **[S]** read in source at the commit above.
- **[X]** run or measured on the reference host (Ryzen 9950X, Linux 7.2.9, glibc 2.43,
  rustc 1.99.1, Kani 0.67.0, Miri on nightly 1.101). Probes are in `bench/probes/`.
- **[W]** fetched from the web on 2026-10-16.
- **[I]** inference.
- **[M]** from memory, not re-checked.

## 1. verified-garbage

verified-garbage is a cryptography library whose assembly primitives are written and proved
in Lean. It is the model for how asmalloc's kernels are authored.

### 1.1 Mechanics

- Assembly is authored as Lean terms and printed; nothing is parsed. The rule is "All
  assembly comes from Lean. Never write `asm!`, `naked_asm!`, `global_asm!` ... by hand"
  (`verified-garbage/CLAUDE.md:21-25`). An implementation is a `Prog isa`, a structured
  program built from `block | seq | ite | loop | call | frame`
  (`lean/VerifiedGarbage/TCB/Code.lean:128-138`). The self-test is
  `.block [.mov .rax (.reg .rdi), .alu .add .rax (.reg .rsi)]`
  (`lean/VerifiedGarbage/Impl/Selftest/X86_64.lean`). [S]
- Pipeline (`lean/README.md:62-108`): a trusted Spec (algorithm, `Sig`, `Contract`, `Api`);
  an untrusted Impl (`Prog`); an untrusted Proof of `Verified target code contract`; a
  registration file binding them into an `Artifact`; `Emit.lean` prints
  `src/asm/<target>/<module>.rs`. [S]
- The emitted form is one Rust naked function per artifact:
  `#[unsafe(naked)] pub(crate) unsafe extern "sysv64" fn name(sig) { core::arch::naked_asm!(r"...") }`
  (`lean/VerifiedGarbage/TCB/Rust.lean:8-20,398`; sample `src/asm/x86_64/selftest.rs`). The
  function ends with `ret` and `.p2align 6`, so its position relative to fetch blocks does
  not depend on link order (`TCB/Print.lean:32-42`). Hand-written safe Rust wraps these and
  is not verified (`lean/README.md:3-8`). [S]

### 1.2 The x86-64 model

- State (`TCB/X86_64/State.lean:37-66`): 16 general registers as `BitVec 64`; CF, ZF, SF
  and OF as `Option Bool`, where `none` is undefined and branching on it faults; vector
  registers; mxcsr; `mem : Addr -> Byte` (total, little-endian, wraps mod 2^64:
  `TCB/Mem.lean:23-45`); `rd` and `wr` lists of permitted regions; `unknowns` (return
  addresses the model does not know); `syms` (addresses of statics). [S]
- Memory safety is by construction. Every load faults outside `rd ++ wr` and every store
  outside `wr` (`State.lean:105-111`), so proving termination without a fault proves the
  code touched only permitted memory (`TCB/Mem.lean:9-13`). [S]
- A memory operand is `[base + index*scale + disp]` only (`State.lean:69-75,99-102`). There
  is no segment override, so no `%fs`. [S]
- Instructions (`TCB/X86_64/Isa.lean:210-380`): `mov`, `store`, `alu` (add adc sub sbb and
  or xor cmp test) in 64- and 32-bit forms, immediate shifts, shifts by CL, `shlx/shrx`,
  `rorx`, `andn`, `bswap`, `movzx8`, `store8`, `movabs`, `lea [rip+sym]`, `mul`, `imul`,
  `mulx`, `adcx`, `adox`, `cmovcc`, frame `push`/`pop`, `lea rsp` for stack buffers, and the
  SSE, AVX and AVX-512 families. Conditions: only `e ne b ae` (`Isa.lean:175-195`). [S]
- Absent, and needed by an allocator: general `lea`, `inc/dec`, signed conditions,
  `tzcnt/lzcnt/bsf/bsr/popcnt/blsr`, `xchg`, `cmpxchg`, `xadd`, `lock`, `pause`,
  `prefetch`, indirect or external `jmp`/`call`, `syscall`, `%fs`/`%gs`, 16-bit operands. A
  search for each in `TCB/X86_64/*.lean` returns nothing. [S]
- Calling convention (`TCB/X86_64/Target.lean:39-45,77-106`): System V. Callee-saved
  registers, the return slot and the MXCSR control bits must be preserved (`abiPreserved`).
  The red zone is not modelled. Stack use is a contract parameter, and `rsp` may move only
  through nested frames (`TCB/Artifact.lean:333`). [S]
- Semantics are big-step, deterministic and sequential: `Exec c s trace s'`
  (`TCB/Code.lean:192-211`), with a leakage trace used only for constant time. There is no
  notion of program point, interrupt, other thread or store buffer. [S]

### 1.3 Specs and proofs

- `Verified T c k` (`TCB/Artifact.lean:230-233`): for every state satisfying `k.pre`, the
  code terminates without fault in a state satisfying `k.post` and the ABI obligations; it
  is constant time; and the precondition is satisfiable. [S]
- `Sig.contract` derives the routine part of the precondition from the Rust signature: where
  arguments are, which regions are readable and writable, disjointness, no wraparound
  (`TCB/Artifact.lean:99-133`). Parameter kinds: integers, `&[T;N]`, `&mut [T;N]`, slices,
  lists of slices (`TCB/Sig.lean`). The `# Safety` doc of each generated function comes from
  the same data. [S]
- Proof style: weakest-precondition rules over `Code`
  (`Proof/Framework/Semantics.lean:83-140`), symbolic execution one instruction at a time
  with rewrite lemmas for register updates, `omega`/`bv_omega` wrappers, kernel evaluation
  of literals, a taint checker for constant time. Banned: `sorry`, `admit`, `native_decide`,
  `bv_decide`, new axioms (`verified-garbage/CLAUDE.md:26-29`). Every declaration must fit
  the default 200,000 heartbeats (`verified-garbage/CLAUDE.md:329-333`). [S]
- Size: the trusted base is 9,969 lines for four targets, about 3,400 of them x86-64. Spec
  38K lines, Impl 181K, Proof 2.19M in 11,764 files: about 12 proof lines per
  implementation line overall. MD5 for x86-64 is 150 lines of implementation and about
  1,000 plus 535 shared lines of proof. All of it was written by LLMs in three weeks (2,317
  commits from 2026-09-26). The Lean build cache is 10.5 GB unpacked and CI shards the
  build (`verified-garbage/CLAUDE.md:12`). [X]
- Toolchain: Lean `v4.34.1` and Mathlib `v4.34.1` (`lean/lean-toolchain`,
  `lean/lakefile.toml:60`). Mathlib is used sparingly and is the main build cost. [S]

### 1.4 Trusted base and CI

- Trusted (`lean/README.md:130-138`): `TCB/` (instruction-set models against the vendor
  manuals, printers, `Sig` and ABI, emitter), each artifact's contract in `Spec/`, Lean's
  kernel, and the assembler in rustc and LLVM. Process rules protect it: changes to `TCB/`
  and `Spec/` are separate from implementation changes (`verified-garbage/CLAUDE.md:30-42`),
  instruction-set changes cite the manual, and `TCB/` imports only Lean core. [S]
- The proved artifact is the shipped artifact: `lake build` checks every proof with warnings
  as errors; `lake env lean --run Emit.lean --check` fails if `src/asm/` differs from what
  Lean generates (`lean/README.md:105-106`, `verified-garbage/CLAUDE.md:717`);
  `#assert_standard_axioms` audits every artifact; `ci/audit_negative.py` injects a `sorry`
  or an axiom and checks that the audit rejects it; golden tests cover the printers
  (`lean/VerifiedGarbageTest/`); the emitter refuses `implemented_by`, `extern` and `csimp`
  so the compiled emitter is what the kernel checked (`verified-garbage/CLAUDE.md:95-99`).
  [S]
- Rust side: 100% line coverage merged across platforms, known-answer tests with
  provenance, Intel SDE for CPU-feature variants, and a benchmark per public API compared
  against the base with a slowdown gate (`verified-garbage/CLAUDE.md:43-59`). [S]

### 1.5 What transfers to allocator fast paths

Reusable as is [I]:

- The memory and region model, `Code`, `Exec`, the weakest-precondition framework, the
  general-register semantics for `mov/store/alu/cmov/shift`, the printer and emitter, the
  axiom audit and its negative test. A fork without the constant-time machinery needs no
  Mathlib and should build in seconds.
- The process: the TCB/Spec/Impl/Proof split, trust changes on their own, generated code
  checked in and diffed by CI, and its notes on proof speed
  (`verified-garbage/CLAUDE.md:324-529`).

Missing, with a sketch and an estimate [I]:

a. **`%fs` TLS (small).** Add `fsBase : BitVec 64` to the state and a segment field to the
   memory operand; the effective address adds the base. The thread block is one more
   region. The rseq area is at `fs:[__rseq_offset]`, a load-time constant from glibc that
   differs between binaries (-320 in one probe binary, -1344 in another with a larger
   static TLS block). The TLS relocation rules are new trusted printer text.

b. **More instructions (small, but every line is trusted).** General `lea`,
   `tzcnt/blsr/popcnt` for bitmaps, signed conditions, `inc/dec`, `xchg`, `lock cmpxchg`,
   `lock xadd`. Each is 5 to 20 lines of semantics transcribed from the manual plus rewrite
   lemmas.

c. **Inline rendering (medium, required).** A naked `extern "sysv64"` function costs a call
   and a return. On the reference host a call and return cost 4 cycles whatever the body,
   so an out-of-line alloc+free pair costs 8 cycles against 2 to 4 inlined
   (`docs/research/zen5-roofline.md`). The primary configuration therefore needs an emitter
   mode verified-garbage does not have: an `#[inline(always)] unsafe fn` containing `asm!`
   with `in`, `out` and clobber operands. The proof obligation that replaces `abiPreserved`: the code writes only
   the declared outputs and clobbers (a decidable check over instruction write sets), does
   not touch the stack (`options(nostack)`), and leaves the direction flag clear. New
   trusted text: the meaning of Rust `asm!` operand classes. The emitter is also the place
   to enforce the code-layout rule from the roofline study, since it sees every byte.

d. **Calls out to unverified Rust (medium).** `Code.call` only calls another verified
   artifact whose body the model runs (`TCB/Code.lean:20-24`). Slow paths need an "external
   call with a contract" node: the model havocs caller-saved registers and assumes the
   callee's postcondition. That postcondition is exactly what the Rust side must discharge,
   so this node is where the two verification worlds meet. The semantics is easy; the
   discipline around it is the hard part.

e. **rseq critical sections (medium).** The model has no program points. Proposal: a new
   `Code` constructor `rseq (enter : Instr) (body : List Step) (commit : Instr)
   (abort : Code)`, where a step is an instruction or a conditional forward exit, the
   printer emits the descriptor, signature and labels, and the semantics is
   nondeterministic: before any step, including the commit, control may transfer to `abort`
   with the current registers and memory. `execBlock` is already a fold over a list
   (`TCB/Code.lean:184-189`), so "the state after the first k steps" is definable with
   `List.take`. Obligations per section:
   1. Abort safety: for every k, the state after k steps has the same abstract value as the
      entry state (stores before the commit touch only slots the abstraction ignores), and
      the registers the abort path reads are unchanged.
   2. Commit: the full run takes the abstract state to the specification's successor in one
      store.
   3. Footprint: every access is inside the slab of the ID read inside the section.
   4. Restart terminates given finitely many aborts (a fairness assumption, stated and not
      proved).

   The kernel's promise becomes a trusted axiom: an abort happens whenever the thread is
   preempted, migrated or takes a signal with its instruction pointer inside the section,
   and nothing else changes. It is small, and the breakpoint harness of section 4.2 tests
   it. Sections are 5 to 10 instructions, so obligation 1 is a handful of cases.

   A consequence: because signal delivery aborts the section, per-CPU operations are atomic
   with respect to signal handlers on the same thread as well as other threads on the same
   CPU. Slow paths under locks are not.

f. **Shared heap state through pointers (small for arrays, medium for lists).** A contract's
   precondition is an arbitrary predicate over the state, so slab shapes can be stated
   directly: the header and `slots[0..cap)` are writable regions, `len <= cap`, slots below
   `len` hold pointers to pairwise-disjoint blocks. A per-CPU array stack needs no
   separation logic. An intrusive free list does (list segments, disjointness of every
   node), which is one more reason to prefer array caches in the proved kernels.

g. **Cross-thread atomics under x86-TSO (hard; avoid).** The model is single-threaded.
   Building a TSO model with store buffers and proving linearizability over it is a research
   project. The cheaper sound route: write cross-thread paths in Rust with
   `core::sync::atomic`, verify the protocol in an interleaving model, and add the syntactic
   side condition that justifies sequential consistency on x86: every write another thread
   can race with is a locked instruction (Owens, "Reasoning about the Implementation of
   Concurrency Abstractions on x86-TSO", triangular-race freedom). [M] Recommendation: no
   lock-prefixed code in the Lean-emitted kernels to begin with.

h. **The specification bridge.** The assembly proofs assume a heap layout invariant that the
   Rust slow paths must establish and preserve. Stated twice in two logics, that invariant
   is trusted glue. Keep it tiny (the per-CPU slab header layout), state it once in Lean,
   and generate the Rust constants, the debug `validate()` and golden test vectors from that
   one definition.

Net [I]: the allocator's assembly is perhaps 100 to 300 lines (pop, push, batch refill,
batch flush, maybe size-class lookup). At verified-garbage's ratios that is 1,000 to 3,000
lines of proof plus roughly 1,000 to 2,000 lines of new trusted base (items a to e). The
proofs are the cheap part.

## 2. Tool landscape

### 2.1 Kani

- Version 0.67.0 is installed: a CBMC-based bounded model checker pinned to
  `nightly-2025-11-21` (`~/.kani/kani-0.67.0/rust-toolchain-version`). [X] Version 0.68.0
  (2026-09-16) moves to `nightly-2026-08-21` and adds `loop_decreases`, stub sets,
  trait-method stubbing, quantifier arithmetic and parallel autoharness. [W]
- Kani lags stable by months, so the verified core must not depend on newer APIs, the
  newly stabilised `Allocator` trait included. Keep trait impls in a thin outer layer.
- "Concurrent features are currently out of scope for Kani"; it compiles concurrent code as
  sequential with a warning. "Kani does not support assembly code". Function contracts, loop
  contracts and stubbing exist behind `-Z` flags. [W]
- wasmalloc's experience [S]: 13 quick harnesses run in about 30 s in the merge gate and
  the full set in about 10 minutes (`wasmalloc/scripts/kani-quick`,
  `wasmalloc/CLAUDE.md:33-43`). CBMC once took the whole machine down, hence a mandatory
  cgroup wrapper (`wasmalloc/CLAUDE.md:26-31`, `wasmalloc/scripts/kani`). Harnesses run in
  one invocation accumulate memory and must run one per process
  (`wasmalloc/docs/soundness-ledger.md:20-26`).
- Fit [I]: excellent for bit-level facts over all inputs (size-class math, alignment by
  construction, block-inside-span geometry, bitmap operations, overflow in `calloc`) and for
  bounded operation sequences on a small simulated heap. It does not prove an unbounded
  data-structure invariant and is blind to concurrency and assembly.

### 2.2 Verus

- Actively released (latest `0.2026.10.04`): SMT-based, code written in the `verus!{}`
  dialect with ghost permissions, concurrency through tokenized state machines and atomic
  invariants. It pins its own Rust toolchain. [W]
- verus-mimalloc (SOSP 2024) [W]: 282 lines of trusted specification, 13,703 lines of proof,
  3,178 lines of executable code (4.3 to 1), verifies in about a minute. No `realloc`, no
  aligned allocation, nothing above 128 KiB, no thread cleanup. It completes 8 of 19
  mimalloc-bench programs and is 2 to 14 times slower than mimalloc on those (cfrac 9.7 s
  against 4.6 s; sh6bench 2.0 s against 0.14 s).
- Fit [I]: the only practical tool today for unbounded proofs about unsafe, concurrent Rust.
  The cost is a dialect, a toolchain pin, restricted idioms (pointer arithmetic,
  thread-locals and assembly go through trusted wrappers) and 4 to 13 lines of proof per
  line of code that must be redone when the design moves. The precedent's slowdown came from
  implementing a subset, not from ghost code, but it shows how hard it is to keep every
  optimisation while proving. Not adopted now.

### 2.3 Lean and Aeneas

- verified-garbage pins Lean 4.34.1 through `lean-toolchain`. [S] Lean is installed per user
  with `elan`; without Mathlib the build is light.
- Aeneas translates "a subset of safe Rust" to pure Lean functions. Unsafe code and
  concurrency are unsupported, as are some loop shapes. [W]
- Fit [I]: Lean hosts the assembly proofs and is the natural home for the abstract allocator
  specification and the protocol models, which gives one logic for the top-level statement.
  Aeneas could later pull safe-Rust parts into the same logic. That is a design lever: the
  more of the slow path that is safe Rust over index-addressed metadata, the more of it can
  reach Lean.

### 2.4 Dynamic checkers

- Miri [X] (`bench/probes/miri-os/`): runs a custom `#[global_allocator]` over `mmap`, with
  threads, under `-Zmiri-strict-provenance`. It rejects inline assembly, `madvise` with
  `MADV_DONTNEED`, partial `munmap`, `mprotect` and `PROT_NONE`, `MAP_NORESERVE`, and any
  integer-to-pointer cast. So the allocator needs an OS-backend trait with a Miri backend,
  a Rust twin of every assembly kernel, and strict-provenance pointer discipline from the
  start. Miri also has a data-race detector, weak-memory emulation and many-seeds mode. [M]
  Miri-GenMC is experimental and "does not support re-using freed memory" [W], which rules
  it out for an allocator.
- loom [M]: exhaustive exploration of small tests under a C11-like model, a few threads,
  atomics routed through its types. Right for each lock-free protocol in isolation.
- shuttle [W]: randomized scheduling that scales to whole-allocator tests, with replayable
  seeds and no weak memory.
- ThreadSanitizer [M]: usable on the Rust paths with the twins; it does not understand
  rseq.
- TLA+ or a Lean transition system [I]: for the few protocols whose correctness is
  non-local (remote hand-off, ownership transfer, draining a per-CPU slab against running
  fast paths).

### 2.5 Prior verified allocators

- verus-mimalloc: section 2.2.
- StarMalloc (OOPSLA 2024) [W]: F* and Steel, extracted to C. About 26K lines of F* for 4.3K
  lines of C, two person-years. Proves memory safety, functional correctness and thread
  safety with a lock per size class and no thread caches. Its performance is compared with
  hardened_malloc (0.84x to 1.38x, one 16x outlier), not with fast allocators.
- The VST-verified malloc (Appel and Naumann), the seL4 and CertiKOS kernel allocators and
  CompCert's memory model are sequential, simple designs. [M]

### 2.6 What to prove

The top-level statement to aim for [I]: every operation refines an atomic step of an
abstract allocator whose state is the set of live blocks with their layouts and the set of
owned address ranges. `alloc` returns null or an aligned block of at least the requested
size, inside owned memory and disjoint from every live block. `dealloc` of a live block with
its layout removes it. `realloc` preserves the common prefix. The allocator never reads or
writes a live block's bytes except where the API says so, and never touches memory it does
not own. Concurrent histories are linearizable to this.

What can honestly be claimed first: that statement in Lean for the abstract model and for
the assembly kernels against the slab invariant; Kani proofs of all arithmetic; bounded Kani
proofs and heavy model-based testing that the Rust code preserves the slab and span
invariants. The link between the Rust code and the Lean model stays a reviewed argument. Not
proved at all: liveness, fragmentation bounds, performance, kernel behaviour, the compiler.

## 3. How wasmalloc does it

wasmalloc is a single-threaded pure-Rust allocator for wasm by the same author as this
project. Its process is the starting point.

- Research before design: `wasmalloc/docs/research/`, then one design document. [S]
- The roofline method (`wasmalloc/bench/roofline/README.md`, `docs/research/roofline.md`):
  "floor" allocators that are the cheapest code able to serve a workload, the incumbents,
  and "mimic" variants that reproduce the real allocator's memory traffic with one detail
  changed each. That attribution found a 16-bit counter costing 2 ns per pair through a
  failed store-forward. Results are committed and rendered to a report; runs are pinned and
  memory-capped. [S]
- "Every unsafe block is proved or reviewed": a Kani harness by default
  (`wasmalloc/CLAUDE.md:77-79`); otherwise an entry in `docs/soundness-ledger.md` (22
  entries) with exact preconditions, the invariants relied on, a proof sketch, the machine
  checks that partly cover it, a change log, and sign-off by a fresh adversarial reviewer
  that did not write the code (`wasmalloc/CLAUDE.md:80-83`). The first review round found a
  real memory-safety bug (`docs/soundness-ledger.md:970-990`). [S]
- The core is generic over a `Memory` backend, so all tests, fuzzing and proofs run on the
  host against simulated memory (`wasmalloc/CLAUDE.md:89-91`). [S]
- A model tester with profiles (`tests/model_heap.rs`); a mutant suite showing the model
  catches each bug class within a bounded number of operations
  (`tests/model_mutants.rs:1-3`); two libFuzzer targets whose input bytes are the operation
  stream (`fuzz/README.md`). [S]
- A merge gate held under a minute (`wasmalloc/CLAUDE.md:65-67`); long proofs and
  benchmarks on demand; `scripts/memlimit` around anything memory-hungry. [S]
- Rules: `no_std`, `deny(unsafe_op_in_unsafe_fn)`, strict-provenance pointer style, never
  allocate, panic or print while serving (`wasmalloc/CLAUDE.md:84-88`). [S]

Carried over: research-then-design documents, the roofline, floor and mimic method, the OS
backend trait, the model tester with mutants, fuzz targets, the ledger with adversarial
review (one file per source file, at the path given in the summary), the fast gate, the
Kani wrapper scripts.

Does not transfer:

- "One thread: no atomics, no TLS, no locks" (`wasmalloc/CLAUDE.md:102`). Kani says nothing
  about interleavings, so the pen-and-paper share of the ledger grows unless a concurrency
  tool is added. Lean models, loom and shuttle fill this gap.
- A simulated linear memory is one growing region. Native needs mmap, munmap, madvise, huge
  pages and their failure modes; the simulated backend must model address-space holes,
  partial failure and concurrent callers.
- A page header address that is a pure function of the `Layout`. That serves Rust's sized
  dealloc; the C `free(ptr)` path gets its class from the address instead, which is a new
  proof surface.
- Benchmarks through JavaScript engines.

## 4. Test strategy

### 4.1 Layers, cheapest first

1. The quick Kani set (arithmetic, geometry), the Lean build of the kernels, and the
   emitter's `--check`.
2. Unit tests of each module against a simulated OS backend, with a `validate()` walker over
   every span, slab and list after each step in a checked build.
3. A model tester ported from wasmalloc. A shadow model tracks live blocks in an interval
   map and checks alignment, non-overlap, `usable_size >= size`, zero-fill for
   `alloc_zeroed`, and the prefix preserved by `realloc`, `grow` and `shrink`. It fills
   every block with a pattern derived from address and size and re-checks it on free, which
   catches the allocator writing into live memory. Because the allocator never writes into
   freed memory, a checked build can also poison freed blocks and verify the poison on
   reuse. Mutant allocators keep the oracle honest.
4. A multithreaded model tester: per-thread operation streams with hand-off queues so
   blocks are freed by other threads, and thread creation and exit mid-stream. The oracle
   is sharded per block so it does not serialise the allocator and hide races.
5. Deterministic simulation: the fuzz input also drives a virtual scheduler that chooses
   which virtual thread runs, on which virtual CPU (a fake ID source, as tcmalloc's
   `ScopedFakeCpuId`, `tcmalloc/tcmalloc/internal/percpu_tcmalloc_test.cc:220`), and where
   the Rust twin of a critical section is aborted or migrated. One OS thread, fully
   replayable.
6. loom for each lock-free protocol in isolation; shuttle for whole-allocator random
   schedules; Miri with many seeds, the race detector and weak-memory emulation on the same
   tests, using the Miri OS backend and the twins.
7. Fault injection in the OS backend: fail the n-th `mmap`, `ENOMEM` from `madvise` and
   `mprotect`, address-space exhaustion, `MAP_FIXED_NOREPLACE` collisions, huge pages
   unavailable. Every failure must surface as null or `AllocError`, never a panic or
   corruption.
8. Real-kernel stress: migration storms with `sched_setaffinity`, sparse cpusets, signal
   storms whose handlers allocate, `membarrier` storms, thread churn, fork under load.

### 4.2 rseq

- **Exhaustive abort injection** [X] (`bench/probes/rseq_abort.rs`). For each critical
  section and each instruction boundary k, arm a hardware execution breakpoint
  (`perf_event_open`, type breakpoint, `sigtrap=1`, `remove_on_exec=1`,
  `exclude_kernel=1`), run the operation, let the SIGTRAP handler disarm it, then assert:
  the operation observed an abort, no abstract state changed, the retry succeeds, and the
  shadow model agrees. The prototype aborts a six-instruction push before each instruction
  and passes at `perf_event_paranoid=2`, so it runs on a default-configured machine. The
  boundary table should be emitted by the same Lean term that emits the code, so it cannot
  drift. Second level: the handler itself performs a malloc or free on the same ID's slab
  before returning, which is both "another thread ran on this CPU while we were preempted"
  and "a signal handler allocated". x86 has four debug registers, enough for one section at
  a time.
- **Stable `asm!` can carry the whole section** [X]: the `__rseq_cs` descriptor through
  `.pushsection`, the abort signature bytes, and an instruction-boundary table exported
  through a `__start_<section>` symbol. Named labels are rejected (`named_asm_labels` is
  deny by default); numeric labels work, which is what verified-garbage's printer emits
  (`verified-garbage/lean/VerifiedGarbage/TCB/Print.lean:125`).
- **Differential checks** of the assembly against its Rust twin over all small slab states
  (every length and capacity up to a bound), and of both against vectors evaluated from the
  Lean specification.
- **The kernel's rseq mode** [X][W]. glibc 2.43 registers the full feature size
  (`AT_RSEQ_FEATURE_SIZE=33`, `__rseq_size=33`, `AT_RSEQ_ALIGN=64`:
  `bench/probes/rseq_auxv.rs`), which per the kernel documentation selects the optimized
  mode with read-only ID fields. Measured (`bench/probes/rseq_readonly.rs`): after setting
  the top bit of `cpu_id_start`, 1000 `sched_yield`s and a 300 ms spin leave the bit set,
  and the first migration kills the process with SIGSEGV. tcmalloc's cached-slab-pointer
  trick is not available under glibc's registration. Only these two behaviours were tested.
- **Kernel knobs** [W][X]: the boot parameter `rseq_debug=` and the root-only debugfs file
  `rseq/debug` turn on stricter kernel validation. The reference host's Fedora kernel has
  `CONFIG_RSEQ_DEBUG_DEFAULT_ENABLE`, `CONFIG_RSEQ_STATS` and `CONFIG_RSEQ_SLICE_EXTENSION`
  all unset (`/boot/config-7.2.9-201.fc44.x86_64`). One run with `rseq_debug=1` is worth
  doing.
- **Environments without working rseq** [M]: under ptrace single-stepping every step
  schedules the tracee out, so a section stepped through in gdb never commits. rr behaves
  the same way. Valgrind does not implement rseq, and glibc then reports `__rseq_size == 0`.
  Where the registration is missing (valgrind, `GLIBC_TUNABLES=glibc.pthread.rseq=0`) the
  allocator detects it at first use and runs without magazines, so the slow mode needs the
  same tests as the fast one. Single-stepping through a sequence remains unsupported.
- **Kernel selftest style** [M]: delay loops, yields and signals injected at numbered points
  (`tools/testing/selftests/rseq/param_test.c`). Useful as stress, weaker than the
  breakpoint method.
- **tcmalloc precedent** [S]: `percpu_tcmalloc_test.cc` stresses 5 slabs of capacity 4 with
  fake CPU ids; `percpu_tcmalloc_fuzz.cc`; `testing/percpu_stale_test.cc` drains per-CPU
  memory while threads are created and destroyed.

### 4.3 API surface and process lifecycle

- Rust: `GlobalAlloc` (alloc, dealloc, alloc_zeroed, realloc) and `Allocator` (allocate,
  allocate_zeroed, deallocate, grow, grow_zeroed, shrink, zero-size and over-aligned
  layouts, the returned slice length). Run the standard library's collection tests and a
  few large crates' test suites with asmalloc as the global allocator, natively and under
  Miri.
- C ABI: `malloc(0)`, `calloc` overflow, `realloc(NULL, n)`, `realloc(p, 0)`,
  `posix_memalign` argument validation and `EINVAL`, `aligned_alloc`, `memalign`, `valloc`,
  `pvalloc`, `malloc_usable_size(NULL)`, `free(NULL)`, `free_sized`, `free_aligned_sized`,
  `reallocarray`, `errno = ENOMEM`, 16-byte minimum alignment, and the C++
  `operator new/delete` symbols including sized and aligned forms if they are interposed.
  Tests in the reference checkouts: `mimalloc/test/test-api.c`, `test-api-fill.c`,
  `test-stress.c`, `main-override*.c*`; glibc's `malloc/tst-*.c`. [M]
- `LD_PRELOAD` against real programs: python3, node, git, clang, a cargo build, redis,
  something that calls `dlopen`. Check with `LD_DEBUG=bindings` that every allocation symbol
  binds to asmalloc; a missed symbol means mixed allocators and a crash on free.
- Symbol audit: the cdylib must have no undefined reference to `malloc`, `free`,
  `__rust_alloc` or `dlsym`, and no formatting or panic machinery reachable from the fast
  path.
- fork: fork while other threads hold every lock and sit in slow paths; the child must be
  able to allocate. Also `vfork`, `posix_spawn` and `clone(CLONE_VM)`.
- Thread exit: allocation and free inside TLS destructors that run after ours, pthread key
  destructors, detached threads exiting during `exit()`, `atexit` handlers that free. A
  per-CPU design keeps little per-thread state, which shrinks this whole class.
- Reentrancy: allocation from a signal handler during a fast path (covered by 4.2) and
  during a slow path.
- Early start: allocation from the dynamic loader and from other libraries' constructors
  before ours ran; before our TLS is usable; with rseq not yet registered for the thread
  (`cpu_id` reads as uninitialised).
- Coverage and mutation: `cargo-llvm-cov` is installed; `cargo-mutants` on the core would
  measure whether the tests notice a wrong line.

## 5. Benchmarking

### 5.1 mimalloc-bench

- Real programs: cfrac, espresso, barnes, gs, leanN, redis 6.2.7, larsonN and
  larsonN-sized, lua, z3, rocksdb 10.10.1 `db_bench fillrandom`. Stress: alloc-test,
  cache-scratch, cache-thrash, glibc-simple and glibc-thread, malloc-large, mleak, rptest,
  mstress, rbstress, sh6bench, sh8bench, xmalloc-test (`mimalloc-bench/README.md`,
  `mimalloc-bench/bench.sh:15-16`). [S]
- What each stresses [S]: larson is server-style "bleeding" of objects between threads;
  xmalloc-test has pure producer threads and pure consumer threads, the worst case for
  thread caches; sh8bench and mstress mix cross-thread frees, lifetimes and thread
  turnover; cache-scratch is passive false sharing; alloc-test uses Pareto sizes;
  malloc-large uses multi-megabyte blocks; mleak checks thread-exit leaks; cfrac and
  espresso are small, short-lived and single-threaded.
- Most relevant [I]: larsonN-sized (sized free, like Rust), xmalloc-testN, sh8benchN,
  mstress, rptest, cache-scratchN (per-CPU caches plus migration can create false sharing),
  alloc-test, mleak, malloc-large; of the real programs redis, rocksdb, leanN and z3.
- Flaws in its runner [S]: one run per test by default (`bench.sh:46-47`); wall time from
  `/usr/bin/time` at 10 ms resolution (`bench.sh:41,618`) while several tests finish in 0.1
  to 0.3 s; peak RSS only; no pinning and no interleaving; allocators built with their own
  flags; the build script wants sudo and distribution packages. Versions are pinned in
  `build-bench-env.sh:40-80` (mimalloc v3.4.5, jemalloc 5.4.0, snmalloc 0.7.6, google
  tcmalloc 2026-02-23 through bazel 8.8.0). Use its programs, not its runner.

### 5.2 connect-rust

- `benches/rpc`: tokio multi-thread servers (`src/bin/echo_server.rs:28`,
  `log_server.rs:79`) and load drivers `echo_bench`, `log_bench` and `fortune_bench` at
  concurrency 16, 64 and 256 with a 3 s warm-up and a 10 s measurement
  (`src/bin/echo_bench.rs:31-36`). There are also Criterion benches without a network:
  `echo_bloat`, `reader_bench`, `view_rope_encode`, `stream_compressed_encode`
  (`benches/rpc/README.md`). [S]
- There is no `#[global_allocator]` anywhere and no allocator result recorded in the tree;
  the binaries use the system allocator (the only mention of an allocator is jemalloc
  through `LD_PRELOAD` for heap profiling, `benches/profile_server.sh:130-140,230`). [S]
  Swapping allocators with `LD_PRELOAD` goes through C `free(ptr)` and discards Rust's size,
  so measuring the path asmalloc cares about needs a feature-gated `#[global_allocator]`
  added to the bench crate.
- Good targets [I]: `log_bench` (decode-heavy, many small allocations freed on other
  workers) and `echo_bench` (framework overhead). Client and server share the machine, so
  pin them to different L3 domains.

### 5.3 Suite

A. **Roofline microbenchmarks**, by wasmalloc's method: floors, incumbents, asmalloc, and
   mimic variants. Shapes: same-size pair; last-in first-out and first-in first-out batches;
   churn among N live objects with N from 1e2 to 1e7 (L1 to DRAM); a Pareto size mix;
   `realloc` doubling; allocate on one thread and free on another (ping-pong,
   producer/consumer, N-to-N); thread churn. Always through `GlobalAlloc` or `Allocator`
   with real `Layout`s, and separately through the C ABI. Report cycles and instructions per
   operation from performance counters, not only time.

B. **Trace replay.** A tracing build records (thread, operation, size, alignment, address
   id, timestamp) from real programs, and a replayer reproduces the stream with the same
   thread structure. This gives allocator-only A/B numbers with performance counters, and
   the statistics that should weight the microbenchmarks: size histogram, lifetime
   distribution, share of cross-thread frees, share of sized frees.

C. **Rust workloads**, each in two modes (native `#[global_allocator]` and `LD_PRELOAD`):
   connect-rust `log_bench` and `echo_bench`; a tokio mpsc pipeline that allocates on one
   worker and frees on another; an axum or hyper server under a load generator; a `cargo
   build` of a mid-size workspace with the allocator preloaded into rustc; rust-analyzer
   `analysis-stats`; a DataFusion or Polars TPC-H run; ruff or oxc over a large repository;
   `serde_json` parsing into `Value`; rayon parallel collect and sort.

D. **C and C++ through `LD_PRELOAD`**: the mimalloc-bench subset of 5.1 under our own
   runner, plus redis and rocksdb.

E. **Memory**: RSS, PSS and `AnonHugePages` sampled from `/proc/<pid>/smaps_rollup` at 10 to
   100 Hz; peak RSS and page faults from `getrusage`; fragmentation as RSS over live bytes
   (the harness tracks live bytes); hugepage coverage; time for RSS to fall after the
   workload frees everything; share of system time; dTLB misses. Huge pages are in `madvise`
   mode on the reference host [X], so results depend on what each allocator advises: record
   it.

### 5.4 Methodology

- Pin with `taskset` to physical cores of one L3 domain and leave their SMT siblings idle.
  On the reference host the domains are CPUs 0-7 (siblings 16-23) and 8-15 (siblings
  24-31). [X]
- Count cycles and instructions with performance counters; they do not depend on frequency.
  User-space counters and breakpoints on our own processes work at
  `perf_event_paranoid=2`; kernel profiling of mmap, madvise and page faults needs 1 or
  lower.
- Warm up; interleave allocators A/B/A/B in random order within one session; use 10 to 30
  process launches per configuration, because physical page placement and huge-page luck
  are per process (verified-garbage measured up to 7% from page placement alone:
  `verified-garbage/CLAUDE.md:695-702`); report medians with bootstrap intervals.
- Always include an A-against-A control and treat a non-null result there as a broken setup.
- Code layout: the roofline study found that byte alignment alone did not move a tight
  loop, while uop count per straight-line run and late 64-byte line crossings did
  (`docs/research/zen5-roofline.md`). Inlined fast paths land in different surroundings at
  each call site, so measure several call sites rather than one loop.
- Gating: instruction counts for fast-path microbenchmarks are stable to about 0.1% and can
  gate CI (verified-garbage compares `instructions:u`:
  `verified-garbage/CLAUDE.md:595-604`). A golden "shape" table per entry point is another
  cheap gate; tcmalloc keeps one (`tcmalloc/tcmalloc/testing/fast_path.release.golden`:
  counts of call, compare, branch, load, store, read-modify-write and push/pop per
  function; its malloc is 6 loads, 1 store, 2 read-modify-writes, 5 branches, no call).
  Cycle and wall-time results are tracked, not gated.

The reference host as of 2026-10-16 [X]: driver `amd-pstate-epp`, governor `powersave`,
energy preference `balance_performance`, boost on, SMT on, no isolated CPUs. All of these
need root to change. `perf_event_paranoid` was 2 for the measurements in this document and
has since been set to -1.

### 5.5 Baselines

| Allocator | On the reference host | Route |
|---|---|---|
| glibc 2.43 malloc | present | default |
| jemalloc 5.3.0 | system package | preload, or the `tikv-jemallocator` crate for native mode; this is the incumbent in the Rust services that motivated the project |
| mimalloc v3.5.3 | source in `third_party/` | compile the `src/static.c` unity file with `cc`, or cmake; the `mimalloc` crate for native mode |
| google/tcmalloc | source in `third_party/`; needs bazel or cmake | run with `GLIBC_TUNABLES=glibc.pthread.rseq=0` for per-CPU mode and without it, and report both |
| gperftools tcmalloc 2.18.1 | `/usr/lib64/libtcmalloc.so.4` | preload; a different allocator from google/tcmalloc, with thread caches only |
| snmalloc, rpmalloc | not installed | `snmalloc-rs` (cmake, C++), `rpmalloc` crates |

google/tcmalloc registers its own `__rseq_abi` (`tcmalloc/tcmalloc/internal/percpu.cc:87`).
With glibc 2.35 and later that fails because glibc already registered, and tcmalloc falls
back to per-thread caches; its own tests expect working rseq only when
`GLIBC_TUNABLES=glibc.pthread.rseq=0` is set (`tcmalloc/tcmalloc/testing/rseq_util.cc:33-46`,
`tcmalloc/tcmalloc/internal/percpu.bzl:30`). [S]

Rust wrapper crates [W][M]: `mimalloc`, `tikv-jemallocator`, `tcmalloc-better` (google
tcmalloc), `tcmalloc` and `tcmalloc2` (gperftools), `snmalloc-rs`, `rpmalloc`. Pin and
record which upstream version each bundles. The `mimalloc` crate 0.1.52 bundles mimalloc
3.3.2 by default and frees with the unsized `mi_free` (`docs/research/prior-art.md`).

## 6. Risks

1. **Three logics.** Lean, Kani and reviewed arguments do not compose mechanically; every
   boundary is a hand-aligned specification. Mitigation: Lean as the spine, the slab
   invariant stated once, Rust constants and test vectors generated from it.
2. **A new trusted base nobody has reviewed**: `%fs`, rseq abort semantics, `asm!` operand
   rules, external calls. A wrong model proves nothing. Mitigation: tiny models, the
   breakpoint harness as an executable check of the rseq axiom, golden tests for printers,
   review by verified-garbage's author.
3. **rseq is a moving kernel contract.** The read-only enforcement measured here arrived in
   v6.19, and time-slice extension is not in every build. rseq is also unavailable under
   valgrind and when glibc's registration is disabled; asmalloc then runs in its slow mode,
   which is one more configuration to test.
4. **Proofs freeze design.** At 7 to 12 proof lines per assembly line, proving code that
   will be rewritten is waste. Prove kernels once their shape survives the roofline work;
   keep Kani and the model tester as the always-on net.
5. **No assembly in Kani, Miri or loom.** Every kernel needs a Rust twin, and the
   equivalence of twin and assembly rests on the Lean proof plus differential tests.
6. **Toolchain pins.** Kani lags stable by months and Aeneas pins its own compiler. The
   core crate must build on all of them.
7. **Benchmark validity.** A desktop with boost, a powersave governor and SMT; baselines
   that silently change mode; `LD_PRELOAD` and native `GlobalAlloc` measuring different
   code paths; one-shot wall-clock runs.

## 7. Not verified

- Verus's pinned Rust version, and whether its current release handles `#[thread_local]`
  or raw-pointer arithmetic better than in 2024.
- That `LD_PRELOAD` overrides rustc's bundled jemalloc.
- Which upstream versions the Rust wrapper crates bundle, other than `mimalloc`.
- That the optimized rseq mode matches the kernel documentation in every detail on 7.2.9;
  only the two behaviours in section 4.2 were tested.
- That google/tcmalloc's registration fails under glibc 2.43; read from source, not run.
- The statements tagged [M] about loom, ThreadSanitizer, valgrind and rr with rseq, the
  kernel selftests, and the x86-TSO argument.
- No Lean was built and no verified-garbage proof was run; section 1 is from reading.
