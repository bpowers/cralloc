module

public import Cralloc.Proof.Slab.AArch64.PopInlineWit
public import Cralloc.Proof.Slab.AArch64.PushInlineWit
public import Cralloc.Proof.Framework.Registration

/-! # The front end's routines on AArch64

The pop and push inline (`aarch64_inline`). One form, reading `__rseq_offset` through a
word holding its address, which links anywhere; on musl, where the word is the crate's
own, there is none yet. -/

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
    witnessOk := PushInline.witnessOk }]

end VG.Artifacts.Slab.AArch64
