module

public import Asmalloc.Proof.Slab.X86_64.Slow
public import Asmalloc.Proof.Slab.X86_64.Witness

/-!
# The store-if-equal on x86-64

`slab_cas` (`Impl.Slab.X86_64.cas`) against `Spec.Slab.casContract`: its
three ways out (no slab; the word is not `expected`; the store), and in
each its run, where it may be aborted, and its memory accesses.
-/

@[expose] public section

namespace VG.Proof.Slab.X86_64.Cas

open VG.X86_64 VG.Spec.Slab VG.Impl.Slab.X86_64 VG.Proof.Slab.X86_64

abbrev cx : Spec.Slab.Ctx isa := ⟨abi, .cas, casSig⟩

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
  .instr (.alu .cmp .rax (.reg .rsi)),
  .exit .ne [] none,
  .instr (.store slabWord .rdx)]

theorem code_eq : cas = .rseq arm body := rfl

/-! ## The states -/

def σ9 (s : State) : State :=
  arithFlags (σ8o s) (wvo s - s.gpr .rsi) (decide ((wvo s).toNat < (s.gpr .rsi).toNat))
    (subOverflow (wvo s) (s.gpr .rsi) (wvo s - s.gpr .rsi))
def mEnd (s : State) : Mem := (m1 s).writeW (wao s) (s.gpr .rdx)
def σ10 (s : State) : State := { σ9 s with mem := mEnd s }
def σC (s : State) : State := (σ2 s).setReg32 .rax 2

/-! ## What the precondition gives -/

structure Pre (s : State) : Prop where
  facts : SFacts cx s
  word : wao s = cx.word s
  w : cx.w s = wvo s

theorem pre_of {s : State} (h : SFacts cx s) : Pre s := by
  have hw : wao s = cx.word s := wao_eq hX rfl
  exact ⟨h, hw, by simp only [Spec.Slab.Ctx.w, mem_of (c := cx) hX, wvo, hw]⟩

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

theorem σ8o_rsi : (σ8o s).gpr .rsi = s.gpr .rsi := gpr_σ8o _ (by decide)

theorem exec_cmp' : exec (.alu .cmp .rax (.reg .rsi)) (σ8o s) = some (σ9 s) := by
  simp only [exec, execAlu, readSrc, Option.bind_some, σ8o_rax, σ8o_rsi]; rfl

theorem eval_ne_true (he : wvo s ≠ s.gpr .rsi) : eval .ne (σ9 s) = some true := by
  simp only [eval, σ9, RegUpd.zf_arithFlags, Option.map_some, Option.some.injEq]
  simp only [Bool.not_eq_eq_eq_not, Bool.not_true, beq_eq_false_iff_ne, ne_eq]
  intro h; exact he (by bv_omega)

theorem eval_ne_false (he : wvo s = s.gpr .rsi) : eval .ne (σ9 s) = some false := by
  simp [eval, σ9, RegUpd.zf_arithFlags, he]

theorem σ9_r9 : (σ9 s).gpr .r9 = s.syms "SLABS" := by rw [σ9, RegUpd.gpr_arithFlags, σ8o_r9]
theorem σ9_r8 : (σ9 s).gpr .r8 = r8o s := by rw [σ9, RegUpd.gpr_arithFlags, σ8o_r8]
theorem σ9_rdx : (σ9 s).gpr .rdx = s.gpr .rdx := by
  rw [σ9, RegUpd.gpr_arithFlags, gpr_σ8o _ (by decide)]

theorem exec_commit (hp : Pre s) (hi : cx.InRange s) :
    exec (.store slabWord .rdx) (σ9 s) = some (σ10 s) := by
  have hin : InRegions (σ9 s).wr ((σ9 s).ea slabWord) 8 := by
    rw [slabWord_ea_o _ σ9_r9 σ9_r8, show (σ9 s).wr = s.wr from rfl, hp.word]
    exact inWr (r := ⟨cx.word s, 8⟩) (hp.facts.word hi)
  rw [exec_store _ hin, slabWord_ea_o _ σ9_r9 σ9_r8, σ9_rdx]
  rfl

end Steps

/-! ## The three ways out -/

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

theorem runBody_M (hp : Pre s) (hi : cx.InRange s) (he : wvo s ≠ s.gpr .rsi) :
    runBody isa body (σA s) = some (.done (σ9 s)) := by
  rw [body, runBody_instr (exec_cpu hp.facts.toArm), runBody_instr exec_cmp,
    runBody_skip (eval_cpu_false hi), runBody_instr exec_shl_r8, runBody_instr exec_off1,
    runBody_instr exec_off2, runBody_instr exec_off3, runBody_instr exec_off4,
    runBody_instr (exec_w hp hi), runBody_instr exec_cmp', runBody_take (eval_ne_true he) rfl]

theorem prefixes_M (hp : Pre s) (hi : cx.InRange s) (he : wvo s ≠ s.gpr .rsi) :
    bodyPrefixes isa body (σA s) =
      [σA s, σ1 s, σ2 s, σ2 s, σ3 s, σ4o s, σ5o s, σ6o s, σ7o s, σ8o s, σ9 s] := by
  rw [body, bodyPrefixes_instr (exec_cpu hp.facts.toArm), bodyPrefixes_instr exec_cmp,
    bodyPrefixes_skip (eval_cpu_false hi), bodyPrefixes_instr exec_shl_r8,
    bodyPrefixes_instr exec_off1, bodyPrefixes_instr exec_off2, bodyPrefixes_instr exec_off3,
    bodyPrefixes_instr exec_off4, bodyPrefixes_instr (exec_w hp hi), bodyPrefixes_instr exec_cmp',
    bodyPrefixes_take (eval_ne_true he)]

theorem end_M (hp : Pre s) (hi : cx.InRange s) (he : wvo s ≠ s.gpr .rsi) :
    bodyEnd isa body (σA s) = some 10 := by
  rw [body, bodyEnd_instr (exec_cpu hp.facts.toArm), bodyEnd_instr exec_cmp,
    bodyEnd_skip (eval_cpu_false hi), bodyEnd_instr exec_shl_r8, bodyEnd_instr exec_off1,
    bodyEnd_instr exec_off2, bodyEnd_instr exec_off3, bodyEnd_instr exec_off4,
    bodyEnd_instr (exec_w hp hi), bodyEnd_instr exec_cmp', bodyEnd_take (eval_ne_true he)]
  rfl

theorem events_M (hp : Pre s) (hi : cx.InRange s) (he : wvo s ≠ s.gpr .rsi) :
    bodyEvents isa body (σA s) =
      [⟨area s + 4, 4, false, (cpu32 s).toNat⟩, ⟨wao s, 8, false, (wvo s).toNat⟩] := by
  rw [body, bodyEvents_instr (exec_cpu hp.facts.toArm), bodyEvents_instr exec_cmp,
    bodyEvents_skip (eval_cpu_false hi), bodyEvents_instr exec_shl_r8, bodyEvents_instr exec_off1,
    bodyEvents_instr exec_off2, bodyEvents_instr exec_off3, bodyEvents_instr exec_off4,
    bodyEvents_instr (exec_w hp hi), bodyEvents_instr exec_cmp', bodyEvents_take (eval_ne_true he)]
  simp only [isa, ev_cpuLoad, ev_word (word_facts hp hi).2]; rfl

theorem runBody_S (hp : Pre s) (hi : cx.InRange s) (he : wvo s = s.gpr .rsi) :
    runBody isa body (σA s) = some (.done (σ10 s)) := by
  rw [body, runBody_instr (exec_cpu hp.facts.toArm), runBody_instr exec_cmp,
    runBody_skip (eval_cpu_false hi), runBody_instr exec_shl_r8, runBody_instr exec_off1,
    runBody_instr exec_off2, runBody_instr exec_off3, runBody_instr exec_off4,
    runBody_instr (exec_w hp hi), runBody_instr exec_cmp', runBody_skip (eval_ne_false he),
    runBody_instr (exec_commit hp hi), runBody_nil']

theorem prefixes_S (hp : Pre s) (hi : cx.InRange s) (he : wvo s = s.gpr .rsi) :
    bodyPrefixes isa body (σA s) =
      [σA s, σ1 s, σ2 s, σ2 s, σ3 s, σ4o s, σ5o s, σ6o s, σ7o s, σ8o s, σ9 s, σ9 s] := by
  rw [body, bodyPrefixes_instr (exec_cpu hp.facts.toArm), bodyPrefixes_instr exec_cmp,
    bodyPrefixes_skip (eval_cpu_false hi), bodyPrefixes_instr exec_shl_r8,
    bodyPrefixes_instr exec_off1, bodyPrefixes_instr exec_off2, bodyPrefixes_instr exec_off3,
    bodyPrefixes_instr exec_off4, bodyPrefixes_instr (exec_w hp hi), bodyPrefixes_instr exec_cmp',
    bodyPrefixes_skip (eval_ne_false he), bodyPrefixes_last (exec_commit hp hi)]

theorem end_S (hp : Pre s) (hi : cx.InRange s) (he : wvo s = s.gpr .rsi) :
    bodyEnd isa body (σA s) = some 12 := by
  rw [body, bodyEnd_instr (exec_cpu hp.facts.toArm), bodyEnd_instr exec_cmp,
    bodyEnd_skip (eval_cpu_false hi), bodyEnd_instr exec_shl_r8, bodyEnd_instr exec_off1,
    bodyEnd_instr exec_off2, bodyEnd_instr exec_off3, bodyEnd_instr exec_off4,
    bodyEnd_instr (exec_w hp hi), bodyEnd_instr exec_cmp', bodyEnd_skip (eval_ne_false he),
    bodyEnd_instr (exec_commit hp hi), bodyEnd_nil']
  rfl

theorem ev_commit :
    Instr.events (.store slabWord .rdx) (σ9 s) (σ10 s) = [⟨wao s, 8, true, (s.gpr .rdx).toNat⟩] := by
  simp only [Instr.events, slabWord_ea_o _ σ9_r9 σ9_r8, toNat_read8]
  rw [show (σ10 s).mem = mEnd s from rfl, mEnd, Mem.readW_writeW_self64]

theorem events_S (hp : Pre s) (hi : cx.InRange s) (he : wvo s = s.gpr .rsi) :
    bodyEvents isa body (σA s) =
      [⟨area s + 4, 4, false, (cpu32 s).toNat⟩, ⟨wao s, 8, false, (wvo s).toNat⟩,
        ⟨wao s, 8, true, (s.gpr .rdx).toNat⟩] := by
  rw [body, bodyEvents_instr (exec_cpu hp.facts.toArm), bodyEvents_instr exec_cmp,
    bodyEvents_skip (eval_cpu_false hi), bodyEvents_instr exec_shl_r8, bodyEvents_instr exec_off1,
    bodyEvents_instr exec_off2, bodyEvents_instr exec_off3, bodyEvents_instr exec_off4,
    bodyEvents_instr (exec_w hp hi), bodyEvents_instr exec_cmp', bodyEvents_skip (eval_ne_false he),
    bodyEvents_instr (exec_commit hp hi), bodyEvents_nil']
  simp only [ev_cpuLoad, ev_word (word_facts hp hi).2, ev_commit]; rfl

end Cases

/-! ## The contract -/

section Contract

variable {s : State}

theorem hv : cx.A.args ((cx.sig.words cx.A.ptrBits).map (·.bits cx.A.ptrBits)) =
    some (fun s => [s.gpr .rdi, s.gpr .rsi, s.gpr .rdx]) := rfl

theorem facts_of_pre (hs : (casContract abi).pre s) : SFacts cx s := by
  have := (pre_iff cx (s := s) hv rfl rfl).mp hs
  exact sfacts_of_world hX this.2.1 this.2.2

theorem abi_m1 (hp : Pre s) {p : State} (hm : Mid scratchT s (m1 s) p) : abiPreserved s p :=
  abiPreserved_of_mid frame_m1 (by simpa using hp.facts.ret_cs) (by decide) hm

theorem unchanged_cs : Unchanged [cx.csField s] (cx.mem s) (m1 s) := by
  rw [csField_of (c := cx) hX, mem_of (c := cx) hX]; exact frame_m1

theorem mσ9 : Mid scratchT s (m1 s) (σ9 s) := mσ8o.arithFlags _ _ _
theorem mσ10 : Mid scratchT s (mEnd s) (σ10 s) := mσ9.withMem _
theorem mσ2 : Mid scratchT s (m1 s) (σ2 s) :=
  mid_σ2 (mid_σ1 (by decide) (mid_σA (by decide) (by decide)))
theorem mσC : Mid scratchT s (m1 s) (σC s) := mσ2.setReg32 (by decide) _

theorem post_eq (s' : State) : (casContract abi).post s s' ↔
    NoCpu cx s s' ∨ (cx.InRange s ∧ s'.gpr .rax = cx.w s ∧
      ((cx.w s = s.gpr .rsi ∧ Unchanged [cx.csField s, ⟨cx.word s, 8⟩] (cx.mem s) s'.mem ∧
          s'.mem.readW (cx.word s) 64 = s.gpr .rdx) ∨
        (cx.w s ≠ s.gpr .rsi ∧ Unchanged [cx.csField s] (cx.mem s) s'.mem))) :=
  post_iff cx hv s s'

theorem correct (hs : (casContract abi).pre s) :
    (∃ s', Exec isa cas s s' ∧ target.abiPreserved s s' ∧ (casContract abi).post s s') ∨
    (∃ t s', ExecExit isa cas s t s' ∧ target.abiPreserved s s' ∧
      (casContract abi).exit t s s') := by
  have h := facts_of_pre hs
  have hp := pre_of h
  refine .inl ?_
  rw [code_eq]
  by_cases hi : cx.InRange s
  · by_cases he : wvo s = s.gpr .rsi
    · have hw := hp.facts.ret_word hi
      rw [← hp.word] at hw
      refine ⟨σ10 s, .rseq (execBlock_arm h.toArm) (runBody_S hp hi he),
        abiPreserved_of_mid (frame_two s.mem _ _ _ _) (by simpa using ⟨hp.facts.ret_cs, hw⟩)
          (by decide) mσ10, (post_eq _).mpr (.inr ⟨hi, ?_, .inl ⟨by rw [hp.w]; exact he, ?_, ?_⟩⟩)⟩
      · show (σ9 s).gpr .rax = _
        rw [σ9, RegUpd.gpr_arithFlags, σ8o_rax, hp.w]
      · rw [csField_of (c := cx) hX, mem_of (c := cx) hX, ← hp.word]
        exact frame_two s.mem _ _ _ _
      · show (mEnd s).readW (cx.word s) 64 = _
        rw [mEnd, ← hp.word, Mem.readW_writeW_self64]
    · refine ⟨σ9 s, .rseq (execBlock_arm h.toArm) (runBody_M hp hi he), abi_m1 hp mσ9,
        (post_eq _).mpr (.inr ⟨hi, ?_, .inr ⟨by rw [hp.w]; exact he,
          by show Unchanged _ _ (σ9 s).mem; rw [mσ9.mem]; exact unchanged_cs⟩⟩)⟩
      rw [σ9, RegUpd.gpr_arithFlags, σ8o_rax, hp.w]
  · exact ⟨σC s, .rseq (execBlock_arm h.toArm) (runBody_C hp hi), abi_m1 hp mσC,
      (post_eq _).mpr (.inl ⟨hi, by simp [σC, State.setReg32]; rfl,
        by show Unchanged _ _ (σC s).mem; rw [mσC.mem]; exact unchanged_cs⟩)⟩

/-! ### Where it may be aborted -/

theorem abort_eq (p : State) : (casContract abi).abort s p ↔
    [p.gpr .rdi, p.gpr .rsi, p.gpr .rdx] = [s.gpr .rdi, s.gpr .rsi, s.gpr .rdx] ∧
      (Unchanged [cx.csField s] (cx.mem s) p.mem ∧ cx.inv p) :=
  abort_iff cx hv s p

theorem abortOk (hp : Pre s) {p : State} (hm : Mid scratchT s (m1 s) p) :
    armed p ∧ target.abiPreserved s p ∧ (casContract abi).abort s p ∧
      ∀ p', target.rely p p' → (casContract abi).env p p' → (casContract abi).pre p' := by
  obtain ⟨hip, hre⟩ := retry hX hv (post := fun s s' => _) (fun _ => rfl) (fun _ => rfl) (by decide)
    hp.facts.pre hm (by decide) (by decide) frame_m1 (harmless_cs1 hX hp.facts.pre)
  refine ⟨armed_of_mid hp.facts.toArm (xs := []) frame_m1 m1_cs (by simp) hm, abi_m1 hp hm, ?_, hre⟩
  refine (abort_eq p).mpr ⟨by rw [hm.gpr .rdi (by decide), hm.gpr .rsi (by decide),
    hm.gpr .rdx (by decide)], by rw [hm.mem]; exact unchanged_cs, hip⟩

/-! ### Its accesses -/

theorem addr8 : s.fsBase + o s + BitVec.ofNat 64 RSEQ_CS = area s + 8 := rfl
theorem addr4 : s.fsBase + o s + BitVec.ofNat 64 RSEQ_CPU_ID = area s + 4 := rfl
theorem rsName : RSEQ_OFFSET = RS := rfl

theorem ofNat_toNat64 (v : BitVec 64) : BitVec.ofNat 64 v.toNat = v := by
  rw [BitVec.ofNat_toNat, BitVec.setWidth_eq]

theorem wordAddr_eq (hp : Pre s) : wordAddr (s.syms SLABS) (cpu32 s).toNat (cx.cls s) = wao s := by
  rw [hp.word]; rfl

theorem trace_ok (hs : (casContract abi).pre s) :
    (casContract abi).trace s (Code.events isa cas s) := by
  have h := facts_of_pre hs
  have hp := pre_of h
  show cx.CasTrace s (cx.arg s 1) (cx.arg s 2) (Code.events isa (.rseq arm body) s)
  simp only [Code.events, execBlock_arm h.toArm, events_arm h.toArm]
  refine ⟨(o s).toNat, s.csDesc.toNat, (cpu32 s).toNat, (wvo s).toNat, ?_⟩
  simp only [Spec.Slab.Ctx.headTrace, Spec.Slab.Ctx.rd, Spec.Slab.Ctx.wr, tp_of (c := cx) hX,
    sym_of (c := cx) hX, ofNat_toNat64, addr8, addr4, rsName,
    show cx.arg s 1 = s.gpr .rsi from rfl, show cx.arg s 2 = s.gpr .rdx from rfl]
  by_cases hi : cx.InRange s
  · have hi' : (cpu32 s).toNat < SLAB_COUNT := (inRange_iff.mp hi)
    rw [wordAddr_eq hp]
    by_cases he : wvo s = s.gpr .rsi
    · rw [events_S hp hi he]; simp [hi', he]
    · rw [events_M hp hi he]
      have : ¬ (wvo s).toNat = (s.gpr .rsi).toNat := fun h' => he (BitVec.eq_of_toNat_eq h')
      simp [hi', this]
  · have hi' : ¬ (cpu32 s).toNat < SLAB_COUNT := fun h' => hi (inRange_iff.mpr h')
    rw [events_C hp hi]; simp [hi']

theorem rseqOk (hs : (casContract abi).pre s) : RseqOk target cas (casContract abi) s := by
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
  · by_cases he : wvo s = s.gpr .rsi
    · rw [prefixes_S hp hi he]
      simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
      exact ⟨abortOk hp mA, abortOk hp m1', abortOk hp mσ2, abortOk hp mσ2, abortOk hp m3,
        abortOk hp mσ4o, abortOk hp mσ5o, abortOk hp mσ6o, abortOk hp mσ7o, abortOk hp mσ8o,
        abortOk hp mσ9, abortOk hp mσ9⟩
    · rw [prefixes_M hp hi he]
      simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
      exact ⟨abortOk hp mA, abortOk hp m1', abortOk hp mσ2, abortOk hp mσ2, abortOk hp m3,
        abortOk hp mσ4o, abortOk hp mσ5o, abortOk hp mσ6o, abortOk hp mσ7o, abortOk hp mσ8o,
        abortOk hp mσ9⟩
  · rw [prefixes_C hp hi]
    simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
    exact ⟨abortOk hp mA, abortOk hp m1', abortOk hp mσ2⟩

end Contract

/-! ## The witnesses -/

/-- A thread as the pop's witnesses (`PopInline.wit`), its offset 8 (class 1's
word), `expected` and `new` in `rsi` and `rdx`. -/
def wit (cpu word e x : BitVec 64) (wr : List Region) : State where
  gpr r := if r = .rsp then 0x7000000 else if r = .rdi then 8 else if r = .rsi then e
    else if r = .rdx then x else 0
  cf := none
  zf := none
  sf := none
  of := none
  mem := mkMem [(0x2000, 0x100), (0x10104, cpu), (0x1000008, word)]
  rd := [⟨0x2000, 8⟩, ⟨0x10104, 4⟩]
  wr := wr
  syms n := if n = "__rseq_offset" then 0x2000 else if n = "SLABS" then 0x1000000 else 0
  fsBase := 0x10000
  csDesc := 0x9000

def w0 : State := wit 8192 0 0 0 [⟨0x10108, 8⟩]
def w1 : State := wit 0 0 8 0x4010 [⟨0x10108, 8⟩, ⟨0x1000008, 8⟩]
def w2 : State := wit 0 0 0 0x4010 [⟨0x10108, 8⟩, ⟨0x1000008, 8⟩]

theorem pre_w (s : State) (hrd : s.rd = cx.ro s) (hwr : s.wr = cx.rw s)
    (hnw : ((cx.regions s).all fun x => decide (x.1.base.toNat + x.1.len ≤ 2 ^ 64)) = true)
    (hp : Sig.pairsSep (cx.regions s) = true)
    (hres : ((cx.regions s).all fun x => (cx.A.reserved 0 s).all fun q => Region.sep x.1 q) = true)
    (hl : layoutCheck cx s = true) :
    (casContract abi).pre s :=
  (pre_iff cx (s := s) hv rfl rfl).mpr ⟨trivial, world_of_check hX hrd hwr hnw hp hres,
    rfl, rfl, trivial, layout_of_check hX hl, trivial⟩

theorem pre_w0 : (casContract abi).pre w0 :=
  pre_w _ (by decide +kernel) (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (by decide +kernel)
theorem pre_w1 : (casContract abi).pre w1 :=
  pre_w _ (by decide +kernel) (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (by decide +kernel)
theorem pre_w2 : (casContract abi).pre w2 :=
  pre_w _ (by decide +kernel) (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (by decide +kernel)

theorem outcomes_eq : cas.outcomes = [(0, true), (1, true), (2, true)] := rfl

theorem visits_w0 : Visits isa cas w0 0 true := by
  have hp := pre_of (facts_of_pre pre_w0)
  have hi : ¬ cx.InRange w0 := by decide +kernel
  exact Visits.rseq (execBlock_arm hp.facts.toArm) (end_C hp hi)

theorem visits_w1 : Visits isa cas w1 1 true := by
  have hp := pre_of (facts_of_pre pre_w1)
  have hi : cx.InRange w1 := by decide +kernel
  have he : ¬ wvo w1 = w1.gpr .rsi := by decide +kernel
  exact Visits.rseq (execBlock_arm hp.facts.toArm) (end_M hp hi he)

theorem visits_w2 : Visits isa cas w2 2 true := by
  have hp := pre_of (facts_of_pre pre_w2)
  have hi : cx.InRange w2 := by decide +kernel
  have he : wvo w2 = w2.gpr .rsi := by decide +kernel
  exact Visits.rseq (execBlock_arm hp.facts.toArm) (end_S hp hi he)

def witnesses : List State := [w0, w1, w2]

theorem witnessOk : (∃ w ∈ witnesses, (casContract abi).pre w) ∧
    ∀ o ∈ cas.outcomes, ∃ w ∈ witnesses, (casContract abi).pre w ∧ Visits isa cas w o.1 o.2 := by
  refine ⟨⟨w0, by simp [witnesses], pre_w0⟩, fun o ho => ?_⟩
  rw [outcomes_eq] at ho
  simp only [List.mem_cons, List.not_mem_nil, or_false] at ho
  rcases ho with rfl | rfl | rfl
  · exact ⟨w0, by simp [witnesses], pre_w0, visits_w0⟩
  · exact ⟨w1, by simp [witnesses], pre_w1, visits_w1⟩
  · exact ⟨w2, by simp [witnesses], pre_w2, visits_w2⟩

theorem verified : Verified target cas (casContract abi) :=
  ⟨fun _ hs => correct hs, fun _ hs => rseqOk hs, ⟨_, pre_w0⟩, fun o ho => by
    obtain ⟨w, -, hw, hv⟩ := witnessOk.2 o ho
    exact ⟨w, hw, hv⟩⟩

end VG.Proof.Slab.X86_64.Cas
