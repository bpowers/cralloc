module

public import Asmalloc.Spec.Slab
public import Asmalloc.Proof.Framework.Mem

/-!
# Stacks of spare magazines

A stack's magazines are determined by its head and memory (`Chain`), and
are unchanged by writes apart from their link slots.
-/

@[expose] public section

namespace VG.Proof.Slab

open VG.Spec.Slab

theorem chain_unique {m : Mem} {h : BitVec 64} {l₁ l₂ : List Addr} (h₁ : Chain m h l₁)
    (h₂ : Chain m h l₂) : l₁ = l₂ := by
  induction h₁ generalizing l₂ with
  | nil =>
    cases h₂ with
    | nil => rfl
    | cons h0 _ _ => simp at h0
  | cons h0 _ _ ih =>
    cases h₂ with
    | nil => simp at h0
    | cons _ _ hc => rw [ih hc]

/-- A marker's lowest bit is set. -/
theorem marker_odd {h : BitVec 64} (hm : IsMarker h) : h.toNat % 2 = 1 := by
  have := Nat.mod_mod_of_dvd h.toNat (Nat.pow_dvd_pow 2 (show 1 ≤ MARKER_SHIFT by decide))
  rw [IsMarker] at hm; rw [hm] at this; simpa using this.symm

/-- A marker heads no stack. -/
theorem not_chain_marker {m : Mem} {h : BitVec 64} {l : List Addr} (hm : IsMarker h) :
    ¬ Chain m h l := by
  intro hc
  have h2 := marker_odd hm
  cases hc with
  | nil => simp at h2
  | cons _ h8 _ => omega

/-- A stack is what it was in memory unchanged at its link slots. -/
theorem chain_frame {m m' : Mem} {h : BitVec 64} {l : List Addr} (hc : Chain m h l)
    (hm : ∀ x ∈ l, m'.readW x 64 = m.readW x 64) : Chain m' h l := by
  induction hc with
  | nil => exact .nil
  | cons h0 h8 _ ih =>
    refine .cons h0 h8 ?_
    rw [hm _ (List.mem_cons_self ..)]
    exact ih fun x hx => hm x (List.mem_cons_of_mem _ hx)

/-- The stack below a nonzero head. -/
theorem chain_tail {m : Mem} {h : BitVec 64} {l : List Addr} (hc : Chain m h l)
    (h0 : h.toNat ≠ 0) : l = h :: l.tail ∧ Chain m (m.readW h 64) l.tail := by
  cases hc with
  | nil => simp at h0
  | cons _ _ hc => exact ⟨rfl, hc⟩

/-- Every link slot of a stack the invariant allows satisfies `ok`. -/
theorem stackOk_chain {m : Mem} {h : BitVec 64} {ok : Region → Prop} (hs : StackOk m h ok)
    {l : List Addr} (hc : Chain m h l) : ∀ x ∈ l, ok ⟨x, 8⟩ := by
  rcases hs with hm | ⟨l', hc', hd⟩
  · exact absurd hc (not_chain_marker hm)
  · rw [chain_unique hc hc']; exact hd

/-- The top magazine's link slot, of a stack the invariant allows whose
head is neither zero nor a marker. -/
theorem stackOk_top {m : Mem} {h : BitVec 64} {ok : Region → Prop} (hs : StackOk m h ok)
    (h0 : h.toNat ≠ 0) (h2 : h.toNat % 2 = 0) : ok ⟨h, 8⟩ := by
  rcases hs with hm | ⟨l, hc, hd⟩
  · have := marker_odd hm; omega
  · exact hd h (by rw [(chain_tail hc h0).1]; exact List.mem_cons_self ..)

/-- A stack the invariant allows, in memory unchanged at its link slots. -/
theorem stackOk_frame {m m' : Mem} {h : BitVec 64} {ok ok' : Region → Prop} (hs : StackOk m h ok)
    (hok : ∀ r, ok r → ok' r) (hm : ∀ x, ok ⟨x, 8⟩ → m'.readW x 64 = m.readW x 64) :
    StackOk m' h ok' := by
  rcases hs with hm' | ⟨l, hc, hd⟩
  · exact .inl hm'
  · exact .inr ⟨l, chain_frame hc fun x hx => hm x (hd x hx), fun x hx => hok _ (hd x hx)⟩

theorem stackOk_zero {m : Mem} {ok : Region → Prop} : StackOk m 0 ok :=
  .inr ⟨[], .nil, by simp⟩

end VG.Proof.Slab
