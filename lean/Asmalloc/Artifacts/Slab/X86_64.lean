module

public import Asmalloc.Proof.Slab.X86_64.Reg
public import Asmalloc.Proof.Framework.Registration

/-! # The front end's pop and push on x86-64

Inline (`x86_64_inline`), an `asm!` block of 13 fused micro-ops, and out of
line (`x86_64`), a naked function that tail-calls its slow path. -/

@[expose] public section

namespace VG.Artifacts.Slab.X86_64

open VG.Spec.Slab VG.Impl.Slab.X86_64 VG.Proof.Slab.X86_64

def artifacts : List Artifact := [
  popInlineApi.artifact VG.X86_64.inlineTarget {
    code := popInline
    verified := popInline_verified
    clobbers := PopInline.clobbers
    clobbersOk := popInline_clobbersOk
    witnesses := PopInline.witnesses
    witnessOk := popInline_witnessOk },
  pushInlineApi.artifact VG.X86_64.inlineTarget {
    code := pushInline
    verified := pushInline_verified
    clobbers := PushInline.clobbers
    clobbersOk := pushInline_clobbersOk
    witnesses := PushInline.witnesses
    witnessOk := pushInline_witnessOk },
  popApi.artifact VG.X86_64.target {
    code := popTable
    verified := popTable_verified
    witnesses := PopTable.witnesses
    witnessOk := popTable_witnessOk },
  pushApi.artifact VG.X86_64.target {
    code := pushTable
    verified := pushTable_verified
    witnesses := PushTable.witnesses
    witnessOk := pushTable_witnessOk }]

end VG.Artifacts.Slab.X86_64
