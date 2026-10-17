module

public import Asmalloc.Proof.Slab.X86_64.Slow
public import Asmalloc.Proof.Slab.X86_64.Witness
public import Asmalloc.Proof.Slab.Stack

/-!
# The stack push on x86-64

`slab_stack_push` (`Impl.Slab.X86_64.stackPush`) against
`Spec.Slab.stackPushContract`: its three ways out (no slab; the head is a
marker; the push), and in each its run, where it may be aborted, and its
memory accesses.
-/

@[expose] public section

namespace VG.Proof.Slab.X86_64.StackPush

open VG.X86_64 VG.Spec.Slab VG.Impl.Slab.X86_64 VG.Proof.Slab.X86_64 VG.Proof.Slab

abbrev cx : Spec.Slab.Ctx isa := ⟨abi, .stackPush, stackPushSig⟩

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
  .instr (.store { base := .rsi } .rax),
  .instr (.store slabWord .rsi)]

theorem code_eq : stackPush = .rseq arm body := rfl

/-! ## The states -/

/-- The magazine pushed. -/
def g (s : State) : BitVec 64 := s.gpr .rsi
def tst (s : State) : BitVec 32 := (wvo s).setWidth 32 &&& 1
def σ9 (s : State) : State := arithFlags (σ8o s) (tst s) false false
def mSlot (s : State) : Mem := (m1 s).writeW (g s) (wvo s)
def mEnd (s : State) : Mem := (mSlot s).writeW (wao s) (g s)
def σ11 (s : State) : State := { σ9 s with mem := mSlot s }
def σ12 (s : State) : State := { σ11 s with mem := mEnd s }
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
  g0 : (g s).toNat ≠ 0
  g8 : (g s).toNat % 8 = 0
  stack : cx.InRange s → StackOk s.mem (wvo s) (cx.Apart s)

theorem pre_of {s : State} (h : SFacts cx s) : Pre s := by
  have hw : wao s = cx.word s := wao_eq hX rfl
  have hwv : cx.w s = wvo s := by simp only [Spec.Slab.Ctx.w, mem_of (c := cx) hX, wvo, hw]
  obtain ⟨-, -, ⟨h0, h8⟩, -, hinv⟩ := h.pre
  refine ⟨h, hw, hwv, h0, h8, fun hi => ?_⟩
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

theorem exec_test : exec (.alu32 .test .rax (.imm 1)) (σ8o s) = some (σ9 s) := by
  rw [Flags.exec_test32_imm, σ8o_rax]; rfl

theorem eval_marker (hm : (wvo s).toNat % 2 = 1) : eval .ne (σ9 s) = some true := by
  have : ¬ tst s = 0 := fun h => (tst_zero_iff s).mp h hm
  simpa [eval, σ9, RegUpd.zf_arithFlags] using this

theorem eval_even (hm : ¬ (wvo s).toNat % 2 = 1) : eval .ne (σ9 s) = some false := by
  have : tst s = 0 := (tst_zero_iff s).mpr hm
  simp [eval, σ9, RegUpd.zf_arithFlags, this]

/-- The magazine's link slot: writable, apart from `rseq_cs`, the head, the
return address and `__rseq_offset`. -/
structure LinkFacts (s : State) : Prop where
  mem : (⟨g s, 8⟩ : Region) ∈ s.wr
  cs : Region.Disjoint ⟨g s, 8⟩ ⟨area s + 8, 8⟩
  word : Region.Disjoint ⟨g s, 8⟩ ⟨wao s, 8⟩
  rsVar : Region.Disjoint ⟨s.syms RS, 8⟩ ⟨g s, 8⟩
  ret : Region.Disjoint ⟨g s, 8⟩ ⟨s.gpr .rsp, 8⟩

theorem linkFacts (hp : Pre s) (hi : cx.InRange s) : LinkFacts s :=
  ⟨hp.facts.link rfl, hp.facts.link_cs rfl, by rw [hp.word]; exact hp.facts.link_word rfl hi,
    hp.facts.link_rsVar rfl, hp.facts.link_ret rfl⟩

theorem σ9_rsi : (σ9 s).gpr .rsi = g s := by
  rw [σ9, RegUpd.gpr_arithFlags, gpr_σ8o _ (by decide)]; rfl
theorem σ9_rax : (σ9 s).gpr .rax = wvo s := by rw [σ9, RegUpd.gpr_arithFlags, σ8o_rax]
theorem σ9_r9 : (σ9 s).gpr .r9 = s.syms "SLABS" := by rw [σ9, RegUpd.gpr_arithFlags, σ8o_r9]
theorem σ9_r8 : (σ9 s).gpr .r8 = r8o s := by rw [σ9, RegUpd.gpr_arithFlags, σ8o_r8]

theorem exec_link (hp : Pre s) (hi : cx.InRange s) :
    exec (.store { base := .rsi } .rax) (σ9 s) = some (σ11 s) := by
  have hl := linkFacts hp hi
  have hin : InRegions (σ9 s).wr ((σ9 s).ea { base := .rsi }) 8 := by
    rw [Access.ea_base, σ9_rsi, show (σ9 s).wr = s.wr from rfl]
    exact inWr (r := ⟨g s, 8⟩) hl.mem
  rw [exec_store _ hin, Access.ea_base, σ9_rsi, σ9_rax]
  rfl

theorem σ11_r9 : (σ11 s).gpr .r9 = s.syms "SLABS" := σ9_r9
theorem σ11_r8 : (σ11 s).gpr .r8 = r8o s := σ9_r8
theorem σ11_rsi : (σ11 s).gpr .rsi = g s := σ9_rsi

theorem exec_commit (hp : Pre s) (hi : cx.InRange s) :
    exec (.store slabWord .rsi) (σ11 s) = some (σ12 s) := by
  have hin : InRegions (σ11 s).wr ((σ11 s).ea slabWord) 8 := by
    rw [slabWord_ea_o _ σ11_r9 σ11_r8, show (σ11 s).wr = s.wr from rfl, hp.word]
    exact inWr (r := ⟨cx.word s, 8⟩) (hp.facts.word hi)
  rw [exec_store _ hin, slabWord_ea_o _ σ11_r9 σ11_r8, σ11_rsi]
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

theorem runBody_M (hp : Pre s) (hi : cx.InRange s) (hm : (wvo s).toNat % 2 = 1) :
    runBody isa body (σA s) = some (.done (σ9 s)) := by
  rw [body, runBody_instr (exec_cpu hp.facts.toArm), runBody_instr exec_cmp,
    runBody_skip (eval_cpu_false hi), runBody_instr exec_shl_r8, runBody_instr exec_off1,
    runBody_instr exec_off2, runBody_instr exec_off3, runBody_instr exec_off4,
    runBody_instr (exec_w hp hi), runBody_instr exec_test, runBody_take (eval_marker hm) rfl]

theorem prefixes_M (hp : Pre s) (hi : cx.InRange s) (hm : (wvo s).toNat % 2 = 1) :
    bodyPrefixes isa body (σA s) =
      [σA s, σ1 s, σ2 s, σ2 s, σ3 s, σ4o s, σ5o s, σ6o s, σ7o s, σ8o s, σ9 s] := by
  rw [body, bodyPrefixes_instr (exec_cpu hp.facts.toArm), bodyPrefixes_instr exec_cmp,
    bodyPrefixes_skip (eval_cpu_false hi), bodyPrefixes_instr exec_shl_r8,
    bodyPrefixes_instr exec_off1, bodyPrefixes_instr exec_off2, bodyPrefixes_instr exec_off3,
    bodyPrefixes_instr exec_off4, bodyPrefixes_instr (exec_w hp hi), bodyPrefixes_instr exec_test,
    bodyPrefixes_take (eval_marker hm)]

theorem end_M (hp : Pre s) (hi : cx.InRange s) (hm : (wvo s).toNat % 2 = 1) :
    bodyEnd isa body (σA s) = some 10 := by
  rw [body, bodyEnd_instr (exec_cpu hp.facts.toArm), bodyEnd_instr exec_cmp,
    bodyEnd_skip (eval_cpu_false hi), bodyEnd_instr exec_shl_r8, bodyEnd_instr exec_off1,
    bodyEnd_instr exec_off2, bodyEnd_instr exec_off3, bodyEnd_instr exec_off4,
    bodyEnd_instr (exec_w hp hi), bodyEnd_instr exec_test, bodyEnd_take (eval_marker hm)]
  rfl

theorem events_M (hp : Pre s) (hi : cx.InRange s) (hm : (wvo s).toNat % 2 = 1) :
    bodyEvents isa body (σA s) =
      [⟨area s + 4, 4, false, (cpu32 s).toNat⟩, ⟨wao s, 8, false, (wvo s).toNat⟩] := by
  rw [body, bodyEvents_instr (exec_cpu hp.facts.toArm), bodyEvents_instr exec_cmp,
    bodyEvents_skip (eval_cpu_false hi), bodyEvents_instr exec_shl_r8, bodyEvents_instr exec_off1,
    bodyEvents_instr exec_off2, bodyEvents_instr exec_off3, bodyEvents_instr exec_off4,
    bodyEvents_instr (exec_w hp hi), bodyEvents_instr exec_test, bodyEvents_take (eval_marker hm)]
  simp only [isa, ev_cpuLoad, ev_word (word_facts hp hi).2]; rfl

theorem runBody_P (hp : Pre s) (hi : cx.InRange s) (hm : ¬ (wvo s).toNat % 2 = 1) :
    runBody isa body (σA s) = some (.done (σ12 s)) := by
  rw [body, runBody_instr (exec_cpu hp.facts.toArm), runBody_instr exec_cmp,
    runBody_skip (eval_cpu_false hi), runBody_instr exec_shl_r8, runBody_instr exec_off1,
    runBody_instr exec_off2, runBody_instr exec_off3, runBody_instr exec_off4,
    runBody_instr (exec_w hp hi), runBody_instr exec_test, runBody_skip (eval_even hm),
    runBody_instr (exec_link hp hi), runBody_instr (exec_commit hp hi), runBody_nil']

theorem prefixes_P (hp : Pre s) (hi : cx.InRange s) (hm : ¬ (wvo s).toNat % 2 = 1) :
    bodyPrefixes isa body (σA s) =
      [σA s, σ1 s, σ2 s, σ2 s, σ3 s, σ4o s, σ5o s, σ6o s, σ7o s, σ8o s, σ9 s, σ9 s, σ11 s] := by
  rw [body, bodyPrefixes_instr (exec_cpu hp.facts.toArm), bodyPrefixes_instr exec_cmp,
    bodyPrefixes_skip (eval_cpu_false hi), bodyPrefixes_instr exec_shl_r8,
    bodyPrefixes_instr exec_off1, bodyPrefixes_instr exec_off2, bodyPrefixes_instr exec_off3,
    bodyPrefixes_instr exec_off4, bodyPrefixes_instr (exec_w hp hi), bodyPrefixes_instr exec_test,
    bodyPrefixes_skip (eval_even hm), bodyPrefixes_instr (exec_link hp hi),
    bodyPrefixes_last (exec_commit hp hi)]

theorem end_P (hp : Pre s) (hi : cx.InRange s) (hm : ¬ (wvo s).toNat % 2 = 1) :
    bodyEnd isa body (σA s) = some 13 := by
  rw [body, bodyEnd_instr (exec_cpu hp.facts.toArm), bodyEnd_instr exec_cmp,
    bodyEnd_skip (eval_cpu_false hi), bodyEnd_instr exec_shl_r8, bodyEnd_instr exec_off1,
    bodyEnd_instr exec_off2, bodyEnd_instr exec_off3, bodyEnd_instr exec_off4,
    bodyEnd_instr (exec_w hp hi), bodyEnd_instr exec_test, bodyEnd_skip (eval_even hm),
    bodyEnd_instr (exec_link hp hi), bodyEnd_instr (exec_commit hp hi), bodyEnd_nil']
  rfl

theorem ev_link : Instr.events (.store { base := .rsi } .rax) (σ9 s) (σ11 s) =
    [⟨g s, 8, true, (wvo s).toNat⟩] := by
  simp only [Instr.events, Access.ea_base, σ9_rsi, toNat_read8]
  rw [show (σ11 s).mem = mSlot s from rfl, mSlot, Mem.readW_writeW_self64]

theorem ev_commit : Instr.events (.store slabWord .rsi) (σ11 s) (σ12 s) =
    [⟨wao s, 8, true, (g s).toNat⟩] := by
  simp only [Instr.events, slabWord_ea_o _ σ11_r9 σ11_r8, toNat_read8]
  rw [show (σ12 s).mem = mEnd s from rfl, mEnd, Mem.readW_writeW_self64]

theorem events_P (hp : Pre s) (hi : cx.InRange s) (hm : ¬ (wvo s).toNat % 2 = 1) :
    bodyEvents isa body (σA s) =
      [⟨area s + 4, 4, false, (cpu32 s).toNat⟩, ⟨wao s, 8, false, (wvo s).toNat⟩,
        ⟨g s, 8, true, (wvo s).toNat⟩, ⟨wao s, 8, true, (g s).toNat⟩] := by
  rw [body, bodyEvents_instr (exec_cpu hp.facts.toArm), bodyEvents_instr exec_cmp,
    bodyEvents_skip (eval_cpu_false hi), bodyEvents_instr exec_shl_r8, bodyEvents_instr exec_off1,
    bodyEvents_instr exec_off2, bodyEvents_instr exec_off3, bodyEvents_instr exec_off4,
    bodyEvents_instr (exec_w hp hi), bodyEvents_instr exec_test, bodyEvents_skip (eval_even hm),
    bodyEvents_instr (exec_link hp hi), bodyEvents_instr (exec_commit hp hi), bodyEvents_nil']
  simp only [ev_cpuLoad, ev_word (word_facts hp hi).2, ev_link, ev_commit]; rfl

end Cases

/-! ## The contract -/

section Contract

variable {s : State}

theorem hv : cx.A.args ((cx.sig.words cx.A.ptrBits).map (·.bits cx.A.ptrBits)) =
    some (fun s => [s.gpr .rdi, s.gpr .rsi]) := rfl

theorem facts_of_pre (hs : (stackPushContract abi).pre s) : SFacts cx s := by
  have := (pre_iff cx (s := s) hv rfl rfl).mp hs
  exact sfacts_of_world hX this.2.1 this.2.2

theorem abi_m1 (hp : Pre s) {p : State} (hm : Mid scratchT s (m1 s) p) : abiPreserved s p :=
  abiPreserved_of_mid frame_m1 (by simpa using hp.facts.ret_cs) (by decide) hm

theorem unchanged_cs : Unchanged [cx.csField s] (cx.mem s) (m1 s) := by
  rw [csField_of (c := cx) hX, mem_of (c := cx) hX]; exact frame_m1

theorem mσ2 : Mid scratchT s (m1 s) (σ2 s) :=
  mid_σ2 (mid_σ1 (by decide) (mid_σA (by decide) (by decide)))
theorem mσ9 : Mid scratchT s (m1 s) (σ9 s) := mσ8o.arithFlags _ _ _
theorem mσ11 : Mid scratchT s (mSlot s) (σ11 s) := mσ9.withMem _
theorem mσ12 : Mid scratchT s (mEnd s) (σ12 s) := mσ11.withMem _
theorem mσC : Mid scratchT s (m1 s) (σC s) := mσ2.setReg32 (by decide) _

theorem link_eq : cx.link s = ⟨g s, 8⟩ := rfl

/-- The stack's link slots are as on entry in memory `m` changed only in
`rseq_cs`, the magazine's link slot and the head. -/
theorem chain_of (hp : Pre s) (hi : cx.InRange s) {m : Mem}
    (hm : ∀ x, Region.Disjoint ⟨x, 8⟩ ⟨wao s, 8⟩ → Region.Disjoint ⟨x, 8⟩ ⟨area s + 8, 8⟩ →
      Region.Disjoint ⟨x, 8⟩ ⟨g s, 8⟩ → m.readW x 64 = s.mem.readW x 64)
    {l : List Addr} (hc : Chain s.mem (wvo s) l) : Chain m (wvo s) l := by
  have hd := stackOk_chain (hp.stack hi) hc
  refine chain_frame hc fun x hx => hm x ?_ ?_ ?_
  · rw [hp.word]; exact (hd x hx).2.1.sub_right (word_sub (cls_lt hp.facts.pre.2.2.1) hi)
  · rw [← csField_of (c := cx) hX]; exact (hd x hx).2.2.1 (cx.csField s, true) cs_mem_fixed
  · have := (hd x hx).2.2.1 (cx.link s, true) (link_mem_fixed rfl)
    rwa [link_eq] at this

theorem pushed (hp : Pre s) (hi : cx.InRange s) :
    Unchanged [cx.csField s, ⟨cx.word s, 8⟩, cx.link s] (cx.mem s) (mEnd s) ∧
    (mEnd s).readW (cx.word s) 64 = cx.arg s 1 ∧
    ∀ l, Chain (cx.mem s) (cx.w s) l → Chain (mEnd s) (cx.arg s 1) (cx.arg s 1 :: l) := by
  have hl := linkFacts hp hi
  refine ⟨?_, ?_, fun l hc => ?_⟩
  · rw [csField_of (c := cx) hX, mem_of (c := cx) hX, ← hp.word, link_eq]
    exact (frame_three s.mem _ _ _ _ _ _).mono (by simp)
  · rw [mEnd, ← hp.word, Mem.readW_writeW_self64]; rfl
  · rw [mem_of (c := cx) hX, hp.w] at hc
    refine .cons hp.g0 hp.g8 ?_
    rw [show cx.arg s 1 = g s from rfl, mEnd, readW64_writeW_disj hl.word, mSlot,
      Mem.readW_writeW_self64]
    exact chain_of hp hi (fun x hw hcs hg => by
      rw [readW64_writeW_disj hw, readW64_writeW_disj hg, m1, readW64_writeW_disj hcs]) hc

theorem post_eq (s' : State) : (stackPushContract abi).post s s' ↔
    NoCpu cx s s' ∨ (cx.InRange s ∧ s'.gpr .rax = cx.w s ∧
      (((cx.w s).toNat % 2 = 1 ∧ Unchanged [cx.csField s] (cx.mem s) s'.mem) ∨
        ((cx.w s).toNat % 2 = 0 ∧
          Unchanged [cx.csField s, ⟨cx.word s, 8⟩, cx.link s] (cx.mem s) s'.mem ∧
          s'.mem.readW (cx.word s) 64 = cx.arg s 1 ∧
          ∀ l, Chain (cx.mem s) (cx.w s) l → Chain s'.mem (cx.arg s 1) (cx.arg s 1 :: l)))) :=
  post_iff cx hv s s'

theorem correct (hs : (stackPushContract abi).pre s) :
    (∃ s', Exec isa stackPush s s' ∧ target.abiPreserved s s' ∧ (stackPushContract abi).post s s') ∨
    (∃ t s', ExecExit isa stackPush s t s' ∧ target.abiPreserved s s' ∧
      (stackPushContract abi).exit t s s') := by
  have h := facts_of_pre hs
  have hp := pre_of h
  refine .inl ?_
  rw [code_eq]
  by_cases hi : cx.InRange s
  · by_cases hm : (wvo s).toNat % 2 = 1
    · refine ⟨σ9 s, .rseq (execBlock_arm h.toArm) (runBody_M hp hi hm), abi_m1 hp mσ9,
        (post_eq _).mpr (.inr ⟨hi, by rw [σ9_rax, hp.w], .inl ⟨by rw [hp.w]; exact hm, ?_⟩⟩)⟩
      show Unchanged _ _ (σ9 s).mem; rw [mσ9.mem]; exact unchanged_cs
    · have hl := linkFacts hp hi
      have hw := hp.facts.ret_word hi
      rw [← hp.word] at hw
      obtain ⟨hu, hr, hch⟩ := pushed hp hi
      refine ⟨σ12 s, .rseq (execBlock_arm h.toArm) (runBody_P hp hi hm),
        abiPreserved_of_mid (frame_three s.mem _ _ _ _ _ _)
          (by simpa using ⟨hp.facts.ret_cs, hl.ret, hw⟩) (by decide) mσ12,
        (post_eq _).mpr (.inr ⟨hi, ?_, .inr ⟨by rw [hp.w]; omega, hu, hr, hch⟩⟩)⟩
      show (σ9 s).gpr .rax = _
      rw [σ9_rax, hp.w]
  · exact ⟨σC s, .rseq (execBlock_arm h.toArm) (runBody_C hp hi), abi_m1 hp mσC,
      (post_eq _).mpr (.inl ⟨hi, by simp [σC, State.setReg32]; rfl,
        by show Unchanged _ _ (σC s).mem; rw [mσC.mem]; exact unchanged_cs⟩)⟩

/-! ### Where it may be aborted -/

/-- Every slab's stack, in memory changed only apart from its link slots:
in `rseq_cs` and the pushed magazine's link slot. -/
theorem chain_any (hp : Pre s) {i : Nat} (hi : i < SLAB_COUNT) {m : Mem}
    (hm : ∀ x, Region.Disjoint ⟨x, 8⟩ ⟨area s + 8, 8⟩ → Region.Disjoint ⟨x, 8⟩ ⟨g s, 8⟩ →
      m.readW x 64 = s.mem.readW x 64)
    {l : List Addr} (hc : Chain s.mem (cx.wordOf s i) l) : Chain m (cx.wordOf s i) l := by
  have hst : StackOk s.mem (cx.wordOf s i) (cx.Apart s) := by
    have := hp.facts.pre.2.2.2.2 i hi
    rwa [mem_of (c := cx) hX] at this
  have hd := stackOk_chain hst hc
  refine chain_frame hc fun x hx => hm x ?_ ?_
  · rw [← csField_of (c := cx) hX]; exact (hd x hx).2.2.1 (cx.csField s, true) cs_mem_fixed
  · have := (hd x hx).2.2.1 (cx.link s, true) (link_mem_fixed rfl)
    rwa [link_eq] at this

theorem abort_eq (p : State) : (stackPushContract abi).abort s p ↔
    [p.gpr .rdi, p.gpr .rsi] = [s.gpr .rdi, s.gpr .rsi] ∧
      ((Unchanged [cx.csField s, cx.link s] (cx.mem s) p.mem ∧
        ∀ i < SLAB_COUNT, ∀ l, Chain (cx.mem s) (cx.wordOf s i) l → Chain p.mem (cx.wordOf s i) l) ∧
        cx.inv p) :=
  abort_iff cx hv s p

theorem abortOk1 (hp : Pre s) {p : State} (hm : Mid scratchT s (m1 s) p) :
    armed p ∧ target.abiPreserved s p ∧ (stackPushContract abi).abort s p ∧
      ∀ p', target.rely p p' → (stackPushContract abi).env p p' → (stackPushContract abi).pre p' := by
  obtain ⟨hip, hre⟩ := retry hX hv (post := fun s s' => _) (fun _ => rfl) (fun _ => rfl) (by decide)
    hp.facts.pre hm (by decide) (by decide) frame_m1 (harmless_cs1 hX hp.facts.pre)
  refine ⟨armed_of_mid hp.facts.toArm (xs := []) frame_m1 m1_cs (by simp) hm, abi_m1 hp hm, ?_, hre⟩
  refine (abort_eq p).mpr ⟨by rw [hm.gpr .rdi (by decide), hm.gpr .rsi (by decide)], ⟨?_,
    fun i hi l hc => ?_⟩, hip⟩
  · rw [hm.mem, csField_of (c := cx) hX, mem_of (c := cx) hX]
    exact frame_m1.mono (by simp)
  · rw [hm.mem]
    rw [mem_of (c := cx) hX] at hc
    exact chain_any hp hi (fun x hcs _ => by rw [m1, readW64_writeW_disj hcs]) hc

theorem abortOk2 (hp : Pre s) (hi : cx.InRange s) :
    armed (σ11 s) ∧ target.abiPreserved s (σ11 s) ∧ (stackPushContract abi).abort s (σ11 s) ∧
      ∀ p', target.rely (σ11 s) p' → (stackPushContract abi).env (σ11 s) p' →
        (stackPushContract abi).pre p' := by
  have hl := linkFacts hp hi
  have hfr : Frame [⟨area s + 8, 8⟩, ⟨g s, 8⟩] s.mem (mSlot s) := frame_two _ _ _ _ _
  have hcs : (mSlot s).readW (area s + 8) 64 = s.csDesc := by
    rw [mSlot, readW64_writeW_disj hl.cs.symm, m1_cs]
  have hW := harmless_cs2 hX hp.facts.pre (r := ⟨g s, 8⟩)
    (by rw [← link_eq]; exact harmless_link rfl hp.facts.pre.2.2.2.1)
  obtain ⟨hip, hre⟩ := retry hX hv (post := fun s s' => _) (fun _ => rfl) (fun _ => rfl) (by decide)
    hp.facts.pre mσ11 (by decide) (by decide) hfr hW
  refine ⟨armed_of_mid hp.facts.toArm (xs := [⟨g s, 8⟩]) hfr hcs (by simpa using hl.rsVar) mσ11,
    abiPreserved_of_mid hfr (by simpa using ⟨hp.facts.ret_cs, hl.ret⟩) (by decide) mσ11, ?_, hre⟩
  refine (abort_eq _).mpr ⟨by rw [mσ11.gpr .rdi (by decide), mσ11.gpr .rsi (by decide)], ⟨?_,
    fun i hi' l hc => ?_⟩, hip⟩
  · rw [mσ11.mem, csField_of (c := cx) hX, mem_of (c := cx) hX, link_eq]; exact hfr
  · rw [mσ11.mem]
    rw [mem_of (c := cx) hX] at hc
    exact chain_any hp hi' (fun x hcs hg => by
      rw [mSlot, readW64_writeW_disj hg, m1, readW64_writeW_disj hcs]) hc

/-! ### Its accesses -/

theorem addr8 : s.fsBase + o s + BitVec.ofNat 64 RSEQ_CS = area s + 8 := rfl
theorem addr4 : s.fsBase + o s + BitVec.ofNat 64 RSEQ_CPU_ID = area s + 4 := rfl
theorem rsName : RSEQ_OFFSET = RS := rfl

theorem ofNat_toNat64 (v : BitVec 64) : BitVec.ofNat 64 v.toNat = v := by
  rw [BitVec.ofNat_toNat, BitVec.setWidth_eq]

theorem wordAddr_eq (hp : Pre s) : wordAddr (s.syms SLABS) (cpu32 s).toNat (cx.cls s) = wao s := by
  rw [hp.word]; rfl

theorem trace_ok (hs : (stackPushContract abi).pre s) :
    (stackPushContract abi).trace s (Code.events isa stackPush s) := by
  have h := facts_of_pre hs
  have hp := pre_of h
  show cx.StackPushTrace s (cx.arg s 1) (Code.events isa (.rseq arm body) s)
  simp only [Code.events, execBlock_arm h.toArm, events_arm h.toArm]
  refine ⟨(o s).toNat, s.csDesc.toNat, (cpu32 s).toNat, (wvo s).toNat, ?_⟩
  simp only [Spec.Slab.Ctx.headTrace, Spec.Slab.Ctx.rd, Spec.Slab.Ctx.wr, tp_of (c := cx) hX,
    sym_of (c := cx) hX, ofNat_toNat64, addr8, addr4, rsName,
    show cx.arg s 1 = g s from rfl]
  by_cases hi : cx.InRange s
  · have hi' : (cpu32 s).toNat < SLAB_COUNT := (inRange_iff.mp hi)
    rw [wordAddr_eq hp]
    by_cases hm : (wvo s).toNat % 2 = 1
    · rw [events_M hp hi hm]; simp [hi', hm]
    · rw [events_P hp hi hm]
      have : (wvo s).toNat % 2 = 0 := by omega
      simp [hi', this]
  · have hi' : ¬ (cpu32 s).toNat < SLAB_COUNT := fun h' => hi (inRange_iff.mpr h')
    rw [events_C hp hi]; simp [hi']

theorem rseqOk (hs : (stackPushContract abi).pre s) :
    RseqOk target stackPush (stackPushContract abi) s := by
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
      exact ⟨abortOk1 hp mA, abortOk1 hp m1', abortOk1 hp mσ2, abortOk1 hp mσ2, abortOk1 hp m3,
        abortOk1 hp mσ4o, abortOk1 hp mσ5o, abortOk1 hp mσ6o, abortOk1 hp mσ7o, abortOk1 hp mσ8o,
        abortOk1 hp mσ9⟩
    · rw [prefixes_P hp hi hm]
      simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
      exact ⟨abortOk1 hp mA, abortOk1 hp m1', abortOk1 hp mσ2, abortOk1 hp mσ2, abortOk1 hp m3,
        abortOk1 hp mσ4o, abortOk1 hp mσ5o, abortOk1 hp mσ6o, abortOk1 hp mσ7o, abortOk1 hp mσ8o,
        abortOk1 hp mσ9, abortOk1 hp mσ9, abortOk2 hp hi⟩
  · rw [prefixes_C hp hi]
    simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
    exact ⟨abortOk1 hp mA, abortOk1 hp m1', abortOk1 hp mσ2⟩

end Contract

/-! ## The witnesses -/

/-- A thread as the store-if-equal's witnesses (`Cas.wit`), offset 8, its
magazine at `0x4000`, the head `head` in slab 0. -/
def wit (cpu head : BitVec 64) (wr : List Region) : State where
  gpr r := if r = .rsp then 0x7000000 else if r = .rdi then 8 else if r = .rsi then 0x4000 else 0
  cf := none
  zf := none
  sf := none
  of := none
  mem := mkMem [(0x2000, 0x100), (0x10104, cpu), (0x1000008, head)]
  rd := [⟨0x2000, 8⟩, ⟨0x10104, 4⟩]
  wr := wr
  syms n := if n = "__rseq_offset" then 0x2000 else if n = "SLABS" then 0x1000000 else 0
  fsBase := 0x10000
  csDesc := 0x9000

def w0 : State := wit 8192 0 [⟨0x10108, 8⟩, ⟨0x4000, 8⟩]
def w1 : State := wit 0 1 [⟨0x10108, 8⟩, ⟨0x4000, 8⟩, ⟨0x1000008, 8⟩]
def w2 : State := wit 0 0 [⟨0x10108, 8⟩, ⟨0x4000, 8⟩, ⟨0x1000008, 8⟩]

theorem pre_w (s : State) (hm : ∃ cpu head wr, s = wit cpu head wr)
    (hrd : s.rd = cx.ro s) (hwr : s.wr = cx.rw s)
    (hnw : ((cx.regions s).all fun x => decide (x.1.base.toNat + x.1.len ≤ 2 ^ 64)) = true)
    (hp : Sig.pairsSep (cx.regions s) = true)
    (hres : ((cx.regions s).all fun x => (cx.A.reserved 0 s).all fun q => Region.sep x.1 q) = true)
    (hl : layoutCheck cx s = true)
    (hargs : (s.gpr .rsi).toNat ≠ 0 ∧ (s.gpr .rsi).toNat % 8 = 0) (hcls : cx.cls s = 1) (hsyms : s.syms SLABS = 0x1000000)
    (h0 : ∀ ok, StackOk s.mem (s.mem.readW (wordAddr 0x1000000 0 1) 64) ok) :
    (stackPushContract abi).pre s := by
  refine (pre_iff cx (s := s) hv rfl rfl).mpr ⟨trivial, world_of_check hX hrd hwr hnw hp hres,
    rfl, rfl, hargs, layout_of_check hX hl, fun i hi => ?_⟩
  show StackOk (cx.mem s) (cx.wordOf s i) _
  simp only [Spec.Slab.Ctx.wordOf]
  rw [mem_of (c := cx) hX, sym_of (c := cx) hX, hcls, hsyms]
  rcases Nat.eq_zero_or_pos i with rfl | hpos
  · exact h0 _
  · obtain ⟨cpu, head, wr, rfl⟩ := hm
    rw [show (wit cpu head wr).mem = mkMem _ from rfl,
      mkMem_slab_zero (by intro bv hbv; simp at hbv; rcases hbv with rfl | rfl | rfl <;> simp)
        hpos hi]
    exact stackOk_zero

theorem pre_w0 : (stackPushContract abi).pre w0 :=
  pre_w _ ⟨_, _, _, rfl⟩ (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (by decide +kernel) (fun _ => by rw [show w0.mem.readW _ 64 = 0 by decide +kernel]; exact stackOk_zero)
theorem pre_w1 : (stackPushContract abi).pre w1 :=
  pre_w _ ⟨_, _, _, rfl⟩ (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (by decide +kernel) (fun _ => .inl (by decide +kernel))
theorem pre_w2 : (stackPushContract abi).pre w2 :=
  pre_w _ ⟨_, _, _, rfl⟩ (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (by decide +kernel) (fun _ => by rw [show w2.mem.readW _ 64 = 0 by decide +kernel]; exact stackOk_zero)

theorem outcomes_eq : stackPush.outcomes = [(0, true), (1, true), (2, true)] := rfl

theorem visits_w0 : Visits isa stackPush w0 0 true := by
  have hp := pre_of (facts_of_pre pre_w0)
  have hi : ¬ cx.InRange w0 := by decide +kernel
  exact Visits.rseq (execBlock_arm hp.facts.toArm) (end_C hp hi)

theorem visits_w1 : Visits isa stackPush w1 1 true := by
  have hp := pre_of (facts_of_pre pre_w1)
  have hi : cx.InRange w1 := by decide +kernel
  have hm : (wvo w1).toNat % 2 = 1 := by decide +kernel
  exact Visits.rseq (execBlock_arm hp.facts.toArm) (end_M hp hi hm)

theorem visits_w2 : Visits isa stackPush w2 2 true := by
  have hp := pre_of (facts_of_pre pre_w2)
  have hi : cx.InRange w2 := by decide +kernel
  have hm : ¬ (wvo w2).toNat % 2 = 1 := by decide +kernel
  exact Visits.rseq (execBlock_arm hp.facts.toArm) (end_P hp hi hm)

def witnesses : List State := [w0, w1, w2]

theorem witnessOk : (∃ w ∈ witnesses, (stackPushContract abi).pre w) ∧
    ∀ o ∈ stackPush.outcomes, ∃ w ∈ witnesses, (stackPushContract abi).pre w ∧
      Visits isa stackPush w o.1 o.2 := by
  refine ⟨⟨w0, by simp [witnesses], pre_w0⟩, fun o ho => ?_⟩
  rw [outcomes_eq] at ho
  simp only [List.mem_cons, List.not_mem_nil, or_false] at ho
  rcases ho with rfl | rfl | rfl
  · exact ⟨w0, by simp [witnesses], pre_w0, visits_w0⟩
  · exact ⟨w1, by simp [witnesses], pre_w1, visits_w1⟩
  · exact ⟨w2, by simp [witnesses], pre_w2, visits_w2⟩

theorem verified : Verified target stackPush (stackPushContract abi) :=
  ⟨fun _ hs => correct hs, fun _ hs => rseqOk hs, ⟨_, pre_w0⟩, fun o ho => by
    obtain ⟨w, -, hw, hv⟩ := witnessOk.2 o ho
    exact ⟨w, hw, hv⟩⟩

end VG.Proof.Slab.X86_64.StackPush
