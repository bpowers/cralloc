module

public import Asmalloc.Proof.Slab.X86_64.PushInline
public import Asmalloc.Proof.Slab.X86_64.PopInlineWit

/-!
# The inline push's witnesses

The inline pop's three states (`PopInline.w0`, `w1`, `w2`): a CPU number without a
slab, an empty slab word, and a magazine of 4 slots holding one pointer,
which has a slot to spare.
-/

@[expose] public section

namespace VG.Proof.Slab.X86_64.PushInline

open VG.X86_64 VG.Spec.Slab VG.Impl.Slab.X86_64 VG.Proof.Slab.X86_64

theorem pre_wit (s : State) (hm : ∃ cpu word rd wr, s = PopInline.wit cpu word rd wr)
    (hrd : s.rd = Ctx.ro cx s) (hwr : s.wr = Ctx.rw cx s)
    (hnw : ((cx.regions s).all fun x => decide (x.1.base.toNat + x.1.len ≤ 2 ^ 64)) = true)
    (hp : Sig.pairsSep (cx.regions s) = true)
    (hres : ((cx.regions s).all fun x => (cx.A.reserved 0 s).all fun q => Region.sep x.1 q) = true)
    (hl : layoutCheck cx s = true)
    (hoff : (cx.arg s 0 - cx.sym s SLABS).toNat = 8 * cx.cls s) (hcls : cx.cls s = 1)
    (hmask : cx.mask s = magMask (2 ^ 2)) (hslots : cx.slots s = 4)
    (hsyms : s.syms SLABS = 0x1000000)
    (h0 : cx.SlotOk s (s.mem.readW (wordAddr 0x1000000 0 1) 64)) :
    (pushInlineContract inlineAbi).pre s := by
  have hzero : ∀ i, 0 < i → i < SLAB_COUNT → cx.wordOf s i = 0 := by
    intro i h hi
    obtain ⟨cpu, word, rd, wr, rfl⟩ := hm
    simp only [Spec.Slab.Ctx.wordOf]
    rw [mem_of (c := cx) hX, sym_of (c := cx) hX, hcls, hsyms]
    exact PopInline.wit_zero cpu word rd wr h hi
  have hslot : ∀ i < SLAB_COUNT, cx.SlotOk s (cx.wordOf s i) := by
    intro i hi
    rcases Nat.eq_zero_or_pos i with rfl | h
    · simp only [Spec.Slab.Ctx.wordOf]
      rw [mem_of (c := cx) hX, sym_of (c := cx) hX, hcls, hsyms]
      exact h0
    · rw [hzero i h hi]; exact ⟨.inl rfl, fun hs => absurd hs.2 (by simp [slot])⟩
  have hex : cx.Exclusive s := by
    have hn : ∀ k, 0 < k → k < SLAB_COUNT → ¬ IsSlot (cx.slots s) (cx.wordOf s k) :=
      fun k h hk hs => by rw [hzero k h hk] at hs; exact absurd hs.2 (by simp [slot])
    intro i hi j hj hij hsi hsj
    rcases Nat.eq_zero_or_pos i with rfl | h
    · exact absurd hsj (hn j (by omega) hj)
    · exact absurd hsi (hn i h hi)
  refine (pre_iff cx (s := s) hv rfl rfl).mpr ⟨trivial, world_of_check hX hrd hwr hnw hp hres,
    rfl, rfl, ⟨hoff, by rw [hcls]; decide, ⟨2, by decide, by decide, hmask⟩⟩,
    layout_of_check hX hl, hslot, hex⟩

theorem pre_w0 : (pushInlineContract inlineAbi).pre PopInline.w0 :=
  pre_wit _ ⟨_, _, _, _, rfl⟩ (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (by decide +kernel) (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (by decide +kernel) (by decide +kernel)
    ⟨by decide +kernel, fun hs => absurd hs (by decide +kernel)⟩

theorem pre_w1 : (pushInlineContract inlineAbi).pre PopInline.w1 :=
  pre_wit _ ⟨_, _, _, _, rfl⟩ (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (by decide +kernel) (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (by decide +kernel) (by decide +kernel)
    ⟨by decide +kernel, fun hs => absurd hs (by decide +kernel)⟩

theorem pre_w2 : (pushInlineContract inlineAbi).pre PopInline.w2 :=
  pre_wit _ ⟨_, _, _, _, rfl⟩ (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (by decide +kernel) (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (by decide +kernel) (by decide +kernel)
    ⟨by decide +kernel, fun _ => apart_of_check hX (by decide +kernel)⟩

theorem outcomes_eq : pushInline.outcomes = [(0, true), (1, true), (2, true)] := rfl

theorem visits_w0 : Visits isa pushInline PopInline.w0 0 true := by
  obtain ⟨k, hp⟩ := pre_of (facts_of_pre pre_w0)
  have hi : ¬ cx.InRange PopInline.w0 := by decide +kernel
  exact Visits.rseq (execBlock_arm hp.facts.toArm) (end_C hp hi)

theorem visits_w1 : Visits isa pushInline PopInline.w1 1 true := by
  obtain ⟨k, hp⟩ := pre_of (facts_of_pre pre_w1)
  have hi : cx.InRange PopInline.w1 := by decide +kernel
  have ht : tst PopInline.w1 = 0 := by decide +kernel
  exact Visits.rseq (execBlock_arm hp.facts.toArm) (end_T hp hi ht)

theorem visits_w2 : Visits isa pushInline PopInline.w2 2 true := by
  obtain ⟨k, hp⟩ := pre_of (facts_of_pre pre_w2)
  have hi : cx.InRange PopInline.w2 := by decide +kernel
  have ht : ¬ tst PopInline.w2 = 0 := by decide +kernel
  exact Visits.rseq (execBlock_arm hp.facts.toArm) (end_P hp hi ht)

def witnesses : List State := [PopInline.w0, PopInline.w1, PopInline.w2]

theorem witnessOk : (∃ w ∈ witnesses, (pushInlineContract inlineAbi).pre w) ∧
    ∀ o ∈ pushInline.outcomes, ∃ w ∈ witnesses, (pushInlineContract inlineAbi).pre w ∧
      Visits isa pushInline w o.1 o.2 := by
  refine ⟨⟨PopInline.w0, by simp [witnesses], pre_w0⟩, fun o ho => ?_⟩
  rw [outcomes_eq] at ho
  simp only [List.mem_cons, List.not_mem_nil, or_false] at ho
  rcases ho with rfl | rfl | rfl
  · exact ⟨PopInline.w0, by simp [witnesses], pre_w0, visits_w0⟩
  · exact ⟨PopInline.w1, by simp [witnesses], pre_w1, visits_w1⟩
  · exact ⟨PopInline.w2, by simp [witnesses], pre_w2, visits_w2⟩

theorem verified : Verified inlineTarget pushInline (pushInlineContract inlineAbi) :=
  ⟨fun _ hs => correct hs, fun _ hs => rseqOk hs, ⟨_, pre_w0⟩, fun o ho => by
    obtain ⟨w, -, hw, hv⟩ := witnessOk.2 o ho
    exact ⟨w, hw, hv⟩⟩

end VG.Proof.Slab.X86_64.PushInline
