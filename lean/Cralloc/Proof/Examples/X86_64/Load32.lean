module

public import Cralloc.Proof.Framework.X86_64.Exec
public import Cralloc.Proof.Framework.X86_64.RegUpd
public import Cralloc.Proof.Framework.X86_64.Access

/-!
# Example: a 32-bit load that zero-extends

The concurrency ID is a 32-bit field of the rseq area, at offset 24. The
fast path loads it into a 32-bit register, which clears the upper half
whatever it held, and shifts it into a slab offset. Because of the
zero-extension, a bound on the ID bounds the offset.
-/

@[expose] public section

namespace VG.Proof.Examples.X86_64.Load32

open VG.X86_64

/-- `mov r8d, DWORD PTR fs:[rdx+24]; shl r8, 12` -/
def cidOffset : List Instr := [
  .mov32 .r8 (.mem { base := .rdx, disp := 24, seg := some .fs }),
  .shift .shl .r8 12]

/-- The ID's address, `fsBase + rdx + 24`. -/
abbrev cidAddr (s : State) : Addr := s.fsBase + (s.gpr .rdx + BitVec.ofInt 64 24)

theorem cidOffset_ok (s : State) (hr : InRegions (s.rd ++ s.wr) (cidAddr s) 4)
    (hcid : (s.mem.readW (cidAddr s) 32).toNat < 8192) :
    WP isa (.block cidOffset) s fun s' =>
      s'.gpr .r8 = (s.mem.readW (cidAddr s) 32).setWidth 64 <<< 12 ∧
      (s'.gpr .r8).toNat < 8192 * 4096 := by
  have hld := Access.exec_mov32_mem (s := s) (d := .r8)
    (m := { base := .rdx, disp := 24, seg := some .fs }) hr
  have hea : s.ea { base := .rdx, disp := 24, seg := some .fs } = cidAddr s := rfl
  apply WP.of_runBlock
  -- The load first, before `exec` would unfold it past `hld`.
  simp only [cidOffset, runBlock_cons, hld, hea, runStep_some]
  simp only [runStep_some, runBlock_nil, exec, execShift, State.setReg32, RegUpd.gpr_setReg_self,
    Nat.reduceLeDiff, and_self, ite_true, Option.some.injEq, exists_eq_left', true_and]
  have h32 := Access.toNat_setWidth_32_lt (s.mem.readW (cidAddr s) 32)
  rw [BitVec.toNat_shiftLeft, BitVec.toNat_setWidth,
    Nat.mod_eq_of_lt (Nat.lt_trans (BitVec.isLt _) (by decide)), Nat.shiftLeft_eq]
  omega

end VG.Proof.Examples.X86_64.Load32
