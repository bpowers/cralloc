module

public import Asmalloc.Proof.Slab.X86_64.Entry
public import Asmalloc.Proof.Framework.Registration

/-! # The front end's routines on x86-64

The pop and push inline (`x86_64_inline`), an `asm!` block of 13 fused
micro-ops, and out of line (`x86_64`), a naked function that tail-calls its
slow path; the slow path's store-if-equal and spare-stack push and pop,
out of line; and the CPU number, both ways. -/

@[expose] public section

namespace VG.Artifacts.Slab.X86_64

open VG.Spec.Slab VG.Impl.Slab.X86_64 VG.Proof.Slab.X86_64

def artifacts : List Artifact := [
  popInlineApi.artifact VG.X86_64.inlineTarget {
    code := popInline
    verified := popInline_verified
    restartsOk := popInline_restartsOk
    clobbers := PopInline.clobbers
    clobbersOk := popInline_clobbersOk
    witnesses := PopInline.witnesses
    witnessOk := popInline_witnessOk },
  pushInlineApi.artifact VG.X86_64.inlineTarget {
    code := pushInline
    verified := pushInline_verified
    restartsOk := pushInline_restartsOk
    clobbers := PushInline.clobbers
    clobbersOk := pushInline_clobbersOk
    witnesses := PushInline.witnesses
    witnessOk := pushInline_witnessOk },
  popApi.artifact VG.X86_64.target {
    code := popTable
    verified := popTable_verified
    restartsOk := popTable_restartsOk
    witnesses := PopTable.witnesses
    witnessOk := popTable_witnessOk },
  pushApi.artifact VG.X86_64.target {
    code := pushTable
    verified := pushTable_verified
    restartsOk := pushTable_restartsOk
    witnesses := PushTable.witnesses
    witnessOk := pushTable_witnessOk },
  casApi.artifact VG.X86_64.target {
    code := cas
    verified := cas_verified
    restartsOk := cas_restartsOk
    witnesses := Cas.witnesses
    witnessOk := cas_witnessOk },
  stackPushApi.artifact VG.X86_64.target {
    code := stackPush
    verified := stackPush_verified
    restartsOk := stackPush_restartsOk
    witnesses := StackPush.witnesses
    witnessOk := stackPush_witnessOk },
  stackPopApi.artifact VG.X86_64.target {
    code := stackPop
    verified := stackPop_verified
    restartsOk := stackPop_restartsOk
    witnesses := StackPop.witnesses
    witnessOk := stackPop_witnessOk },
  currentCpuApi.artifact VG.X86_64.target {
    code := currentCpu
    verified := currentCpu_verified
    witnesses := [Cpu.wit]
    witnessOk := currentCpu_witnessOk },
  currentCpuApi.artifact VG.X86_64.inlineTarget {
    code := currentCpu
    verified := currentCpuInline_verified
    clobbersOk := currentCpuInline_clobbersOk
    witnesses := [Cpu.wit]
    witnessOk := currentCpuInline_witnessOk }]

end VG.Artifacts.Slab.X86_64
