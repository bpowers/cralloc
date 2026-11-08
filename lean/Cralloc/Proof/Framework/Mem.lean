module

public import Cralloc.TCB.Mem
public import Cralloc.Proof.Framework.AddrArith
public import Cralloc.Proof.Framework.GetElem
public import Cralloc.Proof.Framework.RegionsSub

/-!
# Memory: reads after writes, regions, frames
-/

@[expose] public section

-- `rw` closes goals `a ≤ a`.
attribute [refl] Nat.le_refl

namespace VG

theorem nodup_reverse {α : Type _} {l : List α} (h : l.Nodup) : l.reverse.Nodup :=
  List.pairwise_reverse.mpr (h.imp fun h e => h e.symm)

namespace Mem

theorem read_congr {m m' : Mem} {a : Addr} {n : Nat}
    (h : ∀ i < n, m (a + BitVec.ofNat 64 i) = m' (a + BitVec.ofNat 64 i)) :
    m.read a n = m'.read a n := by
  induction n generalizing a with
  | zero => rfl
  | succ n ih =>
    simp only [read]
    have h0 := h 0 (by omega)
    simp only [BitVec.add_zero] at h0
    rw [h0, ih fun i hi => ?_]
    have := h (i + 1) (by omega)
    rwa [Offset.add_ofNat_succ] at this

theorem readW_congr {m m' : Mem} {a : Addr} {w : Nat}
    (h : ∀ i < w / 8, m (a + BitVec.ofNat 64 i) = m' (a + BitVec.ofNat 64 i)) :
    m.readW a w = m'.readW a w := by
  simp only [readW, read_congr h]

theorem write_apply {m : Mem} {b x : Addr} {n : Nat} {v : BitVec (8 * n)}
    (h : ¬ (x - b).toNat < n) : m.write b n v x = m x := by
  simp only [write, h, ite_false]

/-- The byte ranges `[a, a + n)` and `[b, b + k)` (modulo 2⁶⁴) do not overlap. -/
def Sep (a : Addr) (n : Nat) (b : Addr) (k : Nat) : Prop :=
  ∀ x, (x - a).toNat < n → ¬ (x - b).toNat < k

theorem sub_ofNat_toNat (a : Addr) {i : Nat} (hi : i < 2 ^ 64) :
    (a + BitVec.ofNat 64 i - a).toNat = i := by
  rw [Offset.add_sub_cancel_left, BitVec.toNat_ofNat]
  exact Nat.mod_eq_of_lt hi

theorem read_write_sep {m : Mem} {a b : Addr} {n k : Nat} {v : BitVec (8 * k)} (h : Sep a n b k)
    (hn : n < 2 ^ 64) : (m.write b k v).read a n = m.read a n :=
  read_congr fun i hi => write_apply (h _ (by rw [sub_ofNat_toNat a (by omega)]; exact hi))

theorem readW_writeW_sep {m : Mem} {a b : Addr} {w w' : Nat} {v : BitVec w'}
    (h : Sep a (w / 8) b (w' / 8)) (hn : w / 8 < 2 ^ 64) :
    (m.writeW b v).readW a w = m.readW a w := by
  simp only [readW, writeW, read_write_sep h hn]

theorem read_eq_of_bytes {m : Mem} {a : Addr} {n : Nat} {v : BitVec (8 * n)}
    (h : ∀ i < n, m (a + BitVec.ofNat 64 i) = v.extractLsb' (8 * i) 8) : m.read a n = v := by
  induction n generalizing a with
  | zero => exact BitVec.eq_nil _ |>.trans (BitVec.eq_nil _).symm
  | succ n ih =>
    simp only [read]
    have hv : ∀ i < n, m (a + 1 + BitVec.ofNat 64 i) =
        (v.extractLsb' 8 (8 * n)).extractLsb' (8 * i) 8 := by
      intro i hi
      rw [← Offset.add_ofNat_succ, h (i + 1) (by omega)]
      ext j hj
      simp only [BitVec.getElem_extractLsb', BitVec.getLsbD_extractLsb']
      simp only [show 8 * i + j < 8 * n by omega, decide_true, Bool.true_and]
      congr 1; omega
    rw [ih hv]
    have h0 := h 0 (by omega)
    simp only [BitVec.add_zero] at h0
    rw [h0]
    ext j hj
    simp only [BitVec.getElem_append, BitVec.getElem_extractLsb']
    split
    · rw [Nat.mul_zero, Nat.zero_add]; exact BitVec.getLsbD_eq_getElem hj
    · rw [← BitVec.getLsbD_eq_getElem hj]; congr 1; omega

theorem readW_writeW_self (m : Mem) (a : Addr) (n : Nat) (v : BitVec (8 * n)) (hn : n < 2 ^ 64) :
    (m.writeW a v).readW a (8 * n) = v := by
  simp only [readW, writeW]
  rw [show 8 * n / 8 = n by omega]
  simp only [BitVec.setWidth_eq]
  refine read_eq_of_bytes fun i hi => ?_
  simp only [write, sub_ofNat_toNat a (show i < 2 ^ 64 by omega), hi, ite_true]

/-- Byte `j` of a little-endian read. -/
theorem extractLsb'_read (m : Mem) (a : Addr) {n j : Nat} (hj : j < n) :
    (m.read a n).extractLsb' (8 * j) 8 = m (a + BitVec.ofNat 64 j) := by
  induction n generalizing a j with
  | zero => omega
  | succ n ih =>
    simp only [Mem.read]
    cases j with
    | zero =>
      rw [BitVec.ofNat_eq_ofNat, BitVec.add_zero]
      ext i hi
      simp only [BitVec.getElem_extractLsb', BitVec.getLsbD_append]
      simp [hi]
    | succ j =>
      rw [Offset.add_ofNat_succ,
        ← ih (a := a + 1) (by omega)]
      ext i hi
      simp only [BitVec.getElem_extractLsb', BitVec.getLsbD_append,
        show ¬ (8 * (j + 1) + i < 8) by omega, ite_false]
      congr 1; omega

/-- Byte `i` of a little-endian word. -/
theorem readW_byte (m : Mem) (a : Addr) {i : Nat} (hi : i < 4) :
    m (a + BitVec.ofNat 64 i) = (m.readW a 32).extractLsb' (8 * i) 8 := by
  rw [← extractLsb'_read m a (n := 4) hi]
  simp only [Mem.readW]
  rfl

theorem readW_writeW_self32 (m : Mem) (a : Addr) (v : BitVec 32) :
    (m.writeW a v).readW a 32 = v := readW_writeW_self m a 4 v (by decide)

theorem readW_writeW_self64 (m : Mem) (a : Addr) (v : BitVec 64) :
    (m.writeW a v).readW a 64 = v := readW_writeW_self m a 8 v (by decide)

end Mem

/-! ## Regions -/

namespace Region

theorem Contains.byte {r : Region} {b x : Addr} {n : Nat} (h : r.Contains b n)
    (hx : (x - b).toNat < n) : r.Contains x 1 := by
  unfold Contains at *
  have : (x - r.base).toNat ≤ (x - b).toNat + (b - r.base).toNat := by
    rw [← Offset.sub_add_sub_cancel x b r.base, BitVec.toNat_add]
    exact Nat.mod_le _ _
  omega

theorem Disjoint.sep {r₁ r₂ : Region} (h : r₁.Disjoint r₂) {a b : Addr} {n k : Nat}
    (ha : r₁.Contains a n) (hb : r₂.Contains b k) : Mem.Sep a n b k :=
  fun _ hx hy => h _ (ha.byte hx) (hb.byte hy)

theorem Disjoint.symm {r₁ r₂ : Region} (h : r₁.Disjoint r₂) : r₂.Disjoint r₁ :=
  fun a h₂ h₁ => h a h₁ h₂

end Region

/-- A sub-region (every byte of `r` is in `r'`). -/
def Region.Sub (r r' : Region) : Prop := ∀ a, r.Contains a 1 → r'.Contains a 1

theorem Region.Disjoint.sub_right {r₁ r₂ r₂' : Region} (h : r₁.Disjoint r₂) (hs : Region.Sub r₂' r₂) :
    r₁.Disjoint r₂' := fun a h₁ h₂ => h a h₁ (hs a h₂)

theorem Region.Disjoint.sub_left {r₁ r₁' r₂ : Region} (h : r₁.Disjoint r₂) (hs : Region.Sub r₁' r₁) :
    r₁'.Disjoint r₂ := fun a h₁ h₂ => h a (hs a h₁) h₂

theorem Region.sub_prefix {base : Addr} {len len' : Nat} (h : len ≤ len') :
    Region.Sub ⟨base, len⟩ ⟨base, len'⟩ := fun a ha => by
  simp only [Region.Contains] at *; omega

/-- `Region.sub_prefix` for literal lengths (`Nat.ble` evaluates them). -/
theorem Region.sub_of_ble {a : Addr} {m n : Nat} (h : Nat.ble m n = true) :
    Region.Sub ⟨a, m⟩ ⟨a, n⟩ :=
  Region.sub_prefix (Nat.le_of_ble_eq_true h)

/-- A bound on the end of a region bounds the end of a shorter one. -/
theorem Region.end_le_of_ble {x m n b : Nat} (hmn : Nat.ble m n = true) (h : x + n ≤ b) :
    x + m ≤ b :=
  Nat.le_trans (Nat.add_le_add_left (Nat.le_of_ble_eq_true hmn) _) h

theorem Region.contains_self (a : Addr) (n : Nat) : (⟨a, n⟩ : Region).Contains a n := by
  simp [Region.Contains]

theorem InRegions.of_mem {rs : List Region} {r : Region} {a : Addr} {n : Nat} (hr : r ∈ rs)
    (hc : r.Contains a n) : InRegions rs a n := ⟨r, hr, hc⟩

theorem InRegions.append_left {rs ws : List Region} {a : Addr} {n : Nat} (h : InRegions rs a n) :
    InRegions (rs ++ ws) a n :=
  let ⟨r, hr, hc⟩ := h; ⟨r, List.mem_append_left _ hr, hc⟩

theorem InRegions.append_right {rs ws : List Region} {a : Addr} {n : Nat} (h : InRegions ws a n) :
    InRegions (rs ++ ws) a n :=
  let ⟨r, hr, hc⟩ := h; ⟨r, List.mem_append_right _ hr, hc⟩

/-- An access within regions with `f` inserted (as a frame's push inserts
it at the head of the writable regions) is within `f` or the others. -/
theorem InRegions_append_cons {xs ys : List Region} {f : Region} {a : Addr} {n : Nat} :
    InRegions (xs ++ f :: ys) a n ↔ f.Contains a n ∨ InRegions (xs ++ ys) a n := by
  simp only [InRegions, List.mem_append, List.mem_cons]
  constructor
  · rintro ⟨r, hr | rfl | hr, hc⟩
    · exact .inr ⟨r, .inl hr, hc⟩
    · exact .inl hc
    · exact .inr ⟨r, .inr hr, hc⟩
  · rintro (hc | ⟨r, hr | hr, hc⟩)
    · exact ⟨f, .inr (.inl rfl), hc⟩
    · exact ⟨r, .inl hr, hc⟩
    · exact ⟨r, .inr (.inr hr), hc⟩

/-- `m'` agrees with `m` outside the regions `rs`. -/
def Frame (rs : List Region) (m m' : Mem) : Prop :=
  ∀ x, (∀ r ∈ rs, ¬ r.Contains x 1) → m' x = m x

namespace Frame

theorem refl (rs : List Region) (m : Mem) : Frame rs m m := fun _ _ => rfl

theorem trans {rs : List Region} {m₁ m₂ m₃ : Mem} (h₁ : Frame rs m₁ m₂) (h₂ : Frame rs m₂ m₃) :
    Frame rs m₁ m₃ := fun x hx => (h₂ x hx).trans (h₁ x hx)

theorem mono {rs rs' : List Region} {m m' : Mem} (h : Frame rs m m') (hs : ∀ r ∈ rs, r ∈ rs') :
    Frame rs' m m' := fun x hx => h x fun r hr => hx r (hs r hr)

/-- A write inside one of the regions. -/
theorem write {rs : List Region} {m m' : Mem} (h : Frame rs m m') {r : Region} (hr : r ∈ rs)
    {b : Addr} {n : Nat} (v : BitVec (8 * n)) (hb : r.Contains b n) :
    Frame rs m (m'.write b n v) := by
  intro x hx
  rw [Mem.write_apply fun h' => hx r hr (hb.byte h')]
  exact h x hx

theorem writeW {rs : List Region} {m m' : Mem} (h : Frame rs m m') {r : Region} (hr : r ∈ rs)
    {b : Addr} {w : Nat} (v : BitVec w) (hb : r.Contains b (w / 8)) :
    Frame rs m (m'.writeW b v) := h.write hr _ hb

/-- Reading outside the regions. -/
theorem read {rs : List Region} {m m' : Mem} (h : Frame rs m m') {r : Region} {a : Addr} {n : Nat}
    (ha : r.Contains a n) (hd : ∀ r' ∈ rs, r.Disjoint r') (hn : n < 2 ^ 64) :
    m'.read a n = m.read a n :=
  Mem.read_congr fun i hi => h _ fun r' hr' =>
    hd r' hr' _ (ha.byte (by rw [Mem.sub_ofNat_toNat a (by omega)]; exact hi))

theorem sub {rs rs' : List Region} {m m' : Mem} (h : Frame rs m m')
    (hs : ∀ r ∈ rs, ∃ r' ∈ rs', Region.Sub r r') : Frame rs' m m' := fun x hx =>
  h x fun r hr hc => by obtain ⟨r', hr', hsub⟩ := hs r hr; exact hx r' hr' (hsub x hc)

theorem readW {rs : List Region} {m m' : Mem} (h : Frame rs m m') {r : Region} {a : Addr} {w : Nat}
    (ha : r.Contains a (w / 8)) (hd : ∀ r' ∈ rs, r.Disjoint r') (hn : w / 8 < 2 ^ 64) :
    m'.readW a w = m.readW a w := by
  simp only [Mem.readW, h.read ha hd hn]

/-- A byte of a region disjoint from the frame's regions is unchanged. -/
theorem bytes {rs : List Region} {m m' : Mem} (hf : Frame rs m m') {R : Region}
    (hd : ∀ r ∈ rs, R.Disjoint r) (hR : R.len ≤ 2 ^ 64) {i : Nat} (hi : i < R.len) :
    m' (R.base + BitVec.ofNat 64 i) = m (R.base + BitVec.ofNat 64 i) := by
  refine hf _ fun r hr hc => hd r hr _ ?_ hc
  simp only [Region.Contains]
  rw [Offset.add_sub_cancel_left, BitVec.toNat_ofNat,
    Nat.mod_eq_of_lt (by omega)]
  omega

end Frame

end VG

