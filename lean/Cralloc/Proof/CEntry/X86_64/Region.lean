module

public import Cralloc.Proof.CEntry.X86_64.Back
public import Cralloc.Impl.CEntry.X86_64

/-!
# The frees' region index on x86-64

`Impl.CEntry.X86_64.regionIndex`, run from any state `ρ` with `r9` holding
`SLABS`: the shift (32 bits, into `ecx`), `rdi` shifted right by `cl`, the
base index subtracted, and the compare with 4096 that leaves for the slow
path. Its states, its steps and their accesses, under what it needs of
`ρ`'s regions; and, from a C routine's precondition, that what it computes
is the contract's region index (`Ctx.regionIndex`).
-/

@[expose] public section

namespace VG.Proof.CEntry.X86_64.RegionIx

open VG.X86_64 VG.Spec.Slab VG.Spec.CEntry VG.Impl.Slab.X86_64 VG.Impl.CEntry.X86_64
  VG.Proof.Slab.X86_64 VG.Proof.CEntry

variable (m : Mirror) (ρ : State)

/-- The mirror's words' addresses and values. -/
def sa : Addr := ρ.gpr .r9 + BitVec.ofNat 64 m.disp
def ba : Addr := ρ.gpr .r9 + BitVec.ofNat 64 (m.disp + 8)
def sv : BitVec 32 := ρ.mem.readW (sa m ρ) 32
def bw : BitVec 64 := ρ.mem.readW (ba m ρ) 64

def r1 : State := ρ.setReg32 .rcx (sv m ρ)
def r2 : State := (r1 m ρ).setReg .rax (ρ.gpr .rdi)
def r3 : State := execShiftCl .shr .rax (r2 m ρ)
/-- The shifted pointer, and the region index. -/
def sp : BitVec 64 := (r3 m ρ).gpr .rax
def idx : BitVec 64 := sp m ρ - bw m ρ
def r4 : State :=
  (arithFlags (r3 m ρ) (idx m ρ) (decide ((sp m ρ).toNat < (bw m ρ).toNat))
    (subOverflow (sp m ρ) (bw m ρ) (idx m ρ))).setReg .rax (idx m ρ)
def lim : BitVec 64 := (4096 : BitVec 32).signExtend 64
def r5 : State :=
  arithFlags (r4 m ρ) (idx m ρ - lim) (decide ((idx m ρ).toNat < lim.toNat))
    (subOverflow (idx m ρ) lim (idx m ρ - lim))

variable {m ρ}

theorem ofInt_disp (n : Nat) : BitVec.ofInt 64 (n : Int) = BitVec.ofNat 64 n := by
  apply BitVec.eq_of_toNat_eq; simp

theorem sa_ea : ρ.ea { base := .r9, disp := m.disp } = sa m ρ := by
  rw [Access.ea_base_disp, ofInt_disp]; rfl

theorem r1_r9 : (r1 m ρ).gpr .r9 = ρ.gpr .r9 := by
  simp only [r1, State.setReg32, RegUpd.gpr_setReg_of_ne _ _ (show Reg.r9 ≠ .rcx by decide)]

theorem r3_r9 : (r3 m ρ).gpr .r9 = ρ.gpr .r9 := by
  simp only [r3, execShiftCl, RegUpd.gpr_setReg_of_ne _ _ (show Reg.r9 ≠ .rax by decide),
    RegUpd.gpr_setFlags, r2, r1_r9]

theorem r3_mem : (r3 m ρ).mem = ρ.mem := rfl
theorem r3_rd : (r3 m ρ).rd = ρ.rd := rfl
theorem r3_wr : (r3 m ρ).wr = ρ.wr := rfl

theorem ba_ea : (r3 m ρ).ea { base := .r9, disp := m.disp + 8 } = ba m ρ := by
  rw [Access.ea_base_disp, show ((m.disp : Int) + 8) = ((m.disp + 8 : Nat) : Int) by omega,
    ofInt_disp, r3_r9]; rfl

theorem exec_r1 (h : InRegions (ρ.rd ++ ρ.wr) (sa m ρ) 4) :
    exec (.mov32 .rcx (.mem { base := .r9, disp := m.disp })) ρ = some (r1 m ρ) := by
  rw [exec_load32 _ (by rw [sa_ea]; exact h), sa_ea]; rfl

theorem ev_r1 : Instr.events (.mov32 .rcx (.mem { base := .r9, disp := m.disp })) ρ (r1 m ρ) =
    [⟨sa m ρ, 4, false, (sv m ρ).toNat⟩] := by
  simp only [Instr.events, sa_ea, toNat_read4]; rfl

theorem exec_r2 : exec (.mov .rax (.reg .rdi)) (r1 m ρ) = some (r2 m ρ) := by
  simp only [exec, readSrc, Option.map_some, r2, r1, State.setReg32,
    RegUpd.gpr_setReg_of_ne _ _ (show Reg.rdi ≠ .rcx by decide)]

theorem exec_r3 : exec (.shiftCl .shr .rax) (r2 m ρ) = some (r3 m ρ) := rfl

theorem sp_eq : sp m ρ = ρ.gpr .rdi >>> ((sv m ρ).toNat % 64) := by
  simp only [sp, r3, execShiftCl, RegUpd.gpr_setReg_self, VarShiftOp.eval]
  simp only [r2, RegUpd.gpr_setReg_self, RegUpd.gpr_setReg_of_ne _ _ (show Reg.rcx ≠ .rax by decide),
    r1, State.setReg32, BitVec.toNat_setWidth]
  rw [Nat.mod_eq_of_lt (a := (sv m ρ).toNat) (by have := (sv m ρ).isLt; omega)]

theorem exec_r4 (h : InRegions (ρ.rd ++ ρ.wr) (ba m ρ) 8) :
    exec (.alu .sub .rax (.mem { base := .r9, disp := m.disp + 8 })) (r3 m ρ) = some (r4 m ρ) := by
  simp only [exec, execAlu, readSrc, ba_ea, Access.load64_eq (s := r3 m ρ) (by rw [r3_rd, r3_wr]; exact h),
    Option.bind_some, r3_mem]
  rfl

theorem ev_r4 : Instr.events (.alu .sub .rax (.mem { base := .r9, disp := m.disp + 8 })) (r3 m ρ) (r4 m ρ) =
    [⟨ba m ρ, 8, false, (bw m ρ).toNat⟩] := by
  simp only [Instr.events, ba_ea, toNat_read8, r3_mem]; rfl

theorem r4_rax : (r4 m ρ).gpr .rax = idx m ρ := RegUpd.gpr_setReg_self _ _ _

theorem exec_r5 : exec (.alu .cmp .rax (.imm 4096)) (r4 m ρ) = some (r5 m ρ) := by
  simp only [exec, execAlu, readSrc, Option.bind_some, r4_rax]; rfl

theorem lim_toNat : lim.toNat = 4096 := by decide

theorem eval_r5 : eval .ae (r5 m ρ) = some (!decide ((idx m ρ).toNat < 4096)) := by
  show some (!decide ((idx m ρ).toNat < lim.toNat)) = _
  rw [lim_toNat]

theorem eval_r5_t (h : ¬ (idx m ρ).toNat < 4096) : eval .ae (r5 m ρ) = some true := by
  rw [eval_r5]; simp [h]

theorem eval_r5_f (h : (idx m ρ).toNat < 4096) : eval .ae (r5 m ρ) = some false := by
  rw [eval_r5]; simp [h]

theorem r5_gpr {r : Reg} (ha : r ≠ .rax) (hc : r ≠ .rcx) : (r5 m ρ).gpr r = ρ.gpr r := by
  simp only [r5, r4, r3, execShiftCl, r2, r1, State.setReg32, RegUpd.gpr_arithFlags,
    RegUpd.gpr_setReg_of_ne _ _ ha, RegUpd.gpr_setFlags, RegUpd.gpr_setReg_of_ne _ _ hc]

theorem r5_rax : (r5 m ρ).gpr .rax = idx m ρ := by rw [r5, RegUpd.gpr_arithFlags, r4_rax]
theorem r5_mem : (r5 m ρ).mem = ρ.mem := rfl
theorem r5_rd : (r5 m ρ).rd = ρ.rd := rfl
theorem r5_wr : (r5 m ρ).wr = ρ.wr := rfl
theorem r5_syms : (r5 m ρ).syms = ρ.syms := rfl

/-! ## What the steps keep -/

section Mids

variable {rs : List Reg} {s : State} {μ : Mem}

theorem mid_r1 (h : Mid rs s μ ρ) (hc : .rcx ∈ rs) : Mid rs s μ (r1 m ρ) := h.setReg32 hc _
theorem mid_r2 (h : Mid rs s μ ρ) (hc : .rcx ∈ rs) (ha : .rax ∈ rs) : Mid rs s μ (r2 m ρ) :=
  (mid_r1 h hc).setReg ha _
theorem mid_r3 (h : Mid rs s μ ρ) (hc : .rcx ∈ rs) (ha : .rax ∈ rs) : Mid rs s μ (r3 m ρ) :=
  ((mid_r2 h hc ha).setFlags _ _ _ _).setReg ha _
theorem mid_r4 (h : Mid rs s μ ρ) (hc : .rcx ∈ rs) (ha : .rax ∈ rs) : Mid rs s μ (r4 m ρ) :=
  ((mid_r3 h hc ha).arithFlags _ _ _).setReg ha _
theorem mid_r5 (h : Mid rs s μ ρ) (hc : .rcx ∈ rs) (ha : .rax ∈ rs) : Mid rs s μ (r5 m ρ) :=
  (mid_r4 h hc ha).arithFlags _ _ _

end Mids

/-! ## Against the contract -/

/-- A state the frees reach before the region index: memory as after the
arm, the entry's regions, `SLABS` in `r9` and the pointer in `rdi`. -/
structure Before (s ρ : State) : Prop where
  mem : ρ.mem = m1 s
  rd : ρ.rd = s.rd
  wr : ρ.wr = s.wr
  r9 : ρ.gpr .r9 = s.syms SLABS
  rdi : ρ.gpr .rdi = s.gpr .rdi

section Spec

variable {c : Spec.CEntry.Ctx isa} {s : State}

/-- The published words are readable and apart from `rseq_cs`. -/
structure PubFacts (c : Spec.CEntry.Ctx isa) (s : State) : Prop where
  rd : c.published s ∈ s.rd
  cs : Region.Disjoint ⟨area s + 8, 8⟩ (c.published s)

theorem pubFacts (hA : X86 c.A) (hw : c.World s) (hl : c.Layout s)
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

variable {ρ : State}

theorem sa_eq (hb : Before s ρ) (hA : X86 c.A) : sa c.mirror ρ = (c.published s).base := by
  rw [sa, hb.r9, Spec.CEntry.Ctx.published, sym_c hA]

theorem ba_eq (hb : Before s ρ) (hA : X86 c.A) : ba c.mirror ρ = (c.published s).base + 8 := by
  rw [ba, hb.r9, Spec.CEntry.Ctx.published, sym_c hA, BitVec.add_assoc, BitVec.ofNat_add]; rfl

variable (hb : Before s ρ) (hA : X86 c.A) (hp : PubFacts c s)
include hA hb hp

theorem sa_in : InRegions (ρ.rd ++ ρ.wr) (sa c.mirror ρ) 4 := by
  rw [hb.rd, hb.wr, sa_eq hb hA]
  exact (InRegions.of_mem hp.rd pub_contains0).append_left

theorem ba_in : InRegions (ρ.rd ++ ρ.wr) (ba c.mirror ρ) 8 := by
  rw [hb.rd, hb.wr, ba_eq hb hA]
  exact (InRegions.of_mem hp.rd pub_contains8).append_left

theorem sv_eq : sv c.mirror ρ = s.mem.readW (c.published s).base 32 := by
  rw [sv, sa_eq hb hA, hb.mem, m1, readW32_writeW_disj (hp.cs.symm.sub_left pub_sub0)]

theorem bw_eq : bw c.mirror ρ = s.mem.readW ((c.published s).base + 8) 64 := by
  rw [bw, ba_eq hb hA, hb.mem, m1, readW64_writeW_disj (hp.cs.symm.sub_left pub_sub8)]

theorem idx_eq (hptr : c.ptr s = s.gpr .rdi) : idx c.mirror ρ = c.regionIndex s := by
  rw [idx, sp_eq, sv_eq hb hA hp, bw_eq hb hA hp, hb.rdi, Spec.CEntry.Ctx.regionIndex,
    Spec.CEntry.Ctx.shift, Spec.CEntry.Ctx.baseIndex, mem_c hA, hptr]

end Spec

end VG.Proof.CEntry.X86_64.RegionIx
