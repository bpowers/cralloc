module

public import Asmalloc.Spec.Slab
public import Asmalloc.TCB.X86_64.Target
public import Asmalloc.Proof.Slab.Pre
public import Asmalloc.Proof.Slab.Mag

/-!
# The routines' precondition on x86-64, as facts

What a proof of a routine needs of its entry state: the regions it may
access, which are disjoint where one is written, and apart from the return
address; and the precondition beyond the signature (`Ctx.pre`).
-/

@[expose] public section

namespace VG.Proof.Slab.X86_64

open VG.X86_64 VG.Spec.Slab

/-- The name `glibc`'s offset has in the code and in the model. -/
abbrev RS : String := "__rseq_offset"

/-! ## The view through x86-64's calling conventions

`abi`, and the inline target's `inlineAbi`, which reserves the red zone too:
the routines' contracts see the state through either alike, but for the
memory each reserves. -/

/-- A calling convention that sees an x86-64 state as `abi` does, reserving
at least the return address's slot, and the same memory in two states with
the same stack pointer. -/
structure X86 (A : Abi isa) : Prop where
  ptrBits : A.ptrBits = 64
  args : A.args = abi.args
  argArea : A.argArea = abi.argArea
  wf : A.wf = abi.wf
  mem : ∀ s, A.mem s = s.mem
  rd : ∀ s, A.rd s = s.rd
  wr : ∀ s, A.wr s = s.wr
  ret : ∀ s, A.ret s = s.gpr .rax
  tp : A.tp = some fun s => s.fsBase
  sym : A.sym = some fun s => s.syms
  param : A.param = some fun s => s.params
  retSlot : ∀ s, (⟨s.gpr .rsp, 8⟩ : Region) ∈ A.reserved 0 s
  reserved : ∀ s s', s'.gpr .rsp = s.gpr .rsp → A.reserved 0 s' = A.reserved 0 s

theorem x86_abi : X86 abi :=
  ⟨rfl, rfl, rfl, rfl, fun _ => rfl, fun _ => rfl, fun _ => rfl, fun _ => rfl, rfl, rfl, rfl,
    fun _ => List.mem_singleton_self _, fun s s' h => by simp [abi, stackBelow, h]⟩

theorem x86_inline : X86 inlineAbi :=
  ⟨rfl, rfl, rfl, rfl, fun _ => rfl, fun _ => rfl, fun _ => rfl, fun _ => rfl, rfl, rfl, rfl,
    fun _ => List.mem_cons_of_mem _ (List.mem_singleton_self _),
    fun s s' h => by simp [inlineAbi, abi, stackBelow, h]⟩

@[simp] theorem abi_reserved (s : State) : abi.reserved 0 s = [⟨s.gpr .rsp, 8⟩] := rfl
@[simp] theorem inlineAbi_reserved (s : State) :
    inlineAbi.reserved 0 s = [⟨s.gpr .rsp - 128, 128⟩, ⟨s.gpr .rsp, 8⟩] := rfl

/-- The routine's view of the thread's rseq area, its fields and the CPU, on
x86-64. -/
def area (s : State) : Addr := s.fsBase + s.mem.readW (s.syms RS) 64

/-! ## The view of any routine on x86-64 -/

section ViewOf

variable {c : Spec.Slab.Ctx isa} (hA : X86 c.A) (s : State)
include hA

theorem tp_of : c.tp s = s.fsBase := by simp only [Spec.Slab.Ctx.tp, hA.tp]
theorem sym_of (n : String) : c.sym s n = s.syms n := by simp only [Spec.Slab.Ctx.sym, hA.sym]
theorem param_of (n : String) : c.param s n = s.params n := by
  simp only [Spec.Slab.Ctx.param, hA.param]
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

end ViewOf

/-! ## The facts -/

/-- What every routine's arm and CPU load rely on: the rseq area's
variable, `cpu_id` (read) and `rseq_cs` (written), the last two apart from
the first. -/
structure ArmOk (s : State) : Prop where
  rsVar : ⟨s.syms RS, 8⟩ ∈ s.rd
  cpuF : (⟨area s + 4, 4⟩ : Region) ∈ s.rd
  cs : (⟨area s + 8, 8⟩ : Region) ∈ s.wr
  cs_rsVar : Region.Disjoint ⟨area s + 8, 8⟩ ⟨s.syms RS, 8⟩

/-- What a routine `c` may rely on in its entry state `s`. -/
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
  ret_cs : Region.Disjoint ⟨area s + 8, 8⟩ ⟨s.gpr .rsp, 8⟩
  ret_word : c.InRange s → Region.Disjoint ⟨c.word s, 8⟩ ⟨s.gpr .rsp, 8⟩
  ret_mag : c.InRange s → IsSlot (c.slots s) (c.w s) → Region.Disjoint (c.magRegion s) ⟨s.gpr .rsp, 8⟩
  mag_nowrap : c.InRange s → IsSlot (c.slots s) (c.w s) →
    (c.magRegion s).base.toNat + (c.magRegion s).len ≤ 2 ^ 64
  pre : c.pre s

/-- The regions of a pop or push. -/
theorem regions_eq (c : Spec.Slab.Ctx isa) (hA : X86 c.A)
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

theorem facts_of_world {c : Spec.Slab.Ctx isa} (hA : X86 c.A)
    (hslot : c.form = .inline) {s : State}
    (hw : c.World s) (hp : c.pre s) : Facts c s := by
  obtain ⟨hrd, hwr, hnw, hdj, hres⟩ := hw
  have hres := fun x hx => hres x hx _ (hA.retSlot s)
  have hreg := regions_eq c hA hslot s
  simp only [Spec.Slab.Ctx.ro, Spec.Slab.Ctx.rw, hreg, hA.rd, hA.wr] at hrd hwr
  simp only [hreg, hA.ptrBits] at hnw hdj hres
  by_cases hi : c.InRange s <;> by_cases hs : IsSlot (c.slots s) (c.w s) <;>
  simp only [hi, hs, ↓reduceIte, List.append_nil, List.cons_append, List.nil_append,
    List.filter_cons, List.filter_nil, Bool.not_true, Bool.not_false, List.map_cons,
    List.map_nil, List.pairwise_cons, List.mem_cons, List.mem_nil_iff, List.Pairwise.nil,
    forall_eq_or_imp, forall_eq, or_false, Bool.true_or,
    Bool.false_or, Bool.false_eq_true, false_implies, implies_true, forall_const,
    and_true, true_and] at hrd hwr hnw hdj hres <;>
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
    ret_cs := by simp_all
    ret_word := by intro h; first | exact absurd h hi | simp_all
    ret_mag := by intro h h'; first | exact absurd h hi | exact absurd h' hs | simp_all
    mag_nowrap := by intro h h'; first | exact absurd h hi | exact absurd h' hs | simp_all
    pre := hp }

theorem Facts.toArm {c : Spec.Slab.Ctx isa} {s : State} (h : Facts c s) : ArmOk s :=
  ⟨h.rsVar, h.cpuF, h.cs, h.cs_rsVar⟩

end VG.Proof.Slab.X86_64
