module

public import Cralloc.Spec.CEntry
public import Cralloc.Proof.Slab.Pre

/-!
# The C routines' precondition, unfolded, and a retry's

As `Proof/Slab/Pre.lean` for the slab routines: the precondition of a
contract built by `Spec.CEntry.contract` is the convention's
well-formedness, `World` and `Ctx.pre`; the regions are apart because of the
layout and the invariant (`world_of`); and a retry's precondition follows
from the entry state's, the invariant being kept by writes apart from the
slabs and the masks' table (`inv_frame`, `retry_pre`).

Unlike a slab routine's, a C routine's class is read from memory, so it may
be another on a retry; nothing here depends on it staying the same, since
the invariant is of every class.
-/

@[expose] public section

namespace VG.Proof.CEntry

open VG.Spec.Slab VG.Spec.CEntry VG.Proof.Slab

section Contract

variable {M : ISA} (c : Spec.CEntry.Ctx M) {post : M.State → M.State → Prop}
    {exit : String → M.State → M.State → Prop} {abort : M.State → M.State → Prop}
    {trace : M.State → List Event → Prop} {vals : M.State → List (BitVec 64)}

theorem pre_iff {s : M.State}
    (hv : c.A.args ((c.sig.words c.A.ptrBits).map (·.bits c.A.ptrBits)) = some vals)
    (hb : Sig.bufs c.sig.params (vals s) = [])
    (ha : c.A.argArea ((c.sig.words c.A.ptrBits).map (·.bits c.A.ptrBits)) s = []) :
    (contract c post exit abort trace).pre s ↔
      c.A.wf ((c.sig.words c.A.ptrBits).map (·.bits c.A.ptrBits)) 0 s ∧ c.World s ∧ c.pre s := by
  simp only [Spec.CEntry.contract, Sig.contract, Abi.withExtra, hv, hb, ha, List.map_nil, List.append_nil,
    List.filter_nil, List.Pairwise.nil, List.not_mem_nil, false_implies, implies_true, true_and,
    Curry.apply_const]
  constructor
  · rintro ⟨⟨hwf, hd1, hd2, hnw, hp, hres, -, -⟩, hrd, hwr, hpre⟩
    exact ⟨hwf, ⟨take_drop_eq hrd hd1, take_drop_eq hwr hd2, hnw, hp, hres⟩, hpre⟩
  · rintro ⟨hwf, ⟨hrd, hwr, hnw, hp, hres⟩, hpre⟩
    simp only [Spec.CEntry.Ctx.ro, Spec.CEntry.Ctx.rw] at hrd hwr
    simp only [hrd, hwr, Nat.sub_self, List.take_zero, List.drop_zero, List.not_mem_nil,
      false_implies, implies_true, and_true, true_and]
    exact ⟨⟨hwf, hnw, hp, hres⟩, hpre⟩

variable (hv : c.A.args ((c.sig.words c.A.ptrBits).map (·.bits c.A.ptrBits)) = some vals)
include hv

theorem post_iff (s s' : M.State) : (contract c post exit abort trace).post s s' ↔ post s s' := by
  simp only [Spec.CEntry.contract, Sig.contract, Abi.withExtra, hv, Curry.apply_const, true_and]

theorem exit_iff (t : String) (s s' : M.State) :
    (contract c post exit abort trace).exit t s s' ↔ vals s' = vals s ∧ exit t s s' := by
  simp only [Spec.CEntry.contract, Sig.contract, Abi.withExtra, hv]

theorem abort_iff (s p : M.State) :
    (contract c post exit abort trace).abort s p ↔ vals p = vals s ∧ (abort s p ∧ c.inv p) := by
  simp only [Spec.CEntry.contract, Sig.contract, Abi.withExtra, hv]

end Contract

theorem trace_eq {M : ISA} (c : Spec.CEntry.Ctx M) {post : M.State → M.State → Prop}
    {exit : String → M.State → M.State → Prop} {abort : M.State → M.State → Prop}
    {trace : M.State → List Event → Prop} (s : M.State) (evs : List Event) :
    (contract c post exit abort trace).trace s evs = trace s (c.slab.attemptEvents s evs) := rfl

/-! ## The regions, from the layout and the invariant -/

section World

variable {M : ISA} {c : Spec.CEntry.Ctx M} {s : M.State}

/-- The tables a routine reads beyond the masks' (`Ctx.fixed` after its
first four). -/
def tables (c : Spec.CEntry.Ctx M) (s : M.State) : List (Region × Bool) :=
  match c.kind with
  | .malloc => [(c.mallocTable s, false)]
  | .free => [(c.published s, false), (c.regionTable s, false)]
  | .freeSized => [(c.sizedTable s, false), (c.published s, false), (c.regionTable s, false)]
  | .calloc => [(c.callocTable s, false)]

theorem fixed_tables (c : Spec.CEntry.Ctx M) (s : M.State) :
    c.fixed s = (c.rsVar s, false) :: (c.cpuField s, false) :: (c.csField s, true) ::
      (c.maskTable s, false) :: tables c s := by
  obtain ⟨A, k, sig⟩ := c
  cases k <;> rfl

theorem tables_ro {x : Region × Bool} (h : x ∈ tables c s) : x.2 = false := by
  obtain ⟨A, k, sig⟩ := c
  cases k <;> simp only [tables, List.mem_cons, List.not_mem_nil, or_false] at h
  · rw [h]
  · rcases h with h | h <;> rw [h]
  · rcases h with h | h | h <;> rw [h]
  · rw [h]

theorem disj_fixed_more (hw : c.World s) {a b : Region × Bool} (ha : a ∈ c.fixed s)
    (hb : b ∈ c.more s) (h : (a.2 || b.2) = true) : a.1.Disjoint b.1 :=
  (List.pairwise_append.mp hw.disj).2.2 a ha b hb h

theorem mem_fixed {x : Region × Bool} (h : x ∈ c.fixed s) : x ∈ c.regions s :=
  List.mem_append_left _ h

theorem mem_more {x : Region × Bool} (h : x ∈ c.more s) : x ∈ c.regions s :=
  List.mem_append_right _ h

/-- `rseq_cs` and a table, or the masks' table, are apart. -/
theorem cs_table (hl : c.Layout s) {x : Region × Bool}
    (hx : x ∈ (c.maskTable s, false) :: tables c s) : (c.csField s).Disjoint x.1 := by
  have hd := hl.disj
  rw [fixed_tables] at hd
  exact (List.pairwise_cons.mp (List.pairwise_cons.mp (List.pairwise_cons.mp hd).2).2).1 x hx
    (Bool.true_or _)

theorem table_mem_fixed {x : Region × Bool} (hx : x ∈ tables c s) : x ∈ c.fixed s := by
  rw [fixed_tables]; simp [hx]

theorem byteAt_lt (a : Addr) : c.byteAt s a < MASK_COUNT := by
  simp only [Spec.CEntry.Ctx.byteAt, MASK_COUNT]; exact ((c.mem s) a).isLt

/-- The class a routine finds is a byte. -/
theorem cls_lt {k : Nat} (h : c.cls? s = some k) : k < MASK_COUNT := by
  simp only [Spec.CEntry.Ctx.cls?] at h
  split at h <;> (try split at h) <;> simp only [Option.some.injEq, reduceCtorEq] at h <;>
    subst h <;> exact byteAt_lt _

/-- A class byte's word in slab `i` is in the slabs. -/
theorem wordAt_sub {k i : Nat} (hk : k < MASK_COUNT) (hi : i < SLAB_COUNT) :
    Region.Sub ⟨wordAddr (c.sym s SLABS) i k, 8⟩ (c.slabs s) := by
  simp only [Spec.CEntry.Ctx.slabs, wordAddr]
  apply Offset.sub_base
  simp only [MASK_COUNT, SLAB_BYTES, SLAB_SHIFT, SLAB_COUNT] at *
  omega

/-- A class byte's mask entry is in the masks' table. -/
theorem maskAt_sub {k : Nat} (hk : k < MASK_COUNT) :
    Region.Sub ⟨c.sym s SLABS + BitVec.ofNat 64 (8 * k + MASK_DISP), 4⟩ (c.maskTable s) := by
  simp only [Spec.CEntry.Ctx.maskTable]
  rw [show 8 * k + MASK_DISP = MASK_DISP + 8 * k by omega, BitVec.ofNat_add, ← BitVec.add_assoc]
  apply Offset.sub_base
  simp only [MASK_COUNT] at *
  omega

/-- A class byte's whole entry is in the masks' table. -/
theorem maskAt_contains {k : Nat} (hk : k < MASK_COUNT) :
    (c.maskTable s).Contains (c.sym s SLABS + BitVec.ofNat 64 (8 * k + MASK_DISP)) 8 := by
  simp only [Spec.CEntry.Ctx.maskTable]
  rw [show 8 * k + MASK_DISP = MASK_DISP + 8 * k by omega, BitVec.ofNat_add, ← BitVec.add_assoc]
  apply Offset.contains_base
  · simp only [MASK_COUNT] at hk ⊢; omega
  · simp only [MASK_COUNT] at hk; omega

theorem nowrap_word (hpb : c.A.ptrBits = 64) (hl : c.Layout s) {k i : Nat} (hk : k < MASK_COUNT)
    (hi : i < SLAB_COUNT) : (wordAddr (c.sym s SLABS) i k).toNat + 8 ≤ 2 ^ 64 := by
  have h := hl.slabs_nowrap
  simp only [Spec.CEntry.Ctx.slabs, hpb] at h
  simp only [wordAddr, BitVec.toNat_add, BitVec.toNat_ofNat]
  simp only [MASK_COUNT, SLAB_BYTES, SLAB_SHIFT, SLAB_COUNT] at *
  rw [Nat.mod_eq_of_lt (a := i * 2 ^ 12 + 8 * k) (by omega), Nat.mod_eq_of_lt (by omega)]
  omega

theorem mask_ne_zero_of_isSlot {k : Nat} {w : BitVec 64} (h : IsSlot (c.slotsOf' s k) w) :
    c.maskOf s k ≠ 0 := by
  intro hz
  simp only [Spec.CEntry.Ctx.slotsOf', hz] at h
  exact not_isSlot_slotsOf_zero w h

/-- The magazine `more` names lies apart, by the invariant. -/
theorem mag_apart (hinv : c.inv s) {k : Nat} (hk : k < MASK_COUNT) (hi : c.InRange s)
    (hs : IsSlot (c.slotsOf' s k) (c.w s k)) : c.Apart s (c.magRegion s k) :=
  ((hinv k hk).2.1 (mask_ne_zero_of_isSlot hs) _ hi).2 hs

theorem more_cases : c.more s = [] ∨ (∃ k, c.cls? s = some k ∧ c.InRange s ∧
    (c.more s = [(⟨c.word s k, 8⟩, true)] ∨
      (IsSlot (c.slotsOf' s k) (c.w s k) ∧ c.more s = [(⟨c.word s k, 8⟩, true), (c.magRegion s k, true)]))) := by
  unfold Spec.CEntry.Ctx.more
  by_cases hi : c.InRange s
  · simp only [hi, ↓reduceIte]
    cases h : c.cls? s with
    | none => exact .inl rfl
    | some k =>
      refine .inr ⟨k, rfl, trivial, ?_⟩
      by_cases hs : IsSlot (c.slotsOf' s k) (c.w s k)
      · exact .inr ⟨hs, by simp [hs]⟩
      · exact .inl (by simp [hs])
  · simp [hi]

/-- The regions are apart: from the layout and the invariant. -/
theorem world_of (hpb : c.A.ptrBits = 64) (hrd : c.A.rd s = c.ro s) (hwr : c.A.wr s = c.rw s)
    (hl : c.Layout s) (hinv : c.inv s) : c.World s := by
  have hword : ∀ k, c.cls? s = some k → c.InRange s →
      Region.Sub ⟨c.word s k, 8⟩ (c.slabs s) := fun k hk hi => wordAt_sub (cls_lt hk) hi
  refine ⟨hrd, hwr, ?_, ?_, ?_⟩
  · intro x hx
    simp only [Spec.CEntry.Ctx.regions, List.mem_append] at hx
    rcases hx with hx | hx
    · exact hl.nowrap x hx
    · rcases more_cases (c := c) (s := s) with h | ⟨k, hk, hi, h | ⟨hs, h⟩⟩ <;> rw [h] at hx
      · simp at hx
      · rw [List.mem_singleton.mp hx, hpb]; exact nowrap_word hpb hl (cls_lt hk) hi
      · rcases List.mem_cons.mp hx with rfl | hx
        · rw [hpb]; exact nowrap_word hpb hl (cls_lt hk) hi
        · rw [List.mem_singleton.mp hx]; exact (mag_apart hinv (cls_lt hk) hi hs).1
  · simp only [Spec.CEntry.Ctx.regions]
    refine List.pairwise_append.mpr ⟨hl.disj, ?_, ?_⟩
    · rcases more_cases (c := c) (s := s) with h | ⟨k, hk, hi, h | ⟨hs, h⟩⟩ <;> rw [h]
      · exact .nil
      · simp
      · refine List.pairwise_cons.mpr ⟨fun b hb _ => ?_, by simp⟩
        rw [List.mem_singleton.mp hb]
        exact ((mag_apart hinv (cls_lt hk) hi hs).2.1.sub_right (hword k hk hi)).symm
    · intro a ha b hb _
      rcases more_cases (c := c) (s := s) with h | ⟨k, hk, hi, h | ⟨hs, h⟩⟩ <;> rw [h] at hb
      · simp at hb
      · rw [List.mem_singleton.mp hb]; exact (hl.slabs a ha).sub_right (hword k hk hi)
      · rcases List.mem_cons.mp hb with rfl | hb
        · exact (hl.slabs a ha).sub_right (hword k hk hi)
        · rw [List.mem_singleton.mp hb]
          exact ((mag_apart hinv (cls_lt hk) hi hs).2.2.1 a ha).symm
  · intro x hx q hq
    simp only [Spec.CEntry.Ctx.regions, List.mem_append] at hx
    rcases hx with hx | hx
    · exact hl.res x hx q hq
    · rcases more_cases (c := c) (s := s) with h | ⟨k, hk, hi, h | ⟨hs, h⟩⟩ <;> rw [h] at hx
      · simp at hx
      · rw [List.mem_singleton.mp hx]; exact (hl.slabs_res q hq).sub_left (hword k hk hi)
      · rcases List.mem_cons.mp hx with rfl | hx
        · exact (hl.slabs_res q hq).sub_left (hword k hk hi)
        · rw [List.mem_singleton.mp hx]; exact (mag_apart hinv (cls_lt hk) hi hs).2.2.2 q hq

end World

/-! ## States with the same addresses -/

section Same

variable {M : ISA} (c : Spec.CEntry.Ctx M)

/-- `p` has the addresses of `s`: as `Proof.Slab.Same` of the slab view. -/
abbrev Same (s p : M.State) : Prop := Proof.Slab.Same c.slab s p

variable {c} {s p : M.State}

namespace Same

theorem sym' (h : Same c s p) (n : String) : c.sym p n = c.sym s n := h.sym n

theorem rsVar' (h : Same c s p) : c.rsVar p = c.rsVar s := h.rsVar_eq
theorem csField' (h : Same c s p) : c.csField p = c.csField s := h.csField
theorem cpuField' (h : Same c s p) : c.cpuField p = c.cpuField s := h.cpuField

theorem slabs' (h : Same c s p) : c.slabs p = c.slabs s := by
  simp only [Spec.CEntry.Ctx.slabs, h.sym']

theorem fixed' (h : Same c s p) : c.fixed p = c.fixed s := by
  obtain ⟨A, k, sig⟩ := c
  cases k <;> simp only [Spec.CEntry.Ctx.fixed, h.rsVar', h.csField', h.cpuField',
    Spec.CEntry.Ctx.maskTable, Spec.CEntry.Ctx.published, Spec.CEntry.Ctx.regionTable,
    Spec.CEntry.Ctx.mallocTable, Spec.CEntry.Ctx.sizedTable, Spec.CEntry.Ctx.callocTable, h.sym']

theorem res' (h : Same c s p) : c.A.reserved 0 p = c.A.reserved 0 s := h.res

theorem apart' (h : Same c s p) (r : Region) : c.Apart p r ↔ c.Apart s r := by
  simp only [Spec.CEntry.Ctx.Apart, h.slabs', h.fixed', h.res']

theorem layout' (h : Same c s p) : c.Layout p ↔ c.Layout s := by
  have h1 := h.slabs'; have h2 := h.fixed'; have h3 := h.res'
  constructor
  · intro hl
    exact ⟨by rw [← h1]; exact hl.slabs_nowrap, by rw [← h1, ← h3]; exact hl.slabs_res,
      by rw [← h2]; exact hl.nowrap, by rw [← h2]; exact hl.disj, by rw [← h2, ← h1]; exact hl.slabs,
      by rw [← h2, ← h3]; exact hl.res, by rw [← h1]; exact hl.page⟩
  · intro hl
    exact ⟨by rw [h1]; exact hl.slabs_nowrap, by rw [h1, h3]; exact hl.slabs_res,
      by rw [h2]; exact hl.nowrap, by rw [h2]; exact hl.disj, by rw [h2, h1]; exact hl.slabs,
      by rw [h2, h3]; exact hl.res, by rw [h1]; exact hl.page⟩

end Same

end Same

/-! ## The invariant, kept -/

section Inv

variable {M : ISA} {c : Spec.CEntry.Ctx M} {s p : M.State}

/-- What a region a routine writes before its commit must be apart from for
the invariant to be kept: the slabs, the masks' table and `__rseq_offset`. -/
def Harmless (c : Spec.CEntry.Ctx M) (s : M.State) (w : Region) : Prop :=
  w.Disjoint (c.slabs s) ∧ w.Disjoint (c.maskTable s) ∧ w.Disjoint (c.rsVar s)

theorem wordOf_frame (h : Same c s p) {W : List Region} (hm : Frame W (c.mem s) (c.mem p))
    (hW : ∀ w ∈ W, Harmless c s w) {k i : Nat} (hk : k < MASK_COUNT) (hi : i < SLAB_COUNT) :
    c.wordOf p k i = c.wordOf s k i := by
  simp only [Spec.CEntry.Ctx.wordOf, h.sym']
  exact hm.readW (r := ⟨wordAddr (c.sym s SLABS) i k, 8⟩) (Region.contains_self _ _)
    (fun w hw => (hW w hw).1.symm.sub_left (wordAt_sub hk hi)) (by decide)

theorem maskOf_frame (h : Same c s p) {W : List Region} (hm : Frame W (c.mem s) (c.mem p))
    (hW : ∀ w ∈ W, Harmless c s w) {k : Nat} (hk : k < MASK_COUNT) : c.maskOf p k = c.maskOf s k := by
  simp only [Spec.CEntry.Ctx.maskOf, h.sym']
  exact congrArg BitVec.toNat (hm.readW (r := ⟨c.sym s SLABS + BitVec.ofNat 64 (8 * k + MASK_DISP), 4⟩)
    (Region.contains_self _ _) (fun w hw => (hW w hw).2.1.symm.sub_left (maskAt_sub hk)) (by decide))

/-- The invariant is kept by memory changed only in harmless regions. -/
theorem inv_frame (h : Same c s p) {W : List Region} (hm : Frame W (c.mem s) (c.mem p))
    (hW : ∀ w ∈ W, Harmless c s w) (hinv : c.inv s) : c.inv p := by
  intro k hk
  have hmk := maskOf_frame h hm hW hk
  have hsl : c.slotsOf' p k = c.slotsOf' s k := by simp only [Spec.CEntry.Ctx.slotsOf', hmk]
  have hwo := fun i (hi : i < SLAB_COUNT) => wordOf_frame h hm hW hk hi
  obtain ⟨h1, h2, h3⟩ := hinv k hk
  refine ⟨by rw [hmk]; exact h1, fun hz i hi => ?_, ?_⟩
  · simp only [Spec.CEntry.Ctx.SlotOk, hwo i hi, hsl, h.apart']
    exact h2 (by rwa [hmk] at hz) i hi
  · intro i hi j hj hij
    simp only [hsl, hwo i hi, hwo j hj]
    exact h3 i hi j hj hij

end Inv

/-! ## A state with the same addresses and memory -/

section Kept

variable {M : ISA} {c : Spec.CEntry.Ctx M} {s p : M.State}

/-- A state with the addresses and the memory of `s` has its regions. -/
theorem Same.regions' (h : Same c s p) (hm : c.mem p = c.mem s) : c.regions p = c.regions s := by
  have harg : ∀ i, c.arg p i = c.arg s i := h.arg
  have hsym : ∀ n, c.sym p n = c.sym s n := h.sym
  have hsize : c.size p = c.size s := by simp only [Spec.CEntry.Ctx.size, harg]
  have hptr : c.ptr p = c.ptr s := by simp only [Spec.CEntry.Ctx.ptr, harg]
  have hbyte : ∀ a, c.byteAt p a = c.byteAt s a := fun a => by
    simp only [Spec.CEntry.Ctx.byteAt, hm]
  have hover : c.overflows p ↔ c.overflows s := by simp only [Spec.CEntry.Ctx.overflows, harg]
  have hidx : c.regionIndex p = c.regionIndex s := by
    simp only [Spec.CEntry.Ctx.regionIndex, Spec.CEntry.Ctx.shift, Spec.CEntry.Ctx.baseIndex,
      Spec.CEntry.Ctx.published, hptr, hsym, hm]
  have hcls : c.cls? p = c.cls? s := by
    simp only [Spec.CEntry.Ctx.cls?, Spec.CEntry.Ctx.mallocClass, Spec.CEntry.Ctx.predicted,
      Spec.CEntry.Ctx.callocClass, Spec.CEntry.Ctx.regionByte, Spec.CEntry.Ctx.mallocTable,
      Spec.CEntry.Ctx.sizedTable, Spec.CEntry.Ctx.callocTable, Spec.CEntry.Ctx.regionTable, hsize,
      hptr, hidx, hbyte, hsym]
    cases c.kind with
    | calloc => exact ite_congr (propext (by rw [hover])) (fun _ => rfl) (fun _ => rfl)
    | _ => rfl
  have hcpu : c.cpu p = c.cpu s := by
    show ((c.mem p).readW (c.slab.cpuField p).base 32).toNat =
      ((c.mem s).readW (c.slab.cpuField s).base 32).toNat
    rw [hm, Proof.Slab.Same.cpuField h]
  have hword : ∀ k, c.word p k = c.word s k := fun k => by simp only [Spec.CEntry.Ctx.word, hcpu, hsym]
  have hw : ∀ k, c.w p k = c.w s k := fun k => by simp only [Spec.CEntry.Ctx.w, hword, hm]
  have hsl : ∀ k, c.slotsOf' p k = c.slotsOf' s k := fun k => by
    simp only [Spec.CEntry.Ctx.slotsOf', Spec.CEntry.Ctx.maskOf, hsym, hm]
  have hmore : c.more p = c.more s := by
    unfold Spec.CEntry.Ctx.more
    refine ite_congr (by simp only [Spec.CEntry.Ctx.InRange, hcpu]) (fun _ => ?_) (fun _ => rfl)
    rw [hcls]
    cases c.cls? s <;> simp only [Spec.CEntry.Ctx.magRegion, hw, hsl, hword]
  simp only [Spec.CEntry.Ctx.regions, h.fixed', hmore]

/-- The precondition holds in a state with the addresses, the memory and the
permissions of one where it holds, if the convention's well-formedness does. -/
theorem pre_of_same {post : M.State → M.State → Prop}
    {exit : String → M.State → M.State → Prop} {abort : M.State → M.State → Prop}
    {trace : M.State → List Event → Prop} {vals : M.State → List (BitVec 64)}
    (hv : c.A.args ((c.sig.words c.A.ptrBits).map (·.bits c.A.ptrBits)) = some vals)
    (hb : ∀ x, Sig.bufs c.sig.params (vals x) = [])
    (ha : ∀ x, c.A.argArea ((c.sig.words c.A.ptrBits).map (·.bits c.A.ptrBits)) x = [])
    (hwf : c.A.wf ((c.sig.words c.A.ptrBits).map (·.bits c.A.ptrBits)) 0 p)
    (hs : (contract c post exit abort trace).pre s) (hsame : Same c s p) (hm : c.mem p = c.mem s)
    (hrd : c.A.rd p = c.A.rd s) (hwr : c.A.wr p = c.A.wr s) :
    (contract c post exit abort trace).pre p := by
  obtain ⟨-, hw, htp, hsym, hl, hinv⟩ := (pre_iff c hv (hb s) (ha s)).mp hs
  have hreg := hsame.regions' hm
  have hi : c.inv p := inv_frame hsame (W := []) (by rw [hm]; exact Frame.refl _ _) (by simp) hinv
  refine (pre_iff c hv (hb p) (ha p)).mpr ⟨hwf, ⟨?_, ?_, ?_, ?_, ?_⟩, htp, hsym, hsame.layout'.mpr hl, hi⟩
  · simp only [Spec.CEntry.Ctx.ro, hreg, hrd]; exact hw.rd
  · simp only [Spec.CEntry.Ctx.rw, hreg, hwr]; exact hw.wr
  · rw [hreg]; exact hw.nowrap
  · rw [hreg]; exact hw.disj
  · rw [hreg, hsame.res']; exact hw.res

end Kept

/-! ## A retry's precondition -/

theorem retry_pre {M : ISA} (c : Spec.CEntry.Ctx M) {post : M.State → M.State → Prop}
    {exit : String → M.State → M.State → Prop} {abort : M.State → M.State → Prop}
    {trace : M.State → List Event → Prop} {vals : M.State → List (BitVec 64)} {s p p' : M.State}
    (hv : c.A.args ((c.sig.words c.A.ptrBits).map (·.bits c.A.ptrBits)) = some vals)
    (hb : Sig.bufs c.sig.params (vals p') = [])
    (ha : c.A.argArea ((c.sig.words c.A.ptrBits).map (·.bits c.A.ptrBits)) p' = [])
    (hpb : c.A.ptrBits = 64) (hwf : c.A.wf ((c.sig.words c.A.ptrBits).map (·.bits c.A.ptrBits)) 0 p')
    (hs : c.pre s) (hsame : Same c s p') (hinv : c.inv p)
    (he : (contract c post exit abort trace).env p p') : (contract c post exit abort trace).pre p' := by
  obtain ⟨hrd, hwr, -, hinv'⟩ := he
  obtain ⟨htp, hsym, hl, -⟩ := hs
  have hl' := hsame.layout'.mpr hl
  have hi' := hinv' hinv
  exact (pre_iff c hv hb ha).mpr ⟨hwf, world_of hpb hrd hwr hl' hi', htp, hsym, hl', hi'⟩

/-! ## What the routines write before their commit -/

section Harmless

variable {M : ISA} {c : Spec.CEntry.Ctx M} {s : M.State}

theorem fixed_disj (hl : c.Layout s) {a b : Region × Bool} {l₁ l₂ : List (Region × Bool)}
    (h : c.fixed s = l₁ ++ a :: l₂) (hb : b ∈ l₂) (hw : (a.2 || b.2) = true) : a.1.Disjoint b.1 := by
  have := hl.disj
  rw [h] at this
  exact (List.pairwise_cons.mp (List.pairwise_append.mp this).2.1).1 b hb hw

theorem fixed_eq : ∃ l, c.fixed s =
    (c.rsVar s, false) :: (c.cpuField s, false) :: (c.csField s, true) :: (c.maskTable s, false) :: l := by
  obtain ⟨A, k, sig⟩ := c
  cases k <;> exact ⟨_, rfl⟩

theorem cs_mem_fixed : (c.csField s, true) ∈ c.fixed s := by
  obtain ⟨l, h⟩ := fixed_eq (c := c) (s := s); rw [h]; simp

theorem rsVar_mem_fixed : (c.rsVar s, false) ∈ c.fixed s := by
  obtain ⟨l, h⟩ := fixed_eq (c := c) (s := s); rw [h]; simp

theorem maskTable_mem_fixed : (c.maskTable s, false) ∈ c.fixed s := by
  obtain ⟨l, h⟩ := fixed_eq (c := c) (s := s); rw [h]; simp

/-- `rseq_cs` is harmless. -/
theorem harmless_cs (hl : c.Layout s) : Harmless c s (c.csField s) := by
  obtain ⟨l, h⟩ := fixed_eq (c := c) (s := s)
  have hd := hl.disj
  rw [h] at hd
  have h0 := List.pairwise_cons.mp hd
  have h1 := List.pairwise_cons.mp h0.2
  have h2 := List.pairwise_cons.mp h1.2
  have e1 : (c.csField s).Disjoint (c.slabs s) := hl.slabs (c.csField s, true) cs_mem_fixed
  have e2 : (c.csField s).Disjoint (c.maskTable s) :=
    h2.1 (c.maskTable s, false) (List.mem_cons_self ..) (Bool.true_or _)
  have e3 : (c.rsVar s).Disjoint (c.csField s) :=
    h0.1 (c.csField s, true) (List.mem_cons_of_mem _ (List.mem_cons_self ..)) (Bool.or_true _)
  exact ⟨e1, e2, e3.symm⟩

/-- A region within an `Apart` magazine is harmless. -/
theorem harmless_mag {r w : Region} (ha : c.Apart s r) (hs : Region.Sub w r) : Harmless c s w :=
  ⟨ha.2.1.sub_left hs, (ha.2.2.1 (c.maskTable s, false) maskTable_mem_fixed).sub_left hs,
    (ha.2.2.1 (c.rsVar s, false) rsVar_mem_fixed).sub_left hs⟩

end Harmless

end VG.Proof.CEntry
