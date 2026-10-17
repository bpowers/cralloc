module

public import Asmalloc.Proof.Slab.X86_64.Facts
public import Asmalloc.Proof.Framework.Contract

/-!
# Concrete states for the routines' witnesses

A memory given by a few words at given addresses, zero elsewhere, and the
checks, by evaluation, that a concrete state's regions are a routine's.
-/

@[expose] public section

namespace VG.Proof.Slab.X86_64

open VG.X86_64 VG.Spec.Slab

/-- Memory holding the word `v` at `b` for each `(b, v)` of `ws` (the first
that covers an address), and zero elsewhere. -/
def mkMem (ws : List (Addr × BitVec 64)) : Mem := fun a =>
  match ws.find? (fun bv => decide ((a - bv.1).toNat < 8)) with
  | some (b, v) => v.extractLsb' (8 * (a - b).toNat) 8
  | none => 0

theorem mkMem_apply_none {ws : List (Addr × BitVec 64)} {a : Addr}
    (h : ∀ bv ∈ ws, ¬ (a - bv.1).toNat < 8) : mkMem ws a = 0 := by
  unfold mkMem
  rw [List.find?_eq_none.mpr (by simpa using h)]

/-- A word above every word of `mkMem ws` reads as zero. -/
theorem mkMem_readW_above {ws : List (Addr × BitVec 64)} {a : Addr}
    (h : ∀ bv ∈ ws, bv.1.toNat + 8 ≤ a.toNat) (ha : a.toNat + 8 ≤ 2 ^ 64) :
    (mkMem ws).readW a 64 = 0 := by
  unfold Mem.readW
  rw [Mem.read_eq_of_bytes (v := 0) fun i hi => ?_]
  · rfl
  · rw [mkMem_apply_none fun bv hbv => ?_]
    · simp
    · have := h bv hbv
      have hb := bv.1.isLt
      rw [BitVec.toNat_sub, BitVec.toNat_add, BitVec.toNat_ofNat, Nat.mod_eq_of_lt (a := i) (by omega),
        Nat.mod_eq_of_lt (a := a.toNat + i) (by omega)]
      omega

/-- Slab `i`'s word of class 1, for `i ≥ 1`, in a memory of words at or below
slab 0's of `SLABS` at `0x1000000`, is zero. -/
theorem mkMem_slab_zero {ws : List (Addr × BitVec 64)} (hws : ∀ bv ∈ ws, bv.1.toNat ≤ 0x1000008)
    {i : Nat} (h0 : 0 < i) (hi : i < SLAB_COUNT) :
    (mkMem ws).readW (wordAddr 0x1000000 i 1) 64 = 0 := by
  simp only [SLAB_COUNT] at hi
  have ha : (wordAddr 0x1000000 i 1).toNat = 0x1000000 + i * 4096 + 8 := by
    simp only [wordAddr, SLAB_BYTES, SLAB_SHIFT]
    rw [BitVec.toNat_add, show (0x1000000 : BitVec 64).toNat = 0x1000000 from rfl, BitVec.toNat_ofNat,
      Nat.mod_eq_of_lt (by omega), Nat.mod_eq_of_lt (by omega)]
    omega
  apply mkMem_readW_above
  · intro bv hbv; have := hws bv hbv; rw [ha]; omega
  · rw [ha]; omega

theorem stackOk_zero {m : Mem} {rs : List Region} : StackOk m 0 rs :=
  .inr ⟨[], .nil, by simp⟩

instance (n : Nat) (w : BitVec 64) : Decidable (WordOk n w) := by unfold WordOk; infer_instance

theorem res_of_sep {l : List (Region × Bool)} {r : Region}
    (h : (l.all fun x => Region.sep x.1 r) = true) : ∀ x ∈ l, x.1.Disjoint r := by
  simp only [List.all_eq_true] at h
  exact fun x hx => Region.disjoint_of_sep (h x hx)

theorem nowrap_of_check {l : List (Region × Bool)}
    (h : (l.all fun x => decide (x.1.base.toNat + x.1.len ≤ 2 ^ 64)) = true) :
    ∀ x ∈ l, x.1.base.toNat + x.1.len ≤ 2 ^ 64 := by
  simp only [List.all_eq_true, decide_eq_true_eq] at h
  exact h

/-- The regions of a concrete state are the routine's, by evaluation. -/
theorem world_of_check {c : Spec.Slab.Ctx isa} (hA : c.A = abi) {s : State}
    (hrd : s.rd = Ctx.ro c s) (hwr : s.wr = Ctx.rw c s)
    (hnw : ((c.regions s).all fun x => decide (x.1.base.toNat + x.1.len ≤ 2 ^ 64)) = true)
    (hp : Sig.pairsSep (c.regions s) = true)
    (hres : ((c.regions s).all fun x => Region.sep x.1 ⟨s.gpr .rsp, 8⟩) = true) :
    Ctx.World c s where
  rd := by rw [hA]; exact hrd
  wr := by rw [hA]; exact hwr
  nowrap := by rw [hA]; exact nowrap_of_check hnw
  disj := Sig.pairwise_of_pairsSep hp
  res := by
    rw [hA]
    intro x hx r hr
    simp only [abi_reserved, List.mem_singleton] at hr
    subst hr
    exact res_of_sep hres x hx

end VG.Proof.Slab.X86_64
