module

public import Cralloc.Proof.CEntry.AArch64.Facts

/-!
# The C routines' back half on AArch64

Each C routine ends with the pop's or push's steps through `[x3, x5, lsl #3]`, from a state its
front half reaches: one with memory as after the arm, the entry state's regions, the CPU's slab in
`x3`, the class it found in `x5` and the masks' table in `x15` (`Ready`). `Tail` has the steps, from
any state, and `Ready` and `BackFacts` what the precondition makes of them: that they run, and that
their results are the contract's.
-/

@[expose] public section

namespace VG.Proof.CEntry.AArch64

open VG.AArch64 VG.Spec.Slab VG.Spec.CEntry VG.Impl.Slab.AArch64 VG.Impl.CEntry.AArch64
  VG.Proof.Slab.AArch64 VG.Proof.CEntry VG.Proof.Slab

namespace Tail

/-! ## The steps -/

variable (w n : Reg) (τ : State)

/-- The slab word's address and the word; the mask's entry's address and the entry. -/
def wa : Addr := τ.gpr .x3 + τ.gpr .x5 <<< 3
def wv : BitVec 64 := τ.mem.readW (wa τ) 64
def ma : Addr := τ.gpr .x15 + τ.gpr .x5 <<< 3
def mk : BitVec 64 := τ.mem.readW (ma τ) 64

/-- After loading the slab word, and the entry. -/
def t1 : State := τ.setReg w (wv τ)
def t2 : State := (t1 w τ).setReg .x6 (mk τ)

/-! Pop -/

def ptst : BitVec 32 := (wv τ).setWidth 32 &&& (mk τ).setWidth 32
def p3 : State := Flags.tstState (t2 w τ) (ptst τ)
def px : BitVec 64 := τ.mem.readW (wv τ - 8) 64
def p4 : State := (p3 w τ).setReg .x4 (px τ)
def p5 : State := (p4 w τ).setReg w (wv τ - 8)
def p6 : State := { p5 w τ with mem := τ.mem.writeW (wa τ) (wv τ - 8) }

/-! Push of `x0` -/

def q3 : State := (t2 w τ).setReg n (wv τ + 8)
def qtst : BitVec 32 := (wv τ + 8).setWidth 32 &&& (mk τ).setWidth 32
def q4 : State := Flags.tstState (q3 w n τ) (qtst τ)
def qSlot : Mem := τ.mem.writeW (wv τ) (τ.gpr .x0)
def q5 : State := { q4 w n τ with mem := qSlot τ }
def qEnd : Mem := (qSlot τ).writeW (wa τ) (wv τ + 8)
def q6 : State := { q5 w n τ with mem := qEnd τ }

/-- The word's register is none the steps read for anything else, or write otherwise. -/
structure WReg (w : Reg) : Prop where
  x0 : ¬ Reg.x0 = w
  x3 : ¬ Reg.x3 = w
  x4 : ¬ Reg.x4 = w
  x5 : ¬ Reg.x5 = w
  x6 : ¬ Reg.x6 = w
  x15 : ¬ Reg.x15 = w

/-- Nor is the new word's, but that a pop's has it in `x4`. -/
structure NReg (w n : Reg) : Prop where
  x0 : ¬ Reg.x0 = n
  x3 : ¬ Reg.x3 = n
  x5 : ¬ Reg.x5 = n
  x6 : ¬ Reg.x6 = n
  w : ¬ w = n

variable {w n τ}

section Regs

theorem slot_ea {σ : State} (h3 : σ.gpr .x3 = τ.gpr .x3) (h5 : σ.gpr .x5 = τ.gpr .x5) :
    σ.ea slotWord = wa τ := by
  rw [slotWord, Access.ea_reg_lsl, h3, h5]
  rfl

theorem mask_ea {σ : State} (h15 : σ.gpr .x15 = τ.gpr .x15) (h5 : σ.gpr .x5 = τ.gpr .x5) :
    σ.ea maskEntry = ma τ := by
  rw [maskEntry, Access.ea_reg_lsl, h15, h5]
  rfl

theorem t1_gpr {r : Reg} (h : ¬ r = w) : (t1 w τ).gpr r = τ.gpr r := RegUpd.gpr_setReg_of_ne _ _ h
theorem t2_gpr {r : Reg} (h : ¬ r = w) (h6 : ¬ r = .x6) : (t2 w τ).gpr r = τ.gpr r := by
  rw [t2, RegUpd.gpr_setReg_of_ne _ _ h6, t1_gpr h]
theorem t2_w (hr : WReg w) : (t2 w τ).gpr w = wv τ := by
  rw [t2, RegUpd.gpr_setReg_of_ne _ _ (fun h => hr.x6 h.symm), t1, RegUpd.gpr_setReg_self]
theorem t2_x6 : (t2 w τ).gpr .x6 = mk τ := RegUpd.gpr_setReg_self _ _ _

end Regs

theorem exec_t1 (hW : InRegions (τ.rd ++ τ.wr) (wa τ) 8) :
    exec (.ldr .x w slotWord) τ = some (t1 w τ) := by
  have e : τ.ea slotWord = wa τ := slot_ea rfl rfl
  rw [Access.exec_ldr_x (by rw [e]; exact hW), e]
  rfl

theorem ev_t1 : Instr.events (.ldr .x w slotWord) τ (t1 w τ) = [⟨wa τ, 8, false, (wv τ).toNat⟩] := by
  have e : τ.ea slotWord = wa τ := slot_ea rfl rfl
  simp only [Instr.events, e, Width.bytes, toNat_read8]
  rfl

theorem exec_t2 (hr : WReg w) (hM : InRegions (τ.rd ++ τ.wr) (ma τ) 8) :
    exec (.ldr .x .x6 maskEntry) (t1 w τ) = some (t2 w τ) := by
  have e : (t1 w τ).ea maskEntry = ma τ := mask_ea (t1_gpr hr.x15) (t1_gpr hr.x5)
  rw [Access.exec_ldr_x (by rw [e]; exact hM), e]
  rfl

theorem ev_t2 (hr : WReg w) :
    Instr.events (.ldr .x .x6 maskEntry) (t1 w τ) (t2 w τ) = [⟨ma τ, 8, false, (mk τ).toNat⟩] := by
  have e : (t1 w τ).ea maskEntry = ma τ := mask_ea (t1_gpr hr.x15) (t1_gpr hr.x5)
  simp only [Instr.events, e, Width.bytes, toNat_read8]
  rfl

/-! ## The pop's steps -/

section Pop

theorem exec_p3 (hr : WReg w) : exec (.tst .w w (.reg .x6)) (t2 w τ) = some (p3 w τ) := by
  show some (Flags.tstState (t2 w τ) (((t2 w τ).gpr w).setWidth 32 &&& ((t2 w τ).gpr .x6).setWidth 32)) = _
  rw [t2_w hr, t2_x6]
  rfl

theorem eval_p3_t (ht : ptst τ = 0) : eval .eq (p3 w τ) = some true := by
  show some (ptst τ == 0) = _
  rw [ht]
  rfl

theorem eval_p3_f (ht : ¬ ptst τ = 0) : eval .eq (p3 w τ) = some false := by
  show some (ptst τ == 0) = _
  simpa using ht

theorem p3_gpr (r : Reg) : (p3 w τ).gpr r = (t2 w τ).gpr r := by
  rw [p3, Flags.tstState, RegUpd.gpr_setFlags]

theorem exec_p4 (hr : WReg w) (hX : InRegions (τ.rd ++ τ.wr) (wv τ - 8) 8) :
    exec (.ldr .x .x4 { base := w, off := .imm (-8) }) (p3 w τ) = some (p4 w τ) := by
  have e : (p3 w τ).ea { base := w, off := .imm (-8) } = wv τ - 8 := by
    rw [ea_below, p3_gpr, t2_w hr]
  rw [Access.exec_ldr_x (by rw [e]; exact hX), e]
  rfl

theorem ev_p4 (hr : WReg w) : Instr.events (.ldr .x .x4 { base := w, off := .imm (-8) }) (p3 w τ) (p4 w τ) =
    [⟨wv τ - 8, 8, false, (px τ).toNat⟩] := by
  have e : (p3 w τ).ea { base := w, off := .imm (-8) } = wv τ - 8 := by
    rw [ea_below, p3_gpr, t2_w hr]
  simp only [Instr.events, e, Width.bytes, toNat_read8]
  rfl

theorem p4_w (hr : WReg w) : (p4 w τ).gpr w = wv τ := by
  rw [p4, RegUpd.gpr_setReg_of_ne _ _ (fun h => hr.x4 h.symm), p3_gpr, t2_w hr]

theorem p4_gpr {r : Reg} (h : ¬ r = w) (h6 : ¬ r = .x6) (h4 : ¬ r = .x4) : (p4 w τ).gpr r = τ.gpr r := by
  rw [p4, RegUpd.gpr_setReg_of_ne _ _ h4, p3_gpr, t2_gpr h h6]

theorem exec_p5 (hr : WReg w) : exec (.sub .x w w (.imm 8)) (p4 w τ) = some (p5 w τ) := by
  show some ((p4 w τ).setReg w ((p4 w τ).gpr w - BitVec.ofNat 64 8)) = _
  rw [p4_w hr]
  rfl

theorem p5_x3 (hr : WReg w) : (p5 w τ).gpr .x3 = τ.gpr .x3 := by
  rw [p5, RegUpd.gpr_setReg_of_ne _ _ hr.x3, p4_gpr hr.x3 (by decide) (by decide)]
theorem p5_x5 (hr : WReg w) : (p5 w τ).gpr .x5 = τ.gpr .x5 := by
  rw [p5, RegUpd.gpr_setReg_of_ne _ _ hr.x5, p4_gpr hr.x5 (by decide) (by decide)]
theorem p5_w : (p5 w τ).gpr w = wv τ - 8 := RegUpd.gpr_setReg_self _ _ _

theorem exec_p6 (hr : WReg w) (hC : InRegions τ.wr (wa τ) 8) :
    exec (.str .x w slotWord) (p5 w τ) = some (p6 w τ) := by
  have e : (p5 w τ).ea slotWord = wa τ := slot_ea (p5_x3 hr) (p5_x5 hr)
  rw [Access.exec_str_x (by rw [e]; exact hC), e, p5_w]
  rfl

theorem ev_p6 (hr : WReg w) : Instr.events (.str .x w slotWord) (p5 w τ) (p6 w τ) =
    [⟨wa τ, 8, true, (wv τ - 8).toNat⟩] := by
  have e : (p5 w τ).ea slotWord = wa τ := slot_ea (p5_x3 hr) (p5_x5 hr)
  simp only [Instr.events, e, Width.bytes, toNat_read8]
  rw [show (p6 w τ).mem = τ.mem.writeW (wa τ) (wv τ - 8) from rfl, Mem.readW_writeW_self64]

theorem p6_x4 (hr : WReg w) : (p6 w τ).gpr .x4 = px τ := by
  show (p5 w τ).gpr .x4 = _
  rw [p5, RegUpd.gpr_setReg_of_ne _ _ hr.x4, p4, RegUpd.gpr_setReg_self]

/-- What a pop's steps need of `τ`'s regions: the word and the entry readable; on a hit, the slot
below readable and the word writable. -/
structure PopOk (τ : State) : Prop where
  word : InRegions (τ.rd ++ τ.wr) (wa τ) 8
  mask : InRegions (τ.rd ++ τ.wr) (ma τ) 8
  hit : ¬ ptst τ = 0 → InRegions (τ.rd ++ τ.wr) (wv τ - 8) 8 ∧ InRegions τ.wr (wa τ) 8

/-- A pop with nothing to give. -/
theorem popMiss (hr : WReg w) (h : PopOk τ) (ht : ptst τ = 0) :
    Leaves isa (popSteps w) τ (.exit "slow" (p3 w τ)) [τ, t1 w τ, t2 w τ, p3 w τ]
      [⟨wa τ, 8, false, (wv τ).toNat⟩, ⟨ma τ, 8, false, (mk τ).toNat⟩] 3 :=
  .mem (exec_t1 h.word) (ev_t1 (w := w)) <| .mem (exec_t2 hr h.mask) (ev_t2 hr) <|
    .reg (exec_p3 hr) rfl <| .take (eval_p3_t ht)

/-- A pop that takes the top pointer. -/
theorem popHit (hr : WReg w) (h : PopOk τ) (ht : ¬ ptst τ = 0) :
    Through isa (popSteps w) τ (p6 w τ) [τ, t1 w τ, t2 w τ, p3 w τ, p3 w τ, p4 w τ, p5 w τ]
      [⟨wa τ, 8, false, (wv τ).toNat⟩, ⟨ma τ, 8, false, (mk τ).toNat⟩,
        ⟨wv τ - 8, 8, false, (px τ).toNat⟩, ⟨wa τ, 8, true, (wv τ - 8).toNat⟩] :=
  .mem (exec_t1 h.word) (ev_t1 (w := w)) <| .mem (exec_t2 hr h.mask) (ev_t2 hr) <|
    .reg (exec_p3 hr) rfl <| .skip (eval_p3_f ht) <| .mem (exec_p4 hr (h.hit ht).1) (ev_p4 hr) <| .reg (exec_p5 hr) rfl <|
    .mem (exec_p6 hr (h.hit ht).2) (ev_p6 hr) <| .nil _

end Pop

/-! ## The push's steps -/

section Push

theorem exec_q3 (hr : WReg w) : exec (.add .x n w (.imm 8)) (t2 w τ) = some (q3 w n τ) := by
  show some ((t2 w τ).setReg n ((t2 w τ).gpr w + BitVec.ofNat 64 8)) = _
  rw [t2_w hr]
  rfl

theorem q3_gpr {r : Reg} (h : ¬ r = n) : (q3 w n τ).gpr r = (t2 w τ).gpr r :=
  RegUpd.gpr_setReg_of_ne _ _ h
theorem q3_n : (q3 w n τ).gpr n = wv τ + 8 := RegUpd.gpr_setReg_self _ _ _

theorem exec_q4 (hn : NReg w n) : exec (.tst .w n (.reg .x6)) (q3 w n τ) = some (q4 w n τ) := by
  show some (Flags.tstState (q3 w n τ) (((q3 w n τ).gpr n).setWidth 32 &&& ((q3 w n τ).gpr .x6).setWidth 32)) = _
  rw [q3_n, q3_gpr hn.x6, t2_x6]
  rfl

theorem eval_q4_t (ht : qtst τ = 0) : eval .eq (q4 w n τ) = some true := by
  show some (qtst τ == 0) = _
  rw [ht]
  rfl

theorem eval_q4_f (ht : ¬ qtst τ = 0) : eval .eq (q4 w n τ) = some false := by
  show some (qtst τ == 0) = _
  simpa using ht

theorem q4_gpr (r : Reg) : (q4 w n τ).gpr r = (q3 w n τ).gpr r := by
  rw [q4, Flags.tstState, RegUpd.gpr_setFlags]

theorem q4_w (hr : WReg w) (hn : NReg w n) : (q4 w n τ).gpr w = wv τ := by
  rw [q4_gpr, q3_gpr hn.w, t2_w hr]

theorem q4_x0 (hr : WReg w) (hn : NReg w n) : (q4 w n τ).gpr .x0 = τ.gpr .x0 := by
  rw [q4_gpr, q3_gpr hn.x0, t2_gpr hr.x0 (by decide)]

theorem exec_q5 (hr : WReg w) (hn : NReg w n) (hS : InRegions τ.wr (wv τ) 8) :
    exec (.str .x .x0 { base := w }) (q4 w n τ) = some (q5 w n τ) := by
  have e : (q4 w n τ).ea { base := w } = wv τ := by rw [Access.ea_base, q4_w hr hn]
  rw [Access.exec_str_x (by rw [e]; exact hS), e, q4_x0 hr hn]
  rfl

theorem ev_q5 (hr : WReg w) (hn : NReg w n) :
    Instr.events (.str .x .x0 { base := w }) (q4 w n τ) (q5 w n τ) =
      [⟨wv τ, 8, true, (τ.gpr .x0).toNat⟩] := by
  have e : (q4 w n τ).ea { base := w } = wv τ := by rw [Access.ea_base, q4_w hr hn]
  simp only [Instr.events, e, Width.bytes, toNat_read8]
  rw [show (q5 w n τ).mem = qSlot τ from rfl, qSlot, Mem.readW_writeW_self64]

theorem q5_gpr : (q5 w n τ).gpr = (q4 w n τ).gpr := rfl

theorem q5_slot (hr : WReg w) (hn : NReg w n) : (q5 w n τ).ea slotWord = wa τ :=
  slot_ea (by rw [q5_gpr, q4_gpr, q3_gpr hn.x3, t2_gpr hr.x3 (by decide)])
    (by rw [q5_gpr, q4_gpr, q3_gpr hn.x5, t2_gpr hr.x5 (by decide)])

theorem exec_q6 (hr : WReg w) (hn : NReg w n) (hC : InRegions τ.wr (wa τ) 8) :
    exec (.str .x n slotWord) (q5 w n τ) = some (q6 w n τ) := by
  rw [Access.exec_str_x (by rw [q5_slot hr hn]; exact hC), q5_slot hr hn, q5_gpr, q4_gpr, q3_n]
  rfl

theorem ev_q6 (hr : WReg w) (hn : NReg w n) : Instr.events (.str .x n slotWord) (q5 w n τ) (q6 w n τ) =
    [⟨wa τ, 8, true, (wv τ + 8).toNat⟩] := by
  simp only [Instr.events, q5_slot hr hn, Width.bytes, toNat_read8]
  rw [show (q6 w n τ).mem = qEnd τ from rfl, qEnd, Mem.readW_writeW_self64]

/-- What a push's steps need of `τ`'s regions: the word and the entry readable; on a hit, the free
slot and the word writable. -/
structure PushOk (τ : State) : Prop where
  word : InRegions (τ.rd ++ τ.wr) (wa τ) 8
  mask : InRegions (τ.rd ++ τ.wr) (ma τ) 8
  hit : ¬ qtst τ = 0 → InRegions τ.wr (wv τ) 8 ∧ InRegions τ.wr (wa τ) 8

/-- A push with no free slot to spare. -/
theorem pushMiss (hr : WReg w) (hn : NReg w n) (h : PushOk τ) (ht : qtst τ = 0) :
    Leaves isa (pushSteps w n) τ (.exit "slow" (q4 w n τ)) [τ, t1 w τ, t2 w τ, q3 w n τ, q4 w n τ]
      [⟨wa τ, 8, false, (wv τ).toNat⟩, ⟨ma τ, 8, false, (mk τ).toNat⟩] 4 :=
  .mem (exec_t1 h.word) (ev_t1 (w := w)) <| .mem (exec_t2 hr h.mask) (ev_t2 hr) <|
    .reg (exec_q3 hr) rfl <| .reg (exec_q4 hn) rfl <| .take (eval_q4_t ht)

/-- A push that stores its pointer in the free slot and commits. -/
theorem pushHit (hr : WReg w) (hn : NReg w n) (h : PushOk τ) (ht : ¬ qtst τ = 0) :
    Through isa (pushSteps w n) τ (q6 w n τ)
      [τ, t1 w τ, t2 w τ, q3 w n τ, q4 w n τ, q4 w n τ, q5 w n τ]
      [⟨wa τ, 8, false, (wv τ).toNat⟩, ⟨ma τ, 8, false, (mk τ).toNat⟩,
        ⟨wv τ, 8, true, (τ.gpr .x0).toNat⟩, ⟨wa τ, 8, true, (wv τ + 8).toNat⟩] :=
  .mem (exec_t1 h.word) (ev_t1 (w := w)) <| .mem (exec_t2 hr h.mask) (ev_t2 hr) <|
    .reg (exec_q3 hr) rfl <| .reg (exec_q4 hn) rfl <| .skip (eval_q4_f ht) <|
    .mem (exec_q5 hr hn (h.hit ht).1) (ev_q5 hr hn) <| .mem (exec_q6 hr hn (h.hit ht).2) (ev_q6 hr hn) <|
    .nil _

end Push

/-! ## What each state keeps -/

section Mids

variable {rs : List Reg} {s : State} {m : Mem}

theorem mid_t1 (hτ : Mid rs s m τ) (hw : w ∈ rs) : Mid rs s m (t1 w τ) := hτ.setReg hw _
theorem mid_t2 (hτ : Mid rs s m τ) (hw : w ∈ rs) (h6 : .x6 ∈ rs) : Mid rs s m (t2 w τ) :=
  (mid_t1 hτ hw).setReg h6 _
theorem mid_p3 (hτ : Mid rs s m τ) (hw : w ∈ rs) (h6 : .x6 ∈ rs) : Mid rs s m (p3 w τ) :=
  (mid_t2 hτ hw h6).setFlags _ _ _ _
theorem mid_p4 (hτ : Mid rs s m τ) (hw : w ∈ rs) (h6 : .x6 ∈ rs) (h4 : .x4 ∈ rs) :
    Mid rs s m (p4 w τ) := (mid_p3 hτ hw h6).setReg h4 _
theorem mid_p5 (hτ : Mid rs s m τ) (hw : w ∈ rs) (h6 : .x6 ∈ rs) (h4 : .x4 ∈ rs) :
    Mid rs s m (p5 w τ) := (mid_p4 hτ hw h6 h4).setReg hw _
theorem mid_p6 (hτ : Mid rs s m τ) (hw : w ∈ rs) (h6 : .x6 ∈ rs) (h4 : .x4 ∈ rs) :
    Mid rs s (τ.mem.writeW (wa τ) (wv τ - 8)) (p6 w τ) := (mid_p5 hτ hw h6 h4).withMem _
theorem mid_q3 (hτ : Mid rs s m τ) (hw : w ∈ rs) (h6 : .x6 ∈ rs) (hn : n ∈ rs) : Mid rs s m (q3 w n τ) :=
  (mid_t2 hτ hw h6).setReg hn _
theorem mid_q4 (hτ : Mid rs s m τ) (hw : w ∈ rs) (h6 : .x6 ∈ rs) (hn : n ∈ rs) : Mid rs s m (q4 w n τ) :=
  (mid_q3 hτ hw h6 hn).setFlags _ _ _ _
theorem mid_q5 (hτ : Mid rs s m τ) (hw : w ∈ rs) (h6 : .x6 ∈ rs) (hn : n ∈ rs) :
    Mid rs s (qSlot τ) (q5 w n τ) := (mid_q4 hτ hw h6 hn).withMem _
theorem mid_q6 (hτ : Mid rs s m τ) (hw : w ∈ rs) (h6 : .x6 ∈ rs) (hn : n ∈ rs) :
    Mid rs s (qEnd τ) (q6 w n τ) := (mid_q5 hτ hw h6 hn).withMem _

end Mids

end Tail

/-! ## The state the front half reaches -/

/-- A state the front half of a routine reaches, from the state `s` an attempt starts in, having
found the class `k`: memory as after the arm, the state's regions, `x3` the CPU's slab, `x5` the
class and `x15` the masks' table. -/
structure Ready (s : State) (k : Nat) (τ : State) : Prop where
  mem : τ.mem = m1 s
  rd : τ.rd = s.rd
  wr : τ.wr = s.wr
  x3 : τ.gpr .x3 = s.syms SLABS + sh s
  x5 : τ.gpr .x5 = BitVec.ofNat 64 k
  x15 : τ.gpr .x15 = s.syms SLABS + BitVec.ofNat 64 MASK_DISP

/-- A class byte, as a load zero-extends it. -/
theorem byte_ofNat (b : Byte) : b.setWidth 64 = BitVec.ofNat 64 b.toNat := by
  apply BitVec.eq_of_toNat_eq
  have := b.isLt
  simp only [BitVec.toNat_setWidth, BitVec.toNat_ofNat]

theorem ofNat_shl3 (k : Nat) : (BitVec.ofNat 64 k) <<< 3 = BitVec.ofNat 64 (8 * k) := by
  apply BitVec.eq_of_toNat_eq
  simp only [BitVec.toNat_shiftLeft, BitVec.toNat_ofNat, Nat.shiftLeft_eq]
  omega

/-- The low half of a word is the 32-bit word there, little-endian. -/
theorem readW_low (m : Mem) (a : Addr) : (m.readW a 64).setWidth 32 = m.readW a 32 := by
  have h : m.read a 4 = (m.read a 8).setWidth (8 * 4) := by
    refine Mem.read_eq_of_bytes fun i hi => ?_
    rw [← Mem.extractLsb'_read m a (n := 8) (j := i) (by omega)]
    ext j hj
    have hlt : 8 * i + j < 8 * 4 := by omega
    simp [BitVec.getElem_extractLsb', hlt]
  show ((m.read a 8).setWidth 64).setWidth 32 = (m.read a 4).setWidth 32
  rw [h, BitVec.setWidth_setWidth_of_le _ (by decide), BitVec.setWidth_setWidth_of_le _ (by decide)]

section Ready

variable {regs : List Reg} {c : Spec.CEntry.Ctx isa} {s τ : State} {k : Nat}

theorem wa_eq (hA : A64 c.A regs) (h : Ready s k τ) : Tail.wa τ = c.word s k := by
  rw [Tail.wa, h.x3, h.x5, ofNat_shl3, Spec.CEntry.Ctx.word, wordAddr, sym_c hA, cpu_c hA, sh_eq,
    BitVec.ofNat_add, BitVec.add_assoc]

theorem ma_eq (hA : A64 c.A regs) (h : Ready s k τ) :
    Tail.ma τ = c.sym s SLABS + BitVec.ofNat 64 (8 * k + MASK_DISP) := by
  rw [Tail.ma, h.x15, h.x5, ofNat_shl3, sym_c hA, BitVec.add_assoc, ← BitVec.ofNat_add, Nat.add_comm]

end Ready

/-! ## What the precondition gives the back half -/

/-- What the back half of a routine that found the class `k` on a CPU with a slab needs, from the
precondition. -/
structure BackFacts (c : Spec.CEntry.Ctx isa) (s : State) (k : Nat) : Prop where
  hk : k < MASK_COUNT
  word : (⟨c.word s k, 8⟩ : Region) ∈ s.wr
  mask : InRegions (s.rd ++ s.wr) (c.sym s SLABS + BitVec.ofNat 64 (8 * k + MASK_DISP)) 8
  cs_word : Region.Disjoint ⟨area s + 8, 8⟩ ⟨c.word s k, 8⟩
  cs_mask : Region.Disjoint ⟨area s + 8, 8⟩ ⟨c.sym s SLABS + BitVec.ofNat 64 (8 * k + MASK_DISP), 8⟩
  mag : IsSlot (c.slotsOf' s k) (c.w s k) →
    c.magRegion s k ∈ s.wr ∧ Region.Disjoint ⟨area s + 8, 8⟩ (c.magRegion s k) ∧
    Region.Disjoint ⟨c.word s k, 8⟩ (c.magRegion s k) ∧
    Region.Disjoint ⟨s.syms RS, 8⟩ (c.magRegion s k) ∧ c.Apart s (c.magRegion s k)
  pre : c.pre s

theorem backFacts {regs : List Reg} {c : Spec.CEntry.Ctx isa} (hA : A64 c.A regs) {s : State}
    (hw : c.World s) (hp : c.pre s) {k : Nat} (hk : c.cls? s = some k) (hi : c.InRange s) :
    BackFacts c s k := by
  have hkl := cls_lt hk
  have hmore : c.more s = (⟨c.word s k, 8⟩, true) ::
      (if IsSlot (c.slotsOf' s k) (c.w s k) then [(c.magRegion s k, true)] else []) := by
    simp only [Spec.CEntry.Ctx.more, hi, ↓reduceIte, hk]
  have hwm : (⟨c.word s k, 8⟩, true) ∈ c.more s := by rw [hmore]; exact List.mem_cons_self ..
  have hcs : (c.csField s, true) ∈ c.fixed s := cs_mem_fixed
  have hmt : (c.maskTable s, false) ∈ c.fixed s := maskTable_mem_fixed
  have hmc := maskAt_contains (c := c) (s := s) hkl
  have hcsT : (c.csField s).Disjoint (c.maskTable s) :=
    cs_table (c := c) (s := s) hp.2.2.1 (x := (c.maskTable s, false)) (List.mem_cons_self ..)
  have e1 : (c.csField s).Disjoint ⟨c.word s k, 8⟩ :=
    disj_fixed_more hw (a := (c.csField s, true)) (b := (⟨c.word s k, 8⟩, true)) hcs hwm rfl
  have e5 : (⟨c.word s k, 8⟩ : Region) ∈ s.wr :=
    mem_wr hA hw (x := (⟨c.word s k, 8⟩, true)) (mem_more hwm) rfl
  have e6 : c.maskTable s ∈ s.rd := mem_rd hA hw (x := (c.maskTable s, false)) (mem_fixed hmt) rfl
  rw [csField_c hA] at hcsT e1
  refine ⟨hkl, e5, (InRegions.of_mem e6 hmc).append_left, e1,
    hcsT.sub_right (sub_of_contains hmc), ?_, hp⟩
  intro hs
  have hmm : (c.magRegion s k, true) ∈ c.more s := by rw [hmore]; simp [hs]
  have hpw := (List.pairwise_append.mp hw.disj).2.1
  rw [hmore] at hpw
  simp only [hs, ↓reduceIte] at hpw
  have hwmag : Region.Disjoint ⟨c.word s k, 8⟩ (c.magRegion s k) :=
    (List.pairwise_cons.mp hpw).1 (c.magRegion s k, true) (List.mem_singleton_self _) rfl
  have hrsv : (c.rsVar s, false) ∈ c.fixed s := rsVar_mem_fixed
  have f1 : (c.csField s).Disjoint (c.magRegion s k) :=
    disj_fixed_more hw (a := (c.csField s, true)) (b := (c.magRegion s k, true)) hcs hmm rfl
  have f2 : (c.rsVar s).Disjoint (c.magRegion s k) :=
    disj_fixed_more hw (a := (c.rsVar s, false)) (b := (c.magRegion s k, true)) hrsv hmm rfl
  have f4 : c.magRegion s k ∈ s.wr := mem_wr hA hw (x := (c.magRegion s k, true)) (mem_more hmm) rfl
  rw [csField_c hA] at f1
  rw [rsVar_c hA] at f2
  exact ⟨f4, f1, hwmag, f2, mag_apart hp.2.2.2 hkl hi hs⟩

/-! ## The values the back half reads -/

section Values

variable {regs : List Reg} {c : Spec.CEntry.Ctx isa} (hA : A64 c.A regs) {s τ : State} {k : Nat}
  (hr : Ready s k τ) (hb : BackFacts c s k)
include hA hr hb

theorem wv_eq : Tail.wv τ = c.w s k := by
  rw [Tail.wv, wa_eq hA hr, hr.mem, m1, readW64_writeW_disj hb.cs_word.symm,
    Spec.CEntry.Ctx.w, mem_c hA]

theorem mk_eq : (Tail.mk τ).setWidth 32 = BitVec.ofNat 32 (c.maskOf s k) := by
  have h4 : Region.Disjoint ⟨c.sym s SLABS + BitVec.ofNat 64 (8 * k + MASK_DISP), 8⟩ ⟨area s + 8, 8⟩ :=
    hb.cs_mask.symm
  rw [Tail.mk, ma_eq hA hr, hr.mem, m1, readW64_writeW_disj h4, readW_low, Spec.CEntry.Ctx.maskOf,
    mem_c hA, BitVec.ofNat_toNat, BitVec.setWidth_eq]

end Values

/-- What a nonzero mask makes of the class's magazines: `2 ^ j` slots. -/
structure Sized (c : Spec.CEntry.Ctx isa) (s : State) (k j : Nat) : Prop where
  hj : 2 ≤ j
  hj' : j ≤ 16
  mask : c.maskOf s k = magMask (2 ^ j)
  slots : c.slotsOf' s k = 2 ^ j
  ok : c.InRange s → WordOk (2 ^ j) (c.w s k)

theorem sized {c : Spec.CEntry.Ctx isa} {s : State} {k : Nat} (hb : BackFacts c s k)
    (hz : c.maskOf s k ≠ 0) : ∃ j, Sized c s k j := by
  obtain ⟨hm, hinv, -⟩ := hb.pre.2.2.2 k hb.hk
  obtain ⟨j, hj, hj', hmj⟩ := hm.resolve_left hz
  have hsl : c.slotsOf' s k = 2 ^ j := by
    rw [Spec.CEntry.Ctx.slotsOf', hmj]; exact slotsOf_magMask j hj
  exact ⟨j, hj, hj', hmj, hsl, fun hi => by rw [← hsl]; exact (hinv hz (c.cpu s) hi).1⟩

theorem slots_zero {c : Spec.CEntry.Ctx isa} {s : State} {k : Nat} (hz : c.maskOf s k = 0) :
    c.slotsOf' s k = 1 := by
  rw [Spec.CEntry.Ctx.slotsOf', hz]; rfl

/-! ## The pop -/

section Pop

variable {regs : List Reg} {c : Spec.CEntry.Ctx isa} (hA : A64 c.A regs) {s τ : State} {k : Nat}
  (hr : Ready s k τ) (hb : BackFacts c s k)
include hA hr hb

theorem ptst_eq : Tail.ptst τ = 0 ↔
    (c.w s k).setWidth 32 &&& BitVec.ofNat 32 (c.maskOf s k) = 0 := by
  rw [Tail.ptst, wv_eq hA hr hb, mk_eq hA hr hb]

/-- A pop's test that passes was of a nonzero mask. -/
theorem sized_of_ptest (ht : ¬ Tail.ptst τ = 0) : ∃ j, Sized c s k j := by
  refine sized hb fun hz => ht ((ptst_eq hA hr hb).mpr ?_)
  rw [hz]; exact BitVec.and_zero

theorem pop_hit {j : Nat} (hj : Sized c s k j) (hi : c.InRange s) (ht : ¬ Tail.ptst τ = 0) :
    IsSlot (2 ^ j) (c.w s k) ∧ 2 ≤ slot (2 ^ j) (c.w s k) ∧
      (Mag.ofWord (2 ^ j) s.mem (c.w s k)).pop =
        some (s.mem.readW (c.w s k - 8) 64,
          .stack (magBase (2 ^ j) (c.w s k)) (items (2 ^ j) s.mem (c.w s k)).dropLast) := by
  have hm : ¬ (c.w s k).toNat % (8 * 2 ^ j) < 16 := fun h' => ht ((ptst_eq hA hr hb).mpr
    ((test_iff32 hj.hj hj.hj' (c.w s k) _ (by rw [BitVec.toNat_ofNat, hj.mask,
      Nat.mod_eq_of_lt (magMask_lt j hj.hj')])).mpr h'))
  exact pop_of_test (dvd_marker hj.hj') (hj.ok hi) hm

theorem popOk (hi : c.InRange s) : Tail.PopOk τ where
  word := by rw [hr.rd, hr.wr, wa_eq hA hr]; exact inRdWr (r := ⟨c.word s k, 8⟩) hb.word
  mask := by rw [hr.rd, hr.wr, ma_eq hA hr]; exact hb.mask
  hit ht := by
    obtain ⟨j, hj⟩ := sized_of_ptest hA hr hb ht
    obtain ⟨hs, h2, -⟩ := pop_hit hA hr hb hj hi ht
    have hsl : IsSlot (c.slotsOf' s k) (c.w s k) := by rw [hj.slots]; exact hs
    obtain ⟨hmag, -⟩ := hb.mag hsl
    have hc := contains_below hs h2 (slot_lt _ _ (Nat.two_pow_pos j))
    rw [Spec.CEntry.Ctx.magRegion, hj.slots] at hmag
    refine ⟨?_, ?_⟩
    · rw [hr.rd, hr.wr, wv_eq hA hr hb]; exact (InRegions.of_mem hmag hc).append_right
    · rw [hr.wr, wa_eq hA hr]; exact inWr (r := ⟨c.word s k, 8⟩) hb.word

theorem px_eq {j : Nat} (hj : Sized c s k j) (hi : c.InRange s) (ht : ¬ Tail.ptst τ = 0) :
    Tail.px τ = s.mem.readW (c.w s k - 8) 64 := by
  obtain ⟨hs, h2, -⟩ := pop_hit hA hr hb hj hi ht
  have hsl : IsSlot (c.slotsOf' s k) (c.w s k) := by rw [hj.slots]; exact hs
  obtain ⟨-, hcm, -⟩ := hb.mag hsl
  rw [Spec.CEntry.Ctx.magRegion, hj.slots] at hcm
  have hc := contains_below hs h2 (slot_lt _ _ (Nat.two_pow_pos j))
  rw [Tail.px, wv_eq hA hr hb, hr.mem, m1, readW64_writeW_disj (hcm.symm.sub_left (sub_of_contains hc))]

/-- A pop that passes its test took the top pointer, whatever registers the routine then sets. -/
theorem popped (hk : c.cls? s = some k) (hi : c.InRange s) (ht : ¬ Tail.ptst τ = 0) {s' : State}
    (hm : s'.mem = τ.mem.writeW (Tail.wa τ) (Tail.wv τ - 8)) : c.Popped s s' (Tail.px τ) := by
  obtain ⟨j, hj⟩ := sized_of_ptest hA hr hb ht
  obtain ⟨hs, h2, hpop⟩ := pop_hit hA hr hb hj hi ht
  have hsl : IsSlot (c.slotsOf' s k) (c.w s k) := by rw [hj.slots]; exact hs
  obtain ⟨-, hcm, hwm, -⟩ := hb.mag hsl
  rw [Spec.CEntry.Ctx.magRegion, hj.slots] at hcm hwm
  refine ⟨k, .stack (magBase (2 ^ j) (c.w s k)) (items (2 ^ j) s.mem (c.w s k)).dropLast, ?_, ?_, ?_, ?_⟩
  · simp only [Spec.CEntry.Ctx.popResult, hi, ↓reduceIte, hk, Option.bind_some, Spec.CEntry.Ctx.mag,
      hj.slots, mem_c hA, hpop, Option.map_some, px_eq hA hr hb hj hi ht]
  · rw [csField_c hA, mem_c hA, ← wa_eq hA hr, hA.mem, hm, hr.mem]
    exact frame_two s.mem _ _ _ _
  · rw [hA.mem, hm, wa_eq hA hr, Mem.readW_writeW_self64, wv_eq hA hr hb]
  · rw [hA.mem, hm, hj.slots]
    refine ofWord_sub8 (Nat.two_pow_pos j) hs h2 fun i hi' => ?_
    have hsub := sub_item (n := 2 ^ j) (w := c.w s k) (i := i)
      (by have := slot_lt (2 ^ j) (c.w s k) (Nat.two_pow_pos j); omega)
    rw [wa_eq hA hr, readW64_writeW_disj (hwm.symm.sub_left hsub), hr.mem, m1,
      readW64_writeW_disj (hcm.symm.sub_left hsub)]

/-- A pop whose test fails takes nothing. -/
theorem pop_missed (hk : c.cls? s = some k) (hi : c.InRange s) (ht : Tail.ptst τ = 0) :
    c.popResult s = none := by
  simp only [Spec.CEntry.Ctx.popResult, hi, ↓reduceIte, hk, Option.bind_some, Option.map_eq_none_iff,
    Spec.CEntry.Ctx.mag]
  by_cases hz : c.maskOf s k = 0
  · rw [slots_zero hz]; exact pop_one _ _
  · obtain ⟨j, hj⟩ := sized hb hz
    rw [hj.slots]
    refine (pop_eq_none_iff (dvd_marker hj.hj') (hj.ok hi)).mpr
      ((test_iff32 hj.hj hj.hj' (c.w s k) _ (by rw [BitVec.toNat_ofNat, hj.mask,
        Nat.mod_eq_of_lt (magMask_lt j hj.hj')])).mp ((ptst_eq hA hr hb).mp ht))

end Pop

/-! ## The push -/

section Push

variable {regs : List Reg} {c : Spec.CEntry.Ctx isa} (hA : A64 c.A regs) {s τ : State} {k : Nat}
  (hr : Ready s k τ) (hb : BackFacts c s k)
include hA hr hb

theorem qtst_eq : Tail.qtst τ = 0 ↔
    (c.w s k + 8).setWidth 32 &&& BitVec.ofNat 32 (c.maskOf s k) = 0 := by
  rw [Tail.qtst, wv_eq hA hr hb, mk_eq hA hr hb]

theorem sized_of_qtest (ht : ¬ Tail.qtst τ = 0) : ∃ j, Sized c s k j := by
  refine sized hb fun hz => ht ((qtst_eq hA hr hb).mpr ?_)
  rw [hz]; exact BitVec.and_zero

theorem push_hit {j : Nat} (hj : Sized c s k j) (hi : c.InRange s) (ht : ¬ Tail.qtst τ = 0)
    (x : BitVec 64) :
    IsSlot (2 ^ j) (c.w s k) ∧ slot (2 ^ j) (c.w s k) + 1 < 2 ^ j ∧
      (Mag.ofWord (2 ^ j) s.mem (c.w s k)).push (2 ^ j) x =
        some (.stack (magBase (2 ^ j) (c.w s k)) (items (2 ^ j) s.mem (c.w s k) ++ [x])) := by
  have hm : ¬ (c.w s k + 8).toNat % (8 * 2 ^ j) < 16 := fun h' => ht ((qtst_eq hA hr hb).mpr
    ((test_iff32 hj.hj hj.hj' (c.w s k + 8) _ (by rw [BitVec.toNat_ofNat, hj.mask,
      Nat.mod_eq_of_lt (magMask_lt j hj.hj')])).mpr h'))
  exact push_of_test j rfl hj.hj hj.hj' (hj.ok hi) s.mem x hm

/-- The free slot: in the magazine, apart from `rseq_cs`, the slab word and `__rseq_offset`. -/
structure SlotFacts (c : Spec.CEntry.Ctx isa) (s : State) (k j : Nat) : Prop where
  inMag : (⟨magBase (2 ^ j) (c.w s k), 8 * 2 ^ j⟩ : Region).Contains (c.w s k) 8
  mem : (⟨magBase (2 ^ j) (c.w s k), 8 * 2 ^ j⟩ : Region) ∈ s.wr
  cs : Region.Disjoint ⟨c.w s k, 8⟩ ⟨area s + 8, 8⟩
  word : Region.Disjoint ⟨c.w s k, 8⟩ ⟨c.word s k, 8⟩
  rsVar : Region.Disjoint ⟨s.syms RS, 8⟩ ⟨c.w s k, 8⟩
  apart : c.Apart s ⟨magBase (2 ^ j) (c.w s k), 8 * 2 ^ j⟩

theorem slotFacts {j : Nat} (hj : Sized c s k j) (hi : c.InRange s) (ht : ¬ Tail.qtst τ = 0) :
    SlotFacts c s k j := by
  obtain ⟨hs, hlt, -⟩ := push_hit hA hr hb hj hi ht 0
  have hsl : IsSlot (c.slotsOf' s k) (c.w s k) := by rw [hj.slots]; exact hs
  obtain ⟨hmag, hcm, hwm, hrm, hap⟩ := hb.mag hsl
  simp only [Spec.CEntry.Ctx.magRegion, hj.slots] at hmag hcm hwm hrm hap
  have hc := contains_at hs hlt
  have hsub := sub_of_contains hc
  exact ⟨hc, hmag, hcm.symm.sub_left hsub, hwm.symm.sub_left hsub, hrm.sub_right hsub, hap⟩

theorem pushOk (hi : c.InRange s) : Tail.PushOk τ where
  word := by rw [hr.rd, hr.wr, wa_eq hA hr]; exact inRdWr (r := ⟨c.word s k, 8⟩) hb.word
  mask := by rw [hr.rd, hr.wr, ma_eq hA hr]; exact hb.mask
  hit ht := by
    obtain ⟨j, hj⟩ := sized_of_qtest hA hr hb ht
    have hf := slotFacts hA hr hb hj hi ht
    refine ⟨?_, ?_⟩
    · rw [hr.wr, wv_eq hA hr hb]; exact InRegions.of_mem hf.mem hf.inMag
    · rw [hr.wr, wa_eq hA hr]; exact inWr (r := ⟨c.word s k, 8⟩) hb.word

omit hA hr in
/-- Item `i` of the magazine is apart from `rseq_cs`, the slab word and the free slot. -/
theorem item_disjoint {j : Nat} (hj : Sized c s k j)
    (hs : IsSlot (2 ^ j) (c.w s k)) {i : Nat} (hil : i < slot (2 ^ j) (c.w s k) - 1) :
    Region.Disjoint ⟨magBase (2 ^ j) (c.w s k) + BitVec.ofNat 64 (8 * (i + 1)), 8⟩ ⟨area s + 8, 8⟩ ∧
    Region.Disjoint ⟨magBase (2 ^ j) (c.w s k) + BitVec.ofNat 64 (8 * (i + 1)), 8⟩ ⟨c.word s k, 8⟩ ∧
    Region.Disjoint ⟨magBase (2 ^ j) (c.w s k) + BitVec.ofNat 64 (8 * (i + 1)), 8⟩ ⟨c.w s k, 8⟩ := by
  have hsl : IsSlot (c.slotsOf' s k) (c.w s k) := by rw [hj.slots]; exact hs
  have hlt := slot_lt (2 ^ j) (c.w s k) (Nat.two_pow_pos j)
  have hsub := sub_item (n := 2 ^ j) (w := c.w s k) (i := i) (by omega)
  obtain ⟨-, hcm, hwm, -⟩ := hb.mag hsl
  simp only [Spec.CEntry.Ctx.magRegion, hj.slots] at hcm hwm
  have hn : 8 * 2 ^ j ≤ 2 ^ 19 := by
    have := Nat.pow_le_pow_right (n := 2) (by decide) hj.hj'
    omega
  refine ⟨hcm.symm.sub_left hsub, hwm.symm.sub_left hsub, ?_⟩
  have hw := magBase_add_slot hs
  have := Offset.disjoint (magBase (2 ^ j) (c.w s k)) (d := 8 * (i + 1)) (n := 8)
    (e := 8 * slot (2 ^ j) (c.w s k)) (k := 8) (by omega) (by omega) (by omega)
  rwa [hw] at this

/-- A push that passes its test pushed the pointer. -/
theorem pushed {w n : Reg} (hk : c.cls? s = some k) (hi : c.InRange s) (ht : ¬ Tail.qtst τ = 0) :
    c.Pushed s (τ.gpr .x0) (Tail.q6 w n τ) := by
  obtain ⟨j, hj⟩ := sized_of_qtest hA hr hb ht
  obtain ⟨hs, hlt, hpush⟩ := push_hit hA hr hb hj hi ht (τ.gpr .x0)
  have hf := slotFacts hA hr hb hj hi ht
  refine ⟨k, .stack (magBase (2 ^ j) (c.w s k)) (items (2 ^ j) s.mem (c.w s k) ++ [τ.gpr .x0]),
    ?_, ?_, ?_, ?_⟩
  · simp only [Spec.CEntry.Ctx.pushResult, hi, ↓reduceIte, hk, Option.bind_some, Spec.CEntry.Ctx.mag,
      hj.slots, mem_c hA, hpush, Option.map_some]
  · rw [csField_c hA, mem_c hA, ← wa_eq hA hr, ← wv_eq hA hr hb, hA.mem,
      show (Tail.q6 w n τ).mem = Tail.qEnd τ from rfl, Tail.qEnd, Tail.qSlot, hr.mem]
    exact (frame_three s.mem _ _ _ _ _ _).mono (by simp)
  · rw [hA.mem, show (Tail.q6 w n τ).mem = Tail.qEnd τ from rfl, Tail.qEnd, wa_eq hA hr,
      Mem.readW_writeW_self64, wv_eq hA hr hb]
  · rw [hA.mem, show (Tail.q6 w n τ).mem = Tail.qEnd τ from rfl, hj.slots]
    refine ofWord_add8 j hj.hj' rfl hs hlt _ ?_ fun i hil => ?_
    · rw [Tail.qEnd, wa_eq hA hr, readW64_writeW_disj hf.word, Tail.qSlot, wv_eq hA hr hb,
        Mem.readW_writeW_self64]
    · obtain ⟨h1, h2, h3⟩ := item_disjoint hb hj hs hil
      rw [Tail.qEnd, wa_eq hA hr, readW64_writeW_disj h2, Tail.qSlot, wv_eq hA hr hb,
        readW64_writeW_disj h3, hr.mem, m1, readW64_writeW_disj h1]

theorem push_missed (hk : c.cls? s = some k) (hi : c.InRange s) (ht : Tail.qtst τ = 0)
    (x : BitVec 64) : c.pushResult s x = none := by
  simp only [Spec.CEntry.Ctx.pushResult, hi, ↓reduceIte, hk, Option.bind_some, Option.map_eq_none_iff,
    Spec.CEntry.Ctx.mag]
  by_cases hz : c.maskOf s k = 0
  · rw [slots_zero hz]; exact push_one _ _ _
  · obtain ⟨j, hj⟩ := sized hb hz
    rw [hj.slots]
    exact (push_eq_none_iff _ j rfl hj.hj hj.hj' (hj.ok hi)).mpr
      ((test_iff32 hj.hj hj.hj' (c.w s k + 8) _ (by rw [BitVec.toNat_ofNat, hj.mask,
        Nat.mod_eq_of_lt (magMask_lt j hj.hj')])).mp ((qtst_eq hA hr hb).mp ht))

/-- After the free slot is written, every slab's magazine of the class is as on entry: the free
slot is above the CPU's slab's pointers, and in no other slab's magazine (`Exclusive`). -/
theorem mags_slot (hi : c.InRange s) (ht : ¬ Tail.qtst τ = 0) :
    ∀ i < SLAB_COUNT, Mag.ofWord (c.slotsOf' s k) (Tail.qSlot τ) (c.wordOf s k i) =
      Mag.ofWord (c.slotsOf' s k) (c.mem s) (c.wordOf s k i) := by
  obtain ⟨j, hj⟩ := sized_of_qtest hA hr hb ht
  have hf := slotFacts hA hr hb hj hi ht
  obtain ⟨hs0, -, -⟩ := push_hit hA hr hb hj hi ht 0
  have hz : c.maskOf s k ≠ 0 := by rw [hj.mask]; have := four_le hj.hj; simp only [magMask]; omega
  have hfr : Frame [⟨area s + 8, 8⟩, ⟨c.w s k, 8⟩] s.mem (Tail.qSlot τ) := by
    rw [Tail.qSlot, hr.mem, wv_eq hA hr hb]; exact frame_two _ _ _ _ _
  intro i hi'
  by_cases hc : i = c.cpu s
  · subst hc
    show Mag.ofWord _ _ (c.w s k) = Mag.ofWord _ _ (c.w s k)
    rw [hj.slots, mem_c hA]
    exact ofWord_congr fun i hil => by
      obtain ⟨h1, -, h3⟩ := item_disjoint hb hj hs0 hil
      rw [Tail.qSlot, wv_eq hA hr hb, readW64_writeW_disj h3, hr.mem, m1, readW64_writeW_disj h1]
  have hok := (hb.pre.2.2.2 k hb.hk).2.1 hz i hi'
  rw [mem_c hA]
  refine ofWord_frame (by rw [hj.slots]; exact Nat.two_pow_pos j) hfr hok.1 fun hs r hr' => ?_
  rcases List.mem_cons.mp hr' with rfl | hr'
  · rw [← csField_c hA]; exact (hok.2 hs).2.2.1 (c.csField s, true) cs_mem_fixed
  · rw [List.mem_singleton.mp hr']
    have hsc : IsSlot (c.slotsOf' s k) (c.wordOf s k (c.cpu s)) := by
      show IsSlot _ (c.w s k); rw [hj.slots]; exact hs0
    have hex := (hb.pre.2.2.2 k hb.hk).2.2 i hi' (c.cpu s) hi hc hs hsc
    rw [show c.wordOf s k (c.cpu s) = c.w s k from rfl, hj.slots] at hex
    rw [hj.slots]
    exact hex.sub_right (sub_of_contains hf.inMag)

end Push

/-! ## Where a push may be aborted -/

section PushAbort

variable {regs : List Reg} {c : Spec.CEntry.Ctx isa} (hA : A64 c.A regs) {s : State}
include hA

/-- Before the free slot is written, every slab's magazine of a class is as on entry. -/
theorem mags_m1 {k : Nat} (hb : BackFacts c s k) : ∀ i < SLAB_COUNT,
    Mag.ofWord (c.slotsOf' s k) (m1 s) (c.wordOf s k i) = Mag.ofWord (c.slotsOf' s k) (c.mem s) (c.wordOf s k i) := by
  intro i hi
  by_cases hz : c.maskOf s k = 0
  · rw [slots_zero hz]; exact ofWord_one_congr _ _ _
  obtain ⟨j, hj⟩ := sized hb hz
  have hok := (hb.pre.2.2.2 k hb.hk).2.1 hz i hi
  rw [mem_c hA]
  refine ofWord_frame (by rw [hj.slots]; exact Nat.two_pow_pos j) frame_m1 hok.1 fun hs r hr => ?_
  rw [List.mem_singleton.mp hr, ← csField_c hA]
  exact (hok.2 hs).2.2.1 (c.csField s, true) cs_mem_fixed

/-- In memory as after the arm, the push has changed nothing a retry or another thread can see. -/
theorem pushAbort_m1 (hw : c.World s) (hs : c.pre s) {p : State} (hm : p.mem = m1 s) :
    c.PushAbort s p := by
  unfold Spec.CEntry.Ctx.PushAbort
  rw [hA.mem, hm]
  by_cases hi : c.InRange s
  · simp only [hi, ↓reduceIte]
    cases hk : c.cls? s with
    | none => rw [csField_c hA, mem_c hA]; exact frame_m1
    | some k =>
      refine ⟨?_, mags_m1 hA (backFacts hA hw hs hk hi)⟩
      rw [csField_c hA, mem_c hA]; exact frame_m1.mono (by simp)
  · simp only [hi, ↓reduceIte]
    rw [csField_c hA, mem_c hA]; exact frame_m1

/-- After the free slot is written, too. -/
theorem pushAbort_slot (hw : c.World s) (hs : c.pre s) {k : Nat} {τ : State} (hr : Ready s k τ)
    (hk : c.cls? s = some k) (hi : c.InRange s) (ht : ¬ Tail.qtst τ = 0) {p : State}
    (hm : p.mem = Tail.qSlot τ) : c.PushAbort s p := by
  have hb := backFacts hA hw hs hk hi
  unfold Spec.CEntry.Ctx.PushAbort
  simp only [hi, ↓reduceIte, hk]
  rw [hA.mem, hm]
  refine ⟨?_, mags_slot hA hr hb hi ht⟩
  rw [csField_c hA, mem_c hA, Tail.qSlot, hr.mem, wv_eq hA hr hb]
  exact frame_two _ _ _ _ _

end PushAbort

end VG.Proof.CEntry.AArch64
