module

public import Cralloc.Proof.CEntry.Witness
public import Cralloc.Proof.Slab.AArch64.Slow
public import Cralloc.Proof.Framework.Segment
public import Cralloc.Impl.CEntry.AArch64

/-!
# The C routines' precondition on AArch64, as facts

What a proof of a C routine needs of the state an attempt starts in: each region of `Ctx.regions`
is in the state's readable or writable regions as it is marked, and is apart from the others where
one is written; and the registers the setup leaves (`readyC`), which a retry finds as the abort
kept them (`retryC`) and the setup makes hold (`setupOk_of`).
-/

@[expose] public section

namespace VG.Proof.CEntry.AArch64

open VG.AArch64 VG.Spec.Slab VG.Spec.CEntry VG.Impl.Slab.AArch64 VG.Impl.CEntry.AArch64
  VG.Proof.Slab.AArch64 VG.Proof.CEntry VG.Proof.Slab

/-! ## The view on AArch64 -/

section View

variable {regs : List Reg} {c : Spec.CEntry.Ctx isa} (hA : A64 c.A regs) (s : State)
include hA

theorem sym_c (n : String) : c.sym s n = s.syms n := sym_of (c := c.slab) hA s n
theorem mem_c : c.mem s = s.mem := mem_of (c := c.slab) hA s
theorem csField_c : c.csField s = ⟨area s + 8, 8⟩ := csField_of (c := c.slab) hA s
theorem cpuField_c : c.cpuField s = ⟨area s + 4, 4⟩ := cpuField_of (c := c.slab) hA s
theorem rsVar_c : c.rsVar s = ⟨s.syms RS, 8⟩ := rsVar_of (c := c.slab) hA s
theorem cpu_c : c.cpu s = (cpu32 s).toNat := cpu_of (c := c.slab) hA s
theorem inRange_c : c.InRange s ↔ (cpu32 s).toNat < 8192 := by
  simp only [Spec.CEntry.Ctx.InRange, cpu_c hA, SLAB_COUNT]

end View

section World

variable {regs : List Reg} {c : Spec.CEntry.Ctx isa} (hA : A64 c.A regs) {s : State} (hw : c.World s)
include hA hw

theorem mem_rd {x : Region × Bool} (hx : x ∈ c.regions s) (h2 : x.2 = false) : x.1 ∈ s.rd := by
  rw [← hA.rd, hw.rd, Spec.CEntry.Ctx.ro]
  exact List.mem_map.mpr ⟨x, List.mem_filter.mpr ⟨hx, by simp [h2]⟩, rfl⟩

theorem mem_wr {x : Region × Bool} (hx : x ∈ c.regions s) (h2 : x.2 = true) : x.1 ∈ s.wr := by
  rw [← hA.wr, hw.wr, Spec.CEntry.Ctx.rw]
  exact List.mem_map.mpr ⟨x, List.mem_filter.mpr ⟨hx, h2⟩, rfl⟩

/-- `__rseq_offset`, which the setup reads. -/
theorem rs_rd : (⟨s.syms RS, 8⟩ : Region) ∈ s.rd := by
  have hrs : (c.rsVar s, false) ∈ c.fixed s := by rw [fixed_tables]; simp
  have := mem_rd hA hw (mem_fixed hrs) rfl
  rwa [rsVar_c hA] at this

end World

/-- The arm's facts, from the precondition and what the setup left. -/
theorem armOk {regs : List Reg} {c : Spec.CEntry.Ctx isa} (hA : A64 c.A regs) {s : State}
    (hw : c.World s) (hl : c.Layout s) (hr : ready s) : ArmOk s := by
  have hf := fixed_tables c s
  have hcpu : (c.cpuField s, false) ∈ c.fixed s := by rw [hf]; simp
  have hcs : (c.csField s, true) ∈ c.fixed s := by rw [hf]; simp
  have hd := hl.disj
  rw [hf] at hd
  have e : (c.rsVar s).Disjoint (c.csField s) :=
    (List.pairwise_cons.mp hd).1 (c.csField s, true)
      (List.mem_cons_of_mem _ (List.mem_cons_self ..)) (Bool.or_true _)
  refine ⟨rs_rd hA hw, ?_, ?_, ?_, hr⟩
  · have := mem_rd hA hw (mem_fixed hcpu) rfl; rwa [cpuField_c hA] at this
  · have := mem_wr hA hw (mem_fixed hcs) rfl; rwa [csField_c hA] at this
  · rw [← csField_c hA, ← rsVar_c hA]; exact e.symm

/-! ## What the setup leaves -/

/-- Registers a routine's setup forms beyond those every C routine has, each with its value given
the symbols. -/
abbrev Held := List (Reg × ((String → Addr) → BitVec 64))

/-- The page past the slabs, in bytes above them. -/
def PAST_SLABS : Nat := 33554432

/-- What the setup leaves, which a sequence reads and no sequence writes: the rseq area and the
descriptor (`ready`), the slabs in `x13`, the page past them in `x14`, the masks' table in `x15`,
and the routine's own, `extra`. -/
def readyC (extra : Held) (s : State) : Prop :=
  ready s ∧ s.gpr .x13 = s.syms SLABS ∧ s.gpr .x14 = s.syms SLABS + BitVec.ofNat 64 PAST_SLABS ∧
    s.gpr .x15 = s.syms SLABS + BitVec.ofNat 64 MASK_DISP ∧ ∀ x ∈ extra, s.gpr x.1 = x.2 s.syms

/-- The contract of an attempt of a routine of the contract `k`. -/
abbrev withReadyC (extra : Held) (k : Contract isa) : Contract isa :=
  { k with pre := fun s => k.pre s ∧ readyC extra s }

/-- The registers of `readyC`, none of which an attempt writes. -/
structure Keeps (rs : List Reg) (extra : Held) : Prop where
  x10 : .x10 ∉ rs
  x11 : .x11 ∉ rs
  x13 : .x13 ∉ rs
  x14 : .x14 ∉ rs
  x15 : .x15 ∉ rs
  extra : ∀ x ∈ extra, x.1 ∉ rs

/-! ## A retry -/

section Retry

variable {regs : List Reg} {c : Spec.CEntry.Ctx isa}

/-- Where the routine may be aborted, in a state `p` reached from `s` with memory `m` changed only
in harmless regions `W`, it has kept the invariant, and every state the thread may resume in
satisfies an attempt's precondition. -/
theorem retryC (hA : A64 c.A regs) {post : State → State → Prop}
    {exit : String → State → State → Prop} {abort : State → State → Prop}
    {trace : State → List Event → Prop} {vals : State → List (BitVec 64)}
    (hv : c.A.args ((c.sig.words c.A.ptrBits).map (·.bits c.A.ptrBits)) = some vals)
    (hb : ∀ x, Sig.bufs c.sig.params (vals x) = [])
    (ha : ∀ x, c.A.argArea ((c.sig.words c.A.ptrBits).map (·.bits c.A.ptrBits)) x = [])
    (hlen : (c.sig.words 64).length ≤ regs.length) {rs : List Reg} {extra : Held} {s p : State}
    {m : Mem} {W : List Region} (hs : c.pre s) (hr : readyC extra s) (hm : Mid rs s m p)
    (hrs : ∀ r ∈ regs.take (c.sig.words 64).length, r ∉ rs) (hk : Keeps rs extra)
    (hfr : Frame W s.mem m) (hW : ∀ w ∈ W, Harmless c s w) :
    c.inv p ∧ ∀ p', rely p p' → (withReadyC extra (contract c post exit abort trace)).env p p' →
      (withReadyC extra (contract c post exit abort trace)).pre p' := by
  have hA' : A64 c.slab.A regs := hA
  have hrsv : ∀ x, (⟨s.syms RS, 8⟩ : Region).Contains x 1 → p.mem x = s.mem x := fun x hx => by
    rw [hm.mem]
    exact hfr x fun w hw hc => (hW w hw).2.2 x hc (by rwa [rsVar_c hA])
  have hsame : Same c s p := same_of hA' hlen hm.syms hm.tpidr
    (fun r hr => hm.gpr r (hrs r hr)) hrsv
  have hfr' : Frame W (c.mem s) (c.mem p) := by rw [mem_c hA, mem_c hA, hm.mem]; exact hfr
  have hip := inv_frame hsame hfr' hW hs.2.2.2
  refine ⟨hip, fun p' hr' he => ?_⟩
  obtain ⟨hg, -, -, -, -, -, htp', hsy, hcs⟩ := hr'
  have hkept := he.2.2.1 (c.rsVar p) (List.mem_cons_self ..)
  have hsy' : p'.syms = s.syms := hsy.trans hm.syms
  have hsame' : Same c s p' := same_of hA' hlen hsy' (htp'.trans hm.tpidr)
    (fun r hr => by rw [hg]; exact hm.gpr r (hrs r hr)) fun x hx => by
      have h1 := hkept x (by rwa [hsame.rsVar', rsVar_c hA])
      rw [mem_c hA, mem_c hA] at h1
      rw [h1]; exact hrsv x hx
  have hwf : c.A.wf ((c.sig.words c.A.ptrBits).map (·.bits c.A.ptrBits)) 0 p' := by
    rw [hA.wf]; trivial
  have harea : area p' = area s := by
    rw [← area_of hA', ← area_of hA']; exact hsame'.area
  obtain ⟨⟨hr10, hr11⟩, hr13, hr14, hr15, hrx⟩ := hr
  refine ⟨retry_pre c hv (hb p') (ha p') hA.ptrBits hwf hs hsame' hip he,
    ⟨by rw [harea, hg, hm.gpr _ hk.x10]; exact hr10, by rw [hg, hm.gpr _ hk.x11, hcs, hm.csDesc]; exact hr11⟩,
    by rw [hg, hm.gpr _ hk.x13, hr13, hsy'], by rw [hg, hm.gpr _ hk.x14, hr14, hsy'],
    by rw [hg, hm.gpr _ hk.x15, hr15, hsy'], fun x hx => ?_⟩
  rw [hg, hm.gpr _ (hk.extra x hx), hrx x hx, hsy']

/-- `rseq_cs`, which the arm writes, is harmless. -/
theorem harmless_csC (hA : A64 c.A regs) {s : State} (hs : c.pre s) :
    ∀ w ∈ [(⟨area s + 8, 8⟩ : Region)], Harmless c s w := by
  intro w hw
  rw [List.mem_singleton.mp hw, ← csField_c hA]
  exact harmless_cs hs.2.2.1

end Retry

/-! ## The setup -/

section Setup

/-- A size table's first entry into `r`. -/
def tabS (r : Reg) (name : String) (s : State) : State :=
  (s.setReg r (page (s.syms name + BitVec.ofInt 64 2048))).setReg r (s.syms name + BitVec.ofInt 64 2048)

theorem exec_tab (r : Reg) (name : String) (s : State) (is : List Instr) :
    execBlock isa (tableAddr r name ++ is) s = execBlock isa is (tabS r name s) := by
  have h1 : exec (.adrp r (.sym name 2048)) s =
      some (s.setReg r (page (s.syms name + BitVec.ofInt 64 2048))) := rfl
  have h2 : exec (.addLo12 r r (.sym name 2048)) (s.setReg r (page (s.syms name + BitVec.ofInt 64 2048))) =
      some (tabS r name s) := by
    show some ((s.setReg r (page (s.syms name + BitVec.ofInt 64 2048))).setReg r
      ((s.setReg r (page (s.syms name + BitVec.ofInt 64 2048))).gpr r +
        lo12 (s.syms name + BitVec.ofInt 64 2048))) = _
    rw [RegUpd.gpr_setReg_self, Access.page_add_lo12]
    rfl
  rw [tableAddr, List.cons_append, List.cons_append, List.nil_append, execBlock_cons_some h1,
    execBlock_cons_some h2]

/-- The slabs, the page past them and the masks' table, in their registers. -/
def baseS (s : State) : State :=
  ((s.setReg .x13 (s.syms SLABS)).setReg .x14 (s.syms SLABS + BitVec.ofNat 64 PAST_SLABS)).setReg .x15
    (s.syms SLABS + BitVec.ofNat 64 MASK_DISP)

theorem past_aligned {a : Addr} (h : a.toNat % 4096 = 0) :
    (a + BitVec.ofNat 64 PAST_SLABS).toNat % 4096 = 0 := by
  have := a.isLt
  rw [BitVec.toNat_add, BitVec.toNat_ofNat]
  simp only [PAST_SLABS]
  omega

theorem tabS_self (r : Reg) (name : String) (s : State) :
    (tabS r name s).gpr r = s.syms name + BitVec.ofInt 64 2048 := RegUpd.gpr_setReg_self _ _ _

theorem tabS_gpr {r r' : Reg} (name : String) (s : State) (h : ¬ r' = r) :
    (tabS r name s).gpr r' = s.gpr r' := by
  rw [tabS, RegUpd.gpr_setReg_of_ne _ _ h, RegUpd.gpr_setReg_of_ne _ _ h]

theorem exec_base {s : State} (h : (s.syms SLABS).toNat % 4096 = 0) (is : List Instr) :
    execBlock isa (slabsAddr :: masksAddr ++ is) s = execBlock isa is (baseS s) := by
  have h1 : exec slabsAddr s = some (s.setReg .x13 (s.syms SLABS)) := exec_slabs h
  have hs2 : (BitVec.ofInt 64 33554432 : BitVec 64) = BitVec.ofNat 64 PAST_SLABS := by decide
  have h2 : exec (.adrp .x14 (.sym "SLABS" 33554432)) (s.setReg .x13 (s.syms SLABS)) =
      some ((s.setReg .x13 (s.syms SLABS)).setReg .x14 (s.syms SLABS + BitVec.ofNat 64 PAST_SLABS)) := by
    show some ((s.setReg .x13 (s.syms SLABS)).setReg .x14
      (page (s.syms SLABS + BitVec.ofInt 64 33554432))) = _
    rw [hs2, Access.page_of_aligned (past_aligned h)]
  have h3 : exec (.add .x .x15 .x14 (.imm 2048))
      ((s.setReg .x13 (s.syms SLABS)).setReg .x14 (s.syms SLABS + BitVec.ofNat 64 PAST_SLABS)) =
      some (baseS s) := by
    show some (((s.setReg .x13 (s.syms SLABS)).setReg .x14
      (s.syms SLABS + BitVec.ofNat 64 PAST_SLABS)).setReg .x15
      (((s.setReg .x13 (s.syms SLABS)).setReg .x14
        (s.syms SLABS + BitVec.ofNat 64 PAST_SLABS)).gpr .x14 + BitVec.ofNat 64 2048)) = _
    rw [RegUpd.gpr_setReg_self, BitVec.add_assoc]
    rfl
  rw [List.cons_append, execBlock_cons_some h1, masksAddr, List.cons_append, List.cons_append,
    List.nil_append, execBlock_cons_some h2, execBlock_cons_some h3]

theorem baseS_gpr {r : Reg} (s : State) (h13 : ¬ r = .x13) (h14 : ¬ r = .x14) (h15 : ¬ r = .x15) :
    (baseS s).gpr r = s.gpr r := by
  rw [baseS, RegUpd.gpr_setReg_of_ne _ _ h15, RegUpd.gpr_setReg_of_ne _ _ h14,
    RegUpd.gpr_setReg_of_ne _ _ h13]

/-- And the region table, in `x12`. -/
def freeS (s : State) : State :=
  (baseS s).setReg .x12 (s.syms SLABS + BitVec.ofNat 64 REGION_DISP)

theorem exec_free {s : State} (h : (s.syms SLABS).toNat % 4096 = 0) (is : List Instr) :
    execBlock isa (freeTables ++ is) s = execBlock isa is (freeS s) := by
  have h15 : (baseS s).gpr .x15 = s.syms SLABS + BitVec.ofNat 64 MASK_DISP := RegUpd.gpr_setReg_self _ _ _
  have h4 : exec (.add .x .x12 .x15 (.imm 4096)) (baseS s) = some (freeS s) := by
    show some ((baseS s).setReg .x12 ((baseS s).gpr .x15 + BitVec.ofNat 64 4096)) = _
    rw [h15, BitVec.add_assoc]
    rfl
  rw [freeTables, List.append_assoc, exec_base h, List.cons_append, List.nil_append,
    execBlock_cons_some h4]

theorem freeS_x12 (s : State) : (freeS s).gpr .x12 = s.syms SLABS + BitVec.ofNat 64 REGION_DISP :=
  RegUpd.gpr_setReg_self _ _ _

theorem freeS_base {r : Reg} (s : State) (h : ¬ r = .x12) : (freeS s).gpr r = (baseS s).gpr r :=
  RegUpd.gpr_setReg_of_ne _ _ h

theorem baseS_x13 (s : State) : (baseS s).gpr .x13 = s.syms SLABS := by
  rw [baseS, RegUpd.gpr_setReg_of_ne _ _ (by decide), RegUpd.gpr_setReg_of_ne _ _ (by decide),
    RegUpd.gpr_setReg_self]
theorem baseS_x14 (s : State) : (baseS s).gpr .x14 = s.syms SLABS + BitVec.ofNat 64 PAST_SLABS := by
  rw [baseS, RegUpd.gpr_setReg_of_ne _ _ (by decide), RegUpd.gpr_setReg_self]
theorem baseS_x15 (s : State) : (baseS s).gpr .x15 = s.syms SLABS + BitVec.ofNat 64 MASK_DISP :=
  RegUpd.gpr_setReg_self _ _ _

theorem mid_tab {rs : List Reg} {s τ : State} {m : Mem} {r : Reg} {name : String} (hτ : Mid rs s m τ)
    (hr : r ∈ rs) : Mid rs s m (tabS r name τ) := (hτ.setReg hr _).setReg hr _

theorem mid_base {rs : List Reg} {s τ : State} {m : Mem} (hτ : Mid rs s m τ) (h13 : .x13 ∈ rs)
    (h14 : .x14 ∈ rs) (h15 : .x15 ∈ rs) : Mid rs s m (baseS τ) :=
  ((hτ.setReg h13 _).setReg h14 _).setReg h15 _

theorem mid_refl {rs : List Reg} (s : State) : Mid rs s s.mem s :=
  ⟨rfl, rfl, rfl, rfl, rfl, rfl, rfl, fun _ _ => rfl⟩

/-- A C routine's setup: its tables, run to `f s`, then what every routine runs. -/
theorem setupOk_of {c : Spec.CEntry.Ctx isa} (hA : A64 c.A argRegs) {post : State → State → Prop}
    {exit : String → State → State → Prop} {abort : State → State → Prop}
    {trace : State → List Event → Prop} {vals : State → List (BitVec 64)}
    (hv : c.A.args ((c.sig.words c.A.ptrBits).map (·.bits c.A.ptrBits)) = some vals)
    (hb : ∀ x, Sig.bufs c.sig.params (vals x) = [])
    (ha : ∀ x, c.A.argArea ((c.sig.words c.A.ptrBits).map (·.bits c.A.ptrBits)) x = [])
    (hlen : (c.sig.words 64).length ≤ argRegs.length)
    {tbl : List Instr} {f : State → State} {rs : List Reg} {extra : Held}
    (hrun : ∀ s, (s.syms SLABS).toNat % 4096 = 0 → execBlock isa tbl s = some (f s))
    (hmid : ∀ s, Mid rs s s.mem (f s))
    (h13 : ∀ s, (f s).gpr .x13 = s.syms SLABS)
    (h14 : ∀ s, (f s).gpr .x14 = s.syms SLABS + BitVec.ofNat 64 PAST_SLABS)
    (h15 : ∀ s, (f s).gpr .x15 = s.syms SLABS + BitVec.ofNat 64 MASK_DISP)
    (hx : ∀ s, ∀ x ∈ extra, (f s).gpr x.1 = x.2 s.syms)
    (hkx : ∀ x ∈ extra, ¬ x.1 ∈ [Reg.x9, .x10, .x11])
    (hcs : ∀ r ∈ calleeSaved, r ∉ rs ∧ ¬ r ∈ [Reg.x9, .x10, .x11])
    (hargs : ∀ r ∈ argRegs.take (c.sig.words 64).length, r ∉ rs ∧ ¬ r ∈ [Reg.x9, .x10, .x11]) :
    SetupOk target (contract c post exit abort trace)
      (withReadyC extra (contract c post exit abort trace)) c.sig [] (tbl ++ setup) := by
  refine .inr ⟨⟨readyC extra, rfl⟩, fun s hp => ?_⟩
  obtain ⟨-, hw, hpre⟩ := (pre_iff c hv (hb s) (ha s)).mp hp
  have hpage : (s.syms SLABS).toNat % 4096 = 0 := by
    have := hpre.2.2.1.page hA.slabsPage
    rwa [Spec.CEntry.Ctx.slabs, sym_c hA] at this
  have hm := hmid s
  have hk : ∀ r, ¬ r ∈ [Reg.x9, .x10, .x11] → (s6 (f s)).gpr r = (f s).gpr r := fun r hr => s6_keeps hr
  have hsy : (s6 (f s)).syms = s.syms := hm.syms
  have hA' : A64 c.slab.A argRegs := hA
  have hsame : Same c s (s6 (f s)) := same_of hA' hlen hsy hm.tpidr
    (fun r hr => by
      rw [hk r (hargs r hr).2]; exact hm.gpr r (hargs r hr).1)
    fun x _ => by rw [show (s6 (f s)).mem = (f s).mem from rfl, hm.mem]
  have hmem : c.mem (s6 (f s)) = c.mem s := by
    rw [mem_c hA, mem_c hA, show (s6 (f s)).mem = (f s).mem from rfl, hm.mem]
  refine ⟨s6 (f s), ?_, ⟨?_, ?_⟩, ?_, ?_, ?_, fun h => absurd h (by decide)⟩
  · rw [execBlock_append, hrun s hpage, Option.bind_some]
    exact execBlock_setup (by rw [hm.rd, hm.syms]; exact rs_rd hA hw)
  · exact pre_of_same hv hb ha (by rw [hA.wf]; trivial) hp hsame hmem
      (by rw [hA.rd, hA.rd, show (s6 (f s)).rd = (f s).rd from rfl, hm.rd])
      (by rw [hA.wr, hA.wr, show (s6 (f s)).wr = (f s).wr from rfl, hm.wr])
  · refine ⟨s6_ready, ?_, ?_, ?_, fun x hx' => ?_⟩
    · rw [hk _ (by decide), h13, hsy]
    · rw [hk _ (by decide), h14, hsy]
    · rw [hk _ (by decide), h15, hsy]
    · rw [hk _ (hkx x hx'), hx s x hx', hsy]
  · show (s6 (f s)).mem = s.mem
    rw [show (s6 (f s)).mem = (f s).mem from rfl, hm.mem]
  · refine ⟨fun r hr => ?_, ?_⟩
    · rw [hk r (hcs r hr).2]; exact hm.gpr r (hcs r hr).1
    · show (s6 (f s)).sp = s.sp
      rw [show (s6 (f s)).sp = (f s).sp from rfl, hm.sp]
  · exact argsKept_regs a64_abi hlen fun r hr => by
      rw [hk r (hargs r hr).2]; exact hm.gpr r (hargs r hr).1

end Setup

end VG.Proof.CEntry.AArch64
