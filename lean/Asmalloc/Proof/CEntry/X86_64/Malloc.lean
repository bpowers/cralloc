module

public import Asmalloc.Proof.CEntry.X86_64.Back
public import Asmalloc.Impl.CEntry.X86_64

/-!
# `c_malloc` on x86-64

`Impl.CEntry.X86_64.cMalloc` against `Spec.CEntry.mallocContract`: after the
arm and the CPU number, the size is compared with 32768, its class read from
`C_SIZES`, and the class's word in slab 0 formed in `r9`; then the pop's
steps (`Proof/Slab/X86_64/Tail.lean`). Four ways out: no slab for the CPU, a
size above 32768, nothing to pop (each a tail call of the slow path), and
the commit.
-/

@[expose] public section

namespace VG.Proof.CEntry.X86_64.Malloc

open VG.X86_64 VG.Spec.Slab VG.Spec.CEntry VG.Impl.Slab.X86_64 VG.Impl.CEntry.X86_64
  VG.Proof.Slab.X86_64 VG.Proof.CEntry VG.Proof.Slab

abbrev cx : Spec.CEntry.Ctx isa := ⟨abi, .malloc, mallocSig, .c⟩

theorem hX : X86 cx.A := x86_abi

def body : List (Step Instr Cond) :=
  .instr (.mov32 .r8 (.mem { base := .r10, disp := 4, seg := some .fs })) ::
  .instr (.alu32 .cmp .r8 (.imm 8192)) ::
  .exit .ae [] (some "slow") ::
  .instr (.shift .shl .r8 12) ::
  .instr (.alu .cmp .rdi (.imm 32769)) ::
  .exit .ae [] (some "slow") ::
  .instr (.lea .rax { base := .rdi, disp := 7 }) ::
  .instr (.shift32 .shr .rax 3) ::
  .instr (.leaSym .rcx "C_SIZES" 2048) ::
  .instr (.movzx8 .rdx { base := .rcx, index := some .rax }) ::
  .instr (.leaSym .r9 "SLABS") ::
  .instr (.lea .r9 { base := .r9, index := some .rdx, scale := 8 }) ::
  popSteps .r9 [] (some "slow")

theorem code_eq : cMalloc = .rseq arm body := rfl

/-! ## The front half's states -/

/-- The size, as the routine has it in `rdi`. -/
def n (s : State) : BitVec 64 := s.gpr .rdi

def lim : BitVec 64 := (32769 : BitVec 32).signExtend 64

def g1 (s : State) : State :=
  arithFlags (σ3 s) ((σ3 s).gpr .rdi - lim) (decide (((σ3 s).gpr .rdi).toNat < lim.toNat))
    (subOverflow ((σ3 s).gpr .rdi) lim ((σ3 s).gpr .rdi - lim))
def g2 (s : State) : State := (g1 s).setReg .rax ((g1 s).offset { base := .rdi, disp := 7 })
/-- The table index, `(size + 7) >> 3` in 32 bits. -/
def ix (s : State) : BitVec 32 := ((g2 s).gpr .rax).setWidth 32 >>> 3
def g3 (s : State) : State :=
  ((g2 s).setFlags (some ((((g2 s).gpr .rax).setWidth 32).getLsbD 2)) none (some (ix s == 0))
    (some (ix s).msb)).setReg32 .rax (ix s)
/-- The table entry's address. -/
def ta (s : State) : Addr := s.syms "C_SIZES" + 2048 + (ix s).setWidth 64
def g4 (s : State) : State := (g3 s).setReg .rcx ((g3 s).syms "C_SIZES" + BitVec.ofInt 64 2048 + 0)
/-- The class byte. -/
def kb (s : State) : Byte := s.mem (ta s)
def g5 (s : State) : State := (g4 s).setReg .rdx ((kb s).setWidth 64)
def g6 (s : State) : State := (g5 s).setReg .r9 ((g5 s).syms "SLABS" + BitVec.ofInt 64 0 + 0)
def g7 (s : State) : State :=
  (g6 s).setReg .r9 ((g6 s).offset { base := .r9, index := some .rdx, scale := 8 })

/-! ## What the precondition gives -/

structure Pre (s : State) : Prop where
  world : cx.World s
  pre : cx.pre s
  arm : ArmOk s

theorem hv : cx.A.args ((cx.sig.words cx.A.ptrBits).map (·.bits cx.A.ptrBits)) =
    some (fun s => [s.gpr .rdi]) := rfl

theorem pre_of {s : State} (hs : (mallocContract abi).pre s) : Pre s := by
  have := (pre_iff cx (s := s) hv rfl rfl).mp hs
  exact ⟨this.2.1, this.2.2, armOk hX this.2.1 this.2.2.2.2.1⟩

theorem size_eq (s : State) : cx.size s = n s := rfl

/-! ## The front half's steps -/

section Front

variable {s : State}

theorem exec_g1 : exec (.alu .cmp .rdi (.imm 32769)) (σ3 s) = some (g1 s) := rfl

theorem lim_toNat : lim.toNat = 32769 := by decide

theorem eval_g1 : eval .ae (g1 s) = some (!decide ((n s).toNat < 32769)) := by
  show some (!decide (((σ3 s).gpr .rdi).toNat < lim.toNat)) = _
  rw [σ3_rdi, lim_toNat]; rfl

theorem exec_g2 : exec (.lea .rax { base := .rdi, disp := 7 }) (g1 s) = some (g2 s) := rfl

theorem exec_g3 : exec (.shift32 .shr .rax 3) (g2 s) = some (g3 s) := by
  show execShift32 .shr .rax 3 (g2 s) = _
  unfold execShift32
  simp only [show (1 ≤ 3 ∧ 3 ≤ 31) from ⟨by decide, by decide⟩]
  rfl

theorem exec_g4 : exec (.leaSym .rcx "C_SIZES" 2048) (g3 s) = some (g4 s) := rfl

theorem g2_rax : (g2 s).gpr .rax = n s + 7 := by
  rw [g2, RegUpd.gpr_setReg_self, Access.offset_base_disp]
  simp only [g1, RegUpd.gpr_arithFlags, σ3_rdi]; rfl

theorem add0 (x : BitVec 64) : x + 0 = x := BitVec.add_zero x

theorem g4_rcx : (g4 s).gpr .rcx = s.syms "C_SIZES" + 2048 := by
  rw [g4, RegUpd.gpr_setReg_self, BitVec.ofInt_ofNat, add0]; rfl

theorem g4_rax : (g4 s).gpr .rax = (ix s).setWidth 64 := by
  rw [g4, RegUpd.gpr_setReg_of_ne _ _ (by decide)]
  simp only [g3, State.setReg32, RegUpd.gpr_setReg_self]

theorem g4_mem : (g4 s).mem = m1 s := rfl
theorem g4_rd : (g4 s).rd = s.rd := rfl
theorem g4_wr : (g4 s).wr = s.wr := rfl

theorem ea_g5 : (g4 s).ea { base := .rcx, index := some .rax } = ta s := by
  rw [Access.ea_of_seg_none rfl, Access.offset_index, g4_rcx, g4_rax]
  simp only [BitVec.ofInt_ofNat, BitVec.mul_one, BitVec.add_zero]; rfl

/-- For a size the table covers, the index is `(size + 7) / 8`. -/
theorem ix_toNat (hn : (n s).toNat ≤ MALLOC_MAX) : ((ix s).setWidth 64).toNat = ((n s).toNat + 7) / 8 := by
  have h7 : (n s + 7).toNat = (n s).toNat + 7 := by
    rw [BitVec.toNat_add, show (7 : BitVec 64).toNat = 7 from rfl]
    simp only [MALLOC_MAX] at hn
    exact Nat.mod_eq_of_lt (by omega)
  simp only [ix, g2_rax, BitVec.toNat_setWidth, BitVec.toNat_ushiftRight, h7, Nat.shiftRight_eq_div_pow]
  simp only [MALLOC_MAX] at hn
  rw [Nat.mod_eq_of_lt (a := (n s).toNat + 7) (by omega), Nat.mod_eq_of_lt (by omega)]

theorem ta_eq (hn : (n s).toNat ≤ MALLOC_MAX) :
    ta s = (cx.mallocTable s).base + BitVec.ofNat 64 (((cx.size s).toNat + 7) / 8) := by
  simp only [ta, Spec.CEntry.Ctx.mallocTable, sym_c hX, size_eq, TABLE_OFF]
  congr 1
  apply BitVec.eq_of_toNat_eq
  rw [ix_toNat hn, BitVec.toNat_ofNat, Nat.mod_eq_of_lt (by simp only [MALLOC_MAX] at hn; omega)]

theorem ta_eq' (hn : (n s).toNat ≤ MALLOC_MAX) :
    ta s = (cx.mallocTable s).base + BitVec.ofNat 64 (((n s).toNat + 7) / 8) := ta_eq hn

theorem cls_eq (hn : (n s).toNat ≤ MALLOC_MAX) : cx.cls? s = some (kb s).toNat := by
  simp only [Spec.CEntry.Ctx.cls?, size_eq, hn, ↓reduceIte, Spec.CEntry.Ctx.mallocClass,
    Spec.CEntry.Ctx.byteAt, mem_c hX, kb]
  rw [ta_eq hn, size_eq]

/-- The table's entry is readable, and apart from `rseq_cs`. -/
theorem table_facts (hp : Pre s) (hn : (n s).toNat ≤ MALLOC_MAX) :
    InRegions (s.rd ++ s.wr) (ta s) 1 ∧ Region.Disjoint ⟨ta s, 1⟩ ⟨area s + 8, 8⟩ := by
  have hm : (cx.mallocTable s, false) ∈ cx.fixed s := table_mem_fixed (by simp [tables])
  have hin : cx.mallocTable s ∈ s.rd := mem_rd hX hp.world (x := (cx.mallocTable s, false)) (mem_fixed hm) rfl
  have hc : (cx.mallocTable s).Contains (ta s) 1 := by
    rw [ta_eq hn]
    apply Offset.contains_base
    · simp only [MALLOC_LEN, MALLOC_MAX] at hn ⊢; rw [size_eq]; omega
    · rw [size_eq]; simp only [MALLOC_MAX] at hn; omega
  have hd : (cx.csField s).Disjoint (cx.mallocTable s) :=
    cs_table hp.pre.2.2.1 (x := (cx.mallocTable s, false)) (by simp [tables])
  rw [csField_c hX] at hd
  exact ⟨(InRegions.of_mem hin hc).append_left, (hd.sub_right (sub_of_contains hc)).symm⟩

theorem exec_g5 (hp : Pre s) (hn : (n s).toNat ≤ MALLOC_MAX) :
    exec (.movzx8 .rdx { base := .rcx, index := some .rax }) (g4 s) = some (g5 s) := by
  have ⟨hin, hd⟩ := table_facts hp hn
  have hm : (g4 s).mem (ta s) = kb s := by
    rw [g4_mem, m1]
    exact Mem.write_apply (hd.sep (Region.contains_self _ _) (Region.contains_self _ _) (ta s)
      (by simp))
  simp only [exec, ea_g5, Access.load8_eq (by rw [g4_rd, g4_wr]; exact hin), Option.map_some, hm]
  rfl

theorem exec_g6 : exec (.leaSym .r9 "SLABS") (g5 s) = some (g6 s) := rfl
theorem exec_g7 : exec (.lea .r9 { base := .r9, index := some .rdx, scale := 8 }) (g6 s) = some (g7 s) :=
  rfl

theorem ready : Ready cx s (kb s).toNat (g7 s) where
  mem := rfl
  rd := rfl
  wr := rfl
  r8 := by
    simp only [g7, g6, g5, g4, g3, g2, g1, RegUpd.gpr_setReg_of_ne _ _ (show Reg.r8 ≠ .r9 by decide),
      RegUpd.gpr_setReg_of_ne _ _ (show Reg.r8 ≠ .rdx by decide),
      RegUpd.gpr_setReg_of_ne _ _ (show Reg.r8 ≠ .rcx by decide), State.setReg32,
      RegUpd.gpr_setReg_of_ne _ _ (show Reg.r8 ≠ .rax by decide), RegUpd.gpr_setFlags,
      RegUpd.gpr_arithFlags, gpr_σ3_r8]
  r9 := by
    rw [g7, RegUpd.gpr_setReg_self, Access.offset_index]
    simp only [g6, g5, RegUpd.gpr_setReg_self, RegUpd.gpr_setReg_of_ne _ _ (show Reg.rdx ≠ .r9 by decide)]
    have e : (kb s).setWidth 64 * BitVec.ofNat 64 8 = BitVec.ofNat 64 (8 * (kb s).toNat) := by
      apply BitVec.eq_of_toNat_eq
      have := (kb s).isLt
      simp only [BitVec.toNat_mul, BitVec.toNat_setWidth, BitVec.toNat_ofNat]
      omega
    simp only [RegUpd.syms_setReg]
    rw [show (g4 s).syms = s.syms from rfl, e, BitVec.ofInt_ofNat]
    simp only [BitVec.add_zero, add0]
    rfl

end Front

/-! ## The four ways out -/

section Cases

variable {s : State}

theorem eval_cpu_t (hi : ¬ cx.InRange s) : eval .ae (σ2 s) = some true := by
  rw [eval_cpu]; simp [show ¬ (cpu32 s).toNat < 8192 from fun h => hi ((inRange_c hX s).mpr h)]

theorem eval_cpu_f (hi : cx.InRange s) : eval .ae (σ2 s) = some false := by
  rw [eval_cpu]; simp [(inRange_c hX s).mp hi]

theorem eval_g1_f (hn : (n s).toNat ≤ MALLOC_MAX) : eval .ae (g1 s) = some false := by
  rw [eval_g1]; simp only [MALLOC_MAX] at hn; simp; omega

/-- Every step of the body up to the pop's, with the exits not taken. -/
theorem runBody_front (hp : Pre s) (hi : cx.InRange s) (hn : (n s).toNat ≤ MALLOC_MAX) :
    runBody isa body (σA s) = runBody isa (popSteps .r9 [] (some "slow")) (g7 s) := by
  rw [body, runBody_instr (exec_cpu hp.arm), runBody_instr exec_cmp,
    runBody_skip (eval_cpu_f hi), runBody_instr exec_shl_r8, runBody_instr exec_g1,
    runBody_skip (eval_g1_f hn), runBody_instr exec_g2,
    runBody_instr exec_g3, runBody_instr exec_g4, runBody_instr (exec_g5 hp hn), runBody_instr exec_g6,
    runBody_instr exec_g7]

theorem prefixes_front (hp : Pre s) (hi : cx.InRange s) (hn : (n s).toNat ≤ MALLOC_MAX) :
    bodyPrefixes isa body (σA s) = [σA s, σ1 s, σ2 s, σ2 s, σ3 s, g1 s, g1 s, g2 s, g3 s, g4 s, g5 s, g6 s] ++
      bodyPrefixes isa (popSteps .r9 [] (some "slow")) (g7 s) := by
  rw [body, bodyPrefixes_instr (exec_cpu hp.arm), bodyPrefixes_instr exec_cmp,
    bodyPrefixes_skip (eval_cpu_f hi), bodyPrefixes_instr exec_shl_r8,
    bodyPrefixes_instr exec_g1,
    bodyPrefixes_skip (eval_g1_f hn),
    bodyPrefixes_instr exec_g2, bodyPrefixes_instr exec_g3, bodyPrefixes_instr exec_g4,
    bodyPrefixes_instr (exec_g5 hp hn), bodyPrefixes_instr exec_g6, bodyPrefixes_instr exec_g7]
  rfl

theorem end_front (hp : Pre s) (hi : cx.InRange s) (hn : (n s).toNat ≤ MALLOC_MAX) :
    bodyEnd isa body (σA s) = (bodyEnd isa (popSteps .r9 [] (some "slow")) (g7 s)).map (· + 12) := by
  rw [body, bodyEnd_instr (exec_cpu hp.arm), bodyEnd_instr exec_cmp,
    bodyEnd_skip (eval_cpu_f hi), bodyEnd_instr exec_shl_r8, bodyEnd_instr exec_g1,
    bodyEnd_skip (eval_g1_f hn), bodyEnd_instr exec_g2,
    bodyEnd_instr exec_g3, bodyEnd_instr exec_g4, bodyEnd_instr (exec_g5 hp hn), bodyEnd_instr exec_g6,
    bodyEnd_instr exec_g7]
  simp only [Option.map_map]; rfl

theorem ev_cpu' :
    Instr.events (.mov32 .r8 (.mem { base := .r10, disp := 4, seg := some .fs })) (σA s) (σ1 s) =
      [⟨area s + 4, 4, false, (cpu32 s).toNat⟩] := by
  simp only [Instr.events, cpuOp_ea, σA_mem, toNat_read4, m1]
  rw [readW_writeW_disj (cpu_cs_disjoint s) (by decide)]; rfl

theorem ev_g5 (hp : Pre s) (hn : (n s).toNat ≤ MALLOC_MAX) :
    Instr.events (.movzx8 .rdx { base := .rcx, index := some .rax }) (g4 s) (g5 s) =
      [⟨ta s, 1, false, (kb s).toNat⟩] := by
  obtain ⟨-, hd⟩ := table_facts hp hn
  have hb : (m1 s) (ta s) = kb s := Mem.write_apply
    (hd.sep (Region.contains_self _ _) (Region.contains_self _ _) (ta s) (by simp))
  simp only [Instr.events, ea_g5, g4_mem, Mem.read, hb]
  rw [BitVec.toNat_append]
  simp

theorem events_front (hp : Pre s) (hi : cx.InRange s) (hn : (n s).toNat ≤ MALLOC_MAX) :
    bodyEvents isa body (σA s) = [⟨area s + 4, 4, false, (cpu32 s).toNat⟩, ⟨ta s, 1, false, (kb s).toNat⟩] ++
      bodyEvents isa (popSteps .r9 [] (some "slow")) (g7 s) := by
  rw [body, bodyEvents_instr (exec_cpu hp.arm), bodyEvents_instr exec_cmp,
    bodyEvents_skip (eval_cpu_f hi), bodyEvents_instr exec_shl_r8,
    bodyEvents_instr exec_g1,
    bodyEvents_skip (eval_g1_f hn),
    bodyEvents_instr exec_g2, bodyEvents_instr exec_g3, bodyEvents_instr exec_g4,
    bodyEvents_instr (exec_g5 hp hn), bodyEvents_instr exec_g6, bodyEvents_instr exec_g7]
  simp only [ev_cpu', ev_g5 hp hn]; rfl

/-! ### No slab -/

theorem runBody_C (hp : Pre s) (hi : ¬ cx.InRange s) :
    runBody isa body (σA s) = some (.exit "slow" (σ2 s)) := by
  rw [body, runBody_instr (exec_cpu hp.arm), runBody_instr exec_cmp,
    runBody_take (eval_cpu_t hi) execBlock_nil']

theorem prefixes_C (hp : Pre s) (hi : ¬ cx.InRange s) :
    bodyPrefixes isa body (σA s) = [σA s, σ1 s, σ2 s] := by
  rw [body, bodyPrefixes_instr (exec_cpu hp.arm), bodyPrefixes_instr exec_cmp,
    bodyPrefixes_take (eval_cpu_t hi)]

theorem end_C (hp : Pre s) (hi : ¬ cx.InRange s) : bodyEnd isa body (σA s) = some 2 := by
  rw [body, bodyEnd_instr (exec_cpu hp.arm), bodyEnd_instr exec_cmp,
    bodyEnd_take (eval_cpu_t hi)]; rfl

theorem events_C (hp : Pre s) (hi : ¬ cx.InRange s) :
    bodyEvents isa body (σA s) = [⟨area s + 4, 4, false, (cpu32 s).toNat⟩] := by
  rw [body, bodyEvents_instr (exec_cpu hp.arm), bodyEvents_instr exec_cmp,
    bodyEvents_take (eval_cpu_t hi)]
  simp only [isa, ev_cpu']; rfl

/-! ### A size the table does not cover -/

theorem eval_g1_big (hn : ¬ (n s).toNat ≤ MALLOC_MAX) : eval .ae (g1 s) = some true := by
  rw [eval_g1]; simp only [MALLOC_MAX] at hn; simp; omega

theorem runBody_S (hp : Pre s) (hi : cx.InRange s) (hn : ¬ (n s).toNat ≤ MALLOC_MAX) :
    runBody isa body (σA s) = some (.exit "slow" (g1 s)) := by
  rw [body, runBody_instr (exec_cpu hp.arm), runBody_instr exec_cmp,
    runBody_skip (eval_cpu_f hi), runBody_instr exec_shl_r8, runBody_instr exec_g1,
    runBody_take (eval_g1_big hn) execBlock_nil']

theorem prefixes_S (hp : Pre s) (hi : cx.InRange s) (hn : ¬ (n s).toNat ≤ MALLOC_MAX) :
    bodyPrefixes isa body (σA s) = [σA s, σ1 s, σ2 s, σ2 s, σ3 s, g1 s] := by
  rw [body, bodyPrefixes_instr (exec_cpu hp.arm), bodyPrefixes_instr exec_cmp,
    bodyPrefixes_skip (eval_cpu_f hi), bodyPrefixes_instr exec_shl_r8,
    bodyPrefixes_instr exec_g1, bodyPrefixes_take (eval_g1_big hn)]

theorem end_S (hp : Pre s) (hi : cx.InRange s) (hn : ¬ (n s).toNat ≤ MALLOC_MAX) :
    bodyEnd isa body (σA s) = some 5 := by
  rw [body, bodyEnd_instr (exec_cpu hp.arm), bodyEnd_instr exec_cmp,
    bodyEnd_skip (eval_cpu_f hi), bodyEnd_instr exec_shl_r8, bodyEnd_instr exec_g1,
    bodyEnd_take (eval_g1_big hn)]; rfl

theorem events_S (hp : Pre s) (hi : cx.InRange s) (hn : ¬ (n s).toNat ≤ MALLOC_MAX) :
    bodyEvents isa body (σA s) = [⟨area s + 4, 4, false, (cpu32 s).toNat⟩] := by
  rw [body, bodyEvents_instr (exec_cpu hp.arm), bodyEvents_instr exec_cmp,
    bodyEvents_skip (eval_cpu_f hi), bodyEvents_instr exec_shl_r8,
    bodyEvents_instr exec_g1, bodyEvents_take (eval_g1_big hn)]
  simp only [isa, ev_cpu']; rfl

end Cases

/-! ## What each state keeps -/

section Mids

variable {s : State}

theorem mσ2 : Mid scratchC s (m1 s) (σ2 s) :=
  mid_σ2 (mid_σ1 (by decide) (mid_σA (by decide) (by decide)))
theorem mσ3 : Mid scratchC s (m1 s) (σ3 s) := mid_σ3 (by decide) mσ2
theorem mg1 : Mid scratchC s (m1 s) (g1 s) := mσ3.arithFlags _ _ _
theorem mg2 : Mid scratchC s (m1 s) (g2 s) := mg1.setReg (by decide) _
theorem mg3 : Mid scratchC s (m1 s) (g3 s) := (mg2.setFlags _ _ _ _).setReg32 (by decide) _
theorem mg4 : Mid scratchC s (m1 s) (g4 s) := mg3.setReg (by decide) _
theorem mg5 : Mid scratchC s (m1 s) (g5 s) := mg4.setReg (by decide) _
theorem mg6 : Mid scratchC s (m1 s) (g6 s) := mg5.setReg (by decide) _
theorem mg7 : Mid scratchC s (m1 s) (g7 s) := mg6.setReg (by decide) _

end Mids

/-! ## The contract -/

section Contract

variable {s : State}

theorem post_eq (s' : State) : (mallocContract abi).post s s' ↔
    ∃ v, cx.Popped s s' v ∧ s'.gpr .rax = v :=
  post_iff cx hv s s'

theorem exit_eq (t : String) (s' : State) : (mallocContract abi).exit t s s' ↔
    [s'.gpr .rdi] = [s.gpr .rdi] ∧ (t = SLOW ∧ cx.PopMissed s s') :=
  exit_iff cx hv t s s'

theorem abi_m1 (hp : Pre s) {p : State} (hm : Mid scratchC s (m1 s) p) : abiPreserved s p :=
  abiPreserved_of_mid frame_m1 (by
    have := disj_res hX hp.world (x := (cx.csField s, true)) (mem_fixed cs_mem_fixed)
    rw [csField_c hX] at this; simpa using this) (by decide) hm

theorem unchanged_cs : Unchanged [cx.csField s] (cx.mem s) (m1 s) := by
  rw [csField_c hX, mem_c hX]; exact frame_m1

theorem missed_C (hi : ¬ cx.InRange s) {p : State} (hm : Mid scratchC s (m1 s) p) :
    cx.PopMissed s p := by
  refine ⟨?_, by rw [hX.mem, hm.mem]; exact unchanged_cs⟩
  simp only [Spec.CEntry.Ctx.popResult, hi, ↓reduceIte]

theorem missed_S (hn : ¬ (n s).toNat ≤ MALLOC_MAX) {p : State} (hm : Mid scratchC s (m1 s) p) :
    cx.PopMissed s p := by
  refine ⟨?_, by rw [hX.mem, hm.mem]; exact unchanged_cs⟩
  have : cx.cls? s = none := by simp only [Spec.CEntry.Ctx.cls?, size_eq, hn, ↓reduceIte]
  simp only [Spec.CEntry.Ctx.popResult, this, Option.bind_none, ite_self]

theorem exit_C (hi : ¬ cx.InRange s) {p : State} (hm : Mid scratchC s (m1 s) p) :
    (mallocContract abi).exit "slow" s p :=
  (exit_eq _ _).mpr ⟨by rw [hm.gpr _ (by decide)], rfl, missed_C hi hm⟩

theorem correct (hs : (mallocContract abi).pre s) :
    (∃ s', Exec isa cMalloc s s' ∧ target.abiPreserved s s' ∧ (mallocContract abi).post s s') ∨
    (∃ t s', ExecExit isa cMalloc s t s' ∧ target.abiPreserved s s' ∧
      (mallocContract abi).exit t s s') := by
  have hp := pre_of hs
  rw [code_eq]
  by_cases hi : cx.InRange s
  · by_cases hn : (n s).toNat ≤ MALLOC_MAX
    · have hk := cls_eq hn
      have hb := backFacts hX hp.world hp.pre hk hi
      have hr := ready (s := s)
      have ho := popOk hX hr hb hi
      by_cases ht : Tail.ptst .r9 (g7 s) = 0
      · exact .inr ⟨"slow", _, .rseq (execBlock_arm hp.arm)
          (by rw [runBody_front hp hi hn, Tail.runPop_miss ho _ ht]),
          abi_m1 hp (Tail.mid_p2 mg7 (by decide)),
          (exit_eq _ _).mpr ⟨by rw [(Tail.mid_p2 mg7 (by decide)).gpr _ (by decide)], rfl,
            pop_missed hX hr hb hk hi ht,
            by rw [hX.mem, (Tail.mid_p2 mg7 (by decide)).mem]; exact unchanged_cs⟩⟩
      · have hm5 := Tail.mid_p5 (w := .r9) (s := s) mg7 (by decide) (by decide)
        exact .inl ⟨_, .rseq (execBlock_arm hp.arm)
          (by rw [runBody_front hp hi hn, Tail.runPop_hit ho _ ht]),
          abiPreserved_of_mid (by rw [hr.mem, wa_eq hX hr]; exact frame_two s.mem _ _ _ _)
            (by
              have := hb.ret_cs; have := hb.ret_word
              simpa using ⟨‹Region.Disjoint ⟨area s + 8, 8⟩ ⟨s.gpr .rsp, 8⟩›, this⟩)
            (by decide) hm5,
          (post_eq _).mpr ⟨_, popped hX hr hb hk hi ht, Tail.p5_rax⟩⟩
    · exact .inr ⟨"slow", g1 s, .rseq (execBlock_arm hp.arm) (runBody_S hp hi hn), abi_m1 hp mg1,
        (exit_eq _ _).mpr ⟨by rw [mg1.gpr _ (by decide)], rfl, missed_S hn mg1⟩⟩
  · exact .inr ⟨"slow", σ2 s, .rseq (execBlock_arm hp.arm) (runBody_C hp hi), abi_m1 hp mσ2,
      exit_C hi mσ2⟩

/-! ### Where it may be aborted -/

theorem abort_eq (p : State) : (mallocContract abi).abort s p ↔ [p.gpr .rdi] = [s.gpr .rdi] ∧
    (Unchanged [cx.csField s] (cx.mem s) p.mem ∧ cx.inv p) :=
  abort_iff cx hv s p

theorem abortOk (hp : Pre s) {p : State} (hm : Mid scratchC s (m1 s) p) :
    armed p ∧ target.abiPreserved s p ∧ (mallocContract abi).abort s p ∧
      ∀ p', target.rely p p' → (mallocContract abi).env p p' → (mallocContract abi).pre p' := by
  obtain ⟨hip, hre⟩ := retryC hX hv (post := fun s s' => _) (fun _ => rfl) (fun _ => rfl) (by decide)
    hp.pre hm (by decide) (by decide) frame_m1 (harmless_csC hX hp.pre)
  refine ⟨armed_of_mid hp.arm (xs := []) frame_m1 m1_cs (by simp) hm, abi_m1 hp hm, ?_, hre⟩
  exact (abort_eq p).mpr ⟨by rw [hm.gpr _ (by decide)], by rw [hm.mem]; exact unchanged_cs, hip⟩

/-! ### Its accesses -/

theorem ofNat_toNat64 (v : BitVec 64) : BitVec.ofNat 64 v.toNat = v := by
  rw [BitVec.ofNat_toNat, BitVec.setWidth_eq]

theorem addr8 {s : State} : s.fsBase + o s + BitVec.ofNat 64 RSEQ_CS = area s + 8 := rfl
theorem addr4 {s : State} : s.fsBase + o s + BitVec.ofNat 64 RSEQ_CPU_ID = area s + 4 := rfl
theorem rsName : RSEQ_OFFSET = RS := rfl

theorem trace_ok (hs : (mallocContract abi).pre s) :
    (mallocContract abi).trace s (Code.events isa cMalloc s) := by
  have hp := pre_of hs
  show cx.MallocTrace s (Code.events isa (.rseq arm body) s)
  simp only [Code.events, execBlock_arm hp.arm, events_arm hp.arm]
  by_cases hi : cx.InRange s
  · have hi' : (cpu32 s).toNat < SLAB_COUNT := (inRange_c hX s).mp hi
    by_cases hn : (n s).toNat ≤ MALLOC_MAX
    · have hk := cls_eq hn
      have hb := backFacts hX hp.world hp.pre hk hi
      have hr := ready (s := s)
      have ho := popOk hX hr hb hi
      have hw := wa_eq hX hr
      have hma := ma_eq hX hr
      have hwv := wv_eq hX hr hb
      have hmk := mk_eq hX hr hb
      refine ⟨(o s).toNat, s.csDesc.toNat, (cpu32 s).toNat, (kb s).toNat, (Tail.wv .r9 (g7 s)).toNat,
        (Tail.mk .r9 (g7 s)).toNat, (Tail.px .r9 (g7 s)).toNat, ?_⟩
      rw [events_front hp hi hn]
      have hword : wordAddr (s.syms SLABS) (cpu32 s).toNat (kb s).toNat = Tail.wa .r9 (g7 s) := by
        rw [hw]; simp only [Spec.CEntry.Ctx.word, sym_c hX, cpu_c hX]
      have hmask : s.syms SLABS + BitVec.ofNat 64 (8 * (kb s).toNat + MASK_DISP) = Tail.ma .r9 (g7 s) := by
        rw [hma, sym_c hX]
      have htp : cx.slab.tp s = s.fsBase := tp_of (c := cx.slab) hX s
      simp only [Spec.CEntry.Ctx.head, Spec.CEntry.Ctx.popTail, Spec.CEntry.Ctx.rd, Spec.CEntry.Ctx.wr,
        htp, sym_c hX, hi', hn, size_eq, and_self, ↓reduceIte, ← ta_eq' hn, hword, hmask,
        ofNat_toNat64, addr8, addr4, rsName]
      by_cases ht : Tail.ptst .r9 (g7 s) = 0
      · rw [Tail.eventsPop_miss ho _ ht]
        have : BitVec.ofNat 32 (Tail.wv .r9 (g7 s)).toNat &&& BitVec.ofNat 32 (Tail.mk .r9 (g7 s)).toNat = 0 := by
          rw [BitVec.ofNat_toNat, BitVec.ofNat_toNat, BitVec.setWidth_eq]; exact ht
        simp only [this, ne_eq, not_true_eq_false, ↓reduceIte, List.append_nil]; rfl
      · rw [Tail.eventsPop_hit ho _ ht]
        have : ¬ BitVec.ofNat 32 (Tail.wv .r9 (g7 s)).toNat &&& BitVec.ofNat 32 (Tail.mk .r9 (g7 s)).toNat = 0 := by
          rw [BitVec.ofNat_toNat, BitVec.ofNat_toNat, BitVec.setWidth_eq]; exact ht
        simp only [this, ne_eq, not_false_eq_true, ↓reduceIte]; rfl
    · refine ⟨(o s).toNat, s.csDesc.toNat, (cpu32 s).toNat, 0, 0, 0, 0, ?_⟩
      rw [events_S hp hi hn]
      simp only [Spec.CEntry.Ctx.head, Spec.CEntry.Ctx.rd, Spec.CEntry.Ctx.wr,
        tp_of (c := cx.slab) hX, sym_c hX, addr8, addr4, rsName, ofNat_toNat64, size_eq, hn, and_false,
        ↓reduceIte, List.append_nil]
      rfl
  · have hi' : ¬ (cpu32 s).toNat < SLAB_COUNT := fun h => hi ((inRange_c hX s).mpr h)
    refine ⟨(o s).toNat, s.csDesc.toNat, (cpu32 s).toNat, 0, 0, 0, 0, ?_⟩
    rw [events_C hp hi]
    simp only [Spec.CEntry.Ctx.head, Spec.CEntry.Ctx.rd, Spec.CEntry.Ctx.wr,
      tp_of (c := cx.slab) hX, sym_c hX, addr8, addr4, rsName, ofNat_toNat64, hi', false_and,
      ↓reduceIte, List.append_nil]
    rfl

theorem rseqOk (hs : (mallocContract abi).pre s) : RseqOk target cMalloc (mallocContract abi) s := by
  have hp := pre_of hs
  have ht := trace_ok hs
  rw [code_eq] at ht ⊢
  unfold RseqOk
  refine ⟨ht, ⟨_, _, rfl, execBlock_dropLast_arm hp.arm, arms_arm⟩, ?_⟩
  have mA : Mid scratchC s (m1 s) (σA s) := mid_σA (by decide) (by decide)
  have m1' : Mid scratchC s (m1 s) (σ1 s) := mid_σ1 (by decide) mA
  simp only [Code.abortStates, execBlock_arm hp.arm]
  by_cases hi : cx.InRange s
  · by_cases hn : (n s).toNat ≤ MALLOC_MAX
    · have hk := cls_eq hn
      have hb := backFacts hX hp.world hp.pre hk hi
      have hr := ready (s := s)
      have ho := popOk hX hr hb hi
      have front : ∀ p ∈ [σA s, σ1 s, σ2 s, σ2 s, σ3 s, g1 s, g1 s, g2 s, g3 s, g4 s, g5 s, g6 s],
          Mid scratchC s (m1 s) p := by
        simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
        exact ⟨mA, m1', mσ2, mσ2, mσ3, mg1, mg1, mg2, mg3, mg4, mg5, mg6⟩
      rw [prefixes_front hp hi hn]
      intro p hp'
      rcases List.mem_append.mp hp' with hp' | hp'
      · exact abortOk hp (front p hp')
      · by_cases ht : Tail.ptst .r9 (g7 s) = 0
        · rw [Tail.prefixesPop_miss ho _ ht] at hp'
          simp only [List.mem_cons, List.not_mem_nil, or_false] at hp'
          rcases hp' with rfl | rfl | rfl
          · exact abortOk hp mg7
          · exact abortOk hp (Tail.mid_t1 mg7 (by decide))
          · exact abortOk hp (Tail.mid_p2 mg7 (by decide))
        · rw [Tail.prefixesPop_hit ho _ ht] at hp'
          simp only [List.mem_cons, List.not_mem_nil, or_false] at hp'
          rcases hp' with rfl | rfl | rfl | rfl | rfl | rfl
          · exact abortOk hp mg7
          · exact abortOk hp (Tail.mid_t1 mg7 (by decide))
          · exact abortOk hp (Tail.mid_p2 mg7 (by decide))
          · exact abortOk hp (Tail.mid_p2 mg7 (by decide))
          · exact abortOk hp (Tail.mid_p3 mg7 (by decide))
          · exact abortOk hp (Tail.mid_p4 mg7 (by decide) (by decide))
    · rw [prefixes_S hp hi hn]
      simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
      exact ⟨abortOk hp mA, abortOk hp m1', abortOk hp mσ2, abortOk hp mσ2, abortOk hp mσ3,
        abortOk hp mg1⟩
  · rw [prefixes_C hp hi]
    simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
    exact ⟨abortOk hp mA, abortOk hp m1', abortOk hp mσ2⟩

end Contract

end VG.Proof.CEntry.X86_64.Malloc
