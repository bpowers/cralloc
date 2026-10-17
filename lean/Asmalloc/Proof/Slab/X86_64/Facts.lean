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

/-! ## The view through x86-64's calling convention -/

section View

variable {f : Form} {sig : Sig} (s : State)

@[simp] theorem view_A : (⟨abi, f, sig⟩ : Spec.Slab.Ctx isa).A = abi := rfl
@[simp] theorem view_form : (⟨abi, f, sig⟩ : Spec.Slab.Ctx isa).form = f := rfl
@[simp] theorem view_tp : (⟨abi, f, sig⟩ : Spec.Slab.Ctx isa).tp s = s.fsBase := rfl
@[simp] theorem view_sym (n : String) : (⟨abi, f, sig⟩ : Spec.Slab.Ctx isa).sym s n = s.syms n := rfl
@[simp] theorem view_param (n : String) :
    (⟨abi, f, sig⟩ : Spec.Slab.Ctx isa).param s n = s.params n := rfl
@[simp] theorem view_mem : (⟨abi, f, sig⟩ : Spec.Slab.Ctx isa).mem s = s.mem := rfl
@[simp] theorem abi_mem : abi.mem s = s.mem := rfl
@[simp] theorem abi_rd : abi.rd s = s.rd := rfl
@[simp] theorem abi_wr : abi.wr s = s.wr := rfl
@[simp] theorem abi_ret : abi.ret s = s.gpr .rax := rfl
@[simp] theorem abi_reserved : abi.reserved 0 s = [⟨s.gpr .rsp, 8⟩] := rfl
@[simp] theorem abi_ptrBits : abi.ptrBits = 64 := rfl

end View

/-- The routine's view of the thread's rseq area, its fields and the CPU, on
x86-64. -/
def area (s : State) : Addr := s.fsBase + s.mem.readW (s.syms RS) 64

section Fields

variable {f : Form} {sig : Sig} (s : State)

theorem view_area : (⟨abi, f, sig⟩ : Spec.Slab.Ctx isa).area s = area s := rfl
theorem view_csField : (⟨abi, f, sig⟩ : Spec.Slab.Ctx isa).csField s = ⟨area s + 8, 8⟩ := rfl
theorem view_cpuField : (⟨abi, f, sig⟩ : Spec.Slab.Ctx isa).cpuField s = ⟨area s + 4, 4⟩ := rfl
theorem view_cpu :
    (⟨abi, f, sig⟩ : Spec.Slab.Ctx isa).cpu s = (s.mem.readW (area s + 4) 32).toNat := rfl

end Fields

/-! ## The view of any routine on x86-64 -/

section ViewOf

variable {c : Spec.Slab.Ctx isa} (hA : c.A = abi) (s : State)
include hA

theorem tp_of : c.tp s = s.fsBase := by simp only [Spec.Slab.Ctx.tp, hA]; rfl
theorem sym_of (n : String) : c.sym s n = s.syms n := by simp only [Spec.Slab.Ctx.sym, hA]; rfl
theorem param_of (n : String) : c.param s n = s.params n := by
  simp only [Spec.Slab.Ctx.param, hA]; rfl
theorem mem_of : c.mem s = s.mem := by simp only [Spec.Slab.Ctx.mem, hA]; rfl
theorem area_of : c.area s = area s := by
  simp only [Spec.Slab.Ctx.area, tp_of hA, sym_of hA, mem_of hA]; rfl
theorem csField_of : c.csField s = ⟨area s + 8, 8⟩ := by
  simp only [Spec.Slab.Ctx.csField, area_of hA]; rfl
theorem cpuField_of : c.cpuField s = ⟨area s + 4, 4⟩ := by
  simp only [Spec.Slab.Ctx.cpuField, area_of hA]; rfl
theorem cpu_of : c.cpu s = (s.mem.readW (area s + 4) 32).toNat := by
  simp only [Spec.Slab.Ctx.cpu, cpuField_of hA, mem_of hA]

end ViewOf

/-! ## The facts -/

/-- What a routine `c` may rely on in its entry state `s`. -/
structure Facts (c : Spec.Slab.Ctx isa) (s : State) : Prop where
  rsVar : ⟨s.syms RS, 8⟩ ∈ s.rd
  cpuF : (⟨area s + 4, 4⟩ : Region) ∈ s.rd
  cs : (⟨area s + 8, 8⟩ : Region) ∈ s.wr
  mask : c.form = .table → ⟨c.maskAddr s, 4⟩ ∈ s.rd
  word : c.InRange s → ⟨c.word s, 8⟩ ∈ s.wr
  mag : c.InRange s → IsSlot (c.slots s) (c.w s) → c.magRegion s ∈ s.wr
  cs_rsVar : Region.Disjoint ⟨area s + 8, 8⟩ ⟨s.syms RS, 8⟩
  cs_mask : c.form = .table → Region.Disjoint ⟨area s + 8, 8⟩ ⟨c.maskAddr s, 4⟩
  cs_word : c.InRange s → Region.Disjoint ⟨area s + 8, 8⟩ ⟨c.word s, 8⟩
  cs_mag : c.InRange s → IsSlot (c.slots s) (c.w s) → Region.Disjoint ⟨area s + 8, 8⟩ (c.magRegion s)
  word_mag : c.InRange s → IsSlot (c.slots s) (c.w s) → Region.Disjoint ⟨c.word s, 8⟩ (c.magRegion s)
  rsVar_mag : c.InRange s → IsSlot (c.slots s) (c.w s) →
    Region.Disjoint ⟨s.syms RS, 8⟩ (c.magRegion s)
  mask_mag : c.form = .table → c.InRange s → IsSlot (c.slots s) (c.w s) →
    Region.Disjoint ⟨c.maskAddr s, 4⟩ (c.magRegion s)
  ret_cs : Region.Disjoint ⟨area s + 8, 8⟩ ⟨s.gpr .rsp, 8⟩
  ret_word : c.InRange s → Region.Disjoint ⟨c.word s, 8⟩ ⟨s.gpr .rsp, 8⟩
  ret_mag : c.InRange s → IsSlot (c.slots s) (c.w s) → Region.Disjoint (c.magRegion s) ⟨s.gpr .rsp, 8⟩
  mag_nowrap : c.InRange s → IsSlot (c.slots s) (c.w s) →
    (c.magRegion s).base.toNat + (c.magRegion s).len ≤ 2 ^ 64
  pre : c.pre s

/-- The regions of a routine, in its three cases. -/
theorem regions_eq (c : Spec.Slab.Ctx isa) (hA : c.A = abi) (s : State) :
    c.regions s = [(⟨s.syms RS, 8⟩, false), (⟨area s + 4, 4⟩, false), (⟨area s + 8, 8⟩, true)] ++
      (match c.form with | .table => [(⟨c.maskAddr s, 4⟩, false)] | .inline => []) ++
      (if c.InRange s then
        (⟨c.word s, 8⟩, true) :: (if IsSlot (c.slots s) (c.w s) then [(c.magRegion s, true)] else [])
      else []) := by
  obtain ⟨A, f, sig⟩ := c
  cases hA
  rfl

theorem facts_of_world {c : Spec.Slab.Ctx isa} (hA : c.A = abi) {s : State}
    (hw : Ctx.World c s) (hp : c.pre s) : Facts c s := by
  obtain ⟨hrd, hwr, hnw, hdj, hres⟩ := hw
  have hreg := regions_eq c hA s
  simp only [Ctx.ro, Ctx.rw, hreg, hA, abi_rd, abi_wr] at hrd hwr
  simp only [hreg, hA, abi_reserved, abi_ptrBits] at hnw hdj hres
  rcases hf : c.form with _ | _ <;>
  by_cases hi : c.InRange s <;> by_cases hs : IsSlot (c.slots s) (c.w s) <;>
  simp only [hf, hi, hs, ↓reduceIte, List.append_nil, List.cons_append, List.nil_append,
    List.filter_cons, List.filter_nil, Bool.not_true, Bool.not_false, List.map_cons,
    List.map_nil, List.pairwise_cons, List.mem_cons, List.mem_nil_iff, List.Pairwise.nil,
    forall_eq_or_imp, forall_eq, or_false, Bool.true_or,
    Bool.false_or, Bool.false_eq_true, false_implies, implies_true, forall_const,
    and_true, true_and] at hrd hwr hnw hdj hres <;>
  exact {
    rsVar := by rw [hrd]; simp
    cpuF := by rw [hrd]; simp
    cs := by rw [hwr]; simp
    mask := by intro h; rw [hf] at h; first | (rw [hrd]; simp; done) | cases h
    word := by intro h; first | exact absurd h hi | (rw [hwr]; simp)
    mag := by intro h h'; first | exact absurd h hi | exact absurd h' hs | (rw [hwr]; simp)
    cs_rsVar := Region.Disjoint.symm (by simp_all)
    cs_mask := by intro h; rw [hf] at h; first | (simp_all; done) | cases h
    cs_word := by intro h; first | exact absurd h hi | simp_all
    cs_mag := by intro h h'; first | exact absurd h hi | exact absurd h' hs | simp_all
    word_mag := by intro h h'; first | exact absurd h hi | exact absurd h' hs | simp_all
    rsVar_mag := by intro h h'; first | exact absurd h hi | exact absurd h' hs | simp_all
    mask_mag := by
      intro h h' h''; rw [hf] at h; first | exact absurd h' hi | exact absurd h'' hs | (simp_all; done) | cases h
    ret_cs := by simp_all
    ret_word := by intro h; first | exact absurd h hi | simp_all
    ret_mag := by intro h h'; first | exact absurd h hi | exact absurd h' hs | simp_all
    mag_nowrap := by intro h h'; first | exact absurd h hi | exact absurd h' hs | simp_all
    pre := hp }

end VG.Proof.Slab.X86_64
