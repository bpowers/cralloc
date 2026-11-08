module

public import Cralloc.Proof.Slab.AArch64.Setup

/-!
# The pop's and push's slab word on AArch64

`ldr x1, [x3, x8]`: the class's word in the CPU's slab, `x3` holding the address of the
class's word in slab 0 and `x8` the CPU's slab's offset. Nothing here names the slabs:
the address in `x3` is the caller's, which the contract ties to `SLABS` and the class
(`Ctx.argsOk`).
-/

@[expose] public section

namespace VG.Proof.Slab.AArch64

open VG.AArch64 VG.Spec.Slab VG.Impl.Slab.AArch64

/-- The slab word's address, and the word. -/
def wa (s : State) : Addr := s.gpr .x3 + sh s
def wv (s : State) : BitVec 64 := s.mem.readW (wa s) 64

section

variable {regs : List Reg} {c : Spec.Slab.Ctx isa} {τ s : State}

/-- `[x3, x8]`, in any state where they hold the class's word in slab 0 and the CPU's
slab's offset. -/
theorem wordAt_ea (σ : State) (h3 : σ.gpr .x3 = s.gpr .x3) (h8 : σ.gpr .x8 = sh s) :
    σ.ea wordAt = wa s := by
  rw [wordAt, Access.ea_reg, h3, h8]
  rfl

/-- The word address the caller gives is `SLABS` plus 8 times the class. -/
theorem x3_eq (hA : A64 c.A regs) (h0 : c.arg s 0 = s.gpr .x3)
    (hoff : (c.arg s 0 - c.sym s SLABS).toNat = 8 * c.cls s) :
    s.gpr .x3 = s.syms SLABS + BitVec.ofNat 64 (8 * c.cls s) := by
  rw [h0, sym_of hA] at hoff
  have hsub : s.gpr .x3 - s.syms SLABS = BitVec.ofNat 64 (8 * c.cls s) := by
    apply BitVec.eq_of_toNat_eq
    rw [BitVec.toNat_ofNat, ← hoff, Nat.mod_eq_of_lt (BitVec.isLt _)]
  rw [← hsub, BitVec.add_comm, BitVec.sub_add_cancel]

/-- The slab word's address is the class's for the CPU. -/
theorem wa_eq (hA : A64 c.A regs) (h0 : c.arg s 0 = s.gpr .x3)
    (hoff : (c.arg s 0 - c.sym s SLABS).toNat = 8 * c.cls s) : wa s = c.word s := by
  rw [Spec.Slab.Ctx.word, wordAddr, sym_of hA, cpu_of hA, wa, sh_eq, x3_eq hA h0 hoff, cpu32,
    BitVec.ofNat_add, BitVec.add_assoc, BitVec.add_comm (BitVec.ofNat 64 (8 * c.cls s))]

/-- The word, read in memory after arming. -/
theorem read_m1_word (h : Facts c s) (hi : c.InRange s) (hw : wa s = c.word s) :
    (m1 s).readW (wa s) 64 = wv s := by
  rw [m1, readW64_writeW_disj (by rw [hw]; exact (h.cs_word hi).symm)]
  rfl

theorem exec_wordAt (h : Facts c s) (hi : c.InRange s) (hw : wa s = c.word s) (hb : Base s τ)
    (h3 : τ.gpr .x3 = s.gpr .x3) :
    exec (.ldr .x .x1 wordAt) (cpu3 τ s) = some ((cpu3 τ s).setReg .x1 (wv s)) := by
  have hea := wordAt_ea (s := s) (cpu3 τ s) ((gpr_cpu3 (by decide)).trans h3) cpu3_x8
  have hin : InRegions ((cpu3 τ s).rd ++ (cpu3 τ s).wr) ((cpu3 τ s).ea wordAt) 8 := by
    rw [hea, cpu3_rd, cpu3_wr, hb.rd, hb.wr, hw]
    exact inRdWr (r := ⟨c.word s, 8⟩) (h.word hi)
  rw [Access.exec_ldr_x hin, hea, cpu3_mem, hb.mem, read_m1_word h hi hw]

theorem ev_wordAt (h : Facts c s) (hi : c.InRange s) (hw : wa s = c.word s) (hb : Base s τ)
    (h3 : τ.gpr .x3 = s.gpr .x3) :
    Instr.events (.ldr .x .x1 wordAt) (cpu3 τ s) ((cpu3 τ s).setReg .x1 (wv s)) =
      [⟨wa s, 8, false, (wv s).toNat⟩] := by
  have hea := wordAt_ea (s := s) (cpu3 τ s) ((gpr_cpu3 (by decide)).trans h3) cpu3_x8
  simp only [Instr.events, hea, Width.bytes, toNat_read8, cpu3_mem, hb.mem, read_m1_word h hi hw]

end

end VG.Proof.Slab.AArch64
