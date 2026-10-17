module

public import Asmalloc.Proof.Slab.X86_64.PopInline
public import Asmalloc.Proof.Slab.X86_64.Witness

/-!
# The inline pop's witnesses

Three concrete states satisfying the precondition, one for each way out: a
CPU number without a slab, an empty slab word, and a magazine of 4 slots holding
one pointer.
-/

@[expose] public section

namespace VG.Proof.Slab.X86_64.PopInline

open VG.X86_64 VG.Spec.Slab VG.Impl.Slab.X86_64 VG.Proof.Slab.X86_64

/-- A thread whose rseq area is at `0x10100` (glibc's offset `0x100` from an
FS base of `0x10000`), whose CPU number is `cpu`, with `SLABS` at `0x1000000` and its
class 1's word in slab 0 `word`, of magazines of 4 slots (mask 16); slot 1 of
the magazine at `0x4000` holds `0x5000`. -/
def wit (cpu word : BitVec 64) (rd wr : List Region) : State where
  gpr r := if r = .rsp then 0x7000000 else 0
  cf := none
  zf := none
  sf := none
  of := none
  mem := mkMem [(0x2000, 0x100), (0x10104, cpu), (0x1000008, word), (0x4008, 0x5000)]
  rd := rd
  wr := wr
  syms n := if n = "__rseq_offset" then 0x2000 else if n = "SLABS" then 0x1000000 else 0
  fsBase := 0x10000
  params n := if n = "coff" then 8 else if n = "mask" then 16 else 0
  csDesc := 0x9000

def roW : List Region := [⟨0x2000, 8⟩, ⟨0x10104, 4⟩]

/-- No slab: CPU 8192. -/
def w0 : State := wit 8192 0 roW [⟨0x10108, 8⟩]
/-- An empty slab word. -/
def w1 : State := wit 0 0 roW [⟨0x10108, 8⟩, ⟨0x1000008, 8⟩]
/-- A magazine with one pointer: the word names slot 2. -/
def w2 : State := wit 0 0x4010 roW [⟨0x10108, 8⟩, ⟨0x1000008, 8⟩, ⟨0x4000, 32⟩]

theorem slabs_toNat : (0x1000000 : BitVec 64).toNat = 0x1000000 := rfl

/-- Every slab word of class 1 but slab 0's is zero. -/
theorem wit_zero (cpu word : BitVec 64) (rd wr : List Region) {i : Nat} (h0 : 0 < i)
    (hi : i < SLAB_COUNT) : (wit cpu word rd wr).mem.readW (wordAddr 0x1000000 i 1) 64 = 0 := by
  simp only [SLAB_COUNT] at hi
  have ha : (wordAddr 0x1000000 i 1).toNat = 0x1000000 + i * 4096 + 8 := by
    simp only [wordAddr, SLAB_BYTES, SLAB_SHIFT]
    rw [BitVec.toNat_add, slabs_toNat, BitVec.toNat_ofNat, Nat.mod_eq_of_lt (by omega),
      Nat.mod_eq_of_lt (by omega)]
    omega
  apply mkMem_readW_above
  · intro bv hbv
    simp only [List.mem_cons, List.mem_nil_iff, or_false] at hbv
    rw [ha]
    rcases hbv with rfl | rfl | rfl | rfl <;> simp <;> omega
  · rw [ha]; omega

theorem pre_wit (s : State) (hm : ∃ cpu word rd wr, s = wit cpu word rd wr)
    (hrd : s.rd = Ctx.ro cx s) (hwr : s.wr = Ctx.rw cx s)
    (hnw : ((cx.regions s).all fun x => decide (x.1.base.toNat + x.1.len ≤ 2 ^ 64)) = true)
    (hp : Sig.pairsSep (cx.regions s) = true)
    (hres : ((cx.regions s).all fun x => (cx.A.reserved 0 s).all fun q => Region.sep x.1 q) = true)
    (hl : layoutCheck cx s = true)
    (hcoff : (s.params "coff").toNat = 8 * cx.cls s) (hcls : cx.cls s = 1)
    (hmask : cx.mask s = magMask (2 ^ 2)) (hslots : cx.slots s = 4)
    (hsyms : s.syms SLABS = 0x1000000)
    (h0 : cx.SlotOk s (s.mem.readW (wordAddr 0x1000000 0 1) 64)) :
    (popInlineContract inlineAbi).pre s := by
  refine (pre_iff cx (s := s) hv rfl rfl).mpr ⟨trivial, world_of_check hX hrd hwr hnw hp hres,
    rfl, rfl, ⟨rfl, hcoff, by rw [hcls]; decide, ⟨2, by decide, by decide, hmask⟩⟩,
    layout_of_check hX hl, fun i hi => ?_⟩
  simp only [Spec.Slab.Ctx.wordOf]
  rw [mem_of (c := cx) hX, sym_of (c := cx) hX, hcls, hsyms]
  rcases Nat.eq_zero_or_pos i with rfl | h
  · exact h0
  · obtain ⟨cpu, word, rd, wr, rfl⟩ := hm
    rw [wit_zero cpu word rd wr h hi]
    exact ⟨.inl rfl, fun hs => absurd hs.2 (by simp [slot])⟩

theorem pre_w0 : (popInlineContract inlineAbi).pre w0 :=
  pre_wit w0 ⟨_, _, _, _, rfl⟩ (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (by decide +kernel) (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (by decide +kernel) (by decide +kernel)
    ⟨by decide +kernel, fun hs => absurd hs (by decide +kernel)⟩

theorem pre_w1 : (popInlineContract inlineAbi).pre w1 :=
  pre_wit w1 ⟨_, _, _, _, rfl⟩ (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (by decide +kernel) (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (by decide +kernel) (by decide +kernel)
    ⟨by decide +kernel, fun hs => absurd hs (by decide +kernel)⟩

theorem pre_w2 : (popInlineContract inlineAbi).pre w2 :=
  pre_wit w2 ⟨_, _, _, _, rfl⟩ (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (by decide +kernel) (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (by decide +kernel) (by decide +kernel)
    ⟨by decide +kernel, fun _ => apart_of_check hX (by decide +kernel)⟩

/-! ## The three ways out, each taken -/

theorem outcomes_eq : popInline.outcomes = [(0, true), (1, true), (2, true)] := rfl

theorem visits_w0 : Visits isa popInline w0 0 true := by
  obtain ⟨k, hp⟩ := pre_of (facts_of_pre pre_w0)
  have hi : ¬ cx.InRange w0 := by decide +kernel
  exact Visits.rseq (execBlock_arm hp.facts.toArm) (end_C hp hi)

theorem visits_w1 : Visits isa popInline w1 1 true := by
  obtain ⟨k, hp⟩ := pre_of (facts_of_pre pre_w1)
  have hi : cx.InRange w1 := by decide +kernel
  have ht : tst w1 = 0 := by decide +kernel
  exact Visits.rseq (execBlock_arm hp.facts.toArm) (end_T hp hi ht)

theorem visits_w2 : Visits isa popInline w2 2 true := by
  obtain ⟨k, hp⟩ := pre_of (facts_of_pre pre_w2)
  have hi : cx.InRange w2 := by decide +kernel
  have ht : ¬ tst w2 = 0 := by decide +kernel
  exact Visits.rseq (execBlock_arm hp.facts.toArm) (end_P hp hi ht)

/-- The witnesses. -/
def witnesses : List State := [w0, w1, w2]

theorem witnessOk : (∃ w ∈ witnesses, (popInlineContract inlineAbi).pre w) ∧
    ∀ o ∈ popInline.outcomes, ∃ w ∈ witnesses, (popInlineContract inlineAbi).pre w ∧
      Visits isa popInline w o.1 o.2 := by
  refine ⟨⟨w0, by simp [witnesses], pre_w0⟩, fun o ho => ?_⟩
  rw [outcomes_eq] at ho
  simp only [List.mem_cons, List.not_mem_nil, or_false] at ho
  rcases ho with rfl | rfl | rfl
  · exact ⟨w0, by simp [witnesses], pre_w0, visits_w0⟩
  · exact ⟨w1, by simp [witnesses], pre_w1, visits_w1⟩
  · exact ⟨w2, by simp [witnesses], pre_w2, visits_w2⟩

theorem verified : Verified inlineTarget popInline (popInlineContract inlineAbi) :=
  ⟨fun _ hs => correct hs, fun _ hs => rseqOk hs, ⟨w0, pre_w0⟩, fun o ho => by
    obtain ⟨w, -, hw, hv⟩ := witnessOk.2 o ho
    exact ⟨w, hw, hv⟩⟩

end VG.Proof.Slab.X86_64.PopInline
