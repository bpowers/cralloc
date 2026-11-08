module

public import Cralloc.Proof.Slab.X86_64.Witness
public import Cralloc.Proof.Slab.X86_64.Run
public import Cralloc.Proof.Framework.Arm

/-!
# The CPU number on x86-64

`current_cpu` (`Impl.Slab.X86_64.currentCpu`) against `Spec.Slab.cpuContract`,
inline (`inlineAbi`): three loads, `__rseq_offset`'s
address from the word holding it (`movGot`, which the model gives as the symbol's
address), `__rseq_offset`, and `cpu_id` through `fs`, zero-extended.
-/

@[expose] public section

namespace VG.Proof.Slab.X86_64.Cpu

open VG.X86_64 VG.Spec.Slab VG.Impl.Slab.X86_64 VG.Proof.Slab.X86_64

theorem hv : abi.args ((cpuSig.words 64).map (·.bits 64)) = some fun _ => [] := rfl

/-! ## The run -/

/-- `fs:[rax + 4]`. -/
def cpuOp : MemOp := { base := .rax, disp := 4, seg := some .fs }

def body : List Instr :=
  [.movGot .rax RS, .mov .rax (.mem { base := .rax }), .mov32 .rax (.mem cpuOp)]

theorem code_eq : currentCpu = .block body := rfl

/-- After the three loads. -/
def σ1 (s : State) : State := s.setReg .rax (s.syms RS)
def σ2 (s : State) : State := (σ1 s).setReg .rax (o s)
def σ3 (s : State) : State := (σ2 s).setReg32 .rax (cpu32 s)

theorem σ3_rax {s : State} : (σ3 s).gpr .rax = (cpu32 s).setWidth 64 := by
  simp [σ3, State.setReg32, RegUpd.gpr_setReg_self]

theorem gpr_σ3 {s : State} (r : Reg) (hr : r ≠ .rax) : (σ3 s).gpr r = s.gpr r := by
  simp only [σ3, σ2, σ1, State.setReg32, RegUpd.gpr_setReg_of_ne _ _ hr]

theorem mem_σ3 {s : State} : (σ3 s).mem = s.mem := rfl

section Run

variable {A : Abi isa} (hA : X86 A) {s : State}
include hA

theorem view_rsVar : (cpuView A).rsVar s = ⟨s.syms RS, 8⟩ := rsVar_of (c := cpuView A) hA s
theorem view_cpuField : (cpuView A).cpuField s = ⟨area s + 4, 4⟩ :=
  cpuField_of (c := cpuView A) hA s

theorem hvA : A.args ((cpuSig.words A.ptrBits).map (·.bits A.ptrBits)) = some fun _ => [] := by
  rw [hA.args, hA.ptrBits]; exact hv

theorem rd_of (hs : (cpuContract A).pre s) : s.rd = [⟨s.syms RS, 8⟩, ⟨area s + 4, 4⟩] := by
  have h := ((cpu_pre_iff A (hvA hA) rfl (by rw [hA.argArea, hA.ptrBits]; rfl)).mp hs).2.1
  rw [hA.rd] at h
  rw [h]
  simp [cpuRegions, view_rsVar hA, view_cpuField hA]

theorem exec_body (hs : (cpuContract A).pre s) : execBlock isa body s = some (σ3 s) := by
  have hrd := rd_of hA hs
  have h1 : exec (.movGot .rax RS) s = some (σ1 s) := rfl
  have h2 : exec (.mov .rax (.mem { base := .rax })) (σ1 s) = some (σ2 s) := by
    have hin : InRegions ((σ1 s).rd ++ (σ1 s).wr) ((σ1 s).ea { base := .rax }) 8 := by
      rw [Access.ea_base, σ1, RegUpd.gpr_setReg_self, RegUpd.rd_setReg, RegUpd.wr_setReg]
      exact inRd (r := ⟨s.syms RS, 8⟩) (by rw [hrd]; simp)
    rw [exec_load _ hin, Access.ea_base, σ1, RegUpd.gpr_setReg_self, RegUpd.mem_setReg]; rfl
  have h3 : exec (.mov32 .rax (.mem cpuOp)) (σ2 s) = some (σ3 s) := by
    have hea : (σ2 s).ea cpuOp = area s + 4 := by
      rw [cpuOp, Access.ea_fs_base_disp, σ2, RegUpd.gpr_setReg_self, area_eq, BitVec.add_assoc]; rfl
    have hin : InRegions ((σ2 s).rd ++ (σ2 s).wr) ((σ2 s).ea cpuOp) 4 := by
      rw [hea, show (σ2 s).rd = s.rd from rfl, show (σ2 s).wr = s.wr from rfl]
      exact inRd (r := ⟨area s + 4, 4⟩) (by rw [hrd]; simp)
    rw [Access.exec_mov32_mem hin, hea]; rfl
  rw [body, execBlock_cons_some h1, execBlock_cons_some h2, execBlock_cons_some h3, execBlock_nil']

theorem post : (cpuContract A).post s (σ3 s) := by
  simp only [cpuContract, Sig.contract, Abi.withExtra, hvA hA, Curry.apply_const, true_and]
  refine ⟨?_, by rw [hA.mem, hA.mem]; rfl⟩
  rw [hA.ret, σ3_rax]
  apply BitVec.eq_of_toNat_eq
  simp only [BitVec.toNat_setWidth, BitVec.toNat_ofNat, Spec.Slab.Ctx.cpu, view_cpuField hA,
    mem_of (c := cpuView A) hA, cpu32]

/-- Reading `__rseq_offset` directly: the same end state. -/
theorem exec_bodyD (hs : (cpuContract A).pre s) :
    execBlock isa [.movSym .rax RS, .mov32 .rax (.mem cpuOp)] s = some (σ3 s) := by
  have hrd := rd_of hA hs
  have h1 : exec (.movSym .rax RS) s = some (σ2 s) := by
    have hin : InRegions (s.rd ++ s.wr) (s.syms RS) 8 := inRd (r := ⟨s.syms RS, 8⟩) (by rw [hrd]; simp)
    simp only [exec, State.load64, hin, ↓reduceIte, Option.map_some, σ2, σ1, setReg_setReg_same]
    rfl
  have h2 : exec (.mov .rax (.mem { base := .rax })) (σ1 s) = some (σ2 s) := by
    have hin : InRegions ((σ1 s).rd ++ (σ1 s).wr) ((σ1 s).ea { base := .rax }) 8 := by
      rw [Access.ea_base, σ1, RegUpd.gpr_setReg_self, RegUpd.rd_setReg, RegUpd.wr_setReg]
      exact inRd (r := ⟨s.syms RS, 8⟩) (by rw [hrd]; simp)
    rw [exec_load _ hin, Access.ea_base, σ1, RegUpd.gpr_setReg_self, RegUpd.mem_setReg]; rfl
  have h3 := exec_body hA hs
  rw [body, execBlock_cons_some rfl, execBlock_cons_some h2] at h3
  rw [execBlock_cons_some h1]
  exact h3

theorem same_direct (hs : (cpuContract A).pre s) :
    execBlock isa [.movSym .rax RS, .mov32 .rax (.mem cpuOp)] s = execBlock isa body s := by
  rw [exec_bodyD hA hs, exec_body hA hs]

end Run

theorem rseqOk {T : Target} {k : Contract T.isa} {c : Prog T.isa} {s : T.isa.State}
    (h : ∀ arm body, c ≠ .rseq arm body) (h' : ∀ arm body c₂, c ≠ .seq (.rseq arm body) c₂) :
    RseqOk T c k s := RseqOk.of_ne h h'

/-! ## Inline -/

theorem correct_inline {s : State} (hs : (cpuContract inlineAbi).pre s) :
    ∃ s', Exec isa currentCpu s s' ∧ s'.gpr .rsp = s.gpr .rsp ∧
      (cpuContract inlineAbi).post s s' :=
  ⟨σ3 s, by rw [code_eq]; exact .block (exec_body x86_inline hs), gpr_σ3 _ (by decide),
    post x86_inline⟩

/-- No register but the result's changes. -/
theorem clobbersOk {s : State} (hs : (cpuContract inlineAbi).pre s) :
    (∀ s', Exec isa currentCpu s s' → preservedExcept ([] ++ ["rax"]) s s') ∧
    (∀ t s', ExecExit isa currentCpu s t s' → preservedExcept [] s s') ∧
    (∀ p ∈ currentCpu.abortStates isa s, preservedExcept [] s p) := by
  rw [code_eq]
  refine ⟨fun s' he => ?_, (fun t s' he => by cases he), fun p hp => by simp [Code.abortStates] at hp⟩
  cases he with
  | block h =>
    rw [exec_body x86_inline hs] at h
    cases h
    intro r hr
    exact gpr_σ3 r (by rintro rfl; simp [Reg.name] at hr)

/-! ## A witness -/

/-- A thread whose rseq area is at `0x10100` and whose CPU number is 3. -/
def wit : State where
  gpr r := if r = .rsp then 0x7000000 else 0
  cf := none
  zf := none
  sf := none
  of := none
  mem := mkMem [(0x2000, 0x100), (0x10104, 3)]
  rd := [⟨0x2000, 8⟩, ⟨0x10104, 4⟩]
  wr := []
  syms n := if n = "__rseq_offset" then 0x2000 else 0
  fsBase := 0x10000
  csDesc := 0

theorem pre_wit {A : Abi isa} (hA : X86 A)
    (hrd : wit.rd = ((cpuRegions A wit).filter (!·.2)).map (·.1))
    (hres : ((cpuRegions A wit).all fun x => (A.reserved 0 wit).all fun q => Region.sep x.1 q) = true)
    (hnw : ((cpuRegions A wit).all fun x => decide (x.1.base.toNat + x.1.len ≤ 2 ^ 64)) = true) :
    (cpuContract A).pre wit := by
  refine (cpu_pre_iff A (hvA hA) rfl (by rw [hA.argArea, hA.ptrBits]; rfl)).mpr
    ⟨by rw [hA.wf, hA.ptrBits]; simp [abi, cpuSig, Sig.words], by rw [hA.rd]; exact hrd,
      by rw [hA.wr]; simp [cpuRegions, wit], by rw [hA.ptrBits]; exact nowrap_of_check hnw,
      fun x hx q hq => ?_, by rw [hA.tp]; rfl, by rw [hA.sym]; rfl⟩
  simp only [List.all_eq_true] at hres
  exact Region.disjoint_of_sep (hres x hx q hq)

theorem pre_wit_inline : (cpuContract inlineAbi).pre wit :=
  pre_wit x86_inline (by decide +kernel) (by decide +kernel) (by decide +kernel)

theorem verified_inline : Verified inlineTarget currentCpu (cpuContract inlineAbi) :=
  ⟨fun _ hs => .inl (correct_inline hs), fun _ _ => rseqOk (by rw [code_eq]; intro _ _ h; cases h)
    (by rw [code_eq]; intro _ _ _ h; cases h),
    ⟨wit, pre_wit_inline⟩, fun o ho => by rw [code_eq] at ho; cases ho⟩

/-! ## Reading `__rseq_offset` directly -/

theorem codeD_eq : currentCpuDirect = .block [.movSym .rax RS, .mov32 .rax (.mem cpuOp)] := rfl

theorem verifiedD_inline : Verified inlineTarget currentCpuDirect (cpuContract inlineAbi) := by
  rw [codeD_eq]; exact Verified.of_block verified_inline fun s hs => same_direct x86_inline hs

theorem clobbersOkD {s : State} (hs : (cpuContract inlineAbi).pre s) :
    (∀ s', Exec isa currentCpuDirect s s' → preservedExcept ([] ++ ["rax"]) s s') ∧
    (∀ t s', ExecExit isa currentCpuDirect s t s' → preservedExcept [] s s') ∧
    (∀ p ∈ currentCpuDirect.abortStates isa s, preservedExcept [] s p) := by
  rw [codeD_eq]
  refine ⟨fun s' he => ?_, (fun t s' he => by cases he), fun p hp => by simp [Code.abortStates] at hp⟩
  cases he with
  | block h =>
    rw [exec_bodyD x86_inline hs] at h
    cases h
    intro r hr
    exact gpr_σ3 r (by rintro rfl; simp [Reg.name] at hr)

end VG.Proof.Slab.X86_64.Cpu
