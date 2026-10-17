module

public import Asmalloc.Proof.Slab.X86_64.Inline
public import Asmalloc.Proof.Slab.X86_64.Mid

/-!
# The inline push on x86-64

`slab_push` inline (`Impl.Slab.X86_64.pushInline`) against
`Spec.Slab.pushInlineContract`: its three ways out (no slab for the CPU and
no slot to push to, both to the label `miss`; the commit), and in each its
run, where it may be aborted, and its memory accesses.
-/

@[expose] public section

namespace VG.Proof.Slab.X86_64.PushInline

open VG.X86_64 VG.Spec.Slab VG.Impl.Slab.X86_64 VG.Proof.Slab.X86_64

abbrev cx : Spec.Slab.Ctx isa := ⟨abi, .inline, pushInlineSig⟩

def body : List (Step Instr Cond) := [
  .instr (.mov32 .r8 (.mem { base := .r10, disp := 4, seg := some .fs })),
  .instr (.alu32 .cmp .r8 (.imm 8192)),
  .exit .ae [] (some "miss"),
  .instr (.shift .shl .r8 12),
  .instr (.leaSym .r9 "SLABS" 0 (some "coff")),
  .instr (.mov .rcx (.mem slabWord)),
  .instr (.lea .rax { base := .rcx, disp := 8 }),
  .instr (.alu32 .test .rax (.immP "mask")),
  .exit .e [] (some "miss"),
  .instr (.store { base := .rcx } .rdi),
  .instr (.store slabWord .rax)]

theorem code_eq : pushInline = .rseq arm body := rfl

/-! ## The states -/

/-- The pointer pushed. -/
def x (s : State) : BitVec 64 := s.gpr .rdi
def σ6 (s : State) : State := (σ5i s).setReg .rax (wvi s + 8)
def tst (s : State) : BitVec 32 := (wvi s + 8).setWidth 32 &&& (s.params "mask").setWidth 32
def σ7 (s : State) : State := arithFlags (σ6 s) (tst s) false false
/-- Memory after the store into the free slot, and after the commit. -/
def mSlot (s : State) : Mem := (m1 s).writeW (wvi s) (x s)
def mEnd (s : State) : Mem := (mSlot s).writeW (wai s) (wvi s + 8)
def σ9 (s : State) : State := { σ7 s with mem := mSlot s }
def σ10 (s : State) : State := { σ9 s with mem := mEnd s }

/-! ## What the precondition gives -/

structure Pre (s : State) (k : Nat) : Prop where
  facts : Facts cx s
  hk : 2 ≤ k
  hk' : k ≤ 16
  mask : (s.params "mask").toNat = magMask (2 ^ k)
  slots : cx.slots s = 2 ^ k
  word : wai s = cx.word s
  w : cx.w s = wvi s
  ok : cx.InRange s → WordOk (2 ^ k) (wvi s)

theorem pre_of {s : State} (h : Facts cx s) : ∃ k, Pre s k := by
  obtain ⟨-, -, ⟨-, hcoff⟩, -, ⟨k, hk, hk', hm⟩, hinv⟩ := h.pre
  have hmask : cx.mask s = (s.params "mask").toNat := rfl
  have hslots : cx.slots s = 2 ^ k := by
    rw [Spec.Slab.Ctx.slots, hm]; exact slotsOf_magMask k hk
  have hw : wai s = cx.word s := wai_eq rfl hcoff
  have hwv : cx.w s = wvi s := by simp only [Spec.Slab.Ctx.w, view_mem, wvi, hw]
  refine ⟨k, h, hk, hk', by rw [← hmask]; exact hm, hslots, hw, hwv, fun hi => ?_⟩
  have := hinv (cx.cpu s) hi
  rw [← hslots, ← hwv]
  exact this

/-! ## The steps after the slab word -/

section Steps

variable {s : State} {k : Nat}

theorem exec_add8 : exec (.lea .rax { base := .rcx, disp := 8 }) (σ5i s) = some (σ6 s) := by
  rw [exec_lea, Access.offset_base_disp, σ5i_rcx]; rfl

theorem σ6_rax : (σ6 s).gpr .rax = wvi s + 8 := RegUpd.gpr_setReg_self _ _ _
theorem σ6_rcx : (σ6 s).gpr .rcx = wvi s := by
  rw [σ6, RegUpd.gpr_setReg_of_ne _ _ (by decide), σ5i_rcx]

theorem exec_test : exec (.alu32 .test .rax (.immP "mask")) (σ6 s) = some (σ7 s) := by
  rw [exec_test32_immP, σ6_rax]; rfl

theorem eval_test : eval .e (σ7 s) = some (tst s == 0) := rfl

theorem σ7_rd : (σ7 s).rd = s.rd := rfl
theorem σ7_wr : (σ7 s).wr = s.wr := rfl
theorem σ7_mem : (σ7 s).mem = m1 s := rfl
theorem σ7_rcx : (σ7 s).gpr .rcx = wvi s := by rw [σ7, RegUpd.gpr_arithFlags, σ6_rcx]
theorem σ7_rdi : (σ7 s).gpr .rdi = x s := by
  simp only [σ7, RegUpd.gpr_arithFlags, σ6, σ5i, σ4i,
    RegUpd.gpr_setReg_of_ne _ _ (show ¬ Reg.rdi = .rax by decide),
    RegUpd.gpr_setReg_of_ne _ _ (show ¬ Reg.rdi = .rcx by decide),
    RegUpd.gpr_setReg_of_ne _ _ (show ¬ Reg.rdi = .r9 by decide)]
  simp only [σ3, shl12, RegUpd.gpr_setReg_of_ne _ _ (show ¬ Reg.rdi = .r8 by decide),
    RegUpd.gpr_setFlags, σ2, RegUpd.gpr_arithFlags, σ1, State.setReg32, σA, armReg, armB, armA,
    RegUpd.gpr_setReg_of_ne _ _ (show ¬ Reg.rdi = .r11 by decide),
    RegUpd.gpr_setReg_of_ne _ _ (show ¬ Reg.rdi = .r10 by decide)]
  rfl

/-- What a push that passes the test finds. -/
theorem hit (hp : Pre s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    IsSlot (2 ^ k) (wvi s) ∧ slot (2 ^ k) (wvi s) + 1 < 2 ^ k ∧
      (Mag.ofWord (2 ^ k) s.mem (wvi s)).push (2 ^ k) (x s) =
        some (.stack (magBase (2 ^ k) (wvi s)) (items (2 ^ k) s.mem (wvi s) ++ [x s])) := by
  have hm : ¬ (wvi s + 8).toNat % (8 * 2 ^ k) < 16 := fun h' =>
    ht ((test_iff hp.hk hp.hk' (wvi s + 8) (s.params "mask") hp.mask).mpr h')
  exact push_of_test k rfl hp.hk hp.hk' (hp.ok hi) s.mem (x s) hm

theorem magRegion_eq (hp : Pre s k) : cx.magRegion s = ⟨magBase (2 ^ k) (wvi s), 8 * 2 ^ k⟩ := by
  simp only [Spec.Slab.Ctx.magRegion, hp.slots, hp.w]

/-- The free slot's region: in the magazine, apart from `rseq_cs`, the slab
word and `__rseq_offset`. -/
structure SlotFacts (s : State) (k : Nat) : Prop where
  inMag : (⟨magBase (2 ^ k) (wvi s), 8 * 2 ^ k⟩ : Region).Contains (wvi s) 8
  mem : (⟨magBase (2 ^ k) (wvi s), 8 * 2 ^ k⟩ : Region) ∈ s.wr
  cs : Region.Disjoint ⟨wvi s, 8⟩ ⟨area s + 8, 8⟩
  word : Region.Disjoint ⟨wvi s, 8⟩ ⟨wai s, 8⟩
  rsVar : Region.Disjoint ⟨s.syms RS, 8⟩ ⟨wvi s, 8⟩
  ret : Region.Disjoint ⟨wvi s, 8⟩ ⟨s.gpr .rsp, 8⟩

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
    exec (.store { base := .rcx } .rdi) (σ7 s) = some (σ9 s) := by
  have hf := slotFacts hp hi ht
  have hin : InRegions (σ7 s).wr ((σ7 s).ea { base := .rcx }) 8 := by
    rw [Access.ea_base, σ7_rcx, σ7_wr]; exact InRegions.of_mem hf.mem hf.inMag
  rw [exec_store _ hin, Access.ea_base, σ7_rcx, σ7_rdi, σ7_mem]
  rfl

theorem σ9_r9 : (σ9 s).gpr .r9 = r9i s := by
  show (σ7 s).gpr .r9 = _
  simp only [σ7, σ6, RegUpd.gpr_arithFlags, RegUpd.gpr_setReg_of_ne _ _ (show ¬ Reg.r9 = .rax by decide),
    σ5i_r9]
theorem σ9_r8 : (σ9 s).gpr .r8 = sh s := by
  show (σ7 s).gpr .r8 = _
  simp only [σ7, σ6, RegUpd.gpr_arithFlags, RegUpd.gpr_setReg_of_ne _ _ (show ¬ Reg.r8 = .rax by decide),
    σ5i_r8]
theorem σ9_rax : (σ9 s).gpr .rax = wvi s + 8 := by
  show (σ7 s).gpr .rax = _
  rw [σ7, RegUpd.gpr_arithFlags, σ6_rax]
theorem σ9_mem : (σ9 s).mem = mSlot s := rfl
theorem σ9_wr : (σ9 s).wr = s.wr := rfl

theorem exec_commit (hp : Pre s k) (hi : cx.InRange s) :
    exec (.store slabWord .rax) (σ9 s) = some (σ10 s) := by
  have hin : InRegions (σ9 s).wr ((σ9 s).ea slabWord) 8 := by
    rw [slabWord_ea_of _ σ9_r9 σ9_r8, σ9_wr, hp.word]
    exact inWr (r := ⟨cx.word s, 8⟩) (hp.facts.word hi)
  rw [exec_store _ hin, slabWord_ea_of _ σ9_r9 σ9_r8, σ9_rax, σ9_mem]
  rfl

theorem inRange_iff : cx.InRange s ↔ (cpu32 s).toNat < 8192 := by
  simp only [Spec.Slab.Ctx.InRange, cpu_of (c := cx) rfl, cpu32, SLAB_COUNT]

theorem eval_cpu_true (hi : ¬ cx.InRange s) : eval .ae (σ2 s) = some true := by
  rw [eval_cpu, inRange_iff] at *; simp [hi]

theorem eval_cpu_false (hi : cx.InRange s) : eval .ae (σ2 s) = some false := by
  rw [eval_cpu, inRange_iff] at *; simp [hi]

theorem eval_test_true (ht : tst s = 0) : eval .e (σ7 s) = some true := by
  rw [eval_test, ht]; rfl

theorem eval_test_false (ht : ¬ tst s = 0) : eval .e (σ7 s) = some false := by
  rw [eval_test]; simpa using ht

end Steps

/-! ## The three ways out -/

section Cases

variable {s : State} {k : Nat}

theorem runBody_C (hp : Pre s k) (hi : ¬ cx.InRange s) :
    runBody isa body (σA s) = some (.exit "miss" (σ2 s)) := by
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
    runBody isa body (σA s) = some (.exit "miss" (σ7 s)) := by
  rw [body, runBody_instr (exec_cpu hp.facts), runBody_instr exec_cmp,
    runBody_skip (eval_cpu_false hi), runBody_instr exec_shl_r8, runBody_instr exec_r9i,
    runBody_instr (exec_wordi hp.facts hi hp.word), runBody_instr exec_add8,
    runBody_instr exec_test, runBody_take (eval_test_true ht) execBlock_nil']

theorem prefixes_T (hp : Pre s k) (hi : cx.InRange s) (ht : tst s = 0) :
    bodyPrefixes isa body (σA s) = [σA s, σ1 s, σ2 s, σ2 s, σ3 s, σ4i s, σ5i s, σ6 s, σ7 s] := by
  rw [body, bodyPrefixes_instr (exec_cpu hp.facts), bodyPrefixes_instr exec_cmp,
    bodyPrefixes_skip (eval_cpu_false hi), bodyPrefixes_instr exec_shl_r8,
    bodyPrefixes_instr exec_r9i, bodyPrefixes_instr (exec_wordi hp.facts hi hp.word),
    bodyPrefixes_instr exec_add8, bodyPrefixes_instr exec_test,
    bodyPrefixes_take (eval_test_true ht)]

theorem end_T (hp : Pre s k) (hi : cx.InRange s) (ht : tst s = 0) :
    bodyEnd isa body (σA s) = some 8 := by
  rw [body, bodyEnd_instr (exec_cpu hp.facts), bodyEnd_instr exec_cmp,
    bodyEnd_skip (eval_cpu_false hi), bodyEnd_instr exec_shl_r8, bodyEnd_instr exec_r9i,
    bodyEnd_instr (exec_wordi hp.facts hi hp.word), bodyEnd_instr exec_add8,
    bodyEnd_instr exec_test, bodyEnd_take (eval_test_true ht)]
  rfl

theorem runBody_P (hp : Pre s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    runBody isa body (σA s) = some (.done (σ10 s)) := by
  rw [body, runBody_instr (exec_cpu hp.facts), runBody_instr exec_cmp,
    runBody_skip (eval_cpu_false hi), runBody_instr exec_shl_r8, runBody_instr exec_r9i,
    runBody_instr (exec_wordi hp.facts hi hp.word), runBody_instr exec_add8,
    runBody_instr exec_test, runBody_skip (eval_test_false ht),
    runBody_instr (exec_slot hp hi ht), runBody_instr (exec_commit hp hi), runBody_nil']

theorem prefixes_P (hp : Pre s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    bodyPrefixes isa body (σA s) =
      [σA s, σ1 s, σ2 s, σ2 s, σ3 s, σ4i s, σ5i s, σ6 s, σ7 s, σ7 s, σ9 s] := by
  rw [body, bodyPrefixes_instr (exec_cpu hp.facts), bodyPrefixes_instr exec_cmp,
    bodyPrefixes_skip (eval_cpu_false hi), bodyPrefixes_instr exec_shl_r8,
    bodyPrefixes_instr exec_r9i, bodyPrefixes_instr (exec_wordi hp.facts hi hp.word),
    bodyPrefixes_instr exec_add8, bodyPrefixes_instr exec_test,
    bodyPrefixes_skip (eval_test_false ht), bodyPrefixes_instr (exec_slot hp hi ht),
    bodyPrefixes_last (exec_commit hp hi)]

theorem end_P (hp : Pre s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    bodyEnd isa body (σA s) = some 11 := by
  rw [body, bodyEnd_instr (exec_cpu hp.facts), bodyEnd_instr exec_cmp,
    bodyEnd_skip (eval_cpu_false hi), bodyEnd_instr exec_shl_r8, bodyEnd_instr exec_r9i,
    bodyEnd_instr (exec_wordi hp.facts hi hp.word), bodyEnd_instr exec_add8,
    bodyEnd_instr exec_test, bodyEnd_skip (eval_test_false ht),
    bodyEnd_instr (exec_slot hp hi ht), bodyEnd_instr (exec_commit hp hi), bodyEnd_nil']
  rfl

/-! ### Memory accesses -/

theorem ev_cpu :
    Instr.events (.mov32 .r8 (.mem { base := .r10, disp := 4, seg := some .fs })) (σA s) (σ1 s) =
      [⟨area s + 4, 4, false, (cpu32 s).toNat⟩] := by
  simp only [Instr.events, cpuOp_ea, σA_mem, toNat_read4, m1]
  rw [readW_writeW_disj (cpu_cs_disjoint s) (by decide)]; rfl

theorem ev_word (hp : Pre s k) (hi : cx.InRange s) :
    Instr.events (.mov .rcx (.mem slabWord)) (σ4i s) (σ5i s) = [⟨wai s, 8, false, (wvi s).toNat⟩] := by
  simp only [Instr.events, slabWord_ea_i, σ4i_mem, toNat_read8, m1]
  rw [readW64_writeW_disj (by rw [hp.word]; exact (hp.facts.cs_word hi).symm)]; rfl

theorem ev_slot : Instr.events (.store { base := .rcx } .rdi) (σ7 s) (σ9 s) =
    [⟨wvi s, 8, true, (x s).toNat⟩] := by
  simp only [Instr.events, Access.ea_base, σ7_rcx, toNat_read8]
  rw [σ9_mem, mSlot, Mem.readW_writeW_self64]

theorem ev_commit :
    Instr.events (.store slabWord .rax) (σ9 s) (σ10 s) = [⟨wai s, 8, true, (wvi s + 8).toNat⟩] := by
  simp only [Instr.events, slabWord_ea_of _ σ9_r9 σ9_r8, toNat_read8]
  rw [show (σ10 s).mem = mEnd s from rfl, mEnd, Mem.readW_writeW_self64]

theorem events_C (hp : Pre s k) (hi : ¬ cx.InRange s) :
    bodyEvents isa body (σA s) = [⟨area s + 4, 4, false, (cpu32 s).toNat⟩] := by
  rw [body, bodyEvents_instr (exec_cpu hp.facts), bodyEvents_instr exec_cmp,
    bodyEvents_take (eval_cpu_true hi)]
  simp only [isa, ev_cpu]; rfl

theorem events_T (hp : Pre s k) (hi : cx.InRange s) (ht : tst s = 0) :
    bodyEvents isa body (σA s) =
      [⟨area s + 4, 4, false, (cpu32 s).toNat⟩, ⟨wai s, 8, false, (wvi s).toNat⟩] := by
  rw [body, bodyEvents_instr (exec_cpu hp.facts), bodyEvents_instr exec_cmp,
    bodyEvents_skip (eval_cpu_false hi), bodyEvents_instr exec_shl_r8, bodyEvents_instr exec_r9i,
    bodyEvents_instr (exec_wordi hp.facts hi hp.word), bodyEvents_instr exec_add8,
    bodyEvents_instr exec_test, bodyEvents_take (eval_test_true ht)]
  simp only [isa, ev_cpu, ev_word hp hi]; rfl

theorem events_P (hp : Pre s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    bodyEvents isa body (σA s) =
      [⟨area s + 4, 4, false, (cpu32 s).toNat⟩, ⟨wai s, 8, false, (wvi s).toNat⟩,
        ⟨wvi s, 8, true, (x s).toNat⟩, ⟨wai s, 8, true, (wvi s + 8).toNat⟩] := by
  rw [body, bodyEvents_instr (exec_cpu hp.facts), bodyEvents_instr exec_cmp,
    bodyEvents_skip (eval_cpu_false hi), bodyEvents_instr exec_shl_r8, bodyEvents_instr exec_r9i,
    bodyEvents_instr (exec_wordi hp.facts hi hp.word), bodyEvents_instr exec_add8,
    bodyEvents_instr exec_test, bodyEvents_skip (eval_test_false ht),
    bodyEvents_instr (exec_slot hp hi ht), bodyEvents_instr (exec_commit hp hi), bodyEvents_nil']
  simp only [ev_cpu, ev_word hp hi, ev_slot, ev_commit]; rfl

end Cases

/-! ## What each state keeps -/

section Mids

variable {s : State}

theorem mσ2 : Mid scratchI s (m1 s) (σ2 s) :=
  mid_σ2 (mid_σ1 (by decide) (mid_σA (by decide) (by decide)))
theorem mσ3 : Mid scratchI s (m1 s) (σ3 s) := mid_σ3 (by decide) mσ2
theorem mσ4i : Mid scratchI s (m1 s) (σ4i s) := mσ3.setReg (by decide) _
theorem mσ5i : Mid scratchI s (m1 s) (σ5i s) := mσ4i.setReg (by decide) _
theorem mσ6 : Mid scratchI s (m1 s) (σ6 s) := mσ5i.setReg (by decide) _
theorem mσ7 : Mid scratchI s (m1 s) (σ7 s) := mσ6.arithFlags _ _ _
theorem mσ9 : Mid scratchI s (mSlot s) (σ9 s) := mσ7.withMem _
theorem mσ10 : Mid scratchI s (mEnd s) (σ10 s) := mσ9.withMem _

end Mids

/-! ## The contract -/

section Contract

variable {s : State} {k : Nat}

theorem hv : cx.A.args ((cx.sig.words cx.A.ptrBits).map (·.bits cx.A.ptrBits)) =
    some (fun s => [s.gpr .rdi]) := rfl

theorem arg0 : cx.arg s 0 = x s := rfl

theorem facts_of_pre (hs : (pushInlineContract abi).pre s) : Facts cx s := by
  have := (pre_iff cx (s := s) hv rfl rfl).mp hs
  exact facts_of_world rfl this.2.1 this.2.2

/-- Item `i` of the magazine is apart from `rseq_cs`, the slab word, and the
free slot. -/
theorem item_disjoint (hp : Pre s k) (hi : cx.InRange s) (hs : IsSlot (2 ^ k) (wvi s)) {i : Nat}
    (hil : i < slot (2 ^ k) (wvi s) - 1) :
    Region.Disjoint ⟨magBase (2 ^ k) (wvi s) + BitVec.ofNat 64 (8 * (i + 1)), 8⟩ ⟨area s + 8, 8⟩ ∧
    Region.Disjoint ⟨magBase (2 ^ k) (wvi s) + BitVec.ofNat 64 (8 * (i + 1)), 8⟩ ⟨wai s, 8⟩ ∧
    Region.Disjoint ⟨magBase (2 ^ k) (wvi s) + BitVec.ofNat 64 (8 * (i + 1)), 8⟩ ⟨wvi s, 8⟩ := by
  have hsl : IsSlot (cx.slots s) (cx.w s) := by rw [hp.slots, hp.w]; exact hs
  have hlt := slot_lt (2 ^ k) (wvi s) (Nat.two_pow_pos k)
  have hsub := sub_item (n := 2 ^ k) (w := wvi s) (i := i) (by omega)
  have hcm := hp.facts.cs_mag hi hsl
  have hwm := hp.facts.word_mag hi hsl
  rw [magRegion_eq hp] at hcm hwm
  have hn : 8 * 2 ^ k ≤ 2 ^ 19 := by
    have := Nat.pow_le_pow_right (n := 2) (by decide) hp.hk'
    omega
  refine ⟨hcm.symm.sub_left hsub, by rw [hp.word]; exact hwm.symm.sub_left hsub, ?_⟩
  have hw := magBase_add_slot hs
  have := Offset.disjoint (magBase (2 ^ k) (wvi s)) (d := 8 * (i + 1)) (n := 8)
    (e := 8 * slot (2 ^ k) (wvi s)) (k := 8) (by omega) (by omega) (by omega)
  rwa [hw] at this

theorem pushed (hp : Pre s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    cx.Pushed s (cx.arg s 0) (σ10 s) := by
  obtain ⟨hs, hlt, hpush⟩ := hit hp hi ht
  have hf := slotFacts hp hi ht
  refine ⟨.stack (magBase (2 ^ k) (wvi s)) (items (2 ^ k) s.mem (wvi s) ++ [x s]), ?_, ?_, ?_, ?_⟩
  · simp only [Spec.Slab.Ctx.pushResult, hi, ↓reduceIte, Spec.Slab.Ctx.mag, hp.slots,
      mem_of (c := cx) rfl, hp.w, arg0]
    exact hpush
  · rw [csField_of (c := cx) rfl, ← hp.word, hp.w]
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
  rw [csField_of (c := cx) rfl, mem_of (c := cx) rfl]; exact frame_m1

theorem missed_C (hi : ¬ cx.InRange s) : cx.PushMissed s (cx.arg s 0) (σ2 s) := by
  refine ⟨?_, unchanged_cs⟩
  simp only [Spec.Slab.Ctx.pushResult, hi, ↓reduceIte]

theorem missed_T (hp : Pre s k) (hi : cx.InRange s) (ht : tst s = 0) :
    cx.PushMissed s (cx.arg s 0) (σ7 s) := by
  refine ⟨?_, unchanged_cs⟩
  simp only [Spec.Slab.Ctx.pushResult, hi, ↓reduceIte, Spec.Slab.Ctx.mag, hp.slots,
    mem_of (c := cx) rfl, hp.w]
  exact (push_eq_none_iff _ k rfl hp.hk hp.hk' (hp.ok hi)).mpr
    ((test_iff hp.hk hp.hk' (wvi s + 8) (s.params "mask") hp.mask).mp ht)

theorem post_eq (s' : State) : (pushInlineContract abi).post s s' ↔ cx.Pushed s (cx.arg s 0) s' :=
  post_iff cx hv s s'

theorem exit_eq (t : String) (s' : State) : (pushInlineContract abi).exit t s s' ↔
    [s'.gpr .rdi] = [s.gpr .rdi] ∧ (t = MISS ∧ cx.PushMissed s (cx.arg s 0) s') :=
  exit_iff cx hv t s s'

theorem correct (hs : (pushInlineContract abi).pre s) :
    (∃ s', Exec isa pushInline s s' ∧ inlineTarget.abiPreserved s s' ∧
      (pushInlineContract abi).post s s') ∨
    (∃ t s', ExecExit isa pushInline s t s' ∧ inlineTarget.abiPreserved s s' ∧
      (pushInlineContract abi).exit t s s') := by
  have h := facts_of_pre hs
  obtain ⟨k, hp⟩ := pre_of h
  rw [code_eq]
  by_cases hi : cx.InRange s
  · by_cases ht : tst s = 0
    · exact .inr ⟨"miss", σ7 s, .rseq (execBlock_arm h) (runBody_T hp hi ht), inline_abi mσ7,
        (exit_eq _ _).mpr ⟨by rw [mσ7.gpr _ (by decide)], rfl, missed_T hp hi ht⟩⟩
    · exact .inl ⟨σ10 s, .rseq (execBlock_arm h) (runBody_P hp hi ht), inline_abi mσ10,
        (post_eq _).mpr (pushed hp hi ht)⟩
  · exact .inr ⟨"miss", σ2 s, .rseq (execBlock_arm h) (runBody_C hp hi), inline_abi mσ2,
      (exit_eq _ _).mpr ⟨by rw [mσ2.gpr _ (by decide)], rfl, missed_C hi⟩⟩

/-! ### Where it may be aborted -/

/-- Before the free slot is written, the magazine is as on entry. -/
theorem mag_m1 (hp : Pre s k) (hi : cx.InRange s) :
    Mag.ofWord (cx.slots s) (m1 s) (cx.w s) = cx.mag s := by
  simp only [Spec.Slab.Ctx.mag, mem_of (c := cx) rfl, hp.slots, hp.w]
  rcases hp.ok hi with h0 | h1 | hs
  · rw [ofWord_zero h0, ofWord_zero h0]
  · rw [ofWord_marker h1, ofWord_marker h1]
  · exact ofWord_congr fun i hil => by
      rw [m1, readW64_writeW_disj (item_disjoint hp hi hs hil).1]

/-- After it is, too: the free slot is above the pointers. -/
theorem mag_slot (hp : Pre s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    Mag.ofWord (cx.slots s) (mSlot s) (cx.w s) = cx.mag s := by
  obtain ⟨hs, -, -⟩ := hit hp hi ht
  simp only [Spec.Slab.Ctx.mag, mem_of (c := cx) rfl, hp.slots, hp.w]
  exact ofWord_congr fun i hil => by
    obtain ⟨h1, -, h3⟩ := item_disjoint hp hi hs hil
    rw [mSlot, readW64_writeW_disj h3, m1, readW64_writeW_disj h1]

theorem abort_eq (p : State) : (pushInlineContract abi).abort s p ↔ [p.gpr .rdi] = [s.gpr .rdi] ∧
    (Unchanged [cx.csField s, ⟨cx.w s, 8⟩] (cx.mem s) p.mem ∧
      (cx.InRange s → Mag.ofWord (cx.slots s) p.mem (cx.w s) = cx.mag s)) :=
  abort_iff cx hv s p

theorem abortOk1 (hp : Pre s k) {p : State} (hm : Mid scratchI s (m1 s) p) :
    armed p ∧ inlineTarget.abiPreserved s p ∧ (pushInlineContract abi).abort s p ∧
      ∀ p', inlineTarget.rely p p' → (pushInlineContract abi).env p p' →
        (pushInlineContract abi).pre p' := by
  refine ⟨armed_of_mid hp.facts (xs := []) frame_m1 m1_cs (by simp) hm, inline_abi hm, ?_,
    fun p' _ he => (env_eq cx p p').mp he⟩
  refine (abort_eq p).mpr ⟨by rw [hm.gpr _ (by decide)], ?_, fun hi => by rw [hm.mem]; exact mag_m1 hp hi⟩
  rw [hm.mem, csField_of (c := cx) rfl, mem_of (c := cx) rfl]
  exact frame_m1.mono (by simp)

theorem abortOk2 (hp : Pre s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    armed (σ9 s) ∧ inlineTarget.abiPreserved s (σ9 s) ∧ (pushInlineContract abi).abort s (σ9 s) ∧
      ∀ p', inlineTarget.rely (σ9 s) p' → (pushInlineContract abi).env (σ9 s) p' →
        (pushInlineContract abi).pre p' := by
  have hf := slotFacts hp hi ht
  have hfr : Frame [⟨area s + 8, 8⟩, ⟨wvi s, 8⟩] s.mem (mSlot s) := frame_two _ _ _ _ _
  have hcs : (mSlot s).readW (area s + 8) 64 = s.csDesc := by
    rw [mSlot, readW64_writeW_disj hf.cs.symm, m1_cs]
  refine ⟨armed_of_mid hp.facts (xs := [⟨wvi s, 8⟩]) hfr hcs (by simpa using hf.rsVar) mσ9,
    inline_abi mσ9, ?_, fun p' _ he => (env_eq cx _ p').mp he⟩
  refine (abort_eq _).mpr ⟨by rw [mσ9.gpr _ (by decide)], ?_, fun _ => mag_slot hp hi ht⟩
  rw [mσ9.mem, csField_of (c := cx) rfl, mem_of (c := cx) rfl, hp.w]
  exact hfr

/-! ### Its accesses -/

theorem addr8 : s.fsBase + o s + BitVec.ofNat 64 RSEQ_CS = area s + 8 := rfl
theorem addr4 : s.fsBase + o s + BitVec.ofNat 64 RSEQ_CPU_ID = area s + 4 := rfl
theorem rsName : RSEQ_OFFSET = RS := rfl

theorem ofNat_toNat64 (v : BitVec 64) : BitVec.ofNat 64 v.toNat = v := by
  rw [BitVec.ofNat_toNat, BitVec.setWidth_eq]

theorem wordAddr_eq (hp : Pre s k) :
    wordAddr (s.syms SLABS) (cpu32 s).toNat (cx.cls s) = wai s := by
  rw [hp.word]; rfl

theorem test_eq : BitVec.ofNat 32 ((wvi s).toNat + 8) &&& BitVec.ofNat 32 (cx.testMask s 0) = tst s := by
  have : BitVec.ofNat 32 ((wvi s).toNat + 8) = (wvi s + 8).setWidth 32 := by
    apply BitVec.eq_of_toNat_eq
    rw [BitVec.toNat_ofNat, BitVec.toNat_setWidth, add8_toNat_mod,
      Nat.mod_mod_of_dvd _ (by decide : 2 ^ 32 ∣ 2 ^ 64)]
  simp only [Spec.Slab.Ctx.testMask, param_of (c := cx) rfl, BitVec.ofNat_toNat, tst, this]; rfl

theorem trace_ok (hs : (pushInlineContract abi).pre s) :
    (pushInlineContract abi).trace s (Code.events isa pushInline s) := by
  have h := facts_of_pre hs
  obtain ⟨k, hp⟩ := pre_of h
  show cx.PushTrace s (cx.arg s 0) (Code.events isa (.rseq arm body) s)
  simp only [Code.events, execBlock_arm h, events_arm h]
  refine ⟨(o s).toNat, s.csDesc.toNat, (cpu32 s).toNat, (wvi s).toNat, 0, ?_⟩
  simp only [Spec.Slab.Ctx.headTrace, Spec.Slab.Ctx.rd, Spec.Slab.Ctx.wr, tp_of (c := cx) rfl,
    sym_of (c := cx) rfl, test_eq, ofNat_toNat64, addr8, addr4, rsName, arg0]
  by_cases hi : cx.InRange s
  · have hi' : (cpu32 s).toNat < SLAB_COUNT := (inRange_iff.mp hi)
    rw [wordAddr_eq hp]
    by_cases ht : tst s = 0
    · rw [events_T hp hi ht]; simp [hi', ht]
    · rw [events_P hp hi ht]; simpa [hi'] using ht
  · have hi' : ¬ (cpu32 s).toNat < SLAB_COUNT := fun h' => hi (inRange_iff.mpr h')
    rw [events_C hp hi]; simp [hi']

theorem rseqOk (hs : (pushInlineContract abi).pre s) :
    RseqOk inlineTarget pushInline (pushInlineContract abi) s := by
  have h := facts_of_pre hs
  obtain ⟨k, hp⟩ := pre_of h
  have ht := trace_ok hs
  rw [code_eq] at ht ⊢
  unfold RseqOk
  refine ⟨ht, ⟨_, _, rfl, execBlock_dropLast_arm h, arms_arm⟩, ?_⟩
  have mA : Mid scratchI s (m1 s) (σA s) := mid_σA (by decide) (by decide)
  have m1' : Mid scratchI s (m1 s) (σ1 s) := mid_σ1 (by decide) mA
  simp only [Code.abortStates, execBlock_arm h]
  by_cases hi : cx.InRange s
  · by_cases ht : tst s = 0
    · rw [prefixes_T hp hi ht]
      simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
      exact ⟨abortOk1 hp mA, abortOk1 hp m1', abortOk1 hp mσ2, abortOk1 hp mσ2, abortOk1 hp mσ3,
        abortOk1 hp mσ4i, abortOk1 hp mσ5i, abortOk1 hp mσ6, abortOk1 hp mσ7⟩
    · rw [prefixes_P hp hi ht]
      simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
      exact ⟨abortOk1 hp mA, abortOk1 hp m1', abortOk1 hp mσ2, abortOk1 hp mσ2, abortOk1 hp mσ3,
        abortOk1 hp mσ4i, abortOk1 hp mσ5i, abortOk1 hp mσ6, abortOk1 hp mσ7, abortOk1 hp mσ7,
        abortOk2 hp hi ht⟩
  · rw [prefixes_C hp hi]
    simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
    exact ⟨abortOk1 hp mA, abortOk1 hp m1', abortOk1 hp mσ2⟩

/-! ### The registers `asm!` must find kept -/

def clobbers : List String := ["rax", "rcx", "r8", "r9", "r10", "r11"]

theorem names : ∀ r ∈ scratchI, r.name ∈ clobbers := by decide

theorem clobbersOk (hs : (pushInlineContract abi).pre s) :
    (∀ s', Exec isa pushInline s s' →
      preservedExcept (clobbers ++ (if pushInlineSig.ret.isSome then ["rax"] else [])) s s') ∧
    (∀ t s', ExecExit isa pushInline s t s' → preservedExcept clobbers s s') ∧
    (∀ p ∈ pushInline.abortStates isa s, preservedExcept clobbers s p) := by
  have h := facts_of_pre hs
  obtain ⟨k, hp⟩ := pre_of h
  have names' : ∀ r ∈ scratchI,
      r.name ∈ clobbers ++ (if pushInlineSig.ret.isSome then ["rax"] else []) := by decide
  rw [code_eq]
  have mA : Mid scratchI s (m1 s) (σA s) := mid_σA (by decide) (by decide)
  have m1' : Mid scratchI s (m1 s) (σ1 s) := mid_σ1 (by decide) mA
  refine ⟨fun s' he => ?_, fun t s' he => ?_, fun p hpm => ?_⟩
  · cases he with
    | rseq ha hb =>
      rw [execBlock_arm h] at ha; cases ha
      by_cases hi : cx.InRange s
      · by_cases ht : tst s = 0
        · rw [runBody_T hp hi ht] at hb; cases hb
        · rw [runBody_P hp hi ht] at hb; cases hb; exact preservedExcept_of_mid names' mσ10
      · rw [runBody_C hp hi] at hb; cases hb
  · cases he with
    | rseq ha hb =>
      rw [execBlock_arm h] at ha; cases ha
      by_cases hi : cx.InRange s
      · by_cases ht : tst s = 0
        · rw [runBody_T hp hi ht] at hb; cases hb; exact preservedExcept_of_mid names mσ7
        · rw [runBody_P hp hi ht] at hb; cases hb
      · rw [runBody_C hp hi] at hb; cases hb; exact preservedExcept_of_mid names mσ2
  · simp only [Code.abortStates, execBlock_arm h] at hpm
    by_cases hi : cx.InRange s
    · by_cases ht : tst s = 0
      · rw [prefixes_T hp hi ht] at hpm
        simp only [List.mem_cons, List.not_mem_nil, or_false] at hpm
        rcases hpm with rfl | rfl | rfl | rfl | rfl | rfl | rfl | rfl | rfl <;>
          first
          | exact preservedExcept_of_mid names mA | exact preservedExcept_of_mid names m1'
          | exact preservedExcept_of_mid names mσ2 | exact preservedExcept_of_mid names mσ3
          | exact preservedExcept_of_mid names mσ4i | exact preservedExcept_of_mid names mσ5i
          | exact preservedExcept_of_mid names mσ6 | exact preservedExcept_of_mid names mσ7
      · rw [prefixes_P hp hi ht] at hpm
        simp only [List.mem_cons, List.not_mem_nil, or_false] at hpm
        rcases hpm with rfl | rfl | rfl | rfl | rfl | rfl | rfl | rfl | rfl | rfl | rfl <;>
          first
          | exact preservedExcept_of_mid names mA | exact preservedExcept_of_mid names m1'
          | exact preservedExcept_of_mid names mσ2 | exact preservedExcept_of_mid names mσ3
          | exact preservedExcept_of_mid names mσ4i | exact preservedExcept_of_mid names mσ5i
          | exact preservedExcept_of_mid names mσ6 | exact preservedExcept_of_mid names mσ7
          | exact preservedExcept_of_mid names mσ9
    · rw [prefixes_C hp hi] at hpm
      simp only [List.mem_cons, List.not_mem_nil, or_false] at hpm
      rcases hpm with rfl | rfl | rfl <;>
        first
        | exact preservedExcept_of_mid names mA | exact preservedExcept_of_mid names m1'
        | exact preservedExcept_of_mid names mσ2

end Contract

end VG.Proof.Slab.X86_64.PushInline
