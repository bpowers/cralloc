module

/-!
# Pipeline self-test

**Trusted** (as every file in `Spec/`). A deliberately trivial function
that exercises the whole pipeline (spec → implementation → proof →
registration file → Rust), and a worked example of each step.
-/

@[expose] public section

namespace VG.Spec.Selftest

/-- Wrapping 64-bit addition. -/
def add (a b : BitVec 64) : BitVec 64 := a + b

end VG.Spec.Selftest
