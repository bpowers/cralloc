module

public import Cralloc.Proof.Slab.AArch64.Word

/-!
# The inline push on AArch64

`slab_push` inline (`Impl.Slab.AArch64.pushInline`) against
`Spec.Slab.pushInlineRetContract`: its three ways out (no slab for the CPU; no slot to
push to; the commit), and in each its run, where it may be aborted, and its memory
accesses. The class's word in slab 0 is at `x3`, the mask in `x2`, the pointer in `x0` and
the result in `x4`.
-/

@[expose] public section

namespace VG.Proof.Slab.AArch64.PushInline

open VG.AArch64 VG.Spec.Slab VG.Impl.Slab.AArch64 VG.Proof.Slab.AArch64

abbrev cx : Spec.Slab.Ctx isa := ⟨inlineAbiX4, .inline, pushInlineRetSig⟩

theorem hX : A64 cx.A inlineArgRegs := a64_inlineX4

abbrev contract : Contract isa := pushInlineRetContract inlineAbiX4

/-- The contract of an attempt. -/
abbrev attempt : Contract isa := withReady contract

def body : List (Step Instr Cond) := [
  .instr (.mov .x .x4 (.imm 0)),
  .instr (.ldr .w .x8 { base := .x10, off := .imm 4 }),
  .instr (.cmp .w .x8 (.imm 8192)),
  .exit .hs [] none,
  .instr (.lsl .x .x8 .x8 12),
  .instr (.ldr .x .x1 wordAt),
  .instr (.add .x .x9 .x1 (.imm 8)),
  .instr (.tst .w .x9 (.reg .x2)),
  .exit .eq [] none,
  .instr (.str .x .x0 { base := .x1 }),
  .instr (.mov .x .x4 (.reg .x9)),
  .instr (.str .x .x9 wordAt)]

theorem code_eq : pushInline = .rseq arm body := rfl

/-! ## The states -/

/-- A miss's result. -/
def σ0 (s : State) : State := (σA s).setReg .x4 0
abbrev σ1 (s : State) : State := cpu1 (σ0 s) s
abbrev σ2 (s : State) : State := cpu2 (σ0 s) s
abbrev σ3 (s : State) : State := cpu3 (σ0 s) s
/-- After loading the slab word. -/
def σ4 (s : State) : State := (σ3 s).setReg .x1 (wv s)
def σ6 (s : State) : State := (σ4 s).setReg .x9 (wv s + 8)
def tst (s : State) : BitVec 32 := (wv s + 8).setWidth 32 &&& (s.gpr .x2).setWidth 32
def σ7 (s : State) : State := Flags.tstState (σ6 s) (tst s)
/-- The pointer pushed. -/
def x (s : State) : BitVec 64 := s.gpr .x0
/-- Memory after the store into the free slot, and after the commit. -/
def mSlot (s : State) : Mem := (m1 s).writeW (wv s) (x s)
def mEnd (s : State) : Mem := (mSlot s).writeW (wa s) (wv s + 8)
def σ9 (s : State) : State := { σ7 s with mem := mSlot s }
def σ10 (s : State) : State := (σ9 s).setReg .x4 (wv s + 8)
def σ11 (s : State) : State := { σ10 s with mem := mEnd s }

/-! ## What the precondition gives -/

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

theorem exec_x4 : exec (.mov .x .x4 (.imm 0)) (σA s) = some (σ0 s) := rfl

theorem σ0_gpr {r : Reg} (h4 : ¬ r = .x4) : (σ0 s).gpr r = s.gpr r := by
  rw [σ0, RegUpd.gpr_setReg_of_ne _ _ h4]; rfl

theorem base (hr : ready s) : Base s (σ0 s) := ⟨rfl, rfl, rfl, (σ0_gpr (by decide)).trans hr.1⟩

theorem σ4_gpr {r : Reg} (h1 : ¬ r = .x1) (h4 : ¬ r = .x4) (h8 : ¬ r = .x8) :
    (σ4 s).gpr r = s.gpr r := by
  rw [σ4, RegUpd.gpr_setReg_of_ne _ _ h1, gpr_cpu3 h8, σ0_gpr h4]

theorem σ4_x1 : (σ4 s).gpr .x1 = wv s := RegUpd.gpr_setReg_self _ _ _

theorem exec_word (hp : Pre s k) (hi : cx.InRange s) :
    exec (.ldr .x .x1 wordAt) (σ3 s) = some (σ4 s) :=
  exec_wordAt hp.facts hi hp.word (base hp.facts.ready) (σ0_gpr (by decide))

theorem exec_add8 : exec (.add .x .x9 .x1 (.imm 8)) (σ4 s) = some (σ6 s) := by
  show some ((σ4 s).setReg .x9 ((σ4 s).gpr .x1 + BitVec.ofNat 64 8)) = _
  rw [σ4_x1]
  rfl

theorem σ6_x9 : (σ6 s).gpr .x9 = wv s + 8 := RegUpd.gpr_setReg_self _ _ _

theorem σ6_gpr {r : Reg} (h9 : ¬ r = .x9) : (σ6 s).gpr r = (σ4 s).gpr r := by
  rw [σ6, RegUpd.gpr_setReg_of_ne _ _ h9]

theorem exec_test : exec (.tst .w .x9 (.reg .x2)) (σ6 s) = some (σ7 s) := by
  show some (Flags.tstState (σ6 s) (((σ6 s).gpr .x9).setWidth 32 &&& ((σ6 s).gpr .x2).setWidth 32)) = _
  rw [σ6_x9, σ6_gpr (by decide), σ4_gpr (by decide) (by decide) (by decide)]
  rfl

theorem eval_test : eval .eq (σ7 s) = some (tst s == 0) := rfl

theorem σ7_gpr (r : Reg) : (σ7 s).gpr r = (σ6 s).gpr r := by
  rw [σ7, Flags.tstState, RegUpd.gpr_setFlags]

theorem σ7_x1 : (σ7 s).gpr .x1 = wv s := by rw [σ7_gpr, σ6_gpr (by decide), σ4_x1]
theorem σ7_x0 : (σ7 s).gpr .x0 = x s := by
  rw [σ7_gpr, σ6_gpr (by decide), σ4_gpr (by decide) (by decide) (by decide)]
  rfl
theorem σ7_rd : (σ7 s).rd = s.rd := rfl
theorem σ7_wr : (σ7 s).wr = s.wr := rfl
theorem σ7_mem : (σ7 s).mem = m1 s := rfl

/-- What a push that passes the test finds. -/
theorem hit (hp : Pre s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    IsSlot (2 ^ k) (wv s) ∧ slot (2 ^ k) (wv s) + 1 < 2 ^ k ∧
      (Mag.ofWord (2 ^ k) s.mem (wv s)).push (2 ^ k) (x s) =
        some (.stack (magBase (2 ^ k) (wv s)) (items (2 ^ k) s.mem (wv s) ++ [x s])) := by
  have hm : ¬ (wv s + 8).toNat % (8 * 2 ^ k) < 16 := fun h' =>
    ht ((test_iff hp.hk hp.hk' (wv s + 8) (s.gpr .x2) hp.mask).mpr h')
  exact push_of_test k rfl hp.hk hp.hk' (hp.ok hi) s.mem (x s) hm

theorem magRegion_eq (hp : Pre s k) : cx.magRegion s = ⟨magBase (2 ^ k) (wv s), 8 * 2 ^ k⟩ := by
  simp only [Spec.Slab.Ctx.magRegion, hp.slots, hp.w]

/-- The free slot's region: in the magazine, apart from `rseq_cs`, the slab word and
`__rseq_offset`. -/
structure SlotFacts (s : State) (k : Nat) : Prop where
  inMag : (⟨magBase (2 ^ k) (wv s), 8 * 2 ^ k⟩ : Region).Contains (wv s) 8
  mem : (⟨magBase (2 ^ k) (wv s), 8 * 2 ^ k⟩ : Region) ∈ s.wr
  cs : Region.Disjoint ⟨wv s, 8⟩ ⟨area s + 8, 8⟩
  word : Region.Disjoint ⟨wv s, 8⟩ ⟨wa s, 8⟩
  rsVar : Region.Disjoint ⟨s.syms RS, 8⟩ ⟨wv s, 8⟩

theorem slotFacts (hp : Pre s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) : SlotFacts s k := by
  obtain ⟨hs, hlt, -⟩ := hit hp hi ht
  have hsl : IsSlot (cx.slots s) (cx.w s) := by rw [hp.slots, hp.w]; exact hs
  have hc := contains_at hs hlt
  have hsub := sub_of_contains hc
  have hmag := hp.facts.mag hi hsl
  have hcm := hp.facts.cs_mag hi hsl
  have hwm := hp.facts.word_mag hi hsl
  have hrm := hp.facts.rsVar_mag hi hsl
  rw [magRegion_eq hp] at hmag hcm hwm hrm
  exact ⟨hc, hmag, hcm.symm.sub_left hsub, by rw [hp.word]; exact hwm.symm.sub_left hsub,
    hrm.sub_right hsub⟩

theorem exec_slot (hp : Pre s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    exec (.str .x .x0 { base := .x1 }) (σ7 s) = some (σ9 s) := by
  have hf := slotFacts hp hi ht
  have hin : InRegions (σ7 s).wr ((σ7 s).ea { base := .x1 }) 8 := by
    rw [Access.ea_base, σ7_x1, σ7_wr]; exact InRegions.of_mem hf.mem hf.inMag
  rw [Access.exec_str_x hin, Access.ea_base, σ7_x1, σ7_x0, σ7_mem]
  rfl

theorem σ9_gpr (r : Reg) : (σ9 s).gpr r = (σ7 s).gpr r := rfl

theorem exec_ret : exec (.mov .x .x4 (.reg .x9)) (σ9 s) = some (σ10 s) := by
  show some ((σ9 s).setReg .x4 ((σ9 s).gpr .x9)) = _
  rw [σ9_gpr, σ7_gpr, σ6_x9]
  rfl

theorem σ10_x3 : (σ10 s).gpr .x3 = s.gpr .x3 := by
  rw [σ10, RegUpd.gpr_setReg_of_ne _ _ (by decide), σ9_gpr, σ7_gpr, σ6_gpr (by decide),
    σ4_gpr (by decide) (by decide) (by decide)]

theorem σ10_x8 : (σ10 s).gpr .x8 = sh s := by
  rw [σ10, RegUpd.gpr_setReg_of_ne _ _ (by decide), σ9_gpr, σ7_gpr, σ6_gpr (by decide), σ4,
    RegUpd.gpr_setReg_of_ne _ _ (by decide), cpu3_x8]

theorem σ10_x9 : (σ10 s).gpr .x9 = wv s + 8 := by
  rw [σ10, RegUpd.gpr_setReg_of_ne _ _ (by decide), σ9_gpr, σ7_gpr, σ6_x9]

theorem σ10_mem : (σ10 s).mem = mSlot s := rfl
theorem σ10_wr : (σ10 s).wr = s.wr := rfl

theorem exec_commit (hp : Pre s k) (hi : cx.InRange s) :
    exec (.str .x .x9 wordAt) (σ10 s) = some (σ11 s) := by
  have hin : InRegions (σ10 s).wr ((σ10 s).ea wordAt) 8 := by
    rw [wordAt_ea _ σ10_x3 σ10_x8, σ10_wr, hp.word]
    exact inWr (r := ⟨cx.word s, 8⟩) (hp.facts.word hi)
  rw [Access.exec_str_x hin, wordAt_ea _ σ10_x3 σ10_x8, σ10_x9, σ10_mem]
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

theorem eval_test_true (ht : tst s = 0) : eval .eq (σ7 s) = some true := by
  rw [eval_test, ht]; rfl

theorem eval_test_false (ht : ¬ tst s = 0) : eval .eq (σ7 s) = some false := by
  rw [eval_test]; simpa using ht

end Steps

/-! ## The three ways out -/

section Cases

variable {s : State} {k : Nat}

theorem runBody_C (hp : Pre s k) (hi : ¬ cx.InRange s) :
    runBody isa body (σA s) = some (.done (σ2 s)) := by
  rw [body, runBody_instr exec_x4, runBody_instr (exec_cpu hp.facts.toArm (base hp.facts.ready)),
    runBody_instr exec_cmp, runBody_take (eval_cpu_true hi) execBlock_nil']

theorem prefixes_C (hp : Pre s k) (hi : ¬ cx.InRange s) :
    bodyPrefixes isa body (σA s) = [σA s, σ0 s, σ1 s, σ2 s] := by
  rw [body, bodyPrefixes_instr exec_x4,
    bodyPrefixes_instr (exec_cpu hp.facts.toArm (base hp.facts.ready)), bodyPrefixes_instr exec_cmp,
    bodyPrefixes_take (eval_cpu_true hi)]

theorem end_C (hp : Pre s k) (hi : ¬ cx.InRange s) : bodyEnd isa body (σA s) = some 3 := by
  rw [body, bodyEnd_instr exec_x4, bodyEnd_instr (exec_cpu hp.facts.toArm (base hp.facts.ready)),
    bodyEnd_instr exec_cmp, bodyEnd_take (eval_cpu_true hi)]
  rfl

theorem runBody_T (hp : Pre s k) (hi : cx.InRange s) (ht : tst s = 0) :
    runBody isa body (σA s) = some (.done (σ7 s)) := by
  rw [body, runBody_instr exec_x4, runBody_instr (exec_cpu hp.facts.toArm (base hp.facts.ready)),
    runBody_instr exec_cmp, runBody_skip (eval_cpu_false hi), runBody_instr exec_lsl,
    runBody_instr (exec_word hp hi), runBody_instr exec_add8, runBody_instr exec_test,
    runBody_take (eval_test_true ht) execBlock_nil']

theorem prefixes_T (hp : Pre s k) (hi : cx.InRange s) (ht : tst s = 0) :
    bodyPrefixes isa body (σA s) = [σA s, σ0 s, σ1 s, σ2 s, σ2 s, σ3 s, σ4 s, σ6 s, σ7 s] := by
  rw [body, bodyPrefixes_instr exec_x4,
    bodyPrefixes_instr (exec_cpu hp.facts.toArm (base hp.facts.ready)), bodyPrefixes_instr exec_cmp,
    bodyPrefixes_skip (eval_cpu_false hi), bodyPrefixes_instr exec_lsl,
    bodyPrefixes_instr (exec_word hp hi), bodyPrefixes_instr exec_add8, bodyPrefixes_instr exec_test,
    bodyPrefixes_take (eval_test_true ht)]

theorem end_T (hp : Pre s k) (hi : cx.InRange s) (ht : tst s = 0) :
    bodyEnd isa body (σA s) = some 8 := by
  rw [body, bodyEnd_instr exec_x4, bodyEnd_instr (exec_cpu hp.facts.toArm (base hp.facts.ready)),
    bodyEnd_instr exec_cmp, bodyEnd_skip (eval_cpu_false hi), bodyEnd_instr exec_lsl,
    bodyEnd_instr (exec_word hp hi), bodyEnd_instr exec_add8, bodyEnd_instr exec_test,
    bodyEnd_take (eval_test_true ht)]
  rfl

theorem runBody_P (hp : Pre s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    runBody isa body (σA s) = some (.done (σ11 s)) := by
  rw [body, runBody_instr exec_x4, runBody_instr (exec_cpu hp.facts.toArm (base hp.facts.ready)),
    runBody_instr exec_cmp, runBody_skip (eval_cpu_false hi), runBody_instr exec_lsl,
    runBody_instr (exec_word hp hi), runBody_instr exec_add8, runBody_instr exec_test,
    runBody_skip (eval_test_false ht), runBody_instr (exec_slot hp hi ht), runBody_instr exec_ret,
    runBody_instr (exec_commit hp hi), runBody_nil']

theorem prefixes_P (hp : Pre s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    bodyPrefixes isa body (σA s) =
      [σA s, σ0 s, σ1 s, σ2 s, σ2 s, σ3 s, σ4 s, σ6 s, σ7 s, σ7 s, σ9 s, σ10 s] := by
  rw [body, bodyPrefixes_instr exec_x4,
    bodyPrefixes_instr (exec_cpu hp.facts.toArm (base hp.facts.ready)), bodyPrefixes_instr exec_cmp,
    bodyPrefixes_skip (eval_cpu_false hi), bodyPrefixes_instr exec_lsl,
    bodyPrefixes_instr (exec_word hp hi), bodyPrefixes_instr exec_add8, bodyPrefixes_instr exec_test,
    bodyPrefixes_skip (eval_test_false ht), bodyPrefixes_instr (exec_slot hp hi ht),
    bodyPrefixes_instr exec_ret, bodyPrefixes_last (exec_commit hp hi)]

theorem end_P (hp : Pre s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    bodyEnd isa body (σA s) = some 12 := by
  rw [body, bodyEnd_instr exec_x4, bodyEnd_instr (exec_cpu hp.facts.toArm (base hp.facts.ready)),
    bodyEnd_instr exec_cmp, bodyEnd_skip (eval_cpu_false hi), bodyEnd_instr exec_lsl,
    bodyEnd_instr (exec_word hp hi), bodyEnd_instr exec_add8, bodyEnd_instr exec_test,
    bodyEnd_skip (eval_test_false ht), bodyEnd_instr (exec_slot hp hi ht), bodyEnd_instr exec_ret,
    bodyEnd_instr (exec_commit hp hi), bodyEnd_nil']
  rfl

/-! ### Memory accesses -/

theorem ev_word (hp : Pre s k) (hi : cx.InRange s) :
    Instr.events (.ldr .x .x1 wordAt) (σ3 s) (σ4 s) = [⟨wa s, 8, false, (wv s).toNat⟩] :=
  ev_wordAt hp.facts hi hp.word (base hp.facts.ready) (σ0_gpr (by decide))

theorem ev_slot : Instr.events (.str .x .x0 { base := .x1 }) (σ7 s) (σ9 s) =
    [⟨wv s, 8, true, (x s).toNat⟩] := by
  simp only [Instr.events, Access.ea_base, σ7_x1, Width.bytes, toNat_read8]
  rw [show (σ9 s).mem = mSlot s from rfl, mSlot, Mem.readW_writeW_self64]

theorem ev_commit :
    Instr.events (.str .x .x9 wordAt) (σ10 s) (σ11 s) = [⟨wa s, 8, true, (wv s + 8).toNat⟩] := by
  simp only [Instr.events, wordAt_ea _ σ10_x3 σ10_x8, Width.bytes, toNat_read8]
  rw [show (σ11 s).mem = mEnd s from rfl, mEnd, Mem.readW_writeW_self64]

theorem events_C (hp : Pre s k) (hi : ¬ cx.InRange s) :
    bodyEvents isa body (σA s) = [⟨area s + 4, 4, false, (cpu32 s).toNat⟩] := by
  rw [body, bodyEvents_instr exec_x4, bodyEvents_instr (exec_cpu hp.facts.toArm (base hp.facts.ready)),
    bodyEvents_instr exec_cmp, bodyEvents_take (eval_cpu_true hi)]
  simp only [isa, ev_cpu (base hp.facts.ready)]; rfl

theorem events_T (hp : Pre s k) (hi : cx.InRange s) (ht : tst s = 0) :
    bodyEvents isa body (σA s) =
      [⟨area s + 4, 4, false, (cpu32 s).toNat⟩, ⟨wa s, 8, false, (wv s).toNat⟩] := by
  rw [body, bodyEvents_instr exec_x4, bodyEvents_instr (exec_cpu hp.facts.toArm (base hp.facts.ready)),
    bodyEvents_instr exec_cmp, bodyEvents_skip (eval_cpu_false hi), bodyEvents_instr exec_lsl,
    bodyEvents_instr (exec_word hp hi), bodyEvents_instr exec_add8, bodyEvents_instr exec_test,
    bodyEvents_take (eval_test_true ht)]
  simp only [isa, ev_cpu (base hp.facts.ready), ev_word hp hi]; rfl

theorem events_P (hp : Pre s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    bodyEvents isa body (σA s) =
      [⟨area s + 4, 4, false, (cpu32 s).toNat⟩, ⟨wa s, 8, false, (wv s).toNat⟩,
        ⟨wv s, 8, true, (x s).toNat⟩, ⟨wa s, 8, true, (wv s + 8).toNat⟩] := by
  rw [body, bodyEvents_instr exec_x4, bodyEvents_instr (exec_cpu hp.facts.toArm (base hp.facts.ready)),
    bodyEvents_instr exec_cmp, bodyEvents_skip (eval_cpu_false hi), bodyEvents_instr exec_lsl,
    bodyEvents_instr (exec_word hp hi), bodyEvents_instr exec_add8, bodyEvents_instr exec_test,
    bodyEvents_skip (eval_test_false ht), bodyEvents_instr (exec_slot hp hi ht),
    bodyEvents_instr exec_ret, bodyEvents_instr (exec_commit hp hi), bodyEvents_nil']
  simp only [ev_cpu (base hp.facts.ready), ev_word hp hi, ev_slot, ev_commit]; rfl

end Cases

/-! ## What each state keeps -/

/-- The registers an attempt writes. -/
def scratch : List Reg := [.x1, .x4, .x8, .x9]

section Mids

variable {s : State}

theorem mσA : Mid scratch s (m1 s) (σA s) := mid_σA
theorem mσ0 : Mid scratch s (m1 s) (σ0 s) := mσA.setReg (by decide) _
theorem mσ1 : Mid scratch s (m1 s) (σ1 s) := mσ0.cpu1 (by decide)
theorem mσ2 : Mid scratch s (m1 s) (σ2 s) := mσ0.cpu2 (by decide)
theorem mσ3 : Mid scratch s (m1 s) (σ3 s) := mσ0.cpu3 (by decide)
theorem mσ4 : Mid scratch s (m1 s) (σ4 s) := mσ3.setReg (by decide) _
theorem mσ6 : Mid scratch s (m1 s) (σ6 s) := mσ4.setReg (by decide) _
theorem mσ7 : Mid scratch s (m1 s) (σ7 s) := mσ6.setFlags _ _ _ _
theorem mσ9 : Mid scratch s (mSlot s) (σ9 s) := mσ7.withMem _
theorem mσ10 : Mid scratch s (mSlot s) (σ10 s) := mσ9.setReg (by decide) _
theorem mσ11 : Mid scratch s (mEnd s) (σ11 s) := mσ10.withMem _

end Mids

/-! ## The contract -/

section Contract

variable {s : State} {k : Nat}

theorem hv : cx.A.args ((cx.sig.words cx.A.ptrBits).map (·.bits cx.A.ptrBits)) =
    some (fun s => [s.gpr .x3, s.gpr .x2, s.gpr .x0]) := rfl

theorem arg2 : cx.arg s 2 = x s := rfl

theorem facts_of_pre (hs : attempt.pre s) : Facts cx s := by
  have := (pre_iff cx (s := s) hv rfl rfl).mp hs.1
  exact facts_of_world hX rfl this.2.1 this.2.2 hs.2

/-- Item `i` of the magazine is apart from `rseq_cs`, the slab word, and the free slot. -/
theorem item_disjoint (hp : Pre s k) (hi : cx.InRange s) (hs : IsSlot (2 ^ k) (wv s)) {i : Nat}
    (hil : i < slot (2 ^ k) (wv s) - 1) :
    Region.Disjoint ⟨magBase (2 ^ k) (wv s) + BitVec.ofNat 64 (8 * (i + 1)), 8⟩ ⟨area s + 8, 8⟩ ∧
    Region.Disjoint ⟨magBase (2 ^ k) (wv s) + BitVec.ofNat 64 (8 * (i + 1)), 8⟩ ⟨wa s, 8⟩ ∧
    Region.Disjoint ⟨magBase (2 ^ k) (wv s) + BitVec.ofNat 64 (8 * (i + 1)), 8⟩ ⟨wv s, 8⟩ := by
  have hsl : IsSlot (cx.slots s) (cx.w s) := by rw [hp.slots, hp.w]; exact hs
  have hlt := slot_lt (2 ^ k) (wv s) (Nat.two_pow_pos k)
  have hsub := sub_item (n := 2 ^ k) (w := wv s) (i := i) (by omega)
  have hcm := hp.facts.cs_mag hi hsl
  have hwm := hp.facts.word_mag hi hsl
  rw [magRegion_eq hp] at hcm hwm
  have hn : 8 * 2 ^ k ≤ 2 ^ 19 := by
    have := Nat.pow_le_pow_right (n := 2) (by decide) hp.hk'
    omega
  refine ⟨hcm.symm.sub_left hsub, by rw [hp.word]; exact hwm.symm.sub_left hsub, ?_⟩
  have hw := magBase_add_slot hs
  have := Offset.disjoint (magBase (2 ^ k) (wv s)) (d := 8 * (i + 1)) (n := 8)
    (e := 8 * slot (2 ^ k) (wv s)) (k := 8) (by omega) (by omega) (by omega)
  rwa [hw] at this

theorem pushed (hp : Pre s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    cx.Pushed s (cx.arg s 2) (σ11 s) := by
  obtain ⟨hs, hlt, hpush⟩ := hit hp hi ht
  have hf := slotFacts hp hi ht
  refine ⟨.stack (magBase (2 ^ k) (wv s)) (items (2 ^ k) s.mem (wv s) ++ [x s]), ?_, ?_, ?_, ?_⟩
  · simp only [Spec.Slab.Ctx.pushResult, hi, ↓reduceIte, Spec.Slab.Ctx.mag, hp.slots,
      mem_of (c := cx) hX, hp.w, arg2]
    exact hpush
  · rw [csField_of (c := cx) hX, ← hp.word, hp.w]
    show Frame _ s.mem (mEnd s)
    exact (frame_three s.mem _ _ _ _ _ _).mono (by simp)
  · show (mEnd s).readW (cx.word s) 64 = _
    rw [mEnd, ← hp.word, Mem.readW_writeW_self64, hp.w]
  · show Mag.ofWord (cx.slots s) (mEnd s) (cx.w s + 8) = _
    rw [hp.slots, hp.w]
    refine ofWord_add8 k hp.hk' rfl hs hlt (x s) ?_ fun i hil => ?_
    · rw [mEnd, readW64_writeW_disj hf.word, mSlot, Mem.readW_writeW_self64]
    · obtain ⟨h1, h2, h3⟩ := item_disjoint hp hi hs hil
      rw [mEnd, readW64_writeW_disj h2, mSlot, readW64_writeW_disj h3, m1, readW64_writeW_disj h1]

theorem unchanged_cs : Unchanged [cx.csField s] (cx.mem s) (m1 s) := by
  rw [csField_of (c := cx) hX, mem_of (c := cx) hX]; exact frame_m1

theorem missed_C (hi : ¬ cx.InRange s) : cx.PushMissed s (cx.arg s 2) (σ2 s) := by
  refine ⟨?_, unchanged_cs⟩
  simp only [Spec.Slab.Ctx.pushResult, hi, ↓reduceIte]

theorem missed_T (hp : Pre s k) (hi : cx.InRange s) (ht : tst s = 0) :
    cx.PushMissed s (cx.arg s 2) (σ7 s) := by
  refine ⟨?_, unchanged_cs⟩
  simp only [Spec.Slab.Ctx.pushResult, hi, ↓reduceIte, Spec.Slab.Ctx.mag, hp.slots,
    mem_of (c := cx) hX, hp.w]
  exact (push_eq_none_iff _ k rfl hp.hk hp.hk' (hp.ok hi)).mpr
    ((test_iff hp.hk hp.hk' (wv s + 8) (s.gpr .x2) hp.mask).mp ht)

theorem ret_σ2 : (σ2 s).gpr .x4 = 0 := by
  rw [gpr_cpu2 (by decide), σ0, RegUpd.gpr_setReg_self]

theorem ret_σ7 : (σ7 s).gpr .x4 = 0 := by
  rw [σ7_gpr, σ6_gpr (by decide), σ4, RegUpd.gpr_setReg_of_ne _ _ (by decide), gpr_cpu3 (by decide),
    σ0, RegUpd.gpr_setReg_self]

/-- A push's result is the new word, which is not zero: its test found a bit set. -/
theorem ret_σ11 (ht : ¬ tst s = 0) : (σ11 s).gpr .x4 ≠ 0 := by
  have h : (σ11 s).gpr .x4 = wv s + 8 := by
    show (σ10 s).gpr .x4 = _
    rw [σ10, RegUpd.gpr_setReg_self]
  rw [h]
  intro h0
  apply ht
  rw [tst, h0]
  simp

theorem post_eq (s' : State) : attempt.post s s' ↔
    ((cx.Pushed s (cx.arg s 2) s' ∧ s'.gpr .x4 ≠ 0) ∨
      (cx.PushMissed s (cx.arg s 2) s' ∧ s'.gpr .x4 = 0)) :=
  post_iff cx hv s s'

theorem correct (hs : attempt.pre s) :
    (∃ s', Exec isa pushInline s s' ∧ inlineTargetX4.abiPreserved s s' ∧ attempt.post s s') ∨
    (∃ t s', ExecExit isa pushInline s t s' ∧ inlineTargetX4.abiPreserved s s' ∧ attempt.exit t s s') := by
  have h := facts_of_pre hs
  obtain ⟨k, hp⟩ := pre_of h
  refine .inl ?_
  rw [code_eq]
  by_cases hi : cx.InRange s
  · by_cases ht : tst s = 0
    · exact ⟨σ7 s, .rseq (execBlock_arm h.toArm) (runBody_T hp hi ht), mσ7.sp,
        (post_eq _).mpr (.inr ⟨missed_T hp hi ht, ret_σ7⟩)⟩
    · exact ⟨σ11 s, .rseq (execBlock_arm h.toArm) (runBody_P hp hi ht), mσ11.sp,
        (post_eq _).mpr (.inl ⟨pushed hp hi ht, ret_σ11 ht⟩)⟩
  · exact ⟨σ2 s, .rseq (execBlock_arm h.toArm) (runBody_C hp hi), mσ2.sp,
      (post_eq _).mpr (.inr ⟨missed_C hi, ret_σ2⟩)⟩

/-! ### Where it may be aborted -/

/-- Before the free slot is written, the magazine is as on entry. -/
theorem mags_m1 (hp : Pre s k) : ∀ i < SLAB_COUNT,
    Mag.ofWord (cx.slots s) (m1 s) (cx.wordOf s i) = Mag.ofWord (cx.slots s) (cx.mem s) (cx.wordOf s i) := by
  intro i hi
  have hok := hp.facts.pre.2.2.2.2.1 i hi
  rw [mem_of (c := cx) hX]
  refine ofWord_frame (by rw [hp.slots]; exact Nat.two_pow_pos k) frame_m1 hok.1 fun hs r hr => ?_
  rw [List.mem_singleton.mp hr, ← csField_of (c := cx) hX]
  exact (hok.2 hs).2.2.1 (cx.csField s, true) cs_mem_fixed

/-- After it is, too: the free slot is above the pointers. -/
theorem mag_slot (hp : Pre s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    Mag.ofWord (cx.slots s) (mSlot s) (cx.w s) = cx.mag s := by
  obtain ⟨hs, -, -⟩ := hit hp hi ht
  simp only [Spec.Slab.Ctx.mag, mem_of (c := cx) hX, hp.slots, hp.w]
  exact ofWord_congr fun i hil => by
    obtain ⟨h1, -, h3⟩ := item_disjoint hp hi hs hil
    rw [mSlot, readW64_writeW_disj h3, m1, readW64_writeW_disj h1]

/-- After it is, every slab's magazine is as on entry: the free slot is above the routine's
slab's pointers, and in no other slab's magazine (`Exclusive`). -/
theorem mags_slot (hp : Pre s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) : ∀ i < SLAB_COUNT,
    Mag.ofWord (cx.slots s) (mSlot s) (cx.wordOf s i) =
      Mag.ofWord (cx.slots s) (cx.mem s) (cx.wordOf s i) := by
  intro i hi'
  by_cases hc : i = cx.cpu s
  · subst hc
    rw [wordOf_cpu]
    exact mag_slot hp hi ht
  · have hok := hp.facts.pre.2.2.2.2.1 i hi'
    have hf := slotFacts hp hi ht
    obtain ⟨hs0, -, -⟩ := hit hp hi ht
    have hsc : IsSlot (cx.slots s) (cx.wordOf s (cx.cpu s)) := by
      rw [wordOf_cpu, hp.slots, hp.w]; exact hs0
    rw [mem_of (c := cx) hX]
    refine ofWord_frame (by rw [hp.slots]; exact Nat.two_pow_pos k) (frame_two _ _ _ _ _) hok.1
      fun hs r hr => ?_
    rcases List.mem_cons.mp hr with rfl | hr
    · rw [← csField_of (c := cx) hX]; exact (hok.2 hs).2.2.1 (cx.csField s, true) cs_mem_fixed
    · rw [List.mem_singleton.mp hr]
      have hex := hp.facts.pre.2.2.2.2.2 i hi' (cx.cpu s) hi hc hs hsc
      rw [wordOf_cpu, hp.slots, hp.w] at hex
      rw [hp.slots]
      exact hex.sub_right (sub_of_contains hf.inMag)

theorem abort_eq (p : State) : attempt.abort s p ↔
    [p.gpr .x3, p.gpr .x2, p.gpr .x0] = [s.gpr .x3, s.gpr .x2, s.gpr .x0] ∧
    ((Unchanged [cx.csField s, ⟨cx.w s, 8⟩] (cx.mem s) p.mem ∧
      ∀ i < SLAB_COUNT, Mag.ofWord (cx.slots s) p.mem (cx.wordOf s i) =
        Mag.ofWord (cx.slots s) (cx.mem s) (cx.wordOf s i)) ∧ cx.inv p) :=
  abort_iff cx hv s p

/-- The magazine lies apart, by the invariant. -/
theorem mag_apart (hp : Pre s k) (hi : cx.InRange s) (hs : IsSlot (2 ^ k) (wv s)) :
    cx.Apart s ⟨magBase (2 ^ k) (wv s), 8 * 2 ^ k⟩ := by
  have := (hp.facts.pre.2.2.2.2.1 (cx.cpu s) hi).2
  rw [wordOf_cpu, hp.slots, hp.w] at this
  exact this hs

theorem abortOk1 (hp : Pre s k) {p : State} (hm : Mid scratch s (m1 s) p) :
    armed p ∧ inlineTargetX4.abiPreserved s p ∧ attempt.abort s p ∧
      ∀ p', inlineTargetX4.rely p p' → attempt.env p p' → attempt.pre p' := by
  obtain ⟨hip, hre⟩ := retry hX hv (post := fun s s' => _) (fun _ => rfl) (fun _ => rfl) (by decide)
    hp.facts.pre hp.facts.ready hm (by decide) (by decide) (by decide) frame_m1
    (harmless_cs1 hX hp.facts.pre)
  refine ⟨armed_of_mid hp.facts.toArm (xs := []) frame_m1 m1_cs (by simp) hm, hm.sp, ?_, hre⟩
  refine (abort_eq p).mpr ⟨by rw [hm.gpr .x3 (by decide), hm.gpr .x2 (by decide),
    hm.gpr .x0 (by decide)], ⟨?_, fun i hi => by rw [hm.mem]; exact mags_m1 hp i hi⟩, hip⟩
  rw [hm.mem, csField_of (c := cx) hX, mem_of (c := cx) hX]
  exact frame_m1.mono (by simp)

theorem abortOk2 (hp : Pre s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) {p : State}
    (hm : Mid scratch s (mSlot s) p) :
    armed p ∧ inlineTargetX4.abiPreserved s p ∧ attempt.abort s p ∧
      ∀ p', inlineTargetX4.rely p p' → attempt.env p p' → attempt.pre p' := by
  have hf := slotFacts hp hi ht
  have hfr : Frame [⟨area s + 8, 8⟩, ⟨wv s, 8⟩] s.mem (mSlot s) := frame_two _ _ _ _ _
  have hcs : (mSlot s).readW (area s + 8) 64 = s.csDesc := by
    rw [mSlot, readW64_writeW_disj hf.cs.symm, m1_cs]
  have hW := harmless_cs2 hX hp.facts.pre (harmless_mag rfl
    (mag_apart hp hi (hit hp hi ht).1) (sub_of_contains hf.inMag))
  obtain ⟨hip, hre⟩ := retry hX hv (post := fun s s' => _) (fun _ => rfl) (fun _ => rfl) (by decide)
    hp.facts.pre hp.facts.ready hm (by decide) (by decide) (by decide) hfr hW
  refine ⟨armed_of_mid hp.facts.toArm (xs := [⟨wv s, 8⟩]) hfr hcs (by simpa using hf.rsVar) hm,
    hm.sp, ?_, hre⟩
  refine (abort_eq _).mpr ⟨by rw [hm.gpr .x3 (by decide), hm.gpr .x2 (by decide),
    hm.gpr .x0 (by decide)], ⟨?_, fun i hi' => by rw [hm.mem]; exact mags_slot hp hi ht i hi'⟩, hip⟩
  rw [hm.mem, csField_of (c := cx) hX, mem_of (c := cx) hX, hp.w]
  exact hfr

/-! ### Its accesses -/

theorem addr8 : s.tpidr + o s + BitVec.ofNat 64 RSEQ_CS = area s + 8 := rfl
theorem addr4 : s.tpidr + o s + BitVec.ofNat 64 RSEQ_CPU_ID = area s + 4 := rfl
theorem rsName : RSEQ_OFFSET = RS := rfl

theorem ofNat_toNat64 (v : BitVec 64) : BitVec.ofNat 64 v.toNat = v := by
  rw [BitVec.ofNat_toNat, BitVec.setWidth_eq]

theorem wordAddr_eq (hp : Pre s k) :
    wordAddr (s.syms SLABS) (cpu32 s).toNat (cx.cls s) = wa s := by
  rw [hp.word]; rfl

theorem test_eq : BitVec.ofNat 32 ((wv s).toNat + 8) &&& BitVec.ofNat 32 (cx.testMask s) = tst s := by
  have : BitVec.ofNat 32 ((wv s).toNat + 8) = (wv s + 8).setWidth 32 := by
    apply BitVec.eq_of_toNat_eq
    rw [BitVec.toNat_ofNat, BitVec.toNat_setWidth, add8_toNat_mod,
      Nat.mod_mod_of_dvd _ (by decide : 2 ^ 32 ∣ 2 ^ 64)]
  simp only [Spec.Slab.Ctx.testMask, BitVec.ofNat_toNat, tst, this]; rfl

theorem trace_ok (hs : attempt.pre s) : attempt.trace s (Code.events isa pushInline s) := by
  have h := facts_of_pre hs
  obtain ⟨k, hp⟩ := pre_of h
  show cx.PushTrace s (cx.arg s 2) (cx.attemptEvents s (Code.events isa (.rseq arm body) s))
  simp only [Code.events, execBlock_arm h.toArm, events_arm h.toArm, attemptEvents_eq (c := cx) hX]
  refine ⟨(o s).toNat, s.csDesc.toNat, (cpu32 s).toNat, (wv s).toNat, ?_⟩
  simp only [Spec.Slab.Ctx.headTrace, Spec.Slab.Ctx.rd, Spec.Slab.Ctx.wr, tp_of (c := cx) hX,
    sym_of (c := cx) hX, test_eq, ofNat_toNat64, addr8, addr4, rsName, arg2]
  by_cases hi : cx.InRange s
  · have hi' : (cpu32 s).toNat < SLAB_COUNT := (inRange_iff.mp hi)
    rw [wordAddr_eq hp]
    by_cases ht : tst s = 0
    · rw [events_T hp hi ht]; simp [hi', ht, o]
    · rw [events_P hp hi ht]; simpa [hi', o] using ht
  · have hi' : ¬ (cpu32 s).toNat < SLAB_COUNT := fun h' => hi (inRange_iff.mpr h')
    rw [events_C hp hi]; simp [hi', o]

theorem rseqOk (hs : attempt.pre s) : RseqOk inlineTargetX4 pushInline attempt s := by
  have h := facts_of_pre hs
  obtain ⟨k, hp⟩ := pre_of h
  have ht := trace_ok hs
  rw [code_eq] at ht ⊢
  refine ⟨ht, ⟨_, _, rfl, execBlock_dropLast_arm, arms_arm h.toArm⟩, ?_⟩
  simp only [Code.abortStates, execBlock_arm h.toArm]
  by_cases hi : cx.InRange s
  · by_cases ht : tst s = 0
    · rw [prefixes_T hp hi ht]
      simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
      exact ⟨abortOk1 hp mσA, abortOk1 hp mσ0, abortOk1 hp mσ1, abortOk1 hp mσ2, abortOk1 hp mσ2,
        abortOk1 hp mσ3, abortOk1 hp mσ4, abortOk1 hp mσ6, abortOk1 hp mσ7⟩
    · rw [prefixes_P hp hi ht]
      simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
      exact ⟨abortOk1 hp mσA, abortOk1 hp mσ0, abortOk1 hp mσ1, abortOk1 hp mσ2, abortOk1 hp mσ2,
        abortOk1 hp mσ3, abortOk1 hp mσ4, abortOk1 hp mσ6, abortOk1 hp mσ7, abortOk1 hp mσ7,
        abortOk2 hp hi ht mσ9, abortOk2 hp hi ht mσ10⟩
  · rw [prefixes_C hp hi]
    simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
    exact ⟨abortOk1 hp mσA, abortOk1 hp mσ0, abortOk1 hp mσ1, abortOk1 hp mσ2⟩

/-! ### The registers `asm!` must find kept -/

/-- The registers the code and its setup change: `x4` is also its result. -/
def clobbers : List String := ["x1", "x4", "x8", "x9", "x10", "x11"]

theorem names : ∀ r ∈ scratch, r.x ∈ clobbers := by decide

/-- Wherever it may be aborted, the registers are kept but the clobbers. -/
theorem kept_aborts (hp : Pre s k) :
    ∀ p ∈ Code.abortStates isa (.rseq arm body) s, preservedExcept clobbers s p := by
  simp only [Code.abortStates, execBlock_arm hp.facts.toArm]
  have pe {m : Mem} {p : State} (h : Mid scratch s m p) := preservedExcept_of_mid names h
  by_cases hi : cx.InRange s
  · by_cases ht : tst s = 0
    · rw [prefixes_T hp hi ht]
      simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
      exact ⟨pe mσA, pe mσ0, pe mσ1, pe mσ2, pe mσ2, pe mσ3, pe mσ4, pe mσ6, pe mσ7⟩
    · rw [prefixes_P hp hi ht]
      simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
      exact ⟨pe mσA, pe mσ0, pe mσ1, pe mσ2, pe mσ2, pe mσ3, pe mσ4, pe mσ6, pe mσ7, pe mσ7, pe mσ9,
        pe mσ10⟩
  · rw [prefixes_C hp hi]
    simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
    exact ⟨pe mσA, pe mσ0, pe mσ1, pe mσ2⟩

theorem clobbersOk (hs : attempt.pre s) :
    (∀ s', Exec isa pushInline s s' →
      preservedExcept (clobbers ++ (if pushInlineRetSig.ret.isSome then ["x4"] else [])) s s') ∧
    (∀ t s', ExecExit isa pushInline s t s' → preservedExcept clobbers s s') ∧
    (∀ p ∈ pushInline.abortStates isa s, preservedExcept clobbers s p) := by
  have h := facts_of_pre hs
  obtain ⟨k, hp⟩ := pre_of h
  have names' : ∀ r ∈ scratch,
      r.x ∈ clobbers ++ (if pushInlineRetSig.ret.isSome then ["x4"] else []) := by decide
  rw [code_eq]
  refine ⟨fun s' he => ?_, fun t s' he => ?_, kept_aborts hp⟩
  · cases he with
    | rseq ha hb =>
      rw [execBlock_arm h.toArm] at ha; cases ha
      by_cases hi : cx.InRange s
      · by_cases ht : tst s = 0
        · rw [runBody_T hp hi ht] at hb; cases hb; exact preservedExcept_of_mid names' mσ7
        · rw [runBody_P hp hi ht] at hb; cases hb; exact preservedExcept_of_mid names' mσ11
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

theorem setupOk : SetupOk inlineTargetX4 contract attempt pushInlineRetSig clobbers setup := by
  refine .inr ⟨⟨ready, rfl⟩, fun s hp => ?_⟩
  obtain ⟨he, hpre⟩ := setup_ok hX hv (fun _ => rfl) (fun _ => rfl) (by decide) (by decide) hp
  exact ⟨s6 s, he, hpre, rfl, rfl, argsKept_regs hX (by decide) fun r hr => s6_keeps (by
    revert r; decide), fun _ => s6_preserved (by decide)⟩

end Contract

end VG.Proof.Slab.AArch64.PushInline
