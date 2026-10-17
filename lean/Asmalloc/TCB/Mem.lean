module

/-!
# Byte-addressed memory and memory regions

**Trusted.** Memory is a total function from 64-bit addresses to bytes.
Multi-byte accesses are little-endian and addresses wrap modulo 2⁶⁴.

Memory *safety* is not a property of `Mem` itself: each ISA's machine state
also carries the regions the code is permitted to read and write, and every
load/store in the instruction semantics faults if it touches a byte outside of
them. Proving that a program runs to completion therefore proves that it only
touches permitted memory.

This file contains definitions only; lemmas about them live in
`Asmalloc/Proof/Framework/`.
-/

@[expose] public section

namespace VG

abbrev Addr := BitVec 64
abbrev Byte := BitVec 8
abbrev Mem := Addr → Byte

namespace Mem

/-- Little-endian read of `n` bytes starting at `a`. -/
def read (m : Mem) (a : Addr) : (n : Nat) → BitVec (8 * n)
  | 0 => 0#0
  | n + 1 => (read m (a + 1) n ++ m a : BitVec (8 * n + 8))

/-- Little-endian read of a `w`-bit value (`w` a multiple of 8). -/
def readW (m : Mem) (a : Addr) (w : Nat) : BitVec w := (m.read a (w / 8)).setWidth w

/-- Little-endian write of the `n`-byte value `v` starting at `a`. -/
def write (m : Mem) (a : Addr) (n : Nat) (v : BitVec (8 * n)) : Mem :=
  fun x => if (x - a).toNat < n then v.extractLsb' (8 * (x - a).toNat) 8 else m x

/-- Little-endian write of a `w`-bit value (`w` a multiple of 8). -/
def writeW (m : Mem) (a : Addr) {w : Nat} (v : BitVec w) : Mem :=
  m.write a (w / 8) (v.setWidth (8 * (w / 8)))

end Mem

/-- A contiguous range of memory `[base, base + len)` (modulo 2⁶⁴). -/
structure Region where
  base : Addr
  len : Nat
  deriving DecidableEq, Repr

/-- The access `[a, a + n)` lies within region `r`. -/
def Region.Contains (r : Region) (a : Addr) (n : Nat) : Prop :=
  (a - r.base).toNat + n ≤ r.len

instance (r : Region) (a : Addr) (n : Nat) : Decidable (r.Contains a n) := by
  unfold Region.Contains; infer_instance

/-- No byte lies in both regions. -/
def Region.Disjoint (r₁ r₂ : Region) : Prop := ∀ a, r₁.Contains a 1 → ¬ r₂.Contains a 1

/-- The access `[a, a + n)` lies entirely within one of the regions. -/
def InRegions (rs : List Region) (a : Addr) (n : Nat) : Prop := ∃ r ∈ rs, r.Contains a n

instance (rs : List Region) (a : Addr) (n : Nat) : Decidable (InRegions rs a n) := by
  unfold InRegions; infer_instance

end VG
