module

public import Cralloc.Proof.CEntry.X86_64.Reg
public import Cralloc.Proof.Framework.Registration

/-! # The C entry points on x86-64

`c_malloc`, `c_calloc`, `c_free` and `c_free_sized` out of line (`x86_64`), each a
naked function that tail-calls its slow path, and each also exported as the
C symbols its `Api` names (`Api.exports`). One form only, reading glibc's
`__rseq_offset` through a word holding its address (`movGot`): the exported copies are expanded in
the crate that invokes the export macro, whose cargo features are not the
allocator's, and out of line the direct load saves nothing
(docs/research/roofline.md, "Reaching the path: inlining, LTO and how the
call is made"). -/

@[expose] public section

namespace VG.Artifacts.CEntry.X86_64

open VG.Spec.CEntry VG.Impl.CEntry.X86_64 VG.Proof.CEntry.X86_64

def artifacts : List Artifact := [
  callocApi.artifact VG.X86_64.target {
    code := cCalloc
    verified := cCalloc_verified
    restartsOk := cCalloc_restartsOk
    witnesses := Calloc.witnesses
    witnessOk := cCalloc_witnessOk },
  mallocApi.artifact VG.X86_64.target {
    code := cMalloc
    verified := cMalloc_verified
    restartsOk := cMalloc_restartsOk
    witnesses := Malloc.witnesses
    witnessOk := cMalloc_witnessOk },
  freeApi.artifact VG.X86_64.target {
    code := cFree .c
    verified := cFree_verified .c
    restartsOk := cFree_restartsOk .c
    witnesses := FreeWit.F.witnesses .c
    witnessOk := cFree_witnessOk .c },
  freeSizedApi.artifact VG.X86_64.target {
    code := cFreeSized .c
    verified := cFreeSized_verified .c
    restartsOk := cFreeSized_restartsOk .c
    witnesses := FreeWit.S.witnesses .c
    witnessOk := cFreeSized_witnessOk .c }]

end VG.Artifacts.CEntry.X86_64
