module

public import Asmalloc.Proof.Slab.X86_64.Run

/-!
# The inline routines' slab word on x86-64

`lea r9, [rip + SLABS + coff]` and `mov rcx, [r9 + r8]`: the class's word in
the cid's slab, the class's offset `coff` being a constant.
-/

@[expose] public section

namespace VG.Proof.Slab.X86_64

open VG.X86_64 VG.Spec.Slab VG.Impl.Slab.X86_64

/-- The class's word in slab 0. -/
def r9i (s : State) : Addr := s.syms "SLABS" + s.params "coff"
/-- The slab word's address, and the word. -/
def wai (s : State) : Addr := r9i s + sh s
def wvi (s : State) : BitVec 64 := s.mem.readW (wai s) 64

def σ4i (s : State) : State := (σ3 s).setReg .r9 (r9i s)
def σ5i (s : State) : State := (σ4i s).setReg .rcx (wvi s)

section

variable {c : Spec.Slab.Ctx isa} {s : State}

theorem σ4i_mem : (σ4i s).mem = m1 s := rfl
theorem σ4i_rd : (σ4i s).rd = s.rd := rfl
theorem σ4i_wr : (σ4i s).wr = s.wr := rfl
theorem σ5i_mem : (σ5i s).mem = m1 s := rfl
theorem σ5i_rd : (σ5i s).rd = s.rd := rfl
theorem σ5i_wr : (σ5i s).wr = s.wr := rfl
theorem σ5i_params : (σ5i s).params = s.params := rfl
theorem σ5i_rcx : (σ5i s).gpr .rcx = wvi s := RegUpd.gpr_setReg_self _ _ _

theorem exec_r9i : exec (.leaSym .r9 "SLABS" 0 (some "coff")) (σ3 s) = some (σ4i s) := by
  rw [exec_leaSym, σ3_syms, σ3_params]
  simp only [Option.map_some, Option.getD_some, BitVec.ofInt_ofNat, BitVec.ofNat_eq_ofNat,
    BitVec.add_zero]
  rfl

/-- `[r9 + r8]`, in any state where they hold the class's word in slab 0 and
the cid's slab's offset. -/
theorem slabWord_ea_of (σ : State) (h9 : σ.gpr .r9 = r9i s) (h8 : σ.gpr .r8 = sh s) :
    σ.ea slabWord = wai s := by
  rw [slabWord, Access.ea_of_seg_none rfl, Access.offset_index, h9, h8]
  simp only [BitVec.ofInt_ofNat, BitVec.mul_one, BitVec.add_zero]
  rfl

theorem σ4i_r9 : (σ4i s).gpr .r9 = r9i s := RegUpd.gpr_setReg_self _ _ _
theorem σ4i_r8 : (σ4i s).gpr .r8 = sh s := by
  rw [σ4i, RegUpd.gpr_setReg_of_ne _ _ (by decide), gpr_σ3_r8]
theorem σ5i_r9 : (σ5i s).gpr .r9 = r9i s := by
  rw [σ5i, RegUpd.gpr_setReg_of_ne _ _ (by decide), σ4i_r9]
theorem σ5i_r8 : (σ5i s).gpr .r8 = sh s := by
  rw [σ5i, RegUpd.gpr_setReg_of_ne _ _ (by decide), σ4i_r8]

theorem slabWord_ea_i : (σ4i s).ea slabWord = wai s := by
  rw [slabWord, Access.ea_of_seg_none rfl, Access.offset_index]
  simp only [σ4i, RegUpd.gpr_setReg_self, BitVec.ofInt_ofNat, BitVec.mul_one, BitVec.add_zero]
  rw [RegUpd.gpr_setReg_of_ne _ _ (by decide), gpr_σ3_r8]; rfl

/-- The slab word's address is the class's for the cid, an inline routine
being given its class's offset. -/
theorem wai_eq (hA : c.A = abi) (hcoff : (s.params "coff").toNat = 8 * c.cls s) :
    wai s = c.word s := by
  rw [Spec.Slab.Ctx.word, wordAddr, sym_of hA, cid_of hA, wai, r9i, sh_eq, cid32]
  apply BitVec.eq_of_toNat_eq
  simp only [BitVec.toNat_add, BitVec.toNat_ofNat, hcoff, SLABS]
  omega

theorem exec_wordi (h : Facts c s) (hi : c.InRange s) (hw : wai s = c.word s) :
    exec (.mov .rcx (.mem slabWord)) (σ4i s) = some (σ5i s) := by
  have hin : InRegions ((σ4i s).rd ++ (σ4i s).wr) ((σ4i s).ea slabWord) 8 := by
    rw [slabWord_ea_i, σ4i_rd, σ4i_wr, hw]; exact inRdWr (r := ⟨c.word s, 8⟩) (h.word hi)
  rw [exec_load _ hin, slabWord_ea_i, σ4i_mem, m1,
    readW_writeW_disj (w := 64) (by rw [hw]; exact (h.cs_word hi).symm) (by decide)]
  rfl

end

end VG.Proof.Slab.X86_64
