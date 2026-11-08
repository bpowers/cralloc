module

public import Cralloc.Proof.Slab.Mag

/-!
# Words in memory, on any target

What a routine's proof needs of the words it reads and writes: a word's value after a
write to a region apart from it, the regions a few writes change, and that a magazine
is the same in memory changed only in regions apart from it.
-/

@[expose] public section

namespace VG.Proof.Slab

open VG.Spec.Slab

theorem toNat_read8 (m : Mem) (a : Addr) : (m.read a 8).toNat = (m.readW a 64).toNat := by
  simp only [Mem.readW, BitVec.toNat_setWidth]
  exact (Nat.mod_eq_of_lt (m.read a 8).isLt).symm

theorem toNat_read4 (m : Mem) (a : Addr) : (m.read a 4).toNat = (m.readW a 32).toNat := by
  simp only [Mem.readW, BitVec.toNat_setWidth]
  exact (Nat.mod_eq_of_lt (m.read a 4).isLt).symm

/-- A word read after a write to a region it is apart from. -/
theorem readW_writeW_disj {m : Mem} {a b : Addr} {w : Nat} {v : BitVec 64}
    (h : Region.Disjoint ⟨a, w / 8⟩ ⟨b, 8⟩) (hw : w / 8 < 2 ^ 64) :
    (m.writeW b v).readW a w = m.readW a w :=
  Mem.readW_writeW_sep (h.sep (Region.contains_self _ _) (Region.contains_self _ _)) hw

theorem readW64_writeW_disj {m : Mem} {a b : Addr} {v : BitVec 64}
    (h : Region.Disjoint ⟨a, 8⟩ ⟨b, 8⟩) : (m.writeW b v).readW a 64 = m.readW a 64 :=
  readW_writeW_disj h (by decide)

theorem readW32_writeW_disj {m : Mem} {a b : Addr} {v : BitVec 64}
    (h : Region.Disjoint ⟨a, 4⟩ ⟨b, 8⟩) : (m.writeW b v).readW a 32 = m.readW a 32 :=
  readW_writeW_disj h (by decide)

/-- The bytes of an access within a region are in it. -/
theorem sub_of_contains {r : Region} {a : Addr} {n : Nat} (h : r.Contains a n) :
    Region.Sub ⟨a, n⟩ r := fun x hx => h.byte (by simp only [Region.Contains] at hx; omega)

theorem read_writeW_self64 (m : Mem) (a : Addr) (v : BitVec 64) :
    ((m.writeW a v).read a 8).toNat = v.toNat := by
  rw [toNat_read8, Mem.readW_writeW_self64]

/-- The 8-byte region at `a` holds `a`'s word. -/
theorem contains8 (a : Addr) : (⟨a, 8⟩ : Region).Contains a 8 := Region.contains_self a 8

/-- Two words written, in two regions. -/
theorem frame_two (m : Mem) (a b : Addr) (v w : BitVec 64) :
    Frame [⟨a, 8⟩, ⟨b, 8⟩] m ((m.writeW a v).writeW b w) :=
  ((Frame.refl _ _).writeW (r := ⟨a, 8⟩) (List.mem_cons_self ..) _ (Region.contains_self _ _)).writeW
    (r := ⟨b, 8⟩) (List.mem_cons_of_mem _ (List.mem_singleton_self _)) _ (Region.contains_self _ _)

/-- Three words written, in three regions. -/
theorem frame_three (m : Mem) (a b c : Addr) (u v w : BitVec 64) :
    Frame [⟨a, 8⟩, ⟨b, 8⟩, ⟨c, 8⟩] m (((m.writeW a u).writeW b v).writeW c w) :=
  (((Frame.refl _ _).writeW (r := ⟨a, 8⟩) (List.mem_cons_self ..) _ (Region.contains_self _ _)).writeW
    (r := ⟨b, 8⟩) (List.mem_cons_of_mem _ (List.mem_cons_self ..)) _ (Region.contains_self _ _)).writeW
    (r := ⟨c, 8⟩) (List.mem_cons_of_mem _ (List.mem_cons_of_mem _ (List.mem_singleton_self _))) _
    (Region.contains_self _ _)

/-- The magazine a word the invariant allows names is the same in memory
changed only in regions apart from the magazine. -/
theorem ofWord_frame {n : Nat} {m m' : Mem} {w : BitVec 64} {W : List Region} (hn : 0 < n)
    (hfr : Frame W m m') (hw : WordOk n w)
    (hd : IsSlot n w → ∀ r ∈ W, Region.Disjoint ⟨magBase n w, 8 * n⟩ r) :
    Mag.ofWord n m' w = Mag.ofWord n m w := by
  rcases hw with h0 | hm | hs
  · rw [ofWord_zero h0, ofWord_zero h0]
  · rw [ofWord_marker hm, ofWord_marker hm]
  · refine ofWord_congr fun i hi => hfr.readW (r := ⟨magBase n w + BitVec.ofNat 64 (8 * (i + 1)), 8⟩)
      (Region.contains_self _ _) (fun r hr => (hd hs r hr).sub_left (sub_item ?_)) (by decide)
    have := slot_lt n w hn
    omega

end VG.Proof.Slab
