module

public import Cralloc.Proof.Selftest.AArch64
public import Cralloc.Proof.Framework.Registration

/-! # The pipeline self-test on AArch64

Out of line and inline, the two ways a routine is emitted there. Built from the
`Api` directly rather than with `Api.artifact`, whose documentation always has a
`# Safety` section: this function has nothing to put in one. -/

@[expose] public section

namespace VG.Artifacts.Selftest.AArch64

def artifacts : List Artifact := [
  { Spec.Selftest.addApi with
    target := VG.AArch64.target
    doc := Spec.Selftest.addApi.summary
    code := Impl.Selftest.AArch64.add
    contract := Spec.Selftest.addContract VG.AArch64.abi
    verified := Proof.Selftest.AArch64.add_verified
    witnesses := [Proof.Selftest.AArch64.sat]
    witnessOk := Proof.Selftest.AArch64.add_witness },
  { Spec.Selftest.addApi with
    target := VG.AArch64.inlineTarget
    doc := Spec.Selftest.addApi.summary
    code := Impl.Selftest.AArch64.addInline
    contract := Spec.Selftest.addContract VG.AArch64.inlineAbi
    verified := Proof.Selftest.AArch64.addInline_verified
    clobbersOk := Proof.Selftest.AArch64.addInline_clobbersOk
    witnesses := [Proof.Selftest.AArch64.sat]
    witnessOk := Proof.Selftest.AArch64.addInline_witness }]

end VG.Artifacts.Selftest.AArch64
