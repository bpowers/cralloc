module

public import Cralloc.Proof.Slab.AArch64.Word

/-!
# The inline pop on AArch64

`slab_pop` inline (`Impl.Slab.AArch64.popInline`) against
`Spec.Slab.popInlineContract`: its three ways out (no slab for the CPU; no pointer to
pop; the commit), and in each its run, where it may be aborted, and its memory accesses.
The class's word in slab 0 is at `x3` and the mask in `x2`.
-/

@[expose] public section

namespace VG.Proof.Slab.AArch64.PopInline

open VG.AArch64 VG.Spec.Slab VG.Impl.Slab.AArch64 VG.Proof.Slab.AArch64

/-- The routine as the contract sees it. -/
abbrev cx : Spec.Slab.Ctx isa := ⟨inlineAbi, .inline, popInlineSig⟩

theorem hX : A64 cx.A inlineArgRegs := a64_inline

abbrev contract : Contract isa := popInlineContract inlineAbi

/-- The contract of an attempt. -/
abbrev attempt : Contract isa := withReady contract

def body : List (Step Instr Cond) := [
  .instr (.mov .x .x0 (.imm 0)),
  .instr (.ldr .w .x8 { base := .x10, off := .imm 4 }),
  .instr (.cmp .w .x8 (.imm 8192)),
  .exit .hs [] none,
  .instr (.lsl .x .x8 .x8 12),
  .instr (.ldr .x .x1 wordAt),
  .instr (.tst .w .x1 (.reg .x2)),
  .exit .eq [] none,
  .instr (.ldr .x .x0 { base := .x1, off := .imm (-8) }),
  .instr (.sub .x .x1 .x1 (.imm 8)),
  .instr (.str .x .x1 wordAt)]

theorem code_eq : popInline = .rseq arm body := rfl

/-! ## The states -/

/-- A miss's result. -/
def σ0 (s : State) : State := (σA s).setReg .x0 0
abbrev σ1 (s : State) : State := cpu1 (σ0 s) s
abbrev σ2 (s : State) : State := cpu2 (σ0 s) s
abbrev σ3 (s : State) : State := cpu3 (σ0 s) s
/-- After loading the slab word. -/
def σ4 (s : State) : State := (σ3 s).setReg .x1 (wv s)
/-- The mask test. -/
def tst (s : State) : BitVec 32 := (wv s).setWidth 32 &&& (s.gpr .x2).setWidth 32
def σ5 (s : State) : State := Flags.tstState (σ4 s) (tst s)
/-- The pointer popped. -/
def x (s : State) : BitVec 64 := s.mem.readW (wv s - 8) 64
def σ7 (s : State) : State := (σ5 s).setReg .x0 (x s)
def σ8 (s : State) : State := (σ7 s).setReg .x1 (wv s - 8)
/-- Memory after the commit. -/
def mEnd (s : State) : Mem := (m1 s).writeW (wa s) (wv s - 8)
def σ9 (s : State) : State := { σ8 s with mem := mEnd s }

/-! ## What the precondition gives -/

/-- The facts of the entry state, and what the arguments make of the class and mask. -/
structure Pre (s : State) (k : Nat) : Prop where
  facts : Facts cx s
  hk : 2 ≤ k
  hk' : k ≤ 16
  mask : (s.gpr .x2).toNat = magMask (2 ^ k)
  slots : cx.slots s = 2 ^ k
  word : wa s = cx.word s
  w : cx.w s = wv s
  ok : cx.InRange s → WordOk (2 ^ k) (wv s)

theorem arg0 (s : State) : cx.arg s 0 = s.gpr .x3 := rfl

theorem pre_of {s : State} (h : Facts cx s) : ∃ k, Pre s k := by
  obtain ⟨-, -, ⟨hoff, -, ⟨k, hk, hk', hm⟩⟩, -, hinv⟩ := h.pre
  have hmask : cx.mask s = (s.gpr .x2).toNat := rfl
  have hslots : cx.slots s = 2 ^ k := by
    rw [Spec.Slab.Ctx.slots, hm]; exact slotsOf_magMask k hk
  have hw : wa s = cx.word s := wa_eq hX (arg0 s) hoff
  have hwv : cx.w s = wv s := by simp only [Spec.Slab.Ctx.w, mem_of (c := cx) hX, wv, hw]
  refine ⟨k, h, hk, hk', by rw [← hmask]; exact hm, hslots, hw, hwv, fun hi => ?_⟩
  have := (hinv.1 (cx.cpu s) hi).1
  rw [← hslots, ← hwv]
  exact this

/-! ## The steps -/

section Steps

variable {s : State} {k : Nat}

theorem exec_x0 : exec (.mov .x .x0 (.imm 0)) (σA s) = some (σ0 s) := rfl

theorem σ0_gpr {r : Reg} (h0 : ¬ r = .x0) : (σ0 s).gpr r = s.gpr r := by
  rw [σ0, RegUpd.gpr_setReg_of_ne _ _ h0]; rfl

theorem base (hr : ready s) : Base s (σ0 s) := ⟨rfl, rfl, rfl, (σ0_gpr (by decide)).trans hr.1⟩

theorem σ3_gpr {r : Reg} (h0 : ¬ r = .x0) (h8 : ¬ r = .x8) : (σ3 s).gpr r = s.gpr r :=
  (gpr_cpu3 h8).trans (σ0_gpr h0)

theorem σ4_x1 : (σ4 s).gpr .x1 = wv s := RegUpd.gpr_setReg_self _ _ _

theorem σ4_gpr {r : Reg} (h0 : ¬ r = .x0) (h1 : ¬ r = .x1) (h8 : ¬ r = .x8) :
    (σ4 s).gpr r = s.gpr r := by
  rw [σ4, RegUpd.gpr_setReg_of_ne _ _ h1, σ3_gpr h0 h8]

theorem exec_word (hp : Pre s k) (hi : cx.InRange s) :
    exec (.ldr .x .x1 wordAt) (σ3 s) = some (σ4 s) :=
  exec_wordAt hp.facts hi hp.word (base hp.facts.ready) (σ0_gpr (by decide))

theorem exec_test : exec (.tst .w .x1 (.reg .x2)) (σ4 s) = some (σ5 s) := by
  show some (Flags.tstState (σ4 s) (((σ4 s).gpr .x1).setWidth 32 &&& ((σ4 s).gpr .x2).setWidth 32)) = _
  rw [σ4_x1, σ4_gpr (by decide) (by decide) (by decide)]
  rfl

theorem eval_test : eval .eq (σ5 s) = some (tst s == 0) := rfl

/-- What a pop that passes the test pops. -/
theorem hit (hp : Pre s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    IsSlot (2 ^ k) (wv s) ∧ 2 ≤ slot (2 ^ k) (wv s) ∧
      (Mag.ofWord (2 ^ k) s.mem (wv s)).pop =
        some (x s, .stack (magBase (2 ^ k) (wv s)) (items (2 ^ k) s.mem (wv s)).dropLast) := by
  have hm : ¬ (wv s).toNat % (8 * 2 ^ k) < 16 := fun h' =>
    ht ((test_iff hp.hk hp.hk' (wv s) (s.gpr .x2) hp.mask).mpr h')
  exact pop_of_test (dvd_marker hp.hk') (hp.ok hi) hm

/-- The magazine's region, in the routine's terms. -/
theorem magRegion_eq (hp : Pre s k) : cx.magRegion s = ⟨magBase (2 ^ k) (wv s), 8 * 2 ^ k⟩ := by
  simp only [Spec.Slab.Ctx.magRegion, hp.slots, hp.w]

theorem σ5_rd : (σ5 s).rd = s.rd := rfl
theorem σ5_wr : (σ5 s).wr = s.wr := rfl
theorem σ5_mem : (σ5 s).mem = m1 s := rfl
theorem σ5_gpr (r : Reg) : (σ5 s).gpr r = (σ4 s).gpr r := by
  rw [σ5, Flags.tstState, RegUpd.gpr_setFlags]
theorem σ5_x1 : (σ5 s).gpr .x1 = wv s := by rw [σ5_gpr, σ4_x1]

theorem below_ea (σ : State) (h : σ.gpr .x1 = wv s) :
    σ.ea { base := .x1, off := .imm (-8) } = wv s - 8 := by
  rw [ea_below, h]

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
    exec (.ldr .x .x0 { base := .x1, off := .imm (-8) }) (σ5 s) = some (σ7 s) := by
  obtain ⟨hin, hdj⟩ := below_facts hp hi ht
  have hin' : InRegions ((σ5 s).rd ++ (σ5 s).wr) ((σ5 s).ea { base := .x1, off := .imm (-8) }) 8 := by
    rw [below_ea _ σ5_x1, σ5_rd, σ5_wr]; exact hin
  rw [Access.exec_ldr_x hin', below_ea _ σ5_x1, σ5_mem, m1, readW64_writeW_disj hdj]
  rfl

theorem σ7_x1 : (σ7 s).gpr .x1 = wv s := by
  rw [σ7, RegUpd.gpr_setReg_of_ne _ _ (by decide), σ5_x1]

theorem exec_sub8 : exec (.sub .x .x1 .x1 (.imm 8)) (σ7 s) = some (σ8 s) := by
  show some ((σ7 s).setReg .x1 ((σ7 s).gpr .x1 - BitVec.ofNat 64 8)) = _
  rw [σ7_x1]
  rfl

theorem σ8_x3 : (σ8 s).gpr .x3 = s.gpr .x3 := by
  rw [σ8, RegUpd.gpr_setReg_of_ne _ _ (by decide), σ7, RegUpd.gpr_setReg_of_ne _ _ (by decide),
    σ5_gpr, σ4_gpr (by decide) (by decide) (by decide)]

theorem σ8_x8 : (σ8 s).gpr .x8 = sh s := by
  rw [σ8, RegUpd.gpr_setReg_of_ne _ _ (by decide), σ7, RegUpd.gpr_setReg_of_ne _ _ (by decide),
    σ5_gpr, σ4, RegUpd.gpr_setReg_of_ne _ _ (by decide), cpu3_x8]

theorem σ8_x1 : (σ8 s).gpr .x1 = wv s - 8 := RegUpd.gpr_setReg_self _ _ _
theorem σ8_mem : (σ8 s).mem = m1 s := rfl
theorem σ8_wr : (σ8 s).wr = s.wr := rfl

theorem exec_commit (hp : Pre s k) (hi : cx.InRange s) :
    exec (.str .x .x1 wordAt) (σ8 s) = some (σ9 s) := by
  have hin : InRegions (σ8 s).wr ((σ8 s).ea wordAt) 8 := by
    rw [wordAt_ea _ σ8_x3 σ8_x8, σ8_wr, hp.word]
    exact inWr (r := ⟨cx.word s, 8⟩) (hp.facts.word hi)
  rw [Access.exec_str_x hin, wordAt_ea _ σ8_x3 σ8_x8, σ8_x1, σ8_mem]
  rfl

/-! ## The tests -/

theorem inRange_iff : cx.InRange s ↔ (cpu32 s).toNat < 8192 := by
  simp only [Spec.Slab.Ctx.InRange, cpu_of (c := cx) hX, cpu32, SLAB_COUNT]

theorem eval_cpu_true (hi : ¬ cx.InRange s) : eval .hs (σ2 s) = some true := by
  rw [eval_cpu]
  have : ¬ (cpu32 s).toNat < 8192 := fun h => hi (inRange_iff.mpr h)
  simp only [Option.some.injEq, decide_eq_true_eq]
  omega

theorem eval_cpu_false (hi : cx.InRange s) : eval .hs (σ2 s) = some false := by
  rw [eval_cpu]
  have := inRange_iff.mp hi
  simp only [Option.some.injEq, decide_eq_false_iff_not]
  omega

theorem eval_test_true (ht : tst s = 0) : eval .eq (σ5 s) = some true := by
  rw [eval_test, ht]; rfl

theorem eval_test_false (ht : ¬ tst s = 0) : eval .eq (σ5 s) = some false := by
  rw [eval_test]; simpa using ht

end Steps

/-! ## The three ways out -/

section Cases

variable {s : State} {k : Nat}

/-- No slab for the CPU. -/
theorem runBody_C (hp : Pre s k) (hi : ¬ cx.InRange s) :
    runBody isa body (σA s) = some (.done (σ2 s)) := by
  rw [body, runBody_instr exec_x0, runBody_instr (exec_cpu hp.facts.toArm (base hp.facts.ready)),
    runBody_instr exec_cmp, runBody_take (eval_cpu_true hi) execBlock_nil']

theorem prefixes_C (hp : Pre s k) (hi : ¬ cx.InRange s) :
    bodyPrefixes isa body (σA s) = [σA s, σ0 s, σ1 s, σ2 s] := by
  rw [body, bodyPrefixes_instr exec_x0,
    bodyPrefixes_instr (exec_cpu hp.facts.toArm (base hp.facts.ready)), bodyPrefixes_instr exec_cmp,
    bodyPrefixes_take (eval_cpu_true hi)]

theorem end_C (hp : Pre s k) (hi : ¬ cx.InRange s) : bodyEnd isa body (σA s) = some 3 := by
  rw [body, bodyEnd_instr exec_x0, bodyEnd_instr (exec_cpu hp.facts.toArm (base hp.facts.ready)),
    bodyEnd_instr exec_cmp, bodyEnd_take (eval_cpu_true hi)]
  rfl

/-- Nothing to pop. -/
theorem runBody_T (hp : Pre s k) (hi : cx.InRange s) (ht : tst s = 0) :
    runBody isa body (σA s) = some (.done (σ5 s)) := by
  rw [body, runBody_instr exec_x0, runBody_instr (exec_cpu hp.facts.toArm (base hp.facts.ready)),
    runBody_instr exec_cmp, runBody_skip (eval_cpu_false hi), runBody_instr exec_lsl,
    runBody_instr (exec_word hp hi), runBody_instr exec_test,
    runBody_take (eval_test_true ht) execBlock_nil']

theorem prefixes_T (hp : Pre s k) (hi : cx.InRange s) (ht : tst s = 0) :
    bodyPrefixes isa body (σA s) = [σA s, σ0 s, σ1 s, σ2 s, σ2 s, σ3 s, σ4 s, σ5 s] := by
  rw [body, bodyPrefixes_instr exec_x0,
    bodyPrefixes_instr (exec_cpu hp.facts.toArm (base hp.facts.ready)), bodyPrefixes_instr exec_cmp,
    bodyPrefixes_skip (eval_cpu_false hi), bodyPrefixes_instr exec_lsl,
    bodyPrefixes_instr (exec_word hp hi), bodyPrefixes_instr exec_test,
    bodyPrefixes_take (eval_test_true ht)]

theorem end_T (hp : Pre s k) (hi : cx.InRange s) (ht : tst s = 0) :
    bodyEnd isa body (σA s) = some 7 := by
  rw [body, bodyEnd_instr exec_x0, bodyEnd_instr (exec_cpu hp.facts.toArm (base hp.facts.ready)),
    bodyEnd_instr exec_cmp, bodyEnd_skip (eval_cpu_false hi), bodyEnd_instr exec_lsl,
    bodyEnd_instr (exec_word hp hi), bodyEnd_instr exec_test, bodyEnd_take (eval_test_true ht)]
  rfl

/-- A pop. -/
theorem runBody_P (hp : Pre s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    runBody isa body (σA s) = some (.done (σ9 s)) := by
  rw [body, runBody_instr exec_x0, runBody_instr (exec_cpu hp.facts.toArm (base hp.facts.ready)),
    runBody_instr exec_cmp, runBody_skip (eval_cpu_false hi), runBody_instr exec_lsl,
    runBody_instr (exec_word hp hi), runBody_instr exec_test, runBody_skip (eval_test_false ht),
    runBody_instr (exec_x hp hi ht), runBody_instr exec_sub8, runBody_instr (exec_commit hp hi),
    runBody_nil']

theorem prefixes_P (hp : Pre s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    bodyPrefixes isa body (σA s) =
      [σA s, σ0 s, σ1 s, σ2 s, σ2 s, σ3 s, σ4 s, σ5 s, σ5 s, σ7 s, σ8 s] := by
  rw [body, bodyPrefixes_instr exec_x0,
    bodyPrefixes_instr (exec_cpu hp.facts.toArm (base hp.facts.ready)), bodyPrefixes_instr exec_cmp,
    bodyPrefixes_skip (eval_cpu_false hi), bodyPrefixes_instr exec_lsl,
    bodyPrefixes_instr (exec_word hp hi), bodyPrefixes_instr exec_test,
    bodyPrefixes_skip (eval_test_false ht), bodyPrefixes_instr (exec_x hp hi ht),
    bodyPrefixes_instr exec_sub8, bodyPrefixes_last (exec_commit hp hi)]

theorem end_P (hp : Pre s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    bodyEnd isa body (σA s) = some 11 := by
  rw [body, bodyEnd_instr exec_x0, bodyEnd_instr (exec_cpu hp.facts.toArm (base hp.facts.ready)),
    bodyEnd_instr exec_cmp, bodyEnd_skip (eval_cpu_false hi), bodyEnd_instr exec_lsl,
    bodyEnd_instr (exec_word hp hi), bodyEnd_instr exec_test, bodyEnd_skip (eval_test_false ht),
    bodyEnd_instr (exec_x hp hi ht), bodyEnd_instr exec_sub8, bodyEnd_instr (exec_commit hp hi),
    bodyEnd_nil']
  rfl

/-! ### Memory accesses -/

theorem ev_word (hp : Pre s k) (hi : cx.InRange s) :
    Instr.events (.ldr .x .x1 wordAt) (σ3 s) (σ4 s) = [⟨wa s, 8, false, (wv s).toNat⟩] :=
  ev_wordAt hp.facts hi hp.word (base hp.facts.ready) (σ0_gpr (by decide))

theorem ev_x (hp : Pre s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    Instr.events (.ldr .x .x0 { base := .x1, off := .imm (-8) }) (σ5 s) (σ7 s) =
      [⟨wv s - 8, 8, false, (x s).toNat⟩] := by
  obtain ⟨-, hdj⟩ := below_facts hp hi ht
  simp only [Instr.events, below_ea _ σ5_x1, Width.bytes, σ5_mem, toNat_read8, m1]
  rw [readW64_writeW_disj hdj]; rfl

theorem ev_commit :
    Instr.events (.str .x .x1 wordAt) (σ8 s) (σ9 s) = [⟨wa s, 8, true, (wv s - 8).toNat⟩] := by
  simp only [Instr.events, wordAt_ea _ σ8_x3 σ8_x8, Width.bytes, toNat_read8]
  rw [show (σ9 s).mem = mEnd s from rfl, mEnd, Mem.readW_writeW_self64]

theorem events_C (hp : Pre s k) (hi : ¬ cx.InRange s) :
    bodyEvents isa body (σA s) = [⟨area s + 4, 4, false, (cpu32 s).toNat⟩] := by
  rw [body, bodyEvents_instr exec_x0, bodyEvents_instr (exec_cpu hp.facts.toArm (base hp.facts.ready)),
    bodyEvents_instr exec_cmp, bodyEvents_take (eval_cpu_true hi)]
  simp only [isa, ev_cpu (base hp.facts.ready)]; rfl

theorem events_T (hp : Pre s k) (hi : cx.InRange s) (ht : tst s = 0) :
    bodyEvents isa body (σA s) =
      [⟨area s + 4, 4, false, (cpu32 s).toNat⟩, ⟨wa s, 8, false, (wv s).toNat⟩] := by
  rw [body, bodyEvents_instr exec_x0, bodyEvents_instr (exec_cpu hp.facts.toArm (base hp.facts.ready)),
    bodyEvents_instr exec_cmp, bodyEvents_skip (eval_cpu_false hi), bodyEvents_instr exec_lsl,
    bodyEvents_instr (exec_word hp hi), bodyEvents_instr exec_test,
    bodyEvents_take (eval_test_true ht)]
  simp only [isa, ev_cpu (base hp.facts.ready), ev_word hp hi]; rfl

theorem events_P (hp : Pre s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    bodyEvents isa body (σA s) =
      [⟨area s + 4, 4, false, (cpu32 s).toNat⟩, ⟨wa s, 8, false, (wv s).toNat⟩,
        ⟨wv s - 8, 8, false, (x s).toNat⟩, ⟨wa s, 8, true, (wv s - 8).toNat⟩] := by
  rw [body, bodyEvents_instr exec_x0, bodyEvents_instr (exec_cpu hp.facts.toArm (base hp.facts.ready)),
    bodyEvents_instr exec_cmp, bodyEvents_skip (eval_cpu_false hi), bodyEvents_instr exec_lsl,
    bodyEvents_instr (exec_word hp hi), bodyEvents_instr exec_test,
    bodyEvents_skip (eval_test_false ht), bodyEvents_instr (exec_x hp hi ht),
    bodyEvents_instr exec_sub8, bodyEvents_instr (exec_commit hp hi), bodyEvents_nil']
  simp only [ev_cpu (base hp.facts.ready), ev_word hp hi, ev_x hp hi ht, ev_commit]; rfl

end Cases

/-! ## What each state keeps -/

/-- The registers an attempt writes. -/
def scratch : List Reg := [.x0, .x1, .x8]

section Mids

variable {s : State}

theorem mσA : Mid scratch s (m1 s) (σA s) := mid_σA
theorem mσ0 : Mid scratch s (m1 s) (σ0 s) := mσA.setReg (by decide) _
theorem mσ1 : Mid scratch s (m1 s) (σ1 s) := mσ0.cpu1 (by decide)
theorem mσ2 : Mid scratch s (m1 s) (σ2 s) := mσ0.cpu2 (by decide)
theorem mσ3 : Mid scratch s (m1 s) (σ3 s) := mσ0.cpu3 (by decide)
theorem mσ4 : Mid scratch s (m1 s) (σ4 s) := mσ3.setReg (by decide) _
theorem mσ5 : Mid scratch s (m1 s) (σ5 s) := mσ4.setFlags _ _ _ _
theorem mσ7 : Mid scratch s (m1 s) (σ7 s) := mσ5.setReg (by decide) _
theorem mσ8 : Mid scratch s (m1 s) (σ8 s) := mσ7.setReg (by decide) _
theorem mσ9 : Mid scratch s (mEnd s) (σ9 s) := mσ8.withMem _

end Mids

/-! ## The contract -/

section Contract

variable {s : State} {k : Nat}

theorem hv : cx.A.args ((cx.sig.words cx.A.ptrBits).map (·.bits cx.A.ptrBits)) =
    some (fun s => [s.gpr .x3, s.gpr .x2]) := rfl

theorem facts_of_pre (hs : attempt.pre s) : Facts cx s := by
  have := (pre_iff cx (s := s) hv rfl rfl).mp hs.1
  exact facts_of_world hX rfl this.2.1 this.2.2 hs.2

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

theorem ret_σ9 : (σ9 s).gpr .x0 = x s := by
  show (σ8 s).gpr .x0 = _
  rw [σ8, RegUpd.gpr_setReg_of_ne _ _ (by decide), σ7, RegUpd.gpr_setReg_self]

theorem ret_σ2 : (σ2 s).gpr .x0 = 0 := by
  rw [gpr_cpu2 (by decide), σ0, RegUpd.gpr_setReg_self]

theorem ret_σ5 : (σ5 s).gpr .x0 = 0 := by
  rw [σ5_gpr, σ4, RegUpd.gpr_setReg_of_ne _ _ (by decide), gpr_cpu3 (by decide), σ0,
    RegUpd.gpr_setReg_self]

/-- A miss changes only `rseq_cs`. -/
theorem unchanged_cs : Unchanged [cx.csField s] (cx.mem s) (m1 s) := by
  rw [csField_of (c := cx) hX, mem_of (c := cx) hX]; exact frame_m1

theorem missed_C (hi : ¬ cx.InRange s) : cx.PopMissed s (σ2 s) := by
  refine ⟨?_, unchanged_cs⟩
  simp only [Spec.Slab.Ctx.popResult, hi, ↓reduceIte]

theorem missed_T (hp : Pre s k) (hi : cx.InRange s) (ht : tst s = 0) : cx.PopMissed s (σ5 s) := by
  refine ⟨?_, unchanged_cs⟩
  simp only [Spec.Slab.Ctx.popResult, hi, ↓reduceIte, Spec.Slab.Ctx.mag, hp.slots,
    mem_of (c := cx) hX, hp.w]
  exact (pop_eq_none_iff (dvd_marker hp.hk') (hp.ok hi)).mpr
    ((test_iff hp.hk hp.hk' (wv s) (s.gpr .x2) hp.mask).mp ht)

theorem post_eq (s' : State) : attempt.post s s' ↔
    ((∃ v, cx.Popped s s' v ∧ s'.gpr .x0 = v) ∨ (cx.PopMissed s s' ∧ s'.gpr .x0 = 0)) :=
  post_iff cx hv s s'

theorem correct (hs : attempt.pre s) :
    (∃ s', Exec isa popInline s s' ∧ inlineTarget.abiPreserved s s' ∧ attempt.post s s') ∨
    (∃ t s', ExecExit isa popInline s t s' ∧ inlineTarget.abiPreserved s s' ∧ attempt.exit t s s') := by
  have h := facts_of_pre hs
  obtain ⟨k, hp⟩ := pre_of h
  refine .inl ?_
  rw [code_eq]
  by_cases hi : cx.InRange s
  · by_cases ht : tst s = 0
    · exact ⟨σ5 s, .rseq (execBlock_arm h.toArm) (runBody_T hp hi ht), mσ5.sp,
        (post_eq _).mpr (.inr ⟨missed_T hp hi ht, ret_σ5⟩)⟩
    · exact ⟨σ9 s, .rseq (execBlock_arm h.toArm) (runBody_P hp hi ht), mσ9.sp,
        (post_eq _).mpr (.inl ⟨x s, popped hp hi ht, ret_σ9⟩)⟩
  · exact ⟨σ2 s, .rseq (execBlock_arm h.toArm) (runBody_C hp hi), mσ2.sp,
      (post_eq _).mpr (.inr ⟨missed_C hi, ret_σ2⟩)⟩

/-! ### Where it may be aborted -/

theorem abortOk (h : Facts cx s) {p : State} (hm : Mid scratch s (m1 s) p) :
    armed p ∧ inlineTarget.abiPreserved s p ∧ attempt.abort s p ∧
      ∀ p', inlineTarget.rely p p' → attempt.env p p' → attempt.pre p' := by
  obtain ⟨hip, hre⟩ := retry hX hv (post := fun s s' => _) (fun _ => rfl) (fun _ => rfl) (by decide)
    h.pre h.ready hm (by decide) (by decide) (by decide) frame_m1 (harmless_cs1 hX h.pre)
  refine ⟨armed_of_mid h.toArm (xs := []) frame_m1 m1_cs (by simp) hm, hm.sp, ?_, hre⟩
  refine (abort_iff cx hv s p).mpr ⟨by rw [hm.gpr _ (by decide), hm.gpr _ (by decide)], ?_, hip⟩
  show Unchanged [cx.csField s] (cx.mem s) p.mem
  rw [hm.mem]; exact unchanged_cs

/-! ### Its accesses -/

theorem addr8 : s.tpidr + o s + BitVec.ofNat 64 RSEQ_CS = area s + 8 := rfl
theorem addr4 : s.tpidr + o s + BitVec.ofNat 64 RSEQ_CPU_ID = area s + 4 := rfl
theorem rsName : RSEQ_OFFSET = RS := rfl

theorem wordAddr_eq (hp : Pre s k) :
    wordAddr (s.syms SLABS) (cpu32 s).toNat (cx.cls s) = wa s := by
  rw [hp.word]; rfl

theorem test_eq : BitVec.ofNat 32 (wv s).toNat &&& BitVec.ofNat 32 (cx.testMask s) = tst s := by
  simp only [Spec.Slab.Ctx.testMask, BitVec.ofNat_toNat, tst]; rfl

theorem trace_ok (hs : attempt.pre s) : attempt.trace s (Code.events isa popInline s) := by
  have h := facts_of_pre hs
  obtain ⟨k, hp⟩ := pre_of h
  show cx.PopTrace s (cx.attemptEvents s (Code.events isa (.rseq arm body) s))
  simp only [Code.events, execBlock_arm h.toArm, events_arm h.toArm, attemptEvents_eq (c := cx) hX]
  refine ⟨(o s).toNat, s.csDesc.toNat, (cpu32 s).toNat, (wv s).toNat, (x s).toNat, ?_⟩
  simp only [Spec.Slab.Ctx.headTrace, Spec.Slab.Ctx.rd, Spec.Slab.Ctx.wr, tp_of (c := cx) hX,
    sym_of (c := cx) hX, test_eq, ofNat_toNat64, addr8, addr4, rsName]
  by_cases hi : cx.InRange s
  · have hi' : (cpu32 s).toNat < SLAB_COUNT := (inRange_iff.mp hi)
    rw [wordAddr_eq hp]
    by_cases ht : tst s = 0
    · rw [events_T hp hi ht]; simp [hi', ht, o]
    · rw [events_P hp hi ht]; simpa [hi', o] using ht
  · have hi' : ¬ (cpu32 s).toNat < SLAB_COUNT := fun h' => hi (inRange_iff.mpr h')
    rw [events_C hp hi]; simp [hi', o]

/-- Wherever it may be aborted, memory is as after arming. -/
theorem mids (hp : Pre s k) : ∀ p ∈ Code.abortStates isa (.rseq arm body) s, Mid scratch s (m1 s) p := by
  simp only [Code.abortStates, execBlock_arm hp.facts.toArm]
  by_cases hi : cx.InRange s
  · by_cases ht : tst s = 0
    · rw [prefixes_T hp hi ht]
      simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
      exact ⟨mσA, mσ0, mσ1, mσ2, mσ2, mσ3, mσ4, mσ5⟩
    · rw [prefixes_P hp hi ht]
      simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
      exact ⟨mσA, mσ0, mσ1, mσ2, mσ2, mσ3, mσ4, mσ5, mσ5, mσ7, mσ8⟩
  · rw [prefixes_C hp hi]
    simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
    exact ⟨mσA, mσ0, mσ1, mσ2⟩

theorem rseqOk (hs : attempt.pre s) : RseqOk inlineTarget popInline attempt s := by
  have h := facts_of_pre hs
  obtain ⟨k, hp⟩ := pre_of h
  have ht := trace_ok hs
  rw [code_eq] at ht ⊢
  exact ⟨ht, ⟨_, _, rfl, execBlock_dropLast_arm, arms_arm h.toArm⟩,
    fun p hpm => abortOk h (mids hp p hpm)⟩

/-! ### The registers `asm!` must find kept -/

/-- The registers the code and its setup change: `x0` is also its result. -/
def clobbers : List String := ["x0", "x1", "x8", "x9", "x10", "x11"]

theorem names : ∀ r ∈ scratch, r.x ∈ clobbers := by decide

theorem clobbersOk (hs : attempt.pre s) :
    (∀ s', Exec isa popInline s s' →
      preservedExcept (clobbers ++ (if popInlineSig.ret.isSome then ["x0"] else [])) s s') ∧
    (∀ t s', ExecExit isa popInline s t s' → preservedExcept clobbers s s') ∧
    (∀ p ∈ popInline.abortStates isa s, preservedExcept clobbers s p) := by
  have h := facts_of_pre hs
  obtain ⟨k, hp⟩ := pre_of h
  have names' : ∀ r ∈ scratch, r.x ∈ clobbers ++ (if popInlineSig.ret.isSome then ["x0"] else []) :=
    by decide
  rw [code_eq]
  refine ⟨fun s' he => ?_, fun t s' he => ?_, fun p hpm =>
    preservedExcept_of_mid names (mids hp p hpm)⟩
  · cases he with
    | rseq ha hb =>
      rw [execBlock_arm h.toArm] at ha; cases ha
      by_cases hi : cx.InRange s
      · by_cases ht : tst s = 0
        · rw [runBody_T hp hi ht] at hb; cases hb; exact preservedExcept_of_mid names' mσ5
        · rw [runBody_P hp hi ht] at hb; cases hb; exact preservedExcept_of_mid names' mσ9
      · rw [runBody_C hp hi] at hb; cases hb; exact preservedExcept_of_mid names' mσ2
  · cases he with
    | rseq ha hb =>
      rw [execBlock_arm h.toArm] at ha; cases ha
      by_cases hi : cx.InRange s
      · by_cases ht : tst s = 0
        · rw [runBody_T hp hi ht] at hb; cases hb
        · rw [runBody_P hp hi ht] at hb; cases hb
      · rw [runBody_C hp hi] at hb; cases hb

/-! ### The setup -/

theorem setupOk {l : List Instr} {g : State → State} (hl : AreaSetup l g) :
    SetupOk inlineTarget contract attempt popInlineSig clobbers l := by
  refine .inr ⟨⟨ready, rfl⟩, fun s hp => ?_⟩
  obtain ⟨he, hpre⟩ := setup_ok hl hX hv (fun _ => rfl) (fun _ => rfl) (by decide) (by decide) hp
  exact ⟨g s, he, hpre, hl.mem, hl.sp, argsKept_regs hX (by decide) fun r hr => hl.keeps (by
    revert r; decide), fun _ => hl.preserved (by decide)⟩

end Contract

end VG.Proof.Slab.AArch64.PopInline
