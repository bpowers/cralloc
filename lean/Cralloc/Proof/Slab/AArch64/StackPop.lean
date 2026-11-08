module

public import Cralloc.Proof.Slab.AArch64.Slow
public import Cralloc.Proof.Slab.Witness
public import Cralloc.Proof.Slab.Stack

/-!
# The stack pop on AArch64

`slab_stack_pop` (`Impl.Slab.AArch64.stackPop`) against `Spec.Slab.stackPopContract`: its four ways
out (no slab; the head is a marker; the stack is empty; the pop), and in each its run, where it may
be aborted, and its memory accesses. The link it commits is read, inside the sequence, from the head
it read once.
-/

@[expose] public section

namespace VG.Proof.Slab.AArch64.StackPop

open VG.AArch64 VG.Spec.Slab VG.Impl.Slab.AArch64 VG.Proof.Slab.AArch64 VG.Proof.Slab

abbrev cx : Spec.Slab.Ctx isa := ⟨abi, .stackPop, stackPopSig⟩

theorem hX : A64 cx.A argRegs := a64_abi

abbrev contract : Contract isa := stackPopContract abi

/-- The contract of an attempt. -/
abbrev attempt : Contract isa := withReadyS contract

def body : List (Step Instr Cond) := [
  .instr (.mov .x .x3 (.imm 2)),
  .instr (.ldr .w .x8 { base := .x10, off := .imm 4 }),
  .instr (.cmp .w .x8 (.imm 8192)),
  .exit .hs [] none,
  .instr (.lsl .x .x8 .x8 12),
  .instr (.and .x .x9 .x0 (.imm 0xff8)),
  .instr (.add .x .x8 .x8 (.reg .x9)),
  .instr (.ldr .x .x3 slabWord),
  .exit (.tbnz .x3 0) [] none,
  .exit (.cbz .x .x3) [] none,
  .instr (.ldr .x .x4 { base := .x3 }),
  .instr (.str .x .x4 slabWord)]

theorem code_eq : stackPop = .seq (.rseq arm body) (.block retWord) := rfl

/-! ## The states -/

/-- The top magazine's link: the next head. -/
def lk (s : State) : BitVec 64 := s.mem.readW (wvo s) 64
def σ9 (s : State) : State := (σ6o s).setReg .x4 (lk s)
def mEnd (s : State) : Mem := (m1 s).writeW (wao s) (lk s)
def σ10 (s : State) : State := { σ9 s with mem := mEnd s }

/-! ## What the precondition gives -/

structure Pre (s : State) : Prop where
  facts : SFacts cx s
  word : wao s = cx.word s
  w : cx.w s = wvo s
  stack : cx.InRange s → StackOk s.mem (wvo s) (cx.Apart s)

theorem pre_of {s : State} (h : SFacts cx s) : Pre s := by
  have hw : wao s = cx.word s := wao_eq hX h.x13 rfl
  have hwv : cx.w s = wvo s := by simp only [Spec.Slab.Ctx.w, mem_of (c := cx) hX, wvo, hw]
  obtain ⟨-, -, -, -, hinv⟩ := h.pre
  refine ⟨h, hw, hwv, fun hi => ?_⟩
  have := hinv (cx.cpu s) hi
  rw [wordOf_cpu, mem_of (c := cx) hX, hwv] at this
  exact this

section Steps

variable {s : State}

theorem inRange_iff : cx.InRange s ↔ (cpu32 s).toNat < 8192 := by
  simp only [Spec.Slab.Ctx.InRange, cpu_of (c := cx) hX, cpu32, SLAB_COUNT]

theorem eval_cpu_true (hi : ¬ cx.InRange s) : eval .hs (σ2o s) = some true := by
  rw [eval_cpu]
  have : ¬ (cpu32 s).toNat < 8192 := fun h => hi (inRange_iff.mpr h)
  simp only [Option.some.injEq, decide_eq_true_eq]
  omega

theorem eval_cpu_false (hi : cx.InRange s) : eval .hs (σ2o s) = some false := by
  rw [eval_cpu]
  have := inRange_iff.mp hi
  simp only [Option.some.injEq, decide_eq_false_iff_not]
  omega

theorem word_facts (hp : Pre s) (hi : cx.InRange s) :
    InRegions (s.rd ++ s.wr) (wao s) 8 ∧ Region.Disjoint ⟨wao s, 8⟩ ⟨area s + 8, 8⟩ := by
  rw [hp.word]
  exact ⟨inRdWr (r := ⟨cx.word s, 8⟩) (hp.facts.word hi), (hp.facts.cs_word hi).symm⟩

theorem exec_w (hp : Pre s) (hi : cx.InRange s) :
    exec (.ldr .x .x3 slabWord) (σ5o s) = some (σ6o s) :=
  exec_word' (word_facts hp hi).1 (word_facts hp hi).2

theorem eval_zero (h0 : (wvo s).toNat = 0) : eval (.cbz .x .x3) (σ6o s) = some true := by
  have : wvo s = 0 := BitVec.eq_of_toNat_eq h0
  rw [Flags.eval_cbz, RegUpd.read_x, σ6o_x3, this]
  rfl

theorem eval_nonzero (h0 : ¬ (wvo s).toNat = 0) : eval (.cbz .x .x3) (σ6o s) = some false := by
  have : ¬ wvo s = 0 := fun h => h0 (by rw [h]; rfl)
  rw [Flags.eval_cbz, RegUpd.read_x, σ6o_x3]
  simpa using this

/-- The top magazine's link slot: readable, apart from `rseq_cs`. -/
theorem top_facts (hp : Pre s) (hi : cx.InRange s) (h0 : ¬ (wvo s).toNat = 0)
    (hm : ¬ (wvo s).toNat % 2 = 1) :
    InRegions (s.rd ++ s.wr) (wvo s) 8 ∧ Region.Disjoint ⟨wvo s, 8⟩ ⟨area s + 8, 8⟩ := by
  have h0' : (cx.w s).toNat ≠ 0 := by rw [hp.w]; exact h0
  have h2' : (cx.w s).toNat % 2 = 0 := by rw [hp.w]; omega
  have ht := hp.facts.top rfl hi h0' h2'
  have hc := hp.facts.top_cs rfl hi h0' h2'
  rw [hp.w] at ht hc
  exact ⟨inRd (r := ⟨wvo s, 8⟩) ht, hc⟩

theorem exec_link (hp : Pre s) (hi : cx.InRange s) (h0 : ¬ (wvo s).toNat = 0)
    (hm : ¬ (wvo s).toNat % 2 = 1) :
    exec (.ldr .x .x4 { base := .x3 }) (σ6o s) = some (σ9 s) := by
  obtain ⟨hin, hdj⟩ := top_facts hp hi h0 hm
  have hin' : InRegions ((σ6o s).rd ++ (σ6o s).wr) ((σ6o s).ea { base := .x3 }) 8 := by
    rw [Access.ea_base, σ6o_x3]; exact hin
  rw [Access.exec_ldr_x hin', Access.ea_base, σ6o_x3, σ6o_mem, m1, readW64_writeW_disj hdj]
  rfl

theorem σ9_x13 : (σ9 s).gpr .x13 = s.gpr .x13 := by
  rw [σ9, RegUpd.gpr_setReg_of_ne _ _ (by decide), σ6o_x13]
theorem σ9_x8 : (σ9 s).gpr .x8 = r8o s := by
  rw [σ9, RegUpd.gpr_setReg_of_ne _ _ (by decide), σ6o_x8]
theorem σ9_x4 : (σ9 s).gpr .x4 = lk s := RegUpd.gpr_setReg_self _ _ _
theorem σ9_x3 : (σ9 s).gpr .x3 = wvo s := by
  rw [σ9, RegUpd.gpr_setReg_of_ne _ _ (by decide), σ6o_x3]

theorem exec_commit (hp : Pre s) (hi : cx.InRange s) :
    exec (.str .x .x4 slabWord) (σ9 s) = some (σ10 s) := by
  have hin : InRegions (σ9 s).wr ((σ9 s).ea slabWord) 8 := by
    rw [slabWord_ea _ σ9_x13 σ9_x8, show (σ9 s).wr = s.wr from rfl, hp.word]
    exact inWr (r := ⟨cx.word s, 8⟩) (hp.facts.word hi)
  rw [Access.exec_str_x hin, slabWord_ea _ σ9_x13 σ9_x8, σ9_x4]
  rfl

end Steps

/-! ## The four ways out -/

section Cases

variable {s : State}

theorem runBody_C (hp : Pre s) (hi : ¬ cx.InRange s) :
    runBody isa body (σA s) = some (.done (σ2o s)) := by
  rw [body, runBody_instr exec_x3, runBody_instr (exec_cpu hp.facts.toArm (baseS hp.facts.ready)),
    runBody_instr exec_cmp, runBody_take (eval_cpu_true hi) execBlock_nil']

theorem prefixes_C (hp : Pre s) (hi : ¬ cx.InRange s) :
    bodyPrefixes isa body (σA s) = [σA s, σ0o s, σ1o s, σ2o s] := by
  rw [body, bodyPrefixes_instr exec_x3,
    bodyPrefixes_instr (exec_cpu hp.facts.toArm (baseS hp.facts.ready)), bodyPrefixes_instr exec_cmp,
    bodyPrefixes_take (eval_cpu_true hi)]

theorem end_C (hp : Pre s) (hi : ¬ cx.InRange s) : bodyEnd isa body (σA s) = some 3 := by
  rw [body, bodyEnd_instr exec_x3, bodyEnd_instr (exec_cpu hp.facts.toArm (baseS hp.facts.ready)),
    bodyEnd_instr exec_cmp, bodyEnd_take (eval_cpu_true hi)]
  rfl

theorem events_C (hp : Pre s) (hi : ¬ cx.InRange s) :
    bodyEvents isa body (σA s) = [⟨area s + 4, 4, false, (cpu32 s).toNat⟩] := by
  rw [body, bodyEvents_instr exec_x3, bodyEvents_instr (exec_cpu hp.facts.toArm (baseS hp.facts.ready)),
    bodyEvents_instr exec_cmp, bodyEvents_take (eval_cpu_true hi)]
  simp only [isa, ev_cpu (baseS hp.facts.ready)]; rfl

theorem runBody_M (hp : Pre s) (hi : cx.InRange s) (hm : (wvo s).toNat % 2 = 1) :
    runBody isa body (σA s) = some (.done (σ6o s)) := by
  rw [body, runBody_instr exec_x3, runBody_instr (exec_cpu hp.facts.toArm (baseS hp.facts.ready)),
    runBody_instr exec_cmp, runBody_skip (eval_cpu_false hi), runBody_instr exec_lsl,
    runBody_instr exec_off1, runBody_instr exec_off2, runBody_instr (exec_w hp hi), runBody_take (eval_marker hm) execBlock_nil']

theorem prefixes_M (hp : Pre s) (hi : cx.InRange s) (hm : (wvo s).toNat % 2 = 1) :
    bodyPrefixes isa body (σA s) =
      [σA s, σ0o s, σ1o s, σ2o s, σ2o s, σ3o s, σ4o s, σ5o s, σ6o s] := by
  rw [body, bodyPrefixes_instr exec_x3,
    bodyPrefixes_instr (exec_cpu hp.facts.toArm (baseS hp.facts.ready)), bodyPrefixes_instr exec_cmp,
    bodyPrefixes_skip (eval_cpu_false hi), bodyPrefixes_instr exec_lsl, bodyPrefixes_instr exec_off1,
    bodyPrefixes_instr exec_off2, bodyPrefixes_instr (exec_w hp hi), bodyPrefixes_take (eval_marker hm)]

theorem end_M (hp : Pre s) (hi : cx.InRange s) (hm : (wvo s).toNat % 2 = 1) :
    bodyEnd isa body (σA s) = some 8 := by
  rw [body, bodyEnd_instr exec_x3, bodyEnd_instr (exec_cpu hp.facts.toArm (baseS hp.facts.ready)),
    bodyEnd_instr exec_cmp, bodyEnd_skip (eval_cpu_false hi), bodyEnd_instr exec_lsl,
    bodyEnd_instr exec_off1, bodyEnd_instr exec_off2, bodyEnd_instr (exec_w hp hi), bodyEnd_take (eval_marker hm)]
  rfl

theorem events_M (hp : Pre s) (hi : cx.InRange s) (hm : (wvo s).toNat % 2 = 1) :
    bodyEvents isa body (σA s) =
      [⟨area s + 4, 4, false, (cpu32 s).toNat⟩, ⟨wao s, 8, false, (wvo s).toNat⟩] := by
  rw [body, bodyEvents_instr exec_x3, bodyEvents_instr (exec_cpu hp.facts.toArm (baseS hp.facts.ready)),
    bodyEvents_instr exec_cmp, bodyEvents_skip (eval_cpu_false hi), bodyEvents_instr exec_lsl,
    bodyEvents_instr exec_off1, bodyEvents_instr exec_off2, bodyEvents_instr (exec_w hp hi), bodyEvents_take (eval_marker hm)]
  simp only [isa, ev_cpu (baseS hp.facts.ready), ev_word' (word_facts hp hi).2]; rfl

theorem runBody_Z (hp : Pre s) (hi : cx.InRange s) (hm : ¬ (wvo s).toNat % 2 = 1)
    (h0 : (wvo s).toNat = 0) : runBody isa body (σA s) = some (.done (σ6o s)) := by
  rw [body, runBody_instr exec_x3, runBody_instr (exec_cpu hp.facts.toArm (baseS hp.facts.ready)),
    runBody_instr exec_cmp, runBody_skip (eval_cpu_false hi), runBody_instr exec_lsl,
    runBody_instr exec_off1, runBody_instr exec_off2, runBody_instr (exec_w hp hi), runBody_skip (eval_even hm),
    runBody_take (eval_zero h0) execBlock_nil']

theorem prefixes_Z (hp : Pre s) (hi : cx.InRange s) (hm : ¬ (wvo s).toNat % 2 = 1)
    (h0 : (wvo s).toNat = 0) : bodyPrefixes isa body (σA s) =
      [σA s, σ0o s, σ1o s, σ2o s, σ2o s, σ3o s, σ4o s, σ5o s, σ6o s, σ6o s] := by
  rw [body, bodyPrefixes_instr exec_x3,
    bodyPrefixes_instr (exec_cpu hp.facts.toArm (baseS hp.facts.ready)), bodyPrefixes_instr exec_cmp,
    bodyPrefixes_skip (eval_cpu_false hi), bodyPrefixes_instr exec_lsl, bodyPrefixes_instr exec_off1,
    bodyPrefixes_instr exec_off2, bodyPrefixes_instr (exec_w hp hi), bodyPrefixes_skip (eval_even hm),
    bodyPrefixes_take (eval_zero h0)]

theorem end_Z (hp : Pre s) (hi : cx.InRange s) (hm : ¬ (wvo s).toNat % 2 = 1)
    (h0 : (wvo s).toNat = 0) : bodyEnd isa body (σA s) = some 9 := by
  rw [body, bodyEnd_instr exec_x3, bodyEnd_instr (exec_cpu hp.facts.toArm (baseS hp.facts.ready)),
    bodyEnd_instr exec_cmp, bodyEnd_skip (eval_cpu_false hi), bodyEnd_instr exec_lsl,
    bodyEnd_instr exec_off1, bodyEnd_instr exec_off2, bodyEnd_instr (exec_w hp hi), bodyEnd_skip (eval_even hm), bodyEnd_take (eval_zero h0)]
  rfl

theorem events_Z (hp : Pre s) (hi : cx.InRange s) (hm : ¬ (wvo s).toNat % 2 = 1)
    (h0 : (wvo s).toNat = 0) : bodyEvents isa body (σA s) =
      [⟨area s + 4, 4, false, (cpu32 s).toNat⟩, ⟨wao s, 8, false, (wvo s).toNat⟩] := by
  rw [body, bodyEvents_instr exec_x3, bodyEvents_instr (exec_cpu hp.facts.toArm (baseS hp.facts.ready)),
    bodyEvents_instr exec_cmp, bodyEvents_skip (eval_cpu_false hi), bodyEvents_instr exec_lsl,
    bodyEvents_instr exec_off1, bodyEvents_instr exec_off2, bodyEvents_instr (exec_w hp hi), bodyEvents_skip (eval_even hm), bodyEvents_take (eval_zero h0)]
  simp only [isa, ev_cpu (baseS hp.facts.ready), ev_word' (word_facts hp hi).2]; rfl

theorem runBody_P (hp : Pre s) (hi : cx.InRange s) (hm : ¬ (wvo s).toNat % 2 = 1)
    (h0 : ¬ (wvo s).toNat = 0) : runBody isa body (σA s) = some (.done (σ10 s)) := by
  rw [body, runBody_instr exec_x3, runBody_instr (exec_cpu hp.facts.toArm (baseS hp.facts.ready)),
    runBody_instr exec_cmp, runBody_skip (eval_cpu_false hi), runBody_instr exec_lsl,
    runBody_instr exec_off1, runBody_instr exec_off2, runBody_instr (exec_w hp hi), runBody_skip (eval_even hm), runBody_skip (eval_nonzero h0),
    runBody_instr (exec_link hp hi h0 hm), runBody_instr (exec_commit hp hi), runBody_nil']

theorem prefixes_P (hp : Pre s) (hi : cx.InRange s) (hm : ¬ (wvo s).toNat % 2 = 1)
    (h0 : ¬ (wvo s).toNat = 0) : bodyPrefixes isa body (σA s) =
      [σA s, σ0o s, σ1o s, σ2o s, σ2o s, σ3o s, σ4o s, σ5o s, σ6o s, σ6o s, σ6o s, σ9 s] := by
  rw [body, bodyPrefixes_instr exec_x3,
    bodyPrefixes_instr (exec_cpu hp.facts.toArm (baseS hp.facts.ready)), bodyPrefixes_instr exec_cmp,
    bodyPrefixes_skip (eval_cpu_false hi), bodyPrefixes_instr exec_lsl, bodyPrefixes_instr exec_off1,
    bodyPrefixes_instr exec_off2, bodyPrefixes_instr (exec_w hp hi), bodyPrefixes_skip (eval_even hm),
    bodyPrefixes_skip (eval_nonzero h0), bodyPrefixes_instr (exec_link hp hi h0 hm),
    bodyPrefixes_last (exec_commit hp hi)]

theorem end_P (hp : Pre s) (hi : cx.InRange s) (hm : ¬ (wvo s).toNat % 2 = 1)
    (h0 : ¬ (wvo s).toNat = 0) : bodyEnd isa body (σA s) = some 12 := by
  rw [body, bodyEnd_instr exec_x3, bodyEnd_instr (exec_cpu hp.facts.toArm (baseS hp.facts.ready)),
    bodyEnd_instr exec_cmp, bodyEnd_skip (eval_cpu_false hi), bodyEnd_instr exec_lsl,
    bodyEnd_instr exec_off1, bodyEnd_instr exec_off2, bodyEnd_instr (exec_w hp hi), bodyEnd_skip (eval_even hm), bodyEnd_skip (eval_nonzero h0),
    bodyEnd_instr (exec_link hp hi h0 hm), bodyEnd_instr (exec_commit hp hi), bodyEnd_nil']
  rfl

theorem ev_link (hp : Pre s) (hi : cx.InRange s) (h0 : ¬ (wvo s).toNat = 0)
    (hm : ¬ (wvo s).toNat % 2 = 1) :
    Instr.events (.ldr .x .x4 { base := .x3 }) (σ6o s) (σ9 s) =
      [⟨wvo s, 8, false, (lk s).toNat⟩] := by
  obtain ⟨-, hdj⟩ := top_facts hp hi h0 hm
  simp only [Instr.events, Access.ea_base, σ6o_x3, Width.bytes, toNat_read8, σ6o_mem, m1]
  rw [readW64_writeW_disj hdj]; rfl

theorem ev_commit :
    Instr.events (.str .x .x4 slabWord) (σ9 s) (σ10 s) = [⟨wao s, 8, true, (lk s).toNat⟩] := by
  simp only [Instr.events, slabWord_ea _ σ9_x13 σ9_x8, Width.bytes, toNat_read8]
  rw [show (σ10 s).mem = mEnd s from rfl, mEnd, Mem.readW_writeW_self64]

theorem events_P (hp : Pre s) (hi : cx.InRange s) (hm : ¬ (wvo s).toNat % 2 = 1)
    (h0 : ¬ (wvo s).toNat = 0) : bodyEvents isa body (σA s) =
      [⟨area s + 4, 4, false, (cpu32 s).toNat⟩, ⟨wao s, 8, false, (wvo s).toNat⟩,
        ⟨wvo s, 8, false, (lk s).toNat⟩, ⟨wao s, 8, true, (lk s).toNat⟩] := by
  rw [body, bodyEvents_instr exec_x3, bodyEvents_instr (exec_cpu hp.facts.toArm (baseS hp.facts.ready)),
    bodyEvents_instr exec_cmp, bodyEvents_skip (eval_cpu_false hi), bodyEvents_instr exec_lsl,
    bodyEvents_instr exec_off1, bodyEvents_instr exec_off2, bodyEvents_instr (exec_w hp hi), bodyEvents_skip (eval_even hm), bodyEvents_skip (eval_nonzero h0),
    bodyEvents_instr (exec_link hp hi h0 hm), bodyEvents_instr (exec_commit hp hi), bodyEvents_nil']
  simp only [ev_cpu (baseS hp.facts.ready), ev_word' (word_facts hp hi).2, ev_link hp hi h0 hm,
    ev_commit]; rfl

end Cases

/-! ## What each state keeps -/

section Mids

variable {s : State}

theorem mσ9 : Mid scratchS s (m1 s) (σ9 s) := mσ6o.setReg (by decide) _
theorem mσ10 : Mid scratchS s (mEnd s) (σ10 s) := mσ9.withMem _

end Mids

/-! ## The contract -/

section Contract

variable {s : State}

theorem hv : cx.A.args ((cx.sig.words cx.A.ptrBits).map (·.bits cx.A.ptrBits)) =
    some (fun s => [s.gpr .x0]) := rfl

theorem facts_of_pre (hs : attempt.pre s) : SFacts cx s := by
  have := (pre_iff cx (s := s) hv rfl rfl).mp hs.1
  exact sfacts_of_world hX this.2.1 this.2.2 hs.2

theorem unchanged_cs : Unchanged [cx.csField s] (cx.mem s) (m1 s) := by
  rw [csField_of (c := cx) hX, mem_of (c := cx) hX]; exact frame_m1

/-- What the pop leaves: the head is the top magazine's link, read from the head read once, and
the stack is the rest. -/
theorem popped (hp : Pre s) (hi : cx.InRange s) (h0 : ¬ (wvo s).toNat = 0) :
    Unchanged [cx.csField s, ⟨cx.word s, 8⟩] (cx.mem s) (mEnd s) ∧
    (mEnd s).readW (cx.word s) 64 = (cx.mem s).readW (cx.w s) 64 ∧
    ∀ l, Chain (cx.mem s) (cx.w s) l → Chain (mEnd s) ((cx.mem s).readW (cx.w s) 64) l.tail := by
  refine ⟨?_, ?_, fun l hc => ?_⟩
  · rw [csField_of (c := cx) hX, mem_of (c := cx) hX, ← hp.word]
    exact frame_two s.mem _ _ _ _
  · rw [mEnd, ← hp.word, Mem.readW_writeW_self64, mem_of (c := cx) hX, hp.w]; rfl
  · rw [mem_of (c := cx) hX, hp.w] at hc ⊢
    have hd := stackOk_chain (hp.stack hi) hc
    obtain ⟨hl, ht⟩ := chain_tail hc h0
    refine chain_frame ht fun x hx => ?_
    have hx' : x ∈ l := by rw [hl]; exact List.mem_cons_of_mem _ hx
    have hw : Region.Disjoint ⟨x, 8⟩ ⟨wao s, 8⟩ := by
      rw [hp.word]; exact (hd x hx').2.1.sub_right (word_sub (cls_lt hp.facts.pre.2.2.1) hi)
    have hcs : Region.Disjoint ⟨x, 8⟩ ⟨area s + 8, 8⟩ := by
      rw [← csField_of (c := cx) hX]; exact (hd x hx').2.2.1 (cx.csField s, true) cs_mem_fixed
    rw [mEnd, readW64_writeW_disj hw, m1, readW64_writeW_disj hcs]

theorem post_eq (s' : State) : attempt.post s s' ↔
    NoCpu cx s s' ∨ (cx.InRange s ∧ s'.gpr .x0 = cx.w s ∧
      ((((cx.w s).toNat = 0 ∨ (cx.w s).toNat % 2 = 1) ∧
          Unchanged [cx.csField s] (cx.mem s) s'.mem) ∨
        ((cx.w s).toNat ≠ 0 ∧ (cx.w s).toNat % 2 = 0 ∧
          Unchanged [cx.csField s, ⟨cx.word s, 8⟩] (cx.mem s) s'.mem ∧
          s'.mem.readW (cx.word s) 64 = (cx.mem s).readW (cx.w s) 64 ∧
          ∀ l, Chain (cx.mem s) (cx.w s) l → Chain s'.mem ((cx.mem s).readW (cx.w s) 64) l.tail))) :=
  post_iff cx hv s s'

theorem correct (hs : attempt.pre s) :
    (∃ s', Exec isa stackPop s s' ∧ target.abiPreserved s s' ∧ attempt.post s s') ∨
    (∃ t s', ExecExit isa stackPop s t s' ∧ target.abiPreserved s s' ∧ attempt.exit t s s') := by
  have h := facts_of_pre hs
  have hp := pre_of h
  refine .inl ?_
  rw [code_eq]
  by_cases hi : cx.InRange s
  · by_cases hm : (wvo s).toNat % 2 = 1
    · refine ⟨ret0 (σ6o s), .seq (.rseq (execBlock_arm h.toArm) (runBody_M hp hi hm))
        (.block (execBlock_ret _)), abi_ret mσ6o,
        (post_eq _).mpr (.inr ⟨hi, ?_, .inl ⟨.inr (by rw [hp.w]; exact hm), unchanged_cs⟩⟩)⟩
      rw [ret0, RegUpd.gpr_setReg_self, hp.w, σ6o_x3]
    · by_cases h0 : (wvo s).toNat = 0
      · refine ⟨ret0 (σ6o s), .seq (.rseq (execBlock_arm h.toArm) (runBody_Z hp hi hm h0))
          (.block (execBlock_ret _)), abi_ret mσ6o,
          (post_eq _).mpr (.inr ⟨hi, ?_, .inl ⟨.inl (by rw [hp.w]; exact h0), unchanged_cs⟩⟩)⟩
        rw [ret0, RegUpd.gpr_setReg_self, hp.w, σ6o_x3]
      · obtain ⟨hu, hr, hch⟩ := popped hp hi h0
        refine ⟨ret0 (σ10 s), .seq (.rseq (execBlock_arm h.toArm) (runBody_P hp hi hm h0))
          (.block (execBlock_ret _)), abi_ret mσ10,
          (post_eq _).mpr (.inr ⟨hi, ?_, .inr ⟨by rw [hp.w]; exact h0, by rw [hp.w]; omega,
            hu, hr, hch⟩⟩)⟩
        rw [ret0, RegUpd.gpr_setReg_self, hp.w]
        show (σ9 s).gpr .x3 = _
        rw [σ9_x3]
  · exact ⟨ret0 (σ2o s), .seq (.rseq (execBlock_arm h.toArm) (runBody_C hp hi))
      (.block (execBlock_ret _)), abi_ret mσ2o,
      (post_eq _).mpr (.inl ⟨hi, ret_σ2o (s := s), unchanged_cs⟩)⟩

/-! ### Where it may be aborted -/

theorem abort_eq (p : State) : attempt.abort s p ↔
    [p.gpr .x0] = [s.gpr .x0] ∧ (Unchanged [cx.csField s] (cx.mem s) p.mem ∧ cx.inv p) :=
  abort_iff cx hv s p

theorem abortOk (hp : Pre s) {p : State} (hm : Mid scratchS s (m1 s) p) :
    armed p ∧ target.abiPreserved s p ∧ attempt.abort s p ∧
      ∀ p', target.rely p p' → attempt.env p p' → attempt.pre p' := by
  obtain ⟨hip, hre⟩ := retryS hX hv (post := fun s s' => _) (fun _ => rfl) (fun _ => rfl) (by decide)
    hp.facts.pre ⟨hp.facts.ready, hp.facts.x13⟩ hm (by decide) frame_m1
    (harmless_cs1 hX hp.facts.pre)
  refine ⟨armed_of_mid hp.facts.toArm (xs := []) frame_m1 m1_cs (by simp) hm, abi_of hm, ?_, hre⟩
  exact (abort_eq p).mpr ⟨by rw [hm.gpr .x0 (by decide)], by rw [hm.mem]; exact unchanged_cs, hip⟩

/-- Wherever it may be aborted, memory is as after arming. -/
theorem mids (hp : Pre s) :
    ∀ p ∈ Code.abortStates isa (.seq (.rseq arm body) (.block retWord)) s, Mid scratchS s (m1 s) p := by
  simp only [Code.abortStates, execBlock_arm hp.facts.toArm]
  by_cases hi : cx.InRange s
  · by_cases hm : (wvo s).toNat % 2 = 1
    · rw [prefixes_M hp hi hm]
      simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
      exact ⟨mσAo, mσ0o, mσ1o, mσ2o, mσ2o, mσ3o, mσ4o, mσ5o, mσ6o⟩
    · by_cases h0 : (wvo s).toNat = 0
      · rw [prefixes_Z hp hi hm h0]
        simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
        exact ⟨mσAo, mσ0o, mσ1o, mσ2o, mσ2o, mσ3o, mσ4o, mσ5o, mσ6o, mσ6o⟩
      · rw [prefixes_P hp hi hm h0]
        simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
        exact ⟨mσAo, mσ0o, mσ1o, mσ2o, mσ2o, mσ3o, mσ4o, mσ5o, mσ6o, mσ6o, mσ6o, mσ9⟩
  · rw [prefixes_C hp hi]
    simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
    exact ⟨mσAo, mσ0o, mσ1o, mσ2o⟩

/-! ### Its accesses -/

theorem addr8 : s.tpidr + o s + BitVec.ofNat 64 RSEQ_CS = area s + 8 := rfl
theorem addr4 : s.tpidr + o s + BitVec.ofNat 64 RSEQ_CPU_ID = area s + 4 := rfl
theorem rsName : RSEQ_OFFSET = RS := rfl

theorem ofNat_toNat64 (v : BitVec 64) : BitVec.ofNat 64 v.toNat = v := by
  rw [BitVec.ofNat_toNat, BitVec.setWidth_eq]

theorem wordAddr_eq (hp : Pre s) : wordAddr (s.syms SLABS) (cpu32 s).toNat (cx.cls s) = wao s := by
  rw [hp.word]; rfl

/-- The accesses: the link is read at the head the routine read once, and the commit, the last
access, stores what that read returned. -/
theorem trace_ok (hs : attempt.pre s) : attempt.trace s (Code.events isa stackPop s) := by
  have h := facts_of_pre hs
  have hp := pre_of h
  show cx.StackPopTrace s
    (cx.attemptEvents s (Code.events isa (.seq (.rseq arm body) (.block retWord)) s))
  simp only [Code.events, execBlock_arm h.toArm, events_arm h.toArm, attemptEvents_eq (c := cx) hX]
  refine ⟨(o s).toNat, s.csDesc.toNat, (cpu32 s).toNat, (wvo s).toNat, (lk s).toNat, ?_⟩
  simp only [Spec.Slab.Ctx.headTrace, Spec.Slab.Ctx.rd, Spec.Slab.Ctx.wr, tp_of (c := cx) hX,
    sym_of (c := cx) hX, ofNat_toNat64, addr8, addr4, rsName]
  by_cases hi : cx.InRange s
  · have hi' : (cpu32 s).toNat < SLAB_COUNT := (inRange_iff.mp hi)
    rw [wordAddr_eq hp]
    by_cases hm : (wvo s).toNat % 2 = 1
    · rw [events_M hp hi hm]
      have : ¬ (wvo s).toNat % 2 = 0 := by omega
      simp [hi', this, o]
    · by_cases h0 : (wvo s).toNat = 0
      · rw [events_Z hp hi hm h0]; simp [hi', h0, o]
      · rw [events_P hp hi hm h0]
        have : (wvo s).toNat % 2 = 0 := by omega
        simp [hi', h0, this, o]
  · have hi' : ¬ (cpu32 s).toNat < SLAB_COUNT := fun h' => hi (inRange_iff.mpr h')
    rw [events_C hp hi]; simp [hi', o]

theorem rseqOk (hs : attempt.pre s) : RseqOk target stackPop attempt s := by
  have h := facts_of_pre hs
  have hp := pre_of h
  have ht := trace_ok hs
  rw [code_eq] at ht ⊢
  exact ⟨ht, ⟨_, _, rfl, execBlock_dropLast_arm, arms_arm h.toArm⟩,
    fun p hpm => abortOk hp (mids hp p hpm)⟩

/-! ### The setup -/

theorem setupOk : SetupOk target contract attempt stackPopSig [] slowSetup := by
  refine .inr ⟨⟨readyS, rfl⟩, fun s hp => ?_⟩
  obtain ⟨he, hpre⟩ := slowSetup_ok hX hv (fun _ => rfl) (fun _ => rfl) (by decide) (by decide) hp
  exact ⟨s6 (s0 s), he, hpre, rfl, ⟨fun r hr => s6_s0_keeps (by revert r; decide), rfl⟩,
    argsKept_regs hX (by decide) (fun r hr => s6_s0_keeps (by revert r; decide)),
    fun h => absurd h (by decide)⟩

end Contract

/-! ## The witnesses -/

/-- A thread as the store-if-equal's witnesses (`Cas.wit`), offset 8, the head `head` in slab 0; a
magazine at `0x4000` whose link slot holds 0. -/
def wit (cpu head : BitVec 64) (rd wr : List Region) : State where
  gpr r := if r = .x10 then 0x10100 else if r = .x11 then 0x9000 else if r = .x13 then 0x1000000
    else if r = .x0 then 8 else 0
  mem := mkMem [(0x2000, 0x100), (0x10104, cpu), (0x1000008, head)]
  rd := rd
  wr := wr
  syms n := if n = "__rseq_offset" then 0x2000 else if n = "SLABS" then 0x1000000 else 0
  tpidr := 0x10000
  csDesc := 0x9000

def roP : List Region := [⟨0x2000, 8⟩, ⟨0x10104, 4⟩]

/-- No slab; a marker; an empty stack; a stack of one magazine. -/
def w0 : State := wit 8192 0 roP [⟨0x10108, 8⟩]
def w1 : State := wit 0 1 roP [⟨0x10108, 8⟩, ⟨0x1000008, 8⟩]
def w2 : State := wit 0 0 roP [⟨0x10108, 8⟩, ⟨0x1000008, 8⟩]
def w3 : State := wit 0 0x4000 (roP ++ [⟨0x4000, 8⟩]) [⟨0x10108, 8⟩, ⟨0x1000008, 8⟩]

theorem pre_w (s : State) (hm : ∃ cpu head rd wr, s = wit cpu head rd wr)
    (hrd : s.rd = cx.ro s) (hwr : s.wr = cx.rw s)
    (hnw : ((cx.regions s).all fun x => decide (x.1.base.toNat + x.1.len ≤ 2 ^ 64)) = true)
    (hp : Sig.pairsSep (cx.regions s) = true)
    (hres : ((cx.regions s).all fun x => (cx.A.reserved 0 s).all fun q => Region.sep x.1 q) = true)
    (hl : layoutCheck cx s = true) (hcls : cx.cls s = 1) (hsyms : s.syms SLABS = 0x1000000)
    (h0 : StackOk s.mem (s.mem.readW (wordAddr 0x1000000 0 1) 64) (cx.Apart s)) :
    contract.pre s := by
  refine (pre_iff cx (s := s) hv rfl rfl).mpr ⟨trivial,
    Slab.world_of_check hX.ptrBits hrd hwr hnw hp hres, rfl, rfl, trivial,
    layout_of_check hX.ptrBits hl, fun i hi => ?_⟩
  show StackOk (cx.mem s) (cx.wordOf s i) _
  simp only [Spec.Slab.Ctx.wordOf]
  rw [mem_of (c := cx) hX, sym_of (c := cx) hX, hcls, hsyms]
  rcases Nat.eq_zero_or_pos i with rfl | hpos
  · exact h0
  · obtain ⟨cpu, head, rd, wr, rfl⟩ := hm
    rw [show (wit cpu head rd wr).mem = mkMem _ from rfl,
      mkMem_slab_zero (by intro bv hbv; simp at hbv; rcases hbv with rfl | rfl | rfl <;> simp)
        hpos hi]
    exact stackOk_zero

theorem pre_w0 : attempt.pre w0 :=
  ⟨pre_w _ ⟨_, _, _, _, rfl⟩ (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (by rw [show w0.mem.readW _ 64 = 0 by decide +kernel]; exact stackOk_zero),
    ⟨by decide +kernel, by decide +kernel⟩, by decide +kernel⟩
theorem pre_w1 : attempt.pre w1 :=
  ⟨pre_w _ ⟨_, _, _, _, rfl⟩ (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (.inl (by decide +kernel)), ⟨by decide +kernel, by decide +kernel⟩, by decide +kernel⟩
theorem pre_w2 : attempt.pre w2 :=
  ⟨pre_w _ ⟨_, _, _, _, rfl⟩ (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (by rw [show w2.mem.readW _ 64 = 0 by decide +kernel]; exact stackOk_zero),
    ⟨by decide +kernel, by decide +kernel⟩, by decide +kernel⟩
theorem pre_w3 : attempt.pre w3 :=
  ⟨pre_w _ ⟨_, _, _, _, rfl⟩ (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (by
      rw [show w3.mem.readW (wordAddr 0x1000000 0 1) 64 = 0x4000 by decide +kernel]
      refine .inr ⟨[0x4000], .cons (by decide) (by decide) ?_, ?_⟩
      · rw [show w3.mem.readW 0x4000 64 = 0 by decide +kernel]; exact .nil
      · intro x hx
        rw [List.mem_singleton.mp hx]
        exact apart_of_check hX.ptrBits (by decide +kernel)),
    ⟨by decide +kernel, by decide +kernel⟩, by decide +kernel⟩

theorem outcomes_eq : stackPop.outcomes = [(0, true), (1, true), (2, true), (3, true)] := rfl

theorem visits_w0 : Visits isa stackPop w0 0 true := by
  have hp := pre_of (facts_of_pre pre_w0)
  have hi : ¬ cx.InRange w0 := by decide +kernel
  exact Visits.seqL (Visits.rseq (execBlock_arm hp.facts.toArm) (end_C hp hi))

theorem visits_w1 : Visits isa stackPop w1 1 true := by
  have hp := pre_of (facts_of_pre pre_w1)
  have hi : cx.InRange w1 := by decide +kernel
  have hm : (wvo w1).toNat % 2 = 1 := by decide +kernel
  exact Visits.seqL (Visits.rseq (execBlock_arm hp.facts.toArm) (end_M hp hi hm))

theorem visits_w2 : Visits isa stackPop w2 2 true := by
  have hp := pre_of (facts_of_pre pre_w2)
  have hi : cx.InRange w2 := by decide +kernel
  have hm : ¬ (wvo w2).toNat % 2 = 1 := by decide +kernel
  have h0 : (wvo w2).toNat = 0 := by decide +kernel
  exact Visits.seqL (Visits.rseq (execBlock_arm hp.facts.toArm) (end_Z hp hi hm h0))

theorem visits_w3 : Visits isa stackPop w3 3 true := by
  have hp := pre_of (facts_of_pre pre_w3)
  have hi : cx.InRange w3 := by decide +kernel
  have hm : ¬ (wvo w3).toNat % 2 = 1 := by decide +kernel
  have h0 : ¬ (wvo w3).toNat = 0 := by decide +kernel
  exact Visits.seqL (Visits.rseq (execBlock_arm hp.facts.toArm) (end_P hp hi hm h0))

def witnesses : List State := [w0, w1, w2, w3]

theorem witnessOk : (∃ w ∈ witnesses, attempt.pre w) ∧
    ∀ o ∈ stackPop.outcomes, ∃ w ∈ witnesses, attempt.pre w ∧ Visits isa stackPop w o.1 o.2 := by
  refine ⟨⟨w0, by simp [witnesses], pre_w0⟩, fun o ho => ?_⟩
  rw [outcomes_eq] at ho
  simp only [List.mem_cons, List.not_mem_nil, or_false] at ho
  rcases ho with rfl | rfl | rfl | rfl
  · exact ⟨w0, by simp [witnesses], pre_w0, visits_w0⟩
  · exact ⟨w1, by simp [witnesses], pre_w1, visits_w1⟩
  · exact ⟨w2, by simp [witnesses], pre_w2, visits_w2⟩
  · exact ⟨w3, by simp [witnesses], pre_w3, visits_w3⟩

theorem verified : Verified target stackPop attempt :=
  ⟨fun _ hs => correct hs, fun _ hs => rseqOk hs, ⟨_, pre_w0⟩, fun o ho => by
    obtain ⟨w, -, hw, hv⟩ := witnessOk.2 o ho
    exact ⟨w, hw, hv⟩⟩

/-- The last attempt, aborted any number of times, starts with the callee-saved registers, the
stack pointer and the argument of the first. -/
theorem restartsOk : ∀ s s', attempt.pre s → Restarts target stackPop attempt s s' →
    target.abiPreserved s s' ∧ ArgsKept target.abi stackPopSig s s' ∧
    (target.inline = true → target.preservedExcept [] s s') :=
  restartsOk_target verified (by decide) fun _ hs p hp r hr =>
    (mids (pre_of (facts_of_pre hs)) p hp).gpr r (by revert r; decide)

end VG.Proof.Slab.AArch64.StackPop
