module

import Cralloc.Spec.Selftest
public import Cralloc.TCB.AArch64.Target
public import Cralloc.Impl.Selftest.AArch64
import Cralloc.Proof.Framework.Semantics
public import Cralloc.Proof.Framework.Contract
public import Cralloc.Spec.Selftest.Contract
import all Init.Data.Array.Basic
import all Init.Data.Array.DecidableEq
import all Init.Data.Nat.Bitwise.Basic
import all Init.Data.Vector.Basic

/-! # Pipeline self-test: AArch64 proofs, out of line and inline -/

@[expose] public section

namespace VG.Proof.Selftest

open Spec.Selftest

open AArch64 in
/-- The AArch64 contract the proof is written against (the artifact's is the
shared contract of `Spec/`, which implies it), for
`vg_selftest_add(a: u64, b: u64) -> u64`: returns
`add a b` and does not modify memory. -/
def addAArch64 : Contract AArch64.isa where
  pre _ := True
  post s s' := s'.gpr .x0 = add (s.gpr .x0) (s.gpr .x1) ∧ s'.mem = s.mem

open AArch64 in
/-- The same inline, where the arguments are in `x3` and `x2`. -/
def addAArch64Inline : Contract AArch64.isa where
  pre _ := True
  post s s' := s'.gpr .x0 = add (s.gpr .x3) (s.gpr .x2) ∧ s'.mem = s.mem

end VG.Proof.Selftest

namespace VG.Proof.Selftest.AArch64

open VG.AArch64

theorem add_correct (s : State) (_ : Proof.Selftest.addAArch64.pre s) :
    ∃ s', Exec isa Impl.Selftest.AArch64.add s s' ∧ abiPreserved s s' ∧
      Proof.Selftest.addAArch64.post s s' := by
  refine ⟨_, .block rfl, ?_, ?_, ?_⟩
  · simp [abiPreserved, calleeSaved, State.write, State.setReg]
  · simp [State.write, State.setReg, State.read, State.src, Spec.Selftest.add]
  · rfl

theorem addInline_correct (s : State) (_ : Proof.Selftest.addAArch64Inline.pre s) :
    ∃ s', Exec isa Impl.Selftest.AArch64.addInline s s' ∧ s'.sp = s.sp ∧
      Proof.Selftest.addAArch64Inline.post s s' := by
  refine ⟨_, .block rfl, ?_, ?_, ?_⟩
  · rfl
  · simp [State.write, State.setReg, State.read, State.src, Spec.Selftest.add]
  · rfl

/-- A state satisfying the precondition. -/
def sat : State where
  gpr _ := 0
  mem _ := 0
  rd := []
  wr := []

theorem add_verified :
    Verified target Impl.Selftest.AArch64.add (Spec.Selftest.addContract abi) :=
  Verified.of_correct add_correct (by
    sig_implies [Spec.Selftest.addContract, Spec.Selftest.addSig, Proof.Selftest.addAArch64,
      abi, argRegs] [sat] using sat)

theorem addInline_verified :
    Verified inlineTarget Impl.Selftest.AArch64.addInline (Spec.Selftest.addContract inlineAbi) :=
  Verified.of_correct addInline_correct (by
    sig_implies [Spec.Selftest.addContract, Spec.Selftest.addSig, Proof.Selftest.addAArch64Inline,
      inlineAbi, abi, inlineArgRegs, argRegs] [sat] using sat)

/-- `sat` satisfies the precondition; the code does not branch. -/
theorem add_witness : (∃ w ∈ [sat], (Spec.Selftest.addContract abi).pre w) ∧
    ∀ o ∈ Code.outcomes Impl.Selftest.AArch64.add, ∃ w ∈ [sat],
      (Spec.Selftest.addContract abi).pre w ∧
        Visits isa Impl.Selftest.AArch64.add w o.1 o.2 := by
  refine ⟨⟨sat, List.mem_singleton_self _, ?_⟩, fun o ho => by cases ho⟩
  sig_pre [Spec.Selftest.addContract, Spec.Selftest.addSig, abi, argRegs, sat]

theorem addInline_witness : (∃ w ∈ [sat], (Spec.Selftest.addContract inlineAbi).pre w) ∧
    ∀ o ∈ Code.outcomes Impl.Selftest.AArch64.addInline, ∃ w ∈ [sat],
      (Spec.Selftest.addContract inlineAbi).pre w ∧
        Visits isa Impl.Selftest.AArch64.addInline w o.1 o.2 := by
  refine ⟨⟨sat, List.mem_singleton_self _, ?_⟩, fun o ho => by cases ho⟩
  sig_pre [Spec.Selftest.addContract, Spec.Selftest.addSig, inlineAbi, abi, inlineArgRegs,
    argRegs, sat]

/-- The block writes only `x0`, its result. -/
theorem addInline_clobbersOk : inlineTarget.inline = true → ∀ s,
    (Spec.Selftest.addContract inlineAbi).pre s →
      (∀ s', Exec inlineTarget.isa Impl.Selftest.AArch64.addInline s s' →
        inlineTarget.preservedExcept ([] ++
          (if Spec.Selftest.addApi.sig.ret.isSome then [inlineTarget.retReg] else [])) s s') ∧
      (∀ t s', ExecExit inlineTarget.isa Impl.Selftest.AArch64.addInline s t s' →
        inlineTarget.preservedExcept [] s s') ∧
      (∀ p ∈ Impl.Selftest.AArch64.addInline.abortStates inlineTarget.isa s,
        inlineTarget.preservedExcept [] s p) := fun _ s _ => by
  refine ⟨fun s' he r hr => ?_, fun t s' he => ?_, fun p hp => ?_⟩
  · cases he with
    | block h =>
      cases h
      have hx0 : r ≠ .x0 := fun h => by subst h; exact hr (by decide +kernel)
      simp [State.write, State.setReg, hx0]
  · cases he
  · cases hp

end VG.Proof.Selftest.AArch64
