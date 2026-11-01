module

public import Cralloc.Proof.Slab.X86_64.Direct
public import Cralloc.Proof.Framework.Registration

/-! # The front end's routines on x86-64

The pop and push inline (`x86_64_inline`), an `asm!` block of 13 fused
micro-ops; the slow path's store-if-equal and spare-stack push and pop,
out of line (`x86_64`); and the CPU number, inline. Each in two forms: reading
`__rseq_offset` through a word holding its address (`movGot`, the default, which links anywhere), or
directly (`direct`, which links only in an executable), which the crate's
cargo feature `executable` selects, and musl always does: there the word is
the crate's own (`VG.X86_64.ownRseqArea`), and a musl build of the crate is
an executable's. -/

@[expose] public section

namespace VG.Artifacts.Slab.X86_64

open VG.Spec.Slab VG.Impl.Slab.X86_64 VG.Proof.Slab.X86_64

def artifacts : List Artifact := [
  popInlineApi.artifact VG.X86_64.inlineTarget {
    cfg := some ([.feature "executable", .targetEnv "musl"], false)
    code := popInline
    verified := popInline_verified
    restartsOk := popInline_restartsOk
    clobbers := PopInline.clobbers
    clobbersOk := popInline_clobbersOk
    witnesses := PopInline.witnesses
    witnessOk := popInline_witnessOk },
  popInlineApi.artifact VG.X86_64.inlineTarget {
    cfg := some ([.feature "executable", .targetEnv "musl"], true)
    code := (direct popInline)
    verified := popInlineD_verified
    restartsOk := popInlineD_restartsOk
    clobbers := PopInline.clobbers
    clobbersOk := popInlineD_clobbersOk
    witnesses := PopInline.witnesses
    witnessOk := popInlineD_witnessOk },
  pushInlineApi.artifact VG.X86_64.inlineTarget {
    cfg := some ([.feature "executable", .targetEnv "musl"], false)
    code := pushInline
    verified := pushInline_verified
    restartsOk := pushInline_restartsOk
    clobbers := PushInline.clobbers
    clobbersOk := pushInline_clobbersOk
    witnesses := PushInline.witnesses
    witnessOk := pushInline_witnessOk },
  pushInlineApi.artifact VG.X86_64.inlineTarget {
    cfg := some ([.feature "executable", .targetEnv "musl"], true)
    code := (direct pushInline)
    verified := pushInlineD_verified
    restartsOk := pushInlineD_restartsOk
    clobbers := PushInline.clobbers
    clobbersOk := pushInlineD_clobbersOk
    witnesses := PushInline.witnesses
    witnessOk := pushInlineD_witnessOk },
  -- One form only: read directly, `__rseq_offset` would leave it 75 bytes,
  -- past one line by too little (`scripts/check_sizes.py`), and out of
  -- line the direct load saves nothing (docs/research/zen5-roofline.md,
  -- section 13).
  casApi.artifact VG.X86_64.target {
    code := cas
    verified := cas_verified
    restartsOk := cas_restartsOk
    witnesses := Cas.witnesses
    witnessOk := cas_witnessOk },
  stackPushApi.artifact VG.X86_64.target {
    cfg := some ([.feature "executable", .targetEnv "musl"], false)
    code := stackPush
    verified := stackPush_verified
    restartsOk := stackPush_restartsOk
    witnesses := StackPush.witnesses
    witnessOk := stackPush_witnessOk },
  stackPushApi.artifact VG.X86_64.target {
    cfg := some ([.feature "executable", .targetEnv "musl"], true)
    code := (direct stackPush)
    verified := stackPushD_verified
    restartsOk := stackPushD_restartsOk
    witnesses := StackPush.witnesses
    witnessOk := stackPushD_witnessOk },
  stackPopApi.artifact VG.X86_64.target {
    cfg := some ([.feature "executable", .targetEnv "musl"], false)
    code := stackPop
    verified := stackPop_verified
    restartsOk := stackPop_restartsOk
    witnesses := StackPop.witnesses
    witnessOk := stackPop_witnessOk },
  stackPopApi.artifact VG.X86_64.target {
    cfg := some ([.feature "executable", .targetEnv "musl"], true)
    code := (direct stackPop)
    verified := stackPopD_verified
    restartsOk := stackPopD_restartsOk
    witnesses := StackPop.witnesses
    witnessOk := stackPopD_witnessOk },
  currentCpuApi.artifact VG.X86_64.inlineTarget {
    cfg := some ([.feature "executable", .targetEnv "musl"], false)
    code := currentCpu
    verified := currentCpuInline_verified
    clobbersOk := currentCpuInline_clobbersOk
    witnesses := [Cpu.wit]
    witnessOk := currentCpuInline_witnessOk },
  currentCpuApi.artifact VG.X86_64.inlineTarget {
    cfg := some ([.feature "executable", .targetEnv "musl"], true)
    code := currentCpuDirect
    verified := Cpu.verifiedD_inline
    clobbersOk := fun _ _ hs => Cpu.clobbersOkD hs
    witnesses := [Cpu.wit]
    witnessOk := ⟨⟨_, List.mem_singleton_self _, Cpu.pre_wit_inline⟩, fun _ ho => nomatch ho⟩ }]

end VG.Artifacts.Slab.X86_64
