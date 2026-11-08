module

public import Cralloc.Proof.Slab.AArch64.Mid
public import Cralloc.Proof.Framework.AArch64.Retry

/-!
# What every routine runs once

`Impl.Slab.AArch64.setup`, and `muslSetup` on musl, leave the descriptor's address in `x11` and
the rseq area's in `x10`, in `x9` at most what they no longer need, and all else as it was
(`AreaSetup`), so that the state they leave has the addresses and memory of the function's entry
state, and so its precondition, and `ready` (`setup_ok`).
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

/-- What a routine's proof asks of its setup `l`, which runs a state `s` to `g s`, however it finds
the rseq area. -/
structure AreaSetup (l : List Instr) (g : State → State) : Prop where
  run : ∀ {s : State}, (⟨s.syms RS, 8⟩ : Region) ∈ s.rd → execBlock isa l s = some (g s)
  keeps : ∀ {s : State} {r : Reg}, ¬ r ∈ [Reg.x9, .x10, .x11] → (g s).gpr r = s.gpr r
  ready : ∀ {s : State}, ready (g s)
  mem : ∀ {s : State}, (g s).mem = s.mem
  rd : ∀ {s : State}, (g s).rd = s.rd
  wr : ∀ {s : State}, (g s).wr = s.wr
  syms : ∀ {s : State}, (g s).syms = s.syms
  tpidr : ∀ {s : State}, (g s).tpidr = s.tpidr
  sp : ∀ {s : State}, (g s).sp = s.sp

theorem setupArea : AreaSetup setup s6 :=
  ⟨execBlock_setup, s6_keeps, s6_ready, rfl, rfl, rfl, rfl, rfl, rfl⟩

/-- The clobbers of an inline routine name what the setup writes. -/
theorem AreaSetup.preserved {l : List Instr} {g : State → State} (hl : AreaSetup l g) {s : State}
    {cl : List String} (h : ∀ n ∈ ["x9", "x10", "x11"], n ∈ cl) : preservedExcept cl s (g s) :=
  fun r hr => hl.keeps fun hm => hr (h _ (by
    simp only [List.mem_cons, List.not_mem_nil, or_false] at hm
    rcases hm with rfl | rfl | rfl <;> simp [Reg.x]))

/-! ## On musl -/

section Musl

variable {s : State}

/-- The state `muslSetup` leaves. -/
def m3 (s : State) : State := (s2 s).setReg .x10 s.tpidr
def m4 (s : State) : State := (m3 s).setReg .x10 (s.tpidr + page (o s))
def m5 (s : State) : State := (m4 s).setReg .x10 (s.tpidr + o s)

theorem execBlock_muslSetup : execBlock isa muslSetup s = some (m5 s) := by
  have h1 : exec (.adrp .x11 .desc) s = some (s.setReg .x11 (page s.csDesc)) := rfl
  have h2 : exec (.addLo12 .x11 .x11 .desc) (s.setReg .x11 (page s.csDesc)) = some (s2 s) := by
    show some ((s.setReg .x11 (page s.csDesc)).setReg .x11
      ((s.setReg .x11 (page s.csDesc)).gpr .x11 + lo12 s.csDesc)) = _
    rw [RegUpd.gpr_setReg_self, Access.page_add_lo12]
    rfl
  have h3 : exec (.mrsTp .x10) (s2 s) = some (m3 s) := rfl
  have h4 : exec (.addAreaHi .x10 .x10) (m3 s) = some (m4 s) := by
    rw [Access.exec_addAreaHi, m3, RegUpd.gpr_setReg_self]
    rfl
  have h5 : exec (.addAreaLo .x10 .x10) (m4 s) = some (m5 s) := by
    rw [Access.exec_addAreaLo, m4, RegUpd.gpr_setReg_self]
    have ho : ((m3 s).setReg .x10 (s.tpidr + page (o s))).areaOffset = o s := rfl
    rw [ho, BitVec.add_assoc, Access.page_add_lo12]
    rfl
  rw [muslSetup, execBlock_cons_some h1, execBlock_cons_some h2, execBlock_cons_some h3,
    execBlock_cons_some h4, execBlock_cons_some h5, execBlock_nil']

theorem m5_keeps {r : Reg} (h : ¬ r ∈ [Reg.x9, .x10, .x11]) : (m5 s).gpr r = s.gpr r := by
  have h10 : ¬ r = .x10 := fun h' => h (by simp [h'])
  have h11 : ¬ r = .x11 := fun h' => h (by simp [h'])
  rw [m5, RegUpd.gpr_setReg_of_ne _ _ h10, m4, RegUpd.gpr_setReg_of_ne _ _ h10, m3,
    RegUpd.gpr_setReg_of_ne _ _ h10, s2, RegUpd.gpr_setReg_of_ne _ _ h11,
    RegUpd.gpr_setReg_of_ne _ _ h11]

theorem m5_ready : ready (m5 s) := by
  refine ⟨?_, ?_⟩
  · rw [m5, RegUpd.gpr_setReg_self]; rfl
  · rw [m5, RegUpd.gpr_setReg_of_ne _ _ (by decide), m4, RegUpd.gpr_setReg_of_ne _ _ (by decide), m3,
      RegUpd.gpr_setReg_of_ne _ _ (by decide), s2, RegUpd.gpr_setReg_self]
    rfl

end Musl

theorem muslArea : AreaSetup muslSetup m5 :=
  ⟨fun _ => execBlock_muslSetup, m5_keeps, m5_ready, rfl, rfl, rfl, rfl, rfl, rfl⟩

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
theorem setup_ok {l : List Instr} {g : State → State} (hl : AreaSetup l g)
    {regs : List Reg} {c : Spec.Slab.Ctx isa} (hA : A64 c.A regs)
    {post : State → State → Prop} {exit : String → State → State → Prop}
    {abort : State → State → Prop} {trace : State → List Event → Prop}
    {vals : State → List (BitVec 64)}
    (hv : c.A.args ((c.sig.words c.A.ptrBits).map (·.bits c.A.ptrBits)) = some vals)
    (hb : ∀ x, Sig.bufs c.sig.params (vals x) = [])
    (ha : ∀ x, c.A.argArea ((c.sig.words c.A.ptrBits).map (·.bits c.A.ptrBits)) x = [])
    (hlen : (c.sig.words 64).length ≤ regs.length)
    (hargs : ∀ r ∈ regs.take (c.sig.words 64).length, ¬ r ∈ [Reg.x9, .x10, .x11]) {s : State}
    (hp : (contract c post exit abort trace).pre s) :
    execBlock isa l s = some (g s) ∧ (withReady (contract c post exit abort trace)).pre (g s) := by
  have hw := ((pre_iff c hv (hb s) (ha s)).mp hp).2.1
  have hrs : (⟨s.syms RS, 8⟩ : Region) ∈ s.rd := by
    have h := hw.rd
    rw [hA.rd] at h
    rw [h]
    refine List.mem_map.mpr ⟨(c.rsVar s, false), List.mem_filter.mpr ⟨?_, rfl⟩, by rw [rsVar_of hA]⟩
    exact List.mem_append_left _ rsVar_mem_fixed
  have hsame : Same c s (g s) := same_of hA hlen hl.syms hl.tpidr (fun r hr => hl.keeps (hargs r hr))
    fun _ _ => by rw [hl.mem]
  refine ⟨hl.run hrs, pre_of_same hv hb ha ?_ hp hsame ?_ ?_ ?_, hl.ready⟩
  · rw [hA.wf]; trivial
  · rw [mem_of hA, mem_of hA]; exact hl.mem
  · rw [hA.rd, hA.rd]; exact hl.rd
  · rw [hA.wr, hA.wr]; exact hl.wr

end VG.Proof.Slab.AArch64
