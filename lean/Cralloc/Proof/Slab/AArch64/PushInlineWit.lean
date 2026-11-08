module

public import Cralloc.Proof.Slab.AArch64.PushInline
public import Cralloc.Proof.Slab.AArch64.Witness

/-!
# The inline push's witnesses

`w0`, `w1` and `w2`, one for each way out: a CPU number without a slab, an empty slab word,
and a magazine of 4 slots holding one pointer, which has a slot to spare.
-/

@[expose] public section

namespace VG.Proof.Slab.AArch64.PushInline

open VG.AArch64 VG.Spec.Slab VG.Impl.Slab.AArch64 VG.Proof.Slab.AArch64

theorem pre_w0 : attempt.pre w0 :=
  ⟨pre_wit (c := cx) rfl hX hv (fun _ => rfl) (fun _ => rfl) w0 ⟨_, _, _, _, rfl⟩ (by decide +kernel)
    (by decide +kernel) (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (by decide +kernel)
    ⟨by decide +kernel, fun hs => absurd hs (by decide +kernel)⟩, by decide +kernel, by decide +kernel⟩

theorem pre_w1 : attempt.pre w1 :=
  ⟨pre_wit (c := cx) rfl hX hv (fun _ => rfl) (fun _ => rfl) w1 ⟨_, _, _, _, rfl⟩ (by decide +kernel)
    (by decide +kernel) (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (by decide +kernel)
    ⟨by decide +kernel, fun hs => absurd hs (by decide +kernel)⟩, by decide +kernel, by decide +kernel⟩

theorem pre_w2 : attempt.pre w2 :=
  ⟨pre_wit (c := cx) rfl hX hv (fun _ => rfl) (fun _ => rfl) w2 ⟨_, _, _, _, rfl⟩ (by decide +kernel)
    (by decide +kernel) (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (by decide +kernel)
    ⟨by decide +kernel, fun _ => apart_of_check hX.ptrBits (by decide +kernel)⟩,
    by decide +kernel, by decide +kernel⟩

theorem outcomes_eq : pushInline.outcomes = [(0, true), (1, true), (2, true)] := rfl

theorem visits_w0 : Visits isa pushInline w0 0 true := by
  obtain ⟨k, hp⟩ := pre_of (facts_of_pre pre_w0)
  have hi : ¬ cx.InRange w0 := by decide +kernel
  exact Visits.rseq (execBlock_arm hp.facts.toArm) (end_C hp hi)

theorem visits_w1 : Visits isa pushInline w1 1 true := by
  obtain ⟨k, hp⟩ := pre_of (facts_of_pre pre_w1)
  have hi : cx.InRange w1 := by decide +kernel
  have ht : tst w1 = 0 := by decide +kernel
  exact Visits.rseq (execBlock_arm hp.facts.toArm) (end_T hp hi ht)

theorem visits_w2 : Visits isa pushInline w2 2 true := by
  obtain ⟨k, hp⟩ := pre_of (facts_of_pre pre_w2)
  have hi : cx.InRange w2 := by decide +kernel
  have ht : ¬ tst w2 = 0 := by decide +kernel
  exact Visits.rseq (execBlock_arm hp.facts.toArm) (end_P hp hi ht)

/-- The witnesses. -/
def witnesses : List State := [w0, w1, w2]

theorem witnessOk : (∃ w ∈ witnesses, attempt.pre w) ∧
    ∀ o ∈ pushInline.outcomes, ∃ w ∈ witnesses, attempt.pre w ∧ Visits isa pushInline w o.1 o.2 := by
  refine ⟨⟨w0, by simp [witnesses], pre_w0⟩, fun o ho => ?_⟩
  rw [outcomes_eq] at ho
  simp only [List.mem_cons, List.not_mem_nil, or_false] at ho
  rcases ho with rfl | rfl | rfl
  · exact ⟨w0, by simp [witnesses], pre_w0, visits_w0⟩
  · exact ⟨w1, by simp [witnesses], pre_w1, visits_w1⟩
  · exact ⟨w2, by simp [witnesses], pre_w2, visits_w2⟩

theorem verified : Verified inlineTargetX4 pushInline attempt :=
  ⟨fun _ hs => correct hs, fun _ hs => rseqOk hs, ⟨w0, pre_w0⟩, fun o ho => by
    obtain ⟨w, -, hw, hv⟩ := witnessOk.2 o ho
    exact ⟨w, hw, hv⟩⟩

/-- The last attempt, aborted any number of times, starts with the registers of the
first but the clobbers: the arguments, which are none of them, and the stack pointer. -/
theorem restartsOk : ∀ s s', attempt.pre s → Restarts inlineTargetX4 pushInline attempt s s' →
    inlineTargetX4.abiPreserved s s' ∧ ArgsKept inlineTargetX4.abi pushInlineRetSig s s' ∧
    (inlineTargetX4.inline = true → inlineTargetX4.preservedExcept clobbers s s') :=
  restartsOk_inline (A := inlineAbiX4) (ret := "x4") verified (fun _ hs => (clobbersOk hs).2.2)
    fun _ _ hpe => argsKept_regs hX (by decide) fun r hr => hpe r (by revert r; decide)

end VG.Proof.Slab.AArch64.PushInline
