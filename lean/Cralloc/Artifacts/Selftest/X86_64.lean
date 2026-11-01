module

public import Cralloc.Proof.Selftest.X86_64
public import Cralloc.Proof.Framework.Registration

/-! # The pipeline self-test on x86-64

Built from the `Api` directly rather than with `Api.artifact`, whose
documentation always has a `# Safety` section: this function has nothing to
put in one. -/

@[expose] public section

namespace VG.Artifacts.Selftest.X86_64

def artifacts : List Artifact := [
  { Spec.Selftest.addApi with
    target := X86_64.target
    doc := Spec.Selftest.addApi.summary
    code := Impl.Selftest.X86_64.add
    contract := Spec.Selftest.addContract X86_64.abi
    verified := Proof.Selftest.X86_64.add_verified
    witnesses := [Proof.Selftest.X86_64.sat]
    witnessOk := Proof.Selftest.X86_64.add_witness }]

end VG.Artifacts.Selftest.X86_64
