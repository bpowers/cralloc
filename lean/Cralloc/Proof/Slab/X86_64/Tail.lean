module

public import Cralloc.Proof.Slab.X86_64.Mid

/-!
# The out-of-line pop's and push's steps, from any state

`popSteps w` and `pushSteps w p` (`Impl.Slab.X86_64`), run from a state `τ`
in which the register `w` holds the address of a class's word in slab 0 and
`r8` the CPU's slab's offset, whatever code came before: their runs, the
states where they may be aborted, where they end, and their memory accesses,
each under what it needs of `τ`'s regions. A routine that finds its class
first (the C entry points) proves that its own steps reach such a state and
ends with these. Nothing here depends on the register `w` but that the steps
do not write it (`rax`, `rcx` and `r8` are not `w`), nor on how `τ` was
reached.
-/

@[expose] public section

namespace VG.Proof.Slab.X86_64.Tail

open VG.X86_64 VG.Spec.Slab VG.Impl.Slab.X86_64 VG.Proof.Slab.X86_64

variable (w : Reg) (τ : State)

/-- The slab word's address and the word; the mask's address and the mask. -/
def wa : Addr := τ.gpr w + τ.gpr .r8
def wv : BitVec 64 := τ.mem.readW (wa w τ) 64
def ma : Addr := τ.gpr w + 33556480
def mk : BitVec 32 := τ.mem.readW (ma w τ) 32

/-- After loading the slab word. -/
def t1 : State := τ.setReg .rcx (wv w τ)

/-! ## Pop -/

def ptst : BitVec 32 := (wv w τ).setWidth 32 &&& mk w τ
def p2 : State := arithFlags (t1 w τ) (ptst w τ) false false
def p3 : State := (p2 w τ).setReg .rcx (wv w τ - 8)
def px : BitVec 64 := τ.mem.readW (wv w τ - 8) 64
def p4 : State := (p3 w τ).setReg .rax (px w τ)
def p5 : State := { p4 w τ with mem := τ.mem.writeW (wa w τ) (wv w τ - 8) }

/-! ## Push of the register `p` -/

def q2 : State := (t1 w τ).setReg .rax (wv w τ + 8)
def qtst : BitVec 32 := (wv w τ + 8).setWidth 32 &&& mk w τ
def q3 : State := arithFlags (q2 w τ) (qtst w τ) false false
def qSlot (p : Reg) : Mem := τ.mem.writeW (wv w τ) (τ.gpr p)
def q4 (p : Reg) : State := { q3 w τ with mem := qSlot w τ p }
def qEnd (p : Reg) : Mem := (qSlot w τ p).writeW (wa w τ) (wv w τ + 8)
def q5 (p : Reg) : State := { q4 w τ p with mem := qEnd w τ p }

/-- The word register is none the steps write. -/
structure WReg (w : Reg) : Prop where
  rcx : w ≠ .rcx
  rax : w ≠ .rax
  r8 : w ≠ .r8

/-- The pushed pointer's register is none the steps write. -/
structure PReg (p : Reg) : Prop where
  rcx : p ≠ .rcx
  rax : p ≠ .rax

variable {w τ}

section Regs

theorem t1_w (hr : WReg w) : (t1 w τ).gpr w = τ.gpr w := RegUpd.gpr_setReg_of_ne _ _ hr.rcx
theorem t1_r8 : (t1 w τ).gpr .r8 = τ.gpr .r8 := RegUpd.gpr_setReg_of_ne _ _ (by decide)

theorem maskAt_ea {σ : State} (h : σ.gpr w = τ.gpr w) :
    σ.ea { base := w, disp := 33556480 } = ma w τ := by
  rw [Access.ea_base_disp, BitVec.ofInt_ofNat, h]; rfl

theorem wordAt_ea {σ : State} (hw : σ.gpr w = τ.gpr w) (hr : σ.gpr .r8 = τ.gpr .r8) :
    σ.ea (wordAt w) = wa w τ := by
  rw [wordAt, Access.ea_of_seg_none rfl, Access.offset_index, hw, hr]
  simp only [BitVec.ofInt_ofNat, BitVec.mul_one, BitVec.add_zero]; rfl

theorem wordAt_τ : τ.ea (wordAt w) = wa w τ := wordAt_ea rfl rfl

end Regs

theorem exec_word (hW : InRegions (τ.rd ++ τ.wr) (wa w τ) 8) :
    exec (.mov .rcx (.mem (wordAt w))) τ = some (t1 w τ) := by
  rw [exec_load _ (by rw [wordAt_τ]; exact hW), wordAt_τ]; rfl

theorem ev_word :
    Instr.events (.mov .rcx (.mem (wordAt w))) τ (t1 w τ) = [⟨wa w τ, 8, false, (wv w τ).toNat⟩] := by
  simp only [Instr.events, wordAt_τ, toNat_read8]; rfl

/-! ## The pop's steps -/

section Pop

theorem exec_ptest (hr : WReg w) (hM : InRegions (τ.rd ++ τ.wr) (ma w τ) 4) :
    exec (.alu32 .test .rcx (maskAt w)) (t1 w τ) = some (p2 w τ) := by
  have e := maskAt_ea (τ := τ) (σ := t1 w τ) (t1_w hr)
  rw [maskAt, exec_test32_mem _ (by rw [e]; exact hM), e]
  simp only [t1, RegUpd.gpr_setReg_self]; rfl

theorem ev_ptest (hr : WReg w) :
    Instr.events (.alu32 .test .rcx (maskAt w)) (t1 w τ) (p2 w τ) = [⟨ma w τ, 4, false, (mk w τ).toNat⟩] := by
  simp only [Instr.events, maskAt, maskAt_ea (t1_w hr), toNat_read4]; rfl

theorem exec_plea : exec (.lea .rcx { base := .rcx, disp := -8 }) (p2 w τ) = some (p3 w τ) := by
  rw [exec_lea, Access.offset_base_disp, Access.add_ofInt_neg_lit]
  simp only [p2, RegUpd.gpr_arithFlags, t1, RegUpd.gpr_setReg_self]; rfl

theorem p3_rcx : (p3 w τ).gpr .rcx = wv w τ - 8 := RegUpd.gpr_setReg_self _ _ _

theorem exec_px (hX : InRegions (τ.rd ++ τ.wr) (wv w τ - 8) 8) :
    exec (.mov .rax (.mem { base := .rcx })) (p3 w τ) = some (p4 w τ) := by
  rw [exec_load _ (by rw [Access.ea_base, p3_rcx]; exact hX), Access.ea_base, p3_rcx]
  rfl

theorem ev_px : Instr.events (.mov .rax (.mem { base := .rcx })) (p3 w τ) (p4 w τ) =
    [⟨wv w τ - 8, 8, false, (px w τ).toNat⟩] := by
  simp only [Instr.events, Access.ea_base, p3_rcx, toNat_read8]; rfl

theorem p4_w (hr : WReg w) : (p4 w τ).gpr w = τ.gpr w := by
  simp only [p4, p3, p2, t1, RegUpd.gpr_setReg_of_ne _ _ hr.rax, RegUpd.gpr_setReg_of_ne _ _ hr.rcx,
    RegUpd.gpr_arithFlags]
theorem p4_r8 : (p4 w τ).gpr .r8 = τ.gpr .r8 := by
  simp only [p4, p3, p2, t1, RegUpd.gpr_setReg_of_ne _ _ (show Reg.r8 ≠ .rax by decide),
    RegUpd.gpr_setReg_of_ne _ _ (show Reg.r8 ≠ .rcx by decide), RegUpd.gpr_arithFlags]
theorem p4_rcx : (p4 w τ).gpr .rcx = wv w τ - 8 := by
  rw [p4, RegUpd.gpr_setReg_of_ne _ _ (by decide), p3_rcx]
theorem p5_rax : (p5 w τ).gpr .rax = px w τ := by
  show (p4 w τ).gpr .rax = _; exact RegUpd.gpr_setReg_self _ _ _

theorem exec_pcommit (hr : WReg w) (hWw : InRegions τ.wr (wa w τ) 8) :
    exec (.store (wordAt w) .rcx) (p4 w τ) = some (p5 w τ) := by
  have e := wordAt_ea (τ := τ) (p4_w hr) p4_r8
  rw [exec_store _ (by rw [e]; exact hWw), e, p4_rcx]; rfl

theorem ev_pcommit (hr : WReg w) : Instr.events (.store (wordAt w) .rcx) (p4 w τ) (p5 w τ) =
    [⟨wa w τ, 8, true, (wv w τ - 8).toNat⟩] := by
  simp only [Instr.events, wordAt_ea (τ := τ) (p4_w hr) p4_r8, toNat_read8]
  rw [show (p5 w τ).mem = τ.mem.writeW (wa w τ) (wv w τ - 8) from rfl, Mem.readW_writeW_self64]

theorem eval_ptest_true (ht : ptst w τ = 0) : eval .e (p2 w τ) = some true := by
  show some (ptst w τ == 0) = _; rw [ht]; rfl
theorem eval_ptest_false (ht : ¬ ptst w τ = 0) : eval .e (p2 w τ) = some false := by
  show some (ptst w τ == 0) = _; simpa using ht

/-- What a pop's steps need of `τ`'s regions: the word and the mask readable;
on a hit, the slot below readable and the word writable. -/
structure PopOk (w : Reg) (τ : State) : Prop where
  reg : WReg w
  word : InRegions (τ.rd ++ τ.wr) (wa w τ) 8
  mask : InRegions (τ.rd ++ τ.wr) (ma w τ) 4
  hit : ¬ ptst w τ = 0 → InRegions (τ.rd ++ τ.wr) (wv w τ - 8) 8 ∧ InRegions τ.wr (wa w τ) 8

variable (h : PopOk w τ) (t : String)
include h

theorem runPop_miss (ht : ptst w τ = 0) :
    runBody isa (popSteps w [] (some t)) τ = some (.exit t (p2 w τ)) := by
  rw [popSteps, runBody_instr (exec_word h.word), runBody_instr (exec_ptest h.reg h.mask),
    runBody_take (eval_ptest_true ht) execBlock_nil']

theorem prefixesPop_miss (ht : ptst w τ = 0) :
    bodyPrefixes isa (popSteps w [] (some t)) τ = [τ, t1 w τ, p2 w τ] := by
  rw [popSteps, bodyPrefixes_instr (exec_word h.word), bodyPrefixes_instr (exec_ptest h.reg h.mask),
    bodyPrefixes_take (eval_ptest_true ht)]

theorem endPop_miss (ht : ptst w τ = 0) :
    bodyEnd isa (popSteps w [] (some t)) τ = some 2 := by
  rw [popSteps, bodyEnd_instr (exec_word h.word), bodyEnd_instr (exec_ptest h.reg h.mask),
    bodyEnd_take (eval_ptest_true ht)]; rfl

theorem eventsPop_miss (ht : ptst w τ = 0) :
    bodyEvents isa (popSteps w [] (some t)) τ =
      [⟨wa w τ, 8, false, (wv w τ).toNat⟩, ⟨ma w τ, 4, false, (mk w τ).toNat⟩] := by
  rw [popSteps, bodyEvents_instr (exec_word h.word), bodyEvents_instr (exec_ptest h.reg h.mask),
    bodyEvents_take (eval_ptest_true ht)]
  simp only [isa, ev_word, ev_ptest h.reg]; rfl

theorem runPop_hit (ht : ¬ ptst w τ = 0) :
    runBody isa (popSteps w [] (some t)) τ = some (.done (p5 w τ)) := by
  rw [popSteps, runBody_instr (exec_word h.word), runBody_instr (exec_ptest h.reg h.mask),
    runBody_skip (eval_ptest_false ht), runBody_instr exec_plea,
    runBody_instr (exec_px (h.hit ht).1), runBody_instr (exec_pcommit h.reg (h.hit ht).2), runBody_nil']

theorem prefixesPop_hit (ht : ¬ ptst w τ = 0) :
    bodyPrefixes isa (popSteps w [] (some t)) τ = [τ, t1 w τ, p2 w τ, p2 w τ, p3 w τ, p4 w τ] := by
  rw [popSteps, bodyPrefixes_instr (exec_word h.word), bodyPrefixes_instr (exec_ptest h.reg h.mask),
    bodyPrefixes_skip (eval_ptest_false ht), bodyPrefixes_instr exec_plea,
    bodyPrefixes_instr (exec_px (h.hit ht).1), bodyPrefixes_last (exec_pcommit h.reg (h.hit ht).2)]

theorem endPop_hit (ht : ¬ ptst w τ = 0) :
    bodyEnd isa (popSteps w [] (some t)) τ = some 6 := by
  rw [popSteps, bodyEnd_instr (exec_word h.word), bodyEnd_instr (exec_ptest h.reg h.mask),
    bodyEnd_skip (eval_ptest_false ht), bodyEnd_instr exec_plea,
    bodyEnd_instr (exec_px (h.hit ht).1), bodyEnd_instr (exec_pcommit h.reg (h.hit ht).2), bodyEnd_nil']
  rfl

theorem eventsPop_hit (ht : ¬ ptst w τ = 0) :
    bodyEvents isa (popSteps w [] (some t)) τ =
      [⟨wa w τ, 8, false, (wv w τ).toNat⟩, ⟨ma w τ, 4, false, (mk w τ).toNat⟩,
        ⟨wv w τ - 8, 8, false, (px w τ).toNat⟩, ⟨wa w τ, 8, true, (wv w τ - 8).toNat⟩] := by
  rw [popSteps, bodyEvents_instr (exec_word h.word), bodyEvents_instr (exec_ptest h.reg h.mask),
    bodyEvents_skip (eval_ptest_false ht), bodyEvents_instr exec_plea,
    bodyEvents_instr (exec_px (h.hit ht).1), bodyEvents_instr (exec_pcommit h.reg (h.hit ht).2),
    bodyEvents_nil']
  simp only [ev_word, ev_ptest h.reg, ev_px, ev_pcommit h.reg]; rfl

end Pop

/-! ## The push's steps -/

section Push

variable {p : Reg}

theorem exec_qadd : exec (.lea .rax { base := .rcx, disp := 8 }) (t1 w τ) = some (q2 w τ) := by
  rw [exec_lea, Access.offset_base_disp]; simp only [t1, RegUpd.gpr_setReg_self]; rfl

theorem q2_w (hr : WReg w) : (q2 w τ).gpr w = τ.gpr w := by
  rw [q2, RegUpd.gpr_setReg_of_ne _ _ hr.rax, t1_w hr]

theorem exec_qtest (hr : WReg w) (hM : InRegions (τ.rd ++ τ.wr) (ma w τ) 4) :
    exec (.alu32 .test .rax (maskAt w)) (q2 w τ) = some (q3 w τ) := by
  have e := maskAt_ea (τ := τ) (σ := q2 w τ) (q2_w hr)
  rw [maskAt, exec_test32_mem _ (by rw [e]; exact hM), e]
  simp only [q2, RegUpd.gpr_setReg_self]; rfl

theorem ev_qtest (hr : WReg w) :
    Instr.events (.alu32 .test .rax (maskAt w)) (q2 w τ) (q3 w τ) = [⟨ma w τ, 4, false, (mk w τ).toNat⟩] := by
  simp only [Instr.events, maskAt, maskAt_ea (q2_w hr), toNat_read4]; rfl

theorem q3_rcx : (q3 w τ).gpr .rcx = wv w τ := by
  simp only [q3, q2, t1, RegUpd.gpr_arithFlags, RegUpd.gpr_setReg_of_ne _ _ (show Reg.rcx ≠ .rax by decide),
    RegUpd.gpr_setReg_self]
theorem q3_p (hp : PReg p) : (q3 w τ).gpr p = τ.gpr p := by
  simp only [q3, q2, t1, RegUpd.gpr_arithFlags, RegUpd.gpr_setReg_of_ne _ _ hp.rax,
    RegUpd.gpr_setReg_of_ne _ _ hp.rcx]
theorem q3_w (hr : WReg w) : (q3 w τ).gpr w = τ.gpr w := by
  rw [q3, RegUpd.gpr_arithFlags]; exact q2_w hr
theorem q3_r8 : (q3 w τ).gpr .r8 = τ.gpr .r8 := by
  simp only [q3, q2, t1, RegUpd.gpr_arithFlags, RegUpd.gpr_setReg_of_ne _ _ (show Reg.r8 ≠ .rax by decide),
    RegUpd.gpr_setReg_of_ne _ _ (show Reg.r8 ≠ .rcx by decide)]
theorem q3_rax : (q3 w τ).gpr .rax = wv w τ + 8 := by
  simp only [q3, q2, RegUpd.gpr_arithFlags, RegUpd.gpr_setReg_self]

theorem exec_qslot (hp : PReg p) (hS : InRegions τ.wr (wv w τ) 8) :
    exec (.store { base := .rcx } p) (q3 w τ) = some (q4 w τ p) := by
  rw [exec_store _ (by rw [Access.ea_base, q3_rcx]; exact hS), Access.ea_base, q3_rcx, q3_p hp]
  rfl

theorem ev_qslot : Instr.events (.store { base := .rcx } p) (q3 w τ) (q4 w τ p) =
    [⟨wv w τ, 8, true, (τ.gpr p).toNat⟩] := by
  simp only [Instr.events, Access.ea_base, q3_rcx, toNat_read8]
  rw [show (q4 w τ p).mem = qSlot w τ p from rfl, qSlot, Mem.readW_writeW_self64]

theorem q4_gpr : (q4 w τ p).gpr = (q3 w τ).gpr := rfl

theorem exec_qcommit (hr : WReg w) (hWw : InRegions τ.wr (wa w τ) 8) :
    exec (.store (wordAt w) .rax) (q4 w τ p) = some (q5 w τ p) := by
  have e := wordAt_ea (τ := τ) (σ := q4 w τ p) (by rw [q4_gpr]; exact q3_w hr) (by rw [q4_gpr]; exact q3_r8)
  rw [exec_store _ (by rw [e]; exact hWw), e, q4_gpr, q3_rax]
  rfl

theorem ev_qcommit (hr : WReg w) : Instr.events (.store (wordAt w) .rax) (q4 w τ p) (q5 w τ p) =
    [⟨wa w τ, 8, true, (wv w τ + 8).toNat⟩] := by
  simp only [Instr.events, wordAt_ea (τ := τ) (σ := q4 w τ p) (by rw [q4_gpr]; exact q3_w hr)
    (by rw [q4_gpr]; exact q3_r8), toNat_read8]
  rw [show (q5 w τ p).mem = qEnd w τ p from rfl, qEnd, Mem.readW_writeW_self64]

theorem eval_qtest_true (ht : qtst w τ = 0) : eval .e (q3 w τ) = some true := by
  show some (qtst w τ == 0) = _; rw [ht]; rfl
theorem eval_qtest_false (ht : ¬ qtst w τ = 0) : eval .e (q3 w τ) = some false := by
  show some (qtst w τ == 0) = _; simpa using ht

/-- What a push's steps need of `τ`'s regions: the word and the mask
readable; on a hit, the free slot and the word writable. -/
structure PushOk (w p : Reg) (τ : State) : Prop where
  reg : WReg w
  preg : PReg p
  word : InRegions (τ.rd ++ τ.wr) (wa w τ) 8
  mask : InRegions (τ.rd ++ τ.wr) (ma w τ) 4
  hit : ¬ qtst w τ = 0 → InRegions τ.wr (wv w τ) 8 ∧ InRegions τ.wr (wa w τ) 8

variable (h : PushOk w p τ) (t : String)
include h

theorem runPush_miss (ht : qtst w τ = 0) :
    runBody isa (pushSteps w p [] (some t)) τ = some (.exit t (q3 w τ)) := by
  rw [pushSteps, runBody_instr (exec_word h.word), runBody_instr exec_qadd,
    runBody_instr (exec_qtest h.reg h.mask), runBody_take (eval_qtest_true ht) execBlock_nil']

theorem prefixesPush_miss (ht : qtst w τ = 0) :
    bodyPrefixes isa (pushSteps w p [] (some t)) τ = [τ, t1 w τ, q2 w τ, q3 w τ] := by
  rw [pushSteps, bodyPrefixes_instr (exec_word h.word), bodyPrefixes_instr exec_qadd,
    bodyPrefixes_instr (exec_qtest h.reg h.mask), bodyPrefixes_take (eval_qtest_true ht)]

theorem endPush_miss (ht : qtst w τ = 0) :
    bodyEnd isa (pushSteps w p [] (some t)) τ = some 3 := by
  rw [pushSteps, bodyEnd_instr (exec_word h.word), bodyEnd_instr exec_qadd,
    bodyEnd_instr (exec_qtest h.reg h.mask), bodyEnd_take (eval_qtest_true ht)]; rfl

theorem eventsPush_miss (ht : qtst w τ = 0) :
    bodyEvents isa (pushSteps w p [] (some t)) τ =
      [⟨wa w τ, 8, false, (wv w τ).toNat⟩, ⟨ma w τ, 4, false, (mk w τ).toNat⟩] := by
  rw [pushSteps, bodyEvents_instr (exec_word h.word), bodyEvents_instr exec_qadd,
    bodyEvents_instr (exec_qtest h.reg h.mask), bodyEvents_take (eval_qtest_true ht)]
  simp only [isa, ev_word, ev_qtest h.reg]; rfl

theorem runPush_hit (ht : ¬ qtst w τ = 0) :
    runBody isa (pushSteps w p [] (some t)) τ = some (.done (q5 w τ p)) := by
  rw [pushSteps, runBody_instr (exec_word h.word), runBody_instr exec_qadd,
    runBody_instr (exec_qtest h.reg h.mask), runBody_skip (eval_qtest_false ht),
    runBody_instr (exec_qslot h.preg (h.hit ht).1), runBody_instr (exec_qcommit h.reg (h.hit ht).2),
    runBody_nil']

theorem prefixesPush_hit (ht : ¬ qtst w τ = 0) :
    bodyPrefixes isa (pushSteps w p [] (some t)) τ = [τ, t1 w τ, q2 w τ, q3 w τ, q3 w τ, q4 w τ p] := by
  rw [pushSteps, bodyPrefixes_instr (exec_word h.word), bodyPrefixes_instr exec_qadd,
    bodyPrefixes_instr (exec_qtest h.reg h.mask), bodyPrefixes_skip (eval_qtest_false ht),
    bodyPrefixes_instr (exec_qslot h.preg (h.hit ht).1),
    bodyPrefixes_last (exec_qcommit h.reg (h.hit ht).2)]

theorem endPush_hit (ht : ¬ qtst w τ = 0) :
    bodyEnd isa (pushSteps w p [] (some t)) τ = some 6 := by
  rw [pushSteps, bodyEnd_instr (exec_word h.word), bodyEnd_instr exec_qadd,
    bodyEnd_instr (exec_qtest h.reg h.mask), bodyEnd_skip (eval_qtest_false ht),
    bodyEnd_instr (exec_qslot h.preg (h.hit ht).1), bodyEnd_instr (exec_qcommit h.reg (h.hit ht).2),
    bodyEnd_nil']
  rfl

theorem eventsPush_hit (ht : ¬ qtst w τ = 0) :
    bodyEvents isa (pushSteps w p [] (some t)) τ =
      [⟨wa w τ, 8, false, (wv w τ).toNat⟩, ⟨ma w τ, 4, false, (mk w τ).toNat⟩,
        ⟨wv w τ, 8, true, (τ.gpr p).toNat⟩, ⟨wa w τ, 8, true, (wv w τ + 8).toNat⟩] := by
  rw [pushSteps, bodyEvents_instr (exec_word h.word), bodyEvents_instr exec_qadd,
    bodyEvents_instr (exec_qtest h.reg h.mask), bodyEvents_skip (eval_qtest_false ht),
    bodyEvents_instr (exec_qslot h.preg (h.hit ht).1), bodyEvents_instr (exec_qcommit h.reg (h.hit ht).2),
    bodyEvents_nil']
  simp only [ev_word, ev_qtest h.reg, ev_qslot, ev_qcommit h.reg]; rfl

end Push

/-! ## What each state keeps -/

section Mids

variable {rs : List Reg} {s : State} {m : Mem}

theorem mid_t1 (hτ : Mid rs s m τ) (hrc : .rcx ∈ rs) : Mid rs s m (t1 w τ) := hτ.setReg hrc _
theorem mid_p2 (hτ : Mid rs s m τ) (hrc : .rcx ∈ rs) : Mid rs s m (p2 w τ) :=
  (mid_t1 hτ hrc).arithFlags _ _ _
theorem mid_p3 (hτ : Mid rs s m τ) (hrc : .rcx ∈ rs) : Mid rs s m (p3 w τ) :=
  (mid_p2 hτ hrc).setReg hrc _
theorem mid_p4 (hτ : Mid rs s m τ) (hrc : .rcx ∈ rs) (hra : .rax ∈ rs) : Mid rs s m (p4 w τ) :=
  (mid_p3 hτ hrc).setReg hra _
theorem mid_p5 (hτ : Mid rs s m τ) (hrc : .rcx ∈ rs) (hra : .rax ∈ rs) :
    Mid rs s (τ.mem.writeW (wa w τ) (wv w τ - 8)) (p5 w τ) := (mid_p4 hτ hrc hra).withMem _
theorem mid_q2 (hτ : Mid rs s m τ) (hrc : .rcx ∈ rs) (hra : .rax ∈ rs) : Mid rs s m (q2 w τ) :=
  (mid_t1 hτ hrc).setReg hra _
theorem mid_q3 (hτ : Mid rs s m τ) (hrc : .rcx ∈ rs) (hra : .rax ∈ rs) : Mid rs s m (q3 w τ) :=
  (mid_q2 hτ hrc hra).arithFlags _ _ _
theorem mid_q4 (hτ : Mid rs s m τ) (hrc : .rcx ∈ rs) (hra : .rax ∈ rs) (p : Reg) :
    Mid rs s (qSlot w τ p) (q4 w τ p) := (mid_q3 hτ hrc hra).withMem _
theorem mid_q5 (hτ : Mid rs s m τ) (hrc : .rcx ∈ rs) (hra : .rax ∈ rs) (p : Reg) :
    Mid rs s (qEnd w τ p) (q5 w τ p) := (mid_q4 hτ hrc hra p).withMem _

end Mids

end VG.Proof.Slab.X86_64.Tail
