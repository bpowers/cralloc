module

public import Cralloc.Impl.Slab.AArch64
public import Cralloc.Proof.Slab.AArch64.Facts
public import Cralloc.Proof.Framework.Body
public import Cralloc.Proof.Framework.AArch64.Access
public import Cralloc.Proof.Framework.AArch64.RegUpd
public import Cralloc.Proof.Framework.AArch64.Flags
public import Cralloc.Proof.Framework.Offset

/-!
# The routines' common steps on AArch64

The arm, and the CPU's load, compare and shift: every routine starts with them.
Each step is `exec i σ = some σ'` for named states, so that the folds over a
sequence's body (`Proof/Framework/Body.lean`) unfold by rewriting. A routine's first
step writes its miss's result, a different register in each, so the CPU's steps are
of any state `τ` that step may leave (`Base`).
-/

@[expose] public section

namespace VG.Proof.Slab.AArch64

open VG.AArch64 VG.Spec.Slab VG.Impl.Slab.AArch64

/-! ## The arm -/

/-- The rseq area's offset from the thread pointer. -/
def o (s : State) : Addr := s.mem.readW (s.syms RS) 64

theorem area_eq (s : State) : area s = s.tpidr + o s := rfl

/-- Memory after arming: `rseq_cs` holds the descriptor. -/
def m1 (s : State) : Mem := s.mem.writeW (area s + 8) s.csDesc

/-- The state after the arm, where the sequence starts. -/
def σA (s : State) : State := { s with mem := m1 s }

theorem inRd {s : State} {r : Region} (h : r ∈ s.rd) : InRegions (s.rd ++ s.wr) r.base r.len :=
  (InRegions.of_mem h (Region.contains_self _ _)).append_left

theorem inWr {s : State} {r : Region} (h : r ∈ s.wr) : InRegions s.wr r.base r.len :=
  InRegions.of_mem h (Region.contains_self _ _)

theorem inRdWr {s : State} {r : Region} (h : r ∈ s.wr) : InRegions (s.rd ++ s.wr) r.base r.len :=
  (inWr h).append_right

/-- `[x10, #d]`, in a state where `x10` holds the rseq area's address. -/
theorem ea_x10 {s : State} (σ : State) (d : Nat) (h : σ.gpr .x10 = area s) :
    σ.ea { base := .x10, off := .imm (d : Int) } = area s + BitVec.ofNat 64 d := by
  rw [Access.ea_imm, h, BitVec.ofInt_natCast]

/-- `[r, #-8]`. -/
theorem ea_below (σ : State) (r : Reg) : σ.ea { base := r, off := .imm (-8) } = σ.gpr r - 8 := by
  rw [Access.ea_imm, show BitVec.ofInt 64 (-8) = -8 from by decide, BitVec.sub_eq_add_neg]

section Arm

variable {s : State}

theorem σA_rd : (σA s).rd = s.rd := rfl
theorem σA_wr : (σA s).wr = s.wr := rfl
theorem σA_mem : (σA s).mem = m1 s := rfl
theorem σA_gpr (r : Reg) : (σA s).gpr r = s.gpr r := rfl

theorem arm_ea (h : ArmOk s) : s.ea { base := .x10, off := .imm 8 } = area s + 8 :=
  ea_x10 s 8 h.ready.1

theorem exec_arm (h : ArmOk s) : exec (.str .x .x11 { base := .x10, off := .imm 8 }) s = some (σA s) := by
  have hin : InRegions s.wr (s.ea { base := .x10, off := .imm 8 }) 8 := by
    rw [arm_ea h]; exact inWr (r := ⟨area s + 8, 8⟩) h.cs
  rw [Access.exec_str_x hin, arm_ea h, h.ready.2]
  rfl

theorem execBlock_arm (h : ArmOk s) : execBlock isa arm s = some (σA s) := by
  rw [arm, execBlock_cons_some (exec_arm h), execBlock_nil']

theorem execBlock_dropLast_arm : execBlock isa arm.dropLast s = some s := rfl

/-- The arm points `rseq_cs` at the descriptor. -/
theorem arms_arm (h : ArmOk s) : arms (.str .x .x11 { base := .x10, off := .imm 8 }) s :=
  ⟨arm_ea h, h.ready.2⟩

theorem events_arm (h : ArmOk s) :
    blockEvents isa arm s = [⟨area s + 8, 8, true, s.csDesc.toNat⟩] := by
  rw [arm, blockEvents_cons_some (exec_arm h), blockEvents_nil']
  simp only [Instr.events, arm_ea h, toNat_read8, List.append_nil]
  rw [σA_mem, m1, Mem.readW_writeW_self64]

end Arm

/-! ## The CPU number -/

/-- The CPU number, as the routine reads it. -/
def cpu32 (s : State) : BitVec 32 := s.mem.readW (area s + 4) 32

/-- The CPU's slab's offset. -/
def sh (s : State) : BitVec 64 := (cpu32 s).setWidth 64 <<< 12

/-- A state a routine's CPU steps may start in: memory as after arming, and `x10`
holding the rseq area's address. -/
structure Base (s τ : State) : Prop where
  mem : τ.mem = m1 s
  rd : τ.rd = s.rd
  wr : τ.wr = s.wr
  x10 : τ.gpr .x10 = area s

/-- After loading the CPU. -/
def cpu1 (τ s : State) : State := τ.setReg .x8 ((cpu32 s).setWidth 64)

/-- After comparing it with `SLAB_COUNT`. -/
def cpu2 (τ s : State) : State := (cpu1 τ s).addFlags (cpu32 s) (~~~(BitVec.ofNat 32 8192)) true

/-- After the shift: `x8` is the CPU's slab's offset in `SLABS`. -/
def cpu3 (τ s : State) : State := (cpu2 τ s).setReg .x8 (sh s)

/-- `cpu_id` and `rseq_cs` are apart. -/
theorem cpu_cs_disjoint (s : State) : Region.Disjoint ⟨area s + 4, 32 / 8⟩ ⟨area s + 8, 8⟩ :=
  Offset.disjoint (area s) (d := 4) (n := 4) (e := 8) (k := 8) (by omega) (by omega) (by omega)

section Cpu

variable {τ s : State}

theorem cpuOp_ea (hb : Base s τ) : τ.ea { base := .x10, off := .imm 4 } = area s + 4 :=
  ea_x10 τ 4 hb.x10

theorem read_m1_cpu : (m1 s).readW (area s + 4) 32 = cpu32 s := by
  rw [m1, readW_writeW_disj (cpu_cs_disjoint s) (by decide)]
  rfl

theorem exec_cpu (h : ArmOk s) (hb : Base s τ) :
    exec (.ldr .w .x8 { base := .x10, off := .imm 4 }) τ = some (cpu1 τ s) := by
  have hin : InRegions (τ.rd ++ τ.wr) (τ.ea { base := .x10, off := .imm 4 }) 4 := by
    rw [cpuOp_ea hb, hb.rd, hb.wr]; exact inRd (r := ⟨area s + 4, 4⟩) h.cpuF
  rw [Access.exec_ldr_w hin, cpuOp_ea hb, hb.mem, read_m1_cpu]
  rfl

theorem ev_cpu (hb : Base s τ) :
    Instr.events (.ldr .w .x8 { base := .x10, off := .imm 4 }) τ (cpu1 τ s) =
      [⟨area s + 4, 4, false, (cpu32 s).toNat⟩] := by
  simp only [Instr.events, cpuOp_ea hb, Width.bytes, toNat_read4, hb.mem, read_m1_cpu]

theorem read_cpu1 : (cpu1 τ s).read .w .x8 = cpu32 s := by
  rw [RegUpd.read_w, cpu1, RegUpd.gpr_setReg_self, RegUpd.setWidth_setWidth_32]

theorem exec_cmp : exec (.cmp .w .x8 (.imm 8192)) (cpu1 τ s) = some (cpu2 τ s) := by
  rw [Flags.exec_cmp, read_cpu1]
  rfl

theorem eval_cpu : eval .hs (cpu2 τ s) = some (decide (8192 ≤ (cpu32 s).toNat)) :=
  Flags.eval_hs_cmp _ _ _

theorem cpu2_x8 : (cpu2 τ s).gpr .x8 = (cpu32 s).setWidth 64 := by
  rw [cpu2, RegUpd.gpr_addFlags, cpu1, RegUpd.gpr_setReg_self]

theorem exec_lsl : exec (.lsl .x .x8 .x8 12) (cpu2 τ s) = some (cpu3 τ s) := by
  show some ((cpu2 τ s).setReg .x8 ((cpu2 τ s).gpr .x8 <<< 12)) = _
  rw [cpu2_x8]
  rfl

theorem gpr_cpu2 {r : Reg} (h8 : ¬ r = .x8) : (cpu2 τ s).gpr r = τ.gpr r := by
  rw [cpu2, RegUpd.gpr_addFlags, cpu1, RegUpd.gpr_setReg_of_ne _ _ h8]

theorem gpr_cpu3 {r : Reg} (h8 : ¬ r = .x8) : (cpu3 τ s).gpr r = τ.gpr r := by
  rw [cpu3, RegUpd.gpr_setReg_of_ne _ _ h8, gpr_cpu2 h8]

theorem cpu3_x8 : (cpu3 τ s).gpr .x8 = sh s := RegUpd.gpr_setReg_self _ _ _

theorem cpu3_mem : (cpu3 τ s).mem = τ.mem := rfl
theorem cpu3_rd : (cpu3 τ s).rd = τ.rd := rfl
theorem cpu3_wr : (cpu3 τ s).wr = τ.wr := rfl
theorem cpu3_syms : (cpu3 τ s).syms = τ.syms := rfl

/-- The CPU's slab's offset, as a number. -/
theorem sh_eq : sh s = BitVec.ofNat 64 ((cpu32 s).toNat * SLAB_BYTES) := by
  apply BitVec.eq_of_toNat_eq
  have := (cpu32 s).isLt
  simp only [sh, BitVec.toNat_shiftLeft, BitVec.toNat_setWidth, Nat.shiftLeft_eq, BitVec.toNat_ofNat,
    SLAB_BYTES, SLAB_SHIFT]
  rw [Nat.mod_eq_of_lt (show (cpu32 s).toNat < 2 ^ 64 by omega),
    Nat.mod_eq_of_lt (show (cpu32 s).toNat * 2 ^ 12 < 2 ^ 64 by omega)]

end Cpu

end VG.Proof.Slab.AArch64
