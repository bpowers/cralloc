module

public import Cralloc.Proof.Slab.AArch64.Run
public import Cralloc.Proof.Slab.Witness
public import Cralloc.Proof.Framework.Arm

/-!
# The CPU number on AArch64

`current_cpu` (`Impl.Slab.AArch64.currentCpu`) against `Spec.Slab.cpuContract`, inline
(`inlineAbi`): `__rseq_offset`'s address from the word holding it (`movGot`, which the model
gives as the symbol's address), `__rseq_offset`, the thread pointer, and `cpu_id`, zero-extended.
Not a restartable sequence, so it has no setup: its code is all it runs. And on musl, where the
area's distance from the thread pointer is in the instructions, `currentCpuMusl`, and `areaOffset`
against `Spec.Slab.areaOffsetContract`.
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

/-! ## On musl -/

namespace Musl

def bodyM : List Instr := [
  .mrsTp .x10,
  .addAreaHi .x10 .x10,
  .addAreaLo .x10 .x10,
  .ldr .w .x0 { base := .x10, off := .imm 4 }]

theorem code_eq : currentCpuMusl = .block bodyM := rfl

def μ1 (s : State) : State := s.setReg .x10 s.tpidr
def μ2 (s : State) : State := (μ1 s).setReg .x10 (s.tpidr + page (o s))
def μ3 (s : State) : State := (μ2 s).setReg .x10 (s.tpidr + o s)
def μ4 (s : State) : State := (μ3 s).setReg .x0 ((cpu32 s).setWidth 64)

section Run

variable {s : State}

theorem μ4_x0 : (μ4 s).gpr .x0 = (cpu32 s).setWidth 64 := RegUpd.gpr_setReg_self _ _ _

theorem gpr_μ4 {r : Reg} (h0 : ¬ r = .x0) (h10 : ¬ r = .x10) : (μ4 s).gpr r = s.gpr r := by
  rw [μ4, RegUpd.gpr_setReg_of_ne _ _ h0, μ3, RegUpd.gpr_setReg_of_ne _ _ h10, μ2,
    RegUpd.gpr_setReg_of_ne _ _ h10, μ1, RegUpd.gpr_setReg_of_ne _ _ h10]

theorem exec_body (hs : contract.pre s) : execBlock isa bodyM s = some (μ4 s) := by
  have hrd := rd_of hs
  have h1 : exec (.mrsTp .x10) s = some (μ1 s) := rfl
  have h2 : exec (.addAreaHi .x10 .x10) (μ1 s) = some (μ2 s) := by
    rw [Access.exec_addAreaHi, μ1, RegUpd.gpr_setReg_self]
    rfl
  have h3 : exec (.addAreaLo .x10 .x10) (μ2 s) = some (μ3 s) := by
    rw [Access.exec_addAreaLo, μ2, RegUpd.gpr_setReg_self]
    have ho : ((μ1 s).setReg .x10 (s.tpidr + page (o s))).areaOffset = o s := rfl
    rw [ho, BitVec.add_assoc, Access.page_add_lo12]
    rfl
  have h4 : exec (.ldr .w .x0 { base := .x10, off := .imm 4 }) (μ3 s) = some (μ4 s) := by
    have hea : (μ3 s).ea { base := .x10, off := .imm 4 } = area s + 4 :=
      ea_x10 (μ3 s) 4 (by rw [μ3, RegUpd.gpr_setReg_self]; rfl)
    have hin : InRegions ((μ3 s).rd ++ (μ3 s).wr) ((μ3 s).ea { base := .x10, off := .imm 4 }) 4 := by
      rw [hea]
      exact inRd (r := ⟨area s + 4, 4⟩) (by show _ ∈ s.rd; rw [hrd]; simp)
    rw [Access.exec_ldr_w hin, hea]
    rfl
  rw [bodyM, execBlock_cons_some h1, execBlock_cons_some h2, execBlock_cons_some h3,
    execBlock_cons_some h4, execBlock_nil']

theorem post_ok : contract.post s (μ4 s) := by
  simp only [contract, cpuContract, Sig.contract, Abi.withExtra, hv, Curry.apply_const, true_and]
  refine ⟨?_, rfl⟩
  show (μ4 s).gpr .x0 = _
  rw [μ4_x0]
  apply BitVec.eq_of_toNat_eq
  simp only [BitVec.toNat_setWidth, BitVec.toNat_ofNat, Spec.Slab.Ctx.cpu, view_cpuField,
    mem_of (c := cpuView inlineAbi) hX, cpu32]

theorem correct (hs : contract.pre s) :
    ∃ s', Exec isa currentCpuMusl s s' ∧ s'.sp = s.sp ∧ contract.post s s' :=
  ⟨μ4 s, by rw [code_eq]; exact .block (exec_body hs), rfl, post_ok⟩

/-- The registers the code changes. `x0` is also its result. -/
def clobbers : List String := ["x10"]

/-- No other register changes. -/
theorem clobbersOk (hs : contract.pre s) :
    (∀ s', Exec isa currentCpuMusl s s' → preservedExcept (clobbers ++ ["x0"]) s s') ∧
    (∀ t s', ExecExit isa currentCpuMusl s t s' → preservedExcept clobbers s s') ∧
    (∀ p ∈ currentCpuMusl.abortStates isa s, preservedExcept clobbers s p) := by
  rw [code_eq]
  refine ⟨fun s' he => ?_, (fun t s' he => by cases he), fun p hp => by simp [Code.abortStates] at hp⟩
  cases he with
  | block h =>
    rw [exec_body hs] at h
    cases h
    intro r hr
    exact gpr_μ4 (by rintro rfl; simp [Reg.x, clobbers] at hr) (by rintro rfl; simp [Reg.x, clobbers] at hr)

end Run

theorem verified : Verified inlineTarget currentCpuMusl contract :=
  ⟨fun _ hs => .inl (correct hs),
    fun _ _ => RseqOk.of_ne (by rw [code_eq]; intro _ _ h; cases h)
      (by rw [code_eq]; intro _ _ _ h; cases h),
    ⟨wit, pre_wit⟩, fun o ho => by rw [code_eq] at ho; cases ho⟩

theorem witnessOk : (∃ w ∈ [wit], contract.pre w) ∧
    ∀ o ∈ currentCpuMusl.outcomes, ∃ w ∈ [wit], contract.pre w ∧ Visits isa currentCpuMusl w o.1 o.2 :=
  ⟨⟨_, List.mem_singleton_self _, pre_wit⟩, fun o ho => by rw [code_eq] at ho; cases ho⟩

end Musl

/-! ## The area's offset -/

namespace Offset

abbrev contract : Contract isa := areaOffsetContract inlineAbi

theorem hv : inlineAbi.args ((areaOffsetSig.words inlineAbi.ptrBits).map (·.bits inlineAbi.ptrBits)) =
    some fun _ => [] := rfl

def bodyO : List Instr := [.mov .x .x0 (.imm 0), .addAreaHi .x0 .x0, .addAreaLo .x0 .x0]

theorem code_eq : areaOffset = .block bodyO := rfl

def ν1 (s : State) : State := s.setReg .x0 (BitVec.ofNat 64 0)
def ν2 (s : State) : State := (ν1 s).setReg .x0 (page (o s))
def ν3 (s : State) : State := (ν2 s).setReg .x0 (o s)

section Run

variable {s : State}

theorem gpr_ν3 {r : Reg} (h0 : ¬ r = .x0) : (ν3 s).gpr r = s.gpr r := by
  rw [ν3, RegUpd.gpr_setReg_of_ne _ _ h0, ν2, RegUpd.gpr_setReg_of_ne _ _ h0, ν1,
    RegUpd.gpr_setReg_of_ne _ _ h0]

theorem exec_body : execBlock isa bodyO s = some (ν3 s) := by
  have h1 : exec (.mov .x .x0 (.imm 0)) s = some (ν1 s) := rfl
  have h2 : exec (.addAreaHi .x0 .x0) (ν1 s) = some (ν2 s) := by
    rw [Access.exec_addAreaHi, ν1, RegUpd.gpr_setReg_self]
    show some _ = some (((s.setReg .x0 (BitVec.ofNat 64 0))).setReg .x0 (page (o s)))
    have h0 : ∀ a : Addr, BitVec.ofNat 64 0 + a = a := fun a => BitVec.zero_add a
    rw [h0]
    rfl
  have h3 : exec (.addAreaLo .x0 .x0) (ν2 s) = some (ν3 s) := by
    rw [Access.exec_addAreaLo, ν2, RegUpd.gpr_setReg_self]
    have ho : ((ν1 s).setReg .x0 (page (o s))).areaOffset = o s := rfl
    rw [ho, Access.page_add_lo12]
    rfl
  rw [bodyO, execBlock_cons_some h1, execBlock_cons_some h2, execBlock_cons_some h3, execBlock_nil']

theorem post_ok : contract.post s (ν3 s) := by
  simp only [contract, areaOffsetContract, Sig.contract, Abi.withExtra, hv, Curry.apply_const, true_and]
  refine ⟨?_, rfl⟩
  show (ν3 s).gpr .x0 = _
  rw [ν3, RegUpd.gpr_setReg_self]
  rfl

theorem correct : ∃ s', Exec isa areaOffset s s' ∧ s'.sp = s.sp ∧ contract.post s s' :=
  ⟨ν3 s, by rw [code_eq]; exact .block exec_body, rfl, post_ok⟩

/-- The registers the code changes, but for its result, `x0`. -/
def clobbers : List String := []

/-- No other register changes. -/
theorem clobbersOk :
    (∀ s', Exec isa areaOffset s s' → preservedExcept (clobbers ++ ["x0"]) s s') ∧
    (∀ t s', ExecExit isa areaOffset s t s' → preservedExcept clobbers s s') ∧
    (∀ p ∈ areaOffset.abortStates isa s, preservedExcept clobbers s p) := by
  rw [code_eq]
  refine ⟨fun s' he => ?_, (fun t s' he => by cases he), fun p hp => by simp [Code.abortStates] at hp⟩
  cases he with
  | block h =>
    rw [exec_body] at h
    cases h
    intro r hr
    exact gpr_ν3 (by rintro rfl; simp [Reg.x, clobbers] at hr)

end Run

/-- A thread whose rseq area is at `0x100` from its thread pointer. -/
def witO : State where
  gpr _ := 0
  mem := mkMem [(0x2000, 0x100)]
  rd := [⟨0x2000, 8⟩]
  wr := []
  syms n := if n = "__rseq_offset" then 0x2000 else 0

theorem pre_wit : contract.pre witO :=
  (areaOffset_pre_iff inlineAbi hv rfl rfl).mpr ⟨trivial, by decide +kernel, by decide +kernel,
    nowrap_of_check (by decide +kernel), by simp [hX.reserved], rfl⟩

theorem verified : Verified inlineTarget areaOffset contract :=
  ⟨fun _ _ => .inl correct,
    fun _ _ => RseqOk.of_ne (by rw [code_eq]; intro _ _ h; cases h)
      (by rw [code_eq]; intro _ _ _ h; cases h),
    ⟨witO, pre_wit⟩, fun o ho => by rw [code_eq] at ho; cases ho⟩

theorem witnessOk : (∃ w ∈ [witO], contract.pre w) ∧
    ∀ o ∈ areaOffset.outcomes, ∃ w ∈ [witO], contract.pre w ∧ Visits isa areaOffset w o.1 o.2 :=
  ⟨⟨_, List.mem_singleton_self _, pre_wit⟩, fun o ho => by rw [code_eq] at ho; cases ho⟩

end Offset

end VG.Proof.Slab.AArch64.Cpu
