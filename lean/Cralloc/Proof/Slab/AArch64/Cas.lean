module

public import Cralloc.Proof.Slab.AArch64.Slow
public import Cralloc.Proof.Slab.Witness

/-!
# The store-if-equal on AArch64

`slab_cas` (`Impl.Slab.AArch64.cas`) against `Spec.Slab.casContract`: its three ways out (no
slab; the word is not `expected`; the store), and in each its run, where it may be aborted, and
its memory accesses.
-/

@[expose] public section

namespace VG.Proof.Slab.AArch64.Cas

open VG.AArch64 VG.Spec.Slab VG.Impl.Slab.AArch64 VG.Proof.Slab.AArch64

abbrev cx : Spec.Slab.Ctx isa := ⟨abi, .cas, casSig⟩

theorem hX : A64 cx.A argRegs := a64_abi

abbrev contract : Contract isa := casContract abi

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
  .instr (.cmp .x .x3 (.reg .x1)),
  .exit .ne [] none,
  .instr (.str .x .x2 slabWord)]

theorem code_eq : cas = .seq (.rseq arm body) (.block retWord) := rfl

/-! ## The states -/

def σ8 (s : State) : State := (σ6o s).addFlags (wvo s) (~~~(s.gpr .x1)) true
def mEnd (s : State) : Mem := (m1 s).writeW (wao s) (s.gpr .x2)
def σ9 (s : State) : State := { σ8 s with mem := mEnd s }

/-! ## What the precondition gives -/

structure Pre (s : State) : Prop where
  facts : SFacts cx s
  word : wao s = cx.word s
  w : cx.w s = wvo s

theorem pre_of {s : State} (h : SFacts cx s) : Pre s := by
  have hw : wao s = cx.word s := wao_eq hX h.x13 rfl
  exact ⟨h, hw, by simp only [Spec.Slab.Ctx.w, mem_of (c := cx) hX, wvo, hw]⟩

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

theorem exec_cmp' : exec (.cmp .x .x3 (.reg .x1)) (σ6o s) = some (σ8 s) := by
  show some ((σ6o s).addFlags ((σ6o s).gpr .x3) (~~~((σ6o s).gpr .x1)) true) = _
  rw [σ6o_x3, gpr_σ6o (by decide)]
  rfl

theorem eval_ne_true (he : wvo s ≠ s.gpr .x1) : eval .ne (σ8 s) = some true := by
  have : wvo s - s.gpr .x1 ≠ 0 := fun h => he (by bv_omega)
  rw [σ8, Flags.eval_ne_cmp, beq_eq_false_iff_ne.mpr this]
  rfl

theorem eval_ne_false (he : wvo s = s.gpr .x1) : eval .ne (σ8 s) = some false := by
  rw [σ8, Flags.eval_ne_cmp, he]
  simp

theorem σ8_gpr (r : Reg) : (σ8 s).gpr r = (σ6o s).gpr r := by
  rw [σ8, RegUpd.gpr_addFlags]

theorem exec_commit (hp : Pre s) (hi : cx.InRange s) :
    exec (.str .x .x2 slabWord) (σ8 s) = some (σ9 s) := by
  have h13 : (σ8 s).gpr .x13 = s.gpr .x13 := by rw [σ8_gpr, σ6o_x13]
  have h8 : (σ8 s).gpr .x8 = r8o s := by rw [σ8_gpr, σ6o_x8]
  have hin : InRegions (σ8 s).wr ((σ8 s).ea slabWord) 8 := by
    rw [slabWord_ea _ h13 h8, show (σ8 s).wr = s.wr from rfl, hp.word]
    exact inWr (r := ⟨cx.word s, 8⟩) (hp.facts.word hi)
  rw [Access.exec_str_x hin, slabWord_ea _ h13 h8, σ8_gpr, gpr_σ6o (by decide)]
  rfl

theorem ev_commit :
    Instr.events (.str .x .x2 slabWord) (σ8 s) (σ9 s) = [⟨wao s, 8, true, (s.gpr .x2).toNat⟩] := by
  have h13 : (σ8 s).gpr .x13 = s.gpr .x13 := by rw [σ8_gpr, σ6o_x13]
  have h8 : (σ8 s).gpr .x8 = r8o s := by rw [σ8_gpr, σ6o_x8]
  simp only [Instr.events, slabWord_ea _ h13 h8, Width.bytes, toNat_read8]
  rw [show (σ9 s).mem = mEnd s from rfl, mEnd, Mem.readW_writeW_self64]

end Steps

/-! ## The three ways out -/

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

theorem runBody_M (hp : Pre s) (hi : cx.InRange s) (he : wvo s ≠ s.gpr .x1) :
    runBody isa body (σA s) = some (.done (σ8 s)) := by
  rw [body, runBody_instr exec_x3, runBody_instr (exec_cpu hp.facts.toArm (baseS hp.facts.ready)),
    runBody_instr exec_cmp, runBody_skip (eval_cpu_false hi), runBody_instr exec_lsl,
    runBody_instr exec_off1, runBody_instr exec_off2, runBody_instr (exec_w hp hi), runBody_instr exec_cmp', runBody_take (eval_ne_true he) execBlock_nil']

theorem prefixes_M (hp : Pre s) (hi : cx.InRange s) (he : wvo s ≠ s.gpr .x1) :
    bodyPrefixes isa body (σA s) =
      [σA s, σ0o s, σ1o s, σ2o s, σ2o s, σ3o s, σ4o s, σ5o s, σ6o s, σ8 s] := by
  rw [body, bodyPrefixes_instr exec_x3,
    bodyPrefixes_instr (exec_cpu hp.facts.toArm (baseS hp.facts.ready)), bodyPrefixes_instr exec_cmp,
    bodyPrefixes_skip (eval_cpu_false hi), bodyPrefixes_instr exec_lsl, bodyPrefixes_instr exec_off1,
    bodyPrefixes_instr exec_off2, bodyPrefixes_instr (exec_w hp hi), bodyPrefixes_instr exec_cmp',
    bodyPrefixes_take (eval_ne_true he)]

theorem end_M (hp : Pre s) (hi : cx.InRange s) (he : wvo s ≠ s.gpr .x1) :
    bodyEnd isa body (σA s) = some 9 := by
  rw [body, bodyEnd_instr exec_x3, bodyEnd_instr (exec_cpu hp.facts.toArm (baseS hp.facts.ready)),
    bodyEnd_instr exec_cmp, bodyEnd_skip (eval_cpu_false hi), bodyEnd_instr exec_lsl,
    bodyEnd_instr exec_off1, bodyEnd_instr exec_off2, bodyEnd_instr (exec_w hp hi), bodyEnd_instr exec_cmp', bodyEnd_take (eval_ne_true he)]
  rfl

theorem events_M (hp : Pre s) (hi : cx.InRange s) (he : wvo s ≠ s.gpr .x1) :
    bodyEvents isa body (σA s) =
      [⟨area s + 4, 4, false, (cpu32 s).toNat⟩, ⟨wao s, 8, false, (wvo s).toNat⟩] := by
  rw [body, bodyEvents_instr exec_x3, bodyEvents_instr (exec_cpu hp.facts.toArm (baseS hp.facts.ready)),
    bodyEvents_instr exec_cmp, bodyEvents_skip (eval_cpu_false hi), bodyEvents_instr exec_lsl,
    bodyEvents_instr exec_off1, bodyEvents_instr exec_off2, bodyEvents_instr (exec_w hp hi), bodyEvents_instr exec_cmp', bodyEvents_take (eval_ne_true he)]
  simp only [isa, ev_cpu (baseS hp.facts.ready), ev_word' (word_facts hp hi).2]; rfl

theorem runBody_S (hp : Pre s) (hi : cx.InRange s) (he : wvo s = s.gpr .x1) :
    runBody isa body (σA s) = some (.done (σ9 s)) := by
  rw [body, runBody_instr exec_x3, runBody_instr (exec_cpu hp.facts.toArm (baseS hp.facts.ready)),
    runBody_instr exec_cmp, runBody_skip (eval_cpu_false hi), runBody_instr exec_lsl,
    runBody_instr exec_off1, runBody_instr exec_off2, runBody_instr (exec_w hp hi), runBody_instr exec_cmp', runBody_skip (eval_ne_false he),
    runBody_instr (exec_commit hp hi), runBody_nil']

theorem prefixes_S (hp : Pre s) (hi : cx.InRange s) (he : wvo s = s.gpr .x1) :
    bodyPrefixes isa body (σA s) =
      [σA s, σ0o s, σ1o s, σ2o s, σ2o s, σ3o s, σ4o s, σ5o s, σ6o s, σ8 s, σ8 s] := by
  rw [body, bodyPrefixes_instr exec_x3,
    bodyPrefixes_instr (exec_cpu hp.facts.toArm (baseS hp.facts.ready)), bodyPrefixes_instr exec_cmp,
    bodyPrefixes_skip (eval_cpu_false hi), bodyPrefixes_instr exec_lsl, bodyPrefixes_instr exec_off1,
    bodyPrefixes_instr exec_off2, bodyPrefixes_instr (exec_w hp hi), bodyPrefixes_instr exec_cmp',
    bodyPrefixes_skip (eval_ne_false he), bodyPrefixes_last (exec_commit hp hi)]

theorem end_S (hp : Pre s) (hi : cx.InRange s) (he : wvo s = s.gpr .x1) :
    bodyEnd isa body (σA s) = some 11 := by
  rw [body, bodyEnd_instr exec_x3, bodyEnd_instr (exec_cpu hp.facts.toArm (baseS hp.facts.ready)),
    bodyEnd_instr exec_cmp, bodyEnd_skip (eval_cpu_false hi), bodyEnd_instr exec_lsl,
    bodyEnd_instr exec_off1, bodyEnd_instr exec_off2, bodyEnd_instr (exec_w hp hi), bodyEnd_instr exec_cmp', bodyEnd_skip (eval_ne_false he),
    bodyEnd_instr (exec_commit hp hi), bodyEnd_nil']
  rfl

theorem events_S (hp : Pre s) (hi : cx.InRange s) (he : wvo s = s.gpr .x1) :
    bodyEvents isa body (σA s) =
      [⟨area s + 4, 4, false, (cpu32 s).toNat⟩, ⟨wao s, 8, false, (wvo s).toNat⟩,
        ⟨wao s, 8, true, (s.gpr .x2).toNat⟩] := by
  rw [body, bodyEvents_instr exec_x3, bodyEvents_instr (exec_cpu hp.facts.toArm (baseS hp.facts.ready)),
    bodyEvents_instr exec_cmp, bodyEvents_skip (eval_cpu_false hi), bodyEvents_instr exec_lsl,
    bodyEvents_instr exec_off1, bodyEvents_instr exec_off2, bodyEvents_instr (exec_w hp hi), bodyEvents_instr exec_cmp', bodyEvents_skip (eval_ne_false he),
    bodyEvents_instr (exec_commit hp hi), bodyEvents_nil']
  simp only [ev_cpu (baseS hp.facts.ready), ev_word' (word_facts hp hi).2, ev_commit]; rfl

end Cases

/-! ## The contract -/

section Contract

variable {s : State}

theorem hv : cx.A.args ((cx.sig.words cx.A.ptrBits).map (·.bits cx.A.ptrBits)) =
    some (fun s => [s.gpr .x0, s.gpr .x1, s.gpr .x2]) := rfl

theorem facts_of_pre (hs : attempt.pre s) : SFacts cx s := by
  have := (pre_iff cx (s := s) hv rfl rfl).mp hs.1
  exact sfacts_of_world hX this.2.1 this.2.2 hs.2

theorem unchanged_cs : Unchanged [cx.csField s] (cx.mem s) (m1 s) := by
  rw [csField_of (c := cx) hX, mem_of (c := cx) hX]; exact frame_m1

theorem mσ8 : Mid scratchS s (m1 s) (σ8 s) := mσ6o.addFlags _ _ _
theorem mσ9 : Mid scratchS s (mEnd s) (σ9 s) := mσ8.withMem _

theorem post_eq (s' : State) : attempt.post s s' ↔
    NoCpu cx s s' ∨ (cx.InRange s ∧ s'.gpr .x0 = cx.w s ∧
      ((cx.w s = s.gpr .x1 ∧ Unchanged [cx.csField s, ⟨cx.word s, 8⟩] (cx.mem s) s'.mem ∧
          s'.mem.readW (cx.word s) 64 = s.gpr .x2) ∨
        (cx.w s ≠ s.gpr .x1 ∧ Unchanged [cx.csField s] (cx.mem s) s'.mem))) :=
  post_iff cx hv s s'

theorem correct (hs : attempt.pre s) :
    (∃ s', Exec isa cas s s' ∧ target.abiPreserved s s' ∧ attempt.post s s') ∨
    (∃ t s', ExecExit isa cas s t s' ∧ target.abiPreserved s s' ∧ attempt.exit t s s') := by
  have h := facts_of_pre hs
  have hp := pre_of h
  refine .inl ?_
  rw [code_eq]
  by_cases hi : cx.InRange s
  · by_cases he : wvo s = s.gpr .x1
    · refine ⟨ret0 (σ9 s), .seq (.rseq (execBlock_arm h.toArm) (runBody_S hp hi he))
        (.block (execBlock_ret _)), abi_ret mσ9,
        (post_eq _).mpr (.inr ⟨hi, ?_, .inl ⟨by rw [hp.w]; exact he, ?_, ?_⟩⟩)⟩
      · rw [ret0, RegUpd.gpr_setReg_self, hp.w]
        show (σ8 s).gpr .x3 = _
        rw [σ8_gpr, σ6o_x3]
      · rw [csField_of (c := cx) hX, mem_of (c := cx) hX, ← hp.word]
        exact frame_two s.mem _ _ _ _
      · show (mEnd s).readW (cx.word s) 64 = _
        rw [mEnd, ← hp.word, Mem.readW_writeW_self64]
    · refine ⟨ret0 (σ8 s), .seq (.rseq (execBlock_arm h.toArm) (runBody_M hp hi he))
        (.block (execBlock_ret _)), abi_ret mσ8,
        (post_eq _).mpr (.inr ⟨hi, ?_, .inr ⟨by rw [hp.w]; exact he, unchanged_cs⟩⟩)⟩
      rw [ret0, RegUpd.gpr_setReg_self, hp.w, σ8_gpr, σ6o_x3]
  · exact ⟨ret0 (σ2o s), .seq (.rseq (execBlock_arm h.toArm) (runBody_C hp hi))
      (.block (execBlock_ret _)), abi_ret mσ2o,
      (post_eq _).mpr (.inl ⟨hi, ret_σ2o (s := s), unchanged_cs⟩)⟩

/-! ### Where it may be aborted -/

theorem abort_eq (p : State) : attempt.abort s p ↔
    [p.gpr .x0, p.gpr .x1, p.gpr .x2] = [s.gpr .x0, s.gpr .x1, s.gpr .x2] ∧
      (Unchanged [cx.csField s] (cx.mem s) p.mem ∧ cx.inv p) :=
  abort_iff cx hv s p

theorem abortOk (hp : Pre s) {p : State} (hm : Mid scratchS s (m1 s) p) :
    armed p ∧ target.abiPreserved s p ∧ attempt.abort s p ∧
      ∀ p', target.rely p p' → attempt.env p p' → attempt.pre p' := by
  obtain ⟨hip, hre⟩ := retryS hX hv (post := fun s s' => _) (fun _ => rfl) (fun _ => rfl) (by decide)
    hp.facts.pre ⟨hp.facts.ready, hp.facts.x13⟩ hm (by decide) frame_m1
    (harmless_cs1 hX hp.facts.pre)
  refine ⟨armed_of_mid hp.facts.toArm (xs := []) frame_m1 m1_cs (by simp) hm, abi_of hm, ?_, hre⟩
  refine (abort_eq p).mpr ⟨by rw [hm.gpr .x0 (by decide), hm.gpr .x1 (by decide),
    hm.gpr .x2 (by decide)], by rw [hm.mem]; exact unchanged_cs, hip⟩

/-- Wherever it may be aborted, memory is as after arming. -/
theorem mids (hp : Pre s) :
    ∀ p ∈ Code.abortStates isa (.seq (.rseq arm body) (.block retWord)) s, Mid scratchS s (m1 s) p := by
  simp only [Code.abortStates, execBlock_arm hp.facts.toArm]
  by_cases hi : cx.InRange s
  · by_cases he : wvo s = s.gpr .x1
    · rw [prefixes_S hp hi he]
      simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
      exact ⟨mσAo, mσ0o, mσ1o, mσ2o, mσ2o, mσ3o, mσ4o, mσ5o, mσ6o, mσ8, mσ8⟩
    · rw [prefixes_M hp hi he]
      simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
      exact ⟨mσAo, mσ0o, mσ1o, mσ2o, mσ2o, mσ3o, mσ4o, mσ5o, mσ6o, mσ8⟩
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

theorem trace_ok (hs : attempt.pre s) : attempt.trace s (Code.events isa cas s) := by
  have h := facts_of_pre hs
  have hp := pre_of h
  show cx.CasTrace s (cx.arg s 1) (cx.arg s 2)
    (cx.attemptEvents s (Code.events isa (.seq (.rseq arm body) (.block retWord)) s))
  simp only [Code.events, execBlock_arm h.toArm, events_arm h.toArm, attemptEvents_eq (c := cx) hX]
  refine ⟨(o s).toNat, s.csDesc.toNat, (cpu32 s).toNat, (wvo s).toNat, ?_⟩
  simp only [Spec.Slab.Ctx.headTrace, Spec.Slab.Ctx.rd, Spec.Slab.Ctx.wr, tp_of (c := cx) hX,
    sym_of (c := cx) hX, ofNat_toNat64, addr8, addr4, rsName,
    show cx.arg s 1 = s.gpr .x1 from rfl, show cx.arg s 2 = s.gpr .x2 from rfl]
  by_cases hi : cx.InRange s
  · have hi' : (cpu32 s).toNat < SLAB_COUNT := (inRange_iff.mp hi)
    rw [wordAddr_eq hp]
    by_cases he : wvo s = s.gpr .x1
    · rw [events_S hp hi he]; simp [hi', he, o]
    · rw [events_M hp hi he]
      have : ¬ (wvo s).toNat = (s.gpr .x1).toNat := fun h' => he (BitVec.eq_of_toNat_eq h')
      simp [hi', this, o]
  · have hi' : ¬ (cpu32 s).toNat < SLAB_COUNT := fun h' => hi (inRange_iff.mpr h')
    rw [events_C hp hi]; simp [hi', o]

theorem rseqOk (hs : attempt.pre s) : RseqOk target cas attempt s := by
  have h := facts_of_pre hs
  have hp := pre_of h
  have ht := trace_ok hs
  rw [code_eq] at ht ⊢
  exact ⟨ht, ⟨_, _, rfl, execBlock_dropLast_arm, arms_arm h.toArm⟩,
    fun p hpm => abortOk hp (mids hp p hpm)⟩

/-! ### The setup -/

theorem setupOk {l : List Instr} {g : State → State} (hl : AreaSetup l g) :
    SetupOk target contract attempt casSig [] (slowOf l) := by
  refine .inr ⟨⟨readyS, rfl⟩, fun s hp => ?_⟩
  obtain ⟨he, hpre⟩ := slowSetup_ok hl hX hv (fun _ => rfl) (fun _ => rfl) (by decide) (by decide) hp
  exact ⟨g (s0 s), he, hpre, hl.mem, ⟨fun r hr => hl.keeps0 (by revert r; decide), hl.sp⟩,
    argsKept_regs hX (by decide) (fun r hr => hl.keeps0 (by revert r; decide)),
    fun h => absurd h (by decide)⟩

end Contract

/-! ## The witnesses -/

/-- A thread as the pop's witnesses (`wit`), its offset 8 (class 1's word), `expected` and `new` in
`x1` and `x2`. -/
def wit (cpu word e x : BitVec 64) (wr : List Region) : State where
  gpr r :=
    if r = .x10 then 0x10100 else if r = .x11 then 0x9000 else if r = .x13 then 0x1000000
    else if r = .x0 then 8
    else if r = .x1 then e else if r = .x2 then x else 0
  mem := mkMem [(0x2000, 0x100), (0x10104, cpu), (0x1000008, word)]
  rd := [⟨0x2000, 8⟩, ⟨0x10104, 4⟩]
  wr := wr
  syms n := if n = "__rseq_offset" then 0x2000 else if n = "SLABS" then 0x1000000 else 0
  tpidr := 0x10000
  csDesc := 0x9000

def w0 : State := wit 8192 0 0 0 [⟨0x10108, 8⟩]
def w1 : State := wit 0 0 8 0x4010 [⟨0x10108, 8⟩, ⟨0x1000008, 8⟩]
def w2 : State := wit 0 0 0 0x4010 [⟨0x10108, 8⟩, ⟨0x1000008, 8⟩]

theorem pre_w (s : State) (hrd : s.rd = cx.ro s) (hwr : s.wr = cx.rw s)
    (hnw : ((cx.regions s).all fun x => decide (x.1.base.toNat + x.1.len ≤ 2 ^ 64)) = true)
    (hp : Sig.pairsSep (cx.regions s) = true)
    (hres : ((cx.regions s).all fun x => (cx.A.reserved 0 s).all fun q => Region.sep x.1 q) = true)
    (hl : layoutCheck cx s = true) (hm : ∃ cpu word e x wr, s = wit cpu word e x wr)
    (hcls : cx.cls s = 1) (hsyms : s.syms SLABS = 0x1000000)
    (h0 : (s.mem.readW (wordAddr 0x1000000 0 1) 64).toNat ≠ NO_CPU) :
    contract.pre s := by
  refine (pre_iff cx (s := s) hv rfl rfl).mpr ⟨trivial, Slab.world_of_check hX.ptrBits hrd hwr hnw hp hres,
    rfl, rfl, trivial, layout_of_check hX.ptrBits hl, fun i hi => ?_⟩
  simp only [Spec.Slab.Ctx.wordOf]
  rw [mem_of (c := cx) hX, sym_of (c := cx) hX, hcls, hsyms]
  rcases Nat.eq_zero_or_pos i with rfl | hpos
  · exact h0
  · obtain ⟨cpu, word, e, x, wr, rfl⟩ := hm
    rw [show (wit cpu word e x wr).mem = mkMem _ from rfl,
      mkMem_slab_zero (by intro bv hbv; simp at hbv; rcases hbv with rfl | rfl | rfl <;> simp)
        hpos hi]
    decide

theorem pre_w0 : attempt.pre w0 :=
  ⟨pre_w _ (by decide +kernel) (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (by decide +kernel) ⟨_, _, _, _, _, rfl⟩ (by decide +kernel) (by decide +kernel)
    (by decide +kernel), ⟨by decide +kernel, by decide +kernel⟩, by decide +kernel⟩
theorem pre_w1 : attempt.pre w1 :=
  ⟨pre_w _ (by decide +kernel) (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (by decide +kernel) ⟨_, _, _, _, _, rfl⟩ (by decide +kernel) (by decide +kernel)
    (by decide +kernel), ⟨by decide +kernel, by decide +kernel⟩, by decide +kernel⟩
theorem pre_w2 : attempt.pre w2 :=
  ⟨pre_w _ (by decide +kernel) (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (by decide +kernel) ⟨_, _, _, _, _, rfl⟩ (by decide +kernel) (by decide +kernel)
    (by decide +kernel), ⟨by decide +kernel, by decide +kernel⟩, by decide +kernel⟩

theorem outcomes_eq : cas.outcomes = [(0, true), (1, true), (2, true)] := rfl

theorem visits_w0 : Visits isa cas w0 0 true := by
  have hp := pre_of (facts_of_pre pre_w0)
  have hi : ¬ cx.InRange w0 := by decide +kernel
  exact Visits.seqL (Visits.rseq (execBlock_arm hp.facts.toArm) (end_C hp hi))

theorem visits_w1 : Visits isa cas w1 1 true := by
  have hp := pre_of (facts_of_pre pre_w1)
  have hi : cx.InRange w1 := by decide +kernel
  have he : ¬ wvo w1 = w1.gpr .x1 := by decide +kernel
  exact Visits.seqL (Visits.rseq (execBlock_arm hp.facts.toArm) (end_M hp hi he))

theorem visits_w2 : Visits isa cas w2 2 true := by
  have hp := pre_of (facts_of_pre pre_w2)
  have hi : cx.InRange w2 := by decide +kernel
  have he : wvo w2 = w2.gpr .x1 := by decide +kernel
  exact Visits.seqL (Visits.rseq (execBlock_arm hp.facts.toArm) (end_S hp hi he))

def witnesses : List State := [w0, w1, w2]

theorem witnessOk : (∃ w ∈ witnesses, attempt.pre w) ∧
    ∀ o ∈ cas.outcomes, ∃ w ∈ witnesses, attempt.pre w ∧ Visits isa cas w o.1 o.2 := by
  refine ⟨⟨w0, by simp [witnesses], pre_w0⟩, fun o ho => ?_⟩
  rw [outcomes_eq] at ho
  simp only [List.mem_cons, List.not_mem_nil, or_false] at ho
  rcases ho with rfl | rfl | rfl
  · exact ⟨w0, by simp [witnesses], pre_w0, visits_w0⟩
  · exact ⟨w1, by simp [witnesses], pre_w1, visits_w1⟩
  · exact ⟨w2, by simp [witnesses], pre_w2, visits_w2⟩

theorem verified : Verified target cas attempt :=
  ⟨fun _ hs => correct hs, fun _ hs => rseqOk hs, ⟨_, pre_w0⟩, fun o ho => by
    obtain ⟨w, -, hw, hv⟩ := witnessOk.2 o ho
    exact ⟨w, hw, hv⟩⟩

/-- The last attempt, aborted any number of times, starts with the callee-saved registers, the
stack pointer and the arguments of the first. -/
theorem restartsOk : ∀ s s', attempt.pre s → Restarts target cas attempt s s' →
    target.abiPreserved s s' ∧ ArgsKept target.abi casSig s s' ∧
    (target.inline = true → target.preservedExcept [] s s') :=
  restartsOk_target verified (by decide) fun _ hs p hp r hr =>
    (mids (pre_of (facts_of_pre hs)) p hp).gpr r (by revert r; decide)

end VG.Proof.Slab.AArch64.Cas
