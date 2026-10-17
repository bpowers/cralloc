module

public import Asmalloc.TCB.X86_64.Isa

/-!
# x86-64: flags and conditions, for symbolic execution

What `inc`, `dec` and `test` with an immediate leave in the state, read one
field at a time without unfolding `State.setReg` or `State.setFlags` (see
`RegUpd.lean`), and what each branch condition reads of the flags an ALU
instruction sets (`arithFlags`).

Use them with `simp only [runBlock_cons, runStep_some, exec, Flags.gpr_execInc,
Flags.cf_execInc, …]`, and decide a branch with `WP.ite` (or `WP.iteCold`)
and `Flags.eval_e_arithFlags`.
-/

@[expose] public section

namespace VG.X86_64.Flags

variable (s : State)

/-! ## `inc` and `dec` -/

theorem gpr_execInc (d r : Reg) :
    (execInc d s).gpr r = if r = d then s.gpr d + 1 else s.gpr r := rfl
theorem cf_execInc (d : Reg) : (execInc d s).cf = s.cf := rfl
theorem zf_execInc (d : Reg) : (execInc d s).zf = some (s.gpr d + 1 == 0) := rfl
theorem sf_execInc (d : Reg) : (execInc d s).sf = some (s.gpr d + 1).msb := rfl
theorem mem_execInc (d : Reg) : (execInc d s).mem = s.mem := rfl
theorem rd_execInc (d : Reg) : (execInc d s).rd = s.rd := rfl
theorem wr_execInc (d : Reg) : (execInc d s).wr = s.wr := rfl
theorem fsBase_execInc (d : Reg) : (execInc d s).fsBase = s.fsBase := rfl
theorem syms_execInc (d : Reg) : (execInc d s).syms = s.syms := rfl

theorem gpr_execDec (d r : Reg) :
    (execDec d s).gpr r = if r = d then s.gpr d - 1 else s.gpr r := rfl
theorem cf_execDec (d : Reg) : (execDec d s).cf = s.cf := rfl
theorem zf_execDec (d : Reg) : (execDec d s).zf = some (s.gpr d - 1 == 0) := rfl
theorem sf_execDec (d : Reg) : (execDec d s).sf = some (s.gpr d - 1).msb := rfl
theorem mem_execDec (d : Reg) : (execDec d s).mem = s.mem := rfl
theorem rd_execDec (d : Reg) : (execDec d s).rd = s.rd := rfl
theorem wr_execDec (d : Reg) : (execDec d s).wr = s.wr := rfl
theorem fsBase_execDec (d : Reg) : (execDec d s).fsBase = s.fsBase := rfl
theorem syms_execDec (d : Reg) : (execDec d s).syms = s.syms := rfl

/-! ## `test` with an immediate

The 32-bit form tests the low 32 bits against the immediate as it is; the
64-bit form against the immediate sign-extended. Neither writes a register. -/

theorem exec_test32_imm (d : Reg) (v : BitVec 32) :
    exec (.alu32 .test d (.imm v)) s = some (arithFlags s ((s.gpr d).setWidth 32 &&& v) false false) :=
  rfl

theorem exec_test_imm (d : Reg) (v : BitVec 32) :
    exec (.alu .test d (.imm v)) s = some (arithFlags s (s.gpr d &&& v.signExtend 64) false false) :=
  rfl

/-! ## Conditions -/

theorem eval_e : eval .e s = s.zf := rfl
theorem eval_ne : eval .ne s = s.zf.map (!·) := rfl
theorem eval_b : eval .b s = s.cf := rfl
theorem eval_ae : eval .ae s = s.cf.map (!·) := rfl

theorem eval_e_arithFlags {w : Nat} (x : BitVec w) (c o : Bool) :
    eval .e (arithFlags s x c o) = some (x == 0) := rfl
theorem eval_ne_arithFlags {w : Nat} (x : BitVec w) (c o : Bool) :
    eval .ne (arithFlags s x c o) = some !(x == 0) := rfl
theorem eval_b_arithFlags {w : Nat} (x : BitVec w) (c o : Bool) :
    eval .b (arithFlags s x c o) = some c := rfl
theorem eval_ae_arithFlags {w : Nat} (x : BitVec w) (c o : Bool) :
    eval .ae (arithFlags s x c o) = some !c := rfl

end VG.X86_64.Flags
