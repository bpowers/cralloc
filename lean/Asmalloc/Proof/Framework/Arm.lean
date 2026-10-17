module

public import Asmalloc.Proof.Framework.Rseq

/-!
# Another arm, or another block, with the same effect

A restartable sequence whose arm is replaced by one that, from every state
satisfying the precondition, ends in the same state with the same memory
accesses, and whose last instruction arms the sequence as `RseqOk` asks,
runs, exits, may be aborted and visits its branches as the original does: so
it is verified by the original's proof (`Verified.of_arm`), and so are its
clobbers (`clobbersOk_of_arm`) and its retries (`restarts_of_arm`). The same
holds of a block replaced by one with the same effect (`Verified.of_block`).
-/

@[expose] public section

namespace VG

variable {T : Target} {k : Contract T.isa} {arm arm' : List T.isa.Instr}
  {body : List (Step T.isa.Instr T.isa.Cond)}

theorem exec_arm_iff {s s' : T.isa.State} (hx : execBlock T.isa arm' s = execBlock T.isa arm s) :
    Exec T.isa (.rseq arm' body) s s' ↔ Exec T.isa (.rseq arm body) s s' := by
  constructor <;> intro h <;> cases h <;> rename_i h1 h2 <;>
    exact .rseq (by first | rwa [hx] at h1 | rwa [← hx] at h1) h2

theorem execExit_arm_iff {s s' : T.isa.State} {t : String}
    (hx : execBlock T.isa arm' s = execBlock T.isa arm s) :
    ExecExit T.isa (.rseq arm' body) s t s' ↔ ExecExit T.isa (.rseq arm body) s t s' := by
  constructor <;> intro h <;> cases h <;> rename_i h1 h2 <;>
    exact .rseq (by first | rwa [hx] at h1 | rwa [← hx] at h1) h2

theorem abortStates_arm {s : T.isa.State} (hx : execBlock T.isa arm' s = execBlock T.isa arm s) :
    Code.abortStates T.isa (.rseq arm' body) s = Code.abortStates T.isa (.rseq arm body) s := by
  simp only [Code.abortStates, hx]

theorem visits_arm {s : T.isa.State} {o : Nat} {d : Bool}
    (hx : execBlock T.isa arm' s = execBlock T.isa arm s) :
    Visits T.isa (.rseq arm body) s o d → Visits T.isa (.rseq arm' body) s o d := by
  intro h
  cases h with
  | rseq h1 h2 => exact .rseq (by rwa [hx]) h2

/-- A sequence with another arm of the same effect is verified. -/
theorem Verified.of_arm (h : Verified T (.rseq arm body) k)
    (hx : ∀ s, k.pre s → execBlock T.isa arm' s = execBlock T.isa arm s)
    (he : ∀ s, k.pre s → blockEvents T.isa arm' s = blockEvents T.isa arm s)
    (ha : ∀ s, k.pre s → ∃ i a, arm'.getLast? = some i ∧ execBlock T.isa arm'.dropLast s = some a ∧
      T.arms i a) :
    Verified T (.rseq arm' body) k := by
  obtain ⟨hc, hr, hsat, hv⟩ := h
  refine ⟨fun s hs => ?_, fun s hs => ?_, hsat, fun o ho => ?_⟩
  · rcases hc s hs with ⟨s', he', ha', hp⟩ | ⟨t, s', he', ha', hx'⟩
    · exact .inl ⟨s', (exec_arm_iff (hx s hs)).mpr he', ha', hp⟩
    · exact .inr ⟨t, s', (execExit_arm_iff (hx s hs)).mpr he', ha', hx'⟩
  · have h1 := hr s hs
    unfold RseqOk at h1 ⊢
    obtain ⟨ht, -, hab⟩ := h1
    refine ⟨?_, ha s hs, fun p hp => hab p (by rwa [abortStates_arm (hx s hs)] at hp)⟩
    simp only [Code.events, he s hs, hx s hs] at ht ⊢
    exact ht
  · obtain ⟨s, hs, hvs⟩ := hv o ho
    exact ⟨s, hs, visits_arm (hx s hs) hvs⟩

/-- `Artifact.clobbersOk` of a sequence with another arm of the same effect. -/
theorem clobbersOk_of_arm {P Q : T.isa.State → T.isa.State → Prop} {R : String → T.isa.State → T.isa.State → Prop}
    {s : T.isa.State} (hx : execBlock T.isa arm' s = execBlock T.isa arm s)
    (h : (∀ s', Exec T.isa (.rseq arm body) s s' → P s s') ∧
      (∀ t s', ExecExit T.isa (.rseq arm body) s t s' → R t s s') ∧
      (∀ p ∈ Code.abortStates T.isa (.rseq arm body) s, Q s p)) :
    (∀ s', Exec T.isa (.rseq arm' body) s s' → P s s') ∧
      (∀ t s', ExecExit T.isa (.rseq arm' body) s t s' → R t s s') ∧
      (∀ p ∈ Code.abortStates T.isa (.rseq arm' body) s, Q s p) :=
  ⟨fun s' he => h.1 s' ((exec_arm_iff hx).mp he), fun t s' he => h.2.1 t s' ((execExit_arm_iff hx).mp he),
    fun p hp => h.2.2 p (by rwa [abortStates_arm hx] at hp)⟩

/-- A run of a sequence with another arm of the same effect, aborted any
number of times, is one of the original. -/
theorem restarts_of_arm (h : Verified T (.rseq arm body) k)
    (hx : ∀ s, k.pre s → execBlock T.isa arm' s = execBlock T.isa arm s)
    {s s' : T.isa.State} (hs : k.pre s) (hr : Restarts T (.rseq arm' body) k s s') :
    Restarts T (.rseq arm body) k s s' := by
  revert hs
  induction hr with
  | refl => exact fun _ => .refl _
  | @abort s p p' s'' hp hrely henv _ ih =>
    intro hs
    have hp0 : p ∈ Code.abortStates T.isa (.rseq arm body) s := by rwa [abortStates_arm (hx s hs)] at hp
    exact .abort hp0 hrely henv (ih (h.restart_pre hs (.abort hp0 hrely henv (.refl _))))

/-- A block replaced by one with the same effect is verified. -/
theorem Verified.of_block {is is' : List T.isa.Instr} (h : Verified T (.block is) k)
    (hx : ∀ s, k.pre s → execBlock T.isa is' s = execBlock T.isa is s) : Verified T (.block is') k := by
  obtain ⟨hc, -, hsat, -⟩ := h
  refine ⟨fun s hs => ?_, fun _ _ => trivial, hsat, fun o ho => by cases ho⟩
  rcases hc s hs with ⟨s', he, ha, hp⟩ | ⟨t, s', he, -, -⟩
  · cases he with
    | block h1 => exact .inl ⟨s', .block (by rw [hx s hs]; exact h1), ha, hp⟩
  · cases he

end VG
