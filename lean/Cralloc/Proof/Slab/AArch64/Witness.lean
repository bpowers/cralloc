module

public import Cralloc.Proof.Slab.AArch64.Word
public import Cralloc.Proof.Slab.Witness

/-!
# The inline pop's and push's concrete states

Three states satisfying either's precondition, in the registers a setup leaves: a CPU
number without a slab, an empty slab word, and a magazine of 4 slots holding one pointer,
which has a slot to spare.
-/

@[expose] public section

namespace VG.Proof.Slab.AArch64

open VG.AArch64 VG.Spec.Slab VG.Impl.Slab.AArch64

/-- A thread whose rseq area is at `0x10100` (the offset `0x100` from a thread pointer of
`0x10000`), whose CPU number is `cpu`, with `SLABS` at `0x1000000` and its class 1's word
in slab 0 `word`, whose address is in `x3`, of magazines of 4 slots (mask 16, in `x2`);
slot 1 of the magazine at `0x4000` holds `0x5000`. -/
def wit (cpu word : BitVec 64) (rd wr : List Region) : State where
  gpr r :=
    if r = .x10 then 0x10100 else if r = .x11 then 0x9000 else if r = .x3 then 0x1000008
    else if r = .x2 then 16 else if r = .x0 then 0x6000 else 0
  mem := mkMem [(0x2000, 0x100), (0x10104, cpu), (0x1000008, word), (0x4008, 0x5000)]
  rd := rd
  wr := wr
  syms n := if n = "__rseq_offset" then 0x2000 else if n = "SLABS" then 0x1000000 else 0
  tpidr := 0x10000
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

/-- The contract's precondition of a pop or push `c`, in one of these states, by evaluation
but for the words of the slabs other than slab 0, all zero. -/
theorem pre_wit {c : Spec.Slab.Ctx isa} (hf : c.form = .inline) {regs : List Reg} (hA : A64 c.A regs)
    {post : State → State → Prop} {exit : String → State → State → Prop}
    {abort : State → State → Prop} {trace : State → List Event → Prop}
    {vals : State → List (BitVec 64)}
    (hv : c.A.args ((c.sig.words c.A.ptrBits).map (·.bits c.A.ptrBits)) = some vals)
    (hb : ∀ x, Sig.bufs c.sig.params (vals x) = [])
    (ha : ∀ x, c.A.argArea ((c.sig.words c.A.ptrBits).map (·.bits c.A.ptrBits)) x = [])
    (s : State) (hm : ∃ cpu word rd wr, s = wit cpu word rd wr)
    (hrd : s.rd = Ctx.ro c s) (hwr : s.wr = Ctx.rw c s)
    (hnw : ((c.regions s).all fun x => decide (x.1.base.toNat + x.1.len ≤ 2 ^ 64)) = true)
    (hp : Sig.pairsSep (c.regions s) = true)
    (hres : ((c.regions s).all fun x => (c.A.reserved 0 s).all fun q => Region.sep x.1 q) = true)
    (hl : layoutCheck c s = true)
    (hoff : (c.arg s 0 - c.sym s SLABS).toNat = 8 * c.cls s) (hcls : c.cls s = 1)
    (hmask : c.mask s = magMask (2 ^ 2)) (hslots : c.slots s = 4)
    (hsyms : s.syms SLABS = 0x1000000)
    (h0 : c.SlotOk s (s.mem.readW (wordAddr 0x1000000 0 1) 64)) :
    (contract c post exit abort trace).pre s := by
  have hzero : ∀ i, 0 < i → i < SLAB_COUNT → c.wordOf s i = 0 := by
    intro i h hi
    obtain ⟨cpu, word, rd, wr, rfl⟩ := hm
    simp only [Spec.Slab.Ctx.wordOf]
    rw [mem_of hA, sym_of hA, hcls, hsyms]
    exact wit_zero cpu word rd wr h hi
  have hslot : ∀ i < SLAB_COUNT, c.SlotOk s (c.wordOf s i) := by
    intro i hi
    rcases Nat.eq_zero_or_pos i with rfl | h
    · simp only [Spec.Slab.Ctx.wordOf]
      rw [mem_of hA, sym_of hA, hcls, hsyms]
      exact h0
    · rw [hzero i h hi]; exact ⟨.inl rfl, fun hs => absurd hs.2 (by simp [slot])⟩
  have hex : c.Exclusive s := by
    have hn : ∀ k, 0 < k → k < SLAB_COUNT → ¬ IsSlot (c.slots s) (c.wordOf s k) :=
      fun k h hk hs => by rw [hzero k h hk] at hs; exact absurd hs.2 (by simp [slot])
    intro i hi j hj hij hsi hsj
    rcases Nat.eq_zero_or_pos i with rfl | h
    · exact absurd hsj (hn j (by omega) hj)
    · exact absurd hsi (hn i h hi)
  have hw : c.World s := Slab.world_of_check hA.ptrBits (by rw [hA.rd]; exact hrd)
    (by rw [hA.wr]; exact hwr) hnw hp hres
  have hlay : c.Layout s := layout_of_check hA.ptrBits hl
  have htp : c.A.tp.isSome := by rw [hA.tp]; rfl
  have hsy : c.A.sym.isSome := by rw [hA.sym]; rfl
  have hwf : c.A.wf ((c.sig.words c.A.ptrBits).map (·.bits c.A.ptrBits)) 0 s := by
    rw [hA.wf]; trivial
  refine (pre_iff c hv (hb s) (ha s)).mpr ⟨hwf, hw, htp, hsy, ?_, hlay, ?_⟩
  · obtain ⟨A, f, sig⟩ := c
    cases hf
    exact ⟨hoff, by rw [hcls]; decide, ⟨2, by decide, by decide, hmask⟩⟩
  · obtain ⟨A, f, sig⟩ := c
    cases hf
    exact ⟨hslot, hex⟩

end VG.Proof.Slab.AArch64
