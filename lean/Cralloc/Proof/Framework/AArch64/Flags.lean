module

public import Cralloc.TCB.AArch64.Isa
public import Cralloc.Proof.Framework.AArch64.RegUpd

/-!
# AArch64: flags and conditions, for symbolic execution

What a compare and a test leave in the flags, and what each condition then
reads. The model transcribes `AddWithCarry`; a proof wants "above or same" and
"equal", which these say.
-/

@[expose] public section

namespace VG.AArch64.Flags

variable (s : State)

/-- `x + NOT(y) + 1`, the result of a compare, is the difference. -/
theorem add_not_add_one {w : Nat} (x y : BitVec w) :
    x + ~~~y + BitVec.ofNat w (Bool.toNat true) = x - y := by
  have h : ~~~y + BitVec.ofNat w 1 = -y := by
    rw [BitVec.neg_eq_not_add]
  rw [BitVec.add_assoc, Bool.toNat_true, h, BitVec.sub_eq_add_neg]

/-- A compare's carry is "no borrow". -/
theorem carry_cmp {w : Nat} (x y : BitVec w) :
    2 ^ w ≤ x.toNat + (~~~y).toNat + Bool.toNat true ↔ y.toNat ≤ x.toNat := by
  have := y.isLt
  rw [BitVec.toNat_not, Bool.toNat_true]
  omega

theorem cf_cmp {w : Nat} (x y : BitVec w) :
    (s.addFlags x (~~~y) true).cf = decide (y.toNat ≤ x.toNat) := by
  simp only [State.addFlags, RegUpd.cf_setFlags, carry_cmp]

theorem zf_cmp {w : Nat} (x y : BitVec w) :
    (s.addFlags x (~~~y) true).zf = ((x - y) == 0) := by
  simp only [State.addFlags, RegUpd.zf_setFlags, add_not_add_one]

/-- `exec`, a compare at a literal size: the flags alone. -/
theorem exec_cmp (sz : Size) (n : Reg) (src : Src) :
    exec (.cmp sz n src) s = some (s.addFlags (s.read sz n) (~~~(s.src sz src)) true) := rfl

/-- The flags of a test. -/
abbrev tstState {w : Nat} (r : BitVec w) : State := s.setFlags r.msb (r == 0) false false

theorem exec_tst (sz : Size) (n : Reg) (src : Src) :
    exec (.tst sz n src) s = some (tstState s (s.read sz n &&& s.src sz src)) := rfl

/-! ## Conditions -/

theorem eval_eq : eval .eq s = some s.zf := rfl
theorem eval_ne : eval .ne s = some !s.zf := rfl
theorem eval_hs : eval .hs s = some s.cf := rfl
theorem eval_lo : eval .lo s = some !s.cf := rfl
theorem eval_hi : eval .hi s = some (s.cf && !s.zf) := rfl
theorem eval_ls : eval .ls s = some !(s.cf && !s.zf) := rfl
theorem eval_cbz (sz : Size) (r : Reg) : eval (.cbz sz r) s = some (s.read sz r == 0) := rfl
theorem eval_cbnz (sz : Size) (r : Reg) : eval (.cbnz sz r) s = some (s.read sz r != 0) := rfl
theorem eval_tbz (r : Reg) (b : Fin 64) : eval (.tbz r b) s = some !((s.gpr r).getLsbD b) := rfl
theorem eval_tbnz (r : Reg) (b : Fin 64) : eval (.tbnz r b) s = some ((s.gpr r).getLsbD b) := rfl

/-- `b.hs` after a compare. -/
theorem eval_hs_cmp {w : Nat} (x y : BitVec w) :
    eval .hs (s.addFlags x (~~~y) true) = some (decide (y.toNat ≤ x.toNat)) := by
  rw [eval_hs, cf_cmp]

/-- `b.eq` after a test. -/
theorem eval_eq_tst {w : Nat} (r : BitVec w) : eval .eq (tstState s r) = some (r == 0) := rfl

/-- `b.ne` after a compare. -/
theorem eval_ne_cmp {w : Nat} (x y : BitVec w) :
    eval .ne (s.addFlags x (~~~y) true) = some !((x - y) == 0) := by
  rw [eval_ne, zf_cmp]

end VG.AArch64.Flags
