module

public import Cralloc.Proof.Slab.AArch64.Run

/-!
# What a routine keeps, at every point of its run

`Mid rs s m p`: the state `p`, reached from the state `s` an attempt starts in, has
memory `m`, the same symbols, thread pointer, descriptor, stack pointer and regions,
and every register but those of `rs` (the attempt's scratch registers) as in `s`.
Every state of a routine's run is one, so what follows from it (that the sequence stays
armed, the calling convention's obligations, the declared clobbers, a retry's
precondition) is proven once.
-/

@[expose] public section

namespace VG.Proof.Slab.AArch64

open VG.AArch64 VG.Spec.Slab VG.Impl.Slab.AArch64

structure Mid (rs : List Reg) (s : State) (m : Mem) (p : State) : Prop where
  mem : p.mem = m
  syms : p.syms = s.syms
  tpidr : p.tpidr = s.tpidr
  csDesc : p.csDesc = s.csDesc
  sp : p.sp = s.sp
  rd : p.rd = s.rd
  wr : p.wr = s.wr
  gpr : ∀ r, r ∉ rs → p.gpr r = s.gpr r

namespace Mid

variable {rs : List Reg} {s p : State} {m : Mem}

theorem setReg (h : Mid rs s m p) {r : Reg} (hr : r ∈ rs) (v : BitVec 64) : Mid rs s m (p.setReg r v) :=
  ⟨h.mem, h.syms, h.tpidr, h.csDesc, h.sp, h.rd, h.wr, fun r' hr' => by
    rw [RegUpd.gpr_setReg]
    split
    · subst r'; exact absurd hr hr'
    · exact h.gpr r' hr'⟩

theorem addFlags {w : Nat} (h : Mid rs s m p) (x y : BitVec w) (c : Bool) :
    Mid rs s m (p.addFlags x y c) :=
  ⟨h.mem, h.syms, h.tpidr, h.csDesc, h.sp, h.rd, h.wr, h.gpr⟩

theorem setFlags (h : Mid rs s m p) (a b c d : Bool) : Mid rs s m (p.setFlags a b c d) :=
  ⟨h.mem, h.syms, h.tpidr, h.csDesc, h.sp, h.rd, h.wr, h.gpr⟩

theorem withMem (h : Mid rs s m p) (m' : Mem) : Mid rs s m' { p with mem := m' } :=
  ⟨rfl, h.syms, h.tpidr, h.csDesc, h.sp, h.rd, h.wr, h.gpr⟩

/-- A state the CPU's steps may start in. -/
theorem base (h : Mid rs s (m1 s) p) (h10 : .x10 ∉ rs) (hr : ready s) : Base s p :=
  ⟨h.mem, h.rd, h.wr, (h.gpr _ h10).trans hr.1⟩

theorem cpu1 (h : Mid rs s m p) (h8 : .x8 ∈ rs) : Mid rs s m (cpu1 p s) := h.setReg h8 _

theorem cpu2 (h : Mid rs s m p) (h8 : .x8 ∈ rs) : Mid rs s m (cpu2 p s) :=
  (h.cpu1 h8).addFlags _ _ _

theorem cpu3 (h : Mid rs s m p) (h8 : .x8 ∈ rs) : Mid rs s m (cpu3 p s) :=
  (h.cpu2 h8).setReg h8 _

end Mid

section

variable {rs : List Reg} {s p : State} {m : Mem}

theorem mid_σA : Mid rs s (m1 s) (σA s) := ⟨rfl, rfl, rfl, rfl, rfl, rfl, rfl, fun _ _ => rfl⟩

/-- `rseq_cs` still holds the descriptor, in memory `m` that differs from memory
where the attempt started in `rseq_cs` and in regions `xs` apart from `rseq_cs` and
`__rseq_offset`. -/
theorem armed_of_mid (h : ArmOk s) {xs : List Region} (hm : Frame (⟨area s + 8, 8⟩ :: xs) s.mem m)
    (hcs : m.readW (area s + 8) 64 = s.csDesc)
    (hx : ∀ x ∈ xs, Region.Disjoint ⟨s.syms RS, 8⟩ x) (hp : Mid rs s m p) : armed p := by
  have ho : m.readW (s.syms RS) 64 = o s := by
    apply hm.readW (r := ⟨s.syms RS, 8⟩) (Region.contains_self _ _) _ (by decide)
    intro r hr
    rcases List.mem_cons.mp hr with rfl | hr
    · exact h.cs_rsVar.symm
    · exact hx r hr
  simp only [armed, rseqCsAddr, rseqArea, hp.mem, hp.tpidr, hp.syms, hp.csDesc, rseqOffsetSym]
  rw [show "__rseq_offset" = RS from rfl, ho, ← area_eq, hcs]

/-- Memory after arming differs from memory where the attempt started only in
`rseq_cs`. -/
theorem frame_m1 : Frame [⟨area s + 8, 8⟩] s.mem (m1 s) :=
  (Frame.refl _ _).writeW (List.mem_singleton_self _) _ (Region.contains_self _ _)

theorem m1_cs : (m1 s).readW (area s + 8) 64 = s.csDesc := Mem.readW_writeW_self64 _ _ _

/-- The registers AAPCS64 has the callee keep. -/
theorem abiPreserved_of_mid (hr : ∀ r ∈ calleeSaved, r ∉ rs) (hp : Mid rs s m p) : abiPreserved s p :=
  ⟨fun r hr' => hp.gpr r (hr r hr'), hp.sp⟩

/-- Registers outside `rs` are kept. -/
theorem preservedExcept_of_mid {names : List String} (hn : ∀ r ∈ rs, r.x ∈ names)
    (hp : Mid rs s m p) : preservedExcept names s p := fun r hr =>
  hp.gpr r fun h => hr (hn r h)

end

/-! ## A retry's precondition

Wherever the routine may be aborted it has kept the invariant, having written only
harmless regions (`inv_frame`); where the thread resumes, the addresses are the
attempt's (`Same`): the abort keeps the registers, the symbols and the thread pointer
(`rely`), and `env` keeps `__rseq_offset`. So the precondition holds there
(`retry_pre`), and so do the registers a sequence reads, which it does not write. -/

section Retry

variable {regs : List Reg} {c : Spec.Slab.Ctx isa}

/-- An argument, read in a state whose argument registers are another's. -/
theorem arg_eq (hA : A64 c.A regs) (hlen : (c.sig.words 64).length ≤ regs.length) {p s : State}
    (h : ∀ r ∈ regs.take (c.sig.words 64).length, p.gpr r = s.gpr r) (i : Nat) :
    c.arg p i = c.arg s i := by
  have hl : ((c.sig.words 64).map (·.bits 64)).length ≤ regs.length := by simpa using hlen
  simp only [Spec.Slab.Ctx.arg, hA.ptrBits, hA.args, hl, ↓reduceIte]
  congr 1
  exact List.map_congr_left fun r hr => h r (by simpa using hr)

/-- A state with the symbols, thread pointer, argument registers and `__rseq_offset`
of `s` has its addresses. -/
theorem same_of (hA : A64 c.A regs) (hlen : (c.sig.words 64).length ≤ regs.length) {s p : State}
    (hsyms : p.syms = s.syms) (htp : p.tpidr = s.tpidr)
    (hargs : ∀ r ∈ regs.take (c.sig.words 64).length, p.gpr r = s.gpr r)
    (hrs : ∀ x, (⟨s.syms RS, 8⟩ : Region).Contains x 1 → p.mem x = s.mem x) : Same c s p where
  sym n := by rw [sym_of hA, sym_of hA, hsyms]
  tp := by rw [tp_of hA, tp_of hA, htp]
  param n := by rw [param_of hA, param_of hA]
  arg := arg_eq hA hlen hargs
  res := by rw [hA.reserved, hA.reserved]
  rsVar x hx := by rw [mem_of hA, mem_of hA]; exact hrs x (by rwa [rsVar_of hA] at hx)

/-- Where the routine may be aborted, in a state `p` reached from `s` with memory `m`
changed only in harmless regions `W`, it has kept the invariant, and every state the
thread may resume in satisfies an attempt's precondition. -/
theorem retry (hA : A64 c.A regs) {post : State → State → Prop}
    {exit : String → State → State → Prop} {abort : State → State → Prop}
    {trace : State → List Event → Prop} {vals : State → List (BitVec 64)}
    (hv : c.A.args ((c.sig.words c.A.ptrBits).map (·.bits c.A.ptrBits)) = some vals)
    (hb : ∀ x, Sig.bufs c.sig.params (vals x) = [])
    (ha : ∀ x, c.A.argArea ((c.sig.words c.A.ptrBits).map (·.bits c.A.ptrBits)) x = [])
    (hlen : (c.sig.words 64).length ≤ regs.length) {rs : List Reg} {s p : State} {m : Mem}
    {W : List Region} (hs : c.pre s) (hr : ready s) (hm : Mid rs s m p)
    (hrs : ∀ r ∈ regs.take (c.sig.words 64).length, r ∉ rs) (h10 : .x10 ∉ rs) (h11 : .x11 ∉ rs)
    (hfr : Frame W s.mem m) (hW : ∀ w ∈ W, Harmless c s w) :
    c.inv p ∧ ∀ p', rely p p' → (withReady (contract c post exit abort trace)).env p p' →
      (withReady (contract c post exit abort trace)).pre p' := by
  obtain ⟨htp, hsym, hargs, hl, hinv⟩ := hs
  have hrsv : ∀ x, (⟨s.syms RS, 8⟩ : Region).Contains x 1 → p.mem x = s.mem x := fun x hx => by
    rw [hm.mem]
    exact hfr x fun w hw hc => (hW w hw).2.1 x hc (by rwa [rsVar_of hA])
  have hsame : Same c s p := same_of hA hlen hm.syms hm.tpidr
    (fun r hr => hm.gpr r (hrs r hr)) hrsv
  have hfr' : Frame W (c.mem s) (c.mem p) := by rw [mem_of hA, mem_of hA, hm.mem]; exact hfr
  have hip := inv_frame hsame hfr' hW (cls_lt hargs) hinv
  refine ⟨hip, fun p' hr' he => ?_⟩
  obtain ⟨hg, -, -, -, -, -, htp', hsy, hcs⟩ := hr'
  have hkept := he.2.2.1 (c.rsVar p) (List.mem_cons_self ..)
  have hsame' : Same c s p' := same_of hA hlen (hsy.trans hm.syms) (htp'.trans hm.tpidr)
    (fun r hr => by rw [hg]; exact hm.gpr r (hrs r hr)) fun x hx => by
      have h1 := hkept x (by rwa [hsame.rsVar_eq, rsVar_of hA])
      rw [mem_of hA, mem_of hA] at h1
      rw [h1]; exact hrsv x hx
  have hwf : c.A.wf ((c.sig.words c.A.ptrBits).map (·.bits c.A.ptrBits)) 0 p' := by
    rw [hA.wf]; trivial
  have harea : area p' = area s := by
    rw [← area_of hA, ← area_of hA]; exact hsame'.area
  exact ⟨retry_pre c hv (hb p') (ha p') hA.ptrBits hwf ⟨htp, hsym, hargs, hl, hinv⟩ hsame' hip he,
    by rw [harea, hg, hm.gpr _ h10]; exact hr.1,
    by rw [hg, hm.gpr _ h11, hcs, hm.csDesc]; exact hr.2⟩

/-- `rseq_cs`, which the arm writes, is harmless. -/
theorem harmless_cs1 (hA : A64 c.A regs) {s : State} (hs : c.pre s) :
    ∀ w ∈ [(⟨area s + 8, 8⟩ : Region)], Harmless c s w := by
  intro w hw
  rw [List.mem_singleton.mp hw, ← csField_of hA]
  exact harmless_cs hs.2.2.2.1

/-- `rseq_cs` and another harmless region. -/
theorem harmless_cs2 (hA : A64 c.A regs) {s : State} (hs : c.pre s) {r : Region} (hr : Harmless c s r) :
    ∀ w ∈ [(⟨area s + 8, 8⟩ : Region), r], Harmless c s w := by
  intro w hw
  rcases List.mem_cons.mp hw with rfl | hw
  · rw [← csField_of hA]; exact harmless_cs hs.2.2.2.1
  · rw [List.mem_singleton.mp hw]; exact hr

end Retry

end VG.Proof.Slab.AArch64
