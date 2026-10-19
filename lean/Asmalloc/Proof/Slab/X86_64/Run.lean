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

The arm, and the CPU's load, compare and shift: every routine starts with
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

theorem exec_test32_reg (d r : Reg) :
    exec (.alu32 .test d (.reg r)) σ =
      some (arithFlags σ ((σ.gpr d).setWidth 32 &&& (σ.gpr r).setWidth 32) false false) := rfl

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
def armA (s : State) : State := s.setReg .r10 (s.syms RS)
def armB (s : State) : State := (armA s).setReg .r10 (o s)
def armReg (s : State) : State := (armB s).setReg .r11 s.csDesc

/-- The state after the arm, where the sequence starts. -/
def σA (s : State) : State := { armReg s with mem := m1 s }

theorem inRd {s : State} {r : Region} (h : r ∈ s.rd) : InRegions (s.rd ++ s.wr) r.base r.len :=
  (InRegions.of_mem h (Region.contains_self _ _)).append_left

theorem inWr {s : State} {r : Region} (h : r ∈ s.wr) : InRegions s.wr r.base r.len :=
  InRegions.of_mem h (Region.contains_self _ _)

theorem inRdWr {s : State} {r : Region} (h : r ∈ s.wr) : InRegions (s.rd ++ s.wr) r.base r.len :=
  (inWr h).append_right

theorem csDesc_setReg (s : State) (r : Reg) (v : BitVec 64) : (s.setReg r v).csDesc = s.csDesc := rfl

/-- `fs:[r10 + d]`. -/
theorem ea_fs_r10 (σ : State) (d : Nat) :
    σ.ea { base := .r10, disp := (d : Int), seg := some .fs } = σ.fsBase + σ.gpr .r10 + BitVec.ofNat 64 d := by
  rw [Access.ea_fs_base_disp, BitVec.ofInt_natCast, BitVec.add_assoc]

section Arm

variable {c : Spec.Slab.Ctx isa} {s : State}

theorem armReg_rd : (armReg s).rd = s.rd := rfl
theorem armReg_wr : (armReg s).wr = s.wr := rfl
theorem armReg_mem : (armReg s).mem = s.mem := rfl
theorem armReg_desc : (armReg s).gpr .r11 = s.csDesc := RegUpd.gpr_setReg_self _ _ _
theorem σA_rd : (σA s).rd = s.rd := rfl
theorem σA_wr : (σA s).wr = s.wr := rfl
theorem σA_mem : (σA s).mem = m1 s := rfl

theorem gpr_armReg (r : Reg) : (armReg s).gpr r =
    if r = .r11 then s.csDesc else if r = .r10 then o s else s.gpr r := by
  simp only [armReg, armB, armA, RegUpd.gpr_setReg]
  by_cases h : r = .r11 <;> by_cases h' : r = .r10 <;> simp [h, h']

theorem armReg_base : (armReg s).gpr .r10 = o s := by simp [gpr_armReg]

theorem csOp_ea : (armReg s).ea { base := .r10, disp := 8, seg := some .fs } = area s + 8 := by
  rw [show (8 : Int) = ((8 : Nat) : Int) from rfl, ea_fs_r10, armReg_base]; rfl

theorem exec_arm1 : exec (.movGot .r10 RS) s = some (armA s) := rfl

theorem exec_arm2 (h : ArmOk s) :
    exec (.mov .r10 (.mem { base := .r10 })) (armA s) = some (armB s) := by
  have h1 : InRegions ((armA s).rd ++ (armA s).wr) ((armA s).ea { base := .r10 }) 8 := by
    rw [Access.ea_base, armA, RegUpd.gpr_setReg_self, RegUpd.rd_setReg, RegUpd.wr_setReg]
    exact inRd (r := ⟨s.syms RS, 8⟩) h.rsVar
  rw [exec_load _ h1, Access.ea_base, armA, RegUpd.gpr_setReg_self, RegUpd.mem_setReg]
  rfl

theorem exec_arm3 : exec (.leaRseqCs .r11) (armB s) = some (armReg s) := rfl

theorem exec_arm4 (h : ArmOk s) :
    exec (.store { base := .r10, disp := 8, seg := some .fs } .r11) (armReg s) = some (σA s) := by
  have h2 : InRegions (armReg s).wr ((armReg s).ea { base := .r10, disp := 8, seg := some .fs }) 8 := by
    rw [csOp_ea, armReg_wr]; exact inWr (r := ⟨area s + 8, 8⟩) h.cs
  rw [exec_store (r := .r11) _ h2, csOp_ea, armReg_mem, armReg_desc]
  rfl

theorem execBlock_dropLast_arm (h : ArmOk s) : execBlock isa arm.dropLast s = some (armReg s) := by
  rw [show arm.dropLast = [.movGot .r10 RS, .mov .r10 (.mem { base := .r10 }), .leaRseqCs .r11] from rfl,
    execBlock_cons_some exec_arm1, execBlock_cons_some (exec_arm2 h),
    execBlock_cons_some exec_arm3, execBlock_nil']

theorem execBlock_arm (h : ArmOk s) : execBlock isa arm s = some (σA s) := by
  rw [arm, execBlock_cons_some exec_arm1, execBlock_cons_some (exec_arm2 h),
    execBlock_cons_some exec_arm3, execBlock_cons_some (exec_arm4 h), execBlock_nil']

/-- The arm's last instruction points `rseq_cs` at the descriptor. -/
theorem arms_arm : arms (.store { base := .r10, disp := 8, seg := some .fs } .r11) (armReg s) :=
  ⟨by rw [csOp_ea]; rfl, armReg_desc⟩

theorem events_arm (h : ArmOk s) :
    blockEvents isa arm s =
      [⟨s.syms RS, 8, false, (o s).toNat⟩, ⟨area s + 8, 8, true, s.csDesc.toNat⟩] := by
  rw [arm, blockEvents_cons_some exec_arm1, blockEvents_cons_some (exec_arm2 h),
    blockEvents_cons_some exec_arm3, blockEvents_cons_some (exec_arm4 h), blockEvents_nil']
  simp only [Instr.events, List.nil_append, List.append_nil, List.cons_append, Access.ea_base,
    csOp_ea, toNat_read8]
  rw [show (armA s).gpr .r10 = s.syms RS from RegUpd.gpr_setReg_self _ _ _,
    show (armA s).mem = s.mem from rfl, σA_mem, m1, Mem.readW_writeW_self64]
  rfl

/-! ### Armed with `__rseq_offset` read directly

The same states and accesses as `arm`'s: `movSym` loads the variable that
`movGot` and the load after it reach through the word holding its address. -/

theorem setReg_setReg_same (σ : State) (r : Reg) (a b : BitVec 64) :
    (σ.setReg r a).setReg r b = σ.setReg r b := by
  simp only [State.setReg]
  congr 1
  funext r'
  by_cases h : r' = r <;> simp [h]

theorem armB_eq : armB s = s.setReg .r10 (o s) := setReg_setReg_same _ _ _ _

theorem exec_direct1 (h : ArmOk s) : exec (.movSym .r10 RS) s = some (armB s) := by
  have hin : InRegions (s.rd ++ s.wr) (s.syms RS) 8 := inRd (r := ⟨s.syms RS, 8⟩) h.rsVar
  simp only [exec, State.load64, hin, ↓reduceIte, Option.map_some, armB_eq]
  rfl

theorem execBlock_dropLast_armDirect (h : ArmOk s) :
    execBlock isa armDirect.dropLast s = some (armReg s) := by
  rw [show armDirect.dropLast = [.movSym .r10 RS, .leaRseqCs .r11] from rfl,
    execBlock_cons_some (exec_direct1 h), execBlock_cons_some exec_arm3, execBlock_nil']

theorem execBlock_armDirect (h : ArmOk s) : execBlock isa armDirect s = some (σA s) := by
  rw [armDirect, execBlock_cons_some (exec_direct1 h), execBlock_cons_some exec_arm3,
    execBlock_cons_some (exec_arm4 h), execBlock_nil']

theorem events_armDirect (h : ArmOk s) : blockEvents isa armDirect s = blockEvents isa arm s := by
  rw [events_arm h, armDirect, blockEvents_cons_some (exec_direct1 h),
    blockEvents_cons_some exec_arm3, blockEvents_cons_some (exec_arm4 h), blockEvents_nil']
  simp only [Instr.events, List.nil_append, List.append_nil, List.cons_append, csOp_ea, toNat_read8]
  rw [σA_mem, m1, Mem.readW_writeW_self64]
  rfl

/-- `armDirect` does what `arm` does, and arms the sequence as `RseqOk` asks. -/
theorem armDirect_same (h : ArmOk s) :
    execBlock isa armDirect s = execBlock isa arm s ∧
    blockEvents isa armDirect s = blockEvents isa arm s ∧
    ∃ i a, armDirect.getLast? = some i ∧ execBlock isa armDirect.dropLast s = some a ∧ arms i a :=
  ⟨by rw [execBlock_armDirect h, execBlock_arm h], events_armDirect h,
    _, _, rfl, execBlock_dropLast_armDirect h, arms_arm⟩

end Arm

/-! ## The CPU number -/

/-- The CPU number, as the routine reads it. -/
def cpu32 (s : State) : BitVec 32 := s.mem.readW (area s + 4) 32

/-- After loading the CPU. -/
def σ1 (s : State) : State := (σA s).setReg32 .r8 (cpu32 s)

/-- After comparing it with `SLAB_COUNT`. -/
def σ2 (s : State) : State :=
  arithFlags (σ1 s) (cpu32 s - 8192) (decide ((cpu32 s).toNat < (8192 : BitVec 32).toNat))
    (subOverflow (cpu32 s) 8192 (cpu32 s - 8192))

/-- After the shift: `r8` is the CPU's slab's offset in `SLABS`. -/
def σ3 (s : State) : State := shl12 (σ2 s) .r8

/-- The CPU's slab's offset. -/
def sh (s : State) : BitVec 64 := (cpu32 s).setWidth 64 <<< 12

theorem cpuOp_ea (s : State) :
    (σA s).ea { base := .r10, disp := 4, seg := some .fs } = area s + 4 := by
  rw [show (4 : Int) = ((4 : Nat) : Int) from rfl, ea_fs_r10]
  show s.fsBase + (armReg s).gpr .r10 + _ = _
  rw [armReg_base]; rfl

/-- `cpu_id` and `rseq_cs` are apart. -/
theorem cpu_cs_disjoint (s : State) : Region.Disjoint ⟨area s + 4, 32 / 8⟩ ⟨area s + 8, 8⟩ :=
  Offset.disjoint (area s) (d := 4) (n := 4) (e := 8) (k := 8) (by omega) (by omega) (by omega)

section Cpu

variable {c : Spec.Slab.Ctx isa} {s : State}

theorem exec_cpu (h : ArmOk s) :
    exec (.mov32 .r8 (.mem { base := .r10, disp := 4, seg := some .fs })) (σA s) = some (σ1 s) := by
  have hin : InRegions ((σA s).rd ++ (σA s).wr) ((σA s).ea { base := .r10, disp := 4, seg := some .fs }) 4 := by
    rw [cpuOp_ea, σA_rd, σA_wr]; exact inRd (r := ⟨area s + 4, 4⟩) h.cpuF
  have hm : (σA s).mem.readW (area s + 4) 32 = cpu32 s := by
    rw [σA_mem, m1, readW_writeW_disj (cpu_cs_disjoint s) (by decide)]
    rfl
  rw [exec_load32 _ hin, cpuOp_ea, hm]
  rfl

theorem gpr_σ1_r8 : (σ1 s).gpr .r8 = (cpu32 s).setWidth 64 := by
  simp [σ1, State.setReg32, RegUpd.gpr_setReg_self]

theorem exec_cmp : exec (.alu32 .cmp .r8 (.imm 8192)) (σ1 s) = some (σ2 s) := by
  rw [exec_cmp32_imm, gpr_σ1_r8, RegUpd.setWidth_setWidth_32]; rfl

theorem eval_cpu : eval .ae (σ2 s) = some (!decide ((cpu32 s).toNat < 8192)) := rfl

theorem exec_shl_r8 : exec (.shift .shl .r8 12) (σ2 s) = some (σ3 s) := exec_shl _ _

theorem σ3_syms : (σ3 s).syms = s.syms := rfl
theorem σ3_params : (σ3 s).params = s.params := rfl
theorem σ3_mem : (σ3 s).mem = m1 s := rfl
theorem σ3_rd : (σ3 s).rd = s.rd := rfl
theorem σ3_wr : (σ3 s).wr = s.wr := rfl

theorem gpr_σ3_r8 : (σ3 s).gpr .r8 = sh s := by
  simp only [σ3, shl12, RegUpd.gpr_setReg_self, σ2, RegUpd.gpr_arithFlags, gpr_σ1_r8]; rfl

/-- The CPU's slab's offset, as a number. -/
theorem sh_eq : sh s = BitVec.ofNat 64 ((cpu32 s).toNat * SLAB_BYTES) := by
  apply BitVec.eq_of_toNat_eq
  have := (cpu32 s).isLt
  simp only [sh, BitVec.toNat_shiftLeft, BitVec.toNat_setWidth, Nat.shiftLeft_eq, BitVec.toNat_ofNat,
    SLAB_BYTES, SLAB_SHIFT]
  rw [Nat.mod_eq_of_lt (show (cpu32 s).toNat < 2 ^ 64 by omega),
    Nat.mod_eq_of_lt (show (cpu32 s).toNat * 2 ^ 12 < 2 ^ 64 by omega)]

end Cpu

end VG.Proof.Slab.X86_64
