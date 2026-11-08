module

public import Cralloc.Proof.Slab.X86_64.Facts
public import Cralloc.Proof.Slab.Witness

/-!
# Concrete states for the routines' witnesses, on x86-64

The checks of `Proof/Slab/Witness.lean`, through x86-64's calling conventions.
-/

@[expose] public section

namespace VG.Proof.Slab.X86_64

open VG.X86_64 VG.Spec.Slab

export VG.Proof.Slab (mkMem mkMem_apply_none mkMem_readW_above mkMem_readW_clear mkMem_slab_zero
  res_of_sep nowrap_of_check)

/-- The regions of a concrete state are the routine's, by evaluation. -/
theorem world_of_check {c : Spec.Slab.Ctx isa} (hA : X86 c.A) {s : State}
    (hrd : s.rd = Ctx.ro c s) (hwr : s.wr = Ctx.rw c s)
    (hnw : ((c.regions s).all fun x => decide (x.1.base.toNat + x.1.len ≤ 2 ^ 64)) = true)
    (hp : Sig.pairsSep (c.regions s) = true)
    (hres : ((c.regions s).all fun x => (c.A.reserved 0 s).all fun q => Region.sep x.1 q) = true) :
    Ctx.World c s :=
  Slab.world_of_check hA.ptrBits (by rw [hA.rd]; exact hrd) (by rw [hA.wr]; exact hwr) hnw hp hres

theorem layout_of_check {c : Spec.Slab.Ctx isa} (hA : X86 c.A) {s : State}
    (h : Slab.layoutCheck c s = true) : c.Layout s :=
  Slab.layout_of_check hA.ptrBits h

theorem apart_of_check {c : Spec.Slab.Ctx isa} (hA : X86 c.A) {s : State} {r : Region}
    (h : Slab.apartCheck c s r = true) : c.Apart s r :=
  Slab.apart_of_check hA.ptrBits h

end VG.Proof.Slab.X86_64
