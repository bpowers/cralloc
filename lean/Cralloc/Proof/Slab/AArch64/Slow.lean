module

public import Cralloc.Proof.Slab.AArch64.Setup

/-!
# The slow path's routines on AArch64: their word

Their setup forms the slabs' address too, in `x13`, which a retry keeps (`readyS`). Their
body is `mov x3, #2`, the CPU's steps, then `and x9, x0, #0xff8; add x8, x8, x9;
ldr x3, [x13, x8]`: the word at the byte offset in `x0`, rounded down to a word of the slab,
read once. `x3` holds what the routine returns, `NO_CPU` until then, and after the sequence
`mov x0, x3` returns it (`ret0`). And what the routines may rely on of the state an attempt
starts in (`SFacts`).
-/

@[expose] public section

namespace VG.Proof.Slab.AArch64

open VG.AArch64 VG.Spec.Slab VG.Impl.Slab.AArch64

/-- The registers an attempt of one of these routines writes, up to its commit. -/
def scratchS : List Reg := [.x3, .x4, .x8, .x9]

/-- What their setup leaves, which a sequence reads and no sequence writes. -/
def readyS (s : State) : Prop := ready s ∧ s.gpr .x13 = s.syms "SLABS"

/-- The contract of an attempt of such a routine, of the contract `k`. -/
abbrev withReadyS (k : Contract isa) : Contract isa := { k with pre := fun s => k.pre s ∧ readyS s }

/-! ## The offset, rounded down to a word of the slab -/

/-- `and x9, x0, #0xff8`: the offset rounded down to a multiple of 8, within the slab. -/
theorem and_ff8 (x : BitVec 64) :
    (x &&& BitVec.ofNat 64 0xff8).toNat = 8 * (x.toNat % 4096 / 8) := by
  rw [BitVec.toNat_and, show (BitVec.ofNat 64 0xff8).toNat = 4088 from rfl, and_4088]

/-! ## The states -/

def offw (s : State) : BitVec 64 := s.gpr .x0 &&& BitVec.ofNat 64 0xff8
/-- The word's offset in `SLABS`, its address, and the word. -/
def r8o (s : State) : BitVec 64 := sh s + offw s
def wao (s : State) : Addr := s.gpr .x13 + r8o s
def wvo (s : State) : BitVec 64 := s.mem.readW (wao s) 64

/-- No slab's result. -/
def σ0o (s : State) : State := (σA s).setReg .x3 (BitVec.ofNat 64 NO_CPU)
abbrev σ1o (s : State) : State := cpu1 (σ0o s) s
abbrev σ2o (s : State) : State := cpu2 (σ0o s) s
abbrev σ3o (s : State) : State := cpu3 (σ0o s) s
def σ4o (s : State) : State := (σ3o s).setReg .x9 (offw s)
def σ5o (s : State) : State := (σ4o s).setReg .x8 (r8o s)
def σ6o (s : State) : State := (σ5o s).setReg .x3 (wvo s)

/-- What follows the sequence: `mov x0, x3`. -/
def ret0 (p : State) : State := p.setReg .x0 (p.gpr .x3)

theorem execBlock_ret (p : State) : execBlock isa retWord p = some (ret0 p) := rfl

section

variable {s : State}

theorem exec_x3 : exec (.mov .x .x3 (.imm 2)) (σA s) = some (σ0o s) := rfl

theorem σ0o_gpr {r : Reg} (h3 : ¬ r = .x3) : (σ0o s).gpr r = s.gpr r := by
  rw [σ0o, RegUpd.gpr_setReg_of_ne _ _ h3]; rfl

theorem baseS (hr : ready s) : Base s (σ0o s) := ⟨rfl, rfl, rfl, (σ0o_gpr (by decide)).trans hr.1⟩

theorem ret_σ2o : (ret0 (σ2o s)).gpr .x0 = BitVec.ofNat 64 NO_CPU := by
  rw [ret0, RegUpd.gpr_setReg_self, gpr_cpu2 (by decide), σ0o, RegUpd.gpr_setReg_self]

theorem σ3o_x0 : (σ3o s).gpr .x0 = s.gpr .x0 := (gpr_cpu3 (by decide)).trans (σ0o_gpr (by decide))

theorem exec_off1 : exec (.and .x .x9 .x0 (.imm 0xff8)) (σ3o s) = some (σ4o s) := by
  show some ((σ3o s).setReg .x9 ((σ3o s).gpr .x0 &&& BitVec.ofNat 64 0xff8)) = _
  rw [σ3o_x0]
  rfl

theorem σ4o_x8 : (σ4o s).gpr .x8 = sh s := by
  rw [σ4o, RegUpd.gpr_setReg_of_ne _ _ (by decide), cpu3_x8]
theorem σ4o_x9 : (σ4o s).gpr .x9 = offw s := RegUpd.gpr_setReg_self _ _ _

theorem exec_off2 : exec (.add .x .x8 .x8 (.reg .x9)) (σ4o s) = some (σ5o s) := by
  show some ((σ4o s).setReg .x8 ((σ4o s).gpr .x8 + (σ4o s).gpr .x9)) = _
  rw [σ4o_x8, σ4o_x9]
  rfl

theorem σ5o_x8 : (σ5o s).gpr .x8 = r8o s := RegUpd.gpr_setReg_self _ _ _
theorem σ5o_rd : (σ5o s).rd = s.rd := rfl
theorem σ5o_wr : (σ5o s).wr = s.wr := rfl
theorem σ5o_mem : (σ5o s).mem = m1 s := rfl

/-- `[x13, x8]`, in any state where they hold the slabs' address and the word's offset. -/
theorem slabWord_ea (σ : State) (h13 : σ.gpr .x13 = s.gpr .x13) (h8 : σ.gpr .x8 = r8o s) :
    σ.ea slabWord = wao s := by
  rw [slabWord, Access.ea_reg, h13, h8]
  rfl

theorem mσAo : Mid scratchS s (m1 s) (σA s) := mid_σA
theorem mσ0o : Mid scratchS s (m1 s) (σ0o s) := mσAo.setReg (by decide) _
theorem mσ1o : Mid scratchS s (m1 s) (σ1o s) := mσ0o.cpu1 (by decide)
theorem mσ2o : Mid scratchS s (m1 s) (σ2o s) := mσ0o.cpu2 (by decide)
theorem mσ3o : Mid scratchS s (m1 s) (σ3o s) := mσ0o.cpu3 (by decide)
theorem mσ4o : Mid scratchS s (m1 s) (σ4o s) := mσ3o.setReg (by decide) _
theorem mσ5o : Mid scratchS s (m1 s) (σ5o s) := mσ4o.setReg (by decide) _
theorem mσ6o : Mid scratchS s (m1 s) (σ6o s) := mσ5o.setReg (by decide) _

theorem σ5o_x13 : (σ5o s).gpr .x13 = s.gpr .x13 := mσ5o.gpr _ (by decide)

theorem exec_word' (hin : InRegions (s.rd ++ s.wr) (wao s) 8)
    (hdj : Region.Disjoint ⟨wao s, 8⟩ ⟨area s + 8, 8⟩) :
    exec (.ldr .x .x3 slabWord) (σ5o s) = some (σ6o s) := by
  have hin' : InRegions ((σ5o s).rd ++ (σ5o s).wr) ((σ5o s).ea slabWord) 8 := by
    rw [slabWord_ea _ σ5o_x13 σ5o_x8, σ5o_rd, σ5o_wr]; exact hin
  rw [Access.exec_ldr_x hin', slabWord_ea _ σ5o_x13 σ5o_x8, σ5o_mem, m1, readW64_writeW_disj hdj]
  rfl

theorem ev_word' (hdj : Region.Disjoint ⟨wao s, 8⟩ ⟨area s + 8, 8⟩) :
    Instr.events (.ldr .x .x3 slabWord) (σ5o s) (σ6o s) = [⟨wao s, 8, false, (wvo s).toNat⟩] := by
  simp only [Instr.events, slabWord_ea _ σ5o_x13 σ5o_x8, Width.bytes, σ5o_mem, toNat_read8, m1]
  rw [readW64_writeW_disj hdj]; rfl

theorem σ6o_x3 : (σ6o s).gpr .x3 = wvo s := RegUpd.gpr_setReg_self _ _ _
theorem σ6o_x8 : (σ6o s).gpr .x8 = r8o s := by
  rw [σ6o, RegUpd.gpr_setReg_of_ne _ _ (by decide), σ5o_x8]
theorem σ6o_x13 : (σ6o s).gpr .x13 = s.gpr .x13 := mσ6o.gpr _ (by decide)
theorem σ6o_rd : (σ6o s).rd = s.rd := rfl
theorem σ6o_wr : (σ6o s).wr = s.wr := rfl
theorem σ6o_mem : (σ6o s).mem = m1 s := rfl

theorem gpr_σ6o {r : Reg} (hr : r ∉ scratchS) : (σ6o s).gpr r = s.gpr r := mσ6o.gpr r hr

/-- The calling convention's registers, wherever these routines are. -/
theorem abi_of {m : Mem} {p : State} (hm : Mid scratchS s m p) : abiPreserved s p :=
  abiPreserved_of_mid (by decide) hm

/-- And after the sequence. -/
theorem abi_ret {m : Mem} {p : State} (hm : Mid scratchS s m p) : abiPreserved s (ret0 p) := by
  refine ⟨fun r hr => ?_, hm.sp⟩
  rw [ret0, RegUpd.gpr_setReg_of_ne _ _ (fun h => by subst h; exact absurd hr (by decide))]
  exact (abi_of hm).1 r hr

/-! ## A head's lowest bit -/

theorem eval_marker (hm : (wvo s).toNat % 2 = 1) : eval (.tbnz .x3 0) (σ6o s) = some true := by
  rw [Flags.eval_tbnz, σ6o_x3]
  simp [BitVec.getLsbD, Nat.testBit_zero, hm]

theorem eval_even (hm : ¬ (wvo s).toNat % 2 = 1) : eval (.tbnz .x3 0) (σ6o s) = some false := by
  rw [Flags.eval_tbnz, σ6o_x3]
  simp [BitVec.getLsbD, Nat.testBit_zero, hm]

end

/-- The word's address is the routine's, a slow-path routine being given a byte offset in `x0`. -/
theorem wao_eq {regs : List Reg} {c : Spec.Slab.Ctx isa} {s : State} (hA : A64 c.A regs)
    (h13 : s.gpr .x13 = s.syms "SLABS") (hcls : c.cls s = (s.gpr .x0).toNat % SLAB_BYTES / 8) :
    wao s = c.word s := by
  rw [Spec.Slab.Ctx.word, wordAddr, sym_of hA, cpu_of hA, wao, h13, r8o, sh_eq, cpu32, hcls]
  apply BitVec.eq_of_toNat_eq
  have hoff : (offw s).toNat = 8 * ((s.gpr .x0).toNat % 4096 / 8) := and_ff8 _
  simp only [BitVec.toNat_add, BitVec.toNat_ofNat, hoff, SLABS, SLAB_BYTES, SLAB_SHIFT]
  omega

/-! ## What a slow-path routine may rely on -/

/-- What a slow-path routine `c` may rely on in the state an attempt starts in. -/
structure SFacts (c : Spec.Slab.Ctx isa) (s : State) : Prop where
  rsVar : ⟨s.syms RS, 8⟩ ∈ s.rd
  cpuF : (⟨area s + 4, 4⟩ : Region) ∈ s.rd
  cs : (⟨area s + 8, 8⟩ : Region) ∈ s.wr
  word : c.InRange s → ⟨c.word s, 8⟩ ∈ s.wr
  cs_rsVar : Region.Disjoint ⟨area s + 8, 8⟩ ⟨s.syms RS, 8⟩
  cs_word : c.InRange s → Region.Disjoint ⟨area s + 8, 8⟩ ⟨c.word s, 8⟩
  link : c.form = .stackPush → c.link s ∈ s.wr
  link_cs : c.form = .stackPush → Region.Disjoint (c.link s) ⟨area s + 8, 8⟩
  link_word : c.form = .stackPush → c.InRange s → Region.Disjoint (c.link s) ⟨c.word s, 8⟩
  link_rsVar : c.form = .stackPush → Region.Disjoint ⟨s.syms RS, 8⟩ (c.link s)
  top : c.form = .stackPop → c.InRange s → (c.w s).toNat ≠ 0 → (c.w s).toNat % 2 = 0 →
    ⟨c.w s, 8⟩ ∈ s.rd
  top_cs : c.form = .stackPop → c.InRange s → (c.w s).toNat ≠ 0 → (c.w s).toNat % 2 = 0 →
    Region.Disjoint ⟨c.w s, 8⟩ ⟨area s + 8, 8⟩
  pre : c.pre s
  ready : ready s
  x13 : s.gpr .x13 = s.syms "SLABS"

/-- A slow-path routine's facts, from the precondition's: its regions are the state's, and the
layout and the invariant place them apart. -/
theorem sfacts_of_world {regs : List Reg} {c : Spec.Slab.Ctx isa} (hA : A64 c.A regs) {s : State}
    (hw : c.World s) (hp : c.pre s) (hr : readyS s) : SFacts c s := by
  have hl : c.Layout s := hp.2.2.2.1
  have hc : c.cls s < CLASS_LIMIT := cls_lt hp.2.2.1
  have hrd : s.rd = c.ro s := by rw [← hA.rd]; exact hw.rd
  have hwr : s.wr = c.rw s := by rw [← hA.wr]; exact hw.wr
  have hcs := csField_of hA s
  have hrs := rsVar_of hA s
  have hcpu := cpuField_of hA s
  refine {
    rsVar := ?_, cpuF := ?_, cs := ?_, word := fun hi => ?_, cs_rsVar := ?_, cs_word := fun hi => ?_,
    link := fun hf => ?_, link_cs := fun hf => ?_, link_word := fun hf hi => ?_,
    link_rsVar := fun hf => ?_, top := fun hf hi h0 h2 => ?_, top_cs := fun hf hi h0 h2 => ?_,
    pre := hp, ready := hr.1, x13 := hr.2 }
  · rw [hrd, ← hrs]; exact mem_ro (fixed_mem rsVar_mem_fixed)
  · rw [hrd, ← hcpu]; exact mem_ro (fixed_mem cpu_mem_fixed)
  · rw [hwr, ← hcs]; exact mem_rw (fixed_mem cs_mem_fixed)
  · rw [hwr]; exact mem_rw (word_mem hi)
  · rw [← hcs, ← hrs]; exact (disj_cs_rsVar hl).symm
  · rw [← hcs]; exact (hl.slabs (c.csField s, true) cs_mem_fixed).sub_right (word_sub hc hi)
  · rw [hwr]; exact mem_rw (fixed_mem (link_mem_fixed hf))
  · rw [← hcs]; exact link_cs_disj hf hl
  · exact (hl.slabs (c.link s, true) (link_mem_fixed hf)).sub_right (word_sub hc hi)
  · rw [← hrs]; exact (harmless_link hf hl).2.1.symm
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

theorem SFacts.toArm {c : Spec.Slab.Ctx isa} {s : State} (h : SFacts c s) : ArmOk s :=
  ⟨h.rsVar, h.cpuF, h.cs, h.cs_rsVar, h.ready⟩

/-! ## A retry -/

section Retry

variable {regs : List Reg} {c : Spec.Slab.Ctx isa}

/-- As `retry`, for an attempt of these routines: an abort keeps `x13` too. -/
theorem retryS (hA : A64 c.A regs) {post : State → State → Prop}
    {exit : String → State → State → Prop} {abort : State → State → Prop}
    {trace : State → List Event → Prop} {vals : State → List (BitVec 64)}
    (hv : c.A.args ((c.sig.words c.A.ptrBits).map (·.bits c.A.ptrBits)) = some vals)
    (hb : ∀ x, Sig.bufs c.sig.params (vals x) = [])
    (ha : ∀ x, c.A.argArea ((c.sig.words c.A.ptrBits).map (·.bits c.A.ptrBits)) x = [])
    (hlen : (c.sig.words 64).length ≤ regs.length) {s p : State} {m : Mem}
    {W : List Region} (hs : c.pre s) (hr : readyS s) (hm : Mid scratchS s m p)
    (hrs : ∀ r ∈ regs.take (c.sig.words 64).length, r ∉ scratchS)
    (hfr : Frame W s.mem m) (hW : ∀ w ∈ W, Harmless c s w) :
    c.inv p ∧ ∀ p', rely p p' → (withReadyS (contract c post exit abort trace)).env p p' →
      (withReadyS (contract c post exit abort trace)).pre p' := by
  obtain ⟨hip, hre⟩ := retry hA hv hb ha hlen hs hr.1 hm hrs (by decide) (by decide) hfr hW
  refine ⟨hip, fun p' hr' he => ?_⟩
  obtain ⟨hpre, hrd⟩ := hre p' hr' he
  obtain ⟨hg, -, -, -, -, -, -, hsy, -⟩ := hr'
  exact ⟨hpre, hrd, by rw [hg, hm.gpr _ (by decide), hr.2, hsy, hm.syms]⟩

end Retry

/-! ## The setup -/

section Setup

variable {s : State}

/-- After the setup's first instruction: the slabs' address, in `x13`. -/
def s0 (s : State) : State := s.setReg .x13 (s.syms "SLABS")

theorem exec_slabs (h : (s.syms "SLABS").toNat % 4096 = 0) :
    exec (.adrp .x13 (.sym "SLABS")) s = some (s0 s) := by
  show some (s.setReg .x13 (page (s.syms "SLABS" + BitVec.ofInt 64 0))) = _
  have h0 : s.syms "SLABS" + BitVec.ofInt 64 0 = s.syms "SLABS" := by simp
  rw [h0, Access.page_of_aligned h]
  rfl

theorem not_mem_three {r : Reg} (h : ¬ r ∈ [Reg.x9, .x10, .x11, .x13]) :
    ¬ r ∈ [Reg.x9, .x10, .x11] := fun hm => h (by
  simp only [List.mem_cons, List.not_mem_nil, or_false] at hm ⊢
  rcases hm with rfl | rfl | rfl <;> simp)

/-- A register the setup does not write. -/
theorem s6_s0_keeps {r : Reg} (h : ¬ r ∈ [Reg.x9, .x10, .x11, .x13]) : (s6 (s0 s)).gpr r = s.gpr r := by
  rw [s6_keeps (not_mem_three h), s0, RegUpd.gpr_setReg_of_ne _ _ (fun h' => h (by simp [h']))]

/-- From an entry state satisfying the contract of a routine `c`, whose arguments are in none of
the registers the setup writes, the setup runs to a state where an attempt may start. -/
theorem slowSetup_ok {regs : List Reg} {c : Spec.Slab.Ctx isa} (hA : A64 c.A regs)
    {post : State → State → Prop} {exit : String → State → State → Prop}
    {abort : State → State → Prop} {trace : State → List Event → Prop}
    {vals : State → List (BitVec 64)}
    (hv : c.A.args ((c.sig.words c.A.ptrBits).map (·.bits c.A.ptrBits)) = some vals)
    (hb : ∀ x, Sig.bufs c.sig.params (vals x) = [])
    (ha : ∀ x, c.A.argArea ((c.sig.words c.A.ptrBits).map (·.bits c.A.ptrBits)) x = [])
    (hlen : (c.sig.words 64).length ≤ regs.length)
    (hargs : ∀ r ∈ regs.take (c.sig.words 64).length, ¬ r ∈ [Reg.x9, .x10, .x11, .x13])
    (hp : (contract c post exit abort trace).pre s) :
    execBlock isa slowSetup s = some (s6 (s0 s)) ∧
      (withReadyS (contract c post exit abort trace)).pre (s6 (s0 s)) := by
  have hpre := ((pre_iff c hv (hb s) (ha s)).mp hp).2.2
  have hpage : (s.syms "SLABS").toNat % 4096 = 0 := by
    have := hpre.2.2.2.1.page hA.slabsPage
    rwa [Spec.Slab.Ctx.slabs, sym_of hA] at this
  have hsame : Same c s (s0 s) := same_of hA hlen rfl rfl (fun r hr => by
    rw [s0, RegUpd.gpr_setReg_of_ne _ _ (fun h' => hargs r hr (by simp [h']))]) fun _ _ => rfl
  have hp0 : (contract c post exit abort trace).pre (s0 s) :=
    pre_of_same hv hb ha (by rw [hA.wf]; trivial) hp hsame (by rw [mem_of hA, mem_of hA]; rfl)
      (by rw [hA.rd, hA.rd]; rfl) (by rw [hA.wr, hA.wr]; rfl)
  obtain ⟨he, hk, hr⟩ := setup_ok hA hv hb ha hlen (fun r hr => not_mem_three (hargs r hr))
    (s := s0 s) hp0
  refine ⟨?_, hk, hr, ?_⟩
  · rw [slowSetup, execBlock_cons_some (exec_slabs hpage), he]
  · rw [s6_keeps (by decide), s0, RegUpd.gpr_setReg_self]
    rfl

end Setup

end VG.Proof.Slab.AArch64
