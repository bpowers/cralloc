module

public import Cralloc.Proof.Slab.AArch64.Run
public import Cralloc.Proof.Slab.Witness
public import Cralloc.Proof.Framework.Arm

/-!
# The CPU number on AArch64

`current_cpu` (`Impl.Slab.AArch64.currentCpu`) against `Spec.Slab.cpuContract`, inline
(`inlineAbi`): `__rseq_offset`'s address from the word holding it (`movGot`, which the model
gives as the symbol's address), `__rseq_offset`, the thread pointer, and `cpu_id`, zero-extended.
Not a restartable sequence, so it has no setup: its code is all it runs.
-/

@[expose] public section

namespace VG.Proof.Slab.AArch64.Cpu

open VG.AArch64 VG.Spec.Slab VG.Impl.Slab.AArch64 VG.Proof.Slab.AArch64

abbrev contract : Contract isa := cpuContract inlineAbi

theorem hX : A64 inlineAbi inlineArgRegs := a64_inline

theorem hv : inlineAbi.args ((cpuSig.words inlineAbi.ptrBits).map (·.bits inlineAbi.ptrBits)) =
    some fun _ => [] := rfl

def body : List Instr := [
  .movGot .x9 RS,
  .ldr .x .x9 { base := .x9 },
  .mrsTp .x10,
  .add .x .x10 .x10 (.reg .x9),
  .ldr .w .x0 { base := .x10, off := .imm 4 }]

theorem code_eq : currentCpu = .block body := rfl

/-! ## The run -/

def σ1 (s : State) : State := s.setReg .x9 (s.syms RS)
def σ2 (s : State) : State := (σ1 s).setReg .x9 (o s)
def σ3 (s : State) : State := (σ2 s).setReg .x10 s.tpidr
def σ4 (s : State) : State := (σ3 s).setReg .x10 (s.tpidr + o s)
def σ5 (s : State) : State := (σ4 s).setReg .x0 ((cpu32 s).setWidth 64)

theorem σ5_x0 {s : State} : (σ5 s).gpr .x0 = (cpu32 s).setWidth 64 := RegUpd.gpr_setReg_self _ _ _

theorem gpr_σ5 {s : State} {r : Reg} (h0 : ¬ r = .x0) (h9 : ¬ r = .x9) (h10 : ¬ r = .x10) :
    (σ5 s).gpr r = s.gpr r := by
  rw [σ5, RegUpd.gpr_setReg_of_ne _ _ h0, σ4, RegUpd.gpr_setReg_of_ne _ _ h10, σ3,
    RegUpd.gpr_setReg_of_ne _ _ h10, σ2, RegUpd.gpr_setReg_of_ne _ _ h9, σ1,
    RegUpd.gpr_setReg_of_ne _ _ h9]

theorem σ5_sp {s : State} : (σ5 s).sp = s.sp := rfl
theorem σ5_mem {s : State} : (σ5 s).mem = s.mem := rfl

section Run

variable {s : State}

theorem view_rsVar : (cpuView inlineAbi).rsVar s = ⟨s.syms RS, 8⟩ :=
  rsVar_of (c := cpuView inlineAbi) hX s
theorem view_cpuField : (cpuView inlineAbi).cpuField s = ⟨area s + 4, 4⟩ :=
  cpuField_of (c := cpuView inlineAbi) hX s

theorem rd_of (hs : contract.pre s) : s.rd = [⟨s.syms RS, 8⟩, ⟨area s + 4, 4⟩] := by
  have h := ((cpu_pre_iff inlineAbi hv rfl rfl).mp hs).2.1
  have h' : s.rd = ((cpuRegions inlineAbi s).filter (!·.2)).map (·.1) := h
  rw [h']
  simp [cpuRegions, view_rsVar, view_cpuField]

theorem exec_body (hs : contract.pre s) : execBlock isa body s = some (σ5 s) := by
  have hrd := rd_of hs
  have h1 : exec (.movGot .x9 RS) s = some (σ1 s) := rfl
  have h2 : exec (.ldr .x .x9 { base := .x9 }) (σ1 s) = some (σ2 s) := by
    have hea : (σ1 s).ea { base := .x9 } = s.syms RS := by
      rw [Access.ea_base, σ1, RegUpd.gpr_setReg_self]
    have hin : InRegions ((σ1 s).rd ++ (σ1 s).wr) ((σ1 s).ea { base := .x9 }) 8 := by
      rw [hea]
      exact inRd (r := ⟨s.syms RS, 8⟩) (by show _ ∈ s.rd; rw [hrd]; simp)
    rw [Access.exec_ldr_x hin, hea]
    rfl
  have h3 : exec (.mrsTp .x10) (σ2 s) = some (σ3 s) := rfl
  have h4 : exec (.add .x .x10 .x10 (.reg .x9)) (σ3 s) = some (σ4 s) := by
    show some ((σ3 s).setReg .x10 ((σ3 s).gpr .x10 + (σ3 s).gpr .x9)) = _
    rw [σ3, RegUpd.gpr_setReg_self, RegUpd.gpr_setReg_of_ne _ _ (by decide), σ2,
      RegUpd.gpr_setReg_self]
    rfl
  have h5 : exec (.ldr .w .x0 { base := .x10, off := .imm 4 }) (σ4 s) = some (σ5 s) := by
    have hea : (σ4 s).ea { base := .x10, off := .imm 4 } = area s + 4 :=
      ea_x10 (σ4 s) 4 (by rw [σ4, RegUpd.gpr_setReg_self]; rfl)
    have hin : InRegions ((σ4 s).rd ++ (σ4 s).wr) ((σ4 s).ea { base := .x10, off := .imm 4 }) 4 := by
      rw [hea]
      exact inRd (r := ⟨area s + 4, 4⟩) (by show _ ∈ s.rd; rw [hrd]; simp)
    rw [Access.exec_ldr_w hin, hea]
    rfl
  rw [body, execBlock_cons_some h1, execBlock_cons_some h2, execBlock_cons_some h3,
    execBlock_cons_some h4, execBlock_cons_some h5, execBlock_nil']

theorem post_ok : contract.post s (σ5 s) := by
  simp only [contract, cpuContract, Sig.contract, Abi.withExtra, hv, Curry.apply_const, true_and]
  refine ⟨?_, rfl⟩
  show (σ5 s).gpr .x0 = _
  rw [σ5_x0]
  apply BitVec.eq_of_toNat_eq
  simp only [BitVec.toNat_setWidth, BitVec.toNat_ofNat, Spec.Slab.Ctx.cpu, view_cpuField,
    mem_of (c := cpuView inlineAbi) hX, cpu32]

theorem correct (hs : contract.pre s) :
    ∃ s', Exec isa currentCpu s s' ∧ s'.sp = s.sp ∧ contract.post s s' :=
  ⟨σ5 s, by rw [code_eq]; exact .block (exec_body hs), σ5_sp, post_ok⟩

/-- The registers the code changes. `x0` is also its result. -/
def clobbers : List String := ["x9", "x10"]

/-- No other register changes. -/
theorem clobbersOk (hs : contract.pre s) :
    (∀ s', Exec isa currentCpu s s' → preservedExcept (clobbers ++ ["x0"]) s s') ∧
    (∀ t s', ExecExit isa currentCpu s t s' → preservedExcept clobbers s s') ∧
    (∀ p ∈ currentCpu.abortStates isa s, preservedExcept clobbers s p) := by
  rw [code_eq]
  refine ⟨fun s' he => ?_, (fun t s' he => by cases he), fun p hp => by simp [Code.abortStates] at hp⟩
  cases he with
  | block h =>
    rw [exec_body hs] at h
    cases h
    intro r hr
    exact gpr_σ5 (by rintro rfl; simp [Reg.x, clobbers] at hr) (by rintro rfl; simp [Reg.x, clobbers] at hr)
      (by rintro rfl; simp [Reg.x, clobbers] at hr)

end Run

/-! ## A witness -/

/-- A thread whose rseq area is at `0x10100` and whose CPU number is 3. -/
def wit : State where
  gpr _ := 0
  mem := mkMem [(0x2000, 0x100), (0x10104, 3)]
  rd := [⟨0x2000, 8⟩, ⟨0x10104, 4⟩]
  wr := []
  syms n := if n = "__rseq_offset" then 0x2000 else 0
  tpidr := 0x10000
  csDesc := 0

theorem pre_wit : contract.pre wit :=
  (cpu_pre_iff inlineAbi hv rfl rfl).mpr ⟨trivial, by decide +kernel, by decide +kernel,
    nowrap_of_check (by decide +kernel), by simp [hX.reserved], rfl, rfl⟩

theorem verified : Verified inlineTarget currentCpu contract :=
  ⟨fun _ hs => .inl (correct hs),
    fun _ _ => RseqOk.of_ne (by rw [code_eq]; intro _ _ h; cases h)
      (by rw [code_eq]; intro _ _ _ h; cases h),
    ⟨wit, pre_wit⟩, fun o ho => by rw [code_eq] at ho; cases ho⟩

theorem witnessOk : (∃ w ∈ [wit], contract.pre w) ∧
    ∀ o ∈ currentCpu.outcomes, ∃ w ∈ [wit], contract.pre w ∧ Visits isa currentCpu w o.1 o.2 :=
  ⟨⟨_, List.mem_singleton_self _, pre_wit⟩, fun o ho => by rw [code_eq] at ho; cases ho⟩

end VG.Proof.Slab.AArch64.Cpu
