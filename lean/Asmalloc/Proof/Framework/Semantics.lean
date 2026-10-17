module

public import Batteries.Tactic.Init
public import Batteries.Tactic.PermuteGoals
public import Batteries.Tactic.SeqFocus
public import Mathlib.Util.CompileInductive
public import Asmalloc.TCB.Artifact
public import Asmalloc.Proof.Framework.GetElem

/-!
# Reasoning about `Exec`: determinism, weakest preconditions, constant time
-/

@[expose] public section

namespace VG

variable {M : ISA}

theorem execBlock_append (l₁ l₂ : List M.Instr) (s : M.State) :
    execBlock M (l₁ ++ l₂) s =
      (execBlock M l₁ s).bind fun p =>
        (execBlock M l₂ p.1).map fun q => (q.1, p.2 ++ q.2) := by
  induction l₁ generalizing s with
  | nil => simp [execBlock]
  | cons i is ih =>
    simp only [List.cons_append, execBlock]
    cases M.exec i s with
    | none => rfl
    | some s₁ =>
      simp only [ih]
      cases execBlock M is s₁ with
      | none => rfl
      | some p => simp [Option.map_map, Function.comp_def]

/-- The semantics is deterministic. -/
theorem Exec.det {c : Prog M} {s s₁ s₂ : M.State} {t₁ t₂ : List Leak}
    (h₁ : Exec M c s t₁ s₁) (h₂ : Exec M c s t₂ s₂) : t₁ = t₂ ∧ s₁ = s₂ := by
  induction h₁ generalizing t₂ s₂ with
  | block h => cases h₂ with
    | block h' => rw [h] at h'; cases h'; exact ⟨rfl, rfl⟩
  | seq _ _ ih₁ ih₂ => cases h₂ with
    | seq a b =>
      obtain ⟨rfl, rfl⟩ := ih₁ a
      obtain ⟨rfl, rfl⟩ := ih₂ b
      exact ⟨rfl, rfl⟩
  | iteT hc _ ih => cases h₂ with
    | iteT _ b => obtain ⟨rfl, rfl⟩ := ih b; exact ⟨rfl, rfl⟩
    | iteF hc' _ => rw [hc] at hc'; cases hc'
  | iteF hc _ ih => cases h₂ with
    | iteT hc' _ => rw [hc] at hc'; cases hc'
    | iteF _ b => obtain ⟨rfl, rfl⟩ := ih b; exact ⟨rfl, rfl⟩
  | loopExit _ hc ih => cases h₂ with
    | loopExit a _ => obtain ⟨rfl, rfl⟩ := ih a; exact ⟨rfl, rfl⟩
    | loopNext a hc' _ =>
      obtain ⟨rfl, rfl⟩ := ih a; rw [hc] at hc'; cases hc'
  | loopNext _ hc _ ih₁ ih₂ => cases h₂ with
    | loopExit a hc' => obtain ⟨rfl, rfl⟩ := ih₁ a; rw [hc] at hc'; cases hc'
    | loopNext a _ b =>
      obtain ⟨rfl, rfl⟩ := ih₁ a
      obtain ⟨rfl, rfl⟩ := ih₂ b
      exact ⟨rfl, rfl⟩
  | call hc _ hr ih => cases h₂ with
    | call hc' b hr' =>
      rw [hc] at hc'; cases hc'
      obtain ⟨rfl, rfl⟩ := ih b
      rw [hr] at hr'; cases hr'
      exact ⟨rfl, rfl⟩
  | frame hp _ hq ih => cases h₂ with
    | frame hp' b hq' =>
      rw [hp] at hp'; cases hp'
      obtain ⟨rfl, rfl⟩ := ih b
      rw [hq] at hq'; cases hq'
      exact ⟨rfl, rfl⟩

theorem Exec.block_iff {is : List M.Instr} {s s' : M.State} {t : List Leak} :
    Exec M (.block is) s t s' ↔ execBlock M is s = some (s', t) :=
  ⟨fun h => by cases h with | block h => exact h, .block⟩

/-! ## Total-correctness weakest preconditions -/

/-- `WP M c s Q`: from `s`, `c` terminates without faulting in a state satisfying `Q`. -/
def WP (M : ISA) (c : Prog M) (s : M.State) (Q : M.State → Prop) : Prop :=
  ∃ t s', Exec M c s t s' ∧ Q s'

namespace WP

theorem mono {c : Prog M} {s : M.State} {Q Q' : M.State → Prop}
    (h : WP M c s Q) (hq : ∀ s, Q s → Q' s) : WP M c s Q' := by
  obtain ⟨t, s', he, hq'⟩ := h; exact ⟨t, s', he, hq _ hq'⟩

/-- Two postconditions of the same code hold together, as the semantics is
deterministic. -/
theorem and {c : Prog M} {s : M.State} {Q R : M.State → Prop}
    (h₁ : WP M c s Q) (h₂ : WP M c s R) : WP M c s fun t => Q t ∧ R t := by
  obtain ⟨t₁, s₁, e₁, q⟩ := h₁
  obtain ⟨t₂, s₂, e₂, r⟩ := h₂
  obtain ⟨-, rfl⟩ := Exec.det e₁ e₂
  exact ⟨t₁, s₁, e₁, q, r⟩

theorem block {is : List M.Instr} {s : M.State} {Q : M.State → Prop}
    (h : ∃ p, execBlock M is s = some p ∧ Q p.1) : WP M (.block is) s Q := by
  obtain ⟨⟨s', t⟩, h1, h2⟩ := h; exact ⟨t, s', .block h1, h2⟩

theorem block_append {l₁ l₂ : List M.Instr} {s : M.State} {Q : M.State → Prop}
    (h : WP M (.block l₁) s (fun s₁ => WP M (.block l₂) s₁ Q)) :
    WP M (.block (l₁ ++ l₂)) s Q := by
  obtain ⟨t, s₁, h1, t', s₂, h2, hq⟩ := h
  rw [Exec.block_iff] at h1 h2
  exact ⟨t ++ t', s₂, .block (by rw [execBlock_append, h1]; simp [h2]), hq⟩

theorem block_nil {s : M.State} {Q : M.State → Prop} (h : Q s) : WP M (.block []) s Q :=
  ⟨[], s, .block rfl, h⟩

theorem seq {c₁ c₂ : Prog M} {s : M.State} {Q : M.State → Prop}
    (h : WP M c₁ s (fun s₁ => WP M c₂ s₁ Q)) : WP M (.seq c₁ c₂) s Q := by
  obtain ⟨t, s₁, h1, t', s₂, h2, hq⟩ := h; exact ⟨_, _, .seq h1 h2, hq⟩

theorem ite {c : M.Cond} {th el : Prog M} {s : M.State} {Q : M.State → Prop} (b : Bool)
    (hc : M.eval c s = some b) (ht : b = true → WP M th s Q) (he : b = false → WP M el s Q) :
    WP M (.ite c th el) s Q := by
  cases b
  · obtain ⟨t, s', h1, h2⟩ := he rfl; exact ⟨_, _, .iteF hc h1, h2⟩
  · obtain ⟨t, s', h1, h2⟩ := ht rfl; exact ⟨_, _, .iteT hc h1, h2⟩

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
    obtain ⟨t, s', h1, h2⟩ := hstep n s hs
    rcases h2 with ⟨hc, hq⟩ | ⟨hc, m, hm, hi⟩
    · exact ⟨_, _, .loopExit h1 hc, hq⟩
    · obtain ⟨t', s'', h3, hq⟩ := ih m hm s' hi
      exact ⟨_, _, .loopNext h1 hc h3, hq⟩

end WP

/-! ## Constant time -/

/-- Constant time follows from a *leakage function*: if every run from a
`Pre`-state `s` leaks exactly `f s`, and `f` only depends on public data. -/
theorem ConstantTime.of_leakage {Pre : M.State → Prop} {Pub : M.State → M.State → Prop}
    {c : Prog M} (f : M.State → List Leak)
    (hf : ∀ s t s', Pre s → Exec M c s t s' → t = f s)
    (hpub : ∀ s₁ s₂, Pre s₁ → Pre s₂ → Pub s₁ s₂ → f s₁ = f s₂) :
    ConstantTime M Pre Pub c := by
  intro s₁ s₂ t₁ t₂ s₁' s₂' h₁ h₂ hp e₁ e₂
  rw [hf _ _ _ h₁ e₁, hf _ _ _ h₂ e₂, hpub _ _ h₁ h₂ hp]

/-- A program that never leaks anything is constant time for any `Pub`. -/
theorem ConstantTime.of_silent {Pre : M.State → Prop} {Pub : M.State → M.State → Prop}
    {c : Prog M} (h : ∀ s t s', Pre s → Exec M c s t s' → t = []) :
    ConstantTime M Pre Pub c :=
  .of_leakage (fun _ => []) h (fun _ _ _ _ _ => rfl)

/-- The instructions of structured code (including the pushes and pops of
its frames), and of the functions it calls. -/
def instrs {I C : Type} : Code I C → List I
  | .block is => is
  | .seq a b => instrs a ++ instrs b
  | .ite _ t e => instrs t ++ instrs e
  | .loop b _ => instrs b
  | .call _ b => instrs b
  | .frame i b j => i :: instrs b ++ [j]

/-- Whether code calls no function and has no frame: whether it runs only
its own instructions, in the stack it was entered with. -/
def Code.noCalls {I C : Type} : Code I C → Bool
  | .block _ => true
  | .seq a b => a.noCalls && b.noCalls
  | .ite _ t e => t.noCalls && e.noCalls
  | .loop b _ => b.noCalls
  | .call _ _ => false
  | .frame .. => false

/-- Whether code, and the functions it calls, has no frame. -/
def Code.noFrames {I C : Type} : Code I C → Bool
  | .block _ => true
  | .seq a b => a.noFrames && b.noFrames
  | .ite _ t e => t.noFrames && e.noFrames
  | .loop b _ => b.noFrames
  | .call _ b => b.noFrames
  | .frame .. => false

/-- How deeply frames nest in `c` and the functions it calls. -/
def Code.fdepth {I C : Type} : Code I C → Nat
  | .block _ => 0
  | .seq a b => max a.fdepth b.fdepth
  | .ite _ t e => max t.fdepth e.fdepth
  | .loop b _ => b.fdepth
  | .call _ b => b.fdepth
  | .frame _ b _ => b.fdepth + 1

theorem Code.noFrames_of_noCalls {I C : Type} {c : Code I C} (h : c.noCalls = true) :
    c.noFrames = true := by
  induction c <;> simp_all [noCalls, noFrames]

/-- `(instrs c).all p`, without building the list of instructions, which is
much faster for the kernel to evaluate (`decide +kernel`). A block is walked
with `List.rec`, which the kernel evaluates faster than `List.all` (compiled
to `brecOn`). -/
def Code.allInstrs {I C : Type} (p : I → Bool) : Code I C → Bool
  | .block is => List.rec (motive := fun _ => Bool) true (fun i _ ih => p i && ih) is
  | .seq a b => a.allInstrs p && b.allInstrs p
  | .ite _ t e => t.allInstrs p && e.allInstrs p
  | .loop b _ => b.allInstrs p
  | .call _ b => b.allInstrs p
  | .frame i b j => p i && b.allInstrs p && p j

theorem Code.allInstrs_eq {I C : Type} (p : I → Bool) (c : Code I C) :
    c.allInstrs p = (instrs c).all p := by
  induction c with
  | block is => induction is <;> simp_all [allInstrs, instrs]
  | _ => simp [allInstrs, instrs, List.all_append, Bool.and_assoc, *]

/-- `Code.all p` holds of all code when `p` holds of every instruction: for
`Artifact.spSafe` on the ISAs whose `writesSp` is always `false`, without
evaluating the code. -/
theorem Code.all_of_forall {I C : Type} {p : I → Bool} (h : ∀ i, p i = true) (c : Code I C) :
    c.all p = true := by
  induction c <;> simp_all [Code.all]

/-- `Code.all p` from `Code.allInstrs p`: for `Artifact.spSafe` on the other
ISAs, `Code.all_of_allInstrs (by lit_decide)` has the kernel evaluate the
faster `Code.allInstrs`. -/
theorem Code.all_of_allInstrs {I C : Type} {p : I → Bool} {c : Code I C} (h : c.allInstrs p = true) :
    c.all p = true := by
  induction c with
  | block is => induction is <;> simp_all [Code.all, Code.allInstrs]
  | _ => simp_all [Code.all, Code.allInstrs]

/-- Moving a proof to a contract `k'` whose states permit more than those of
`k`: each state `s` of `k'` narrows to a state `n s` of `k`, and an execution
from `n s` gives one from `s` with the same trace, ending in `w s s₁`. -/
theorem Verified.of_narrow {T : Target} {c : Prog T.isa} {k k' : Contract T.isa} (h : Verified T c k)
    (n : T.isa.State → T.isa.State) (w : T.isa.State → T.isa.State → T.isa.State)
    (hpre : ∀ s, k'.pre s → k.pre (n s))
    (hexec : ∀ s t s₁, k'.pre s → Exec T.isa c (n s) t s₁ → Exec T.isa c s t (w s s₁))
    (hpost : ∀ s t s₁, k'.pre s → Exec T.isa c (n s) t s₁ → T.abiPreserved (n s) s₁ →
      k.post (n s) s₁ → T.abiPreserved s (w s s₁) ∧ k'.post s (w s s₁))
    (hpub : ∀ s₁ s₂, k'.pre s₁ → k'.pre s₂ → k'.pub s₁ s₂ → k.pub (n s₁) (n s₂))
    (hsat : ∃ s, k'.pre s) : Verified T c k' := by
  obtain ⟨hc, hct, -⟩ := h
  refine ⟨fun s hs => ?_, fun s₁ s₂ t₁ t₂ s₁' s₂' h₁ h₂ hp e₁ e₂ => ?_, hsat⟩
  · obtain ⟨t, s₁, he, ha, hq⟩ := hc _ (hpre s hs)
    exact ⟨t, w s s₁, hexec s t s₁ hs he, hpost s t s₁ hs he ha hq⟩
  · obtain ⟨u₁, r₁, f₁, -⟩ := hc _ (hpre _ h₁)
    obtain ⟨u₂, r₂, f₂, -⟩ := hc _ (hpre _ h₂)
    rw [(Exec.det e₁ (hexec _ _ _ h₁ f₁)).1, (Exec.det e₂ (hexec _ _ _ h₂ f₂)).1]
    exact hct _ _ _ _ _ _ (hpre _ h₁) (hpre _ h₂) (hpub _ _ h₁ h₂ hp) f₁ f₂

end VG
