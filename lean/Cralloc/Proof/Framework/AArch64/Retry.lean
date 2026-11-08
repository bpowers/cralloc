module

public import Cralloc.TCB.AArch64.Target
public import Cralloc.Proof.Framework.Rseq

/-!
# AArch64: what a routine's retries keep of an attempt's start

As on x86-64 (`Proof/Framework/X86_64/Retry.lean`): `Verified.restarts` says that
the last attempt of a routine, aborted any number of times, meets the contract
relative to the state it starts in. Here that state is tied to the first
attempt's: out of line, the callee-saved registers, the stack pointer and the
argument registers a routine keeps wherever it may be aborted; inline, every
register but the clobbers. The return address is in `x30`, so nothing of memory
need be kept, as it must on x86-64.
-/

@[expose] public section

namespace VG.AArch64

theorem abiPreserved_trans {s₁ s₂ s₃ : State} (h₁ : abiPreserved s₁ s₂) (h₂ : abiPreserved s₂ s₃) :
    abiPreserved s₁ s₃ :=
  ⟨fun r hr => by rw [h₂.1 r hr, h₁.1 r hr], by rw [h₂.2, h₁.2]⟩

theorem preservedExcept_trans {cl : List String} {s₁ s₂ s₃ : State} (h₁ : preservedExcept cl s₁ s₂)
    (h₂ : preservedExcept cl s₂ s₃) : preservedExcept cl s₁ s₃ := fun r hr => by
  rw [h₂ r hr, h₁ r hr]

theorem preservedExcept_mono {cl cl' : List String} {s s' : State} (h : preservedExcept cl s s')
    (hs : ∀ n ∈ cl, n ∈ cl') : preservedExcept cl' s s' := fun r hr => h r fun h' => hr (hs _ h')

/-- Out of line: the last attempt starts with the callee-saved registers, the
stack pointer and the registers `rs` as the first did, if the routine keeps `rs`
wherever it may be aborted. -/
theorem restarts_target {c : Prog isa} {k : Contract isa} (h : Verified target c k)
    {rs : List Reg} (hargs : ∀ s, k.pre s → ∀ p ∈ c.abortStates isa s, ∀ r ∈ rs, p.gpr r = s.gpr r)
    {s s' : State} (hs : k.pre s) (hr : Restarts target c k s s') :
    abiPreserved s s' ∧ ∀ r ∈ rs, s'.gpr r = s.gpr r := by
  refine h.restarts_keep (fun s s' => abiPreserved s s' ∧ ∀ r ∈ rs, s'.gpr r = s.gpr r)
    (fun _ => ⟨⟨fun _ _ => rfl, rfl⟩, fun _ _ => rfl⟩) ?_ hs hr
  intro s p p' s'' hs hp hrely _ _ ⟨hab', hrs⟩
  have hab : abiPreserved s p := (h.abortOk hs hp).2.1
  have hg : p'.gpr = p.gpr := hrely.1
  have hsp : p'.sp = p.sp := hrely.2.1
  have hpp' : abiPreserved p p' := ⟨fun r _ => by rw [hg], hsp⟩
  exact ⟨abiPreserved_trans hab (abiPreserved_trans hpp' hab'),
    fun r hr => by rw [hrs r hr, hg, hargs s hs p hp r hr]⟩

/-- An inline target, of the calling convention `A` and the result register `ret`:
`inlineTarget` and `inlineTargetX4` are two. -/
abbrev inlineFor (A : Abi isa) (ret : String) : Target := { inlineTarget with abi := A, retReg := ret }

/-- Inline: the last attempt starts with every register but the clobbers `cl` as
the first did, and the stack pointer, if the routine keeps them wherever it may
be aborted (`Artifact.clobbersOk`). -/
theorem restarts_inline {A : Abi isa} {ret : String} {c : Prog isa} {k : Contract isa}
    (h : Verified (inlineFor A ret) c k)
    {cl : List String} (hcl : ∀ s, k.pre s → ∀ p ∈ c.abortStates isa s, preservedExcept cl s p)
    {s s' : State} (hs : k.pre s) (hr : Restarts (inlineFor A ret) c k s s') :
    preservedExcept cl s s' ∧ s'.sp = s.sp := by
  refine h.restarts_keep (fun s s' => preservedExcept cl s s' ∧ s'.sp = s.sp)
    (fun _ => ⟨fun _ _ => rfl, rfl⟩) ?_ hs hr
  intro s p p' s'' hs hp hrely _ _ ⟨hpe, hsp⟩
  have hab : p.sp = s.sp := (h.abortOk hs hp).2.1
  have hg : p'.gpr = p.gpr := hrely.1
  have hs' : p'.sp = p.sp := hrely.2.1
  exact ⟨preservedExcept_trans (hcl s hs p hp) (fun r hr => by rw [hpe r hr, hg]),
    by rw [hsp, hs', hab]⟩

/-! ## As `Artifact.restartsOk` states it -/

/-- Arguments in the argument registers, which `s'` holds as `s` does. -/
theorem argsKept_of {sig : Sig} (hlen : (sig.words 64).length ≤ 8) {s s' : State}
    (h : ∀ r ∈ argRegs.take (sig.words 64).length, s'.gpr r = s.gpr r) : ArgsKept abi sig s s' := by
  intro vals hv
  have hl : ((sig.words 64).map (·.bits 64)).length ≤ argRegs.length := by
    simpa [argRegs] using hlen
  have ha : abi.args ((sig.words 64).map (·.bits 64)) =
      some fun s => (argRegs.take ((sig.words 64).map (·.bits 64)).length).map s.gpr := by
    simp only [abi, hl, ↓reduceIte]
  have hv' : abi.args ((sig.words 64).map (·.bits 64)) = some vals := hv
  rw [ha] at hv'
  cases hv'
  exact List.map_congr_left fun r hr => h r (by simpa using hr)

/-- `Artifact.restartsOk` out of line, from what `restarts_target` needs. -/
theorem restartsOk_target {c : Prog isa} {k : Contract isa} {sig : Sig} {cl : List String}
    (h : Verified target c k) (hlen : (sig.words 64).length ≤ 8)
    (hargs : ∀ s, k.pre s → ∀ p ∈ c.abortStates isa s, ∀ r ∈ argRegs.take (sig.words 64).length,
      p.gpr r = s.gpr r) :
    ∀ s s', k.pre s → Restarts target c k s s' →
      target.abiPreserved s s' ∧ ArgsKept target.abi sig s s' ∧
      (target.inline = true → target.preservedExcept cl s s') := fun s s' hs hr =>
  have ⟨hab, hrs⟩ := restarts_target h hargs hs hr
  ⟨hab, argsKept_of hlen hrs, fun h' => absurd h' (by decide)⟩

/-- `Artifact.restartsOk` inline, from what `restarts_inline` needs: what the
clobbers `cl` leave keeps the arguments. -/
theorem restartsOk_inline {A : Abi isa} {ret : String} {c : Prog isa} {k : Contract isa} {sig : Sig}
    {cl : List String} (h : Verified (inlineFor A ret) c k)
    (hcl : ∀ s, k.pre s → ∀ p ∈ c.abortStates isa s, preservedExcept cl s p)
    (hargs : ∀ s s', preservedExcept cl s s' → ArgsKept A sig s s') :
    ∀ s s', k.pre s → Restarts (inlineFor A ret) c k s s' →
      (inlineFor A ret).abiPreserved s s' ∧ ArgsKept (inlineFor A ret).abi sig s s' ∧
      ((inlineFor A ret).inline = true → (inlineFor A ret).preservedExcept cl s s') := fun s s' hs hr =>
  have ⟨hpe, hsp⟩ := restarts_inline h hcl hs hr
  ⟨hsp, hargs s s' hpe, fun _ => hpe⟩

end VG.AArch64
