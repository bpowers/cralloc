module

public import Cralloc.Proof.CEntry.AArch64.Steps

/-!
# The frees' region index on AArch64

`Impl.CEntry.AArch64.regionIndex`, run from any state `ρ` with `x14` holding the page past the slabs:
the shift (32 bits, into `w6`), the base index (into `x7`), the pointer in `x0` shifted right, the base
index subtracted, and the compare with 4096 that leaves for the slow path. Its states and steps, and,
from a C routine's precondition, that what it computes is the contract's region index
(`Ctx.regionIndex`).
-/

@[expose] public section

namespace VG.Proof.CEntry.AArch64.RegionIx

open VG.AArch64 VG.Spec.Slab VG.Spec.CEntry VG.Impl.Slab.AArch64 VG.Impl.CEntry.AArch64
  VG.Proof.Slab.AArch64 VG.Proof.CEntry VG.Proof.CEntry.AArch64 VG.Proof.Slab

variable (ρ : State)

/-- The mirror's words' addresses and values. -/
def sa : Addr := ρ.gpr .x14 + BitVec.ofInt 64 1024
def ba : Addr := ρ.gpr .x14 + BitVec.ofInt 64 1032
def sv : BitVec 32 := ρ.mem.readW (sa ρ) 32
def bw : BitVec 64 := ρ.mem.readW (ba ρ) 64

def r1 : State := ρ.setReg .x6 ((sv ρ).setWidth 64)
def r2 : State := (r1 ρ).setReg .x7 (bw ρ)
/-- The shifted pointer, and the region index. -/
def sp : BitVec 64 := ρ.gpr .x0 >>> ((sv ρ).toNat % 64)
def r3 : State := (r2 ρ).setReg .x4 (sp ρ)
def idx : BitVec 64 := sp ρ - bw ρ
def r4 : State := (r3 ρ).setReg .x4 (idx ρ)
def r5 : State := (r4 ρ).addFlags (idx ρ) (~~~(BitVec.ofNat 64 4096)) true

variable {ρ}

theorem r1_gpr {r : Reg} (h6 : ¬ r = .x6) : (r1 ρ).gpr r = ρ.gpr r := RegUpd.gpr_setReg_of_ne _ _ h6

theorem r2_gpr {r : Reg} (h6 : ¬ r = .x6) (h7 : ¬ r = .x7) : (r2 ρ).gpr r = ρ.gpr r := by
  rw [r2, RegUpd.gpr_setReg_of_ne _ _ h7, r1_gpr h6]

theorem r5_gpr {r : Reg} (h4 : ¬ r = .x4) (h6 : ¬ r = .x6) (h7 : ¬ r = .x7) : (r5 ρ).gpr r = ρ.gpr r := by
  rw [r5, RegUpd.gpr_addFlags, r4, RegUpd.gpr_setReg_of_ne _ _ h4, r3, RegUpd.gpr_setReg_of_ne _ _ h4,
    r2_gpr h6 h7]

theorem r5_x4 : (r5 ρ).gpr .x4 = idx ρ := by
  rw [r5, RegUpd.gpr_addFlags, r4, RegUpd.gpr_setReg_self]

theorem exec_r1 (h : InRegions (ρ.rd ++ ρ.wr) (sa ρ) 4) :
    exec (.ldr .w .x6 { base := .x14, off := .imm 1024 }) ρ = some (r1 ρ) := by
  have e : ρ.ea { base := .x14, off := .imm 1024 } = sa ρ := rfl
  rw [Access.exec_ldr_w (by rw [e]; exact h), e]
  rfl

theorem ev_r1 : Instr.events (.ldr .w .x6 { base := .x14, off := .imm 1024 }) ρ (r1 ρ) =
    [⟨sa ρ, 4, false, (sv ρ).toNat⟩] := by
  have e : ρ.ea { base := .x14, off := .imm 1024 } = sa ρ := rfl
  simp only [Instr.events, e, Width.bytes, toNat_read4]
  rfl

theorem r1_sa : (r1 ρ).ea { base := .x14, off := .imm 1032 } = ba ρ := by
  rw [Access.ea_imm, r1_gpr (by decide)]
  rfl

theorem exec_r2 (h : InRegions (ρ.rd ++ ρ.wr) (ba ρ) 8) :
    exec (.ldr .x .x7 { base := .x14, off := .imm 1032 }) (r1 ρ) = some (r2 ρ) := by
  rw [Access.exec_ldr_x (by rw [r1_sa]; exact h), r1_sa]
  rfl

theorem ev_r2 : Instr.events (.ldr .x .x7 { base := .x14, off := .imm 1032 }) (r1 ρ) (r2 ρ) =
    [⟨ba ρ, 8, false, (bw ρ).toNat⟩] := by
  simp only [Instr.events, r1_sa, Width.bytes, toNat_read8]
  rfl

theorem exec_r3 : exec (.lsrv .x .x4 .x0 .x6) (r2 ρ) = some (r3 ρ) := by
  show some ((r2 ρ).setReg .x4 ((r2 ρ).gpr .x0 >>> (((r2 ρ).gpr .x6).toNat % 64))) = _
  have h6 : (r2 ρ).gpr .x6 = (sv ρ).setWidth 64 := by
    rw [r2, RegUpd.gpr_setReg_of_ne _ _ (by decide), r1, RegUpd.gpr_setReg_self]
  have hn : ((sv ρ).setWidth 64).toNat = (sv ρ).toNat := by
    have := (sv ρ).isLt
    simp only [BitVec.toNat_setWidth]
    omega
  rw [r2_gpr (by decide) (by decide), h6, hn]
  rfl

theorem exec_r4 : exec (.sub .x .x4 .x4 (.reg .x7)) (r3 ρ) = some (r4 ρ) := by
  show some ((r3 ρ).setReg .x4 ((r3 ρ).gpr .x4 - (r3 ρ).gpr .x7)) = _
  have h7 : (r3 ρ).gpr .x7 = bw ρ := by
    rw [r3, RegUpd.gpr_setReg_of_ne _ _ (by decide), r2, RegUpd.gpr_setReg_self]
  rw [show (r3 ρ).gpr .x4 = sp ρ from RegUpd.gpr_setReg_self _ _ _, h7]
  rfl

theorem exec_r5 : exec (.cmp .x .x4 (.imm 4096)) (r4 ρ) = some (r5 ρ) := by
  show some ((r4 ρ).addFlags ((r4 ρ).gpr .x4) (~~~(BitVec.ofNat 64 4096)) true) = _
  rw [show (r4 ρ).gpr .x4 = idx ρ from RegUpd.gpr_setReg_self _ _ _]
  rfl

theorem eval_r5 : eval .hs (r5 ρ) = some (decide (4096 ≤ (idx ρ).toNat)) :=
  Flags.eval_hs_cmp _ _ _

theorem eval_r5_t (h : ¬ (idx ρ).toNat < 4096) : eval .hs (r5 ρ) = some true := by
  rw [eval_r5]; simp only [Option.some.injEq, decide_eq_true_eq]; omega

theorem eval_r5_f (h : (idx ρ).toNat < 4096) : eval .hs (r5 ρ) = some false := by
  rw [eval_r5]; simp only [Option.some.injEq, decide_eq_false_iff_not]; omega

/-- An index in range. -/
theorem regionT (hs : InRegions (ρ.rd ++ ρ.wr) (sa ρ) 4) (hb : InRegions (ρ.rd ++ ρ.wr) (ba ρ) 8)
    (h : (idx ρ).toNat < 4096) :
    Through isa regionIndex ρ (r5 ρ) [ρ, r1 ρ, r2 ρ, r3 ρ, r4 ρ, r5 ρ]
      [⟨sa ρ, 4, false, (sv ρ).toNat⟩, ⟨ba ρ, 8, false, (bw ρ).toNat⟩] :=
  .mem (exec_r1 hs) ev_r1 <| .mem (exec_r2 hb) ev_r2 <| .reg exec_r3 rfl <| .reg exec_r4 rfl <|
    .reg exec_r5 rfl <| .skip (eval_r5_f h) <| .nil _

/-- An index out of range. -/
theorem regionX (hs : InRegions (ρ.rd ++ ρ.wr) (sa ρ) 4) (hb : InRegions (ρ.rd ++ ρ.wr) (ba ρ) 8)
    (h : ¬ (idx ρ).toNat < 4096) :
    Leaves isa regionIndex ρ (.exit "slow" (r5 ρ)) [ρ, r1 ρ, r2 ρ, r3 ρ, r4 ρ, r5 ρ]
      [⟨sa ρ, 4, false, (sv ρ).toNat⟩, ⟨ba ρ, 8, false, (bw ρ).toNat⟩] 5 :=
  .mem (exec_r1 hs) ev_r1 <| .mem (exec_r2 hb) ev_r2 <| .reg exec_r3 rfl <| .reg exec_r4 rfl <|
    .reg exec_r5 rfl <| .take (eval_r5_t h)

/-! ## What the steps keep -/

section Mids

variable {rs : List Reg} {s : State} {μ : Mem}

theorem mid_r1 (h : Mid rs s μ ρ) (h6 : .x6 ∈ rs) : Mid rs s μ (r1 ρ) := h.setReg h6 _
theorem mid_r2 (h : Mid rs s μ ρ) (h6 : .x6 ∈ rs) (h7 : .x7 ∈ rs) : Mid rs s μ (r2 ρ) :=
  (mid_r1 h h6).setReg h7 _
theorem mid_r3 (h : Mid rs s μ ρ) (h6 : .x6 ∈ rs) (h7 : .x7 ∈ rs) (h4 : .x4 ∈ rs) : Mid rs s μ (r3 ρ) :=
  (mid_r2 h h6 h7).setReg h4 _
theorem mid_r4 (h : Mid rs s μ ρ) (h6 : .x6 ∈ rs) (h7 : .x7 ∈ rs) (h4 : .x4 ∈ rs) : Mid rs s μ (r4 ρ) :=
  (mid_r3 h h6 h7 h4).setReg h4 _
theorem mid_r5 (h : Mid rs s μ ρ) (h6 : .x6 ∈ rs) (h7 : .x7 ∈ rs) (h4 : .x4 ∈ rs) : Mid rs s μ (r5 ρ) :=
  (mid_r4 h h6 h7 h4).addFlags _ _ _

end Mids

/-! ## Against the contract -/

/-- A state the frees reach before the region index: memory as after the arm, the entry's regions,
the page past the slabs in `x14` and the pointer in `x0`. -/
structure Before (s ρ : State) : Prop where
  mem : ρ.mem = m1 s
  rd : ρ.rd = s.rd
  wr : ρ.wr = s.wr
  x14 : ρ.gpr .x14 = s.syms SLABS + BitVec.ofNat 64 PAST_SLABS
  x0 : ρ.gpr .x0 = s.gpr .x0

section Spec

variable {regs : List Reg} {c : Spec.CEntry.Ctx isa} {s : State}

/-- The published words are readable and apart from `rseq_cs`. -/
structure PubFacts (c : Spec.CEntry.Ctx isa) (s : State) : Prop where
  rd : c.published s ∈ s.rd
  cs : Region.Disjoint ⟨area s + 8, 8⟩ (c.published s)

theorem pubFacts (hA : A64 c.A regs) (hw : c.World s) (hl : c.Layout s)
    (hm : (c.published s, false) ∈ tables c s) : PubFacts c s := by
  have hd : (c.csField s).Disjoint (c.published s) :=
    cs_table hl (x := (c.published s, false)) (List.mem_cons_of_mem _ hm)
  rw [csField_c hA] at hd
  exact ⟨mem_rd hA hw (x := (c.published s, false)) (mem_fixed (table_mem_fixed hm)) rfl, hd⟩

theorem pub_eq : c.published s = ⟨(c.published s).base, 16⟩ := rfl

theorem pub_contains0 : (c.published s).Contains (c.published s).base 4 := by
  rw [pub_eq]; simp only [Region.Contains, BitVec.sub_self, BitVec.toNat_zero]; omega

theorem pub_contains8 : (c.published s).Contains ((c.published s).base + 8) 8 := by
  rw [pub_eq]
  exact Offset.contains_base (c.published s).base (d := 8) (n := 8) (k := 16) (by omega) (by omega)

theorem pub_sub0 : Region.Sub ⟨(c.published s).base, 4⟩ (c.published s) := sub_of_contains pub_contains0
theorem pub_sub8 : Region.Sub ⟨(c.published s).base + 8, 8⟩ (c.published s) := sub_of_contains pub_contains8

variable {ρ : State} (hb : Before s ρ) (hA : A64 c.A regs) (hmir : c.mirror = .c)
include hb hA hmir

theorem sa_eq : sa ρ = (c.published s).base := by
  rw [sa, hb.x14, Spec.CEntry.Ctx.published, sym_c hA, hmir, BitVec.add_assoc]
  rfl

theorem ba_eq : ba ρ = (c.published s).base + 8 := by
  rw [ba, hb.x14, Spec.CEntry.Ctx.published, sym_c hA, hmir, BitVec.add_assoc, BitVec.add_assoc]
  rfl

variable (hp : PubFacts c s)
include hp

theorem sa_in : InRegions (ρ.rd ++ ρ.wr) (sa ρ) 4 := by
  rw [hb.rd, hb.wr, sa_eq hb hA hmir]
  exact (InRegions.of_mem hp.rd pub_contains0).append_left

theorem ba_in : InRegions (ρ.rd ++ ρ.wr) (ba ρ) 8 := by
  rw [hb.rd, hb.wr, ba_eq hb hA hmir]
  exact (InRegions.of_mem hp.rd pub_contains8).append_left

theorem sv_eq : sv ρ = s.mem.readW (c.published s).base 32 := by
  rw [sv, sa_eq hb hA hmir, hb.mem, m1, readW32_writeW_disj (hp.cs.symm.sub_left pub_sub0)]

theorem bw_eq : bw ρ = s.mem.readW ((c.published s).base + 8) 64 := by
  rw [bw, ba_eq hb hA hmir, hb.mem, m1, readW64_writeW_disj (hp.cs.symm.sub_left pub_sub8)]

theorem idx_eq (hptr : c.ptr s = s.gpr .x0) : idx ρ = c.regionIndex s := by
  rw [idx, sp, sv_eq hb hA hmir hp, bw_eq hb hA hmir hp, hb.x0, Spec.CEntry.Ctx.regionIndex,
    Spec.CEntry.Ctx.shift, Spec.CEntry.Ctx.baseIndex, mem_c hA, hptr]

end Spec

end VG.Proof.CEntry.AArch64.RegionIx
