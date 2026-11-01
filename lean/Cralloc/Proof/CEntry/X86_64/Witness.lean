module

public import Cralloc.Proof.CEntry.X86_64.Facts

/-!
# Checking a C routine's precondition on a concrete state

As `Proof/Slab/X86_64/Witness.lean` for the slab routines: the layout and
the regions decided by evaluation, and the invariant from a memory in which
one class, 1, has a mask and only slab 0's word of it may name a magazine.
-/

@[expose] public section

namespace VG.Proof.CEntry.X86_64

open VG.X86_64 VG.Spec.Slab VG.Spec.CEntry VG.Proof.Slab.X86_64 VG.Proof.CEntry

/-- A read of up to 8 bytes apart from every word of `mkMem ws` is zero. -/
theorem mkMem_readW_clear_le {ws : List (Addr × BitVec 64)} {a : Addr} {w : Nat} (hw : w / 8 ≤ 8)
    (h : ∀ bv ∈ ws, bv.1.toNat + 8 ≤ a.toNat ∨ (a.toNat + 8 ≤ bv.1.toNat ∧ bv.1.toNat + 8 ≤ 2 ^ 64))
    (ha : a.toNat + 8 ≤ 2 ^ 64) :
    (mkMem ws).readW a w = 0 := by
  unfold Mem.readW
  rw [Mem.read_eq_of_bytes (v := 0) fun i hi => ?_]
  · simp
  · rw [mkMem_apply_none fun bv hbv => ?_]
    · simp
    · have := h bv hbv
      have hb := bv.1.isLt
      rw [BitVec.toNat_sub, BitVec.toNat_add, BitVec.toNat_ofNat, Nat.mod_eq_of_lt (a := i) (by omega),
        Nat.mod_eq_of_lt (a := a.toNat + i) (by omega)]
      omega

/-- The layout's facts, decided by `Region.sep`. -/
def layoutCheck (c : Spec.CEntry.Ctx isa) (s : State) : Bool :=
  decide ((c.slabs s).base.toNat + (c.slabs s).len ≤ 2 ^ 64) &&
  ((c.A.reserved 0 s).all fun q => Region.sep (c.slabs s) q) &&
  ((c.fixed s).all fun x => decide (x.1.base.toNat + x.1.len ≤ 2 ^ 64)) &&
  Sig.pairsSep (c.fixed s) &&
  ((c.fixed s).all fun x => Region.sep x.1 (c.slabs s)) &&
  ((c.fixed s).all fun x => (c.A.reserved 0 s).all fun q => Region.sep x.1 q)

theorem layout_of_check {c : Spec.CEntry.Ctx isa} (hA : X86 c.A) {s : State}
    (h : layoutCheck c s = true) : c.Layout s := by
  simp only [layoutCheck, Bool.and_eq_true, List.all_eq_true, decide_eq_true_eq] at h
  obtain ⟨⟨⟨⟨⟨h1, h2⟩, h3⟩, h4⟩, h5⟩, h6⟩ := h
  exact ⟨by rw [hA.ptrBits]; exact h1, fun q hq => Region.disjoint_of_sep (h2 q hq),
    by rw [hA.ptrBits]; exact h3, Sig.pairwise_of_pairsSep h4,
    fun x hx => Region.disjoint_of_sep (h5 x hx),
    fun x hx q hq => Region.disjoint_of_sep (h6 x hx q hq)⟩

def apartCheck (c : Spec.CEntry.Ctx isa) (s : State) (r : Region) : Bool :=
  decide (r.base.toNat + r.len ≤ 2 ^ 64) && Region.sep r (c.slabs s) &&
  ((c.fixed s).all fun x => Region.sep r x.1) && ((c.A.reserved 0 s).all fun q => Region.sep r q)

theorem apart_of_check {c : Spec.CEntry.Ctx isa} (hA : X86 c.A) {s : State} {r : Region}
    (h : apartCheck c s r = true) : c.Apart s r := by
  simp only [apartCheck, Bool.and_eq_true, List.all_eq_true, decide_eq_true_eq] at h
  obtain ⟨⟨⟨h1, h2⟩, h3⟩, h4⟩ := h
  exact ⟨by rw [hA.ptrBits]; exact h1, Region.disjoint_of_sep h2,
    fun x hx => Region.disjoint_of_sep (h3 x hx), fun q hq => Region.disjoint_of_sep (h4 q hq)⟩

theorem world_of_check {c : Spec.CEntry.Ctx isa} (hA : X86 c.A) {s : State}
    (hrd : s.rd = c.ro s) (hwr : s.wr = c.rw s)
    (hnw : ((c.regions s).all fun x => decide (x.1.base.toNat + x.1.len ≤ 2 ^ 64)) = true)
    (hp : Sig.pairsSep (c.regions s) = true)
    (hres : ((c.regions s).all fun x => (c.A.reserved 0 s).all fun q => Region.sep x.1 q) = true) :
    c.World s where
  rd := by rw [hA.rd]; exact hrd
  wr := by rw [hA.wr]; exact hwr
  nowrap := by
    rw [hA.ptrBits]
    simp only [List.all_eq_true, decide_eq_true_eq] at hnw
    exact hnw
  disj := Sig.pairwise_of_pairsSep hp
  res := by
    intro x hx q hq
    simp only [List.all_eq_true] at hres
    exact Region.disjoint_of_sep (hres x hx q hq)

/-- The invariant, where only the class 1 has a mask, an allowed one, and
only slab 0's word of it may be other than zero. -/
theorem inv_one {c : Spec.CEntry.Ctx isa} {s : State}
    (hz : ∀ k < MASK_COUNT, k ≠ 1 → c.maskOf s k = 0) (hm : MaskOk (c.maskOf s 1))
    (hw0 : ∀ i, 0 < i → i < SLAB_COUNT → c.wordOf s 1 i = 0) (h0 : c.SlotOk s 1 (c.wordOf s 1 0)) :
    c.inv s := by
  intro k hk
  by_cases h1 : k = 1
  · subst h1
    have hn : ∀ i, 0 < i → i < SLAB_COUNT → ¬ IsSlot (c.slotsOf' s 1) (c.wordOf s 1 i) :=
      fun i h hi hs => by rw [hw0 i h hi] at hs; exact absurd hs.2 (by simp [slot])
    refine ⟨.inr hm, fun _ i hi => ?_, fun i hi j hj hij hsi hsj => ?_⟩
    · rcases Nat.eq_zero_or_pos i with rfl | h
      · exact h0
      · rw [hw0 i h hi]; exact ⟨.inl rfl, fun hs => absurd hs.2 (by simp [slot])⟩
    · rcases Nat.eq_zero_or_pos i with rfl | h
      · exact absurd hsj (hn j (by omega) hj)
      · exact absurd hsi (hn i h hi)
  · have hz' := hz k hk h1
    have hs : ∀ w, ¬ IsSlot (c.slotsOf' s k) w := fun w => by
      simp only [Spec.CEntry.Ctx.slotsOf', hz']; exact Proof.Slab.not_isSlot_slotsOf_zero w
    exact ⟨.inl hz', fun h => absurd hz' h, fun i _ j _ _ hsi => absurd hsi (hs _)⟩

end VG.Proof.CEntry.X86_64
