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
      c.A.wf ((c.sig.words c.A.ptrBits).map (·.bits c.A.ptrBits)) 0 s ∧ c.World s ∧ c.pre s := by
  simp only [contract, Sig.contract, Abi.withExtra, hv, hb, ha, List.map_nil, List.append_nil,
    List.filter_nil, List.Pairwise.nil, List.not_mem_nil, false_implies, implies_true, true_and,
    Curry.apply_const]
  constructor
  · rintro ⟨⟨hwf, hd1, hd2, hnw, hp, hres, -, -⟩, hrd, hwr, hpre⟩
    exact ⟨hwf, ⟨take_drop_eq hrd hd1, take_drop_eq hwr hd2, hnw, hp, hres⟩, hpre⟩
  · rintro ⟨hwf, ⟨hrd, hwr, hnw, hp, hres⟩, hpre⟩
    simp only [Spec.Slab.Ctx.ro, Spec.Slab.Ctx.rw] at hrd hwr
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
    (contract c post exit abort trace).env p p' ↔
      c.A.wf ((c.sig.words c.A.ptrBits).map (·.bits c.A.ptrBits)) 0 p' ∧ c.World p' ∧ c.inv p' :=
  Iff.rfl

/-- A retry's precondition: the world `env` gives, and the arguments the
routine kept. -/
theorem pre_of_env {M : ISA} (c : Spec.Slab.Ctx M) {post : M.State → M.State → Prop}
    {exit : String → M.State → M.State → Prop} {abort : M.State → M.State → Prop}
    {trace : M.State → List Event → Prop} {vals : M.State → List (BitVec 64)} {p p' : M.State}
    (hv : c.A.args ((c.sig.words c.A.ptrBits).map (·.bits c.A.ptrBits)) = some vals)
    (hb : Sig.bufs c.sig.params (vals p') = [])
    (ha : c.A.argArea ((c.sig.words c.A.ptrBits).map (·.bits c.A.ptrBits)) p' = [])
    (he : (contract c post exit abort trace).env p p') (htp : c.A.tp.isSome) (hsym : c.A.sym.isSome)
    (hargs : c.argsOk p') : (contract c post exit abort trace).pre p' :=
  (pre_iff c hv hb ha).mpr ⟨he.1, he.2.1, htp, hsym, hargs, he.2.2⟩

end VG.Proof.Slab
