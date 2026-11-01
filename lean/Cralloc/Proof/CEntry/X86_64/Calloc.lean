module

public import Cralloc.Proof.CEntry.X86_64.Back
public import Cralloc.Impl.CEntry.X86_64

/-!
# `c_calloc` on x86-64

`Impl.CEntry.X86_64.cCalloc` against `Spec.CEntry.callocContract`: after the
arm and the CPU number, the product `n * size` (`mul`), the slow path if it
overflows or is above 1024, its class read from `C_CALLOC_SIZES`, the class's
word in slab 0 in `r9`, and the pop's steps; after the commit, outside the
sequence, the block into `rdx` and a jump to the zeroing tail. Five ways out:
no slab for the CPU, an overflow, a total above 1024, nothing to pop (each a
tail call of the slow path), and the commit followed by the tail call of the
zeroing tail.
-/

@[expose] public section

namespace VG.Proof.CEntry.X86_64.Calloc

open VG.X86_64 VG.Spec.Slab VG.Spec.CEntry VG.Impl.Slab.X86_64 VG.Impl.CEntry.X86_64
  VG.Proof.Slab.X86_64 VG.Proof.CEntry VG.Proof.Slab

abbrev cx : Spec.CEntry.Ctx isa := ⟨abi, .calloc, callocSig, .c⟩

theorem hX : X86 cx.A := x86_abi

def body : List (Step Instr Cond) :=
  .instr (.mov32 .r8 (.mem { base := .r10, disp := 4, seg := some .fs })) ::
  .instr (.alu32 .cmp .r8 (.imm 8192)) ::
  .exit .ae [] (some "slow") ::
  .instr (.shift .shl .r8 12) ::
  .instr (.mov .rax (.reg .rdi)) ::
  .instr (.mul .rsi) ::
  .exit .b [] (some "slow") ::
  .instr (.alu .cmp .rax (.imm 1025)) ::
  .exit .ae [] (some "slow") ::
  .instr (.leaSym .rcx "C_CALLOC_SIZES" 2048) ::
  .instr (.movzx8 .rdx { base := .rcx, index := some .rax }) ::
  .instr (.leaSym .r9 "SLABS") ::
  .instr (.lea .r9 { base := .r9, index := some .rdx, scale := 8 }) ::
  popSteps .r9 [] (some "slow")

def tail : List Instr := [.mov .rdx (.reg .rax)]

theorem code_eq : cCalloc = .seq (.rseq arm body) (.jump tail "zero") := rfl

/-! ## The front half's states -/

def c1 (s : State) : State := (σ3 s).setReg .rax ((σ3 s).gpr .rdi)
def c2 (s : State) : State := execMul .rsi (c1 s)
/-- The product, as the routine has it in `rax`. -/
def tot (s : State) : BitVec 64 := (c2 s).gpr .rax
def lim : BitVec 64 := (1025 : BitVec 32).signExtend 64
def c3 (s : State) : State :=
  arithFlags (c2 s) (tot s - lim) (decide ((tot s).toNat < lim.toNat)) (subOverflow (tot s) lim (tot s - lim))
def c4 (s : State) : State := (c3 s).setReg .rcx ((c3 s).syms "C_CALLOC_SIZES" + BitVec.ofInt 64 2048 + 0)
/-- The table entry's address, and the class byte. -/
def ta (s : State) : Addr := s.syms "C_CALLOC_SIZES" + 2048 + tot s
def kb (s : State) : Byte := s.mem (ta s)
def c5 (s : State) : State := (c4 s).setReg .rdx ((kb s).setWidth 64)
def c6 (s : State) : State := (c5 s).setReg .r9 ((c5 s).syms "SLABS" + BitVec.ofInt 64 0 + 0)
def c7 (s : State) : State :=
  (c6 s).setReg .r9 ((c6 s).offset { base := .r9, index := some .rdx, scale := 8 })
/-- After the jump's one instruction: the block in `rdx`. -/
def q (s : State) : State := (Tail.p5 .r9 (c7 s)).setReg .rdx ((Tail.p5 .r9 (c7 s)).gpr .rax)

/-! ## What the precondition gives -/

structure Pre (s : State) : Prop where
  world : cx.World s
  pre : cx.pre s
  arm : ArmOk s

theorem hv : cx.A.args ((cx.sig.words cx.A.ptrBits).map (·.bits cx.A.ptrBits)) =
    some (fun s => [s.gpr .rdi, s.gpr .rsi]) := rfl

theorem pre_of {s : State} (hs : (callocContract abi).pre s) : Pre s := by
  have := (pre_iff cx (s := s) hv rfl rfl).mp hs
  exact ⟨this.2.1, this.2.2, armOk hX this.2.1 this.2.2.2.2.1⟩

/-- The total is one the table covers. -/
def Ok (s : State) : Prop := ¬ cx.overflows s ∧ (cx.size s).toNat ≤ CALLOC_MAX

/-! ## The front half's steps -/

section Front

variable {s : State}

theorem add0 (x : BitVec 64) : x + 0 = x := BitVec.add_zero x

theorem exec_c1 : exec (.mov .rax (.reg .rdi)) (σ3 s) = some (c1 s) := by
  simp only [exec, readSrc, Option.map_some]; rfl

theorem exec_c2 : exec (.mul .rsi) (c1 s) = some (c2 s) := rfl

theorem c1_rax : (c1 s).gpr .rax = s.gpr .rdi := by rw [c1, RegUpd.gpr_setReg_self, σ3_rdi]
theorem c1_rsi : (c1 s).gpr .rsi = s.gpr .rsi := by
  rw [c1, RegUpd.gpr_setReg_of_ne _ _ (by decide)]; exact gpr_σ3 (by decide) (by decide) (by decide)

theorem tot_eq : tot s = cx.size s := by
  apply BitVec.eq_of_toNat_eq
  simp only [tot, c2, execMul, RegUpd.gpr_setReg_of_ne _ _ (show Reg.rax ≠ .rdx by decide),
    RegUpd.gpr_setReg_self, c1_rax, c1_rsi, BitVec.toNat_ofNat]
  rfl

theorem eval_c2 : eval .b (c2 s) = some (decide (cx.overflows s)) := by
  show (c2 s).cf = _
  simp only [c2, execMul, RegUpd.cf_setReg, RegUpd.cf_setFlags, c1_rax, c1_rsi]
  have hp := Nat.mul_lt_mul'' (s.gpr .rdi).isLt (s.gpr .rsi).isLt
  have hq : (s.gpr .rdi).toNat * (s.gpr .rsi).toNat / 2 ^ 64 < 2 ^ 64 := by
    apply Nat.div_lt_of_lt_mul; rw [← Nat.pow_add]; exact hp
  have hz : (BitVec.ofNat 64 ((s.gpr .rdi).toNat * (s.gpr .rsi).toNat / 2 ^ 64) != 0) =
      decide (2 ^ 64 ≤ (s.gpr .rdi).toNat * (s.gpr .rsi).toNat) := by
    rw [Bool.eq_iff_iff, bne_iff_ne, ne_eq, ← BitVec.toNat_inj, BitVec.toNat_ofNat,
      show (0 : BitVec 64).toNat = 0 from rfl, Nat.mod_eq_of_lt hq, Nat.div_eq_zero_iff_lt (by decide),
      decide_eq_true_iff]
    omega
  rw [hz]; rfl

theorem exec_c3 : exec (.alu .cmp .rax (.imm 1025)) (c2 s) = some (c3 s) := rfl

theorem lim_toNat : lim.toNat = 1025 := by decide

theorem eval_c3 : eval .ae (c3 s) = some (!decide ((cx.size s).toNat < 1025)) := by
  show some (!decide ((tot s).toNat < lim.toNat)) = _
  rw [lim_toNat, tot_eq]

theorem exec_c4 : exec (.leaSym .rcx "C_CALLOC_SIZES" 2048) (c3 s) = some (c4 s) := rfl

theorem c4_rcx : (c4 s).gpr .rcx = s.syms "C_CALLOC_SIZES" + 2048 := by
  rw [c4, RegUpd.gpr_setReg_self, BitVec.ofInt_ofNat, add0]; rfl

theorem c4_rax : (c4 s).gpr .rax = tot s := by
  rw [c4, RegUpd.gpr_setReg_of_ne _ _ (by decide)]; simp only [c3, RegUpd.gpr_arithFlags]; rfl

theorem c4_mem : (c4 s).mem = m1 s := rfl
theorem c4_rd : (c4 s).rd = s.rd := rfl
theorem c4_wr : (c4 s).wr = s.wr := rfl

theorem ea_c5 : (c4 s).ea { base := .rcx, index := some .rax } = ta s := by
  rw [Access.ea_of_seg_none rfl, Access.offset_index, c4_rcx, c4_rax]
  simp only [BitVec.ofInt_ofNat, BitVec.mul_one, BitVec.add_zero]; rfl

theorem ta_eq : ta s = (cx.callocTable s).base + BitVec.ofNat 64 (cx.size s).toNat := by
  rw [ta, tot_eq, BitVec.ofNat_toNat, BitVec.setWidth_eq, Spec.CEntry.Ctx.callocTable, sym_c hX]; rfl

theorem cls_eq (ho : Ok s) : cx.cls? s = some (kb s).toNat := by
  simp only [Spec.CEntry.Ctx.cls?, show ¬ cx.overflows s from ho.1, ho.2, not_false_eq_true, and_self,
    ↓reduceIte, Spec.CEntry.Ctx.callocClass, Spec.CEntry.Ctx.byteAt, mem_c hX, kb]
  rw [ta_eq]

/-- The table's entry is readable, and apart from `rseq_cs`. -/
theorem table_facts (hp : Pre s) (ho : Ok s) :
    InRegions (s.rd ++ s.wr) (ta s) 1 ∧ Region.Disjoint ⟨ta s, 1⟩ ⟨area s + 8, 8⟩ := by
  have hm : (cx.callocTable s, false) ∈ cx.fixed s := table_mem_fixed (by simp [tables])
  have hin : cx.callocTable s ∈ s.rd := mem_rd hX hp.world (x := (cx.callocTable s, false)) (mem_fixed hm) rfl
  have h2 := ho.2
  have hc : (cx.callocTable s).Contains (ta s) 1 := by
    rw [ta_eq]
    apply Offset.contains_base
    · simp only [CALLOC_LEN, CALLOC_MAX] at h2 ⊢; omega
    · simp only [CALLOC_MAX] at h2; omega
  have hd : (cx.csField s).Disjoint (cx.callocTable s) :=
    cs_table hp.pre.2.2.1 (x := (cx.callocTable s, false)) (by simp [tables])
  rw [csField_c hX] at hd
  exact ⟨(InRegions.of_mem hin hc).append_left, (hd.sub_right (sub_of_contains hc)).symm⟩

theorem kb_m1 (hp : Pre s) (ho : Ok s) : (m1 s) (ta s) = kb s :=
  Mem.write_apply ((table_facts hp ho).2.sep (Region.contains_self _ _) (Region.contains_self _ _)
    (ta s) (by simp))

theorem exec_c5 (hp : Pre s) (ho : Ok s) :
    exec (.movzx8 .rdx { base := .rcx, index := some .rax }) (c4 s) = some (c5 s) := by
  simp only [exec, ea_c5, Access.load8_eq (s := c4 s) (by rw [c4_rd, c4_wr]; exact (table_facts hp ho).1),
    Option.map_some]
  rw [c4_mem, kb_m1 hp ho]; rfl

theorem ev_c5 (hp : Pre s) (ho : Ok s) :
    Instr.events (.movzx8 .rdx { base := .rcx, index := some .rax }) (c4 s) (c5 s) =
      [⟨ta s, 1, false, (kb s).toNat⟩] := by
  simp only [Instr.events, ea_c5, c4_mem, Mem.read, kb_m1 hp ho]
  rw [BitVec.toNat_append]
  simp

theorem exec_c6 : exec (.leaSym .r9 "SLABS") (c5 s) = some (c6 s) := rfl
theorem exec_c7 : exec (.lea .r9 { base := .r9, index := some .rdx, scale := 8 }) (c6 s) = some (c7 s) :=
  rfl

theorem ready : Ready cx s (kb s).toNat (c7 s) where
  mem := rfl
  rd := rfl
  wr := rfl
  r8 := by
    simp only [c7, c6, c5, c4, c3, c2, c1, execMul, RegUpd.gpr_setReg_of_ne _ _ (show Reg.r8 ≠ .r9 by decide),
      RegUpd.gpr_setReg_of_ne _ _ (show Reg.r8 ≠ .rdx by decide),
      RegUpd.gpr_setReg_of_ne _ _ (show Reg.r8 ≠ .rcx by decide),
      RegUpd.gpr_setReg_of_ne _ _ (show Reg.r8 ≠ .rax by decide), RegUpd.gpr_setFlags,
      RegUpd.gpr_arithFlags, gpr_σ3_r8]
  r9 := by
    rw [c7, RegUpd.gpr_setReg_self, Access.offset_index]
    simp only [c6, c5, RegUpd.gpr_setReg_self, RegUpd.gpr_setReg_of_ne _ _ (show Reg.rdx ≠ .r9 by decide)]
    have e : (kb s).setWidth 64 * BitVec.ofNat 64 8 = BitVec.ofNat 64 (8 * (kb s).toNat) := by
      apply BitVec.eq_of_toNat_eq
      have := (kb s).isLt
      simp only [BitVec.toNat_mul, BitVec.toNat_setWidth, BitVec.toNat_ofNat]
      omega
    simp only [RegUpd.syms_setReg]
    rw [show (c4 s).syms = s.syms from rfl, e, BitVec.ofInt_ofNat]
    simp only [BitVec.add_zero, add0]
    rfl

end Front

/-! ## The five ways out -/

section Cases

variable {s : State}

theorem eval_cpu_t (hi : ¬ cx.InRange s) : eval .ae (σ2 s) = some true := by
  rw [eval_cpu]; simp [show ¬ (cpu32 s).toNat < 8192 from fun h => hi ((inRange_c hX s).mpr h)]

theorem eval_cpu_f (hi : cx.InRange s) : eval .ae (σ2 s) = some false := by
  rw [eval_cpu]; simp [(inRange_c hX s).mp hi]

theorem eval_c2_t (ho : cx.overflows s) : eval .b (c2 s) = some true := by
  rw [eval_c2]; simp [ho]

theorem eval_c2_f (ho : ¬ cx.overflows s) : eval .b (c2 s) = some false := by
  rw [eval_c2]; simp [ho]

theorem eval_c3_t (hn : ¬ (cx.size s).toNat ≤ CALLOC_MAX) : eval .ae (c3 s) = some true := by
  rw [eval_c3]; simp only [CALLOC_MAX] at hn; simp; omega

theorem eval_c3_f (hn : (cx.size s).toNat ≤ CALLOC_MAX) : eval .ae (c3 s) = some false := by
  rw [eval_c3]; simp only [CALLOC_MAX] at hn; simp; omega

/-- Every step of the body up to the pop's, with the exits not taken. -/
theorem runBody_front (hp : Pre s) (hi : cx.InRange s) (ho : Ok s) :
    runBody isa body (σA s) = runBody isa (popSteps .r9 [] (some "slow")) (c7 s) := by
  rw [body, runBody_instr (exec_cpu hp.arm), runBody_instr exec_cmp,
    runBody_skip (eval_cpu_f hi), runBody_instr exec_shl_r8, runBody_instr exec_c1,
    runBody_instr exec_c2, runBody_skip (eval_c2_f ho.1), runBody_instr exec_c3,
    runBody_skip (eval_c3_f ho.2), runBody_instr exec_c4, runBody_instr (exec_c5 hp ho),
    runBody_instr exec_c6, runBody_instr exec_c7]

def frontStates (s : State) : List State :=
  [σA s, σ1 s, σ2 s, σ2 s, σ3 s, c1 s, c2 s, c2 s, c3 s, c3 s, c4 s, c5 s, c6 s]

theorem prefixes_front (hp : Pre s) (hi : cx.InRange s) (ho : Ok s) :
    bodyPrefixes isa body (σA s) = frontStates s ++
      bodyPrefixes isa (popSteps .r9 [] (some "slow")) (c7 s) := by
  rw [body, bodyPrefixes_instr (exec_cpu hp.arm), bodyPrefixes_instr exec_cmp,
    bodyPrefixes_skip (eval_cpu_f hi), bodyPrefixes_instr exec_shl_r8, bodyPrefixes_instr exec_c1,
    bodyPrefixes_instr exec_c2, bodyPrefixes_skip (eval_c2_f ho.1), bodyPrefixes_instr exec_c3,
    bodyPrefixes_skip (eval_c3_f ho.2), bodyPrefixes_instr exec_c4,
    bodyPrefixes_instr (exec_c5 hp ho), bodyPrefixes_instr exec_c6, bodyPrefixes_instr exec_c7]
  rfl

theorem end_front (hp : Pre s) (hi : cx.InRange s) (ho : Ok s) :
    bodyEnd isa body (σA s) = (bodyEnd isa (popSteps .r9 [] (some "slow")) (c7 s)).map (· + 13) := by
  rw [body, bodyEnd_instr (exec_cpu hp.arm), bodyEnd_instr exec_cmp,
    bodyEnd_skip (eval_cpu_f hi), bodyEnd_instr exec_shl_r8, bodyEnd_instr exec_c1,
    bodyEnd_instr exec_c2, bodyEnd_skip (eval_c2_f ho.1), bodyEnd_instr exec_c3,
    bodyEnd_skip (eval_c3_f ho.2), bodyEnd_instr exec_c4, bodyEnd_instr (exec_c5 hp ho),
    bodyEnd_instr exec_c6, bodyEnd_instr exec_c7]
  simp only [Option.map_map]; rfl

theorem ev_cpu' :
    Instr.events (.mov32 .r8 (.mem { base := .r10, disp := 4, seg := some .fs })) (σA s) (σ1 s) =
      [⟨area s + 4, 4, false, (cpu32 s).toNat⟩] := by
  simp only [Instr.events, cpuOp_ea, σA_mem, toNat_read4, m1]
  rw [readW_writeW_disj (cpu_cs_disjoint s) (by decide)]; rfl

theorem events_front (hp : Pre s) (hi : cx.InRange s) (ho : Ok s) :
    bodyEvents isa body (σA s) = [⟨area s + 4, 4, false, (cpu32 s).toNat⟩, ⟨ta s, 1, false, (kb s).toNat⟩] ++
      bodyEvents isa (popSteps .r9 [] (some "slow")) (c7 s) := by
  rw [body, bodyEvents_instr (exec_cpu hp.arm), bodyEvents_instr exec_cmp,
    bodyEvents_skip (eval_cpu_f hi), bodyEvents_instr exec_shl_r8, bodyEvents_instr exec_c1,
    bodyEvents_instr exec_c2, bodyEvents_skip (eval_c2_f ho.1), bodyEvents_instr exec_c3,
    bodyEvents_skip (eval_c3_f ho.2), bodyEvents_instr exec_c4,
    bodyEvents_instr (exec_c5 hp ho), bodyEvents_instr exec_c6, bodyEvents_instr exec_c7]
  simp only [ev_cpu', ev_c5 hp ho]; rfl

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

/-! ### An overflowing product -/

theorem runBody_O (hp : Pre s) (hi : cx.InRange s) (ho : cx.overflows s) :
    runBody isa body (σA s) = some (.exit "slow" (c2 s)) := by
  rw [body, runBody_instr (exec_cpu hp.arm), runBody_instr exec_cmp,
    runBody_skip (eval_cpu_f hi), runBody_instr exec_shl_r8, runBody_instr exec_c1,
    runBody_instr exec_c2, runBody_take (eval_c2_t ho) execBlock_nil']

theorem prefixes_O (hp : Pre s) (hi : cx.InRange s) (ho : cx.overflows s) :
    bodyPrefixes isa body (σA s) = [σA s, σ1 s, σ2 s, σ2 s, σ3 s, c1 s, c2 s] := by
  rw [body, bodyPrefixes_instr (exec_cpu hp.arm), bodyPrefixes_instr exec_cmp,
    bodyPrefixes_skip (eval_cpu_f hi), bodyPrefixes_instr exec_shl_r8, bodyPrefixes_instr exec_c1,
    bodyPrefixes_instr exec_c2, bodyPrefixes_take (eval_c2_t ho)]

theorem end_O (hp : Pre s) (hi : cx.InRange s) (ho : cx.overflows s) :
    bodyEnd isa body (σA s) = some 6 := by
  rw [body, bodyEnd_instr (exec_cpu hp.arm), bodyEnd_instr exec_cmp,
    bodyEnd_skip (eval_cpu_f hi), bodyEnd_instr exec_shl_r8, bodyEnd_instr exec_c1,
    bodyEnd_instr exec_c2, bodyEnd_take (eval_c2_t ho)]; rfl

theorem events_O (hp : Pre s) (hi : cx.InRange s) (ho : cx.overflows s) :
    bodyEvents isa body (σA s) = [⟨area s + 4, 4, false, (cpu32 s).toNat⟩] := by
  rw [body, bodyEvents_instr (exec_cpu hp.arm), bodyEvents_instr exec_cmp,
    bodyEvents_skip (eval_cpu_f hi), bodyEvents_instr exec_shl_r8, bodyEvents_instr exec_c1,
    bodyEvents_instr exec_c2, bodyEvents_take (eval_c2_t ho)]
  simp only [isa, ev_cpu']; rfl

/-! ### A total the table does not cover -/

theorem runBody_S (hp : Pre s) (hi : cx.InRange s) (ho : ¬ cx.overflows s)
    (hn : ¬ (cx.size s).toNat ≤ CALLOC_MAX) :
    runBody isa body (σA s) = some (.exit "slow" (c3 s)) := by
  rw [body, runBody_instr (exec_cpu hp.arm), runBody_instr exec_cmp,
    runBody_skip (eval_cpu_f hi), runBody_instr exec_shl_r8, runBody_instr exec_c1,
    runBody_instr exec_c2, runBody_skip (eval_c2_f ho), runBody_instr exec_c3,
    runBody_take (eval_c3_t hn) execBlock_nil']

theorem prefixes_S (hp : Pre s) (hi : cx.InRange s) (ho : ¬ cx.overflows s)
    (hn : ¬ (cx.size s).toNat ≤ CALLOC_MAX) :
    bodyPrefixes isa body (σA s) = [σA s, σ1 s, σ2 s, σ2 s, σ3 s, c1 s, c2 s, c2 s, c3 s] := by
  rw [body, bodyPrefixes_instr (exec_cpu hp.arm), bodyPrefixes_instr exec_cmp,
    bodyPrefixes_skip (eval_cpu_f hi), bodyPrefixes_instr exec_shl_r8, bodyPrefixes_instr exec_c1,
    bodyPrefixes_instr exec_c2, bodyPrefixes_skip (eval_c2_f ho), bodyPrefixes_instr exec_c3,
    bodyPrefixes_take (eval_c3_t hn)]

theorem end_S (hp : Pre s) (hi : cx.InRange s) (ho : ¬ cx.overflows s)
    (hn : ¬ (cx.size s).toNat ≤ CALLOC_MAX) : bodyEnd isa body (σA s) = some 8 := by
  rw [body, bodyEnd_instr (exec_cpu hp.arm), bodyEnd_instr exec_cmp,
    bodyEnd_skip (eval_cpu_f hi), bodyEnd_instr exec_shl_r8, bodyEnd_instr exec_c1,
    bodyEnd_instr exec_c2, bodyEnd_skip (eval_c2_f ho), bodyEnd_instr exec_c3,
    bodyEnd_take (eval_c3_t hn)]; rfl

theorem events_S (hp : Pre s) (hi : cx.InRange s) (ho : ¬ cx.overflows s)
    (hn : ¬ (cx.size s).toNat ≤ CALLOC_MAX) :
    bodyEvents isa body (σA s) = [⟨area s + 4, 4, false, (cpu32 s).toNat⟩] := by
  rw [body, bodyEvents_instr (exec_cpu hp.arm), bodyEvents_instr exec_cmp,
    bodyEvents_skip (eval_cpu_f hi), bodyEvents_instr exec_shl_r8, bodyEvents_instr exec_c1,
    bodyEvents_instr exec_c2, bodyEvents_skip (eval_c2_f ho), bodyEvents_instr exec_c3,
    bodyEvents_take (eval_c3_t hn)]
  simp only [isa, ev_cpu']; rfl

end Cases

/-! ## What each state keeps -/

section Mids

variable {s : State}

theorem mσ2 : Mid scratchC s (m1 s) (σ2 s) :=
  mid_σ2 (mid_σ1 (by decide) (mid_σA (by decide) (by decide)))
theorem mσ3 : Mid scratchC s (m1 s) (σ3 s) := mid_σ3 (by decide) mσ2
theorem mc1 : Mid scratchC s (m1 s) (c1 s) := mσ3.setReg (by decide) _
theorem mc2 : Mid scratchC s (m1 s) (c2 s) :=
  ((mc1.setFlags _ _ _ _).setReg (by decide) _).setReg (by decide) _
theorem mc3 : Mid scratchC s (m1 s) (c3 s) := mc2.arithFlags _ _ _
theorem mc4 : Mid scratchC s (m1 s) (c4 s) := mc3.setReg (by decide) _
theorem mc5 : Mid scratchC s (m1 s) (c5 s) := mc4.setReg (by decide) _
theorem mc6 : Mid scratchC s (m1 s) (c6 s) := mc5.setReg (by decide) _
theorem mc7 : Mid scratchC s (m1 s) (c7 s) := mc6.setReg (by decide) _

theorem mid_front : ∀ p ∈ frontStates s, Mid scratchC s (m1 s) p := by
  simp only [frontStates, List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
  exact ⟨mid_σA (by decide) (by decide), mid_σ1 (by decide) (mid_σA (by decide) (by decide)), mσ2, mσ2,
    mσ3, mc1, mc2, mc2, mc3, mc3, mc4, mc5, mc6⟩

end Mids

/-! ## The contract -/

section Contract

variable {s : State}

theorem exit_eq (t : String) (s' : State) : (callocContract abi).exit t s s' ↔
    [s'.gpr .rdi, s'.gpr .rsi] = [s.gpr .rdi, s.gpr .rsi] ∧
      ((t = SLOW ∧ cx.PopMissed s s') ∨ (t = ZERO ∧ ∃ x, cx.Popped s s' x ∧ cx.third s' = x)) :=
  exit_iff cx hv t s s'

theorem abi_m1 (hp : Pre s) {p : State} (hm : Mid scratchC s (m1 s) p) : abiPreserved s p :=
  abiPreserved_of_mid frame_m1 (by
    have := disj_res hX hp.world (x := (cx.csField s, true)) (mem_fixed cs_mem_fixed)
    rw [csField_c hX] at this; simpa using this) (by decide) hm

theorem unchanged_cs : Unchanged [cx.csField s] (cx.mem s) (m1 s) := by
  rw [csField_c hX, mem_c hX]; exact frame_m1

theorem missed (hc : cx.cls? s = none ∨ ¬ cx.InRange s) {p : State} (hm : Mid scratchC s (m1 s) p) :
    cx.PopMissed s p := by
  refine ⟨?_, by rw [hX.mem, hm.mem]; exact unchanged_cs⟩
  rcases hc with hc | hc
  · simp only [Spec.CEntry.Ctx.popResult, hc, Option.bind_none, ite_self]
  · simp only [Spec.CEntry.Ctx.popResult, hc, ↓reduceIte]

theorem exit_slow (hc : cx.cls? s = none ∨ ¬ cx.InRange s) {p : State} (hm : Mid scratchC s (m1 s) p) :
    (callocContract abi).exit "slow" s p :=
  (exit_eq _ _).mpr ⟨by rw [hm.gpr .rdi (by decide), hm.gpr .rsi (by decide)], .inl ⟨rfl, missed hc hm⟩⟩

theorem cls_none (ho : ¬ Ok s) : cx.cls? s = none := by
  have ho' : ¬ (¬ cx.overflows s ∧ (cx.size s).toNat ≤ CALLOC_MAX) := ho
  simp only [Spec.CEntry.Ctx.cls?, ho', ↓reduceIte]

/-- After the commit, outside the sequence: the block into `rdx`. -/
theorem exec_q : execBlock isa tail (Tail.p5 .r9 (c7 s)) = some (q s) := by
  simp only [tail, execBlock, exec, readSrc, Option.map_some]; rfl

theorem correct (hs : (callocContract abi).pre s) :
    (∃ s', Exec isa cCalloc s s' ∧ target.abiPreserved s s' ∧ (callocContract abi).post s s') ∨
    (∃ t s', ExecExit isa cCalloc s t s' ∧ target.abiPreserved s s' ∧
      (callocContract abi).exit t s s') := by
  have hp := pre_of hs
  rw [code_eq]
  by_cases hi : cx.InRange s
  · by_cases ho : cx.overflows s
    · exact .inr ⟨"slow", c2 s, .seqL (.rseq (execBlock_arm hp.arm) (runBody_O hp hi ho)),
        abi_m1 hp mc2, exit_slow (.inl (cls_none fun h => h.1 ho)) mc2⟩
    by_cases hn : (cx.size s).toNat ≤ CALLOC_MAX
    · have hok : Ok s := ⟨ho, hn⟩
      have hk := cls_eq hok
      have hb := backFacts hX hp.world hp.pre hk hi
      have hr := ready (s := s)
      have hpo := popOk hX hr hb hi
      by_cases ht : Tail.ptst .r9 (c7 s) = 0
      · have hm := Tail.mid_p2 (w := .r9) (s := s) mc7 (by decide)
        exact .inr ⟨"slow", _, .seqL (.rseq (execBlock_arm hp.arm)
          (by rw [runBody_front hp hi hok, Tail.runPop_miss hpo _ ht])),
          abi_m1 hp hm,
          (exit_eq _ _).mpr ⟨by rw [hm.gpr .rdi (by decide), hm.gpr .rsi (by decide)],
            .inl ⟨rfl, pop_missed hX hr hb hk hi ht, by rw [hX.mem, hm.mem]; exact unchanged_cs⟩⟩⟩
      · have hm5 := Tail.mid_p5 (w := .r9) (s := s) mc7 (by decide) (by decide)
        have hmq : Mid scratchC s _ (q s) := hm5.setReg (by decide) _
        exact .inr ⟨"zero", q s, .seqR (.rseq (execBlock_arm hp.arm)
          (by rw [runBody_front hp hi hok, Tail.runPop_hit hpo _ ht])) (.jump exec_q),
          abiPreserved_of_mid (by rw [hr.mem, wa_eq hX hr]; exact frame_two s.mem _ _ _ _)
            (by
              have := hb.ret_cs; have := hb.ret_word
              simpa using ⟨‹Region.Disjoint ⟨area s + 8, 8⟩ ⟨s.gpr .rsp, 8⟩›, this⟩)
            (by decide) hmq,
          (exit_eq _ _).mpr ⟨by rw [hmq.gpr .rdi (by decide), hmq.gpr .rsi (by decide)],
            .inr ⟨rfl, _, popped hX hr hb hk hi ht, by
              show (q s).gpr .rdx = _
              rw [q, RegUpd.gpr_setReg_self, Tail.p5_rax]⟩⟩⟩
    · exact .inr ⟨"slow", c3 s, .seqL (.rseq (execBlock_arm hp.arm) (runBody_S hp hi ho hn)),
        abi_m1 hp mc3, exit_slow (.inl (cls_none fun h => hn h.2)) mc3⟩
  · exact .inr ⟨"slow", σ2 s, .seqL (.rseq (execBlock_arm hp.arm) (runBody_C hp hi)), abi_m1 hp mσ2,
      exit_slow (.inr hi) mσ2⟩

/-! ### Where it may be aborted -/

theorem abort_eq (p : State) : (callocContract abi).abort s p ↔
    [p.gpr .rdi, p.gpr .rsi] = [s.gpr .rdi, s.gpr .rsi] ∧
    (Unchanged [cx.csField s] (cx.mem s) p.mem ∧ cx.inv p) :=
  abort_iff cx hv s p

theorem abortOk (hp : Pre s) {p : State} (hm : Mid scratchC s (m1 s) p) :
    armed p ∧ target.abiPreserved s p ∧ (callocContract abi).abort s p ∧
      ∀ p', target.rely p p' → (callocContract abi).env p p' → (callocContract abi).pre p' := by
  obtain ⟨hip, hre⟩ := retryC hX hv (post := fun s s' => _) (fun _ => rfl) (fun _ => rfl) (by decide)
    hp.pre hm (by decide) (by decide) frame_m1 (harmless_csC hX hp.pre)
  refine ⟨armed_of_mid hp.arm (xs := []) frame_m1 m1_cs (by simp) hm, abi_m1 hp hm, ?_, hre⟩
  exact (abort_eq p).mpr ⟨by rw [hm.gpr .rdi (by decide), hm.gpr .rsi (by decide)],
    by rw [hm.mem]; exact unchanged_cs, hip⟩

/-! ### Its accesses -/

theorem ofNat_toNat64 (v : BitVec 64) : BitVec.ofNat 64 v.toNat = v := by
  rw [BitVec.ofNat_toNat, BitVec.setWidth_eq]

theorem addr8 {s : State} : s.fsBase + o s + BitVec.ofNat 64 RSEQ_CS = area s + 8 := rfl
theorem addr4 {s : State} : s.fsBase + o s + BitVec.ofNat 64 RSEQ_CPU_ID = area s + 4 := rfl
theorem rsName : RSEQ_OFFSET = RS := rfl

theorem trace_ok (hs : (callocContract abi).pre s) :
    (callocContract abi).trace s (Code.events isa cCalloc s) := by
  have hp := pre_of hs
  show cx.CallocTrace s (Code.events isa (.seq (.rseq arm body) (.jump tail "zero")) s)
  simp only [Code.events, execBlock_arm hp.arm, events_arm hp.arm]
  have htp : cx.slab.tp s = s.fsBase := tp_of (c := cx.slab) hX s
  by_cases hi : cx.InRange s
  · have hi' : (cpu32 s).toNat < SLAB_COUNT := (inRange_c hX s).mp hi
    by_cases ho : cx.overflows s
    · refine ⟨(o s).toNat, s.csDesc.toNat, (cpu32 s).toNat, 0, 0, 0, 0, ?_⟩
      rw [events_O hp hi ho]
      simp only [Spec.CEntry.Ctx.head, Spec.CEntry.Ctx.rd, Spec.CEntry.Ctx.wr, htp, sym_c hX, addr8,
        addr4, rsName, ofNat_toNat64, ho, not_true_eq_false, false_and, and_false, ↓reduceIte,
        List.append_nil]
      rfl
    by_cases hn : (cx.size s).toNat ≤ CALLOC_MAX
    · have hok : Ok s := ⟨ho, hn⟩
      have hk := cls_eq hok
      have hb := backFacts hX hp.world hp.pre hk hi
      have hr := ready (s := s)
      have hpo := popOk hX hr hb hi
      have hw := wa_eq hX hr
      have hma := ma_eq hX hr
      refine ⟨(o s).toNat, s.csDesc.toNat, (cpu32 s).toNat, (kb s).toNat, (Tail.wv .r9 (c7 s)).toNat,
        (Tail.mk .r9 (c7 s)).toNat, (Tail.px .r9 (c7 s)).toNat, ?_⟩
      rw [events_front hp hi hok]
      have hword : wordAddr (s.syms SLABS) (cpu32 s).toNat (kb s).toNat = Tail.wa .r9 (c7 s) := by
        rw [hw]; simp only [Spec.CEntry.Ctx.word, sym_c hX, cpu_c hX]
      have hmask : s.syms SLABS + BitVec.ofNat 64 (8 * (kb s).toNat + MASK_DISP) = Tail.ma .r9 (c7 s) := by
        rw [hma, sym_c hX]
      simp only [Spec.CEntry.Ctx.head, Spec.CEntry.Ctx.popTail, Spec.CEntry.Ctx.rd, Spec.CEntry.Ctx.wr,
        htp, sym_c hX, hi', ho, hn, not_false_eq_true, and_self, ↓reduceIte, hword, hmask,
        ofNat_toNat64, addr8, addr4, rsName]
      by_cases ht : Tail.ptst .r9 (c7 s) = 0
      · rw [Tail.eventsPop_miss hpo _ ht]
        have : BitVec.ofNat 32 (Tail.wv .r9 (c7 s)).toNat &&& BitVec.ofNat 32 (Tail.mk .r9 (c7 s)).toNat = 0 := by
          rw [BitVec.ofNat_toNat, BitVec.ofNat_toNat, BitVec.setWidth_eq]; exact ht
        simp only [this, ne_eq, not_true_eq_false, ↓reduceIte, List.append_nil]; rfl
      · rw [Tail.eventsPop_hit hpo _ ht]
        have : ¬ BitVec.ofNat 32 (Tail.wv .r9 (c7 s)).toNat &&& BitVec.ofNat 32 (Tail.mk .r9 (c7 s)).toNat = 0 := by
          rw [BitVec.ofNat_toNat, BitVec.ofNat_toNat, BitVec.setWidth_eq]; exact ht
        simp only [this, ne_eq, not_false_eq_true, ↓reduceIte]; rfl
    · refine ⟨(o s).toNat, s.csDesc.toNat, (cpu32 s).toNat, 0, 0, 0, 0, ?_⟩
      rw [events_S hp hi ho hn]
      simp only [Spec.CEntry.Ctx.head, Spec.CEntry.Ctx.rd, Spec.CEntry.Ctx.wr, htp, sym_c hX, addr8,
        addr4, rsName, ofNat_toNat64, hn, and_false, ↓reduceIte, List.append_nil]
      rfl
  · have hi' : ¬ (cpu32 s).toNat < SLAB_COUNT := fun h => hi ((inRange_c hX s).mpr h)
    refine ⟨(o s).toNat, s.csDesc.toNat, (cpu32 s).toNat, 0, 0, 0, 0, ?_⟩
    rw [events_C hp hi]
    simp only [Spec.CEntry.Ctx.head, Spec.CEntry.Ctx.rd, Spec.CEntry.Ctx.wr, htp, sym_c hX, addr8,
      addr4, rsName, ofNat_toNat64, hi', false_and, ↓reduceIte, List.append_nil]
    rfl

theorem rseqOk (hs : (callocContract abi).pre s) : RseqOk target cCalloc (callocContract abi) s := by
  have hp := pre_of hs
  have ht := trace_ok hs
  rw [code_eq] at ht ⊢
  unfold RseqOk
  refine ⟨ht, ⟨_, _, rfl, execBlock_dropLast_arm hp.arm, arms_arm⟩, ?_⟩
  have mA : Mid scratchC s (m1 s) (σA s) := mid_σA (by decide) (by decide)
  have m1' : Mid scratchC s (m1 s) (σ1 s) := mid_σ1 (by decide) mA
  simp only [Code.abortStates, execBlock_arm hp.arm]
  by_cases hi : cx.InRange s
  · by_cases ho : cx.overflows s
    · rw [prefixes_O hp hi ho]
      simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
      exact ⟨abortOk hp mA, abortOk hp m1', abortOk hp mσ2, abortOk hp mσ2, abortOk hp mσ3,
        abortOk hp mc1, abortOk hp mc2⟩
    by_cases hn : (cx.size s).toNat ≤ CALLOC_MAX
    · have hok : Ok s := ⟨ho, hn⟩
      have hk := cls_eq hok
      have hb := backFacts hX hp.world hp.pre hk hi
      have hr := ready (s := s)
      have hpo := popOk hX hr hb hi
      rw [prefixes_front hp hi hok]
      intro p hp'
      rcases List.mem_append.mp hp' with hp' | hp'
      · exact abortOk hp (mid_front p hp')
      · by_cases ht : Tail.ptst .r9 (c7 s) = 0
        · rw [Tail.prefixesPop_miss hpo _ ht] at hp'
          simp only [List.mem_cons, List.not_mem_nil, or_false] at hp'
          rcases hp' with rfl | rfl | rfl
          · exact abortOk hp mc7
          · exact abortOk hp (Tail.mid_t1 mc7 (by decide))
          · exact abortOk hp (Tail.mid_p2 mc7 (by decide))
        · rw [Tail.prefixesPop_hit hpo _ ht] at hp'
          simp only [List.mem_cons, List.not_mem_nil, or_false] at hp'
          rcases hp' with rfl | rfl | rfl | rfl | rfl | rfl
          · exact abortOk hp mc7
          · exact abortOk hp (Tail.mid_t1 mc7 (by decide))
          · exact abortOk hp (Tail.mid_p2 mc7 (by decide))
          · exact abortOk hp (Tail.mid_p2 mc7 (by decide))
          · exact abortOk hp (Tail.mid_p3 mc7 (by decide))
          · exact abortOk hp (Tail.mid_p4 mc7 (by decide) (by decide))
    · rw [prefixes_S hp hi ho hn]
      simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
      exact ⟨abortOk hp mA, abortOk hp m1', abortOk hp mσ2, abortOk hp mσ2, abortOk hp mσ3,
        abortOk hp mc1, abortOk hp mc2, abortOk hp mc2, abortOk hp mc3⟩
  · rw [prefixes_C hp hi]
    simp only [List.forall_mem_cons, List.not_mem_nil, false_implies, implies_true, and_true]
    exact ⟨abortOk hp mA, abortOk hp m1', abortOk hp mσ2⟩

end Contract

end VG.Proof.CEntry.X86_64.Calloc
