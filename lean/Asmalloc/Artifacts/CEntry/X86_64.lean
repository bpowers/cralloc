module

public import Asmalloc.Proof.CEntry.X86_64.Reg
public import Asmalloc.Proof.Framework.Registration

/-! # The C entry points on x86-64

`c_malloc`, `c_free` and `c_free_sized` out of line (`x86_64`), each a
naked function that tail-calls its slow path, and each also exported as the
C symbols its `Api` names (`Api.exports`). One form only, reading glibc's
`__rseq_offset` through its GOT slot: the exported copies are expanded in
the crate that invokes the export macro, whose cargo features are not the
allocator's, and out of line the direct load saves nothing
(docs/research/zen5-roofline.md, section 13). -/

@[expose] public section

namespace VG.Artifacts.CEntry.X86_64

open VG.Spec.CEntry VG.Impl.CEntry.X86_64 VG.Proof.CEntry.X86_64

def artifacts : List Artifact := [
  mallocApi.artifact VG.X86_64.target {
    code := cMalloc
    verified := cMalloc_verified
    restartsOk := cMalloc_restartsOk
    witnesses := Malloc.witnesses
    witnessOk := cMalloc_witnessOk },
  freeApi.artifact VG.X86_64.target {
    code := cFree
    verified := cFree_verified
    restartsOk := cFree_restartsOk
    witnesses := FreeWit.F.witnesses
    witnessOk := cFree_witnessOk },
  freeSizedApi.artifact VG.X86_64.target {
    code := cFreeSized
    verified := cFreeSized_verified
    restartsOk := cFreeSized_restartsOk
    witnesses := FreeWit.S.witnesses
    witnessOk := cFreeSized_witnessOk }]

end VG.Artifacts.CEntry.X86_64
