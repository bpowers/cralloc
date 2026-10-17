module

public import Asmalloc.Proof.Slab.X86_64.Table

/-!
# The out-of-line push on x86-64

`slab_push` (`Impl.Slab.X86_64.pushTable`), of `rsi` to the class in `rdi`,
against `Spec.Slab.pushContract`: its three ways out (no slab for the CPU
and no slot to push to, both a tail call of the slow path; the commit), and
in each its run, where it may be aborted, and its memory accesses.
-/

@[expose] public section

namespace VG.Proof.Slab.X86_64.PushTable

open VG.X86_64 VG.Spec.Slab VG.Impl.Slab.X86_64 VG.Proof.Slab.X86_64

abbrev cx : Spec.Slab.Ctx isa := ⟨abi, .table, pushSig⟩

def body : List (Step Instr Cond) := [
  .instr (.mov32 .r8 (.mem { base := .rdx, disp := 4, seg := some .fs })),
  .instr (.alu32 .cmp .r8 (.imm 8192)),
  .exit .ae [] (some "slow"),
  .instr (.shift .shl .r8 12),
  .instr (.leaSym .r9 "SLABS"),
  .instr (.lea .r9 { base := .r9, index := some .rdi, scale := 8 }),
  .instr (.mov .rcx (.mem slabWord)),
  .instr (.leaSym .r11 "MAG_MASKS"),
  .instr (.lea .rax { base := .rcx, disp := 8 }),
  .instr (.alu32 .test .rax maskEntry),
  .exit .e [] (some "slow"),
  .instr (.store { base := .rcx } .rsi),
  .instr (.store slabWord .rax)]

theorem code_eq : pushTable = .rseq arm body := rfl

/-! ## The states -/

/-- The pointer pushed. -/
def x (s : State) : BitVec 64 := s.gpr .rsi
def σ8 (s : State) : State := (σ7t s).setReg .rax (wvt s + 8)
def tst (s : State) : BitVec 32 := (wvt s + 8).setWidth 32 &&& mk s
def σ9 (s : State) : State := arithFlags (σ8 s) (tst s) false false
def mSlot (s : State) : Mem := (m1 s).writeW (wvt s) (x s)
def mEnd (s : State) : Mem := (mSlot s).writeW (wat s) (wvt s + 8)
def σ11 (s : State) : State := { σ9 s with mem := mSlot s }
def σ12 (s : State) : State := { σ11 s with mem := mEnd s }

/-! ## What the precondition gives -/

structure Pre (s : State) (k : Nat) : Prop where
  facts : Facts cx s
  hk : 2 ≤ k
  hk' : k ≤ 16
  mask : (mk s).toNat = magMask (2 ^ k)
  slots : cx.slots s = 2 ^ k
  word : wat s = cx.word s
  maskA : maskA s = cx.maskAddr s
  w : cx.w s = wvt s
  ok : cx.InRange s → WordOk (2 ^ k) (wvt s)

theorem cls_eq (s : State) : cx.cls s = (s.gpr .rdi).toNat := rfl

theorem pre_of {s : State} (h : Facts cx s) : ∃ k, Pre s k := by
  obtain ⟨-, -, -, -, ⟨k, hk, hk', hm⟩, hinv⟩ := h.pre
  have hma : maskA s = cx.maskAddr s := maskA_eq rfl (cls_eq s)
  have hmask : cx.mask s = (mk s).toNat := by
    show (s.mem.readW (cx.maskAddr s) 32).toNat = _; rw [← hma]; rfl
  have hslots : cx.slots s = 2 ^ k := by
    rw [Spec.Slab.Ctx.slots, hm]; exact slotsOf_magMask k hk
  have hw : wat s = cx.word s := wat_eq rfl (cls_eq s)
  have hwv : cx.w s = wvt s := by simp only [Spec.Slab.Ctx.w, view_mem, wvt, hw]
  refine ⟨k, h, hk, hk', by rw [← hmask]; exact hm, hslots, hw, hma, hwv, fun hi => ?_⟩
  have := hinv (cx.cpu s) hi
  rw [← hslots, ← hwv]
  exact this

/-! ## The steps after the slab word -/

section Steps

variable {s : State} {k : Nat}

theorem exec_add8 : exec (.lea .rax { base := .rcx, disp := 8 }) (σ7t s) = some (σ8 s) := by
  rw [exec_lea, Access.offset_base_disp, σ7t_rcx]; rfl

theorem σ8_rax : (σ8 s).gpr .rax = wvt s + 8 := RegUpd.gpr_setReg_self _ _ _
theorem σ8_rcx : (σ8 s).gpr .rcx = wvt s := by
  rw [σ8, RegUpd.gpr_setReg_of_ne _ _ (by decide), σ7t_rcx]
theorem σ8_rdi : (σ8 s).gpr .rdi = s.gpr .rdi := by
  rw [σ8, RegUpd.gpr_setReg_of_ne _ _ (by decide), σ7t_rdi]
theorem σ8_r11 : (σ8 s).gpr .r11 = s.syms "MAG_MASKS" := by
  rw [σ8, RegUpd.gpr_setReg_of_ne _ _ (by decide), σ7t_r11]
theorem σ8_rd : (σ8 s).rd = s.rd := rfl
theorem σ8_wr : (σ8 s).wr = s.wr := rfl
theorem σ8_mem : (σ8 s).mem = m1 s := rfl

theorem exec_test (hp : Pre s k) : exec (.alu32 .test .rax maskEntry) (σ8 s) = some (σ9 s) := by
  have hin := inMask hp.facts rfl hp.maskA (σ8 s) σ8_rd σ8_wr
  rw [← maskEntry_ea (σ8 s) σ8_r11 σ8_rdi] at hin
  rw [maskEntry, exec_test32_mem _ hin, maskEntry_ea _ σ8_r11 σ8_rdi, σ8_mem,
    m1_mask hp.facts rfl hp.maskA, σ8_rax]
  rfl

theorem eval_test : eval .e (σ9 s) = some (tst s == 0) := rfl

theorem σ9_wr : (σ9 s).wr = s.wr := rfl
theorem σ9_mem : (σ9 s).mem = m1 s := rfl
theorem σ9_rcx : (σ9 s).gpr .rcx = wvt s := by rw [σ9, RegUpd.gpr_arithFlags, σ8_rcx]
theorem σ9_rsi : (σ9 s).gpr .rsi = x s := by
  rw [σ9, RegUpd.gpr_arithFlags, σ8, RegUpd.gpr_setReg_of_ne _ _ (by decide), σ7t_rsi]; rfl

/-- What a push that passes the test finds. -/
theorem hit (hp : Pre s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    IsSlot (2 ^ k) (wvt s) ∧ slot (2 ^ k) (wvt s) + 1 < 2 ^ k ∧
      (Mag.ofWord (2 ^ k) s.mem (wvt s)).push (2 ^ k) (x s) =
        some (.stack (magBase (2 ^ k) (wvt s)) (items (2 ^ k) s.mem (wvt s) ++ [x s])) := by
  have hm : ¬ (wvt s + 8).toNat % (8 * 2 ^ k) < 16 := fun h' =>
    ht ((test_iff32 hp.hk hp.hk' (wvt s + 8) (mk s) hp.mask).mpr h')
  exact push_of_test k rfl hp.hk hp.hk' (hp.ok hi) s.mem (x s) hm

theorem magRegion_eq (hp : Pre s k) : cx.magRegion s = ⟨magBase (2 ^ k) (wvt s), 8 * 2 ^ k⟩ := by
  simp only [Spec.Slab.Ctx.magRegion, hp.slots, hp.w]

structure SlotFacts (s : State) (k : Nat) : Prop where
  inMag : (⟨magBase (2 ^ k) (wvt s), 8 * 2 ^ k⟩ : Region).Contains (wvt s) 8
  mem : (⟨magBase (2 ^ k) (wvt s), 8 * 2 ^ k⟩ : Region) ∈ s.wr
  cs : Region.Disjoint ⟨wvt s, 8⟩ ⟨area s + 8, 8⟩
  word : Region.Disjoint ⟨wvt s, 8⟩ ⟨wat s, 8⟩
  rsVar : Region.Disjoint ⟨s.syms RS, 8⟩ ⟨wvt s, 8⟩
  ret : Region.Disjoint ⟨wvt s, 8⟩ ⟨s.gpr .rsp, 8⟩

theorem slotFacts (hp : Pre s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) : SlotFacts s k := by
  obtain ⟨hs, hlt, -⟩ := hit hp hi ht
  have hsl : IsSlot (cx.slots s) (cx.w s) := by rw [hp.slots, hp.w]; exact hs
  have hc := contains_at hs hlt
  have hsub := sub_of_contains hc
  have hmag := hp.facts.mag hi hsl
  have hcm := hp.facts.cs_mag hi hsl
  have hwm := hp.facts.word_mag hi hsl
  have hrm := hp.facts.rsVar_mag hi hsl
  have hret := hp.facts.ret_mag hi hsl
  rw [magRegion_eq hp] at hmag hcm hwm hrm hret
  exact ⟨hc, hmag, hcm.symm.sub_left hsub, by rw [hp.word]; exact hwm.symm.sub_left hsub,
    hrm.sub_right hsub, hret.sub_left hsub⟩

theorem exec_slot (hp : Pre s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    exec (.store { base := .rcx } .rsi) (σ9 s) = some (σ11 s) := by
  have hf := slotFacts hp hi ht
  have hin : InRegions (σ9 s).wr ((σ9 s).ea { base := .rcx }) 8 := by
    rw [Access.ea_base, σ9_rcx, σ9_wr]; exact InRegions.of_mem hf.mem hf.inMag
  rw [exec_store _ hin, Access.ea_base, σ9_rcx, σ9_rsi, σ9_mem]
  rfl

theorem σ11_r9 : (σ11 s).gpr .r9 = r9t s := by
  show (σ9 s).gpr .r9 = _
  simp only [σ9, σ8, σ7t, σ6t, RegUpd.gpr_arithFlags,
    RegUpd.gpr_setReg_of_ne _ _ (show ¬ Reg.r9 = .rax by decide),
    RegUpd.gpr_setReg_of_ne _ _ (show ¬ Reg.r9 = .r11 by decide),
    RegUpd.gpr_setReg_of_ne _ _ (show ¬ Reg.r9 = .rcx by decide), σ5t_r9]
theorem σ11_r8 : (σ11 s).gpr .r8 = sh s := by
  show (σ9 s).gpr .r8 = _
  simp only [σ9, σ8, σ7t, σ6t, RegUpd.gpr_arithFlags,
    RegUpd.gpr_setReg_of_ne _ _ (show ¬ Reg.r8 = .rax by decide),
    RegUpd.gpr_setReg_of_ne _ _ (show ¬ Reg.r8 = .r11 by decide),
    RegUpd.gpr_setReg_of_ne _ _ (show ¬ Reg.r8 = .rcx by decide), σ5t_r8]
theorem σ11_rax : (σ11 s).gpr .rax = wvt s + 8 := by
  show (σ9 s).gpr .rax = _
  rw [σ9, RegUpd.gpr_arithFlags, σ8_rax]
theorem σ11_mem : (σ11 s).mem = mSlot s := rfl
theorem σ11_wr : (σ11 s).wr = s.wr := rfl

theorem exec_commit (hp : Pre s k) (hi : cx.InRange s) :
    exec (.store slabWord .rax) (σ11 s) = some (σ12 s) := by
  have hin : InRegions (σ11 s).wr ((σ11 s).ea slabWord) 8 := by
    rw [slabWord_ea_t _ σ11_r9 σ11_r8, σ11_wr, hp.word]
    exact inWr (r := ⟨cx.word s, 8⟩) (hp.facts.word hi)
  rw [exec_store _ hin, slabWord_ea_t _ σ11_r9 σ11_r8, σ11_rax, σ11_mem]
  rfl

theorem inRange_iff : cx.InRange s ↔ (cpu32 s).toNat < 8192 := by
  simp only [Spec.Slab.Ctx.InRange, cpu_of (c := cx) rfl, cpu32, SLAB_COUNT]

theorem eval_cpu_true (hi : ¬ cx.InRange s) : eval .ae (σ2 s) = some true := by
  rw [eval_cpu, inRange_iff] at *; simp [hi]

theorem eval_cpu_false (hi : cx.InRange s) : eval .ae (σ2 s) = some false := by
  rw [eval_cpu, inRange_iff] at *; simp [hi]

theorem eval_test_true (ht : tst s = 0) : eval .e (σ9 s) = some true := by
  rw [eval_test, ht]; rfl

theorem eval_test_false (ht : ¬ tst s = 0) : eval .e (σ9 s) = some false := by
  rw [eval_test]; simpa using ht

end Steps

/-! ## The three ways out -/

section Cases

variable {s : State} {k : Nat}

theorem runBody_C (hp : Pre s k) (hi : ¬ cx.InRange s) :
    runBody isa body (σA s) = some (.exit "slow" (σ2 s)) := by
  rw [body, runBody_instr (exec_cpu hp.facts), runBody_instr exec_cmp,
    runBody_take (eval_cpu_true hi) execBlock_nil']

theorem prefixes_C (hp : Pre s k) (hi : ¬ cx.InRange s) :
    bodyPrefixes isa body (σA s) = [σA s, σ1 s, σ2 s] := by
  rw [body, bodyPrefixes_instr (exec_cpu hp.facts), bodyPrefixes_instr exec_cmp,
    bodyPrefixes_take (eval_cpu_true hi)]

theorem end_C (hp : Pre s k) (hi : ¬ cx.InRange s) : bodyEnd isa body (σA s) = some 2 := by
  rw [body, bodyEnd_instr (exec_cpu hp.facts), bodyEnd_instr exec_cmp,
    bodyEnd_take (eval_cpu_true hi)]
  rfl

theorem runBody_T (hp : Pre s k) (hi : cx.InRange s) (ht : tst s = 0) :
    runBody isa body (σA s) = some (.exit "slow" (σ9 s)) := by
  rw [body, runBody_instr (exec_cpu hp.facts), runBody_instr exec_cmp,
    runBody_skip (eval_cpu_false hi), runBody_instr exec_shl_r8, runBody_instr exec_r9t,
    runBody_instr exec_r9idx, runBody_instr (exec_wordt hp.facts hi hp.word), runBody_instr exec_r11,
    runBody_instr exec_add8, runBody_instr (exec_test hp),
    runBody_take (eval_test_true ht) execBlock_nil']

theorem prefixes_T (hp : Pre s k) (hi : cx.InRange s) (ht : tst s = 0) :
    bodyPrefixes isa body (σA s) =
      [σA s, σ1 s, σ2 s, σ2 s, σ3 s, σ4t s, σ5t s, σ6t s, σ7t s, σ8 s, σ9 s] := by
  rw [body, bodyPrefixes_instr (exec_cpu hp.facts), bodyPrefixes_instr exec_cmp,
    bodyPrefixes_skip (eval_cpu_false hi), bodyPrefixes_instr exec_shl_r8,
    bodyPrefixes_instr exec_r9t, bodyPrefixes_instr exec_r9idx,
    bodyPrefixes_instr (exec_wordt hp.facts hi hp.word), bodyPrefixes_instr exec_r11,
    bodyPrefixes_instr exec_add8, bodyPrefixes_instr (exec_test hp),
    bodyPrefixes_take (eval_test_true ht)]

theorem end_T (hp : Pre s k) (hi : cx.InRange s) (ht : tst s = 0) :
    bodyEnd isa body (σA s) = some 10 := by
  rw [body, bodyEnd_instr (exec_cpu hp.facts), bodyEnd_instr exec_cmp,
    bodyEnd_skip (eval_cpu_false hi), bodyEnd_instr exec_shl_r8, bodyEnd_instr exec_r9t,
    bodyEnd_instr exec_r9idx, bodyEnd_instr (exec_wordt hp.facts hi hp.word), bodyEnd_instr exec_r11,
    bodyEnd_instr exec_add8, bodyEnd_instr (exec_test hp), bodyEnd_take (eval_test_true ht)]
  rfl

theorem runBody_P (hp : Pre s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    runBody isa body (σA s) = some (.done (σ12 s)) := by
  rw [body, runBody_instr (exec_cpu hp.facts), runBody_instr exec_cmp,
    runBody_skip (eval_cpu_false hi), runBody_instr exec_shl_r8, runBody_instr exec_r9t,
    runBody_instr exec_r9idx, runBody_instr (exec_wordt hp.facts hi hp.word), runBody_instr exec_r11,
    runBody_instr exec_add8, runBody_instr (exec_test hp), runBody_skip (eval_test_false ht),
    runBody_instr (exec_slot hp hi ht), runBody_instr (exec_commit hp hi), runBody_nil']

theorem prefixes_P (hp : Pre s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    bodyPrefixes isa body (σA s) =
      [σA s, σ1 s, σ2 s, σ2 s, σ3 s, σ4t s, σ5t s, σ6t s, σ7t s, σ8 s, σ9 s, σ9 s, σ11 s] := by
  rw [body, bodyPrefixes_instr (exec_cpu hp.facts), bodyPrefixes_instr exec_cmp,
    bodyPrefixes_skip (eval_cpu_false hi), bodyPrefixes_instr exec_shl_r8,
    bodyPrefixes_instr exec_r9t, bodyPrefixes_instr exec_r9idx,
    bodyPrefixes_instr (exec_wordt hp.facts hi hp.word), bodyPrefixes_instr exec_r11,
    bodyPrefixes_instr exec_add8, bodyPrefixes_instr (exec_test hp),
    bodyPrefixes_skip (eval_test_false ht), bodyPrefixes_instr (exec_slot hp hi ht),
    bodyPrefixes_last (exec_commit hp hi)]

theorem end_P (hp : Pre s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    bodyEnd isa body (σA s) = some 13 := by
  rw [body, bodyEnd_instr (exec_cpu hp.facts), bodyEnd_instr exec_cmp,
    bodyEnd_skip (eval_cpu_false hi), bodyEnd_instr exec_shl_r8, bodyEnd_instr exec_r9t,
    bodyEnd_instr exec_r9idx, bodyEnd_instr (exec_wordt hp.facts hi hp.word), bodyEnd_instr exec_r11,
    bodyEnd_instr exec_add8, bodyEnd_instr (exec_test hp), bodyEnd_skip (eval_test_false ht),
    bodyEnd_instr (exec_slot hp hi ht), bodyEnd_instr (exec_commit hp hi), bodyEnd_nil']
  rfl

/-! ### Memory accesses -/

theorem ev_cpu :
    Instr.events (.mov32 .r8 (.mem { base := .rdx, disp := 4, seg := some .fs })) (σA s) (σ1 s) =
      [⟨area s + 4, 4, false, (cpu32 s).toNat⟩] := by
  simp only [Instr.events, cpuOp_ea, σA_mem, toNat_read4, m1]
  rw [readW_writeW_disj (cpu_cs_disjoint s) (by decide)]; rfl

theorem ev_word (hp : Pre s k) (hi : cx.InRange s) :
    Instr.events (.mov .rcx (.mem slabWord)) (σ5t s) (σ6t s) = [⟨wat s, 8, false, (wvt s).toNat⟩] := by
  simp only [Instr.events, slabWord_ea_t _ σ5t_r9 σ5t_r8, σ5t_mem, toNat_read8, m1]
  rw [readW64_writeW_disj (by rw [hp.word]; exact (hp.facts.cs_word hi).symm)]; rfl

theorem ev_mask (hp : Pre s k) :
    Instr.events (.alu32 .test .rax maskEntry) (σ8 s) (σ9 s) = [⟨maskA s, 4, false, (mk s).toNat⟩] := by
  simp only [Instr.events, maskEntry, maskEntry_ea _ σ8_r11 σ8_rdi, σ8_mem, toNat_read4,
    m1_mask hp.facts rfl hp.maskA]

theorem ev_slot : Instr.events (.store { base := .rcx } .rsi) (σ9 s) (σ11 s) =
    [⟨wvt s, 8, true, (x s).toNat⟩] := by
  simp only [Instr.events, Access.ea_base, σ9_rcx, toNat_read8]
  rw [σ11_mem, mSlot, Mem.readW_writeW_self64]

theorem ev_commit :
    Instr.events (.store slabWord .rax) (σ11 s) (σ12 s) = [⟨wat s, 8, true, (wvt s + 8).toNat⟩] := by
  simp only [Instr.events, slabWord_ea_t _ σ11_r9 σ11_r8, toNat_read8]
  rw [show (σ12 s).mem = mEnd s from rfl, mEnd, Mem.readW_writeW_self64]

theorem events_C (hp : Pre s k) (hi : ¬ cx.InRange s) :
    bodyEvents isa body (σA s) = [⟨area s + 4, 4, false, (cpu32 s).toNat⟩] := by
  rw [body, bodyEvents_instr (exec_cpu hp.facts), bodyEvents_instr exec_cmp,
    bodyEvents_take (eval_cpu_true hi)]
  simp only [isa, ev_cpu]; rfl

theorem events_T (hp : Pre s k) (hi : cx.InRange s) (ht : tst s = 0) :
    bodyEvents isa body (σA s) =
      [⟨area s + 4, 4, false, (cpu32 s).toNat⟩, ⟨wat s, 8, false, (wvt s).toNat⟩,
        ⟨maskA s, 4, false, (mk s).toNat⟩] := by
  rw [body, bodyEvents_instr (exec_cpu hp.facts), bodyEvents_instr exec_cmp,
    bodyEvents_skip (eval_cpu_false hi), bodyEvents_instr exec_shl_r8, bodyEvents_instr exec_r9t,
    bodyEvents_instr exec_r9idx, bodyEvents_instr (exec_wordt hp.facts hi hp.word),
    bodyEvents_instr exec_r11, bodyEvents_instr exec_add8, bodyEvents_instr (exec_test hp),
    bodyEvents_take (eval_test_true ht)]
  simp only [isa, ev_cpu, ev_word hp hi, ev_mask hp]; rfl

theorem events_P (hp : Pre s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    bodyEvents isa body (σA s) =
      [⟨area s + 4, 4, false, (cpu32 s).toNat⟩, ⟨wat s, 8, false, (wvt s).toNat⟩,
        ⟨maskA s, 4, false, (mk s).toNat⟩, ⟨wvt s, 8, true, (x s).toNat⟩,
        ⟨wat s, 8, true, (wvt s + 8).toNat⟩] := by
  rw [body, bodyEvents_instr (exec_cpu hp.facts), bodyEvents_instr exec_cmp,
    bodyEvents_skip (eval_cpu_false hi), bodyEvents_instr exec_shl_r8, bodyEvents_instr exec_r9t,
    bodyEvents_instr exec_r9idx, bodyEvents_instr (exec_wordt hp.facts hi hp.word),
    bodyEvents_instr exec_r11, bodyEvents_instr exec_add8, bodyEvents_instr (exec_test hp),
    bodyEvents_skip (eval_test_false ht), bodyEvents_instr (exec_slot hp hi ht),
    bodyEvents_instr (exec_commit hp hi), bodyEvents_nil']
  simp only [ev_cpu, ev_word hp hi, ev_mask hp, ev_slot, ev_commit]; rfl

end Cases

/-! ## What each state keeps -/

section Mids

variable {s : State}

theorem mσ2 : Mid scratchT s (m1 s) (σ2 s) :=
  mid_σ2 (mid_σ1 (by decide) (mid_σA (by decide) (by decide)))
theorem mσ3 : Mid scratchT s (m1 s) (σ3 s) := mid_σ3 (by decide) mσ2
theorem mσ4t : Mid scratchT s (m1 s) (σ4t s) := (mσ7t mσ3 (by decide) (by decide) (by decide)).1
theorem mσ5t : Mid scratchT s (m1 s) (σ5t s) := (mσ7t mσ3 (by decide) (by decide) (by decide)).2.1
theorem mσ6t : Mid scratchT s (m1 s) (σ6t s) := (mσ7t mσ3 (by decide) (by decide) (by decide)).2.2.1
theorem mσ7 : Mid scratchT s (m1 s) (σ7t s) := (mσ7t mσ3 (by decide) (by decide) (by decide)).2.2.2
theorem mσ8 : Mid scratchT s (m1 s) (σ8 s) := mσ7.setReg (by decide) _
theorem mσ9 : Mid scratchT s (m1 s) (σ9 s) := mσ8.arithFlags _ _ _
theorem mσ11 : Mid scratchT s (mSlot s) (σ11 s) := mσ9.withMem _
theorem mσ12 : Mid scratchT s (mEnd s) (σ12 s) := mσ11.withMem _

end Mids

end VG.Proof.Slab.X86_64.PushTable
