module

public import Cralloc.Proof.CEntry.AArch64.Steps
public import Cralloc.Proof.CEntry.AArch64.Witness

/-!
# `c_calloc` on AArch64

`Impl.CEntry.AArch64.cCalloc` against `Spec.CEntry.callocContract`: after the CPU number, the product
`n * size` (`umulh`, then `mul`), the slow path if it overflows or is above 1024, its class read from
`C_CALLOC_SIZES`, the CPU's slab, and the pop's steps with the word in `x2`; after the commit, outside
the sequence, the block into `x2` and a jump to the zeroing tail. Five ways out: no slab for the CPU, an
overflow, a total above 1024, nothing to pop (each a tail call of the slow path), and the commit
followed by the tail call of the zeroing tail.
-/

@[expose] public section

namespace VG.Proof.CEntry.AArch64.Calloc

open VG.AArch64 VG.Spec.Slab VG.Spec.CEntry VG.Impl.Slab.AArch64 VG.Impl.CEntry.AArch64
  VG.Proof.Slab.AArch64 VG.Proof.CEntry VG.Proof.CEntry.AArch64 VG.Proof.Slab

abbrev cx : Spec.CEntry.Ctx isa := ⟨abi, .calloc, callocSig, .c⟩

theorem hX : A64 cx.A argRegs := a64_abi

abbrev contract : Contract isa := callocContract abi

/-- What this routine's setup forms beyond what every routine's does: its table. -/
def extra : Held := [(.x12, fun y => y CALLOC_SIZES + BitVec.ofInt 64 2048)]

/-- The contract of an attempt. -/
abbrev attempt : Contract isa := withReadyC extra contract

/-- The registers an attempt writes, up to its commit. -/
def scratch : List Reg := [.x2, .x3, .x4, .x5, .x6, .x8, .x9]

theorem keeps : Keeps scratch extra :=
  ⟨by decide, by decide, by decide, by decide, by decide, by decide⟩

def body : List (Step Instr Cond) := cpuC ++ callocFront ++ slabOf ++ popSteps .x2
def tail : List Instr := [.mov .x .x2 (.reg .x4)]

theorem code_eq : cCalloc = .seq (.rseq arm body) (.jump tail "zero") := rfl

/-! ## The front half's states -/

abbrev c1 (s : State) : State := cpu1 (σA s) s
abbrev c2 (s : State) : State := cpu2 (σA s) s

/-- The product's high half. -/
def hi (s : State) : BitVec 64 := BitVec.ofNat 64 ((s.gpr .x0).toNat * (s.gpr .x1).toNat / 2 ^ 64)
def g1 (s : State) : State := (c2 s).setReg .x2 (hi s)
/-- The product. -/
def tot (s : State) : BitVec 64 := s.gpr .x0 * s.gpr .x1
def g2 (s : State) : State := (g1 s).setReg .x9 (tot s)
def g3 (s : State) : State := (g2 s).addFlags (tot s) (~~~(BitVec.ofNat 64 1024)) true
/-- The table entry's address, and the class byte. -/
def ta (s : State) : Addr := s.syms CALLOC_SIZES + BitVec.ofInt 64 2048 + tot s
def kb (s : State) : Byte := s.mem (ta s)
def g4 (s : State) : State := (g3 s).setReg .x5 ((kb s).setWidth 64)
abbrev g5 (s : State) : State := slab1 (g4 s) s
abbrev g6 (s : State) : State := slab2 (g4 s) s

/-! ## What the precondition gives -/

structure Pre (s : State) : Prop where
  world : cx.World s
  pre : cx.pre s
  arm : ArmOk s
  rdy : readyC extra s

theorem hv : cx.A.args ((cx.sig.words cx.A.ptrBits).map (·.bits cx.A.ptrBits)) =
    some (fun s => [s.gpr .x0, s.gpr .x1]) := rfl

theorem pre_of {s : State} (hs : attempt.pre s) : Pre s := by
  have := (pre_iff cx (s := s) hv rfl rfl).mp hs.1
  exact ⟨this.2.1, this.2.2, armOk hX this.2.1 this.2.2.2.2.1 hs.2.1, hs.2⟩

theorem size_eq (s : State) : cx.size s = tot s := rfl

/-- The total is one the table covers. -/
def Ok (s : State) : Prop := ¬ cx.overflows s ∧ (cx.size s).toNat ≤ CALLOC_MAX

section Front

variable {s : State}

/-! ## What each state keeps -/

theorem mA : Mid scratch s (m1 s) (σA s) := mid_σA
theorem mc1 : Mid scratch s (m1 s) (c1 s) := mA.cpu1 (by decide)
theorem mc2 : Mid scratch s (m1 s) (c2 s) := mA.cpu2 (by decide)
theorem mg1 : Mid scratch s (m1 s) (g1 s) := mc2.setReg (by decide) _
theorem mg2 : Mid scratch s (m1 s) (g2 s) := mg1.setReg (by decide) _
theorem mg3 : Mid scratch s (m1 s) (g3 s) := mg2.addFlags _ _ _
theorem mg4 : Mid scratch s (m1 s) (g4 s) := mg3.setReg (by decide) _
theorem mg5 : Mid scratch s (m1 s) (g5 s) := mid_slab1 mg4 (by decide)
theorem mg6 : Mid scratch s (m1 s) (g6 s) := mid_slab2 mg4 (by decide) (by decide)

/-! ## The front half's steps -/

theorem x12_eq (hp : Pre s) : s.gpr .x12 = s.syms CALLOC_SIZES + BitVec.ofInt 64 2048 :=
  hp.rdy.2.2.2.2 (.x12, _) (List.mem_singleton_self _)

theorem exec_g1 : exec (.umulh .x2 .x0 .x1) (c2 s) = some (g1 s) := by
  show some ((c2 s).setReg .x2 (BitVec.ofNat 64 (((c2 s).gpr .x0).toNat * ((c2 s).gpr .x1).toNat / 2 ^ 64))) = _
  rw [mc2.gpr .x0 (by decide), mc2.gpr .x1 (by decide)]
  rfl

theorem eval_g1 : eval (.cbnz .x .x2) (g1 s) = some (decide (cx.overflows s)) := by
  rw [Flags.eval_cbnz, RegUpd.read_x, show (g1 s).gpr .x2 = hi s from RegUpd.gpr_setReg_self _ _ _]
  have hp := Nat.mul_lt_mul'' (s.gpr .x0).isLt (s.gpr .x1).isLt
  have hq : (s.gpr .x0).toNat * (s.gpr .x1).toNat / 2 ^ 64 < 2 ^ 64 := by
    apply Nat.div_lt_of_lt_mul; rw [← Nat.pow_add]; exact hp
  have hz : (hi s != 0) = decide (2 ^ 64 ≤ (s.gpr .x0).toNat * (s.gpr .x1).toNat) := by
    rw [hi, Bool.eq_iff_iff, bne_iff_ne, ne_eq, ← BitVec.toNat_inj, BitVec.toNat_ofNat,
      show (0 : BitVec 64).toNat = 0 from rfl, Nat.mod_eq_of_lt hq, Nat.div_eq_zero_iff_lt (by decide),
      decide_eq_true_iff]
    omega
  rw [hz]
  rfl

theorem eval_g1_t (ho : cx.overflows s) : eval (.cbnz .x .x2) (g1 s) = some true := by
  rw [eval_g1]; simp [ho]

theorem eval_g1_f (ho : ¬ cx.overflows s) : eval (.cbnz .x .x2) (g1 s) = some false := by
  rw [eval_g1]; simp [ho]

theorem exec_g2 : exec (.mul .x .x9 .x0 .x1) (g1 s) = some (g2 s) := by
  show some ((g1 s).setReg .x9 ((g1 s).gpr .x0 * (g1 s).gpr .x1)) = _
  rw [mg1.gpr .x0 (by decide), mg1.gpr .x1 (by decide)]
  rfl

theorem g2_x9 : (g2 s).gpr .x9 = tot s := RegUpd.gpr_setReg_self _ _ _

theorem exec_g3 : exec (.cmp .x .x9 (.imm 1024)) (g2 s) = some (g3 s) := by
  show some ((g2 s).addFlags ((g2 s).gpr .x9) (~~~(BitVec.ofNat 64 1024)) true) = _
  rw [g2_x9]
  rfl

theorem eval_g3 : eval .hi (g3 s) = some (decide (1024 < (tot s).toNat)) :=
  Flags.eval_hi_cmp _ _ _

theorem eval_g3_f (hn : (cx.size s).toNat ≤ CALLOC_MAX) : eval .hi (g3 s) = some false := by
  rw [eval_g3]
  simp only [CALLOC_MAX, size_eq] at hn
  simp only [Option.some.injEq, decide_eq_false_iff_not]
  omega

theorem eval_g3_t (hn : ¬ (cx.size s).toNat ≤ CALLOC_MAX) : eval .hi (g3 s) = some true := by
  rw [eval_g3]
  simp only [CALLOC_MAX, size_eq] at hn
  simp only [Option.some.injEq, decide_eq_true_eq]
  omega

theorem ta_eq : ta s = (cx.callocTable s).base + BitVec.ofNat 64 (cx.size s).toNat := by
  rw [ta, BitVec.ofNat_toNat, BitVec.setWidth_eq]
  rfl

theorem cls_eq (ho : Ok s) : cx.cls? s = some (kb s).toNat := by
  simp only [Spec.CEntry.Ctx.cls?, show ¬ cx.overflows s from ho.1, ho.2, not_false_eq_true, and_self,
    ↓reduceIte, Spec.CEntry.Ctx.callocClass, Spec.CEntry.Ctx.byteAt, mem_c hX, kb]
  rw [ta_eq]

/-- The table's entry is readable, and apart from `rseq_cs`. -/
theorem table_ok (hp : Pre s) (ho : Ok s) : ByteOk s (ta s) := by
  have h2 := ho.2
  refine byte_ok hX hp.world hp.pre.2.2.1 (r := cx.callocTable s) (by simp [tables]) ?_
  rw [ta_eq]
  apply Offset.contains_base
  · simp only [CALLOC_LEN, CALLOC_MAX] at h2 ⊢; omega
  · simp only [CALLOC_MAX] at h2; omega

theorem g3_x9 : (g3 s).gpr .x9 = tot s := by rw [g3, RegUpd.gpr_addFlags, g2_x9]

theorem ea_g4 (hp : Pre s) : (g3 s).ea { base := .x12, off := .reg .x9 } = ta s := by
  rw [Access.ea_reg, mg3.gpr _ (by decide), x12_eq hp, g3_x9]
  rfl

theorem tFront (hp : Pre s) (ho : Ok s) :
    Through isa callocFront (c2 s) (g4 s) [c2 s, g1 s, g1 s, g2 s, g3 s, g3 s]
      [⟨ta s, 1, false, (kb s).toNat⟩] :=
  .reg exec_g1 rfl <| .skip (eval_g1_f ho.1) <| .reg exec_g2 rfl <| .reg exec_g3 rfl <|
    .skip (eval_g3_f ho.2) <|
    .mem (exec_byte (table_ok hp ho) mg3 (ea_g4 hp)) (ev_byte (t := .x5) (table_ok hp ho) mg3 (ea_g4 hp)) <|
    .nil _

/-- An overflowing product. -/
theorem xO (ho : cx.overflows s) :
    Leaves isa callocFront (c2 s) (.exit "slow" (g1 s)) [c2 s, g1 s] [] 1 :=
  .reg exec_g1 rfl <| .take (eval_g1_t ho)

/-- A total the table does not cover. -/
theorem xN (ho : ¬ cx.overflows s) (hn : ¬ (cx.size s).toNat ≤ CALLOC_MAX) :
    Leaves isa callocFront (c2 s) (.exit "slow" (g3 s)) [c2 s, g1 s, g1 s, g2 s, g3 s] [] 4 :=
  .reg exec_g1 rfl <| .skip (eval_g1_f ho) <| .reg exec_g2 rfl <| .reg exec_g3 rfl <| .take (eval_g3_t hn)

theorem tSlab (hp : Pre s) : Through isa slabOf (g4 s) (g6 s) [g4 s, g5 s] [] :=
  slabT (by
      rw [g4, RegUpd.gpr_setReg_of_ne _ _ (by decide), g3, RegUpd.gpr_addFlags, g2,
        RegUpd.gpr_setReg_of_ne _ _ (by decide), g1, RegUpd.gpr_setReg_of_ne _ _ (by decide), cpu2_x8])
    (by rw [mg4.gpr _ (by decide)]; exact hp.rdy.2.1)

theorem ready (hp : Pre s) : Ready s (kb s).toNat (g6 s) where
  mem := rfl
  rd := rfl
  wr := rfl
  x3 := slab2_x3
  x5 := by
    rw [slab2_gpr (by decide) (by decide), g4, RegUpd.gpr_setReg_self, byte_ofNat]
  x15 := by rw [mg6.gpr _ (by decide)]; exact hp.rdy.2.2.2.1

/-! ## The ways out -/

def cpuEv (s : State) : Event := ⟨area s + 4, 4, false, (cpu32 s).toNat⟩
def tableEv (s : State) : Event := ⟨ta s, 1, false, (kb s).toNat⟩

theorem xC (hp : Pre s) (hi : ¬ (cpu32 s).toNat < 8192) :
    Leaves isa body (σA s) (.exit "slow" (c2 s)) [σA s, c1 s, c2 s] [cpuEv s] 2 :=
  (((cpuX hp.arm hi).before _).before _).before _

theorem xOver (hp : Pre s) (hi : (cpu32 s).toNat < 8192) (ho : cx.overflows s) :
    Leaves isa body (σA s) (.exit "slow" (g1 s)) [σA s, c1 s, c2 s, c2 s, g1 s] [cpuEv s] 4 :=
  ((Leaves.after (cpuT hp.arm hi) (xO ho)).before _).before _

theorem xBig (hp : Pre s) (hi : (cpu32 s).toNat < 8192) (ho : ¬ cx.overflows s)
    (hn : ¬ (cx.size s).toNat ≤ CALLOC_MAX) :
    Leaves isa body (σA s) (.exit "slow" (g3 s)) [σA s, c1 s, c2 s, c2 s, g1 s, g1 s, g2 s, g3 s]
      [cpuEv s] 7 :=
  ((Leaves.after (cpuT hp.arm hi) (xN ho hn)).before _).before _

theorem tHead (hp : Pre s) (hi : (cpu32 s).toNat < 8192) (ho : Ok s) :
    Through isa (cpuC ++ callocFront ++ slabOf) (σA s) (g6 s)
      [σA s, c1 s, c2 s, c2 s, g1 s, g1 s, g2 s, g3 s, g3 s, g4 s, g5 s] [cpuEv s, tableEv s] :=
  ((cpuT hp.arm hi).trans (tFront hp ho)).trans (tSlab hp)

theorem popOk' (hp : Pre s) (hi : (cpu32 s).toNat < 8192) (ho : Ok s) : Tail.PopOk (g6 s) :=
  popOk hX (ready hp) (backFacts hX hp.world hp.pre (cls_eq ho) ((inRange_c hX s).mpr hi))
    ((inRange_c hX s).mpr hi)

theorem wreg : Tail.WReg .x2 := ⟨by decide, by decide, by decide, by decide, by decide, by decide⟩

theorem xP (hp : Pre s) (hi : (cpu32 s).toNat < 8192) (ho : Ok s) (ht : Tail.ptst (g6 s) = 0) :
    Leaves isa body (σA s) (.exit "slow" (Tail.p3 .x2 (g6 s)))
      [σA s, c1 s, c2 s, c2 s, g1 s, g1 s, g2 s, g3 s, g3 s, g4 s, g5 s,
        g6 s, Tail.t1 .x2 (g6 s), Tail.t2 .x2 (g6 s), Tail.p3 .x2 (g6 s)]
      [cpuEv s, tableEv s, ⟨Tail.wa (g6 s), 8, false, (Tail.wv (g6 s)).toNat⟩,
        ⟨Tail.ma (g6 s), 8, false, (Tail.mk (g6 s)).toNat⟩] 14 :=
  Leaves.after (tHead hp hi ho) (Tail.popMiss wreg (popOk' hp hi ho) ht)

theorem tP (hp : Pre s) (hi : (cpu32 s).toNat < 8192) (ho : Ok s) (ht : ¬ Tail.ptst (g6 s) = 0) :
    Through isa body (σA s) (Tail.p6 .x2 (g6 s))
      [σA s, c1 s, c2 s, c2 s, g1 s, g1 s, g2 s, g3 s, g3 s, g4 s, g5 s,
        g6 s, Tail.t1 .x2 (g6 s), Tail.t2 .x2 (g6 s), Tail.p3 .x2 (g6 s), Tail.p3 .x2 (g6 s),
        Tail.p4 .x2 (g6 s), Tail.p5 .x2 (g6 s)]
      [cpuEv s, tableEv s, ⟨Tail.wa (g6 s), 8, false, (Tail.wv (g6 s)).toNat⟩,
        ⟨Tail.ma (g6 s), 8, false, (Tail.mk (g6 s)).toNat⟩,
        ⟨Tail.wv (g6 s) - 8, 8, false, (Tail.px (g6 s)).toNat⟩,
        ⟨Tail.wa (g6 s), 8, true, (Tail.wv (g6 s) - 8).toNat⟩] :=
  (tHead hp hi ho).trans (Tail.popHit wreg (popOk' hp hi ho) ht)

/-- After the commit, outside the sequence: the block into `x2`. -/
def ret2 (p : State) : State := p.setReg .x2 (p.gpr .x4)

theorem exec_tail (p : State) : execBlock isa tail p = some (ret2 p) := rfl

end Front

/-! ## The contract -/

section Contract

variable {s : State}

theorem exit_eq (t : String) (s' : State) : attempt.exit t s s' ↔
    [s'.gpr .x0, s'.gpr .x1] = [s.gpr .x0, s.gpr .x1] ∧
      ((t = SLOW ∧ cx.PopMissed s s') ∨ (t = ZERO ∧ ∃ x, cx.Popped s s' x ∧ cx.third s' = x)) :=
  exit_iff cx hv t s s'

theorem unchanged_cs : Unchanged [cx.csField s] (cx.mem s) (m1 s) := by
  rw [csField_c hX, mem_c hX]; exact frame_m1

theorem abi_of {m : Mem} {p : State} (hm : Mid scratch s m p) : abiPreserved s p :=
  abiPreserved_of_mid (by decide) hm

theorem missed (hc : cx.popResult s = none) {p : State} (hm : Mid scratch s (m1 s) p) :
    attempt.exit "slow" s p :=
  (exit_eq _ _).mpr ⟨by rw [hm.gpr .x0 (by decide), hm.gpr .x1 (by decide)],
    .inl ⟨rfl, hc, by rw [hX.mem, hm.mem]; exact unchanged_cs⟩⟩

theorem none_of (ho : ¬ Ok s) : cx.popResult s = none := by
  have ho' : ¬ (¬ cx.overflows s ∧ (cx.size s).toNat ≤ CALLOC_MAX) := ho
  have : cx.cls? s = none := by simp only [Spec.CEntry.Ctx.cls?, ho', ↓reduceIte]
  simp only [Spec.CEntry.Ctx.popResult, this, Option.bind_none, ite_self]

theorem ret_p6_mem {τ : State} :
    (ret2 (Tail.p6 .x2 τ)).mem = τ.mem.writeW (Tail.wa τ) (Tail.wv τ - 8) := rfl

theorem ret_p6_x2 {τ : State} : cx.third (ret2 (Tail.p6 .x2 τ)) = Tail.px τ := by
  show (ret2 (Tail.p6 .x2 τ)).gpr .x2 = _
  rw [ret2, RegUpd.gpr_setReg_self]
  exact Tail.p6_x4 wreg

theorem mid_ret {m : Mem} {p : State} (hm : Mid scratch s m p) : Mid scratch s m (ret2 p) :=
  hm.setReg (by decide) _

theorem correct (hs : attempt.pre s) :
    (∃ s', Exec isa cCalloc s s' ∧ target.abiPreserved s s' ∧ attempt.post s s') ∨
    (∃ t s', ExecExit isa cCalloc s t s' ∧ target.abiPreserved s s' ∧ attempt.exit t s s') := by
  have hp := pre_of hs
  rw [code_eq]
  by_cases hi : (cpu32 s).toNat < 8192
  · have hi' := (inRange_c hX s).mpr hi
    by_cases ho : cx.overflows s
    · exact .inr ⟨"slow", g1 s, .seqL (.rseq (execBlock_arm hp.arm) (xOver hp hi ho).run0), abi_of mg1,
        missed (none_of fun h => h.1 ho) mg1⟩
    by_cases hn : (cx.size s).toNat ≤ CALLOC_MAX
    · have hok : Ok s := ⟨ho, hn⟩
      have hk := cls_eq hok
      have hb := backFacts hX hp.world hp.pre hk hi'
      by_cases ht : Tail.ptst (g6 s) = 0
      · have hm := Tail.mid_p3 (w := .x2) (τ := g6 s) mg6 (by decide) (by decide)
        exact .inr ⟨"slow", _, .seqL (.rseq (execBlock_arm hp.arm) (xP hp hi hok ht).run0), abi_of hm,
          missed (pop_missed hX (ready hp) hb hk hi' ht) hm⟩
      · have hmq := mid_ret (Tail.mid_p6 (w := .x2) (τ := g6 s) mg6 (by decide) (by decide) (by decide))
        exact .inr ⟨"zero", _, .seqR (.rseq (execBlock_arm hp.arm) (tP hp hi hok ht).run0)
          (.jump (exec_tail _)), abi_of hmq,
          (exit_eq _ _).mpr ⟨by rw [hmq.gpr .x0 (by decide), hmq.gpr .x1 (by decide)],
            .inr ⟨rfl, _, popped hX (ready hp) hb hk hi' ht ret_p6_mem, ret_p6_x2⟩⟩⟩
    · exact .inr ⟨"slow", g3 s, .seqL (.rseq (execBlock_arm hp.arm) (xBig hp hi ho hn).run0), abi_of mg3,
        missed (none_of fun h => hn h.2) mg3⟩
  · have hc : cx.popResult s = none := by
      have hi' : ¬ cx.InRange s := fun h => hi ((inRange_c hX s).mp h)
      simp only [Spec.CEntry.Ctx.popResult, hi', ↓reduceIte]
    exact .inr ⟨"slow", c2 s, .seqL (.rseq (execBlock_arm hp.arm) (xC hp hi).run0), abi_of mc2,
      missed hc mc2⟩

/-! ### Where it may be aborted -/

theorem abort_eq (p : State) : attempt.abort s p ↔ [p.gpr .x0, p.gpr .x1] = [s.gpr .x0, s.gpr .x1] ∧
    (Unchanged [cx.csField s] (cx.mem s) p.mem ∧ cx.inv p) :=
  abort_iff cx hv s p

theorem abortOk (hp : Pre s) {p : State} (hm : Mid scratch s (m1 s) p) :
    armed p ∧ target.abiPreserved s p ∧ attempt.abort s p ∧
      ∀ p', target.rely p p' → attempt.env p p' → attempt.pre p' := by
  obtain ⟨hip, hre⟩ := retryC hX hv (post := fun s s' => _) (fun _ => rfl) (fun _ => rfl) (by decide)
    hp.pre hp.rdy hm (by decide) keeps frame_m1 (harmless_csC hX hp.pre)
  refine ⟨armed_of_mid hp.arm (xs := []) frame_m1 m1_cs (by simp) hm, abi_of hm, ?_, hre⟩
  exact (abort_eq p).mpr ⟨by rw [hm.gpr .x0 (by decide), hm.gpr .x1 (by decide)],
    by rw [hm.mem]; exact unchanged_cs, hip⟩

/-- Wherever it may be aborted, memory is as after arming. -/
theorem mids (hp : Pre s) :
    ∀ p ∈ Code.abortStates isa (.seq (.rseq arm body) (.jump tail "zero")) s, Mid scratch s (m1 s) p := by
  simp only [Code.abortStates, execBlock_arm hp.arm]
  by_cases hi : (cpu32 s).toNat < 8192
  · by_cases ho : cx.overflows s
    · rw [(xOver hp hi ho).pre0]
      simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
      exact ⟨mA, mc1, mc2, mc2, mg1⟩
    by_cases hn : (cx.size s).toNat ≤ CALLOC_MAX
    · by_cases ht : Tail.ptst (g6 s) = 0
      · rw [(xP hp hi ⟨ho, hn⟩ ht).pre0]
        simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
        exact ⟨mA, mc1, mc2, mc2, mg1, mg1, mg2, mg3, mg3, mg4, mg5, mg6, Tail.mid_t1 mg6 (by decide),
          Tail.mid_t2 mg6 (by decide) (by decide), Tail.mid_p3 mg6 (by decide) (by decide)⟩
      · rw [(tP hp hi ⟨ho, hn⟩ ht).pre0]
        simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
        exact ⟨mA, mc1, mc2, mc2, mg1, mg1, mg2, mg3, mg3, mg4, mg5, mg6, Tail.mid_t1 mg6 (by decide),
          Tail.mid_t2 mg6 (by decide) (by decide), Tail.mid_p3 mg6 (by decide) (by decide),
          Tail.mid_p3 mg6 (by decide) (by decide), Tail.mid_p4 mg6 (by decide) (by decide) (by decide),
          Tail.mid_p5 mg6 (by decide) (by decide) (by decide)⟩
    · rw [(xBig hp hi ho hn).pre0]
      simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
      exact ⟨mA, mc1, mc2, mc2, mg1, mg1, mg2, mg3⟩
  · rw [(xC hp hi).pre0]
    simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
    exact ⟨mA, mc1, mc2⟩

/-! ### Its accesses -/

theorem trace_ok (hs : attempt.pre s) : attempt.trace s (Code.events isa cCalloc s) := by
  have hp := pre_of hs
  show cx.CallocTrace s
    (cx.slab.attemptEvents s (Code.events isa (.seq (.rseq arm body) (.jump tail "zero")) s))
  simp only [Code.events, execBlock_arm hp.arm, events_arm hp.arm, attemptEvents_eq (c := cx.slab) hX]
  by_cases hi : (cpu32 s).toNat < 8192
  · have hi' : (cpu32 s).toNat < SLAB_COUNT := hi
    by_cases ho : cx.overflows s
    · refine ⟨(o s).toNat, s.csDesc.toNat, (cpu32 s).toNat, 0, 0, 0, 0, ?_⟩
      rw [head_eq hX, (xOver hp hi ho).ev0]
      simp only [ho, not_true_eq_false, false_and, and_false, ↓reduceIte, List.append_nil]
      rfl
    by_cases hn : (cx.size s).toNat ≤ CALLOC_MAX
    · have hr := ready hp
      by_cases ht : Tail.ptst (g6 s) = 0
      · refine ⟨(o s).toNat, s.csDesc.toNat, (cpu32 s).toNat, (kb s).toNat, (Tail.wv (g6 s)).toNat,
          (Tail.mk (g6 s)).toNat, 0, ?_⟩
        rw [head_eq hX, (xP hp hi ⟨ho, hn⟩ ht).ev0, popTail_miss hX hr ht]
        simp only [hi', ho, hn, not_false_eq_true, and_self, ↓reduceIte, ← ta_eq, Spec.CEntry.Ctx.rd]
        rfl
      · refine ⟨(o s).toNat, s.csDesc.toNat, (cpu32 s).toNat, (kb s).toNat, (Tail.wv (g6 s)).toNat,
          (Tail.mk (g6 s)).toNat, (Tail.px (g6 s)).toNat, ?_⟩
        rw [head_eq hX, (tP hp hi ⟨ho, hn⟩ ht).ev0, popTail_hit hX hr ht]
        simp only [hi', ho, hn, not_false_eq_true, and_self, ↓reduceIte, ← ta_eq, Spec.CEntry.Ctx.rd]
        rfl
    · refine ⟨(o s).toNat, s.csDesc.toNat, (cpu32 s).toNat, 0, 0, 0, 0, ?_⟩
      rw [head_eq hX, (xBig hp hi ho hn).ev0]
      simp only [hn, and_false, ↓reduceIte, List.append_nil]
      rfl
  · have hi' : ¬ (cpu32 s).toNat < SLAB_COUNT := hi
    refine ⟨(o s).toNat, s.csDesc.toNat, (cpu32 s).toNat, 0, 0, 0, 0, ?_⟩
    rw [head_eq hX, (xC hp hi).ev0]
    simp only [hi', false_and, ↓reduceIte, List.append_nil]
    rfl

theorem rseqOk (hs : attempt.pre s) : RseqOk target cCalloc attempt s := by
  have hp := pre_of hs
  have ht := trace_ok hs
  rw [code_eq] at ht ⊢
  exact ⟨ht, ⟨_, _, rfl, execBlock_dropLast_arm, arms_arm hp.arm⟩,
    fun p hpm => abortOk hp (mids hp p hpm)⟩

/-! ### The setup -/

theorem setupOk {l : List Instr} {g : State → State} (hl : AreaSetup l g) :
    SetupOk target contract attempt callocSig [] (callocTables ++ l) :=
  setupOk_of hl hX hv (fun _ => rfl) (fun _ => rfl) (by decide) (tbl := callocTables)
    (f := fun s => baseS (tabS .x12 CALLOC_SIZES s)) (rs := [.x12, .x13, .x14, .x15])
    (fun s h => by
      rw [callocTables, exec_tab, ← List.append_nil (slabsAddr :: masksAddr)]
      exact (exec_base (s := tabS .x12 CALLOC_SIZES s) h []))
    (fun s => mid_base (mid_tab (mid_refl s) (by decide)) (by decide) (by decide) (by decide))
    (fun s => baseS_x13 _) (fun s => baseS_x14 _) (fun s => baseS_x15 _)
    (fun s x hx => by
      rw [List.mem_singleton.mp hx]
      show (baseS (tabS .x12 CALLOC_SIZES s)).gpr .x12 = _
      rw [baseS_gpr _ (by decide) (by decide) (by decide), tabS_self])
    (by decide) (by decide) (by decide)

end Contract

/-! ## The witnesses

`C_CALLOC_SIZES` gives the total 8 the class 1, and a magazine at `0x4000` has 4 slots, whose slot 1
holds `0x5000`. One state per way out: a CPU number with no slab, a product that overflows, a total
above 1024, an empty slab word, and a pop. -/

def wsOf (cpu word : BitVec 64) : List (Addr × BitVec 64) :=
  [(0x2000, 0x100), (0x10104, cpu), (0x1000008, word), (0x4008, 0x5000), (0x3000808, 16),
    (0x8000808, 1)]

def witM (cpu n size word : BitVec 64) (rd wr : List Region) : State :=
  witS CALLOC_SIZES n size 0 0x8000800 (wsOf cpu word) rd wr

def roM : List Region := [⟨0x2000, 8⟩, ⟨0x10104, 4⟩, ⟨0x3000800, 2048⟩, ⟨0x8000800, 1025⟩]

def w0 : State := witM 8192 1 8 0 roM [⟨0x10108, 8⟩]
def w1 : State := witM 0 0x8000000000000000 2 0 roM [⟨0x10108, 8⟩]
def w2 : State := witM 0 1 2000 0 roM [⟨0x10108, 8⟩]
def w3 : State := witM 0 1 8 0 roM [⟨0x10108, 8⟩, ⟨0x1000008, 8⟩]
def w4 : State := witM 0 1 8 0x4010 roM [⟨0x10108, 8⟩, ⟨0x1000008, 8⟩, ⟨0x4000, 32⟩]

theorem pre_w0 : attempt.pre w0 :=
  ⟨pre_of_check hX (ws := wsOf 8192 0) rfl rfl hv (fun _ => rfl) (fun _ => rfl) (by decide +kernel),
    ⟨by decide +kernel, by decide +kernel⟩, by decide +kernel, by decide +kernel, by decide +kernel,
    by decide +kernel⟩

theorem pre_w1 : attempt.pre w1 :=
  ⟨pre_of_check hX (ws := wsOf 0 0) rfl rfl hv (fun _ => rfl) (fun _ => rfl) (by decide +kernel),
    ⟨by decide +kernel, by decide +kernel⟩, by decide +kernel, by decide +kernel, by decide +kernel,
    by decide +kernel⟩

theorem pre_w2 : attempt.pre w2 :=
  ⟨pre_of_check hX (ws := wsOf 0 0) rfl rfl hv (fun _ => rfl) (fun _ => rfl) (by decide +kernel),
    ⟨by decide +kernel, by decide +kernel⟩, by decide +kernel, by decide +kernel, by decide +kernel,
    by decide +kernel⟩

theorem pre_w3 : attempt.pre w3 :=
  ⟨pre_of_check hX (ws := wsOf 0 0) rfl rfl hv (fun _ => rfl) (fun _ => rfl) (by decide +kernel),
    ⟨by decide +kernel, by decide +kernel⟩, by decide +kernel, by decide +kernel, by decide +kernel,
    by decide +kernel⟩

theorem pre_w4 : attempt.pre w4 :=
  ⟨pre_of_check hX (ws := wsOf 0 0x4010) rfl rfl hv (fun _ => rfl) (fun _ => rfl) (by decide +kernel),
    ⟨by decide +kernel, by decide +kernel⟩, by decide +kernel, by decide +kernel, by decide +kernel,
    by decide +kernel⟩

theorem outcomes_eq : cCalloc.outcomes = [(0, true), (1, true), (2, true), (3, true), (4, true)] := rfl

theorem visits_w0 : Visits isa cCalloc w0 0 true := by
  have hp := pre_of pre_w0
  exact .seqL (Visits.rseq (execBlock_arm hp.arm) (xC hp (by decide +kernel)).stop0)

theorem visits_w1 : Visits isa cCalloc w1 1 true := by
  have hp := pre_of pre_w1
  exact .seqL (Visits.rseq (execBlock_arm hp.arm) (xOver hp (by decide +kernel) (by decide +kernel)).stop0)

theorem visits_w2 : Visits isa cCalloc w2 2 true := by
  have hp := pre_of pre_w2
  exact .seqL (Visits.rseq (execBlock_arm hp.arm)
    (xBig hp (by decide +kernel) (by decide +kernel) (by decide +kernel)).stop0)

theorem visits_w3 : Visits isa cCalloc w3 3 true := by
  have hp := pre_of pre_w3
  have hok : Ok w3 := ⟨by decide +kernel, by decide +kernel⟩
  exact .seqL (Visits.rseq (execBlock_arm hp.arm)
    (xP hp (by decide +kernel) hok (by decide +kernel)).stop0)

theorem visits_w4 : Visits isa cCalloc w4 4 true := by
  have hp := pre_of pre_w4
  have hok : Ok w4 := ⟨by decide +kernel, by decide +kernel⟩
  exact .seqL (Visits.rseq (execBlock_arm hp.arm)
    (tP hp (by decide +kernel) hok (by decide +kernel)).stop0)

def witnesses : List State := [w0, w1, w2, w3, w4]

theorem witnessOk : (∃ w ∈ witnesses, attempt.pre w) ∧
    ∀ o ∈ cCalloc.outcomes, ∃ w ∈ witnesses, attempt.pre w ∧ Visits isa cCalloc w o.1 o.2 := by
  refine ⟨⟨w0, by simp [witnesses], pre_w0⟩, fun o ho => ?_⟩
  rw [outcomes_eq] at ho
  simp only [List.mem_cons, List.not_mem_nil, or_false] at ho
  rcases ho with rfl | rfl | rfl | rfl | rfl
  · exact ⟨w0, by simp [witnesses], pre_w0, visits_w0⟩
  · exact ⟨w1, by simp [witnesses], pre_w1, visits_w1⟩
  · exact ⟨w2, by simp [witnesses], pre_w2, visits_w2⟩
  · exact ⟨w3, by simp [witnesses], pre_w3, visits_w3⟩
  · exact ⟨w4, by simp [witnesses], pre_w4, visits_w4⟩

theorem verified : Verified target cCalloc attempt :=
  ⟨fun _ hs => correct hs, fun _ hs => rseqOk hs, ⟨_, pre_w0⟩, fun o ho => by
    obtain ⟨w, -, hw, hv⟩ := witnessOk.2 o ho
    exact ⟨w, hw, hv⟩⟩

/-- The last attempt, aborted any number of times, starts with the callee-saved registers, the stack
pointer and the arguments of the first. -/
theorem restartsOk : ∀ s s', attempt.pre s → Restarts target cCalloc attempt s s' →
    target.abiPreserved s s' ∧ ArgsKept target.abi callocSig s s' ∧
    (target.inline = true → target.preservedExcept [] s s') :=
  restartsOk_target verified (by decide) fun _ hs p hp r hr =>
    (mids (pre_of hs) p hp).gpr r (by revert r; decide)

end VG.Proof.CEntry.AArch64.Calloc
