module

public import Asmalloc.Proof.CEntry.Pre
public import Asmalloc.Proof.Slab.X86_64.Witness
public import Asmalloc.Proof.Slab.X86_64.Word

/-!
# The C routines' precondition on x86-64, as facts

What a proof of a C routine needs of its entry state, from `World`: each
region of `Ctx.regions` is in the state's readable or writable regions as it
is marked, and is apart from the others where one is written, and from the
return address.
-/

@[expose] public section

namespace VG.Proof.CEntry.X86_64

open VG.X86_64 VG.Spec.Slab VG.Spec.CEntry VG.Proof.Slab.X86_64 VG.Proof.CEntry

/-- The tables a routine reads beyond the masks' (`Ctx.fixed` after its
first four). -/
def tables {M : ISA} (c : Spec.CEntry.Ctx M) (s : M.State) : List (Region × Bool) :=
  match c.kind with
  | .malloc => [(c.mallocTable s, false)]
  | .free => [(c.published s, false), (c.regionTable s, false)]
  | .freeSized => [(c.sizedTable s, false), (c.published s, false), (c.regionTable s, false)]
  | .calloc => [(c.callocTable s, false)]

theorem fixed_tables {M : ISA} (c : Spec.CEntry.Ctx M) (s : M.State) :
    c.fixed s = (c.rsVar s, false) :: (c.cpuField s, false) :: (c.csField s, true) ::
      (c.maskTable s, false) :: tables c s := by
  obtain ⟨A, k, sig⟩ := c
  cases k <;> rfl

theorem tables_ro {M : ISA} {c : Spec.CEntry.Ctx M} {s : M.State} {x : Region × Bool}
    (h : x ∈ tables c s) : x.2 = false := by
  obtain ⟨A, k, sig⟩ := c
  cases k <;> simp only [tables, List.mem_cons, List.not_mem_nil, or_false] at h
  · rw [h]
  · rcases h with h | h <;> rw [h]
  · rcases h with h | h | h <;> rw [h]
  · rw [h]

section

variable {c : Spec.CEntry.Ctx isa} (hA : X86 c.A) {s : State} (hw : c.World s)
include hA hw

theorem mem_rd {x : Region × Bool} (hx : x ∈ c.regions s) (h2 : x.2 = false) : x.1 ∈ s.rd := by
  rw [← hA.rd, hw.rd, Spec.CEntry.Ctx.ro]
  exact List.mem_map.mpr ⟨x, List.mem_filter.mpr ⟨hx, by simp [h2]⟩, rfl⟩

theorem mem_wr {x : Region × Bool} (hx : x ∈ c.regions s) (h2 : x.2 = true) : x.1 ∈ s.wr := by
  rw [← hA.wr, hw.wr, Spec.CEntry.Ctx.rw]
  exact List.mem_map.mpr ⟨x, List.mem_filter.mpr ⟨hx, h2⟩, rfl⟩

theorem disj_res {x : Region × Bool} (hx : x ∈ c.regions s) : x.1.Disjoint ⟨s.gpr .rsp, 8⟩ :=
  hw.res x hx _ (hA.retSlot s)

theorem nowrap_of {x : Region × Bool} (hx : x ∈ c.regions s) : x.1.base.toNat + x.1.len ≤ 2 ^ 64 := by
  have := hw.nowrap x hx; rwa [hA.ptrBits] at this

end

theorem disj_fixed_more {c : Spec.CEntry.Ctx isa} {s : State} (hw : c.World s)
    {a b : Region × Bool} (ha : a ∈ c.fixed s) (hb : b ∈ c.more s)
    (h : (a.2 || b.2) = true) : a.1.Disjoint b.1 :=
  (List.pairwise_append.mp hw.disj).2.2 a ha b hb h

theorem mem_fixed {c : Spec.CEntry.Ctx isa} {s : State} {x : Region × Bool} (h : x ∈ c.fixed s) :
    x ∈ c.regions s :=
  List.mem_append_left _ h

theorem mem_more {c : Spec.CEntry.Ctx isa} {s : State} {x : Region × Bool} (h : x ∈ c.more s) :
    x ∈ c.regions s :=
  List.mem_append_right _ h

/-! ## The view on x86-64 -/

section View

variable {c : Spec.CEntry.Ctx isa} (hA : X86 c.A) (s : State)
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

/-- The arm's facts, from the precondition. -/
theorem armOk {c : Spec.CEntry.Ctx isa} (hA : X86 c.A) {s : State} (hw : c.World s)
    (hl : c.Layout s) : ArmOk s := by
  have hf := fixed_tables c s
  have hrs : (c.rsVar s, false) ∈ c.fixed s := by rw [hf]; simp
  have hcpu : (c.cpuField s, false) ∈ c.fixed s := by rw [hf]; simp
  have hcs : (c.csField s, true) ∈ c.fixed s := by rw [hf]; simp
  have hd := hl.disj
  rw [hf] at hd
  have e : (c.rsVar s).Disjoint (c.csField s) :=
    (List.pairwise_cons.mp hd).1 (c.csField s, true)
      (List.mem_cons_of_mem _ (List.mem_cons_self ..)) (Bool.or_true _)
  refine ⟨?_, ?_, ?_, ?_⟩
  · have := mem_rd hA hw (mem_fixed hrs) rfl; rwa [rsVar_c hA] at this
  · have := mem_rd hA hw (mem_fixed hcpu) rfl; rwa [cpuField_c hA] at this
  · have := mem_wr hA hw (mem_fixed hcs) rfl; rwa [csField_c hA] at this
  · rw [← csField_c hA, ← rsVar_c hA]; exact e.symm

/-- `rseq_cs` and a table, or the masks' table, are apart. -/
theorem cs_table {c : Spec.CEntry.Ctx isa} {s : State} (hl : c.Layout s) {x : Region × Bool}
    (hx : x ∈ (c.maskTable s, false) :: tables c s) : (c.csField s).Disjoint x.1 := by
  have hd := hl.disj
  rw [fixed_tables] at hd
  exact (List.pairwise_cons.mp (List.pairwise_cons.mp (List.pairwise_cons.mp hd).2).2).1 x hx
    (Bool.true_or _)

theorem table_mem_fixed {c : Spec.CEntry.Ctx isa} {s : State} {x : Region × Bool}
    (hx : x ∈ tables c s) : x ∈ c.fixed s := by
  rw [fixed_tables]; simp [hx]

end VG.Proof.CEntry.X86_64
