module

public import Cralloc.Proof.CEntry.X86_64.Region

/-!
# `c_free_sized` on x86-64

`Impl.CEntry.X86_64.cFreeSized` against `Spec.CEntry.freeSizedContract`:
after the arm and the CPU number, the class the size predicts (the byte of
`C_SIZED_SIZES` at the size's low 16 bits), the region index (`RegionIx`),
the compare of the prediction with the region's byte, the class's word in
slab 0 in `r9`, and the push's steps. Five ways out: no slab for the CPU, an
index out of range, a prediction the region does not confirm, no free slot
(each a tail call of the slow path with both arguments), and the commit.
-/

@[expose] public section

namespace VG.Proof.CEntry.X86_64.FreeSized

open VG.X86_64 VG.Spec.Slab VG.Spec.CEntry VG.Impl.Slab.X86_64 VG.Impl.CEntry.X86_64
  VG.Proof.Slab.X86_64 VG.Proof.CEntry VG.Proof.Slab

variable {m : Mirror}

abbrev cx (m : Mirror) : Spec.CEntry.Ctx isa := ⟨abi, .freeSized, freeSizedSig, m⟩

theorem hX : X86 (cx m).A := x86_abi

def body (m : Mirror) : List (Step Instr Cond) :=
  .instr (.mov32 .r8 (.mem { base := .r10, disp := 4, seg := some .fs })) ::
  .instr (.alu32 .cmp .r8 (.imm 8192)) ::
  .exit .ae [] (some "slow") ::
  .instr (.shift .shl .r8 12) ::
  .instr (.leaSym .r9 "SLABS") ::
  .instr (.leaSym .rcx "C_SIZED_SIZES" 2048) ::
  .instr (.movzx16 .rax .rsi) ::
  .instr (.movzx8 .rdx { base := .rcx, index := some .rax }) ::
  .instr (.mov32 .rcx (.mem { base := .r9, disp := m.disp })) ::
  .instr (.mov .rax (.reg .rdi)) ::
  .instr (.shiftCl .shr .rax) ::
  .instr (.alu .sub .rax (.mem { base := .r9, disp := m.disp + 8 })) ::
  .instr (.alu .cmp .rax (.imm 4096)) ::
  .exit .ae [] (some "slow") ::
  .instr (.cmp8 .rdx { base := .r9, index := some .rax, disp := 33560576 }) ::
  .exit .ne [] (some "slow") ::
  .instr (.lea .r9 { base := .r9, index := some .rdx, scale := 8 }) ::
  pushSteps .r9 .rdi [] (some "slow")

theorem code_eq : (cFreeSized m) = .rseq arm (body m) := rfl

/-! ## The front half's states -/

def k1 (s : State) : State := (σ3 s).setReg .r9 ((σ3 s).syms "SLABS" + BitVec.ofInt 64 0 + 0)
def k2 (s : State) : State := (k1 s).setReg .rcx ((k1 s).syms "C_SIZED_SIZES" + BitVec.ofInt 64 2048 + 0)
/-- The size's low 16 bits, the index of the prediction. -/
def sz (s : State) : BitVec 64 := ((s.gpr .rsi).setWidth 16).setWidth 64
def k3 (s : State) : State := (k2 s).setReg .rax (((k2 s).gpr .rsi).setWidth 16 |>.setWidth 64)
/-- The prediction's address, and the predicted class byte. -/
def pa (s : State) : Addr := s.syms "C_SIZED_SIZES" + 2048 + sz s
def pb (s : State) : Byte := s.mem (pa s)
def k4 (s : State) : State := (k3 s).setReg .rdx ((pb s).setWidth 64)
def ρ5 (m : Mirror) (s : State) : State := RegionIx.r5 m (k4 s)
/-- The region byte's address, and the byte. -/
def ra (m : Mirror) (s : State) : Addr := (ρ5 m s).ea { base := .r9, index := some .rax, disp := 33560576 }
def rb (m : Mirror) (s : State) : Byte := s.mem (ra m s)
def k6 (m : Mirror) (s : State) : State := execCmp8 (pb s) (rb m s) (ρ5 m s)
def k7 (m : Mirror) (s : State) : State :=
  (k6 m s).setReg .r9 ((k6 m s).offset { base := .r9, index := some .rdx, scale := 8 })

/-! ## What the precondition gives -/

structure Pre (m : Mirror) (s : State) : Prop where
  world : (cx m).World s
  pre : (cx m).pre s
  arm : ArmOk s
  pub : RegionIx.PubFacts (cx m) s

theorem hv : (cx m).A.args (((cx m).sig.words (cx m).A.ptrBits).map (·.bits (cx m).A.ptrBits)) =
    some (fun s => [s.gpr .rdi, s.gpr .rsi]) := rfl

theorem pre_of {s : State} (hs : (freeSizedContract abi m).pre s) : Pre m s := by
  have := (pre_iff (cx m) (s := s) hv rfl rfl).mp hs
  exact ⟨this.2.1, this.2.2, armOk hX this.2.1 this.2.2.2.2.1,
    RegionIx.pubFacts hX this.2.1 this.2.2.2.2.1 (by simp [tables])⟩

theorem ptr_eq (s : State) : (cx m).ptr s = s.gpr .rdi := rfl
theorem size_eq (s : State) : (cx m).size s = s.gpr .rsi := rfl

/-! ## The front half's steps -/

section Front

variable {s : State}

theorem add0 (x : BitVec 64) : x + 0 = x := BitVec.add_zero x

theorem exec_k1 : exec (.leaSym .r9 "SLABS") (σ3 s) = some (k1 s) := rfl
theorem exec_k2 : exec (.leaSym .rcx "C_SIZED_SIZES" 2048) (k1 s) = some (k2 s) := rfl
theorem exec_k3 : exec (.movzx16 .rax .rsi) (k2 s) = some (k3 s) := rfl

theorem k2_rsi : (k2 s).gpr .rsi = s.gpr .rsi := by
  simp only [k2, k1, RegUpd.gpr_setReg_of_ne _ _ (show Reg.rsi ≠ .rcx by decide),
    RegUpd.gpr_setReg_of_ne _ _ (show Reg.rsi ≠ .r9 by decide)]
  exact gpr_σ3 (by decide) (by decide) (by decide)

theorem k3_rcx : (k3 s).gpr .rcx = s.syms "C_SIZED_SIZES" + 2048 := by
  rw [k3, RegUpd.gpr_setReg_of_ne _ _ (by decide), k2, RegUpd.gpr_setReg_self, BitVec.ofInt_ofNat, add0]
  rfl

theorem k3_rax : (k3 s).gpr .rax = sz s := by
  rw [k3, RegUpd.gpr_setReg_self, k2_rsi]; rfl

theorem ea_k4 : (k3 s).ea { base := .rcx, index := some .rax } = pa s := by
  rw [Access.ea_of_seg_none rfl, Access.offset_index, k3_rcx, k3_rax]
  simp only [BitVec.ofInt_ofNat, BitVec.mul_one, BitVec.add_zero]; rfl

theorem sz_toNat : (sz s).toNat = ((cx m).size s).toNat % SIZED_LEN := by
  simp only [sz, BitVec.toNat_setWidth, size_eq (m := m), SIZED_LEN]
  omega

theorem sz_eq : sz s = BitVec.ofNat 64 (((cx m).size s).toNat % SIZED_LEN) := by
  apply BitVec.eq_of_toNat_eq
  rw [sz_toNat (m := m), BitVec.toNat_ofNat]
  simp only [SIZED_LEN]
  omega

theorem pa_eq : pa s = ((cx m).sizedTable s).base + BitVec.ofNat 64 (((cx m).size s).toNat % SIZED_LEN) := by
  rw [pa, sz_eq (m := m), Spec.CEntry.Ctx.sizedTable, sym_c hX]; rfl

/-- The prediction is readable, and apart from `rseq_cs`. -/
theorem table_facts (hp : Pre m s) :
    InRegions (s.rd ++ s.wr) (pa s) 1 ∧ Region.Disjoint ⟨pa s, 1⟩ ⟨area s + 8, 8⟩ := by
  have hm : ((cx m).sizedTable s, false) ∈ (cx m).fixed s := table_mem_fixed (by simp [tables])
  have hin : (cx m).sizedTable s ∈ s.rd :=
    mem_rd hX hp.world (x := ((cx m).sizedTable s, false)) (mem_fixed hm) rfl
  have hc : ((cx m).sizedTable s).Contains (pa s) 1 := by
    rw [pa_eq]
    apply Offset.contains_base
    · simp only [SIZED_LEN]; omega
    · simp only [SIZED_LEN]; omega
  have hd : ((cx m).csField s).Disjoint ((cx m).sizedTable s) :=
    cs_table hp.pre.2.2.1 (x := ((cx m).sizedTable s, false)) (by simp [tables])
  rw [csField_c hX] at hd
  exact ⟨(InRegions.of_mem hin hc).append_left, (hd.sub_right (sub_of_contains hc)).symm⟩

theorem pb_m1 (hp : Pre m s) : (m1 s) (pa s) = pb s :=
  Mem.write_apply ((table_facts hp).2.sep (Region.contains_self _ _) (Region.contains_self _ _)
    (pa s) (by simp))

theorem exec_k4 (hp : Pre m s) :
    exec (.movzx8 .rdx { base := .rcx, index := some .rax }) (k3 s) = some (k4 s) := by
  simp only [exec, ea_k4, Access.load8_eq (s := k3 s) (by exact (table_facts hp).1), Option.map_some]
  rw [show (k3 s).mem = m1 s from rfl, pb_m1 hp]; rfl

theorem ev_k4 (hp : Pre m s) :
    Instr.events (.movzx8 .rdx { base := .rcx, index := some .rax }) (k3 s) (k4 s) =
      [⟨pa s, 1, false, (pb s).toNat⟩] := by
  simp only [Instr.events, ea_k4, Mem.read, show (k3 s).mem = m1 s from rfl, pb_m1 hp]
  rw [BitVec.toNat_append]
  simp

theorem before : RegionIx.Before s (k4 s) where
  mem := rfl
  rd := rfl
  wr := rfl
  r9 := by
    simp only [k4, k3, k2, RegUpd.gpr_setReg_of_ne _ _ (show Reg.r9 ≠ .rdx by decide),
      RegUpd.gpr_setReg_of_ne _ _ (show Reg.r9 ≠ .rax by decide),
      RegUpd.gpr_setReg_of_ne _ _ (show Reg.r9 ≠ .rcx by decide), k1, RegUpd.gpr_setReg_self,
      BitVec.ofInt_ofNat, add0]
    simp only [BitVec.add_zero]; rfl
  rdi := by
    simp only [k4, k3, k2, k1, RegUpd.gpr_setReg_of_ne _ _ (show Reg.rdi ≠ .rdx by decide),
      RegUpd.gpr_setReg_of_ne _ _ (show Reg.rdi ≠ .rax by decide),
      RegUpd.gpr_setReg_of_ne _ _ (show Reg.rdi ≠ .rcx by decide),
      RegUpd.gpr_setReg_of_ne _ _ (show Reg.rdi ≠ .r9 by decide), σ3_rdi]

theorem idx_eq (hp : Pre m s) : RegionIx.idx m (k4 s) = (cx m).regionIndex s :=
  RegionIx.idx_eq (c := cx m) before hX hp.pub (ptr_eq (m := m) s)

theorem ρ5_r9 : (ρ5 m s).gpr .r9 = s.syms SLABS := by
  rw [ρ5, RegionIx.r5_gpr (by decide) (by decide), before.r9]

theorem ρ5_rdx : (ρ5 m s).gpr .rdx = (pb s).setWidth 64 := by
  rw [ρ5, RegionIx.r5_gpr (by decide) (by decide), k4, RegUpd.gpr_setReg_self]

theorem ea_k6 : (ρ5 m s).ea { base := .r9, index := some .rax, disp := 33560576 } = ra m s := rfl

theorem ra_eq (hp : Pre m s) : ra m s = ((cx m).regionTable s).base + (cx m).regionIndex s := by
  rw [ra, Access.ea_of_seg_none rfl, Access.offset_index, ρ5_r9, ρ5, RegionIx.r5_rax, idx_eq hp,
    Spec.CEntry.Ctx.regionTable, sym_c hX, BitVec.mul_one, BitVec.ofInt_ofNat, BitVec.add_assoc,
    BitVec.add_comm ((cx m).regionIndex s), ← BitVec.add_assoc]
  rfl

/-- The region byte is readable, and apart from `rseq_cs`. -/
theorem region_facts (hp : Pre m s) (hix : ((cx m).regionIndex s).toNat < REGION_COUNT) :
    InRegions (s.rd ++ s.wr) (ra m s) 1 ∧ Region.Disjoint ⟨ra m s, 1⟩ ⟨area s + 8, 8⟩ := by
  have hm : ((cx m).regionTable s, false) ∈ (cx m).fixed s := table_mem_fixed (by simp [tables])
  have hin : (cx m).regionTable s ∈ s.rd :=
    mem_rd hX hp.world (x := ((cx m).regionTable s, false)) (mem_fixed hm) rfl
  have hc : ((cx m).regionTable s).Contains (ra m s) 1 := by
    rw [ra_eq hp, show (cx m).regionIndex s = BitVec.ofNat 64 ((cx m).regionIndex s).toNat by simp]
    apply Offset.contains_base
    · simp only [REGION_COUNT] at hix ⊢; omega
    · simp only [REGION_COUNT] at hix; omega
  have hd : ((cx m).csField s).Disjoint ((cx m).regionTable s) :=
    cs_table hp.pre.2.2.1 (x := ((cx m).regionTable s, false)) (by simp [tables])
  rw [csField_c hX] at hd
  exact ⟨(InRegions.of_mem hin hc).append_left, (hd.sub_right (sub_of_contains hc)).symm⟩

/-- A byte apart from `rseq_cs` reads after the arm as on entry. -/
theorem m1_byte {a : Addr} (hd : Region.Disjoint ⟨a, 1⟩ ⟨area s + 8, 8⟩) : (m1 s) a = s.mem a :=
  Mem.write_apply (hd.sep (Region.contains_self _ _) (Region.contains_self _ _) a (by simp))

theorem rb_m1 (hp : Pre m s) (hix : ((cx m).regionIndex s).toNat < REGION_COUNT) : (m1 s) (ra m s) = rb m s :=
  m1_byte (region_facts hp hix).2

theorem k6_a : ((ρ5 m s).gpr .rdx).setWidth 8 = pb s := by
  rw [ρ5_rdx]; apply BitVec.eq_of_toNat_eq; simp

theorem exec_k6 (hp : Pre m s) (hix : ((cx m).regionIndex s).toNat < REGION_COUNT) :
    exec (.cmp8 .rdx { base := .r9, index := some .rax, disp := 33560576 }) (ρ5 m s) = some (k6 m s) := by
  simp only [exec, ea_k6, Access.load8_eq (s := ρ5 m s) (by exact (region_facts hp hix).1),
    Option.map_some]
  rw [show (ρ5 m s).mem = m1 s from rfl, rb_m1 hp hix, k6_a]; rfl

theorem ev_k6 (hp : Pre m s) (hix : ((cx m).regionIndex s).toNat < REGION_COUNT) :
    Instr.events (.cmp8 .rdx { base := .r9, index := some .rax, disp := 33560576 }) (ρ5 m s) (k6 m s) =
      [⟨ra m s, 1, false, (rb m s).toNat⟩] := by
  simp only [Instr.events, ea_k6, Mem.read, show (ρ5 m s).mem = m1 s from rfl, rb_m1 hp hix]
  rw [BitVec.toNat_append]
  simp

theorem eval_k6 : eval .ne (k6 m s) = some (!(pb s - rb m s == 0)) := rfl

theorem eval_k6_t (h : ¬ pb s = rb m s) : eval .ne (k6 m s) = some true := by
  have : ¬ pb s - rb m s = 0 := fun h' => h (by
    bv_omega)
  rw [eval_k6, show (pb s - rb m s == 0) = false from beq_eq_false_iff_ne.mpr this]; rfl

theorem eval_k6_f (h : pb s = rb m s) : eval .ne (k6 m s) = some false := by
  rw [eval_k6, h, BitVec.sub_self]; rfl

theorem exec_k7 : exec (.lea .r9 { base := .r9, index := some .rdx, scale := 8 }) (k6 m s) = some (k7 m s) :=
  rfl

theorem k6_gpr : (k6 m s).gpr = (ρ5 m s).gpr := RegUpd.gpr_arithFlags _ _ _ _

theorem k7_r9 : (k7 m s).gpr .r9 = s.syms SLABS + BitVec.ofNat 64 (8 * (pb s).toNat) := by
  rw [k7, RegUpd.gpr_setReg_self, Access.offset_index]
  have h9 : (k6 m s).gpr .r9 = s.syms SLABS := by
    rw [k6_gpr, ρ5_r9]
  have hd : (k6 m s).gpr .rdx = (pb s).setWidth 64 := by
    rw [k6_gpr, ρ5_rdx]
  rw [h9, hd]
  have e : (pb s).setWidth 64 * BitVec.ofNat 64 8 = BitVec.ofNat 64 (8 * (pb s).toNat) := by
    apply BitVec.eq_of_toNat_eq
    have := (pb s).isLt
    simp only [BitVec.toNat_mul, BitVec.toNat_setWidth, BitVec.toNat_ofNat]
    omega
  rw [e, BitVec.ofInt_ofNat]
  simp only [BitVec.add_zero]

theorem ready : Ready (cx m) s (pb s).toNat (k7 m s) where
  mem := rfl
  rd := rfl
  wr := rfl
  r8 := by
    simp only [k7, RegUpd.gpr_setReg_of_ne _ _ (show Reg.r8 ≠ .r9 by decide)]
    rw [k6_gpr, ρ5,
      RegionIx.r5_gpr (show Reg.r8 ≠ .rax by decide) (show Reg.r8 ≠ .rcx by decide)]
    simp only [k4, k3, k2, k1, RegUpd.gpr_setReg_of_ne _ _ (show Reg.r8 ≠ .rdx by decide),
      RegUpd.gpr_setReg_of_ne _ _ (show Reg.r8 ≠ .rax by decide),
      RegUpd.gpr_setReg_of_ne _ _ (show Reg.r8 ≠ .rcx by decide),
      RegUpd.gpr_setReg_of_ne _ _ (show Reg.r8 ≠ .r9 by decide), gpr_σ3_r8]
  r9 := k7_r9

theorem cls_eq (hp : Pre m s) (hix : ((cx m).regionIndex s).toNat < REGION_COUNT) (hm : pb s = rb m s) :
    (cx m).cls? s = some (pb s).toNat := by
  have hr : (cx m).regionByte s = (rb m s).toNat := by
    simp only [Spec.CEntry.Ctx.regionByte, Spec.CEntry.Ctx.byteAt, mem_c hX, rb, ra_eq hp]
  have hpr : (cx m).predicted s = (pb s).toNat := by
    simp only [Spec.CEntry.Ctx.predicted, Spec.CEntry.Ctx.byteAt, mem_c hX, pb, pa_eq (m := m)]
  simp only [Spec.CEntry.Ctx.cls?, hix, hr, hpr, hm, and_self, ↓reduceIte]

theorem none_M (hp : Pre m s) (hm : ¬ pb s = rb m s) : (cx m).pushResult s ((cx m).ptr s) = none := by
  have hr : (cx m).regionByte s = (rb m s).toNat := by
    simp only [Spec.CEntry.Ctx.regionByte, Spec.CEntry.Ctx.byteAt, mem_c hX, rb, ra_eq hp]
  have hpr : (cx m).predicted s = (pb s).toNat := by
    simp only [Spec.CEntry.Ctx.predicted, Spec.CEntry.Ctx.byteAt, mem_c hX, pb, pa_eq (m := m)]
  have hne : ¬ (cx m).regionByte s = (cx m).predicted s := by
    rw [hr, hpr]; exact fun h => hm (BitVec.eq_of_toNat_eq h.symm)
  have : (cx m).cls? s = none := by simp only [Spec.CEntry.Ctx.cls?, hne, and_false, ↓reduceIte]
  simp only [Spec.CEntry.Ctx.pushResult, this, Option.bind_none, ite_self]

theorem none_S (hix : ¬ ((cx m).regionIndex s).toNat < REGION_COUNT) :
    (cx m).pushResult s ((cx m).ptr s) = none := by
  have : (cx m).cls? s = none := by simp only [Spec.CEntry.Ctx.cls?, hix, false_and, ↓reduceIte]
  simp only [Spec.CEntry.Ctx.pushResult, this, Option.bind_none, ite_self]

theorem ptr_k7 : (k7 m s).gpr .rdi = (cx m).ptr s := by
  simp only [k7, RegUpd.gpr_setReg_of_ne _ _ (show Reg.rdi ≠ .r9 by decide)]
  rw [k6_gpr, ρ5,
    RegionIx.r5_gpr (show Reg.rdi ≠ .rax by decide) (show Reg.rdi ≠ .rcx by decide), before.rdi]; rfl

end Front

/-! ## The ways out -/

section Cases

variable {s : State}

theorem r5_k4 : RegionIx.r5 m (k4 s) = ρ5 m s := rfl

theorem eval_cpu_t (hi : ¬ (cx m).InRange s) : eval .ae (σ2 s) = some true := by
  rw [eval_cpu]; simp [show ¬ (cpu32 s).toNat < 8192 from fun h => hi ((inRange_c hX s).mpr h)]

theorem eval_cpu_f (hi : (cx m).InRange s) : eval .ae (σ2 s) = some false := by
  rw [eval_cpu]; simp [(inRange_c hX s).mp hi]

theorem eval_ix_t (hp : Pre m s) (hix : ¬ ((cx m).regionIndex s).toNat < REGION_COUNT) :
    eval .ae (ρ5 m s) = some true := RegionIx.eval_r5_t (by rw [idx_eq hp]; exact hix)

theorem eval_ix_f (hp : Pre m s) (hix : ((cx m).regionIndex s).toNat < REGION_COUNT) :
    eval .ae (ρ5 m s) = some false := RegionIx.eval_r5_f (by rw [idx_eq hp]; exact hix)

theorem sa_in (hp : Pre m s) : InRegions ((k4 s).rd ++ (k4 s).wr) (RegionIx.sa m (k4 s)) 4 :=
  RegionIx.sa_in (c := cx m) before hX hp.pub
theorem ba_in (hp : Pre m s) : InRegions ((k4 s).rd ++ (k4 s).wr) (RegionIx.ba m (k4 s)) 8 :=
  RegionIx.ba_in (c := cx m) before hX hp.pub

end Cases

section Cases2

variable {s : State}

theorem runBody_front (hp : Pre m s) (hi : (cx m).InRange s) (hix : ((cx m).regionIndex s).toNat < REGION_COUNT)
    (hm : pb s = rb m s) :
    runBody isa (body m) (σA s) = runBody isa (pushSteps .r9 .rdi [] (some "slow")) (k7 m s) := by
  rw [body, runBody_instr (exec_cpu hp.arm), runBody_instr exec_cmp, runBody_skip (eval_cpu_f hi),
    runBody_instr exec_shl_r8, runBody_instr exec_k1, runBody_instr exec_k2, runBody_instr exec_k3, runBody_instr (exec_k4 hp),
    runBody_instr (RegionIx.exec_r1 (sa_in hp)), runBody_instr RegionIx.exec_r2, runBody_instr RegionIx.exec_r3,
    runBody_instr (RegionIx.exec_r4 (ba_in hp)), runBody_instr RegionIx.exec_r5,
    runBody_skip (eval_ix_f hp hix), runBody_instr (exec_k6 hp hix), runBody_skip (eval_k6_f hm),
    runBody_instr exec_k7]

def frontStates (m : Mirror) (s : State) : List State :=
  [σA s, σ1 s, σ2 s, σ2 s, σ3 s, k1 s, k2 s, k3 s, k4 s, RegionIx.r1 m (k4 s), RegionIx.r2 m (k4 s),
    RegionIx.r3 m (k4 s), RegionIx.r4 m (k4 s), ρ5 m s, ρ5 m s, k6 m s, k6 m s]

theorem prefixes_front (hp : Pre m s) (hi : (cx m).InRange s) (hix : ((cx m).regionIndex s).toNat < REGION_COUNT)
    (hm : pb s = rb m s) :
    bodyPrefixes isa (body m) (σA s) = frontStates m s ++
      bodyPrefixes isa (pushSteps .r9 .rdi [] (some "slow")) (k7 m s) := by
  rw [body, bodyPrefixes_instr (exec_cpu hp.arm), bodyPrefixes_instr exec_cmp, bodyPrefixes_skip (eval_cpu_f hi),
    bodyPrefixes_instr exec_shl_r8, bodyPrefixes_instr exec_k1, bodyPrefixes_instr exec_k2, bodyPrefixes_instr exec_k3, bodyPrefixes_instr (exec_k4 hp),
    bodyPrefixes_instr (RegionIx.exec_r1 (sa_in hp)), bodyPrefixes_instr RegionIx.exec_r2, bodyPrefixes_instr RegionIx.exec_r3,
    bodyPrefixes_instr (RegionIx.exec_r4 (ba_in hp)), bodyPrefixes_instr RegionIx.exec_r5,
    bodyPrefixes_skip (eval_ix_f hp hix), bodyPrefixes_instr (exec_k6 hp hix),
    bodyPrefixes_skip (eval_k6_f hm), bodyPrefixes_instr exec_k7]
  rfl

theorem end_front (hp : Pre m s) (hi : (cx m).InRange s) (hix : ((cx m).regionIndex s).toNat < REGION_COUNT)
    (hm : pb s = rb m s) :
    bodyEnd isa (body m) (σA s) = (bodyEnd isa (pushSteps .r9 .rdi [] (some "slow")) (k7 m s)).map (· + 17) := by
  rw [body, bodyEnd_instr (exec_cpu hp.arm), bodyEnd_instr exec_cmp, bodyEnd_skip (eval_cpu_f hi),
    bodyEnd_instr exec_shl_r8, bodyEnd_instr exec_k1, bodyEnd_instr exec_k2, bodyEnd_instr exec_k3, bodyEnd_instr (exec_k4 hp),
    bodyEnd_instr (RegionIx.exec_r1 (sa_in hp)), bodyEnd_instr RegionIx.exec_r2, bodyEnd_instr RegionIx.exec_r3,
    bodyEnd_instr (RegionIx.exec_r4 (ba_in hp)), bodyEnd_instr RegionIx.exec_r5,
    bodyEnd_skip (eval_ix_f hp hix), bodyEnd_instr (exec_k6 hp hix), bodyEnd_skip (eval_k6_f hm),
    bodyEnd_instr exec_k7]
  simp only [Option.map_map]; rfl

theorem ev_cpu' :
    Instr.events (.mov32 .r8 (.mem { base := .r10, disp := 4, seg := some .fs })) (σA s) (σ1 s) =
      [⟨area s + 4, 4, false, (cpu32 s).toNat⟩] := by
  simp only [Instr.events, cpuOp_ea, σA_mem, toNat_read4, m1]
  rw [readW_writeW_disj (cpu_cs_disjoint s) (by decide)]; rfl

def pubEvents (m : Mirror) (s : State) : List Event :=
  [⟨RegionIx.sa m (k4 s), 4, false, (RegionIx.sv m (k4 s)).toNat⟩,
    ⟨RegionIx.ba m (k4 s), 8, false, (RegionIx.bw m (k4 s)).toNat⟩]

theorem events_front (hp : Pre m s) (hi : (cx m).InRange s) (hix : ((cx m).regionIndex s).toNat < REGION_COUNT)
    (hm : pb s = rb m s) :
    bodyEvents isa (body m) (σA s) = [⟨area s + 4, 4, false, (cpu32 s).toNat⟩, ⟨pa s, 1, false, (pb s).toNat⟩] ++
      pubEvents m s ++ [⟨ra m s, 1, false, (rb m s).toNat⟩] ++
      bodyEvents isa (pushSteps .r9 .rdi [] (some "slow")) (k7 m s) := by
  rw [body, bodyEvents_instr (exec_cpu hp.arm), bodyEvents_instr exec_cmp, bodyEvents_skip (eval_cpu_f hi),
    bodyEvents_instr exec_shl_r8, bodyEvents_instr exec_k1, bodyEvents_instr exec_k2, bodyEvents_instr exec_k3, bodyEvents_instr (exec_k4 hp),
    bodyEvents_instr (RegionIx.exec_r1 (sa_in hp)), bodyEvents_instr RegionIx.exec_r2, bodyEvents_instr RegionIx.exec_r3,
    bodyEvents_instr (RegionIx.exec_r4 (ba_in hp)), bodyEvents_instr RegionIx.exec_r5,
    bodyEvents_skip (eval_ix_f hp hix), bodyEvents_instr (exec_k6 hp hix),
    bodyEvents_skip (eval_k6_f hm), bodyEvents_instr exec_k7]
  simp only [ev_cpu', ev_k4 hp, RegionIx.ev_r1, RegionIx.ev_r4, r5_k4, ev_k6 hp hix, pubEvents]
  simp only [Instr.events]
  events_norm

/-! ### No slab -/

theorem runBody_C (hp : Pre m s) (hi : ¬ (cx m).InRange s) :
    runBody isa (body m) (σA s) = some (.exit "slow" (σ2 s)) := by
  rw [body, runBody_instr (exec_cpu hp.arm), runBody_instr exec_cmp,
    runBody_take (eval_cpu_t hi) execBlock_nil']

theorem prefixes_C (hp : Pre m s) (hi : ¬ (cx m).InRange s) :
    bodyPrefixes isa (body m) (σA s) = [σA s, σ1 s, σ2 s] := by
  rw [body, bodyPrefixes_instr (exec_cpu hp.arm), bodyPrefixes_instr exec_cmp,
    bodyPrefixes_take (eval_cpu_t hi)]

theorem end_C (hp : Pre m s) (hi : ¬ (cx m).InRange s) : bodyEnd isa (body m) (σA s) = some 2 := by
  rw [body, bodyEnd_instr (exec_cpu hp.arm), bodyEnd_instr exec_cmp, bodyEnd_take (eval_cpu_t hi)]; rfl

theorem events_C (hp : Pre m s) (hi : ¬ (cx m).InRange s) :
    bodyEvents isa (body m) (σA s) = [⟨area s + 4, 4, false, (cpu32 s).toNat⟩] := by
  rw [body, bodyEvents_instr (exec_cpu hp.arm), bodyEvents_instr exec_cmp,
    bodyEvents_take (eval_cpu_t hi)]
  simp only [isa, ev_cpu']; rfl

/-! ### An index out of range -/

theorem runBody_S (hp : Pre m s) (hi : (cx m).InRange s) (hix : ¬ ((cx m).regionIndex s).toNat < REGION_COUNT) :
    runBody isa (body m) (σA s) = some (.exit "slow" (ρ5 m s)) := by
  rw [body, runBody_instr (exec_cpu hp.arm), runBody_instr exec_cmp, runBody_skip (eval_cpu_f hi),
    runBody_instr exec_shl_r8, runBody_instr exec_k1, runBody_instr exec_k2, runBody_instr exec_k3, runBody_instr (exec_k4 hp),
    runBody_instr (RegionIx.exec_r1 (sa_in hp)), runBody_instr RegionIx.exec_r2, runBody_instr RegionIx.exec_r3,
    runBody_instr (RegionIx.exec_r4 (ba_in hp)), runBody_instr RegionIx.exec_r5, runBody_take (eval_ix_t hp hix) execBlock_nil']
  rfl

theorem prefixes_S (hp : Pre m s) (hi : (cx m).InRange s) (hix : ¬ ((cx m).regionIndex s).toNat < REGION_COUNT) :
    bodyPrefixes isa (body m) (σA s) = [σA s, σ1 s, σ2 s, σ2 s, σ3 s, k1 s, k2 s, k3 s, k4 s,
      RegionIx.r1 m (k4 s), RegionIx.r2 m (k4 s), RegionIx.r3 m (k4 s), RegionIx.r4 m (k4 s), ρ5 m s] := by
  rw [body, bodyPrefixes_instr (exec_cpu hp.arm), bodyPrefixes_instr exec_cmp, bodyPrefixes_skip (eval_cpu_f hi),
    bodyPrefixes_instr exec_shl_r8, bodyPrefixes_instr exec_k1, bodyPrefixes_instr exec_k2, bodyPrefixes_instr exec_k3, bodyPrefixes_instr (exec_k4 hp),
    bodyPrefixes_instr (RegionIx.exec_r1 (sa_in hp)), bodyPrefixes_instr RegionIx.exec_r2, bodyPrefixes_instr RegionIx.exec_r3,
    bodyPrefixes_instr (RegionIx.exec_r4 (ba_in hp)), bodyPrefixes_instr RegionIx.exec_r5, bodyPrefixes_take (eval_ix_t hp hix)]
  rfl

theorem end_S (hp : Pre m s) (hi : (cx m).InRange s) (hix : ¬ ((cx m).regionIndex s).toNat < REGION_COUNT) :
    bodyEnd isa (body m) (σA s) = some 13 := by
  rw [body, bodyEnd_instr (exec_cpu hp.arm), bodyEnd_instr exec_cmp, bodyEnd_skip (eval_cpu_f hi),
    bodyEnd_instr exec_shl_r8, bodyEnd_instr exec_k1, bodyEnd_instr exec_k2, bodyEnd_instr exec_k3, bodyEnd_instr (exec_k4 hp),
    bodyEnd_instr (RegionIx.exec_r1 (sa_in hp)), bodyEnd_instr RegionIx.exec_r2, bodyEnd_instr RegionIx.exec_r3,
    bodyEnd_instr (RegionIx.exec_r4 (ba_in hp)), bodyEnd_instr RegionIx.exec_r5, bodyEnd_take (eval_ix_t hp hix)]; rfl

theorem events_S (hp : Pre m s) (hi : (cx m).InRange s) (hix : ¬ ((cx m).regionIndex s).toNat < REGION_COUNT) :
    bodyEvents isa (body m) (σA s) =
      [⟨area s + 4, 4, false, (cpu32 s).toNat⟩, ⟨pa s, 1, false, (pb s).toNat⟩] ++ pubEvents m s := by
  rw [body, bodyEvents_instr (exec_cpu hp.arm), bodyEvents_instr exec_cmp, bodyEvents_skip (eval_cpu_f hi),
    bodyEvents_instr exec_shl_r8, bodyEvents_instr exec_k1, bodyEvents_instr exec_k2, bodyEvents_instr exec_k3, bodyEvents_instr (exec_k4 hp),
    bodyEvents_instr (RegionIx.exec_r1 (sa_in hp)), bodyEvents_instr RegionIx.exec_r2, bodyEvents_instr RegionIx.exec_r3,
    bodyEvents_instr (RegionIx.exec_r4 (ba_in hp)), bodyEvents_instr RegionIx.exec_r5, bodyEvents_take (eval_ix_t hp hix)]
  simp only [ev_cpu', ev_k4 hp, RegionIx.ev_r1, RegionIx.ev_r4, pubEvents]
  simp only [Instr.events, blockEvents]
  events_norm

/-! ### A prediction the region does not confirm -/

theorem runBody_M (hp : Pre m s) (hi : (cx m).InRange s) (hix : ((cx m).regionIndex s).toNat < REGION_COUNT)
    (hm : ¬ pb s = rb m s) : runBody isa (body m) (σA s) = some (.exit "slow" (k6 m s)) := by
  rw [body, runBody_instr (exec_cpu hp.arm), runBody_instr exec_cmp, runBody_skip (eval_cpu_f hi),
    runBody_instr exec_shl_r8, runBody_instr exec_k1, runBody_instr exec_k2, runBody_instr exec_k3, runBody_instr (exec_k4 hp),
    runBody_instr (RegionIx.exec_r1 (sa_in hp)), runBody_instr RegionIx.exec_r2, runBody_instr RegionIx.exec_r3,
    runBody_instr (RegionIx.exec_r4 (ba_in hp)), runBody_instr RegionIx.exec_r5, runBody_skip (eval_ix_f hp hix), runBody_instr (exec_k6 hp hix),
    runBody_take (eval_k6_t hm) execBlock_nil']

theorem prefixes_M (hp : Pre m s) (hi : (cx m).InRange s) (hix : ((cx m).regionIndex s).toNat < REGION_COUNT)
    (hm : ¬ pb s = rb m s) :
    bodyPrefixes isa (body m) (σA s) = [σA s, σ1 s, σ2 s, σ2 s, σ3 s, k1 s, k2 s, k3 s, k4 s,
      RegionIx.r1 m (k4 s), RegionIx.r2 m (k4 s), RegionIx.r3 m (k4 s), RegionIx.r4 m (k4 s), ρ5 m s, ρ5 m s,
      k6 m s] := by
  rw [body, bodyPrefixes_instr (exec_cpu hp.arm), bodyPrefixes_instr exec_cmp, bodyPrefixes_skip (eval_cpu_f hi),
    bodyPrefixes_instr exec_shl_r8, bodyPrefixes_instr exec_k1, bodyPrefixes_instr exec_k2, bodyPrefixes_instr exec_k3, bodyPrefixes_instr (exec_k4 hp),
    bodyPrefixes_instr (RegionIx.exec_r1 (sa_in hp)), bodyPrefixes_instr RegionIx.exec_r2, bodyPrefixes_instr RegionIx.exec_r3,
    bodyPrefixes_instr (RegionIx.exec_r4 (ba_in hp)), bodyPrefixes_instr RegionIx.exec_r5, bodyPrefixes_skip (eval_ix_f hp hix), bodyPrefixes_instr (exec_k6 hp hix),
    bodyPrefixes_take (eval_k6_t hm)]
  rfl

theorem end_M (hp : Pre m s) (hi : (cx m).InRange s) (hix : ((cx m).regionIndex s).toNat < REGION_COUNT)
    (hm : ¬ pb s = rb m s) : bodyEnd isa (body m) (σA s) = some 15 := by
  rw [body, bodyEnd_instr (exec_cpu hp.arm), bodyEnd_instr exec_cmp, bodyEnd_skip (eval_cpu_f hi),
    bodyEnd_instr exec_shl_r8, bodyEnd_instr exec_k1, bodyEnd_instr exec_k2, bodyEnd_instr exec_k3, bodyEnd_instr (exec_k4 hp),
    bodyEnd_instr (RegionIx.exec_r1 (sa_in hp)), bodyEnd_instr RegionIx.exec_r2, bodyEnd_instr RegionIx.exec_r3,
    bodyEnd_instr (RegionIx.exec_r4 (ba_in hp)), bodyEnd_instr RegionIx.exec_r5, bodyEnd_skip (eval_ix_f hp hix), bodyEnd_instr (exec_k6 hp hix),
    bodyEnd_take (eval_k6_t hm)]; rfl

theorem events_M (hp : Pre m s) (hi : (cx m).InRange s) (hix : ((cx m).regionIndex s).toNat < REGION_COUNT)
    (hm : ¬ pb s = rb m s) :
    bodyEvents isa (body m) (σA s) =
      [⟨area s + 4, 4, false, (cpu32 s).toNat⟩, ⟨pa s, 1, false, (pb s).toNat⟩] ++ pubEvents m s ++
        [⟨ra m s, 1, false, (rb m s).toNat⟩] := by
  rw [body, bodyEvents_instr (exec_cpu hp.arm), bodyEvents_instr exec_cmp, bodyEvents_skip (eval_cpu_f hi),
    bodyEvents_instr exec_shl_r8, bodyEvents_instr exec_k1, bodyEvents_instr exec_k2, bodyEvents_instr exec_k3, bodyEvents_instr (exec_k4 hp),
    bodyEvents_instr (RegionIx.exec_r1 (sa_in hp)), bodyEvents_instr RegionIx.exec_r2, bodyEvents_instr RegionIx.exec_r3,
    bodyEvents_instr (RegionIx.exec_r4 (ba_in hp)), bodyEvents_instr RegionIx.exec_r5, bodyEvents_skip (eval_ix_f hp hix), bodyEvents_instr (exec_k6 hp hix),
    bodyEvents_take (eval_k6_t hm)]
  simp only [ev_cpu', ev_k4 hp, RegionIx.ev_r1, RegionIx.ev_r4, r5_k4, ev_k6 hp hix, pubEvents]
  simp only [Instr.events, blockEvents]
  events_norm

end Cases2

/-! ## What each state keeps -/

section Mids

variable {s : State}

theorem mσ2 : Mid scratchC s (m1 s) (σ2 s) :=
  mid_σ2 (mid_σ1 (by decide) (mid_σA (by decide) (by decide)))
theorem mσ3 : Mid scratchC s (m1 s) (σ3 s) := mid_σ3 (by decide) mσ2
theorem mk1 : Mid scratchC s (m1 s) (k1 s) := mσ3.setReg (by decide) _
theorem mk2 : Mid scratchC s (m1 s) (k2 s) := mk1.setReg (by decide) _
theorem mk3 : Mid scratchC s (m1 s) (k3 s) := mk2.setReg (by decide) _
theorem mk4 : Mid scratchC s (m1 s) (k4 s) := mk3.setReg (by decide) _
theorem mr1 : Mid scratchC s (m1 s) (RegionIx.r1 m (k4 s)) := RegionIx.mid_r1 mk4 (by decide)
theorem mr2 : Mid scratchC s (m1 s) (RegionIx.r2 m (k4 s)) := RegionIx.mid_r2 mk4 (by decide) (by decide)
theorem mr3 : Mid scratchC s (m1 s) (RegionIx.r3 m (k4 s)) := RegionIx.mid_r3 mk4 (by decide) (by decide)
theorem mr4 : Mid scratchC s (m1 s) (RegionIx.r4 m (k4 s)) := RegionIx.mid_r4 mk4 (by decide) (by decide)
theorem mρ5 : Mid scratchC s (m1 s) (ρ5 m s) := RegionIx.mid_r5 mk4 (by decide) (by decide)
theorem mk6 : Mid scratchC s (m1 s) (k6 m s) := mρ5.arithFlags _ _ _
theorem mk7 : Mid scratchC s (m1 s) (k7 m s) := mk6.setReg (by decide) _

theorem mid_front : ∀ p ∈ frontStates m s, Mid scratchC s (m1 s) p := by
  simp only [frontStates, List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
  exact ⟨mid_σA (by decide) (by decide), mid_σ1 (by decide) (mid_σA (by decide) (by decide)), mσ2, mσ2,
    mσ3, mk1, mk2, mk3, mk4, mr1, mr2, mr3, mr4, mρ5, mρ5, mk6, mk6⟩

end Mids

/-! ## The contract -/

section Contract

variable {s : State}

theorem post_eq (s' : State) : (freeSizedContract abi m).post s s' ↔ (cx m).Pushed s ((cx m).ptr s) s' :=
  post_iff (cx m) hv s s'

theorem exit_eq (t : String) (s' : State) : (freeSizedContract abi m).exit t s s' ↔
    [s'.gpr .rdi, s'.gpr .rsi] = [s.gpr .rdi, s.gpr .rsi] ∧ (t = SLOW ∧ (cx m).PushMissed s ((cx m).ptr s) s') :=
  exit_iff (cx m) hv t s s'

theorem abi_m1 (hp : Pre m s) {p : State} (hm : Mid scratchC s (m1 s) p) : abiPreserved s p :=
  abiPreserved_of_mid frame_m1 (by
    have := disj_res hX hp.world (x := ((cx m).csField s, true)) (mem_fixed cs_mem_fixed)
    rw [csField_c hX] at this; simpa using this) (by decide) hm

theorem unchanged_cs : Unchanged [(cx m).csField s] ((cx m).mem s) (m1 s) := by
  rw [csField_c hX, mem_c hX]; exact frame_m1

theorem missed (hn : (cx m).pushResult s ((cx m).ptr s) = none) {p : State} (hm : Mid scratchC s (m1 s) p) :
    (freeSizedContract abi m).exit "slow" s p :=
  (exit_eq _ _).mpr ⟨by rw [hm.gpr .rdi (by decide), hm.gpr .rsi (by decide)], rfl, hn,
    by rw [hX.mem, hm.mem]; exact unchanged_cs⟩

theorem none_C (hi : ¬ (cx m).InRange s) : (cx m).pushResult s ((cx m).ptr s) = none := by
  simp only [Spec.CEntry.Ctx.pushResult, hi, ↓reduceIte]

theorem correct (hs : (freeSizedContract abi m).pre s) :
    (∃ s', Exec isa (cFreeSized m) s s' ∧ target.abiPreserved s s' ∧ (freeSizedContract abi m).post s s') ∨
    (∃ t s', ExecExit isa (cFreeSized m) s t s' ∧ target.abiPreserved s s' ∧
      (freeSizedContract abi m).exit t s s') := by
  have hp := pre_of hs
  rw [code_eq]
  by_cases hi : (cx m).InRange s
  · by_cases hix : ((cx m).regionIndex s).toNat < REGION_COUNT
    · by_cases hm : pb s = rb m s
      · have hk := cls_eq hp hix hm
        have hb := backFacts hX hp.world hp.pre hk hi
        have hr := ready (m := m) (s := s)
        have ho := pushOk hX hr hb hi
        by_cases ht : Tail.qtst .r9 (k7 m s) = 0
        · exact .inr ⟨"slow", _, .rseq (execBlock_arm hp.arm)
            (by rw [runBody_front hp hi hix hm, Tail.runPush_miss ho _ ht]),
            abi_m1 hp (Tail.mid_q3 (mk7 (m := m)) (by decide) (by decide)),
            missed (push_missed hX hr hb hk hi ht _) (Tail.mid_q3 (mk7 (m := m)) (by decide) (by decide))⟩
        · obtain ⟨j, hj⟩ := sized_of_qtest hX hr hb ht
          have hf := slotFacts hX hr hb hj hi ht
          have hm5 := Tail.mid_q5 (w := .r9) (s := s) (mk7 (m := m)) (by decide) (by decide) .rdi
          refine .inl ⟨_, .rseq (execBlock_arm hp.arm)
            (by rw [runBody_front hp hi hix hm, Tail.runPush_hit ho _ ht]), ?_,
            (post_eq _).mpr (by rw [← ptr_k7]; exact pushed hX hr hb hk hi ht)⟩
          refine abiPreserved_of_mid (xs := [⟨area s + 8, 8⟩, ⟨(cx m).w s (pb s).toNat, 8⟩,
            ⟨(cx m).word s (pb s).toNat, 8⟩]) ?_ ?_ (by decide) hm5
          · rw [Tail.qEnd, Tail.qSlot, hr.mem, wa_eq hX hr, wv_eq hX hr hb]
            exact frame_three s.mem _ _ _ _ _ _
          · simpa using ⟨hb.ret_cs, hf.ret, hb.ret_word⟩
      · exact .inr ⟨"slow", k6 m s, .rseq (execBlock_arm hp.arm) (runBody_M hp hi hix hm), abi_m1 hp mk6,
          missed (none_M hp hm) mk6⟩
    · exact .inr ⟨"slow", ρ5 m s, .rseq (execBlock_arm hp.arm) (runBody_S hp hi hix), abi_m1 hp mρ5,
        missed (none_S hix) mρ5⟩
  · exact .inr ⟨"slow", σ2 s, .rseq (execBlock_arm hp.arm) (runBody_C hp hi), abi_m1 hp mσ2,
      missed (none_C hi) mσ2⟩

/-! ### Where it may be aborted -/

theorem abort_eq (p : State) : (freeSizedContract abi m).abort s p ↔
    [p.gpr .rdi, p.gpr .rsi] = [s.gpr .rdi, s.gpr .rsi] ∧ ((cx m).PushAbort s p ∧ (cx m).inv p) :=
  abort_iff (cx m) hv s p

theorem abortOk (hp : Pre m s) {p : State} (hm : Mid scratchC s (m1 s) p) :
    armed p ∧ target.abiPreserved s p ∧ (freeSizedContract abi m).abort s p ∧
      ∀ p', target.rely p p' → (freeSizedContract abi m).env p p' → (freeSizedContract abi m).pre p' := by
  obtain ⟨hip, hre⟩ := retryC hX hv (post := fun s s' => _) (fun _ => rfl) (fun _ => rfl) (by cases m <;> decide)
    hp.pre hm (by cases m <;> decide) (by cases m <;> decide) frame_m1 (harmless_csC hX hp.pre)
  refine ⟨armed_of_mid hp.arm (xs := []) frame_m1 m1_cs (by simp) hm, abi_m1 hp hm, ?_, hre⟩
  exact (abort_eq p).mpr ⟨by rw [hm.gpr .rdi (by decide), hm.gpr .rsi (by decide)],
    pushAbort_m1 hX hp.world hp.pre hm.mem, hip⟩

theorem abortOk_slot (hp : Pre m s) (hi : (cx m).InRange s) {k : Nat} (hk : (cx m).cls? s = some k)
    (hr : Ready (cx m) s k (k7 m s)) (ht : ¬ Tail.qtst .r9 (k7 m s) = 0) :
    armed (Tail.q4 .r9 (k7 m s) .rdi) ∧ target.abiPreserved s (Tail.q4 .r9 (k7 m s) .rdi) ∧
      (freeSizedContract abi m).abort s (Tail.q4 .r9 (k7 m s) .rdi) ∧
      ∀ p', target.rely (Tail.q4 .r9 (k7 m s) .rdi) p' →
        (freeSizedContract abi m).env (Tail.q4 .r9 (k7 m s) .rdi) p' → (freeSizedContract abi m).pre p' := by
  have hb := backFacts hX hp.world hp.pre hk hi
  obtain ⟨j, hj⟩ := sized_of_qtest hX hr hb ht
  have hf := slotFacts hX hr hb hj hi ht
  have hm := Tail.mid_q4 (w := .r9) (s := s) (mk7 (m := m)) (by decide) (by decide) .rdi
  have hfr : Frame [⟨area s + 8, 8⟩, ⟨(cx m).w s k, 8⟩] s.mem (Tail.qSlot .r9 (k7 m s) .rdi) := by
    rw [Tail.qSlot, hr.mem, wv_eq hX hr hb]; exact frame_two _ _ _ _ _
  have hcs : (Tail.qSlot .r9 (k7 m s) .rdi).readW (area s + 8) 64 = s.csDesc := by
    rw [Tail.qSlot, hr.mem, wv_eq hX hr hb, readW64_writeW_disj hf.cs.symm, m1_cs]
  have hW : ∀ w ∈ [(⟨area s + 8, 8⟩ : Region), ⟨(cx m).w s k, 8⟩], Harmless (cx m) s w := by
    intro w hw
    rcases List.mem_cons.mp hw with rfl | hw
    · rw [← csField_c hX]; exact harmless_cs hp.pre.2.2.1
    · rw [List.mem_singleton.mp hw]; exact harmless_mag hf.apart (sub_of_contains hf.inMag)
  obtain ⟨hip, hre⟩ := retryC hX hv (post := fun s s' => _) (fun _ => rfl) (fun _ => rfl) (by cases m <;> decide)
    hp.pre hm (by cases m <;> decide) (by cases m <;> decide) hfr hW
  refine ⟨armed_of_mid hp.arm (xs := [⟨(cx m).w s k, 8⟩]) hfr hcs (by simpa using hf.rsVar) hm,
    abiPreserved_of_mid hfr (by simpa using ⟨hb.ret_cs, hf.ret⟩) (by decide) hm, ?_, hre⟩
  exact (abort_eq _).mpr ⟨by rw [hm.gpr .rdi (by decide), hm.gpr .rsi (by decide)],
    pushAbort_slot hX hp.world hp.pre hr hk hi ht hm.mem, hip⟩

/-! ### Its accesses -/

theorem ofNat_toNat64 (v : BitVec 64) : BitVec.ofNat 64 v.toNat = v := by
  rw [BitVec.ofNat_toNat, BitVec.setWidth_eq]

theorem addr8 : s.fsBase + o s + BitVec.ofNat 64 RSEQ_CS = area s + 8 := rfl
theorem addr4 : s.fsBase + o s + BitVec.ofNat 64 RSEQ_CPU_ID = area s + 4 := rfl
theorem rsName : RSEQ_OFFSET = RS := rfl

theorem pub_eq : pubEvents m s =
    [⟨((cx m).published s).base, 4, false, (RegionIx.sv m (k4 s)).toNat⟩,
      ⟨((cx m).published s).base + 8, 8, false, (RegionIx.bw m (k4 s)).toNat⟩] := by
  rw [pubEvents, RegionIx.sa_eq before hX, RegionIx.ba_eq before hX]

theorem idx_vals (hp : Pre m s) : ((cx m).ptr s >>> ((RegionIx.sv m (k4 s)).toNat % 64)) -
    RegionIx.bw m (k4 s) = (cx m).regionIndex s := by
  show s.gpr .rdi >>> _ - _ = _
  rw [← idx_eq hp, RegionIx.idx, RegionIx.sp_eq, before.rdi]

theorem trace_ok (hs : (freeSizedContract abi m).pre s) :
    (freeSizedContract abi m).trace s (Code.events isa (cFreeSized m) s) := by
  have hp := pre_of hs
  show (cx m).FreeSizedTrace s (Code.events isa (.rseq arm (body m)) s)
  simp only [Code.events, execBlock_arm hp.arm, events_arm hp.arm]
  have htp : (cx m).slab.tp s = s.fsBase := tp_of (c := (cx m).slab) (hX (m := m)) s
  by_cases hi : (cx m).InRange s
  · have hi' : (cpu32 s).toNat < SLAB_COUNT := (inRange_c hX s).mp hi
    refine ⟨(o s).toNat, s.csDesc.toNat, (cpu32 s).toNat, (pb s).toNat, (RegionIx.sv m (k4 s)).toNat,
      (RegionIx.bw m (k4 s)).toNat, (rb m s).toNat, (Tail.wv .r9 (k7 m s)).toNat, (Tail.mk .r9 (k7 m s)).toNat, ?_⟩
    simp only [Spec.CEntry.Ctx.regionReads, ofNat_toNat64, idx_vals hp, Spec.CEntry.Ctx.head,
      Spec.CEntry.Ctx.rd, Spec.CEntry.Ctx.wr, htp, sym_c hX, addr8, addr4, rsName, hi', ↓reduceIte,
      ← pa_eq]
    by_cases hix : ((cx m).regionIndex s).toNat < REGION_COUNT
    · have hra : ((cx m).regionTable s).base + (cx m).regionIndex s = ra m s := (ra_eq hp).symm
      by_cases hm : pb s = rb m s
      · have hk := cls_eq hp hix hm
        have hb := backFacts hX hp.world hp.pre hk hi
        have hr := ready (m := m) (s := s)
        have ho := pushOk hX hr hb hi
        have hmn : (rb m s).toNat = (pb s).toNat := by rw [hm]
        rw [events_front hp hi hix hm, pub_eq]
        have hword : wordAddr (s.syms SLABS) (cpu32 s).toNat (pb s).toNat = Tail.wa .r9 (k7 m s) := by
          rw [wa_eq hX hr]; simp only [Spec.CEntry.Ctx.word, sym_c hX, cpu_c hX]
        have hmask : s.syms SLABS + BitVec.ofNat 64 (8 * (pb s).toNat + MASK_DISP) = Tail.ma .r9 (k7 m s) := by
          rw [ma_eq hX hr, sym_c hX]
        have hq : BitVec.ofNat 32 ((Tail.wv .r9 (k7 m s)).toNat + 8) = (Tail.wv .r9 (k7 m s) + 8).setWidth 32 := by
          apply BitVec.eq_of_toNat_eq
          rw [BitVec.toNat_ofNat, BitVec.toNat_setWidth, add8_toNat_mod,
            Nat.mod_mod_of_dvd _ (by decide : 2 ^ 32 ∣ 2 ^ 64)]
        simp only [Spec.CEntry.Ctx.pushTail, hix, hmn, and_self, ↓reduceIte, hra, sym_c hX, hword, hmask, ptr_eq]
        by_cases ht : Tail.qtst .r9 (k7 m s) = 0
        · rw [Tail.eventsPush_miss ho _ ht]
          have : BitVec.ofNat 32 ((Tail.wv .r9 (k7 m s)).toNat + 8) &&& BitVec.ofNat 32 (Tail.mk .r9 (k7 m s)).toNat = 0 := by
            rw [hq, BitVec.ofNat_toNat, BitVec.setWidth_eq]; exact ht
          simp only [this, ne_eq, not_true_eq_false, ↓reduceIte, List.append_nil]
          events_norm
        · rw [Tail.eventsPush_hit ho _ ht]
          have : ¬ BitVec.ofNat 32 ((Tail.wv .r9 (k7 m s)).toNat + 8) &&& BitVec.ofNat 32 (Tail.mk .r9 (k7 m s)).toNat = 0 := by
            rw [hq, BitVec.ofNat_toNat, BitVec.setWidth_eq]; exact ht
          have hx : (k7 m s).gpr .rdi = s.gpr .rdi := ptr_k7
          simp only [this, ne_eq, not_false_eq_true, ↓reduceIte, ofNat_toNat64, hx]
          events_norm
      · have hmn : ¬ (rb m s).toNat = (pb s).toNat := fun h => hm (BitVec.eq_of_toNat_eq h.symm)
        rw [events_M hp hi hix hm, pub_eq]
        simp only [hix, hmn, and_false, ↓reduceIte, hra, List.append_nil]
        events_norm
    · rw [events_S hp hi hix, pub_eq]
      simp only [hix, false_and, ↓reduceIte, List.append_nil]
      events_norm
  · have hi' : ¬ (cpu32 s).toNat < SLAB_COUNT := fun h => hi ((inRange_c hX s).mpr h)
    refine ⟨(o s).toNat, s.csDesc.toNat, (cpu32 s).toNat, 0, 0, 0, 0, 0, 0, ?_⟩
    rw [events_C hp hi]
    simp only [Spec.CEntry.Ctx.head, Spec.CEntry.Ctx.rd, Spec.CEntry.Ctx.wr, htp, sym_c hX, addr8, addr4,
      rsName, ofNat_toNat64, hi', ↓reduceIte, List.append_nil]
    events_norm

theorem rseqOk (hs : (freeSizedContract abi m).pre s) :
    RseqOk target (cFreeSized m) (freeSizedContract abi m) s := by
  have hp := pre_of hs
  have ht := trace_ok hs
  rw [code_eq] at ht ⊢
  unfold RseqOk
  refine ⟨ht, ⟨_, _, rfl, execBlock_dropLast_arm hp.arm, arms_arm⟩, ?_⟩
  have mA : Mid scratchC s (m1 s) (σA s) := mid_σA (by decide) (by decide)
  have m1' : Mid scratchC s (m1 s) (σ1 s) := mid_σ1 (by decide) mA
  simp only [Code.abortStates, execBlock_arm hp.arm]
  by_cases hi : (cx m).InRange s
  · by_cases hix : ((cx m).regionIndex s).toNat < REGION_COUNT
    · by_cases hm : pb s = rb m s
      · have hk := cls_eq hp hix hm
        have hb := backFacts hX hp.world hp.pre hk hi
        have hr := ready (m := m) (s := s)
        have ho := pushOk hX hr hb hi
        rw [prefixes_front hp hi hix hm]
        intro p hp'
        rcases List.mem_append.mp hp' with hp' | hp'
        · exact abortOk hp (mid_front p hp')
        · by_cases ht : Tail.qtst .r9 (k7 m s) = 0
          · rw [Tail.prefixesPush_miss ho _ ht] at hp'
            simp only [List.mem_cons, List.not_mem_nil, or_false] at hp'
            rcases hp' with rfl | rfl | rfl | rfl
            · exact abortOk hp mk7
            · exact abortOk hp (Tail.mid_t1 (mk7 (m := m)) (by decide))
            · exact abortOk hp (Tail.mid_q2 (mk7 (m := m)) (by decide) (by decide))
            · exact abortOk hp (Tail.mid_q3 (mk7 (m := m)) (by decide) (by decide))
          · rw [Tail.prefixesPush_hit ho _ ht] at hp'
            simp only [List.mem_cons, List.not_mem_nil, or_false] at hp'
            rcases hp' with rfl | rfl | rfl | rfl | rfl | rfl
            · exact abortOk hp mk7
            · exact abortOk hp (Tail.mid_t1 (mk7 (m := m)) (by decide))
            · exact abortOk hp (Tail.mid_q2 (mk7 (m := m)) (by decide) (by decide))
            · exact abortOk hp (Tail.mid_q3 (mk7 (m := m)) (by decide) (by decide))
            · exact abortOk hp (Tail.mid_q3 (mk7 (m := m)) (by decide) (by decide))
            · exact abortOk_slot hp hi hk hr ht
      · rw [prefixes_M hp hi hix hm]
        simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
        exact ⟨abortOk hp mA, abortOk hp m1', abortOk hp mσ2, abortOk hp mσ2, abortOk hp mσ3,
          abortOk hp mk1, abortOk hp mk2, abortOk hp mk3, abortOk hp mk4, abortOk hp mr1, abortOk hp mr2,
          abortOk hp mr3, abortOk hp mr4, abortOk hp mρ5, abortOk hp mρ5, abortOk hp mk6⟩
    · rw [prefixes_S hp hi hix]
      simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
      exact ⟨abortOk hp mA, abortOk hp m1', abortOk hp mσ2, abortOk hp mσ2, abortOk hp mσ3,
        abortOk hp mk1, abortOk hp mk2, abortOk hp mk3, abortOk hp mk4, abortOk hp mr1, abortOk hp mr2,
        abortOk hp mr3, abortOk hp mr4, abortOk hp mρ5⟩
  · rw [prefixes_C hp hi]
    simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
    exact ⟨abortOk hp mA, abortOk hp m1', abortOk hp mσ2⟩

end Contract

end VG.Proof.CEntry.X86_64.FreeSized
