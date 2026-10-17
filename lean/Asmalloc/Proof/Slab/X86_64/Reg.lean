module

public import Asmalloc.Proof.Slab.X86_64.PopInlineWit
public import Asmalloc.Proof.Slab.X86_64.PushInlineWit
public import Asmalloc.Proof.Slab.X86_64.PopTableWit
public import Asmalloc.Proof.Slab.X86_64.PushTableWit
public import Asmalloc.Proof.Slab.X86_64.Cas
public import Asmalloc.Proof.Slab.X86_64.StackPush
public import Asmalloc.Proof.Slab.X86_64.StackPop
public import Asmalloc.TCB.Registration

/-!
# The routines' proofs, as their registration states them

Against each `Api`'s contract on its target, with no tables of constants
(`Abi.withConsts`, which is then the calling convention itself) and no stack.
-/

@[expose] public section

namespace VG.Proof.Slab.X86_64

open VG.X86_64 VG.Spec.Slab VG.Impl.Slab.X86_64

theorem withConsts_nil : abi.withConsts [] = abi := rfl
theorem withConsts_nil_inline : inlineAbi.withConsts [] = inlineAbi := rfl

theorem popInline_verified :
    Verified inlineTarget popInline (popInlineApi.contract (inlineTarget.abi.withConsts []) 0) := by
  show Verified inlineTarget popInline (popInlineContract (inlineAbi.withConsts []))
  rw [withConsts_nil_inline]; exact PopInline.verified

theorem pushInline_verified :
    Verified inlineTarget pushInline (pushInlineApi.contract (inlineTarget.abi.withConsts []) 0) := by
  show Verified inlineTarget pushInline (pushInlineContract (inlineAbi.withConsts []))
  rw [withConsts_nil_inline]; exact PushInline.verified

theorem popTable_verified :
    Verified target popTable (popApi.contract (target.abi.withConsts []) 0) := by
  show Verified target popTable (popContract (abi.withConsts []))
  rw [withConsts_nil]; exact PopTable.verified

theorem pushTable_verified :
    Verified target pushTable (pushApi.contract (target.abi.withConsts []) 0) := by
  show Verified target pushTable (pushContract (abi.withConsts []))
  rw [withConsts_nil]; exact PushTable.verified

theorem popInline_witnessOk :
    (∃ w ∈ PopInline.witnesses, (popInlineApi.contract (inlineTarget.abi.withConsts []) 0).pre w) ∧
    ∀ o ∈ popInline.outcomes, ∃ w ∈ PopInline.witnesses,
      (popInlineApi.contract (inlineTarget.abi.withConsts []) 0).pre w ∧
        Visits inlineTarget.isa popInline w o.1 o.2 := by
  show (∃ w ∈ PopInline.witnesses, (popInlineContract (inlineAbi.withConsts [])).pre w) ∧ _
  rw [withConsts_nil_inline]; exact PopInline.witnessOk

theorem pushInline_witnessOk :
    (∃ w ∈ PushInline.witnesses, (pushInlineApi.contract (inlineTarget.abi.withConsts []) 0).pre w) ∧
    ∀ o ∈ pushInline.outcomes, ∃ w ∈ PushInline.witnesses,
      (pushInlineApi.contract (inlineTarget.abi.withConsts []) 0).pre w ∧
        Visits inlineTarget.isa pushInline w o.1 o.2 := by
  show (∃ w ∈ PushInline.witnesses, (pushInlineContract (inlineAbi.withConsts [])).pre w) ∧ _
  rw [withConsts_nil_inline]; exact PushInline.witnessOk

theorem popTable_witnessOk :
    (∃ w ∈ PopTable.witnesses, (popApi.contract (target.abi.withConsts []) 0).pre w) ∧
    ∀ o ∈ popTable.outcomes, ∃ w ∈ PopTable.witnesses,
      (popApi.contract (target.abi.withConsts []) 0).pre w ∧ Visits target.isa popTable w o.1 o.2 := by
  show (∃ w ∈ PopTable.witnesses, (popContract (abi.withConsts [])).pre w) ∧ _
  rw [withConsts_nil]; exact PopTable.witnessOk

theorem pushTable_witnessOk :
    (∃ w ∈ PushTable.witnesses, (pushApi.contract (target.abi.withConsts []) 0).pre w) ∧
    ∀ o ∈ pushTable.outcomes, ∃ w ∈ PushTable.witnesses,
      (pushApi.contract (target.abi.withConsts []) 0).pre w ∧ Visits target.isa pushTable w o.1 o.2 := by
  show (∃ w ∈ PushTable.witnesses, (pushContract (abi.withConsts [])).pre w) ∧ _
  rw [withConsts_nil]; exact PushTable.witnessOk

theorem popInline_clobbersOk : inlineTarget.inline = true → ∀ s,
    (popInlineApi.contract (inlineTarget.abi.withConsts []) 0).pre s →
      (∀ s', Exec inlineTarget.isa popInline s s' →
        inlineTarget.preservedExcept (PopInline.clobbers ++
          (if popInlineApi.sig.ret.isSome then [inlineTarget.retReg] else [])) s s') ∧
      (∀ t s', ExecExit inlineTarget.isa popInline s t s' →
        inlineTarget.preservedExcept PopInline.clobbers s s') ∧
      (∀ p ∈ popInline.abortStates inlineTarget.isa s,
        inlineTarget.preservedExcept PopInline.clobbers s p) := fun _ s hs => by
  have hs' : (popInlineContract inlineAbi).pre s := by
    rw [← withConsts_nil_inline]; exact hs
  exact PopInline.clobbersOk hs'

theorem pushInline_clobbersOk : inlineTarget.inline = true → ∀ s,
    (pushInlineApi.contract (inlineTarget.abi.withConsts []) 0).pre s →
      (∀ s', Exec inlineTarget.isa pushInline s s' →
        inlineTarget.preservedExcept (PushInline.clobbers ++
          (if pushInlineApi.sig.ret.isSome then [inlineTarget.retReg] else [])) s s') ∧
      (∀ t s', ExecExit inlineTarget.isa pushInline s t s' →
        inlineTarget.preservedExcept PushInline.clobbers s s') ∧
      (∀ p ∈ pushInline.abortStates inlineTarget.isa s,
        inlineTarget.preservedExcept PushInline.clobbers s p) := fun _ s hs => by
  have hs' : (pushInlineContract inlineAbi).pre s := by
    rw [← withConsts_nil_inline]; exact hs
  exact PushInline.clobbersOk hs'

theorem cas_verified :
    Verified target cas (casApi.contract (target.abi.withConsts []) 0) := by
  show Verified target cas (casContract (abi.withConsts []))
  rw [withConsts_nil]; exact Cas.verified

theorem cas_witnessOk :
    (∃ w ∈ Cas.witnesses, (casApi.contract (target.abi.withConsts []) 0).pre w) ∧
    ∀ o ∈ cas.outcomes, ∃ w ∈ Cas.witnesses,
      (casApi.contract (target.abi.withConsts []) 0).pre w ∧ Visits target.isa cas w o.1 o.2 := by
  show (∃ w ∈ Cas.witnesses, (casContract (abi.withConsts [])).pre w) ∧ _
  rw [withConsts_nil]; exact Cas.witnessOk

theorem stackPush_verified :
    Verified target stackPush (stackPushApi.contract (target.abi.withConsts []) 0) := by
  show Verified target stackPush (stackPushContract (abi.withConsts []))
  rw [withConsts_nil]; exact StackPush.verified

theorem stackPush_witnessOk :
    (∃ w ∈ StackPush.witnesses, (stackPushApi.contract (target.abi.withConsts []) 0).pre w) ∧
    ∀ o ∈ stackPush.outcomes, ∃ w ∈ StackPush.witnesses,
      (stackPushApi.contract (target.abi.withConsts []) 0).pre w ∧ Visits target.isa stackPush w o.1 o.2 := by
  show (∃ w ∈ StackPush.witnesses, (stackPushContract (abi.withConsts [])).pre w) ∧ _
  rw [withConsts_nil]; exact StackPush.witnessOk

theorem stackPop_verified :
    Verified target stackPop (stackPopApi.contract (target.abi.withConsts []) 0) := by
  show Verified target stackPop (stackPopContract (abi.withConsts []))
  rw [withConsts_nil]; exact StackPop.verified

theorem stackPop_witnessOk :
    (∃ w ∈ StackPop.witnesses, (stackPopApi.contract (target.abi.withConsts []) 0).pre w) ∧
    ∀ o ∈ stackPop.outcomes, ∃ w ∈ StackPop.witnesses,
      (stackPopApi.contract (target.abi.withConsts []) 0).pre w ∧ Visits target.isa stackPop w o.1 o.2 := by
  show (∃ w ∈ StackPop.witnesses, (stackPopContract (abi.withConsts [])).pre w) ∧ _
  rw [withConsts_nil]; exact StackPop.witnessOk

end VG.Proof.Slab.X86_64
