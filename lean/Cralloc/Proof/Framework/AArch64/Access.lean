module

public import Cralloc.TCB.AArch64.Isa
public import Cralloc.Proof.Framework.Mem

/-!
# AArch64: memory accesses and addresses, for symbolic execution

`State.load64` and its kin are an `if` on whether the access is permitted, which
`simp` cannot decide from a hypothesis that the access lies in a region: these
lemmas take that fact and give the access's result. The `ea_*` lemmas give the
address of each form of memory operand, and `page_add_lo12` that `adrp` and `add`
at `:lo12:` make an address.
-/

@[expose] public section

namespace VG.AArch64.Access

variable {s : State} {a : Addr}

/-! ## Loads and stores that are permitted -/

theorem load64_eq (h : InRegions (s.rd ++ s.wr) a 8) : s.load64 a = some (s.mem.readW a 64) := by
  simp [State.load64, h]

theorem load32_eq (h : InRegions (s.rd ++ s.wr) a 4) : s.load32 a = some (s.mem.readW a 32) := by
  simp [State.load32, h]

theorem load8_eq (h : InRegions (s.rd ++ s.wr) a 1) : s.load8 a = some (s.mem a) := by
  simp [State.load8, h]

theorem store64_eq {v : BitVec 64} (h : InRegions s.wr a 8) :
    s.store64 a v = some { s with mem := s.mem.writeW a v } := by
  simp [State.store64, h]

theorem store32_eq {v : BitVec 32} (h : InRegions s.wr a 4) :
    s.store32 a v = some { s with mem := s.mem.writeW a v } := by
  simp [State.store32, h]

theorem store8_eq {v : Byte} (h : InRegions s.wr a 1) :
    s.store8 a v = some { s with mem := s.mem.writeW a v } := by
  simp [State.store8, h]

/-! ## Each instruction, permitted -/

theorem exec_ldr_x {t : Reg} {m : MemOp} (h : InRegions (s.rd ++ s.wr) (s.ea m) 8) :
    exec (.ldr .x t m) s = some (s.setReg t (s.mem.readW (s.ea m) 64)) := by
  simp only [exec, load64_eq h, Option.map_some]

theorem exec_ldr_w {t : Reg} {m : MemOp} (h : InRegions (s.rd ++ s.wr) (s.ea m) 4) :
    exec (.ldr .w t m) s = some (s.setReg t ((s.mem.readW (s.ea m) 32).setWidth 64)) := by
  simp only [exec, load32_eq h, Option.map_some]

theorem exec_ldr_b {t : Reg} {m : MemOp} (h : InRegions (s.rd ++ s.wr) (s.ea m) 1) :
    exec (.ldr .b t m) s = some (s.setReg t ((s.mem (s.ea m)).setWidth 64)) := by
  simp only [exec, load8_eq h, Option.map_some]

theorem exec_str_x {t : Reg} {m : MemOp} (h : InRegions s.wr (s.ea m) 8) :
    exec (.str .x t m) s = some { s with mem := s.mem.writeW (s.ea m) (s.gpr t) } := by
  simp only [exec, store64_eq h]

/-! ## Addresses of memory operands -/

/-- `[base]` -/
theorem ea_base (b : Reg) : s.ea { base := b } = s.gpr b := by
  simp [State.ea]

/-- `[base, #d]` -/
theorem ea_imm (b : Reg) (d : Int) : s.ea { base := b, off := .imm d } = s.gpr b + BitVec.ofInt 64 d :=
  rfl

/-- `[base, r]` -/
theorem ea_reg (b r : Reg) : s.ea { base := b, off := .reg r } = s.gpr b + s.gpr r := by
  simp [State.ea]

/-- `[base, r, lsl #sh]` -/
theorem ea_reg_lsl (b r : Reg) (sh : Nat) :
    s.ea { base := b, off := .reg r sh } = s.gpr b + s.gpr r <<< sh := rfl

/-- `[base, :lo12:t]` -/
theorem ea_lo12 (b : Reg) (t : Rel) :
    s.ea { base := b, off := .lo12 t } = s.gpr b + lo12 (s.relAddr t) := rfl

/-! ## Pages -/

/-- A page and the low 12 bits make the address. -/
theorem page_add_lo12 (a : Addr) : page a + lo12 a = a := by
  apply BitVec.eq_of_toNat_eq
  have := a.isLt
  simp only [page, lo12, BitVec.toNat_add, BitVec.toNat_ofNat]
  have h : a.toNat / 4096 * 4096 + a.toNat % 4096 = a.toNat := Nat.div_add_mod' a.toNat 4096
  omega

/-- An address on a page is its own page. -/
theorem page_of_aligned {a : Addr} (h : a.toNat % 4096 = 0) : page a = a := by
  apply BitVec.eq_of_toNat_eq
  have := a.isLt
  have h' : a.toNat / 4096 * 4096 = a.toNat := Nat.div_mul_cancel (Nat.dvd_of_mod_eq_zero h)
  simp only [page, BitVec.toNat_ofNat]
  omega

/-! ## The state after a store -/

theorem gpr_withMem (m : Mem) : ({ s with mem := m } : State).gpr = s.gpr := rfl
theorem rd_withMem (m : Mem) : ({ s with mem := m } : State).rd = s.rd := rfl
theorem wr_withMem (m : Mem) : ({ s with mem := m } : State).wr = s.wr := rfl
theorem mem_withMem (m : Mem) : ({ s with mem := m } : State).mem = m := rfl

end VG.AArch64.Access
