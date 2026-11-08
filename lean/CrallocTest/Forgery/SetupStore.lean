import Cralloc.TCB.Axioms
import Cralloc.Proof.Slab.AArch64.PopInlineWit
import Cralloc.Proof.Framework.Registration
import CrallocTest.Forgery.Refused

/-!
# A store in a setup

`Impl.Slab.AArch64.popInline` against `Spec.Slab.popInlineContract`, with its real `arm`,
`body`, `attempt` and proof, and a setup that does what the real one does and then, with no
branch, reads the slab word of the CPU it is on (`rseq_cs` if the number has no slab) and
stores what it read back. In the model memory has not changed, so `SetupOk` holds; `trace` is
of an attempt, so it does not see it. On the machine nothing protects this thread yet: if it is
preempted between the load and the store, it overwrites whatever another thread or a drainer
wrote meanwhile, and on another CPU.
-/

namespace VG.Test.Forgery.SetupStore

open VG VG.AArch64 VG.Spec.Slab VG.Impl.Slab.AArch64 VG.Proof.Slab.AArch64
open VG.Proof.Slab.AArch64.PopInline

def extra : List Instr := [
  .ldr .w .x8 { base := .x10, off := .imm 4 },
  .lsr .x .x6 .x8 13,
  .sub .x .x6 .x6 (.imm 1),
  .lsr .x .x6 .x6 63,
  .lsl .x .x7 .x8 12,
  .add .x .x7 .x3 (.reg .x7),
  .add .x .x5 .x10 (.imm 8),
  .sub .x .x7 .x7 (.reg .x5),
  .mul .x .x7 .x7 .x6,
  .add .x .x7 .x5 (.reg .x7),
  .ldr .x .x6 { base := .x7 },
  .str .x .x6 { base := .x7 }]

def setupS : List Instr := setup ++ extra

/-- Writing back what a word holds changes nothing. -/
theorem writeW_readW (m : Mem) (a : Addr) : m.writeW a (m.readW a 64) = m := by
  funext y
  show (if (y - a).toNat < 64 / 8 then
      ((m.readW a 64).setWidth (8 * (64 / 8))).extractLsb' (8 * (y - a).toNat) 8 else m y) = m y
  split
  · rename_i h
    have h' : (y - a).toNat < 8 := h
    have hr : ((m.readW a 64).setWidth (8 * (64 / 8))) = m.read a 8 := by
      apply BitVec.eq_of_toNat_eq
      simp only [Mem.readW, BitVec.toNat_setWidth]
      rw [Nat.mod_eq_of_lt (m.read a 8).isLt, Nat.mod_eq_of_lt (m.read a 8).isLt]
    rw [hr, Mem.extractLsb'_read m a h']
    congr 1
    apply BitVec.eq_of_toNat_eq
    simp only [BitVec.toNat_add, BitVec.toNat_ofNat, BitVec.toNat_sub]
    have := a.isLt
    have := y.isLt
    omega
  · rfl

section Values

variable (τ : State)

def cv : BitVec 64 := (cpu32 τ).setWidth 64
def v2 : BitVec 64 := cv τ >>> 13
def v3 : BitVec 64 := v2 τ - BitVec.ofNat 64 1
def bit : BitVec 64 := v3 τ >>> 63
def csA : BitVec 64 := area τ + BitVec.ofNat 64 8
def v8 : BitVec 64 := wa τ - csA τ
def v9 : BitVec 64 := v8 τ * bit τ
/-- `rseq_cs` plus `bit` times the distance to the slab word. -/
def addr : BitVec 64 := csA τ + v9 τ

def t1 : State := τ.setReg .x8 (cv τ)
def t2 : State := (t1 τ).setReg .x6 (v2 τ)
def t3 : State := (t2 τ).setReg .x6 (v3 τ)
def t4 : State := (t3 τ).setReg .x6 (bit τ)
def t5 : State := (t4 τ).setReg .x7 (sh τ)
def t6 : State := (t5 τ).setReg .x7 (wa τ)
def t7 : State := (t6 τ).setReg .x5 (csA τ)
def t8 : State := (t7 τ).setReg .x7 (v8 τ)
def t9 : State := (t8 τ).setReg .x7 (v9 τ)
def t10 : State := (t9 τ).setReg .x7 (addr τ)
def t11 : State := (t10 τ).setReg .x6 (τ.mem.readW (addr τ) 64)

theorem t10_gpr {r : Reg} (h5 : ¬ r = .x5) (h6 : ¬ r = .x6) (h7 : ¬ r = .x7) (h8 : ¬ r = .x8) :
    (t10 τ).gpr r = τ.gpr r := by
  simp only [t10, t9, t8, t7, t6, t5, t4, t3, t2, t1, RegUpd.gpr_setReg_of_ne _ _ h5,
    RegUpd.gpr_setReg_of_ne _ _ h6, RegUpd.gpr_setReg_of_ne _ _ h7, RegUpd.gpr_setReg_of_ne _ _ h8]

theorem t11_gpr {r : Reg} (h5 : ¬ r = .x5) (h6 : ¬ r = .x6) (h7 : ¬ r = .x7) (h8 : ¬ r = .x8) :
    (t11 τ).gpr r = τ.gpr r := by
  rw [t11, RegUpd.gpr_setReg_of_ne _ _ h6, t10_gpr τ h5 h6 h7 h8]

end Values

section Selects

variable {τ : State}

theorem cv_toNat : (cv τ).toNat = (cpu32 τ).toNat := by
  simp only [cv, BitVec.toNat_setWidth]
  exact Nat.mod_eq_of_lt (Nat.lt_of_lt_of_le (cpu32 τ).isLt (by decide))

/-- A number with a slab: the bit is 1, and the address the slab word's. -/
theorem addr_in (h : (cpu32 τ).toNat < 8192) : addr τ = wa τ := by
  have h2 : v2 τ = 0 := by
    apply BitVec.eq_of_toNat_eq
    simp only [v2, BitVec.toNat_ushiftRight, cv_toNat, Nat.shiftRight_eq_div_pow]
    exact Nat.div_eq_of_lt (by omega)
  have hb : bit τ = 1 := by rw [bit, v3, h2]; decide
  have h1 : v8 τ * (1 : BitVec 64) = v8 τ := by simp
  rw [addr, v9, hb, h1, v8, BitVec.add_comm, BitVec.sub_add_cancel]

/-- A number without one: the bit is 0, and the address `rseq_cs`. -/
theorem addr_out (h : ¬ (cpu32 τ).toNat < 8192) : addr τ = csA τ := by
  have hlt := (cpu32 τ).isLt
  have hv2 : (v2 τ).toNat = (cpu32 τ).toNat / 8192 := by
    simp only [v2, BitVec.toNat_ushiftRight, cv_toNat, Nat.shiftRight_eq_div_pow]
  have hb : bit τ = 0 := by
    apply BitVec.eq_of_toNat_eq
    have h1 : 1 ≤ (v2 τ).toNat := by rw [hv2]; exact (Nat.le_div_iff_mul_le (by decide)).mpr (by omega)
    have h2 : (v2 τ).toNat < 2 ^ 19 := by rw [hv2]; exact Nat.div_lt_of_lt_mul (by omega)
    have h3 : (v3 τ).toNat = (v2 τ).toNat - 1 := by
      simp only [v3, BitVec.toNat_sub, BitVec.toNat_ofNat]
      omega
    simp only [bit, BitVec.toNat_ushiftRight, Nat.shiftRight_eq_div_pow, h3]
    exact Nat.div_eq_of_lt (by omega)
  have h0 : v8 τ * (0 : BitVec 64) = 0 := by simp
  rw [addr, v9, hb, h0]
  simp

end Selects

theorem exec_extra {τ : State} (h : Facts cx τ) (hp : ∃ k, Pre τ k) :
    execBlock isa extra τ = some { t11 τ with mem := τ.mem } := by
  obtain ⟨k, hp⟩ := hp
  have hr := h.ready
  have hc : τ.ea { base := .x10, off := .imm 4 } = area τ + 4 := ea_x10 τ 4 hr.1
  have h1 : exec (.ldr .w .x8 { base := .x10, off := .imm 4 }) τ = some (t1 τ) := by
    rw [Access.exec_ldr_w (by rw [hc]; exact inRd (r := ⟨area τ + 4, 4⟩) h.cpuF), hc]
    rfl
  have h2 : exec (.lsr .x .x6 .x8 13) (t1 τ) = some (t2 τ) := by
    show some ((t1 τ).setReg .x6 ((t1 τ).gpr .x8 >>> 13)) = _
    rw [t1, RegUpd.gpr_setReg_self]; rfl
  have h3 : exec (.sub .x .x6 .x6 (.imm 1)) (t2 τ) = some (t3 τ) := by
    show some ((t2 τ).setReg .x6 ((t2 τ).gpr .x6 - BitVec.ofNat 64 1)) = _
    rw [t2, RegUpd.gpr_setReg_self]; rfl
  have h4 : exec (.lsr .x .x6 .x6 63) (t3 τ) = some (t4 τ) := by
    show some ((t3 τ).setReg .x6 ((t3 τ).gpr .x6 >>> 63)) = _
    rw [t3, RegUpd.gpr_setReg_self]; rfl
  have h8t4 : (t4 τ).gpr .x8 = cv τ := by
    simp only [t4, t3, t2, t1, RegUpd.gpr_setReg_of_ne _ _ (show ¬ Reg.x8 = Reg.x6 by decide),
      RegUpd.gpr_setReg_self]
  have h5 : exec (.lsl .x .x7 .x8 12) (t4 τ) = some (t5 τ) := by
    show some ((t4 τ).setReg .x7 ((t4 τ).gpr .x8 <<< 12)) = _
    rw [h8t4]; rfl
  have h3t5 : (t5 τ).gpr .x3 = τ.gpr .x3 := by
    simp only [t5, t4, t3, t2, t1, RegUpd.gpr_setReg_of_ne _ _ (show ¬ Reg.x3 = Reg.x7 by decide),
      RegUpd.gpr_setReg_of_ne _ _ (show ¬ Reg.x3 = Reg.x6 by decide),
      RegUpd.gpr_setReg_of_ne _ _ (show ¬ Reg.x3 = Reg.x8 by decide)]
  have h6 : exec (.add .x .x7 .x3 (.reg .x7)) (t5 τ) = some (t6 τ) := by
    show some ((t5 τ).setReg .x7 ((t5 τ).gpr .x3 + (t5 τ).gpr .x7)) = _
    rw [h3t5, t5, RegUpd.gpr_setReg_self]; rfl
  have h10t6 : (t6 τ).gpr .x10 = area τ := by
    simp only [t6, t5, t4, t3, t2, t1, RegUpd.gpr_setReg_of_ne _ _ (show ¬ Reg.x10 = Reg.x7 by decide),
      RegUpd.gpr_setReg_of_ne _ _ (show ¬ Reg.x10 = Reg.x6 by decide),
      RegUpd.gpr_setReg_of_ne _ _ (show ¬ Reg.x10 = Reg.x8 by decide)]
    exact hr.1
  have h7 : exec (.add .x .x5 .x10 (.imm 8)) (t6 τ) = some (t7 τ) := by
    show some ((t6 τ).setReg .x5 ((t6 τ).gpr .x10 + BitVec.ofNat 64 8)) = _
    rw [h10t6]; rfl
  have h7t7 : (t7 τ).gpr .x7 = wa τ := by
    rw [t7, RegUpd.gpr_setReg_of_ne _ _ (by decide), t6, RegUpd.gpr_setReg_self]
  have h5t7 : (t7 τ).gpr .x5 = csA τ := by rw [t7, RegUpd.gpr_setReg_self]
  have h8' : exec (.sub .x .x7 .x7 (.reg .x5)) (t7 τ) = some (t8 τ) := by
    show some ((t7 τ).setReg .x7 ((t7 τ).gpr .x7 - (t7 τ).gpr .x5)) = _
    rw [h7t7, h5t7]; rfl
  have h6t8 : (t8 τ).gpr .x6 = bit τ := by
    simp only [t8, t7, t6, t5, t4, RegUpd.gpr_setReg_of_ne _ _ (show ¬ Reg.x6 = Reg.x7 by decide),
      RegUpd.gpr_setReg_of_ne _ _ (show ¬ Reg.x6 = Reg.x5 by decide), RegUpd.gpr_setReg_self]
  have h9 : exec (.mul .x .x7 .x7 .x6) (t8 τ) = some (t9 τ) := by
    show some ((t8 τ).setReg .x7 ((t8 τ).gpr .x7 * (t8 τ).gpr .x6)) = _
    rw [h6t8, t8, RegUpd.gpr_setReg_self]; rfl
  have h5t9 : (t9 τ).gpr .x5 = csA τ := by
    simp only [t9, t8, RegUpd.gpr_setReg_of_ne _ _ (show ¬ Reg.x5 = Reg.x7 by decide)]
    exact h5t7
  have h10 : exec (.add .x .x7 .x5 (.reg .x7)) (t9 τ) = some (t10 τ) := by
    show some ((t9 τ).setReg .x7 ((t9 τ).gpr .x5 + (t9 τ).gpr .x7)) = _
    rw [h5t9, t9, RegUpd.gpr_setReg_self]; rfl
  -- The address is one the routine may write: the slab word's, or `rseq_cs`.
  have hcs : (⟨csA τ, 8⟩ : Region) ∈ τ.wr := h.cs
  have hwr : InRegions τ.wr (addr τ) 8 := by
    by_cases hi : (cpu32 τ).toNat < 8192
    · rw [addr_in hi, hp.word]
      exact inWr (r := ⟨cx.word τ, 8⟩) (h.word (inRange_iff.mpr hi))
    · rw [addr_out hi]
      exact inWr (s := τ) (r := ⟨csA τ, 8⟩) hcs
  have h7t10 : (t10 τ).gpr .x7 = addr τ := by rw [t10, RegUpd.gpr_setReg_self]
  have h11 : exec (.ldr .x .x6 { base := .x7 }) (t10 τ) = some (t11 τ) := by
    have hea : (t10 τ).ea { base := .x7 } = addr τ := by rw [Access.ea_base, h7t10]
    rw [Access.exec_ldr_x (by rw [hea]; exact hwr.append_right), hea]
    rfl
  have h12 : exec (.str .x .x6 { base := .x7 }) (t11 τ) = some { t11 τ with mem := τ.mem } := by
    have h7t11 : (t11 τ).gpr .x7 = addr τ := by
      rw [t11, RegUpd.gpr_setReg_of_ne _ _ (by decide), h7t10]
    have hea : (t11 τ).ea { base := .x7 } = addr τ := by rw [Access.ea_base, h7t11]
    have h6t11 : (t11 τ).gpr .x6 = τ.mem.readW (addr τ) 64 := by rw [t11, RegUpd.gpr_setReg_self]
    rw [Access.exec_str_x (by rw [hea]; exact hwr), hea, h6t11]
    show some { t11 τ with mem := τ.mem.writeW (addr τ) (τ.mem.readW (addr τ) 64) } = _
    rw [writeW_readW]
  rw [extra, execBlock_cons_some h1, execBlock_cons_some h2, execBlock_cons_some h3,
    execBlock_cons_some h4, execBlock_cons_some h5, execBlock_cons_some h6, execBlock_cons_some h7,
    execBlock_cons_some h8', execBlock_cons_some h9, execBlock_cons_some h10, execBlock_cons_some h11,
    execBlock_cons_some h12, execBlock_nil']

/-- The state the setup leaves. -/
def after (s : State) : State := { t11 (s6 s) with mem := (s6 s).mem }

def clobbersS : List String := ["x0", "x1", "x5", "x6", "x7", "x8", "x9", "x10", "x11"]

theorem after_gpr {s : State} {r : Reg} (h5 : ¬ r = .x5) (h6 : ¬ r = .x6) (h7 : ¬ r = .x7)
    (h8 : ¬ r = .x8) : (after s).gpr r = (s6 s).gpr r := by
  show ({ t11 (s6 s) with mem := (s6 s).mem } : State).gpr r = _
  rw [Access.gpr_withMem]
  exact t11_gpr (s6 s) h5 h6 h7 h8

theorem keeps_list : ∀ r ∈ [Reg.x5, .x6, .x7, .x8, .x9, .x10, .x11], r.x ∈ clobbersS := by decide

theorem setupOkS : SetupOk inlineTarget contract attempt popInlineSig clobbersS setupS := by
  refine .inr ⟨⟨ready, rfl⟩, fun s hp => ?_⟩
  obtain ⟨he, hpre⟩ := setup_ok setupArea hX hv (fun _ => rfl) (fun _ => rfl) (by decide) (by decide) hp
  have hf := facts_of_pre hpre
  have hexec : execBlock isa setupS s = some (after s) := by
    rw [setupS, execBlock_append, he]
    exact exec_extra hf (pre_of hf)
  have hkeep {r : Reg} (h : ¬ r ∈ [Reg.x5, .x6, .x7, .x8, .x9, .x10, .x11]) :
      (after s).gpr r = s.gpr r := by
    rw [after_gpr (fun h' => h (by simp [h'])) (fun h' => h (by simp [h'])) (fun h' => h (by simp [h']))
      (fun h' => h (by simp [h']))]
    exact s6_keeps fun hm => h (by
      simp only [List.mem_cons, List.not_mem_nil, or_false] at hm ⊢
      rcases hm with h' | h' | h' <;> simp [h'])
  have hsame : VG.Proof.Slab.Same cx s (after s) := same_of hX (by decide) rfl rfl
    (fun r hr => hkeep (r := r) (by revert r; decide)) fun _ _ => rfl
  have hpre' : contract.pre (after s) :=
    VG.Proof.Slab.pre_of_same hv (fun _ => rfl) (fun _ => rfl) (by rw [hX.wf]; trivial) hp hsame
      (by rw [mem_of hX, mem_of hX]; rfl) (by rw [hX.rd, hX.rd]; rfl) (by rw [hX.wr, hX.wr]; rfl)
  have h10 : (after s).gpr .x10 = area (after s) :=
    (after_gpr (by decide) (by decide) (by decide) (by decide)).trans s6_ready.1
  have h11 : (after s).gpr .x11 = (after s).csDesc :=
    (after_gpr (by decide) (by decide) (by decide) (by decide)).trans s6_ready.2
  exact ⟨after s, hexec, ⟨hpre', h10, h11⟩, rfl, rfl,
    argsKept_regs hX (by decide) (fun r hr => hkeep (r := r) (by revert r; decide)),
    fun _ r hr => hkeep fun hm => hr (keeps_list r hm)⟩

theorem clobbersOkS {s : State} (hs : attempt.pre s) :
    (∀ s', Exec isa popInline s s' →
      preservedExcept (clobbersS ++ (if popInlineSig.ret.isSome then ["x0"] else [])) s s') ∧
    (∀ t s', ExecExit isa popInline s t s' → preservedExcept clobbersS s s') ∧
    (∀ p ∈ popInline.abortStates isa s, preservedExcept clobbersS s p) := by
  have h := PopInline.clobbersOk hs
  exact ⟨fun s' he => preservedExcept_mono (h.1 s' he) (by decide),
    fun t s' he => preservedExcept_mono (h.2.1 t s' he) (by decide),
    fun p hp => preservedExcept_mono (h.2.2 p hp) (by decide)⟩

theorem restartsOkS : ∀ s s', attempt.pre s → Restarts inlineTarget popInline attempt s s' →
    inlineTarget.abiPreserved s s' ∧ ArgsKept inlineTarget.abi popInlineSig s s' ∧
    (inlineTarget.inline = true → inlineTarget.preservedExcept clobbersS s s') :=
  restartsOk_inline (ret := .x0) verified (fun _ hs p hp => (clobbersOkS hs).2.2 p hp)
    fun _ _ hpe => argsKept_regs hX (by decide) fun r hr => hpe r (by revert r; decide)

def art : Artifact := popInlineApi.artifact VG.AArch64.inlineTarget {
  cfg := some ([.targetEnv "musl"], false)
  setup := setupS
  code := popInline
  attempt := attempt
  verified := verified
  restartsOk := restartsOkS
  setupOk := setupOkS
  clobbers := clobbersS
  clobbersOk := fun _ _ hs => clobbersOkS hs
  witnesses := witnesses
  witnessOk := witnessOk }

#assert_standard_axioms VG.Test.Forgery.SetupStore.art

#guard refusedFor art "setup stores to memory"

end VG.Test.Forgery.SetupStore
