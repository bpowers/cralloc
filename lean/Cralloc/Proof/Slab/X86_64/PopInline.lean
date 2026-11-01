module

public import Cralloc.Proof.Slab.X86_64.Word

/-!
# The inline pop on x86-64

`slab_pop` inline (`Impl.Slab.X86_64.popInline`) against
`Spec.Slab.popInlineContract`: its three ways out (no slab for the CPU; no
pointer to pop; the commit), and in each its run, where it may be aborted,
and its memory accesses. The class's word in slab 0 is at `rdi` and the mask
in `rsi`.
-/

@[expose] public section

namespace VG.Proof.Slab.X86_64.PopInline

open VG.X86_64 VG.Spec.Slab VG.Impl.Slab.X86_64 VG.Proof.Slab.X86_64

/-- The routine as the contract sees it. -/
abbrev cx : Spec.Slab.Ctx isa := ⟨inlineAbi, .inline, popInlineSig⟩

theorem hX : X86 cx.A := x86_inline

def body : List (Step Instr Cond) := [
  .instr (.mov32 .r8 (.mem { base := .r10, disp := 4, seg := some .fs })),
  .instr (.alu32 .cmp .r8 (.imm 8192)),
  .exit .ae zero none,
  .instr (.shift .shl .r8 12),
  .instr (.mov .rcx (.mem (wordAt .rdi))),
  .instr (.alu32 .test .rcx (.reg .rsi)),
  .exit .e zero none,
  .instr (.mov .rax (.mem { base := .rcx, disp := -8 })),
  .instr (.lea .rcx { base := .rcx, disp := -8 }),
  .instr (.store (wordAt .rdi) .rcx)]

theorem code_eq : popInline = .rseq arm body := rfl

/-! ## The states -/

/-- The mask test. -/
def tst (s : State) : BitVec 32 := (wv s).setWidth 32 &&& (s.gpr .rsi).setWidth 32
def σ5 (s : State) : State := arithFlags (σ4 s) (tst s) false false
/-- The pointer popped. -/
def x (s : State) : BitVec 64 := s.mem.readW (wv s - 8) 64
def σ7 (s : State) : State := (σ5 s).setReg .rax (x s)
def σ8 (s : State) : State := (σ7 s).setReg .rcx (wv s - 8)
/-- Memory after the commit. -/
def mEnd (s : State) : Mem := (m1 s).writeW (wa s) (wv s - 8)
def σ9 (s : State) : State := { σ8 s with mem := mEnd s }
/-- After a miss: no slab, or nothing to pop. -/
def σC (s : State) : State := (σ2 s).setReg32 .rax 0
def σT (s : State) : State := (σ5 s).setReg32 .rax 0

/-! ## What the precondition gives -/

/-- The facts of the entry state, and what the arguments make of the class
and mask. -/
structure Pre (s : State) (k : Nat) : Prop where
  facts : Facts cx s
  hk : 2 ≤ k
  hk' : k ≤ 16
  mask : (s.gpr .rsi).toNat = magMask (2 ^ k)
  slots : cx.slots s = 2 ^ k
  word : wa s = cx.word s
  w : cx.w s = wv s
  ok : cx.InRange s → WordOk (2 ^ k) (wv s)

theorem arg0 (s : State) : cx.arg s 0 = s.gpr .rdi := rfl

theorem pre_of {s : State} (h : Facts cx s) : ∃ k, Pre s k := by
  obtain ⟨-, -, ⟨hoff, -, ⟨k, hk, hk', hm⟩⟩, -, hinv⟩ := h.pre
  have hmask : cx.mask s = (s.gpr .rsi).toNat := rfl
  have hslots : cx.slots s = 2 ^ k := by
    rw [Spec.Slab.Ctx.slots, hm]; exact slotsOf_magMask k hk
  have hw : wa s = cx.word s := wa_eq hX (arg0 s) hoff
  have hwv : cx.w s = wv s := by simp only [Spec.Slab.Ctx.w, mem_of (c := cx) hX, wv, hw]
  refine ⟨k, h, hk, hk', by rw [← hmask]; exact hm, hslots, hw, hwv, fun hi => ?_⟩
  have := (hinv.1 (cx.cpu s) hi).1
  rw [← hslots, ← hwv]
  exact this

/-! ## The steps after the slab word -/

section Steps

variable {s : State} {k : Nat}

theorem σ4_rsi : (σ4 s).gpr .rsi = s.gpr .rsi := gpr_σ4 (by decide) (by decide) (by decide) (by decide)

theorem exec_test : exec (.alu32 .test .rcx (.reg .rsi)) (σ4 s) = some (σ5 s) := by
  rw [exec_test32_reg, σ4_rcx, σ4_rsi]; rfl

theorem eval_test : eval .e (σ5 s) = some (tst s == 0) := rfl

theorem exec_cold_cpu : execBlock isa zero (σ2 s) = some (σC s) := rfl
theorem exec_cold_test : execBlock isa zero (σ5 s) = some (σT s) := rfl

/-- What a pop that passes the test pops. -/
theorem hit (hp : Pre s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    IsSlot (2 ^ k) (wv s) ∧ 2 ≤ slot (2 ^ k) (wv s) ∧
      (Mag.ofWord (2 ^ k) s.mem (wv s)).pop =
        some (x s, .stack (magBase (2 ^ k) (wv s)) (items (2 ^ k) s.mem (wv s)).dropLast) := by
  have hm : ¬ (wv s).toNat % (8 * 2 ^ k) < 16 := fun h' =>
    ht ((test_iff hp.hk hp.hk' (wv s) (s.gpr .rsi) hp.mask).mpr h')
  exact pop_of_test (dvd_marker hp.hk') (hp.ok hi) hm

/-- The magazine's region, in the routine's terms. -/
theorem magRegion_eq (hp : Pre s k) : cx.magRegion s = ⟨magBase (2 ^ k) (wv s), 8 * 2 ^ k⟩ := by
  simp only [Spec.Slab.Ctx.magRegion, hp.slots, hp.w]

theorem σ5_rd : (σ5 s).rd = s.rd := rfl
theorem σ5_wr : (σ5 s).wr = s.wr := rfl
theorem σ5_mem : (σ5 s).mem = m1 s := rfl
theorem σ5_rcx : (σ5 s).gpr .rcx = wv s := by rw [σ5, RegUpd.gpr_arithFlags, σ4_rcx]

theorem below_ea (σ : State) (h : σ.gpr .rcx = wv s) :
    σ.ea { base := .rcx, disp := -8 } = wv s - 8 := by
  rw [Access.ea_base_disp, Access.add_ofInt_neg_lit, h]; rfl

theorem below_facts (hp : Pre s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    InRegions (s.rd ++ s.wr) (wv s - 8) 8 ∧ Region.Disjoint ⟨wv s - 8, 8⟩ ⟨area s + 8, 8⟩ := by
  obtain ⟨hs, h2, -⟩ := hit hp hi ht
  have hsl : IsSlot (cx.slots s) (cx.w s) := by rw [hp.slots, hp.w]; exact hs
  have hmag := hp.facts.mag hi hsl
  have hcm := hp.facts.cs_mag hi hsl
  rw [magRegion_eq hp] at hmag hcm
  have hc := contains_below hs h2 (slot_lt _ _ (Nat.two_pow_pos k))
  exact ⟨(InRegions.of_mem hmag hc).append_right, hcm.symm.sub_left (sub_of_contains hc)⟩

theorem exec_x (hp : Pre s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    exec (.mov .rax (.mem { base := .rcx, disp := -8 })) (σ5 s) = some (σ7 s) := by
  obtain ⟨hin, hdj⟩ := below_facts hp hi ht
  have hin' : InRegions ((σ5 s).rd ++ (σ5 s).wr) ((σ5 s).ea { base := .rcx, disp := -8 }) 8 := by
    rw [below_ea _ σ5_rcx, σ5_rd, σ5_wr]; exact hin
  rw [exec_load _ hin', below_ea _ σ5_rcx, σ5_mem, m1, readW64_writeW_disj hdj]
  rfl

theorem σ7_rcx : (σ7 s).gpr .rcx = wv s := by
  rw [σ7, RegUpd.gpr_setReg_of_ne _ _ (by decide), σ5_rcx]

theorem exec_lea8 : exec (.lea .rcx { base := .rcx, disp := -8 }) (σ7 s) = some (σ8 s) := by
  rw [exec_lea, Access.offset_base_disp, Access.add_ofInt_neg_lit, σ7_rcx]; rfl

theorem σ8_rdi : (σ8 s).gpr .rdi = s.gpr .rdi := by
  simp only [σ8, σ7, σ5, RegUpd.gpr_setReg_of_ne _ _ (show ¬ Reg.rdi = .rcx by decide),
    RegUpd.gpr_setReg_of_ne _ _ (show ¬ Reg.rdi = .rax by decide), RegUpd.gpr_arithFlags, σ4_rdi]
theorem σ8_r8 : (σ8 s).gpr .r8 = sh s := by
  simp only [σ8, σ7, σ5, RegUpd.gpr_setReg_of_ne _ _ (show ¬ Reg.r8 = .rcx by decide),
    RegUpd.gpr_setReg_of_ne _ _ (show ¬ Reg.r8 = .rax by decide), RegUpd.gpr_arithFlags, σ4_r8]
theorem σ8_rcx : (σ8 s).gpr .rcx = wv s - 8 := RegUpd.gpr_setReg_self _ _ _
theorem σ8_mem : (σ8 s).mem = m1 s := rfl
theorem σ8_wr : (σ8 s).wr = s.wr := rfl

theorem exec_commit (hp : Pre s k) (hi : cx.InRange s) :
    exec (.store (wordAt .rdi) .rcx) (σ8 s) = some (σ9 s) := by
  have hin : InRegions (σ8 s).wr ((σ8 s).ea (wordAt .rdi)) 8 := by
    rw [wordAt_ea _ σ8_rdi σ8_r8, σ8_wr, hp.word]
    exact inWr (r := ⟨cx.word s, 8⟩) (hp.facts.word hi)
  rw [exec_store _ hin, wordAt_ea _ σ8_rdi σ8_r8, σ8_rcx, σ8_mem]
  rfl

/-! ## The cpu's test -/

theorem inRange_iff : cx.InRange s ↔ (cpu32 s).toNat < 8192 := by
  simp only [Spec.Slab.Ctx.InRange, cpu_of (c := cx) hX, cpu32, SLAB_COUNT]

theorem eval_cpu_true (hi : ¬ cx.InRange s) : eval .ae (σ2 s) = some true := by
  rw [eval_cpu, inRange_iff] at *; simp [hi]

theorem eval_cpu_false (hi : cx.InRange s) : eval .ae (σ2 s) = some false := by
  rw [eval_cpu, inRange_iff] at *; simp [hi]

theorem eval_test_true (ht : tst s = 0) : eval .e (σ5 s) = some true := by
  rw [eval_test, ht]; rfl

theorem eval_test_false (ht : ¬ tst s = 0) : eval .e (σ5 s) = some false := by
  rw [eval_test]; simpa using ht

end Steps

/-! ## The three ways out -/

section Cases

variable {s : State} {k : Nat}

/-- No slab for the CPU. -/
theorem runBody_C (hp : Pre s k) (hi : ¬ cx.InRange s) :
    runBody isa body (σA s) = some (.done (σC s)) := by
  rw [body, runBody_instr (exec_cpu hp.facts.toArm), runBody_instr exec_cmp,
    runBody_take (eval_cpu_true hi) exec_cold_cpu]

theorem prefixes_C (hp : Pre s k) (hi : ¬ cx.InRange s) :
    bodyPrefixes isa body (σA s) = [σA s, σ1 s, σ2 s] := by
  rw [body, bodyPrefixes_instr (exec_cpu hp.facts.toArm), bodyPrefixes_instr exec_cmp,
    bodyPrefixes_take (eval_cpu_true hi)]

theorem end_C (hp : Pre s k) (hi : ¬ cx.InRange s) : bodyEnd isa body (σA s) = some 2 := by
  rw [body, bodyEnd_instr (exec_cpu hp.facts.toArm), bodyEnd_instr exec_cmp,
    bodyEnd_take (eval_cpu_true hi)]
  rfl

/-- Nothing to pop. -/
theorem runBody_T (hp : Pre s k) (hi : cx.InRange s) (ht : tst s = 0) :
    runBody isa body (σA s) = some (.done (σT s)) := by
  rw [body, runBody_instr (exec_cpu hp.facts.toArm), runBody_instr exec_cmp,
    runBody_skip (eval_cpu_false hi), runBody_instr exec_shl_r8,
    runBody_instr (exec_wordAt hp.facts hi hp.word), runBody_instr exec_test,
    runBody_take (eval_test_true ht) exec_cold_test]

theorem prefixes_T (hp : Pre s k) (hi : cx.InRange s) (ht : tst s = 0) :
    bodyPrefixes isa body (σA s) = [σA s, σ1 s, σ2 s, σ2 s, σ3 s, σ4 s, σ5 s] := by
  rw [body, bodyPrefixes_instr (exec_cpu hp.facts.toArm), bodyPrefixes_instr exec_cmp,
    bodyPrefixes_skip (eval_cpu_false hi), bodyPrefixes_instr exec_shl_r8,
    bodyPrefixes_instr (exec_wordAt hp.facts hi hp.word),
    bodyPrefixes_instr exec_test, bodyPrefixes_take (eval_test_true ht)]

theorem end_T (hp : Pre s k) (hi : cx.InRange s) (ht : tst s = 0) :
    bodyEnd isa body (σA s) = some 6 := by
  rw [body, bodyEnd_instr (exec_cpu hp.facts.toArm), bodyEnd_instr exec_cmp,
    bodyEnd_skip (eval_cpu_false hi), bodyEnd_instr exec_shl_r8,
    bodyEnd_instr (exec_wordAt hp.facts hi hp.word), bodyEnd_instr exec_test,
    bodyEnd_take (eval_test_true ht)]
  rfl

/-- A pop. -/
theorem runBody_P (hp : Pre s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    runBody isa body (σA s) = some (.done (σ9 s)) := by
  rw [body, runBody_instr (exec_cpu hp.facts.toArm), runBody_instr exec_cmp,
    runBody_skip (eval_cpu_false hi), runBody_instr exec_shl_r8,
    runBody_instr (exec_wordAt hp.facts hi hp.word), runBody_instr exec_test,
    runBody_skip (eval_test_false ht), runBody_instr (exec_x hp hi ht), runBody_instr exec_lea8,
    runBody_instr (exec_commit hp hi), runBody_nil']

theorem prefixes_P (hp : Pre s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    bodyPrefixes isa body (σA s) =
      [σA s, σ1 s, σ2 s, σ2 s, σ3 s, σ4 s, σ5 s, σ5 s, σ7 s, σ8 s] := by
  rw [body, bodyPrefixes_instr (exec_cpu hp.facts.toArm), bodyPrefixes_instr exec_cmp,
    bodyPrefixes_skip (eval_cpu_false hi), bodyPrefixes_instr exec_shl_r8,
    bodyPrefixes_instr (exec_wordAt hp.facts hi hp.word),
    bodyPrefixes_instr exec_test, bodyPrefixes_skip (eval_test_false ht),
    bodyPrefixes_instr (exec_x hp hi ht), bodyPrefixes_instr exec_lea8,
    bodyPrefixes_last (exec_commit hp hi)]

theorem end_P (hp : Pre s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    bodyEnd isa body (σA s) = some 10 := by
  rw [body, bodyEnd_instr (exec_cpu hp.facts.toArm), bodyEnd_instr exec_cmp,
    bodyEnd_skip (eval_cpu_false hi), bodyEnd_instr exec_shl_r8,
    bodyEnd_instr (exec_wordAt hp.facts hi hp.word), bodyEnd_instr exec_test,
    bodyEnd_skip (eval_test_false ht), bodyEnd_instr (exec_x hp hi ht), bodyEnd_instr exec_lea8,
    bodyEnd_instr (exec_commit hp hi), bodyEnd_nil']
  rfl

/-! ### Memory accesses -/

theorem ev_cpu :
    Instr.events (.mov32 .r8 (.mem { base := .r10, disp := 4, seg := some .fs })) (σA s) (σ1 s) =
      [⟨area s + 4, 4, false, (cpu32 s).toNat⟩] := by
  simp only [Instr.events, cpuOp_ea, σA_mem, toNat_read4, m1]
  rw [readW_writeW_disj (cpu_cs_disjoint s) (by decide)]; rfl

theorem ev_x (hp : Pre s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    Instr.events (.mov .rax (.mem { base := .rcx, disp := -8 })) (σ5 s) (σ7 s) =
      [⟨wv s - 8, 8, false, (x s).toNat⟩] := by
  obtain ⟨-, hdj⟩ := below_facts hp hi ht
  simp only [Instr.events, below_ea _ σ5_rcx, σ5_mem, toNat_read8, m1]
  rw [readW64_writeW_disj hdj]; rfl

theorem ev_commit :
    Instr.events (.store (wordAt .rdi) .rcx) (σ8 s) (σ9 s) = [⟨wa s, 8, true, (wv s - 8).toNat⟩] := by
  simp only [Instr.events, wordAt_ea _ σ8_rdi σ8_r8, toNat_read8]
  rw [show (σ9 s).mem = mEnd s from rfl, mEnd, Mem.readW_writeW_self64]

theorem events_C (hp : Pre s k) (hi : ¬ cx.InRange s) :
    bodyEvents isa body (σA s) = [⟨area s + 4, 4, false, (cpu32 s).toNat⟩] := by
  rw [body, bodyEvents_instr (exec_cpu hp.facts.toArm), bodyEvents_instr exec_cmp,
    bodyEvents_take (eval_cpu_true hi)]
  simp only [isa, ev_cpu]; rfl

theorem events_T (hp : Pre s k) (hi : cx.InRange s) (ht : tst s = 0) :
    bodyEvents isa body (σA s) =
      [⟨area s + 4, 4, false, (cpu32 s).toNat⟩, ⟨wa s, 8, false, (wv s).toNat⟩] := by
  rw [body, bodyEvents_instr (exec_cpu hp.facts.toArm), bodyEvents_instr exec_cmp,
    bodyEvents_skip (eval_cpu_false hi), bodyEvents_instr exec_shl_r8,
    bodyEvents_instr (exec_wordAt hp.facts hi hp.word), bodyEvents_instr exec_test,
    bodyEvents_take (eval_test_true ht)]
  simp only [isa, ev_cpu, ev_wordAt hp.facts hi hp.word]; rfl

theorem events_P (hp : Pre s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    bodyEvents isa body (σA s) =
      [⟨area s + 4, 4, false, (cpu32 s).toNat⟩, ⟨wa s, 8, false, (wv s).toNat⟩,
        ⟨wv s - 8, 8, false, (x s).toNat⟩, ⟨wa s, 8, true, (wv s - 8).toNat⟩] := by
  rw [body, bodyEvents_instr (exec_cpu hp.facts.toArm), bodyEvents_instr exec_cmp,
    bodyEvents_skip (eval_cpu_false hi), bodyEvents_instr exec_shl_r8,
    bodyEvents_instr (exec_wordAt hp.facts hi hp.word), bodyEvents_instr exec_test,
    bodyEvents_skip (eval_test_false ht), bodyEvents_instr (exec_x hp hi ht),
    bodyEvents_instr exec_lea8, bodyEvents_instr (exec_commit hp hi), bodyEvents_nil']
  simp only [ev_cpu, ev_wordAt hp.facts hi hp.word, ev_x hp hi ht, ev_commit]; rfl

end Cases

/-! ## What each state keeps -/

section Mids

variable {s : State}

theorem mσ2 : Mid scratchI s (m1 s) (σ2 s) :=
  mid_σ2 (mid_σ1 (by decide) (mid_σA (by decide) (by decide)))
theorem mσ3 : Mid scratchI s (m1 s) (σ3 s) := mid_σ3 (by decide) mσ2
theorem mσ4' : Mid scratchI s (m1 s) (σ4 s) := mσ4 mσ3 (by decide)
theorem mσ5 : Mid scratchI s (m1 s) (σ5 s) := mσ4'.arithFlags _ _ _
theorem mσ7 : Mid scratchI s (m1 s) (σ7 s) := mσ5.setReg (by decide) _
theorem mσ8 : Mid scratchI s (m1 s) (σ8 s) := mσ7.setReg (by decide) _
theorem mσ9 : Mid scratchI s (mEnd s) (σ9 s) := mσ8.withMem _
theorem mσC : Mid scratchI s (m1 s) (σC s) := mσ2.setReg32 (by decide) _
theorem mσT : Mid scratchI s (m1 s) (σT s) := mσ5.setReg32 (by decide) _

end Mids

/-! ## The contract -/

section Contract

variable {s : State} {k : Nat}

theorem hv : cx.A.args ((cx.sig.words cx.A.ptrBits).map (·.bits cx.A.ptrBits)) =
    some (fun s => [s.gpr .rdi, s.gpr .rsi]) := rfl

theorem facts_of_pre (hs : (popInlineContract inlineAbi).pre s) : Facts cx s := by
  have := (pre_iff cx (s := s) hv rfl rfl).mp hs
  exact facts_of_world hX rfl this.2.1 this.2.2

theorem ofNat_toNat64 (v : BitVec 64) : BitVec.ofNat 64 v.toNat = v := by
  rw [BitVec.ofNat_toNat, BitVec.setWidth_eq]

/-- What a pop that passes the test leaves. -/
theorem popped (hp : Pre s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    cx.Popped s (σ9 s) (x s) := by
  obtain ⟨hs, h2, hpop⟩ := hit hp hi ht
  have hsl : IsSlot (cx.slots s) (cx.w s) := by rw [hp.slots, hp.w]; exact hs
  have hmag := magRegion_eq hp
  have hcm := hp.facts.cs_mag hi hsl
  have hwm := hp.facts.word_mag hi hsl
  rw [hmag] at hcm hwm
  refine ⟨.stack (magBase (2 ^ k) (wv s)) (items (2 ^ k) s.mem (wv s)).dropLast, ?_, ?_, ?_, ?_⟩
  · simp only [Spec.Slab.Ctx.popResult, hi, ↓reduceIte, Spec.Slab.Ctx.mag, hp.slots,
      mem_of (c := cx) hX, hp.w]
    exact hpop
  · rw [csField_of (c := cx) hX, ← hp.word]
    exact frame_two s.mem _ _ _ _
  · show (mEnd s).readW (cx.word s) 64 = _
    rw [mEnd, ← hp.word, Mem.readW_writeW_self64, hp.w]
  · show Mag.ofWord (cx.slots s) (mEnd s) (cx.w s - 8) = _
    rw [hp.slots, hp.w]
    refine ofWord_sub8 (Nat.two_pow_pos k) hs h2 fun i hi' => ?_
    have hsub := sub_item (n := 2 ^ k) (w := wv s) (i := i)
      (by have := slot_lt (2 ^ k) (wv s) (Nat.two_pow_pos k); omega)
    rw [mEnd, readW64_writeW_disj (by rw [hp.word]; exact (hwm.symm.sub_left hsub)), m1,
      readW64_writeW_disj (hcm.symm.sub_left hsub)]

theorem ret_σ9 : (σ9 s).gpr .rax = x s := by
  simp only [σ9, σ8, RegUpd.gpr_setReg_of_ne _ _ (show ¬ Reg.rax = .rcx by decide), σ7,
    RegUpd.gpr_setReg_self]

theorem ret_zero (σ : State) : (σ.setReg32 .rax 0).gpr .rax = 0 := by
  simp only [State.setReg32, RegUpd.gpr_setReg_self]; rfl

/-- A miss changes only `rseq_cs`. -/
theorem unchanged_cs : Unchanged [cx.csField s] (cx.mem s) (m1 s) := by
  rw [csField_of (c := cx) hX, mem_of (c := cx) hX]; exact frame_m1

theorem missed_C (hi : ¬ cx.InRange s) : cx.PopMissed s (σC s) := by
  refine ⟨?_, unchanged_cs⟩
  simp only [Spec.Slab.Ctx.popResult, hi, ↓reduceIte]

theorem missed_T (hp : Pre s k) (hi : cx.InRange s) (ht : tst s = 0) : cx.PopMissed s (σT s) := by
  refine ⟨?_, unchanged_cs⟩
  simp only [Spec.Slab.Ctx.popResult, hi, ↓reduceIte, Spec.Slab.Ctx.mag, hp.slots,
    mem_of (c := cx) hX, hp.w]
  exact (pop_eq_none_iff (dvd_marker hp.hk') (hp.ok hi)).mpr
    ((test_iff hp.hk hp.hk' (wv s) (s.gpr .rsi) hp.mask).mp ht)

theorem post_eq (s' : State) : (popInlineContract inlineAbi).post s s' ↔
    ((∃ v, cx.Popped s s' v ∧ s'.gpr .rax = v) ∨ (cx.PopMissed s s' ∧ s'.gpr .rax = 0)) :=
  post_iff cx hv s s'

theorem correct (hs : (popInlineContract inlineAbi).pre s) :
    (∃ s', Exec isa popInline s s' ∧ inlineTarget.abiPreserved s s' ∧
      (popInlineContract inlineAbi).post s s') ∨
    (∃ t s', ExecExit isa popInline s t s' ∧ inlineTarget.abiPreserved s s' ∧
      (popInlineContract inlineAbi).exit t s s') := by
  have h := facts_of_pre hs
  obtain ⟨k, hp⟩ := pre_of h
  refine .inl ?_
  rw [code_eq]
  by_cases hi : cx.InRange s
  · by_cases ht : tst s = 0
    · exact ⟨σT s, .rseq (execBlock_arm h.toArm) (runBody_T hp hi ht), inline_abi mσT,
        (post_eq _).mpr (.inr ⟨missed_T hp hi ht, ret_zero (σ5 s)⟩)⟩
    · exact ⟨σ9 s, .rseq (execBlock_arm h.toArm) (runBody_P hp hi ht), inline_abi mσ9,
        (post_eq _).mpr (.inl ⟨x s, popped hp hi ht, ret_σ9⟩)⟩
  · exact ⟨σC s, .rseq (execBlock_arm h.toArm) (runBody_C hp hi), inline_abi mσC,
      (post_eq _).mpr (.inr ⟨missed_C hi, ret_zero (σ2 s)⟩)⟩

/-! ### Where it may be aborted -/

theorem abortOk (h : Facts cx s) {p : State} (hm : Mid scratchI s (m1 s) p) :
    armed p ∧ inlineTarget.abiPreserved s p ∧ (popInlineContract inlineAbi).abort s p ∧
      ∀ p', inlineTarget.rely p p' → (popInlineContract inlineAbi).env p p' →
        (popInlineContract inlineAbi).pre p' := by
  obtain ⟨hip, hre⟩ := retry hX hv (post := fun s s' => _) (fun _ => rfl) (fun _ => rfl) (by decide)
    h.pre hm (by decide) (by decide) frame_m1 (harmless_cs1 hX h.pre)
  refine ⟨armed_of_mid h.toArm (xs := []) frame_m1 m1_cs (by simp) hm, inline_abi hm, ?_, hre⟩
  refine (abort_iff cx hv s p).mpr ⟨by rw [hm.gpr _ (by decide), hm.gpr _ (by decide)], ?_, hip⟩
  show Unchanged [cx.csField s] (cx.mem s) p.mem
  rw [hm.mem]; exact unchanged_cs

/-! ### Its accesses -/

theorem addr8 : s.fsBase + o s + BitVec.ofNat 64 RSEQ_CS = area s + 8 := rfl
theorem addr4 : s.fsBase + o s + BitVec.ofNat 64 RSEQ_CPU_ID = area s + 4 := rfl
theorem rsName : RSEQ_OFFSET = RS := rfl

theorem wordAddr_eq (hp : Pre s k) :
    wordAddr (s.syms SLABS) (cpu32 s).toNat (cx.cls s) = wa s := by
  rw [hp.word]; rfl

theorem test_eq : BitVec.ofNat 32 (wv s).toNat &&& BitVec.ofNat 32 (cx.testMask s) = tst s := by
  simp only [Spec.Slab.Ctx.testMask, BitVec.ofNat_toNat, tst]; rfl

theorem trace_ok (hs : (popInlineContract inlineAbi).pre s) :
    (popInlineContract inlineAbi).trace s (Code.events isa popInline s) := by
  have h := facts_of_pre hs
  obtain ⟨k, hp⟩ := pre_of h
  show cx.PopTrace s (Code.events isa (.rseq arm body) s)
  simp only [Code.events, execBlock_arm h.toArm, events_arm h.toArm]
  refine ⟨(o s).toNat, s.csDesc.toNat, (cpu32 s).toNat, (wv s).toNat, (x s).toNat, ?_⟩
  simp only [Spec.Slab.Ctx.headTrace, Spec.Slab.Ctx.rd, Spec.Slab.Ctx.wr, tp_of (c := cx) hX,
    sym_of (c := cx) hX, test_eq, ofNat_toNat64, addr8, addr4, rsName]
  by_cases hi : cx.InRange s
  · have hi' : (cpu32 s).toNat < SLAB_COUNT := (inRange_iff.mp hi)
    rw [wordAddr_eq hp]
    by_cases ht : tst s = 0
    · rw [events_T hp hi ht]; simp [hi', ht]
    · rw [events_P hp hi ht]; simpa [hi'] using ht
  · have hi' : ¬ (cpu32 s).toNat < SLAB_COUNT := fun h' => hi (inRange_iff.mpr h')
    rw [events_C hp hi]; simp [hi']

theorem rseqOk (hs : (popInlineContract inlineAbi).pre s) :
    RseqOk inlineTarget popInline (popInlineContract inlineAbi) s := by
  have h := facts_of_pre hs
  obtain ⟨k, hp⟩ := pre_of h
  have ht := trace_ok hs
  rw [code_eq] at ht ⊢
  unfold RseqOk
  refine ⟨ht, ⟨_, _, rfl, execBlock_dropLast_arm h.toArm, arms_arm⟩, ?_⟩
  have mA : Mid scratchI s (m1 s) (σA s) := mid_σA (by decide) (by decide)
  have m1' : Mid scratchI s (m1 s) (σ1 s) := mid_σ1 (by decide) mA
  simp only [Code.abortStates, execBlock_arm h.toArm]
  by_cases hi : cx.InRange s
  · by_cases ht : tst s = 0
    · rw [prefixes_T hp hi ht]
      simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
      exact ⟨abortOk h mA, abortOk h m1', abortOk h mσ2, abortOk h mσ2, abortOk h mσ3,
        abortOk h mσ4', abortOk h mσ5⟩
    · rw [prefixes_P hp hi ht]
      simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
      exact ⟨abortOk h mA, abortOk h m1', abortOk h mσ2, abortOk h mσ2, abortOk h mσ3,
        abortOk h mσ4', abortOk h mσ5, abortOk h mσ5, abortOk h mσ7, abortOk h mσ8⟩
  · rw [prefixes_C hp hi]
    simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
    exact ⟨abortOk h mA, abortOk h m1', abortOk h mσ2⟩

/-! ### The registers `asm!` must find kept -/

/-- The registers the code changes: `rax` is also its result. -/
def clobbers : List String := ["rax", "rcx", "r8", "r10", "r11"]

theorem names : ∀ r ∈ scratchI, r.name ∈ clobbers := by decide

theorem clobbersOk (hs : (popInlineContract inlineAbi).pre s) :
    (∀ s', Exec isa popInline s s' →
      preservedExcept (clobbers ++ (if popInlineSig.ret.isSome then ["rax"] else [])) s s') ∧
    (∀ t s', ExecExit isa popInline s t s' → preservedExcept clobbers s s') ∧
    (∀ p ∈ popInline.abortStates isa s, preservedExcept clobbers s p) := by
  have h := facts_of_pre hs
  obtain ⟨k, hp⟩ := pre_of h
  have names' : ∀ r ∈ scratchI, r.name ∈ clobbers ++ (if popInlineSig.ret.isSome then ["rax"] else []) :=
    by decide
  rw [code_eq]
  have mA : Mid scratchI s (m1 s) (σA s) := mid_σA (by decide) (by decide)
  have m1' : Mid scratchI s (m1 s) (σ1 s) := mid_σ1 (by decide) mA
  refine ⟨fun s' he => ?_, fun t s' he => ?_, fun p hpm => ?_⟩
  · cases he with
    | rseq ha hb =>
      rw [execBlock_arm h.toArm] at ha; cases ha
      by_cases hi : cx.InRange s
      · by_cases ht : tst s = 0
        · rw [runBody_T hp hi ht] at hb; cases hb; exact preservedExcept_of_mid names' mσT
        · rw [runBody_P hp hi ht] at hb; cases hb; exact preservedExcept_of_mid names' mσ9
      · rw [runBody_C hp hi] at hb; cases hb; exact preservedExcept_of_mid names' mσC
  · cases he with
    | rseq ha hb =>
      rw [execBlock_arm h.toArm] at ha; cases ha
      by_cases hi : cx.InRange s
      · by_cases ht : tst s = 0
        · rw [runBody_T hp hi ht] at hb; cases hb
        · rw [runBody_P hp hi ht] at hb; cases hb
      · rw [runBody_C hp hi] at hb; cases hb
  · simp only [Code.abortStates, execBlock_arm h.toArm] at hpm
    by_cases hi : cx.InRange s
    · by_cases ht : tst s = 0
      · rw [prefixes_T hp hi ht] at hpm
        simp only [List.mem_cons, List.not_mem_nil, or_false] at hpm
        rcases hpm with rfl | rfl | rfl | rfl | rfl | rfl | rfl <;>
          first
          | exact preservedExcept_of_mid names mA | exact preservedExcept_of_mid names m1'
          | exact preservedExcept_of_mid names mσ2 | exact preservedExcept_of_mid names mσ3
          | exact preservedExcept_of_mid names mσ4' | exact preservedExcept_of_mid names mσ5
      · rw [prefixes_P hp hi ht] at hpm
        simp only [List.mem_cons, List.not_mem_nil, or_false] at hpm
        rcases hpm with rfl | rfl | rfl | rfl | rfl | rfl | rfl | rfl | rfl | rfl <;>
          first
          | exact preservedExcept_of_mid names mA | exact preservedExcept_of_mid names m1'
          | exact preservedExcept_of_mid names mσ2 | exact preservedExcept_of_mid names mσ3
          | exact preservedExcept_of_mid names mσ4' | exact preservedExcept_of_mid names mσ5
          | exact preservedExcept_of_mid names mσ7 | exact preservedExcept_of_mid names mσ8
    · rw [prefixes_C hp hi] at hpm
      simp only [List.mem_cons, List.not_mem_nil, or_false] at hpm
      rcases hpm with rfl | rfl | rfl <;>
        first
        | exact preservedExcept_of_mid names mA | exact preservedExcept_of_mid names m1'
        | exact preservedExcept_of_mid names mσ2

end Contract

end VG.Proof.Slab.X86_64.PopInline
