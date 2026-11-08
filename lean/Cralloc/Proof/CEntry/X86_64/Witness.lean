module

public import Cralloc.Proof.CEntry.X86_64.Facts
public import Cralloc.Proof.CEntry.Witness

/-!
# Checking a C routine's precondition on a concrete state, on x86-64

`Proof/CEntry/Witness.lean`, with the permissions and the pointer width of a state of this
target.
-/

@[expose] public section

namespace VG.Proof.CEntry.X86_64

open VG.X86_64 VG.Spec.Slab VG.Spec.CEntry VG.Proof.Slab.X86_64 VG.Proof.CEntry

theorem layout_of_check {c : Spec.CEntry.Ctx isa} (hA : X86 c.A) {s : State}
    (h : layoutCheck c s = true) : c.Layout s :=
  Proof.CEntry.layout_of_check hA.ptrBits h

theorem apart_of_check {c : Spec.CEntry.Ctx isa} (hA : X86 c.A) {s : State} {r : Region}
    (h : apartCheck c s r = true) : c.Apart s r :=
  Proof.CEntry.apart_of_check hA.ptrBits h

theorem world_of_check {c : Spec.CEntry.Ctx isa} (hA : X86 c.A) {s : State}
    (hrd : s.rd = c.ro s) (hwr : s.wr = c.rw s)
    (hnw : ((c.regions s).all fun x => decide (x.1.base.toNat + x.1.len ≤ 2 ^ 64)) = true)
    (hp : Sig.pairsSep (c.regions s) = true)
    (hres : ((c.regions s).all fun x => (c.A.reserved 0 s).all fun q => Region.sep x.1 q) = true) :
    c.World s :=
  Proof.CEntry.world_of_check hA.ptrBits (by rw [hA.rd]; exact hrd) (by rw [hA.wr]; exact hwr) hnw hp hres

end VG.Proof.CEntry.X86_64
