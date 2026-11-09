module

public import Cralloc.Spec.Slab
public import Cralloc.TCB.AArch64.Target
public import Cralloc.Proof.Slab.Pre
public import Cralloc.Proof.Slab.Words

/-!
# The routines' precondition on AArch64, as facts

What a proof of a routine needs of the state an attempt starts in: the regions it
may access, which are disjoint where one is written; the precondition beyond the
signature (`Ctx.pre`); and the registers the setup leaves (`ready`).
-/

@[expose] public section

namespace VG.Proof.Slab.AArch64

open VG.AArch64 VG.Spec.Slab

/-- The name of the variable holding the rseq area's offset, in the code and in the
model. -/
abbrev RS : String := "__rseq_offset"

/-! ## The view through AArch64's calling conventions

`abi`, and the inline targets' `inlineAbi` and `inlineAbiX4`, which differ from it in
where the arguments are and the result: the routines' contracts see the state through
each alike. None reserves any memory. -/

/-- A calling convention that sees an AArch64 state as `abi` does, with its arguments
in the registers `regs`. -/
structure A64 (A : Abi isa) (regs : List Reg) : Prop where
  ptrBits : A.ptrBits = 64
  args : A.args = fun ws => if ws.length ≤ regs.length then
    some fun s => (regs.take ws.length).map s.gpr else none
  argArea : A.argArea = abi.argArea
  wf : A.wf = abi.wf
  mem : ∀ s, A.mem s = s.mem
  rd : ∀ s, A.rd s = s.rd
  wr : ∀ s, A.wr s = s.wr
  tp : A.tp = some fun s => s.tpidr
  sym : A.sym = some fun s => s.syms
  param : A.param = none
  reserved : ∀ s, A.reserved 0 s = []
  setupFinds : A.setupFindsArea = true
  slabsPage : A.slabsOnPage = true

theorem a64_abi : A64 abi argRegs :=
  ⟨rfl, rfl, rfl, rfl, fun _ => rfl, fun _ => rfl, fun _ => rfl, rfl, rfl, rfl, fun _ => rfl, rfl, rfl⟩

theorem a64_inline : A64 inlineAbi inlineArgRegs :=
  ⟨rfl, rfl, rfl, rfl, fun _ => rfl, fun _ => rfl, fun _ => rfl, rfl, rfl, rfl, fun _ => rfl, rfl, rfl⟩

theorem a64_inlineX4 : A64 inlineAbiX4 inlineArgRegs :=
  ⟨rfl, rfl, rfl, rfl, fun _ => rfl, fun _ => rfl, fun _ => rfl, rfl, rfl, rfl, fun _ => rfl, rfl, rfl⟩

/-- The thread's rseq area. -/
def area (s : State) : Addr := s.tpidr + s.mem.readW (s.syms RS) 64

/-- What the setup leaves, which a sequence reads and no sequence writes. -/
def ready (s : State) : Prop := s.gpr .x10 = area s ∧ s.gpr .x11 = s.csDesc

/-- The contract of an attempt of a routine of the contract `k`: its precondition, and
`ready`. -/
abbrev withReady (k : Contract isa) : Contract isa := { k with pre := fun s => k.pre s ∧ ready s }

/-! ## The view of any routine on AArch64 -/

section ViewOf

variable {regs : List Reg} {c : Spec.Slab.Ctx isa} (hA : A64 c.A regs) (s : State)
include hA

theorem tp_of : c.tp s = s.tpidr := by simp only [Spec.Slab.Ctx.tp, hA.tp]
theorem sym_of (n : String) : c.sym s n = s.syms n := by simp only [Spec.Slab.Ctx.sym, hA.sym]
theorem param_of (n : String) : c.param s n = 0 := by simp only [Spec.Slab.Ctx.param, hA.param]
theorem mem_of : c.mem s = s.mem := by simp only [Spec.Slab.Ctx.mem, hA.mem]
theorem area_of : c.area s = area s := by
  simp only [Spec.Slab.Ctx.area, tp_of hA, sym_of hA, mem_of hA]; rfl
theorem csField_of : c.csField s = ⟨area s + 8, 8⟩ := by
  simp only [Spec.Slab.Ctx.csField, area_of hA]; rfl
theorem cpuField_of : c.cpuField s = ⟨area s + 4, 4⟩ := by
  simp only [Spec.Slab.Ctx.cpuField, area_of hA]; rfl
theorem cpu_of : c.cpu s = (s.mem.readW (area s + 4) 32).toNat := by
  simp only [Spec.Slab.Ctx.cpu, cpuField_of hA, mem_of hA]
theorem rsVar_of : c.rsVar s = ⟨s.syms RS, 8⟩ := by
  simp only [Spec.Slab.Ctx.rsVar, sym_of hA]; rfl

/-- The accesses of an attempt, as a contract's trace states them, start with the
read of `__rseq_offset`, which the setup made. -/
theorem attemptEvents_eq (evs : List Event) :
    c.attemptEvents s evs = ⟨s.syms RS, 8, false, (s.mem.readW (s.syms RS) 64).toNat⟩ :: evs := by
  simp only [Spec.Slab.Ctx.attemptEvents, hA.setupFinds, sym_of hA, mem_of hA, Bool.cond_true,
    Spec.Slab.Ctx.rd]
  rfl

end ViewOf

/-! ## The facts -/

/-- What every routine's arm and CPU load rely on: the rseq area's variable, `cpu_id`
(read) and `rseq_cs` (written), the last apart from the first; and the registers the
setup leaves. -/
structure ArmOk (s : State) : Prop where
  rsVar : ⟨s.syms RS, 8⟩ ∈ s.rd
  cpuF : (⟨area s + 4, 4⟩ : Region) ∈ s.rd
  cs : (⟨area s + 8, 8⟩ : Region) ∈ s.wr
  cs_rsVar : Region.Disjoint ⟨area s + 8, 8⟩ ⟨s.syms RS, 8⟩
  ready : ready s

/-- What a pop or push `c` may rely on in the state `s` an attempt starts in. -/
structure Facts (c : Spec.Slab.Ctx isa) (s : State) : Prop where
  rsVar : ⟨s.syms RS, 8⟩ ∈ s.rd
  cpuF : (⟨area s + 4, 4⟩ : Region) ∈ s.rd
  cs : (⟨area s + 8, 8⟩ : Region) ∈ s.wr
  word : c.InRange s → ⟨c.word s, 8⟩ ∈ s.wr
  mag : c.InRange s → IsSlot (c.slots s) (c.w s) → c.magRegion s ∈ s.wr
  cs_rsVar : Region.Disjoint ⟨area s + 8, 8⟩ ⟨s.syms RS, 8⟩
  cs_word : c.InRange s → Region.Disjoint ⟨area s + 8, 8⟩ ⟨c.word s, 8⟩
  cs_mag : c.InRange s → IsSlot (c.slots s) (c.w s) → Region.Disjoint ⟨area s + 8, 8⟩ (c.magRegion s)
  word_mag : c.InRange s → IsSlot (c.slots s) (c.w s) → Region.Disjoint ⟨c.word s, 8⟩ (c.magRegion s)
  rsVar_mag : c.InRange s → IsSlot (c.slots s) (c.w s) →
    Region.Disjoint ⟨s.syms RS, 8⟩ (c.magRegion s)
  pre : c.pre s
  ready : ready s

/-- The regions of a pop or push. -/
theorem regions_eq {regs : List Reg} (c : Spec.Slab.Ctx isa) (hA : A64 c.A regs)
    (hslot : c.form = .inline) (s : State) :
    c.regions s = [(⟨s.syms RS, 8⟩, false), (⟨area s + 4, 4⟩, false), (⟨area s + 8, 8⟩, true)] ++
      (if c.InRange s then
        (⟨c.word s, 8⟩, true) :: (if IsSlot (c.slots s) (c.w s) then [(c.magRegion s, true)] else [])
      else []) := by
  obtain ⟨A, f, sig⟩ := c
  cases hslot
  simp only [Spec.Slab.Ctx.regions, Spec.Slab.Ctx.fixed, Spec.Slab.Ctx.more, rsVar_of hA,
    cpuField_of hA, csField_of hA, List.append_assoc]
  rfl

theorem facts_of_world {regs : List Reg} {c : Spec.Slab.Ctx isa} (hA : A64 c.A regs)
    (hslot : c.form = .inline) {s : State}
    (hw : c.World s) (hp : c.pre s) (hr : ready s) : Facts c s := by
  obtain ⟨hrd, hwr, -, hdj, -⟩ := hw
  have hreg := regions_eq c hA hslot s
  simp only [Spec.Slab.Ctx.ro, Spec.Slab.Ctx.rw, hreg, hA.rd, hA.wr] at hrd hwr
  simp only [hreg] at hdj
  by_cases hi : c.InRange s <;> by_cases hs : IsSlot (c.slots s) (c.w s) <;>
  simp only [hi, hs, ↓reduceIte, List.append_nil, List.cons_append, List.nil_append,
    List.filter_cons, List.filter_nil, Bool.not_true, Bool.not_false, List.map_cons,
    List.map_nil, List.pairwise_cons, List.mem_cons, List.mem_nil_iff, List.Pairwise.nil,
    forall_eq_or_imp, forall_eq, or_false, Bool.true_or,
    Bool.false_or, Bool.false_eq_true, false_implies, implies_true, forall_const,
    and_true, true_and] at hrd hwr hdj <;>
  exact {
    rsVar := by rw [hrd]; simp
    cpuF := by rw [hrd]; simp
    cs := by rw [hwr]; simp
    word := by intro h; first | exact absurd h hi | (rw [hwr]; simp)
    mag := by intro h h'; first | exact absurd h hi | exact absurd h' hs | (rw [hwr]; simp)
    cs_rsVar := Region.Disjoint.symm (by simp_all)
    cs_word := by intro h; first | exact absurd h hi | simp_all
    cs_mag := by intro h h'; first | exact absurd h hi | exact absurd h' hs | simp_all
    word_mag := by intro h h'; first | exact absurd h hi | exact absurd h' hs | simp_all
    rsVar_mag := by intro h h'; first | exact absurd h hi | exact absurd h' hs | simp_all
    pre := hp
    ready := hr }

theorem Facts.toArm {c : Spec.Slab.Ctx isa} {s : State} (h : Facts c s) : ArmOk s :=
  ⟨h.rsVar, h.cpuF, h.cs, h.cs_rsVar, h.ready⟩

end VG.Proof.Slab.AArch64
