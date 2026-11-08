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
store-if-equal and spare-stack push and pop out of line (`aarch64`). Two forms of each: reading
`__rseq_offset` through a word holding its address, which links anywhere; and on musl, where the
area is the crate's own and has no word, with its distance from the thread pointer in the
instructions (`muslSetup`). There the crate also needs that distance itself, to find the area it
registers (`area_offset`). -/

@[expose] public section

namespace VG.Artifacts.Slab.AArch64

open VG.Spec.Slab VG.Impl.Slab.AArch64 VG.Proof.Slab.AArch64

def artifacts : List Artifact := [
  popInlineApi.artifact VG.AArch64.inlineTarget {
    cfg := some ([.targetEnv "musl"], false)
    setup := setup
    code := popInline
    attempt := PopInline.attempt
    verified := PopInline.verified
    restartsOk := PopInline.restartsOk
    setupOk := PopInline.setupOk setupArea
    clobbers := PopInline.clobbers
    clobbersOk := fun _ _ hs => PopInline.clobbersOk hs
    witnesses := PopInline.witnesses
    witnessOk := PopInline.witnessOk },
  popInlineApi.artifact VG.AArch64.inlineTarget {
    cfg := some ([.targetEnv "musl"], true)
    setup := muslSetup
    code := popInline
    attempt := PopInline.attempt
    verified := PopInline.verified
    restartsOk := PopInline.restartsOk
    setupOk := PopInline.setupOk muslArea
    clobbers := PopInline.clobbers
    clobbersOk := fun _ _ hs => PopInline.clobbersOk hs
    witnesses := PopInline.witnesses
    witnessOk := PopInline.witnessOk },
  pushInlineRetApi.artifact VG.AArch64.inlineTargetX4 {
    cfg := some ([.targetEnv "musl"], false)
    setup := setup
    code := pushInline
    attempt := PushInline.attempt
    verified := PushInline.verified
    restartsOk := PushInline.restartsOk
    setupOk := PushInline.setupOk setupArea
    clobbers := PushInline.clobbers
    clobbersOk := fun _ _ hs => PushInline.clobbersOk hs
    witnesses := PushInline.witnesses
    witnessOk := PushInline.witnessOk },
  pushInlineRetApi.artifact VG.AArch64.inlineTargetX4 {
    cfg := some ([.targetEnv "musl"], true)
    setup := muslSetup
    code := pushInline
    attempt := PushInline.attempt
    verified := PushInline.verified
    restartsOk := PushInline.restartsOk
    setupOk := PushInline.setupOk muslArea
    clobbers := PushInline.clobbers
    clobbersOk := fun _ _ hs => PushInline.clobbersOk hs
    witnesses := PushInline.witnesses
    witnessOk := PushInline.witnessOk },
  casApi.artifact VG.AArch64.target {
    cfg := some ([.targetEnv "musl"], false)
    setup := slowSetup
    code := cas
    attempt := Cas.attempt
    verified := Cas.verified
    restartsOk := Cas.restartsOk
    setupOk := Cas.setupOk setupArea
    witnesses := Cas.witnesses
    witnessOk := Cas.witnessOk },
  casApi.artifact VG.AArch64.target {
    cfg := some ([.targetEnv "musl"], true)
    setup := slowMuslSetup
    code := cas
    attempt := Cas.attempt
    verified := Cas.verified
    restartsOk := Cas.restartsOk
    setupOk := Cas.setupOk muslArea
    witnesses := Cas.witnesses
    witnessOk := Cas.witnessOk },
  stackPushApi.artifact VG.AArch64.target {
    cfg := some ([.targetEnv "musl"], false)
    setup := slowSetup
    code := stackPush
    attempt := StackPush.attempt
    verified := StackPush.verified
    restartsOk := StackPush.restartsOk
    setupOk := StackPush.setupOk setupArea
    witnesses := StackPush.witnesses
    witnessOk := StackPush.witnessOk },
  stackPushApi.artifact VG.AArch64.target {
    cfg := some ([.targetEnv "musl"], true)
    setup := slowMuslSetup
    code := stackPush
    attempt := StackPush.attempt
    verified := StackPush.verified
    restartsOk := StackPush.restartsOk
    setupOk := StackPush.setupOk muslArea
    witnesses := StackPush.witnesses
    witnessOk := StackPush.witnessOk },
  stackPopApi.artifact VG.AArch64.target {
    cfg := some ([.targetEnv "musl"], false)
    setup := slowSetup
    code := stackPop
    attempt := StackPop.attempt
    verified := StackPop.verified
    restartsOk := StackPop.restartsOk
    setupOk := StackPop.setupOk setupArea
    witnesses := StackPop.witnesses
    witnessOk := StackPop.witnessOk },
  stackPopApi.artifact VG.AArch64.target {
    cfg := some ([.targetEnv "musl"], true)
    setup := slowMuslSetup
    code := stackPop
    attempt := StackPop.attempt
    verified := StackPop.verified
    restartsOk := StackPop.restartsOk
    setupOk := StackPop.setupOk muslArea
    witnesses := StackPop.witnesses
    witnessOk := StackPop.witnessOk },
  currentCpuApi.artifact VG.AArch64.inlineTarget {
    cfg := some ([.targetEnv "musl"], false)
    code := currentCpu
    verified := Cpu.verified
    clobbers := Cpu.clobbers
    clobbersOk := fun _ _ hs => Cpu.clobbersOk hs
    witnesses := [Cpu.wit]
    witnessOk := Cpu.witnessOk },
  currentCpuApi.artifact VG.AArch64.inlineTarget {
    cfg := some ([.targetEnv "musl"], true)
    code := currentCpuMusl
    verified := Cpu.Musl.verified
    clobbers := Cpu.Musl.clobbers
    clobbersOk := fun _ _ hs => Cpu.Musl.clobbersOk hs
    witnesses := [Cpu.wit]
    witnessOk := Cpu.Musl.witnessOk },
  areaOffsetApi.artifact VG.AArch64.inlineTarget {
    cfg := some ([.targetEnv "musl"], true)
    code := areaOffset
    verified := Cpu.Offset.verified
    clobbers := Cpu.Offset.clobbers
    clobbersOk := fun _ _ _ => Cpu.Offset.clobbersOk
    witnesses := [Cpu.Offset.witO]
    witnessOk := Cpu.Offset.witnessOk }]

end VG.Artifacts.Slab.AArch64
