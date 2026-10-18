module

public import Asmalloc.Proof.CEntry.X86_64.Free
public import Asmalloc.Proof.CEntry.X86_64.FreeSized
public import Asmalloc.Proof.CEntry.X86_64.Witness

/-!
# The frees' witnesses

The memory of the pop's witnesses (`Malloc.witM`), with the shift 12 and
base index `0x6000` in both mirrors, at `0x3000400` and `0x3000410`, so that the pointer `0x6000123`
is in region 0, whose byte in the region table at `0x3001800` is the class
1; and `C_SIZED_SIZES` at `0x8000000`, its table 2048 bytes in, predicting
the class 1 for the size 8. The magazine at `0x4000` has 4 slots; a word of
`0x4008` (slot 1) leaves room to push.
-/

@[expose] public section

namespace VG.Proof.CEntry.X86_64.FreeWit

open VG.X86_64 VG.Spec.Slab VG.Spec.CEntry VG.Impl.Slab.X86_64 VG.Impl.CEntry.X86_64
  VG.Proof.Slab.X86_64 VG.Proof.CEntry VG.Proof.Slab

def wsOf (cpu word : BitVec 64) : List (Addr × BitVec 64) :=
  [(0x2000, 0x100), (0x10104, cpu), (0x1000008, word), (0x4008, 0x5000), (0x3000808, 16),
    (0x3000400, 12), (0x3000408, 0x6000), (0x3000410, 12), (0x3000418, 0x6000), (0x3001800, 1),
    (0x8000808, 1)]

def witF (cpu ptr size word : BitVec 64) (rd wr : List Region) : State where
  gpr r := if r = .rsp then 0x7000000 else if r = .rdi then ptr else if r = .rsi then size else 0
  cf := none
  zf := none
  sf := none
  of := none
  mem := mkMem (wsOf cpu word)
  rd := rd
  wr := wr
  syms n := if n = "__rseq_offset" then 0x2000 else if n = "SLABS" then 0x1000000 else
    if n = "C_SIZED_SIZES" then 0x8000000 else 0
  fsBase := 0x10000
  csDesc := 0x9000

def addrs : List Addr :=
  [0x2000, 0x10104, 0x1000008, 0x4008, 0x3000808, 0x3000400, 0x3000408, 0x3000410, 0x3000418,
    0x3001800, 0x8000808]

theorem mem_addrs {cpu word : BitVec 64} {bv : Addr × BitVec 64} (h : bv ∈ wsOf cpu word) :
    bv.1 ∈ addrs := by
  have : (wsOf cpu word).map Prod.fst = addrs := rfl
  rw [← this]; exact List.mem_map_of_mem h

theorem addrs_toNat : ∀ a ∈ addrs, a.toNat = 0x2000 ∨ a.toNat = 0x10104 ∨ a.toNat = 0x1000008 ∨
    a.toNat = 0x4008 ∨ a.toNat = 0x3000808 ∨ a.toNat = 0x3000400 ∨ a.toNat = 0x3000408 ∨
    a.toNat = 0x3000410 ∨ a.toNat = 0x3000418 ∨ a.toNat = 0x3001800 ∨ a.toNat = 0x8000808 := by
  intro a ha
  simp only [addrs, List.mem_cons, List.not_mem_nil, or_false] at ha
  rcases ha with rfl | rfl | rfl | rfl | rfl | rfl | rfl | rfl | rfl | rfl | rfl
  · exact .inl rfl
  · exact .inr (.inl rfl)
  · exact .inr (.inr (.inl rfl))
  · exact .inr (.inr (.inr (.inl rfl)))
  · exact .inr (.inr (.inr (.inr (.inl rfl))))
  · exact .inr (.inr (.inr (.inr (.inr (.inl rfl)))))
  · exact .inr (.inr (.inr (.inr (.inr (.inr (.inl rfl))))))
  · exact .inr (.inr (.inr (.inr (.inr (.inr (.inr (.inl rfl)))))))
  · exact .inr (.inr (.inr (.inr (.inr (.inr (.inr (.inr (.inl rfl))))))))
  · exact .inr (.inr (.inr (.inr (.inr (.inr (.inr (.inr (.inr (.inl rfl)))))))))
  · exact .inr (.inr (.inr (.inr (.inr (.inr (.inr (.inr (.inr (.inr rfl)))))))))

section

variable {c : Spec.CEntry.Ctx isa} (hA : X86 c.A)
include hA

theorem masks_zero (cpu ptr size word : BitVec 64) (rd wr : List Region) :
    ∀ k < MASK_COUNT, k ≠ 1 → c.maskOf (witF cpu ptr size word rd wr) k = 0 := by
  intro k hk h1
  simp only [MASK_COUNT] at hk
  have ha : ((0x1000000 : BitVec 64) + BitVec.ofNat 64 (8 * k + MASK_DISP)).toNat =
      0x1000000 + 8 * k + 33556480 := by
    rw [BitVec.toNat_add, show (0x1000000 : BitVec 64).toNat = 0x1000000 from rfl, BitVec.toNat_ofNat,
      MASK_DISP, Nat.mod_eq_of_lt (a := 8 * k + 33556480) (by omega), Nat.mod_eq_of_lt (by omega)]
    omega
  simp only [Spec.CEntry.Ctx.maskOf, mem_c hA, sym_c hA]
  rw [show (witF cpu ptr size word rd wr).syms SLABS = 0x1000000 from rfl,
    show (witF cpu ptr size word rd wr).mem = mkMem (wsOf cpu word) from rfl,
    mkMem_readW_clear_le (by decide) ?_ (by rw [ha]; omega)]
  · rfl
  · intro bv hbv
    rw [ha]
    have := bv.1.isLt
    rcases addrs_toNat _ (mem_addrs hbv) with h | h | h | h | h | h | h | h | h | h | h <;> rw [h] <;> omega

theorem words_zero (cpu ptr size word : BitVec 64) (rd wr : List Region) {i : Nat} (h0 : 0 < i)
    (hi : i < SLAB_COUNT) : c.wordOf (witF cpu ptr size word rd wr) 1 i = 0 := by
  simp only [SLAB_COUNT] at hi
  have ha : (wordAddr 0x1000000 i 1).toNat = 0x1000000 + i * 4096 + 8 := by
    simp only [wordAddr, SLAB_BYTES, SLAB_SHIFT]
    rw [BitVec.toNat_add, show (0x1000000 : BitVec 64).toNat = 0x1000000 from rfl, BitVec.toNat_ofNat,
      Nat.mod_eq_of_lt (by omega), Nat.mod_eq_of_lt (by omega)]
    omega
  simp only [Spec.CEntry.Ctx.wordOf, mem_c hA, sym_c hA]
  rw [show (witF cpu ptr size word rd wr).syms SLABS = 0x1000000 from rfl,
    show (witF cpu ptr size word rd wr).mem = mkMem (wsOf cpu word) from rfl]
  apply mkMem_readW_clear
  · intro bv hbv
    rw [ha]
    rcases addrs_toNat _ (mem_addrs hbv) with h | h | h | h | h | h | h | h | h | h | h <;> rw [h] <;> omega
  · rw [ha]; omega

end

/-- The mirror's words in the witnesses' memory. -/
def pubAt (m : Mirror) : Region := ⟨0x3000400 + BitVec.ofNat 64 (16 * m.index), 16⟩

/-! ## `c_free` -/

namespace F

open Free

def roF (m : Mirror) : List Region := [⟨0x2000, 8⟩, ⟨0x10104, 4⟩, ⟨0x3000800, 2048⟩, pubAt m, ⟨0x3001800, 4096⟩]

def w0 (m : Mirror) : State := witF 8192 0x6000123 0 0 (roF m) [⟨0x10108, 8⟩]
def w1 (m : Mirror) : State := witF 0 0 0 0 (roF m) [⟨0x10108, 8⟩]
def w2 (m : Mirror) : State := witF 0 0x7fff00000000 0 0 (roF m) [⟨0x10108, 8⟩]
def w3 (m : Mirror) : State := witF 0 0x6000123 0 0 (roF m) [⟨0x10108, 8⟩, ⟨0x1000008, 8⟩]
def w4 (m : Mirror) : State := witF 0 0x6000123 0 0x4008 (roF m) [⟨0x10108, 8⟩, ⟨0x1000008, 8⟩, ⟨0x4000, 32⟩]

theorem pre_w (m : Mirror) (s : State) (hm : ∃ cpu ptr size word rd wr, s = witF cpu ptr size word rd wr)
    (hrd : s.rd = (cx m).ro s) (hwr : s.wr = (cx m).rw s)
    (hnw : (((cx m).regions s).all fun x => decide (x.1.base.toNat + x.1.len ≤ 2 ^ 64)) = true)
    (hp : Sig.pairsSep ((cx m).regions s) = true)
    (hres : (((cx m).regions s).all fun x => ((cx m).A.reserved 0 s).all fun q => Region.sep x.1 q) = true)
    (hl : layoutCheck (cx m) s = true) (hmask : (cx m).maskOf s 1 = magMask (2 ^ 2))
    (h0 : (cx m).SlotOk s 1 ((cx m).wordOf s 1 0)) :
    (freeContract abi m).pre s := by
  obtain ⟨cpu, ptr, size, word, rd, wr, rfl⟩ := hm
  refine (pre_iff (cx m) hv rfl rfl).mpr ⟨trivial, world_of_check hX hrd hwr hnw hp hres,
    rfl, rfl, layout_of_check hX hl, inv_one (masks_zero hX _ _ _ _ _ _) ⟨2, by decide, by decide, hmask⟩
      (fun i h hi => words_zero hX _ _ _ _ _ _ h hi) h0⟩

theorem pre_w0 (m : Mirror) : (freeContract abi m).pre (w0 m) :=
  pre_w m _ ⟨_, _, _, _, _, _, rfl⟩ (by cases m <;> decide +kernel) (by cases m <;> decide +kernel) (by cases m <;> decide +kernel)
    (by cases m <;> decide +kernel) (by cases m <;> decide +kernel) (by cases m <;> decide +kernel) (by cases m <;> decide +kernel)
    ⟨by cases m <;> decide +kernel, fun hs => absurd hs (by cases m <;> decide +kernel)⟩

theorem pre_w1 (m : Mirror) : (freeContract abi m).pre (w1 m) :=
  pre_w m _ ⟨_, _, _, _, _, _, rfl⟩ (by cases m <;> decide +kernel) (by cases m <;> decide +kernel) (by cases m <;> decide +kernel)
    (by cases m <;> decide +kernel) (by cases m <;> decide +kernel) (by cases m <;> decide +kernel) (by cases m <;> decide +kernel)
    ⟨by cases m <;> decide +kernel, fun hs => absurd hs (by cases m <;> decide +kernel)⟩

theorem pre_w2 (m : Mirror) : (freeContract abi m).pre (w2 m) :=
  pre_w m _ ⟨_, _, _, _, _, _, rfl⟩ (by cases m <;> decide +kernel) (by cases m <;> decide +kernel) (by cases m <;> decide +kernel)
    (by cases m <;> decide +kernel) (by cases m <;> decide +kernel) (by cases m <;> decide +kernel) (by cases m <;> decide +kernel)
    ⟨by cases m <;> decide +kernel, fun hs => absurd hs (by cases m <;> decide +kernel)⟩

theorem pre_w3 (m : Mirror) : (freeContract abi m).pre (w3 m) :=
  pre_w m _ ⟨_, _, _, _, _, _, rfl⟩ (by cases m <;> decide +kernel) (by cases m <;> decide +kernel) (by cases m <;> decide +kernel)
    (by cases m <;> decide +kernel) (by cases m <;> decide +kernel) (by cases m <;> decide +kernel) (by cases m <;> decide +kernel)
    ⟨by cases m <;> decide +kernel, fun hs => absurd hs (by cases m <;> decide +kernel)⟩

theorem pre_w4 (m : Mirror) : (freeContract abi m).pre (w4 m) :=
  pre_w m _ ⟨_, _, _, _, _, _, rfl⟩ (by cases m <;> decide +kernel) (by cases m <;> decide +kernel) (by cases m <;> decide +kernel)
    (by cases m <;> decide +kernel) (by cases m <;> decide +kernel) (by cases m <;> decide +kernel) (by cases m <;> decide +kernel)
    ⟨by cases m <;> decide +kernel, fun _ => apart_of_check hX (by cases m <;> decide +kernel)⟩

theorem outcomes_eq (m : Mirror) : (cFree m).outcomes = [(0, true), (1, true), (2, true), (3, true), (4, true)] := rfl

theorem visits_w0 (m : Mirror) : Visits isa (cFree m) (w0 m) 0 true := by
  have hp := pre_of (pre_w0 m)
  exact Visits.rseq (execBlock_arm hp.arm) (end_C hp (by cases m <;> decide +kernel))

theorem visits_w1 (m : Mirror) : Visits isa (cFree m) (w1 m) 1 true := by
  have hp := pre_of (pre_w1 m)
  exact Visits.rseq (execBlock_arm hp.arm) (end_N hp (by cases m <;> decide +kernel) (by cases m <;> decide +kernel))

theorem visits_w2 (m : Mirror) : Visits isa (cFree m) (w2 m) 2 true := by
  have hp := pre_of (pre_w2 m)
  exact Visits.rseq (execBlock_arm hp.arm)
    (end_S hp (by cases m <;> decide +kernel) (by cases m <;> decide +kernel) (by cases m <;> decide +kernel))

theorem visits_w3 (m : Mirror) : Visits isa (cFree m) (w3 m) 3 true := by
  have hp := pre_of (pre_w3 m)
  have hi : (cx m).InRange (w3 m) := by cases m <;> decide +kernel
  have h0 : ¬ (ptr (w3 m)).toNat = 0 := by cases m <;> decide +kernel
  have hix : ((cx m).regionIndex (w3 m)).toNat < REGION_COUNT := by cases m <;> decide +kernel
  have hb := backFacts hX hp.world hp.pre (cls_eq hp h0 hix) hi
  have ho := pushOk hX (ready (m := m) (s := (w3 m))) hb hi
  have ht : Tail.qtst .r9 (h5 m (w3 m)) = 0 := by cases m <;> decide +kernel
  have he := end_front hp hi h0 hix
  rw [Tail.endPush_miss ho _ ht] at he
  exact Visits.rseq (execBlock_arm hp.arm) he

theorem visits_w4 (m : Mirror) : Visits isa (cFree m) (w4 m) 4 true := by
  have hp := pre_of (pre_w4 m)
  have hi : (cx m).InRange (w4 m) := by cases m <;> decide +kernel
  have h0 : ¬ (ptr (w4 m)).toNat = 0 := by cases m <;> decide +kernel
  have hix : ((cx m).regionIndex (w4 m)).toNat < REGION_COUNT := by cases m <;> decide +kernel
  have hb := backFacts hX hp.world hp.pre (cls_eq hp h0 hix) hi
  have ho := pushOk hX (ready (m := m) (s := (w4 m))) hb hi
  have ht : ¬ Tail.qtst .r9 (h5 m (w4 m)) = 0 := by cases m <;> decide +kernel
  have he := end_front hp hi h0 hix
  rw [Tail.endPush_hit ho _ ht] at he
  exact Visits.rseq (execBlock_arm hp.arm) he

def witnesses (m : Mirror) : List State := [(w0 m), (w1 m), (w2 m), (w3 m), (w4 m)]

theorem witnessOk (m : Mirror) : (∃ w ∈ (witnesses m), (freeContract abi m).pre w) ∧
    ∀ o ∈ (cFree m).outcomes, ∃ w ∈ (witnesses m), (freeContract abi m).pre w ∧ Visits isa (cFree m) w o.1 o.2 := by
  refine ⟨⟨(w0 m), by simp [witnesses], (pre_w0 m)⟩, fun o ho => ?_⟩
  rw [outcomes_eq] at ho
  simp only [List.mem_cons, List.not_mem_nil, or_false] at ho
  rcases ho with rfl | rfl | rfl | rfl | rfl
  · exact ⟨(w0 m), by simp [witnesses], (pre_w0 m), (visits_w0 m)⟩
  · exact ⟨(w1 m), by simp [witnesses], (pre_w1 m), (visits_w1 m)⟩
  · exact ⟨(w2 m), by simp [witnesses], (pre_w2 m), (visits_w2 m)⟩
  · exact ⟨(w3 m), by simp [witnesses], (pre_w3 m), (visits_w3 m)⟩
  · exact ⟨(w4 m), by simp [witnesses], (pre_w4 m), (visits_w4 m)⟩

theorem verified (m : Mirror) : Verified target (cFree m) (freeContract abi m) :=
  ⟨fun _ hs => correct hs, fun _ hs => rseqOk hs, ⟨_, (pre_w0 m)⟩, fun o ho => by
    obtain ⟨w, -, hw, hv⟩ := (witnessOk m).2 o ho
    exact ⟨w, hw, hv⟩⟩

end F

/-! ## `c_free_sized` -/

namespace S

open FreeSized

def roS (m : Mirror) : List Region := [⟨0x2000, 8⟩, ⟨0x10104, 4⟩, ⟨0x3000800, 2048⟩, ⟨0x8000800, 65536⟩,
  pubAt m, ⟨0x3001800, 4096⟩]

def w0 (m : Mirror) : State := witF 8192 0x6000123 8 0 (roS m) [⟨0x10108, 8⟩]
def w1 (m : Mirror) : State := witF 0 0x7fff00000000 8 0 (roS m) [⟨0x10108, 8⟩]
def w2 (m : Mirror) : State := witF 0 0x6000123 16 0 (roS m) [⟨0x10108, 8⟩]
def w3 (m : Mirror) : State := witF 0 0x6000123 8 0 (roS m) [⟨0x10108, 8⟩, ⟨0x1000008, 8⟩]
def w4 (m : Mirror) : State := witF 0 0x6000123 8 0x4008 (roS m) [⟨0x10108, 8⟩, ⟨0x1000008, 8⟩, ⟨0x4000, 32⟩]

theorem pre_w (m : Mirror) (s : State) (hm : ∃ cpu ptr size word rd wr, s = witF cpu ptr size word rd wr)
    (hrd : s.rd = (cx m).ro s) (hwr : s.wr = (cx m).rw s)
    (hnw : (((cx m).regions s).all fun x => decide (x.1.base.toNat + x.1.len ≤ 2 ^ 64)) = true)
    (hp : Sig.pairsSep ((cx m).regions s) = true)
    (hres : (((cx m).regions s).all fun x => ((cx m).A.reserved 0 s).all fun q => Region.sep x.1 q) = true)
    (hl : layoutCheck (cx m) s = true) (hmask : (cx m).maskOf s 1 = magMask (2 ^ 2))
    (h0 : (cx m).SlotOk s 1 ((cx m).wordOf s 1 0)) :
    (freeSizedContract abi m).pre s := by
  obtain ⟨cpu, ptr, size, word, rd, wr, rfl⟩ := hm
  refine (pre_iff (cx m) hv rfl rfl).mpr ⟨trivial, world_of_check hX hrd hwr hnw hp hres,
    rfl, rfl, layout_of_check hX hl, inv_one (masks_zero hX _ _ _ _ _ _) ⟨2, by decide, by decide, hmask⟩
      (fun i h hi => words_zero hX _ _ _ _ _ _ h hi) h0⟩

theorem pre_w0 (m : Mirror) : (freeSizedContract abi m).pre (w0 m) :=
  pre_w m _ ⟨_, _, _, _, _, _, rfl⟩ (by cases m <;> decide +kernel) (by cases m <;> decide +kernel) (by cases m <;> decide +kernel)
    (by cases m <;> decide +kernel) (by cases m <;> decide +kernel) (by cases m <;> decide +kernel) (by cases m <;> decide +kernel)
    ⟨by cases m <;> decide +kernel, fun hs => absurd hs (by cases m <;> decide +kernel)⟩

theorem pre_w1 (m : Mirror) : (freeSizedContract abi m).pre (w1 m) :=
  pre_w m _ ⟨_, _, _, _, _, _, rfl⟩ (by cases m <;> decide +kernel) (by cases m <;> decide +kernel) (by cases m <;> decide +kernel)
    (by cases m <;> decide +kernel) (by cases m <;> decide +kernel) (by cases m <;> decide +kernel) (by cases m <;> decide +kernel)
    ⟨by cases m <;> decide +kernel, fun hs => absurd hs (by cases m <;> decide +kernel)⟩

theorem pre_w2 (m : Mirror) : (freeSizedContract abi m).pre (w2 m) :=
  pre_w m _ ⟨_, _, _, _, _, _, rfl⟩ (by cases m <;> decide +kernel) (by cases m <;> decide +kernel) (by cases m <;> decide +kernel)
    (by cases m <;> decide +kernel) (by cases m <;> decide +kernel) (by cases m <;> decide +kernel) (by cases m <;> decide +kernel)
    ⟨by cases m <;> decide +kernel, fun hs => absurd hs (by cases m <;> decide +kernel)⟩

theorem pre_w3 (m : Mirror) : (freeSizedContract abi m).pre (w3 m) :=
  pre_w m _ ⟨_, _, _, _, _, _, rfl⟩ (by cases m <;> decide +kernel) (by cases m <;> decide +kernel) (by cases m <;> decide +kernel)
    (by cases m <;> decide +kernel) (by cases m <;> decide +kernel) (by cases m <;> decide +kernel) (by cases m <;> decide +kernel)
    ⟨by cases m <;> decide +kernel, fun hs => absurd hs (by cases m <;> decide +kernel)⟩

theorem pre_w4 (m : Mirror) : (freeSizedContract abi m).pre (w4 m) :=
  pre_w m _ ⟨_, _, _, _, _, _, rfl⟩ (by cases m <;> decide +kernel) (by cases m <;> decide +kernel) (by cases m <;> decide +kernel)
    (by cases m <;> decide +kernel) (by cases m <;> decide +kernel) (by cases m <;> decide +kernel) (by cases m <;> decide +kernel)
    ⟨by cases m <;> decide +kernel, fun _ => apart_of_check hX (by cases m <;> decide +kernel)⟩

theorem outcomes_eq (m : Mirror) : (cFreeSized m).outcomes = [(0, true), (1, true), (2, true), (3, true), (4, true)] := rfl

theorem visits_w0 (m : Mirror) : Visits isa (cFreeSized m) (w0 m) 0 true := by
  have hp := pre_of (pre_w0 m)
  exact Visits.rseq (execBlock_arm hp.arm) (end_C hp (by cases m <;> decide +kernel))

theorem visits_w1 (m : Mirror) : Visits isa (cFreeSized m) (w1 m) 1 true := by
  have hp := pre_of (pre_w1 m)
  exact Visits.rseq (execBlock_arm hp.arm) (end_S hp (by cases m <;> decide +kernel) (by cases m <;> decide +kernel))

theorem visits_w2 (m : Mirror) : Visits isa (cFreeSized m) (w2 m) 2 true := by
  have hp := pre_of (pre_w2 m)
  exact Visits.rseq (execBlock_arm hp.arm)
    (end_M hp (by cases m <;> decide +kernel) (by cases m <;> decide +kernel) (by cases m <;> decide +kernel))

theorem visits_w3 (m : Mirror) : Visits isa (cFreeSized m) (w3 m) 3 true := by
  have hp := pre_of (pre_w3 m)
  have hi : (cx m).InRange (w3 m) := by cases m <;> decide +kernel
  have hix : ((cx m).regionIndex (w3 m)).toNat < REGION_COUNT := by cases m <;> decide +kernel
  have hm : pb (w3 m) = rb m (w3 m) := by cases m <;> decide +kernel
  have hb := backFacts hX hp.world hp.pre (cls_eq hp hix hm) hi
  have ho := pushOk hX (ready (m := m) (s := (w3 m))) hb hi
  have ht : Tail.qtst .r9 (k7 m (w3 m)) = 0 := by cases m <;> decide +kernel
  have he := end_front hp hi hix hm
  rw [Tail.endPush_miss ho _ ht] at he
  exact Visits.rseq (execBlock_arm hp.arm) he

theorem visits_w4 (m : Mirror) : Visits isa (cFreeSized m) (w4 m) 4 true := by
  have hp := pre_of (pre_w4 m)
  have hi : (cx m).InRange (w4 m) := by cases m <;> decide +kernel
  have hix : ((cx m).regionIndex (w4 m)).toNat < REGION_COUNT := by cases m <;> decide +kernel
  have hm : pb (w4 m) = rb m (w4 m) := by cases m <;> decide +kernel
  have hb := backFacts hX hp.world hp.pre (cls_eq hp hix hm) hi
  have ho := pushOk hX (ready (m := m) (s := (w4 m))) hb hi
  have ht : ¬ Tail.qtst .r9 (k7 m (w4 m)) = 0 := by cases m <;> decide +kernel
  have he := end_front hp hi hix hm
  rw [Tail.endPush_hit ho _ ht] at he
  exact Visits.rseq (execBlock_arm hp.arm) he

def witnesses (m : Mirror) : List State := [(w0 m), (w1 m), (w2 m), (w3 m), (w4 m)]

theorem witnessOk (m : Mirror) : (∃ w ∈ (witnesses m), (freeSizedContract abi m).pre w) ∧
    ∀ o ∈ (cFreeSized m).outcomes, ∃ w ∈ (witnesses m), (freeSizedContract abi m).pre w ∧
      Visits isa (cFreeSized m) w o.1 o.2 := by
  refine ⟨⟨(w0 m), by simp [witnesses], (pre_w0 m)⟩, fun o ho => ?_⟩
  rw [outcomes_eq] at ho
  simp only [List.mem_cons, List.not_mem_nil, or_false] at ho
  rcases ho with rfl | rfl | rfl | rfl | rfl
  · exact ⟨(w0 m), by simp [witnesses], (pre_w0 m), (visits_w0 m)⟩
  · exact ⟨(w1 m), by simp [witnesses], (pre_w1 m), (visits_w1 m)⟩
  · exact ⟨(w2 m), by simp [witnesses], (pre_w2 m), (visits_w2 m)⟩
  · exact ⟨(w3 m), by simp [witnesses], (pre_w3 m), (visits_w3 m)⟩
  · exact ⟨(w4 m), by simp [witnesses], (pre_w4 m), (visits_w4 m)⟩

theorem verified (m : Mirror) : Verified target (cFreeSized m) (freeSizedContract abi m) :=
  ⟨fun _ hs => correct hs, fun _ hs => rseqOk hs, ⟨_, (pre_w0 m)⟩, fun o ho => by
    obtain ⟨w, -, hw, hv⟩ := (witnessOk m).2 o ho
    exact ⟨w, hw, hv⟩⟩

end S

end VG.Proof.CEntry.X86_64.FreeWit
