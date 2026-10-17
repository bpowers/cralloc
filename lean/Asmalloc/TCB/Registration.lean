module

public import Asmalloc.TCB.Artifact

/-!
# Registering an implementation of an `Api`

**Trusted.** A registration file makes each function's `Artifact` from its
`Api` with `Api.artifact`, stating only what the implementation chooses: the
target, the code and its proof, the stack it uses, the constant tables it
reads, the CPU features it needs, and any notes on it and the suffix of its
name. Everything else comes from the `Api` (`Spec/`): the Rust module, name,
signature and documentation, `writeArgs`, and the contract on the target,
which the proof is of. So of a registration, a reviewer reads `suffix`,
`notes` and `features` (which the emitter checks against the code); the
kernel checks the rest.
-/

@[expose] public section

namespace VG

/-- Proves `Registration.spSafe`, by default. `Proof/Framework/Registration.lean`
implements it; how it is proven is not trusted. -/
syntax (name := spSafe) "sp_safe" : tactic

/-- Proves `Registration.ofSig`, by default. `Proof/Framework/Registration.lean`
implements it; how it is proven is not trusted. -/
syntax (name := ofSig) "of_sig" : tactic

/-- `api`'s contract under the calling convention `A`, for an implementation
whose calls and frames use `stack` bytes of stack (`Api.contracts`), or, if
`api` has none, one that no state satisfies, which `Verified` rejects. -/
def Api.contract {M : ISA} (api : Api) (A : Abi M) (stack : Nat) : Contract M :=
  match api.contracts with
  | some f => f A stack
  | none => { pre := fun _ => False, post := fun _ _ => False }

/-- An implementation of `api` on `target`: what a registration file states
of a function made from `api` (see `Api.artifact`). -/
structure Registration (api : Api) (target : Target) where
  /-- What follows `api`'s name in the function's: that of the CPU features
  or other variant it is for (e.g. `_bmi2`), or nothing. -/
  suffix : String := ""
  /-- Paragraphs on the implementation, between `api`'s summary and its
  `# Safety` section (`Api.doc`). -/
  notes : List String := []
  /-- `Artifact.consts`. -/
  consts : List (String × List (BitVec 64)) := []
  /-- `Artifact.stack`. -/
  stack : Nat := 0
  code : Prog target.isa
  /-- The contract `verified` proves, which is `api`'s on `target` for
  `consts` and `stack` (`ofApi`); unification finds it in `verified`'s type. -/
  contract : Contract target.isa := api.contract (target.abi.withConsts consts) stack
  verified : Verified target code contract
  ofApi : contract = api.contract (target.abi.withConsts consts) stack := by exact rfl
  /-- `Artifact.ofSig`. -/
  ofSig : ∃ ext pre post x,
      contract = api.sig.contract ((target.abi.withConsts consts).withExtra ext) pre post
        api.writeArgs stack x := by
    of_sig
  /-- `Artifact.spSafe`. -/
  spSafe : code.all (fun i => !target.isa.writesSp i) = true := by sp_safe
  /-- `Artifact.features`. -/
  features : List String := []
  /-- `Artifact.clobbers`. -/
  clobbers : List String := []
  /-- `Artifact.clobbersOk`. -/
  clobbersOk : target.inline = true → ∀ s, contract.pre s →
      (∀ s', Exec target.isa code s s' →
        target.preservedExcept (clobbers ++ (if api.sig.ret.isSome then [target.retReg] else [])) s s') ∧
      (∀ t s', ExecExit target.isa code s t s' → target.preservedExcept clobbers s s') := by
    exact fun h => nomatch h
  /-- `Artifact.witnesses`. -/
  witnesses : List target.isa.State
  /-- `Artifact.witnessOk`. -/
  witnessOk : (∃ w ∈ witnesses, contract.pre w) ∧
    ∀ o ∈ code.outcomes, ∃ w ∈ witnesses, contract.pre w ∧ Visits target.isa code w o.1 o.2

/-- The artifact of the implementation `r` of `api` on `target`: its module,
signature and `writeArgs` are `api`'s, its name `api`'s followed by
`r.suffix`, its documentation `api`'s with `r.notes` (`Api.doc`), and its
contract `api`'s on `target`, for `r.consts` and `r.stack` (`Api.contract`). -/
def Api.artifact (api : Api) (target : Target) (r : Registration api target) : Artifact where
  target := target
  module := api.module
  name := api.name ++ r.suffix
  sig := api.sig
  doc := api.doc r.notes
  consts := r.consts
  operands := api.operands
  generics := api.generics
  code := r.code
  contract := r.contract
  verified := r.verified
  writeArgs := api.writeArgs
  stack := r.stack
  ofSig := r.ofSig
  contracts := api.contracts
  ofApi := by
    rw [r.ofApi]
    unfold Api.contract
    cases api.contracts <;> exact (by trivial)
  spSafe := r.spSafe
  features := r.features
  witnesses := r.witnesses
  witnessOk := r.witnessOk
  clobbers := r.clobbers
  clobbersOk := r.clobbersOk

end VG
