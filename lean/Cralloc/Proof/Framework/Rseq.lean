module

public import Cralloc.Proof.Framework.Semantics

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
    · exact (hok.2.2 p hp).2.2.2 p' hrely henv
    · exact (hok.2.2 p hp).2.2.2 p' hrely henv
    · rename_i h1 h2
      exfalso
      unfold Code.abortStates at hp
      split at hp
      · exact h1 _ _ rfl
      · exact h2 _ _ _ rfl
      · simp at hp

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

/-- Where a verified routine may be aborted, `RseqOk`'s obligations hold. -/
theorem Verified.abortOk {T : Target} {c : Prog T.isa} {k : Contract T.isa}
    (h : Verified T c k) {s p : T.isa.State} (hs : k.pre s) (hp : p ∈ c.abortStates T.isa s) :
    T.armed p ∧ T.abiPreserved s p ∧ k.abort s p ∧
      ∀ p', T.rely p p' → k.env p p' → k.pre p' := by
  have hok := h.2.1 s hs
  unfold RseqOk at hok
  split at hok
  · exact hok.2.2 p hp
  · exact hok.2.2 p hp
  · rename_i h1 h2
    exfalso
    unfold Code.abortStates at hp
    split at hp
    · exact h1 _ _ rfl
    · exact h2 _ _ _ rfl
    · simp at hp

/-- What every abort keeps relates the caller's entry state to the state
the last attempt starts in: a relation `R` of an entry state and a later
attempt's start, which holds of a state and itself, and which holds of an
attempt's start `s` and a later one's `s'` if it holds of the state `p'`
the thread resumed in after an abort of the attempt from `s`, and `s'`. -/
theorem Verified.restarts_keep {T : Target} {c : Prog T.isa} {k : Contract T.isa}
    (h : Verified T c k) (R : T.isa.State → T.isa.State → Prop) (hrefl : ∀ s, R s s)
    (hstep : ∀ s p p' s', k.pre s → p ∈ c.abortStates T.isa s → T.rely p p' → k.env p p' →
      k.pre p' → R p' s' → R s s')
    {s s' : T.isa.State} (hs : k.pre s) (hr : Restarts T c k s s') : R s s' := by
  revert hs
  induction hr with
  | refl => exact fun _ => hrefl _
  | @abort s p p' s'' hp hrely henv hrest ih =>
    intro hs
    have hp' : k.pre p' := h.restart_pre hs (.abort hp hrely henv (.refl _))
    exact hstep s p p' s'' hs hp hrely henv hp' (ih hp')

/-! ## A routine with a setup

`Verified` is about attempts of the code that follows the setup. What a run of
the whole function does, from an entry state satisfying the function's
precondition: the setup runs without changing memory or the arguments, the first
attempt starts in a state satisfying the attempt's precondition, and so does
every later one; the last attempt then completes or exits, meeting the
postcondition relative to the state it started in. -/

/-- The whole of a routine's run, from entry. -/
theorem Verified.setup_restarts {T : Target} {c : Prog T.isa} {k k' : Contract T.isa} {sig : Sig}
    {cl : List String} {setup : List T.isa.Instr} (h : Verified T c k')
    (hs : SetupOk T k k' sig cl setup) {s : T.isa.State} (hp : k.pre s) :
    ∃ a, execBlock T.isa setup s = some a ∧ k'.pre a ∧ T.abi.mem a = T.abi.mem s ∧
      ∀ s', Restarts T c k' a s' → k'.pre s' ∧
        ((∃ s'', Exec T.isa c s' s'' ∧ T.abiPreserved s' s'' ∧ k'.post s' s'') ∨
          (∃ t s'', ExecExit T.isa c s' t s'' ∧ T.abiPreserved s' s'' ∧ k'.exit t s' s'')) := by
  rcases hs with ⟨rfl, rfl⟩ | ⟨-, hs⟩
  · exact ⟨s, rfl, hp, rfl, fun s' hr => h.restarts hp hr⟩
  · obtain ⟨a, ha, hka, hm, -, -, -⟩ := hs s hp
    exact ⟨a, ha, hka, hm, fun s' hr => h.restarts hka hr⟩

end VG
