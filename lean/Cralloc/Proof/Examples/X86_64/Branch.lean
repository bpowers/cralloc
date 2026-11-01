module

public import Cralloc.Proof.Framework.X86_64.Exec
public import Cralloc.Proof.Framework.X86_64.RegUpd
public import Cralloc.Proof.Framework.X86_64.Flags

/-!
# Example: a mask test that picks the slow path, and `inc` and `dec`

`check` is the bounds check of a magazine pop: `test ecx, MASK` and a
branch, taken when the slot bits are zero, to code laid out after the
return (`Code.iteCold`); otherwise it decrements. `cmpInc` shows that `inc`
keeps the carry a compare set.
-/

@[expose] public section

namespace VG.Proof.Examples.X86_64.Branch

open VG.X86_64

/-- `test ecx, 0x1f0; je cold; dec rcx; mov rax, rcx; …; cold: mov rax, 0` -/
def check : Prog isa :=
  .seq (.block [.alu32 .test .rcx (.imm 0x1f0)])
    (Code.iteCold .e (.block [.movImm64 .rax 0]) (.block [.dec .rcx, .mov .rax (.reg .rcx)]))

theorem check_ok (s : State) :
    WP isa check s fun s' =>
      s'.gpr .rax = if (s.gpr .rcx).setWidth 32 &&& 0x1f0 = 0 then 0 else s.gpr .rcx - 1 := by
  apply WP.seq
  apply WP.of_runBlock
  simp only [runBlock_cons, runStep_some, runBlock_nil, Flags.exec_test32_imm, Option.some.injEq,
    exists_eq_left']
  apply WP.iteCold _ (Flags.eval_e_arithFlags s _ false false)
  · intro h
    apply WP.of_runBlock
    simp only [beq_iff_eq] at h
    simp only [runBlock_cons, runStep_some, runBlock_nil, exec, Option.some.injEq, exists_eq_left',
      RegUpd.gpr_setReg_self, h, ite_true]
  · intro h
    apply WP.of_runBlock
    simp only [beq_eq_false_iff_ne, ne_eq] at h
    simp only [runBlock_cons, runStep_some, runBlock_nil, exec, readSrc, Option.map_some,
      Option.some.injEq, exists_eq_left', RegUpd.gpr_setReg, Flags.gpr_execDec, RegUpd.gpr_arithFlags,
      ite_true, h, ite_false]

/-- `cmp rax, rbx; inc rcx` -/
def cmpInc : List Instr := [.alu .cmp .rax (.reg .rbx), .inc .rcx]

theorem cmpInc_ok (s : State) :
    WP isa (.block cmpInc) s fun s' =>
      s'.cf = some (decide ((s.gpr .rax).toNat < (s.gpr .rbx).toNat)) ∧
      s'.zf = some (s.gpr .rcx + 1 == 0) ∧
      s'.gpr .rcx = s.gpr .rcx + 1 := by
  apply WP.of_runBlock
  simp only [cmpInc, runBlock_cons, runStep_some, runBlock_nil, exec, execAlu, readSrc,
    Option.bind_some, Option.some.injEq, exists_eq_left', Flags.cf_execInc, Flags.zf_execInc,
    Flags.gpr_execInc, RegUpd.cf_arithFlags, RegUpd.gpr_arithFlags, ite_true, and_self]

end VG.Proof.Examples.X86_64.Branch
