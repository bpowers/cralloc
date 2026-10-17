module

/-!
# Pipeline self-test

**Trusted** (as every file in `Spec/`). Not a cryptographic algorithm: a
deliberately trivial function that exercises the whole pipeline (spec →
implementation → proof → `Artifacts.lean` → Rust) so that the pipeline is
tested before any real primitive exists.
-/

@[expose] public section

namespace VG.Spec.Selftest

/-- Wrapping 64-bit addition. -/
def add (a b : BitVec 64) : BitVec 64 := a + b

end VG.Spec.Selftest
