import Cralloc.TCB.Axioms
import Cralloc.Proof.Examples.AArch64.Seq
import Cralloc.Proof.Framework.Registration
import CrallocTest.Forgery.Refused

/-!
# A routine that verifies and is wrong: a store after the commit

The worked example `Proof/Examples/AArch64/Seq.lean`, under its own contract
(`seqContract`, `attempt`, `setup`, `arm`, `body`), with one more instruction after the
commit, outside the sequence: `str x1, [x0]`, the commit's store again.

`Exec` runs the code after the commit from the state the commit left, with nothing in
between; `Code.abortStates` has no state there, as it is never aborted; and
`Code.events` stops at the body, so `Contract.trace` does not see it. On the machine the
thread can be preempted right after the commit, another thread on the CPU or a drainer
can change `*p` (the `env` of the contract says they may, while the thread is off the
CPU), and this store then writes the stale value over theirs, with no sequence to abort it.
It even writes when the CPU number was out of range, which the sequence checked.
-/

namespace VG.Test.Forgery.Tail

open VG VG.Spec.Slab VG.AArch64
open VG.Proof.Examples.AArch64.Seq

def tail : List Instr := [.str .x .x1 { base := .x0 }]

def codeT : Prog isa := .seq (.rseq arm body) (.block tail)

theorem abort_same (s : State) : Code.abortStates isa codeT s = Code.abortStates isa code s := rfl

variable {s : State}

/-- The tail's store, from a state where the arguments are as on entry. -/
def after (s t : State) : State := { t with mem := t.mem.writeW (s.gpr .x0) (s.gpr .x1) }

theorem exec_tail (h : Facts s) {t : State} (h0 : t.gpr .x0 = s.gpr .x0) (h1 : t.gpr .x1 = s.gpr .x1)
    (hw : t.wr = s.wr) : execBlock isa tail t = some (after s t) := by
  have hea : t.ea { base := .x0 } = s.gpr .x0 := by rw [Access.ea_base, h0]
  have hin : InRegions t.wr (t.ea { base := .x0 }) 8 := by
    rw [hea, hw]; exact InRegions.of_mem h.word (Region.contains_self _ _)
  have he : exec (.str .x .x1 { base := .x0 }) t = some (after s t) := by
    rw [Access.exec_str_x hin, hea, h1]
    rfl
  rw [tail, execBlock_cons_some he, execBlock_nil']

theorem frame_after {t : State} (hf : Frame [⟨area s + 8, 8⟩, ⟨s.gpr .x0, 8⟩] s.mem t.mem) :
    Frame [⟨area s + 8, 8⟩, ⟨s.gpr .x0, 8⟩] s.mem (after s t).mem :=
  hf.writeW (r := ⟨s.gpr .x0, 8⟩) (List.mem_cons_of_mem _ (List.mem_singleton_self _)) _
    (Region.contains_self _ _)

theorem correct (hs : attempt.pre s) :
    (∃ s', Exec isa codeT s s' ∧ target.abiPreserved s s' ∧ attempt.post s s') ∨
    (∃ t s', ExecExit isa codeT s t s' ∧ target.abiPreserved s s' ∧ attempt.exit t s s') := by
  have hf := (pre_iff.mp hs.1).facts
  have hr := hs.2
  have hgpr : ∀ r, r ≠ .x8 → (σ2 s).gpr r = s.gpr r := fun _ h => σ2_gpr h
  have hgpr3 : ∀ r, r ≠ .x8 → (σ3 s).gpr r = s.gpr r := by
    intro r h
    show (σ2 s).gpr r = s.gpr r
    exact σ2_gpr h
  have habi : ∀ t : State, (∀ r, r ≠ .x8 → t.gpr r = s.gpr r) → t.sp = s.sp →
      target.abiPreserved s (after s t) := fun t ht hsp =>
    ⟨fun r hr => ht r (fun h => by subst h; exact absurd hr (by decide)), hsp⟩
  refine .inl ?_
  by_cases hi : 8192 ≤ (cpu32 s).toNat
  · refine ⟨after s (σ2 s), .seq (.rseq (execBlock_arm hf hr) (runBody_miss hf hr hi))
      (.block (exec_tail hf (hgpr _ (by decide)) (hgpr _ (by decide)) rfl)), habi _ hgpr rfl,
      post_iff.mpr ⟨?_, fun _ => Mem.readW_writeW_self64 _ _ _⟩⟩
    exact frame_after (t := σ2 s) (frame_m1.mono fun r hr => by
      rw [List.mem_singleton.mp hr]; exact List.mem_cons_self ..)
  · refine ⟨after s (σ3 s), .seq (.rseq (execBlock_arm hf hr) (runBody_hit hf hr hi))
      (.block (exec_tail hf (hgpr3 _ (by decide)) (hgpr3 _ (by decide)) rfl)), habi _ hgpr3 rfl,
      post_iff.mpr ⟨?_, fun _ => Mem.readW_writeW_self64 _ _ _⟩⟩
    exact frame_after (t := σ3 s) (((Frame.refl _ _).writeW (r := ⟨area s + 8, 8⟩) (List.mem_cons_self ..) _
        (Region.contains_self _ _)).writeW
        (r := ⟨s.gpr .x0, 8⟩) (List.mem_cons_of_mem _ (List.mem_singleton_self _)) _
        (Region.contains_self _ _))

theorem rseqOk (hs : attempt.pre s) : RseqOk target codeT attempt s :=
  ⟨trivial, ⟨_, s, rfl, rfl, arm_ea hs.2, hs.2.2⟩, fun p hp => abortOk hs (mids hs p (by
    rwa [abort_same] at hp))⟩

theorem visits_w0T : Visits isa codeT w0 0 true := .seqL visits_w0
theorem visits_w1T : Visits isa codeT w1 1 true := .seqL visits_w1

theorem witnessOkT : (∃ w ∈ witnesses, attempt.pre w) ∧
    ∀ o ∈ codeT.outcomes, ∃ w ∈ witnesses, attempt.pre w ∧ Visits isa codeT w o.1 o.2 := by
  refine ⟨⟨w0, by simp [witnesses], pre_w0⟩, fun o ho => ?_⟩
  rw [show codeT.outcomes = [(0, true), (1, true)] from rfl] at ho
  simp only [List.mem_cons, List.not_mem_nil, or_false] at ho
  rcases ho with rfl | rfl
  · exact ⟨w0, by simp [witnesses], pre_w0, visits_w0T⟩
  · exact ⟨w1, by simp [witnesses], pre_w1, visits_w1T⟩

theorem verifiedT : Verified target codeT attempt :=
  ⟨fun _ hs => correct hs, fun _ hs => rseqOk hs, ⟨w0, pre_w0⟩, fun o ho => by
    obtain ⟨w, -, hw, hv⟩ := witnessOkT.2 o ho
    exact ⟨w, hw, hv⟩⟩

theorem restartsOkT : ∀ s s', attempt.pre s → Restarts target codeT attempt s s' →
    target.abiPreserved s s' ∧ ArgsKept target.abi sig s s' ∧
    (target.inline = true → target.preservedExcept [] s s') :=
  restartsOk_target verifiedT (by decide) fun _ hs p hp r hr => (mids hs p (by
    rwa [abort_same] at hp)).gpr r fun h => by subst h; exact absurd hr (by decide)

def api : Api where
  module := "forgery"
  name := "vg_forgery_tail"
  sig := sig
  contracts := some fun A _ => seqContract A
  summary := "A forgery."
  safety := ["The thread's rseq area must be registered."]
  operands := [("__rseq_offset", .sym "crate::front::__rseq_offset")]

def art : Artifact := api.artifact VG.AArch64.target {
  setup := setup
  code := codeT
  attempt := attempt
  verified := verifiedT
  restartsOk := restartsOkT
  setupOk := setupOk
  witnesses := witnesses
  witnessOk := witnessOkT }

#assert_standard_axioms VG.Test.Forgery.Tail.art

#guard refusedFor art "code after its commit accesses memory"

end VG.Test.Forgery.Tail
