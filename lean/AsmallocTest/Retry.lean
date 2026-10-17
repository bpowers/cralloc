import Asmalloc.Proof.Slab.X86_64.PushInlineWit

/-!
# A retry's precondition is proved, not assumed

A review built a state satisfying the inline push's precondition in which
CPU 0's slab word is the address of CPU 1's (read as slot 1 of a 4-slot
magazine), so the push's store into its free slot, before the commit,
lands in CPU 1's slab word, and no state the thread may resume in satisfies
the precondition. Now the invariant places every magazine apart from the
slabs (`Ctx.Apart`), and that state does not satisfy it.
-/

open VG VG.X86_64 VG.Spec.Slab VG.Proof.Slab VG.Proof.Slab.X86_64

namespace VG.Test.Retry

/-- CPU 0's word of class 1 is `SLABS + 4096 + 8`, CPU 1's word's address. -/
def sBad : State :=
  { PopInline.wit 0 0x1001008 PopInline.roW [⟨0x10108, 8⟩, ⟨0x1000008, 8⟩, ⟨0x1001000, 32⟩] with
    gpr := fun r => if r = .rsp then 0x7000000 else if r = .rdi then 4 else 0 }

theorem bad_rejected : ¬ (pushInlineContract inlineAbi).pre sBad := by
  intro h
  obtain ⟨-, -, -, -, -, -, hinv⟩ := (pre_iff PushInline.cx (s := sBad) PushInline.hv rfl rfl).mp h
  have hap := (hinv 0 (by decide)).2 (by decide +kernel)
  exact hap.2.1 0x1001000 (by decide +kernel) (by decide +kernel)

end VG.Test.Retry
