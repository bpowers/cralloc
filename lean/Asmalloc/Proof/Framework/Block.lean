module

public import Asmalloc.Proof.Framework.Semantics

/-!
# Running straight-line blocks symbolically

`runBlock` is the state part of `execBlock`; `WP.of_runBlock` reduces a
weakest precondition of a block to `∃ s', runBlock M is s = some s' ∧ Q s'`,
which `simp` can evaluate one instruction at a time.
-/

@[expose] public section

namespace VG

variable {M : ISA}

theorem WP.block_nil_iff {s : M.State} {Q : M.State → Prop} : WP M (.block []) s Q ↔ Q s := by
  constructor
  · rintro ⟨t, s', h, hq⟩
    rw [Exec.block_iff] at h
    simp only [execBlock, Option.some.injEq, Prod.mk.injEq] at h
    exact h.1 ▸ hq
  · exact WP.block_nil

theorem WP.block_cons_iff {i : M.Instr} {is : List M.Instr} {s : M.State} {Q : M.State → Prop} :
    WP M (.block (i :: is)) s Q ↔ ∃ s', M.exec i s = some s' ∧ WP M (.block is) s' Q := by
  constructor
  · rintro ⟨t, s', h, hq⟩
    rw [Exec.block_iff] at h
    simp only [execBlock] at h
    split at h
    · cases h
    · rename_i s₁ he
      simp only [Option.map_eq_some_iff, Prod.exists] at h
      obtain ⟨s₂, t₂, h, he'⟩ := h
      simp only [Prod.mk.injEq] at he'
      exact ⟨s₁, he, t₂, s₂, .block h, he'.1 ▸ hq⟩
  · rintro ⟨s₁, he, t, s', h, hq⟩
    rw [Exec.block_iff] at h
    exact ⟨(M.addrs i s).map Leak.addr ++ t, s', .block (by simp [execBlock, he, h]), hq⟩

theorem WP.block_append_iff {l₁ l₂ : List M.Instr} {s : M.State} {Q : M.State → Prop} :
    WP M (.block (l₁ ++ l₂)) s Q ↔ WP M (.block l₁) s (fun s' => WP M (.block l₂) s' Q) := by
  induction l₁ generalizing s with
  | nil => simp [WP.block_nil_iff]
  | cons i is ih => simp [WP.block_cons_iff, ih]

/-- The final state of a straight-line block, if it does not fault. -/
def runBlock (M : ISA) : List M.Instr → M.State → Option M.State
  | [], s => some s
  | i :: is, s => (M.exec i s).bind (runBlock M is)

/-- `runBlock` after one instruction. Unlike `Option.bind (runBlock M is)`, the
rest of the block is data rather than a function, and `runStep_congr` keeps
`simp` from looking into it, so `simp` does not evaluate the rest of the
block symbolically (from an unknown state, after every instruction) before
the instruction in front of it has run. Blocks are stepped through with each
ISA's `runBlock_cons`, `runStep_some` and `runBlock_nil`
(`Framework/<ISA>/Exec.lean`), stated at its concrete types: `simp` does not
match them otherwise. -/
def runStep (M : ISA) (o : Option M.State) (is : List M.Instr) : Option M.State :=
  o.bind (runBlock M is)

@[congr] theorem runStep_congr {o o' : Option M.State} (h : o = o') (is : List M.Instr) :
    runStep M o is = runStep M o' is := h ▸ rfl

theorem WP.of_runBlock {is : List M.Instr} {s : M.State} {Q : M.State → Prop}
    (h : ∃ s', runBlock M is s = some s' ∧ Q s') : WP M (.block is) s Q := by
  induction is generalizing s with
  | nil => obtain ⟨s', h, hq⟩ := h; cases h; exact WP.block_nil hq
  | cons i is ih =>
    obtain ⟨s', h, hq⟩ := h
    simp only [runBlock, Option.bind_eq_some_iff] at h
    obtain ⟨s₁, h₁, h₂⟩ := h
    exact WP.block_cons_iff.mpr ⟨s₁, h₁, ih ⟨s', h₂, hq⟩⟩

theorem WP.seq_iff {c₁ c₂ : Prog M} {s : M.State} {Q : M.State → Prop} :
    WP M (.seq c₁ c₂) s Q ↔ WP M c₁ s (fun s' => WP M c₂ s' Q) := by
  constructor
  · rintro ⟨t, s', h, hq⟩
    cases h with
    | seq h₁ h₂ => exact ⟨_, _, h₁, _, _, h₂, hq⟩
  · exact WP.seq

end VG
