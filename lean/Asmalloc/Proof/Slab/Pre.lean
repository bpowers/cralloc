module

public import Asmalloc.Spec.Slab
public import Asmalloc.Proof.Framework.Sig
public import Asmalloc.Proof.Framework.Offset
public import Asmalloc.Proof.Slab.Stack

/-!
# The routines' precondition, unfolded, and a retry's

A routine's contract (`Spec.Slab.contract`) is built with `Sig.contract` on
the calling convention `Abi.withExtra (some c.regions)`. For a signature
with no buffers and no arguments in memory, its precondition says: the
convention's well-formedness, that the state's regions are exactly
`c.regions` (`World`), and `Ctx.pre`.

A retry's precondition follows from the entry state's: the regions are
apart (`world_of`) because of where the slabs and what `Ctx.fixed` names are
(`Layout`), which a state with the same addresses shares (`Same`), and
because of the invariant, which a routine keeps wherever it may be aborted
if it writes only regions apart from the slabs (`inv_frame`).
-/

@[expose] public section

namespace VG.Proof.Slab

open VG.Spec.Slab

theorem take_drop_eq {α : Type} {l r : List α} (h₁ : l.take (l.length - r.length) = [])
    (h₂ : l.drop (l.length - r.length) = r) : l = r := by
  rcases List.take_eq_nil_iff.mp h₁ with h | h
  · rw [h, List.drop_zero] at h₂; exact h₂
  · subst h; simpa using h₂

/-- The precondition of a routine with no buffers and no arguments in
memory. -/
theorem pre_iff {M : ISA} (c : Spec.Slab.Ctx M) {post : M.State → M.State → Prop}
    {exit : String → M.State → M.State → Prop} {abort : M.State → M.State → Prop}
    {trace : M.State → List Event → Prop} {s : M.State}
    {vals : M.State → List (BitVec 64)}
    (hv : c.A.args ((c.sig.words c.A.ptrBits).map (·.bits c.A.ptrBits)) = some vals)
    (hb : Sig.bufs c.sig.params (vals s) = [])
    (ha : c.A.argArea ((c.sig.words c.A.ptrBits).map (·.bits c.A.ptrBits)) s = []) :
    (contract c post exit abort trace).pre s ↔
      c.A.wf ((c.sig.words c.A.ptrBits).map (·.bits c.A.ptrBits)) 0 s ∧ c.World s ∧ c.pre s := by
  simp only [contract, Sig.contract, Abi.withExtra, hv, hb, ha, List.map_nil, List.append_nil,
    List.filter_nil, List.Pairwise.nil, List.not_mem_nil, false_implies, implies_true, true_and,
    Curry.apply_const]
  constructor
  · rintro ⟨⟨hwf, hd1, hd2, hnw, hp, hres, -, -⟩, hrd, hwr, hpre⟩
    exact ⟨hwf, ⟨take_drop_eq hrd hd1, take_drop_eq hwr hd2, hnw, hp, hres⟩, hpre⟩
  · rintro ⟨hwf, ⟨hrd, hwr, hnw, hp, hres⟩, hpre⟩
    simp only [Spec.Slab.Ctx.ro, Spec.Slab.Ctx.rw] at hrd hwr
    simp only [hrd, hwr, Nat.sub_self, List.take_zero, List.drop_zero, List.not_mem_nil,
      false_implies, implies_true, and_true, true_and]
    exact ⟨⟨hwf, hnw, hp, hres⟩, hpre⟩

section Parts

variable {M : ISA} (c : Spec.Slab.Ctx M) {post : M.State → M.State → Prop}
    {exit : String → M.State → M.State → Prop} {abort : M.State → M.State → Prop}
    {trace : M.State → List Event → Prop} {vals : M.State → List (BitVec 64)}
    (hv : c.A.args ((c.sig.words c.A.ptrBits).map (·.bits c.A.ptrBits)) = some vals)
include hv

theorem post_iff (s s' : M.State) : (contract c post exit abort trace).post s s' ↔ post s s' := by
  simp only [contract, Sig.contract, Abi.withExtra, hv, Curry.apply_const, true_and]

theorem exit_iff (t : String) (s s' : M.State) :
    (contract c post exit abort trace).exit t s s' ↔ vals s' = vals s ∧ exit t s s' := by
  simp only [contract, Sig.contract, Abi.withExtra, hv]

theorem abort_iff (s p : M.State) :
    (contract c post exit abort trace).abort s p ↔ vals p = vals s ∧ (abort s p ∧ c.inv p) := by
  simp only [contract, Sig.contract, Abi.withExtra, hv]

end Parts

theorem trace_eq {M : ISA} (c : Spec.Slab.Ctx M) {post : M.State → M.State → Prop}
    {exit : String → M.State → M.State → Prop} {abort : M.State → M.State → Prop}
    {trace : M.State → List Event → Prop} :
    (contract c post exit abort trace).trace = trace := rfl

theorem env_eq {M : ISA} (c : Spec.Slab.Ctx M) {post : M.State → M.State → Prop}
    {exit : String → M.State → M.State → Prop} {abort : M.State → M.State → Prop}
    {trace : M.State → List Event → Prop} (p p' : M.State) :
    (contract c post exit abort trace).env p p' ↔
      c.A.rd p' = c.ro p' ∧ c.A.wr p' = c.rw p' ∧
        Kept (c.rsVar p :: c.A.reserved 0 p) (c.mem p) (c.mem p') ∧ (c.inv p → c.inv p') :=
  Iff.rfl

/-! ## The regions, from the layout and the invariant -/

section World

variable {M : ISA} {c : Spec.Slab.Ctx M} {s : M.State}

/-- A routine's class has a word in the slab. -/
theorem cls_lt (h : c.argsOk s) : c.cls s < CLASS_LIMIT := by
  obtain ⟨A, f, sig⟩ := c
  cases f
  · exact h.2.2.1
  · exact h
  all_goals
    simp only [Spec.Slab.Ctx.cls, SLAB_BYTES, SLAB_SHIFT, CLASS_LIMIT]
    omega

theorem wordOf_cpu : c.wordOf s (c.cpu s) = c.w s := rfl

/-- Slab `i`'s word of the class is in the slabs. -/
theorem wordAt_sub {i : Nat} (hc : c.cls s < CLASS_LIMIT) (hi : i < SLAB_COUNT) :
    Region.Sub ⟨wordAddr (c.sym s SLABS) i (c.cls s), 8⟩ (c.slabs s) := by
  simp only [Spec.Slab.Ctx.slabs, wordAddr]
  apply Offset.sub_base
  simp only [CLASS_LIMIT, SLAB_BYTES, SLAB_SHIFT, SLAB_COUNT] at *
  omega

/-- The slab word is in the slabs. -/
theorem word_sub (hc : c.cls s < CLASS_LIMIT) (hi : c.InRange s) :
    Region.Sub ⟨c.word s, 8⟩ (c.slabs s) :=
  wordAt_sub hc hi

/-- A region within the slabs, which do not wrap around, does not either. -/
theorem nowrap_sub (hpb : c.A.ptrBits = 64) (hl : c.Layout s) {i : Nat} (hc : c.cls s < CLASS_LIMIT)
    (hi : i < SLAB_COUNT) : (wordAddr (c.sym s SLABS) i (c.cls s)).toNat + 8 ≤ 2 ^ 64 := by
  have h := hl.slabs_nowrap
  simp only [Spec.Slab.Ctx.slabs, hpb] at h
  simp only [wordAddr, BitVec.toNat_add, BitVec.toNat_ofNat]
  simp only [CLASS_LIMIT, SLAB_BYTES, SLAB_SHIFT, SLAB_COUNT] at *
  rw [Nat.mod_eq_of_lt (a := i * 2 ^ 12 + 8 * c.cls s) (by omega), Nat.mod_eq_of_lt (by omega)]
  omega

/-- What `more` names lies apart, by the invariant. -/
theorem more_apart (hinv : c.inv s) (hi : c.InRange s) : ∀ x ∈ c.more s, c.Apart s x.1 := by
  obtain ⟨A, f, sig⟩ := c
  cases f <;> simp only [Spec.Slab.Ctx.more, Spec.Slab.Ctx.inv] at hinv ⊢
  · split
    · intro x hx
      simp only [List.mem_singleton] at hx; subst hx
      exact (hinv _ hi).2 (by assumption)
    · simp
  · split
    · intro x hx
      simp only [List.mem_singleton] at hx; subst hx
      exact (hinv.2 _ hi).2 (by assumption)
    · simp
  · simp
  · simp
  · split
    · rename_i h
      intro x hx
      simp only [List.mem_singleton] at hx; subst hx
      exact stackOk_top (hinv _ hi) h.1 h.2
    · simp

/-- `more` names at most one region. -/
theorem more_cases : c.more s = [] ∨ ∃ x, c.more s = [x] := by
  obtain ⟨A, f, sig⟩ := c
  cases f <;> simp only [Spec.Slab.Ctx.more] <;> (try split) <;> simp

/-- The regions are apart: from the layout and the invariant. -/
theorem world_of (hpb : c.A.ptrBits = 64) (hrd : c.A.rd s = c.ro s) (hwr : c.A.wr s = c.rw s)
    (hl : c.Layout s) (hinv : c.inv s) (hc : c.cls s < CLASS_LIMIT) : c.World s := by
  refine ⟨hrd, hwr, ?_, ?_, ?_⟩
  · intro x hx
    simp only [Spec.Slab.Ctx.regions, List.mem_append] at hx
    rcases hx with hx | hx
    · exact hl.nowrap x hx
    · split at hx
      · rename_i hi
        rcases List.mem_cons.mp hx with rfl | hx
        · rw [hpb]; exact nowrap_sub hpb hl hc hi
        · exact (more_apart hinv hi x hx).1
      · simp at hx
  · simp only [Spec.Slab.Ctx.regions]
    refine List.pairwise_append.mpr ⟨hl.disj, ?_, ?_⟩
    · split
      · rename_i hi
        refine List.pairwise_cons.mpr ⟨fun b hb _ => ?_, ?_⟩
        · exact ((more_apart hinv hi b hb).2.1.sub_right (word_sub hc hi)).symm
        · rcases more_cases (c := c) (s := s) with h | ⟨x, h⟩ <;> rw [h] <;> simp
      · exact .nil
    · intro a ha b hb _
      split at hb
      · rename_i hi
        rcases List.mem_cons.mp hb with rfl | hb
        · exact (hl.slabs a ha).sub_right (word_sub hc hi)
        · exact ((more_apart hinv hi b hb).2.2.1 a ha).symm
      · simp at hb
  · intro x hx q hq
    simp only [Spec.Slab.Ctx.regions, List.mem_append] at hx
    rcases hx with hx | hx
    · exact hl.res x hx q hq
    · split at hx
      · rename_i hi
        rcases List.mem_cons.mp hx with rfl | hx
        · exact (hl.slabs_res q hq).sub_left (word_sub hc hi)
        · exact (more_apart hinv hi x hx).2.2.2 q hq
      · simp at hx

end World

/-! ## States with the same addresses -/

section Same

variable {M : ISA} (c : Spec.Slab.Ctx M)

/-- `p` has the addresses of `s`: the same symbols, thread pointer,
constants, arguments and reserved stack, and `__rseq_offset`. -/
structure Same (s p : M.State) : Prop where
  sym : ∀ n, c.sym p n = c.sym s n
  tp : c.tp p = c.tp s
  param : ∀ n, c.param p n = c.param s n
  arg : ∀ i, c.arg p i = c.arg s i
  res : c.A.reserved 0 p = c.A.reserved 0 s
  rsVar : ∀ x, (c.rsVar s).Contains x 1 → c.mem p x = c.mem s x

variable {c} {s p : M.State}

theorem rsVar_contains (a : Addr) {i : Nat} (hi : i < 8) :
    (⟨a, 8⟩ : Region).Contains (a + BitVec.ofNat 64 i) 1 := by
  simp only [Region.Contains]
  rw [Mem.sub_ofNat_toNat a (by omega)]; omega

namespace Same

theorem rsVar_eq (h : Same c s p) : c.rsVar p = c.rsVar s := by
  simp only [Spec.Slab.Ctx.rsVar, h.sym]

theorem area (h : Same c s p) : c.area p = c.area s := by
  simp only [Spec.Slab.Ctx.area, h.tp, h.sym]
  congr 1
  exact Mem.readW_congr fun i hi => h.rsVar _ (rsVar_contains _ (by omega))

theorem csField (h : Same c s p) : c.csField p = c.csField s := by
  simp only [Spec.Slab.Ctx.csField, h.area]

theorem cpuField (h : Same c s p) : c.cpuField p = c.cpuField s := by
  simp only [Spec.Slab.Ctx.cpuField, h.area]

theorem cls (h : Same c s p) : c.cls p = c.cls s := by
  obtain ⟨A, f, sig⟩ := c
  cases f <;> simp only [Spec.Slab.Ctx.cls, h.param, h.arg]

theorem maskEntry (h : Same c s p) : c.maskEntry p = c.maskEntry s := by
  simp only [Spec.Slab.Ctx.maskEntry, Spec.Slab.Ctx.maskAddr, h.sym, h.cls]

theorem link (h : Same c s p) : c.link p = c.link s := by
  simp only [Spec.Slab.Ctx.link, h.arg]

theorem slabs (h : Same c s p) : c.slabs p = c.slabs s := by
  simp only [Spec.Slab.Ctx.slabs, h.sym]

theorem fixed (h : Same c s p) : c.fixed p = c.fixed s := by
  simp only [Spec.Slab.Ctx.fixed, h.rsVar_eq, h.cpuField, h.csField, h.maskEntry, h.link]

theorem apart (h : Same c s p) (r : Region) : c.Apart p r ↔ c.Apart s r := by
  simp only [Spec.Slab.Ctx.Apart, h.slabs, h.fixed, h.res]

theorem symm (h : Same c s p) : Same c p s where
  sym n := (h.sym n).symm
  tp := h.tp.symm
  param n := (h.param n).symm
  arg i := (h.arg i).symm
  res := h.res.symm
  rsVar x hx := (h.rsVar x (by rwa [← h.rsVar_eq])).symm

theorem layout_of {s p : M.State} (h1 : c.slabs p = c.slabs s) (h2 : c.fixed p = c.fixed s)
    (h3 : c.A.reserved 0 p = c.A.reserved 0 s) (hl : c.Layout s) : c.Layout p where
  slabs_nowrap := by rw [h1]; exact hl.slabs_nowrap
  slabs_res := by rw [h1, h3]; exact hl.slabs_res
  nowrap := by rw [h2]; exact hl.nowrap
  disj := by rw [h2]; exact hl.disj
  slabs := by rw [h2, h1]; exact hl.slabs
  res := by rw [h2, h3]; exact hl.res

theorem layout (h : Same c s p) : c.Layout p ↔ c.Layout s :=
  ⟨layout_of h.symm.slabs h.symm.fixed h.symm.res, layout_of h.slabs h.fixed h.res⟩

theorem argsOk (h : Same c s p) : c.argsOk p ↔ c.argsOk s := by
  obtain ⟨A, f, sig⟩ := c
  cases f <;> simp only [Spec.Slab.Ctx.argsOk, Spec.Slab.Ctx.mask, h.param, h.cls, h.arg]

end Same

end Same

/-! ## The invariant, kept -/

section Inv

variable {M : ISA} {c : Spec.Slab.Ctx M} {s p : M.State}

/-- What a region a routine writes before its commit must be apart from, for
the invariant to be kept: the slabs, `__rseq_offset`, the mask's entry and,
for a stack's routine, every region that may hold a link slot. -/
def Harmless (c : Spec.Slab.Ctx M) (s : M.State) (w : Region) : Prop :=
  w.Disjoint (c.slabs s) ∧ w.Disjoint (c.rsVar s) ∧ (c.form = .table → w.Disjoint (c.maskEntry s)) ∧
    ((c.form = .stackPush ∨ c.form = .stackPop) → ∀ r, c.Apart s r → r.Disjoint w)

/-- Memory changed only in harmless regions keeps the class's words. -/
theorem wordOf_frame (h : Same c s p) {W : List Region} (hm : Frame W (c.mem s) (c.mem p))
    (hW : ∀ w ∈ W, Harmless c s w) (hc : c.cls s < CLASS_LIMIT) {i : Nat} (hi : i < SLAB_COUNT) :
    c.wordOf p i = c.wordOf s i := by
  simp only [Spec.Slab.Ctx.wordOf, h.sym, h.cls]
  exact hm.readW (r := ⟨wordAddr (c.sym s SLABS) i (c.cls s), 8⟩) (Region.contains_self _ _)
    (fun w hw => (hW w hw).1.symm.sub_left (wordAt_sub hc hi)) (by decide)

/-- The routine's mask, in memory changed only in harmless regions. -/
theorem mask_frame (h : Same c s p) {W : List Region} (hm : Frame W (c.mem s) (c.mem p))
    (hW : ∀ w ∈ W, Harmless c s w) : c.mask p = c.mask s := by
  have hma : c.maskAddr p = c.maskAddr s := congrArg Region.base h.maskEntry
  obtain ⟨A, f, sig⟩ := c
  cases f <;> simp only [Spec.Slab.Ctx.mask, h.param]
  rw [hma]
  congr 1
  exact hm.readW (r := ⟨Spec.Slab.Ctx.maskAddr ⟨A, .table, sig⟩ s, 4⟩) (Region.contains_self _ _)
    (fun w hw => ((hW w hw).2.2.1 rfl).symm) (by decide)

theorem slots_frame (h : Same c s p) {W : List Region} (hm : Frame W (c.mem s) (c.mem p))
    (hW : ∀ w ∈ W, Harmless c s w) : c.slots p = c.slots s := by
  simp only [Spec.Slab.Ctx.slots, mask_frame h hm hW]

/-- The invariant is kept by memory changed only in harmless regions. -/
theorem inv_frame (h : Same c s p) {W : List Region} (hm : Frame W (c.mem s) (c.mem p))
    (hW : ∀ w ∈ W, Harmless c s w) (hc : c.cls s < CLASS_LIMIT) (hinv : c.inv s) : c.inv p := by
  have hwo := fun i (hi : i < SLAB_COUNT) => wordOf_frame h hm hW hc hi
  have hsl := slots_frame h hm hW
  have hap := h.apart
  obtain ⟨A, f, sig⟩ := c
  cases f <;> simp only [Spec.Slab.Ctx.inv, Spec.Slab.Ctx.SlotOk] at hinv ⊢
  · intro i hi
    rw [hwo i hi, hsl, hap]
    exact hinv i hi
  · refine ⟨by rw [mask_frame h hm hW]; exact hinv.1, fun i hi => ?_⟩
    rw [hwo i hi, hsl, hap]
    exact hinv.2 i hi
  all_goals
    intro i hi
    rw [hwo i hi]
    refine stackOk_frame (hinv i hi) (fun r hr => (hap r).mpr hr) fun x hx => ?_
    exact hm.readW (r := ⟨x, 8⟩) (Region.contains_self _ _)
      (fun w hw => (hW w hw).2.2.2 (by simp) _ hx) (by decide)

end Inv

/-! ## A retry's precondition -/

/-- A retry's precondition holds in a state `p'` with the addresses of the
entry state `s`, where `env` holds after an abort in `p`, where the routine
kept the invariant. -/
theorem retry_pre {M : ISA} (c : Spec.Slab.Ctx M) {post : M.State → M.State → Prop}
    {exit : String → M.State → M.State → Prop} {abort : M.State → M.State → Prop}
    {trace : M.State → List Event → Prop} {vals : M.State → List (BitVec 64)} {s p p' : M.State}
    (hv : c.A.args ((c.sig.words c.A.ptrBits).map (·.bits c.A.ptrBits)) = some vals)
    (hb : Sig.bufs c.sig.params (vals p') = [])
    (ha : c.A.argArea ((c.sig.words c.A.ptrBits).map (·.bits c.A.ptrBits)) p' = [])
    (hpb : c.A.ptrBits = 64) (hwf : c.A.wf ((c.sig.words c.A.ptrBits).map (·.bits c.A.ptrBits)) 0 p')
    (hs : c.pre s) (hsame : Same c s p') (hinv : c.inv p)
    (he : (contract c post exit abort trace).env p p') : (contract c post exit abort trace).pre p' := by
  obtain ⟨hrd, hwr, -, hinv'⟩ := he
  obtain ⟨htp, hsym, hargs, hl, -⟩ := hs
  have hargs' := hsame.argsOk.mpr hargs
  have hl' := hsame.layout.mpr hl
  have hi' := hinv' hinv
  exact (pre_iff c hv hb ha).mpr ⟨hwf, world_of hpb hrd hwr hl' hi' (cls_lt hargs'),
    htp, hsym, hargs', hl', hi'⟩

/-! ## What the routines write before their commit -/

section Harmless

variable {M : ISA} {c : Spec.Slab.Ctx M} {s : M.State}

theorem rsVar_mem_fixed : (c.rsVar s, false) ∈ c.fixed s :=
  List.mem_append_left _ (List.mem_cons_self ..)

theorem cs_mem_fixed : (c.csField s, true) ∈ c.fixed s :=
  List.mem_append_left _ (List.mem_cons_of_mem _ (List.mem_cons_of_mem _ (List.mem_singleton_self _)))

theorem mask_mem_fixed (hf : c.form = .table) : (c.maskEntry s, false) ∈ c.fixed s := by
  obtain ⟨A, f, sig⟩ := c
  cases hf
  exact List.mem_append_right _ (List.mem_singleton_self _)

theorem link_mem_fixed (hf : c.form = .stackPush) : (c.link s, true) ∈ c.fixed s := by
  obtain ⟨A, f, sig⟩ := c
  cases hf
  exact List.mem_append_right _ (List.mem_singleton_self _)

/-- Two regions of `fixed`, the first before the second, one written. -/
theorem fixed_disj (hl : c.Layout s) {a b : Region × Bool} {l₁ l₂ : List (Region × Bool)}
    (h : c.fixed s = l₁ ++ a :: l₂) (hb : b ∈ l₂) (hw : (a.2 || b.2) = true) : a.1.Disjoint b.1 := by
  have := hl.disj
  rw [h] at this
  exact (List.pairwise_cons.mp (List.pairwise_append.mp this).2.1).1 b hb hw

/-- `rseq_cs` and `__rseq_offset` are apart: the first is written. -/
theorem disj_cs_rsVar (hl : c.Layout s) : (c.rsVar s).Disjoint (c.csField s) :=
  fixed_disj (a := (c.rsVar s, false)) (b := (c.csField s, true)) (l₁ := []) hl rfl
    (List.mem_cons_of_mem _ (List.mem_cons_self ..)) (Bool.or_true _)

/-- `rseq_cs` is harmless. -/
theorem harmless_cs (hl : c.Layout s) : Harmless c s (c.csField s) := by
  have h1 : (c.csField s).Disjoint (c.slabs s) := hl.slabs (c.csField s, true) cs_mem_fixed
  have h2 : (c.csField s).Disjoint (c.rsVar s) := (disj_cs_rsVar hl).symm
  have h4 : ∀ r, c.Apart s r → r.Disjoint (c.csField s) := fun r hr => hr.2.2.1 (c.csField s, true) cs_mem_fixed
  refine ⟨h1, h2, fun hf => ?_, fun _ => h4⟩
  obtain ⟨A, f, sig⟩ := c
  cases hf
  have hd := hl.disj
  have e : Spec.Slab.Ctx.fixed ⟨A, .table, sig⟩ s =
      [(Spec.Slab.Ctx.rsVar ⟨A, .table, sig⟩ s, false), (Spec.Slab.Ctx.cpuField ⟨A, .table, sig⟩ s, false),
        (Spec.Slab.Ctx.csField ⟨A, .table, sig⟩ s, true),
        (Spec.Slab.Ctx.maskEntry ⟨A, .table, sig⟩ s, false)] := rfl
  rw [e] at hd
  exact (List.pairwise_cons.mp (List.pairwise_cons.mp (List.pairwise_cons.mp hd).2).2).1
    (Spec.Slab.Ctx.maskEntry ⟨A, .table, sig⟩ s, false) (List.mem_singleton_self _) (Bool.true_or _)

/-- A stack push's magazine's link slot is harmless. -/
theorem harmless_link (hf : c.form = .stackPush) (hl : c.Layout s) : Harmless c s (c.link s) := by
  refine ⟨hl.slabs (c.link s, true) (link_mem_fixed hf), ?_, (fun h => by rw [hf] at h; cases h),
    fun _ r hr => hr.2.2.1 (c.link s, true) (link_mem_fixed hf)⟩
  obtain ⟨A, f, sig⟩ := c
  cases hf
  exact (fixed_disj (a := (Spec.Slab.Ctx.rsVar ⟨A, .stackPush, sig⟩ s, false)) (l₁ := []) hl rfl
    (List.mem_cons_of_mem _ (List.mem_cons_of_mem _ (List.mem_singleton_self _)))
    (Bool.or_true _)).symm

/-- A region within an `Apart` magazine of a pop's or push's is harmless. -/
theorem harmless_mag (hf : c.form = .inline ∨ c.form = .table) {r w : Region} (ha : c.Apart s r)
    (hs : Region.Sub w r) : Harmless c s w := by
  refine ⟨ha.2.1.sub_left hs, (ha.2.2.1 (c.rsVar s, false) rsVar_mem_fixed).sub_left hs,
    fun h => (ha.2.2.1 (c.maskEntry s, false) (mask_mem_fixed h)).sub_left hs, fun h => ?_⟩
  rcases hf with hf | hf <;> rw [hf] at h <;> simp at h

end Harmless

end VG.Proof.Slab
