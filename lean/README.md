# Lean: the assembly routines, their proofs, and the generator

asmalloc's assembly is written here as Lean terms over a model of x86-64,
proved correct against that model, and printed as Rust source. Nothing in the
allocator is assembly written by hand: the generated files are the only
assembly it contains, and a check fails if they differ from what Lean
generates now.

This directory is a fork of the Lean framework of
[verified-garbage](https://github.com/pyca/verified-garbage), which does the
same for cryptography, taken at commit
`bbeedea2382fb085e03f41fb844516cd761278e8`. It keeps the framework and drops
the cryptography. Its files are derived from upstream's and stay under
upstream's licence, Apache-2.0 OR BSD-3-Clause (the same as asmalloc's); the
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
* the AArch64, ARMv7 and IA-32 models;
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
directory given on its command line, `generated/` by default.

## Layout

```
Asmalloc/
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
  Spec/         Specifications and contracts (trusted, must be reviewed)
  Impl/         Implementations: `Prog`s over an ISA model (untrusted)
  Proof/        Proofs (untrusted)
    Framework/      determinism, weakest-precondition rules, memory frames,
                    contract tactics, symbolic execution
  Artifacts/    One registration file per routine and target, listing what it emits
AsmallocTest/   Golden tests for the printers and the emitter, tests of the audits
Emit.lean       Renders every artifact into `generated/` (or the directory given)
generated/      The emitted Rust, checked in
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
2. **Impl**: `Impl/<Name>/X86_64.lean` defines the code as a `Prog`.
3. **Proof**: `Proof/<Name>/…` proves `Verified target code contract`:
   termination without faults (hence memory safety), the postcondition, the
   calling-convention obligations, and that the precondition is satisfiable.
4. **Registry**: `Artifacts/<Name>/X86_64.lean` binds them into an
   `Artifact`, which cannot be built without the proof. Registration files
   only bind: no theorems and no tactic proofs but the defaults
   (`scripts/check_registrations.py`).
5. **Emit**: `Emit.lean` audits every artifact (only the standard axioms;
   the compiled code it runs is what the kernel checked; `VG.Spec` names come
   from `Spec/`) and renders each as a Rust naked function.

The self-test (`Spec/Selftest*`, `Impl/Selftest/`, `Proof/Selftest/`,
`Artifacts/Selftest/`) is a worked example of every step.

## What you need to trust

* `TCB/`: in particular the ISA model, which must match the Intel SDM, and
  the printers, which must print what the model means.
* For each artifact: its contract and `Api` in `Spec/`.
* Lean's kernel, and the assembler in `rustc` and LLVM.

Everything in `Impl/` and `Proof/` is checked by Lean and need not be read.

## Rules

* No `sorry`, `admit`, `native_decide`, `bv_decide` or new `axiom` in
  anything an artifact depends on. `lake build` treats warnings as errors and
  the emitter's audit rejects any axiom beyond Lean's standard three;
  `scripts/audit_negative.py` checks that the audit still does.
* Never set `maxHeartbeats` or another resource limit: a proof that hits one
  is too slow, so make it faster.
* Changes to `TCB/` or `Spec/` are trust changes: keep them small, commit them
  apart from implementations and proofs, and justify each change to the ISA
  model by citing the Intel SDM. Never weaken a model or contract to make a
  proof go through.
* The emitter runs compiled code, so nothing it runs may carry
  `implemented_by`, `extern` or `export`, and nothing under `Asmalloc/` may
  declare a `csimp` theorem or an `initialize` (`TCB/Audit.lean`).
* Never edit `generated/` by hand.

For symbolic execution, step a block with `runBlock_cons`, `runStep_some`
and `runBlock_nil` (`Proof/Framework/X86_64/Exec.lean`) and read registers
through writes with the `RegUpd` lemmas; never unfold `State.setReg`,
`arithFlags` or `State.setFlags`. Upstream's `CLAUDE.md`, "Keeping proofs
fast", explains why, with more patterns worth following.

## Building, checking and emitting

Install Lean with [elan](https://github.com/leanprover/elan); it reads the
version from `lean-toolchain`. Then, in this directory:

```sh
lake build --log-level=warning          # every proof and golden test
lake env lean --run Emit.lean           # write generated/
lake env lean --run Emit.lean --check   # fail if generated/ is out of date
lake env lean --run Emit.lean DIR       # write somewhere else
scripts/check.sh                        # all of the checks
```
