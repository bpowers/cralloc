module

public import Cralloc.TCB.Artifact
public import Cralloc.Proof.Framework.GetElem

/-!
# Reasoning about `Exec`: determinism and weakest preconditions
-/

@[expose] public section

namespace VG

variable {M : ISA}

theorem execBlock_append (l₁ l₂ : List M.Instr) (s : M.State) :
    execBlock M (l₁ ++ l₂) s = (execBlock M l₁ s).bind (execBlock M l₂) := by
  induction l₁ generalizing s with
  | nil => rfl
  | cons i is ih =>
    simp only [List.cons_append, execBlock]
    cases M.exec i s with
    | none => rfl
    | some s₁ => exact ih s₁

/-- The semantics is deterministic. -/
theorem Exec.det {c : Prog M} {s s₁ s₂ : M.State}
    (h₁ : Exec M c s s₁) (h₂ : Exec M c s s₂) : s₁ = s₂ := by
  induction h₁ generalizing s₂ with
  | block h => cases h₂ with
    | block h' => rw [h] at h'; cases h'; rfl
  | seq _ _ ih₁ ih₂ => cases h₂ with
    | seq a b =>
      cases ih₁ a
      exact ih₂ b
  | iteT hc _ ih => cases h₂ with
    | iteT _ b => exact ih b
    | iteF hc' _ => rw [hc] at hc'; cases hc'
  | iteF hc _ ih => cases h₂ with
    | iteT hc' _ => rw [hc] at hc'; cases hc'
    | iteF _ b => exact ih b
  | loopExit _ hc ih => cases h₂ with
    | loopExit a _ => exact ih a
    | loopNext a hc' _ =>
      cases ih a; rw [hc] at hc'; cases hc'
  | loopNext _ hc _ ih₁ ih₂ => cases h₂ with
    | loopExit a hc' => cases ih₁ a; rw [hc] at hc'; cases hc'
    | loopNext a _ b =>
      cases ih₁ a
      exact ih₂ b
  | call hc _ hr ih => cases h₂ with
    | call hc' b hr' =>
      rw [hc] at hc'; cases hc'
      cases ih b
      rw [hr] at hr'; cases hr'
      rfl
  | frame hp _ hq ih => cases h₂ with
    | frame hp' b hq' =>
      rw [hp] at hp'; cases hp'
      cases ih b
      rw [hq] at hq'; cases hq'
      rfl
  | rseq ha hb => cases h₂ with
    | rseq ha' hb' =>
      rw [ha] at ha'; cases ha'
      rw [hb] at hb'; cases hb'
      rfl

theorem Exec.block_iff {is : List M.Instr} {s s' : M.State} :
    Exec M (.block is) s s' ↔ execBlock M is s = some s' :=
  ⟨fun h => by cases h with | block h => exact h, .block⟩

/-! ## Total-correctness weakest preconditions -/

/-- `WP M c s Q`: from `s`, `c` terminates without faulting in a state satisfying `Q`. -/
def WP (M : ISA) (c : Prog M) (s : M.State) (Q : M.State → Prop) : Prop :=
  ∃ s', Exec M c s s' ∧ Q s'

namespace WP

theorem mono {c : Prog M} {s : M.State} {Q Q' : M.State → Prop}
    (h : WP M c s Q) (hq : ∀ s, Q s → Q' s) : WP M c s Q' := by
  obtain ⟨s', he, hq'⟩ := h; exact ⟨s', he, hq _ hq'⟩

/-- Two postconditions of the same code hold together, as the semantics is
deterministic. -/
theorem and {c : Prog M} {s : M.State} {Q R : M.State → Prop}
    (h₁ : WP M c s Q) (h₂ : WP M c s R) : WP M c s fun t => Q t ∧ R t := by
  obtain ⟨s₁, e₁, q⟩ := h₁
  obtain ⟨s₂, e₂, r⟩ := h₂
  cases Exec.det e₁ e₂
  exact ⟨s₁, e₁, q, r⟩

theorem block {is : List M.Instr} {s : M.State} {Q : M.State → Prop}
    (h : ∃ s', execBlock M is s = some s' ∧ Q s') : WP M (.block is) s Q := by
  obtain ⟨s', h1, h2⟩ := h; exact ⟨s', .block h1, h2⟩

theorem block_append {l₁ l₂ : List M.Instr} {s : M.State} {Q : M.State → Prop}
    (h : WP M (.block l₁) s (fun s₁ => WP M (.block l₂) s₁ Q)) :
    WP M (.block (l₁ ++ l₂)) s Q := by
  obtain ⟨s₁, h1, s₂, h2, hq⟩ := h
  rw [Exec.block_iff] at h1 h2
  exact ⟨s₂, .block (by rw [execBlock_append, h1]; exact h2), hq⟩

theorem block_nil {s : M.State} {Q : M.State → Prop} (h : Q s) : WP M (.block []) s Q :=
  ⟨s, .block rfl, h⟩

theorem seq {c₁ c₂ : Prog M} {s : M.State} {Q : M.State → Prop}
    (h : WP M c₁ s (fun s₁ => WP M c₂ s₁ Q)) : WP M (.seq c₁ c₂) s Q := by
  obtain ⟨s₁, h1, s₂, h2, hq⟩ := h; exact ⟨_, .seq h1 h2, hq⟩

theorem ite {c : M.Cond} {th el : Prog M} {s : M.State} {Q : M.State → Prop} (b : Bool)
    (hc : M.eval c s = some b) (ht : b = true → WP M th s Q) (he : b = false → WP M el s Q) :
    WP M (.ite c th el) s Q := by
  cases b
  · obtain ⟨s', h1, h2⟩ := he rfl; exact ⟨_, .iteF hc h1, h2⟩
  · obtain ⟨s', h1, h2⟩ := ht rfl; exact ⟨_, .iteT hc h1, h2⟩

/-- `ite` with its `then` branch out of line (`Code.iteCold`): the same rule,
past the two empty blocks that mark the branch. -/
theorem iteCold {c : M.Cond} {th el : Prog M} {s : M.State} {Q : M.State → Prop} (b : Bool)
    (hc : M.eval c s = some b) (ht : b = true → WP M th s Q) (he : b = false → WP M el s Q) :
    WP M (Code.iteCold c th el) s Q :=
  ite b hc (fun h => seq (seq (block_nil (block_nil (ht h))))) he

/-- The loop rule: an invariant indexed by a natural-number measure that
decreases on every iteration that loops back. -/
theorem loop {body : Prog M} {c : M.Cond} {Q : M.State → Prop}
    (Inv : Nat → M.State → Prop)
    (hstep : ∀ n s, Inv n s → WP M body s (fun s' =>
        (M.eval c s' = some false ∧ Q s') ∨
        (M.eval c s' = some true ∧ ∃ m < n, Inv m s')))
    (n : Nat) (s : M.State) (hs : Inv n s) : WP M (.loop body c) s Q := by
  induction n using Nat.strongRecOn generalizing s with
  | _ n ih =>
    obtain ⟨s', h1, h2⟩ := hstep n s hs
    rcases h2 with ⟨hc, hq⟩ | ⟨hc, m, hm, hi⟩
    · exact ⟨_, .loopExit h1 hc, hq⟩
    · obtain ⟨s'', h3, hq⟩ := ih m hm s' hi
      exact ⟨_, .loopNext h1 hc h3, hq⟩

end WP

/-- The instructions of structured code (including the pushes and pops of
its frames), and of the functions it calls. -/
def instrs {I C : Type} : Code I C → List I
  | .block is => is
  | .seq a b => instrs a ++ instrs b
  | .ite _ t e => instrs t ++ instrs e
  | .loop b _ => instrs b
  | .call _ b => instrs b
  | .frame i b j => i :: instrs b ++ [j]
  | .rseq arm body => arm ++ body.flatMap Step.instrs
  | .jump is _ => is

/-- Whether code calls no function and has no frame: whether it runs only
its own instructions, in the stack it was entered with. -/
def Code.noCalls {I C : Type} : Code I C → Bool
  | .block _ => true
  | .seq a b => a.noCalls && b.noCalls
  | .ite _ t e => t.noCalls && e.noCalls
  | .loop b _ => b.noCalls
  | .call _ _ => false
  | .frame .. => false
  | .rseq .. | .jump .. => true

/-- `(instrs c).all p`, without building the list of instructions, which is
much faster for the kernel to evaluate (`decide +kernel`). A block is walked
with `List.rec`, which the kernel evaluates faster than `List.all` (compiled
to `brecOn`). Only the kernel evaluates it, so it needs no compiled code
(which `List.rec` has none of). -/
noncomputable def Code.allInstrs {I C : Type} (p : I → Bool) : Code I C → Bool
  | .block is => List.rec (motive := fun _ => Bool) true (fun i _ ih => p i && ih) is
  | .seq a b => a.allInstrs p && b.allInstrs p
  | .ite _ t e => t.allInstrs p && e.allInstrs p
  | .loop b _ => b.allInstrs p
  | .call _ b => b.allInstrs p
  | .frame i b j => p i && b.allInstrs p && p j
  | .rseq arm body => (arm ++ body.flatMap Step.instrs).all p
  | .jump is _ => is.all p

theorem Code.allInstrs_eq {I C : Type} (p : I → Bool) (c : Code I C) :
    c.allInstrs p = (instrs c).all p := by
  induction c with
  | block is => induction is <;> simp_all [allInstrs, instrs]
  | _ => simp [allInstrs, instrs, List.all_append, Bool.and_assoc, *]

/-- `Code.all p` holds of all code when `p` holds of every instruction,
without evaluating the code. -/
theorem Code.all_of_forall {I C : Type} {p : I → Bool} (h : ∀ i, p i = true) (c : Code I C) :
    c.all p = true := by
  induction c <;> simp_all [Code.all]

/-- `Code.all p` from `Code.allInstrs p`: for `Artifact.spSafe`,
`Code.all_of_allInstrs (by decide +kernel)` has the kernel evaluate the
faster `Code.allInstrs`. -/
theorem Code.all_of_allInstrs {I C : Type} {p : I → Bool} {c : Code I C} (h : c.allInstrs p = true) :
    c.all p = true := by
  induction c with
  | block is => induction is <;> simp_all [Code.all, Code.allInstrs]
  | rseq arm body =>
    simp only [Code.allInstrs, List.all_append, Bool.and_eq_true, List.all_flatMap,
      List.all_eq_true] at h
    simp only [Code.all, Bool.and_eq_true, List.all_eq_true]
    exact h
  | jump is t => simpa [Code.all, Code.allInstrs] using h
  | _ => simp_all [Code.all, Code.allInstrs]

end VG
