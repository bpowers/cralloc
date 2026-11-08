module

public import Cralloc.Proof.Slab.AArch64.Mid
public import Cralloc.Proof.Framework.AArch64.Retry

/-!
# What every routine runs once

`Impl.Slab.AArch64.setup` leaves the descriptor's address in `x11` and the rseq area's in
`x10`, in `x9` what it no longer needs, and all else as it was, so that the state it
leaves has the addresses and memory of the function's entry state, and so its
precondition, and `ready` (`setup_ok`).
-/

@[expose] public section

namespace VG.Proof.Slab.AArch64

open VG.AArch64 VG.Spec.Slab VG.Impl.Slab.AArch64

section Steps

variable {s : State}

/-- The address of the descriptor, in `x11`. -/
def s2 (s : State) : State := (s.setReg .x11 (page s.csDesc)).setReg .x11 s.csDesc
def s3 (s : State) : State := (s2 s).setReg .x9 (s.syms RS)
def s4 (s : State) : State := (s3 s).setReg .x9 (o s)
def s5 (s : State) : State := (s4 s).setReg .x10 s.tpidr
/-- The state the setup leaves: the rseq area's address in `x10`. -/
def s6 (s : State) : State := (s5 s).setReg .x10 (s.tpidr + o s)

theorem exec_ldrWord {σ : State} {d n : Reg} {name : String} (h : σ.gpr n = σ.csDesc) :
    exec (.ldrWord d n name) σ = some (σ.setReg d (σ.syms name)) := by
  simp only [exec, h, ↓reduceIte]

theorem execBlock_setup (h : ⟨s.syms RS, 8⟩ ∈ s.rd) : execBlock isa setup s = some (s6 s) := by
  have h1 : exec (.adrp .x11 .desc) s = some (s.setReg .x11 (page s.csDesc)) := rfl
  have h2 : exec (.addLo12 .x11 .x11 .desc) (s.setReg .x11 (page s.csDesc)) = some (s2 s) := by
    show some ((s.setReg .x11 (page s.csDesc)).setReg .x11
      ((s.setReg .x11 (page s.csDesc)).gpr .x11 + lo12 s.csDesc)) = _
    rw [RegUpd.gpr_setReg_self, Access.page_add_lo12]
    rfl
  have h3 : exec (.ldrWord .x9 .x11 RS) (s2 s) = some (s3 s) :=
    exec_ldrWord (RegUpd.gpr_setReg_self _ _ _)
  have hea : (s3 s).ea { base := .x9 } = s.syms RS := by
    rw [Access.ea_base, s3, RegUpd.gpr_setReg_self]
  have h4 : exec (.ldr .x .x9 { base := .x9 }) (s3 s) = some (s4 s) := by
    rw [Access.exec_ldr_x (by rw [hea]; exact inRd (r := ⟨s.syms RS, 8⟩) h), hea]
    rfl
  have h5 : exec (.mrsTp .x10) (s4 s) = some (s5 s) := rfl
  have h6 : exec (.add .x .x10 .x10 (.reg .x9)) (s5 s) = some (s6 s) := by
    show some ((s5 s).setReg .x10 ((s5 s).gpr .x10 + (s5 s).gpr .x9)) = _
    rw [s5, RegUpd.gpr_setReg_self, RegUpd.gpr_setReg_of_ne _ _ (by decide), s4, RegUpd.gpr_setReg_self]
    rfl
  rw [setup, execBlock_cons_some h1, execBlock_cons_some h2, execBlock_cons_some h3,
    execBlock_cons_some h4, execBlock_cons_some h5, execBlock_cons_some h6, execBlock_nil']

theorem s6_gpr {r : Reg} (h9 : ¬ r = .x9) (h10 : ¬ r = .x10) (h11 : ¬ r = .x11) :
    (s6 s).gpr r = s.gpr r := by
  rw [s6, RegUpd.gpr_setReg_of_ne _ _ h10, s5, RegUpd.gpr_setReg_of_ne _ _ h10, s4,
    RegUpd.gpr_setReg_of_ne _ _ h9, s3, RegUpd.gpr_setReg_of_ne _ _ h9, s2,
    RegUpd.gpr_setReg_of_ne _ _ h11, RegUpd.gpr_setReg_of_ne _ _ h11]

theorem s6_ready : ready (s6 s) := by
  refine ⟨?_, ?_⟩
  · rw [s6, RegUpd.gpr_setReg_self]; rfl
  · rw [s6, RegUpd.gpr_setReg_of_ne _ _ (by decide), s5, RegUpd.gpr_setReg_of_ne _ _ (by decide), s4,
      RegUpd.gpr_setReg_of_ne _ _ (by decide), s3, RegUpd.gpr_setReg_of_ne _ _ (by decide), s2,
      RegUpd.gpr_setReg_self]
    rfl

end Steps

/-- A register the setup writes is none of those in `h`. -/
theorem s6_keeps {s : State} {r : Reg} (h : ¬ r ∈ [Reg.x9, .x10, .x11]) : (s6 s).gpr r = s.gpr r :=
  s6_gpr (fun h' => h (by simp [h'])) (fun h' => h (by simp [h'])) (fun h' => h (by simp [h']))

/-- The clobbers of an inline routine name what the setup writes. -/
theorem s6_preserved {s : State} {cl : List String} (h : ∀ n ∈ ["x9", "x10", "x11"], n ∈ cl) :
    preservedExcept cl s (s6 s) := fun r hr =>
  s6_keeps fun hm => hr (h _ (by
    simp only [List.mem_cons, List.not_mem_nil, or_false] at hm
    rcases hm with rfl | rfl | rfl <;> simp [Reg.x]))

/-- A calling convention's arguments, in registers `s'` holds as `s` does. -/
theorem argsKept_regs {regs : List Reg} {A : Abi isa} (hA : A64 A regs) {sig : Sig}
    (hlen : (sig.words 64).length ≤ regs.length) {s s' : State}
    (h : ∀ r ∈ regs.take (sig.words 64).length, s'.gpr r = s.gpr r) : ArgsKept A sig s s' := by
  intro vals hv
  have hl : ((sig.words 64).map (·.bits 64)).length ≤ regs.length := by simpa using hlen
  rw [hA.ptrBits, hA.args] at hv
  simp only [hl, ↓reduceIte, Option.some.injEq] at hv
  subst hv
  exact List.map_congr_left fun r hr => h r (by simpa using hr)

/-- From an entry state satisfying the contract of a routine `c`, whose arguments are
in none of the registers the setup writes, the setup runs to a state where an attempt
may start. -/
theorem setup_ok {regs : List Reg} {c : Spec.Slab.Ctx isa} (hA : A64 c.A regs)
    {post : State → State → Prop} {exit : String → State → State → Prop}
    {abort : State → State → Prop} {trace : State → List Event → Prop}
    {vals : State → List (BitVec 64)}
    (hv : c.A.args ((c.sig.words c.A.ptrBits).map (·.bits c.A.ptrBits)) = some vals)
    (hb : ∀ x, Sig.bufs c.sig.params (vals x) = [])
    (ha : ∀ x, c.A.argArea ((c.sig.words c.A.ptrBits).map (·.bits c.A.ptrBits)) x = [])
    (hlen : (c.sig.words 64).length ≤ regs.length)
    (hargs : ∀ r ∈ regs.take (c.sig.words 64).length, ¬ r ∈ [Reg.x9, .x10, .x11]) {s : State}
    (hp : (contract c post exit abort trace).pre s) :
    execBlock isa setup s = some (s6 s) ∧ (withReady (contract c post exit abort trace)).pre (s6 s) := by
  have hw := ((pre_iff c hv (hb s) (ha s)).mp hp).2.1
  have hrs : (⟨s.syms RS, 8⟩ : Region) ∈ s.rd := by
    have h := hw.rd
    rw [hA.rd] at h
    rw [h]
    refine List.mem_map.mpr ⟨(c.rsVar s, false), List.mem_filter.mpr ⟨?_, rfl⟩, by rw [rsVar_of hA]⟩
    exact List.mem_append_left _ rsVar_mem_fixed
  have hsame : Same c s (s6 s) := same_of hA hlen rfl rfl (fun r hr => s6_keeps (hargs r hr))
    fun _ _ => rfl
  refine ⟨execBlock_setup hrs, pre_of_same hv hb ha ?_ hp hsame ?_ ?_ ?_, s6_ready⟩
  · rw [hA.wf]; trivial
  · rw [mem_of hA, mem_of hA]; rfl
  · rw [hA.rd, hA.rd]; rfl
  · rw [hA.wr, hA.wr]; rfl

end VG.Proof.Slab.AArch64
