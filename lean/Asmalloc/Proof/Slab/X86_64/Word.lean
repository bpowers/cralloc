module

public import Asmalloc.Proof.Slab.X86_64.Mid

/-!
# The pop's and push's slab word on x86-64

`mov rcx, [rdi + r8]`: the class's word in the CPU's slab, `rdi` holding the
address of the class's word in slab 0 and `r8` the CPU's slab's offset; and,
out of line, the class's mask, the 32 bits at `[rdi + 33556480]`, past the slabs. Nothing here
names the slabs: the address in `rdi` is the caller's, which the contract
ties to `SLABS` and the class (`Ctx.argsOk`).
-/

@[expose] public section

namespace VG.Proof.Slab.X86_64

open VG.X86_64 VG.Spec.Slab VG.Impl.Slab.X86_64

/-- The slab word's address, and the word. -/
def wa (s : State) : Addr := s.gpr .rdi + sh s
def wv (s : State) : BitVec 64 := s.mem.readW (wa s) 64
/-- The class's mask's address, and the mask, out of line. -/
def maskA (s : State) : Addr := s.gpr .rdi + 33556480
def mk (s : State) : BitVec 32 := s.mem.readW (maskA s) 32

/-- After loading the slab word. -/
def σ4 (s : State) : State := (σ3 s).setReg .rcx (wv s)

section

variable {c : Spec.Slab.Ctx isa} {s : State}

/-- A register the arm and the CPU number's steps leave alone. -/
theorem gpr_σ3 {r : Reg} (h8 : ¬ r = .r8) (h10 : ¬ r = .r10) (h11 : ¬ r = .r11) :
    (σ3 s).gpr r = s.gpr r := by
  simp only [σ3, shl12, RegUpd.gpr_setReg_of_ne _ _ h8, RegUpd.gpr_setFlags, σ2,
    RegUpd.gpr_arithFlags, σ1, State.setReg32, σA, armReg, armB, armA,
    RegUpd.gpr_setReg_of_ne _ _ h11, RegUpd.gpr_setReg_of_ne _ _ h10]

theorem σ3_rdi : (σ3 s).gpr .rdi = s.gpr .rdi := gpr_σ3 (by decide) (by decide) (by decide)

theorem σ4_rd : (σ4 s).rd = s.rd := rfl
theorem σ4_wr : (σ4 s).wr = s.wr := rfl
theorem σ4_mem : (σ4 s).mem = m1 s := rfl
theorem σ4_rcx : (σ4 s).gpr .rcx = wv s := RegUpd.gpr_setReg_self _ _ _
theorem gpr_σ4 {r : Reg} (hc : ¬ r = .rcx) (h8 : ¬ r = .r8) (h10 : ¬ r = .r10) (h11 : ¬ r = .r11) :
    (σ4 s).gpr r = s.gpr r := by
  rw [σ4, RegUpd.gpr_setReg_of_ne _ _ hc, gpr_σ3 h8 h10 h11]
theorem σ4_rdi : (σ4 s).gpr .rdi = s.gpr .rdi := gpr_σ4 (by decide) (by decide) (by decide) (by decide)
theorem σ4_r8 : (σ4 s).gpr .r8 = sh s := by
  rw [σ4, RegUpd.gpr_setReg_of_ne _ _ (by decide), gpr_σ3_r8]

/-- `[rdi + r8]`, in any state where they hold the class's word in slab 0
and the CPU's slab's offset. -/
theorem wordAt_ea (σ : State) (hdi : σ.gpr .rdi = s.gpr .rdi) (h8 : σ.gpr .r8 = sh s) :
    σ.ea (wordAt .rdi) = wa s := by
  rw [wordAt, Access.ea_of_seg_none rfl, Access.offset_index, hdi, h8]
  simp only [BitVec.ofInt_ofNat, BitVec.mul_one, BitVec.add_zero]
  rfl

/-- `[rdi + 33556480]`, in any state where `rdi` is as on entry. -/
theorem maskAt_ea (σ : State) (hdi : σ.gpr .rdi = s.gpr .rdi) :
    σ.ea { base := .rdi, disp := 33556480 } = maskA s := by
  rw [Access.ea_base_disp, BitVec.ofInt_ofNat, hdi]; rfl

/-- The word address the caller gives is `SLABS` plus 8 times the class. -/
theorem rdi_eq (hA : X86 c.A) (h0 : c.arg s 0 = s.gpr .rdi)
    (hoff : (c.arg s 0 - c.sym s SLABS).toNat = 8 * c.cls s) :
    s.gpr .rdi = s.syms SLABS + BitVec.ofNat 64 (8 * c.cls s) := by
  rw [h0, sym_of hA] at hoff
  have hsub : s.gpr .rdi - s.syms SLABS = BitVec.ofNat 64 (8 * c.cls s) := by
    apply BitVec.eq_of_toNat_eq
    rw [BitVec.toNat_ofNat, ← hoff, Nat.mod_eq_of_lt (BitVec.isLt _)]
  rw [← hsub, BitVec.add_comm, BitVec.sub_add_cancel]

/-- The slab word's address is the class's for the CPU. -/
theorem wa_eq (hA : X86 c.A) (h0 : c.arg s 0 = s.gpr .rdi)
    (hoff : (c.arg s 0 - c.sym s SLABS).toNat = 8 * c.cls s) : wa s = c.word s := by
  rw [Spec.Slab.Ctx.word, wordAddr, sym_of hA, cpu_of hA, wa, sh_eq, rdi_eq hA h0 hoff, cpu32,
    BitVec.ofNat_add, BitVec.add_assoc, BitVec.add_comm (BitVec.ofNat 64 (8 * c.cls s))]

/-- The mask's address is the routine's, out of line. -/
theorem maskA_eq (h0 : c.arg s 0 = s.gpr .rdi) : maskA s = c.maskAddr s := by
  rw [Spec.Slab.Ctx.maskAddr, h0]; rfl

theorem exec_wordAt (h : Facts c s) (hi : c.InRange s) (hw : wa s = c.word s) :
    exec (.mov .rcx (.mem (wordAt .rdi))) (σ3 s) = some (σ4 s) := by
  have hin : InRegions ((σ3 s).rd ++ (σ3 s).wr) ((σ3 s).ea (wordAt .rdi)) 8 := by
    rw [wordAt_ea _ σ3_rdi gpr_σ3_r8, σ3_rd, σ3_wr, hw]
    exact inRdWr (r := ⟨c.word s, 8⟩) (h.word hi)
  rw [exec_load _ hin, wordAt_ea _ σ3_rdi gpr_σ3_r8, σ3_mem, m1,
    readW64_writeW_disj (by rw [hw]; exact (h.cs_word hi).symm)]
  rfl

theorem ev_wordAt (h : Facts c s) (hi : c.InRange s) (hw : wa s = c.word s) :
    Instr.events (.mov .rcx (.mem (wordAt .rdi))) (σ3 s) (σ4 s) =
      [⟨wa s, 8, false, (wv s).toNat⟩] := by
  simp only [Instr.events, wordAt_ea _ σ3_rdi gpr_σ3_r8, σ3_mem, toNat_read8, m1]
  rw [readW64_writeW_disj (by rw [hw]; exact (h.cs_word hi).symm)]; rfl

/-- The mask, read from memory after the arm. -/
theorem m1_mask (h : Facts c s) (hf : c.form = .table) (hm : maskA s = c.maskAddr s) :
    (m1 s).readW (maskA s) 32 = mk s := by
  rw [m1, readW32_writeW_disj (by rw [hm]; exact (h.cs_mask hf).symm)]; rfl

theorem inMask (h : Facts c s) (hf : c.form = .table) (hm : maskA s = c.maskAddr s) (σ : State)
    (hrd : σ.rd = s.rd) (hwr : σ.wr = s.wr) : InRegions (σ.rd ++ σ.wr) (maskA s) 4 := by
  rw [hrd, hwr, hm]; exact inRd (r := ⟨c.maskAddr s, 4⟩) (h.mask hf)

/-- The test of a word with the mask from memory, `test r32, [rdi + 33556480]`,
in a state `σ` reached from `s` with memory `m1 s` and `rdi` as on entry. -/
theorem exec_test_mask (h : Facts c s) (hf : c.form = .table) (hm : maskA s = c.maskAddr s)
    {σ : State} (d : Reg) (hrd : σ.rd = s.rd) (hwr : σ.wr = s.wr) (hmem : σ.mem = m1 s)
    (hdi : σ.gpr .rdi = s.gpr .rdi) :
    exec (.alu32 .test d (maskAt .rdi)) σ =
      some (arithFlags σ ((σ.gpr d).setWidth 32 &&& mk s) false false) := by
  have hin := inMask h hf hm σ hrd hwr
  rw [← maskAt_ea σ hdi] at hin
  rw [maskAt, exec_test32_mem _ hin, maskAt_ea σ hdi, hmem, m1_mask h hf hm]

theorem ev_test_mask (h : Facts c s) (hf : c.form = .table) (hm : maskA s = c.maskAddr s)
    {σ σ' : State} (d : Reg) (hmem : σ.mem = m1 s) (hdi : σ.gpr .rdi = s.gpr .rdi) :
    Instr.events (.alu32 .test d (maskAt .rdi)) σ σ' = [⟨maskA s, 4, false, (mk s).toNat⟩] := by
  simp only [Instr.events, maskAt, maskAt_ea σ hdi, hmem, toNat_read4, m1_mask h hf hm]

theorem mσ4 {rs : List Reg} (h3 : Mid rs s (m1 s) (σ3 s)) (hc : .rcx ∈ rs) :
    Mid rs s (m1 s) (σ4 s) := h3.setReg hc _

end

end VG.Proof.Slab.X86_64
