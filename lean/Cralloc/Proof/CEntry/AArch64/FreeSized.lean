module

public import Cralloc.Proof.CEntry.AArch64.Region
public import Cralloc.Proof.CEntry.AArch64.Witness

/-!
# `c_free_sized` on AArch64

`Impl.CEntry.AArch64.cFreeSized` against `Spec.CEntry.freeSizedContract`: after the CPU number, the
CPU's slab, the class the size predicts (the byte of `C_SIZED_SIZES` at the size's low 16 bits), the
region index (`RegionIx`), the compare of the prediction with the region's byte, and the push's steps
with the word in `x9` and the new word in `x4`. Five ways out: no slab for the CPU, an index out of
range, a prediction the region does not confirm, no free slot (each a tail call of the slow path with
both arguments), and the commit.
-/

@[expose] public section

namespace VG.Proof.CEntry.AArch64.FreeSized

open VG.AArch64 VG.Spec.Slab VG.Spec.CEntry VG.Impl.Slab.AArch64 VG.Impl.CEntry.AArch64
  VG.Proof.Slab.AArch64 VG.Proof.CEntry VG.Proof.CEntry.AArch64 VG.Proof.Slab

abbrev cx : Spec.CEntry.Ctx isa := ⟨abi, .freeSized, freeSizedSig, .c⟩

theorem hX : A64 cx.A argRegs := a64_abi

abbrev contract : Contract isa := freeSizedContract abi .c

/-- What this routine's setup forms beyond what every routine's does: the region table and its own. -/
def extra : Held :=
  [(.x12, fun y => y SLABS + BitVec.ofNat 64 REGION_DISP), (.x2, fun y => y SIZED + BitVec.ofInt 64 2048)]

/-- The contract of an attempt. -/
abbrev attempt : Contract isa := withReadyC extra contract

/-- The registers an attempt writes. -/
def scratch : List Reg := [.x3, .x4, .x5, .x6, .x7, .x8, .x9]

theorem keeps : Keeps scratch extra :=
  ⟨by decide, by decide, by decide, by decide, by decide, by decide⟩

def body : List (Step Instr Cond) :=
  cpuC ++ slabOf ++ sizedPredict ++ regionIndex ++ sizedConfirm ++ pushSteps .x9 .x4

theorem code_eq : cFreeSized = .rseq arm body := rfl

/-! ## The front half's states -/

abbrev c1 (s : State) : State := cpu1 (σA s) s
abbrev c2 (s : State) : State := cpu2 (σA s) s
abbrev f1 (s : State) : State := slab1 (c2 s) s
abbrev f2 (s : State) : State := slab2 (c2 s) s
/-- The size's low 16 bits, the index of the prediction. -/
def sz (s : State) : BitVec 64 := s.gpr .x1 &&& BitVec.ofNat 64 0xffff
def k1 (s : State) : State := (f2 s).setReg .x9 (sz s)
/-- The prediction's address, and the predicted class byte. -/
def pa (s : State) : Addr := s.syms SIZED + BitVec.ofInt 64 2048 + sz s
def pb (s : State) : Byte := s.mem (pa s)
def k2 (s : State) : State := (k1 s).setReg .x5 ((s.mem (pa s)).setWidth 64)
abbrev r1 (s : State) : State := RegionIx.r1 (k2 s)
abbrev r2 (s : State) : State := RegionIx.r2 (k2 s)
abbrev r3 (s : State) : State := RegionIx.r3 (k2 s)
abbrev r4 (s : State) : State := RegionIx.r4 (k2 s)
abbrev r5 (s : State) : State := RegionIx.r5 (k2 s)
/-- The region byte's address, and the byte. -/
def ra (s : State) : Addr := s.syms SLABS + BitVec.ofNat 64 REGION_DISP + RegionIx.idx (k2 s)
def rb (s : State) : Byte := s.mem (ra s)
def k3 (s : State) : State := (r5 s).setReg .x7 ((s.mem (ra s)).setWidth 64)
/-- A byte, as `cmp` reads it from the register it was loaded into. -/
def low (b : Byte) : BitVec 32 := (b.setWidth 64).setWidth 32
/-- The state the push's steps start in. -/
def k4 (s : State) : State := (k3 s).addFlags (low (pb s)) (~~~(low (rb s))) true

/-! ## What the precondition gives -/

structure Pre (s : State) : Prop where
  world : cx.World s
  pre : cx.pre s
  arm : ArmOk s
  rdy : readyC extra s
  pub : RegionIx.PubFacts cx s

theorem hv : cx.A.args ((cx.sig.words cx.A.ptrBits).map (·.bits cx.A.ptrBits)) =
    some (fun s => [s.gpr .x0, s.gpr .x1]) := rfl

theorem pre_of {s : State} (hs : attempt.pre s) : Pre s := by
  have := (pre_iff cx (s := s) hv rfl rfl).mp hs.1
  exact ⟨this.2.1, this.2.2, armOk hX this.2.1 this.2.2.2.2.1 hs.2.1, hs.2,
    RegionIx.pubFacts hX this.2.1 this.2.2.2.2.1 (by simp [tables])⟩

theorem ptr_eq (s : State) : cx.ptr s = s.gpr .x0 := rfl
theorem size_eq (s : State) : cx.size s = s.gpr .x1 := rfl

section Front

variable {s : State}

/-! ## What each state keeps -/

theorem mA : Mid scratch s (m1 s) (σA s) := mid_σA
theorem mc1 : Mid scratch s (m1 s) (c1 s) := mA.cpu1 (by decide)
theorem mc2 : Mid scratch s (m1 s) (c2 s) := mA.cpu2 (by decide)
theorem mf1 : Mid scratch s (m1 s) (f1 s) := mid_slab1 mc2 (by decide)
theorem mf2 : Mid scratch s (m1 s) (f2 s) := mid_slab2 mc2 (by decide) (by decide)
theorem mk1 : Mid scratch s (m1 s) (k1 s) := mf2.setReg (by decide) _
theorem mk2 : Mid scratch s (m1 s) (k2 s) := mk1.setReg (by decide) _
theorem mr1 : Mid scratch s (m1 s) (r1 s) := RegionIx.mid_r1 mk2 (by decide)
theorem mr2 : Mid scratch s (m1 s) (r2 s) := RegionIx.mid_r2 mk2 (by decide) (by decide)
theorem mr3 : Mid scratch s (m1 s) (r3 s) := RegionIx.mid_r3 mk2 (by decide) (by decide) (by decide)
theorem mr4 : Mid scratch s (m1 s) (r4 s) := RegionIx.mid_r4 mk2 (by decide) (by decide) (by decide)
theorem mr5 : Mid scratch s (m1 s) (r5 s) := RegionIx.mid_r5 mk2 (by decide) (by decide) (by decide)
theorem mk3 : Mid scratch s (m1 s) (k3 s) := mr5.setReg (by decide) _
theorem mk4 : Mid scratch s (m1 s) (k4 s) := mk3.addFlags _ _ _

/-! ## The front half's steps -/

theorem x12_eq (hp : Pre s) : s.gpr .x12 = s.syms SLABS + BitVec.ofNat 64 REGION_DISP :=
  hp.rdy.2.2.2.2 (.x12, _) (List.mem_cons_self ..)

theorem x2_eq (hp : Pre s) : s.gpr .x2 = s.syms SIZED + BitVec.ofInt 64 2048 :=
  hp.rdy.2.2.2.2 (.x2, _) (List.mem_cons_of_mem _ (List.mem_singleton_self _))

theorem tSlab (hp : Pre s) : Through isa slabOf (c2 s) (f2 s) [c2 s, f1 s] [] :=
  slabT cpu2_x8 (by rw [mc2.gpr _ (by decide)]; exact hp.rdy.2.1)

theorem exec_k1 : exec (.and .x .x9 .x1 (.imm 0xffff)) (f2 s) = some (k1 s) := by
  show some ((f2 s).setReg .x9 ((f2 s).gpr .x1 &&& BitVec.ofNat 64 0xffff)) = _
  rw [mf2.gpr _ (by decide)]
  rfl

theorem sz_eq : sz s = BitVec.ofNat 64 ((cx.size s).toNat % SIZED_LEN) := by
  apply BitVec.eq_of_toNat_eq
  rw [sz, BitVec.toNat_and, show (BitVec.ofNat 64 0xffff).toNat = 2 ^ 16 - 1 from rfl,
    Nat.and_two_pow_sub_one_eq_mod, BitVec.toNat_ofNat, size_eq]
  simp only [SIZED_LEN]
  omega

theorem pa_eq : pa s = (cx.sizedTable s).base + BitVec.ofNat 64 ((cx.size s).toNat % SIZED_LEN) := by
  rw [pa, sz_eq]
  rfl

/-- The prediction is readable, and apart from `rseq_cs`. -/
theorem table_ok (hp : Pre s) : ByteOk s (pa s) := by
  refine byte_ok hX hp.world hp.pre.2.2.1 (r := cx.sizedTable s) (by simp [tables]) ?_
  rw [pa_eq]
  apply Offset.contains_base
  · simp only [SIZED_LEN]; omega
  · simp only [SIZED_LEN]; omega

theorem ea_k2 (hp : Pre s) : (k1 s).ea { base := .x2, off := .reg .x9 } = pa s := by
  rw [Access.ea_reg, mk1.gpr _ (by decide), x2_eq hp,
    show (k1 s).gpr .x9 = sz s from RegUpd.gpr_setReg_self _ _ _]
  rfl

theorem tPredict (hp : Pre s) :
    Through isa sizedPredict (f2 s) (k2 s) [f2 s, k1 s] [⟨pa s, 1, false, (pb s).toNat⟩] :=
  .reg exec_k1 rfl <|
    .mem (exec_byte (table_ok hp) mk1 (ea_k2 hp)) (ev_byte (t := .x5) (table_ok hp) mk1 (ea_k2 hp)) <|
    .nil _

theorem before (hp : Pre s) : RegionIx.Before s (k2 s) :=
  ⟨mk2.mem, mk2.rd, mk2.wr, by rw [mk2.gpr _ (by decide)]; exact hp.rdy.2.2.1, mk2.gpr _ (by decide)⟩

theorem idx_eq (hp : Pre s) : RegionIx.idx (k2 s) = cx.regionIndex s :=
  RegionIx.idx_eq (before hp) hX rfl hp.pub (ptr_eq s)

theorem sa_in (hp : Pre s) : InRegions ((k2 s).rd ++ (k2 s).wr) (RegionIx.sa (k2 s)) 4 :=
  RegionIx.sa_in (before hp) hX rfl hp.pub

theorem ba_in (hp : Pre s) : InRegions ((k2 s).rd ++ (k2 s).wr) (RegionIx.ba (k2 s)) 8 :=
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

theorem ea_k3 (hp : Pre s) : (r5 s).ea { base := .x12, off := .reg .x4 } = ra s := by
  rw [Access.ea_reg, mr5.gpr _ (by decide), x12_eq hp, RegionIx.r5_x4]
  rfl

theorem k3_x5 : (k3 s).gpr .x5 = (pb s).setWidth 64 := by
  rw [k3, RegUpd.gpr_setReg_of_ne _ _ (by decide), RegionIx.r5_gpr (by decide) (by decide) (by decide),
    k2, RegUpd.gpr_setReg_self]
  rfl

theorem k3_x7 : (k3 s).gpr .x7 = (rb s).setWidth 64 := RegUpd.gpr_setReg_self _ _ _

theorem exec_k4 : exec (.cmp .w .x5 (.reg .x7)) (k3 s) = some (k4 s) := by
  show some ((k3 s).addFlags (((k3 s).gpr .x5).setWidth 32) (~~~(((k3 s).gpr .x7).setWidth 32)) true) = _
  rw [k3_x5, k3_x7]
  rfl

theorem low_toNat (b : Byte) : (low b).toNat = b.toNat := by
  have := b.isLt
  simp only [low, BitVec.toNat_setWidth]
  omega

theorem eval_k4_t (h : ¬ pb s = rb s) : eval .ne (k4 s) = some true := by
  rw [k4, Flags.eval_ne_cmp]
  have hne : low (pb s) - low (rb s) ≠ 0 := fun h' => h (by
    have e := BitVec.sub_add_cancel (low (pb s)) (low (rb s))
    rw [h'] at e
    have e' : low (pb s) = low (rb s) := by simpa using e.symm
    apply BitVec.eq_of_toNat_eq
    rw [← low_toNat, ← low_toNat, e'])
  rw [beq_eq_false_iff_ne.mpr hne]
  rfl

theorem eval_k4_f (h : pb s = rb s) : eval .ne (k4 s) = some false := by
  rw [k4, Flags.eval_ne_cmp, h, BitVec.sub_self]
  rfl

theorem tConfirm (hp : Pre s) (hix : (cx.regionIndex s).toNat < REGION_COUNT) (h : pb s = rb s) :
    Through isa sizedConfirm (r5 s) (k4 s) [r5 s, k3 s, k4 s] [⟨ra s, 1, false, (rb s).toNat⟩] :=
  .mem (exec_byte (region_ok hp hix) mr5 (ea_k3 hp)) (ev_byte (t := .x7) (region_ok hp hix) mr5 (ea_k3 hp)) <|
    .reg exec_k4 rfl <| .skip (eval_k4_f h) <| .nil _

theorem xConfirm (hp : Pre s) (hix : (cx.regionIndex s).toNat < REGION_COUNT) (h : ¬ pb s = rb s) :
    Leaves isa sizedConfirm (r5 s) (.exit "slow" (k4 s)) [r5 s, k3 s, k4 s]
      [⟨ra s, 1, false, (rb s).toNat⟩] 2 :=
  .mem (exec_byte (region_ok hp hix) mr5 (ea_k3 hp)) (ev_byte (t := .x7) (region_ok hp hix) mr5 (ea_k3 hp)) <|
    .reg exec_k4 rfl <| .take (eval_k4_t h)

theorem regionByte_eq (hp : Pre s) : cx.regionByte s = (rb s).toNat := by
  simp only [Spec.CEntry.Ctx.regionByte, Spec.CEntry.Ctx.byteAt, mem_c hX, rb, ra_eq hp]

theorem predicted_eq : cx.predicted s = (pb s).toNat := by
  simp only [Spec.CEntry.Ctx.predicted, Spec.CEntry.Ctx.byteAt, mem_c hX, pb, pa_eq]

theorem cls_eq (hp : Pre s) (hix : (cx.regionIndex s).toNat < REGION_COUNT) (hm : pb s = rb s) :
    cx.cls? s = some (pb s).toNat := by
  simp only [Spec.CEntry.Ctx.cls?, hix, regionByte_eq hp, predicted_eq, hm, and_self, ↓reduceIte]

theorem ready (hp : Pre s) : Ready s (pb s).toNat (k4 s) where
  mem := mk4.mem
  rd := mk4.rd
  wr := mk4.wr
  x3 := by
    rw [k4, RegUpd.gpr_addFlags, k3, RegUpd.gpr_setReg_of_ne _ _ (by decide),
      RegionIx.r5_gpr (by decide) (by decide) (by decide), k2, RegUpd.gpr_setReg_of_ne _ _ (by decide), k1,
      RegUpd.gpr_setReg_of_ne _ _ (by decide), slab2_x3]
  x5 := by rw [k4, RegUpd.gpr_addFlags, k3_x5, byte_ofNat]
  x15 := by rw [mk4.gpr _ (by decide)]; exact hp.rdy.2.2.2.1

/-! ## The ways out -/

def cpuEv (s : State) : Event := ⟨area s + 4, 4, false, (cpu32 s).toNat⟩
def predEv (s : State) : Event := ⟨pa s, 1, false, (pb s).toNat⟩
def shiftEv (s : State) : Event := ⟨RegionIx.sa (k2 s), 4, false, (RegionIx.sv (k2 s)).toNat⟩
def baseEv (s : State) : Event := ⟨RegionIx.ba (k2 s), 8, false, (RegionIx.bw (k2 s)).toNat⟩
def byteEv (s : State) : Event := ⟨ra s, 1, false, (rb s).toNat⟩

/-- No slab. -/
theorem xC (hp : Pre s) (hi : ¬ (cpu32 s).toNat < 8192) :
    Leaves isa body (σA s) (.exit "slow" (c2 s)) [σA s, c1 s, c2 s] [cpuEv s] 2 :=
  ((((((cpuX hp.arm hi).before _).before _).before _).before _)).before _

theorem tTo (hp : Pre s) (hi : (cpu32 s).toNat < 8192) :
    Through isa (cpuC ++ slabOf ++ sizedPredict) (σA s) (k2 s)
      [σA s, c1 s, c2 s, c2 s, f1 s, f2 s, k1 s] [cpuEv s, predEv s] :=
  ((cpuT hp.arm hi).trans (tSlab hp)).trans (tPredict hp)

/-- An index out of range. -/
theorem xR (hp : Pre s) (hi : (cpu32 s).toNat < 8192) (hix : ¬ (cx.regionIndex s).toNat < REGION_COUNT) :
    Leaves isa body (σA s) (.exit "slow" (r5 s))
      [σA s, c1 s, c2 s, c2 s, f1 s, f2 s, k1 s, k2 s, r1 s, r2 s, r3 s, r4 s, r5 s]
      [cpuEv s, predEv s, shiftEv s, baseEv s] 12 :=
  ((Leaves.after (tTo hp hi) (RegionIx.regionX (sa_in hp) (ba_in hp)
    (by rw [idx_eq hp]; exact hix))).before _).before _

theorem tRegion (hp : Pre s) (hi : (cpu32 s).toNat < 8192) (hix : (cx.regionIndex s).toNat < REGION_COUNT) :
    Through isa (cpuC ++ slabOf ++ sizedPredict ++ regionIndex) (σA s) (r5 s)
      [σA s, c1 s, c2 s, c2 s, f1 s, f2 s, k1 s, k2 s, r1 s, r2 s, r3 s, r4 s, r5 s]
      [cpuEv s, predEv s, shiftEv s, baseEv s] :=
  (tTo hp hi).trans (RegionIx.regionT (sa_in hp) (ba_in hp) (by rw [idx_eq hp]; exact hix))

/-- A prediction the region does not confirm. -/
theorem xM (hp : Pre s) (hi : (cpu32 s).toNat < 8192) (hix : (cx.regionIndex s).toNat < REGION_COUNT)
    (hm : ¬ pb s = rb s) :
    Leaves isa body (σA s) (.exit "slow" (k4 s))
      [σA s, c1 s, c2 s, c2 s, f1 s, f2 s, k1 s, k2 s, r1 s, r2 s, r3 s, r4 s, r5 s, r5 s, k3 s, k4 s]
      [cpuEv s, predEv s, shiftEv s, baseEv s, byteEv s] 15 :=
  (Leaves.after (tRegion hp hi hix) (xConfirm hp hix hm)).before _

theorem tHead (hp : Pre s) (hi : (cpu32 s).toNat < 8192) (hix : (cx.regionIndex s).toNat < REGION_COUNT)
    (hm : pb s = rb s) :
    Through isa (cpuC ++ slabOf ++ sizedPredict ++ regionIndex ++ sizedConfirm) (σA s) (k4 s)
      [σA s, c1 s, c2 s, c2 s, f1 s, f2 s, k1 s, k2 s, r1 s, r2 s, r3 s, r4 s, r5 s, r5 s, k3 s, k4 s]
      [cpuEv s, predEv s, shiftEv s, baseEv s, byteEv s] :=
  (tRegion hp hi hix).trans (tConfirm hp hix hm)

theorem pushOk' (hp : Pre s) (hi : (cpu32 s).toNat < 8192) (hix : (cx.regionIndex s).toNat < REGION_COUNT)
    (hm : pb s = rb s) : Tail.PushOk (k4 s) :=
  pushOk hX (ready hp) (backFacts hX hp.world hp.pre (cls_eq hp hix hm) ((inRange_c hX s).mpr hi))
    ((inRange_c hX s).mpr hi)

theorem wreg : Tail.WReg .x9 := ⟨by decide, by decide, by decide, by decide, by decide, by decide⟩
theorem nreg : Tail.NReg .x9 .x4 := ⟨by decide, by decide, by decide, by decide, by decide⟩

/-- No free slot to spare. -/
theorem xP (hp : Pre s) (hi : (cpu32 s).toNat < 8192) (hix : (cx.regionIndex s).toNat < REGION_COUNT)
    (hm : pb s = rb s) (ht : Tail.qtst (k4 s) = 0) :
    Leaves isa body (σA s) (.exit "slow" (Tail.q4 .x9 .x4 (k4 s)))
      [σA s, c1 s, c2 s, c2 s, f1 s, f2 s, k1 s, k2 s, r1 s, r2 s, r3 s, r4 s, r5 s, r5 s, k3 s, k4 s,
        k4 s, Tail.t1 .x9 (k4 s), Tail.t2 .x9 (k4 s), Tail.q3 .x9 .x4 (k4 s), Tail.q4 .x9 .x4 (k4 s)]
      [cpuEv s, predEv s, shiftEv s, baseEv s, byteEv s,
        ⟨Tail.wa (k4 s), 8, false, (Tail.wv (k4 s)).toNat⟩,
        ⟨Tail.ma (k4 s), 8, false, (Tail.mk (k4 s)).toNat⟩] 20 :=
  Leaves.after (tHead hp hi hix hm) (Tail.pushMiss wreg nreg (pushOk' hp hi hix hm) ht)

/-- The commit. -/
theorem tP (hp : Pre s) (hi : (cpu32 s).toNat < 8192) (hix : (cx.regionIndex s).toNat < REGION_COUNT)
    (hm : pb s = rb s) (ht : ¬ Tail.qtst (k4 s) = 0) :
    Through isa body (σA s) (Tail.q6 .x9 .x4 (k4 s))
      [σA s, c1 s, c2 s, c2 s, f1 s, f2 s, k1 s, k2 s, r1 s, r2 s, r3 s, r4 s, r5 s, r5 s, k3 s, k4 s,
        k4 s, Tail.t1 .x9 (k4 s), Tail.t2 .x9 (k4 s), Tail.q3 .x9 .x4 (k4 s), Tail.q4 .x9 .x4 (k4 s),
        Tail.q4 .x9 .x4 (k4 s), Tail.q5 .x9 .x4 (k4 s)]
      [cpuEv s, predEv s, shiftEv s, baseEv s, byteEv s,
        ⟨Tail.wa (k4 s), 8, false, (Tail.wv (k4 s)).toNat⟩,
        ⟨Tail.ma (k4 s), 8, false, (Tail.mk (k4 s)).toNat⟩,
        ⟨Tail.wv (k4 s), 8, true, ((k4 s).gpr .x0).toNat⟩,
        ⟨Tail.wa (k4 s), 8, true, (Tail.wv (k4 s) + 8).toNat⟩] :=
  (tHead hp hi hix hm).trans (Tail.pushHit wreg nreg (pushOk' hp hi hix hm) ht)

end Front

/-! ## The contract -/

section Contract

variable {s : State}

theorem post_eq (s' : State) : attempt.post s s' ↔ cx.Pushed s (cx.ptr s) s' :=
  post_iff cx hv s s'

theorem exit_eq (t : String) (s' : State) : attempt.exit t s s' ↔
    [s'.gpr .x0, s'.gpr .x1] = [s.gpr .x0, s.gpr .x1] ∧ (t = SLOW ∧ cx.PushMissed s (cx.ptr s) s') :=
  exit_iff cx hv t s s'

theorem unchanged_cs : Unchanged [cx.csField s] (cx.mem s) (m1 s) := by
  rw [csField_c hX, mem_c hX]; exact frame_m1

theorem abi_of {m : Mem} {p : State} (hm : Mid scratch s m p) : abiPreserved s p :=
  abiPreserved_of_mid (by decide) hm

theorem missed (hc : cx.pushResult s (cx.ptr s) = none) {p : State} (hm : Mid scratch s (m1 s) p) :
    attempt.exit "slow" s p :=
  (exit_eq _ _).mpr ⟨by rw [hm.gpr .x0 (by decide), hm.gpr .x1 (by decide)], rfl, hc,
    by rw [hX.mem, hm.mem]; exact unchanged_cs⟩

theorem none_C (hi : ¬ (cpu32 s).toNat < 8192) : cx.pushResult s (cx.ptr s) = none := by
  have hi' : ¬ cx.InRange s := fun h => hi ((inRange_c hX s).mp h)
  simp only [Spec.CEntry.Ctx.pushResult, hi', ↓reduceIte]

theorem none_R (hix : ¬ (cx.regionIndex s).toNat < REGION_COUNT) :
    cx.pushResult s (cx.ptr s) = none := by
  have : cx.cls? s = none := by simp only [Spec.CEntry.Ctx.cls?, hix, false_and, ↓reduceIte]
  simp only [Spec.CEntry.Ctx.pushResult, this, Option.bind_none, ite_self]

theorem none_M (hp : Pre s) (hm : ¬ pb s = rb s) : cx.pushResult s (cx.ptr s) = none := by
  have hne : ¬ cx.regionByte s = cx.predicted s := by
    rw [regionByte_eq hp, predicted_eq]
    exact fun h => hm (BitVec.eq_of_toNat_eq h.symm)
  have : cx.cls? s = none := by simp only [Spec.CEntry.Ctx.cls?, hne, and_false, ↓reduceIte]
  simp only [Spec.CEntry.Ctx.pushResult, this, Option.bind_none, ite_self]

theorem ptr_k4 : (k4 s).gpr .x0 = cx.ptr s := mk4.gpr _ (by decide)

theorem correct (hs : attempt.pre s) :
    (∃ s', Exec isa cFreeSized s s' ∧ target.abiPreserved s s' ∧ attempt.post s s') ∨
    (∃ t s', ExecExit isa cFreeSized s t s' ∧ target.abiPreserved s s' ∧ attempt.exit t s s') := by
  have hp := pre_of hs
  rw [code_eq]
  by_cases hi : (cpu32 s).toNat < 8192
  · have hi' := (inRange_c hX s).mpr hi
    by_cases hix : (cx.regionIndex s).toNat < REGION_COUNT
    · by_cases hm : pb s = rb s
      · have hk := cls_eq hp hix hm
        have hb := backFacts hX hp.world hp.pre hk hi'
        by_cases ht : Tail.qtst (k4 s) = 0
        · have hq := Tail.mid_q4 (w := .x9) (n := .x4) (τ := k4 s) mk4 (by decide) (by decide) (by decide)
          exact .inr ⟨"slow", _, .rseq (execBlock_arm hp.arm) (xP hp hi hix hm ht).run0, abi_of hq,
            missed (push_missed hX (ready hp) hb hk hi' ht _) hq⟩
        · have hq := Tail.mid_q6 (w := .x9) (n := .x4) (τ := k4 s) mk4 (by decide) (by decide) (by decide)
          exact .inl ⟨_, .rseq (execBlock_arm hp.arm) (tP hp hi hix hm ht).run0, abi_of hq,
            (post_eq _).mpr (by rw [← ptr_k4]; exact pushed hX (ready hp) hb hk hi' ht)⟩
      · exact .inr ⟨"slow", k4 s, .rseq (execBlock_arm hp.arm) (xM hp hi hix hm).run0, abi_of mk4,
          missed (none_M hp hm) mk4⟩
    · exact .inr ⟨"slow", r5 s, .rseq (execBlock_arm hp.arm) (xR hp hi hix).run0, abi_of mr5,
        missed (none_R hix) mr5⟩
  · exact .inr ⟨"slow", c2 s, .rseq (execBlock_arm hp.arm) (xC hp hi).run0, abi_of mc2,
      missed (none_C hi) mc2⟩

/-! ### Where it may be aborted -/

theorem abort_eq (p : State) : attempt.abort s p ↔ [p.gpr .x0, p.gpr .x1] = [s.gpr .x0, s.gpr .x1] ∧
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
  exact (abort_eq p).mpr ⟨by rw [hm.gpr .x0 (by decide), hm.gpr .x1 (by decide)],
    pushAbort_m1 hX hp.world hp.pre hm.mem, hip⟩

/-- After the free slot is written. -/
theorem abortOk_slot (hp : Pre s) (hi : cx.InRange s) {k : Nat} (hk : cx.cls? s = some k)
    (hr : Ready s k (k4 s)) (ht : ¬ Tail.qtst (k4 s) = 0) : AbortOk s (Tail.q5 .x9 .x4 (k4 s)) := by
  have hb := backFacts hX hp.world hp.pre hk hi
  obtain ⟨j, hj⟩ := sized_of_qtest hX hr hb ht
  have hf := slotFacts hX hr hb hj hi ht
  have hm := Tail.mid_q5 (w := .x9) (n := .x4) (τ := k4 s) mk4 (by decide) (by decide) (by decide)
  have hfr : Frame [⟨area s + 8, 8⟩, ⟨cx.w s k, 8⟩] s.mem (Tail.qSlot (k4 s)) := by
    rw [Tail.qSlot, hr.mem, wv_eq hX hr hb]; exact frame_two _ _ _ _ _
  have hcs : (Tail.qSlot (k4 s)).readW (area s + 8) 64 = s.csDesc := by
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
  exact (abort_eq _).mpr ⟨by rw [hm.gpr .x0 (by decide), hm.gpr .x1 (by decide)],
    pushAbort_slot hX hp.world hp.pre hr hk hi ht hm.mem, hip⟩

theorem aborts (hp : Pre s) : ∀ p ∈ Code.abortStates isa (.rseq arm body) s, AbortOk s p := by
  simp only [Code.abortStates, execBlock_arm hp.arm]
  have a {p : State} (hm : Mid scratch s (m1 s) p) : AbortOk s p := abortOk hp hm
  by_cases hi : (cpu32 s).toNat < 8192
  · by_cases hix : (cx.regionIndex s).toNat < REGION_COUNT
    · by_cases hm : pb s = rb s
      · by_cases ht : Tail.qtst (k4 s) = 0
        · rw [(xP hp hi hix hm ht).pre0]
          simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
          exact ⟨a mA, a mc1, a mc2, a mc2, a mf1, a mf2, a mk1, a mk2, a mr1, a mr2, a mr3, a mr4, a mr5,
            a mr5, a mk3, a mk4, a mk4, a (Tail.mid_t1 mk4 (by decide)),
            a (Tail.mid_t2 mk4 (by decide) (by decide)),
            a (Tail.mid_q3 mk4 (by decide) (by decide) (by decide)),
            a (Tail.mid_q4 mk4 (by decide) (by decide) (by decide))⟩
        · rw [(tP hp hi hix hm ht).pre0]
          simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
          exact ⟨a mA, a mc1, a mc2, a mc2, a mf1, a mf2, a mk1, a mk2, a mr1, a mr2, a mr3, a mr4, a mr5,
            a mr5, a mk3, a mk4, a mk4, a (Tail.mid_t1 mk4 (by decide)),
            a (Tail.mid_t2 mk4 (by decide) (by decide)),
            a (Tail.mid_q3 mk4 (by decide) (by decide) (by decide)),
            a (Tail.mid_q4 mk4 (by decide) (by decide) (by decide)),
            a (Tail.mid_q4 mk4 (by decide) (by decide) (by decide)),
            abortOk_slot hp ((inRange_c hX s).mpr hi) (cls_eq hp hix hm) (ready hp) ht⟩
      · rw [(xM hp hi hix hm).pre0]
        simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
        exact ⟨a mA, a mc1, a mc2, a mc2, a mf1, a mf2, a mk1, a mk2, a mr1, a mr2, a mr3, a mr4, a mr5,
          a mr5, a mk3, a mk4⟩
    · rw [(xR hp hi hix).pre0]
      simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
      exact ⟨a mA, a mc1, a mc2, a mc2, a mf1, a mf2, a mk1, a mk2, a mr1, a mr2, a mr3, a mr4, a mr5⟩
  · rw [(xC hp hi).pre0]
    simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
    exact ⟨a mA, a mc1, a mc2⟩

/-! ### Its accesses -/

theorem sa_eq (hp : Pre s) : RegionIx.sa (k2 s) = (cx.published s).base :=
  RegionIx.sa_eq (before hp) hX rfl

theorem ba_eq (hp : Pre s) : RegionIx.ba (k2 s) = (cx.published s).base + 8 :=
  RegionIx.ba_eq (before hp) hX rfl

/-- The index, as the trace states it from the values read. -/
theorem idx_vals (hp : Pre s) : (cx.ptr s >>> ((RegionIx.sv (k2 s)).toNat % 64)) -
    RegionIx.bw (k2 s) = cx.regionIndex s := by
  rw [← idx_eq hp, RegionIx.idx, RegionIx.sp, (before hp).x0]
  rfl

theorem trace_ok (hs : attempt.pre s) : attempt.trace s (Code.events isa cFreeSized s) := by
  have hp := pre_of hs
  show cx.FreeSizedTrace s (cx.slab.attemptEvents s (Code.events isa (.rseq arm body) s))
  simp only [Code.events, execBlock_arm hp.arm, events_arm hp.arm, attemptEvents_eq (c := cx.slab) hX]
  by_cases hi : (cpu32 s).toNat < 8192
  · have hi' : (cpu32 s).toNat < SLAB_COUNT := hi
    refine ⟨(o s).toNat, s.csDesc.toNat, (cpu32 s).toNat, (pb s).toNat, (RegionIx.sv (k2 s)).toNat,
      (RegionIx.bw (k2 s)).toNat, (rb s).toNat, (Tail.wv (k4 s)).toNat, (Tail.mk (k4 s)).toNat, ?_⟩
    rw [head_eq hX]
    have hpa : (cx.sizedTable s).base + BitVec.ofNat 64 ((cx.size s).toNat % SIZED_LEN) = pa s := pa_eq.symm
    simp only [Spec.CEntry.Ctx.regionReads, ofNat_toNat64, idx_vals hp, hi', ↓reduceIte, hpa]
    by_cases hix : (cx.regionIndex s).toNat < REGION_COUNT
    · have hra : (cx.regionTable s).base + cx.regionIndex s = ra s := (ra_eq hp).symm
      by_cases hm : pb s = rb s
      · have hm' : (rb s).toNat = (pb s).toNat := by rw [hm]
        have hr := ready hp
        by_cases ht : Tail.qtst (k4 s) = 0
        · rw [(xP hp hi hix hm ht).ev0, pushTail_miss hX hr ht]
          simp only [hix, hm', and_self, ↓reduceIte, hra, Spec.CEntry.Ctx.rd, sa_eq hp, ba_eq hp, cpuEv,
            predEv, shiftEv, baseEv, byteEv, List.cons_append, List.nil_append]
          rfl
        · rw [(tP hp hi hix hm ht).ev0, pushTail_hit hX hr ht, ptr_k4]
          simp only [hix, hm', and_self, ↓reduceIte, hra, Spec.CEntry.Ctx.rd, sa_eq hp, ba_eq hp, cpuEv,
            predEv, shiftEv, baseEv, byteEv, List.cons_append, List.nil_append]
          rfl
      · have hm' : ¬ (rb s).toNat = (pb s).toNat := fun h => hm (BitVec.eq_of_toNat_eq h.symm)
        rw [(xM hp hi hix hm).ev0]
        simp only [hix, hm', and_false, ↓reduceIte, List.append_nil, hra, Spec.CEntry.Ctx.rd, sa_eq hp,
          ba_eq hp, cpuEv, predEv, shiftEv, baseEv, byteEv, List.cons_append, List.nil_append]
        rfl
    · rw [(xR hp hi hix).ev0]
      simp only [hix, false_and, ↓reduceIte, List.append_nil, Spec.CEntry.Ctx.rd, sa_eq hp, ba_eq hp,
        cpuEv, predEv, shiftEv, baseEv, List.cons_append, List.nil_append]
      rfl
  · have hi' : ¬ (cpu32 s).toNat < SLAB_COUNT := hi
    refine ⟨(o s).toNat, s.csDesc.toNat, (cpu32 s).toNat, 0, 0, 0, 0, 0, 0, ?_⟩
    rw [head_eq hX, (xC hp hi).ev0]
    simp only [hi', ↓reduceIte, List.append_nil]
    rfl

theorem rseqOk (hs : attempt.pre s) : RseqOk target cFreeSized attempt s := by
  have hp := pre_of hs
  have ht := trace_ok hs
  rw [code_eq] at ht ⊢
  exact ⟨ht, ⟨_, _, rfl, execBlock_dropLast_arm, arms_arm hp.arm⟩, aborts hp⟩

/-! ### The setup -/

theorem setupOk : SetupOk target contract attempt freeSizedSig [] freeSizedSetup :=
  setupOk_of hX hv (fun _ => rfl) (fun _ => rfl) (by decide) (tbl := freeSizedTables)
    (f := fun s => freeS (tabS .x2 SIZED s)) (rs := [.x2, .x12, .x13, .x14, .x15])
    (fun s h => by
      rw [freeSizedTables, exec_tab, ← List.append_nil freeTables]
      exact exec_free (s := tabS .x2 SIZED s) h [])
    (fun s => (mid_base (mid_tab (mid_refl s) (by decide)) (by decide) (by decide) (by decide)).setReg
      (by decide) _)
    (fun s => (freeS_base _ (by decide)).trans (baseS_x13 _))
    (fun s => (freeS_base _ (by decide)).trans (baseS_x14 _))
    (fun s => (freeS_base _ (by decide)).trans (baseS_x15 _))
    (fun s x hx => by
      simp only [extra, List.mem_cons, List.not_mem_nil, or_false] at hx
      rcases hx with rfl | rfl
      · exact freeS_x12 _
      · show (freeS (tabS .x2 SIZED s)).gpr .x2 = _
        rw [freeS_base _ (by decide), baseS_gpr _ (by decide) (by decide) (by decide), tabS_self])
    (by decide) (by decide) (by decide)

end Contract

/-! ## The witnesses

The memory of `c_free`'s, and `C_SIZED_SIZES` at `0x8000000`, its table 2048 bytes in, predicting the
class 1 for the size 8. -/

def wsOf (cpu word : BitVec 64) : List (Addr × BitVec 64) :=
  [(0x2000, 0x100), (0x10104, cpu), (0x1000008, word), (0x4008, 0x5000), (0x3000808, 16),
    (0x3000400, 12), (0x3000408, 0x6000), (0x3001800, 1), (0x8000808, 1)]

def witF (cpu ptr size word : BitVec 64) (wr : List Region) : State :=
  witS SIZED ptr size 0x8000800 0x3001800 (wsOf cpu word)
    [⟨0x2000, 8⟩, ⟨0x10104, 4⟩, ⟨0x3000800, 2048⟩, ⟨0x8000800, 65536⟩, ⟨0x3000400, 16⟩,
      ⟨0x3001800, 4096⟩] wr

def w0 : State := witF 8192 0x6000123 8 0 [⟨0x10108, 8⟩]
def w1 : State := witF 0 0x7fff00000000 8 0 [⟨0x10108, 8⟩]
def w2 : State := witF 0 0x6000123 16 0 [⟨0x10108, 8⟩]
def w3 : State := witF 0 0x6000123 8 0 [⟨0x10108, 8⟩, ⟨0x1000008, 8⟩]
def w4 : State := witF 0 0x6000123 8 0x4008 [⟨0x10108, 8⟩, ⟨0x1000008, 8⟩, ⟨0x4000, 32⟩]

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

theorem outcomes_eq : cFreeSized.outcomes = [(0, true), (1, true), (2, true), (3, true), (4, true)] := rfl

theorem visits_w0 : Visits isa cFreeSized w0 0 true := by
  have hp := pre_of pre_w0
  exact Visits.rseq (execBlock_arm hp.arm) (xC hp (by decide +kernel)).stop0

theorem visits_w1 : Visits isa cFreeSized w1 1 true := by
  have hp := pre_of pre_w1
  exact Visits.rseq (execBlock_arm hp.arm) (xR hp (by decide +kernel) (by decide +kernel)).stop0

theorem visits_w2 : Visits isa cFreeSized w2 2 true := by
  have hp := pre_of pre_w2
  exact Visits.rseq (execBlock_arm hp.arm)
    (xM hp (by decide +kernel) (by decide +kernel) (by decide +kernel)).stop0

theorem visits_w3 : Visits isa cFreeSized w3 3 true := by
  have hp := pre_of pre_w3
  exact Visits.rseq (execBlock_arm hp.arm)
    (xP hp (by decide +kernel) (by decide +kernel) (by decide +kernel) (by decide +kernel)).stop0

theorem visits_w4 : Visits isa cFreeSized w4 4 true := by
  have hp := pre_of pre_w4
  exact Visits.rseq (execBlock_arm hp.arm)
    (tP hp (by decide +kernel) (by decide +kernel) (by decide +kernel) (by decide +kernel)).stop0

def witnesses : List State := [w0, w1, w2, w3, w4]

theorem witnessOk : (∃ w ∈ witnesses, attempt.pre w) ∧
    ∀ o ∈ cFreeSized.outcomes, ∃ w ∈ witnesses, attempt.pre w ∧ Visits isa cFreeSized w o.1 o.2 := by
  refine ⟨⟨w0, by simp [witnesses], pre_w0⟩, fun o ho => ?_⟩
  rw [outcomes_eq] at ho
  simp only [List.mem_cons, List.not_mem_nil, or_false] at ho
  rcases ho with rfl | rfl | rfl | rfl | rfl
  · exact ⟨w0, by simp [witnesses], pre_w0, visits_w0⟩
  · exact ⟨w1, by simp [witnesses], pre_w1, visits_w1⟩
  · exact ⟨w2, by simp [witnesses], pre_w2, visits_w2⟩
  · exact ⟨w3, by simp [witnesses], pre_w3, visits_w3⟩
  · exact ⟨w4, by simp [witnesses], pre_w4, visits_w4⟩

theorem verified : Verified target cFreeSized attempt :=
  ⟨fun _ hs => correct hs, fun _ hs => rseqOk hs, ⟨_, pre_w0⟩, fun o ho => by
    obtain ⟨w, -, hw, hv⟩ := witnessOk.2 o ho
    exact ⟨w, hw, hv⟩⟩

/-- The last attempt, aborted any number of times, starts with the callee-saved registers, the stack
pointer and the arguments of the first. -/
theorem restartsOk : ∀ s s', attempt.pre s → Restarts target cFreeSized attempt s s' →
    target.abiPreserved s s' ∧ ArgsKept target.abi freeSizedSig s s' ∧
    (target.inline = true → target.preservedExcept [] s s') :=
  restartsOk_target verified (by decide) fun s hs p hp r hr => by
    have h := ((abort_eq p).mp (aborts (pre_of hs) p hp).2.2.1).1
    have hr' : r ∈ [Reg.x0, Reg.x1] := hr
    simp only [List.cons.injEq, and_true] at h
    simp only [List.mem_cons, List.not_mem_nil, or_false] at hr'
    rcases hr' with rfl | rfl
    · exact h.1
    · exact h.2

end VG.Proof.CEntry.AArch64.FreeSized
