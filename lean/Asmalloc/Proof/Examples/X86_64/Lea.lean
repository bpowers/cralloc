module

public import Asmalloc.Proof.Framework.X86_64.Exec
public import Asmalloc.Proof.Framework.X86_64.RegUpd
public import Asmalloc.Proof.Framework.X86_64.Access

/-!
# Example: addresses with `lea`

The address arithmetic of a magazine pop: step a slot pointer back by one
slot, form an address from a base and a scaled index, and form the address
of an entry of a static table. None of it touches memory or the flags, so a
compare before it still decides a branch after it.
-/

@[expose] public section

namespace VG.Proof.Examples.X86_64.Lea

open VG.X86_64

/-- `cmp rax, rbx; lea rcx, [rcx-8]; lea rdx, [rdi+rsi*8+16]; lea r9, [rip + VG_T + 24]` -/
def addrs : List Instr := [
  .alu .cmp .rax (.reg .rbx),
  .lea .rcx { base := .rcx, disp := -8 },
  .lea .rdx { base := .rdi, index := some .rsi, scale := 8, disp := 16 },
  .leaSym .r9 "VG_T" 24]

theorem addrs_ok (s : State) :
    WP isa (.block addrs) s fun s' =>
      s'.gpr .rcx = s.gpr .rcx - 8 ∧
      s'.gpr .rdx = s.gpr .rdi + s.gpr .rsi * 8 + 16 ∧
      s'.gpr .r9 = s.syms "VG_T" + 24 ∧
      s'.zf = some (s.gpr .rax - s.gpr .rbx == 0) ∧
      s'.mem = s.mem := by
  apply WP.of_runBlock
  simp only [addrs, runBlock_cons, runStep_some, runBlock_nil, exec, execAlu, readSrc,
    Option.bind_some, Access.offset_base_disp, Access.offset_index, RegUpd.gpr_setReg,
    RegUpd.gpr_arithFlags, RegUpd.syms_setReg, RegUpd.syms_arithFlags, RegUpd.zf_setReg,
    RegUpd.zf_arithFlags, RegUpd.mem_setReg, RegUpd.mem_arithFlags, Option.some.injEq,
    exists_eq_left', reduceCtorEq, ite_true, ite_false, Access.add_ofInt_neg_lit,
    BitVec.ofInt_ofNat, BitVec.ofNat_eq_ofNat, and_self]

end VG.Proof.Examples.X86_64.Lea
