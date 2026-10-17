module

public import Asmalloc.Proof.Slab.X86_64.Slow
public import Asmalloc.Proof.Slab.X86_64.Witness
public import Asmalloc.Proof.Slab.Stack

/-!
# The stack pop on x86-64

`slab_stack_pop` (`Impl.Slab.X86_64.stackPop`) against
`Spec.Slab.stackPopContract`: its four ways out (no slab; the head is a
marker; the stack is empty; the pop), and in each its run, where it may be
aborted, and its memory accesses. The link it commits is read, inside the
sequence, from the head it read once.
-/

@[expose] public section

namespace VG.Proof.Slab.X86_64.StackPop

open VG.X86_64 VG.Spec.Slab VG.Impl.Slab.X86_64 VG.Proof.Slab.X86_64 VG.Proof.Slab

abbrev cx : Spec.Slab.Ctx isa := ⟨abi, .stackPop, stackPopSig⟩

theorem hX : X86 cx.A := x86_abi

def body : List (Step Instr Cond) := [
  .instr (.mov32 .r8 (.mem { base := .r10, disp := 4, seg := some .fs })),
  .instr (.alu32 .cmp .r8 (.imm 8192)),
  .exit .ae noCpu none,
  .instr (.shift .shl .r8 12),
  .instr (.mov32 .r9 (.reg .rdi)),
  .instr (.alu32 .and .r9 (.imm 4088)),
  .instr (.alu .add .r8 (.reg .r9)),
  .instr (.leaSym .r9 "SLABS"),
  .instr (.mov .rax (.mem slabWord)),
  .instr (.alu32 .test .rax (.imm 1)),
  .exit .ne [] none,
  .instr (.alu .test .rax (.reg .rax)),
  .exit .e [] none,
  .instr (.mov .rcx (.mem { base := .rax })),
  .instr (.store slabWord .rcx)]

theorem code_eq : stackPop = .rseq arm body := rfl

/-! ## The states -/

def tst (s : State) : BitVec 32 := (wvo s).setWidth 32 &&& 1
def σ9 (s : State) : State := arithFlags (σ8o s) (tst s) false false
def σ10 (s : State) : State := arithFlags (σ9 s) (wvo s &&& wvo s) false false
/-- The top magazine's link: the next head. -/
def lk (s : State) : BitVec 64 := s.mem.readW (wvo s) 64
def σ12 (s : State) : State := (σ10 s).setReg .rcx (lk s)
def mEnd (s : State) : Mem := (m1 s).writeW (wao s) (lk s)
def σ13 (s : State) : State := { σ12 s with mem := mEnd s }
def σC (s : State) : State := (σ2 s).setReg32 .rax 2

theorem tst_zero_iff (s : State) : tst s = 0 ↔ ¬ (wvo s).toNat % 2 = 1 := by
  have h : (tst s).toNat = (wvo s).toNat % 2 := by
    rw [tst, BitVec.toNat_and, BitVec.toNat_setWidth, show (1 : BitVec 32).toNat = 1 from rfl,
      Nat.and_one_is_mod, Nat.mod_mod_of_dvd _ (by decide : 2 ∣ 2 ^ 32)]
  rw [← BitVec.toNat_inj, h, show (0 : BitVec 32).toNat = 0 from rfl]; omega

/-! ## What the precondition gives -/

structure Pre (s : State) : Prop where
  facts : SFacts cx s
  word : wao s = cx.word s
  w : cx.w s = wvo s
  stack : cx.InRange s → StackOk s.mem (wvo s) (cx.Apart s)

theorem pre_of {s : State} (h : SFacts cx s) : Pre s := by
  have hw : wao s = cx.word s := wao_eq hX rfl
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

theorem eval_cpu_true (hi : ¬ cx.InRange s) : eval .ae (σ2 s) = some true := by
  rw [eval_cpu, inRange_iff] at *; simp [hi]

theorem eval_cpu_false (hi : cx.InRange s) : eval .ae (σ2 s) = some false := by
  rw [eval_cpu, inRange_iff] at *; simp [hi]

theorem word_facts (hp : Pre s) (hi : cx.InRange s) :
    InRegions (s.rd ++ s.wr) (wao s) 8 ∧ Region.Disjoint ⟨wao s, 8⟩ ⟨area s + 8, 8⟩ := by
  rw [hp.word]
  exact ⟨inRdWr (r := ⟨cx.word s, 8⟩) (hp.facts.word hi), (hp.facts.cs_word hi).symm⟩

theorem exec_w (hp : Pre s) (hi : cx.InRange s) :
    exec (.mov .rax (.mem slabWord)) (σ7o s) = some (σ8o s) :=
  exec_word (word_facts hp hi).1 (word_facts hp hi).2

theorem exec_test1 : exec (.alu32 .test .rax (.imm 1)) (σ8o s) = some (σ9 s) := by
  rw [Flags.exec_test32_imm, σ8o_rax]; rfl

theorem eval_marker (hm : (wvo s).toNat % 2 = 1) : eval .ne (σ9 s) = some true := by
  have : ¬ tst s = 0 := fun h => (tst_zero_iff s).mp h hm
  simpa [eval, σ9, RegUpd.zf_arithFlags] using this

theorem eval_even (hm : ¬ (wvo s).toNat % 2 = 1) : eval .ne (σ9 s) = some false := by
  have : tst s = 0 := (tst_zero_iff s).mpr hm
  simp [eval, σ9, RegUpd.zf_arithFlags, this]

theorem σ9_rax : (σ9 s).gpr .rax = wvo s := by rw [σ9, RegUpd.gpr_arithFlags, σ8o_rax]

theorem exec_test0 : exec (.alu .test .rax (.reg .rax)) (σ9 s) = some (σ10 s) := by
  simp only [exec, execAlu, readSrc, Option.bind_some, σ9_rax]; rfl

theorem eval_zero (h0 : (wvo s).toNat = 0) : eval .e (σ10 s) = some true := by
  have : wvo s = 0 := BitVec.eq_of_toNat_eq h0
  simp [eval, σ10, RegUpd.zf_arithFlags, this]

theorem eval_nonzero (h0 : ¬ (wvo s).toNat = 0) : eval .e (σ10 s) = some false := by
  have : ¬ wvo s = 0 := fun h => h0 (by rw [h]; rfl)
  simpa [eval, σ10, RegUpd.zf_arithFlags, BitVec.and_self] using this

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

theorem σ10_rax : (σ10 s).gpr .rax = wvo s := by rw [σ10, RegUpd.gpr_arithFlags, σ9_rax]

theorem exec_link (hp : Pre s) (hi : cx.InRange s) (h0 : ¬ (wvo s).toNat = 0)
    (hm : ¬ (wvo s).toNat % 2 = 1) :
    exec (.mov .rcx (.mem { base := .rax })) (σ10 s) = some (σ12 s) := by
  obtain ⟨hin, hdj⟩ := top_facts hp hi h0 hm
  have hin' : InRegions ((σ10 s).rd ++ (σ10 s).wr) ((σ10 s).ea { base := .rax }) 8 := by
    rw [Access.ea_base, σ10_rax]; exact hin
  rw [exec_load _ hin', Access.ea_base, σ10_rax, show (σ10 s).mem = m1 s from rfl, m1,
    readW64_writeW_disj hdj]
  rfl

theorem σ12_r9 : (σ12 s).gpr .r9 = s.syms "SLABS" := by
  rw [σ12, RegUpd.gpr_setReg_of_ne _ _ (by decide), σ10, RegUpd.gpr_arithFlags, σ9,
    RegUpd.gpr_arithFlags, σ8o_r9]
theorem σ12_r8 : (σ12 s).gpr .r8 = r8o s := by
  rw [σ12, RegUpd.gpr_setReg_of_ne _ _ (by decide), σ10, RegUpd.gpr_arithFlags, σ9,
    RegUpd.gpr_arithFlags, σ8o_r8]
theorem σ12_rcx : (σ12 s).gpr .rcx = lk s := RegUpd.gpr_setReg_self _ _ _

theorem exec_commit (hp : Pre s) (hi : cx.InRange s) :
    exec (.store slabWord .rcx) (σ12 s) = some (σ13 s) := by
  have hin : InRegions (σ12 s).wr ((σ12 s).ea slabWord) 8 := by
    rw [slabWord_ea_o _ σ12_r9 σ12_r8, show (σ12 s).wr = s.wr from rfl, hp.word]
    exact inWr (r := ⟨cx.word s, 8⟩) (hp.facts.word hi)
  rw [exec_store _ hin, slabWord_ea_o _ σ12_r9 σ12_r8, σ12_rcx]
  rfl

end Steps

/-! ## The four ways out -/

section Cases

variable {s : State}

theorem runBody_C (hp : Pre s) (hi : ¬ cx.InRange s) :
    runBody isa body (σA s) = some (.done (σC s)) := by
  rw [body, runBody_instr (exec_cpu hp.facts.toArm), runBody_instr exec_cmp,
    runBody_take (eval_cpu_true hi) rfl]
  rfl

theorem prefixes_C (hp : Pre s) (hi : ¬ cx.InRange s) :
    bodyPrefixes isa body (σA s) = [σA s, σ1 s, σ2 s] := by
  rw [body, bodyPrefixes_instr (exec_cpu hp.facts.toArm), bodyPrefixes_instr exec_cmp,
    bodyPrefixes_take (eval_cpu_true hi)]

theorem end_C (hp : Pre s) (hi : ¬ cx.InRange s) : bodyEnd isa body (σA s) = some 2 := by
  rw [body, bodyEnd_instr (exec_cpu hp.facts.toArm), bodyEnd_instr exec_cmp,
    bodyEnd_take (eval_cpu_true hi)]
  rfl

theorem events_C (hp : Pre s) (hi : ¬ cx.InRange s) :
    bodyEvents isa body (σA s) = [⟨area s + 4, 4, false, (cpu32 s).toNat⟩] := by
  rw [body, bodyEvents_instr (exec_cpu hp.facts.toArm), bodyEvents_instr exec_cmp,
    bodyEvents_take (eval_cpu_true hi)]
  simp only [isa, ev_cpuLoad]; rfl

theorem runBody_M (hp : Pre s) (hi : cx.InRange s) (hm : (wvo s).toNat % 2 = 1) :
    runBody isa body (σA s) = some (.done (σ9 s)) := by
  rw [body, runBody_instr (exec_cpu hp.facts.toArm), runBody_instr exec_cmp,
    runBody_skip (eval_cpu_false hi), runBody_instr exec_shl_r8, runBody_instr exec_off1,
    runBody_instr exec_off2, runBody_instr exec_off3, runBody_instr exec_off4,
    runBody_instr (exec_w hp hi), runBody_instr exec_test1, runBody_take (eval_marker hm) rfl]

theorem prefixes_M (hp : Pre s) (hi : cx.InRange s) (hm : (wvo s).toNat % 2 = 1) :
    bodyPrefixes isa body (σA s) =
      [σA s, σ1 s, σ2 s, σ2 s, σ3 s, σ4o s, σ5o s, σ6o s, σ7o s, σ8o s, σ9 s] := by
  rw [body, bodyPrefixes_instr (exec_cpu hp.facts.toArm), bodyPrefixes_instr exec_cmp,
    bodyPrefixes_skip (eval_cpu_false hi), bodyPrefixes_instr exec_shl_r8,
    bodyPrefixes_instr exec_off1, bodyPrefixes_instr exec_off2, bodyPrefixes_instr exec_off3,
    bodyPrefixes_instr exec_off4, bodyPrefixes_instr (exec_w hp hi), bodyPrefixes_instr exec_test1,
    bodyPrefixes_take (eval_marker hm)]

theorem end_M (hp : Pre s) (hi : cx.InRange s) (hm : (wvo s).toNat % 2 = 1) :
    bodyEnd isa body (σA s) = some 10 := by
  rw [body, bodyEnd_instr (exec_cpu hp.facts.toArm), bodyEnd_instr exec_cmp,
    bodyEnd_skip (eval_cpu_false hi), bodyEnd_instr exec_shl_r8, bodyEnd_instr exec_off1,
    bodyEnd_instr exec_off2, bodyEnd_instr exec_off3, bodyEnd_instr exec_off4,
    bodyEnd_instr (exec_w hp hi), bodyEnd_instr exec_test1, bodyEnd_take (eval_marker hm)]
  rfl

theorem events_M (hp : Pre s) (hi : cx.InRange s) (hm : (wvo s).toNat % 2 = 1) :
    bodyEvents isa body (σA s) =
      [⟨area s + 4, 4, false, (cpu32 s).toNat⟩, ⟨wao s, 8, false, (wvo s).toNat⟩] := by
  rw [body, bodyEvents_instr (exec_cpu hp.facts.toArm), bodyEvents_instr exec_cmp,
    bodyEvents_skip (eval_cpu_false hi), bodyEvents_instr exec_shl_r8, bodyEvents_instr exec_off1,
    bodyEvents_instr exec_off2, bodyEvents_instr exec_off3, bodyEvents_instr exec_off4,
    bodyEvents_instr (exec_w hp hi), bodyEvents_instr exec_test1, bodyEvents_take (eval_marker hm)]
  simp only [isa, ev_cpuLoad, ev_word (word_facts hp hi).2]; rfl

theorem runBody_Z (hp : Pre s) (hi : cx.InRange s) (hm : ¬ (wvo s).toNat % 2 = 1)
    (h0 : (wvo s).toNat = 0) : runBody isa body (σA s) = some (.done (σ10 s)) := by
  rw [body, runBody_instr (exec_cpu hp.facts.toArm), runBody_instr exec_cmp,
    runBody_skip (eval_cpu_false hi), runBody_instr exec_shl_r8, runBody_instr exec_off1,
    runBody_instr exec_off2, runBody_instr exec_off3, runBody_instr exec_off4,
    runBody_instr (exec_w hp hi), runBody_instr exec_test1, runBody_skip (eval_even hm),
    runBody_instr exec_test0, runBody_take (eval_zero h0) rfl]

theorem prefixes_Z (hp : Pre s) (hi : cx.InRange s) (hm : ¬ (wvo s).toNat % 2 = 1)
    (h0 : (wvo s).toNat = 0) : bodyPrefixes isa body (σA s) =
      [σA s, σ1 s, σ2 s, σ2 s, σ3 s, σ4o s, σ5o s, σ6o s, σ7o s, σ8o s, σ9 s, σ9 s, σ10 s] := by
  rw [body, bodyPrefixes_instr (exec_cpu hp.facts.toArm), bodyPrefixes_instr exec_cmp,
    bodyPrefixes_skip (eval_cpu_false hi), bodyPrefixes_instr exec_shl_r8,
    bodyPrefixes_instr exec_off1, bodyPrefixes_instr exec_off2, bodyPrefixes_instr exec_off3,
    bodyPrefixes_instr exec_off4, bodyPrefixes_instr (exec_w hp hi), bodyPrefixes_instr exec_test1,
    bodyPrefixes_skip (eval_even hm), bodyPrefixes_instr exec_test0, bodyPrefixes_take (eval_zero h0)]

theorem end_Z (hp : Pre s) (hi : cx.InRange s) (hm : ¬ (wvo s).toNat % 2 = 1)
    (h0 : (wvo s).toNat = 0) : bodyEnd isa body (σA s) = some 12 := by
  rw [body, bodyEnd_instr (exec_cpu hp.facts.toArm), bodyEnd_instr exec_cmp,
    bodyEnd_skip (eval_cpu_false hi), bodyEnd_instr exec_shl_r8, bodyEnd_instr exec_off1,
    bodyEnd_instr exec_off2, bodyEnd_instr exec_off3, bodyEnd_instr exec_off4,
    bodyEnd_instr (exec_w hp hi), bodyEnd_instr exec_test1, bodyEnd_skip (eval_even hm),
    bodyEnd_instr exec_test0, bodyEnd_take (eval_zero h0)]
  rfl

theorem events_Z (hp : Pre s) (hi : cx.InRange s) (hm : ¬ (wvo s).toNat % 2 = 1)
    (h0 : (wvo s).toNat = 0) : bodyEvents isa body (σA s) =
      [⟨area s + 4, 4, false, (cpu32 s).toNat⟩, ⟨wao s, 8, false, (wvo s).toNat⟩] := by
  rw [body, bodyEvents_instr (exec_cpu hp.facts.toArm), bodyEvents_instr exec_cmp,
    bodyEvents_skip (eval_cpu_false hi), bodyEvents_instr exec_shl_r8, bodyEvents_instr exec_off1,
    bodyEvents_instr exec_off2, bodyEvents_instr exec_off3, bodyEvents_instr exec_off4,
    bodyEvents_instr (exec_w hp hi), bodyEvents_instr exec_test1, bodyEvents_skip (eval_even hm),
    bodyEvents_instr exec_test0, bodyEvents_take (eval_zero h0)]
  simp only [isa, ev_cpuLoad, ev_word (word_facts hp hi).2]; rfl

theorem runBody_P (hp : Pre s) (hi : cx.InRange s) (hm : ¬ (wvo s).toNat % 2 = 1)
    (h0 : ¬ (wvo s).toNat = 0) : runBody isa body (σA s) = some (.done (σ13 s)) := by
  rw [body, runBody_instr (exec_cpu hp.facts.toArm), runBody_instr exec_cmp,
    runBody_skip (eval_cpu_false hi), runBody_instr exec_shl_r8, runBody_instr exec_off1,
    runBody_instr exec_off2, runBody_instr exec_off3, runBody_instr exec_off4,
    runBody_instr (exec_w hp hi), runBody_instr exec_test1, runBody_skip (eval_even hm),
    runBody_instr exec_test0, runBody_skip (eval_nonzero h0), runBody_instr (exec_link hp hi h0 hm),
    runBody_instr (exec_commit hp hi), runBody_nil']

theorem prefixes_P (hp : Pre s) (hi : cx.InRange s) (hm : ¬ (wvo s).toNat % 2 = 1)
    (h0 : ¬ (wvo s).toNat = 0) : bodyPrefixes isa body (σA s) =
      [σA s, σ1 s, σ2 s, σ2 s, σ3 s, σ4o s, σ5o s, σ6o s, σ7o s, σ8o s, σ9 s, σ9 s, σ10 s, σ10 s,
        σ12 s] := by
  rw [body, bodyPrefixes_instr (exec_cpu hp.facts.toArm), bodyPrefixes_instr exec_cmp,
    bodyPrefixes_skip (eval_cpu_false hi), bodyPrefixes_instr exec_shl_r8,
    bodyPrefixes_instr exec_off1, bodyPrefixes_instr exec_off2, bodyPrefixes_instr exec_off3,
    bodyPrefixes_instr exec_off4, bodyPrefixes_instr (exec_w hp hi), bodyPrefixes_instr exec_test1,
    bodyPrefixes_skip (eval_even hm), bodyPrefixes_instr exec_test0,
    bodyPrefixes_skip (eval_nonzero h0), bodyPrefixes_instr (exec_link hp hi h0 hm),
    bodyPrefixes_last (exec_commit hp hi)]

theorem end_P (hp : Pre s) (hi : cx.InRange s) (hm : ¬ (wvo s).toNat % 2 = 1)
    (h0 : ¬ (wvo s).toNat = 0) : bodyEnd isa body (σA s) = some 15 := by
  rw [body, bodyEnd_instr (exec_cpu hp.facts.toArm), bodyEnd_instr exec_cmp,
    bodyEnd_skip (eval_cpu_false hi), bodyEnd_instr exec_shl_r8, bodyEnd_instr exec_off1,
    bodyEnd_instr exec_off2, bodyEnd_instr exec_off3, bodyEnd_instr exec_off4,
    bodyEnd_instr (exec_w hp hi), bodyEnd_instr exec_test1, bodyEnd_skip (eval_even hm),
    bodyEnd_instr exec_test0, bodyEnd_skip (eval_nonzero h0), bodyEnd_instr (exec_link hp hi h0 hm),
    bodyEnd_instr (exec_commit hp hi), bodyEnd_nil']
  rfl

theorem ev_link (hp : Pre s) (hi : cx.InRange s) (h0 : ¬ (wvo s).toNat = 0)
    (hm : ¬ (wvo s).toNat % 2 = 1) :
    Instr.events (.mov .rcx (.mem { base := .rax })) (σ10 s) (σ12 s) =
      [⟨wvo s, 8, false, (lk s).toNat⟩] := by
  obtain ⟨-, hdj⟩ := top_facts hp hi h0 hm
  simp only [Instr.events, Access.ea_base, σ10_rax, toNat_read8,
    show (σ10 s).mem = m1 s from rfl, m1]
  rw [readW64_writeW_disj hdj]; rfl

theorem ev_commit :
    Instr.events (.store slabWord .rcx) (σ12 s) (σ13 s) = [⟨wao s, 8, true, (lk s).toNat⟩] := by
  simp only [Instr.events, slabWord_ea_o _ σ12_r9 σ12_r8, toNat_read8]
  rw [show (σ13 s).mem = mEnd s from rfl, mEnd, Mem.readW_writeW_self64]

theorem events_P (hp : Pre s) (hi : cx.InRange s) (hm : ¬ (wvo s).toNat % 2 = 1)
    (h0 : ¬ (wvo s).toNat = 0) : bodyEvents isa body (σA s) =
      [⟨area s + 4, 4, false, (cpu32 s).toNat⟩, ⟨wao s, 8, false, (wvo s).toNat⟩,
        ⟨wvo s, 8, false, (lk s).toNat⟩, ⟨wao s, 8, true, (lk s).toNat⟩] := by
  rw [body, bodyEvents_instr (exec_cpu hp.facts.toArm), bodyEvents_instr exec_cmp,
    bodyEvents_skip (eval_cpu_false hi), bodyEvents_instr exec_shl_r8, bodyEvents_instr exec_off1,
    bodyEvents_instr exec_off2, bodyEvents_instr exec_off3, bodyEvents_instr exec_off4,
    bodyEvents_instr (exec_w hp hi), bodyEvents_instr exec_test1, bodyEvents_skip (eval_even hm),
    bodyEvents_instr exec_test0, bodyEvents_skip (eval_nonzero h0),
    bodyEvents_instr (exec_link hp hi h0 hm), bodyEvents_instr (exec_commit hp hi), bodyEvents_nil']
  simp only [ev_cpuLoad, ev_word (word_facts hp hi).2, ev_link hp hi h0 hm, ev_commit]; rfl

end Cases

/-! ## What each state keeps -/

section Mids

variable {s : State}

theorem mσ2 : Mid scratchT s (m1 s) (σ2 s) :=
  mid_σ2 (mid_σ1 (by decide) (mid_σA (by decide) (by decide)))
theorem mσ9 : Mid scratchT s (m1 s) (σ9 s) := mσ8o.arithFlags _ _ _
theorem mσ10 : Mid scratchT s (m1 s) (σ10 s) := mσ9.arithFlags _ _ _
theorem mσ12 : Mid scratchT s (m1 s) (σ12 s) := mσ10.setReg (by decide) _
theorem mσ13 : Mid scratchT s (mEnd s) (σ13 s) := mσ12.withMem _
theorem mσC : Mid scratchT s (m1 s) (σC s) := mσ2.setReg32 (by decide) _

end Mids

/-! ## The contract -/

section Contract

variable {s : State}

theorem hv : cx.A.args ((cx.sig.words cx.A.ptrBits).map (·.bits cx.A.ptrBits)) =
    some (fun s => [s.gpr .rdi]) := rfl

theorem facts_of_pre (hs : (stackPopContract abi).pre s) : SFacts cx s := by
  have := (pre_iff cx (s := s) hv rfl rfl).mp hs
  exact sfacts_of_world hX this.2.1 this.2.2

theorem abi_m1 (hp : Pre s) {p : State} (hm : Mid scratchT s (m1 s) p) : abiPreserved s p :=
  abiPreserved_of_mid frame_m1 (by simpa using hp.facts.ret_cs) (by decide) hm

theorem unchanged_cs : Unchanged [cx.csField s] (cx.mem s) (m1 s) := by
  rw [csField_of (c := cx) hX, mem_of (c := cx) hX]; exact frame_m1

theorem σ12_rax : (σ12 s).gpr .rax = wvo s := by
  rw [σ12, RegUpd.gpr_setReg_of_ne _ _ (by decide), σ10_rax]

/-- What the pop leaves: the head is the top magazine's link, read from the
head read once, and the stack is the rest. -/
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

theorem post_eq (s' : State) : (stackPopContract abi).post s s' ↔
    NoCpu cx s s' ∨ (cx.InRange s ∧ s'.gpr .rax = cx.w s ∧
      ((((cx.w s).toNat = 0 ∨ (cx.w s).toNat % 2 = 1) ∧
          Unchanged [cx.csField s] (cx.mem s) s'.mem) ∨
        ((cx.w s).toNat ≠ 0 ∧ (cx.w s).toNat % 2 = 0 ∧
          Unchanged [cx.csField s, ⟨cx.word s, 8⟩] (cx.mem s) s'.mem ∧
          s'.mem.readW (cx.word s) 64 = (cx.mem s).readW (cx.w s) 64 ∧
          ∀ l, Chain (cx.mem s) (cx.w s) l → Chain s'.mem ((cx.mem s).readW (cx.w s) 64) l.tail))) :=
  post_iff cx hv s s'

theorem correct (hs : (stackPopContract abi).pre s) :
    (∃ s', Exec isa stackPop s s' ∧ target.abiPreserved s s' ∧ (stackPopContract abi).post s s') ∨
    (∃ t s', ExecExit isa stackPop s t s' ∧ target.abiPreserved s s' ∧
      (stackPopContract abi).exit t s s') := by
  have h := facts_of_pre hs
  have hp := pre_of h
  refine .inl ?_
  rw [code_eq]
  by_cases hi : cx.InRange s
  · by_cases hm : (wvo s).toNat % 2 = 1
    · refine ⟨σ9 s, .rseq (execBlock_arm h.toArm) (runBody_M hp hi hm), abi_m1 hp mσ9,
        (post_eq _).mpr (.inr ⟨hi, by rw [σ9_rax, hp.w], .inl ⟨.inr (by rw [hp.w]; exact hm), ?_⟩⟩)⟩
      show Unchanged _ _ (σ9 s).mem; rw [mσ9.mem]; exact unchanged_cs
    · by_cases h0 : (wvo s).toNat = 0
      · refine ⟨σ10 s, .rseq (execBlock_arm h.toArm) (runBody_Z hp hi hm h0), abi_m1 hp mσ10,
          (post_eq _).mpr (.inr ⟨hi, by rw [σ10_rax, hp.w], .inl ⟨.inl (by rw [hp.w]; exact h0), ?_⟩⟩)⟩
        show Unchanged _ _ (σ10 s).mem; rw [mσ10.mem]; exact unchanged_cs
      · have hw := hp.facts.ret_word hi
        rw [← hp.word] at hw
        obtain ⟨hu, hr, hch⟩ := popped hp hi h0
        refine ⟨σ13 s, .rseq (execBlock_arm h.toArm) (runBody_P hp hi hm h0),
          abiPreserved_of_mid (frame_two s.mem _ _ _ _) (by simpa using ⟨hp.facts.ret_cs, hw⟩)
            (by decide) mσ13,
          (post_eq _).mpr (.inr ⟨hi, ?_, .inr ⟨by rw [hp.w]; exact h0, by rw [hp.w]; omega,
            hu, hr, hch⟩⟩)⟩
        show (σ12 s).gpr .rax = _
        rw [σ12_rax, hp.w]
  · exact ⟨σC s, .rseq (execBlock_arm h.toArm) (runBody_C hp hi), abi_m1 hp mσC,
      (post_eq _).mpr (.inl ⟨hi, by simp [σC, State.setReg32]; rfl,
        by show Unchanged _ _ (σC s).mem; rw [mσC.mem]; exact unchanged_cs⟩)⟩

/-! ### Where it may be aborted -/

theorem abort_eq (p : State) : (stackPopContract abi).abort s p ↔
    [p.gpr .rdi] = [s.gpr .rdi] ∧ (Unchanged [cx.csField s] (cx.mem s) p.mem ∧ cx.inv p) :=
  abort_iff cx hv s p

theorem abortOk (hp : Pre s) {p : State} (hm : Mid scratchT s (m1 s) p) :
    armed p ∧ target.abiPreserved s p ∧ (stackPopContract abi).abort s p ∧
      ∀ p', target.rely p p' → (stackPopContract abi).env p p' → (stackPopContract abi).pre p' := by
  obtain ⟨hip, hre⟩ := retry hX hv (post := fun s s' => _) (fun _ => rfl) (fun _ => rfl) (by decide)
    hp.facts.pre hm (by decide) (by decide) frame_m1 (harmless_cs1 hX hp.facts.pre)
  refine ⟨armed_of_mid hp.facts.toArm (xs := []) frame_m1 m1_cs (by simp) hm, abi_m1 hp hm, ?_, hre⟩
  exact (abort_eq p).mpr ⟨by rw [hm.gpr .rdi (by decide)], by rw [hm.mem]; exact unchanged_cs, hip⟩

/-! ### Its accesses -/

theorem addr8 : s.fsBase + o s + BitVec.ofNat 64 RSEQ_CS = area s + 8 := rfl
theorem addr4 : s.fsBase + o s + BitVec.ofNat 64 RSEQ_CPU_ID = area s + 4 := rfl
theorem rsName : RSEQ_OFFSET = RS := rfl

theorem ofNat_toNat64 (v : BitVec 64) : BitVec.ofNat 64 v.toNat = v := by
  rw [BitVec.ofNat_toNat, BitVec.setWidth_eq]

theorem wordAddr_eq (hp : Pre s) : wordAddr (s.syms SLABS) (cpu32 s).toNat (cx.cls s) = wao s := by
  rw [hp.word]; rfl

/-- The accesses: the link is read at the head the routine read once, and
the commit, the last access, stores what that read returned. -/
theorem trace_ok (hs : (stackPopContract abi).pre s) :
    (stackPopContract abi).trace s (Code.events isa stackPop s) := by
  have h := facts_of_pre hs
  have hp := pre_of h
  show cx.StackPopTrace s (Code.events isa (.rseq arm body) s)
  simp only [Code.events, execBlock_arm h.toArm, events_arm h.toArm]
  refine ⟨(o s).toNat, s.csDesc.toNat, (cpu32 s).toNat, (wvo s).toNat, (lk s).toNat, ?_⟩
  simp only [Spec.Slab.Ctx.headTrace, Spec.Slab.Ctx.rd, Spec.Slab.Ctx.wr, tp_of (c := cx) hX,
    sym_of (c := cx) hX, ofNat_toNat64, addr8, addr4, rsName]
  by_cases hi : cx.InRange s
  · have hi' : (cpu32 s).toNat < SLAB_COUNT := (inRange_iff.mp hi)
    rw [wordAddr_eq hp]
    by_cases hm : (wvo s).toNat % 2 = 1
    · rw [events_M hp hi hm]
      have : ¬ (wvo s).toNat % 2 = 0 := by omega
      simp [hi', this]
    · by_cases h0 : (wvo s).toNat = 0
      · rw [events_Z hp hi hm h0]; simp [hi', h0]
      · rw [events_P hp hi hm h0]
        have : (wvo s).toNat % 2 = 0 := by omega
        simp [hi', h0, this]
  · have hi' : ¬ (cpu32 s).toNat < SLAB_COUNT := fun h' => hi (inRange_iff.mpr h')
    rw [events_C hp hi]; simp [hi']

/-- The link is read inside the sequence: the state holding it, before the
commit, is one where the sequence may still be aborted. -/
theorem link_in_sequence (hs : (stackPopContract abi).pre s) (hi : cx.InRange s)
    (hm : ¬ (wvo s).toNat % 2 = 1) (h0 : ¬ (wvo s).toNat = 0) :
    σ12 s ∈ stackPop.abortStates isa s ∧ (σ12 s).gpr .rcx = s.mem.readW (wvo s) 64 := by
  have h := facts_of_pre hs
  have hp := pre_of h
  rw [code_eq]
  simp only [Code.abortStates, execBlock_arm h.toArm, prefixes_P hp hi hm h0]
  exact ⟨by simp, σ12_rcx⟩

theorem rseqOk (hs : (stackPopContract abi).pre s) :
    RseqOk target stackPop (stackPopContract abi) s := by
  have h := facts_of_pre hs
  have hp := pre_of h
  have ht := trace_ok hs
  rw [code_eq] at ht ⊢
  unfold RseqOk
  refine ⟨ht, ⟨_, _, rfl, execBlock_dropLast_arm h.toArm, arms_arm⟩, ?_⟩
  have mA : Mid scratchT s (m1 s) (σA s) := mid_σA (by decide) (by decide)
  have m1' : Mid scratchT s (m1 s) (σ1 s) := mid_σ1 (by decide) mA
  have m3 : Mid scratchT s (m1 s) (σ3 s) := mid_σ3 (by decide) mσ2
  simp only [Code.abortStates, execBlock_arm h.toArm]
  by_cases hi : cx.InRange s
  · by_cases hm : (wvo s).toNat % 2 = 1
    · rw [prefixes_M hp hi hm]
      simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
      exact ⟨abortOk hp mA, abortOk hp m1', abortOk hp mσ2, abortOk hp mσ2, abortOk hp m3,
        abortOk hp mσ4o, abortOk hp mσ5o, abortOk hp mσ6o, abortOk hp mσ7o, abortOk hp mσ8o,
        abortOk hp mσ9⟩
    · by_cases h0 : (wvo s).toNat = 0
      · rw [prefixes_Z hp hi hm h0]
        simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
        exact ⟨abortOk hp mA, abortOk hp m1', abortOk hp mσ2, abortOk hp mσ2, abortOk hp m3,
          abortOk hp mσ4o, abortOk hp mσ5o, abortOk hp mσ6o, abortOk hp mσ7o, abortOk hp mσ8o,
          abortOk hp mσ9, abortOk hp mσ9, abortOk hp mσ10⟩
      · rw [prefixes_P hp hi hm h0]
        simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
        exact ⟨abortOk hp mA, abortOk hp m1', abortOk hp mσ2, abortOk hp mσ2, abortOk hp m3,
          abortOk hp mσ4o, abortOk hp mσ5o, abortOk hp mσ6o, abortOk hp mσ7o, abortOk hp mσ8o,
          abortOk hp mσ9, abortOk hp mσ9, abortOk hp mσ10, abortOk hp mσ10, abortOk hp mσ12⟩
  · rw [prefixes_C hp hi]
    simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
    exact ⟨abortOk hp mA, abortOk hp m1', abortOk hp mσ2⟩

end Contract

/-! ## The witnesses -/

/-- A thread as the store-if-equal's witnesses (`Cas.wit`), offset 8, the
head `head` in slab 0; a magazine at `0x4000` whose link slot holds 0. -/
def wit (cpu head : BitVec 64) (rd wr : List Region) : State where
  gpr r := if r = .rsp then 0x7000000 else if r = .rdi then 8 else 0
  cf := none
  zf := none
  sf := none
  of := none
  mem := mkMem [(0x2000, 0x100), (0x10104, cpu), (0x1000008, head)]
  rd := rd
  wr := wr
  syms n := if n = "__rseq_offset" then 0x2000 else if n = "SLABS" then 0x1000000 else 0
  fsBase := 0x10000
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
    (stackPopContract abi).pre s := by
  refine (pre_iff cx (s := s) hv rfl rfl).mpr ⟨trivial, world_of_check hX hrd hwr hnw hp hres,
    rfl, rfl, trivial, layout_of_check hX hl, fun i hi => ?_⟩
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

theorem pre_w0 : (stackPopContract abi).pre w0 :=
  pre_w _ ⟨_, _, _, _, rfl⟩ (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (by rw [show w0.mem.readW _ 64 = 0 by decide +kernel]; exact stackOk_zero)
theorem pre_w1 : (stackPopContract abi).pre w1 :=
  pre_w _ ⟨_, _, _, _, rfl⟩ (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (.inl (by decide +kernel))
theorem pre_w2 : (stackPopContract abi).pre w2 :=
  pre_w _ ⟨_, _, _, _, rfl⟩ (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (by rw [show w2.mem.readW _ 64 = 0 by decide +kernel]; exact stackOk_zero)
theorem pre_w3 : (stackPopContract abi).pre w3 :=
  pre_w _ ⟨_, _, _, _, rfl⟩ (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (by
      rw [show w3.mem.readW (wordAddr 0x1000000 0 1) 64 = 0x4000 by decide +kernel]
      refine .inr ⟨[0x4000], .cons (by decide) (by decide) ?_, ?_⟩
      · rw [show w3.mem.readW 0x4000 64 = 0 by decide +kernel]; exact .nil
      · intro x hx
        rw [List.mem_singleton.mp hx]
        exact apart_of_check hX (by decide +kernel))

theorem outcomes_eq : stackPop.outcomes = [(0, true), (1, true), (2, true), (3, true)] := rfl

theorem visits_w0 : Visits isa stackPop w0 0 true := by
  have hp := pre_of (facts_of_pre pre_w0)
  have hi : ¬ cx.InRange w0 := by decide +kernel
  exact Visits.rseq (execBlock_arm hp.facts.toArm) (end_C hp hi)

theorem visits_w1 : Visits isa stackPop w1 1 true := by
  have hp := pre_of (facts_of_pre pre_w1)
  have hi : cx.InRange w1 := by decide +kernel
  have hm : (wvo w1).toNat % 2 = 1 := by decide +kernel
  exact Visits.rseq (execBlock_arm hp.facts.toArm) (end_M hp hi hm)

theorem visits_w2 : Visits isa stackPop w2 2 true := by
  have hp := pre_of (facts_of_pre pre_w2)
  have hi : cx.InRange w2 := by decide +kernel
  have hm : ¬ (wvo w2).toNat % 2 = 1 := by decide +kernel
  have h0 : (wvo w2).toNat = 0 := by decide +kernel
  exact Visits.rseq (execBlock_arm hp.facts.toArm) (end_Z hp hi hm h0)

theorem visits_w3 : Visits isa stackPop w3 3 true := by
  have hp := pre_of (facts_of_pre pre_w3)
  have hi : cx.InRange w3 := by decide +kernel
  have hm : ¬ (wvo w3).toNat % 2 = 1 := by decide +kernel
  have h0 : ¬ (wvo w3).toNat = 0 := by decide +kernel
  exact Visits.rseq (execBlock_arm hp.facts.toArm) (end_P hp hi hm h0)

def witnesses : List State := [w0, w1, w2, w3]

theorem witnessOk : (∃ w ∈ witnesses, (stackPopContract abi).pre w) ∧
    ∀ o ∈ stackPop.outcomes, ∃ w ∈ witnesses, (stackPopContract abi).pre w ∧
      Visits isa stackPop w o.1 o.2 := by
  refine ⟨⟨w0, by simp [witnesses], pre_w0⟩, fun o ho => ?_⟩
  rw [outcomes_eq] at ho
  simp only [List.mem_cons, List.not_mem_nil, or_false] at ho
  rcases ho with rfl | rfl | rfl | rfl
  · exact ⟨w0, by simp [witnesses], pre_w0, visits_w0⟩
  · exact ⟨w1, by simp [witnesses], pre_w1, visits_w1⟩
  · exact ⟨w2, by simp [witnesses], pre_w2, visits_w2⟩
  · exact ⟨w3, by simp [witnesses], pre_w3, visits_w3⟩

theorem verified : Verified target stackPop (stackPopContract abi) :=
  ⟨fun _ hs => correct hs, fun _ hs => rseqOk hs, ⟨_, pre_w0⟩, fun o ho => by
    obtain ⟨w, -, hw, hv⟩ := witnessOk.2 o ho
    exact ⟨w, hw, hv⟩⟩

end VG.Proof.Slab.X86_64.StackPop
