module

public import Asmalloc.Proof.Slab.X86_64.Run

/-!
# What a routine keeps, at every point of its run

`Mid rs s m p`: the state `p`, reached from the entry state `s`, has memory
`m`, the same symbols, constants, thread pointer, descriptor and regions, and
every register but those of `rs` (the routine's scratch registers) as in
`s`. Every state of a routine's run is one, so what follows from it (that
the sequence stays armed, the calling convention's obligations, the
declared clobbers) is proven once.
-/

@[expose] public section

namespace VG.Proof.Slab.X86_64

open VG.X86_64 VG.Spec.Slab VG.Impl.Slab.X86_64

structure Mid (rs : List Reg) (s : State) (m : Mem) (p : State) : Prop where
  mem : p.mem = m
  syms : p.syms = s.syms
  params : p.params = s.params
  fsBase : p.fsBase = s.fsBase
  csDesc : p.csDesc = s.csDesc
  rd : p.rd = s.rd
  wr : p.wr = s.wr
  gpr : ∀ r, r ∉ rs → p.gpr r = s.gpr r

namespace Mid

variable {rs : List Reg} {s p : State} {m : Mem}

theorem setReg (h : Mid rs s m p) {r : Reg} (hr : r ∈ rs) (v : BitVec 64) : Mid rs s m (p.setReg r v) :=
  ⟨h.mem, h.syms, h.params, h.fsBase, h.csDesc, h.rd, h.wr, fun r' hr' => by
    rw [RegUpd.gpr_setReg]
    split
    · subst r'; exact absurd hr hr'
    · exact h.gpr r' hr'⟩

theorem setReg32 (h : Mid rs s m p) {r : Reg} (hr : r ∈ rs) (v : BitVec 32) :
    Mid rs s m (p.setReg32 r v) := h.setReg hr _

theorem arithFlags {w : Nat} (h : Mid rs s m p) (x : BitVec w) (c o : Bool) :
    Mid rs s m (VG.X86_64.arithFlags p x c o) :=
  ⟨h.mem, h.syms, h.params, h.fsBase, h.csDesc, h.rd, h.wr, h.gpr⟩

theorem setFlags (h : Mid rs s m p) (a b c d : Option Bool) : Mid rs s m (p.setFlags a b c d) :=
  ⟨h.mem, h.syms, h.params, h.fsBase, h.csDesc, h.rd, h.wr, h.gpr⟩

theorem withMem (h : Mid rs s m p) (m' : Mem) : Mid rs s m' { p with mem := m' } :=
  ⟨rfl, h.syms, h.params, h.fsBase, h.csDesc, h.rd, h.wr, h.gpr⟩

theorem shl12 (h : Mid rs s m p) {r : Reg} (hr : r ∈ rs) : Mid rs s m (VG.Proof.Slab.X86_64.shl12 p r) :=
  (h.setFlags _ _ _ _).setReg hr _

end Mid

/-- The scratch registers of the routines, inline (`scratchI`) and out of
line (`scratchT`): the same six. -/
def scratchI : List Reg := [.rax, .rcx, .r8, .r9, .r10, .r11]
abbrev scratchT : List Reg := scratchI

section

variable {rs : List Reg} {c : Spec.Slab.Ctx isa} {s p : State} {m : Mem}

theorem mid_σA (hb : .r10 ∈ rs) (hd : .r11 ∈ rs) : Mid rs s (m1 s) (σA s) :=
  ((((Mid.mk rfl rfl rfl rfl rfl rfl rfl fun _ _ => rfl : Mid rs s s.mem s).setReg hb _).setReg
    hb _).setReg hd _).withMem _

theorem mid_σ1 (h8 : .r8 ∈ rs) (hA : Mid rs s (m1 s) (σA s)) : Mid rs s (m1 s) (σ1 s) :=
  hA.setReg32 h8 _

theorem mid_σ2 (h1 : Mid rs s (m1 s) (σ1 s)) : Mid rs s (m1 s) (σ2 s) := h1.arithFlags _ _ _

theorem mid_σ3 (h8 : .r8 ∈ rs) (h2 : Mid rs s (m1 s) (σ2 s)) : Mid rs s (m1 s) (σ3 s) := h2.shl12 h8

/-- `rseq_cs` still holds the descriptor, in memory `m` that differs from
memory on entry in `rseq_cs` and in regions `xs` apart from `rseq_cs` and
`__rseq_offset`. -/
theorem armed_of_mid (h : Facts c s) {xs : List Region} (hm : Frame (⟨area s + 8, 8⟩ :: xs) s.mem m)
    (hcs : m.readW (area s + 8) 64 = s.csDesc)
    (hx : ∀ x ∈ xs, Region.Disjoint ⟨s.syms RS, 8⟩ x) (hp : Mid rs s m p) : armed p := by
  have ho : m.readW (s.syms RS) 64 = o s := by
    apply hm.readW (r := ⟨s.syms RS, 8⟩) (Region.contains_self _ _) _ (by decide)
    intro r hr
    rcases List.mem_cons.mp hr with rfl | hr
    · exact h.cs_rsVar.symm
    · exact hx r hr
  simp only [armed, rseqCsAddr, rseqArea, hp.mem, hp.fsBase, hp.syms, hp.csDesc, rseqOffsetSym]
  rw [show "__rseq_offset" = RS from rfl, ho, ← area_eq, hcs]

/-- Memory after arming differs from memory on entry only in `rseq_cs`. -/
theorem frame_m1 : Frame [⟨area s + 8, 8⟩] s.mem (m1 s) :=
  (Frame.refl _ _).writeW (List.mem_singleton_self _) _ (Region.contains_self _ _)

theorem m1_cs : (m1 s).readW (area s + 8) 64 = s.csDesc := Mem.readW_writeW_self64 _ _ _

/-- The registers the System V convention has the callee keep, and the return
address, in memory differing only in regions apart from it. -/
theorem abiPreserved_of_mid {xs : List Region} (hm : Frame xs s.mem m)
    (hx : ∀ x ∈ xs, Region.Disjoint x ⟨s.gpr .rsp, 8⟩) (hr : ∀ r ∈ calleeSaved, r ∉ rs)
    (hp : Mid rs s m p) : abiPreserved s p := by
  refine ⟨fun r hr' => hp.gpr r (hr r hr'), ?_⟩
  rw [hp.mem]
  exact hm.readW (r := ⟨s.gpr .rsp, 8⟩) (Region.contains_self _ _)
    (fun x hx' => (hx x hx').symm) (by decide)

/-- Two words written, in two regions. -/
theorem frame_two (m : Mem) (a b : Addr) (v w : BitVec 64) :
    Frame [⟨a, 8⟩, ⟨b, 8⟩] m ((m.writeW a v).writeW b w) :=
  ((Frame.refl _ _).writeW (r := ⟨a, 8⟩) (List.mem_cons_self ..) _ (Region.contains_self _ _)).writeW
    (r := ⟨b, 8⟩) (List.mem_cons_of_mem _ (List.mem_singleton_self _)) _ (Region.contains_self _ _)

/-- Three words written, in three regions. -/
theorem frame_three (m : Mem) (a b c : Addr) (u v w : BitVec 64) :
    Frame [⟨a, 8⟩, ⟨b, 8⟩, ⟨c, 8⟩] m (((m.writeW a u).writeW b v).writeW c w) :=
  (((Frame.refl _ _).writeW (r := ⟨a, 8⟩) (List.mem_cons_self ..) _ (Region.contains_self _ _)).writeW
    (r := ⟨b, 8⟩) (List.mem_cons_of_mem _ (List.mem_cons_self ..)) _ (Region.contains_self _ _)).writeW
    (r := ⟨c, 8⟩) (List.mem_cons_of_mem _ (List.mem_cons_of_mem _ (List.mem_singleton_self _))) _
    (Region.contains_self _ _)

/-- `asm!`'s obligation on the stack pointer. -/
theorem inline_abi (hp : Mid scratchI s m p) : inlineTarget.abiPreserved s p :=
  hp.gpr .rsp (by decide)

/-- Registers outside `rs` are kept. -/
theorem preservedExcept_of_mid {names : List String} (hn : ∀ r ∈ rs, r.name ∈ names)
    (hp : Mid rs s m p) : preservedExcept names s p := fun r hr =>
  hp.gpr r fun h => hr (hn r h)

end

/-! ## A retry's precondition

`env` gives the world a retry starts in; the routine's arguments and
constants, the rest of the precondition, are those the routine kept. -/

section Retry

variable {c : Spec.Slab.Ctx isa}

/-- An argument, read in a state whose argument registers are another's. -/
theorem arg_eq (hA : c.A = abi) (hlen : (c.sig.words 64).length ≤ 6) {p s : State}
    (h : ∀ r ∈ argRegs.take (c.sig.words 64).length, p.gpr r = s.gpr r) (i : Nat) :
    c.arg p i = c.arg s i := by
  obtain ⟨A, f, sig⟩ := c
  simp only at hA
  subst hA
  have hl : ((sig.words 64).map (·.bits 64)).length ≤ argRegs.length := by
    simpa [argRegs] using hlen
  have hargs : abi.args ((sig.words 64).map (·.bits 64)) =
      some fun s => (argRegs.take ((sig.words 64).map (·.bits 64)).length).map s.gpr := by
    simp only [abi, hl, ↓reduceIte]
  simp only [Spec.Slab.Ctx.arg, abi_ptrBits, hargs]
  congr 1
  exact List.map_congr_left fun r hr => h r (by simpa using hr)

/-- The arguments' and constants' facts, in a state with the same. -/
theorem argsOk_of (hA : c.A = abi) {p s : State} (hpar : p.params = s.params)
    (harg : ∀ i, c.arg p i = c.arg s i) : c.argsOk p ↔ c.argsOk s := by
  obtain ⟨A, f, sig⟩ := c
  simp only at hA
  subst hA
  cases f <;> simp only [Spec.Slab.Ctx.argsOk, Spec.Slab.Ctx.cls, Spec.Slab.Ctx.mask, view_param,
    harg, hpar]

/-- A retry's precondition holds: from `env`, and the arguments and
constants the routine and the abort kept. -/
theorem retry (hA : c.A = abi) {post : State → State → Prop}
    {exit : String → State → State → Prop} {abort : State → State → Prop}
    {trace : State → List Event → Prop} {vals : State → List (BitVec 64)}
    (hv : c.A.args ((c.sig.words c.A.ptrBits).map (·.bits c.A.ptrBits)) = some vals)
    (hb : ∀ x, Sig.bufs c.sig.params (vals x) = [])
    (ha : ∀ x, c.A.argArea ((c.sig.words c.A.ptrBits).map (·.bits c.A.ptrBits)) x = [])
    (hlen : (c.sig.words 64).length ≤ 6) {rs : List Reg} {s p p' : State} {m : Mem}
    (hs : c.pre s) (hm : Mid rs s m p) (hrs : ∀ r ∈ argRegs.take (c.sig.words 64).length, r ∉ rs) (hr : rely p p')
    (he : (contract c post exit abort trace).env p p') : (contract c post exit abort trace).pre p' := by
  obtain ⟨htp, hsym, hargs, -⟩ := hs
  obtain ⟨hg, -, -, -, -, -, -, hpar, -, -⟩ := hr
  refine pre_of_env c hv (hb p') (ha p') he htp hsym ((argsOk_of hA ?_ ?_).mpr hargs)
  · rw [hpar, hm.params]
  · exact arg_eq hA hlen fun r hr' => by rw [hg, hm.gpr r (hrs r hr')]

end Retry

end VG.Proof.Slab.X86_64
