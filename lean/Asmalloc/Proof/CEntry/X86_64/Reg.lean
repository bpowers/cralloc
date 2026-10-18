module

public import Asmalloc.Proof.CEntry.X86_64.MallocWit
public import Asmalloc.Proof.CEntry.X86_64.CallocWit
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

theorem cCalloc_verified :
    Verified target cCalloc (callocApi.contract (target.abi.withConsts []) 0) := by
  show Verified target cCalloc (callocContract (abi.withConsts []))
  rw [withConsts_nil]; exact Calloc.verified

theorem cCalloc_witnessOk :
    (∃ w ∈ Calloc.witnesses, (callocApi.contract (target.abi.withConsts []) 0).pre w) ∧
    ∀ o ∈ cCalloc.outcomes, ∃ w ∈ Calloc.witnesses,
      (callocApi.contract (target.abi.withConsts []) 0).pre w ∧ Visits target.isa cCalloc w o.1 o.2 := by
  show (∃ w ∈ Calloc.witnesses, (callocContract (abi.withConsts [])).pre w) ∧ _
  rw [withConsts_nil]; exact Calloc.witnessOk

theorem cCalloc_restartsOk : ∀ s s', (callocContract abi).pre s →
    Restarts target cCalloc (callocContract abi) s s' →
      target.abiPreserved s s' ∧ ArgsKept target.abi callocSig s s' ∧
      (target.inline = true → target.preservedExcept [] s s') :=
  restartsOk_target (sig := callocSig) Calloc.verified (by decide)
    (fun s hs p hp r hr => by
      have h := ((Proof.CEntry.abort_iff Calloc.cx Calloc.hv s p).mp
        (Calloc.verified.abortOk hs hp).2.2.1).1
      have hr' : r ∈ [Reg.rdi, Reg.rsi] := hr
      simp only [List.cons.injEq, and_true] at h
      simp only [List.mem_cons, List.not_mem_nil, or_false] at hr'
      rcases hr' with rfl | rfl <;> simp_all)
    (fun _ _ he => env_retC Calloc.hX he)

/-! The frees, for any mirror (`Mirror`). -/

theorem cFree_verified (m : Mirror) :
    Verified target (cFree m) (freeContract (target.abi.withConsts []) m) := by
  show Verified target (cFree m) (freeContract (abi.withConsts []) m)
  rw [withConsts_nil]; exact FreeWit.F.verified m

theorem cFree_witnessOk (m : Mirror) :
    (∃ w ∈ FreeWit.F.witnesses m, (freeContract (target.abi.withConsts []) m).pre w) ∧
    ∀ o ∈ (cFree m).outcomes, ∃ w ∈ FreeWit.F.witnesses m,
      (freeContract (target.abi.withConsts []) m).pre w ∧ Visits target.isa (cFree m) w o.1 o.2 := by
  show (∃ w ∈ FreeWit.F.witnesses m, (freeContract (abi.withConsts []) m).pre w) ∧ _
  rw [withConsts_nil]; exact FreeWit.F.witnessOk m

theorem cFree_restartsOk (m : Mirror) : ∀ s s', (freeContract abi m).pre s →
    Restarts target (cFree m) (freeContract abi m) s s' →
      target.abiPreserved s s' ∧ ArgsKept target.abi freeSig s s' ∧
      (target.inline = true → target.preservedExcept [] s s') :=
  restartsOk_target (sig := freeSig) (FreeWit.F.verified m) (by decide)
    (fun s hs p hp r hr => by
      have h := ((Proof.CEntry.abort_iff (Free.cx m) Free.hv s p).mp
        ((FreeWit.F.verified m).abortOk hs hp).2.2.1).1
      have hr' : r ∈ [Reg.rdi] := hr
      simp only [List.cons.injEq, and_true] at h
      simp only [List.mem_cons, List.not_mem_nil, or_false] at hr'
      rcases hr' with rfl <;> simp_all)
    (fun _ _ he => env_retC (Free.hX (m := m)) he)

theorem cFreeSized_verified (m : Mirror) :
    Verified target (cFreeSized m) (freeSizedContract (target.abi.withConsts []) m) := by
  show Verified target (cFreeSized m) (freeSizedContract (abi.withConsts []) m)
  rw [withConsts_nil]; exact FreeWit.S.verified m

theorem cFreeSized_witnessOk (m : Mirror) :
    (∃ w ∈ FreeWit.S.witnesses m, (freeSizedContract (target.abi.withConsts []) m).pre w) ∧
    ∀ o ∈ (cFreeSized m).outcomes, ∃ w ∈ FreeWit.S.witnesses m,
      (freeSizedContract (target.abi.withConsts []) m).pre w ∧
        Visits target.isa (cFreeSized m) w o.1 o.2 := by
  show (∃ w ∈ FreeWit.S.witnesses m, (freeSizedContract (abi.withConsts []) m).pre w) ∧ _
  rw [withConsts_nil]; exact FreeWit.S.witnessOk m

theorem cFreeSized_restartsOk (m : Mirror) : ∀ s s', (freeSizedContract abi m).pre s →
    Restarts target (cFreeSized m) (freeSizedContract abi m) s s' →
      target.abiPreserved s s' ∧ ArgsKept target.abi freeSizedSig s s' ∧
      (target.inline = true → target.preservedExcept [] s s') :=
  restartsOk_target (sig := freeSizedSig) (FreeWit.S.verified m) (by decide)
    (fun s hs p hp r hr => by
      have h := ((Proof.CEntry.abort_iff (FreeSized.cx m) FreeSized.hv s p).mp
        ((FreeWit.S.verified m).abortOk hs hp).2.2.1).1
      have hr' : r ∈ [Reg.rdi, Reg.rsi] := hr
      simp only [List.cons.injEq, and_true] at h
      simp only [List.mem_cons, List.not_mem_nil, or_false] at hr'
      rcases hr' with rfl | rfl <;> simp_all)
    (fun _ _ he => env_retC (FreeSized.hX (m := m)) he)

end VG.Proof.CEntry.X86_64
