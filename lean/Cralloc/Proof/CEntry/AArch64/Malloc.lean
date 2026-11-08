module

public import Cralloc.Proof.CEntry.AArch64.Steps

/-!
# `c_malloc` on AArch64

`Impl.CEntry.AArch64.cMalloc` against `Spec.CEntry.mallocContract`: after the CPU number, the size
is compared with 32768, its class read from `C_SIZES`, the CPU's slab formed, and the pop's steps
(`Tail`). Four ways out: no slab for the CPU, a size above 32768, nothing to pop (each a tail call
of the slow path), and the commit, after which the block leaves for `x0`.
-/

@[expose] public section

namespace VG.Proof.CEntry.AArch64.Malloc

open VG.AArch64 VG.Spec.Slab VG.Spec.CEntry VG.Impl.Slab.AArch64 VG.Impl.CEntry.AArch64
  VG.Proof.Slab.AArch64 VG.Proof.CEntry VG.Proof.CEntry.AArch64 VG.Proof.Slab

abbrev cx : Spec.CEntry.Ctx isa := ⟨abi, .malloc, mallocSig, .c⟩

theorem hX : A64 cx.A argRegs := a64_abi

abbrev contract : Contract isa := mallocContract abi

/-- What this routine's setup forms beyond what every routine's does: its table. -/
def extra : Held := [(.x12, fun y => y SIZES + BitVec.ofInt 64 2048)]

/-- The contract of an attempt. -/
abbrev attempt : Contract isa := withReadyC extra contract

/-- The registers an attempt writes, up to its commit. -/
def scratch : List Reg := [.x1, .x2, .x3, .x4, .x5, .x6, .x8]

theorem keeps : Keeps scratch extra :=
  ⟨by decide, by decide, by decide, by decide, by decide, by decide⟩

def body : List (Step Instr Cond) := cpuC ++ mallocFront ++ slabOf ++ popSteps .x1
def tail : List Instr := [.mov .x .x0 (.reg .x4)]

theorem code_eq : cMalloc = .seq (.rseq arm body) (.block tail) := rfl

/-! ## The front half's states -/

/-- The size, as the routine has it in `x0`. -/
def n (s : State) : BitVec 64 := s.gpr .x0

abbrev c1 (s : State) : State := cpu1 (σA s) s
abbrev c2 (s : State) : State := cpu2 (σA s) s

def g1 (s : State) : State := (c2 s).addFlags (n s) (~~~(BitVec.ofNat 64 32768)) true
def g2 (s : State) : State := (g1 s).setReg .x2 (n s + BitVec.ofNat 64 7)
/-- The table index, `(size + 7) >> 3`. -/
def ix (s : State) : BitVec 64 := (n s + BitVec.ofNat 64 7) >>> 3
def g3 (s : State) : State := (g2 s).setReg .x2 (ix s)
/-- The table entry's address, and the class byte. -/
def ta (s : State) : Addr := s.syms SIZES + BitVec.ofInt 64 2048 + ix s
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
    some (fun s => [s.gpr .x0]) := rfl

theorem pre_of {s : State} (hs : attempt.pre s) : Pre s := by
  have := (pre_iff cx (s := s) hv rfl rfl).mp hs.1
  exact ⟨this.2.1, this.2.2, armOk hX this.2.1 this.2.2.2.2.1 hs.2.1, hs.2⟩

theorem size_eq (s : State) : cx.size s = n s := rfl

section Front

variable {s : State}

/-! ## What each state keeps -/

theorem mA : Mid scratch s (m1 s) (σA s) := mid_σA
theorem mc1 : Mid scratch s (m1 s) (c1 s) := mA.cpu1 (by decide)
theorem mc2 : Mid scratch s (m1 s) (c2 s) := mA.cpu2 (by decide)
theorem mg1 : Mid scratch s (m1 s) (g1 s) := mc2.addFlags _ _ _
theorem mg2 : Mid scratch s (m1 s) (g2 s) := mg1.setReg (by decide) _
theorem mg3 : Mid scratch s (m1 s) (g3 s) := mg2.setReg (by decide) _
theorem mg4 : Mid scratch s (m1 s) (g4 s) := mg3.setReg (by decide) _
theorem mg5 : Mid scratch s (m1 s) (g5 s) := mid_slab1 mg4 (by decide)
theorem mg6 : Mid scratch s (m1 s) (g6 s) := mid_slab2 mg4 (by decide) (by decide)

/-! ## The front half's steps -/

theorem x12_eq (hp : Pre s) : s.gpr .x12 = s.syms SIZES + BitVec.ofInt 64 2048 :=
  hp.rdy.2.2.2.2 (.x12, _) (List.mem_singleton_self _)

theorem exec_g1 : exec (.cmp .x .x0 (.imm 32768)) (c2 s) = some (g1 s) := by
  show some ((c2 s).addFlags ((c2 s).gpr .x0) (~~~(BitVec.ofNat 64 32768)) true) = _
  rw [mc2.gpr _ (by decide)]
  rfl

theorem eval_g1 : eval .hi (g1 s) = some (decide (32768 < (n s).toNat)) :=
  Flags.eval_hi_cmp _ _ _

theorem eval_g1_f (hn : (n s).toNat ≤ MALLOC_MAX) : eval .hi (g1 s) = some false := by
  rw [eval_g1]; simp only [MALLOC_MAX] at hn; simp only [Option.some.injEq, decide_eq_false_iff_not]; omega

theorem eval_g1_t (hn : ¬ (n s).toNat ≤ MALLOC_MAX) : eval .hi (g1 s) = some true := by
  rw [eval_g1]; simp only [MALLOC_MAX] at hn; simp only [Option.some.injEq, decide_eq_true_eq]; omega

theorem exec_g2 : exec (.add .x .x2 .x0 (.imm 7)) (g1 s) = some (g2 s) := by
  show some ((g1 s).setReg .x2 ((g1 s).gpr .x0 + BitVec.ofNat 64 7)) = _
  rw [mg1.gpr _ (by decide)]
  rfl

theorem exec_g3 : exec (.lsr .x .x2 .x2 3) (g2 s) = some (g3 s) := by
  show some ((g2 s).setReg .x2 ((g2 s).gpr .x2 >>> 3)) = _
  rw [show (g2 s).gpr .x2 = n s + BitVec.ofNat 64 7 from RegUpd.gpr_setReg_self _ _ _]
  rfl

/-- For a size the table covers, the index is `(size + 7) / 8`. -/
theorem ix_toNat (hn : (n s).toNat ≤ MALLOC_MAX) : (ix s).toNat = ((n s).toNat + 7) / 8 := by
  simp only [MALLOC_MAX] at hn
  have h7 : (n s + BitVec.ofNat 64 7).toNat = (n s).toNat + 7 := by
    rw [BitVec.toNat_add, BitVec.toNat_ofNat]
    omega
  simp only [ix, BitVec.toNat_ushiftRight, h7, Nat.shiftRight_eq_div_pow]

theorem ta_eq (hn : (n s).toNat ≤ MALLOC_MAX) :
    ta s = (cx.mallocTable s).base + BitVec.ofNat 64 (((cx.size s).toNat + 7) / 8) := by
  have hix : ix s = BitVec.ofNat 64 (((n s).toNat + 7) / 8) := by
    apply BitVec.eq_of_toNat_eq
    rw [ix_toNat hn, BitVec.toNat_ofNat, Nat.mod_eq_of_lt (by simp only [MALLOC_MAX] at hn; omega)]
  rw [ta, hix]
  rfl

theorem cls_eq (hn : (n s).toNat ≤ MALLOC_MAX) : cx.cls? s = some (kb s).toNat := by
  simp only [Spec.CEntry.Ctx.cls?, size_eq, hn, ↓reduceIte, Spec.CEntry.Ctx.mallocClass,
    Spec.CEntry.Ctx.byteAt, mem_c hX, kb]
  rw [ta_eq hn, size_eq]

/-- The table's entry is readable, and apart from `rseq_cs`. -/
theorem table_ok (hp : Pre s) (hn : (n s).toNat ≤ MALLOC_MAX) : ByteOk s (ta s) := by
  refine byte_ok hX hp.world hp.pre.2.2.1 (r := cx.mallocTable s) (by simp [tables]) ?_
  rw [ta_eq hn]
  apply Offset.contains_base
  · simp only [MALLOC_LEN, MALLOC_MAX] at hn ⊢; rw [size_eq]; omega
  · rw [size_eq]; simp only [MALLOC_MAX] at hn; omega

theorem ea_g4 (hp : Pre s) : (g3 s).ea { base := .x12, off := .reg .x2 } = ta s := by
  rw [Access.ea_reg, mg3.gpr _ (by decide), x12_eq hp,
    show (g3 s).gpr .x2 = ix s from RegUpd.gpr_setReg_self _ _ _]
  rfl

theorem tFront (hp : Pre s) (hn : (n s).toNat ≤ MALLOC_MAX) :
    Through isa mallocFront (c2 s) (g4 s) [c2 s, g1 s, g1 s, g2 s, g3 s]
      [⟨ta s, 1, false, (kb s).toNat⟩] :=
  .reg exec_g1 rfl <| .skip (eval_g1_f hn) <| .reg exec_g2 rfl <| .reg exec_g3 rfl <|
    .mem (exec_byte (table_ok hp hn) mg3 (ea_g4 hp)) (ev_byte (t := .x5) (table_ok hp hn) mg3 (ea_g4 hp)) <| .nil _

theorem xFront (hn : ¬ (n s).toNat ≤ MALLOC_MAX) :
    Leaves isa mallocFront (c2 s) (.exit "slow" (g1 s)) [c2 s, g1 s] [] 1 :=
  .reg exec_g1 rfl <| .take (eval_g1_t hn)

theorem tSlab (hp : Pre s) : Through isa slabOf (g4 s) (g6 s) [g4 s, g5 s] [] :=
  slabT (by
      rw [g4, RegUpd.gpr_setReg_of_ne _ _ (by decide), g3, RegUpd.gpr_setReg_of_ne _ _ (by decide), g2,
        RegUpd.gpr_setReg_of_ne _ _ (by decide), g1, RegUpd.gpr_addFlags, cpu2_x8])
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

/-- The accesses up to the class's byte. -/
def cpuEv (s : State) : Event := ⟨area s + 4, 4, false, (cpu32 s).toNat⟩
def tableEv (s : State) : Event := ⟨ta s, 1, false, (kb s).toNat⟩

/-- No slab. -/
theorem xC (hp : Pre s) (hi : ¬ (cpu32 s).toNat < 8192) :
    Leaves isa body (σA s) (.exit "slow" (c2 s)) [σA s, c1 s, c2 s] [cpuEv s] 2 :=
  (((cpuX hp.arm hi).before _).before _).before _

/-- A size the table does not cover. -/
theorem xS (hp : Pre s) (hi : (cpu32 s).toNat < 8192) (hn : ¬ (n s).toNat ≤ MALLOC_MAX) :
    Leaves isa body (σA s) (.exit "slow" (g1 s)) [σA s, c1 s, c2 s, c2 s, g1 s] [cpuEv s] 4 :=
  ((Leaves.after (cpuT hp.arm hi) (xFront hn)).before _).before _

/-- The steps before the pop's. -/
theorem tHead (hp : Pre s) (hi : (cpu32 s).toNat < 8192) (hn : (n s).toNat ≤ MALLOC_MAX) :
    Through isa (cpuC ++ mallocFront ++ slabOf) (σA s) (g6 s)
      [σA s, c1 s, c2 s, c2 s, g1 s, g1 s, g2 s, g3 s, g4 s, g5 s] [cpuEv s, tableEv s] :=
  ((cpuT hp.arm hi).trans (tFront hp hn)).trans (tSlab hp)

theorem popOk' (hp : Pre s) (hi : (cpu32 s).toNat < 8192) (hn : (n s).toNat ≤ MALLOC_MAX) :
    Tail.PopOk (g6 s) :=
  popOk hX (ready hp) (backFacts hX hp.world hp.pre (cls_eq hn) ((inRange_c hX s).mpr hi))
    ((inRange_c hX s).mpr hi)

theorem wreg : Tail.WReg .x1 := ⟨by decide, by decide, by decide, by decide, by decide, by decide⟩

/-- Nothing to pop. -/
theorem xP (hp : Pre s) (hi : (cpu32 s).toNat < 8192) (hn : (n s).toNat ≤ MALLOC_MAX)
    (ht : Tail.ptst (g6 s) = 0) :
    Leaves isa body (σA s) (.exit "slow" (Tail.p3 .x1 (g6 s)))
      [σA s, c1 s, c2 s, c2 s, g1 s, g1 s, g2 s, g3 s, g4 s, g5 s,
        g6 s, Tail.t1 .x1 (g6 s), Tail.t2 .x1 (g6 s), Tail.p3 .x1 (g6 s)]
      [cpuEv s, tableEv s, ⟨Tail.wa (g6 s), 8, false, (Tail.wv (g6 s)).toNat⟩,
        ⟨Tail.ma (g6 s), 8, false, (Tail.mk (g6 s)).toNat⟩] 13 :=
  Leaves.after (tHead hp hi hn) (Tail.popMiss wreg (popOk' hp hi hn) ht)

/-- The commit. -/
theorem tP (hp : Pre s) (hi : (cpu32 s).toNat < 8192) (hn : (n s).toNat ≤ MALLOC_MAX)
    (ht : ¬ Tail.ptst (g6 s) = 0) :
    Through isa body (σA s) (Tail.p6 .x1 (g6 s))
      [σA s, c1 s, c2 s, c2 s, g1 s, g1 s, g2 s, g3 s, g4 s, g5 s,
        g6 s, Tail.t1 .x1 (g6 s), Tail.t2 .x1 (g6 s), Tail.p3 .x1 (g6 s), Tail.p3 .x1 (g6 s),
        Tail.p4 .x1 (g6 s), Tail.p5 .x1 (g6 s)]
      [cpuEv s, tableEv s, ⟨Tail.wa (g6 s), 8, false, (Tail.wv (g6 s)).toNat⟩,
        ⟨Tail.ma (g6 s), 8, false, (Tail.mk (g6 s)).toNat⟩,
        ⟨Tail.wv (g6 s) - 8, 8, false, (Tail.px (g6 s)).toNat⟩,
        ⟨Tail.wa (g6 s), 8, true, (Tail.wv (g6 s) - 8).toNat⟩] :=
  (tHead hp hi hn).trans (Tail.popHit wreg (popOk' hp hi hn) ht)

/-- What follows the commit. -/
def ret0 (p : State) : State := p.setReg .x0 (p.gpr .x4)

theorem exec_tail (p : State) : execBlock isa tail p = some (ret0 p) := rfl

end Front

/-! ## The contract -/

section Contract

variable {s : State}

theorem post_eq (s' : State) : attempt.post s s' ↔ ∃ v, cx.Popped s s' v ∧ s'.gpr .x0 = v :=
  post_iff cx hv s s'

theorem exit_eq (t : String) (s' : State) : attempt.exit t s s' ↔
    [s'.gpr .x0] = [s.gpr .x0] ∧ (t = SLOW ∧ cx.PopMissed s s') :=
  exit_iff cx hv t s s'

theorem unchanged_cs : Unchanged [cx.csField s] (cx.mem s) (m1 s) := by
  rw [csField_c hX, mem_c hX]; exact frame_m1

theorem abi_of {m : Mem} {p : State} (hm : Mid scratch s m p) : abiPreserved s p :=
  abiPreserved_of_mid (by decide) hm

theorem missed (hc : cx.popResult s = none) {p : State} (hm : Mid scratch s (m1 s) p) :
    attempt.exit "slow" s p :=
  (exit_eq _ _).mpr ⟨by rw [hm.gpr _ (by decide)], rfl, hc, by rw [hX.mem, hm.mem]; exact unchanged_cs⟩

/-- The registers the convention keeps, after the sequence too. -/
theorem abi_ret {m : Mem} {p : State} (hm : Mid scratch s m p) : abiPreserved s (ret0 p) := by
  refine ⟨fun r hr => ?_, hm.sp⟩
  rw [ret0, RegUpd.gpr_setReg_of_ne _ _ (fun h => by subst h; exact absurd hr (by decide))]
  exact (abi_of hm).1 r hr

theorem ret_p6_mem {τ : State} :
    (ret0 (Tail.p6 .x1 τ)).mem = τ.mem.writeW (Tail.wa τ) (Tail.wv τ - 8) := rfl

theorem ret_p6_x0 {τ : State} : (ret0 (Tail.p6 .x1 τ)).gpr .x0 = Tail.px τ := by
  rw [ret0, RegUpd.gpr_setReg_self]
  exact Tail.p6_x4 wreg

theorem correct (hs : attempt.pre s) :
    (∃ s', Exec isa cMalloc s s' ∧ target.abiPreserved s s' ∧ attempt.post s s') ∨
    (∃ t s', ExecExit isa cMalloc s t s' ∧ target.abiPreserved s s' ∧ attempt.exit t s s') := by
  have hp := pre_of hs
  rw [code_eq]
  by_cases hi : (cpu32 s).toNat < 8192
  · have hi' := (inRange_c hX s).mpr hi
    by_cases hn : (n s).toNat ≤ MALLOC_MAX
    · have hk := cls_eq hn
      have hb := backFacts hX hp.world hp.pre hk hi'
      by_cases ht : Tail.ptst (g6 s) = 0
      · have hm := Tail.mid_p3 (w := .x1) (τ := g6 s) mg6 (by decide) (by decide)
        exact .inr ⟨"slow", _, .seqL (.rseq (execBlock_arm hp.arm) (xP hp hi hn ht).run0), abi_of hm,
          missed (pop_missed hX (ready hp) hb hk hi' ht) hm⟩
      · exact .inl ⟨ret0 (Tail.p6 .x1 (g6 s)), .seq (.rseq (execBlock_arm hp.arm) (tP hp hi hn ht).run0)
          (.block (exec_tail _)),
          abi_ret (Tail.mid_p6 (w := .x1) (τ := g6 s) mg6 (by decide) (by decide) (by decide)),
          (post_eq _).mpr ⟨_, popped hX (ready hp) hb hk hi' ht ret_p6_mem, ret_p6_x0⟩⟩
    · have hc : cx.popResult s = none := by
        have : cx.cls? s = none := by simp only [Spec.CEntry.Ctx.cls?, size_eq, hn, ↓reduceIte]
        simp only [Spec.CEntry.Ctx.popResult, this, Option.bind_none, ite_self]
      exact .inr ⟨"slow", g1 s, .seqL (.rseq (execBlock_arm hp.arm) (xS hp hi hn).run0), abi_of mg1,
        missed hc mg1⟩
  · have hc : cx.popResult s = none := by
      have hi' : ¬ cx.InRange s := fun h => hi ((inRange_c hX s).mp h)
      simp only [Spec.CEntry.Ctx.popResult, hi', ↓reduceIte]
    exact .inr ⟨"slow", c2 s, .seqL (.rseq (execBlock_arm hp.arm) (xC hp hi).run0), abi_of mc2,
      missed hc mc2⟩

/-! ### Where it may be aborted -/

theorem abort_eq (p : State) : attempt.abort s p ↔ [p.gpr .x0] = [s.gpr .x0] ∧
    (Unchanged [cx.csField s] (cx.mem s) p.mem ∧ cx.inv p) :=
  abort_iff cx hv s p

theorem abortOk (hp : Pre s) {p : State} (hm : Mid scratch s (m1 s) p) :
    armed p ∧ target.abiPreserved s p ∧ attempt.abort s p ∧
      ∀ p', target.rely p p' → attempt.env p p' → attempt.pre p' := by
  obtain ⟨hip, hre⟩ := retryC hX hv (post := fun s s' => _) (fun _ => rfl) (fun _ => rfl) (by decide)
    hp.pre hp.rdy hm (by decide) keeps frame_m1 (harmless_csC hX hp.pre)
  refine ⟨armed_of_mid hp.arm (xs := []) frame_m1 m1_cs (by simp) hm, abi_of hm, ?_, hre⟩
  exact (abort_eq p).mpr ⟨by rw [hm.gpr _ (by decide)], by rw [hm.mem]; exact unchanged_cs, hip⟩

/-- Wherever it may be aborted, memory is as after arming. -/
theorem mids (hp : Pre s) :
    ∀ p ∈ Code.abortStates isa (.seq (.rseq arm body) (.block tail)) s, Mid scratch s (m1 s) p := by
  simp only [Code.abortStates, execBlock_arm hp.arm]
  by_cases hi : (cpu32 s).toNat < 8192
  · by_cases hn : (n s).toNat ≤ MALLOC_MAX
    · by_cases ht : Tail.ptst (g6 s) = 0
      · rw [(xP hp hi hn ht).pre0]
        simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
        exact ⟨mA, mc1, mc2, mc2, mg1, mg1, mg2, mg3, mg4, mg5, mg6, Tail.mid_t1 mg6 (by decide),
          Tail.mid_t2 mg6 (by decide) (by decide), Tail.mid_p3 mg6 (by decide) (by decide)⟩
      · rw [(tP hp hi hn ht).pre0]
        simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
        exact ⟨mA, mc1, mc2, mc2, mg1, mg1, mg2, mg3, mg4, mg5, mg6, Tail.mid_t1 mg6 (by decide),
          Tail.mid_t2 mg6 (by decide) (by decide), Tail.mid_p3 mg6 (by decide) (by decide),
          Tail.mid_p3 mg6 (by decide) (by decide), Tail.mid_p4 mg6 (by decide) (by decide) (by decide),
          Tail.mid_p5 mg6 (by decide) (by decide) (by decide)⟩
    · rw [(xS hp hi hn).pre0]
      simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
      exact ⟨mA, mc1, mc2, mc2, mg1⟩
  · rw [(xC hp hi).pre0]
    simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
    exact ⟨mA, mc1, mc2⟩

/-! ### Its accesses -/

theorem trace_ok (hs : attempt.pre s) : attempt.trace s (Code.events isa cMalloc s) := by
  have hp := pre_of hs
  show cx.MallocTrace s (cx.slab.attemptEvents s (Code.events isa (.seq (.rseq arm body) (.block tail)) s))
  simp only [Code.events, execBlock_arm hp.arm, events_arm hp.arm, attemptEvents_eq (c := cx.slab) hX]
  by_cases hi : (cpu32 s).toNat < 8192
  · have hi' : (cpu32 s).toNat < SLAB_COUNT := hi
    by_cases hn : (n s).toNat ≤ MALLOC_MAX
    · have hsize : (cx.size s).toNat ≤ MALLOC_MAX := hn
      have hr := ready hp
      by_cases ht : Tail.ptst (g6 s) = 0
      · refine ⟨(o s).toNat, s.csDesc.toNat, (cpu32 s).toNat, (kb s).toNat, (Tail.wv (g6 s)).toNat,
          (Tail.mk (g6 s)).toNat, 0, ?_⟩
        rw [head_eq hX, (xP hp hi hn ht).ev0, popTail_miss hX hr ht]
        simp only [hi', hsize, and_self, ↓reduceIte, ← ta_eq hn, Spec.CEntry.Ctx.rd]
        rfl
      · refine ⟨(o s).toNat, s.csDesc.toNat, (cpu32 s).toNat, (kb s).toNat, (Tail.wv (g6 s)).toNat,
          (Tail.mk (g6 s)).toNat, (Tail.px (g6 s)).toNat, ?_⟩
        rw [head_eq hX, (tP hp hi hn ht).ev0, popTail_hit hX hr ht]
        simp only [hi', hsize, and_self, ↓reduceIte, ← ta_eq hn, Spec.CEntry.Ctx.rd]
        rfl
    · refine ⟨(o s).toNat, s.csDesc.toNat, (cpu32 s).toNat, 0, 0, 0, 0, ?_⟩
      have hsize : ¬ (cx.size s).toNat ≤ MALLOC_MAX := hn
      rw [head_eq hX, (xS hp hi hn).ev0]
      simp only [hsize, and_false, ↓reduceIte, List.append_nil]
      rfl
  · have hi' : ¬ (cpu32 s).toNat < SLAB_COUNT := hi
    refine ⟨(o s).toNat, s.csDesc.toNat, (cpu32 s).toNat, 0, 0, 0, 0, ?_⟩
    rw [head_eq hX, (xC hp hi).ev0]
    simp only [hi', false_and, ↓reduceIte, List.append_nil]
    rfl

theorem rseqOk (hs : attempt.pre s) : RseqOk target cMalloc attempt s := by
  have hp := pre_of hs
  have ht := trace_ok hs
  rw [code_eq] at ht ⊢
  exact ⟨ht, ⟨_, _, rfl, execBlock_dropLast_arm, arms_arm hp.arm⟩,
    fun p hpm => abortOk hp (mids hp p hpm)⟩

/-! ### The setup -/

theorem setupOk {l : List Instr} {g : State → State} (hl : AreaSetup l g) :
    SetupOk target contract attempt mallocSig [] (mallocTables ++ l) :=
  setupOk_of hl hX hv (fun _ => rfl) (fun _ => rfl) (by decide) (tbl := mallocTables)
    (f := fun s => baseS (tabS .x12 SIZES s)) (rs := [.x12, .x13, .x14, .x15])
    (fun s h => by
      rw [mallocTables, exec_tab, ← List.append_nil (slabsAddr :: masksAddr)]
      exact (exec_base (s := tabS .x12 SIZES s) h []))
    (fun s => mid_base (mid_tab (mid_refl s) (by decide)) (by decide) (by decide) (by decide))
    (fun s => baseS_x13 _) (fun s => baseS_x14 _) (fun s => baseS_x15 _)
    (fun s x hx => by
      rw [List.mem_singleton.mp hx]
      show (baseS (tabS .x12 SIZES s)).gpr .x12 = _
      rw [baseS_gpr _ (by decide) (by decide) (by decide), tabS_self])
    (by decide) (by decide) (by decide)

end Contract

end VG.Proof.CEntry.AArch64.Malloc
