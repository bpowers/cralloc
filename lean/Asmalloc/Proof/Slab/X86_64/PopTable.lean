module

public import Asmalloc.Proof.Slab.X86_64.Word

/-!
# The out-of-line pop on x86-64

`slab_pop` (`Impl.Slab.X86_64.popTable`), of the class whose word in slab 0
is at `rdi`, against `Spec.Slab.popContract`: its three ways out (no slab
for the CPU and nothing to pop, both a tail call of the slow path; the
commit), and in each its run, where it may be aborted, and its memory
accesses. The mask comes from the table below the slabs and may be zero, in
which case there is never anything to pop.
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
  .instr (.mov .rcx (.mem (wordAt .rdi))),
  .instr (.alu32 .test .rcx (maskAt .rdi)),
  .exit .e [] (some "slow"),
  .instr (.lea .rcx { base := .rcx, disp := -8 }),
  .instr (.mov .rax (.mem { base := .rcx })),
  .instr (.store (wordAt .rdi) .rcx)]

theorem code_eq : popTable = .rseq arm body := rfl

/-! ## The states -/

def tst (s : State) : BitVec 32 := (wv s).setWidth 32 &&& mk s
def σ5 (s : State) : State := arithFlags (σ4 s) (tst s) false false
def σ6 (s : State) : State := (σ5 s).setReg .rcx (wv s - 8)
def x (s : State) : BitVec 64 := s.mem.readW (wv s - 8) 64
def σ7 (s : State) : State := (σ6 s).setReg .rax (x s)
def mEnd (s : State) : Mem := (m1 s).writeW (wa s) (wv s - 8)
def σ8 (s : State) : State := { σ7 s with mem := mEnd s }

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

/-- A zero mask: one-slot magazines. -/
theorem slots_zero {s : State} (hz : cx.mask s = 0) : cx.slots s = 1 := by
  rw [Spec.Slab.Ctx.slots, hz]; rfl

/-! ## The steps after the slab word -/

section Steps

variable {s : State}

theorem exec_test (hp : Pre s) : exec (.alu32 .test .rcx (maskAt .rdi)) (σ4 s) = some (σ5 s) := by
  rw [exec_test_mask hp.facts rfl hp.maskA .rcx σ4_rd σ4_wr σ4_mem σ4_rdi, σ4_rcx]; rfl

theorem eval_test : eval .e (σ5 s) = some (tst s == 0) := rfl

/-- A test that passes was of a nonzero mask. -/
theorem sized_of_test (hp : Pre s) (ht : ¬ tst s = 0) : ∃ k, Sized s k := by
  refine hp.sized fun hz => ht ?_
  rw [hp.mask] at hz
  have : mk s = 0 := BitVec.eq_of_toNat_eq (by rw [hz]; rfl)
  rw [tst, this]; exact BitVec.and_zero

/-- What a pop that passes the test pops. -/
theorem hit {k : Nat} (_hp : Pre s) (hk : Sized s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    IsSlot (2 ^ k) (wv s) ∧ 2 ≤ slot (2 ^ k) (wv s) ∧
      (Mag.ofWord (2 ^ k) s.mem (wv s)).pop =
        some (x s, .stack (magBase (2 ^ k) (wv s)) (items (2 ^ k) s.mem (wv s)).dropLast) := by
  have hm : ¬ (wv s).toNat % (8 * 2 ^ k) < 16 := fun h' =>
    ht ((test_iff32 hk.hk hk.hk' (wv s) (mk s) hk.mask).mpr h')
  exact pop_of_test (dvd_marker hk.hk') (hk.ok hi) hm

theorem magRegion_eq {k : Nat} (hp : Pre s) (hk : Sized s k) :
    cx.magRegion s = ⟨magBase (2 ^ k) (wv s), 8 * 2 ^ k⟩ := by
  simp only [Spec.Slab.Ctx.magRegion, hk.slots, hp.w]

theorem σ5_rd : (σ5 s).rd = s.rd := rfl
theorem σ5_wr : (σ5 s).wr = s.wr := rfl
theorem σ5_mem : (σ5 s).mem = m1 s := rfl
theorem σ5_rcx : (σ5 s).gpr .rcx = wv s := by rw [σ5, RegUpd.gpr_arithFlags, σ4_rcx]

theorem exec_lea8 : exec (.lea .rcx { base := .rcx, disp := -8 }) (σ5 s) = some (σ6 s) := by
  rw [exec_lea, Access.offset_base_disp, Access.add_ofInt_neg_lit, σ5_rcx]; rfl

theorem σ6_rcx : (σ6 s).gpr .rcx = wv s - 8 := RegUpd.gpr_setReg_self _ _ _
theorem σ6_rd : (σ6 s).rd = s.rd := rfl
theorem σ6_wr : (σ6 s).wr = s.wr := rfl
theorem σ6_mem : (σ6 s).mem = m1 s := rfl

theorem below_facts {k : Nat} (hp : Pre s) (hk : Sized s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    InRegions (s.rd ++ s.wr) (wv s - 8) 8 ∧ Region.Disjoint ⟨wv s - 8, 8⟩ ⟨area s + 8, 8⟩ := by
  obtain ⟨hs, h2, -⟩ := hit hp hk hi ht
  have hsl : IsSlot (cx.slots s) (cx.w s) := by rw [hk.slots, hp.w]; exact hs
  have hmag := hp.facts.mag hi hsl
  have hcm := hp.facts.cs_mag hi hsl
  rw [magRegion_eq hp hk] at hmag hcm
  have hc := contains_below hs h2 (slot_lt _ _ (Nat.two_pow_pos k))
  exact ⟨(InRegions.of_mem hmag hc).append_right, hcm.symm.sub_left (sub_of_contains hc)⟩

theorem exec_x {k : Nat} (hp : Pre s) (hk : Sized s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    exec (.mov .rax (.mem { base := .rcx })) (σ6 s) = some (σ7 s) := by
  obtain ⟨hin, hdj⟩ := below_facts hp hk hi ht
  have hin' : InRegions ((σ6 s).rd ++ (σ6 s).wr) ((σ6 s).ea { base := .rcx }) 8 := by
    rw [Access.ea_base, σ6_rcx, σ6_rd, σ6_wr]; exact hin
  rw [exec_load _ hin', Access.ea_base, σ6_rcx, σ6_mem, m1, readW64_writeW_disj hdj]
  rfl

theorem σ7_rdi : (σ7 s).gpr .rdi = s.gpr .rdi := by
  simp only [σ7, σ6, σ5, RegUpd.gpr_setReg_of_ne _ _ (show ¬ Reg.rdi = .rcx by decide),
    RegUpd.gpr_setReg_of_ne _ _ (show ¬ Reg.rdi = .rax by decide), RegUpd.gpr_arithFlags, σ4_rdi]
theorem σ7_r8 : (σ7 s).gpr .r8 = sh s := by
  simp only [σ7, σ6, σ5, RegUpd.gpr_setReg_of_ne _ _ (show ¬ Reg.r8 = .rcx by decide),
    RegUpd.gpr_setReg_of_ne _ _ (show ¬ Reg.r8 = .rax by decide), RegUpd.gpr_arithFlags, σ4_r8]
theorem σ7_rcx : (σ7 s).gpr .rcx = wv s - 8 := by
  rw [σ7, RegUpd.gpr_setReg_of_ne _ _ (by decide), σ6_rcx]
theorem σ7_mem : (σ7 s).mem = m1 s := rfl
theorem σ7_wr : (σ7 s).wr = s.wr := rfl

theorem exec_commit (hp : Pre s) (hi : cx.InRange s) :
    exec (.store (wordAt .rdi) .rcx) (σ7 s) = some (σ8 s) := by
  have hin : InRegions (σ7 s).wr ((σ7 s).ea (wordAt .rdi)) 8 := by
    rw [wordAt_ea _ σ7_rdi σ7_r8, σ7_wr, hp.word]
    exact inWr (r := ⟨cx.word s, 8⟩) (hp.facts.word hi)
  rw [exec_store _ hin, wordAt_ea _ σ7_rdi σ7_r8, σ7_rcx, σ7_mem]
  rfl

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
    runBody isa body (σA s) = some (.exit "slow" (σ5 s)) := by
  rw [body, runBody_instr (exec_cpu hp.facts.toArm), runBody_instr exec_cmp,
    runBody_skip (eval_cpu_false hi), runBody_instr exec_shl_r8,
    runBody_instr (exec_wordAt hp.facts hi hp.word), runBody_instr (exec_test hp),
    runBody_take (eval_test_true ht) execBlock_nil']

theorem prefixes_T (hp : Pre s) (hi : cx.InRange s) (ht : tst s = 0) :
    bodyPrefixes isa body (σA s) = [σA s, σ1 s, σ2 s, σ2 s, σ3 s, σ4 s, σ5 s] := by
  rw [body, bodyPrefixes_instr (exec_cpu hp.facts.toArm), bodyPrefixes_instr exec_cmp,
    bodyPrefixes_skip (eval_cpu_false hi), bodyPrefixes_instr exec_shl_r8,
    bodyPrefixes_instr (exec_wordAt hp.facts hi hp.word), bodyPrefixes_instr (exec_test hp),
    bodyPrefixes_take (eval_test_true ht)]

theorem end_T (hp : Pre s) (hi : cx.InRange s) (ht : tst s = 0) :
    bodyEnd isa body (σA s) = some 6 := by
  rw [body, bodyEnd_instr (exec_cpu hp.facts.toArm), bodyEnd_instr exec_cmp,
    bodyEnd_skip (eval_cpu_false hi), bodyEnd_instr exec_shl_r8,
    bodyEnd_instr (exec_wordAt hp.facts hi hp.word), bodyEnd_instr (exec_test hp),
    bodyEnd_take (eval_test_true ht)]
  rfl

theorem runBody_P (hp : Pre s) (hk : Sized s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    runBody isa body (σA s) = some (.done (σ8 s)) := by
  rw [body, runBody_instr (exec_cpu hp.facts.toArm), runBody_instr exec_cmp,
    runBody_skip (eval_cpu_false hi), runBody_instr exec_shl_r8,
    runBody_instr (exec_wordAt hp.facts hi hp.word), runBody_instr (exec_test hp),
    runBody_skip (eval_test_false ht), runBody_instr exec_lea8,
    runBody_instr (exec_x hp hk hi ht), runBody_instr (exec_commit hp hi), runBody_nil']

theorem prefixes_P (hp : Pre s) (hk : Sized s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    bodyPrefixes isa body (σA s) =
      [σA s, σ1 s, σ2 s, σ2 s, σ3 s, σ4 s, σ5 s, σ5 s, σ6 s, σ7 s] := by
  rw [body, bodyPrefixes_instr (exec_cpu hp.facts.toArm), bodyPrefixes_instr exec_cmp,
    bodyPrefixes_skip (eval_cpu_false hi), bodyPrefixes_instr exec_shl_r8,
    bodyPrefixes_instr (exec_wordAt hp.facts hi hp.word), bodyPrefixes_instr (exec_test hp),
    bodyPrefixes_skip (eval_test_false ht), bodyPrefixes_instr exec_lea8,
    bodyPrefixes_instr (exec_x hp hk hi ht), bodyPrefixes_last (exec_commit hp hi)]

theorem end_P (hp : Pre s) (hk : Sized s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    bodyEnd isa body (σA s) = some 10 := by
  rw [body, bodyEnd_instr (exec_cpu hp.facts.toArm), bodyEnd_instr exec_cmp,
    bodyEnd_skip (eval_cpu_false hi), bodyEnd_instr exec_shl_r8,
    bodyEnd_instr (exec_wordAt hp.facts hi hp.word), bodyEnd_instr (exec_test hp),
    bodyEnd_skip (eval_test_false ht), bodyEnd_instr exec_lea8,
    bodyEnd_instr (exec_x hp hk hi ht), bodyEnd_instr (exec_commit hp hi), bodyEnd_nil']
  rfl

/-! ### Memory accesses -/

theorem ev_cpu :
    Instr.events (.mov32 .r8 (.mem { base := .r10, disp := 4, seg := some .fs })) (σA s) (σ1 s) =
      [⟨area s + 4, 4, false, (cpu32 s).toNat⟩] := by
  simp only [Instr.events, cpuOp_ea, σA_mem, toNat_read4, m1]
  rw [readW_writeW_disj (cpu_cs_disjoint s) (by decide)]; rfl

theorem ev_mask (hp : Pre s) :
    Instr.events (.alu32 .test .rcx (maskAt .rdi)) (σ4 s) (σ5 s) = [⟨maskA s, 4, false, (mk s).toNat⟩] :=
  ev_test_mask hp.facts rfl hp.maskA .rcx σ4_mem σ4_rdi

theorem ev_x (hp : Pre s) (hk : Sized s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    Instr.events (.mov .rax (.mem { base := .rcx })) (σ6 s) (σ7 s) =
      [⟨wv s - 8, 8, false, (x s).toNat⟩] := by
  obtain ⟨-, hdj⟩ := below_facts hp hk hi ht
  simp only [Instr.events, Access.ea_base, σ6_rcx, σ6_mem, toNat_read8, m1]
  rw [readW64_writeW_disj hdj]; rfl

theorem ev_commit :
    Instr.events (.store (wordAt .rdi) .rcx) (σ7 s) (σ8 s) = [⟨wa s, 8, true, (wv s - 8).toNat⟩] := by
  simp only [Instr.events, wordAt_ea _ σ7_rdi σ7_r8, toNat_read8]
  rw [show (σ8 s).mem = mEnd s from rfl, mEnd, Mem.readW_writeW_self64]

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
    bodyEvents_instr (exec_wordAt hp.facts hi hp.word), bodyEvents_instr (exec_test hp),
    bodyEvents_take (eval_test_true ht)]
  simp only [isa, ev_cpu, ev_wordAt hp.facts hi hp.word, ev_mask hp]; rfl

theorem events_P (hp : Pre s) (hk : Sized s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    bodyEvents isa body (σA s) =
      [⟨area s + 4, 4, false, (cpu32 s).toNat⟩, ⟨wa s, 8, false, (wv s).toNat⟩,
        ⟨maskA s, 4, false, (mk s).toNat⟩, ⟨wv s - 8, 8, false, (x s).toNat⟩,
        ⟨wa s, 8, true, (wv s - 8).toNat⟩] := by
  rw [body, bodyEvents_instr (exec_cpu hp.facts.toArm), bodyEvents_instr exec_cmp,
    bodyEvents_skip (eval_cpu_false hi), bodyEvents_instr exec_shl_r8,
    bodyEvents_instr (exec_wordAt hp.facts hi hp.word), bodyEvents_instr (exec_test hp),
    bodyEvents_skip (eval_test_false ht), bodyEvents_instr exec_lea8,
    bodyEvents_instr (exec_x hp hk hi ht), bodyEvents_instr (exec_commit hp hi), bodyEvents_nil']
  simp only [ev_cpu, ev_wordAt hp.facts hi hp.word, ev_mask hp, ev_x hp hk hi ht, ev_commit]; rfl

end Cases

/-! ## What each state keeps -/

section Mids

variable {s : State}

theorem mσ2 : Mid scratchI s (m1 s) (σ2 s) :=
  mid_σ2 (mid_σ1 (by decide) (mid_σA (by decide) (by decide)))
theorem mσ3 : Mid scratchI s (m1 s) (σ3 s) := mid_σ3 (by decide) mσ2
theorem mσ4' : Mid scratchI s (m1 s) (σ4 s) := mσ4 mσ3 (by decide)
theorem mσ5 : Mid scratchI s (m1 s) (σ5 s) := mσ4'.arithFlags _ _ _
theorem mσ6 : Mid scratchI s (m1 s) (σ6 s) := mσ5.setReg (by decide) _
theorem mσ7 : Mid scratchI s (m1 s) (σ7 s) := mσ6.setReg (by decide) _
theorem mσ8 : Mid scratchI s (mEnd s) (σ8 s) := mσ7.withMem _

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
theorem abi_m1 (hp : Pre s) {p : State} (hm : Mid scratchI s (m1 s) p) : abiPreserved s p :=
  abiPreserved_of_mid frame_m1 (by simpa using hp.facts.ret_cs) (by decide) hm

theorem popped (hp : Pre s) (hk : Sized s k) (hi : cx.InRange s) (ht : ¬ tst s = 0) :
    cx.Popped s (σ8 s) (x s) := by
  obtain ⟨hs, h2, hpop⟩ := hit hp hk hi ht
  have hsl : IsSlot (cx.slots s) (cx.w s) := by rw [hk.slots, hp.w]; exact hs
  have hcm := hp.facts.cs_mag hi hsl
  have hwm := hp.facts.word_mag hi hsl
  rw [magRegion_eq hp hk] at hcm hwm
  refine ⟨.stack (magBase (2 ^ k) (wv s)) (items (2 ^ k) s.mem (wv s)).dropLast, ?_, ?_, ?_, ?_⟩
  · simp only [Spec.Slab.Ctx.popResult, hi, ↓reduceIte, Spec.Slab.Ctx.mag, hk.slots,
      mem_of (c := cx) hX, hp.w]
    exact hpop
  · rw [csField_of (c := cx) hX, ← hp.word]
    exact frame_two s.mem _ _ _ _
  · show (mEnd s).readW (cx.word s) 64 = _
    rw [mEnd, ← hp.word, Mem.readW_writeW_self64, hp.w]
  · show Mag.ofWord (cx.slots s) (mEnd s) (cx.w s - 8) = _
    rw [hk.slots, hp.w]
    refine ofWord_sub8 (Nat.two_pow_pos k) hs h2 fun i hi' => ?_
    have hsub := sub_item (n := 2 ^ k) (w := wv s) (i := i)
      (by have := slot_lt (2 ^ k) (wv s) (Nat.two_pow_pos k); omega)
    rw [mEnd, readW64_writeW_disj (by rw [hp.word]; exact (hwm.symm.sub_left hsub)), m1,
      readW64_writeW_disj (hcm.symm.sub_left hsub)]

theorem ret_σ8 : (σ8 s).gpr .rax = x s := by
  simp only [σ8, σ7, RegUpd.gpr_setReg_self]

theorem unchanged_cs : Unchanged [cx.csField s] (cx.mem s) (m1 s) := by
  rw [csField_of (c := cx) hX, mem_of (c := cx) hX]; exact frame_m1

theorem missed_C (hi : ¬ cx.InRange s) : cx.PopMissed s (σ2 s) := by
  refine ⟨?_, unchanged_cs⟩
  simp only [Spec.Slab.Ctx.popResult, hi, ↓reduceIte]

theorem missed_T (hp : Pre s) (hi : cx.InRange s) (ht : tst s = 0) : cx.PopMissed s (σ5 s) := by
  refine ⟨?_, unchanged_cs⟩
  simp only [Spec.Slab.Ctx.popResult, hi, ↓reduceIte, Spec.Slab.Ctx.mag]
  by_cases hz : cx.mask s = 0
  · rw [slots_zero hz]; exact pop_one _ _
  · obtain ⟨k, hk⟩ := hp.sized hz
    rw [hk.slots, mem_of (c := cx) hX, hp.w]
    exact (pop_eq_none_iff (dvd_marker hk.hk') (hk.ok hi)).mpr
      ((test_iff32 hk.hk hk.hk' (wv s) (mk s) hk.mask).mp ht)

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
  have hp := pre_of h
  rw [code_eq]
  by_cases hi : cx.InRange s
  · by_cases ht : tst s = 0
    · exact .inr ⟨"slow", σ5 s, .rseq (execBlock_arm h.toArm) (runBody_T hp hi ht), abi_m1 hp mσ5,
        (exit_eq _ _).mpr ⟨by rw [mσ5.gpr _ (by decide)], rfl, missed_T hp hi ht⟩⟩
    · obtain ⟨k, hk⟩ := sized_of_test hp ht
      have hw := hp.facts.ret_word hi
      rw [← hp.word] at hw
      exact .inl ⟨σ8 s, .rseq (execBlock_arm h.toArm) (runBody_P hp hk hi ht),
        abiPreserved_of_mid (frame_two s.mem _ _ _ _) (by simpa using ⟨hp.facts.ret_cs, hw⟩)
          (by decide) mσ8,
        (post_eq _).mpr ⟨x s, popped hp hk hi ht, ret_σ8⟩⟩
  · exact .inr ⟨"slow", σ2 s, .rseq (execBlock_arm h.toArm) (runBody_C hp hi), abi_m1 hp mσ2,
      (exit_eq _ _).mpr ⟨by rw [mσ2.gpr _ (by decide)], rfl, missed_C hi⟩⟩

/-! ### Where it may be aborted -/

theorem abort_eq (p : State) : (popContract abi).abort s p ↔ [p.gpr .rdi] = [s.gpr .rdi] ∧
    (Unchanged [cx.csField s] (cx.mem s) p.mem ∧ cx.inv p) :=
  abort_iff cx hv s p

theorem abortOk (hp : Pre s) {p : State} (hm : Mid scratchI s (m1 s) p) :
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

theorem wordAddr_eq (hp : Pre s) :
    wordAddr (s.syms SLABS) (cpu32 s).toNat (cx.cls s) = wa s := by
  rw [hp.word]; rfl

theorem maskAddr_eq (hp : Pre s) : cx.maskAddr s = maskA s := hp.maskA.symm

theorem test_eq : BitVec.ofNat 32 (wv s).toNat &&& BitVec.ofNat 32 (cx.testMask s (mk s).toNat) =
    tst s := by
  simp only [Spec.Slab.Ctx.testMask, BitVec.ofNat_toNat, tst]; rfl

theorem trace_ok (hs : (popContract abi).pre s) :
    (popContract abi).trace s (Code.events isa popTable s) := by
  have h := facts_of_pre hs
  have hp := pre_of h
  show cx.PopTrace s (Code.events isa (.rseq arm body) s)
  simp only [Code.events, execBlock_arm h.toArm, events_arm h.toArm]
  refine ⟨(o s).toNat, s.csDesc.toNat, (cpu32 s).toNat, (wv s).toNat, (mk s).toNat, (x s).toNat, ?_⟩
  simp only [Spec.Slab.Ctx.headTrace, Spec.Slab.Ctx.rd, Spec.Slab.Ctx.wr, tp_of (c := cx) hX,
    sym_of (c := cx) hX, test_eq, ofNat_toNat64, addr8, addr4, rsName, maskAddr_eq hp]
  by_cases hi : cx.InRange s
  · have hi' : (cpu32 s).toNat < SLAB_COUNT := (inRange_iff.mp hi)
    rw [wordAddr_eq hp]
    by_cases ht : tst s = 0
    · rw [events_T hp hi ht]; simp [hi', ht]
    · obtain ⟨k, hk⟩ := sized_of_test hp ht
      rw [events_P hp hk hi ht]; simpa [hi'] using ht
  · have hi' : ¬ (cpu32 s).toNat < SLAB_COUNT := fun h' => hi (inRange_iff.mpr h')
    rw [events_C hp hi]; simp [hi']

theorem rseqOk (hs : (popContract abi).pre s) : RseqOk target popTable (popContract abi) s := by
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
      exact ⟨abortOk hp mA, abortOk hp m1', abortOk hp mσ2, abortOk hp mσ2, abortOk hp mσ3,
        abortOk hp mσ4', abortOk hp mσ5⟩
    · obtain ⟨k, hk⟩ := sized_of_test hp ht
      rw [prefixes_P hp hk hi ht]
      simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
      exact ⟨abortOk hp mA, abortOk hp m1', abortOk hp mσ2, abortOk hp mσ2, abortOk hp mσ3,
        abortOk hp mσ4', abortOk hp mσ5, abortOk hp mσ5, abortOk hp mσ6, abortOk hp mσ7⟩
  · rw [prefixes_C hp hi]
    simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
    exact ⟨abortOk hp mA, abortOk hp m1', abortOk hp mσ2⟩

end Contract

end VG.Proof.Slab.X86_64.PopTable
