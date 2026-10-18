module

public import Asmalloc.Proof.CEntry.X86_64.Facts
public import Asmalloc.Proof.Slab.X86_64.Tail

/-!
# The C routines' back half on x86-64

Each C routine ends with the out-of-line pop's or push's steps on `r9`
(`Proof/Slab/X86_64/Tail.lean`), from a state its front half reaches: one
with memory as after the arm, the entry state's regions, and in `r9` the
address of the word in slab 0 of the class `k` it found (`Ready`). From the
precondition, such a state meets what the steps need of it (`popOk`,
`pushOk`), and their results are the contract's (`popped`, `pushed`, and
the misses). Where a routine may be aborted, `retryC` gives the retry's
precondition from what the run has written, as `Mid.retry` does for the
slab routines.
-/

@[expose] public section

namespace VG.Proof.CEntry.X86_64

open VG.X86_64 VG.Spec.Slab VG.Spec.CEntry VG.Proof.Slab.X86_64 VG.Proof.CEntry VG.Proof.Slab

/-- Closes an equation of two lists of accesses that differ only in how
they are grouped and in `Ctx.rd` and `Ctx.wr` unfolded. -/
macro "events_norm" : tactic =>
  `(tactic| (set_option linter.unusedSimpArgs false in
    simp only [Spec.CEntry.Ctx.rd, Spec.CEntry.Ctx.wr, List.append_assoc, List.cons_append,
      List.nil_append, List.append_nil]))

/-- The C routines' scratch registers. -/
def scratchC : List Reg := [.rax, .rcx, .rdx, .r8, .r9, .r10, .r11]

/-! ## A retry -/

section Retry

variable {c : Spec.CEntry.Ctx isa}

theorem retryC (hA : X86 c.A) {post : State → State → Prop}
    {exit : String → State → State → Prop} {abort : State → State → Prop}
    {trace : State → List Event → Prop} {vals : State → List (BitVec 64)}
    (hv : c.A.args ((c.sig.words c.A.ptrBits).map (·.bits c.A.ptrBits)) = some vals)
    (hb : ∀ x, Sig.bufs c.sig.params (vals x) = [])
    (ha : ∀ x, c.A.argArea ((c.sig.words c.A.ptrBits).map (·.bits c.A.ptrBits)) x = [])
    (hlen : (c.sig.words 64).length ≤ 6) {rs : List Reg} {s p : State} {m : Mem}
    {W : List Region} (hs : c.pre s) (hm : Mid rs s m p)
    (hrs : ∀ r ∈ argRegs.take (c.sig.words 64).length, r ∉ rs) (hsp : Reg.rsp ∉ rs)
    (hfr : Frame W s.mem m) (hW : ∀ w ∈ W, Harmless c s w) :
    c.inv p ∧ ∀ p', rely p p' → (contract c post exit abort trace).env p p' →
      (contract c post exit abort trace).pre p' := by
  have hA' : X86 c.slab.A := hA
  have hrsv : ∀ x, (⟨s.syms RS, 8⟩ : Region).Contains x 1 → p.mem x = s.mem x := fun x hx => by
    rw [hm.mem]
    exact hfr x fun w hw hc => (hW w hw).2.2 x hc (by rwa [rsVar_c hA])
  have hsame : Same c s p := same_of hA' hlen hm.syms hm.fsBase hm.params
    (fun r hr => hm.gpr r (hrs r hr)) (hm.gpr .rsp hsp) hrsv
  have hfr' : Frame W (c.mem s) (c.mem p) := by rw [mem_c hA, mem_c hA, hm.mem]; exact hfr
  have hip := inv_frame hsame hfr' hW hs.2.2.2
  refine ⟨hip, fun p' hr he => ?_⟩
  obtain ⟨hg, -, -, -, -, hfs, hsy, hpar, -, -⟩ := hr
  have hkept := he.2.2.1 (c.rsVar p) (List.mem_cons_self ..)
  have hsame' : Same c s p' := same_of hA' hlen (hsy.trans hm.syms) (hfs.trans hm.fsBase)
    (hpar.trans hm.params) (fun r hr => by rw [hg]; exact hm.gpr r (hrs r hr))
    (by rw [hg]; exact hm.gpr .rsp hsp) fun x hx => by
      have h1 := hkept x (by rwa [hsame.rsVar', rsVar_c hA])
      rw [mem_c hA, mem_c hA] at h1
      rw [h1]; exact hrsv x hx
  have hl6 : ((c.sig.words 64).map (·.bits 64)).length ≤ argRegs.length := by
    simpa [argRegs] using hlen
  have hwf : c.A.wf ((c.sig.words c.A.ptrBits).map (·.bits c.A.ptrBits)) 0 p' := by
    rw [hA.wf, hA.ptrBits]; simp only [abi, hl6, ↓reduceIte]
  exact retry_pre c hv (hb p') (ha p') hA.ptrBits hwf hs hsame' hip he

/-- `env` keeps the return address's slot. -/
theorem env_retC (hA : X86 c.A) {post : State → State → Prop}
    {exit : String → State → State → Prop} {abort : State → State → Prop}
    {trace : State → List Event → Prop} {p p' : State}
    (he : (contract c post exit abort trace).env p p') :
    p'.mem.readW (p.gpr .rsp) 64 = p.mem.readW (p.gpr .rsp) 64 := by
  have hk := he.2.2.1 ⟨p.gpr .rsp, 8⟩ (List.mem_cons_of_mem _ (hA.retSlot p))
  rw [mem_c hA, mem_c hA] at hk
  exact Mem.readW_congr fun i hi => hk _ (rsVar_contains _ (by omega))

theorem harmless_csC (hA : X86 c.A) {s : State} (hs : c.pre s) :
    ∀ w ∈ [(⟨area s + 8, 8⟩ : Region)], Harmless c s w := by
  intro w hw
  rw [List.mem_singleton.mp hw, ← csField_c hA]
  exact harmless_cs hs.2.2.1

end Retry

/-! ## The state the front half reaches -/

/-- A state the front half of a routine reaches, from the entry state `s`,
having found the class `k`: memory as after the arm, the entry state's
regions, `r8` the CPU's slab's offset, and `r9` the class's word in slab 0. -/
structure Ready (c : Spec.CEntry.Ctx isa) (s : State) (k : Nat) (τ : State) : Prop where
  mem : τ.mem = m1 s
  rd : τ.rd = s.rd
  wr : τ.wr = s.wr
  r8 : τ.gpr .r8 = sh s
  r9 : τ.gpr .r9 = s.syms SLABS + BitVec.ofNat 64 (8 * k)

section Ready

variable {c : Spec.CEntry.Ctx isa} {s τ : State} {k : Nat}

theorem wa_eq (hA : X86 c.A) (h : Ready c s k τ) : Tail.wa .r9 τ = c.word s k := by
  rw [Tail.wa, h.r9, h.r8, Spec.CEntry.Ctx.word, wordAddr, sym_c hA, cpu_c hA, sh_eq,
    BitVec.ofNat_add, BitVec.add_assoc, BitVec.add_comm (BitVec.ofNat 64 (8 * k))]

theorem ma_eq (hA : X86 c.A) (h : Ready c s k τ) :
    Tail.ma .r9 τ = c.sym s SLABS + BitVec.ofNat 64 (8 * k + MASK_DISP) := by
  rw [Tail.ma, h.r9, sym_c hA, BitVec.add_assoc, BitVec.ofNat_add]; rfl

end Ready

/-! ## What the precondition gives the back half -/

/-- What the back half of a routine that found the class `k` on a CPU with a
slab needs, from the precondition. -/
structure BackFacts (c : Spec.CEntry.Ctx isa) (s : State) (k : Nat) : Prop where
  hk : k < MASK_COUNT
  word : (⟨c.word s k, 8⟩ : Region) ∈ s.wr
  mask : InRegions (s.rd ++ s.wr) (c.sym s SLABS + BitVec.ofNat 64 (8 * k + MASK_DISP)) 4
  cs_word : Region.Disjoint ⟨area s + 8, 8⟩ ⟨c.word s k, 8⟩
  cs_mask : Region.Disjoint ⟨area s + 8, 8⟩ ⟨c.sym s SLABS + BitVec.ofNat 64 (8 * k + MASK_DISP), 4⟩
  ret_cs : Region.Disjoint ⟨area s + 8, 8⟩ ⟨s.gpr .rsp, 8⟩
  ret_word : Region.Disjoint ⟨c.word s k, 8⟩ ⟨s.gpr .rsp, 8⟩
  mag : IsSlot (c.slotsOf' s k) (c.w s k) →
    c.magRegion s k ∈ s.wr ∧ Region.Disjoint ⟨area s + 8, 8⟩ (c.magRegion s k) ∧
    Region.Disjoint ⟨c.word s k, 8⟩ (c.magRegion s k) ∧
    Region.Disjoint ⟨s.syms RS, 8⟩ (c.magRegion s k) ∧
    Region.Disjoint (c.magRegion s k) ⟨s.gpr .rsp, 8⟩ ∧ c.Apart s (c.magRegion s k)
  pre : c.pre s

theorem backFacts {c : Spec.CEntry.Ctx isa} (hA : X86 c.A) {s : State} (hw : c.World s)
    (hp : c.pre s) {k : Nat} (hk : c.cls? s = some k) (hi : c.InRange s) : BackFacts c s k := by
  have hkl := cls_lt hk
  have hmore : c.more s = (⟨c.word s k, 8⟩, true) ::
      (if IsSlot (c.slotsOf' s k) (c.w s k) then [(c.magRegion s k, true)] else []) := by
    simp only [Spec.CEntry.Ctx.more, hi, ↓reduceIte, hk]
  have hwm : (⟨c.word s k, 8⟩, true) ∈ c.more s := by rw [hmore]; exact List.mem_cons_self ..
  have hcs : (c.csField s, true) ∈ c.fixed s := cs_mem_fixed
  have hmt : (c.maskTable s, false) ∈ c.fixed s := maskTable_mem_fixed
  have hmsub := maskAt_sub (c := c) (s := s) hkl
  have hmc : (c.maskTable s).Contains (c.sym s SLABS + BitVec.ofNat 64 (8 * k + MASK_DISP)) 4 := by
    simp only [Spec.CEntry.Ctx.maskTable]
    rw [show 8 * k + MASK_DISP = MASK_DISP + 8 * k by omega, BitVec.ofNat_add, ← BitVec.add_assoc]
    apply Offset.contains_base
    · simp only [MASK_COUNT] at hkl ⊢; omega
    · simp only [MASK_COUNT] at hkl; omega
  have hcsT : (c.csField s).Disjoint (c.maskTable s) :=
    cs_table (c := c) (s := s) hp.2.2.1 (x := (c.maskTable s, false)) (List.mem_cons_self ..)
  have e1 : (c.csField s).Disjoint ⟨c.word s k, 8⟩ := disj_fixed_more hw (a := (c.csField s, true)) (b := (⟨c.word s k, 8⟩, true)) hcs hwm rfl
  have e3 : (c.csField s).Disjoint ⟨s.gpr .rsp, 8⟩ := disj_res hA hw (x := (c.csField s, true)) (mem_fixed hcs)
  have e4 : Region.Disjoint ⟨c.word s k, 8⟩ ⟨s.gpr .rsp, 8⟩ := disj_res hA hw (x := (⟨c.word s k, 8⟩, true)) (mem_more hwm)
  have e5 : (⟨c.word s k, 8⟩ : Region) ∈ s.wr := mem_wr hA hw (x := (⟨c.word s k, 8⟩, true)) (mem_more hwm) rfl
  have e6 : c.maskTable s ∈ s.rd := mem_rd hA hw (x := (c.maskTable s, false)) (mem_fixed hmt) rfl
  rw [csField_c hA] at hcsT e1 e3
  refine ⟨hkl, e5, (InRegions.of_mem e6 hmc).append_left, e1, hcsT.sub_right hmsub, e3, e4, ?_, hp⟩
  · intro hs
    have hmm : (c.magRegion s k, true) ∈ c.more s := by rw [hmore]; simp [hs]
    have hpw := (List.pairwise_append.mp hw.disj).2.1
    rw [hmore] at hpw
    simp only [hs, ↓reduceIte] at hpw
    have hwmag : Region.Disjoint ⟨c.word s k, 8⟩ (c.magRegion s k) :=
      (List.pairwise_cons.mp hpw).1 (c.magRegion s k, true) (List.mem_singleton_self _) rfl
    have hrsv : (c.rsVar s, false) ∈ c.fixed s := rsVar_mem_fixed
    have f1 : (c.csField s).Disjoint (c.magRegion s k) := disj_fixed_more hw (a := (c.csField s, true)) (b := (c.magRegion s k, true)) hcs hmm rfl
    have f2 : (c.rsVar s).Disjoint (c.magRegion s k) := disj_fixed_more hw (a := (c.rsVar s, false)) (b := (c.magRegion s k, true)) hrsv hmm rfl
    have f3 : (c.magRegion s k).Disjoint ⟨s.gpr .rsp, 8⟩ := disj_res hA hw (x := (c.magRegion s k, true)) (mem_more hmm)
    have f4 : c.magRegion s k ∈ s.wr := mem_wr hA hw (x := (c.magRegion s k, true)) (mem_more hmm) rfl
    rw [csField_c hA] at f1
    rw [rsVar_c hA] at f2
    exact ⟨f4, f1, hwmag, f2, f3, mag_apart hp.2.2.2 hkl hi hs⟩

/-! ## The values the back half reads -/

section Values

variable {c : Spec.CEntry.Ctx isa} (hA : X86 c.A) {s τ : State} {k : Nat}
  (hr : Ready c s k τ) (hb : BackFacts c s k)
include hA hr hb

theorem wv_eq : Tail.wv .r9 τ = c.w s k := by
  rw [Tail.wv, wa_eq hA hr, hr.mem, m1, readW64_writeW_disj hb.cs_word.symm,
    Spec.CEntry.Ctx.w, mem_c hA]

theorem mk_eq : (Tail.mk .r9 τ).toNat = c.maskOf s k := by
  rw [Tail.mk, ma_eq hA hr, hr.mem, m1, readW32_writeW_disj hb.cs_mask.symm,
    Spec.CEntry.Ctx.maskOf, mem_c hA]

end Values

/-- What a nonzero mask makes of the class's magazines: `2 ^ j` slots. -/
structure Sized (c : Spec.CEntry.Ctx isa) (s : State) (k j : Nat) : Prop where
  hj : 2 ≤ j
  hj' : j ≤ 16
  mask : c.maskOf s k = magMask (2 ^ j)
  slots : c.slotsOf' s k = 2 ^ j
  ok : c.InRange s → WordOk (2 ^ j) (c.w s k)

theorem sized {c : Spec.CEntry.Ctx isa} {s : State} {k : Nat} (hb : BackFacts c s k)
    (hz : c.maskOf s k ≠ 0) : ∃ j, Sized c s k j := by
  obtain ⟨hm, hinv, -⟩ := hb.pre.2.2.2 k hb.hk
  obtain ⟨j, hj, hj', hmj⟩ := hm.resolve_left hz
  have hsl : c.slotsOf' s k = 2 ^ j := by
    rw [Spec.CEntry.Ctx.slotsOf', hmj]; exact slotsOf_magMask j hj
  exact ⟨j, hj, hj', hmj, hsl, fun hi => by rw [← hsl]; exact (hinv hz (c.cpu s) hi).1⟩

theorem slots_zero {c : Spec.CEntry.Ctx isa} {s : State} {k : Nat} (hz : c.maskOf s k = 0) :
    c.slotsOf' s k = 1 := by
  rw [Spec.CEntry.Ctx.slotsOf', hz]; rfl

/-! ## The pop -/

section Pop

variable {c : Spec.CEntry.Ctx isa} (hA : X86 c.A) {s τ : State} {k : Nat}
  (hr : Ready c s k τ) (hb : BackFacts c s k)
include hA hr hb

theorem ptst_eq : Tail.ptst .r9 τ = 0 ↔
    (c.w s k).setWidth 32 &&& BitVec.ofNat 32 (c.maskOf s k) = 0 := by
  rw [Tail.ptst, wv_eq hA hr hb, ← mk_eq hA hr hb, BitVec.ofNat_toNat, BitVec.setWidth_eq]

/-- A pop's test that passes was of a nonzero mask. -/
theorem sized_of_ptest (ht : ¬ Tail.ptst .r9 τ = 0) : ∃ j, Sized c s k j := by
  refine sized hb fun hz => ht ((ptst_eq hA hr hb).mpr ?_)
  rw [hz]; exact BitVec.and_zero

theorem pop_hit {j : Nat} (hj : Sized c s k j) (hi : c.InRange s) (ht : ¬ Tail.ptst .r9 τ = 0) :
    IsSlot (2 ^ j) (c.w s k) ∧ 2 ≤ slot (2 ^ j) (c.w s k) ∧
      (Mag.ofWord (2 ^ j) s.mem (c.w s k)).pop =
        some (s.mem.readW (c.w s k - 8) 64,
          .stack (magBase (2 ^ j) (c.w s k)) (items (2 ^ j) s.mem (c.w s k)).dropLast) := by
  have hm : ¬ (c.w s k).toNat % (8 * 2 ^ j) < 16 := fun h' => ht ((ptst_eq hA hr hb).mpr
    ((test_iff32 hj.hj hj.hj' (c.w s k) _ (by rw [BitVec.toNat_ofNat, hj.mask,
      Nat.mod_eq_of_lt (magMask_lt j hj.hj')])).mpr h'))
  exact pop_of_test (dvd_marker hj.hj') (hj.ok hi) hm

theorem popOk (hi : c.InRange s) : Tail.PopOk .r9 τ where
  reg := ⟨by decide, by decide, by decide⟩
  word := by rw [hr.rd, hr.wr, wa_eq hA hr]; exact inRdWr (r := ⟨c.word s k, 8⟩) hb.word
  mask := by rw [hr.rd, hr.wr, ma_eq hA hr]; exact hb.mask
  hit ht := by
    obtain ⟨j, hj⟩ := sized_of_ptest hA hr hb ht
    obtain ⟨hs, h2, -⟩ := pop_hit hA hr hb hj hi ht
    have hsl : IsSlot (c.slotsOf' s k) (c.w s k) := by rw [hj.slots]; exact hs
    obtain ⟨hmag, -⟩ := hb.mag hsl
    have hc := contains_below hs h2 (slot_lt _ _ (Nat.two_pow_pos j))
    rw [Spec.CEntry.Ctx.magRegion, hj.slots] at hmag
    refine ⟨?_, ?_⟩
    · rw [hr.rd, hr.wr, wv_eq hA hr hb]; exact (InRegions.of_mem hmag hc).append_right
    · rw [hr.wr, wa_eq hA hr]; exact inWr (r := ⟨c.word s k, 8⟩) hb.word

theorem px_eq {j : Nat} (hj : Sized c s k j) (hi : c.InRange s) (ht : ¬ Tail.ptst .r9 τ = 0) :
    Tail.px .r9 τ = s.mem.readW (c.w s k - 8) 64 := by
  obtain ⟨hs, h2, -⟩ := pop_hit hA hr hb hj hi ht
  have hsl : IsSlot (c.slotsOf' s k) (c.w s k) := by rw [hj.slots]; exact hs
  obtain ⟨-, hcm, -⟩ := hb.mag hsl
  rw [Spec.CEntry.Ctx.magRegion, hj.slots] at hcm
  have hc := contains_below hs h2 (slot_lt _ _ (Nat.two_pow_pos j))
  rw [Tail.px, wv_eq hA hr hb, hr.mem, m1, readW64_writeW_disj (hcm.symm.sub_left (sub_of_contains hc))]

/-- A pop that passes its test took the top pointer. -/
theorem popped (hk : c.cls? s = some k) (hi : c.InRange s) (ht : ¬ Tail.ptst .r9 τ = 0) :
    c.Popped s (Tail.p5 .r9 τ) (Tail.px .r9 τ) := by
  obtain ⟨j, hj⟩ := sized_of_ptest hA hr hb ht
  obtain ⟨hs, h2, hpop⟩ := pop_hit hA hr hb hj hi ht
  have hsl : IsSlot (c.slotsOf' s k) (c.w s k) := by rw [hj.slots]; exact hs
  obtain ⟨-, hcm, hwm, -⟩ := hb.mag hsl
  rw [Spec.CEntry.Ctx.magRegion, hj.slots] at hcm hwm
  refine ⟨k, .stack (magBase (2 ^ j) (c.w s k)) (items (2 ^ j) s.mem (c.w s k)).dropLast, ?_, ?_, ?_, ?_⟩
  · simp only [Spec.CEntry.Ctx.popResult, hi, ↓reduceIte, hk, Option.bind_some, Spec.CEntry.Ctx.mag,
      hj.slots, mem_c hA, hpop, Option.map_some, px_eq hA hr hb hj hi ht]
  · rw [csField_c hA, mem_c hA, ← wa_eq hA hr, hA.mem,
      show (Tail.p5 .r9 τ).mem = τ.mem.writeW (Tail.wa .r9 τ) (Tail.wv .r9 τ - 8) from rfl, hr.mem]
    exact frame_two s.mem _ _ _ _
  · rw [hA.mem, show (Tail.p5 .r9 τ).mem = τ.mem.writeW (Tail.wa .r9 τ) (Tail.wv .r9 τ - 8) from rfl,
      wa_eq hA hr, Mem.readW_writeW_self64, wv_eq hA hr hb]
  · rw [hA.mem, show (Tail.p5 .r9 τ).mem = τ.mem.writeW (Tail.wa .r9 τ) (Tail.wv .r9 τ - 8) from rfl,
      hj.slots]
    refine ofWord_sub8 (Nat.two_pow_pos j) hs h2 fun i hi' => ?_
    have hsub := sub_item (n := 2 ^ j) (w := c.w s k) (i := i)
      (by have := slot_lt (2 ^ j) (c.w s k) (Nat.two_pow_pos j); omega)
    rw [wa_eq hA hr, readW64_writeW_disj (hwm.symm.sub_left hsub), hr.mem, m1,
      readW64_writeW_disj (hcm.symm.sub_left hsub)]

/-- A pop whose test fails takes nothing. -/
theorem pop_missed (hk : c.cls? s = some k) (hi : c.InRange s) (ht : Tail.ptst .r9 τ = 0) :
    c.popResult s = none := by
  simp only [Spec.CEntry.Ctx.popResult, hi, ↓reduceIte, hk, Option.bind_some, Option.map_eq_none_iff,
    Spec.CEntry.Ctx.mag]
  by_cases hz : c.maskOf s k = 0
  · rw [slots_zero hz]; exact pop_one _ _
  · obtain ⟨j, hj⟩ := sized hb hz
    rw [hj.slots]
    refine (pop_eq_none_iff (dvd_marker hj.hj') (hj.ok hi)).mpr
      ((test_iff32 hj.hj hj.hj' (c.w s k) _ (by rw [BitVec.toNat_ofNat, hj.mask,
        Nat.mod_eq_of_lt (magMask_lt j hj.hj')])).mp ((ptst_eq hA hr hb).mp ht))

end Pop

/-! ## The push -/

section Push

variable {c : Spec.CEntry.Ctx isa} (hA : X86 c.A) {s τ : State} {k : Nat}
  (hr : Ready c s k τ) (hb : BackFacts c s k)
include hA hr hb

theorem qtst_eq : Tail.qtst .r9 τ = 0 ↔
    (c.w s k + 8).setWidth 32 &&& BitVec.ofNat 32 (c.maskOf s k) = 0 := by
  rw [Tail.qtst, wv_eq hA hr hb, ← mk_eq hA hr hb, BitVec.ofNat_toNat, BitVec.setWidth_eq]

theorem sized_of_qtest (ht : ¬ Tail.qtst .r9 τ = 0) : ∃ j, Sized c s k j := by
  refine sized hb fun hz => ht ((qtst_eq hA hr hb).mpr ?_)
  rw [hz]; exact BitVec.and_zero

theorem push_hit {j : Nat} (hj : Sized c s k j) (hi : c.InRange s) (ht : ¬ Tail.qtst .r9 τ = 0)
    (x : BitVec 64) :
    IsSlot (2 ^ j) (c.w s k) ∧ slot (2 ^ j) (c.w s k) + 1 < 2 ^ j ∧
      (Mag.ofWord (2 ^ j) s.mem (c.w s k)).push (2 ^ j) x =
        some (.stack (magBase (2 ^ j) (c.w s k)) (items (2 ^ j) s.mem (c.w s k) ++ [x])) := by
  have hm : ¬ (c.w s k + 8).toNat % (8 * 2 ^ j) < 16 := fun h' => ht ((qtst_eq hA hr hb).mpr
    ((test_iff32 hj.hj hj.hj' (c.w s k + 8) _ (by rw [BitVec.toNat_ofNat, hj.mask,
      Nat.mod_eq_of_lt (magMask_lt j hj.hj')])).mpr h'))
  exact push_of_test j rfl hj.hj hj.hj' (hj.ok hi) s.mem x hm

/-- The free slot: in the magazine, apart from `rseq_cs`, the slab word,
`__rseq_offset` and the return address. -/
structure SlotFacts (c : Spec.CEntry.Ctx isa) (s : State) (k j : Nat) : Prop where
  inMag : (⟨magBase (2 ^ j) (c.w s k), 8 * 2 ^ j⟩ : Region).Contains (c.w s k) 8
  mem : (⟨magBase (2 ^ j) (c.w s k), 8 * 2 ^ j⟩ : Region) ∈ s.wr
  cs : Region.Disjoint ⟨c.w s k, 8⟩ ⟨area s + 8, 8⟩
  word : Region.Disjoint ⟨c.w s k, 8⟩ ⟨c.word s k, 8⟩
  rsVar : Region.Disjoint ⟨s.syms RS, 8⟩ ⟨c.w s k, 8⟩
  ret : Region.Disjoint ⟨c.w s k, 8⟩ ⟨s.gpr .rsp, 8⟩
  apart : c.Apart s ⟨magBase (2 ^ j) (c.w s k), 8 * 2 ^ j⟩

theorem slotFacts {j : Nat} (hj : Sized c s k j) (hi : c.InRange s) (ht : ¬ Tail.qtst .r9 τ = 0) :
    SlotFacts c s k j := by
  obtain ⟨hs, hlt, -⟩ := push_hit hA hr hb hj hi ht 0
  have hsl : IsSlot (c.slotsOf' s k) (c.w s k) := by rw [hj.slots]; exact hs
  obtain ⟨hmag, hcm, hwm, hrm, hret, hap⟩ := hb.mag hsl
  simp only [Spec.CEntry.Ctx.magRegion, hj.slots] at hmag hcm hwm hrm hret hap
  have hc := contains_at hs hlt
  have hsub := sub_of_contains hc
  exact ⟨hc, hmag, hcm.symm.sub_left hsub, hwm.symm.sub_left hsub, hrm.sub_right hsub,
    hret.sub_left hsub, hap⟩

theorem pushOk (hi : c.InRange s) : Tail.PushOk .r9 .rdi τ where
  reg := ⟨by decide, by decide, by decide⟩
  preg := ⟨by decide, by decide⟩
  word := by rw [hr.rd, hr.wr, wa_eq hA hr]; exact inRdWr (r := ⟨c.word s k, 8⟩) hb.word
  mask := by rw [hr.rd, hr.wr, ma_eq hA hr]; exact hb.mask
  hit ht := by
    obtain ⟨j, hj⟩ := sized_of_qtest hA hr hb ht
    have hf := slotFacts hA hr hb hj hi ht
    refine ⟨?_, ?_⟩
    · rw [hr.wr, wv_eq hA hr hb]; exact InRegions.of_mem hf.mem hf.inMag
    · rw [hr.wr, wa_eq hA hr]; exact inWr (r := ⟨c.word s k, 8⟩) hb.word

omit hA hr in
/-- Item `i` of the magazine is apart from `rseq_cs`, the slab word and the
free slot. -/
theorem item_disjoint {j : Nat} (hj : Sized c s k j)
    (hs : IsSlot (2 ^ j) (c.w s k)) {i : Nat} (hil : i < slot (2 ^ j) (c.w s k) - 1) :
    Region.Disjoint ⟨magBase (2 ^ j) (c.w s k) + BitVec.ofNat 64 (8 * (i + 1)), 8⟩ ⟨area s + 8, 8⟩ ∧
    Region.Disjoint ⟨magBase (2 ^ j) (c.w s k) + BitVec.ofNat 64 (8 * (i + 1)), 8⟩ ⟨c.word s k, 8⟩ ∧
    Region.Disjoint ⟨magBase (2 ^ j) (c.w s k) + BitVec.ofNat 64 (8 * (i + 1)), 8⟩ ⟨c.w s k, 8⟩ := by
  have hsl : IsSlot (c.slotsOf' s k) (c.w s k) := by rw [hj.slots]; exact hs
  have hlt := slot_lt (2 ^ j) (c.w s k) (Nat.two_pow_pos j)
  have hsub := sub_item (n := 2 ^ j) (w := c.w s k) (i := i) (by omega)
  obtain ⟨-, hcm, hwm, -⟩ := hb.mag hsl
  simp only [Spec.CEntry.Ctx.magRegion, hj.slots] at hcm hwm
  have hn : 8 * 2 ^ j ≤ 2 ^ 19 := by
    have := Nat.pow_le_pow_right (n := 2) (by decide) hj.hj'
    omega
  refine ⟨hcm.symm.sub_left hsub, hwm.symm.sub_left hsub, ?_⟩
  have hw := magBase_add_slot hs
  have := Offset.disjoint (magBase (2 ^ j) (c.w s k)) (d := 8 * (i + 1)) (n := 8)
    (e := 8 * slot (2 ^ j) (c.w s k)) (k := 8) (by omega) (by omega) (by omega)
  rwa [hw] at this

/-- A push that passes its test pushed the pointer. -/
theorem pushed (hk : c.cls? s = some k) (hi : c.InRange s) (ht : ¬ Tail.qtst .r9 τ = 0) :
    c.Pushed s (τ.gpr .rdi) (Tail.q5 .r9 τ .rdi) := by
  obtain ⟨j, hj⟩ := sized_of_qtest hA hr hb ht
  obtain ⟨hs, hlt, hpush⟩ := push_hit hA hr hb hj hi ht (τ.gpr .rdi)
  have hf := slotFacts hA hr hb hj hi ht
  refine ⟨k, .stack (magBase (2 ^ j) (c.w s k)) (items (2 ^ j) s.mem (c.w s k) ++ [τ.gpr .rdi]),
    ?_, ?_, ?_, ?_⟩
  · simp only [Spec.CEntry.Ctx.pushResult, hi, ↓reduceIte, hk, Option.bind_some, Spec.CEntry.Ctx.mag,
      hj.slots, mem_c hA, hpush, Option.map_some]
  · rw [csField_c hA, mem_c hA, ← wa_eq hA hr, ← wv_eq hA hr hb, hA.mem,
      show (Tail.q5 .r9 τ .rdi).mem = Tail.qEnd .r9 τ .rdi from rfl, Tail.qEnd, Tail.qSlot, hr.mem]
    exact (frame_three s.mem _ _ _ _ _ _).mono (by simp)
  · rw [hA.mem, show (Tail.q5 .r9 τ .rdi).mem = Tail.qEnd .r9 τ .rdi from rfl, Tail.qEnd, wa_eq hA hr,
      Mem.readW_writeW_self64, wv_eq hA hr hb]
  · rw [hA.mem, show (Tail.q5 .r9 τ .rdi).mem = Tail.qEnd .r9 τ .rdi from rfl, hj.slots]
    refine ofWord_add8 j hj.hj' rfl hs hlt _ ?_ fun i hil => ?_
    · rw [Tail.qEnd, wa_eq hA hr, readW64_writeW_disj hf.word, Tail.qSlot, wv_eq hA hr hb,
        Mem.readW_writeW_self64]
    · obtain ⟨h1, h2, h3⟩ := item_disjoint hb hj hs hil
      rw [Tail.qEnd, wa_eq hA hr, readW64_writeW_disj h2, Tail.qSlot, wv_eq hA hr hb,
        readW64_writeW_disj h3, hr.mem, m1, readW64_writeW_disj h1]

theorem push_missed (hk : c.cls? s = some k) (hi : c.InRange s) (ht : Tail.qtst .r9 τ = 0)
    (x : BitVec 64) : c.pushResult s x = none := by
  simp only [Spec.CEntry.Ctx.pushResult, hi, ↓reduceIte, hk, Option.bind_some, Option.map_eq_none_iff,
    Spec.CEntry.Ctx.mag]
  by_cases hz : c.maskOf s k = 0
  · rw [slots_zero hz]; exact push_one _ _ _
  · obtain ⟨j, hj⟩ := sized hb hz
    rw [hj.slots]
    exact (push_eq_none_iff _ j rfl hj.hj hj.hj' (hj.ok hi)).mpr
      ((test_iff32 hj.hj hj.hj' (c.w s k + 8) _ (by rw [BitVec.toNat_ofNat, hj.mask,
        Nat.mod_eq_of_lt (magMask_lt j hj.hj')])).mp ((qtst_eq hA hr hb).mp ht))

/-- After the free slot is written, every slab's magazine of the class is as
on entry: the free slot is above the CPU's slab's pointers, and in no other
slab's magazine (`Exclusive`). -/
theorem mags_slot (hi : c.InRange s) (ht : ¬ Tail.qtst .r9 τ = 0) :
    ∀ i < SLAB_COUNT, Mag.ofWord (c.slotsOf' s k) (Tail.qSlot .r9 τ .rdi) (c.wordOf s k i) =
      Mag.ofWord (c.slotsOf' s k) (c.mem s) (c.wordOf s k i) := by
  obtain ⟨j, hj⟩ := sized_of_qtest hA hr hb ht
  have hf := slotFacts hA hr hb hj hi ht
  obtain ⟨hs0, -, -⟩ := push_hit hA hr hb hj hi ht 0
  have hz : c.maskOf s k ≠ 0 := by rw [hj.mask]; have := four_le hj.hj; simp only [magMask]; omega
  have hfr : Frame [⟨area s + 8, 8⟩, ⟨c.w s k, 8⟩] s.mem (Tail.qSlot .r9 τ .rdi) := by
    rw [Tail.qSlot, hr.mem, wv_eq hA hr hb]; exact frame_two _ _ _ _ _
  intro i hi'
  by_cases hc : i = c.cpu s
  · subst hc
    show Mag.ofWord _ _ (c.w s k) = Mag.ofWord _ _ (c.w s k)
    rw [hj.slots, mem_c hA]
    exact ofWord_congr fun i hil => by
      obtain ⟨h1, -, h3⟩ := item_disjoint hb hj hs0 hil
      rw [Tail.qSlot, wv_eq hA hr hb, readW64_writeW_disj h3, hr.mem, m1, readW64_writeW_disj h1]
  have hok := (hb.pre.2.2.2 k hb.hk).2.1 hz i hi'
  rw [mem_c hA]
  refine ofWord_frame (by rw [hj.slots]; exact Nat.two_pow_pos j) hfr hok.1 fun hs r hr' => ?_
  rcases List.mem_cons.mp hr' with rfl | hr'
  · rw [← csField_c hA]; exact (hok.2 hs).2.2.1 (c.csField s, true) cs_mem_fixed
  · rw [List.mem_singleton.mp hr']
    have hsc : IsSlot (c.slotsOf' s k) (c.wordOf s k (c.cpu s)) := by
      show IsSlot _ (c.w s k); rw [hj.slots]; exact hs0
    have hex := (hb.pre.2.2.2 k hb.hk).2.2 i hi' (c.cpu s) hi hc hs hsc
    rw [show c.wordOf s k (c.cpu s) = c.w s k from rfl, hj.slots] at hex
    rw [hj.slots]
    exact hex.sub_right (sub_of_contains hf.inMag)

end Push

/-! ## Where a push may be aborted -/

section PushAbort

variable {c : Spec.CEntry.Ctx isa} (hA : X86 c.A) {s : State}
include hA

/-- Before the free slot is written, every slab's magazine of a class is as
on entry. -/
theorem mags_m1 {k : Nat} (hb : BackFacts c s k) : ∀ i < SLAB_COUNT,
    Mag.ofWord (c.slotsOf' s k) (m1 s) (c.wordOf s k i) = Mag.ofWord (c.slotsOf' s k) (c.mem s) (c.wordOf s k i) := by
  intro i hi
  by_cases hz : c.maskOf s k = 0
  · rw [slots_zero hz]; exact ofWord_one_congr _ _ _
  obtain ⟨j, hj⟩ := sized hb hz
  have hok := (hb.pre.2.2.2 k hb.hk).2.1 hz i hi
  rw [mem_c hA]
  refine ofWord_frame (by rw [hj.slots]; exact Nat.two_pow_pos j) frame_m1 hok.1 fun hs r hr => ?_
  rw [List.mem_singleton.mp hr, ← csField_c hA]
  exact (hok.2 hs).2.2.1 (c.csField s, true) cs_mem_fixed

/-- In memory as after the arm, the push has changed nothing a retry or
another thread can see. -/
theorem pushAbort_m1 (hw : c.World s) (hs : c.pre s) {p : State} (hm : p.mem = m1 s) :
    c.PushAbort s p := by
  unfold Spec.CEntry.Ctx.PushAbort
  rw [hA.mem, hm]
  by_cases hi : c.InRange s
  · simp only [hi, ↓reduceIte]
    cases hk : c.cls? s with
    | none => rw [csField_c hA, mem_c hA]; exact frame_m1
    | some k =>
      refine ⟨?_, mags_m1 hA (backFacts hA hw hs hk hi)⟩
      rw [csField_c hA, mem_c hA]; exact frame_m1.mono (by simp)
  · simp only [hi, ↓reduceIte]
    rw [csField_c hA, mem_c hA]; exact frame_m1

/-- After the free slot is written, too. -/
theorem pushAbort_slot (hw : c.World s) (hs : c.pre s) {k : Nat} {τ : State} (hr : Ready c s k τ)
    (hk : c.cls? s = some k) (hi : c.InRange s) (ht : ¬ Tail.qtst .r9 τ = 0) {p : State}
    (hm : p.mem = Tail.qSlot .r9 τ .rdi) : c.PushAbort s p := by
  have hb := backFacts hA hw hs hk hi
  unfold Spec.CEntry.Ctx.PushAbort
  simp only [hi, ↓reduceIte, hk]
  rw [hA.mem, hm]
  refine ⟨?_, mags_slot hA hr hb hi ht⟩
  rw [csField_c hA, mem_c hA, Tail.qSlot, hr.mem, wv_eq hA hr hb]
  exact frame_two _ _ _ _ _

end PushAbort

end VG.Proof.CEntry.X86_64
