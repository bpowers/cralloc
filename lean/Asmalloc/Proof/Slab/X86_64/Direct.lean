module

public import Asmalloc.Proof.Slab.X86_64.Entry
public import Asmalloc.Proof.Slab.X86_64.Cpu
public import Asmalloc.Proof.Framework.Arm

/-!
# The routines reading `__rseq_offset` directly

Each routine's direct form (`Impl.Slab.X86_64.direct`, `currentCpuDirect`)
differs from its GOT form only in how it reads `__rseq_offset`: from every
state satisfying the precondition, `armDirect` ends in the state `arm` ends
in, with the same accesses (`armDirect_same`). So each direct form is
verified, keeps its clobbers and its caller's entry, and visits its
branches, by its GOT form's proofs (`Proof/Framework/Arm.lean`).
-/

@[expose] public section

namespace VG.Proof.Slab.X86_64

open VG.X86_64 VG.Spec.Slab VG.Impl.Slab.X86_64

section Generic

variable {k : Contract isa} {body : List (Step Instr Cond)} (harm : ∀ s, k.pre s → ArmOk s)
include harm

theorem hx_direct : ∀ s, k.pre s → execBlock isa armDirect s = execBlock isa arm s :=
  fun s hs => (armDirect_same (harm s hs)).1

theorem verified_direct (h : Verified target (.rseq arm body) k) :
    Verified target (.rseq armDirect body) k :=
  Verified.of_arm h (hx_direct harm) (fun s hs => (armDirect_same (harm s hs)).2.1)
    (fun s hs => (armDirect_same (harm s hs)).2.2)

theorem verified_direct_inline (h : Verified inlineTarget (.rseq arm body) k) :
    Verified inlineTarget (.rseq armDirect body) k :=
  Verified.of_arm h (hx_direct harm) (fun s hs => (armDirect_same (harm s hs)).2.1)
    (fun s hs => (armDirect_same (harm s hs)).2.2)

theorem witnessOk_direct {ws : List State}
    (h : (∃ w ∈ ws, k.pre w) ∧ ∀ o ∈ Code.outcomes (Code.rseq arm body), ∃ w ∈ ws, k.pre w ∧
      Visits isa (.rseq arm body) w o.1 o.2) :
    (∃ w ∈ ws, k.pre w) ∧ ∀ o ∈ Code.outcomes (Code.rseq armDirect body), ∃ w ∈ ws, k.pre w ∧
      Visits isa (.rseq armDirect body) w o.1 o.2 :=
  ⟨h.1, fun o ho => by
    obtain ⟨w, hw, hp, hv⟩ := h.2 o ho
    exact ⟨w, hw, hp, visits_arm (T := target) (hx_direct harm w hp) hv⟩⟩

theorem restartsOk_direct {P : State → State → Prop}
    (h : Verified target (.rseq arm body) k)
    (hok : ∀ s s', k.pre s → Restarts target (.rseq arm body) k s s' → P s s') :
    ∀ s s', k.pre s → Restarts target (.rseq armDirect body) k s s' → P s s' :=
  fun s s' hs hr => hok s s' hs (restarts_of_arm h (hx_direct harm) hs hr)

theorem restartsOk_direct_inline {P : State → State → Prop}
    (h : Verified inlineTarget (.rseq arm body) k)
    (hok : ∀ s s', k.pre s → Restarts inlineTarget (.rseq arm body) k s s' → P s s') :
    ∀ s s', k.pre s → Restarts inlineTarget (.rseq armDirect body) k s s' → P s s' :=
  fun s s' hs hr => hok s s' hs (restarts_of_arm h (hx_direct harm) hs hr)

end Generic

/-! ## Each routine -/

theorem casD_verified : Verified target (direct cas) (casContract abi) :=
  verified_direct (fun _ hs => (Cas.facts_of_pre hs).toArm) Cas.verified

theorem casD_witnessOk : (∃ w ∈ Cas.witnesses, (casContract abi).pre w) ∧
    ∀ o ∈ (direct cas).outcomes, ∃ w ∈ Cas.witnesses, (casContract abi).pre w ∧
      Visits isa (direct cas) w o.1 o.2 :=
  witnessOk_direct (fun _ hs => (Cas.facts_of_pre hs).toArm) Cas.witnessOk

theorem casD_restartsOk : ∀ s s', (casContract abi).pre s →
    Restarts target (direct cas) (casContract abi) s s' →
      target.abiPreserved s s' ∧ ArgsKept target.abi casSig s s' ∧
      (target.inline = true → target.preservedExcept [] s s') :=
  restartsOk_direct (fun _ hs => (Cas.facts_of_pre hs).toArm) Cas.verified cas_restartsOk

theorem stackPushD_verified : Verified target (direct stackPush) (stackPushContract abi) :=
  verified_direct (fun _ hs => (StackPush.facts_of_pre hs).toArm) StackPush.verified

theorem stackPushD_witnessOk : (∃ w ∈ StackPush.witnesses, (stackPushContract abi).pre w) ∧
    ∀ o ∈ (direct stackPush).outcomes, ∃ w ∈ StackPush.witnesses, (stackPushContract abi).pre w ∧
      Visits isa (direct stackPush) w o.1 o.2 :=
  witnessOk_direct (fun _ hs => (StackPush.facts_of_pre hs).toArm) StackPush.witnessOk

theorem stackPushD_restartsOk : ∀ s s', (stackPushContract abi).pre s →
    Restarts target (direct stackPush) (stackPushContract abi) s s' →
      target.abiPreserved s s' ∧ ArgsKept target.abi stackPushSig s s' ∧
      (target.inline = true → target.preservedExcept [] s s') :=
  restartsOk_direct (fun _ hs => (StackPush.facts_of_pre hs).toArm) StackPush.verified stackPush_restartsOk

theorem stackPopD_verified : Verified target (direct stackPop) (stackPopContract abi) :=
  verified_direct (fun _ hs => (StackPop.facts_of_pre hs).toArm) StackPop.verified

theorem stackPopD_witnessOk : (∃ w ∈ StackPop.witnesses, (stackPopContract abi).pre w) ∧
    ∀ o ∈ (direct stackPop).outcomes, ∃ w ∈ StackPop.witnesses, (stackPopContract abi).pre w ∧
      Visits isa (direct stackPop) w o.1 o.2 :=
  witnessOk_direct (fun _ hs => (StackPop.facts_of_pre hs).toArm) StackPop.witnessOk

theorem stackPopD_restartsOk : ∀ s s', (stackPopContract abi).pre s →
    Restarts target (direct stackPop) (stackPopContract abi) s s' →
      target.abiPreserved s s' ∧ ArgsKept target.abi stackPopSig s s' ∧
      (target.inline = true → target.preservedExcept [] s s') :=
  restartsOk_direct (fun _ hs => (StackPop.facts_of_pre hs).toArm) StackPop.verified stackPop_restartsOk

theorem popInlineD_verified : Verified inlineTarget (direct popInline) (popInlineContract inlineAbi) :=
  verified_direct_inline (fun _ hs => (PopInline.facts_of_pre hs).toArm) PopInline.verified

theorem popInlineD_witnessOk : (∃ w ∈ PopInline.witnesses, (popInlineContract inlineAbi).pre w) ∧
    ∀ o ∈ (direct popInline).outcomes, ∃ w ∈ PopInline.witnesses, (popInlineContract inlineAbi).pre w ∧
      Visits isa (direct popInline) w o.1 o.2 :=
  witnessOk_direct (fun _ hs => (PopInline.facts_of_pre hs).toArm) PopInline.witnessOk

theorem popInlineD_restartsOk : ∀ s s', (popInlineContract inlineAbi).pre s →
    Restarts inlineTarget (direct popInline) (popInlineContract inlineAbi) s s' →
      inlineTarget.abiPreserved s s' ∧ ArgsKept inlineTarget.abi popInlineSig s s' ∧
      (inlineTarget.inline = true → inlineTarget.preservedExcept PopInline.clobbers s s') :=
  restartsOk_direct_inline (fun _ hs => (PopInline.facts_of_pre hs).toArm) PopInline.verified
    popInline_restartsOk

theorem popInlineD_clobbersOk : inlineTarget.inline = true → ∀ s, (popInlineContract inlineAbi).pre s →
    (∀ s', Exec isa (direct popInline) s s' →
      preservedExcept (PopInline.clobbers ++ (if popInlineSig.ret.isSome then ["rax"] else [])) s s') ∧
    (∀ t s', ExecExit isa (direct popInline) s t s' → preservedExcept PopInline.clobbers s s') ∧
    (∀ p ∈ (direct popInline).abortStates isa s, preservedExcept PopInline.clobbers s p) :=
  fun _ _ hs => clobbersOk_of_arm (T := inlineTarget)
    (hx_direct (fun _ hs => (PopInline.facts_of_pre hs).toArm) _ hs) (PopInline.clobbersOk hs)

theorem pushInlineD_verified : Verified inlineTarget (direct pushInline) (pushInlineContract inlineAbi) :=
  verified_direct_inline (fun _ hs => (PushInline.facts_of_pre hs).toArm) PushInline.verified

theorem pushInlineD_witnessOk : (∃ w ∈ PushInline.witnesses, (pushInlineContract inlineAbi).pre w) ∧
    ∀ o ∈ (direct pushInline).outcomes, ∃ w ∈ PushInline.witnesses, (pushInlineContract inlineAbi).pre w ∧
      Visits isa (direct pushInline) w o.1 o.2 :=
  witnessOk_direct (fun _ hs => (PushInline.facts_of_pre hs).toArm) PushInline.witnessOk

theorem pushInlineD_restartsOk : ∀ s s', (pushInlineContract inlineAbi).pre s →
    Restarts inlineTarget (direct pushInline) (pushInlineContract inlineAbi) s s' →
      inlineTarget.abiPreserved s s' ∧ ArgsKept inlineTarget.abi pushInlineSig s s' ∧
      (inlineTarget.inline = true → inlineTarget.preservedExcept PushInline.clobbers s s') :=
  restartsOk_direct_inline (fun _ hs => (PushInline.facts_of_pre hs).toArm) PushInline.verified
    pushInline_restartsOk

theorem pushInlineD_clobbersOk : inlineTarget.inline = true → ∀ s, (pushInlineContract inlineAbi).pre s →
    (∀ s', Exec isa (direct pushInline) s s' →
      preservedExcept (PushInline.clobbers ++ (if pushInlineSig.ret.isSome then ["rax"] else [])) s s') ∧
    (∀ t s', ExecExit isa (direct pushInline) s t s' → preservedExcept PushInline.clobbers s s') ∧
    (∀ p ∈ (direct pushInline).abortStates isa s, preservedExcept PushInline.clobbers s p) :=
  fun _ _ hs => clobbersOk_of_arm (T := inlineTarget)
    (hx_direct (fun _ hs => (PushInline.facts_of_pre hs).toArm) _ hs) (PushInline.clobbersOk hs)

end VG.Proof.Slab.X86_64
