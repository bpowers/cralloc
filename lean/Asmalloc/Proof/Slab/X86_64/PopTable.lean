module

public import Asmalloc.Proof.Slab.X86_64.Table

/-!
# The out-of-line pop on x86-64

`slab_pop` (`Impl.Slab.X86_64.popTable`), of the class in `rdi`, against
`Spec.Slab.popContract`: its three ways out (no slab for the CPU and nothing
to pop, both a tail call of the slow path; the commit), and in each its run,
where it may be aborted, and its memory accesses.
-/

@[expose] public section

namespace VG.Proof.Slab.X86_64.PopTable

open VG.X86_64 VG.Spec.Slab VG.Impl.Slab.X86_64 VG.Proof.Slab.X86_64

abbrev cx : Spec.Slab.Ctx isa := ⟨abi, .table, popSig⟩

theorem hX : X86 cx.A := x86_abi

def body : List (Step Instr Cond) := [
  .instr (.mov32 .r8 (.mem { base := .r10, disp := 4, seg := some .fs })),
  .instr (.alu32 .cmp .r8 (.imm 8192)),
  .exit .ae [] (some "slow"),
  .instr (.shift .shl .r8 12),
  .instr (.leaSym .r9 "SLABS"),
  .instr (.lea .r9 { base := .r9, index := some .rdi, scale := 8 }),
  .instr (.mov .rcx (.mem slabWord)),
  .instr (.leaSym .r11 "MAG_MASKS"),
  .instr (.alu32 .test .rcx maskEntry),
  .exit .e [] (some "slow"),
  .instr (.mov .rax (.mem { base := .rcx, disp := -8 })),
  .instr (.lea .rcx { base := .rcx, disp := -8 }),
  .instr (.store slabWord .rcx)]

theorem code_eq : popTable = .rseq arm body := rfl

/-! ## The states -/

def tst (s : State) : BitVec 32 := (wvt s).setWidth 32 &&& mk s
def σ8 (s : State) : State := arithFlags (σ7t s) (tst s) false false
def x (s : State) : BitVec 64 := s.mem.readW (wvt s - 8) 64
def σ9 (s : State) : State := (σ8 s).setReg .rax (x s)
def σ10 (s : State) : State := (σ9 s).setReg .rcx (wvt s - 8)
def mEnd (s : State) : Mem := (m1 s).writeW (wat s) (wvt s - 8)
def σ11 (s : State) : State := { σ10 s with mem := mEnd s }

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
  have hma : maskA s = cx.maskAddr s := maskA_eq hX (cls_eq s)
  have hmask : cx.mask s = (mk s).toNat := by
    show (s.mem.readW (cx.maskAddr s) 32).toNat = _; rw [← hma]; rfl
  have hslots : cx.slots s = 2 ^ k := by
    rw [Spec.Slab.Ctx.slots, hm]; exact slotsOf_magMask k hk
  have hw : wat s = cx.word s := wat_eq hX (cls_eq s)
  have hwv : cx.w s = wvt s := by simp only [Spec.Slab.Ctx.w, mem_of (c := cx) hX, wvt, hw]
  refine ⟨k, h, hk, hk', by rw [← hmask]; exact hm, hslots, hw, hma, hwv, fun hi => ?_⟩
  have := (hinv (cx.cpu s) hi).1
  rw [← hslots, ← hwv]
  exact this

/-! ## The steps after the slab word -/

section Steps

variable {s : State} {k : Nat}

theorem exec_test (hp : Pre s k) : exec (.alu32 .test .rcx maskEntry) (σ7t s) = some (σ8 s) := by
  have hin := inMask hp.facts rfl hp.maskA (σ7t s) σ7t_rd σ7t_wr
  rw [← maskEntry_ea (σ7t s) σ7t_r11 σ7t_rdi] at hin
  rw [maskEntry, exec_test32_mem _ hin, maskEntry_ea _ σ7t_r11 σ7t_rdi, σ7t_mem,
    m1_mask hp.facts rfl hp.maskA, σ7t_rcx]
  rfl

theorem eval_test : eval .e (σ8 s) = some (tst s == 0) := rfl

/-- What a pop that passes the test pops. -/
theorem hit (hp : Pre s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    IsSlot (2 ^ k) (wvt s) ∧ 2 ≤ slot (2 ^ k) (wvt s) ∧
      (Mag.ofWord (2 ^ k) s.mem (wvt s)).pop =
        some (x s, .stack (magBase (2 ^ k) (wvt s)) (items (2 ^ k) s.mem (wvt s)).dropLast) := by
  have hm : ¬ (wvt s).toNat % (8 * 2 ^ k) < 16 := fun h' =>
    ht ((test_iff32 hp.hk hp.hk' (wvt s) (mk s) hp.mask).mpr h')
  exact pop_of_test (dvd_marker hp.hk') (hp.ok hi) hm

theorem magRegion_eq (hp : Pre s k) : cx.magRegion s = ⟨magBase (2 ^ k) (wvt s), 8 * 2 ^ k⟩ := by
  simp only [Spec.Slab.Ctx.magRegion, hp.slots, hp.w]

theorem σ8_rd : (σ8 s).rd = s.rd := rfl
theorem σ8_wr : (σ8 s).wr = s.wr := rfl
theorem σ8_mem : (σ8 s).mem = m1 s := rfl
theorem σ8_rcx : (σ8 s).gpr .rcx = wvt s := by rw [σ8, RegUpd.gpr_arithFlags, σ7t_rcx]

theorem below_ea (σ : State) (h : σ.gpr .rcx = wvt s) :
    σ.ea { base := .rcx, disp := -8 } = wvt s - 8 := by
  rw [Access.ea_base_disp, Access.add_ofInt_neg_lit, h]; rfl

theorem below_facts (hp : Pre s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    InRegions (s.rd ++ s.wr) (wvt s - 8) 8 ∧ Region.Disjoint ⟨wvt s - 8, 8⟩ ⟨area s + 8, 8⟩ := by
  obtain ⟨hs, h2, -⟩ := hit hp hi ht
  have hsl : IsSlot (cx.slots s) (cx.w s) := by rw [hp.slots, hp.w]; exact hs
  have hmag := hp.facts.mag hi hsl
  have hcm := hp.facts.cs_mag hi hsl
  rw [magRegion_eq hp] at hmag hcm
  have hc := contains_below hs h2 (slot_lt _ _ (Nat.two_pow_pos k))
  exact ⟨(InRegions.of_mem hmag hc).append_right, hcm.symm.sub_left (sub_of_contains hc)⟩

theorem exec_x (hp : Pre s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    exec (.mov .rax (.mem { base := .rcx, disp := -8 })) (σ8 s) = some (σ9 s) := by
  obtain ⟨hin, hdj⟩ := below_facts hp hi ht
  have hin' : InRegions ((σ8 s).rd ++ (σ8 s).wr) ((σ8 s).ea { base := .rcx, disp := -8 }) 8 := by
    rw [below_ea _ σ8_rcx, σ8_rd, σ8_wr]; exact hin
  rw [exec_load _ hin', below_ea _ σ8_rcx, σ8_mem, m1, readW64_writeW_disj hdj]
  rfl

theorem σ9_rcx : (σ9 s).gpr .rcx = wvt s := by
  rw [σ9, RegUpd.gpr_setReg_of_ne _ _ (by decide), σ8_rcx]

theorem exec_lea8 : exec (.lea .rcx { base := .rcx, disp := -8 }) (σ9 s) = some (σ10 s) := by
  rw [exec_lea, Access.offset_base_disp, Access.add_ofInt_neg_lit, σ9_rcx]; rfl

theorem σ10_r9 : (σ10 s).gpr .r9 = r9t s := by
  simp only [σ10, σ9, σ8, σ7t, σ6t, RegUpd.gpr_setReg_of_ne _ _ (show ¬ Reg.r9 = .rcx by decide),
    RegUpd.gpr_setReg_of_ne _ _ (show ¬ Reg.r9 = .rax by decide),
    RegUpd.gpr_setReg_of_ne _ _ (show ¬ Reg.r9 = .r11 by decide), RegUpd.gpr_arithFlags, σ5t_r9]
theorem σ10_r8 : (σ10 s).gpr .r8 = sh s := by
  simp only [σ10, σ9, σ8, σ7t, σ6t, RegUpd.gpr_setReg_of_ne _ _ (show ¬ Reg.r8 = .rcx by decide),
    RegUpd.gpr_setReg_of_ne _ _ (show ¬ Reg.r8 = .rax by decide),
    RegUpd.gpr_setReg_of_ne _ _ (show ¬ Reg.r8 = .r11 by decide), RegUpd.gpr_arithFlags, σ5t_r8]
theorem σ10_rcx : (σ10 s).gpr .rcx = wvt s - 8 := RegUpd.gpr_setReg_self _ _ _
theorem σ10_mem : (σ10 s).mem = m1 s := rfl
theorem σ10_wr : (σ10 s).wr = s.wr := rfl

theorem exec_commit (hp : Pre s k) (hi : cx.InRange s) :
    exec (.store slabWord .rcx) (σ10 s) = some (σ11 s) := by
  have hin : InRegions (σ10 s).wr ((σ10 s).ea slabWord) 8 := by
    rw [slabWord_ea_t _ σ10_r9 σ10_r8, σ10_wr, hp.word]
    exact inWr (r := ⟨cx.word s, 8⟩) (hp.facts.word hi)
  rw [exec_store _ hin, slabWord_ea_t _ σ10_r9 σ10_r8, σ10_rcx, σ10_mem]
  rfl

theorem inRange_iff : cx.InRange s ↔ (cpu32 s).toNat < 8192 := by
  simp only [Spec.Slab.Ctx.InRange, cpu_of (c := cx) hX, cpu32, SLAB_COUNT]

theorem eval_cpu_true (hi : ¬ cx.InRange s) : eval .ae (σ2 s) = some true := by
  rw [eval_cpu, inRange_iff] at *; simp [hi]

theorem eval_cpu_false (hi : cx.InRange s) : eval .ae (σ2 s) = some false := by
  rw [eval_cpu, inRange_iff] at *; simp [hi]

theorem eval_test_true (ht : tst s = 0) : eval .e (σ8 s) = some true := by
  rw [eval_test, ht]; rfl

theorem eval_test_false (ht : ¬ tst s = 0) : eval .e (σ8 s) = some false := by
  rw [eval_test]; simpa using ht

end Steps

/-! ## The three ways out -/

section Cases

variable {s : State} {k : Nat}

theorem runBody_C (hp : Pre s k) (hi : ¬ cx.InRange s) :
    runBody isa body (σA s) = some (.exit "slow" (σ2 s)) := by
  rw [body, runBody_instr (exec_cpu hp.facts.toArm), runBody_instr exec_cmp,
    runBody_take (eval_cpu_true hi) execBlock_nil']

theorem prefixes_C (hp : Pre s k) (hi : ¬ cx.InRange s) :
    bodyPrefixes isa body (σA s) = [σA s, σ1 s, σ2 s] := by
  rw [body, bodyPrefixes_instr (exec_cpu hp.facts.toArm), bodyPrefixes_instr exec_cmp,
    bodyPrefixes_take (eval_cpu_true hi)]

theorem end_C (hp : Pre s k) (hi : ¬ cx.InRange s) : bodyEnd isa body (σA s) = some 2 := by
  rw [body, bodyEnd_instr (exec_cpu hp.facts.toArm), bodyEnd_instr exec_cmp,
    bodyEnd_take (eval_cpu_true hi)]
  rfl

theorem runBody_T (hp : Pre s k) (hi : cx.InRange s) (ht : tst s = 0) :
    runBody isa body (σA s) = some (.exit "slow" (σ8 s)) := by
  rw [body, runBody_instr (exec_cpu hp.facts.toArm), runBody_instr exec_cmp,
    runBody_skip (eval_cpu_false hi), runBody_instr exec_shl_r8, runBody_instr exec_r9t,
    runBody_instr exec_r9idx, runBody_instr (exec_wordt hp.facts hi hp.word), runBody_instr exec_r11,
    runBody_instr (exec_test hp), runBody_take (eval_test_true ht) execBlock_nil']

theorem prefixes_T (hp : Pre s k) (hi : cx.InRange s) (ht : tst s = 0) :
    bodyPrefixes isa body (σA s) =
      [σA s, σ1 s, σ2 s, σ2 s, σ3 s, σ4t s, σ5t s, σ6t s, σ7t s, σ8 s] := by
  rw [body, bodyPrefixes_instr (exec_cpu hp.facts.toArm), bodyPrefixes_instr exec_cmp,
    bodyPrefixes_skip (eval_cpu_false hi), bodyPrefixes_instr exec_shl_r8,
    bodyPrefixes_instr exec_r9t, bodyPrefixes_instr exec_r9idx,
    bodyPrefixes_instr (exec_wordt hp.facts hi hp.word), bodyPrefixes_instr exec_r11,
    bodyPrefixes_instr (exec_test hp), bodyPrefixes_take (eval_test_true ht)]

theorem end_T (hp : Pre s k) (hi : cx.InRange s) (ht : tst s = 0) :
    bodyEnd isa body (σA s) = some 9 := by
  rw [body, bodyEnd_instr (exec_cpu hp.facts.toArm), bodyEnd_instr exec_cmp,
    bodyEnd_skip (eval_cpu_false hi), bodyEnd_instr exec_shl_r8, bodyEnd_instr exec_r9t,
    bodyEnd_instr exec_r9idx, bodyEnd_instr (exec_wordt hp.facts hi hp.word), bodyEnd_instr exec_r11,
    bodyEnd_instr (exec_test hp), bodyEnd_take (eval_test_true ht)]
  rfl

theorem runBody_P (hp : Pre s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    runBody isa body (σA s) = some (.done (σ11 s)) := by
  rw [body, runBody_instr (exec_cpu hp.facts.toArm), runBody_instr exec_cmp,
    runBody_skip (eval_cpu_false hi), runBody_instr exec_shl_r8, runBody_instr exec_r9t,
    runBody_instr exec_r9idx, runBody_instr (exec_wordt hp.facts hi hp.word), runBody_instr exec_r11,
    runBody_instr (exec_test hp), runBody_skip (eval_test_false ht), runBody_instr (exec_x hp hi ht),
    runBody_instr exec_lea8, runBody_instr (exec_commit hp hi), runBody_nil']

theorem prefixes_P (hp : Pre s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    bodyPrefixes isa body (σA s) =
      [σA s, σ1 s, σ2 s, σ2 s, σ3 s, σ4t s, σ5t s, σ6t s, σ7t s, σ8 s, σ8 s, σ9 s, σ10 s] := by
  rw [body, bodyPrefixes_instr (exec_cpu hp.facts.toArm), bodyPrefixes_instr exec_cmp,
    bodyPrefixes_skip (eval_cpu_false hi), bodyPrefixes_instr exec_shl_r8,
    bodyPrefixes_instr exec_r9t, bodyPrefixes_instr exec_r9idx,
    bodyPrefixes_instr (exec_wordt hp.facts hi hp.word), bodyPrefixes_instr exec_r11,
    bodyPrefixes_instr (exec_test hp), bodyPrefixes_skip (eval_test_false ht),
    bodyPrefixes_instr (exec_x hp hi ht), bodyPrefixes_instr exec_lea8,
    bodyPrefixes_last (exec_commit hp hi)]

theorem end_P (hp : Pre s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    bodyEnd isa body (σA s) = some 13 := by
  rw [body, bodyEnd_instr (exec_cpu hp.facts.toArm), bodyEnd_instr exec_cmp,
    bodyEnd_skip (eval_cpu_false hi), bodyEnd_instr exec_shl_r8, bodyEnd_instr exec_r9t,
    bodyEnd_instr exec_r9idx, bodyEnd_instr (exec_wordt hp.facts hi hp.word), bodyEnd_instr exec_r11,
    bodyEnd_instr (exec_test hp), bodyEnd_skip (eval_test_false ht), bodyEnd_instr (exec_x hp hi ht),
    bodyEnd_instr exec_lea8, bodyEnd_instr (exec_commit hp hi), bodyEnd_nil']
  rfl

/-! ### Memory accesses -/

theorem ev_cpu :
    Instr.events (.mov32 .r8 (.mem { base := .r10, disp := 4, seg := some .fs })) (σA s) (σ1 s) =
      [⟨area s + 4, 4, false, (cpu32 s).toNat⟩] := by
  simp only [Instr.events, cpuOp_ea, σA_mem, toNat_read4, m1]
  rw [readW_writeW_disj (cpu_cs_disjoint s) (by decide)]; rfl

theorem ev_word (hp : Pre s k) (hi : cx.InRange s) :
    Instr.events (.mov .rcx (.mem slabWord)) (σ5t s) (σ6t s) = [⟨wat s, 8, false, (wvt s).toNat⟩] := by
  simp only [Instr.events, slabWord_ea_t _ σ5t_r9 σ5t_r8, σ5t_mem, toNat_read8, m1]
  rw [readW64_writeW_disj (by rw [hp.word]; exact (hp.facts.cs_word hi).symm)]; rfl

theorem ev_mask (hp : Pre s k) :
    Instr.events (.alu32 .test .rcx maskEntry) (σ7t s) (σ8 s) = [⟨maskA s, 4, false, (mk s).toNat⟩] := by
  simp only [Instr.events, maskEntry, maskEntry_ea _ σ7t_r11 σ7t_rdi, σ7t_mem, toNat_read4,
    m1_mask hp.facts rfl hp.maskA]

theorem ev_x (hp : Pre s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    Instr.events (.mov .rax (.mem { base := .rcx, disp := -8 })) (σ8 s) (σ9 s) =
      [⟨wvt s - 8, 8, false, (x s).toNat⟩] := by
  obtain ⟨-, hdj⟩ := below_facts hp hi ht
  simp only [Instr.events, below_ea _ σ8_rcx, σ8_mem, toNat_read8, m1]
  rw [readW64_writeW_disj hdj]; rfl

theorem ev_commit :
    Instr.events (.store slabWord .rcx) (σ10 s) (σ11 s) = [⟨wat s, 8, true, (wvt s - 8).toNat⟩] := by
  simp only [Instr.events, slabWord_ea_t _ σ10_r9 σ10_r8, toNat_read8]
  rw [show (σ11 s).mem = mEnd s from rfl, mEnd, Mem.readW_writeW_self64]

theorem events_C (hp : Pre s k) (hi : ¬ cx.InRange s) :
    bodyEvents isa body (σA s) = [⟨area s + 4, 4, false, (cpu32 s).toNat⟩] := by
  rw [body, bodyEvents_instr (exec_cpu hp.facts.toArm), bodyEvents_instr exec_cmp,
    bodyEvents_take (eval_cpu_true hi)]
  simp only [isa, ev_cpu]; rfl

theorem events_T (hp : Pre s k) (hi : cx.InRange s) (ht : tst s = 0) :
    bodyEvents isa body (σA s) =
      [⟨area s + 4, 4, false, (cpu32 s).toNat⟩, ⟨wat s, 8, false, (wvt s).toNat⟩,
        ⟨maskA s, 4, false, (mk s).toNat⟩] := by
  rw [body, bodyEvents_instr (exec_cpu hp.facts.toArm), bodyEvents_instr exec_cmp,
    bodyEvents_skip (eval_cpu_false hi), bodyEvents_instr exec_shl_r8, bodyEvents_instr exec_r9t,
    bodyEvents_instr exec_r9idx, bodyEvents_instr (exec_wordt hp.facts hi hp.word),
    bodyEvents_instr exec_r11, bodyEvents_instr (exec_test hp), bodyEvents_take (eval_test_true ht)]
  simp only [isa, ev_cpu, ev_word hp hi, ev_mask hp]; rfl

theorem events_P (hp : Pre s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    bodyEvents isa body (σA s) =
      [⟨area s + 4, 4, false, (cpu32 s).toNat⟩, ⟨wat s, 8, false, (wvt s).toNat⟩,
        ⟨maskA s, 4, false, (mk s).toNat⟩, ⟨wvt s - 8, 8, false, (x s).toNat⟩,
        ⟨wat s, 8, true, (wvt s - 8).toNat⟩] := by
  rw [body, bodyEvents_instr (exec_cpu hp.facts.toArm), bodyEvents_instr exec_cmp,
    bodyEvents_skip (eval_cpu_false hi), bodyEvents_instr exec_shl_r8, bodyEvents_instr exec_r9t,
    bodyEvents_instr exec_r9idx, bodyEvents_instr (exec_wordt hp.facts hi hp.word),
    bodyEvents_instr exec_r11, bodyEvents_instr (exec_test hp), bodyEvents_skip (eval_test_false ht),
    bodyEvents_instr (exec_x hp hi ht), bodyEvents_instr exec_lea8,
    bodyEvents_instr (exec_commit hp hi), bodyEvents_nil']
  simp only [ev_cpu, ev_word hp hi, ev_mask hp, ev_x hp hi ht, ev_commit]; rfl

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
theorem mσ8 : Mid scratchT s (m1 s) (σ8 s) := mσ7.arithFlags _ _ _
theorem mσ9 : Mid scratchT s (m1 s) (σ9 s) := mσ8.setReg (by decide) _
theorem mσ10 : Mid scratchT s (m1 s) (σ10 s) := mσ9.setReg (by decide) _
theorem mσ11 : Mid scratchT s (mEnd s) (σ11 s) := mσ10.withMem _

end Mids

/-! ## The contract -/

section Contract

variable {s : State} {k : Nat}

theorem hv : cx.A.args ((cx.sig.words cx.A.ptrBits).map (·.bits cx.A.ptrBits)) =
    some (fun s => [s.gpr .rdi]) := rfl

theorem facts_of_pre (hs : (popContract abi).pre s) : Facts cx s := by
  have := (pre_iff cx (s := s) hv rfl rfl).mp hs
  exact facts_of_world hX (.inr rfl) this.2.1 this.2.2

/-- The calling convention's obligations, in memory changed only in regions
apart from the return address. -/
theorem abi_m1 (hp : Pre s k) {p : State} (hm : Mid scratchT s (m1 s) p) : abiPreserved s p :=
  abiPreserved_of_mid frame_m1 (by simpa using hp.facts.ret_cs) (by decide) hm

theorem popped (hp : Pre s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    cx.Popped s (σ11 s) (x s) := by
  obtain ⟨hs, h2, hpop⟩ := hit hp hi ht
  have hsl : IsSlot (cx.slots s) (cx.w s) := by rw [hp.slots, hp.w]; exact hs
  have hcm := hp.facts.cs_mag hi hsl
  have hwm := hp.facts.word_mag hi hsl
  rw [magRegion_eq hp] at hcm hwm
  refine ⟨.stack (magBase (2 ^ k) (wvt s)) (items (2 ^ k) s.mem (wvt s)).dropLast, ?_, ?_, ?_, ?_⟩
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
    have hsub := sub_item (n := 2 ^ k) (w := wvt s) (i := i)
      (by have := slot_lt (2 ^ k) (wvt s) (Nat.two_pow_pos k); omega)
    rw [mEnd, readW64_writeW_disj (by rw [hp.word]; exact (hwm.symm.sub_left hsub)), m1,
      readW64_writeW_disj (hcm.symm.sub_left hsub)]

theorem ret_σ11 : (σ11 s).gpr .rax = x s := by
  simp only [σ11, σ10, RegUpd.gpr_setReg_of_ne _ _ (show ¬ Reg.rax = .rcx by decide), σ9,
    RegUpd.gpr_setReg_self]

theorem unchanged_cs : Unchanged [cx.csField s] (cx.mem s) (m1 s) := by
  rw [csField_of (c := cx) hX, mem_of (c := cx) hX]; exact frame_m1

theorem missed_C (hi : ¬ cx.InRange s) : cx.PopMissed s (σ2 s) := by
  refine ⟨?_, unchanged_cs⟩
  simp only [Spec.Slab.Ctx.popResult, hi, ↓reduceIte]

theorem missed_T (hp : Pre s k) (hi : cx.InRange s) (ht : tst s = 0) : cx.PopMissed s (σ8 s) := by
  refine ⟨?_, unchanged_cs⟩
  simp only [Spec.Slab.Ctx.popResult, hi, ↓reduceIte, Spec.Slab.Ctx.mag, hp.slots,
    mem_of (c := cx) hX, hp.w]
  exact (pop_eq_none_iff (dvd_marker hp.hk') (hp.ok hi)).mpr ((test_iff32 hp.hk hp.hk' (wvt s) (mk s) hp.mask).mp ht)

theorem post_eq (s' : State) : (popContract abi).post s s' ↔
    ∃ v, cx.Popped s s' v ∧ s'.gpr .rax = v :=
  post_iff cx hv s s'

theorem exit_eq (t : String) (s' : State) : (popContract abi).exit t s s' ↔
    [s'.gpr .rdi] = [s.gpr .rdi] ∧ (t = SLOW ∧ cx.PopMissed s s') :=
  exit_iff cx hv t s s'

theorem correct (hs : (popContract abi).pre s) :
    (∃ s', Exec isa popTable s s' ∧ target.abiPreserved s s' ∧ (popContract abi).post s s') ∨
    (∃ t s', ExecExit isa popTable s t s' ∧ target.abiPreserved s s' ∧
      (popContract abi).exit t s s') := by
  have h := facts_of_pre hs
  obtain ⟨k, hp⟩ := pre_of h
  rw [code_eq]
  by_cases hi : cx.InRange s
  · by_cases ht : tst s = 0
    · exact .inr ⟨"slow", σ8 s, .rseq (execBlock_arm h.toArm) (runBody_T hp hi ht), abi_m1 hp mσ8,
        (exit_eq _ _).mpr ⟨by rw [mσ8.gpr _ (by decide)], rfl, missed_T hp hi ht⟩⟩
    · have hw := hp.facts.ret_word hi
      rw [← hp.word] at hw
      exact .inl ⟨σ11 s, .rseq (execBlock_arm h.toArm) (runBody_P hp hi ht),
        abiPreserved_of_mid (frame_two s.mem _ _ _ _) (by simpa using ⟨hp.facts.ret_cs, hw⟩)
          (by decide) mσ11,
        (post_eq _).mpr ⟨x s, popped hp hi ht, ret_σ11⟩⟩
  · exact .inr ⟨"slow", σ2 s, .rseq (execBlock_arm h.toArm) (runBody_C hp hi), abi_m1 hp mσ2,
      (exit_eq _ _).mpr ⟨by rw [mσ2.gpr _ (by decide)], rfl, missed_C hi⟩⟩

/-! ### Where it may be aborted -/

theorem abort_eq (p : State) : (popContract abi).abort s p ↔ [p.gpr .rdi] = [s.gpr .rdi] ∧
    (Unchanged [cx.csField s] (cx.mem s) p.mem ∧ cx.inv p) :=
  abort_iff cx hv s p

theorem abortOk (hp : Pre s k) {p : State} (hm : Mid scratchT s (m1 s) p) :
    armed p ∧ target.abiPreserved s p ∧ (popContract abi).abort s p ∧
      ∀ p', target.rely p p' → (popContract abi).env p p' → (popContract abi).pre p' := by
  obtain ⟨hip, hre⟩ := retry hX hv (post := fun s s' => _) (fun _ => rfl) (fun _ => rfl) (by decide)
    hp.facts.pre hm (by decide) (by decide) frame_m1 (harmless_cs1 hX hp.facts.pre)
  refine ⟨armed_of_mid hp.facts.toArm (xs := []) frame_m1 m1_cs (by simp) hm, abi_m1 hp hm, ?_, hre⟩
  refine (abort_eq p).mpr ⟨by rw [hm.gpr _ (by decide)], by rw [hm.mem]; exact unchanged_cs, hip⟩

/-! ### Its accesses -/

theorem addr8 : s.fsBase + o s + BitVec.ofNat 64 RSEQ_CS = area s + 8 := rfl
theorem addr4 : s.fsBase + o s + BitVec.ofNat 64 RSEQ_CPU_ID = area s + 4 := rfl
theorem rsName : RSEQ_OFFSET = RS := rfl

theorem ofNat_toNat64 (v : BitVec 64) : BitVec.ofNat 64 v.toNat = v := by
  rw [BitVec.ofNat_toNat, BitVec.setWidth_eq]

theorem wordAddr_eq (hp : Pre s k) :
    wordAddr (s.syms SLABS) (cpu32 s).toNat (cx.cls s) = wat s := by
  rw [hp.word]; rfl

theorem maskAddr_eq (hp : Pre s k) : cx.maskAddr s = maskA s := hp.maskA.symm

theorem test_eq : BitVec.ofNat 32 (wvt s).toNat &&& BitVec.ofNat 32 (cx.testMask s (mk s).toNat) =
    tst s := by
  simp only [Spec.Slab.Ctx.testMask, BitVec.ofNat_toNat, tst]; rfl

theorem trace_ok (hs : (popContract abi).pre s) :
    (popContract abi).trace s (Code.events isa popTable s) := by
  have h := facts_of_pre hs
  obtain ⟨k, hp⟩ := pre_of h
  show cx.PopTrace s (Code.events isa (.rseq arm body) s)
  simp only [Code.events, execBlock_arm h.toArm, events_arm h.toArm]
  refine ⟨(o s).toNat, s.csDesc.toNat, (cpu32 s).toNat, (wvt s).toNat, (mk s).toNat, (x s).toNat, ?_⟩
  simp only [Spec.Slab.Ctx.headTrace, Spec.Slab.Ctx.rd, Spec.Slab.Ctx.wr, tp_of (c := cx) hX,
    sym_of (c := cx) hX, test_eq, ofNat_toNat64, addr8, addr4, rsName, maskAddr_eq hp]
  by_cases hi : cx.InRange s
  · have hi' : (cpu32 s).toNat < SLAB_COUNT := (inRange_iff.mp hi)
    rw [wordAddr_eq hp]
    by_cases ht : tst s = 0
    · rw [events_T hp hi ht]; simp [hi', ht]
    · rw [events_P hp hi ht]; simpa [hi'] using ht
  · have hi' : ¬ (cpu32 s).toNat < SLAB_COUNT := fun h' => hi (inRange_iff.mpr h')
    rw [events_C hp hi]; simp [hi']

theorem rseqOk (hs : (popContract abi).pre s) : RseqOk target popTable (popContract abi) s := by
  have h := facts_of_pre hs
  obtain ⟨k, hp⟩ := pre_of h
  have ht := trace_ok hs
  rw [code_eq] at ht ⊢
  unfold RseqOk
  refine ⟨ht, ⟨_, _, rfl, execBlock_dropLast_arm h.toArm, arms_arm⟩, ?_⟩
  have mA : Mid scratchT s (m1 s) (σA s) := mid_σA (by decide) (by decide)
  have m1' : Mid scratchT s (m1 s) (σ1 s) := mid_σ1 (by decide) mA
  simp only [Code.abortStates, execBlock_arm h.toArm]
  by_cases hi : cx.InRange s
  · by_cases ht : tst s = 0
    · rw [prefixes_T hp hi ht]
      simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
      exact ⟨abortOk hp mA, abortOk hp m1', abortOk hp mσ2, abortOk hp mσ2, abortOk hp mσ3,
        abortOk hp mσ4t, abortOk hp mσ5t, abortOk hp mσ6t, abortOk hp mσ7, abortOk hp mσ8⟩
    · rw [prefixes_P hp hi ht]
      simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
      exact ⟨abortOk hp mA, abortOk hp m1', abortOk hp mσ2, abortOk hp mσ2, abortOk hp mσ3,
        abortOk hp mσ4t, abortOk hp mσ5t, abortOk hp mσ6t, abortOk hp mσ7, abortOk hp mσ8,
        abortOk hp mσ8, abortOk hp mσ9, abortOk hp mσ10⟩
  · rw [prefixes_C hp hi]
    simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
    exact ⟨abortOk hp mA, abortOk hp m1', abortOk hp mσ2⟩

end Contract

end VG.Proof.Slab.X86_64.PopTable
