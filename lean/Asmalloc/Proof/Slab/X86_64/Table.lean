module

public import Asmalloc.Proof.Slab.X86_64.Mid

/-!
# The out-of-line routines' slab word and mask on x86-64

`lea r9, [rip + SLABS]`, `lea r9, [r9 + rdi*8]`, `mov rcx, [r9 + r8]` and
`lea r11, [rip + MAG_MASKS]`: the class's word in the CPU's slab, the class
being in `rdi`, and its mask, `[r11 + rdi*4]`.
-/

@[expose] public section

namespace VG.Proof.Slab.X86_64

open VG.X86_64 VG.Spec.Slab VG.Impl.Slab.X86_64

/-- The class's word in slab 0. -/
def r9t (s : State) : Addr := s.syms "SLABS" + s.gpr .rdi * 8
/-- The slab word's address, and the word. -/
def wat (s : State) : Addr := r9t s + sh s
def wvt (s : State) : BitVec 64 := s.mem.readW (wat s) 64
/-- The class's mask's address, and the mask. -/
def maskA (s : State) : Addr := s.syms "MAG_MASKS" + s.gpr .rdi * 4
def mk (s : State) : BitVec 32 := s.mem.readW (maskA s) 32

def σ4t (s : State) : State := (σ3 s).setReg .r9 (s.syms "SLABS")
def σ5t (s : State) : State := (σ4t s).setReg .r9 (r9t s)
def σ6t (s : State) : State := (σ5t s).setReg .rcx (wvt s)
def σ7t (s : State) : State := (σ6t s).setReg .r11 (s.syms "MAG_MASKS")

section

variable {c : Spec.Slab.Ctx isa} {s : State}

theorem gpr_σ3_rdi : (σ3 s).gpr .rdi = s.gpr .rdi := by
  simp only [σ3, shl12, RegUpd.gpr_setReg_of_ne _ _ (show ¬ Reg.rdi = .r8 by decide),
    RegUpd.gpr_setFlags, σ2, RegUpd.gpr_arithFlags, σ1, State.setReg32, σA, armReg, armB, armA,
    RegUpd.gpr_setReg_of_ne _ _ (show ¬ Reg.rdi = .r11 by decide),
    RegUpd.gpr_setReg_of_ne _ _ (show ¬ Reg.rdi = .r10 by decide)]

theorem gpr_σ3_rsi : (σ3 s).gpr .rsi = s.gpr .rsi := by
  simp only [σ3, shl12, RegUpd.gpr_setReg_of_ne _ _ (show ¬ Reg.rsi = .r8 by decide),
    RegUpd.gpr_setFlags, σ2, RegUpd.gpr_arithFlags, σ1, State.setReg32, σA, armReg, armB, armA,
    RegUpd.gpr_setReg_of_ne _ _ (show ¬ Reg.rsi = .r11 by decide),
    RegUpd.gpr_setReg_of_ne _ _ (show ¬ Reg.rsi = .r10 by decide)]

theorem σ4t_rdi : (σ4t s).gpr .rdi = s.gpr .rdi := by
  rw [σ4t, RegUpd.gpr_setReg_of_ne _ _ (by decide), gpr_σ3_rdi]
theorem σ5t_rdi : (σ5t s).gpr .rdi = s.gpr .rdi := by
  rw [σ5t, RegUpd.gpr_setReg_of_ne _ _ (by decide), σ4t_rdi]
theorem σ7t_rdi : (σ7t s).gpr .rdi = s.gpr .rdi := by
  rw [σ7t, RegUpd.gpr_setReg_of_ne _ _ (by decide), σ6t, RegUpd.gpr_setReg_of_ne _ _ (by decide),
    σ5t_rdi]
theorem σ7t_rsi : (σ7t s).gpr .rsi = s.gpr .rsi := by
  simp only [σ7t, σ6t, σ5t, σ4t, RegUpd.gpr_setReg_of_ne _ _ (show ¬ Reg.rsi = .r11 by decide),
    RegUpd.gpr_setReg_of_ne _ _ (show ¬ Reg.rsi = .rcx by decide),
    RegUpd.gpr_setReg_of_ne _ _ (show ¬ Reg.rsi = .r9 by decide), gpr_σ3_rsi]

theorem σ4t_rd : (σ4t s).rd = s.rd := rfl
theorem σ4t_wr : (σ4t s).wr = s.wr := rfl
theorem σ5t_rd : (σ5t s).rd = s.rd := rfl
theorem σ5t_wr : (σ5t s).wr = s.wr := rfl
theorem σ5t_mem : (σ5t s).mem = m1 s := rfl
theorem σ7t_rd : (σ7t s).rd = s.rd := rfl
theorem σ7t_wr : (σ7t s).wr = s.wr := rfl
theorem σ7t_mem : (σ7t s).mem = m1 s := rfl
theorem σ7t_rcx : (σ7t s).gpr .rcx = wvt s := by
  rw [σ7t, RegUpd.gpr_setReg_of_ne _ _ (by decide), σ6t, RegUpd.gpr_setReg_self]
theorem σ7t_r11 : (σ7t s).gpr .r11 = s.syms "MAG_MASKS" := RegUpd.gpr_setReg_self _ _ _

theorem exec_r9t : exec (.leaSym .r9 "SLABS") (σ3 s) = some (σ4t s) := by
  rw [exec_leaSym0, σ3_syms]; rfl

theorem exec_r9idx : exec (.lea .r9 { base := .r9, index := some .rdi, scale := 8 }) (σ4t s) =
    some (σ5t s) := by
  rw [exec_lea, Access.offset_index, σ4t_rdi, σ4t, RegUpd.gpr_setReg_self]
  simp only [BitVec.ofInt_ofNat, BitVec.add_zero]
  rfl

theorem σ5t_r9 : (σ5t s).gpr .r9 = r9t s := RegUpd.gpr_setReg_self _ _ _
theorem σ5t_r8 : (σ5t s).gpr .r8 = sh s := by
  rw [σ5t, RegUpd.gpr_setReg_of_ne _ _ (by decide), σ4t, RegUpd.gpr_setReg_of_ne _ _ (by decide),
    gpr_σ3_r8]

/-- `[r9 + r8]`, in any state where they hold the class's word in slab 0 and
the CPU's slab's offset. -/
theorem slabWord_ea_t (σ : State) (h9 : σ.gpr .r9 = r9t s) (h8 : σ.gpr .r8 = sh s) :
    σ.ea slabWord = wat s := by
  rw [slabWord, Access.ea_of_seg_none rfl, Access.offset_index, h9, h8]
  simp only [BitVec.ofInt_ofNat, BitVec.mul_one, BitVec.add_zero]
  rfl

/-- The slab word's address is the class's for the CPU, an out-of-line
routine being given its class in `rdi`. -/
theorem wat_eq (hA : c.A = abi) (hcls : c.cls s = (s.gpr .rdi).toNat) : wat s = c.word s := by
  rw [Spec.Slab.Ctx.word, wordAddr, sym_of hA, cpu_of hA, wat, r9t, sh_eq, cpu32, hcls]
  apply BitVec.eq_of_toNat_eq
  simp only [BitVec.toNat_add, BitVec.toNat_mul, BitVec.toNat_ofNat, SLABS, toNat_eight]
  omega

/-- The mask's address is the routine's. -/
theorem maskA_eq (hA : c.A = abi) (hcls : c.cls s = (s.gpr .rdi).toNat) : maskA s = c.maskAddr s := by
  rw [Spec.Slab.Ctx.maskAddr, sym_of hA, hcls, maskA]
  apply BitVec.eq_of_toNat_eq
  simp only [BitVec.toNat_add, BitVec.toNat_mul, BitVec.toNat_ofNat, MAG_MASKS,
    show (4 : BitVec 64).toNat = 4 from rfl]
  omega

theorem exec_wordt (h : Facts c s) (hi : c.InRange s) (hw : wat s = c.word s) :
    exec (.mov .rcx (.mem slabWord)) (σ5t s) = some (σ6t s) := by
  have hin : InRegions ((σ5t s).rd ++ (σ5t s).wr) ((σ5t s).ea slabWord) 8 := by
    rw [slabWord_ea_t _ σ5t_r9 σ5t_r8, σ5t_rd, σ5t_wr, hw]
    exact inRdWr (r := ⟨c.word s, 8⟩) (h.word hi)
  rw [exec_load _ hin, slabWord_ea_t _ σ5t_r9 σ5t_r8, σ5t_mem, m1,
    readW64_writeW_disj (by rw [hw]; exact (h.cs_word hi).symm)]
  rfl

theorem exec_r11 : exec (.leaSym .r11 "MAG_MASKS") (σ6t s) = some (σ7t s) := by
  rw [exec_leaSym0]; rfl

theorem maskEntry_ea (σ : State) (h11 : σ.gpr .r11 = s.syms "MAG_MASKS") (hdi : σ.gpr .rdi = s.gpr .rdi) :
    σ.ea { base := .r11, index := some .rdi, scale := 4 } = maskA s := by
  rw [Access.ea_of_seg_none rfl, Access.offset_index, h11, hdi]
  simp only [BitVec.ofInt_ofNat, BitVec.add_zero]
  rfl

/-- The mask, read from memory after the arm. -/
theorem m1_mask (h : Facts c s) (hf : c.form = .table) (hm : maskA s = c.maskAddr s) :
    (m1 s).readW (maskA s) 32 = mk s := by
  rw [m1, readW32_writeW_disj (by rw [hm]; exact (h.cs_mask hf).symm)]; rfl

theorem inMask (h : Facts c s) (hf : c.form = .table) (hm : maskA s = c.maskAddr s) (σ : State)
    (hrd : σ.rd = s.rd) (hwr : σ.wr = s.wr) : InRegions (σ.rd ++ σ.wr) (maskA s) 4 := by
  rw [hrd, hwr, hm]; exact inRd (r := ⟨c.maskAddr s, 4⟩) (h.mask hf)

theorem mσ7t {rs : List Reg} (h3 : Mid rs s (m1 s) (σ3 s)) (h9 : .r9 ∈ rs) (hc : .rcx ∈ rs)
    (h11 : .r11 ∈ rs) : Mid rs s (m1 s) (σ4t s) ∧ Mid rs s (m1 s) (σ5t s) ∧ Mid rs s (m1 s) (σ6t s) ∧
      Mid rs s (m1 s) (σ7t s) :=
  have h4 := h3.setReg h9 _
  have h5 := h4.setReg h9 _
  have h6 := h5.setReg hc _
  ⟨h4, h5, h6, h6.setReg h11 _⟩

end

end VG.Proof.Slab.X86_64
