module

public import Asmalloc.Proof.Slab.X86_64.Word

/-!
# The out-of-line push on x86-64

`slab_push` (`Impl.Slab.X86_64.pushTable`) against `Spec.Slab.pushContract`:
its three ways out (no slab for the CPU and no slot to push to, both a tail
call of the slow path; the commit), and in each its run, where it may be
aborted, and its memory accesses. The class's word in slab 0 is at `rdi` and
the pointer in `rsi`; the mask comes from the table below the slabs and may
be zero, in which case there is never a slot to push to.
-/

@[expose] public section

namespace VG.Proof.Slab.X86_64.PushTable

open VG.X86_64 VG.Spec.Slab VG.Impl.Slab.X86_64 VG.Proof.Slab.X86_64

abbrev cx : Spec.Slab.Ctx isa := ⟨abi, .table, pushSig⟩

theorem hX : X86 cx.A := x86_abi

def body : List (Step Instr Cond) := [
  .instr (.mov32 .r8 (.mem { base := .r10, disp := 4, seg := some .fs })),
  .instr (.alu32 .cmp .r8 (.imm 8192)),
  .exit .ae [] (some "slow"),
  .instr (.shift .shl .r8 12),
  .instr (.mov .rcx (.mem (wordAt .rdi))),
  .instr (.lea .rax { base := .rcx, disp := 8 }),
  .instr (.alu32 .test .rax (maskAt .rdi)),
  .exit .e [] (some "slow"),
  .instr (.store { base := .rcx } .rsi),
  .instr (.store (wordAt .rdi) .rax)]

theorem code_eq : pushTable = .rseq arm body := rfl

/-! ## The states -/

/-- The pointer pushed. -/
def x (s : State) : BitVec 64 := s.gpr .rsi
def σ6 (s : State) : State := (σ4 s).setReg .rax (wv s + 8)
def tst (s : State) : BitVec 32 := (wv s + 8).setWidth 32 &&& mk s
def σ7 (s : State) : State := arithFlags (σ6 s) (tst s) false false
/-- Memory after the store into the free slot, and after the commit. -/
def mSlot (s : State) : Mem := (m1 s).writeW (wv s) (x s)
def mEnd (s : State) : Mem := (mSlot s).writeW (wa s) (wv s + 8)
def σ9 (s : State) : State := { σ7 s with mem := mSlot s }
def σ10 (s : State) : State := { σ9 s with mem := mEnd s }

/-! ## What the precondition gives -/

/-- What a nonzero mask makes of the magazines: `2 ^ k` slots. -/
structure Sized (s : State) (k : Nat) : Prop where
  hk : 2 ≤ k
  hk' : k ≤ 16
  mask : (mk s).toNat = magMask (2 ^ k)
  slots : cx.slots s = 2 ^ k
  ok : cx.InRange s → WordOk (2 ^ k) (wv s)

structure Pre (s : State) : Prop where
  facts : Facts cx s
  word : wa s = cx.word s
  maskA : maskA s = cx.maskAddr s
  w : cx.w s = wv s
  mask : cx.mask s = (mk s).toNat
  sized : cx.mask s ≠ 0 → ∃ k, Sized s k

theorem arg0 (s : State) : cx.arg s 0 = s.gpr .rdi := rfl

theorem pre_of {s : State} (h : Facts cx s) : Pre s := by
  obtain ⟨-, -, ⟨hoff, -⟩, -, ⟨hmok, hinv, -⟩⟩ := h.pre
  have hma : maskA s = cx.maskAddr s := maskA_eq (arg0 s)
  have hmask : cx.mask s = (mk s).toNat := by
    show (s.mem.readW (cx.maskAddr s) 32).toNat = _; rw [← hma]; rfl
  have hw : wa s = cx.word s := wa_eq hX (arg0 s) hoff
  have hwv : cx.w s = wv s := by simp only [Spec.Slab.Ctx.w, mem_of (c := cx) hX, wv, hw]
  refine ⟨h, hw, hma, hwv, hmask, fun hz => ?_⟩
  obtain ⟨k, hk, hk', hm⟩ := hmok.resolve_left hz
  have hslots : cx.slots s = 2 ^ k := by
    rw [Spec.Slab.Ctx.slots, hm]; exact slotsOf_magMask k hk
  refine ⟨k, hk, hk', by rw [← hmask]; exact hm, hslots, fun hi => ?_⟩
  have := (hinv hz (cx.cpu s) hi).1
  rw [← hslots, ← hwv]
  exact this

theorem slots_zero {s : State} (hz : cx.mask s = 0) : cx.slots s = 1 := by
  rw [Spec.Slab.Ctx.slots, hz]; rfl

theorem mask_ne_zero {s : State} {k : Nat} (hp : Pre s) (hk : Sized s k) : cx.mask s ≠ 0 := by
  rw [hp.mask, hk.mask]
  have := four_le hk.hk
  simp only [magMask]
  omega

/-! ## The steps after the slab word -/

section Steps

variable {s : State} {k : Nat}

theorem exec_add8 : exec (.lea .rax { base := .rcx, disp := 8 }) (σ4 s) = some (σ6 s) := by
  rw [exec_lea, Access.offset_base_disp, σ4_rcx]; rfl

theorem σ6_rax : (σ6 s).gpr .rax = wv s + 8 := RegUpd.gpr_setReg_self _ _ _
theorem σ6_rcx : (σ6 s).gpr .rcx = wv s := by
  rw [σ6, RegUpd.gpr_setReg_of_ne _ _ (by decide), σ4_rcx]
theorem σ6_rdi : (σ6 s).gpr .rdi = s.gpr .rdi := by
  rw [σ6, RegUpd.gpr_setReg_of_ne _ _ (by decide), σ4_rdi]

theorem exec_test (hp : Pre s) : exec (.alu32 .test .rax (maskAt .rdi)) (σ6 s) = some (σ7 s) := by
  rw [exec_test_mask (σ := σ6 s) hp.facts rfl hp.maskA .rax rfl rfl rfl σ6_rdi, σ6_rax]; rfl

theorem eval_test : eval .e (σ7 s) = some (tst s == 0) := rfl

theorem σ7_rd : (σ7 s).rd = s.rd := rfl
theorem σ7_wr : (σ7 s).wr = s.wr := rfl
theorem σ7_mem : (σ7 s).mem = m1 s := rfl
theorem σ7_rcx : (σ7 s).gpr .rcx = wv s := by rw [σ7, RegUpd.gpr_arithFlags, σ6_rcx]
theorem σ7_rsi : (σ7 s).gpr .rsi = x s := by
  rw [σ7, RegUpd.gpr_arithFlags, σ6, RegUpd.gpr_setReg_of_ne _ _ (by decide),
    gpr_σ4 (by decide) (by decide) (by decide) (by decide)]
  rfl
theorem σ7_rdi : (σ7 s).gpr .rdi = s.gpr .rdi := by
  rw [σ7, RegUpd.gpr_arithFlags, σ6, RegUpd.gpr_setReg_of_ne _ _ (by decide), σ4_rdi]
theorem σ7_r8 : (σ7 s).gpr .r8 = sh s := by
  rw [σ7, RegUpd.gpr_arithFlags, σ6, RegUpd.gpr_setReg_of_ne _ _ (by decide), σ4_r8]

/-- A test that passes was of a nonzero mask. -/
theorem sized_of_test (hp : Pre s) (ht : ¬ tst s = 0) : ∃ k, Sized s k := by
  refine hp.sized fun hz => ht ?_
  rw [hp.mask] at hz
  have : mk s = 0 := BitVec.eq_of_toNat_eq (by rw [hz]; rfl)
  rw [tst, this]; exact BitVec.and_zero

/-- What a push that passes the test finds. -/
theorem hit (hp : Sized s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    IsSlot (2 ^ k) (wv s) ∧ slot (2 ^ k) (wv s) + 1 < 2 ^ k ∧
      (Mag.ofWord (2 ^ k) s.mem (wv s)).push (2 ^ k) (x s) =
        some (.stack (magBase (2 ^ k) (wv s)) (items (2 ^ k) s.mem (wv s) ++ [x s])) := by
  have hm : ¬ (wv s + 8).toNat % (8 * 2 ^ k) < 16 := fun h' =>
    ht ((test_iff32 hp.hk hp.hk' (wv s + 8) (mk s) hp.mask).mpr h')
  exact push_of_test k rfl hp.hk hp.hk' (hp.ok hi) s.mem (x s) hm

theorem magRegion_eq (hp : Pre s) (hk : Sized s k) :
    cx.magRegion s = ⟨magBase (2 ^ k) (wv s), 8 * 2 ^ k⟩ := by
  simp only [Spec.Slab.Ctx.magRegion, hk.slots, hp.w]

/-- The free slot's region: in the magazine, apart from `rseq_cs`, the slab
word and `__rseq_offset`. -/
structure SlotFacts (s : State) (k : Nat) : Prop where
  inMag : (⟨magBase (2 ^ k) (wv s), 8 * 2 ^ k⟩ : Region).Contains (wv s) 8
  mem : (⟨magBase (2 ^ k) (wv s), 8 * 2 ^ k⟩ : Region) ∈ s.wr
  cs : Region.Disjoint ⟨wv s, 8⟩ ⟨area s + 8, 8⟩
  word : Region.Disjoint ⟨wv s, 8⟩ ⟨wa s, 8⟩
  rsVar : Region.Disjoint ⟨s.syms RS, 8⟩ ⟨wv s, 8⟩
  ret : Region.Disjoint ⟨wv s, 8⟩ ⟨s.gpr .rsp, 8⟩

theorem slotFacts (hp : Pre s) (hk : Sized s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    SlotFacts s k := by
  obtain ⟨hs, hlt, -⟩ := hit hk hi ht
  have hsl : IsSlot (cx.slots s) (cx.w s) := by rw [hk.slots, hp.w]; exact hs
  have hc := contains_at hs hlt
  have hsub := sub_of_contains hc
  have hmag := hp.facts.mag hi hsl
  have hcm := hp.facts.cs_mag hi hsl
  have hwm := hp.facts.word_mag hi hsl
  have hrm := hp.facts.rsVar_mag hi hsl
  have hret := hp.facts.ret_mag hi hsl
  rw [magRegion_eq hp hk] at hmag hcm hwm hrm hret
  exact ⟨hc, hmag, hcm.symm.sub_left hsub, by rw [hp.word]; exact hwm.symm.sub_left hsub,
    hrm.sub_right hsub, hret.sub_left hsub⟩

theorem exec_slot (hp : Pre s) (hk : Sized s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    exec (.store { base := .rcx } .rsi) (σ7 s) = some (σ9 s) := by
  have hf := slotFacts hp hk hi ht
  have hin : InRegions (σ7 s).wr ((σ7 s).ea { base := .rcx }) 8 := by
    rw [Access.ea_base, σ7_rcx, σ7_wr]; exact InRegions.of_mem hf.mem hf.inMag
  rw [exec_store _ hin, Access.ea_base, σ7_rcx, σ7_rsi, σ7_mem]
  rfl

theorem σ9_rdi : (σ9 s).gpr .rdi = s.gpr .rdi := σ7_rdi
theorem σ9_r8 : (σ9 s).gpr .r8 = sh s := σ7_r8
theorem σ9_rax : (σ9 s).gpr .rax = wv s + 8 := by
  show (σ7 s).gpr .rax = _
  rw [σ7, RegUpd.gpr_arithFlags, σ6_rax]
theorem σ9_mem : (σ9 s).mem = mSlot s := rfl
theorem σ9_wr : (σ9 s).wr = s.wr := rfl

theorem exec_commit (hp : Pre s) (hi : cx.InRange s) :
    exec (.store (wordAt .rdi) .rax) (σ9 s) = some (σ10 s) := by
  have hin : InRegions (σ9 s).wr ((σ9 s).ea (wordAt .rdi)) 8 := by
    rw [wordAt_ea _ σ9_rdi σ9_r8, σ9_wr, hp.word]
    exact inWr (r := ⟨cx.word s, 8⟩) (hp.facts.word hi)
  rw [exec_store _ hin, wordAt_ea _ σ9_rdi σ9_r8, σ9_rax, σ9_mem]
  rfl

theorem inRange_iff : cx.InRange s ↔ (cpu32 s).toNat < 8192 := by
  simp only [Spec.Slab.Ctx.InRange, cpu_of (c := cx) hX, cpu32, SLAB_COUNT]

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

theorem runBody_C (hp : Pre s) (hi : ¬ cx.InRange s) :
    runBody isa body (σA s) = some (.exit "slow" (σ2 s)) := by
  rw [body, runBody_instr (exec_cpu hp.facts.toArm), runBody_instr exec_cmp,
    runBody_take (eval_cpu_true hi) execBlock_nil']

theorem prefixes_C (hp : Pre s) (hi : ¬ cx.InRange s) :
    bodyPrefixes isa body (σA s) = [σA s, σ1 s, σ2 s] := by
  rw [body, bodyPrefixes_instr (exec_cpu hp.facts.toArm), bodyPrefixes_instr exec_cmp,
    bodyPrefixes_take (eval_cpu_true hi)]

theorem end_C (hp : Pre s) (hi : ¬ cx.InRange s) : bodyEnd isa body (σA s) = some 2 := by
  rw [body, bodyEnd_instr (exec_cpu hp.facts.toArm), bodyEnd_instr exec_cmp,
    bodyEnd_take (eval_cpu_true hi)]
  rfl

theorem runBody_T (hp : Pre s) (hi : cx.InRange s) (ht : tst s = 0) :
    runBody isa body (σA s) = some (.exit "slow" (σ7 s)) := by
  rw [body, runBody_instr (exec_cpu hp.facts.toArm), runBody_instr exec_cmp,
    runBody_skip (eval_cpu_false hi), runBody_instr exec_shl_r8,
    runBody_instr (exec_wordAt hp.facts hi hp.word), runBody_instr exec_add8,
    runBody_instr (exec_test hp), runBody_take (eval_test_true ht) execBlock_nil']

theorem prefixes_T (hp : Pre s) (hi : cx.InRange s) (ht : tst s = 0) :
    bodyPrefixes isa body (σA s) = [σA s, σ1 s, σ2 s, σ2 s, σ3 s, σ4 s, σ6 s, σ7 s] := by
  rw [body, bodyPrefixes_instr (exec_cpu hp.facts.toArm), bodyPrefixes_instr exec_cmp,
    bodyPrefixes_skip (eval_cpu_false hi), bodyPrefixes_instr exec_shl_r8,
    bodyPrefixes_instr (exec_wordAt hp.facts hi hp.word),
    bodyPrefixes_instr exec_add8, bodyPrefixes_instr (exec_test hp),
    bodyPrefixes_take (eval_test_true ht)]

theorem end_T (hp : Pre s) (hi : cx.InRange s) (ht : tst s = 0) :
    bodyEnd isa body (σA s) = some 7 := by
  rw [body, bodyEnd_instr (exec_cpu hp.facts.toArm), bodyEnd_instr exec_cmp,
    bodyEnd_skip (eval_cpu_false hi), bodyEnd_instr exec_shl_r8,
    bodyEnd_instr (exec_wordAt hp.facts hi hp.word), bodyEnd_instr exec_add8,
    bodyEnd_instr (exec_test hp), bodyEnd_take (eval_test_true ht)]
  rfl

theorem runBody_P (hp : Pre s) (hk : Sized s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    runBody isa body (σA s) = some (.done (σ10 s)) := by
  rw [body, runBody_instr (exec_cpu hp.facts.toArm), runBody_instr exec_cmp,
    runBody_skip (eval_cpu_false hi), runBody_instr exec_shl_r8,
    runBody_instr (exec_wordAt hp.facts hi hp.word), runBody_instr exec_add8,
    runBody_instr (exec_test hp), runBody_skip (eval_test_false ht),
    runBody_instr (exec_slot hp hk hi ht), runBody_instr (exec_commit hp hi), runBody_nil']

theorem prefixes_P (hp : Pre s) (hk : Sized s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    bodyPrefixes isa body (σA s) =
      [σA s, σ1 s, σ2 s, σ2 s, σ3 s, σ4 s, σ6 s, σ7 s, σ7 s, σ9 s] := by
  rw [body, bodyPrefixes_instr (exec_cpu hp.facts.toArm), bodyPrefixes_instr exec_cmp,
    bodyPrefixes_skip (eval_cpu_false hi), bodyPrefixes_instr exec_shl_r8,
    bodyPrefixes_instr (exec_wordAt hp.facts hi hp.word),
    bodyPrefixes_instr exec_add8, bodyPrefixes_instr (exec_test hp),
    bodyPrefixes_skip (eval_test_false ht), bodyPrefixes_instr (exec_slot hp hk hi ht),
    bodyPrefixes_last (exec_commit hp hi)]

theorem end_P (hp : Pre s) (hk : Sized s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    bodyEnd isa body (σA s) = some 10 := by
  rw [body, bodyEnd_instr (exec_cpu hp.facts.toArm), bodyEnd_instr exec_cmp,
    bodyEnd_skip (eval_cpu_false hi), bodyEnd_instr exec_shl_r8,
    bodyEnd_instr (exec_wordAt hp.facts hi hp.word), bodyEnd_instr exec_add8,
    bodyEnd_instr (exec_test hp), bodyEnd_skip (eval_test_false ht),
    bodyEnd_instr (exec_slot hp hk hi ht), bodyEnd_instr (exec_commit hp hi), bodyEnd_nil']
  rfl

/-! ### Memory accesses -/

theorem ev_cpu :
    Instr.events (.mov32 .r8 (.mem { base := .r10, disp := 4, seg := some .fs })) (σA s) (σ1 s) =
      [⟨area s + 4, 4, false, (cpu32 s).toNat⟩] := by
  simp only [Instr.events, cpuOp_ea, σA_mem, toNat_read4, m1]
  rw [readW_writeW_disj (cpu_cs_disjoint s) (by decide)]; rfl

theorem ev_mask (hp : Pre s) :
    Instr.events (.alu32 .test .rax (maskAt .rdi)) (σ6 s) (σ7 s) = [⟨maskA s, 4, false, (mk s).toNat⟩] :=
  ev_test_mask (σ := σ6 s) hp.facts rfl hp.maskA .rax rfl σ6_rdi

theorem ev_slot : Instr.events (.store { base := .rcx } .rsi) (σ7 s) (σ9 s) =
    [⟨wv s, 8, true, (x s).toNat⟩] := by
  simp only [Instr.events, Access.ea_base, σ7_rcx, toNat_read8]
  rw [σ9_mem, mSlot, Mem.readW_writeW_self64]

theorem ev_commit :
    Instr.events (.store (wordAt .rdi) .rax) (σ9 s) (σ10 s) = [⟨wa s, 8, true, (wv s + 8).toNat⟩] := by
  simp only [Instr.events, wordAt_ea _ σ9_rdi σ9_r8, toNat_read8]
  rw [show (σ10 s).mem = mEnd s from rfl, mEnd, Mem.readW_writeW_self64]

theorem events_C (hp : Pre s) (hi : ¬ cx.InRange s) :
    bodyEvents isa body (σA s) = [⟨area s + 4, 4, false, (cpu32 s).toNat⟩] := by
  rw [body, bodyEvents_instr (exec_cpu hp.facts.toArm), bodyEvents_instr exec_cmp,
    bodyEvents_take (eval_cpu_true hi)]
  simp only [isa, ev_cpu]; rfl

theorem events_T (hp : Pre s) (hi : cx.InRange s) (ht : tst s = 0) :
    bodyEvents isa body (σA s) =
      [⟨area s + 4, 4, false, (cpu32 s).toNat⟩, ⟨wa s, 8, false, (wv s).toNat⟩,
        ⟨maskA s, 4, false, (mk s).toNat⟩] := by
  rw [body, bodyEvents_instr (exec_cpu hp.facts.toArm), bodyEvents_instr exec_cmp,
    bodyEvents_skip (eval_cpu_false hi), bodyEvents_instr exec_shl_r8,
    bodyEvents_instr (exec_wordAt hp.facts hi hp.word), bodyEvents_instr exec_add8,
    bodyEvents_instr (exec_test hp), bodyEvents_take (eval_test_true ht)]
  simp only [isa, ev_cpu, ev_wordAt hp.facts hi hp.word, ev_mask hp]; rfl

theorem events_P (hp : Pre s) (hk : Sized s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    bodyEvents isa body (σA s) =
      [⟨area s + 4, 4, false, (cpu32 s).toNat⟩, ⟨wa s, 8, false, (wv s).toNat⟩,
        ⟨maskA s, 4, false, (mk s).toNat⟩,
        ⟨wv s, 8, true, (x s).toNat⟩, ⟨wa s, 8, true, (wv s + 8).toNat⟩] := by
  rw [body, bodyEvents_instr (exec_cpu hp.facts.toArm), bodyEvents_instr exec_cmp,
    bodyEvents_skip (eval_cpu_false hi), bodyEvents_instr exec_shl_r8,
    bodyEvents_instr (exec_wordAt hp.facts hi hp.word), bodyEvents_instr exec_add8,
    bodyEvents_instr (exec_test hp), bodyEvents_skip (eval_test_false ht),
    bodyEvents_instr (exec_slot hp hk hi ht), bodyEvents_instr (exec_commit hp hi), bodyEvents_nil']
  simp only [ev_cpu, ev_wordAt hp.facts hi hp.word, ev_mask hp, ev_slot, ev_commit]; rfl

end Cases

/-! ## What each state keeps -/

section Mids

variable {s : State}

theorem mσ2 : Mid scratchI s (m1 s) (σ2 s) :=
  mid_σ2 (mid_σ1 (by decide) (mid_σA (by decide) (by decide)))
theorem mσ3 : Mid scratchI s (m1 s) (σ3 s) := mid_σ3 (by decide) mσ2
theorem mσ4' : Mid scratchI s (m1 s) (σ4 s) := mσ4 mσ3 (by decide)
theorem mσ6 : Mid scratchI s (m1 s) (σ6 s) := mσ4'.setReg (by decide) _
theorem mσ7 : Mid scratchI s (m1 s) (σ7 s) := mσ6.arithFlags _ _ _
theorem mσ9 : Mid scratchI s (mSlot s) (σ9 s) := mσ7.withMem _
theorem mσ10 : Mid scratchI s (mEnd s) (σ10 s) := mσ9.withMem _

end Mids

/-! ## The contract -/

section Contract

variable {s : State} {k : Nat}

theorem hv : cx.A.args ((cx.sig.words cx.A.ptrBits).map (·.bits cx.A.ptrBits)) =
    some (fun s => [s.gpr .rdi, s.gpr .rsi]) := rfl

theorem arg1 : cx.arg s 1 = x s := rfl

theorem facts_of_pre (hs : (pushContract abi).pre s) : Facts cx s := by
  have := (pre_iff cx (s := s) hv rfl rfl).mp hs
  exact facts_of_world hX (.inr rfl) this.2.1 this.2.2

/-- The calling convention's obligations, in memory changed only in regions
apart from the return address. -/
theorem abi_m1 (hp : Pre s) {p : State} (hm : Mid scratchI s (m1 s) p) : abiPreserved s p :=
  abiPreserved_of_mid frame_m1 (by simpa using hp.facts.ret_cs) (by decide) hm

/-- Item `i` of the magazine is apart from `rseq_cs`, the slab word, and the
free slot. -/
theorem item_disjoint (hp : Pre s) (hk : Sized s k) (hi : cx.InRange s) (hs : IsSlot (2 ^ k) (wv s))
    {i : Nat}
    (hil : i < slot (2 ^ k) (wv s) - 1) :
    Region.Disjoint ⟨magBase (2 ^ k) (wv s) + BitVec.ofNat 64 (8 * (i + 1)), 8⟩ ⟨area s + 8, 8⟩ ∧
    Region.Disjoint ⟨magBase (2 ^ k) (wv s) + BitVec.ofNat 64 (8 * (i + 1)), 8⟩ ⟨wa s, 8⟩ ∧
    Region.Disjoint ⟨magBase (2 ^ k) (wv s) + BitVec.ofNat 64 (8 * (i + 1)), 8⟩ ⟨wv s, 8⟩ := by
  have hsl : IsSlot (cx.slots s) (cx.w s) := by rw [hk.slots, hp.w]; exact hs
  have hlt := slot_lt (2 ^ k) (wv s) (Nat.two_pow_pos k)
  have hsub := sub_item (n := 2 ^ k) (w := wv s) (i := i) (by omega)
  have hcm := hp.facts.cs_mag hi hsl
  have hwm := hp.facts.word_mag hi hsl
  rw [magRegion_eq hp hk] at hcm hwm
  have hn : 8 * 2 ^ k ≤ 2 ^ 19 := by
    have := Nat.pow_le_pow_right (n := 2) (by decide) hk.hk'
    omega
  refine ⟨hcm.symm.sub_left hsub, by rw [hp.word]; exact hwm.symm.sub_left hsub, ?_⟩
  have hw := magBase_add_slot hs
  have := Offset.disjoint (magBase (2 ^ k) (wv s)) (d := 8 * (i + 1)) (n := 8)
    (e := 8 * slot (2 ^ k) (wv s)) (k := 8) (by omega) (by omega) (by omega)
  rwa [hw] at this

theorem pushed (hp : Pre s) (hk : Sized s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    cx.Pushed s (cx.arg s 1) (σ10 s) := by
  obtain ⟨hs, hlt, hpush⟩ := hit hk hi ht
  have hf := slotFacts hp hk hi ht
  refine ⟨.stack (magBase (2 ^ k) (wv s)) (items (2 ^ k) s.mem (wv s) ++ [x s]), ?_, ?_, ?_, ?_⟩
  · simp only [Spec.Slab.Ctx.pushResult, hi, ↓reduceIte, Spec.Slab.Ctx.mag, hk.slots,
      mem_of (c := cx) hX, hp.w, arg1]
    exact hpush
  · rw [csField_of (c := cx) hX, ← hp.word, hp.w]
    show Frame _ s.mem (mEnd s)
    exact (frame_three s.mem _ _ _ _ _ _).mono (by simp)
  · show (mEnd s).readW (cx.word s) 64 = _
    rw [mEnd, ← hp.word, Mem.readW_writeW_self64, hp.w]
  · show Mag.ofWord (cx.slots s) (mEnd s) (cx.w s + 8) = _
    rw [hk.slots, hp.w]
    refine ofWord_add8 k hk.hk' rfl hs hlt (x s) ?_ fun i hil => ?_
    · rw [mEnd, readW64_writeW_disj hf.word, mSlot, Mem.readW_writeW_self64]
    · obtain ⟨h1, h2, h3⟩ := item_disjoint hp hk hi hs hil
      rw [mEnd, readW64_writeW_disj h2, mSlot, readW64_writeW_disj h3, m1, readW64_writeW_disj h1]

theorem unchanged_cs : Unchanged [cx.csField s] (cx.mem s) (m1 s) := by
  rw [csField_of (c := cx) hX, mem_of (c := cx) hX]; exact frame_m1

theorem missed_C (hi : ¬ cx.InRange s) : cx.PushMissed s (cx.arg s 1) (σ2 s) := by
  refine ⟨?_, unchanged_cs⟩
  simp only [Spec.Slab.Ctx.pushResult, hi, ↓reduceIte]

theorem missed_T (hp : Pre s) (hi : cx.InRange s) (ht : tst s = 0) :
    cx.PushMissed s (cx.arg s 1) (σ7 s) := by
  refine ⟨?_, unchanged_cs⟩
  simp only [Spec.Slab.Ctx.pushResult, hi, ↓reduceIte, Spec.Slab.Ctx.mag]
  by_cases hz : cx.mask s = 0
  · rw [slots_zero hz]; exact push_one _ _ _
  · obtain ⟨k, hk⟩ := hp.sized hz
    rw [hk.slots, mem_of (c := cx) hX, hp.w]
    exact (push_eq_none_iff _ k rfl hk.hk hk.hk' (hk.ok hi)).mpr
      ((test_iff32 hk.hk hk.hk' (wv s + 8) (mk s) hk.mask).mp ht)

theorem post_eq (s' : State) : (pushContract abi).post s s' ↔ cx.Pushed s (cx.arg s 1) s' :=
  post_iff cx hv s s'

theorem exit_eq (t : String) (s' : State) : (pushContract abi).exit t s s' ↔
    [s'.gpr .rdi, s'.gpr .rsi] = [s.gpr .rdi, s.gpr .rsi] ∧
      (t = SLOW ∧ cx.PushMissed s (cx.arg s 1) s') :=
  exit_iff cx hv t s s'

theorem correct (hs : (pushContract abi).pre s) :
    (∃ s', Exec isa pushTable s s' ∧ target.abiPreserved s s' ∧
      (pushContract abi).post s s') ∨
    (∃ t s', ExecExit isa pushTable s t s' ∧ target.abiPreserved s s' ∧
      (pushContract abi).exit t s s') := by
  have h := facts_of_pre hs
  have hp := pre_of h
  rw [code_eq]
  by_cases hi : cx.InRange s
  · by_cases ht : tst s = 0
    · exact .inr ⟨"slow", σ7 s, .rseq (execBlock_arm h.toArm) (runBody_T hp hi ht), abi_m1 hp mσ7,
        (exit_eq _ _).mpr ⟨by rw [mσ7.gpr .rdi (by decide), mσ7.gpr .rsi (by decide)], rfl,
          missed_T hp hi ht⟩⟩
    · obtain ⟨k, hk⟩ := sized_of_test hp ht
      have hf := slotFacts hp hk hi ht
      have hw := hp.facts.ret_word hi
      rw [← hp.word] at hw
      exact .inl ⟨σ10 s, .rseq (execBlock_arm h.toArm) (runBody_P hp hk hi ht),
        abiPreserved_of_mid (frame_three s.mem _ _ _ _ _ _)
          (by simpa using ⟨hp.facts.ret_cs, hf.ret, hw⟩) (by decide) mσ10,
        (post_eq _).mpr (pushed hp hk hi ht)⟩
  · exact .inr ⟨"slow", σ2 s, .rseq (execBlock_arm h.toArm) (runBody_C hp hi), abi_m1 hp mσ2,
      (exit_eq _ _).mpr ⟨by rw [mσ2.gpr .rdi (by decide), mσ2.gpr .rsi (by decide)], rfl,
        missed_C hi⟩⟩

/-! ### Where it may be aborted -/

/-- After the free slot is written, the magazine is as on entry: the free
slot is above the pointers. -/
theorem mag_slot (hp : Pre s) (hk : Sized s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    Mag.ofWord (cx.slots s) (mSlot s) (cx.w s) = cx.mag s := by
  obtain ⟨hs, -, -⟩ := hit hk hi ht
  simp only [Spec.Slab.Ctx.mag, mem_of (c := cx) hX, hk.slots, hp.w]
  exact ofWord_congr fun i hil => by
    obtain ⟨h1, -, h3⟩ := item_disjoint hp hk hi hs hil
    rw [mSlot, readW64_writeW_disj h3, m1, readW64_writeW_disj h1]

/-- Before the free slot is written, every slab's magazine is as on entry:
with a zero mask, because a one-slot magazine reads no memory. -/
theorem mags_m1 (hp : Pre s) : ∀ i < SLAB_COUNT,
    Mag.ofWord (cx.slots s) (m1 s) (cx.wordOf s i) = Mag.ofWord (cx.slots s) (cx.mem s) (cx.wordOf s i) := by
  intro i hi
  by_cases hz : cx.mask s = 0
  · rw [slots_zero hz]; exact ofWord_one_congr _ _ _
  obtain ⟨k, hk⟩ := hp.sized hz
  have hok := hp.facts.pre.2.2.2.2.2.1 hz i hi
  rw [mem_of (c := cx) hX]
  refine ofWord_frame (by rw [hk.slots]; exact Nat.two_pow_pos k) frame_m1 hok.1 fun hs r hr => ?_
  rw [List.mem_singleton.mp hr, ← csField_of (c := cx) hX]
  exact (hok.2 hs).2.2.1 (cx.csField s, true) cs_mem_fixed

/-- After it is, too: the free slot is above the routine's slab's pointers,
and in no other slab's magazine (`Exclusive`). -/
theorem mags_slot (hp : Pre s) (hk : Sized s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    ∀ i < SLAB_COUNT, Mag.ofWord (cx.slots s) (mSlot s) (cx.wordOf s i) =
      Mag.ofWord (cx.slots s) (cx.mem s) (cx.wordOf s i) := by
  intro i hi'
  by_cases hc : i = cx.cpu s
  · subst hc
    rw [wordOf_cpu]
    exact mag_slot hp hk hi ht
  · have hz : cx.mask s ≠ 0 := mask_ne_zero hp hk
    have hok := hp.facts.pre.2.2.2.2.2.1 hz i hi'
    have hf := slotFacts hp hk hi ht
    obtain ⟨hs0, -, -⟩ := hit hk hi ht
    have hsc : IsSlot (cx.slots s) (cx.wordOf s (cx.cpu s)) := by
      rw [wordOf_cpu, hk.slots, hp.w]; exact hs0
    rw [mem_of (c := cx) hX]
    refine ofWord_frame (by rw [hk.slots]; exact Nat.two_pow_pos k) (frame_two _ _ _ _ _) hok.1
      fun hs r hr => ?_
    rcases List.mem_cons.mp hr with rfl | hr
    · rw [← csField_of (c := cx) hX]; exact (hok.2 hs).2.2.1 (cx.csField s, true) cs_mem_fixed
    · rw [List.mem_singleton.mp hr]
      have hex := hp.facts.pre.2.2.2.2.2.2 i hi' (cx.cpu s) hi hc hs hsc
      rw [wordOf_cpu, hk.slots, hp.w] at hex
      rw [hk.slots]
      exact hex.sub_right (sub_of_contains hf.inMag)

theorem abort_eq (p : State) : (pushContract abi).abort s p ↔
    [p.gpr .rdi, p.gpr .rsi] = [s.gpr .rdi, s.gpr .rsi] ∧
    ((Unchanged [cx.csField s, ⟨cx.w s, 8⟩] (cx.mem s) p.mem ∧
      ∀ i < SLAB_COUNT, Mag.ofWord (cx.slots s) p.mem (cx.wordOf s i) =
        Mag.ofWord (cx.slots s) (cx.mem s) (cx.wordOf s i)) ∧ cx.inv p) :=
  abort_iff cx hv s p

/-- The magazine lies apart, by the invariant. -/
theorem mag_apart (hp : Pre s) (hk : Sized s k) (hz : cx.mask s ≠ 0) (hi : cx.InRange s)
    (hs : IsSlot (2 ^ k) (wv s)) : cx.Apart s ⟨magBase (2 ^ k) (wv s), 8 * 2 ^ k⟩ := by
  have := (hp.facts.pre.2.2.2.2.2.1 hz (cx.cpu s) hi).2
  rw [wordOf_cpu, hk.slots, hp.w] at this
  exact this hs

theorem abortOk1 (hp : Pre s) {p : State} (hm : Mid scratchI s (m1 s) p) :
    armed p ∧ target.abiPreserved s p ∧ (pushContract abi).abort s p ∧
      ∀ p', target.rely p p' → (pushContract abi).env p p' → (pushContract abi).pre p' := by
  obtain ⟨hip, hre⟩ := retry hX hv (post := fun s s' => _) (fun _ => rfl) (fun _ => rfl) (by decide)
    hp.facts.pre hm (by decide) (by decide) frame_m1 (harmless_cs1 hX hp.facts.pre)
  refine ⟨armed_of_mid hp.facts.toArm (xs := []) frame_m1 m1_cs (by simp) hm, abi_m1 hp hm, ?_, hre⟩
  refine (abort_eq p).mpr ⟨by rw [hm.gpr .rdi (by decide), hm.gpr .rsi (by decide)],
    ⟨?_, fun i hi => by rw [hm.mem]; exact mags_m1 hp i hi⟩, hip⟩
  rw [hm.mem, csField_of (c := cx) hX, mem_of (c := cx) hX]
  exact frame_m1.mono (by simp)

theorem abortOk2 (hp : Pre s) (hk : Sized s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    armed (σ9 s) ∧ target.abiPreserved s (σ9 s) ∧ (pushContract abi).abort s (σ9 s) ∧
      ∀ p', target.rely (σ9 s) p' → (pushContract abi).env (σ9 s) p' →
        (pushContract abi).pre p' := by
  have hf := slotFacts hp hk hi ht
  have hz : cx.mask s ≠ 0 := mask_ne_zero hp hk
  have hfr : Frame [⟨area s + 8, 8⟩, ⟨wv s, 8⟩] s.mem (mSlot s) := frame_two _ _ _ _ _
  have hcs : (mSlot s).readW (area s + 8) 64 = s.csDesc := by
    rw [mSlot, readW64_writeW_disj hf.cs.symm, m1_cs]
  have hW := harmless_cs2 hX hp.facts.pre (harmless_mag (.inr rfl)
    (mag_apart hp hk hz hi (hit hk hi ht).1) (sub_of_contains hf.inMag))
  obtain ⟨hip, hre⟩ := retry hX hv (post := fun s s' => _) (fun _ => rfl) (fun _ => rfl) (by decide)
    hp.facts.pre mσ9 (by decide) (by decide) hfr hW
  refine ⟨armed_of_mid hp.facts.toArm (xs := [⟨wv s, 8⟩]) hfr hcs (by simpa using hf.rsVar) mσ9,
    abiPreserved_of_mid hfr (by simpa using ⟨hp.facts.ret_cs, hf.ret⟩) (by decide) mσ9, ?_, hre⟩
  refine (abort_eq _).mpr ⟨by rw [mσ9.gpr .rdi (by decide), mσ9.gpr .rsi (by decide)],
    ⟨?_, mags_slot hp hk hi ht⟩, hip⟩
  rw [mσ9.mem, csField_of (c := cx) hX, mem_of (c := cx) hX, hp.w]
  exact hfr

/-! ### Its accesses -/

theorem addr8 : s.fsBase + o s + BitVec.ofNat 64 RSEQ_CS = area s + 8 := rfl
theorem addr4 : s.fsBase + o s + BitVec.ofNat 64 RSEQ_CPU_ID = area s + 4 := rfl
theorem rsName : RSEQ_OFFSET = RS := rfl

theorem ofNat_toNat64 (v : BitVec 64) : BitVec.ofNat 64 v.toNat = v := by
  rw [BitVec.ofNat_toNat, BitVec.setWidth_eq]

theorem wordAddr_eq (hp : Pre s) :
    wordAddr (s.syms SLABS) (cpu32 s).toNat (cx.cls s) = wa s := by
  rw [hp.word]; rfl

theorem maskAddr_eq (hp : Pre s) : cx.maskAddr s = maskA s := hp.maskA.symm

theorem test_eq : BitVec.ofNat 32 ((wv s).toNat + 8) &&& BitVec.ofNat 32 (cx.testMask s (mk s).toNat) =
    tst s := by
  have : BitVec.ofNat 32 ((wv s).toNat + 8) = (wv s + 8).setWidth 32 := by
    apply BitVec.eq_of_toNat_eq
    rw [BitVec.toNat_ofNat, BitVec.toNat_setWidth, add8_toNat_mod,
      Nat.mod_mod_of_dvd _ (by decide : 2 ^ 32 ∣ 2 ^ 64)]
  simp only [Spec.Slab.Ctx.testMask, BitVec.ofNat_toNat, tst, this]; rfl

theorem trace_ok (hs : (pushContract abi).pre s) :
    (pushContract abi).trace s (Code.events isa pushTable s) := by
  have h := facts_of_pre hs
  have hp := pre_of h
  show cx.PushTrace s (cx.arg s 1) (Code.events isa (.rseq arm body) s)
  simp only [Code.events, execBlock_arm h.toArm, events_arm h.toArm]
  refine ⟨(o s).toNat, s.csDesc.toNat, (cpu32 s).toNat, (wv s).toNat, (mk s).toNat, ?_⟩
  simp only [Spec.Slab.Ctx.headTrace, Spec.Slab.Ctx.rd, Spec.Slab.Ctx.wr, tp_of (c := cx) hX,
    sym_of (c := cx) hX, test_eq, ofNat_toNat64, addr8, addr4, rsName, arg1, maskAddr_eq hp]
  by_cases hi : cx.InRange s
  · have hi' : (cpu32 s).toNat < SLAB_COUNT := (inRange_iff.mp hi)
    rw [wordAddr_eq hp]
    by_cases ht : tst s = 0
    · rw [events_T hp hi ht]; simp [hi', ht]
    · obtain ⟨k, hk⟩ := sized_of_test hp ht
      rw [events_P hp hk hi ht]; simpa [hi'] using ht
  · have hi' : ¬ (cpu32 s).toNat < SLAB_COUNT := fun h' => hi (inRange_iff.mpr h')
    rw [events_C hp hi]; simp [hi']

theorem rseqOk (hs : (pushContract abi).pre s) : RseqOk target pushTable (pushContract abi) s := by
  have h := facts_of_pre hs
  have hp := pre_of h
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
      exact ⟨abortOk1 hp mA, abortOk1 hp m1', abortOk1 hp mσ2, abortOk1 hp mσ2, abortOk1 hp mσ3,
        abortOk1 hp mσ4', abortOk1 hp mσ6, abortOk1 hp mσ7⟩
    · obtain ⟨k, hk⟩ := sized_of_test hp ht
      rw [prefixes_P hp hk hi ht]
      simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
      exact ⟨abortOk1 hp mA, abortOk1 hp m1', abortOk1 hp mσ2, abortOk1 hp mσ2, abortOk1 hp mσ3,
        abortOk1 hp mσ4', abortOk1 hp mσ6, abortOk1 hp mσ7, abortOk1 hp mσ7,
        abortOk2 hp hk hi ht⟩
  · rw [prefixes_C hp hi]
    simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
    exact ⟨abortOk1 hp mA, abortOk1 hp m1', abortOk1 hp mσ2⟩

end Contract

end VG.Proof.Slab.X86_64.PushTable
