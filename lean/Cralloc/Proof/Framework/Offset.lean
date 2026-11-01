module

public import Cralloc.Proof.Framework.PlainSimp
public import Cralloc.Proof.Framework.Mem

/-!
# Byte ranges at offsets from a base address

Facts about the addresses `p + d` (`d` a natural number) of a buffer at `p`,
proven once for any offsets: the distance between two of them, and that two
of its sub-ranges are separate or one contains the other. `bv_omega` proves
each instance, but takes a fraction of a second or more to (it turns every
bit vector in the context into a bounded natural number, and case-splits on
the wrap-around of every subtraction), and proofs need them for many ranges.
Here a distance from `p + d` is the distance from `p` less `d` (`lt_iff`), so
what remains is arithmetic on natural numbers: the hypotheses of these lemmas
are closed by `omega`, or by `decide` for literal offsets.

The identities of bit vectors (`add_add`, `add_sub_cancel_left`,
`ofNat_sub_ofNat`, `sub_sub_eq`, …) are in the same namespace, in
`Cralloc.Proof.Framework.AddrArith` (which `Mem` imports).
-/

@[expose] public section

namespace VG.Offset

/-! ## Distances -/

/-- The distance from `p + d` to `x`, from the distance from `p`. -/
theorem toNat_sub_add (x p : Addr) {d : Nat} (hd : d < 2 ^ 64) :
    (x - (p + BitVec.ofNat 64 d)).toNat = (2 ^ 64 - d + (x - p).toNat) % 2 ^ 64 := by
  rw [← BitVec.sub_sub, BitVec.toNat_sub, BitVec.toNat_ofNat, Nat.mod_eq_of_lt hd]

/-- `x` is within `n` bytes of `p + d` iff its distance from `p` is in `[d, d + n)`. -/
theorem lt_iff (x p : Addr) {d n : Nat} (h : d + n ≤ 2 ^ 64) :
    (x - (p + BitVec.ofNat 64 d)).toNat < n ↔ d ≤ (x - p).toNat ∧ (x - p).toNat < d + n := by
  have := (x - p).isLt
  rcases Nat.eq_zero_or_pos n with rfl | hn
  · simp only [Nat.not_lt_zero, false_iff, Nat.add_zero]; omega
  rw [toNat_sub_add x p (by omega)]
  constructor
  · intro h'
    by_cases hd : d ≤ (x - p).toNat
    · rw [show 2 ^ 64 - d + (x - p).toNat = (x - p).toNat - d + 2 ^ 64 by omega, Nat.add_mod_right,
        Nat.mod_eq_of_lt (by omega)] at h'
      omega
    · rw [Nat.mod_eq_of_lt (by omega)] at h'; omega
  · intro ⟨h₁, h₂⟩
    rw [show 2 ^ 64 - d + (x - p).toNat = (x - p).toNat - d + 2 ^ 64 by omega, Nat.add_mod_right,
      Nat.mod_eq_of_lt (by omega)]
    omega

/-- The distance from `p + d` to `p + e`. -/
theorem sub_toNat (p : Addr) {d e : Nat} (h : d ≤ e) (he : e < 2 ^ 64) :
    (p + BitVec.ofNat 64 e - (p + BitVec.ofNat 64 d)).toNat = e - d := by
  rw [toNat_sub_add _ _ (by omega), Mem.sub_ofNat_toNat p he,
    show 2 ^ 64 - d + e = e - d + 2 ^ 64 by omega, Nat.add_mod_right, Nat.mod_eq_of_lt (by omega)]

/-- The distance from `p + d` to `p + e`, either way round. -/
theorem sub_toNat' (p : Addr) {d e : Nat} (hd : d < 2 ^ 64) (he : e < 2 ^ 64) :
    (p + BitVec.ofNat 64 e - (p + BitVec.ofNat 64 d)).toNat =
      if d ≤ e then e - d else 2 ^ 64 + e - d := by
  rw [toNat_sub_add _ _ hd, Mem.sub_ofNat_toNat p he]
  split
  · rw [show 2 ^ 64 - d + e = e - d + 2 ^ 64 by omega, Nat.add_mod_right, Nat.mod_eq_of_lt (by omega)]
  · rw [Nat.mod_eq_of_lt (by omega)]; omega

/-- Distinct offsets of `p` are distinct addresses. -/
theorem add_ofNat_ne (p : Addr) {a b : Nat} (ha : a < 2 ^ 64) (hb : b < 2 ^ 64) (h : a ≠ b) :
    p + BitVec.ofNat 64 a ≠ p + BitVec.ofNat 64 b := by
  intro e
  apply h
  have := congrArg (fun y => (y - p).toNat) e
  simp only [Mem.sub_ofNat_toNat p ha, Mem.sub_ofNat_toNat p hb] at this
  exact this

/-! ## Separate ranges -/

/-- `[p + d, p + d + n)` and `[p + e, p + e + k)` are separate if the offsets
say so. -/
theorem sep (p : Addr) {d n e k : Nat} (h : d + n ≤ e ∨ e + k ≤ d) (hd : d + n ≤ 2 ^ 64)
    (he : e + k ≤ 2 ^ 64) : Mem.Sep (p + BitVec.ofNat 64 d) n (p + BitVec.ofNat 64 e) k := by
  intro x h₁ h₂
  rw [lt_iff x p hd] at h₁
  rw [lt_iff x p he] at h₂
  omega

/-- `[p, p + n)` and `[p + e, p + e + k)` are separate if `n ≤ e`. -/
theorem sep_base (p : Addr) {n e k : Nat} (h : n ≤ e) (he : e + k ≤ 2 ^ 64) :
    Mem.Sep p n (p + BitVec.ofNat 64 e) k := by
  intro x h₁ h₂
  rw [lt_iff x p he] at h₂
  omega

/-- Two regions at offsets of `p` that do not overlap. -/
theorem disjoint (p : Addr) {d n e k : Nat} (h : d + n ≤ e ∨ e + k ≤ d) (hd : d + n ≤ 2 ^ 64)
    (he : e + k ≤ 2 ^ 64) : Region.Disjoint ⟨p + BitVec.ofNat 64 d, n⟩ ⟨p + BitVec.ofNat 64 e, k⟩ :=
  fun x h₁ h₂ => sep p h hd he x h₁ h₂

/-- `[p + d, p + d + n)` lies above `[p, p + k)`. -/
theorem disjoint_base (p : Addr) {d n k : Nat} (h : k ≤ d) (hd : d + n ≤ 2 ^ 64) :
    Region.Disjoint ⟨p + BitVec.ofNat 64 d, n⟩ ⟨p, k⟩ :=
  fun x h₁ h₂ => sep_base p h hd x h₂ h₁

/-- `[p, p + k)` lies below `[p + e, p + e + n)`. -/
theorem base_disjoint (p : Addr) {e n k : Nat} (h : k ≤ e) (he : e + n ≤ 2 ^ 64) :
    Region.Disjoint ⟨p, k⟩ ⟨p + BitVec.ofNat 64 e, n⟩ :=
  fun x h₁ h₂ => sep_base p h he x h₁ h₂

/-- `[p + d, p + d + k)` lies above `[p - n, p)`. -/
theorem disjoint_below (p : Addr) {n d k : Nat} (h : n + d + k ≤ 2 ^ 64) :
    Region.Disjoint ⟨p + BitVec.ofNat 64 d, k⟩ ⟨p - BitVec.ofNat 64 n, n⟩ := by
  rw [show p + BitVec.ofNat 64 d = p - BitVec.ofNat 64 n + BitVec.ofNat 64 (n + d) by
    rw [BitVec.ofNat_add, ← BitVec.add_assoc, BitVec.sub_add_cancel]]
  exact disjoint_base _ (Nat.le_add_right _ _) (by omega)

/-- `[p, p + k)` lies above `[p - n, p)`. -/
theorem base_disjoint_below (p : Addr) {n k : Nat} (h : n + k ≤ 2 ^ 64) :
    Region.Disjoint ⟨p, k⟩ ⟨p - BitVec.ofNat 64 n, n⟩ := by
  have := disjoint_below p (n := n) (d := 0) (k := k) (by omega)
  rwa [show p + BitVec.ofNat 64 0 = p from BitVec.add_zero p] at this

/-- Two regions, the first below the second, are disjoint. -/
theorem disjoint_of_le {r₁ r₂ : Region} (h : r₁.base.toNat + r₁.len ≤ r₂.base.toNat)
    (h' : r₂.base.toNat + r₂.len ≤ 2 ^ 64) : r₁.Disjoint r₂ := by
  intro a h₁ h₂
  simp only [Region.Contains] at h₁ h₂
  bv_omega

/-! ## Ranges within ranges -/

/-- A range at an offset of `p` within a region at an offset of `p`. -/
theorem contains (p : Addr) {d n e k : Nat} (h₁ : e ≤ d) (h₂ : d + n ≤ e + k) (h₃ : e + k < 2 ^ 64) :
    (⟨p + BitVec.ofNat 64 e, k⟩ : Region).Contains (p + BitVec.ofNat 64 d) n := by
  simp only [Region.Contains]
  rw [sub_toNat p h₁ (by omega)]
  omega

/-- A range at an offset of `p` within a region at `p`. -/
theorem contains_base (p : Addr) {d n k : Nat} (h : d + n ≤ k) (hd : d < 2 ^ 64) :
    (⟨p, k⟩ : Region).Contains (p + BitVec.ofNat 64 d) n := by
  simp only [Region.Contains]
  rw [Mem.sub_ofNat_toNat p hd]
  exact h

/-- `[p + d, p + d + n)` lies in `[p, p + k)`. -/
theorem sub_base (p : Addr) {d n k : Nat} (h : d + n ≤ k) :
    Region.Sub ⟨p + BitVec.ofNat 64 d, n⟩ ⟨p, k⟩ := by
  intro x hx
  simp only [Region.Contains] at hx ⊢
  have := (x - p).isLt
  by_cases hk : k < 2 ^ 64
  · rw [Nat.lt_iff_add_one_le.symm, lt_iff x p (by omega)] at hx
    omega
  · omega

/-- `[p + d, p + d + n)` lies in `[p + e, p + e + k)`. -/
theorem sub (p : Addr) {d n e k : Nat} (h₁ : e ≤ d) (h₂ : d + n ≤ e + k) :
    Region.Sub ⟨p + BitVec.ofNat 64 d, n⟩ ⟨p + BitVec.ofNat 64 e, k⟩ := by
  rw [show p + BitVec.ofNat 64 d = p + BitVec.ofNat 64 e + BitVec.ofNat 64 (d - e) by
    rw [BitVec.add_assoc, BitVec.ofNat_add_ofNat, Nat.add_sub_cancel' h₁]]
  exact sub_base _ (by omega)

/-! ## Ranges below a base

The ranges at `p - a` for natural numbers `a` (such as the words below a
stack pointer), as ranges at offsets `c - a` of a lower base `p - c`. -/

/-- `p - a` as an offset of `p - b`, for `a ≤ b`. -/
theorem sub_ofNat_eq {w : Nat} (p : BitVec w) {a b : Nat} (h : a ≤ b) :
    p - BitVec.ofNat w a = p - BitVec.ofNat w b + BitVec.ofNat w (b - a) := by
  rw [← sub_ofNat_sub_sub_ofNat p h, BitVec.add_comm, BitVec.sub_add_cancel]

/-- `[p - a, p - a + n)` lies in `[p - b, p - b + m)`. -/
theorem sub_below (p : Addr) {a n b m : Nat} (h₁ : a ≤ b) (h₂ : b - a + n ≤ m) :
    Region.Sub ⟨p - BitVec.ofNat 64 a, n⟩ ⟨p - BitVec.ofNat 64 b, m⟩ := by
  rw [sub_ofNat_eq p h₁]; exact sub_base _ h₂

/-- `[p - b, p - b + m)` contains `[p - a, p - a + n)`. -/
theorem contains_below (p : Addr) {a n b m : Nat} (h₁ : a ≤ b) (h₂ : b - a + n ≤ m) (hb : b < 2 ^ 64) :
    (⟨p - BitVec.ofNat 64 b, m⟩ : Region).Contains (p - BitVec.ofNat 64 a) n := by
  rw [sub_ofNat_eq p h₁]; exact contains_base _ h₂ (by omega)

/-- `[p - a, p - a + n)` and `[p - b, p - b + k)` are separate if their
offsets from `p - c` say so. -/
theorem sep_below (p : Addr) (c : Nat) {a n b k : Nat} (ha : a ≤ c) (hb : b ≤ c)
    (h : c - a + n ≤ c - b ∨ c - b + k ≤ c - a) (hn : c - a + n ≤ 2 ^ 64) (hk : c - b + k ≤ 2 ^ 64) :
    Mem.Sep (p - BitVec.ofNat 64 a) n (p - BitVec.ofNat 64 b) k := by
  have := sep (p - BitVec.ofNat 64 c) h hn hk
  rwa [← sub_ofNat_eq p ha, ← sub_ofNat_eq p hb] at this

end VG.Offset

/-- Closes `b + c₁ + c₂ + … = b + c` for literal offsets `cᵢ`, `c` (as
`BitVec.ofNat` or numerals): the offsets are added by `decide`. -/
macro "off_rfl" : tactic => `(tactic| plain_simp_names (
                   simp only [BitVec.add_assoc]
                   try exact VG.Offset.add_congr _ (by decide)))

/-- Closes `Region.Disjoint r r'` for ranges at offsets from a common base
(`Offset.disjoint`, `Offset.base_disjoint`, `Offset.disjoint_base`), with the
side conditions by `omega`. -/
macro "off_disj" : tactic => `(tactic| first
  | exact VG.Offset.disjoint _ (by omega) (by omega) (by omega)
  | exact VG.Offset.base_disjoint _ (by omega) (by omega)
  | exact VG.Offset.disjoint_base _ (by omega) (by omega))
