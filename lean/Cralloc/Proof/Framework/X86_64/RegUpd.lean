module

public import Cralloc.TCB.X86_64.Isa

/-!
# x86-64: reading a state after a write, for symbolic execution

Unfolding `State.setReg` (or `setFlags`, `arithFlags`) turns the state into a
structure of functions `fun r' => if r' = r then v else …`, and `simp` then
re-simplifies the body of each such lambda, under a fresh variable, every
time it traverses the state: the cost of a straight-line block grows with the
square of its length, and caching does not help. These lemmas keep the
writes folded, and read one field through them one write at a time: the
state stays a first-order term that `simp` caches.

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
theorem sf_setReg (r : Reg) (v : BitVec 64) : (s.setReg r v).sf = s.sf := rfl
theorem of_setReg (r : Reg) (v : BitVec 64) : (s.setReg r v).of = s.of := rfl
theorem syms_setReg (r : Reg) (v : BitVec 64) : (s.setReg r v).syms = s.syms := rfl
theorem fsBase_setReg (r : Reg) (v : BitVec 64) : (s.setReg r v).fsBase = s.fsBase := rfl
theorem params_setReg (r : Reg) (v : BitVec 64) : (s.setReg r v).params = s.params := rfl

/-- A 32-bit value written to a register (zero-extended) and read back. -/
theorem setWidth_setWidth_32 (x : BitVec 32) : (x.setWidth 64).setWidth 32 = x := by
  rw [BitVec.setWidth_setWidth_of_le _ (by decide), BitVec.setWidth_eq]

/-! ## `setFlags` and `arithFlags` -/

theorem gpr_setFlags (a b c d : Option Bool) : (s.setFlags a b c d).gpr = s.gpr := rfl
theorem cf_setFlags (a b c d : Option Bool) : (s.setFlags a b c d).cf = a := rfl
theorem of_setFlags (a b c d : Option Bool) : (s.setFlags a b c d).of = b := rfl
theorem zf_setFlags (a b c d : Option Bool) : (s.setFlags a b c d).zf = c := rfl
theorem sf_setFlags (a b c d : Option Bool) : (s.setFlags a b c d).sf = d := rfl
theorem mem_setFlags (a b c d : Option Bool) : (s.setFlags a b c d).mem = s.mem := rfl
theorem rd_setFlags (a b c d : Option Bool) : (s.setFlags a b c d).rd = s.rd := rfl
theorem wr_setFlags (a b c d : Option Bool) : (s.setFlags a b c d).wr = s.wr := rfl
theorem syms_setFlags (a b c d : Option Bool) : (s.setFlags a b c d).syms = s.syms := rfl
theorem fsBase_setFlags (a b c d : Option Bool) : (s.setFlags a b c d).fsBase = s.fsBase := rfl
theorem params_setFlags (a b c d : Option Bool) : (s.setFlags a b c d).params = s.params := rfl

theorem gpr_arithFlags {w : Nat} (x : BitVec w) (c o : Bool) : (arithFlags s x c o).gpr = s.gpr :=
  rfl
theorem mem_arithFlags {w : Nat} (x : BitVec w) (c o : Bool) : (arithFlags s x c o).mem = s.mem := rfl
theorem rd_arithFlags {w : Nat} (x : BitVec w) (c o : Bool) : (arithFlags s x c o).rd = s.rd := rfl
theorem wr_arithFlags {w : Nat} (x : BitVec w) (c o : Bool) : (arithFlags s x c o).wr = s.wr := rfl
theorem syms_arithFlags {w : Nat} (x : BitVec w) (c o : Bool) :
    (arithFlags s x c o).syms = s.syms := rfl
theorem fsBase_arithFlags {w : Nat} (x : BitVec w) (c o : Bool) :
    (arithFlags s x c o).fsBase = s.fsBase := rfl
theorem params_arithFlags {w : Nat} (x : BitVec w) (c o : Bool) :
    (arithFlags s x c o).params = s.params := rfl
theorem zf_arithFlags {w : Nat} (x : BitVec w) (c o : Bool) :
    (arithFlags s x c o).zf = some (x == 0) := rfl
theorem cf_arithFlags {w : Nat} (x : BitVec w) (c o : Bool) :
    (arithFlags s x c o).cf = some c := rfl
theorem of_arithFlags {w : Nat} (x : BitVec w) (c o : Bool) :
    (arithFlags s x c o).of = some o := rfl
theorem sf_arithFlags {w : Nat} (x : BitVec w) (c o : Bool) :
    (arithFlags s x c o).sf = some x.msb := rfl

end VG.X86_64.RegUpd
