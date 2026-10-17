module

public import Asmalloc.TCB.X86_64.Isa

/-!
# x86-64: reading a state after a write, for symbolic execution

Unfolding `State.setReg` (or `setFlags`, `arithFlags`, `setXmm`, `setV`)
turns the state into a structure of functions
`fun r' => if r' = r then v else …`, and `simp` then re-simplifies the body of
each such lambda, under a fresh variable, every time it traverses the state:
the cost of a straight-line block grows with the square of its length, and
caching does not help. These lemmas keep the writes folded, and read one
field through them one write at a time: the state stays a first-order term
that `simp` caches.

A register read after a register write has two forms:
* `gpr_setReg`, an `if` on the registers, which `simp` decides when both are
  literals (`ite_true`, `ite_false`, `reduceCtorEq`);
* `gpr_setReg_self` and `gpr_setReg_of_ne`, whose side condition `¬r = d`
  `simp` discharges from hypotheses, for registers that are variables.

Use them with `simp only [runBlock_cons, runStep_some, exec, …,
State.setReg32, RegUpd.gpr_setReg, RegUpd.mem_setReg, …]` and never unfold
`State.setReg`, `arithFlags` or `State.setFlags`.
-/

@[expose] public section

namespace VG.X86_64.RegUpd

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
theorem xmm_setReg (r : Reg) (v : BitVec 64) : (s.setReg r v).xmm = s.xmm := rfl
theorem ymmHi_setReg (r : Reg) (v : BitVec 64) : (s.setReg r v).ymmHi = s.ymmHi := rfl
theorem mxcsr_setReg (r : Reg) (v : BitVec 64) : (s.setReg r v).mxcsr = s.mxcsr := rfl

/-- A 32-bit value written to a register (zero-extended) and read back. -/
theorem setWidth_setWidth_32 (x : BitVec 32) : (x.setWidth 64).setWidth 32 = x := by
  rw [BitVec.setWidth_setWidth_of_le _ (by decide), BitVec.setWidth_eq]

/-! ## `setFlags` and `arithFlags` -/

theorem gpr_setFlags (a b c d : Option Bool) : (s.setFlags a b c d).gpr = s.gpr := rfl
theorem cf_setFlags (a b c d : Option Bool) : (s.setFlags a b c d).cf = a := rfl
theorem mem_setFlags (a b c d : Option Bool) : (s.setFlags a b c d).mem = s.mem := rfl
theorem rd_setFlags (a b c d : Option Bool) : (s.setFlags a b c d).rd = s.rd := rfl
theorem wr_setFlags (a b c d : Option Bool) : (s.setFlags a b c d).wr = s.wr := rfl
theorem xmm_setFlags (a b c d : Option Bool) : (s.setFlags a b c d).xmm = s.xmm := rfl
theorem ymmHi_setFlags (a b c d : Option Bool) : (s.setFlags a b c d).ymmHi = s.ymmHi := rfl
theorem mxcsr_setFlags (a b c d : Option Bool) : (s.setFlags a b c d).mxcsr = s.mxcsr := rfl

theorem gpr_arithFlags {w : Nat} (x : BitVec w) (c o : Bool) : (arithFlags s x c o).gpr = s.gpr :=
  rfl
theorem mem_arithFlags {w : Nat} (x : BitVec w) (c o : Bool) : (arithFlags s x c o).mem = s.mem := rfl
theorem rd_arithFlags {w : Nat} (x : BitVec w) (c o : Bool) : (arithFlags s x c o).rd = s.rd := rfl
theorem wr_arithFlags {w : Nat} (x : BitVec w) (c o : Bool) : (arithFlags s x c o).wr = s.wr := rfl
theorem xmm_arithFlags {w : Nat} (x : BitVec w) (c o : Bool) : (arithFlags s x c o).xmm = s.xmm := rfl
theorem ymmHi_arithFlags {w : Nat} (x : BitVec w) (c o : Bool) :
    (arithFlags s x c o).ymmHi = s.ymmHi := rfl
theorem zf_arithFlags {w : Nat} (x : BitVec w) (c o : Bool) :
    (arithFlags s x c o).zf = some (x == 0) := rfl
theorem cf_arithFlags {w : Nat} (x : BitVec w) (c o : Bool) :
    (arithFlags s x c o).cf = some c := rfl

/-! ## `setXmm` and `setV` -/

theorem xmm_setXmm_self (r : XReg) (v : BitVec 128) : (s.setXmm r v).xmm r = v := by
  simp [State.setXmm]

theorem xmm_setXmm_of_ne {r r' : XReg} (v : BitVec 128) (h : ¬r' = r) :
    (s.setXmm r v).xmm r' = s.xmm r' := by
  simp [State.setXmm, h]

theorem gpr_setXmm (r : XReg) (v : BitVec 128) : (s.setXmm r v).gpr = s.gpr := rfl
theorem mem_setXmm (r : XReg) (v : BitVec 128) : (s.setXmm r v).mem = s.mem := rfl
theorem rd_setXmm (r : XReg) (v : BitVec 128) : (s.setXmm r v).rd = s.rd := rfl
theorem wr_setXmm (r : XReg) (v : BitVec 128) : (s.setXmm r v).wr = s.wr := rfl
theorem zf_setXmm (r : XReg) (v : BitVec 128) : (s.setXmm r v).zf = s.zf := rfl

theorem xmm_setV (len : VLen) (r : XReg) (lo hi : BitVec 128) (r' : XReg) :
    (s.setV len r lo hi).xmm r' = if r' = r then lo else s.xmm r' := rfl
theorem ymmHi_setV_256 (r : XReg) (lo hi : BitVec 128) (r' : XReg) :
    (s.setV .l256 r lo hi).ymmHi r' = if r' = r then hi else s.ymmHi r' := rfl
theorem ymmHi_setV_128 (r : XReg) (lo hi : BitVec 128) (r' : XReg) :
    (s.setV .l128 r lo hi).ymmHi r' = if r' = r then 0 else s.ymmHi r' := rfl
theorem gpr_setV (len : VLen) (r : XReg) (lo hi : BitVec 128) :
    (s.setV len r lo hi).gpr = s.gpr := rfl
theorem mem_setV (len : VLen) (r : XReg) (lo hi : BitVec 128) :
    (s.setV len r lo hi).mem = s.mem := rfl
theorem rd_setV (len : VLen) (r : XReg) (lo hi : BitVec 128) :
    (s.setV len r lo hi).rd = s.rd := rfl
theorem wr_setV (len : VLen) (r : XReg) (lo hi : BitVec 128) :
    (s.setV len r lo hi).wr = s.wr := rfl
theorem mxcsr_setV (len : VLen) (r : XReg) (lo hi : BitVec 128) :
    (s.setV len r lo hi).mxcsr = s.mxcsr := rfl

end VG.X86_64.RegUpd
