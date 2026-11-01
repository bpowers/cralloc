module

public import Cralloc.Proof.CEntry.X86_64.Region

/-!
# `c_free` on x86-64

`Impl.CEntry.X86_64.cFree` against `Spec.CEntry.freeContract`: after the arm
and the CPU number, a null pointer returns; otherwise the region index
(`RegionIx`), the region's class byte, the class's word in slab 0 in `r9`,
and the push's steps (`Proof/Slab/X86_64/Tail.lean`). Five ways out: no
slab for the CPU, null (a return), an index out of range, no free slot
(the three tail calls of the slow path), and the commit.
-/

@[expose] public section

namespace VG.Proof.CEntry.X86_64.Free

open VG.X86_64 VG.Spec.Slab VG.Spec.CEntry VG.Impl.Slab.X86_64 VG.Impl.CEntry.X86_64
  VG.Proof.Slab.X86_64 VG.Proof.CEntry VG.Proof.Slab

variable {m : Mirror}

abbrev cx (m : Mirror) : Spec.CEntry.Ctx isa := ⟨abi, .free, freeSig, m⟩

theorem hX : X86 (cx m).A := x86_abi

def body (m : Mirror) : List (Step Instr Cond) :=
  .instr (.mov32 .r8 (.mem { base := .r10, disp := 4, seg := some .fs })) ::
  .instr (.alu32 .cmp .r8 (.imm 8192)) ::
  .exit .ae [] (some "slow") ::
  .instr (.shift .shl .r8 12) ::
  .instr (.alu .test .rdi (.reg .rdi)) ::
  .exit .e [] none ::
  .instr (.leaSym .r9 "SLABS") ::
  .instr (.mov32 .rcx (.mem { base := .r9, disp := m.disp })) ::
  .instr (.mov .rax (.reg .rdi)) ::
  .instr (.shiftCl .shr .rax) ::
  .instr (.alu .sub .rax (.mem { base := .r9, disp := m.disp + 8 })) ::
  .instr (.alu .cmp .rax (.imm 4096)) ::
  .exit .ae [] (some "slow") ::
  .instr (.lea .rdx { base := .r9, disp := 33560576 }) ::
  .instr (.movzx8 .rdx { base := .rdx, index := some .rax }) ::
  .instr (.lea .r9 { base := .r9, index := some .rdx, scale := 8 }) ::
  pushSteps .r9 .rdi [] (some "slow")

theorem code_eq : (cFree m) = .rseq arm (body m) := rfl

/-! ## The front half's states -/

def ptr (s : State) : BitVec 64 := s.gpr .rdi

def h1 (s : State) : State := arithFlags (σ3 s) ((σ3 s).gpr .rdi &&& (σ3 s).gpr .rdi) false false
def h2 (s : State) : State := (h1 s).setReg .r9 ((h1 s).syms "SLABS" + BitVec.ofInt 64 0 + 0)
/-- After the region index. -/
def ρ5 (m : Mirror) (s : State) : State := RegionIx.r5 m (h2 s)
def h3 (m : Mirror) (s : State) : State := (ρ5 m s).setReg .rdx ((ρ5 m s).offset { base := .r9, disp := 33560576 })
/-- The region byte's address, and the byte. -/
def ra (m : Mirror) (s : State) : Addr := s.syms SLABS + 33560576 + RegionIx.idx m (h2 s)
def rb (m : Mirror) (s : State) : Byte := s.mem (ra m s)
def h4 (m : Mirror) (s : State) : State := (h3 m s).setReg .rdx ((rb m s).setWidth 64)
def h5 (m : Mirror) (s : State) : State :=
  (h4 m s).setReg .r9 ((h4 m s).offset { base := .r9, index := some .rdx, scale := 8 })

/-! ## What the precondition gives -/

structure Pre (m : Mirror) (s : State) : Prop where
  world : (cx m).World s
  pre : (cx m).pre s
  arm : ArmOk s
  pub : RegionIx.PubFacts (cx m) s

theorem hv : (cx m).A.args (((cx m).sig.words (cx m).A.ptrBits).map (·.bits (cx m).A.ptrBits)) =
    some (fun s => [s.gpr .rdi]) := rfl

theorem pre_of {s : State} (hs : (freeContract abi m).pre s) : Pre m s := by
  have := (pre_iff (cx m) (s := s) hv rfl rfl).mp hs
  exact ⟨this.2.1, this.2.2, armOk hX this.2.1 this.2.2.2.2.1,
    RegionIx.pubFacts hX this.2.1 this.2.2.2.2.1 (by simp [tables])⟩

theorem ptr_eq (s : State) : (cx m).ptr s = s.gpr .rdi := rfl

/-! ## The front half's steps -/

section Front

variable {s : State}

theorem add0 (x : BitVec 64) : x + 0 = x := BitVec.add_zero x

theorem exec_h1 : exec (.alu .test .rdi (.reg .rdi)) (σ3 s) = some (h1 s) := rfl

theorem eval_h1 : eval .e (h1 s) = some (ptr s == 0) := by
  show some ((σ3 s).gpr .rdi &&& (σ3 s).gpr .rdi == 0) = _
  rw [σ3_rdi, BitVec.and_self]; rfl

theorem exec_h2 : exec (.leaSym .r9 "SLABS") (h1 s) = some (h2 s) := rfl

theorem before : RegionIx.Before s (h2 s) where
  mem := rfl
  rd := rfl
  wr := rfl
  r9 := by rw [h2, RegUpd.gpr_setReg_self, BitVec.ofInt_ofNat, add0]; simp only [BitVec.add_zero]; rfl
  rdi := by
    rw [h2, RegUpd.gpr_setReg_of_ne _ _ (by decide)]
    simp only [h1, RegUpd.gpr_arithFlags, σ3_rdi]

theorem idx_eq (hp : Pre m s) : RegionIx.idx m (h2 s) = (cx m).regionIndex s :=
  RegionIx.idx_eq (c := cx m) before hX hp.pub (ptr_eq (m := m) s)

theorem ρ5_r9 : (ρ5 m s).gpr .r9 = s.syms SLABS := by
  rw [ρ5, RegionIx.r5_gpr (by decide) (by decide), before.r9]

theorem exec_h3 : exec (.lea .rdx { base := .r9, disp := 33560576 }) (ρ5 m s) = some (h3 m s) := rfl

theorem h3_rdx : (h3 m s).gpr .rdx = s.syms SLABS + 33560576 := by
  rw [h3, RegUpd.gpr_setReg_self, Access.offset_base_disp, ρ5_r9, BitVec.ofInt_ofNat]; rfl

theorem h3_rax : (h3 m s).gpr .rax = RegionIx.idx m (h2 s) := by
  rw [h3, RegUpd.gpr_setReg_of_ne _ _ (by decide), ρ5, RegionIx.r5_rax]

theorem ea_h4 : (h3 m s).ea { base := .rdx, index := some .rax } = ra m s := by
  rw [Access.ea_of_seg_none rfl, Access.offset_index, h3_rdx, h3_rax]
  simp only [BitVec.ofInt_ofNat, BitVec.mul_one, BitVec.add_zero]; rfl

theorem ra_eq (hp : Pre m s) : ra m s = ((cx m).regionTable s).base + (cx m).regionIndex s := by
  rw [ra, idx_eq hp, Spec.CEntry.Ctx.regionTable, sym_c hX]; rfl

theorem cls_eq (hp : Pre m s) (h0 : ¬ (ptr s).toNat = 0) (hix : ((cx m).regionIndex s).toNat < REGION_COUNT) :
    (cx m).cls? s = some (rb m s).toNat := by
  simp only [Spec.CEntry.Ctx.cls?, ptr_eq, show (s.gpr .rdi).toNat ≠ 0 from h0, hix, ne_eq,
    not_false_eq_true, and_self, ↓reduceIte, Spec.CEntry.Ctx.regionByte, Spec.CEntry.Ctx.byteAt,
    mem_c hX, rb, ra_eq hp]

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

theorem byte_m1 (hp : Pre m s) (hix : ((cx m).regionIndex s).toNat < REGION_COUNT) : (m1 s) (ra m s) = rb m s :=
  Mem.write_apply ((region_facts hp hix).2.sep (Region.contains_self _ _) (Region.contains_self _ _)
    (ra m s) (by simp))

theorem exec_h4 (hp : Pre m s) (hix : ((cx m).regionIndex s).toNat < REGION_COUNT) :
    exec (.movzx8 .rdx { base := .rdx, index := some .rax }) (h3 m s) = some (h4 m s) := by
  have hin := (region_facts hp hix).1
  simp only [exec, ea_h4, Access.load8_eq (s := h3 m s) (by exact hin), Option.map_some]
  rw [show (h3 m s).mem = m1 s from rfl, byte_m1 hp hix]; rfl

theorem ev_h4 (hp : Pre m s) (hix : ((cx m).regionIndex s).toNat < REGION_COUNT) :
    Instr.events (.movzx8 .rdx { base := .rdx, index := some .rax }) (h3 m s) (h4 m s) =
      [⟨ra m s, 1, false, (rb m s).toNat⟩] := by
  simp only [Instr.events, ea_h4, Mem.read, show (h3 m s).mem = m1 s from rfl, byte_m1 hp hix]
  rw [BitVec.toNat_append]
  simp

theorem exec_h5 : exec (.lea .r9 { base := .r9, index := some .rdx, scale := 8 }) (h4 m s) = some (h5 m s) :=
  rfl

theorem ready : Ready (cx m) s (rb m s).toNat (h5 m s) where
  mem := rfl
  rd := rfl
  wr := rfl
  r8 := by
    simp only [h5, h4, h3, RegUpd.gpr_setReg_of_ne _ _ (show Reg.r8 ≠ .r9 by decide),
      RegUpd.gpr_setReg_of_ne _ _ (show Reg.r8 ≠ .rdx by decide), ρ5,
      RegionIx.r5_gpr (show Reg.r8 ≠ .rax by decide) (show Reg.r8 ≠ .rcx by decide), h2, h1,
      RegUpd.gpr_arithFlags, gpr_σ3_r8]
  r9 := by
    rw [h5, RegUpd.gpr_setReg_self, Access.offset_index]
    simp only [h4, RegUpd.gpr_setReg_self, RegUpd.gpr_setReg_of_ne _ _ (show Reg.r9 ≠ .rdx by decide),
      h3, ρ5_r9]
    have e : (rb m s).setWidth 64 * BitVec.ofNat 64 8 = BitVec.ofNat 64 (8 * (rb m s).toNat) := by
      apply BitVec.eq_of_toNat_eq
      have := (rb m s).isLt
      simp only [BitVec.toNat_mul, BitVec.toNat_setWidth, BitVec.toNat_ofNat]
      omega
    rw [e, BitVec.ofInt_ofNat]
    simp only [BitVec.add_zero]

end Front

/-! ## The ways out -/

section Cases

variable {s : State}

theorem eval_cpu_t (hi : ¬ (cx m).InRange s) : eval .ae (σ2 s) = some true := by
  rw [eval_cpu]; simp [show ¬ (cpu32 s).toNat < 8192 from fun h => hi ((inRange_c hX s).mpr h)]

theorem eval_cpu_f (hi : (cx m).InRange s) : eval .ae (σ2 s) = some false := by
  rw [eval_cpu]; simp [(inRange_c hX s).mp hi]

theorem eval_h1_t (h0 : (ptr s).toNat = 0) : eval .e (h1 s) = some true := by
  rw [eval_h1]; simp [show ptr s = 0 from BitVec.eq_of_toNat_eq (by simpa using h0)]

theorem eval_h1_f (h0 : ¬ (ptr s).toNat = 0) : eval .e (h1 s) = some false := by
  rw [eval_h1]; simp only [beq_eq_false_iff_ne, ne_eq, Option.some.injEq]
  exact fun h => h0 (by rw [h]; rfl)

theorem eval_ix_t (hp : Pre m s) (hix : ¬ ((cx m).regionIndex s).toNat < REGION_COUNT) :
    eval .ae (ρ5 m s) = some true := RegionIx.eval_r5_t (by rw [idx_eq hp]; exact hix)

theorem eval_ix_f (hp : Pre m s) (hix : ((cx m).regionIndex s).toNat < REGION_COUNT) :
    eval .ae (ρ5 m s) = some false := RegionIx.eval_r5_f (by rw [idx_eq hp]; exact hix)

theorem sa_in (hp : Pre m s) : InRegions ((h2 s).rd ++ (h2 s).wr) (RegionIx.sa m (h2 s)) 4 :=
  RegionIx.sa_in (c := cx m) before hX hp.pub
theorem ba_in (hp : Pre m s) : InRegions ((h2 s).rd ++ (h2 s).wr) (RegionIx.ba m (h2 s)) 8 :=
  RegionIx.ba_in (c := cx m) before hX hp.pub

theorem runBody_front (hp : Pre m s) (hi : (cx m).InRange s) (h0 : ¬ (ptr s).toNat = 0)
    (hix : ((cx m).regionIndex s).toNat < REGION_COUNT) :
    runBody isa (body m) (σA s) = runBody isa (pushSteps .r9 .rdi [] (some "slow")) (h5 m s) := by
  rw [body, runBody_instr (exec_cpu hp.arm), runBody_instr exec_cmp, runBody_skip (eval_cpu_f hi),
    runBody_instr exec_shl_r8, runBody_instr exec_h1, runBody_skip (eval_h1_f h0),
    runBody_instr exec_h2, runBody_instr (RegionIx.exec_r1 (sa_in hp)), runBody_instr RegionIx.exec_r2,
    runBody_instr RegionIx.exec_r3, runBody_instr (RegionIx.exec_r4 (ba_in hp)),
    runBody_instr RegionIx.exec_r5, runBody_skip (eval_ix_f hp hix), runBody_instr exec_h3,
    runBody_instr (exec_h4 hp hix), runBody_instr exec_h5]

def frontStates (m : Mirror) (s : State) : List State :=
  [σA s, σ1 s, σ2 s, σ2 s, σ3 s, h1 s, h1 s, h2 s, RegionIx.r1 m (h2 s), RegionIx.r2 m (h2 s),
    RegionIx.r3 m (h2 s), RegionIx.r4 m (h2 s), ρ5 m s, ρ5 m s, h3 m s, h4 m s]

theorem prefixes_front (hp : Pre m s) (hi : (cx m).InRange s) (h0 : ¬ (ptr s).toNat = 0)
    (hix : ((cx m).regionIndex s).toNat < REGION_COUNT) :
    bodyPrefixes isa (body m) (σA s) = frontStates m s ++
      bodyPrefixes isa (pushSteps .r9 .rdi [] (some "slow")) (h5 m s) := by
  rw [body, bodyPrefixes_instr (exec_cpu hp.arm), bodyPrefixes_instr exec_cmp,
    bodyPrefixes_skip (eval_cpu_f hi), bodyPrefixes_instr exec_shl_r8, bodyPrefixes_instr exec_h1,
    bodyPrefixes_skip (eval_h1_f h0), bodyPrefixes_instr exec_h2,
    bodyPrefixes_instr (RegionIx.exec_r1 (sa_in hp)), bodyPrefixes_instr RegionIx.exec_r2,
    bodyPrefixes_instr RegionIx.exec_r3, bodyPrefixes_instr (RegionIx.exec_r4 (ba_in hp)),
    bodyPrefixes_instr RegionIx.exec_r5, bodyPrefixes_skip (eval_ix_f hp hix),
    bodyPrefixes_instr exec_h3, bodyPrefixes_instr (exec_h4 hp hix), bodyPrefixes_instr exec_h5]
  rfl

theorem end_front (hp : Pre m s) (hi : (cx m).InRange s) (h0 : ¬ (ptr s).toNat = 0)
    (hix : ((cx m).regionIndex s).toNat < REGION_COUNT) :
    bodyEnd isa (body m) (σA s) = (bodyEnd isa (pushSteps .r9 .rdi [] (some "slow")) (h5 m s)).map (· + 16) := by
  rw [body, bodyEnd_instr (exec_cpu hp.arm), bodyEnd_instr exec_cmp, bodyEnd_skip (eval_cpu_f hi),
    bodyEnd_instr exec_shl_r8, bodyEnd_instr exec_h1, bodyEnd_skip (eval_h1_f h0),
    bodyEnd_instr exec_h2, bodyEnd_instr (RegionIx.exec_r1 (sa_in hp)), bodyEnd_instr RegionIx.exec_r2,
    bodyEnd_instr RegionIx.exec_r3, bodyEnd_instr (RegionIx.exec_r4 (ba_in hp)),
    bodyEnd_instr RegionIx.exec_r5, bodyEnd_skip (eval_ix_f hp hix), bodyEnd_instr exec_h3,
    bodyEnd_instr (exec_h4 hp hix), bodyEnd_instr exec_h5]
  simp only [Option.map_map]; rfl

theorem ev_cpu' :
    Instr.events (.mov32 .r8 (.mem { base := .r10, disp := 4, seg := some .fs })) (σA s) (σ1 s) =
      [⟨area s + 4, 4, false, (cpu32 s).toNat⟩] := by
  simp only [Instr.events, cpuOp_ea, σA_mem, toNat_read4, m1]
  rw [readW_writeW_disj (cpu_cs_disjoint s) (by decide)]; rfl

/-- The accesses up to the region index. -/
def pubEvents (m : Mirror) (s : State) : List Event :=
  [⟨RegionIx.sa m (h2 s), 4, false, (RegionIx.sv m (h2 s)).toNat⟩,
    ⟨RegionIx.ba m (h2 s), 8, false, (RegionIx.bw m (h2 s)).toNat⟩]

theorem events_front (hp : Pre m s) (hi : (cx m).InRange s) (h0 : ¬ (ptr s).toNat = 0)
    (hix : ((cx m).regionIndex s).toNat < REGION_COUNT) :
    bodyEvents isa (body m) (σA s) = [⟨area s + 4, 4, false, (cpu32 s).toNat⟩] ++ pubEvents m s ++
      [⟨ra m s, 1, false, (rb m s).toNat⟩] ++ bodyEvents isa (pushSteps .r9 .rdi [] (some "slow")) (h5 m s) := by
  rw [body, bodyEvents_instr (exec_cpu hp.arm), bodyEvents_instr exec_cmp,
    bodyEvents_skip (eval_cpu_f hi), bodyEvents_instr exec_shl_r8, bodyEvents_instr exec_h1,
    bodyEvents_skip (eval_h1_f h0), bodyEvents_instr exec_h2,
    bodyEvents_instr (RegionIx.exec_r1 (sa_in hp)), bodyEvents_instr RegionIx.exec_r2,
    bodyEvents_instr RegionIx.exec_r3, bodyEvents_instr (RegionIx.exec_r4 (ba_in hp)),
    bodyEvents_instr RegionIx.exec_r5, bodyEvents_skip (eval_ix_f hp hix),
    bodyEvents_instr exec_h3, bodyEvents_instr (exec_h4 hp hix), bodyEvents_instr exec_h5]
  simp only [ev_cpu', RegionIx.ev_r1, RegionIx.ev_r4, ev_h4 hp hix, pubEvents]; rfl

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

/-! ### Null -/

theorem runBody_N (hp : Pre m s) (hi : (cx m).InRange s) (h0 : (ptr s).toNat = 0) :
    runBody isa (body m) (σA s) = some (.done (h1 s)) := by
  rw [body, runBody_instr (exec_cpu hp.arm), runBody_instr exec_cmp, runBody_skip (eval_cpu_f hi),
    runBody_instr exec_shl_r8, runBody_instr exec_h1, runBody_take (eval_h1_t h0) execBlock_nil']

theorem prefixes_N (hp : Pre m s) (hi : (cx m).InRange s) (h0 : (ptr s).toNat = 0) :
    bodyPrefixes isa (body m) (σA s) = [σA s, σ1 s, σ2 s, σ2 s, σ3 s, h1 s] := by
  rw [body, bodyPrefixes_instr (exec_cpu hp.arm), bodyPrefixes_instr exec_cmp,
    bodyPrefixes_skip (eval_cpu_f hi), bodyPrefixes_instr exec_shl_r8, bodyPrefixes_instr exec_h1,
    bodyPrefixes_take (eval_h1_t h0)]

theorem end_N (hp : Pre m s) (hi : (cx m).InRange s) (h0 : (ptr s).toNat = 0) :
    bodyEnd isa (body m) (σA s) = some 5 := by
  rw [body, bodyEnd_instr (exec_cpu hp.arm), bodyEnd_instr exec_cmp, bodyEnd_skip (eval_cpu_f hi),
    bodyEnd_instr exec_shl_r8, bodyEnd_instr exec_h1, bodyEnd_take (eval_h1_t h0)]; rfl

theorem events_N (hp : Pre m s) (hi : (cx m).InRange s) (h0 : (ptr s).toNat = 0) :
    bodyEvents isa (body m) (σA s) = [⟨area s + 4, 4, false, (cpu32 s).toNat⟩] := by
  rw [body, bodyEvents_instr (exec_cpu hp.arm), bodyEvents_instr exec_cmp,
    bodyEvents_skip (eval_cpu_f hi), bodyEvents_instr exec_shl_r8, bodyEvents_instr exec_h1,
    bodyEvents_take (eval_h1_t h0)]
  simp only [isa, ev_cpu']; rfl

/-! ### An index out of range -/

theorem runBody_S (hp : Pre m s) (hi : (cx m).InRange s) (h0 : ¬ (ptr s).toNat = 0)
    (hix : ¬ ((cx m).regionIndex s).toNat < REGION_COUNT) :
    runBody isa (body m) (σA s) = some (.exit "slow" (ρ5 m s)) := by
  rw [body, runBody_instr (exec_cpu hp.arm), runBody_instr exec_cmp, runBody_skip (eval_cpu_f hi),
    runBody_instr exec_shl_r8, runBody_instr exec_h1, runBody_skip (eval_h1_f h0),
    runBody_instr exec_h2, runBody_instr (RegionIx.exec_r1 (sa_in hp)), runBody_instr RegionIx.exec_r2,
    runBody_instr RegionIx.exec_r3, runBody_instr (RegionIx.exec_r4 (ba_in hp)),
    runBody_instr RegionIx.exec_r5, runBody_take (eval_ix_t hp hix) execBlock_nil']
  rfl

theorem prefixes_S (hp : Pre m s) (hi : (cx m).InRange s) (h0 : ¬ (ptr s).toNat = 0)
    (hix : ¬ ((cx m).regionIndex s).toNat < REGION_COUNT) :
    bodyPrefixes isa (body m) (σA s) = [σA s, σ1 s, σ2 s, σ2 s, σ3 s, h1 s, h1 s, h2 s,
      RegionIx.r1 m (h2 s), RegionIx.r2 m (h2 s), RegionIx.r3 m (h2 s), RegionIx.r4 m (h2 s), ρ5 m s] := by
  rw [body, bodyPrefixes_instr (exec_cpu hp.arm), bodyPrefixes_instr exec_cmp,
    bodyPrefixes_skip (eval_cpu_f hi), bodyPrefixes_instr exec_shl_r8, bodyPrefixes_instr exec_h1,
    bodyPrefixes_skip (eval_h1_f h0), bodyPrefixes_instr exec_h2,
    bodyPrefixes_instr (RegionIx.exec_r1 (sa_in hp)), bodyPrefixes_instr RegionIx.exec_r2,
    bodyPrefixes_instr RegionIx.exec_r3, bodyPrefixes_instr (RegionIx.exec_r4 (ba_in hp)),
    bodyPrefixes_instr RegionIx.exec_r5, bodyPrefixes_take (eval_ix_t hp hix)]
  rfl

theorem end_S (hp : Pre m s) (hi : (cx m).InRange s) (h0 : ¬ (ptr s).toNat = 0)
    (hix : ¬ ((cx m).regionIndex s).toNat < REGION_COUNT) : bodyEnd isa (body m) (σA s) = some 12 := by
  rw [body, bodyEnd_instr (exec_cpu hp.arm), bodyEnd_instr exec_cmp, bodyEnd_skip (eval_cpu_f hi),
    bodyEnd_instr exec_shl_r8, bodyEnd_instr exec_h1, bodyEnd_skip (eval_h1_f h0),
    bodyEnd_instr exec_h2, bodyEnd_instr (RegionIx.exec_r1 (sa_in hp)), bodyEnd_instr RegionIx.exec_r2,
    bodyEnd_instr RegionIx.exec_r3, bodyEnd_instr (RegionIx.exec_r4 (ba_in hp)),
    bodyEnd_instr RegionIx.exec_r5, bodyEnd_take (eval_ix_t hp hix)]; rfl

theorem events_S (hp : Pre m s) (hi : (cx m).InRange s) (h0 : ¬ (ptr s).toNat = 0)
    (hix : ¬ ((cx m).regionIndex s).toNat < REGION_COUNT) :
    bodyEvents isa (body m) (σA s) = [⟨area s + 4, 4, false, (cpu32 s).toNat⟩] ++ pubEvents m s := by
  rw [body, bodyEvents_instr (exec_cpu hp.arm), bodyEvents_instr exec_cmp,
    bodyEvents_skip (eval_cpu_f hi), bodyEvents_instr exec_shl_r8, bodyEvents_instr exec_h1,
    bodyEvents_skip (eval_h1_f h0), bodyEvents_instr exec_h2,
    bodyEvents_instr (RegionIx.exec_r1 (sa_in hp)), bodyEvents_instr RegionIx.exec_r2,
    bodyEvents_instr RegionIx.exec_r3, bodyEvents_instr (RegionIx.exec_r4 (ba_in hp)),
    bodyEvents_instr RegionIx.exec_r5, bodyEvents_take (eval_ix_t hp hix)]
  simp only [ev_cpu', RegionIx.ev_r1, RegionIx.ev_r4, pubEvents]; rfl

end Cases

/-! ## What each state keeps -/

section Mids

variable {s : State}

theorem mσ2 : Mid scratchC s (m1 s) (σ2 s) :=
  mid_σ2 (mid_σ1 (by decide) (mid_σA (by decide) (by decide)))
theorem mσ3 : Mid scratchC s (m1 s) (σ3 s) := mid_σ3 (by decide) mσ2
theorem mh1 : Mid scratchC s (m1 s) (h1 s) := mσ3.arithFlags _ _ _
theorem mh2 : Mid scratchC s (m1 s) (h2 s) := mh1.setReg (by decide) _
theorem mr1 : Mid scratchC s (m1 s) (RegionIx.r1 m (h2 s)) := RegionIx.mid_r1 mh2 (by decide)
theorem mr2 : Mid scratchC s (m1 s) (RegionIx.r2 m (h2 s)) := RegionIx.mid_r2 mh2 (by decide) (by decide)
theorem mr3 : Mid scratchC s (m1 s) (RegionIx.r3 m (h2 s)) := RegionIx.mid_r3 mh2 (by decide) (by decide)
theorem mr4 : Mid scratchC s (m1 s) (RegionIx.r4 m (h2 s)) := RegionIx.mid_r4 mh2 (by decide) (by decide)
theorem mρ5 : Mid scratchC s (m1 s) (ρ5 m s) := RegionIx.mid_r5 mh2 (by decide) (by decide)
theorem mh3 : Mid scratchC s (m1 s) (h3 m s) := mρ5.setReg (by decide) _
theorem mh4 : Mid scratchC s (m1 s) (h4 m s) := mh3.setReg (by decide) _
theorem mh5 : Mid scratchC s (m1 s) (h5 m s) := mh4.setReg (by decide) _

theorem mid_front : ∀ p ∈ frontStates m s, Mid scratchC s (m1 s) p := by
  simp only [frontStates, List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
  exact ⟨mid_σA (by decide) (by decide), mid_σ1 (by decide) (mid_σA (by decide) (by decide)), mσ2, mσ2,
    mσ3, mh1, mh1, mh2, mr1, mr2, mr3, mr4, mρ5, mρ5, mh3, mh4⟩

end Mids

/-! ## The contract -/

section Contract

variable {s : State}

theorem post_eq (s' : State) : (freeContract abi m).post s s' ↔
    (((cx m).ptr s).toNat = 0 ∧ Unchanged [(cx m).csField s] ((cx m).mem s) s'.mem) ∨ (cx m).Pushed s ((cx m).ptr s) s' :=
  post_iff (cx m) hv s s'

theorem exit_eq (t : String) (s' : State) : (freeContract abi m).exit t s s' ↔
    [s'.gpr .rdi] = [s.gpr .rdi] ∧ (t = SLOW ∧ (cx m).PushMissed s ((cx m).ptr s) s') :=
  exit_iff (cx m) hv t s s'

theorem abi_m1 (hp : Pre m s) {p : State} (hm : Mid scratchC s (m1 s) p) : abiPreserved s p :=
  abiPreserved_of_mid frame_m1 (by
    have := disj_res hX hp.world (x := ((cx m).csField s, true)) (mem_fixed cs_mem_fixed)
    rw [csField_c hX] at this; simpa using this) (by decide) hm

theorem unchanged_cs : Unchanged [(cx m).csField s] ((cx m).mem s) (m1 s) := by
  rw [csField_c hX, mem_c hX]; exact frame_m1

theorem missed (hn : (cx m).pushResult s ((cx m).ptr s) = none) {p : State} (hm : Mid scratchC s (m1 s) p) :
    (freeContract abi m).exit "slow" s p :=
  (exit_eq _ _).mpr ⟨by rw [hm.gpr _ (by decide)], rfl, hn, by rw [hX.mem, hm.mem]; exact unchanged_cs⟩

theorem none_C (hi : ¬ (cx m).InRange s) : (cx m).pushResult s ((cx m).ptr s) = none := by
  simp only [Spec.CEntry.Ctx.pushResult, hi, ↓reduceIte]

theorem none_S (hix : ¬ ((cx m).regionIndex s).toNat < REGION_COUNT) :
    (cx m).pushResult s ((cx m).ptr s) = none := by
  have : (cx m).cls? s = none := by simp only [Spec.CEntry.Ctx.cls?, hix, and_false, ↓reduceIte]
  simp only [Spec.CEntry.Ctx.pushResult, this, Option.bind_none, ite_self]

theorem ptr_h5 : (h5 m s).gpr .rdi = (cx m).ptr s := by
  simp only [h5, h4, h3, RegUpd.gpr_setReg_of_ne _ _ (show Reg.rdi ≠ .r9 by decide),
    RegUpd.gpr_setReg_of_ne _ _ (show Reg.rdi ≠ .rdx by decide), ρ5,
    RegionIx.r5_gpr (show Reg.rdi ≠ .rax by decide) (show Reg.rdi ≠ .rcx by decide), before.rdi]; rfl

theorem correct (hs : (freeContract abi m).pre s) :
    (∃ s', Exec isa (cFree m) s s' ∧ target.abiPreserved s s' ∧ (freeContract abi m).post s s') ∨
    (∃ t s', ExecExit isa (cFree m) s t s' ∧ target.abiPreserved s s' ∧
      (freeContract abi m).exit t s s') := by
  have hp := pre_of hs
  rw [code_eq]
  by_cases hi : (cx m).InRange s
  · by_cases h0 : (ptr s).toNat = 0
    · exact .inl ⟨h1 s, .rseq (execBlock_arm hp.arm) (runBody_N hp hi h0), abi_m1 hp mh1,
        (post_eq _).mpr (.inl ⟨h0, by rw [mh1.mem]; exact unchanged_cs⟩)⟩
    by_cases hix : ((cx m).regionIndex s).toNat < REGION_COUNT
    · have hk := cls_eq hp h0 hix
      have hb := backFacts hX hp.world hp.pre hk hi
      have hr := ready (m := m) (s := s)
      have ho := pushOk hX hr hb hi
      by_cases ht : Tail.qtst .r9 (h5 m s) = 0
      · exact .inr ⟨"slow", _, .rseq (execBlock_arm hp.arm)
          (by rw [runBody_front hp hi h0 hix, Tail.runPush_miss ho _ ht]),
          abi_m1 hp (Tail.mid_q3 mh5 (by decide) (by decide)),
          missed (push_missed hX hr hb hk hi ht _) (Tail.mid_q3 mh5 (by decide) (by decide))⟩
      · obtain ⟨j, hj⟩ := sized_of_qtest hX hr hb ht
        have hf := slotFacts hX hr hb hj hi ht
        have hm5 := Tail.mid_q5 (w := .r9) (s := s) (mh5 (m := m)) (by decide) (by decide) .rdi
        refine .inl ⟨_, .rseq (execBlock_arm hp.arm)
          (by rw [runBody_front hp hi h0 hix, Tail.runPush_hit ho _ ht]), ?_,
          (post_eq _).mpr (.inr (by rw [← ptr_h5]; exact pushed hX hr hb hk hi ht))⟩
        refine abiPreserved_of_mid (xs := [⟨area s + 8, 8⟩, ⟨(cx m).w s (rb m s).toNat, 8⟩,
          ⟨(cx m).word s (rb m s).toNat, 8⟩]) ?_ ?_ (by decide) hm5
        · rw [Tail.qEnd, Tail.qSlot, hr.mem, wa_eq hX hr, wv_eq hX hr hb]
          exact frame_three s.mem _ _ _ _ _ _
        · simpa using ⟨hb.ret_cs, hf.ret, hb.ret_word⟩
    · exact .inr ⟨"slow", ρ5 m s, .rseq (execBlock_arm hp.arm) (runBody_S hp hi h0 hix), abi_m1 hp mρ5,
        missed (none_S hix) mρ5⟩
  · exact .inr ⟨"slow", σ2 s, .rseq (execBlock_arm hp.arm) (runBody_C hp hi), abi_m1 hp mσ2,
      missed (none_C hi) mσ2⟩

/-! ### Where it may be aborted -/

theorem abort_eq (p : State) : (freeContract abi m).abort s p ↔ [p.gpr .rdi] = [s.gpr .rdi] ∧
    ((cx m).PushAbort s p ∧ (cx m).inv p) :=
  abort_iff (cx m) hv s p

theorem abortOk (hp : Pre m s) {p : State} (hm : Mid scratchC s (m1 s) p) :
    armed p ∧ target.abiPreserved s p ∧ (freeContract abi m).abort s p ∧
      ∀ p', target.rely p p' → (freeContract abi m).env p p' → (freeContract abi m).pre p' := by
  obtain ⟨hip, hre⟩ := retryC hX hv (post := fun s s' => _) (fun _ => rfl) (fun _ => rfl) (by cases m <;> decide)
    hp.pre hm (by cases m <;> decide) (by cases m <;> decide) frame_m1 (harmless_csC hX hp.pre)
  refine ⟨armed_of_mid hp.arm (xs := []) frame_m1 m1_cs (by simp) hm, abi_m1 hp hm, ?_, hre⟩
  exact (abort_eq p).mpr ⟨by rw [hm.gpr _ (by decide)], pushAbort_m1 hX hp.world hp.pre hm.mem, hip⟩

/-- After the free slot is written. -/
theorem abortOk_slot (hp : Pre m s) (hi : (cx m).InRange s) {k : Nat} (hk : (cx m).cls? s = some k)
    (hr : Ready (cx m) s k (h5 m s)) (ht : ¬ Tail.qtst .r9 (h5 m s) = 0) :
    armed (Tail.q4 .r9 (h5 m s) .rdi) ∧ target.abiPreserved s (Tail.q4 .r9 (h5 m s) .rdi) ∧
      (freeContract abi m).abort s (Tail.q4 .r9 (h5 m s) .rdi) ∧
      ∀ p', target.rely (Tail.q4 .r9 (h5 m s) .rdi) p' → (freeContract abi m).env (Tail.q4 .r9 (h5 m s) .rdi) p' →
        (freeContract abi m).pre p' := by
  have hb := backFacts hX hp.world hp.pre hk hi
  obtain ⟨j, hj⟩ := sized_of_qtest hX hr hb ht
  have hf := slotFacts hX hr hb hj hi ht
  have hm := Tail.mid_q4 (w := .r9) (s := s) (mh5 (m := m)) (by decide) (by decide) .rdi
  have hfr : Frame [⟨area s + 8, 8⟩, ⟨(cx m).w s k, 8⟩] s.mem (Tail.qSlot .r9 (h5 m s) .rdi) := by
    rw [Tail.qSlot, hr.mem, wv_eq hX hr hb]; exact frame_two _ _ _ _ _
  have hcs : (Tail.qSlot .r9 (h5 m s) .rdi).readW (area s + 8) 64 = s.csDesc := by
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
  exact (abort_eq _).mpr ⟨by rw [hm.gpr _ (by decide)],
    pushAbort_slot hX hp.world hp.pre hr hk hi ht hm.mem, hip⟩

/-! ### Its accesses -/

theorem ofNat_toNat64 (v : BitVec 64) : BitVec.ofNat 64 v.toNat = v := by
  rw [BitVec.ofNat_toNat, BitVec.setWidth_eq]

theorem addr8 : s.fsBase + o s + BitVec.ofNat 64 RSEQ_CS = area s + 8 := rfl
theorem addr4 : s.fsBase + o s + BitVec.ofNat 64 RSEQ_CPU_ID = area s + 4 := rfl
theorem rsName : RSEQ_OFFSET = RS := rfl

/-- The published reads as the contract states them. -/
theorem pub_eq : pubEvents m s =
    [⟨((cx m).published s).base, 4, false, (RegionIx.sv m (h2 s)).toNat⟩,
      ⟨((cx m).published s).base + 8, 8, false, (RegionIx.bw m (h2 s)).toNat⟩] := by
  rw [pubEvents, RegionIx.sa_eq before hX, RegionIx.ba_eq before hX]

theorem idx_vals (hp : Pre m s) : ((cx m).ptr s >>> ((RegionIx.sv m (h2 s)).toNat % 64)) -
    RegionIx.bw m (h2 s) = (cx m).regionIndex s := by
  show s.gpr .rdi >>> _ - _ = _
  rw [← idx_eq hp, RegionIx.idx, RegionIx.sp_eq, before.rdi]

theorem trace_ok (hs : (freeContract abi m).pre s) :
    (freeContract abi m).trace s (Code.events isa (cFree m) s) := by
  have hp := pre_of hs
  show (cx m).FreeTrace s (Code.events isa (.rseq arm (body m)) s)
  simp only [Code.events, execBlock_arm hp.arm, events_arm hp.arm]
  have htp : (cx m).slab.tp s = s.fsBase := tp_of (c := (cx m).slab) (hX (m := m)) s
  by_cases hi : (cx m).InRange s
  · have hi' : (cpu32 s).toNat < SLAB_COUNT := (inRange_c hX s).mp hi
    by_cases h0 : (ptr s).toNat = 0
    · refine ⟨(o s).toNat, s.csDesc.toNat, (cpu32 s).toNat, 0, 0, 0, 0, 0, ?_⟩
      rw [events_N hp hi h0]
      simp only [Spec.CEntry.Ctx.head, Spec.CEntry.Ctx.rd, Spec.CEntry.Ctx.wr, htp, sym_c hX, addr8, addr4,
        rsName, ofNat_toNat64, ptr_eq, show (s.gpr .rdi).toNat = 0 from h0, ne_eq, not_true_eq_false,
        and_false, ↓reduceIte, List.append_nil]
      events_norm
    have h0' : ((cx m).ptr s).toNat ≠ 0 := h0
    refine ⟨(o s).toNat, s.csDesc.toNat, (cpu32 s).toNat, (RegionIx.sv m (h2 s)).toNat,
      (RegionIx.bw m (h2 s)).toNat, (rb m s).toNat, (Tail.wv .r9 (h5 m s)).toNat, (Tail.mk .r9 (h5 m s)).toNat, ?_⟩
    simp only [Spec.CEntry.Ctx.regionReads, idx_vals hp, Spec.CEntry.Ctx.head, Spec.CEntry.Ctx.rd,
      Spec.CEntry.Ctx.wr, htp, sym_c hX, addr8, addr4, rsName, ofNat_toNat64, hi', h0', ne_eq,
      not_false_eq_true, and_self, ↓reduceIte]
    by_cases hix : ((cx m).regionIndex s).toNat < REGION_COUNT
    · have hk := cls_eq hp h0 hix
      have hb := backFacts hX hp.world hp.pre hk hi
      have hr := ready (m := m) (s := s)
      have ho := pushOk hX hr hb hi
      rw [events_front hp hi h0 hix, pub_eq]
      have hword : wordAddr (s.syms SLABS) (cpu32 s).toNat (rb m s).toNat = Tail.wa .r9 (h5 m s) := by
        rw [wa_eq hX hr]; simp only [Spec.CEntry.Ctx.word, sym_c hX, cpu_c hX]
      have hmask : s.syms SLABS + BitVec.ofNat 64 (8 * (rb m s).toNat + MASK_DISP) = Tail.ma .r9 (h5 m s) := by
        rw [ma_eq hX hr, sym_c hX]
      have hra : ((cx m).regionTable s).base + (cx m).regionIndex s = ra m s := (ra_eq hp).symm
      simp only [Spec.CEntry.Ctx.pushTail, hix, ↓reduceIte, hra, hword, hmask, sym_c hX, ptr_eq]
      have hq : BitVec.ofNat 32 ((Tail.wv .r9 (h5 m s)).toNat + 8) = (Tail.wv .r9 (h5 m s) + 8).setWidth 32 := by
        apply BitVec.eq_of_toNat_eq
        rw [BitVec.toNat_ofNat, BitVec.toNat_setWidth, add8_toNat_mod,
          Nat.mod_mod_of_dvd _ (by decide : 2 ^ 32 ∣ 2 ^ 64)]
      by_cases ht : Tail.qtst .r9 (h5 m s) = 0
      · rw [Tail.eventsPush_miss ho _ ht]
        have : BitVec.ofNat 32 ((Tail.wv .r9 (h5 m s)).toNat + 8) &&& BitVec.ofNat 32 (Tail.mk .r9 (h5 m s)).toNat = 0 := by
          rw [hq, BitVec.ofNat_toNat, BitVec.setWidth_eq]; exact ht
        simp only [this, ne_eq, not_true_eq_false, ↓reduceIte, List.append_nil]
        events_norm
      · rw [Tail.eventsPush_hit ho _ ht]
        have : ¬ BitVec.ofNat 32 ((Tail.wv .r9 (h5 m s)).toNat + 8) &&& BitVec.ofNat 32 (Tail.mk .r9 (h5 m s)).toNat = 0 := by
          rw [hq, BitVec.ofNat_toNat, BitVec.setWidth_eq]; exact ht
        have hx : (h5 m s).gpr .rdi = s.gpr .rdi := ptr_h5
        simp only [this, ne_eq, not_false_eq_true, ↓reduceIte, ofNat_toNat64, hx]
        events_norm
    · rw [events_S hp hi h0 hix, pub_eq]
      simp only [hix, ↓reduceIte, List.append_nil]
      events_norm
  · have hi' : ¬ (cpu32 s).toNat < SLAB_COUNT := fun h => hi ((inRange_c hX s).mpr h)
    refine ⟨(o s).toNat, s.csDesc.toNat, (cpu32 s).toNat, 0, 0, 0, 0, 0, ?_⟩
    rw [events_C hp hi]
    simp only [Spec.CEntry.Ctx.head, Spec.CEntry.Ctx.rd, Spec.CEntry.Ctx.wr, htp, sym_c hX, addr8, addr4,
      rsName, ofNat_toNat64, hi', false_and, ↓reduceIte, List.append_nil]
    events_norm

theorem rseqOk (hs : (freeContract abi m).pre s) : RseqOk target (cFree m) (freeContract abi m) s := by
  have hp := pre_of hs
  have ht := trace_ok hs
  rw [code_eq] at ht ⊢
  unfold RseqOk
  refine ⟨ht, ⟨_, _, rfl, execBlock_dropLast_arm hp.arm, arms_arm⟩, ?_⟩
  have mA : Mid scratchC s (m1 s) (σA s) := mid_σA (by decide) (by decide)
  have m1' : Mid scratchC s (m1 s) (σ1 s) := mid_σ1 (by decide) mA
  simp only [Code.abortStates, execBlock_arm hp.arm]
  by_cases hi : (cx m).InRange s
  · by_cases h0 : (ptr s).toNat = 0
    · rw [prefixes_N hp hi h0]
      simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
      exact ⟨abortOk hp mA, abortOk hp m1', abortOk hp mσ2, abortOk hp mσ2, abortOk hp mσ3, abortOk hp mh1⟩
    by_cases hix : ((cx m).regionIndex s).toNat < REGION_COUNT
    · have hk := cls_eq hp h0 hix
      have hb := backFacts hX hp.world hp.pre hk hi
      have hr := ready (m := m) (s := s)
      have ho := pushOk hX hr hb hi
      rw [prefixes_front hp hi h0 hix]
      intro p hp'
      rcases List.mem_append.mp hp' with hp' | hp'
      · exact abortOk hp (mid_front p hp')
      · by_cases ht : Tail.qtst .r9 (h5 m s) = 0
        · rw [Tail.prefixesPush_miss ho _ ht] at hp'
          simp only [List.mem_cons, List.not_mem_nil, or_false] at hp'
          rcases hp' with rfl | rfl | rfl | rfl
          · exact abortOk hp mh5
          · exact abortOk hp (Tail.mid_t1 mh5 (by decide))
          · exact abortOk hp (Tail.mid_q2 mh5 (by decide) (by decide))
          · exact abortOk hp (Tail.mid_q3 mh5 (by decide) (by decide))
        · rw [Tail.prefixesPush_hit ho _ ht] at hp'
          simp only [List.mem_cons, List.not_mem_nil, or_false] at hp'
          rcases hp' with rfl | rfl | rfl | rfl | rfl | rfl
          · exact abortOk hp mh5
          · exact abortOk hp (Tail.mid_t1 mh5 (by decide))
          · exact abortOk hp (Tail.mid_q2 mh5 (by decide) (by decide))
          · exact abortOk hp (Tail.mid_q3 mh5 (by decide) (by decide))
          · exact abortOk hp (Tail.mid_q3 mh5 (by decide) (by decide))
          · exact abortOk_slot hp hi hk hr ht
    · rw [prefixes_S hp hi h0 hix]
      simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
      exact ⟨abortOk hp mA, abortOk hp m1', abortOk hp mσ2, abortOk hp mσ2, abortOk hp mσ3,
        abortOk hp mh1, abortOk hp mh1, abortOk hp mh2, abortOk hp mr1, abortOk hp mr2, abortOk hp mr3,
        abortOk hp mr4, abortOk hp mρ5⟩
  · rw [prefixes_C hp hi]
    simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
    exact ⟨abortOk hp mA, abortOk hp m1', abortOk hp mσ2⟩

end Contract

end VG.Proof.CEntry.X86_64.Free
