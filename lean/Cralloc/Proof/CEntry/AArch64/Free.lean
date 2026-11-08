module

public import Cralloc.Proof.CEntry.AArch64.Region
public import Cralloc.Proof.CEntry.AArch64.Witness

/-!
# `c_free` on AArch64

`Impl.CEntry.AArch64.cFree` against `Spec.CEntry.freeContract`: after the CPU number, a null pointer
returns; otherwise the CPU's slab, the region index (`RegionIx`), the region's class byte, and the
push's steps (`Tail`). Five ways out: no slab for the CPU, null (a return), an index out of range, no
free slot (the three tail calls of the slow path), and the commit.
-/

@[expose] public section

namespace VG.Proof.CEntry.AArch64.Free

open VG.AArch64 VG.Spec.Slab VG.Spec.CEntry VG.Impl.Slab.AArch64 VG.Impl.CEntry.AArch64
  VG.Proof.Slab.AArch64 VG.Proof.CEntry VG.Proof.CEntry.AArch64 VG.Proof.Slab

abbrev cx : Spec.CEntry.Ctx isa := ⟨abi, .free, freeSig, .c⟩

theorem hX : A64 cx.A argRegs := a64_abi

abbrev contract : Contract isa := freeContract abi .c

/-- What this routine's setup forms beyond what every routine's does: the region table. -/
def extra : Held := [(.x12, fun y => y SLABS + BitVec.ofNat 64 REGION_DISP)]

/-- The contract of an attempt. -/
abbrev attempt : Contract isa := withReadyC extra contract

/-- The registers an attempt writes. -/
def scratch : List Reg := [.x1, .x2, .x3, .x4, .x5, .x6, .x7, .x8]

theorem keeps : Keeps scratch extra :=
  ⟨by decide, by decide, by decide, by decide, by decide, by decide⟩

def body : List (Step Instr Cond) :=
  cpuC ++ freeNull ++ slabOf ++ regionIndex ++ freeByte ++ pushSteps .x1 .x2

theorem code_eq : cFree = .rseq arm body := rfl

/-! ## The front half's states -/

/-- The pointer. -/
def n (s : State) : BitVec 64 := s.gpr .x0

abbrev c1 (s : State) : State := cpu1 (σA s) s
abbrev c2 (s : State) : State := cpu2 (σA s) s
abbrev f1 (s : State) : State := slab1 (c2 s) s
abbrev f2 (s : State) : State := slab2 (c2 s) s
abbrev r1 (s : State) : State := RegionIx.r1 (f2 s)
abbrev r2 (s : State) : State := RegionIx.r2 (f2 s)
abbrev r3 (s : State) : State := RegionIx.r3 (f2 s)
abbrev r4 (s : State) : State := RegionIx.r4 (f2 s)
abbrev r5 (s : State) : State := RegionIx.r5 (f2 s)
/-- The region byte's address, and the byte. -/
def ra (s : State) : Addr := s.syms SLABS + BitVec.ofNat 64 REGION_DISP + RegionIx.idx (f2 s)
def rb (s : State) : Byte := s.mem (ra s)
/-- The state the push's steps start in. -/
def τ (s : State) : State := (r5 s).setReg .x5 ((s.mem (ra s)).setWidth 64)

/-! ## What the precondition gives -/

structure Pre (s : State) : Prop where
  world : cx.World s
  pre : cx.pre s
  arm : ArmOk s
  rdy : readyC extra s
  pub : RegionIx.PubFacts cx s

theorem hv : cx.A.args ((cx.sig.words cx.A.ptrBits).map (·.bits cx.A.ptrBits)) =
    some (fun s => [s.gpr .x0]) := rfl

theorem pre_of {s : State} (hs : attempt.pre s) : Pre s := by
  have := (pre_iff cx (s := s) hv rfl rfl).mp hs.1
  exact ⟨this.2.1, this.2.2, armOk hX this.2.1 this.2.2.2.2.1 hs.2.1, hs.2,
    RegionIx.pubFacts hX this.2.1 this.2.2.2.2.1 (by simp [tables])⟩

theorem ptr_eq (s : State) : cx.ptr s = s.gpr .x0 := rfl

section Front

variable {s : State}

/-! ## What each state keeps -/

theorem mA : Mid scratch s (m1 s) (σA s) := mid_σA
theorem mc1 : Mid scratch s (m1 s) (c1 s) := mA.cpu1 (by decide)
theorem mc2 : Mid scratch s (m1 s) (c2 s) := mA.cpu2 (by decide)
theorem mf1 : Mid scratch s (m1 s) (f1 s) := mid_slab1 mc2 (by decide)
theorem mf2 : Mid scratch s (m1 s) (f2 s) := mid_slab2 mc2 (by decide) (by decide)
theorem mr1 : Mid scratch s (m1 s) (r1 s) := RegionIx.mid_r1 mf2 (by decide)
theorem mr2 : Mid scratch s (m1 s) (r2 s) := RegionIx.mid_r2 mf2 (by decide) (by decide)
theorem mr3 : Mid scratch s (m1 s) (r3 s) := RegionIx.mid_r3 mf2 (by decide) (by decide) (by decide)
theorem mr4 : Mid scratch s (m1 s) (r4 s) := RegionIx.mid_r4 mf2 (by decide) (by decide) (by decide)
theorem mr5 : Mid scratch s (m1 s) (r5 s) := RegionIx.mid_r5 mf2 (by decide) (by decide) (by decide)
theorem mτ : Mid scratch s (m1 s) (τ s) := mr5.setReg (by decide) _

/-! ## The front half's steps -/

theorem x12_eq (hp : Pre s) : s.gpr .x12 = s.syms SLABS + BitVec.ofNat 64 REGION_DISP :=
  hp.rdy.2.2.2.2 (.x12, _) (List.mem_singleton_self _)

theorem eval_null : eval (.cbz .x .x0) (c2 s) = some (n s == 0) := by
  rw [Flags.eval_cbz, RegUpd.read_x, mc2.gpr _ (by decide)]
  rfl

theorem eval_null_t (h0 : (n s).toNat = 0) : eval (.cbz .x .x0) (c2 s) = some true := by
  rw [eval_null, show n s = 0 from BitVec.eq_of_toNat_eq h0]
  rfl

theorem eval_null_f (h0 : ¬ (n s).toNat = 0) : eval (.cbz .x .x0) (c2 s) = some false := by
  rw [eval_null, beq_eq_false_iff_ne.mpr fun h => h0 (by rw [h]; rfl)]

theorem tNull (h0 : ¬ (n s).toNat = 0) : Through isa freeNull (c2 s) (c2 s) [c2 s] [] :=
  .skip (eval_null_f h0) <| .nil _

theorem xNull (h0 : (n s).toNat = 0) : Leaves isa freeNull (c2 s) (.done (c2 s)) [c2 s] [] 0 :=
  .take (eval_null_t h0)

theorem tSlab (hp : Pre s) : Through isa slabOf (c2 s) (f2 s) [c2 s, f1 s] [] :=
  slabT cpu2_x8 (by rw [mc2.gpr _ (by decide)]; exact hp.rdy.2.1)

theorem before (hp : Pre s) : RegionIx.Before s (f2 s) :=
  ⟨mf2.mem, mf2.rd, mf2.wr, by rw [mf2.gpr _ (by decide)]; exact hp.rdy.2.2.1, mf2.gpr _ (by decide)⟩

theorem idx_eq (hp : Pre s) : RegionIx.idx (f2 s) = cx.regionIndex s :=
  RegionIx.idx_eq (before hp) hX rfl hp.pub (ptr_eq s)

theorem sa_in (hp : Pre s) : InRegions ((f2 s).rd ++ (f2 s).wr) (RegionIx.sa (f2 s)) 4 :=
  RegionIx.sa_in (before hp) hX rfl hp.pub

theorem ba_in (hp : Pre s) : InRegions ((f2 s).rd ++ (f2 s).wr) (RegionIx.ba (f2 s)) 8 :=
  RegionIx.ba_in (before hp) hX rfl hp.pub

theorem ra_eq (hp : Pre s) : ra s = (cx.regionTable s).base + cx.regionIndex s := by
  rw [ra, idx_eq hp]
  rfl

/-- The region byte is readable, and apart from `rseq_cs`. -/
theorem region_ok (hp : Pre s) (hix : (cx.regionIndex s).toNat < REGION_COUNT) : ByteOk s (ra s) := by
  refine byte_ok hX hp.world hp.pre.2.2.1 (r := cx.regionTable s) (by simp [tables]) ?_
  rw [ra_eq hp, show cx.regionIndex s = BitVec.ofNat 64 (cx.regionIndex s).toNat by simp]
  apply Offset.contains_base
  · simp only [REGION_COUNT] at hix ⊢; omega
  · simp only [REGION_COUNT] at hix; omega

theorem ea_τ (hp : Pre s) : (r5 s).ea { base := .x12, off := .reg .x4 } = ra s := by
  rw [Access.ea_reg, mr5.gpr _ (by decide), x12_eq hp, RegionIx.r5_x4]
  rfl

theorem tByte (hp : Pre s) (hix : (cx.regionIndex s).toNat < REGION_COUNT) :
    Through isa freeByte (r5 s) (τ s) [r5 s] [⟨ra s, 1, false, (rb s).toNat⟩] :=
  .mem (exec_byte (region_ok hp hix) mr5 (ea_τ hp)) (ev_byte (t := .x5) (region_ok hp hix) mr5 (ea_τ hp)) <|
    .nil _

theorem cls_eq (hp : Pre s) (h0 : ¬ (n s).toNat = 0) (hix : (cx.regionIndex s).toNat < REGION_COUNT) :
    cx.cls? s = some (rb s).toNat := by
  simp only [Spec.CEntry.Ctx.cls?, ptr_eq, show (s.gpr .x0).toNat ≠ 0 from h0, hix, ne_eq,
    not_false_eq_true, and_self, ↓reduceIte, Spec.CEntry.Ctx.regionByte, Spec.CEntry.Ctx.byteAt,
    mem_c hX, rb, ra_eq hp]

theorem ready (hp : Pre s) : Ready s (rb s).toNat (τ s) where
  mem := mτ.mem
  rd := mτ.rd
  wr := mτ.wr
  x3 := by
    rw [τ, RegUpd.gpr_setReg_of_ne _ _ (by decide), RegionIx.r5_gpr (by decide) (by decide) (by decide),
      slab2_x3]
  x5 := by rw [τ, RegUpd.gpr_setReg_self, byte_ofNat]; rfl
  x15 := by rw [mτ.gpr _ (by decide)]; exact hp.rdy.2.2.2.1

/-! ## The ways out -/

def cpuEv (s : State) : Event := ⟨area s + 4, 4, false, (cpu32 s).toNat⟩
def shiftEv (s : State) : Event := ⟨RegionIx.sa (f2 s), 4, false, (RegionIx.sv (f2 s)).toNat⟩
def baseEv (s : State) : Event := ⟨RegionIx.ba (f2 s), 8, false, (RegionIx.bw (f2 s)).toNat⟩
def byteEv (s : State) : Event := ⟨ra s, 1, false, (rb s).toNat⟩

/-- No slab. -/
theorem xC (hp : Pre s) (hi : ¬ (cpu32 s).toNat < 8192) :
    Leaves isa body (σA s) (.exit "slow" (c2 s)) [σA s, c1 s, c2 s] [cpuEv s] 2 :=
  ((((((cpuX hp.arm hi).before _).before _).before _).before _)).before _

/-- Null. -/
theorem xN (hp : Pre s) (hi : (cpu32 s).toNat < 8192) (h0 : (n s).toNat = 0) :
    Leaves isa body (σA s) (.done (c2 s)) [σA s, c1 s, c2 s, c2 s] [cpuEv s] 3 :=
  ((((Leaves.after (cpuT hp.arm hi) (xNull h0)).before _).before _).before _).before _

theorem tTo (hp : Pre s) (hi : (cpu32 s).toNat < 8192) (h0 : ¬ (n s).toNat = 0) :
    Through isa (cpuC ++ freeNull ++ slabOf) (σA s) (f2 s)
      [σA s, c1 s, c2 s, c2 s, c2 s, f1 s] [cpuEv s] :=
  ((cpuT hp.arm hi).trans (tNull h0)).trans (tSlab hp)

/-- An index out of range. -/
theorem xR (hp : Pre s) (hi : (cpu32 s).toNat < 8192) (h0 : ¬ (n s).toNat = 0)
    (hix : ¬ (cx.regionIndex s).toNat < REGION_COUNT) :
    Leaves isa body (σA s) (.exit "slow" (r5 s))
      [σA s, c1 s, c2 s, c2 s, c2 s, f1 s, f2 s, r1 s, r2 s, r3 s, r4 s, r5 s]
      [cpuEv s, shiftEv s, baseEv s] 11 :=
  ((Leaves.after (tTo hp hi h0) (RegionIx.regionX (sa_in hp) (ba_in hp)
    (by rw [idx_eq hp]; exact hix))).before _).before _

theorem tHead (hp : Pre s) (hi : (cpu32 s).toNat < 8192) (h0 : ¬ (n s).toNat = 0)
    (hix : (cx.regionIndex s).toNat < REGION_COUNT) :
    Through isa (cpuC ++ freeNull ++ slabOf ++ regionIndex ++ freeByte) (σA s) (τ s)
      [σA s, c1 s, c2 s, c2 s, c2 s, f1 s, f2 s, r1 s, r2 s, r3 s, r4 s, r5 s, r5 s]
      [cpuEv s, shiftEv s, baseEv s, byteEv s] :=
  ((tTo hp hi h0).trans (RegionIx.regionT (sa_in hp) (ba_in hp) (by rw [idx_eq hp]; exact hix))).trans
    (tByte hp hix)

theorem pushOk' (hp : Pre s) (hi : (cpu32 s).toNat < 8192) (h0 : ¬ (n s).toNat = 0)
    (hix : (cx.regionIndex s).toNat < REGION_COUNT) : Tail.PushOk (τ s) :=
  pushOk hX (ready hp) (backFacts hX hp.world hp.pre (cls_eq hp h0 hix) ((inRange_c hX s).mpr hi))
    ((inRange_c hX s).mpr hi)

theorem wreg : Tail.WReg .x1 := ⟨by decide, by decide, by decide, by decide, by decide, by decide⟩
theorem nreg : Tail.NReg .x1 .x2 := ⟨by decide, by decide, by decide, by decide, by decide⟩

/-- No free slot to spare. -/
theorem xP (hp : Pre s) (hi : (cpu32 s).toNat < 8192) (h0 : ¬ (n s).toNat = 0)
    (hix : (cx.regionIndex s).toNat < REGION_COUNT) (ht : Tail.qtst (τ s) = 0) :
    Leaves isa body (σA s) (.exit "slow" (Tail.q4 .x1 .x2 (τ s)))
      [σA s, c1 s, c2 s, c2 s, c2 s, f1 s, f2 s, r1 s, r2 s, r3 s, r4 s, r5 s, r5 s,
        τ s, Tail.t1 .x1 (τ s), Tail.t2 .x1 (τ s), Tail.q3 .x1 .x2 (τ s), Tail.q4 .x1 .x2 (τ s)]
      [cpuEv s, shiftEv s, baseEv s, byteEv s, ⟨Tail.wa (τ s), 8, false, (Tail.wv (τ s)).toNat⟩,
        ⟨Tail.ma (τ s), 8, false, (Tail.mk (τ s)).toNat⟩] 17 :=
  Leaves.after (tHead hp hi h0 hix) (Tail.pushMiss wreg nreg (pushOk' hp hi h0 hix) ht)

/-- The commit. -/
theorem tP (hp : Pre s) (hi : (cpu32 s).toNat < 8192) (h0 : ¬ (n s).toNat = 0)
    (hix : (cx.regionIndex s).toNat < REGION_COUNT) (ht : ¬ Tail.qtst (τ s) = 0) :
    Through isa body (σA s) (Tail.q6 .x1 .x2 (τ s))
      [σA s, c1 s, c2 s, c2 s, c2 s, f1 s, f2 s, r1 s, r2 s, r3 s, r4 s, r5 s, r5 s,
        τ s, Tail.t1 .x1 (τ s), Tail.t2 .x1 (τ s), Tail.q3 .x1 .x2 (τ s), Tail.q4 .x1 .x2 (τ s),
        Tail.q4 .x1 .x2 (τ s), Tail.q5 .x1 .x2 (τ s)]
      [cpuEv s, shiftEv s, baseEv s, byteEv s, ⟨Tail.wa (τ s), 8, false, (Tail.wv (τ s)).toNat⟩,
        ⟨Tail.ma (τ s), 8, false, (Tail.mk (τ s)).toNat⟩,
        ⟨Tail.wv (τ s), 8, true, ((τ s).gpr .x0).toNat⟩,
        ⟨Tail.wa (τ s), 8, true, (Tail.wv (τ s) + 8).toNat⟩] :=
  (tHead hp hi h0 hix).trans (Tail.pushHit wreg nreg (pushOk' hp hi h0 hix) ht)

end Front

/-! ## The contract -/

section Contract

variable {s : State}

theorem post_eq (s' : State) : attempt.post s s' ↔
    ((cx.ptr s).toNat = 0 ∧ Unchanged [cx.csField s] (cx.mem s) s'.mem) ∨ cx.Pushed s (cx.ptr s) s' :=
  post_iff cx hv s s'

theorem exit_eq (t : String) (s' : State) : attempt.exit t s s' ↔
    [s'.gpr .x0] = [s.gpr .x0] ∧ (t = SLOW ∧ cx.PushMissed s (cx.ptr s) s') :=
  exit_iff cx hv t s s'

theorem unchanged_cs : Unchanged [cx.csField s] (cx.mem s) (m1 s) := by
  rw [csField_c hX, mem_c hX]; exact frame_m1

theorem abi_of {m : Mem} {p : State} (hm : Mid scratch s m p) : abiPreserved s p :=
  abiPreserved_of_mid (by decide) hm

theorem missed (hc : cx.pushResult s (cx.ptr s) = none) {p : State} (hm : Mid scratch s (m1 s) p) :
    attempt.exit "slow" s p :=
  (exit_eq _ _).mpr ⟨by rw [hm.gpr _ (by decide)], rfl, hc, by rw [hX.mem, hm.mem]; exact unchanged_cs⟩

theorem none_C (hi : ¬ (cpu32 s).toNat < 8192) : cx.pushResult s (cx.ptr s) = none := by
  have hi' : ¬ cx.InRange s := fun h => hi ((inRange_c hX s).mp h)
  simp only [Spec.CEntry.Ctx.pushResult, hi', ↓reduceIte]

theorem none_R (hix : ¬ (cx.regionIndex s).toNat < REGION_COUNT) :
    cx.pushResult s (cx.ptr s) = none := by
  have : cx.cls? s = none := by simp only [Spec.CEntry.Ctx.cls?, hix, and_false, ↓reduceIte]
  simp only [Spec.CEntry.Ctx.pushResult, this, Option.bind_none, ite_self]

theorem ptr_τ : (τ s).gpr .x0 = cx.ptr s := mτ.gpr _ (by decide)

theorem correct (hs : attempt.pre s) :
    (∃ s', Exec isa cFree s s' ∧ target.abiPreserved s s' ∧ attempt.post s s') ∨
    (∃ t s', ExecExit isa cFree s t s' ∧ target.abiPreserved s s' ∧ attempt.exit t s s') := by
  have hp := pre_of hs
  rw [code_eq]
  by_cases hi : (cpu32 s).toNat < 8192
  · have hi' := (inRange_c hX s).mpr hi
    by_cases h0 : (n s).toNat = 0
    · exact .inl ⟨c2 s, .rseq (execBlock_arm hp.arm) (xN hp hi h0).run0, abi_of mc2,
        (post_eq _).mpr (.inl ⟨h0, by rw [mc2.mem]; exact unchanged_cs⟩)⟩
    by_cases hix : (cx.regionIndex s).toNat < REGION_COUNT
    · have hk := cls_eq hp h0 hix
      have hb := backFacts hX hp.world hp.pre hk hi'
      by_cases ht : Tail.qtst (τ s) = 0
      · have hm := Tail.mid_q4 (w := .x1) (n := .x2) (τ := τ s) mτ (by decide) (by decide) (by decide)
        exact .inr ⟨"slow", _, .rseq (execBlock_arm hp.arm) (xP hp hi h0 hix ht).run0, abi_of hm,
          missed (push_missed hX (ready hp) hb hk hi' ht _) hm⟩
      · have hm := Tail.mid_q6 (w := .x1) (n := .x2) (τ := τ s) mτ (by decide) (by decide) (by decide)
        exact .inl ⟨_, .rseq (execBlock_arm hp.arm) (tP hp hi h0 hix ht).run0, abi_of hm,
          (post_eq _).mpr (.inr (by rw [← ptr_τ]; exact pushed hX (ready hp) hb hk hi' ht))⟩
    · exact .inr ⟨"slow", r5 s, .rseq (execBlock_arm hp.arm) (xR hp hi h0 hix).run0, abi_of mr5,
        missed (none_R hix) mr5⟩
  · exact .inr ⟨"slow", c2 s, .rseq (execBlock_arm hp.arm) (xC hp hi).run0, abi_of mc2,
      missed (none_C hi) mc2⟩

/-! ### Where it may be aborted -/

theorem abort_eq (p : State) : attempt.abort s p ↔ [p.gpr .x0] = [s.gpr .x0] ∧
    (cx.PushAbort s p ∧ cx.inv p) :=
  abort_iff cx hv s p

/-- What must hold wherever it may be aborted. -/
abbrev AbortOk (s p : State) : Prop :=
  armed p ∧ target.abiPreserved s p ∧ attempt.abort s p ∧
    ∀ p', target.rely p p' → attempt.env p p' → attempt.pre p'

theorem abortOk (hp : Pre s) {p : State} (hm : Mid scratch s (m1 s) p) : AbortOk s p := by
  obtain ⟨hip, hre⟩ := retryC hX hv (post := fun s s' => _) (fun _ => rfl) (fun _ => rfl) (by decide)
    hp.pre hp.rdy hm (by decide) keeps frame_m1 (harmless_csC hX hp.pre)
  refine ⟨armed_of_mid hp.arm (xs := []) frame_m1 m1_cs (by simp) hm, abi_of hm, ?_, hre⟩
  exact (abort_eq p).mpr ⟨by rw [hm.gpr _ (by decide)], pushAbort_m1 hX hp.world hp.pre hm.mem, hip⟩

/-- After the free slot is written. -/
theorem abortOk_slot (hp : Pre s) (hi : cx.InRange s) {k : Nat} (hk : cx.cls? s = some k)
    (hr : Ready s k (τ s)) (ht : ¬ Tail.qtst (τ s) = 0) : AbortOk s (Tail.q5 .x1 .x2 (τ s)) := by
  have hb := backFacts hX hp.world hp.pre hk hi
  obtain ⟨j, hj⟩ := sized_of_qtest hX hr hb ht
  have hf := slotFacts hX hr hb hj hi ht
  have hm := Tail.mid_q5 (w := .x1) (n := .x2) (τ := τ s) mτ (by decide) (by decide) (by decide)
  have hfr : Frame [⟨area s + 8, 8⟩, ⟨cx.w s k, 8⟩] s.mem (Tail.qSlot (τ s)) := by
    rw [Tail.qSlot, hr.mem, wv_eq hX hr hb]; exact frame_two _ _ _ _ _
  have hcs : (Tail.qSlot (τ s)).readW (area s + 8) 64 = s.csDesc := by
    rw [Tail.qSlot, hr.mem, wv_eq hX hr hb, readW64_writeW_disj hf.cs.symm, m1_cs]
  have hW : ∀ w ∈ [(⟨area s + 8, 8⟩ : Region), ⟨cx.w s k, 8⟩], Harmless cx s w := by
    intro w hw
    rcases List.mem_cons.mp hw with rfl | hw
    · rw [← csField_c hX]; exact harmless_cs hp.pre.2.2.1
    · rw [List.mem_singleton.mp hw]; exact harmless_mag hf.apart (sub_of_contains hf.inMag)
  obtain ⟨hip, hre⟩ := retryC hX hv (post := fun s s' => _) (fun _ => rfl) (fun _ => rfl) (by decide)
    hp.pre hp.rdy hm (by decide) keeps hfr hW
  refine ⟨armed_of_mid hp.arm (xs := [⟨cx.w s k, 8⟩]) hfr hcs (by simpa using hf.rsVar) hm, abi_of hm,
    ?_, hre⟩
  exact (abort_eq _).mpr ⟨by rw [hm.gpr _ (by decide)],
    pushAbort_slot hX hp.world hp.pre hr hk hi ht hm.mem, hip⟩

theorem aborts (hp : Pre s) : ∀ p ∈ Code.abortStates isa (.rseq arm body) s, AbortOk s p := by
  simp only [Code.abortStates, execBlock_arm hp.arm]
  have a {p : State} (hm : Mid scratch s (m1 s) p) : AbortOk s p := abortOk hp hm
  by_cases hi : (cpu32 s).toNat < 8192
  · by_cases h0 : (n s).toNat = 0
    · rw [(xN hp hi h0).pre0]
      simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
      exact ⟨a mA, a mc1, a mc2, a mc2⟩
    by_cases hix : (cx.regionIndex s).toNat < REGION_COUNT
    · by_cases ht : Tail.qtst (τ s) = 0
      · rw [(xP hp hi h0 hix ht).pre0]
        simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
        exact ⟨a mA, a mc1, a mc2, a mc2, a mc2, a mf1, a mf2, a mr1, a mr2, a mr3, a mr4, a mr5, a mr5,
          a mτ, a (Tail.mid_t1 mτ (by decide)), a (Tail.mid_t2 mτ (by decide) (by decide)),
          a (Tail.mid_q3 mτ (by decide) (by decide) (by decide)),
          a (Tail.mid_q4 mτ (by decide) (by decide) (by decide))⟩
      · rw [(tP hp hi h0 hix ht).pre0]
        simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
        exact ⟨a mA, a mc1, a mc2, a mc2, a mc2, a mf1, a mf2, a mr1, a mr2, a mr3, a mr4, a mr5, a mr5,
          a mτ, a (Tail.mid_t1 mτ (by decide)), a (Tail.mid_t2 mτ (by decide) (by decide)),
          a (Tail.mid_q3 mτ (by decide) (by decide) (by decide)),
          a (Tail.mid_q4 mτ (by decide) (by decide) (by decide)),
          a (Tail.mid_q4 mτ (by decide) (by decide) (by decide)),
          abortOk_slot hp ((inRange_c hX s).mpr hi) (cls_eq hp h0 hix) (ready hp) ht⟩
    · rw [(xR hp hi h0 hix).pre0]
      simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
      exact ⟨a mA, a mc1, a mc2, a mc2, a mc2, a mf1, a mf2, a mr1, a mr2, a mr3, a mr4, a mr5⟩
  · rw [(xC hp hi).pre0]
    simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
    exact ⟨a mA, a mc1, a mc2⟩

/-! ### Its accesses -/

theorem sa_eq (hp : Pre s) : RegionIx.sa (f2 s) = (cx.published s).base :=
  RegionIx.sa_eq (before hp) hX rfl

theorem ba_eq (hp : Pre s) : RegionIx.ba (f2 s) = (cx.published s).base + 8 :=
  RegionIx.ba_eq (before hp) hX rfl

/-- The index, as the trace states it from the values read. -/
theorem idx_vals (hp : Pre s) : (cx.ptr s >>> ((RegionIx.sv (f2 s)).toNat % 64)) -
    RegionIx.bw (f2 s) = cx.regionIndex s := by
  rw [← idx_eq hp, RegionIx.idx, RegionIx.sp, (before hp).x0]
  rfl

theorem trace_ok (hs : attempt.pre s) : attempt.trace s (Code.events isa cFree s) := by
  have hp := pre_of hs
  show cx.FreeTrace s (cx.slab.attemptEvents s (Code.events isa (.rseq arm body) s))
  simp only [Code.events, execBlock_arm hp.arm, events_arm hp.arm, attemptEvents_eq (c := cx.slab) hX]
  by_cases hi : (cpu32 s).toNat < 8192
  · have hi' : (cpu32 s).toNat < SLAB_COUNT := hi
    by_cases h0 : (n s).toNat = 0
    · refine ⟨(o s).toNat, s.csDesc.toNat, (cpu32 s).toNat, 0, 0, 0, 0, 0, ?_⟩
      have h0' : (cx.ptr s).toNat = 0 := h0
      rw [head_eq hX, (xN hp hi h0).ev0]
      simp only [h0', ne_eq, not_true_eq_false, and_false, ↓reduceIte, List.append_nil]
      rfl
    have h0' : (cx.ptr s).toNat ≠ 0 := h0
    refine ⟨(o s).toNat, s.csDesc.toNat, (cpu32 s).toNat, (RegionIx.sv (f2 s)).toNat,
      (RegionIx.bw (f2 s)).toNat, (rb s).toNat, (Tail.wv (τ s)).toNat, (Tail.mk (τ s)).toNat, ?_⟩
    rw [head_eq hX]
    simp only [Spec.CEntry.Ctx.regionReads, ofNat_toNat64, idx_vals hp, hi', h0', ne_eq,
      not_false_eq_true, and_self, ↓reduceIte]
    by_cases hix : (cx.regionIndex s).toNat < REGION_COUNT
    · have hr := ready hp
      have hra : (cx.regionTable s).base + cx.regionIndex s = ra s := (ra_eq hp).symm
      by_cases ht : Tail.qtst (τ s) = 0
      · rw [(xP hp hi h0 hix ht).ev0, pushTail_miss hX hr ht]
        simp only [hix, ↓reduceIte, hra, Spec.CEntry.Ctx.rd, sa_eq hp, ba_eq hp, cpuEv, shiftEv, baseEv,
          byteEv, List.cons_append, List.nil_append]
        rfl
      · rw [(tP hp hi h0 hix ht).ev0, pushTail_hit hX hr ht, ptr_τ]
        simp only [hix, ↓reduceIte, hra, Spec.CEntry.Ctx.rd, sa_eq hp, ba_eq hp, cpuEv, shiftEv, baseEv,
          byteEv, List.cons_append, List.nil_append]
        rfl
    · rw [(xR hp hi h0 hix).ev0]
      simp only [hix, ↓reduceIte, List.append_nil, Spec.CEntry.Ctx.rd, sa_eq hp, ba_eq hp, cpuEv,
        shiftEv, baseEv, List.cons_append, List.nil_append]
      rfl
  · have hi' : ¬ (cpu32 s).toNat < SLAB_COUNT := hi
    refine ⟨(o s).toNat, s.csDesc.toNat, (cpu32 s).toNat, 0, 0, 0, 0, 0, ?_⟩
    rw [head_eq hX, (xC hp hi).ev0]
    simp only [hi', false_and, ↓reduceIte, List.append_nil]
    rfl

theorem rseqOk (hs : attempt.pre s) : RseqOk target cFree attempt s := by
  have hp := pre_of hs
  have ht := trace_ok hs
  rw [code_eq] at ht ⊢
  exact ⟨ht, ⟨_, _, rfl, execBlock_dropLast_arm, arms_arm hp.arm⟩, aborts hp⟩

/-! ### The setup -/

theorem setupOk {l : List Instr} {g : State → State} (hl : AreaSetup l g) :
    SetupOk target contract attempt freeSig [] (freeTables ++ l) :=
  setupOk_of hl hX hv (fun _ => rfl) (fun _ => rfl) (by decide) (tbl := freeTables)
    (f := freeS) (rs := [.x12, .x13, .x14, .x15])
    (fun s h => by
      rw [← List.append_nil freeTables]
      exact exec_free h [])
    (fun s => (mid_base (mid_refl s) (by decide) (by decide) (by decide)).setReg (by decide) _)
    (fun s => by rw [freeS_base _ (by decide), baseS_x13])
    (fun s => by rw [freeS_base _ (by decide), baseS_x14])
    (fun s => by rw [freeS_base _ (by decide), baseS_x15])
    (fun s x hx => by
      rw [List.mem_singleton.mp hx]
      exact freeS_x12 s)
    (by decide) (by decide) (by decide)

end Contract

/-! ## The witnesses

The shift 12 and base index `0x6000` in the mirror at `0x3000400`, so that the pointer `0x6000123` is in
region 0, whose byte in the region table at `0x3001800` is the class 1. The magazine at `0x4000` has 4
slots; a word of `0x4008` (slot 1) leaves room to push. -/

def wsOf (cpu word : BitVec 64) : List (Addr × BitVec 64) :=
  [(0x2000, 0x100), (0x10104, cpu), (0x1000008, word), (0x4008, 0x5000), (0x3000808, 16),
    (0x3000400, 12), (0x3000408, 0x6000), (0x3001800, 1)]

def witF (cpu ptr word : BitVec 64) (wr : List Region) : State :=
  witS "" ptr 0 0 0x3001800 (wsOf cpu word)
    [⟨0x2000, 8⟩, ⟨0x10104, 4⟩, ⟨0x3000800, 2048⟩, ⟨0x3000400, 16⟩, ⟨0x3001800, 4096⟩] wr

def w0 : State := witF 8192 0x6000123 0 [⟨0x10108, 8⟩]
def w1 : State := witF 0 0 0 [⟨0x10108, 8⟩]
def w2 : State := witF 0 0x7fff00000000 0 [⟨0x10108, 8⟩]
def w3 : State := witF 0 0x6000123 0 [⟨0x10108, 8⟩, ⟨0x1000008, 8⟩]
def w4 : State := witF 0 0x6000123 0x4008 [⟨0x10108, 8⟩, ⟨0x1000008, 8⟩, ⟨0x4000, 32⟩]

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
  ⟨pre_of_check hX (ws := wsOf 0 0x4008) rfl rfl hv (fun _ => rfl) (fun _ => rfl) (by decide +kernel),
    ⟨by decide +kernel, by decide +kernel⟩, by decide +kernel, by decide +kernel, by decide +kernel,
    by decide +kernel⟩

theorem outcomes_eq : cFree.outcomes = [(0, true), (1, true), (2, true), (3, true), (4, true)] := rfl

theorem visits_w0 : Visits isa cFree w0 0 true := by
  have hp := pre_of pre_w0
  exact Visits.rseq (execBlock_arm hp.arm) (xC hp (by decide +kernel)).stop0

theorem visits_w1 : Visits isa cFree w1 1 true := by
  have hp := pre_of pre_w1
  exact Visits.rseq (execBlock_arm hp.arm) (xN hp (by decide +kernel) (by decide +kernel)).stop0

theorem visits_w2 : Visits isa cFree w2 2 true := by
  have hp := pre_of pre_w2
  exact Visits.rseq (execBlock_arm hp.arm)
    (xR hp (by decide +kernel) (by decide +kernel) (by decide +kernel)).stop0

theorem visits_w3 : Visits isa cFree w3 3 true := by
  have hp := pre_of pre_w3
  exact Visits.rseq (execBlock_arm hp.arm)
    (xP hp (by decide +kernel) (by decide +kernel) (by decide +kernel) (by decide +kernel)).stop0

theorem visits_w4 : Visits isa cFree w4 4 true := by
  have hp := pre_of pre_w4
  exact Visits.rseq (execBlock_arm hp.arm)
    (tP hp (by decide +kernel) (by decide +kernel) (by decide +kernel) (by decide +kernel)).stop0

def witnesses : List State := [w0, w1, w2, w3, w4]

theorem witnessOk : (∃ w ∈ witnesses, attempt.pre w) ∧
    ∀ o ∈ cFree.outcomes, ∃ w ∈ witnesses, attempt.pre w ∧ Visits isa cFree w o.1 o.2 := by
  refine ⟨⟨w0, by simp [witnesses], pre_w0⟩, fun o ho => ?_⟩
  rw [outcomes_eq] at ho
  simp only [List.mem_cons, List.not_mem_nil, or_false] at ho
  rcases ho with rfl | rfl | rfl | rfl | rfl
  · exact ⟨w0, by simp [witnesses], pre_w0, visits_w0⟩
  · exact ⟨w1, by simp [witnesses], pre_w1, visits_w1⟩
  · exact ⟨w2, by simp [witnesses], pre_w2, visits_w2⟩
  · exact ⟨w3, by simp [witnesses], pre_w3, visits_w3⟩
  · exact ⟨w4, by simp [witnesses], pre_w4, visits_w4⟩

theorem verified : Verified target cFree attempt :=
  ⟨fun _ hs => correct hs, fun _ hs => rseqOk hs, ⟨_, pre_w0⟩, fun o ho => by
    obtain ⟨w, -, hw, hv⟩ := witnessOk.2 o ho
    exact ⟨w, hw, hv⟩⟩

/-- The last attempt, aborted any number of times, starts with the callee-saved registers, the stack
pointer and the argument of the first. -/
theorem restartsOk : ∀ s s', attempt.pre s → Restarts target cFree attempt s s' →
    target.abiPreserved s s' ∧ ArgsKept target.abi freeSig s s' ∧
    (target.inline = true → target.preservedExcept [] s s') :=
  restartsOk_target verified (by decide) fun s hs p hp r hr => by
    have h := ((abort_eq p).mp (aborts (pre_of hs) p hp).2.2.1).1
    have hr' : r ∈ [Reg.x0] := hr
    simp only [List.mem_cons, List.not_mem_nil, or_false] at hr'
    subst hr'
    simpa using h

end VG.Proof.CEntry.AArch64.Free
