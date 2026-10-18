module

public import Asmalloc.Proof.Slab.X86_64.Word

/-!
# The slow path's routines on x86-64: their word

`mov r9d, edi; and r9d, 4088; add r8, r9; lea r9, [rip + SLABS];
mov rax, [r9 + r8]`: the word at the byte offset in `rdi`, rounded down to
a word of the slab, read once. And what the routines may rely on of their
entry state (`SFacts`).
-/

@[expose] public section

namespace VG.Proof.Slab.X86_64

open VG.X86_64 VG.Spec.Slab VG.Impl.Slab.X86_64

/-! ## The offset, rounded down to a word of the slab -/

theorem testBit_4088 (i : Nat) : (4088 : Nat).testBit i = decide (3 ≤ i ∧ i < 12) := by
  by_cases h : i < 12
  · have : ∀ j < 12, (4088 : Nat).testBit j = decide (3 ≤ j ∧ j < 12) := by decide
    exact this i h
  · rw [Nat.testBit_lt_two_pow (Nat.lt_of_lt_of_le (by decide : 4088 < 2 ^ 12)
      (Nat.pow_le_pow_right (by decide) (by omega)))]
    simp [h]

/-- `and r9d, 4088` of the offset's low 32 bits: the offset rounded down to a
multiple of 8, within the slab. -/
theorem and_4088 (x : Nat) : x % 2 ^ 32 &&& 4088 = 8 * (x % 4096 / 8) := by
  have h : 8 * (x % 4096 / 8) = (x % 2 ^ 12) >>> 3 <<< 3 := by
    rw [Nat.shiftLeft_eq, Nat.shiftRight_eq_div_pow]; omega
  rw [h]
  apply Nat.eq_of_testBit_eq
  intro i
  rw [Nat.testBit_and, Nat.testBit_mod_two_pow, testBit_4088, Nat.testBit_shiftLeft,
    Nat.testBit_shiftRight, Nat.testBit_mod_two_pow]
  by_cases h3 : 3 ≤ i
  · rw [show 3 + (i - 3) = i by omega]
    by_cases h12 : i < 12
    · simp [h3, h12, show i < 32 by omega]
    · simp [h3, h12]
  · simp [h3]

/-! ## The states -/

/-- The offset's low 32 bits, rounded down to a word of the slab. -/
def offw (s : State) : BitVec 32 := (s.gpr .rdi).setWidth 32 &&& 4088
/-- The word's offset in `SLABS`, its address, and the word. -/
def r8o (s : State) : BitVec 64 := sh s + (offw s).setWidth 64
def wao (s : State) : Addr := s.syms "SLABS" + r8o s
def wvo (s : State) : BitVec 64 := s.mem.readW (wao s) 64

def σ4o (s : State) : State := (σ3 s).setReg32 .r9 ((s.gpr .rdi).setWidth 32)
def σ5o (s : State) : State := (arithFlags (σ4o s) (offw s) false false).setReg32 .r9 (offw s)
def σ6o (s : State) : State :=
  (arithFlags (σ5o s) (r8o s) (decide (2 ^ 64 ≤ (sh s).toNat + ((offw s).setWidth 64).toNat))
    (addOverflow (sh s) ((offw s).setWidth 64) (r8o s))).setReg .r8 (r8o s)
def σ7o (s : State) : State := (σ6o s).setReg .r9 (s.syms "SLABS")
def σ8o (s : State) : State := (σ7o s).setReg .rax (wvo s)

section

variable {c : Spec.Slab.Ctx isa} {s : State}

theorem exec_off1 : exec (.mov32 .r9 (.reg .rdi)) (σ3 s) = some (σ4o s) := by
  simp only [exec, readSrc32, Option.map_some, σ3_rdi]; rfl

theorem σ4o_r9 : (σ4o s).gpr .r9 = ((s.gpr .rdi).setWidth 32).setWidth 64 := by
  simp [σ4o, State.setReg32, RegUpd.gpr_setReg_self]

theorem exec_off2 : exec (.alu32 .and .r9 (.imm 4088)) (σ4o s) = some (σ5o s) := by
  simp only [exec, execAlu32, readSrc32, Option.bind_some, σ4o_r9, RegUpd.setWidth_setWidth_32]; rfl

theorem σ5o_r8 : (σ5o s).gpr .r8 = sh s := by
  simp only [σ5o, State.setReg32, RegUpd.gpr_setReg_of_ne _ _ (show ¬ Reg.r8 = .r9 by decide),
    RegUpd.gpr_arithFlags, σ4o, gpr_σ3_r8]
theorem σ5o_r9 : (σ5o s).gpr .r9 = (offw s).setWidth 64 := by
  simp [σ5o, State.setReg32, RegUpd.gpr_setReg_self]

theorem exec_off3 : exec (.alu .add .r8 (.reg .r9)) (σ5o s) = some (σ6o s) := by
  simp only [exec, execAlu, readSrc, Option.bind_some, σ5o_r8, σ5o_r9]; rfl

theorem exec_off4 : exec (.leaSym .r9 "SLABS") (σ6o s) = some (σ7o s) := by
  rw [exec_leaSym0]; rfl

theorem σ7o_r9 : (σ7o s).gpr .r9 = s.syms "SLABS" := RegUpd.gpr_setReg_self _ _ _
theorem σ7o_r8 : (σ7o s).gpr .r8 = r8o s := by
  rw [σ7o, RegUpd.gpr_setReg_of_ne _ _ (by decide), σ6o, RegUpd.gpr_setReg_self]
theorem σ7o_rd : (σ7o s).rd = s.rd := rfl
theorem σ7o_wr : (σ7o s).wr = s.wr := rfl
theorem σ7o_mem : (σ7o s).mem = m1 s := rfl

/-- `[r9 + r8]`, in any state where they hold `SLABS` and the word's offset. -/
theorem slabWord_ea_o (σ : State) (h9 : σ.gpr .r9 = s.syms "SLABS") (h8 : σ.gpr .r8 = r8o s) :
    σ.ea slabWord = wao s := by
  rw [slabWord, Access.ea_of_seg_none rfl, Access.offset_index, h9, h8]
  simp only [BitVec.ofInt_ofNat, BitVec.mul_one, BitVec.add_zero]
  rfl

/-- The word's address is the routine's, a slow-path routine being given a
byte offset in `rdi`. -/
theorem wao_eq (hA : X86 c.A) (hcls : c.cls s = (s.gpr .rdi).toNat % SLAB_BYTES / 8) :
    wao s = c.word s := by
  rw [Spec.Slab.Ctx.word, wordAddr, sym_of hA, cpu_of hA, wao, r8o, sh_eq, cpu32, hcls]
  apply BitVec.eq_of_toNat_eq
  have hoff : ((offw s).setWidth 64).toNat = 8 * ((s.gpr .rdi).toNat % 4096 / 8) := by
    rw [BitVec.toNat_setWidth, offw, BitVec.toNat_and, BitVec.toNat_setWidth]
    rw [show (4088 : BitVec 32).toNat = 4088 from rfl, and_4088]
    have : 8 * ((s.gpr .rdi).toNat % 4096 / 8) < 2 ^ 64 := by omega
    exact Nat.mod_eq_of_lt (by omega)
  simp only [BitVec.toNat_add, BitVec.toNat_ofNat, hoff, SLABS, SLAB_BYTES, SLAB_SHIFT]
  omega

theorem exec_word (hin : InRegions (s.rd ++ s.wr) (wao s) 8)
    (hdj : Region.Disjoint ⟨wao s, 8⟩ ⟨area s + 8, 8⟩) :
    exec (.mov .rax (.mem slabWord)) (σ7o s) = some (σ8o s) := by
  have hin' : InRegions ((σ7o s).rd ++ (σ7o s).wr) ((σ7o s).ea slabWord) 8 := by
    rw [slabWord_ea_o _ σ7o_r9 σ7o_r8, σ7o_rd, σ7o_wr]; exact hin
  rw [exec_load _ hin', slabWord_ea_o _ σ7o_r9 σ7o_r8, σ7o_mem, m1, readW64_writeW_disj hdj]
  rfl

theorem ev_word (hdj : Region.Disjoint ⟨wao s, 8⟩ ⟨area s + 8, 8⟩) :
    Instr.events (.mov .rax (.mem slabWord)) (σ7o s) (σ8o s) = [⟨wao s, 8, false, (wvo s).toNat⟩] := by
  simp only [Instr.events, slabWord_ea_o _ σ7o_r9 σ7o_r8, σ7o_mem, toNat_read8, m1]
  rw [readW64_writeW_disj hdj]; rfl

theorem σ8o_rax : (σ8o s).gpr .rax = wvo s := RegUpd.gpr_setReg_self _ _ _
theorem σ8o_r9 : (σ8o s).gpr .r9 = s.syms "SLABS" := by
  rw [σ8o, RegUpd.gpr_setReg_of_ne _ _ (by decide), σ7o_r9]
theorem σ8o_r8 : (σ8o s).gpr .r8 = r8o s := by
  rw [σ8o, RegUpd.gpr_setReg_of_ne _ _ (by decide), σ7o_r8]
theorem σ8o_rd : (σ8o s).rd = s.rd := rfl
theorem σ8o_wr : (σ8o s).wr = s.wr := rfl
theorem σ8o_mem : (σ8o s).mem = m1 s := rfl

theorem gpr_σ8o (r : Reg) (hr : r ∉ scratchT) : (σ8o s).gpr r = s.gpr r := by
  have hm : Mid scratchT s (m1 s) (σ3 s) := mid_σ3 (by decide) (mid_σ2 (mid_σ1 (by decide)
    (mid_σA (by decide) (by decide))))
  have h8 : Mid scratchT s (m1 s) (σ8o s) :=
    (((((hm.setReg32 (by decide) _).arithFlags _ _ _).setReg32 (by decide) _).arithFlags _ _ _
      |>.setReg (by decide) _).setReg (by decide) _).setReg (by decide) _
  exact h8.gpr r hr

theorem mσ8o : Mid scratchT s (m1 s) (σ8o s) :=
  (((((((mid_σ3 (by decide) (mid_σ2 (mid_σ1 (by decide) (mid_σA (by decide) (by decide))))).setReg32
    (by decide) _).arithFlags _ _ _).setReg32 (by decide) _).arithFlags _ _ _).setReg (by decide) _
    ).setReg (by decide) _).setReg (by decide) _

theorem mσ4o : Mid scratchT s (m1 s) (σ4o s) :=
  (mid_σ3 (by decide) (mid_σ2 (mid_σ1 (by decide) (mid_σA (by decide) (by decide))))).setReg32
    (by decide) _
theorem mσ5o : Mid scratchT s (m1 s) (σ5o s) := (mσ4o.arithFlags _ _ _).setReg32 (by decide) _
theorem mσ6o : Mid scratchT s (m1 s) (σ6o s) := (mσ5o.arithFlags _ _ _).setReg (by decide) _
theorem mσ7o : Mid scratchT s (m1 s) (σ7o s) := mσ6o.setReg (by decide) _

end

/-! ## What a slow-path routine may rely on -/

/-- What a slow-path routine `c` may rely on in its entry state `s`. -/
structure SFacts (c : Spec.Slab.Ctx isa) (s : State) : Prop where
  rsVar : ⟨s.syms RS, 8⟩ ∈ s.rd
  cpuF : (⟨area s + 4, 4⟩ : Region) ∈ s.rd
  cs : (⟨area s + 8, 8⟩ : Region) ∈ s.wr
  word : c.InRange s → ⟨c.word s, 8⟩ ∈ s.wr
  cs_rsVar : Region.Disjoint ⟨area s + 8, 8⟩ ⟨s.syms RS, 8⟩
  cs_word : c.InRange s → Region.Disjoint ⟨area s + 8, 8⟩ ⟨c.word s, 8⟩
  ret_cs : Region.Disjoint ⟨area s + 8, 8⟩ ⟨s.gpr .rsp, 8⟩
  ret_word : c.InRange s → Region.Disjoint ⟨c.word s, 8⟩ ⟨s.gpr .rsp, 8⟩
  link : c.form = .stackPush → c.link s ∈ s.wr
  link_cs : c.form = .stackPush → Region.Disjoint (c.link s) ⟨area s + 8, 8⟩
  link_word : c.form = .stackPush → c.InRange s → Region.Disjoint (c.link s) ⟨c.word s, 8⟩
  link_rsVar : c.form = .stackPush → Region.Disjoint ⟨s.syms RS, 8⟩ (c.link s)
  link_ret : c.form = .stackPush → Region.Disjoint (c.link s) ⟨s.gpr .rsp, 8⟩
  top : c.form = .stackPop → c.InRange s → (c.w s).toNat ≠ 0 → (c.w s).toNat % 2 = 0 →
    ⟨c.w s, 8⟩ ∈ s.rd
  top_cs : c.form = .stackPop → c.InRange s → (c.w s).toNat ≠ 0 → (c.w s).toNat % 2 = 0 →
    Region.Disjoint ⟨c.w s, 8⟩ ⟨area s + 8, 8⟩
  pre : c.pre s

section Of

variable {c : Spec.Slab.Ctx isa} {s : State}

theorem mem_ro {x : Region} (h : (x, false) ∈ c.regions s) : x ∈ c.ro s :=
  List.mem_map.mpr ⟨(x, false), List.mem_filter.mpr ⟨h, rfl⟩, rfl⟩

theorem mem_rw {x : Region} (h : (x, true) ∈ c.regions s) : x ∈ c.rw s :=
  List.mem_map.mpr ⟨(x, true), List.mem_filter.mpr ⟨h, rfl⟩, rfl⟩

theorem fixed_mem {x : Region × Bool} (h : x ∈ c.fixed s) : x ∈ c.regions s :=
  List.mem_append_left _ h

theorem word_mem (hi : c.InRange s) : (⟨c.word s, 8⟩, true) ∈ c.regions s := by
  simp only [Spec.Slab.Ctx.regions, hi, ↓reduceIte]
  exact List.mem_append_right _ (List.mem_cons_self ..)

theorem more_mem (hi : c.InRange s) {x : Region × Bool} (h : x ∈ c.more s) : x ∈ c.regions s := by
  simp only [Spec.Slab.Ctx.regions, hi, ↓reduceIte]
  exact List.mem_append_right _ (List.mem_cons_of_mem _ h)

theorem cpu_mem_fixed : (c.cpuField s, false) ∈ c.fixed s :=
  List.mem_append_left _ (List.mem_cons_of_mem _ (List.mem_cons_self ..))

/-- A stack push's magazine's link slot is apart from `rseq_cs`. -/
theorem link_cs_disj (hf : c.form = .stackPush) (hl : c.Layout s) :
    (c.link s).Disjoint (c.csField s) := by
  obtain ⟨A, f, sig⟩ := c
  cases hf
  exact (fixed_disj (a := (Spec.Slab.Ctx.csField ⟨A, .stackPush, sig⟩ s, true))
    (l₁ := [(Spec.Slab.Ctx.rsVar ⟨A, .stackPush, sig⟩ s, false),
      (Spec.Slab.Ctx.cpuField ⟨A, .stackPush, sig⟩ s, false)]) hl rfl (List.mem_singleton_self _)
    (Bool.true_or _)).symm

end Of

/-- A slow-path routine's facts, from the precondition's: its regions are
the state's, and the layout and the invariant place them apart. -/
theorem sfacts_of_world {c : Spec.Slab.Ctx isa} (hA : X86 c.A) {s : State}
    (hw : c.World s) (hp : c.pre s) : SFacts c s := by
  have hl : c.Layout s := hp.2.2.2.1
  have hc : c.cls s < CLASS_LIMIT := cls_lt hp.2.2.1
  have hrd : s.rd = c.ro s := by rw [← hA.rd]; exact hw.rd
  have hwr : s.wr = c.rw s := by rw [← hA.wr]; exact hw.wr
  have hret := hA.retSlot s
  have hcs := csField_of hA s
  have hrs := rsVar_of hA s
  have hcpu := cpuField_of hA s
  refine {
    rsVar := ?_, cpuF := ?_, cs := ?_, word := fun hi => ?_, cs_rsVar := ?_, cs_word := fun hi => ?_,
    ret_cs := ?_, ret_word := fun hi => ?_, link := fun hf => ?_, link_cs := fun hf => ?_,
    link_word := fun hf hi => ?_, link_rsVar := fun hf => ?_, link_ret := fun hf => ?_,
    top := fun hf hi h0 h2 => ?_, top_cs := fun hf hi h0 h2 => ?_, pre := hp }
  · rw [hrd, ← hrs]; exact mem_ro (fixed_mem rsVar_mem_fixed)
  · rw [hrd, ← hcpu]; exact mem_ro (fixed_mem cpu_mem_fixed)
  · rw [hwr, ← hcs]; exact mem_rw (fixed_mem cs_mem_fixed)
  · rw [hwr]; exact mem_rw (word_mem hi)
  · rw [← hcs, ← hrs]; exact (disj_cs_rsVar hl).symm
  · rw [← hcs]; exact (hl.slabs (c.csField s, true) cs_mem_fixed).sub_right (word_sub hc hi)
  · rw [← hcs]; exact hl.res (c.csField s, true) cs_mem_fixed _ hret
  · exact (hl.slabs_res _ hret).sub_left (word_sub hc hi)
  · rw [hwr]; exact mem_rw (fixed_mem (link_mem_fixed hf))
  · rw [← hcs]; exact link_cs_disj hf hl
  · exact (hl.slabs (c.link s, true) (link_mem_fixed hf)).sub_right (word_sub hc hi)
  · rw [← hrs]; exact (harmless_link hf hl).2.1.symm
  · exact hl.res (c.link s, true) (link_mem_fixed hf) _ hret
  · rw [hrd]
    refine mem_ro (more_mem hi ?_)
    obtain ⟨A, f, sig⟩ := c
    cases hf
    simp [Spec.Slab.Ctx.more, h0, h2]
  · have hm : (⟨c.w s, 8⟩, false) ∈ c.more s := by
      obtain ⟨A, f, sig⟩ := c
      cases hf
      simp [Spec.Slab.Ctx.more, h0, h2]
    rw [← hcs]
    exact (more_apart hp.2.2.2.2 hi _ hm).2.2.1 (c.csField s, true) cs_mem_fixed

theorem ev_cpuLoad {s : State} :
    Instr.events (.mov32 .r8 (.mem { base := .r10, disp := 4, seg := some .fs })) (σA s) (σ1 s) =
      [⟨area s + 4, 4, false, (cpu32 s).toNat⟩] := by
  simp only [Instr.events, cpuOp_ea, σA_mem, toNat_read4, m1]
  rw [readW_writeW_disj (cpu_cs_disjoint s) (by decide)]; rfl

theorem SFacts.toArm {c : Spec.Slab.Ctx isa} {s : State} (h : SFacts c s) : ArmOk s :=
  ⟨h.rsVar, h.cpuF, h.cs, h.cs_rsVar⟩

end VG.Proof.Slab.X86_64
