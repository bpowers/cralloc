module

public import Asmalloc.Proof.CEntry.X86_64.Malloc
public import Asmalloc.Proof.CEntry.X86_64.Witness

/-!
# `c_malloc`'s witnesses

The rseq area at `0x10100`
(`__rseq_offset` at `0x2000` holding `0x100`, the FS base `0x10000`), the
slabs at `0x1000000`, class 1's mask, 16 (magazines of 4 slots), in its
entry `0x3000808`, and a magazine at `0x4000` whose slot 1 holds `0x5000`.
`C_SIZES` is at `0x8000000`, its table 2048 bytes in, and gives the size 8
(index 1) the class 1. One state per way out: a CPU number with no slab, a
size above 32768, an empty slab word, and a pop.
-/

@[expose] public section

namespace VG.Proof.CEntry.X86_64.Malloc

open VG.X86_64 VG.Spec.Slab VG.Spec.CEntry VG.Impl.Slab.X86_64 VG.Impl.CEntry.X86_64
  VG.Proof.Slab.X86_64 VG.Proof.CEntry VG.Proof.Slab

def wsOf (cpu word : BitVec 64) : List (Addr × BitVec 64) :=
  [(0x2000, 0x100), (0x10104, cpu), (0x1000008, word), (0x4008, 0x5000), (0x3000808, 16),
    (0x8000800, 0x100)]

def witM (cpu size word : BitVec 64) (rd wr : List Region) : State where
  gpr r := if r = .rsp then 0x7000000 else if r = .rdi then size else 0
  cf := none
  zf := none
  sf := none
  of := none
  mem := mkMem (wsOf cpu word)
  rd := rd
  wr := wr
  syms n := if n = "__rseq_offset" then 0x2000 else if n = "SLABS" then 0x1000000 else
    if n = "C_SIZES" then 0x8000000 else 0
  fsBase := 0x10000
  csDesc := 0x9000

def roM : List Region := [⟨0x2000, 8⟩, ⟨0x10104, 4⟩, ⟨0x3000800, 2048⟩, ⟨0x8000800, 4097⟩]

def w0 : State := witM 8192 8 0 roM [⟨0x10108, 8⟩]
def w1 : State := witM 0 40000 0 roM [⟨0x10108, 8⟩]
def w2 : State := witM 0 8 0 roM [⟨0x10108, 8⟩, ⟨0x1000008, 8⟩]
def w3 : State := witM 0 8 0x4010 roM [⟨0x10108, 8⟩, ⟨0x1000008, 8⟩, ⟨0x4000, 32⟩]

/-- The addresses of the words of the witnesses' memory. -/
def addrs : List Addr := [0x2000, 0x10104, 0x1000008, 0x4008, 0x3000808, 0x8000800]

theorem mem_addrs {cpu word : BitVec 64} {bv : Addr × BitVec 64} (h : bv ∈ wsOf cpu word) :
    bv.1 ∈ addrs := by
  have : (wsOf cpu word).map Prod.fst = addrs := rfl
  rw [← this]; exact List.mem_map_of_mem h

theorem addrs_toNat : ∀ a ∈ addrs, a.toNat = 0x2000 ∨ a.toNat = 0x10104 ∨ a.toNat = 0x1000008 ∨
    a.toNat = 0x4008 ∨ a.toNat = 0x3000808 ∨ a.toNat = 0x8000800 := by
  intro a ha
  simp only [addrs, List.mem_cons, List.not_mem_nil, or_false] at ha
  rcases ha with rfl | rfl | rfl | rfl | rfl | rfl
  · exact .inl rfl
  · exact .inr (.inl rfl)
  · exact .inr (.inr (.inl rfl))
  · exact .inr (.inr (.inr (.inl rfl)))
  · exact .inr (.inr (.inr (.inr (.inl rfl))))
  · exact .inr (.inr (.inr (.inr (.inr rfl))))

theorem syms_SLABS (cpu size word : BitVec 64) (rd wr : List Region) :
    (witM cpu size word rd wr).syms SLABS = 0x1000000 := rfl

/-- Every mask but class 1's is zero. -/
theorem masks_zero (cpu size word : BitVec 64) (rd wr : List Region) :
    ∀ k < MASK_COUNT, k ≠ 1 → cx.maskOf (witM cpu size word rd wr) k = 0 := by
  intro k hk h1
  simp only [MASK_COUNT] at hk
  have ha : ((0x1000000 : BitVec 64) + BitVec.ofNat 64 (8 * k + MASK_DISP)).toNat =
      0x1000000 + 8 * k + 33556480 := by
    rw [BitVec.toNat_add, show (0x1000000 : BitVec 64).toNat = 0x1000000 from rfl, BitVec.toNat_ofNat,
      MASK_DISP, Nat.mod_eq_of_lt (a := 8 * k + 33556480) (by omega), Nat.mod_eq_of_lt (by omega)]
    omega
  simp only [Spec.CEntry.Ctx.maskOf, mem_c hX, sym_c hX, syms_SLABS]
  rw [show (witM cpu size word rd wr).mem = mkMem (wsOf cpu word) from rfl,
    mkMem_readW_clear_le (by decide) ?_ (by rw [ha]; omega)]
  · rfl
  · intro bv hbv
    rw [ha]
    have := bv.1.isLt
    rcases addrs_toNat _ (mem_addrs hbv) with h | h | h | h | h | h <;> rw [h] <;> omega

/-- Every slab word of class 1 but slab 0's is zero. -/
theorem words_zero (cpu size word : BitVec 64) (rd wr : List Region) {i : Nat} (h0 : 0 < i)
    (hi : i < SLAB_COUNT) : cx.wordOf (witM cpu size word rd wr) 1 i = 0 := by
  simp only [SLAB_COUNT] at hi
  have ha : (wordAddr 0x1000000 i 1).toNat = 0x1000000 + i * 4096 + 8 := by
    simp only [wordAddr, SLAB_BYTES, SLAB_SHIFT]
    rw [BitVec.toNat_add, show (0x1000000 : BitVec 64).toNat = 0x1000000 from rfl, BitVec.toNat_ofNat,
      Nat.mod_eq_of_lt (by omega), Nat.mod_eq_of_lt (by omega)]
    omega
  simp only [Spec.CEntry.Ctx.wordOf, mem_c hX, sym_c hX, syms_SLABS]
  rw [show (witM cpu size word rd wr).mem = mkMem (wsOf cpu word) from rfl]
  apply mkMem_readW_clear
  · intro bv hbv
    rw [ha]
    rcases addrs_toNat _ (mem_addrs hbv) with h | h | h | h | h | h <;> rw [h] <;> omega
  · rw [ha]; omega

theorem pre_w (s : State) (hm : ∃ cpu size word rd wr, s = witM cpu size word rd wr)
    (hrd : s.rd = cx.ro s) (hwr : s.wr = cx.rw s)
    (hnw : ((cx.regions s).all fun x => decide (x.1.base.toNat + x.1.len ≤ 2 ^ 64)) = true)
    (hp : Sig.pairsSep (cx.regions s) = true)
    (hres : ((cx.regions s).all fun x => (cx.A.reserved 0 s).all fun q => Region.sep x.1 q) = true)
    (hl : layoutCheck cx s = true) (hmask : cx.maskOf s 1 = magMask (2 ^ 2))
    (h0 : cx.SlotOk s 1 (cx.wordOf s 1 0)) :
    (mallocContract abi).pre s := by
  obtain ⟨cpu, size, word, rd, wr, rfl⟩ := hm
  refine (pre_iff cx hv rfl rfl).mpr ⟨trivial, world_of_check hX hrd hwr hnw hp hres,
    rfl, rfl, layout_of_check hX hl, inv_one (masks_zero _ _ _ _ _) ⟨2, by decide, by decide, hmask⟩
      (fun i h hi => words_zero _ _ _ _ _ h hi) h0⟩

theorem pre_w0 : (mallocContract abi).pre w0 :=
  pre_w _ ⟨_, _, _, _, _, rfl⟩ (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (by decide +kernel) (by decide +kernel) (by decide +kernel)
    ⟨by decide +kernel, fun hs => absurd hs (by decide +kernel)⟩

theorem pre_w1 : (mallocContract abi).pre w1 :=
  pre_w _ ⟨_, _, _, _, _, rfl⟩ (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (by decide +kernel) (by decide +kernel) (by decide +kernel)
    ⟨by decide +kernel, fun hs => absurd hs (by decide +kernel)⟩

theorem pre_w2 : (mallocContract abi).pre w2 :=
  pre_w _ ⟨_, _, _, _, _, rfl⟩ (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (by decide +kernel) (by decide +kernel) (by decide +kernel)
    ⟨by decide +kernel, fun hs => absurd hs (by decide +kernel)⟩

theorem pre_w3 : (mallocContract abi).pre w3 :=
  pre_w _ ⟨_, _, _, _, _, rfl⟩ (by decide +kernel) (by decide +kernel) (by decide +kernel)
    (by decide +kernel) (by decide +kernel) (by decide +kernel) (by decide +kernel)
    ⟨by decide +kernel, fun _ => apart_of_check hX (by decide +kernel)⟩

theorem outcomes_eq : cMalloc.outcomes = [(0, true), (1, true), (2, true), (3, true)] := rfl

theorem visits_w0 : Visits isa cMalloc w0 0 true := by
  have hp := pre_of pre_w0
  have hi : ¬ cx.InRange w0 := by decide +kernel
  exact Visits.rseq (execBlock_arm hp.arm) (end_C hp hi)

theorem visits_w1 : Visits isa cMalloc w1 1 true := by
  have hp := pre_of pre_w1
  have hi : cx.InRange w1 := by decide +kernel
  have hn : ¬ (n w1).toNat ≤ MALLOC_MAX := by decide +kernel
  exact Visits.rseq (execBlock_arm hp.arm) (end_S hp hi hn)

theorem visits_w2 : Visits isa cMalloc w2 2 true := by
  have hp := pre_of pre_w2
  have hi : cx.InRange w2 := by decide +kernel
  have hn : (n w2).toNat ≤ MALLOC_MAX := by decide +kernel
  have hk := cls_eq hn
  have hb := backFacts hX hp.world hp.pre hk hi
  have ho := popOk hX (ready (s := w2)) hb hi
  have ht : Tail.ptst .r9 (g7 w2) = 0 := by decide +kernel
  have he := end_front hp hi hn
  rw [Tail.endPop_miss ho _ ht] at he
  exact Visits.rseq (execBlock_arm hp.arm) he

theorem visits_w3 : Visits isa cMalloc w3 3 true := by
  have hp := pre_of pre_w3
  have hi : cx.InRange w3 := by decide +kernel
  have hn : (n w3).toNat ≤ MALLOC_MAX := by decide +kernel
  have hk := cls_eq hn
  have hb := backFacts hX hp.world hp.pre hk hi
  have ho := popOk hX (ready (s := w3)) hb hi
  have ht : ¬ Tail.ptst .r9 (g7 w3) = 0 := by decide +kernel
  have he := end_front hp hi hn
  rw [Tail.endPop_hit ho _ ht] at he
  exact Visits.rseq (execBlock_arm hp.arm) he

def witnesses : List State := [w0, w1, w2, w3]

theorem witnessOk : (∃ w ∈ witnesses, (mallocContract abi).pre w) ∧
    ∀ o ∈ cMalloc.outcomes, ∃ w ∈ witnesses, (mallocContract abi).pre w ∧
      Visits isa cMalloc w o.1 o.2 := by
  refine ⟨⟨w0, by simp [witnesses], pre_w0⟩, fun o ho => ?_⟩
  rw [outcomes_eq] at ho
  simp only [List.mem_cons, List.not_mem_nil, or_false] at ho
  rcases ho with rfl | rfl | rfl | rfl
  · exact ⟨w0, by simp [witnesses], pre_w0, visits_w0⟩
  · exact ⟨w1, by simp [witnesses], pre_w1, visits_w1⟩
  · exact ⟨w2, by simp [witnesses], pre_w2, visits_w2⟩
  · exact ⟨w3, by simp [witnesses], pre_w3, visits_w3⟩

theorem verified : Verified target cMalloc (mallocContract abi) :=
  ⟨fun _ hs => correct hs, fun _ hs => rseqOk hs, ⟨_, pre_w0⟩, fun o ho => by
    obtain ⟨w, -, hw, hv⟩ := witnessOk.2 o ho
    exact ⟨w, hw, hv⟩⟩

end VG.Proof.CEntry.X86_64.Malloc
