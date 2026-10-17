module

public import Asmalloc.Impl.Slab.X86_64
public import Asmalloc.Proof.Slab.X86_64.Facts
public import Asmalloc.Proof.Framework.Body
public import Asmalloc.Proof.Framework.X86_64.Access
public import Asmalloc.Proof.Framework.X86_64.RegUpd
public import Asmalloc.Proof.Framework.X86_64.Flags
public import Asmalloc.Proof.Framework.Offset

/-!
# The routines' common steps on x86-64

The arm, and the cid's load, compare and shift: every routine starts with
them. Each step is `exec i σ = some σ'` for named states, so that the folds
over a sequence's body (`Proof/Framework/Body.lean`) unfold by rewriting.
-/

@[expose] public section

namespace VG.Proof.Slab.X86_64

open VG.X86_64 VG.Spec.Slab VG.Impl.Slab.X86_64

/-! ## One instruction -/

section Steps

variable (σ : State)

theorem exec_movGot (d : Reg) (n : String) : exec (.movGot d n) σ = some (σ.setReg d (σ.syms n)) :=
  rfl

theorem exec_load {d : Reg} {m : MemOp} (h : InRegions (σ.rd ++ σ.wr) (σ.ea m) 8) :
    exec (.mov d (.mem m)) σ = some (σ.setReg d (σ.mem.readW (σ.ea m) 64)) := by
  simp only [exec, readSrc, Access.load64_eq h, Option.map_some]

theorem exec_leaRseqCs (d : Reg) : exec (.leaRseqCs d) σ = some (σ.setReg d σ.csDesc) := rfl

theorem exec_store {m : MemOp} {r : Reg} (h : InRegions σ.wr (σ.ea m) 8) :
    exec (.store m r) σ = some { σ with mem := σ.mem.writeW (σ.ea m) (σ.gpr r) } := by
  simp only [exec, Access.store64_eq h]

theorem exec_load32 {d : Reg} {m : MemOp} (h : InRegions (σ.rd ++ σ.wr) (σ.ea m) 4) :
    exec (.mov32 d (.mem m)) σ = some (σ.setReg32 d (σ.mem.readW (σ.ea m) 32)) :=
  Access.exec_mov32_mem h

theorem exec_cmp32_imm (d : Reg) (v : BitVec 32) :
    exec (.alu32 .cmp d (.imm v)) σ = some (arithFlags σ ((σ.gpr d).setWidth 32 - v)
      (decide (((σ.gpr d).setWidth 32).toNat < v.toNat))
      (subOverflow ((σ.gpr d).setWidth 32) v ((σ.gpr d).setWidth 32 - v))) := rfl

/-- `shl r, 12`. -/
def shl12 (d : Reg) : State :=
  (σ.setFlags (some ((σ.gpr d).getLsbD 52)) none (some ((σ.gpr d <<< 12) == 0))
    (some (σ.gpr d <<< 12).msb)).setReg d (σ.gpr d <<< 12)

theorem exec_shl (d : Reg) : exec (.shift .shl d 12) σ = some (shl12 σ d) := rfl

theorem exec_leaSym (d : Reg) (n : String) (off : Int) (offP : Option String) :
    exec (.leaSym d n off offP) σ =
      some (σ.setReg d (σ.syms n + BitVec.ofInt 64 off + ((offP.map σ.params).getD 0))) := rfl

/-- `lea r, [rip + n]`. -/
theorem exec_leaSym0 (d : Reg) (n : String) :
    exec (.leaSym d n) σ = some (σ.setReg d (σ.syms n)) := by
  rw [exec_leaSym]; simp

theorem exec_lea (d : Reg) (m : MemOp) : exec (.lea d m) σ = some (σ.setReg d (σ.offset m)) := rfl

theorem exec_test32_immP (d : Reg) (n : String) :
    exec (.alu32 .test d (.immP n)) σ =
      some (arithFlags σ ((σ.gpr d).setWidth 32 &&& (σ.params n).setWidth 32) false false) := rfl

theorem exec_test32_mem {d : Reg} {m : MemOp} (h : InRegions (σ.rd ++ σ.wr) (σ.ea m) 4) :
    exec (.alu32 .test d (.mem m)) σ =
      some (arithFlags σ ((σ.gpr d).setWidth 32 &&& σ.mem.readW (σ.ea m) 32) false false) := by
  simp only [exec, execAlu32, readSrc32, Access.load32_eq h, Option.bind_some]

theorem exec_mov32_imm (d : Reg) (v : BitVec 32) : exec (.mov32 d (.imm v)) σ = some (σ.setReg32 d v) :=
  rfl

end Steps

/-! ## Words in memory -/

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

/-! ## The arm -/

/-- The rseq area's offset from the FS base. -/
def o (s : State) : Addr := s.mem.readW (s.syms RS) 64

theorem area_eq (s : State) : area s = s.fsBase + o s := rfl

/-- Memory after arming: `rseq_cs` holds the descriptor. -/
def m1 (s : State) : Mem := s.mem.writeW (area s + 8) s.csDesc

/-- The registers after each of the arm's first three instructions. -/
def armA (s : State) : State := s.setReg .rdx (s.syms RS)
def armB (s : State) : State := (armA s).setReg .rdx (o s)
def armReg (s : State) : State := (armB s).setReg .r10 s.csDesc

/-- The state after the arm, where the sequence starts. -/
def σA (s : State) : State := { armReg s with mem := m1 s }

theorem inRd {s : State} {r : Region} (h : r ∈ s.rd) : InRegions (s.rd ++ s.wr) r.base r.len :=
  (InRegions.of_mem h (Region.contains_self _ _)).append_left

theorem inWr {s : State} {r : Region} (h : r ∈ s.wr) : InRegions s.wr r.base r.len :=
  InRegions.of_mem h (Region.contains_self _ _)

theorem inRdWr {s : State} {r : Region} (h : r ∈ s.wr) : InRegions (s.rd ++ s.wr) r.base r.len :=
  (inWr h).append_right

theorem csDesc_setReg (s : State) (r : Reg) (v : BitVec 64) : (s.setReg r v).csDesc = s.csDesc := rfl

/-- `fs:[rdx + d]`. -/
theorem ea_fs_rdx (σ : State) (d : Nat) :
    σ.ea { base := .rdx, disp := (d : Int), seg := some .fs } = σ.fsBase + σ.gpr .rdx + BitVec.ofNat 64 d := by
  rw [Access.ea_fs_base_disp, BitVec.ofInt_natCast, BitVec.add_assoc]

section Arm

variable {c : Spec.Slab.Ctx isa} {s : State}

theorem armReg_rd : (armReg s).rd = s.rd := rfl
theorem armReg_wr : (armReg s).wr = s.wr := rfl
theorem armReg_mem : (armReg s).mem = s.mem := rfl
theorem armReg_r10 : (armReg s).gpr .r10 = s.csDesc := RegUpd.gpr_setReg_self _ _ _
theorem σA_rd : (σA s).rd = s.rd := rfl
theorem σA_wr : (σA s).wr = s.wr := rfl
theorem σA_mem : (σA s).mem = m1 s := rfl

theorem gpr_armReg (r : Reg) : (armReg s).gpr r =
    if r = .r10 then s.csDesc else if r = .rdx then o s else s.gpr r := by
  simp only [armReg, armB, armA, RegUpd.gpr_setReg]
  by_cases h : r = .r10 <;> by_cases h' : r = .rdx <;> simp [h, h']

theorem armReg_rdx : (armReg s).gpr .rdx = o s := by simp [gpr_armReg]

theorem csOp_ea : (armReg s).ea { base := .rdx, disp := 8, seg := some .fs } = area s + 8 := by
  rw [show (8 : Int) = ((8 : Nat) : Int) from rfl, ea_fs_rdx, armReg_rdx]; rfl

theorem exec_arm1 : exec (.movGot .rdx RS) s = some (armA s) := rfl

theorem exec_arm2 (h : Facts c s) :
    exec (.mov .rdx (.mem { base := .rdx })) (armA s) = some (armB s) := by
  have h1 : InRegions ((armA s).rd ++ (armA s).wr) ((armA s).ea { base := .rdx }) 8 := by
    rw [Access.ea_base, armA, RegUpd.gpr_setReg_self, RegUpd.rd_setReg, RegUpd.wr_setReg]
    exact inRd (r := ⟨s.syms RS, 8⟩) h.rsVar
  rw [exec_load _ h1, Access.ea_base, armA, RegUpd.gpr_setReg_self, RegUpd.mem_setReg]
  rfl

theorem exec_arm3 : exec (.leaRseqCs .r10) (armB s) = some (armReg s) := rfl

theorem exec_arm4 (h : Facts c s) :
    exec (.store { base := .rdx, disp := 8, seg := some .fs } .r10) (armReg s) = some (σA s) := by
  have h2 : InRegions (armReg s).wr ((armReg s).ea { base := .rdx, disp := 8, seg := some .fs }) 8 := by
    rw [csOp_ea, armReg_wr]; exact inWr (r := ⟨area s + 8, 8⟩) h.cs
  rw [exec_store (r := .r10) _ h2, csOp_ea, armReg_mem, armReg_r10]
  rfl

theorem execBlock_dropLast_arm (h : Facts c s) : execBlock isa arm.dropLast s = some (armReg s) := by
  rw [show arm.dropLast = [.movGot .rdx RS, .mov .rdx (.mem { base := .rdx }), .leaRseqCs .r10] from rfl,
    execBlock_cons_some exec_arm1, execBlock_cons_some (exec_arm2 h),
    execBlock_cons_some exec_arm3, execBlock_nil']

theorem execBlock_arm (h : Facts c s) : execBlock isa arm s = some (σA s) := by
  rw [arm, execBlock_cons_some exec_arm1, execBlock_cons_some (exec_arm2 h),
    execBlock_cons_some exec_arm3, execBlock_cons_some (exec_arm4 h), execBlock_nil']

/-- The arm's last instruction points `rseq_cs` at the descriptor. -/
theorem arms_arm : arms (.store { base := .rdx, disp := 8, seg := some .fs } .r10) (armReg s) :=
  ⟨by rw [csOp_ea]; rfl, armReg_r10⟩

theorem events_arm (h : Facts c s) :
    blockEvents isa arm s =
      [⟨s.syms RS, 8, false, (o s).toNat⟩, ⟨area s + 8, 8, true, s.csDesc.toNat⟩] := by
  rw [arm, blockEvents_cons_some exec_arm1, blockEvents_cons_some (exec_arm2 h),
    blockEvents_cons_some exec_arm3, blockEvents_cons_some (exec_arm4 h), blockEvents_nil']
  simp only [Instr.events, List.nil_append, List.append_nil, List.cons_append, Access.ea_base,
    csOp_ea, toNat_read8]
  rw [show (armA s).gpr .rdx = s.syms RS from RegUpd.gpr_setReg_self _ _ _,
    show (armA s).mem = s.mem from rfl, σA_mem, m1, Mem.readW_writeW_self64]
  rfl

end Arm

/-! ## The cid -/

/-- The cid, as the routine reads it. -/
def cid32 (s : State) : BitVec 32 := s.mem.readW (area s + 24) 32

/-- After loading the cid. -/
def σ1 (s : State) : State := (σA s).setReg32 .r8 (cid32 s)

/-- After comparing it with `SLAB_COUNT`. -/
def σ2 (s : State) : State :=
  arithFlags (σ1 s) (cid32 s - 8192) (decide ((cid32 s).toNat < (8192 : BitVec 32).toNat))
    (subOverflow (cid32 s) 8192 (cid32 s - 8192))

/-- After the shift: `r8` is the cid's slab's offset in `SLABS`. -/
def σ3 (s : State) : State := shl12 (σ2 s) .r8

/-- The cid's slab's offset. -/
def sh (s : State) : BitVec 64 := (cid32 s).setWidth 64 <<< 12

theorem cidOp_ea (s : State) :
    (σA s).ea { base := .rdx, disp := 24, seg := some .fs } = area s + 24 := by
  rw [show (24 : Int) = ((24 : Nat) : Int) from rfl, ea_fs_rdx]
  show s.fsBase + (armReg s).gpr .rdx + _ = _
  rw [armReg_rdx]; rfl

/-- The cid field and `rseq_cs` are apart. -/
theorem cid_cs_disjoint (s : State) : Region.Disjoint ⟨area s + 24, 32 / 8⟩ ⟨area s + 8, 8⟩ :=
  Offset.disjoint (area s) (d := 24) (n := 4) (e := 8) (k := 8) (by omega) (by omega) (by omega)

section Cid

variable {c : Spec.Slab.Ctx isa} {s : State}

theorem exec_cid (h : Facts c s) :
    exec (.mov32 .r8 (.mem { base := .rdx, disp := 24, seg := some .fs })) (σA s) = some (σ1 s) := by
  have hin : InRegions ((σA s).rd ++ (σA s).wr) ((σA s).ea { base := .rdx, disp := 24, seg := some .fs }) 4 := by
    rw [cidOp_ea, σA_rd, σA_wr]; exact inRd (r := ⟨area s + 24, 4⟩) h.cidF
  have hm : (σA s).mem.readW (area s + 24) 32 = cid32 s := by
    rw [σA_mem, m1, readW_writeW_disj (cid_cs_disjoint s) (by decide)]
    rfl
  rw [exec_load32 _ hin, cidOp_ea, hm]
  rfl

theorem gpr_σ1_r8 : (σ1 s).gpr .r8 = (cid32 s).setWidth 64 := by
  simp [σ1, State.setReg32, RegUpd.gpr_setReg_self]

theorem exec_cmp : exec (.alu32 .cmp .r8 (.imm 8192)) (σ1 s) = some (σ2 s) := by
  rw [exec_cmp32_imm, gpr_σ1_r8, RegUpd.setWidth_setWidth_32]; rfl

theorem eval_cid : eval .ae (σ2 s) = some (!decide ((cid32 s).toNat < 8192)) := rfl

theorem exec_shl_r8 : exec (.shift .shl .r8 12) (σ2 s) = some (σ3 s) := exec_shl _ _

theorem σ3_syms : (σ3 s).syms = s.syms := rfl
theorem σ3_params : (σ3 s).params = s.params := rfl
theorem σ3_mem : (σ3 s).mem = m1 s := rfl
theorem σ3_rd : (σ3 s).rd = s.rd := rfl
theorem σ3_wr : (σ3 s).wr = s.wr := rfl

theorem gpr_σ3_r8 : (σ3 s).gpr .r8 = sh s := by
  simp only [σ3, shl12, RegUpd.gpr_setReg_self, σ2, RegUpd.gpr_arithFlags, gpr_σ1_r8]; rfl

/-- The cid's slab's offset, as a number. -/
theorem sh_eq : sh s = BitVec.ofNat 64 ((cid32 s).toNat * SLAB_BYTES) := by
  apply BitVec.eq_of_toNat_eq
  have := (cid32 s).isLt
  simp only [sh, BitVec.toNat_shiftLeft, BitVec.toNat_setWidth, Nat.shiftLeft_eq, BitVec.toNat_ofNat,
    SLAB_BYTES, SLAB_SHIFT]
  rw [Nat.mod_eq_of_lt (show (cid32 s).toNat < 2 ^ 64 by omega),
    Nat.mod_eq_of_lt (show (cid32 s).toNat * 2 ^ 12 < 2 ^ 64 by omega)]

end Cid

end VG.Proof.Slab.X86_64
