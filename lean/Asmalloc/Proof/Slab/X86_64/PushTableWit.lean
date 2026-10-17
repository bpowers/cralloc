module

public import Asmalloc.Proof.Slab.X86_64.PushTable
public import Asmalloc.Proof.Slab.X86_64.PopTableWit

/-! # The out-of-line push's witnesses: the out-of-line pop's states. -/

@[expose] public section

namespace VG.Proof.Slab.X86_64.PushTable

open VG.X86_64 VG.Spec.Slab VG.Impl.Slab.X86_64 VG.Proof.Slab.X86_64

theorem pre_w (s : State) (hm : ∃ cpu word rd wr, s = PopTable.witT cpu word rd wr)
    (hrd : s.rd = Ctx.ro cx s) (hwr : s.wr = Ctx.rw cx s)
    (hnw : ((cx.regions s).all fun x => decide (x.1.base.toNat + x.1.len ≤ 2 ^ 64)) = true)
    (hp : Sig.pairsSep (cx.regions s) = true)
    (hres : ((cx.regions s).all fun x => Region.sep x.1 ⟨s.gpr .rsp, 8⟩) = true)
    (hcls : cx.cls s = 1) (hmask : cx.mask s = magMask (2 ^ 2)) (hslots : cx.slots s = 4)
    (hsyms : s.syms SLABS = 0x1000000) (h0 : WordOk 4 (s.mem.readW (wordAddr 0x1000000 0 1) 64)) :
    (pushContract abi).pre s := by
  refine (pre_iff cx (s := s) hv rfl rfl).mpr ⟨trivial, world_of_check rfl hrd hwr hnw hp hres,
    rfl, rfl, show cx.cls s < CLASS_LIMIT by rw [hcls]; decide, ⟨2, by decide, by decide, hmask⟩,
    fun i hi => ?_⟩
  rw [hslots, mem_of (c := cx) rfl, sym_of (c := cx) rfl, hcls, hsyms]
  rcases Nat.eq_zero_or_pos i with rfl | h
  · exact h0
  · obtain ⟨cpu, word, rd, wr, rfl⟩ := hm
    rw [PopTable.witT_zero cpu word rd wr h hi]
    exact .inl rfl

theorem pre_w0 : (pushContract abi).pre PopTable.w0 :=
  pre_w _ ⟨_, _, _, _, rfl⟩ (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (by decide +kernel) (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (by decide +kernel)

theorem pre_w1 : (pushContract abi).pre PopTable.w1 :=
  pre_w _ ⟨_, _, _, _, rfl⟩ (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (by decide +kernel) (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (by decide +kernel)

theorem pre_w2 : (pushContract abi).pre PopTable.w2 :=
  pre_w _ ⟨_, _, _, _, rfl⟩ (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (by decide +kernel) (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (by decide +kernel)

theorem outcomes_eq : pushTable.outcomes = [(0, true), (1, true), (2, true)] := rfl

theorem visits_w0 : Visits isa pushTable PopTable.w0 0 true := by
  obtain ⟨k, hp⟩ := pre_of (facts_of_pre pre_w0)
  have hi : ¬ cx.InRange PopTable.w0 := by decide +kernel
  exact Visits.rseq (execBlock_arm hp.facts.toArm) (end_C hp hi)

theorem visits_w1 : Visits isa pushTable PopTable.w1 1 true := by
  obtain ⟨k, hp⟩ := pre_of (facts_of_pre pre_w1)
  have hi : cx.InRange PopTable.w1 := by decide +kernel
  have ht : tst PopTable.w1 = 0 := by decide +kernel
  exact Visits.rseq (execBlock_arm hp.facts.toArm) (end_T hp hi ht)

theorem visits_w2 : Visits isa pushTable PopTable.w2 2 true := by
  obtain ⟨k, hp⟩ := pre_of (facts_of_pre pre_w2)
  have hi : cx.InRange PopTable.w2 := by decide +kernel
  have ht : ¬ tst PopTable.w2 = 0 := by decide +kernel
  exact Visits.rseq (execBlock_arm hp.facts.toArm) (end_P hp hi ht)

def witnesses : List State := [PopTable.w0, PopTable.w1, PopTable.w2]

theorem witnessOk : (∃ w ∈ witnesses, (pushContract abi).pre w) ∧
    ∀ o ∈ pushTable.outcomes, ∃ w ∈ witnesses, (pushContract abi).pre w ∧
      Visits isa pushTable w o.1 o.2 := by
  refine ⟨⟨PopTable.w0, by simp [witnesses], pre_w0⟩, fun o ho => ?_⟩
  rw [outcomes_eq] at ho
  simp only [List.mem_cons, List.not_mem_nil, or_false] at ho
  rcases ho with rfl | rfl | rfl
  · exact ⟨PopTable.w0, by simp [witnesses], pre_w0, visits_w0⟩
  · exact ⟨PopTable.w1, by simp [witnesses], pre_w1, visits_w1⟩
  · exact ⟨PopTable.w2, by simp [witnesses], pre_w2, visits_w2⟩

theorem verified : Verified target pushTable (pushContract abi) :=
  ⟨fun _ hs => correct hs, fun _ hs => rseqOk hs, ⟨_, pre_w0⟩, fun o ho => by
    obtain ⟨w, -, hw, hv⟩ := witnessOk.2 o ho
    exact ⟨w, hw, hv⟩⟩

end VG.Proof.Slab.X86_64.PushTable
