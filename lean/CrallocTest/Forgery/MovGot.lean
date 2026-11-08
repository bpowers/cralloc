import Cralloc.TCB.Axioms
import Cralloc.Proof.Examples.AArch64.Seq
import Cralloc.Proof.Framework.Registration
import CrallocTest.Forgery.Refused

/-!
# A routine that verifies and is wrong: `movGot` in a sequence's body

`movGot x12, S` is two instructions on the machine, `adrp x12, 8b` and
`ldr x12, [x12, :lo12:8b]`, but one step of the model. So an abort between them,
which leaves `x12` holding a page address, is not among `Code.abortStates`.

The routine keeps the address of the word `S` in `x12`, set up once. Its body reads
through `x12`, then "refreshes" `x12` (in the model, to the value it already holds),
then commits. Every abort state of the model has `x12 = S`, so a retry's precondition
holds. On the machine, an abort between the two instructions leaves `x12 = page(8b)`,
and the retry reads the first word of that page and commits it to `*p`.
-/

namespace VG.Test.Forgery.MovGot

open VG VG.Spec.Slab VG.AArch64

def sig : Sig where
  params := [("p", .int .usize)]

section Contract

variable {M : ISA} (A : Abi M)

abbrev cx : Ctx M := ⟨A, .cas, sig⟩

def slot (s : M.State) : Region := ⟨(cx A).arg s 0, 8⟩
def src (s : M.State) : Region := ⟨(cx A).sym s "S", 8⟩

def regions (s : M.State) : List (Region × Bool) :=
  [((cx A).rsVar s, false), (src A s, false), ((cx A).csField s, true), (slot A s, true)]

def ro (s : M.State) : List Region := ((regions A s).filter (!·.2)).map (·.1)
def rw (s : M.State) : List Region := ((regions A s).filter (·.2)).map (·.1)

/-- It stores the word at `S` to `*p`. Wherever it may be aborted it has changed only
`rseq_cs`. While it is aborted, anything but `__rseq_offset` may change. -/
def theContract : Contract M :=
  sig.contract (A.withExtra (some (regions A))) (post := Curry.const (fun _ _ _ => True) _)
    (x := {
      post := fun s s' => (A.mem s').readW (slot A s).base 64 = ((cx A).mem s).readW (src A s).base 64
      abort := fun s p => Unchanged [(cx A).csField s] ((cx A).mem s) (A.mem p)
      env := fun p p' => A.rd p' = ro A p' ∧ A.wr p' = rw A p' ∧
        Kept [(cx A).rsVar p] ((cx A).mem p) ((cx A).mem p')
      trace := fun _ _ => True })

end Contract

abbrev RS : String := "__rseq_offset"

def setup : List Instr := [
  .adrp .x11 .desc,
  .addLo12 .x11 .x11 .desc,
  .ldrWord .x9 .x11 RS,
  .ldr .x .x9 { base := .x9 },
  .mrsTp .x10,
  .add .x .x10 .x10 (.reg .x9),
  .movGot .x12 "S"]

def arm : List Instr := [.str .x .x11 { base := .x10, off := .imm 8 }]

def body : List (Step Instr Cond) := [
  .instr (.ldr .x .x5 { base := .x12 }),
  .instr (.movGot .x12 "S"),
  .instr (.str .x .x5 { base := .x0 })]

def code : Prog isa := .rseq arm body

def area (s : State) : Addr := s.tpidr + s.mem.readW (s.syms RS) 64

def ready (s : State) : Prop :=
  s.gpr .x10 = area s ∧ s.gpr .x11 = s.csDesc ∧ s.gpr .x12 = s.syms "S"

abbrev contract : Contract isa := theContract abi
abbrev attempt : Contract isa := { contract with pre := fun s => contract.pre s ∧ ready s }

theorem regions_eq (s : State) : regions abi s =
    [(⟨s.syms RS, 8⟩, false), (⟨s.syms "S", 8⟩, false), (⟨area s + 8, 8⟩, true),
      (⟨s.gpr .x0, 8⟩, true)] := rfl

theorem hv : abi.args ((sig.words abi.ptrBits).map (·.bits abi.ptrBits)) =
    some (fun s => [s.gpr .x0]) := rfl

structure World (s : State) : Prop where
  rd : s.rd = ro abi s
  wr : s.wr = rw abi s
  nowrap : ∀ x ∈ regions abi s, x.1.base.toNat + x.1.len ≤ 2 ^ 64
  disj : (regions abi s).Pairwise (fun a b => (a.2 || b.2) → a.1.Disjoint b.1)

theorem pre_iff {s : State} : contract.pre s ↔ World s := by
  have hb : Sig.bufs sig.params ((fun s : State => [s.gpr .x0]) s) = [] := rfl
  have ha : abi.argArea ((sig.words abi.ptrBits).map (·.bits abi.ptrBits)) s = [] := rfl
  simp only [contract, theContract, Sig.contract, Abi.withExtra, hv, hb, ha, List.map_nil,
    List.append_nil, List.filter_nil, List.Pairwise.nil, List.not_mem_nil, false_implies,
    implies_true, true_and, Curry.apply_const, and_true]
  constructor
  · rintro ⟨⟨-, hd1, hd2, hnw, hp, -, -, -⟩, hrd, hwr⟩
    exact ⟨Proof.Slab.take_drop_eq hrd hd1, Proof.Slab.take_drop_eq hwr hd2, hnw, hp⟩
  · rintro ⟨hrd, hwr, hnw, hp⟩
    have hrd' : abi.rd s = ((regions abi s).filter (!·.2)).map (·.1) := hrd
    have hwr' : abi.wr s = ((regions abi s).filter (·.2)).map (·.1) := hwr
    simp only [hrd', hwr', Nat.sub_self, List.take_zero, List.drop_zero, List.not_mem_nil,
      false_implies, implies_true, and_true, true_and]
    exact ⟨trivial, hnw, hp, fun _ _ r hr => absurd hr (by simp [abi, stackBelow])⟩

structure Facts (s : State) : Prop where
  rs : ⟨s.syms RS, 8⟩ ∈ s.rd
  source : (⟨s.syms "S", 8⟩ : Region) ∈ s.rd
  cs : (⟨area s + 8, 8⟩ : Region) ∈ s.wr
  word : (⟨s.gpr .x0, 8⟩ : Region) ∈ s.wr
  rs_cs : Region.Disjoint ⟨s.syms RS, 8⟩ ⟨area s + 8, 8⟩
  src_cs : Region.Disjoint ⟨s.syms "S", 8⟩ ⟨area s + 8, 8⟩

theorem World.facts {s : State} (h : World s) : Facts s := by
  obtain ⟨hrd, hwr, -, hp⟩ := h
  rw [regions_eq] at hp
  have hrd' : s.rd = [⟨s.syms RS, 8⟩, ⟨s.syms "S", 8⟩] := hrd
  have hwr' : s.wr = [⟨area s + 8, 8⟩, ⟨s.gpr .x0, 8⟩] := hwr
  have h1 := List.pairwise_cons.mp hp
  have h2 := List.pairwise_cons.mp h1.2
  exact {
    rs := by rw [hrd']; simp
    source := by rw [hrd']; simp
    cs := by rw [hwr']; simp
    word := by rw [hwr']; simp
    rs_cs := h1.1 (⟨area s + 8, 8⟩, true) (by simp) rfl
    src_cs := h2.1 (⟨area s + 8, 8⟩, true) (by simp) rfl }

section Steps

variable {s : State}

def m1 (s : State) : Mem := s.mem.writeW (area s + 8) s.csDesc
def σA (s : State) : State := { s with mem := m1 s }
def val (s : State) : BitVec 64 := s.mem.readW (s.syms "S") 64
def σ1 (s : State) : State := (σA s).setReg .x5 (val s)
/-- In the model, `movGot` writes `x12` in one step. -/
def σ2 (s : State) : State := (σ1 s).setReg .x12 (s.syms "S")
def mEnd (s : State) : Mem := (m1 s).writeW (s.gpr .x0) (val s)
def σ3 (s : State) : State := { σ2 s with mem := mEnd s }

theorem exec_arm (h : Facts s) (hr : ready s) :
    exec (.str .x .x11 { base := .x10, off := .imm 8 }) s = some (σA s) := by
  have hea : s.ea { base := .x10, off := .imm 8 } = area s + 8 := by
    rw [Access.ea_imm, hr.1]; rfl
  have hin : InRegions s.wr (s.ea { base := .x10, off := .imm 8 }) 8 := by
    rw [hea]; exact InRegions.of_mem h.cs (Region.contains_self _ _)
  rw [Access.exec_str_x hin, hea, hr.2.1]
  rfl

theorem exec_read (h : Facts s) (hr : ready s) :
    exec (.ldr .x .x5 { base := .x12 }) (σA s) = some (σ1 s) := by
  have hea : (σA s).ea { base := .x12 } = s.syms "S" := by
    rw [Access.ea_base]; exact hr.2.2
  have hin : InRegions ((σA s).rd ++ (σA s).wr) ((σA s).ea { base := .x12 }) 8 := by
    rw [hea]
    exact (InRegions.of_mem h.source (Region.contains_self _ _)).append_left
  have hm : (σA s).mem.readW (s.syms "S") 64 = val s :=
    Mem.readW_writeW_sep (h.src_cs.sep (Region.contains_self _ _) (Region.contains_self _ _)) (by decide)
  rw [Access.exec_ldr_x hin, hea, hm]
  rfl

theorem exec_got : exec (.movGot .x12 "S") (σ1 s) = some (σ2 s) := rfl

theorem σ2_gpr {r : Reg} (h5 : r ≠ .x5) (h12 : r ≠ .x12) : (σ2 s).gpr r = s.gpr r := by
  rw [σ2, RegUpd.gpr_setReg_of_ne _ _ h12, σ1, RegUpd.gpr_setReg_of_ne _ _ h5]
  rfl

theorem exec_commit (h : Facts s) :
    exec (.str .x .x5 { base := .x0 }) (σ2 s) = some (σ3 s) := by
  have hea : (σ2 s).ea { base := .x0 } = s.gpr .x0 := by
    rw [Access.ea_base, σ2_gpr (by decide) (by decide)]
  have hin : InRegions (σ2 s).wr ((σ2 s).ea { base := .x0 }) 8 := by
    rw [hea]; exact InRegions.of_mem h.word (Region.contains_self _ _)
  have hx5 : (σ2 s).gpr .x5 = val s := by
    rw [σ2, RegUpd.gpr_setReg_of_ne _ _ (by decide), σ1, RegUpd.gpr_setReg_self]
  rw [Access.exec_str_x hin, hea, hx5]
  rfl

theorem execBlock_arm (h : Facts s) (hr : ready s) : execBlock isa arm s = some (σA s) := by
  rw [arm, execBlock_cons_some (exec_arm h hr), execBlock_nil']

theorem runBody_all (h : Facts s) (hr : ready s) :
    runBody isa body (σA s) = some (.done (σ3 s)) := by
  rw [body, runBody_instr (exec_read h hr), runBody_instr exec_got,
    runBody_instr (exec_commit h), runBody_nil']

theorem prefixes_all (h : Facts s) (hr : ready s) :
    bodyPrefixes isa body (σA s) = [σA s, σ1 s, σ2 s] := by
  rw [body, bodyPrefixes_instr (exec_read h hr), bodyPrefixes_instr exec_got,
    bodyPrefixes_last (exec_commit h)]

theorem end_all (h : Facts s) (hr : ready s) :
    bodyEnd isa body (σA s) = some 3 := by
  rw [body, bodyEnd_instr (exec_read h hr), bodyEnd_instr exec_got,
    bodyEnd_instr (exec_commit h), bodyEnd_nil']
  rfl

/-- `p` on the way from `s`: memory as after arming, every register but `x5` and `x12`
as in `s`, and `x12` holding `S`. -/
structure Mid (s p : State) : Prop where
  mem : p.mem = m1 s
  gpr : ∀ r, r ≠ .x5 → r ≠ .x12 → p.gpr r = s.gpr r
  x12 : p.gpr .x12 = s.syms "S"
  sp : p.sp = s.sp
  syms : p.syms = s.syms
  tpidr : p.tpidr = s.tpidr
  csDesc : p.csDesc = s.csDesc

theorem mid_σA (hr : ready s) : Mid s (σA s) := ⟨rfl, fun _ _ _ => rfl, hr.2.2, rfl, rfl, rfl, rfl⟩

theorem mid_σ1 (hr : ready s) : Mid s (σ1 s) :=
  ⟨rfl, fun _ h _ => RegUpd.gpr_setReg_of_ne _ _ h,
    by rw [σ1, RegUpd.gpr_setReg_of_ne _ _ (by decide)]; exact hr.2.2, rfl, rfl, rfl, rfl⟩

theorem mid_σ2 : Mid s (σ2 s) :=
  ⟨rfl, fun _ h5 h12 => σ2_gpr h5 h12, RegUpd.gpr_setReg_self _ _ _, rfl, rfl, rfl, rfl⟩

theorem rs_m1 (h : Facts s) : (m1 s).readW (s.syms RS) 64 = s.mem.readW (s.syms RS) 64 :=
  Mem.readW_writeW_sep (h.rs_cs.sep (Region.contains_self _ _) (Region.contains_self _ _)) (by decide)

theorem armed_of_mid (h : Facts s) {p : State} (hm : Mid s p) : armed p := by
  have ha : rseqArea p = area s := by
    simp only [rseqArea, rseqOffsetSym, hm.mem, hm.tpidr, hm.syms]
    rw [rs_m1 h]
    rfl
  simp only [armed, rseqCsAddr, ha, hm.mem, hm.csDesc]
  exact Mem.readW_writeW_self64 _ _ _

theorem abi_of_mid {p : State} (hm : Mid s p) : abiPreserved s p :=
  ⟨fun r hr => hm.gpr r (fun h => by subst h; exact absurd hr (by decide))
    (fun h => by subst h; exact absurd hr (by decide)), hm.sp⟩

theorem frame_m1 : Frame [⟨area s + 8, 8⟩] s.mem (m1 s) :=
  (Frame.refl _ _).writeW (List.mem_singleton_self _) _ (Region.contains_self _ _)

theorem retry (hs : attempt.pre s) {p p' : State} (hm : Mid s p) (hr : rely p p')
    (he : attempt.env p p') : attempt.pre p' := by
  obtain ⟨hw, hrd⟩ := hs
  have hw' := pre_iff.mp hw
  have hf := hw'.facts
  obtain ⟨hg, -, -, -, -, -, htp, hsy, hcs⟩ := hr
  obtain ⟨hrdp, hwrp, hk⟩ := he
  have hkept : ∀ x, (⟨s.syms RS, 8⟩ : Region).Contains x 1 → p'.mem x = p.mem x := fun x hx =>
    hk _ (List.mem_singleton_self _) x (by
      show Region.Contains ⟨p.syms RS, 8⟩ x 1
      rwa [hm.syms])
  have hrs : p'.mem.readW (p'.syms RS) 64 = s.mem.readW (s.syms RS) 64 := by
    rw [hsy, hm.syms, ← rs_m1 hf, ← hm.mem]
    refine Mem.readW_congr fun i hi => hkept _ ?_
    simp only [Region.Contains]
    rw [Mem.sub_ofNat_toNat _ (by omega)]
    omega
  have harea : area p' = area s := by
    simp only [area, htp, hm.tpidr, hrs]
  have hreg : regions abi p' = regions abi s := by
    rw [regions_eq, regions_eq, harea, hsy, hm.syms, hg, hm.gpr .x0 (by decide) (by decide)]
  refine ⟨pre_iff.mpr ⟨hrdp, hwrp, ?_, ?_⟩, ?_, ?_, ?_⟩
  · rw [hreg]; exact hw'.nowrap
  · rw [hreg]; exact hw'.disj
  · rw [harea, hg, hm.gpr _ (by decide) (by decide)]; exact hrd.1
  · rw [hg, hm.gpr _ (by decide) (by decide), hcs, hm.csDesc]; exact hrd.2.1
  · rw [hg, hm.x12, hsy, hm.syms]

theorem abortOk (hs : attempt.pre s) {p : State} (hm : Mid s p) :
    armed p ∧ target.abiPreserved s p ∧ attempt.abort s p ∧
      ∀ p', target.rely p p' → attempt.env p p' → attempt.pre p' := by
  have hf := (pre_iff.mp hs.1).facts
  refine ⟨armed_of_mid hf hm, abi_of_mid hm, ⟨?_, ?_⟩, fun p' hr he => retry hs hm hr he⟩
  · show [p.gpr .x0] = [s.gpr .x0]
    rw [hm.gpr .x0 (by decide) (by decide)]
  · show Unchanged [⟨area s + 8, 8⟩] s.mem p.mem
    rw [hm.mem]; exact frame_m1

theorem mids (hs : attempt.pre s) : ∀ p ∈ Code.abortStates isa code s, Mid s p := by
  have hf := (pre_iff.mp hs.1).facts
  have hr := hs.2
  intro p hp
  simp only [code, Code.abortStates, execBlock_arm hf hr] at hp
  rw [prefixes_all hf hr] at hp
  simp only [List.mem_cons, List.not_mem_nil, or_false] at hp
  rcases hp with rfl | rfl | rfl
  · exact mid_σA hr
  · exact mid_σ1 hr
  · exact mid_σ2

theorem correct (hs : attempt.pre s) :
    (∃ s', Exec isa code s s' ∧ target.abiPreserved s s' ∧ attempt.post s s') ∨
    (∃ t s', ExecExit isa code s t s' ∧ target.abiPreserved s s' ∧ attempt.exit t s s') := by
  have hf := (pre_iff.mp hs.1).facts
  have hr := hs.2
  refine .inl ⟨σ3 s, .rseq (execBlock_arm hf hr) (runBody_all hf hr), ?_, ?_⟩
  · refine ⟨fun r hr' => ?_, rfl⟩
    show (σ2 s).gpr r = s.gpr r
    exact σ2_gpr (fun h => by subst h; exact absurd hr' (by decide))
      (fun h => by subst h; exact absurd hr' (by decide))
  · refine ⟨trivial, ?_⟩
    show ((m1 s).writeW (s.gpr .x0) (val s)).readW (s.gpr .x0) 64 = s.mem.readW (s.syms "S") 64
    exact Mem.readW_writeW_self64 _ _ _

theorem rseqOk (hs : attempt.pre s) : RseqOk target code attempt s :=
  ⟨trivial, ⟨_, s, rfl, rfl, by rw [Access.ea_imm, hs.2.1]; rfl, hs.2.2.1⟩,
    fun p hp => abortOk hs (mids hs p hp)⟩

end Steps

section Setup

variable {s : State}

def s6 (s : State) : State :=
  (((((((s.setReg .x11 (page s.csDesc)).setReg .x11 s.csDesc).setReg .x9 (s.syms RS)).setReg .x9
    (s.mem.readW (s.syms RS) 64)).setReg .x10 s.tpidr).setReg .x10
      (s.tpidr + s.mem.readW (s.syms RS) 64)).setReg .x12 (s.syms "S"))

theorem execBlock_setup (h : ⟨s.syms RS, 8⟩ ∈ s.rd) : execBlock isa setup s = some (s6 s) := by
  have h1 : exec (.adrp .x11 .desc) s = some (s.setReg .x11 (page s.csDesc)) := rfl
  let a1 := s.setReg .x11 (page s.csDesc)
  let a2 := a1.setReg .x11 s.csDesc
  let a3 := a2.setReg .x9 (s.syms RS)
  let a4 := a3.setReg .x9 (s.mem.readW (s.syms RS) 64)
  let a5 := a4.setReg .x10 s.tpidr
  let a6 := a5.setReg .x10 (s.tpidr + s.mem.readW (s.syms RS) 64)
  have h2 : exec (.addLo12 .x11 .x11 .desc) a1 = some a2 := by
    show some (a1.setReg .x11 (a1.gpr .x11 + lo12 s.csDesc)) = _
    rw [show a1.gpr .x11 = page s.csDesc from RegUpd.gpr_setReg_self _ _ _, Access.page_add_lo12]
  have h3 : exec (.ldrWord .x9 .x11 RS) a2 = some a3 := by
    simp only [exec, show a2.gpr .x11 = a2.csDesc from RegUpd.gpr_setReg_self _ _ _, ↓reduceIte]
    rfl
  have hea : a3.ea { base := .x9 } = s.syms RS := by
    rw [Access.ea_base]; exact RegUpd.gpr_setReg_self _ _ _
  have h4 : exec (.ldr .x .x9 { base := .x9 }) a3 = some a4 := by
    rw [Access.exec_ldr_x (by
      rw [hea]; exact (InRegions.of_mem (r := ⟨s.syms RS, 8⟩) h (Region.contains_self _ _)).append_left), hea]
    rfl
  have h5 : exec (.mrsTp .x10) a4 = some a5 := rfl
  have h6 : exec (.add .x .x10 .x10 (.reg .x9)) a5 = some a6 := by
    show some (a5.setReg .x10 (a5.gpr .x10 + a5.gpr .x9)) = _
    rw [show a5.gpr .x10 = s.tpidr from RegUpd.gpr_setReg_self _ _ _,
      show a5.gpr .x9 = s.mem.readW (s.syms RS) 64 from
        (RegUpd.gpr_setReg_of_ne _ _ (by decide)).trans (RegUpd.gpr_setReg_self _ _ _)]
  have h7 : exec (.movGot .x12 "S") a6 = some (s6 s) := rfl
  rw [setup, execBlock_cons_some h1, execBlock_cons_some h2, execBlock_cons_some h3,
    execBlock_cons_some h4, execBlock_cons_some h5, execBlock_cons_some h6, execBlock_cons_some h7,
    execBlock_nil']

theorem s6_gpr {r : Reg} (h9 : r ≠ .x9) (h10 : r ≠ .x10) (h11 : r ≠ .x11) (h12 : r ≠ .x12) :
    (s6 s).gpr r = s.gpr r := by
  simp only [s6, RegUpd.gpr_setReg_of_ne _ _ h9, RegUpd.gpr_setReg_of_ne _ _ h10,
    RegUpd.gpr_setReg_of_ne _ _ h11, RegUpd.gpr_setReg_of_ne _ _ h12]

theorem setupOk : SetupOk target contract attempt sig [] setup := by
  refine .inr ⟨⟨ready, rfl⟩, fun s hp => ?_⟩
  have hw := pre_iff.mp hp
  have hf := hw.facts
  have hreg : regions abi (s6 s) = regions abi s := by
    rw [regions_eq, regions_eq, s6_gpr (by decide) (by decide) (by decide) (by decide)]
    rfl
  refine ⟨s6 s, execBlock_setup hf.rs, ⟨pre_iff.mpr ⟨hw.rd, hw.wr, ?_, ?_⟩, ?_, ?_, ?_⟩, rfl,
    ⟨fun r hr => ?_, rfl⟩, ?_, fun h => absurd h (by decide)⟩
  · rw [hreg]; exact hw.nowrap
  · rw [hreg]; exact hw.disj
  · show (s6 s).gpr .x10 = _
    rw [s6, RegUpd.gpr_setReg_of_ne _ _ (by decide), RegUpd.gpr_setReg_self]; rfl
  · show (s6 s).gpr .x11 = _
    simp only [s6, RegUpd.gpr_setReg_of_ne _ _ (show ¬(Reg.x11 = Reg.x12) by decide),
      RegUpd.gpr_setReg_of_ne _ _ (show ¬(Reg.x11 = Reg.x10) by decide),
      RegUpd.gpr_setReg_of_ne _ _ (show ¬(Reg.x11 = Reg.x9) by decide), RegUpd.gpr_setReg_self]
    rfl
  · show (s6 s).gpr .x12 = _
    rw [s6, RegUpd.gpr_setReg_self]; rfl
  · exact s6_gpr (fun h => by subst h; exact absurd hr (by decide))
      (fun h => by subst h; exact absurd hr (by decide))
      (fun h => by subst h; exact absurd hr (by decide))
      (fun h => by subst h; exact absurd hr (by decide))
  · exact argsKept_of (by decide) fun r hr => by
      have : r = .x0 := by simpa [sig, Sig.words, Param.words, IntTy.bits, argRegs] using hr
      subst this; exact s6_gpr (by decide) (by decide) (by decide) (by decide)

end Setup

/-- A thread whose rseq area is at `0x10100`, in the registers a setup leaves. -/
def w0 : State where
  gpr r := if r = .x10 then 0x10100 else if r = .x11 then 0x9000 else if r = .x12 then 0x3000
    else if r = .x0 then 0x5000 else 0
  mem a := if (a - 0x2000).toNat < 8 then (0x100 : BitVec 64).extractLsb' (8 * (a - 0x2000).toNat) 8 else 0
  rd := [⟨0x2000, 8⟩, ⟨0x3000, 8⟩]
  wr := [⟨0x10108, 8⟩, ⟨0x5000, 8⟩]
  syms n := if n = RS then 0x2000 else if n = "S" then 0x3000 else 0
  tpidr := 0x10000
  csDesc := 0x9000

theorem pre_w0 : attempt.pre w0 :=
  ⟨pre_iff.mpr ⟨by decide +kernel, by decide +kernel, by decide +kernel,
    Sig.pairwise_of_pairsSep (by decide +kernel)⟩, by decide +kernel, by decide +kernel,
    by decide +kernel⟩

theorem visits_w0 : Visits isa code w0 0 true :=
  have hf := (pre_iff.mp pre_w0.1).facts
  Visits.rseq (execBlock_arm hf pre_w0.2) (end_all hf pre_w0.2)

def witnesses : List State := [w0]

theorem witnessOk : (∃ w ∈ witnesses, attempt.pre w) ∧
    ∀ o ∈ code.outcomes, ∃ w ∈ witnesses, attempt.pre w ∧ Visits isa code w o.1 o.2 := by
  refine ⟨⟨w0, by simp [witnesses], pre_w0⟩, fun o ho => ?_⟩
  rw [show code.outcomes = [(0, true)] from rfl] at ho
  simp only [List.mem_cons, List.not_mem_nil, or_false] at ho
  subst ho
  exact ⟨w0, by simp [witnesses], pre_w0, visits_w0⟩

theorem verified : Verified target code attempt :=
  ⟨fun _ hs => correct hs, fun _ hs => rseqOk hs, ⟨w0, pre_w0⟩, fun o ho => by
    obtain ⟨w, -, hw, hv⟩ := witnessOk.2 o ho
    exact ⟨w, hw, hv⟩⟩

theorem restartsOk : ∀ s s', attempt.pre s → Restarts target code attempt s s' →
    target.abiPreserved s s' ∧ ArgsKept target.abi sig s s' ∧
    (target.inline = true → target.preservedExcept [] s s') :=
  restartsOk_target verified (by decide) fun _ hs p hp r hr => (mids hs p hp).gpr r
    (fun h => by subst h; exact absurd hr (by decide))
    (fun h => by subst h; exact absurd hr (by decide))

def api : Api where
  module := "forgery"
  name := "vg_forgery_movgot"
  sig := sig
  contracts := some fun A _ => theContract A
  summary := "A forgery."
  safety := fun _ => ["The thread's rseq area must be registered."]
  operands := [("__rseq_offset", .sym "crate::front::__rseq_offset"), ("S", .sym "crate::front::S")]

def art : Artifact := api.artifact VG.AArch64.target {
  setup := setup
  code := code
  attempt := attempt
  verified := verified
  restartsOk := restartsOk
  setupOk := setupOk
  witnesses := witnesses
  witnessOk := witnessOk }

#assert_standard_axioms VG.Test.Forgery.MovGot.art

#guard refusedFor art "`movGot`, one step of the model and two instructions"

end VG.Test.Forgery.MovGot
