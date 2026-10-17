module

public import Asmalloc.Spec.Slab
public import Asmalloc.Proof.Framework.Sig

/-!
# The routines' precondition, unfolded

A routine's contract (`Spec.Slab.contract`) is built with `Sig.contract` on
the calling convention `Abi.withExtra (some c.regions)`. For a signature
with no buffers and no arguments in memory, its precondition says: the
convention's well-formedness, that the state's regions are exactly
`c.regions` (`World`), and `Ctx.pre`.
-/

@[expose] public section

namespace VG.Proof.Slab

open VG.Spec.Slab

namespace Ctx

variable {M : ISA} (c : Spec.Slab.Ctx M) (s : M.State)

/-- The regions the routine may read but not write, and those it may write. -/
def ro : List Region := ((c.regions s).filter (!·.2)).map (·.1)
def rw : List Region := ((c.regions s).filter (·.2)).map (·.1)

/-- The state's regions are the routine's, which do not wrap around, do not
overlap where one is written, and do not overlap the return address. -/
structure World : Prop where
  rd : c.A.rd s = ro c s
  wr : c.A.wr s = rw c s
  nowrap : ∀ x ∈ c.regions s, x.1.base.toNat + x.1.len ≤ 2 ^ c.A.ptrBits
  disj : (c.regions s).Pairwise (fun a b => (a.2 || b.2) → a.1.Disjoint b.1)
  res : ∀ x ∈ c.regions s, ∀ r ∈ c.A.reserved 0 s, x.1.Disjoint r

end Ctx

theorem take_drop_eq {α : Type} {l r : List α} (h₁ : l.take (l.length - r.length) = [])
    (h₂ : l.drop (l.length - r.length) = r) : l = r := by
  rcases List.take_eq_nil_iff.mp h₁ with h | h
  · rw [h, List.drop_zero] at h₂; exact h₂
  · subst h; simpa using h₂

/-- The precondition of a routine with no buffers and no arguments in
memory. -/
theorem pre_iff {M : ISA} (c : Spec.Slab.Ctx M) {post : M.State → M.State → Prop}
    {exit : String → M.State → M.State → Prop} {abort : M.State → M.State → Prop}
    {trace : M.State → List Event → Prop} {s : M.State}
    {vals : M.State → List (BitVec 64)}
    (hv : c.A.args ((c.sig.words c.A.ptrBits).map (·.bits c.A.ptrBits)) = some vals)
    (hb : Sig.bufs c.sig.params (vals s) = [])
    (ha : c.A.argArea ((c.sig.words c.A.ptrBits).map (·.bits c.A.ptrBits)) s = []) :
    (contract c post exit abort trace).pre s ↔
      c.A.wf ((c.sig.words c.A.ptrBits).map (·.bits c.A.ptrBits)) 0 s ∧ Ctx.World c s ∧ c.pre s := by
  simp only [contract, Sig.contract, Abi.withExtra, hv, hb, ha, List.map_nil, List.append_nil,
    List.filter_nil, List.Pairwise.nil, List.not_mem_nil, false_implies, implies_true, true_and,
    Curry.apply_const]
  constructor
  · rintro ⟨⟨hwf, hd1, hd2, hnw, hp, hres, -, -⟩, hrd, hwr, hpre⟩
    exact ⟨hwf, ⟨take_drop_eq hrd hd1, take_drop_eq hwr hd2, hnw, hp, hres⟩, hpre⟩
  · rintro ⟨hwf, ⟨hrd, hwr, hnw, hp, hres⟩, hpre⟩
    simp only [Ctx.ro, Ctx.rw] at hrd hwr
    simp only [hrd, hwr, Nat.sub_self, List.take_zero, List.drop_zero, List.not_mem_nil,
      false_implies, implies_true, and_true, true_and]
    exact ⟨⟨hwf, hnw, hp, hres⟩, hpre⟩

section Parts

variable {M : ISA} (c : Spec.Slab.Ctx M) {post : M.State → M.State → Prop}
    {exit : String → M.State → M.State → Prop} {abort : M.State → M.State → Prop}
    {trace : M.State → List Event → Prop} {vals : M.State → List (BitVec 64)}
    (hv : c.A.args ((c.sig.words c.A.ptrBits).map (·.bits c.A.ptrBits)) = some vals)
include hv

theorem post_iff (s s' : M.State) : (contract c post exit abort trace).post s s' ↔ post s s' := by
  simp only [contract, Sig.contract, Abi.withExtra, hv, Curry.apply_const, true_and]

theorem exit_iff (t : String) (s s' : M.State) :
    (contract c post exit abort trace).exit t s s' ↔ vals s' = vals s ∧ exit t s s' := by
  simp only [contract, Sig.contract, Abi.withExtra, hv]

theorem abort_iff (s p : M.State) :
    (contract c post exit abort trace).abort s p ↔ vals p = vals s ∧ abort s p := by
  simp only [contract, Sig.contract, Abi.withExtra, hv]

end Parts

theorem trace_eq {M : ISA} (c : Spec.Slab.Ctx M) {post : M.State → M.State → Prop}
    {exit : String → M.State → M.State → Prop} {abort : M.State → M.State → Prop}
    {trace : M.State → List Event → Prop} :
    (contract c post exit abort trace).trace = trace := rfl

theorem env_eq {M : ISA} (c : Spec.Slab.Ctx M) {post : M.State → M.State → Prop}
    {exit : String → M.State → M.State → Prop} {abort : M.State → M.State → Prop}
    {trace : M.State → List Event → Prop} (p p' : M.State) :
    (contract c post exit abort trace).env p p' ↔ (contract c post exit abort trace).pre p' :=
  Iff.rfl

end VG.Proof.Slab
