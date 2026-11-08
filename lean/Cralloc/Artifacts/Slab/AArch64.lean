module

public import Cralloc.Proof.Slab.AArch64.PopInlineWit
public import Cralloc.Proof.Slab.AArch64.PushInlineWit
public import Cralloc.Proof.Slab.AArch64.Cas
public import Cralloc.Proof.Slab.AArch64.StackPush
public import Cralloc.Proof.Slab.AArch64.StackPop
public import Cralloc.Proof.Slab.AArch64.Cpu
public import Cralloc.Proof.Framework.Registration

/-! # The front end's routines on AArch64

The pop, the push and the CPU number inline (`aarch64_inline`), and the slow path's
store-if-equal and spare-stack push and pop out of line (`aarch64`). One form, reading
`__rseq_offset` through a word holding its address, which links anywhere; on musl, where the
word is the crate's own, there is none yet. -/

@[expose] public section

namespace VG.Artifacts.Slab.AArch64

open VG.Spec.Slab VG.Impl.Slab.AArch64 VG.Proof.Slab.AArch64

def artifacts : List Artifact := [
  (popInlineApi a64Docs).artifact VG.AArch64.inlineTarget {
    cfg := some ([.targetEnv "musl"], false)
    setup := setup
    code := popInline
    attempt := PopInline.attempt
    verified := PopInline.verified
    restartsOk := PopInline.restartsOk
    setupOk := PopInline.setupOk
    clobbers := PopInline.clobbers
    clobbersOk := fun _ _ hs => PopInline.clobbersOk hs
    witnesses := PopInline.witnesses
    witnessOk := PopInline.witnessOk },
  (pushInlineRetApi a64Docs).artifact VG.AArch64.inlineTargetX4 {
    cfg := some ([.targetEnv "musl"], false)
    setup := setup
    code := pushInline
    attempt := PushInline.attempt
    verified := PushInline.verified
    restartsOk := PushInline.restartsOk
    setupOk := PushInline.setupOk
    clobbers := PushInline.clobbers
    clobbersOk := fun _ _ hs => PushInline.clobbersOk hs
    witnesses := PushInline.witnesses
    witnessOk := PushInline.witnessOk },
  (casApi a64Docs).artifact VG.AArch64.target {
    cfg := some ([.targetEnv "musl"], false)
    setup := slowSetup
    code := cas
    attempt := Cas.attempt
    verified := Cas.verified
    restartsOk := Cas.restartsOk
    setupOk := Cas.setupOk
    witnesses := Cas.witnesses
    witnessOk := Cas.witnessOk },
  (stackPushApi a64Docs).artifact VG.AArch64.target {
    cfg := some ([.targetEnv "musl"], false)
    setup := slowSetup
    code := stackPush
    attempt := StackPush.attempt
    verified := StackPush.verified
    restartsOk := StackPush.restartsOk
    setupOk := StackPush.setupOk
    witnesses := StackPush.witnesses
    witnessOk := StackPush.witnessOk },
  (stackPopApi a64Docs).artifact VG.AArch64.target {
    cfg := some ([.targetEnv "musl"], false)
    setup := slowSetup
    code := stackPop
    attempt := StackPop.attempt
    verified := StackPop.verified
    restartsOk := StackPop.restartsOk
    setupOk := StackPop.setupOk
    witnesses := StackPop.witnesses
    witnessOk := StackPop.witnessOk },
  (currentCpuApi a64Docs).artifact VG.AArch64.inlineTarget {
    cfg := some ([.targetEnv "musl"], false)
    code := currentCpu
    verified := Cpu.verified
    clobbers := Cpu.clobbers
    clobbersOk := fun _ _ hs => Cpu.clobbersOk hs
    witnesses := [Cpu.wit]
    witnessOk := Cpu.witnessOk }]

end VG.Artifacts.Slab.AArch64
