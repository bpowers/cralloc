module

public import Asmalloc.Spec.Selftest
public import Asmalloc.TCB.Artifact

/-!
# Pipeline self-test: the contract, on every target

**Trusted** (as every file in `Spec/`). `A` is the target's calling
convention.
-/

@[expose] public section

namespace VG.Spec.Selftest

/-- `vg_selftest_add(a: u64, b: u64) -> u64`. Both arguments are secret. -/
def addSig : Sig where
  params := [("a", .int .u64 false), ("b", .int .u64 false)]
  ret := some .u64

/-- Returns `add a b` and does not modify memory. -/
def addContract {M : ISA} (A : Abi M) : Contract M :=
  addSig.contract A (post := fun a b m m' r => r = add a b ∧ m' = m)

/-- `vg_selftest_add` on every target. It has no `# Safety` section: its
documentation is its `summary` alone. -/
def addApi : Api where
  module := "selftest"
  name := "vg_selftest_add"
  sig := addSig
  contracts := some fun A _ => addContract A
  summary := "Pipeline self-test: returns `a.wrapping_add(b)`.\n\n\
    Contract: `VG.Spec.Selftest.addContract`. No safety requirements."
  safety := []

end VG.Spec.Selftest
