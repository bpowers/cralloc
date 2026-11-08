import Cralloc.TCB.Axioms
import Cralloc.Proof.Examples.AArch64.Seq
import Cralloc.Proof.Framework.Registration
import CrallocTest.Forgery.Refused

/-!
# An `adrp` in a sequence's body

The worked example `Proof/Examples/AArch64/Seq.lean`, with an `adrp` before its commit. It is one
step of the model, in place. But rustc links with `--fix-cortex-a53-843419`: where an `adrp` lands
at page offset 0xff8 or 0xffc, a load or store through its register that follows moves to a stub
outside the sequence's range, where a preemption does not abort it.
-/

namespace VG.Test.Forgery.Adrp

open VG VG.Spec.Slab VG.AArch64
open VG.Proof.Examples.AArch64.Seq

def bodyA : List (Step Instr Cond) := [
  .instr (.ldr .w .x8 { base := .x10, off := .imm 4 }),
  .instr (.cmp .w .x8 (.imm 8192)),
  .exit .hs [] none,
  .instr (.adrp .x8 (.sym "S")),
  .instr (.str .x .x1 { base := .x0 })]

def codeA : Prog isa := .rseq arm bodyA

variable {s : State}

def σP (s : State) : State := (σ2 s).setReg .x8 (page (s.syms "S" + BitVec.ofInt 64 0))
def σE (s : State) : State := { σP s with mem := mEnd s }

theorem exec_adrp : exec (.adrp .x8 (.sym "S")) (σ2 s) = some (σP s) := rfl

theorem σP_gpr {r : Reg} (hr : r ≠ .x8) : (σP s).gpr r = s.gpr r := by
  rw [σP, RegUpd.gpr_setReg_of_ne _ _ hr, σ2_gpr hr]

theorem exec_commitA (h : Facts s) : exec (.str .x .x1 { base := .x0 }) (σP s) = some (σE s) := by
  have hea : (σP s).ea { base := .x0 } = s.gpr .x0 := by rw [Access.ea_base, σP_gpr (by decide)]
  have hin : InRegions (σP s).wr ((σP s).ea { base := .x0 }) 8 := by
    rw [hea]; exact InRegions.of_mem h.word (Region.contains_self _ _)
  rw [Access.exec_str_x hin, hea, σP_gpr (by decide)]
  rfl

theorem runBody_missA (h : Facts s) (hr : ready s) (hi : 8192 ≤ (cpu32 s).toNat) :
    runBody isa bodyA (σA s) = some (.done (σ2 s)) := by
  rw [bodyA, runBody_instr (exec_cpu h hr), runBody_instr exec_cmp,
    runBody_take (by rw [show isa.eval = eval from rfl, eval_cpu]; simp [hi]) rfl]

theorem runBody_hitA (h : Facts s) (hr : ready s) (hi : ¬ 8192 ≤ (cpu32 s).toNat) :
    runBody isa bodyA (σA s) = some (.done (σE s)) := by
  rw [bodyA, runBody_instr (exec_cpu h hr), runBody_instr exec_cmp,
    runBody_skip (by rw [show isa.eval = eval from rfl, eval_cpu]; simp [hi]),
    runBody_instr exec_adrp, runBody_instr (exec_commitA h), runBody_nil']

theorem prefixes_missA (h : Facts s) (hr : ready s) (hi : 8192 ≤ (cpu32 s).toNat) :
    bodyPrefixes isa bodyA (σA s) = [σA s, σ1 s, σ2 s] := by
  rw [bodyA, bodyPrefixes_instr (exec_cpu h hr), bodyPrefixes_instr exec_cmp,
    bodyPrefixes_take (by rw [show isa.eval = eval from rfl, eval_cpu]; simp [hi])]

theorem prefixes_hitA (h : Facts s) (hr : ready s) (hi : ¬ 8192 ≤ (cpu32 s).toNat) :
    bodyPrefixes isa bodyA (σA s) = [σA s, σ1 s, σ2 s, σ2 s, σP s] := by
  rw [bodyA, bodyPrefixes_instr (exec_cpu h hr), bodyPrefixes_instr exec_cmp,
    bodyPrefixes_skip (by rw [show isa.eval = eval from rfl, eval_cpu]; simp [hi]),
    bodyPrefixes_instr exec_adrp, bodyPrefixes_last (exec_commitA h)]

theorem end_missA (h : Facts s) (hr : ready s) (hi : 8192 ≤ (cpu32 s).toNat) :
    bodyEnd isa bodyA (σA s) = some 2 := by
  rw [bodyA, bodyEnd_instr (exec_cpu h hr), bodyEnd_instr exec_cmp,
    bodyEnd_take (by rw [show isa.eval = eval from rfl, eval_cpu]; simp [hi])]
  rfl

theorem end_hitA (h : Facts s) (hr : ready s) (hi : ¬ 8192 ≤ (cpu32 s).toNat) :
    bodyEnd isa bodyA (σA s) = some 5 := by
  rw [bodyA, bodyEnd_instr (exec_cpu h hr), bodyEnd_instr exec_cmp,
    bodyEnd_skip (by rw [show isa.eval = eval from rfl, eval_cpu]; simp [hi]),
    bodyEnd_instr exec_adrp, bodyEnd_instr (exec_commitA h), bodyEnd_nil']
  rfl

theorem mid_σP : Mid s (σP s) := ⟨rfl, fun _ hr => σP_gpr hr, rfl, rfl, rfl, rfl⟩

theorem midsA (hs : attempt.pre s) : ∀ p ∈ Code.abortStates isa codeA s, Mid s p := by
  have hf := (pre_iff.mp hs.1).facts
  have hr := hs.2
  intro p hp
  simp only [codeA, Code.abortStates, execBlock_arm hf hr] at hp
  by_cases hi : 8192 ≤ (cpu32 s).toNat
  · rw [prefixes_missA hf hr hi] at hp
    simp only [List.mem_cons, List.not_mem_nil, or_false] at hp
    rcases hp with rfl | rfl | rfl
    · exact mid_σA
    · exact mid_σ1
    · exact mid_σ2
  · rw [prefixes_hitA hf hr hi] at hp
    simp only [List.mem_cons, List.not_mem_nil, or_false] at hp
    rcases hp with rfl | rfl | rfl | rfl | rfl
    · exact mid_σA
    · exact mid_σ1
    · exact mid_σ2
    · exact mid_σ2
    · exact mid_σP

theorem correctA (hs : attempt.pre s) :
    (∃ s', Exec isa codeA s s' ∧ target.abiPreserved s s' ∧ attempt.post s s') ∨
    (∃ t s', ExecExit isa codeA s t s' ∧ target.abiPreserved s s' ∧ attempt.exit t s s') := by
  have hf := (pre_iff.mp hs.1).facts
  have hr := hs.2
  refine .inl ?_
  by_cases hi : 8192 ≤ (cpu32 s).toNat
  · refine ⟨σ2 s, .rseq (execBlock_arm hf hr) (runBody_missA hf hr hi), abi_of_mid mid_σ2,
      post_iff.mpr ⟨?_, fun h => ?_⟩⟩
    · exact frame_m1.mono fun r hr => by
        rw [List.mem_singleton.mp hr]; exact List.mem_cons_self ..
    · have : ¬ (s.mem.readW (area s + 4) 32).toNat < 8192 := by
        have h' : 8192 ≤ (s.mem.readW (area s + 4) 32).toNat := hi
        omega
      exact absurd h this
  · refine ⟨σE s, .rseq (execBlock_arm hf hr) (runBody_hitA hf hr hi), ?_, post_iff.mpr ⟨?_, ?_⟩⟩
    · refine ⟨fun r hr => ?_, rfl⟩
      show (σP s).gpr r = s.gpr r
      exact σP_gpr fun h => by subst h; exact absurd hr (by decide)
    · exact ((Frame.refl _ _).writeW (r := ⟨area s + 8, 8⟩) (List.mem_cons_self ..) _
        (Region.contains_self _ _)).writeW
        (r := ⟨s.gpr .x0, 8⟩) (List.mem_cons_of_mem _ (List.mem_singleton_self _)) _
        (Region.contains_self _ _)
    · exact fun _ => Mem.readW_writeW_self64 _ _ _

theorem rseqOkA (hs : attempt.pre s) : RseqOk target codeA attempt s :=
  ⟨trivial, ⟨_, s, rfl, rfl, arm_ea hs.2, hs.2.2⟩, fun p hp => abortOk hs (midsA hs p hp)⟩

theorem visits_w0A : Visits isa codeA w0 0 true :=
  have hf := (pre_iff.mp pre_w0.1).facts
  Visits.rseq (execBlock_arm hf pre_w0.2) (end_missA hf pre_w0.2 (by decide +kernel))

theorem visits_w1A : Visits isa codeA w1 1 true :=
  have hf := (pre_iff.mp pre_w1.1).facts
  Visits.rseq (execBlock_arm hf pre_w1.2) (end_hitA hf pre_w1.2 (by decide +kernel))

theorem witnessOkA : (∃ w ∈ witnesses, attempt.pre w) ∧
    ∀ o ∈ codeA.outcomes, ∃ w ∈ witnesses, attempt.pre w ∧ Visits isa codeA w o.1 o.2 := by
  refine ⟨⟨w0, by simp [witnesses], pre_w0⟩, fun o ho => ?_⟩
  rw [show codeA.outcomes = [(0, true), (1, true)] from rfl] at ho
  simp only [List.mem_cons, List.not_mem_nil, or_false] at ho
  rcases ho with rfl | rfl
  · exact ⟨w0, by simp [witnesses], pre_w0, visits_w0A⟩
  · exact ⟨w1, by simp [witnesses], pre_w1, visits_w1A⟩

theorem verifiedA : Verified target codeA attempt :=
  ⟨fun _ hs => correctA hs, fun _ hs => rseqOkA hs, ⟨w0, pre_w0⟩, fun o ho => by
    obtain ⟨w, -, hw, hv⟩ := witnessOkA.2 o ho
    exact ⟨w, hw, hv⟩⟩

theorem restartsOkA : ∀ s s', attempt.pre s → Restarts target codeA attempt s s' →
    target.abiPreserved s s' ∧ ArgsKept target.abi sig s s' ∧
    (target.inline = true → target.preservedExcept [] s s') :=
  restartsOk_target verifiedA (by decide) fun _ hs p hp r hr => (midsA hs p hp).gpr r fun h => by
    subst h; exact absurd hr (by decide)

def api : Api where
  module := "forgery"
  name := "vg_forgery_adrp"
  sig := sig
  contracts := some fun A _ => seqContract A
  summary := "A forgery."
  safety := fun _ => ["The thread's rseq area must be registered."]
  operands := [("__rseq_offset", .sym "crate::front::__rseq_offset"), ("S", .sym "crate::front::S")]

def art : Artifact := api.artifact VG.AArch64.target {
  setup := setup
  code := codeA
  attempt := attempt
  verified := verifiedA
  restartsOk := restartsOkA
  setupOk := setupOk
  witnesses := witnesses
  witnessOk := witnessOkA }

#assert_standard_axioms VG.Test.Forgery.Adrp.art

#guard refusedFor art "an instruction the linker writes"

end VG.Test.Forgery.Adrp
