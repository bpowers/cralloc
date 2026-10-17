module

public import Asmalloc.Spec.Slab
public import Asmalloc.Proof.Framework.Mem
public import Asmalloc.Proof.Framework.Offset

/-!
# The mask tests and the abstract magazine

What the routines' two tests decide, for a slab word that satisfies the
invariant (`WordOk`): `word & mask` is zero exactly when there is nothing to
pop, and `(word + 8) & mask` exactly when there is no slot to push to; and
what the magazine is after the commit.
-/

@[expose] public section

namespace VG.Proof.Slab

open VG.Spec.Slab

/-! ## Bits -/

theorem eq_zero_iff_testBit (y : Nat) : y = 0 ↔ ∀ i, y.testBit i = false := by
  constructor
  · rintro rfl i; exact Nat.zero_testBit i
  · intro h; exact Nat.eq_of_testBit_eq fun i => by rw [h i, Nat.zero_testBit]

theorem lt_two_pow_iff_testBit (y n : Nat) : y < 2 ^ n ↔ ∀ i, n ≤ i → y.testBit i = false := by
  constructor
  · intro h i hi
    exact Nat.testBit_lt_two_pow (Nat.lt_of_lt_of_le h (Nat.pow_le_pow_right (by decide) hi))
  · intro h; exact Nat.lt_pow_two_of_testBit y h

theorem magMask_eq (k : Nat) (hk : 1 ≤ k) : magMask (2 ^ k) = (2 ^ (k - 1) - 1) <<< 4 := by
  have h : 2 ^ k = 2 * 2 ^ (k - 1) := by
    rw [← Nat.pow_succ']; congr 1; omega
  have := Nat.two_pow_pos (k - 1)
  rw [magMask, Nat.shiftLeft_eq, h]
  omega

theorem testBit_magMask (k i : Nat) (hk : 1 ≤ k) :
    (magMask (2 ^ k)).testBit i = decide (4 ≤ i ∧ i < k + 3) := by
  rw [magMask_eq k hk, Nat.testBit_shiftLeft, Nat.testBit_two_pow_sub_one]
  by_cases h : 4 ≤ i
  · simp only [ge_iff_le, h, decide_true, Bool.true_and, true_and]
    exact decide_eq_decide.mpr (by omega)
  · simp [h]

theorem eight_mul_two_pow (k : Nat) : 8 * 2 ^ k = 2 ^ (k + 3) := by
  rw [Nat.pow_add]; omega

/-- The mask test on the low 32 bits of `x`: zero exactly when `x` is less
than 16 past a multiple of the magazine's size. -/
theorem mask_test (k x : Nat) (hk : 2 ≤ k) (hk' : k ≤ 29) :
    (x % 2 ^ 32) &&& magMask (2 ^ k) = 0 ↔ x % (8 * 2 ^ k) < 16 := by
  rw [eight_mul_two_pow, show (16 : Nat) = 2 ^ 4 from rfl, eq_zero_iff_testBit,
    lt_two_pow_iff_testBit]
  simp only [Nat.testBit_and, Nat.testBit_mod_two_pow, testBit_magMask k _ (by omega)]
  constructor
  · intro h i hi
    have := h i
    by_cases hi' : i < k + 3
    · simp only [show i < 32 by omega, decide_true, Bool.true_and, show 4 ≤ i from hi, hi',
        and_self, Bool.and_true] at this
      simp [hi', this]
    · simp [hi']
  · intro h i
    by_cases hi : 4 ≤ i ∧ i < k + 3
    · have := h i hi.1
      simp only [hi.2, decide_true, Bool.true_and] at this
      simp [this]
    · simp [hi]

/-- The mask of an allowed size is below `2 ^ 32`. -/
theorem magMask_lt (k : Nat) (hk' : k ≤ 16) : magMask (2 ^ k) < 2 ^ 32 := by
  have : 2 ^ k ≤ 2 ^ 16 := Nat.pow_le_pow_right (by decide) hk'
  simp only [magMask]
  omega

/-- The mask test as the routines make it, on 32-bit values. -/
theorem mask_test_bv (k x : Nat) (hk : 2 ≤ k) (hk' : k ≤ 16) :
    BitVec.ofNat 32 x &&& BitVec.ofNat 32 (magMask (2 ^ k)) = 0 ↔ x % (8 * 2 ^ k) < 16 := by
  rw [← mask_test k x hk (by omega), ← BitVec.toNat_inj, BitVec.toNat_and, BitVec.toNat_ofNat,
    BitVec.toNat_ofNat, Nat.mod_eq_of_lt (magMask_lt k hk')]
  simp

/-! ## Slots -/

/-- An allowed mask's magazine size. -/
theorem slotsOf_magMask (k : Nat) (hk : 2 ≤ k) : slotsOf (magMask (2 ^ k)) = 2 ^ k := by
  have : 4 ≤ 2 ^ k := by
    calc 4 = 2 ^ 2 := rfl
      _ ≤ 2 ^ k := Nat.pow_le_pow_right (by decide) hk
  simp only [slotsOf, magMask]
  omega

/-- The magazine's bytes divide `2 ^ 64`. -/
theorem dvd_two_pow_64 (k : Nat) (hk' : k ≤ 16) : 8 * 2 ^ k ∣ 2 ^ 64 := by
  rw [eight_mul_two_pow]; exact Nat.pow_dvd_pow 2 (by omega)

theorem slot_lt_of {n : Nat} {w : BitVec 64} (h : IsSlot n w) : 1 ≤ slot n w := h.2

theorem slot_lt (n : Nat) (w : BitVec 64) (hn : 0 < n) : slot n w < n := by
  simp only [slot]
  have := Nat.mod_lt w.toNat (show 0 < 8 * n by omega)
  omega

/-- A slot address's offset in its magazine. -/
theorem mod_eq_of_isSlot {n : Nat} {w : BitVec 64} (h : IsSlot n w) :
    w.toNat % (8 * n) = 8 * slot n w := by
  obtain ⟨h8, -⟩ := h
  simp only [slot]
  have : w.toNat % (8 * n) % 8 = 0 := by
    rw [Nat.mod_mod_of_dvd _ (Nat.dvd_mul_right 8 n)]; exact h8
  omega

/-- The magazine's base, as a number. -/
theorem magBase_toNat {n : Nat} {w : BitVec 64} (h : IsSlot n w) :
    (magBase n w).toNat = w.toNat - 8 * slot n w := by
  have hm := mod_eq_of_isSlot h
  have hle : 8 * slot n w ≤ w.toNat := by
    rw [← hm]; exact Nat.mod_le _ _
  have hlt : 8 * slot n w < 2 ^ 64 := Nat.lt_of_le_of_lt hle w.isLt
  rw [magBase, BitVec.toNat_sub, BitVec.toNat_ofNat, Nat.mod_eq_of_lt hlt]
  omega

/-- A slot address is at least 8. -/
theorem le_toNat_of_isSlot {n : Nat} {w : BitVec 64} (h : IsSlot n w) : 8 * slot n w ≤ w.toNat := by
  rw [← mod_eq_of_isSlot h]; exact Nat.mod_le _ _

theorem ne_zero_of_isSlot {n : Nat} {w : BitVec 64} (h : IsSlot n w) : w.toNat ≠ 0 := by
  have := le_toNat_of_isSlot h; have := h.2; omega

/-- A slot address has its lowest bit clear: it is not a marker. -/
theorem even_of_isSlot {n : Nat} {w : BitVec 64} (h : IsSlot n w) : ¬ w.toNat % 2 = 1 := by
  have := h.1; omega

/-- A marker's offset in a magazine of an allowed size is 1: its slot-number
bits are clear. -/
theorem marker_mod {n : Nat} {w : BitVec 64} (h : IsMarker w) (hd : 8 * n ∣ 2 ^ MARKER_SHIFT) :
    w.toNat % (8 * n) = 1 := by
  have := Nat.mod_mod_of_dvd w.toNat hd
  rw [IsMarker] at h
  rw [← this, h]
  have : 2 ≤ 8 * n := by
    rcases Nat.eq_zero_or_pos n with rfl | hn
    · simp [MARKER_SHIFT, MAG_LOG_MAX] at hd
    · omega
  exact Nat.mod_eq_of_lt this

/-- A magazine of an allowed size divides a marker's low bits. -/
theorem dvd_marker {k : Nat} (hk' : k ≤ 16) : 8 * 2 ^ k ∣ 2 ^ MARKER_SHIFT := by
  rw [eight_mul_two_pow]; exact Nat.pow_dvd_pow 2 (by simp [MARKER_SHIFT, MAG_LOG_MAX]; omega)

theorem marker_lt {w : BitVec 64} (h : IsMarker w) : w.toNat + 2 ^ MARKER_SHIFT ≤ 2 ^ 64 + 1 := by
  rw [IsMarker] at h
  have hw := w.isLt
  have := Nat.div_add_mod w.toNat (2 ^ MARKER_SHIFT)
  have hq : w.toNat / 2 ^ MARKER_SHIFT < 2 ^ (64 - MARKER_SHIFT) := by
    rw [Nat.div_lt_iff_lt_mul (Nat.two_pow_pos _), ← Nat.pow_add]
    simp only [MARKER_SHIFT, MAG_LOG_MAX] at *; exact hw
  have : 2 ^ MARKER_SHIFT * (w.toNat / 2 ^ MARKER_SHIFT + 1) ≤ 2 ^ 64 := by
    have := Nat.mul_le_mul_left (2 ^ MARKER_SHIFT) (show w.toNat / 2 ^ MARKER_SHIFT + 1 ≤ 2 ^ (64 - MARKER_SHIFT) by omega)
    rw [← Nat.pow_add] at this
    simpa [MARKER_SHIFT, MAG_LOG_MAX] using this
  rw [Nat.mul_add, Nat.mul_one] at this
  omega

/-- The base plus `8 j`, as an offset from the word. -/
theorem magBase_add {n : Nat} {w : BitVec 64} (j : Nat) :
    magBase n w + BitVec.ofNat 64 (8 * j) = w - BitVec.ofNat 64 (8 * slot n w) + BitVec.ofNat 64 (8 * j) :=
  rfl

/-! ## The word one slot down or up -/

theorem toNat_eight : (8 : BitVec 64).toNat = 8 := rfl

theorem four_le {k : Nat} (hk : 2 ≤ k) : 4 ≤ 2 ^ k :=
  Nat.le_trans (by decide : 4 ≤ 2 ^ 2) (Nat.pow_le_pow_right (by decide) hk)

theorem add8_toNat_mod (w : BitVec 64) : (w + 8).toNat = (w.toNat + 8) % 2 ^ 64 := by
  rw [BitVec.toNat_add, toNat_eight]

theorem sub8_toNat {w : BitVec 64} (h : 8 ≤ w.toNat) : (w - 8).toNat = w.toNat - 8 := by
  rw [BitVec.toNat_sub, toNat_eight]
  have := w.isLt
  omega

theorem slot_sub8 {n : Nat} {w : BitVec 64} (hn : 0 < n) (h : IsSlot n w) (h2 : 2 ≤ slot n w) :
    slot n (w - 8) = slot n w - 1 ∧ IsSlot n (w - 8) ∧ magBase n (w - 8) = magBase n w := by
  have hm := mod_eq_of_isSlot h
  have hle := le_toNat_of_isSlot h
  have ht : (w - 8).toNat = w.toNat - 8 := sub8_toNat (by omega)
  have hs : slot n (w - 8) = slot n w - 1 := by
    simp only [slot, ht]
    have : (w.toNat - 8) % (8 * n) = w.toNat % (8 * n) - 8 := by
      have := Nat.div_add_mod w.toNat (8 * n)
      have : (w.toNat - 8) = 8 * n * (w.toNat / (8 * n)) + (w.toNat % (8 * n) - 8) := by omega
      rw [this, Nat.mul_add_mod]
      exact Nat.mod_eq_of_lt (by have := Nat.mod_lt w.toNat (show 0 < 8 * n by omega); omega)
    rw [this, hm]; simp only [slot] at *; omega
  refine ⟨hs, ⟨?_, by omega⟩, ?_⟩
  · rw [ht]; have := h.1; omega
  · apply BitVec.eq_of_toNat_eq
    have h' : IsSlot n (w - 8) := ⟨by rw [ht]; have := h.1; omega, by omega⟩
    rw [magBase_toNat h, magBase_toNat h', hs, ht]
    omega

theorem add8_toNat {n : Nat} {w : BitVec 64} (k : Nat) (hk' : k ≤ 16) (hn : n = 2 ^ k)
    (h : IsSlot n w) (hs : slot n w + 1 < n) : (w + 8).toNat = w.toNat + 8 := by
  have hm := mod_eq_of_isSlot h
  have hd := dvd_two_pow_64 k hk'
  rw [← hn] at hd
  obtain ⟨q, hq⟩ := hd
  have := Nat.div_add_mod w.toNat (8 * n)
  have hw := w.isLt
  -- `w` is in a block of `8 n` bytes that ends at or before `2 ^ 64`.
  have hb : 8 * n * (w.toNat / (8 * n)) + 8 * n ≤ 2 ^ 64 := by
    have h1 : w.toNat / (8 * n) < q := by
      rw [Nat.div_lt_iff_lt_mul (by omega)]
      rw [hq] at hw; rw [Nat.mul_comm]; exact hw
    rw [hq]
    have := Nat.mul_le_mul_left (8 * n) (show w.toNat / (8 * n) + 1 ≤ q by omega)
    rw [Nat.mul_add, Nat.mul_one] at this
    exact this
  rw [add8_toNat_mod]
  apply Nat.mod_eq_of_lt
  rw [hm] at this
  omega

theorem slot_add8 {n : Nat} {w : BitVec 64} (k : Nat) (hk' : k ≤ 16) (hn : n = 2 ^ k)
    (h : IsSlot n w) (hs : slot n w + 1 < n) :
    slot n (w + 8) = slot n w + 1 ∧ IsSlot n (w + 8) ∧ magBase n (w + 8) = magBase n w := by
  have hm := mod_eq_of_isSlot h
  have ht := add8_toNat k hk' hn h hs
  have hss : slot n (w + 8) = slot n w + 1 := by
    simp only [slot, ht]
    have : (w.toNat + 8) % (8 * n) = w.toNat % (8 * n) + 8 := by
      have := Nat.div_add_mod w.toNat (8 * n)
      have : (w.toNat + 8) = 8 * n * (w.toNat / (8 * n)) + (w.toNat % (8 * n) + 8) := by omega
      rw [this, Nat.mul_add_mod]
      exact Nat.mod_eq_of_lt (by rw [hm]; omega)
    rw [this, hm]; simp only [slot] at *; omega
  have h' : IsSlot n (w + 8) := ⟨by rw [ht]; have := h.1; omega, by omega⟩
  refine ⟨hss, h', ?_⟩
  apply BitVec.eq_of_toNat_eq
  rw [magBase_toNat h, magBase_toNat h', hss, ht]
  omega

/-! ## Pop -/

/-- The cached pointers of a magazine. -/
def items (n : Nat) (m : Mem) (w : BitVec 64) : List (BitVec 64) :=
  (List.range (slot n w - 1)).map fun i => m.readW (magBase n w + BitVec.ofNat 64 (8 * (i + 1))) 64

theorem ofWord_slot {n : Nat} {m : Mem} {w : BitVec 64} (h : IsSlot n w) :
    Mag.ofWord n m w = .stack (magBase n w) (items n m w) := by
  simp [Mag.ofWord, ne_zero_of_isSlot h, even_of_isSlot h, items]

/-- The slot below the word is its top pointer's. -/
theorem magBase_add_top {n : Nat} {w : BitVec 64} (h : IsSlot n w) (h1 : 1 ≤ slot n w) :
    magBase n w + BitVec.ofNat 64 (8 * (slot n w - 1)) = w - 8 := by
  apply BitVec.eq_of_toNat_eq
  have hle := le_toNat_of_isSlot h
  rw [BitVec.toNat_add, magBase_toNat h, sub8_toNat (by omega), BitVec.toNat_ofNat,
    Nat.mod_eq_of_lt (by have := w.isLt; omega)]
  have := w.isLt
  rw [Nat.mod_eq_of_lt (by omega)]
  omega

/-- What the pop test decides, for a word satisfying the invariant: there is
a pointer to pop exactly when the test is not zero. -/
theorem ofWord_zero {n : Nat} {m : Mem} {w : BitVec 64} (h : w.toNat = 0) :
    Mag.ofWord n m w = .absent := by
  simp [Mag.ofWord, h]

theorem ofWord_marker {n : Nat} {m : Mem} {w : BitVec 64} (h : IsMarker w) :
    Mag.ofWord n m w = .marker := by
  have h2 : w.toNat % 2 = 1 := by
    have := Nat.mod_mod_of_dvd w.toNat (Nat.pow_dvd_pow 2 (show 1 ≤ MARKER_SHIFT by decide))
    rw [IsMarker] at h; rw [h] at this; simpa using this.symm
  have h0 : w.toNat ≠ 0 := by intro h'; rw [h'] at h2; simp at h2
  simp [Mag.ofWord, h0, h2]

theorem pop_eq_none_iff {n : Nat} {m : Mem} {w : BitVec 64} (hd : 8 * n ∣ 2 ^ MARKER_SHIFT)
    (h : WordOk n w) : (Mag.ofWord n m w).pop = none ↔ w.toNat % (8 * n) < 16 := by
  rcases h with h | h | h
  · rw [ofWord_zero h, h]; simp [Mag.pop]
  · rw [ofWord_marker h, marker_mod h hd]
    simp only [Mag.pop, true_iff]; decide
  · rw [ofWord_slot h, mod_eq_of_isSlot h]
    simp only [Mag.pop, Option.map_eq_none_iff, List.getLast?_eq_none_iff, items,
      List.map_eq_nil_iff, List.range_eq_nil]
    have := h.2
    omega

/-- The top pointer is in the slot below the word. -/
theorem items_getLast? {n : Nat} {m : Mem} {w : BitVec 64} (h : IsSlot n w) (h2 : 2 ≤ slot n w) :
    (items n m w).getLast? = some (m.readW (w - 8) 64) := by
  simp only [items, List.getLast?_map, List.getLast?_range, show ¬ (slot n w - 1 = 0) by omega,
    ↓reduceIte, Option.map_some, Option.some.injEq]
  rw [show slot n w - 1 - 1 + 1 = slot n w - 1 by omega, magBase_add_top h (by omega)]

/-- The cached pointers but the top one. -/
theorem items_dropLast {n : Nat} {m : Mem} {w : BitVec 64} (h2 : 2 ≤ slot n w) :
    (items n m w).dropLast = (List.range (slot n w - 2)).map fun i =>
      m.readW (magBase n w + BitVec.ofNat 64 (8 * (i + 1))) 64 := by
  simp only [items]
  rw [show slot n w - 1 = (slot n w - 2) + 1 by omega, List.range_succ, List.map_append,
    List.map_cons, List.map_nil, List.dropLast_append_of_ne_nil (by simp), List.dropLast_singleton,
    List.append_nil]

/-- A pop that takes a pointer: the pointer in the slot below the word, and
the rest of the magazine. -/
theorem pop_of_test {n : Nat} {m : Mem} {w : BitVec 64} (hd : 8 * n ∣ 2 ^ MARKER_SHIFT)
    (h : WordOk n w) (ht : ¬ w.toNat % (8 * n) < 16) :
    IsSlot n w ∧ 2 ≤ slot n w ∧
      (Mag.ofWord n m w).pop = some (m.readW (w - 8) 64, .stack (magBase n w) (items n m w).dropLast) := by
  have h' : IsSlot n w := by
    rcases h with h | h | h
    · rw [h] at ht; exact absurd (by simp) ht
    · rw [marker_mod h hd] at ht; exact absurd (by decide) ht
    · exact h
  have h2 : 2 ≤ slot n w := by rw [mod_eq_of_isSlot h'] at ht; omega
  refine ⟨h', h2, ?_⟩
  rw [ofWord_slot h']
  simp only [Mag.pop, items_getLast? h' h2, Option.map_some]

/-- The magazine after a pop's commit, if memory is unchanged in the slots
below the new top. -/
theorem ofWord_sub8 {n : Nat} {m m' : Mem} {w : BitVec 64} (hn : 0 < n) (h : IsSlot n w)
    (h2 : 2 ≤ slot n w)
    (hm : ∀ i < slot n w - 2, m'.readW (magBase n w + BitVec.ofNat 64 (8 * (i + 1))) 64 =
      m.readW (magBase n w + BitVec.ofNat 64 (8 * (i + 1))) 64) :
    Mag.ofWord n m' (w - 8) = .stack (magBase n w) (items n m w).dropLast := by
  obtain ⟨hs, h', hb⟩ := slot_sub8 hn h h2
  rw [ofWord_slot h', hb, items_dropLast h2]
  congr 1
  simp only [items, hs, hb, show slot n w - 1 - 1 = slot n w - 2 by omega]
  exact List.map_congr_left fun i hi => hm i (List.mem_range.mp hi)

/-! ## Push -/

/-- What the push test decides, for a word satisfying the invariant: there is
a slot to push to exactly when the test of the word plus 8 is not zero. -/
theorem push_eq_none_iff {n : Nat} {m : Mem} {w : BitVec 64} (x : BitVec 64) (k : Nat)
    (hn : n = 2 ^ k) (hk : 2 ≤ k) (hk' : k ≤ 16) (h : WordOk n w) :
    (Mag.ofWord n m w).push n x = none ↔ (w + 8).toNat % (8 * n) < 16 := by
  have h4 : 4 ≤ n := by rw [hn]; exact four_le hk
  rcases h with h | h | h
  · rw [ofWord_zero h, add8_toNat_mod, h]
    simp only [Mag.push, true_iff]
    exact Nat.lt_of_le_of_lt (Nat.mod_le _ _) (by decide)
  · rw [ofWord_marker h, add8_toNat_mod]
    simp only [Mag.push, true_iff]
    have hd := dvd_marker hk'
    rw [← hn] at hd
    have hm := marker_mod h hd
    have := marker_lt h
    rw [Nat.mod_eq_of_lt (a := w.toNat + 8) (b := 2 ^ 64)
      (by simp only [MARKER_SHIFT, MAG_LOG_MAX] at this; omega)]
    have h64 := Nat.div_add_mod w.toNat (8 * n)
    have : w.toNat + 8 = 8 * n * (w.toNat / (8 * n)) + 9 := by omega
    rw [this, Nat.mul_add_mod, Nat.mod_eq_of_lt (by omega)]; omega
  · rw [ofWord_slot h]
    simp only [Mag.push, items, List.length_map, List.length_range]
    have hs := slot_lt n w (by omega)
    have := h.2
    by_cases hlt : slot n w + 1 < n
    · have ht := add8_toNat k hk' hn h hlt
      rw [ht]
      have hm := mod_eq_of_isSlot h
      have : (w.toNat + 8) % (8 * n) = w.toNat % (8 * n) + 8 := by
        have := Nat.div_add_mod w.toNat (8 * n)
        have : (w.toNat + 8) = 8 * n * (w.toNat / (8 * n)) + (w.toNat % (8 * n) + 8) := by omega
        rw [this, Nat.mul_add_mod]
        exact Nat.mod_eq_of_lt (by rw [hm]; omega)
      rw [this, hm]
      simp only [show slot n w - 1 + 2 < n by omega, ↓reduceIte, reduceCtorEq, false_iff]
      omega
    · -- The last slot: the word plus 8 is the next magazine's base.
      have he : slot n w = n - 1 := by omega
      simp only [show ¬ (slot n w - 1 + 2 < n) by omega, ↓reduceIte, true_iff]
      have hm := mod_eq_of_isSlot h
      rw [add8_toNat_mod]
      have hd := dvd_two_pow_64 k hk'
      rw [← hn] at hd
      rw [Nat.mod_mod_of_dvd _ hd]
      have hq := Nat.div_add_mod w.toNat (8 * n)
      rw [hm, he] at hq
      have : (w.toNat + 8) = 8 * n * (w.toNat / (8 * n) + 1) := by
        rw [Nat.mul_add, Nat.mul_one]; omega
      rw [this, Nat.mul_mod_right]; omega

/-- A push that finds a slot: the word names a slot below the last. -/
theorem push_of_test {n : Nat} {w : BitVec 64} (k : Nat) (hn : n = 2 ^ k) (hk : 2 ≤ k)
    (hk' : k ≤ 16) (h : WordOk n w) (m : Mem) (x : BitVec 64)
    (ht : ¬ (w + 8).toNat % (8 * n) < 16) :
    IsSlot n w ∧ slot n w + 1 < n ∧
      (Mag.ofWord n m w).push n x = some (.stack (magBase n w) (items n m w ++ [x])) := by
  have hne : (Mag.ofWord n m w).push n x ≠ none := fun h' =>
    ht ((push_eq_none_iff x k hn hk hk' h).mp h')
  have h' : IsSlot n w := by
    rcases h with h | h | h
    · exact absurd (by rw [ofWord_zero h]; rfl) hne
    · exact absurd (by rw [ofWord_marker h]; rfl) hne
    · exact h
  rw [ofWord_slot h'] at hne ⊢
  simp only [Mag.push, items, List.length_map, List.length_range] at hne ⊢
  have hs : slot n w - 1 + 2 < n := by
    by_cases hc : slot n w - 1 + 2 < n
    · exact hc
    · exact absurd (by simp only [hc, ↓reduceIte]) hne
  refine ⟨h', by omega, ?_⟩
  simp only [hs, ↓reduceIte]

/-- The magazine after a push's commit, if memory holds `x` in the slot the
word named and is unchanged in the slots below. -/
theorem ofWord_add8 {n : Nat} {m m' : Mem} {w : BitVec 64} (k : Nat) (hk' : k ≤ 16) (hn : n = 2 ^ k)
    (h : IsSlot n w) (hs : slot n w + 1 < n) (x : BitVec 64) (hx : m'.readW w 64 = x)
    (hm : ∀ i < slot n w - 1, m'.readW (magBase n w + BitVec.ofNat 64 (8 * (i + 1))) 64 =
      m.readW (magBase n w + BitVec.ofNat 64 (8 * (i + 1))) 64) :
    Mag.ofWord n m' (w + 8) = .stack (magBase n w) (items n m w ++ [x]) := by
  obtain ⟨hss, h', hb⟩ := slot_add8 k hk' hn h hs
  rw [ofWord_slot h', hb]
  congr 1
  simp only [items, hss, hb]
  have h1 := h.2
  rw [show slot n w + 1 - 1 = (slot n w - 1) + 1 by omega, List.range_succ, List.map_append]
  congr 1
  · apply List.map_congr_left
    intro i hi
    exact hm i (List.mem_range.mp hi)
  · simp only [List.map_cons, List.map_nil, List.cons.injEq, and_true]
    rw [show slot n w - 1 + 1 = slot n w by omega]
    rw [show magBase n w + BitVec.ofNat 64 (8 * slot n w) = w by
      apply BitVec.eq_of_toNat_eq
      rw [BitVec.toNat_add, magBase_toNat h, BitVec.toNat_ofNat]
      have := le_toNat_of_isSlot h
      have := w.isLt
      rw [Nat.mod_eq_of_lt (by omega)]
      rw [Nat.mod_eq_of_lt (by omega)]
      omega]
    exact hx

/-- The slot a word names is its base plus 8 per slot. -/
theorem magBase_add_slot {n : Nat} {w : BitVec 64} (h : IsSlot n w) :
    magBase n w + BitVec.ofNat 64 (8 * slot n w) = w := by
  apply BitVec.eq_of_toNat_eq
  rw [BitVec.toNat_add, magBase_toNat h, BitVec.toNat_ofNat]
  have := le_toNat_of_isSlot h
  have := w.isLt
  rw [Nat.mod_eq_of_lt (by omega), Nat.mod_eq_of_lt (by omega)]
  omega

/-- The magazine a word names depends only on the slots below it. -/
theorem ofWord_congr {n : Nat} {m m' : Mem} {w : BitVec 64}
    (hm : ∀ i < slot n w - 1, m'.readW (magBase n w + BitVec.ofNat 64 (8 * (i + 1))) 64 =
      m.readW (magBase n w + BitVec.ofNat 64 (8 * (i + 1))) 64) :
    Mag.ofWord n m' w = Mag.ofWord n m w := by
  unfold Mag.ofWord
  split
  · rfl
  · split
    · rfl
    · congr 1
      exact List.map_congr_left fun i hi => hm i (List.mem_range.mp hi)

/-! ## The routines' tests, as they make them -/

/-- The 32-bit test of a word with a mask held in 64 bits. -/
theorem test_iff {k : Nat} (hk : 2 ≤ k) (hk' : k ≤ 16) (w x : BitVec 64)
    (hx : x.toNat = magMask (2 ^ k)) :
    w.setWidth 32 &&& x.setWidth 32 = 0 ↔ w.toNat % (8 * 2 ^ k) < 16 := by
  rw [← mask_test k w.toNat hk (by omega), ← BitVec.toNat_inj, BitVec.toNat_and, BitVec.toNat_setWidth,
    BitVec.toNat_setWidth, hx, Nat.mod_eq_of_lt (magMask_lt k hk')]
  simp

/-- The 32-bit test of a word with a mask read as 32 bits. -/
theorem test_iff32 {k : Nat} (hk : 2 ≤ k) (hk' : k ≤ 16) (w : BitVec 64) (y : BitVec 32)
    (hy : y.toNat = magMask (2 ^ k)) :
    w.setWidth 32 &&& y = 0 ↔ w.toNat % (8 * 2 ^ k) < 16 := by
  rw [← mask_test k w.toNat hk (by omega), ← BitVec.toNat_inj, BitVec.toNat_and, BitVec.toNat_setWidth,
    hy]
  simp

/-! ## Slots in the magazine's region -/

/-- The slot below a word that names slot 2 or above. -/
theorem contains_below {n : Nat} {w : BitVec 64} (h : IsSlot n w) (h2 : 2 ≤ slot n w)
    (hs : slot n w < n) : (⟨magBase n w, 8 * n⟩ : Region).Contains (w - 8) 8 := by
  have hle := le_toNat_of_isSlot h
  simp only [Region.Contains, magBase]
  have := w.isLt
  have hs8 : 8 * slot n w < 2 ^ 64 := by omega
  rw [BitVec.toNat_sub, BitVec.toNat_sub, BitVec.toNat_sub, BitVec.toNat_ofNat, toNat_eight,
    Nat.mod_eq_of_lt hs8]
  omega

/-- The slot a word names, below the last. -/
theorem contains_at {n : Nat} {w : BitVec 64} (h : IsSlot n w) (hs : slot n w + 1 < n) :
    (⟨magBase n w, 8 * n⟩ : Region).Contains w 8 := by
  have hle := le_toNat_of_isSlot h
  simp only [Region.Contains, magBase]
  rw [Offset.sub_sub_eq, BitVec.sub_self, BitVec.zero_add, BitVec.toNat_ofNat]
  have := w.isLt
  omega

/-- The slot of item `i`, below the last slot. -/
theorem sub_item {n : Nat} {w : BitVec 64} {i : Nat} (hi : i + 2 ≤ n) :
    Region.Sub ⟨magBase n w + BitVec.ofNat 64 (8 * (i + 1)), 8⟩ ⟨magBase n w, 8 * n⟩ :=
  Offset.sub_base _ (by omega)

end VG.Proof.Slab
