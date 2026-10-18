module

public import Asmalloc.Proof.CEntry.X86_64.Reg
public import Asmalloc.Proof.Framework.Registration

/-! # The C entry points on x86-64

`c_malloc`, `c_free` and `c_free_sized`, and `c_delete` and `c_delete_sized`, out of line (`x86_64`), each a
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
    witnessOk := cFreeSized_witnessOk .c },
  -- The same code with the `delete` family's mirror, leaving for its slow paths.
  deleteApi.artifact VG.X86_64.target {
    code := cFree .delete
    verified := cFree_verified .delete
    restartsOk := cFree_restartsOk .delete
    witnesses := FreeWit.F.witnesses .delete
    witnessOk := cFree_witnessOk .delete },
  deleteSizedApi.artifact VG.X86_64.target {
    code := cFreeSized .delete
    verified := cFreeSized_verified .delete
    restartsOk := cFreeSized_restartsOk .delete
    witnesses := FreeWit.S.witnesses .delete
    witnessOk := cFreeSized_witnessOk .delete }]

end VG.Artifacts.CEntry.X86_64
