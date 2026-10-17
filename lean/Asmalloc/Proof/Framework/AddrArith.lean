module

public import Asmalloc.TCB.Mem
public import Asmalloc.Proof.Framework.PowLit

/-!
# Address arithmetic without `bv_omega`

`bv_omega` turns every `(x - y).toNat` into `(2 ^ w - y.toNat + x.toNat) % 2 ^ w`,
and an address such as `E - m` into another such term inside it, so facts
about where an address lies relative to a stack pointer or region base `E`
cost it (and the kernel, checking its proof) a lot of case splitting. These
identities keep `x - E` together instead: rewrite `x - (E ± m)` to
`(x - E) ± m`, and `omega` then only sees the one atom `(x - E).toNat`.

These are the part of `VG.Offset` below `Mem` (which uses them); the facts
about byte ranges at offsets from a base are in `Proof/Framework/Offset.lean`.
-/

@[expose] public section

namespace VG.Offset

variable {w : Nat}

theorem add_sub_cancel_left (x y : BitVec w) : x + y - x = y := by
  rw [BitVec.add_comm, BitVec.add_sub_cancel]

theorem add_sub_add_left (x y z : BitVec w) : x + y - (x + z) = y - z := by
  rw [BitVec.sub_eq_iff_eq_add, BitVec.add_comm x z, ← BitVec.add_assoc, BitVec.sub_add_cancel,
    BitVec.add_comm]

theorem add_ofNat_add_sub (x : BitVec w) (a k : Nat) :
    x + BitVec.ofNat w (a + k) - (x + BitVec.ofNat w a) = BitVec.ofNat w k := by
  rw [add_sub_add_left, BitVec.ofNat_add, add_sub_cancel_left]

theorem sub_sub_eq (x E y : BitVec w) : x - (E - y) = (x - E) + y := by
  rw [BitVec.sub_eq_iff_eq_add, BitVec.add_assoc, BitVec.add_comm y, BitVec.sub_add_cancel,
    BitVec.sub_add_cancel]

theorem sub_add_sub_cancel (x y z : BitVec w) : x - y + (y - z) = x - z := by
  rw [eq_comm, BitVec.sub_eq_iff_eq_add, BitVec.add_assoc, BitVec.sub_add_cancel, BitVec.sub_add_cancel]

theorem add_ofNat_succ (a : BitVec w) (i : Nat) :
    a + BitVec.ofNat w (i + 1) = a + 1 + BitVec.ofNat w i := by
  rw [BitVec.ofNat_add, BitVec.add_comm (BitVec.ofNat w i), ← BitVec.add_assoc]; rfl

/-- The next byte after offset `k`. -/
theorem add_ofNat_add_one (a : BitVec w) (k : Nat) : a + BitVec.ofNat w k + 1 = a + BitVec.ofNat w (k + 1) := by
  rw [BitVec.add_assoc, show (1 : BitVec w) = BitVec.ofNat w 1 from rfl, BitVec.ofNat_add_ofNat]

/-- Offsets add up. -/
theorem add_ofNat_add_ofNat (a : BitVec w) (j k : Nat) :
    a + BitVec.ofNat w j + BitVec.ofNat w k = a + BitVec.ofNat w (j + k) := by
  rw [BitVec.add_assoc, BitVec.ofNat_add_ofNat]

theorem sub_add_eq (x E y : BitVec w) : x - (E + y) = (x - E) - y := by
  rw [BitVec.sub_sub]

theorem ofNat_sub_ofNat {k d : Nat} (h : d ≤ k) :
    BitVec.ofNat w k - BitVec.ofNat w d = BitVec.ofNat w (k - d) := by
  rw [BitVec.sub_eq_iff_eq_add, ← BitVec.ofNat_add, Nat.sub_add_cancel h]

theorem sub_ofNat_sub_sub_ofNat (x : BitVec w) {a b : Nat} (h : a ≤ b) :
    x - BitVec.ofNat w a - (x - BitVec.ofNat w b) = BitVec.ofNat w (b - a) := by
  rw [sub_sub_eq, BitVec.sub_eq_add_neg x (BitVec.ofNat w a), add_sub_cancel_left, BitVec.add_comm,
    ← BitVec.sub_eq_add_neg, ofNat_sub_ofNat h]

/-- `(t + m).toNat`, for a small `m`. -/
theorem toNat_add_ofNat (t : BitVec w) (m : Nat) :
    (t + BitVec.ofNat w m).toNat = (t.toNat + m % 2 ^ w) % 2 ^ w := by
  rw [BitVec.toNat_add, BitVec.toNat_ofNat]

/-- `(t - m).toNat`, for a small `m`. -/
theorem toNat_sub_ofNat (t : BitVec w) (m : Nat) :
    (t - BitVec.ofNat w m).toNat = (2 ^ w - m % 2 ^ w + t.toNat) % 2 ^ w := by
  rw [BitVec.toNat_sub, BitVec.toNat_ofNat]

/-- Byte `k` is not among the `n` bytes from `d`, at the same base. -/
theorem not_lt_sub_ofNat {d n k : Nat} (hsep : k < d ∨ d + n ≤ k) (hk : k < 2 ^ 64)
    (hn : 0 < n) (hdn : d + n ≤ 2 ^ 64) : ¬ (BitVec.ofNat 64 k - BitVec.ofNat 64 d).toNat < n := by
  rw [toNat_sub_ofNat, BitVec.toNat_ofNat, Nat.mod_eq_of_lt (a := k) hk]
  rcases hsep with h | h
  · rw [Nat.mod_eq_of_lt (a := d) (by omega), Nat.mod_eq_of_lt (by omega)]; omega
  · rw [Nat.mod_eq_of_lt (a := d) (by omega), show 2 ^ 64 - d + k = (k - d) + 2 ^ 64 by omega,
      Nat.add_mod_right, Nat.mod_eq_of_lt (by omega)]
    omega

/-- The `m` bytes below `E` and the `l` bytes from `E + a` do not overlap. -/
theorem disjoint_below_above (E : Addr) {m a l : Nat} (h : m + a + l ≤ 2 ^ 64) :
    Region.Disjoint ⟨E - BitVec.ofNat 64 m, m⟩ ⟨E + BitVec.ofNat 64 a, l⟩ := by
  intro x hx hy
  simp only [Region.Contains, sub_sub_eq, sub_add_eq, toNat_add_ofNat, toNat_sub_ofNat] at hx hy
  have := (x - E).isLt
  omega

/-- The `a` bytes below `E` lie within the `b ≥ a` bytes below `E`. -/
theorem below_mono (E : Addr) {a b : Nat} (h : a ≤ b) (hb : b < 2 ^ 64) (x : Addr)
    (hx : Region.Contains ⟨E - BitVec.ofNat 64 a, a⟩ x 1) :
    Region.Contains ⟨E - BitVec.ofNat 64 b, b⟩ x 1 := by
  simp only [Region.Contains, sub_sub_eq, toNat_add_ofNat] at hx ⊢
  have := (x - E).isLt
  rw [Nat.mod_eq_of_lt (a := a) (by omega)] at hx
  rw [Nat.mod_eq_of_lt (a := b) hb]
  by_cases h' : (x - E).toNat + b < 2 ^ 64
  · rw [Nat.mod_eq_of_lt h'] at ⊢; omega
  · rw [Nat.mod_eq_sub_mod (by omega), Nat.mod_eq_of_lt (by omega)]; omega

/-- The `m` bytes below `E` lie within the `r` bytes below `E + d`. -/
theorem below_sub_below (E : Addr) {m d r : Nat} (h : m + d ≤ r) (hr : r < 2 ^ 64) (x : Addr)
    (hx : Region.Contains ⟨E - BitVec.ofNat 64 m, m⟩ x 1) :
    Region.Contains ⟨E + BitVec.ofNat 64 d - BitVec.ofNat 64 r, r⟩ x 1 := by
  simp only [Region.Contains, sub_sub_eq, sub_add_eq, toNat_add_ofNat, toNat_sub_ofNat] at hx ⊢
  have := (x - E).isLt
  omega

/-! ## Arithmetic on offsets -/

theorem add_add {w : Nat} (p : BitVec w) (a b : Nat) :
    p + BitVec.ofNat w a + BitVec.ofNat w b = p + BitVec.ofNat w (a + b) := by
  rw [BitVec.add_assoc, BitVec.ofNat_add_ofNat]

theorem add_add_eq {w : Nat} (p : BitVec w) {a b c : Nat} (h : a + b = c) :
    p + BitVec.ofNat w a + BitVec.ofNat w b = p + BitVec.ofNat w c := by
  rw [add_add, h]

/-- Adding equal offsets. -/
theorem add_congr (b : Addr) {x y : BitVec 64} (h : x = y) : b + x = b + y := h ▸ rfl

theorem add_sub_comm {w : Nat} (a b c : BitVec w) : a + c - b = a - b + c := by
  rw [BitVec.sub_eq_add_neg, BitVec.sub_eq_add_neg, BitVec.add_assoc, BitVec.add_comm c,
    ← BitVec.add_assoc]

theorem add_sub_add {w : Nat} (p : BitVec w) {a b : Nat} (h : b ≤ a) :
    p + BitVec.ofNat w a - (p + BitVec.ofNat w b) = BitVec.ofNat w (a - b) := by
  rw [add_sub_add_left, ofNat_sub_ofNat h]

theorem add_ofNat_sub (p : Addr) {a k : Nat} (h : k ≤ a) :
    p + BitVec.ofNat 64 a - BitVec.ofNat 64 k = p + BitVec.ofNat 64 (a - k) := by
  rw [← ofNat_sub_ofNat h]; bv_omega

theorem add_ofNat_add_neg (p : Addr) {a k : Nat} (h : k ≤ a) :
    p + BitVec.ofNat 64 a + (0 - BitVec.ofNat 64 k) = p + BitVec.ofNat 64 (a - k) := by
  rw [← add_ofNat_sub p h]; bv_omega

theorem add_sub_one32 (a b : BitVec 32) (h : 1 ≤ b.toNat) :
    a + b - 1 = a + BitVec.ofNat 32 (b.toNat - 1) := by
  bv_omega

theorem add_sub_one64 (a b : BitVec 64) (h : 1 ≤ b.toNat) :
    a + b - 1 = a + BitVec.ofNat 64 (b.toNat - 1) := by
  bv_omega

theorem add_ofInt_neg_one (a b : BitVec 64) (h : 1 ≤ b.toNat) :
    a + b + BitVec.ofInt 64 (-1) = a + BitVec.ofNat 64 (b.toNat - 1) := by
  rw [show BitVec.ofInt 64 (-1) = 0 - 1 by decide]
  bv_omega

theorem add_one_eq (x : BitVec 64) : x + 1 = BitVec.ofNat 64 (x.toNat + 1) := by
  rw [BitVec.ofNat_add, BitVec.ofNat_toNat, BitVec.setWidth_eq]; rfl

/-- `x == y` for numbers below `2⁶⁴`. -/
theorem ofNat_sub_ofNat_beq {x y : Nat} (hx : x < 2 ^ 64) (hy : y < 2 ^ 64) :
    (BitVec.ofNat 64 x - BitVec.ofNat 64 y == 0) = decide (x = y) := by
  rw [Bool.eq_iff_iff, beq_iff_eq, decide_eq_true_iff]
  bv_omega

end VG.Offset
