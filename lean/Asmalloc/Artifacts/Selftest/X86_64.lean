module

public import Asmalloc.Proof.Selftest.X86_64
public import Asmalloc.Proof.Framework.Semantics

/-! # The pipeline self-test on x86-64 -/

@[expose] public section

namespace VG.Artifacts.Selftest.X86_64

def artifacts : List Artifact := [
  { Spec.Selftest.addApi with
    target := X86_64.target
    doc := Spec.Selftest.addApi.summary
    code := Impl.Selftest.X86_64.add
    contract := Spec.Selftest.addContract X86_64.abi
    verified := Proof.Selftest.X86_64.add_verified }]

end VG.Artifacts.Selftest.X86_64
