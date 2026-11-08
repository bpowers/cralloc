module

public import Cralloc.Proof.CEntry.AArch64.Calloc
public import Cralloc.Proof.CEntry.AArch64.MallocWit
public import Cralloc.Proof.CEntry.AArch64.Free
public import Cralloc.Proof.CEntry.AArch64.FreeSized
public import Cralloc.Proof.Framework.Registration

/-! # The C entry points on AArch64

`c_malloc`, `c_calloc`, `c_free` and `c_free_sized` out of line (`aarch64`), each a naked function
that tail-calls its slow path, and each also exported as the C symbols its `Api` names
(`Api.exports`). Two forms of each, as the other routines have (`Artifacts/Slab/AArch64.lean`). -/

@[expose] public section

namespace VG.Artifacts.CEntry.AArch64

open VG.Spec.CEntry VG.Impl.CEntry.AArch64 VG.Proof.CEntry.AArch64 VG.Proof.Slab.AArch64

def artifacts : List Artifact := [
  callocApi.artifact VG.AArch64.target {
    cfg := some ([.targetEnv "musl"], false)
    setup := callocSetup
    code := cCalloc
    attempt := Calloc.attempt
    verified := Calloc.verified
    restartsOk := Calloc.restartsOk
    setupOk := Calloc.setupOk setupArea
    witnesses := Calloc.witnesses
    witnessOk := Calloc.witnessOk },
  callocApi.artifact VG.AArch64.target {
    cfg := some ([.targetEnv "musl"], true)
    setup := callocMuslSetup
    code := cCalloc
    attempt := Calloc.attempt
    verified := Calloc.verified
    restartsOk := Calloc.restartsOk
    setupOk := Calloc.setupOk muslArea
    witnesses := Calloc.witnesses
    witnessOk := Calloc.witnessOk },
  mallocApi.artifact VG.AArch64.target {
    cfg := some ([.targetEnv "musl"], false)
    setup := mallocSetup
    code := cMalloc
    attempt := Malloc.attempt
    verified := Malloc.verified
    restartsOk := Malloc.restartsOk
    setupOk := Malloc.setupOk setupArea
    witnesses := Malloc.witnesses
    witnessOk := Malloc.witnessOk },
  mallocApi.artifact VG.AArch64.target {
    cfg := some ([.targetEnv "musl"], true)
    setup := mallocMuslSetup
    code := cMalloc
    attempt := Malloc.attempt
    verified := Malloc.verified
    restartsOk := Malloc.restartsOk
    setupOk := Malloc.setupOk muslArea
    witnesses := Malloc.witnesses
    witnessOk := Malloc.witnessOk },
  freeApi.artifact VG.AArch64.target {
    cfg := some ([.targetEnv "musl"], false)
    setup := freeSetup
    code := cFree
    attempt := Free.attempt
    verified := Free.verified
    restartsOk := Free.restartsOk
    setupOk := Free.setupOk setupArea
    witnesses := Free.witnesses
    witnessOk := Free.witnessOk },
  freeApi.artifact VG.AArch64.target {
    cfg := some ([.targetEnv "musl"], true)
    setup := freeMuslSetup
    code := cFree
    attempt := Free.attempt
    verified := Free.verified
    restartsOk := Free.restartsOk
    setupOk := Free.setupOk muslArea
    witnesses := Free.witnesses
    witnessOk := Free.witnessOk },
  freeSizedApi.artifact VG.AArch64.target {
    cfg := some ([.targetEnv "musl"], false)
    setup := freeSizedSetup
    code := cFreeSized
    attempt := FreeSized.attempt
    verified := FreeSized.verified
    restartsOk := FreeSized.restartsOk
    setupOk := FreeSized.setupOk setupArea
    witnesses := FreeSized.witnesses
    witnessOk := FreeSized.witnessOk },
  freeSizedApi.artifact VG.AArch64.target {
    cfg := some ([.targetEnv "musl"], true)
    setup := freeSizedMuslSetup
    code := cFreeSized
    attempt := FreeSized.attempt
    verified := FreeSized.verified
    restartsOk := FreeSized.restartsOk
    setupOk := FreeSized.setupOk muslArea
    witnesses := FreeSized.witnesses
    witnessOk := FreeSized.witnessOk }]

end VG.Artifacts.CEntry.AArch64
