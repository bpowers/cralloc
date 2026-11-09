module

public import Cralloc.TCB.AArch64.Isa

/-!
# AArch64: reading a state after a write, for symbolic execution

As on x86-64 (`Proof/Framework/X86_64/RegUpd.lean`): these keep the writes folded,
so that the state stays a first-order term that `simp` caches. Never unfold
`State.setReg` or `State.setFlags` in a long block.
-/

@[expose] public section

namespace VG.AArch64.RegUpd

variable (s : State)

/-! ## `setReg` -/

theorem gpr_setReg (d : Reg) (v : BitVec 64) (r : Reg) :
    (s.setReg d v).gpr r = if r = d then v else s.gpr r := rfl

theorem gpr_setReg_self (r : Reg) (v : BitVec 64) : (s.setReg r v).gpr r = v := by
  simp [State.setReg]

theorem gpr_setReg_of_ne {r r' : Reg} (v : BitVec 64) (h : ¬r' = r) :
    (s.setReg r v).gpr r' = s.gpr r' := by
  simp [State.setReg, h]

theorem mem_setReg (r : Reg) (v : BitVec 64) : (s.setReg r v).mem = s.mem := rfl
theorem rd_setReg (r : Reg) (v : BitVec 64) : (s.setReg r v).rd = s.rd := rfl
theorem wr_setReg (r : Reg) (v : BitVec 64) : (s.setReg r v).wr = s.wr := rfl
theorem zf_setReg (r : Reg) (v : BitVec 64) : (s.setReg r v).zf = s.zf := rfl
theorem cf_setReg (r : Reg) (v : BitVec 64) : (s.setReg r v).cf = s.cf := rfl
theorem nf_setReg (r : Reg) (v : BitVec 64) : (s.setReg r v).nf = s.nf := rfl
theorem vf_setReg (r : Reg) (v : BitVec 64) : (s.setReg r v).vf = s.vf := rfl
theorem sp_setReg (r : Reg) (v : BitVec 64) : (s.setReg r v).sp = s.sp := rfl
theorem syms_setReg (r : Reg) (v : BitVec 64) : (s.setReg r v).syms = s.syms := rfl
theorem tpidr_setReg (r : Reg) (v : BitVec 64) : (s.setReg r v).tpidr = s.tpidr := rfl
theorem csDesc_setReg (r : Reg) (v : BitVec 64) : (s.setReg r v).csDesc = s.csDesc := rfl

/-! ## `read` and `write`, at a literal size -/

theorem read_x (r : Reg) : s.read .x r = s.gpr r := rfl
theorem read_w (r : Reg) : s.read .w r = (s.gpr r).setWidth 32 := rfl
theorem write_x (r : Reg) (v : BitVec 64) : s.write .x r v = s.setReg r v := rfl
theorem write_w (r : Reg) (v : BitVec 32) : s.write .w r v = s.setReg r (v.setWidth 64) := rfl

theorem src_reg (sz : Size) (r : Reg) : s.src sz (.reg r) = s.read sz r := rfl
theorem src_lsl (sz : Size) (r : Reg) (n : Nat) : s.src sz (.lsl r n) = s.read sz r <<< n := rfl
theorem src_imm (sz : Size) (v : Nat) : s.src sz (.imm v) = BitVec.ofNat sz.bits v := rfl

/-- A 32-bit value written to a register (zero-extended) and read back. -/
theorem setWidth_setWidth_32 (x : BitVec 32) : (x.setWidth 64).setWidth 32 = x := by
  rw [BitVec.setWidth_setWidth_of_le _ (by decide), BitVec.setWidth_eq]

/-! ## `setFlags` -/

theorem gpr_setFlags (n z c v : Bool) : (s.setFlags n z c v).gpr = s.gpr := rfl
theorem nf_setFlags (n z c v : Bool) : (s.setFlags n z c v).nf = n := rfl
theorem zf_setFlags (n z c v : Bool) : (s.setFlags n z c v).zf = z := rfl
theorem cf_setFlags (n z c v : Bool) : (s.setFlags n z c v).cf = c := rfl
theorem vf_setFlags (n z c v : Bool) : (s.setFlags n z c v).vf = v := rfl
theorem mem_setFlags (n z c v : Bool) : (s.setFlags n z c v).mem = s.mem := rfl
theorem rd_setFlags (n z c v : Bool) : (s.setFlags n z c v).rd = s.rd := rfl
theorem wr_setFlags (n z c v : Bool) : (s.setFlags n z c v).wr = s.wr := rfl
theorem sp_setFlags (n z c v : Bool) : (s.setFlags n z c v).sp = s.sp := rfl
theorem syms_setFlags (n z c v : Bool) : (s.setFlags n z c v).syms = s.syms := rfl
theorem tpidr_setFlags (n z c v : Bool) : (s.setFlags n z c v).tpidr = s.tpidr := rfl
theorem csDesc_setFlags (n z c v : Bool) : (s.setFlags n z c v).csDesc = s.csDesc := rfl

/-! ## `addFlags` -/

variable {w : Nat} (x y : BitVec w) (c : Bool)

theorem gpr_addFlags : (s.addFlags x y c).gpr = s.gpr := rfl
theorem mem_addFlags : (s.addFlags x y c).mem = s.mem := rfl
theorem rd_addFlags : (s.addFlags x y c).rd = s.rd := rfl
theorem wr_addFlags : (s.addFlags x y c).wr = s.wr := rfl
theorem sp_addFlags : (s.addFlags x y c).sp = s.sp := rfl
theorem syms_addFlags : (s.addFlags x y c).syms = s.syms := rfl
theorem tpidr_addFlags : (s.addFlags x y c).tpidr = s.tpidr := rfl
theorem csDesc_addFlags : (s.addFlags x y c).csDesc = s.csDesc := rfl

end VG.AArch64.RegUpd
