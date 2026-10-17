module

public import Asmalloc.Proof.Framework.Semantics

/-!
# Restartable sequences: retries

A routine that is a restartable sequence may be aborted any number of times
before an attempt completes (`Restarts`). Each abort resumes the thread at
the sequence's arm, the routine's first instruction, in a state its
contract's `RseqOk` says satisfies the precondition again; so every attempt
starts in a state satisfying the precondition, and `Verified`'s first
conjunct covers the one that completes. Termination is the run that is not
aborted (`Exec`): that some attempt is not aborted is a fairness assumption
(`Restarts`).
-/

@[expose] public section

namespace VG

/-- Every state a verified routine may start an attempt in satisfies its
precondition. -/
theorem Verified.restart_pre {T : Target} {c : Prog T.isa} {k : Contract T.isa}
    (h : Verified T c k) {s s' : T.isa.State} (hs : k.pre s) (hr : Restarts T c k s s') :
    k.pre s' := by
  revert hs
  induction hr with
  | refl => exact id
  | @abort s p p' s'' hp hrely henv _ ih =>
    intro hs
    apply ih
    have hok := h.2.1 s hs
    unfold RseqOk at hok
    split at hok
    · exact (hok.2 p hp).2.2.2 p' hrely henv
    · simp [Code.abortStates] at hp

/-- A verified routine, aborted any finite number of times, meets its
contract: its last attempt starts in a state `s'` satisfying the
precondition and runs to completion without faulting, meeting the
postcondition and the calling convention's obligations relative to `s'`, or
jumps to an external target meeting the contract's `exit`.
Every aborted attempt meets the contract's `abort` (`RseqOk`). -/
theorem Verified.restarts {T : Target} {c : Prog T.isa} {k : Contract T.isa}
    (h : Verified T c k) {s s' : T.isa.State} (hs : k.pre s) (hr : Restarts T c k s s') :
    k.pre s' ∧ ((∃ s'', Exec T.isa c s' s'' ∧ T.abiPreserved s' s'' ∧ k.post s' s'') ∨
      (∃ t s'', ExecExit T.isa c s' t s'' ∧ T.abiPreserved s' s'' ∧ k.exit t s' s'')) :=
  have hs' := h.restart_pre hs hr
  ⟨hs', h.1 s' hs'⟩

end VG
