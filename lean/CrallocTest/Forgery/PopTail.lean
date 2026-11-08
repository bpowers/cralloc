import Cralloc.TCB.Axioms
import Cralloc.Proof.Slab.AArch64.PopInlineWit
import Cralloc.Proof.Framework.Registration
import CrallocTest.Forgery.Refused

/-!
# The real inline pop, under its real contract, with a store after the commit

`Impl.Slab.AArch64.popInline` against `Spec.Slab.popInlineContract`, with the real
`setup`, `arm`, `body`, `attempt`, and `PopTrace` as the trace, followed outside the
sequence by

    cbnz x0, 20f; b 21f; 20: str x1, [x3, x8]; 21:

which stores the word the commit stored, again, if a pointer was popped. In the model the
second store changes nothing. On the machine, once the commit has run nothing protects this
thread: a drainer, or another thread on the CPU, may change the word first, and the store
overwrites what they wrote. `Code.events` stops at the body, so `PopTrace` does not see it.
-/

namespace VG.Test.Forgery.PopTail

open VG VG.AArch64 VG.Spec.Slab VG.Impl.Slab.AArch64 VG.Proof.Slab.AArch64
open VG.Proof.Slab.AArch64.PopInline

def tail : Prog isa := .ite (.cbnz .x .x0) (.block [.str .x .x1 wordAt]) (.block [])

def codeT : Prog isa := .seq (.rseq arm body) tail

variable {s : State} {k : Nat}

theorem write_same (m : Mem) (a : Addr) (n : Nat) (v : BitVec (8 * n)) :
    (m.write a n v).write a n v = m.write a n v := by
  funext y
  by_cases h : (y - a).toNat < n <;> simp only [Mem.write, h, ite_true, ite_false]

/-- The tail's store, after a pop: to the word, of what the commit stored. -/
theorem exec_tail_hit (hp : Pre s k) (hi : cx.InRange s) :
    execBlock isa [.str .x .x1 wordAt] (σ9 s) = some (σ9 s) := by
  have h3 : (σ9 s).gpr .x3 = s.gpr .x3 := by show (σ8 s).gpr .x3 = _; exact σ8_x3
  have h8 : (σ9 s).gpr .x8 = sh s := by show (σ8 s).gpr .x8 = _; exact σ8_x8
  have h1 : (σ9 s).gpr .x1 = wv s - 8 := by show (σ8 s).gpr .x1 = _; exact σ8_x1
  have hea : (σ9 s).ea wordAt = wa s := wordAt_ea (σ9 s) h3 h8
  have hin : InRegions (σ9 s).wr ((σ9 s).ea wordAt) 8 := by
    rw [hea, show (σ9 s).wr = s.wr from rfl, hp.word]
    exact inWr (r := ⟨cx.word s, 8⟩) (hp.facts.word hi)
  have hm : (σ9 s).mem.writeW (wa s) (wv s - 8) = (σ9 s).mem := by
    show ((m1 s).write (wa s) (64 / 8) _).write (wa s) (64 / 8) _ = (m1 s).write (wa s) (64 / 8) _
    exact write_same _ _ _ _
  have he : exec (.str .x .x1 wordAt) (σ9 s) = some (σ9 s) := by
    rw [Access.exec_str_x hin, hea, h1, hm]
  rw [execBlock_cons_some he, execBlock_nil']

theorem eval_cbnz (t : State) : isa.eval (.cbnz .x .x0) t = some (t.gpr .x0 != 0) := rfl

theorem eval_zero {t : State} (h : t.gpr .x0 = 0) : isa.eval (.cbnz .x .x0) t = some false := by
  rw [eval_cbnz, h]; rfl

theorem eval_nonzero {t : State} (h : t.gpr .x0 ≠ 0) : isa.eval (.cbnz .x .x0) t = some true := by
  rw [eval_cbnz]; simpa using h

theorem correctT (hs : attempt.pre s) :
    (∃ s', Exec isa codeT s s' ∧ inlineTarget.abiPreserved s s' ∧ attempt.post s s') ∨
    (∃ t s', ExecExit isa codeT s t s' ∧ inlineTarget.abiPreserved s s' ∧ attempt.exit t s s') := by
  have h := facts_of_pre hs
  obtain ⟨k, hp⟩ := pre_of h
  refine .inl ?_
  by_cases hi : cx.InRange s
  · by_cases ht : tst s = 0
    · exact ⟨σ5 s, .seq (.rseq (execBlock_arm h.toArm) (runBody_T hp hi ht))
        (.iteF (eval_zero ret_σ5) (.block rfl)), mσ5.sp,
        (post_eq _).mpr (.inr ⟨missed_T hp hi ht, ret_σ5⟩)⟩
    · by_cases hx : x s = 0
      · exact ⟨σ9 s, .seq (.rseq (execBlock_arm h.toArm) (runBody_P hp hi ht))
          (.iteF (eval_zero (ret_σ9.trans hx)) (.block rfl)), mσ9.sp,
          (post_eq _).mpr (.inl ⟨x s, popped hp hi ht, ret_σ9⟩)⟩
      · exact ⟨σ9 s, .seq (.rseq (execBlock_arm h.toArm) (runBody_P hp hi ht))
          (.iteT (eval_nonzero (by rw [ret_σ9]; exact hx)) (.block (exec_tail_hit hp hi))), mσ9.sp,
          (post_eq _).mpr (.inl ⟨x s, popped hp hi ht, ret_σ9⟩)⟩
  · exact ⟨σ2 s, .seq (.rseq (execBlock_arm h.toArm) (runBody_C hp hi))
      (.iteF (eval_zero ret_σ2) (.block rfl)), mσ2.sp,
      (post_eq _).mpr (.inr ⟨missed_C hi, ret_σ2⟩)⟩

/-- The contract, the trace and the abort states are the real routine's: `Code.events` and
`Code.abortStates` stop at the commit. -/
theorem rseqOkT (hs : attempt.pre s) : RseqOk inlineTarget codeT attempt s := by
  have h := PopInline.rseqOk hs
  rw [code_eq] at h
  exact h

theorem str_gpr {t t' : State} (h : execBlock isa [.str .x .x1 wordAt] t = some t') :
    t'.gpr = t.gpr := by
  by_cases hin : InRegions t.wr (t.ea wordAt) 8
  · rw [execBlock_cons_some (Access.exec_str_x hin), execBlock_nil'] at h
    cases h; rfl
  · have he : exec (.str .x .x1 wordAt) t = none := by simp [exec, State.store64, hin]
    simp only [execBlock] at h
    rw [show exec (.str .x .x1 wordAt) t = none from he] at h
    cases h

theorem clobbersOkT (hs : attempt.pre s) :
    (∀ s', Exec isa codeT s s' →
      preservedExcept (clobbers ++ (if popInlineSig.ret.isSome then ["x0"] else [])) s s') ∧
    (∀ t s', ExecExit isa codeT s t s' → preservedExcept clobbers s s') ∧
    (∀ p ∈ codeT.abortStates isa s, preservedExcept clobbers s p) := by
  have hc := PopInline.clobbersOk hs
  refine ⟨fun s' he => ?_, fun t s' he => ?_, hc.2.2⟩
  · rw [codeT] at he
    cases he with
    | seq h1 h2 =>
      have a := hc.1 _ h1
      rw [tail] at h2
      cases h2 with
      | iteT _ h3 =>
        cases h3 with
        | block hb => exact fun r hr => by rw [congrFun (str_gpr hb) r]; exact a r hr
      | iteF _ h3 =>
        cases h3 with
        | block hb => cases hb; exact a
  · rw [codeT] at he
    cases he with
    | seqL h1 => exact hc.2.1 _ _ h1
    | seqR h1 h2 =>
      rw [tail] at h2
      cases h2 with
      | iteT _ h3 => cases h3
      | iteF _ h3 => cases h3

theorem outcomes_eqT : codeT.outcomes = [(0, true), (1, true), (2, true), (3, true), (3, false)] := rfl

theorem visits3t : Visits isa codeT w2 3 true := by
  obtain ⟨k, hp⟩ := pre_of (facts_of_pre pre_w2)
  have hi : cx.InRange w2 := by decide +kernel
  have ht : ¬ tst w2 = 0 := by decide +kernel
  have hx : x w2 ≠ 0 := by decide +kernel
  have he : Exec isa (.rseq arm body) w2 (σ9 w2) :=
    .rseq (execBlock_arm hp.facts.toArm) (runBody_P hp hi ht)
  exact Visits.seqR (c₁ := .rseq arm body) (c₂ := tail) (i := 0) he
    (.ite (eval_nonzero (by rw [ret_σ9]; exact hx)))

theorem visits3f : Visits isa codeT w1 3 false := by
  obtain ⟨k, hp⟩ := pre_of (facts_of_pre pre_w1)
  have hi : cx.InRange w1 := by decide +kernel
  have ht : tst w1 = 0 := by decide +kernel
  have he : Exec isa (.rseq arm body) w1 (σ5 w1) :=
    .rseq (execBlock_arm hp.facts.toArm) (runBody_T hp hi ht)
  exact Visits.seqR (c₁ := .rseq arm body) (c₂ := tail) (i := 0) he (.ite (eval_zero ret_σ5))

theorem witnessOkT : (∃ w ∈ witnesses, attempt.pre w) ∧
    ∀ o ∈ codeT.outcomes, ∃ w ∈ witnesses, attempt.pre w ∧ Visits isa codeT w o.1 o.2 := by
  refine ⟨⟨w0, by simp [witnesses], pre_w0⟩, fun o ho => ?_⟩
  rw [outcomes_eqT] at ho
  simp only [List.mem_cons, List.not_mem_nil, or_false] at ho
  rcases ho with rfl | rfl | rfl | rfl | rfl
  · exact ⟨w0, by simp [witnesses], pre_w0, .seqL visits_w0⟩
  · exact ⟨w1, by simp [witnesses], pre_w1, .seqL visits_w1⟩
  · exact ⟨w2, by simp [witnesses], pre_w2, .seqL visits_w2⟩
  · exact ⟨w2, by simp [witnesses], pre_w2, visits3t⟩
  · exact ⟨w1, by simp [witnesses], pre_w1, visits3f⟩

theorem verifiedT : Verified inlineTarget codeT attempt :=
  ⟨fun _ hs => correctT hs, fun _ hs => rseqOkT hs, ⟨w0, pre_w0⟩, fun o ho => by
    obtain ⟨w, -, hw, hv⟩ := witnessOkT.2 o ho
    exact ⟨w, hw, hv⟩⟩

theorem restartsOkT : ∀ s s', attempt.pre s → Restarts inlineTarget codeT attempt s s' →
    inlineTarget.abiPreserved s s' ∧ ArgsKept inlineTarget.abi popInlineSig s s' ∧
    (inlineTarget.inline = true → inlineTarget.preservedExcept clobbers s s') :=
  restartsOk_inline (ret := .x0) verifiedT (fun _ hs => (clobbersOkT hs).2.2)
    fun _ _ hpe => argsKept_regs hX (by decide) fun r hr => hpe r (by revert r; decide)

def art : Artifact := popInlineApi.artifact VG.AArch64.inlineTarget {
  cfg := some ([.targetEnv "musl"], false)
  setup := setup
  code := codeT
  attempt := attempt
  verified := verifiedT
  restartsOk := restartsOkT
  setupOk := PopInline.setupOk
  clobbers := clobbers
  clobbersOk := fun _ _ hs => clobbersOkT hs
  witnesses := witnesses
  witnessOk := witnessOkT }

#assert_standard_axioms VG.Test.Forgery.PopTail.art

#guard refusedFor art "code after its commit is not straight-line"

end VG.Test.Forgery.PopTail
