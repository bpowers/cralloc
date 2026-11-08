module

public import Cralloc.Spec.Slab
public import Cralloc.Proof.Framework.Contract
public import Cralloc.Proof.Framework.Body
public import Cralloc.Proof.Slab.Pre
public import Cralloc.Proof.Framework.AArch64.Access
public import Cralloc.Proof.Framework.AArch64.Flags
public import Cralloc.Proof.Framework.AArch64.Retry

/-!
# Example: a restartable sequence with a setup

The smallest routine with the shape of the AArch64 routines, proved through all
that `Artifact` asks of one: a setup that forms the rseq area's and the descriptor's
addresses once, in `x10` and `x11`; an arm of one store, to which an abort returns;
and a body that reads the CPU number and, if it is in range, commits its second
argument to the word its first points at:

```
    adrp x11, 3f
    add  x11, x11, :lo12:3f
    ldr  x9, [x11, #32]        ; the word after the descriptor: &__rseq_offset
    ldr  x9, [x9]
    mrs  x10, tpidr_el0
    add  x10, x10, x9
5:  str  x11, [x10, #8]        ; arm; an abort returns here
6:  ldr  w8, [x10, #4]
    cmp  w8, #2, lsl #12
    b.hs 7f
    str  x1, [x0]              ; commit
7:
```

What a retry needs is its registers: `ready`, which the setup establishes
(`setupOk`) and an abort keeps (`retry`). The routine is of no use but as this
example: `CrallocTest/SeqAArch64.lean` emits it, and no crate compiles it.
-/

@[expose] public section

namespace VG.Proof.Examples.AArch64.Seq

open VG.Spec.Slab

/-- `fn(p: usize, v: u64)`. -/
def sig : Sig where
  params := [("p", .int .usize), ("v", .int .u64)]

section Contract

variable {M : ISA} (A : Abi M)

/-- The routine's view of the state, as the slab routines': the rseq area. -/
abbrev cx : Ctx M := ⟨A, .cas, sig⟩

/-- The word it commits to. -/
def slot (s : M.State) : Region := ⟨(cx A).arg s 0, 8⟩

/-- The memory it may access: `__rseq_offset` and `cpu_id` (read), `rseq_cs` and the
word (written). -/
def regions (s : M.State) : List (Region × Bool) :=
  [((cx A).rsVar s, false), ((cx A).cpuField s, false), ((cx A).csField s, true), (slot A s, true)]

def ro (s : M.State) : List Region := ((regions A s).filter (!·.2)).map (·.1)
def rw (s : M.State) : List Region := ((regions A s).filter (·.2)).map (·.1)

/-- It commits `v` to the word at `p` if the CPU number has a slab, and changes nothing but
`rseq_cs` and that word. Wherever it may be aborted, it has changed only `rseq_cs`.
While it is aborted, anything but `__rseq_offset` may change. -/
def seqContract : Contract M :=
  sig.contract (A.withExtra (some (regions A))) (post := Curry.const (fun _ _ _ => True) _)
    (x := {
      post := fun s s' => Unchanged [(cx A).csField s, slot A s] ((cx A).mem s) (A.mem s') ∧
        ((cx A).InRange s → (A.mem s').readW (slot A s).base 64 = (cx A).arg s 1)
      abort := fun s p => Unchanged [(cx A).csField s] ((cx A).mem s) (A.mem p)
      env := fun p p' => A.rd p' = ro A p' ∧ A.wr p' = rw A p' ∧
        Kept [(cx A).rsVar p] ((cx A).mem p) ((cx A).mem p')
      trace := fun _ _ => True })

end Contract

open VG.AArch64

abbrev RS : String := "__rseq_offset"

/-- The instructions run once. -/
def setup : List Instr := [
  .adrp .x11 .desc,
  .addLo12 .x11 .x11 .desc,
  .ldrWord .x9 .x11 RS,
  .ldr .x .x9 { base := .x9 },
  .mrsTp .x10,
  .add .x .x10 .x10 (.reg .x9)]

def arm : List Instr := [.str .x .x11 { base := .x10, off := .imm 8 }]

def body : List (Step Instr Cond) := [
  .instr (.ldr .w .x8 { base := .x10, off := .imm 4 }),
  .instr (.cmp .w .x8 (.imm 8192)),
  .exit .hs [] none,
  .instr (.str .x .x1 { base := .x0 })]

def code : Prog isa := .rseq arm body

/-! ## An attempt's precondition -/

/-- The rseq area of a state. -/
def area (s : State) : Addr := s.tpidr + s.mem.readW (s.syms RS) 64

/-- What a sequence's registers hold, wherever it may start. -/
def ready (s : State) : Prop := s.gpr .x10 = area s ∧ s.gpr .x11 = s.csDesc

abbrev contract : Contract isa := seqContract abi

/-- The contract of an attempt. -/
abbrev attempt : Contract isa := { contract with pre := fun s => contract.pre s ∧ ready s }

theorem regions_eq (s : State) : regions abi s =
    [(⟨s.syms RS, 8⟩, false), (⟨area s + 4, 4⟩, false), (⟨area s + 8, 8⟩, true),
      (⟨s.gpr .x0, 8⟩, true)] := rfl

theorem hv : abi.args ((sig.words abi.ptrBits).map (·.bits abi.ptrBits)) =
    some (fun s => [s.gpr .x0, s.gpr .x1]) := rfl

/-- The world a function of this contract runs in: its regions are the state's, none
wraps around, and those that are written are apart from the others. -/
structure World (s : State) : Prop where
  rd : s.rd = ro abi s
  wr : s.wr = rw abi s
  nowrap : ∀ x ∈ regions abi s, x.1.base.toNat + x.1.len ≤ 2 ^ 64
  disj : (regions abi s).Pairwise (fun a b => (a.2 || b.2) → a.1.Disjoint b.1)

theorem pre_iff {s : State} : contract.pre s ↔ World s := by
  have hb : Sig.bufs sig.params ((fun s : State => [s.gpr .x0, s.gpr .x1]) s) = [] := rfl
  have ha : abi.argArea ((sig.words abi.ptrBits).map (·.bits abi.ptrBits)) s = [] := rfl
  simp only [contract, seqContract, Sig.contract, Abi.withExtra, hv, hb, ha, List.map_nil,
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

theorem post_iff {s s' : State} : contract.post s s' ↔
    (Unchanged [⟨area s + 8, 8⟩, ⟨s.gpr .x0, 8⟩] s.mem s'.mem ∧
      ((s.mem.readW (area s + 4) 32).toNat < 8192 → s'.mem.readW (s.gpr .x0) 64 = s.gpr .x1)) := by
  simp only [contract, seqContract, Sig.contract, Abi.withExtra, hv, Curry.apply_const, true_and]
  exact Iff.rfl

theorem abort_iff {s p : State} : contract.abort s p ↔
    ([p.gpr .x0, p.gpr .x1] = [s.gpr .x0, s.gpr .x1] ∧ Unchanged [⟨area s + 8, 8⟩] s.mem p.mem) := by
  simp only [contract, seqContract, Sig.contract, Abi.withExtra, hv]
  exact Iff.rfl

/-- What the proof uses of the world. -/
structure Facts (s : State) : Prop where
  rs : ⟨s.syms RS, 8⟩ ∈ s.rd
  cpu : (⟨area s + 4, 4⟩ : Region) ∈ s.rd
  cs : (⟨area s + 8, 8⟩ : Region) ∈ s.wr
  word : (⟨s.gpr .x0, 8⟩ : Region) ∈ s.wr
  rs_cs : Region.Disjoint ⟨s.syms RS, 8⟩ ⟨area s + 8, 8⟩
  cpu_cs : Region.Disjoint ⟨area s + 4, 4⟩ ⟨area s + 8, 8⟩

theorem World.facts {s : State} (h : World s) : Facts s := by
  obtain ⟨hrd, hwr, -, hp⟩ := h
  rw [regions_eq] at hp
  have hrd' : s.rd = [⟨s.syms RS, 8⟩, ⟨area s + 4, 4⟩] := hrd
  have hwr' : s.wr = [⟨area s + 8, 8⟩, ⟨s.gpr .x0, 8⟩] := hwr
  have h1 := List.pairwise_cons.mp hp
  have h2 := List.pairwise_cons.mp h1.2
  exact {
    rs := by rw [hrd']; simp
    cpu := by rw [hrd']; simp
    cs := by rw [hwr']; simp
    word := by rw [hwr']; simp
    rs_cs := h1.1 (⟨area s + 8, 8⟩, true) (by simp) rfl
    cpu_cs := h2.1 (⟨area s + 8, 8⟩, true) (by simp) rfl }

/-! ## The steps of an attempt -/

section Steps

variable {s : State}

/-- Memory after arming. -/
def m1 (s : State) : Mem := s.mem.writeW (area s + 8) s.csDesc
def σA (s : State) : State := { s with mem := m1 s }
/-- The CPU number, as the routine reads it. -/
def cpu32 (s : State) : BitVec 32 := s.mem.readW (area s + 4) 32
def σ1 (s : State) : State := (σA s).setReg .x8 ((cpu32 s).setWidth 64)
def σ2 (s : State) : State := (σ1 s).addFlags (cpu32 s) (~~~(BitVec.ofNat 32 8192)) true
def mEnd (s : State) : Mem := (m1 s).writeW (s.gpr .x0) (s.gpr .x1)
def σ3 (s : State) : State := { σ2 s with mem := mEnd s }

theorem ea_imm_x10 (σ : State) (d : Nat) (h : σ.gpr .x10 = area s) :
    σ.ea { base := .x10, off := .imm (d : Int) } = area s + BitVec.ofNat 64 d := by
  rw [Access.ea_imm, h, BitVec.ofInt_natCast]

theorem arm_ea (hr : ready s) : s.ea { base := .x10, off := .imm 8 } = area s + 8 :=
  ea_imm_x10 s 8 hr.1

theorem exec_arm (h : Facts s) (hr : ready s) :
    exec (.str .x .x11 { base := .x10, off := .imm 8 }) s = some (σA s) := by
  have hin : InRegions s.wr (s.ea { base := .x10, off := .imm 8 }) 8 := by
    rw [arm_ea hr]; exact InRegions.of_mem h.cs (Region.contains_self _ _)
  rw [Access.exec_str_x hin, arm_ea hr, hr.2]
  rfl

theorem exec_cpu (h : Facts s) (hr : ready s) :
    exec (.ldr .w .x8 { base := .x10, off := .imm 4 }) (σA s) = some (σ1 s) := by
  have hea : (σA s).ea { base := .x10, off := .imm 4 } = area s + 4 := ea_imm_x10 _ 4 hr.1
  have hin : InRegions ((σA s).rd ++ (σA s).wr) ((σA s).ea { base := .x10, off := .imm 4 }) 4 := by
    rw [hea]
    exact (InRegions.of_mem h.cpu (Region.contains_self _ _)).append_left
  have hm : (σA s).mem.readW (area s + 4) 32 = cpu32 s :=
    Mem.readW_writeW_sep (h.cpu_cs.sep (Region.contains_self _ _) (Region.contains_self _ _)) (by decide)
  rw [Access.exec_ldr_w hin, hea, hm]
  rfl

theorem read_σ1 : (σ1 s).read .w .x8 = cpu32 s := by
  rw [RegUpd.read_w, σ1, RegUpd.gpr_setReg_self, RegUpd.setWidth_setWidth_32]

theorem exec_cmp : exec (.cmp .w .x8 (.imm 8192)) (σ1 s) = some (σ2 s) := by
  rw [Flags.exec_cmp, read_σ1]
  rfl

theorem eval_cpu : eval .hs (σ2 s) = some (decide (8192 ≤ (cpu32 s).toNat)) :=
  Flags.eval_hs_cmp _ _ _

theorem σ2_gpr {r : Reg} (hr : r ≠ .x8) : (σ2 s).gpr r = s.gpr r := by
  rw [σ2, RegUpd.gpr_addFlags, σ1, RegUpd.gpr_setReg_of_ne _ _ hr]
  rfl

theorem exec_commit (h : Facts s) :
    exec (.str .x .x1 { base := .x0 }) (σ2 s) = some (σ3 s) := by
  have hea : (σ2 s).ea { base := .x0 } = s.gpr .x0 := by
    rw [Access.ea_base, σ2_gpr (by decide)]
  have hin : InRegions (σ2 s).wr ((σ2 s).ea { base := .x0 }) 8 := by
    rw [hea]; exact InRegions.of_mem h.word (Region.contains_self _ _)
  rw [Access.exec_str_x hin, hea, σ2_gpr (by decide)]
  rfl

end Steps

/-! ## The two ways out -/

section Ways

variable {s : State}

theorem execBlock_arm (h : Facts s) (hr : ready s) : execBlock isa arm s = some (σA s) := by
  rw [arm, execBlock_cons_some (exec_arm h hr), execBlock_nil']

theorem runBody_miss (h : Facts s) (hr : ready s) (hi : 8192 ≤ (cpu32 s).toNat) :
    runBody isa body (σA s) = some (.done (σ2 s)) := by
  rw [body, runBody_instr (exec_cpu h hr), runBody_instr exec_cmp,
    runBody_take (by rw [show isa.eval = eval from rfl, eval_cpu]; simp [hi]) rfl]

theorem runBody_hit (h : Facts s) (hr : ready s) (hi : ¬ 8192 ≤ (cpu32 s).toNat) :
    runBody isa body (σA s) = some (.done (σ3 s)) := by
  rw [body, runBody_instr (exec_cpu h hr), runBody_instr exec_cmp,
    runBody_skip (by rw [show isa.eval = eval from rfl, eval_cpu]; simp [hi]),
    runBody_instr (exec_commit h), runBody_nil']

theorem prefixes_miss (h : Facts s) (hr : ready s) (hi : 8192 ≤ (cpu32 s).toNat) :
    bodyPrefixes isa body (σA s) = [σA s, σ1 s, σ2 s] := by
  rw [body, bodyPrefixes_instr (exec_cpu h hr), bodyPrefixes_instr exec_cmp,
    bodyPrefixes_take (by rw [show isa.eval = eval from rfl, eval_cpu]; simp [hi])]

theorem prefixes_hit (h : Facts s) (hr : ready s) (hi : ¬ 8192 ≤ (cpu32 s).toNat) :
    bodyPrefixes isa body (σA s) = [σA s, σ1 s, σ2 s, σ2 s] := by
  rw [body, bodyPrefixes_instr (exec_cpu h hr), bodyPrefixes_instr exec_cmp,
    bodyPrefixes_skip (by rw [show isa.eval = eval from rfl, eval_cpu]; simp [hi]),
    bodyPrefixes_last (exec_commit h)]

theorem end_miss (h : Facts s) (hr : ready s) (hi : 8192 ≤ (cpu32 s).toNat) :
    bodyEnd isa body (σA s) = some 2 := by
  rw [body, bodyEnd_instr (exec_cpu h hr), bodyEnd_instr exec_cmp,
    bodyEnd_take (by rw [show isa.eval = eval from rfl, eval_cpu]; simp [hi])]
  rfl

theorem end_hit (h : Facts s) (hr : ready s) (hi : ¬ 8192 ≤ (cpu32 s).toNat) :
    bodyEnd isa body (σA s) = some 4 := by
  rw [body, bodyEnd_instr (exec_cpu h hr), bodyEnd_instr exec_cmp,
    bodyEnd_skip (by rw [show isa.eval = eval from rfl, eval_cpu]; simp [hi]),
    bodyEnd_instr (exec_commit h), bodyEnd_nil']
  rfl

end Ways

/-! ## What each state keeps -/

/-- The state `p`, on the way from `s`: memory as after arming, and every register but
the CPU number's as in `s`. -/
structure Mid (s p : State) : Prop where
  mem : p.mem = m1 s
  gpr : ∀ r, r ≠ .x8 → p.gpr r = s.gpr r
  sp : p.sp = s.sp
  syms : p.syms = s.syms
  tpidr : p.tpidr = s.tpidr
  csDesc : p.csDesc = s.csDesc

section Keep

variable {s : State}

theorem mid_σA : Mid s (σA s) := ⟨rfl, fun _ _ => rfl, rfl, rfl, rfl, rfl⟩

theorem mid_σ1 : Mid s (σ1 s) :=
  ⟨rfl, fun _ hr => RegUpd.gpr_setReg_of_ne _ _ hr, rfl, rfl, rfl, rfl⟩

theorem mid_σ2 : Mid s (σ2 s) := ⟨rfl, fun _ hr => σ2_gpr hr, rfl, rfl, rfl, rfl⟩

theorem rs_m1 (h : Facts s) : (m1 s).readW (s.syms RS) 64 = s.mem.readW (s.syms RS) 64 :=
  Mem.readW_writeW_sep (h.rs_cs.sep (Region.contains_self _ _) (Region.contains_self _ _)) (by decide)

/-- `rseq_cs` holds the descriptor wherever the sequence may be aborted. -/
theorem armed_of_mid (h : Facts s) {p : State} (hm : Mid s p) : armed p := by
  have ha : rseqArea p = area s := by
    simp only [rseqArea, rseqOffsetSym, hm.mem, hm.tpidr, hm.syms]
    rw [rs_m1 h]
    rfl
  simp only [armed, rseqCsAddr, ha, hm.mem, hm.csDesc]
  exact Mem.readW_writeW_self64 _ _ _

theorem abi_of_mid {p : State} (hm : Mid s p) : abiPreserved s p :=
  ⟨fun r hr => hm.gpr r fun h => by subst h; exact absurd hr (by decide), hm.sp⟩

/-- Memory after arming differs from memory on entry only in `rseq_cs`. -/
theorem frame_m1 : Frame [⟨area s + 8, 8⟩] s.mem (m1 s) :=
  (Frame.refl _ _).writeW (List.mem_singleton_self _) _ (Region.contains_self _ _)

/-- A state the thread may resume in holds what a retry needs: the abort kept the
registers, and `env` kept `__rseq_offset`, so the addresses are those of `s`. -/
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
    rw [regions_eq, regions_eq, harea, hsy, hm.syms, hg, hm.gpr .x0 (by decide)]
  refine ⟨pre_iff.mpr ⟨hrdp, hwrp, ?_, ?_⟩, ?_, ?_⟩
  · rw [hreg]; exact hw'.nowrap
  · rw [hreg]; exact hw'.disj
  · rw [harea, hg, hm.gpr _ (by decide)]; exact hrd.1
  · rw [hg, hm.gpr _ (by decide), hcs, hm.csDesc]; exact hrd.2

/-- Where it may be aborted. -/
theorem abortOk (hs : attempt.pre s) {p : State} (hm : Mid s p) :
    armed p ∧ target.abiPreserved s p ∧ attempt.abort s p ∧
      ∀ p', target.rely p p' → attempt.env p p' → attempt.pre p' := by
  have hf := (pre_iff.mp hs.1).facts
  refine ⟨armed_of_mid hf hm, abi_of_mid hm, abort_iff.mpr ⟨?_, ?_⟩, fun p' hr he => retry hs hm hr he⟩
  · rw [hm.gpr .x0 (by decide), hm.gpr .x1 (by decide)]
  · rw [hm.mem]; exact frame_m1

end Keep

/-! ## The attempt -/

section Attempt

variable {s : State}

theorem mids (hs : attempt.pre s) : ∀ p ∈ Code.abortStates isa code s, Mid s p := by
  have hf := (pre_iff.mp hs.1).facts
  have hr := hs.2
  intro p hp
  simp only [code, Code.abortStates, execBlock_arm hf hr] at hp
  by_cases hi : 8192 ≤ (cpu32 s).toNat
  · rw [prefixes_miss hf hr hi] at hp
    simp only [List.mem_cons, List.not_mem_nil, or_false] at hp
    rcases hp with rfl | rfl | rfl
    · exact mid_σA
    · exact mid_σ1
    · exact mid_σ2
  · rw [prefixes_hit hf hr hi] at hp
    simp only [List.mem_cons, List.not_mem_nil, or_false] at hp
    rcases hp with rfl | rfl | rfl | rfl
    · exact mid_σA
    · exact mid_σ1
    · exact mid_σ2
    · exact mid_σ2

theorem correct (hs : attempt.pre s) :
    (∃ s', Exec isa code s s' ∧ target.abiPreserved s s' ∧ attempt.post s s') ∨
    (∃ t s', ExecExit isa code s t s' ∧ target.abiPreserved s s' ∧ attempt.exit t s s') := by
  have hf := (pre_iff.mp hs.1).facts
  have hr := hs.2
  refine .inl ?_
  by_cases hi : 8192 ≤ (cpu32 s).toNat
  · refine ⟨σ2 s, .rseq (execBlock_arm hf hr) (runBody_miss hf hr hi), abi_of_mid mid_σ2,
      post_iff.mpr ⟨?_, fun h => ?_⟩⟩
    · exact frame_m1.mono fun r hr => by
        rw [List.mem_singleton.mp hr]; exact List.mem_cons_self ..
    · have : ¬ (s.mem.readW (area s + 4) 32).toNat < 8192 := by
        have h' : 8192 ≤ (s.mem.readW (area s + 4) 32).toNat := hi
        omega
      exact absurd h this
  · refine ⟨σ3 s, .rseq (execBlock_arm hf hr) (runBody_hit hf hr hi), ?_, post_iff.mpr ⟨?_, ?_⟩⟩
    · refine ⟨fun r hr => ?_, rfl⟩
      show (σ2 s).gpr r = s.gpr r
      exact σ2_gpr fun h => by subst h; exact absurd hr (by decide)
    · exact ((Frame.refl _ _).writeW (r := ⟨area s + 8, 8⟩) (List.mem_cons_self ..) _
        (Region.contains_self _ _)).writeW
        (r := ⟨s.gpr .x0, 8⟩) (List.mem_cons_of_mem _ (List.mem_singleton_self _)) _
        (Region.contains_self _ _)
    · exact fun _ => Mem.readW_writeW_self64 _ _ _

theorem rseqOk (hs : attempt.pre s) : RseqOk target code attempt s :=
  ⟨trivial, ⟨_, s, rfl, rfl, arm_ea hs.2, hs.2.2⟩, fun p hp => abortOk hs (mids hs p hp)⟩

end Attempt

/-! ## The setup -/

section Setup

variable {s : State}

/-- The address of the descriptor, in `x11`. -/
def s2 (s : State) : State := (s.setReg .x11 (page s.csDesc)).setReg .x11 s.csDesc
def s3 (s : State) : State := (s2 s).setReg .x9 (s.syms RS)
/-- The area's offset from the thread pointer. -/
def off (s : State) : Addr := s.mem.readW (s.syms RS) 64
def s4 (s : State) : State := (s3 s).setReg .x9 (off s)
def s5 (s : State) : State := (s4 s).setReg .x10 s.tpidr
def s6 (s : State) : State := (s5 s).setReg .x10 (s.tpidr + off s)

theorem exec_ldrWord {σ : State} {d n : Reg} {name : String} (h : σ.gpr n = σ.csDesc) :
    exec (.ldrWord d n name) σ = some (σ.setReg d (σ.syms name)) := by
  simp only [exec, h, ↓reduceIte]

theorem execBlock_setup (h : ⟨s.syms RS, 8⟩ ∈ s.rd) : execBlock isa setup s = some (s6 s) := by
  have h1 : exec (.adrp .x11 .desc) s = some (s.setReg .x11 (page s.csDesc)) := rfl
  have h2 : exec (.addLo12 .x11 .x11 .desc) (s.setReg .x11 (page s.csDesc)) = some (s2 s) := by
    show some ((s.setReg .x11 (page s.csDesc)).setReg .x11
      ((s.setReg .x11 (page s.csDesc)).gpr .x11 + lo12 s.csDesc)) = _
    rw [RegUpd.gpr_setReg_self, Access.page_add_lo12]
    rfl
  have h3 : exec (.ldrWord .x9 .x11 RS) (s2 s) = some (s3 s) :=
    exec_ldrWord (RegUpd.gpr_setReg_self _ _ _)
  have hea : (s3 s).ea { base := .x9 } = s.syms RS := by
    rw [Access.ea_base, s3, RegUpd.gpr_setReg_self]
  have h4 : exec (.ldr .x .x9 { base := .x9 }) (s3 s) = some (s4 s) := by
    rw [Access.exec_ldr_x (by
      rw [hea]; exact (InRegions.of_mem (r := ⟨s.syms RS, 8⟩) h (Region.contains_self _ _)).append_left), hea]
    rfl
  have h5 : exec (.mrsTp .x10) (s4 s) = some (s5 s) := rfl
  have h6 : exec (.add .x .x10 .x10 (.reg .x9)) (s5 s) = some (s6 s) := by
    show some ((s5 s).setReg .x10 ((s5 s).gpr .x10 + (s5 s).gpr .x9)) = _
    rw [s5, RegUpd.gpr_setReg_self, RegUpd.gpr_setReg_of_ne _ _ (by decide), s4, RegUpd.gpr_setReg_self]
    rfl
  rw [setup, execBlock_cons_some h1, execBlock_cons_some h2, execBlock_cons_some h3,
    execBlock_cons_some h4, execBlock_cons_some h5, execBlock_cons_some h6, execBlock_nil']

theorem s6_gpr {r : Reg} (h9 : r ≠ .x9) (h10 : r ≠ .x10) (h11 : r ≠ .x11) : (s6 s).gpr r = s.gpr r := by
  rw [s6, RegUpd.gpr_setReg_of_ne _ _ h10, s5, RegUpd.gpr_setReg_of_ne _ _ h10, s4,
    RegUpd.gpr_setReg_of_ne _ _ h9, s3, RegUpd.gpr_setReg_of_ne _ _ h9, s2,
    RegUpd.gpr_setReg_of_ne _ _ h11, RegUpd.gpr_setReg_of_ne _ _ h11]

/-- The setup leaves what an attempt needs. -/
theorem setupOk : SetupOk target contract attempt sig [] setup := by
  refine .inr ⟨⟨ready, rfl⟩, fun s hp => ?_⟩
  have hw := pre_iff.mp hp
  have hf := hw.facts
  have hreg : regions abi (s6 s) = regions abi s := by
    rw [regions_eq, regions_eq, s6_gpr (by decide) (by decide) (by decide)]
    rfl
  refine ⟨s6 s, execBlock_setup hf.rs, ⟨pre_iff.mpr ⟨hw.rd, hw.wr, ?_, ?_⟩, ?_, ?_⟩, rfl,
    ⟨fun r hr => ?_, rfl⟩, ?_, fun h => absurd h (by decide)⟩
  · rw [hreg]; exact hw.nowrap
  · rw [hreg]; exact hw.disj
  · rw [s6, RegUpd.gpr_setReg_self]; rfl
  · rw [s6, RegUpd.gpr_setReg_of_ne _ _ (by decide), s5, RegUpd.gpr_setReg_of_ne _ _ (by decide), s4,
      RegUpd.gpr_setReg_of_ne _ _ (by decide), s3, RegUpd.gpr_setReg_of_ne _ _ (by decide), s2,
      RegUpd.gpr_setReg_self]
    rfl
  · exact s6_gpr (fun h => by subst h; exact absurd hr (by decide))
      (fun h => by subst h; exact absurd hr (by decide))
      (fun h => by subst h; exact absurd hr (by decide))
  · exact argsKept_of (by decide) fun r hr => by
      have : r = .x0 ∨ r = .x1 := by simpa [sig, Sig.words, Param.words, IntTy.bits, argRegs] using hr
      rcases this with rfl | rfl <;> exact s6_gpr (by decide) (by decide) (by decide)

end Setup

/-! ## Its witnesses -/

/-- A thread whose rseq area is at `0x10100`, at the offset `0x100` from its thread
pointer, its CPU number `cpu`, in the registers a setup leaves. -/
def wit (cpu : BitVec 32) : State where
  gpr r := if r = .x10 then 0x10100 else if r = .x11 then 0x9000 else if r = .x0 then 0x5000 else if r = .x1 then 7 else 0
  mem a :=
    if (a - 0x2000).toNat < 8 then (0x100 : BitVec 64).extractLsb' (8 * (a - 0x2000).toNat) 8
    else if (a - 0x10104).toNat < 4 then cpu.extractLsb' (8 * (a - 0x10104).toNat) 8
    else 0
  rd := [⟨0x2000, 8⟩, ⟨0x10104, 4⟩]
  wr := [⟨0x10108, 8⟩, ⟨0x5000, 8⟩]
  syms n := if n = RS then 0x2000 else 0
  tpidr := 0x10000
  csDesc := 0x9000

def w0 : State := wit 8192
def w1 : State := wit 3

theorem pre_w0 : attempt.pre w0 :=
  ⟨pre_iff.mpr ⟨by decide +kernel, by decide +kernel, by decide +kernel,
    Sig.pairwise_of_pairsSep (by decide +kernel)⟩, by decide +kernel, by decide +kernel⟩

theorem pre_w1 : attempt.pre w1 :=
  ⟨pre_iff.mpr ⟨by decide +kernel, by decide +kernel, by decide +kernel,
    Sig.pairwise_of_pairsSep (by decide +kernel)⟩, by decide +kernel, by decide +kernel⟩

theorem visits_w0 : Visits isa code w0 0 true :=
  have hf := (pre_iff.mp pre_w0.1).facts
  Visits.rseq (execBlock_arm hf pre_w0.2) (end_miss hf pre_w0.2 (by decide +kernel))

theorem visits_w1 : Visits isa code w1 1 true :=
  have hf := (pre_iff.mp pre_w1.1).facts
  Visits.rseq (execBlock_arm hf pre_w1.2) (end_hit hf pre_w1.2 (by decide +kernel))

def witnesses : List State := [w0, w1]

theorem witnessOk : (∃ w ∈ witnesses, attempt.pre w) ∧
    ∀ o ∈ code.outcomes, ∃ w ∈ witnesses, attempt.pre w ∧ Visits isa code w o.1 o.2 := by
  refine ⟨⟨w0, by simp [witnesses], pre_w0⟩, fun o ho => ?_⟩
  rw [show code.outcomes = [(0, true), (1, true)] from rfl] at ho
  simp only [List.mem_cons, List.not_mem_nil, or_false] at ho
  rcases ho with rfl | rfl
  · exact ⟨w0, by simp [witnesses], pre_w0, visits_w0⟩
  · exact ⟨w1, by simp [witnesses], pre_w1, visits_w1⟩

theorem verified : Verified target code attempt :=
  ⟨fun _ hs => correct hs, fun _ hs => rseqOk hs, ⟨w0, pre_w0⟩, fun o ho => by
    obtain ⟨w, -, hw, hv⟩ := witnessOk.2 o ho
    exact ⟨w, hw, hv⟩⟩

theorem restartsOk : ∀ s s', attempt.pre s → Restarts target code attempt s s' →
    target.abiPreserved s s' ∧ ArgsKept target.abi sig s s' ∧
    (target.inline = true → target.preservedExcept [] s s') :=
  restartsOk_target verified (by decide) fun _ hs p hp r hr => (mids hs p hp).gpr r fun h => by
    subst h; exact absurd hr (by decide)

/-- What a run of the whole function does: `Verified.setup_restarts`. Its setup runs,
and each of its attempts, however many aborts come first, starts as `attempt`
says and completes. -/
theorem run {s : State} (hp : contract.pre s) :
    ∃ a, execBlock isa setup s = some a ∧ attempt.pre a ∧ a.mem = s.mem ∧
      ∀ s', Restarts target code attempt a s' → attempt.pre s' ∧
        ((∃ s'', Exec isa code s' s'' ∧ target.abiPreserved s' s'' ∧ attempt.post s' s'') ∨
          (∃ t s'', ExecExit isa code s' t s'' ∧ target.abiPreserved s' s'' ∧ attempt.exit t s' s'')) :=
  Verified.setup_restarts verified setupOk hp

end VG.Proof.Examples.AArch64.Seq
