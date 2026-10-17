module

public import Asmalloc.Proof.Slab.X86_64.PopTable
public import Asmalloc.Proof.Slab.X86_64.PopInlineWit

/-!
# The out-of-line pop's witnesses

As the inline routines' (`PopInline.wit`), with the class, 1, in `rdi`, its
mask, 16, in `MAG_MASKS` at `0x6000`, and a pointer to push in `rsi`.
-/

@[expose] public section

namespace VG.Proof.Slab.X86_64.PopTable

open VG.X86_64 VG.Spec.Slab VG.Impl.Slab.X86_64 VG.Proof.Slab.X86_64

def witT (cpu word : BitVec 64) (rd wr : List Region) : State where
  gpr r := if r = .rsp then 0x7000000 else if r = .rdi then 1 else if r = .rsi then 0x5550 else 0
  cf := none
  zf := none
  sf := none
  of := none
  mem := mkMem [(0x2000, 0x100), (0x10104, cpu), (0x1000008, word), (0x4008, 0x5000),
    (0x6000, 16 <<< 32)]
  rd := rd
  wr := wr
  syms n := if n = "__rseq_offset" then 0x2000 else if n = "SLABS" then 0x1000000
    else if n = "MAG_MASKS" then 0x6000 else 0
  fsBase := 0x10000
  csDesc := 0x9000

def roT : List Region := [⟨0x2000, 8⟩, ⟨0x10104, 4⟩, ⟨0x6004, 4⟩]

def w0 : State := witT 8192 0 roT [⟨0x10108, 8⟩]
def w1 : State := witT 0 0 roT [⟨0x10108, 8⟩, ⟨0x1000008, 8⟩]
def w2 : State := witT 0 0x4010 roT [⟨0x10108, 8⟩, ⟨0x1000008, 8⟩, ⟨0x4000, 32⟩]

/-- Every slab word of class 1 but slab 0's is zero. -/
theorem witT_zero (cpu word : BitVec 64) (rd wr : List Region) {i : Nat} (h0 : 0 < i)
    (hi : i < SLAB_COUNT) : (witT cpu word rd wr).mem.readW (wordAddr 0x1000000 i 1) 64 = 0 := by
  simp only [SLAB_COUNT] at hi
  have ha : (wordAddr 0x1000000 i 1).toNat = 0x1000000 + i * 4096 + 8 := by
    simp only [wordAddr, SLAB_BYTES, SLAB_SHIFT]
    rw [BitVec.toNat_add, PopInline.slabs_toNat, BitVec.toNat_ofNat, Nat.mod_eq_of_lt (by omega),
      Nat.mod_eq_of_lt (by omega)]
    omega
  apply mkMem_readW_above
  · intro bv hbv
    simp only [List.mem_cons, List.mem_nil_iff, or_false] at hbv
    rw [ha]
    rcases hbv with rfl | rfl | rfl | rfl | rfl <;> simp <;> omega
  · rw [ha]; omega

theorem pre_w (s : State) (hm : ∃ cpu word rd wr, s = witT cpu word rd wr)
    (hrd : s.rd = Ctx.ro cx s) (hwr : s.wr = Ctx.rw cx s)
    (hnw : ((cx.regions s).all fun x => decide (x.1.base.toNat + x.1.len ≤ 2 ^ 64)) = true)
    (hp : Sig.pairsSep (cx.regions s) = true)
    (hres : ((cx.regions s).all fun x => Region.sep x.1 ⟨s.gpr .rsp, 8⟩) = true)
    (hcls : cx.cls s = 1) (hmask : cx.mask s = magMask (2 ^ 2)) (hslots : cx.slots s = 4)
    (hsyms : s.syms SLABS = 0x1000000) (h0 : WordOk 4 (s.mem.readW (wordAddr 0x1000000 0 1) 64)) :
    (popContract abi).pre s := by
  refine (pre_iff cx (s := s) hv rfl rfl).mpr ⟨trivial, world_of_check rfl hrd hwr hnw hp hres,
    rfl, rfl, show cx.cls s < CLASS_LIMIT by rw [hcls]; decide, ⟨2, by decide, by decide, hmask⟩,
    fun i hi => ?_⟩
  rw [hslots, mem_of (c := cx) rfl, sym_of (c := cx) rfl, hcls, hsyms]
  rcases Nat.eq_zero_or_pos i with rfl | h
  · exact h0
  · obtain ⟨cpu, word, rd, wr, rfl⟩ := hm
    rw [witT_zero cpu word rd wr h hi]
    exact .inl rfl

theorem pre_w0 : (popContract abi).pre w0 :=
  pre_w _ ⟨_, _, _, _, rfl⟩ (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (by decide +kernel) (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (by decide +kernel)

theorem pre_w1 : (popContract abi).pre w1 :=
  pre_w _ ⟨_, _, _, _, rfl⟩ (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (by decide +kernel) (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (by decide +kernel)

theorem pre_w2 : (popContract abi).pre w2 :=
  pre_w _ ⟨_, _, _, _, rfl⟩ (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (by decide +kernel) (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (by decide +kernel)

theorem outcomes_eq : popTable.outcomes = [(0, true), (1, true), (2, true)] := rfl

theorem visits_w0 : Visits isa popTable w0 0 true := by
  obtain ⟨k, hp⟩ := pre_of (facts_of_pre pre_w0)
  have hi : ¬ cx.InRange w0 := by decide +kernel
  exact Visits.rseq (execBlock_arm hp.facts.toArm) (end_C hp hi)

theorem visits_w1 : Visits isa popTable w1 1 true := by
  obtain ⟨k, hp⟩ := pre_of (facts_of_pre pre_w1)
  have hi : cx.InRange w1 := by decide +kernel
  have ht : tst w1 = 0 := by decide +kernel
  exact Visits.rseq (execBlock_arm hp.facts.toArm) (end_T hp hi ht)

theorem visits_w2 : Visits isa popTable w2 2 true := by
  obtain ⟨k, hp⟩ := pre_of (facts_of_pre pre_w2)
  have hi : cx.InRange w2 := by decide +kernel
  have ht : ¬ tst w2 = 0 := by decide +kernel
  exact Visits.rseq (execBlock_arm hp.facts.toArm) (end_P hp hi ht)

def witnesses : List State := [w0, w1, w2]

theorem witnessOk : (∃ w ∈ witnesses, (popContract abi).pre w) ∧
    ∀ o ∈ popTable.outcomes, ∃ w ∈ witnesses, (popContract abi).pre w ∧
      Visits isa popTable w o.1 o.2 := by
  refine ⟨⟨w0, by simp [witnesses], pre_w0⟩, fun o ho => ?_⟩
  rw [outcomes_eq] at ho
  simp only [List.mem_cons, List.not_mem_nil, or_false] at ho
  rcases ho with rfl | rfl | rfl
  · exact ⟨w0, by simp [witnesses], pre_w0, visits_w0⟩
  · exact ⟨w1, by simp [witnesses], pre_w1, visits_w1⟩
  · exact ⟨w2, by simp [witnesses], pre_w2, visits_w2⟩

theorem verified : Verified target popTable (popContract abi) :=
  ⟨fun _ hs => correct hs, fun _ hs => rseqOk hs, ⟨_, pre_w0⟩, fun o ho => by
    obtain ⟨w, -, hw, hv⟩ := witnessOk.2 o ho
    exact ⟨w, hw, hv⟩⟩

end VG.Proof.Slab.X86_64.PopTable
