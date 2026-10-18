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
    gpr := fun r => if r = .rsp then 0x7000000 else if r = .rdi then 0x1000008
      else if r = .rsi then 16 else if r = .rdx then 4 else 0 }

theorem bad_rejected : ¬ (pushInlineContract inlineAbi).pre sBad := by
  intro h
  obtain ⟨-, -, -, -, -, -, hinv⟩ := (pre_iff PushInline.cx (s := sBad) PushInline.hv rfl rfl).mp h
  have hap := (hinv.1 0 (by decide)).2 (by decide +kernel)
  exact hap.2.1 0x1001000 (by decide +kernel) (by decide +kernel)

/-- CPU 0's word names slot 1, and CPU 1's slot 2, of the same magazine: a
push on CPU 0 aborted before its commit would change CPU 1's magazine. Each
slab's magazine is now its own (`Ctx.Exclusive`). -/
def sShare : State where
  gpr r := if r = .rsp then 0x7000000 else if r = .rdi then 0x1000008 else if r = .rsi then 16
    else if r = .rdx then 0x6000 else 0
  cf := none
  zf := none
  sf := none
  of := none
  mem := mkMem [(0x2000, 0x100), (0x10104, 0), (0x1000008, 0x4008), (0x1001008, 0x4010),
    (0x4008, 0x5000)]
  rd := PopInline.roW
  wr := [⟨0x10108, 8⟩, ⟨0x1000008, 8⟩, ⟨0x4000, 32⟩]
  syms n := if n = "__rseq_offset" then 0x2000 else if n = "SLABS" then 0x1000000 else 0
  fsBase := 0x10000
  params _ := 0
  csDesc := 0x9000

theorem share_rejected : ¬ (pushInlineContract inlineAbi).pre sShare := by
  intro h
  obtain ⟨-, -, -, -, -, -, -, hex⟩ := (pre_iff PushInline.cx (s := sShare) PushInline.hv rfl rfl).mp h
  exact hex 0 (by decide) 1 (by decide) (by decide) (by decide +kernel) (by decide +kernel) 0x4000
    (by decide +kernel) (by decide +kernel)

end VG.Test.Retry
