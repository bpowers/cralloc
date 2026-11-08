# Lean: the assembly routines, their proofs, and the generator

cralloc's assembly is written here as Lean terms over a model of the
architecture, x86-64 or AArch64, proved correct against that model, and printed
as Rust source. Nothing in the allocator is assembly written by hand: the
generated files are the only assembly it contains, and a check fails if they
differ from what Lean generates now. (The AArch64 routines are moving here from
`front/provisional/`, written by hand; only the self-test, the pop and the push are
generated so far.)

This directory is a fork of the Lean framework of
[verified-garbage](https://github.com/pyca/verified-garbage), which does the
same for cryptography, taken at commit
`bbeedea2382fb085e03f41fb844516cd761278e8`. It keeps the framework and drops
the cryptography. Its files are derived from upstream's and stay under
upstream's licence, Apache-2.0 OR BSD-3-Clause (the same as cralloc's); the
upstream licence and copyright notice are in `LICENSES/verified-garbage/`.
The Lean namespace is still `VG`, so that a file here can be compared with its
upstream original line by line.

## What was kept, removed and changed

Kept: structured code and its big-step semantics, the memory and region
model, signatures, contracts and `Verified`, the x86-64 state and its
general-purpose instructions (including the BMI1, BMI2 and ADX ones), the
x86-64 printer and System V target, the Rust emitter, the axiom and compiler
audits, the weakest-precondition rules and symbolic-execution lemmas, the
contract tactics (`sig_implies` and friends), the golden tests of the
printers, the negative test of the audit, and the self-test artifact.

Removed:

* every cryptographic specification, implementation and proof, and the
  known-answer tests;
* the ARMv7 and IA-32 models, and upstream's AArch64 model, in whose place
  `TCB/AArch64/` is written for the routines here, in the shape x86-64's took;
* the SSE, AVX and AVX-512 instructions and state (vector registers, MXCSR)
  and the instructions that carry their own tables (`aluTbl`,
  `vmovdquTbl`);
* the constant-time machinery: the leakage trace in the semantics, the
  `ConstantTime` clause of `Verified`, the `pub` part of contracts and
  signatures, and the taint checker;
* the proof libraries for calls, frames and inlining, which depended on the
  constant-time machinery (`Code.call` and `Code.frame` remain in the
  trusted core, unproved helpers aside);
* the literal machinery (`materialize_code`, `lit_decide`): routines are a
  few instructions long, and `decide +kernel` checks them directly;
* lists of slices as parameters, the `written` bounds of writable arrays,
  generic callers and variants (`Generic/`, `Variants/`), and the parallel
  and per-file emitters;
* Mathlib and Batteries. The framework now depends on Lean core alone.

Changed: `Exec` has no trace; `Verified` is total correctness plus
non-vacuity; the CPU-features constant an artifact may carry is a plain
`&[&str]` rather than a type of upstream's crate; the emitter writes to a
directory given on its command line, by default the allocator crate's
`crates/cralloc/src/front/generated/` (relative to the repository root).

## Layout

```
Cralloc/
  TCB/          Trusted computing base: definitions only, Lean core only
    Mem.lean        byte-addressed memory, regions
    Code.lean       structured programs, calls and stack frames, big-step semantics
    Print.lean      lowering of structured control flow to labels and branches
    Sig.lean        Rust signatures and calling conventions (`Abi`)
    Artifact.lean   Target, Contract, `Verified`, `Artifact`: what "verified" means;
                    `Sig.contract`: the contract obligations a signature implies
    Registration.lean  `Api.artifact`: an `Artifact` made from an `Api`
    Rust.lean       rendering artifacts as Rust naked functions, and the checks
                    on them (calls, tables, features, encodability, docs)
    Axioms.lean     `#assert_standard_axioms`
    Audit.lean      `#assert_no_compiler_overrides`, `#assert_spec_origin`
    Emit.lean       what is emitted, and writing or checking the output
    X86_64/         ISA model, printer, System V ABI target
    AArch64/        ISA model, printer, AAPCS64 target
  Spec/         Specifications and contracts (trusted, must be reviewed)
  Impl/         Implementations: `Prog`s over an ISA model (untrusted)
  Proof/        Proofs (untrusted)
    Framework/      determinism, weakest-precondition rules, memory frames,
                    contract tactics, symbolic execution
    Examples/       small proved programs, one per instruction family the
                    routines rely on, showing the lemmas each needs
  Artifacts/    One registration file per routine and target, listing what it emits
CrallocTest/    Golden tests for the printers and the emitter, tests of the
                semantics on concrete states, and of the audits
Emit.lean       Renders every artifact into the allocator crate's
                `src/front/generated/` (or the directory given), where it is checked in
scripts/        The checks: imports, registration files, the audit's negative test
```

`TCB/` imports only Lean core; `Spec/` and `Impl/` never import `Proof/`
(`scripts/check_lean_imports.py`).

## The pipeline

1. **Spec**: `Spec/<Name>.lean` states what a routine computes, and
   `Spec/<Name>/Contract.lean` its Rust signature (`Sig`), its `Contract`
   (built with `Sig.contract`, which derives from the signature and the
   calling convention where the arguments are, the memory the routine may read
   and write, and disjointness; the contract adds a postcondition and any
   further precondition) and its `Api` (Rust module, name, documentation).
2. **Impl**: `Impl/<Name>/<Arch>.lean` defines the code as a `Prog`.
3. **Proof**: `Proof/<Name>/…` proves `Verified target code contract`:
   termination without faults (hence memory safety), the postcondition, the
   calling-convention obligations, and that the precondition is satisfiable.
4. **Registry**: `Artifacts/<Name>/<Arch>.lean` binds them into an
   `Artifact`, which cannot be built without the proof. Registration files
   only bind: no theorems and no tactic proofs but the defaults
   (`scripts/check_registrations.py`).
5. **Emit**: `Emit.lean` imports the registration files into its own
   process, audits the lists they define (only the standard axioms, found by
   walking the declarations; the compiled code it runs is what the kernel
   checked; `VG.Spec` names come from `Spec/`), and evaluates and renders
   those same lists. Nothing it audits or emits is elaborated in a file that
   imports them, so their syntax cannot change it.

The self-test (`Spec/Selftest*`, `Impl/Selftest/`, `Proof/Selftest/`,
`Artifacts/Selftest/`) is a worked example of every step.

The front end's pop and push (`Spec/Slab.lean`, `Impl/Slab/`,
`Proof/Slab/`, `Artifacts/Slab/`) are restartable sequences, each emitted
inline (`front/generated/x86_64_inline/slab.rs`, an `asm!` block in an
`#[inline(always)]` function) and out of line (`front/generated/x86_64/slab.rs`, a
naked function). Neither names a class or the slabs: each is given the
address of its class's word in slab 0, inline with the mask in a register,
out of line with the mask in a table `MASK_DISP` bytes above that word, past
the slabs. Their modules also carry the layout constants the proofs assume
(`Spec.Slab.rustConsts`). The Rust crate must provide what their `# Safety`
sections and `Spec.Slab.path` name: `front::SLABS` (the slabs, which the
slow path's routines name, followed by the masks' table),
`front::pop_slow`, `front::push_slow` and glibc's `__rseq_offset` as
`front::__rseq_offset`. On musl, which has none, the crate defines the area
and the word itself, from data the emitter writes into `x86_64/area.rs`
(`Target.ownArea`, `VG.X86_64.ownRseqArea`; data, not code, so not proved),
and the routines' direct forms, which a musl build always selects
(`Artifact.cfg`), read that word.

The out-of-line pop's and push's steps after the CPU number's are
definitions of their own (`Impl.Slab.X86_64.popSteps`, `pushSteps`), taking
the register that holds the word's address and the exits as parameters, so
that a longer routine (a C entry point that finds the class first) can end
with them.

On x86-64 an abort restarts a routine from its first instruction. On AArch64 a
routine forms the rseq area's and its descriptor's addresses once, in registers,
and an abort returns to the arming store alone, so a retry runs on the registers
the abort kept. The code of such a routine is what follows the setup, and an
artifact states the setup apart (`Artifact.setup`): its attempts have a contract
of their own, the function's with a further precondition on those registers
(`Artifact.attempt`), which a retry must prove from what the abort left, and the
setup must establish (`SetupOk`). So a body that overwrites a register the
restart needs cannot be verified. `Verified` is the same for both, about attempts.

## What you need to trust

* `TCB/`: in particular the ISA models, which must match the Intel SDM and the
  Arm ARM, and the printers, which must print what the models mean. On an AArch64
  host `scripts/check_aarch64.py` runs each instruction form on the processor and
  compares it with the model.
* For each artifact: its contract and `Api` in `Spec/`.
* Lean's kernel, and the assembler in `rustc` and LLVM.

Everything in `Impl/` and `Proof/` is checked by Lean and need not be read.

## Rules

* No `sorry`, `admit`, `native_decide`, `bv_decide` or new `axiom` in
  anything an artifact depends on. `lake build` treats warnings as errors,
  the emitter's audit rejects any axiom beyond Lean's standard three, and
  `leanchecker` replays every module through the kernel, which rejects a
  declaration elaborated without it (`debug.skipKernelTC`).
  `scripts/audit_negative.py` checks each of these against forgeries.
* No initializers (`initialize`, `@[init]`) under `Cralloc/`: the emitter
  imports the registration files and must run none of their code.
* Never set `maxHeartbeats` or another resource limit: a proof that hits one
  is too slow, so make it faster.
* Changes to `TCB/` or `Spec/` are trust changes: keep them small, commit them
  apart from implementations and proofs, and justify each change to an ISA
  model by citing the Intel SDM or the Arm ARM. Never weaken a model or contract
  to make a proof go through.
* The emitter runs compiled code, so nothing it runs may carry
  `implemented_by`, `extern` or `export`, and nothing under `Cralloc/` may
  declare a `csimp` theorem or an `initialize` (`TCB/Audit.lean`).
* Never edit the generated files by hand.

For symbolic execution, step a block with `runBlock_cons`, `runStep_some`
and `runBlock_nil` (`Proof/Framework/<Arch>/Exec.lean`), read registers
through writes with the `RegUpd` lemmas, give loads and stores their region
facts with the `Access` lemmas, and read flags and conditions with the
`Flags` lemmas; never unfold `State.setReg`, `arithFlags` or
`State.setFlags`. `Proof/Examples/` shows each in use. Upstream's
`CLAUDE.md`, "Keeping proofs fast", explains why, with more patterns worth
following.

## Building, checking and emitting

Install Lean with [elan](https://github.com/leanprover/elan); it reads the
version from `lean-toolchain`. Then, in this directory:

```sh
lake build --log-level=warning          # every proof and golden test
lake env lean --run Emit.lean           # write ../crates/cralloc/src/front/generated/
lake env lean --run Emit.lean --check   # fail if those files are out of date
lake env lean --run Emit.lean DIR       # write somewhere else
scripts/check.sh                        # all of the checks
```
