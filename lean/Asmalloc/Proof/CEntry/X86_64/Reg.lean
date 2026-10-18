module

public import Asmalloc.Proof.CEntry.X86_64.MallocWit
public import Asmalloc.Proof.CEntry.X86_64.FreeWit
public import Asmalloc.Proof.Framework.X86_64.Retry
public import Asmalloc.Proof.Slab.X86_64.Reg
public import Asmalloc.TCB.Registration

/-!
# The C routines' proofs, as their registration states them

Against each `Api`'s contract on the out-of-line target, with no tables of
constants and no stack; and, aborted any number of times, against the
caller's entry (`Artifact.restartsOk`): the arguments, the callee-saved
registers, the stack pointer and the return address are as on entry.
-/

@[expose] public section

namespace VG.Proof.CEntry.X86_64

open VG.X86_64 VG.Spec.Slab VG.Spec.CEntry VG.Impl.CEntry.X86_64 VG.Proof.Slab.X86_64

theorem cMalloc_verified :
    Verified target cMalloc (mallocApi.contract (target.abi.withConsts []) 0) := by
  show Verified target cMalloc (mallocContract (abi.withConsts []))
  rw [withConsts_nil]; exact Malloc.verified

theorem cMalloc_witnessOk :
    (∃ w ∈ Malloc.witnesses, (mallocApi.contract (target.abi.withConsts []) 0).pre w) ∧
    ∀ o ∈ cMalloc.outcomes, ∃ w ∈ Malloc.witnesses,
      (mallocApi.contract (target.abi.withConsts []) 0).pre w ∧ Visits target.isa cMalloc w o.1 o.2 := by
  show (∃ w ∈ Malloc.witnesses, (mallocContract (abi.withConsts [])).pre w) ∧ _
  rw [withConsts_nil]; exact Malloc.witnessOk

theorem cMalloc_restartsOk : ∀ s s', (mallocContract abi).pre s →
    Restarts target cMalloc (mallocContract abi) s s' →
      target.abiPreserved s s' ∧ ArgsKept target.abi mallocSig s s' ∧
      (target.inline = true → target.preservedExcept [] s s') :=
  restartsOk_target (sig := mallocSig) Malloc.verified (by decide)
    (fun s hs p hp r hr => by
      have h := ((Proof.CEntry.abort_iff Malloc.cx Malloc.hv s p).mp
        (Malloc.verified.abortOk hs hp).2.2.1).1
      have hr' : r ∈ [Reg.rdi] := hr
      simp only [List.cons.injEq, and_true] at h
      simp only [List.mem_cons, List.not_mem_nil, or_false] at hr'
      rcases hr' with rfl <;> simp_all)
    (fun _ _ he => env_retC Malloc.hX he)

theorem cFree_verified :
    Verified target cFree (freeApi.contract (target.abi.withConsts []) 0) := by
  show Verified target cFree (freeContract (abi.withConsts []))
  rw [withConsts_nil]; exact FreeWit.F.verified

theorem cFree_witnessOk :
    (∃ w ∈ FreeWit.F.witnesses, (freeApi.contract (target.abi.withConsts []) 0).pre w) ∧
    ∀ o ∈ cFree.outcomes, ∃ w ∈ FreeWit.F.witnesses,
      (freeApi.contract (target.abi.withConsts []) 0).pre w ∧ Visits target.isa cFree w o.1 o.2 := by
  show (∃ w ∈ FreeWit.F.witnesses, (freeContract (abi.withConsts [])).pre w) ∧ _
  rw [withConsts_nil]; exact FreeWit.F.witnessOk

theorem cFree_restartsOk : ∀ s s', (freeContract abi).pre s →
    Restarts target cFree (freeContract abi) s s' →
      target.abiPreserved s s' ∧ ArgsKept target.abi freeSig s s' ∧
      (target.inline = true → target.preservedExcept [] s s') :=
  restartsOk_target (sig := freeSig) FreeWit.F.verified (by decide)
    (fun s hs p hp r hr => by
      have h := ((Proof.CEntry.abort_iff Free.cx Free.hv s p).mp
        (FreeWit.F.verified.abortOk hs hp).2.2.1).1
      have hr' : r ∈ [Reg.rdi] := hr
      simp only [List.cons.injEq, and_true] at h
      simp only [List.mem_cons, List.not_mem_nil, or_false] at hr'
      rcases hr' with rfl <;> simp_all)
    (fun _ _ he => env_retC Free.hX he)

theorem cFreeSized_verified :
    Verified target cFreeSized (freeSizedApi.contract (target.abi.withConsts []) 0) := by
  show Verified target cFreeSized (freeSizedContract (abi.withConsts []))
  rw [withConsts_nil]; exact FreeWit.S.verified

theorem cFreeSized_witnessOk :
    (∃ w ∈ FreeWit.S.witnesses, (freeSizedApi.contract (target.abi.withConsts []) 0).pre w) ∧
    ∀ o ∈ cFreeSized.outcomes, ∃ w ∈ FreeWit.S.witnesses,
      (freeSizedApi.contract (target.abi.withConsts []) 0).pre w ∧
        Visits target.isa cFreeSized w o.1 o.2 := by
  show (∃ w ∈ FreeWit.S.witnesses, (freeSizedContract (abi.withConsts [])).pre w) ∧ _
  rw [withConsts_nil]; exact FreeWit.S.witnessOk

theorem cFreeSized_restartsOk : ∀ s s', (freeSizedContract abi).pre s →
    Restarts target cFreeSized (freeSizedContract abi) s s' →
      target.abiPreserved s s' ∧ ArgsKept target.abi freeSizedSig s s' ∧
      (target.inline = true → target.preservedExcept [] s s') :=
  restartsOk_target (sig := freeSizedSig) FreeWit.S.verified (by decide)
    (fun s hs p hp r hr => by
      have h := ((Proof.CEntry.abort_iff FreeSized.cx FreeSized.hv s p).mp
        (FreeWit.S.verified.abortOk hs hp).2.2.1).1
      have hr' : r ∈ [Reg.rdi, Reg.rsi] := hr
      simp only [List.cons.injEq, and_true] at h
      simp only [List.mem_cons, List.not_mem_nil, or_false] at hr'
      rcases hr' with rfl | rfl <;> simp_all)
    (fun _ _ he => env_retC FreeSized.hX he)

end VG.Proof.CEntry.X86_64
