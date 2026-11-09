module

public import Cralloc.Proof.CEntry.AArch64.Steps

/-!
# The C routines' concrete states on AArch64

A thread whose rseq area is at `0x10100` (the offset `0x100`, in `__rseq_offset` at `0x2000`, from a
thread pointer of `0x10000`), with the slabs at `0x1000000`, a routine's table at `0x8000000` (2048
bytes in, its first entry), class 1's mask in its entry at `0x3000808`, and the registers a setup
leaves. `witCheck` decides by evaluation all that a routine's precondition asks of such a state.
-/

@[expose] public section

namespace VG.Proof.CEntry.AArch64

open VG.AArch64 VG.Spec.Slab VG.Spec.CEntry VG.Proof.Slab.AArch64 VG.Proof.CEntry VG.Proof.Slab

/-- A state of a thread as above: `x0`, `x1`, `x2` and `x12` are the routine's, and memory the
words `ws`. -/
def witS (tbl : String) (x0 x1 x2 x12 : BitVec 64) (ws : List (Addr × BitVec 64)) (rd wr : List Region) :
    State where
  gpr r :=
    if r = .x0 then x0 else if r = .x1 then x1 else if r = .x2 then x2 else
    if r = .x10 then 0x10100 else if r = .x11 then 0x9000 else if r = .x12 then x12 else
    if r = .x13 then 0x1000000 else if r = .x14 then 0x3000000 else if r = .x15 then 0x3000800 else 0
  mem := mkMem ws
  rd := rd
  wr := wr
  syms n := if n = "__rseq_offset" then 0x2000 else if n = "SLABS" then 0x1000000 else
    if n = tbl then 0x8000000 else 0
  tpidr := 0x10000
  csDesc := 0x9000

/-- No word of `ws` is in the masks' table, but class 1's entry. -/
def masksClear (ws : List (Addr × BitVec 64)) : Bool :=
  ws.all fun bv => decide (bv.1.toNat + 8 ≤ 0x3000800) || bv.1.toNat == 0x3000808 ||
    decide (0x3001000 ≤ bv.1.toNat ∧ bv.1.toNat + 8 ≤ 2 ^ 64)

/-- No word of `ws` is in a slab but slab 0. -/
def slabsClear (ws : List (Addr × BitVec 64)) : Bool :=
  ws.all fun bv => decide (bv.1.toNat ≤ 0x1000008) ||
    decide (0x3000000 ≤ bv.1.toNat ∧ bv.1.toNat + 8 ≤ 2 ^ 64)

/-- All that a routine's precondition asks of a concrete state whose memory is `ws`, by
evaluation. -/
def witCheck (c : Spec.CEntry.Ctx isa) (s : State) (ws : List (Addr × BitVec 64)) : Bool :=
  decide (s.rd = c.ro s) && decide (s.wr = c.rw s) &&
  ((c.regions s).all fun x => decide (x.1.base.toNat + x.1.len ≤ 2 ^ 64)) &&
  Sig.pairsSep (c.regions s) &&
  ((c.regions s).all fun x => (c.A.reserved 0 s).all fun q => Region.sep x.1 q) &&
  layoutCheck c s && decide (c.maskOf s 1 = magMask (2 ^ 2)) &&
  decide (WordOk (c.slotsOf' s 1) (c.wordOf s 1 0)) &&
  (!decide (IsSlot (c.slotsOf' s 1) (c.wordOf s 1 0)) ||
    apartCheck c s ⟨magBase (c.slotsOf' s 1) (c.wordOf s 1 0), 8 * c.slotsOf' s 1⟩) &&
  masksClear ws && slabsClear ws

section

variable {regs : List Reg} {c : Spec.CEntry.Ctx isa} (hA : A64 c.A regs) {s : State}
  {ws : List (Addr × BitVec 64)} (hm : s.mem = mkMem ws) (hsl : s.syms SLABS = 0x1000000)
include hA hm hsl

/-- Every mask but class 1's is zero. -/
theorem masks_zero (h : masksClear ws = true) : ∀ k < MASK_COUNT, k ≠ 1 → c.maskOf s k = 0 := by
  intro k hk h1
  simp only [MASK_COUNT] at hk
  have ha : ((0x1000000 : BitVec 64) + BitVec.ofNat 64 (8 * k + MASK_DISP)).toNat =
      0x1000000 + 8 * k + 33556480 := by
    rw [BitVec.toNat_add, show (0x1000000 : BitVec 64).toNat = 0x1000000 from rfl, BitVec.toNat_ofNat,
      MASK_DISP, Nat.mod_eq_of_lt (a := 8 * k + 33556480) (by omega), Nat.mod_eq_of_lt (by omega)]
    omega
  simp only [Spec.CEntry.Ctx.maskOf, mem_c hA, sym_c hA, hsl, hm]
  rw [mkMem_readW_clear_le (by decide) ?_ (by rw [ha]; omega)]
  · rfl
  · intro bv hbv
    simp only [masksClear, List.all_eq_true, Bool.or_eq_true, decide_eq_true_eq, beq_iff_eq] at h
    rw [ha]
    have := bv.1.isLt
    rcases h bv hbv with (h' | h') | h' <;> omega

/-- Every slab word of class 1 but slab 0's is zero. -/
theorem words_zero (h : slabsClear ws = true) {i : Nat} (h0 : 0 < i) (hi : i < SLAB_COUNT) :
    c.wordOf s 1 i = 0 := by
  simp only [SLAB_COUNT] at hi
  have ha : (wordAddr 0x1000000 i 1).toNat = 0x1000000 + i * 4096 + 8 := by
    simp only [wordAddr, SLAB_BYTES, SLAB_SHIFT]
    rw [BitVec.toNat_add, show (0x1000000 : BitVec 64).toNat = 0x1000000 from rfl, BitVec.toNat_ofNat,
      Nat.mod_eq_of_lt (by omega), Nat.mod_eq_of_lt (by omega)]
    omega
  simp only [Spec.CEntry.Ctx.wordOf, mem_c hA, sym_c hA, hsl, hm]
  apply mkMem_readW_clear
  · intro bv hbv
    simp only [slabsClear, List.all_eq_true, Bool.or_eq_true, decide_eq_true_eq] at h
    rw [ha]
    rcases h bv hbv with h' | h' <;> omega
  · rw [ha]; omega

/-- The contract's precondition, in such a state. -/
theorem pre_of_check {post : State → State → Prop} {exit : String → State → State → Prop}
    {abort : State → State → Prop} {trace : State → List Event → Prop}
    {vals : State → List (BitVec 64)}
    (hv : c.A.args ((c.sig.words c.A.ptrBits).map (·.bits c.A.ptrBits)) = some vals)
    (hb : ∀ x, Sig.bufs c.sig.params (vals x) = [])
    (ha : ∀ x, c.A.argArea ((c.sig.words c.A.ptrBits).map (·.bits c.A.ptrBits)) x = [])
    (h : witCheck c s ws = true) : (contract c post exit abort trace).pre s := by
  simp only [witCheck, Bool.and_eq_true, decide_eq_true_eq, Bool.or_eq_true, Bool.not_eq_true',
    decide_eq_false_iff_not] at h
  obtain ⟨⟨⟨⟨⟨⟨⟨⟨⟨⟨hrd, hwr⟩, hnw⟩, hp⟩, hres⟩, hl⟩, hmask⟩, hword⟩, hap⟩, hm1⟩, hm2⟩ := h
  have hwf : c.A.wf ((c.sig.words c.A.ptrBits).map (·.bits c.A.ptrBits)) 0 s := by
    rw [hA.wf]; trivial
  have hmk : MaskOk (c.maskOf s 1) := ⟨2, by decide, by decide, hmask⟩
  have htp : c.A.tp.isSome := by rw [hA.tp]; rfl
  have hsy : c.A.sym.isSome := by rw [hA.sym]; rfl
  refine (pre_iff c hv (hb s) (ha s)).mpr ⟨hwf, ?_, htp, hsy, layout_of_check hA.ptrBits hl,
    inv_one (masks_zero hA hm hsl hm1) hmk (fun i h0 hi => words_zero hA hm hsl hm2 h0 hi) ?_⟩
  · exact world_of_check hA.ptrBits (by rw [hA.rd]; exact hrd) (by rw [hA.wr]; exact hwr)
      (by simpa only [List.all_eq_true, decide_eq_true_eq] using hnw) hp
      (by simpa only [List.all_eq_true] using hres)
  · refine ⟨hword, fun hs => ?_⟩
    rcases hap with hn | hc
    · exact absurd hs hn
    · exact apart_of_check hA.ptrBits hc

end

end VG.Proof.CEntry.AArch64
