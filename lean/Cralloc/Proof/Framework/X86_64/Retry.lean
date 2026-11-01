module

public import Cralloc.TCB.X86_64.Target
public import Cralloc.Proof.Framework.Rseq

/-!
# x86-64: what a routine's retries keep of the caller's entry state

`Verified.restarts` says that the last attempt of a routine, aborted any
number of times, meets the contract relative to the state it starts in.
Here that state is tied to the caller's: out of line, the callee-saved
registers, the stack pointer and the return address are the caller's, and
so are the argument registers a routine keeps wherever it may be aborted,
if `env` keeps the return address's slot; inline, every register but the
clobbers. So the last attempt's return meets the calling convention's (or
`asm!`'s) obligations relative to the caller's entry.
-/

@[expose] public section

namespace VG.X86_64

theorem abiPreserved_trans {s₁ s₂ s₃ : State} (h₁ : abiPreserved s₁ s₂) (h₂ : abiPreserved s₂ s₃) :
    abiPreserved s₁ s₃ := by
  have hsp : s₂.gpr .rsp = s₁.gpr .rsp := h₁.1 .rsp (by decide)
  refine ⟨fun r hr => by rw [h₂.1 r hr, h₁.1 r hr], ?_⟩
  have := h₂.2
  rw [hsp] at this
  rw [this, h₁.2]

theorem preservedExcept_trans {cl : List String} {s₁ s₂ s₃ : State} (h₁ : preservedExcept cl s₁ s₂)
    (h₂ : preservedExcept cl s₂ s₃) : preservedExcept cl s₁ s₃ := fun r hr => by
  rw [h₂ r hr, h₁ r hr]

theorem preservedExcept_mono {cl cl' : List String} {s s' : State} (h : preservedExcept cl s s')
    (hs : ∀ n ∈ cl, n ∈ cl') : preservedExcept cl' s s' := fun r hr => h r fun h' => hr (hs _ h')

/-- Out of line: the last attempt starts with the callee-saved registers,
the stack pointer, the return address and the registers `rs` as on entry,
if the routine keeps `rs` wherever it may be aborted and `env` keeps the
return address's slot. -/
theorem restarts_target {c : Prog isa} {k : Contract isa} (h : Verified target c k)
    {rs : List Reg} (hargs : ∀ s, k.pre s → ∀ p ∈ c.abortStates isa s, ∀ r ∈ rs, p.gpr r = s.gpr r)
    (hret : ∀ p p', k.env p p' → p'.mem.readW (p.gpr .rsp) 64 = p.mem.readW (p.gpr .rsp) 64)
    {s s' : State} (hs : k.pre s) (hr : Restarts target c k s s') :
    abiPreserved s s' ∧ ∀ r ∈ rs, s'.gpr r = s.gpr r := by
  refine h.restarts_keep (fun s s' => abiPreserved s s' ∧ ∀ r ∈ rs, s'.gpr r = s.gpr r)
    (fun _ => ⟨⟨fun _ _ => rfl, rfl⟩, fun _ _ => rfl⟩) ?_ hs hr
  intro s p p' s'' hs hp hrely henv _ ⟨hab', hrs⟩
  have hab : abiPreserved s p := (h.abortOk hs hp).2.1
  have hg : p'.gpr = p.gpr := hrely.1
  have hpp' : abiPreserved p p' := ⟨fun r _ => by rw [hg], hret p p' henv⟩
  exact ⟨abiPreserved_trans hab (abiPreserved_trans hpp' hab'),
    fun r hr => by rw [hrs r hr, hg, hargs s hs p hp r hr]⟩

/-- Out of line, the whole of a routine's run: its last attempt starts in a
state satisfying the precondition, with the registers `rs` as on entry, and
returns meeting the calling convention's obligations relative to the
caller's entry and the postcondition relative to the attempt's start, or
exits meeting `exit`. -/
theorem restarts_target_run {c : Prog isa} {k : Contract isa} (h : Verified target c k)
    {rs : List Reg} (hargs : ∀ s, k.pre s → ∀ p ∈ c.abortStates isa s, ∀ r ∈ rs, p.gpr r = s.gpr r)
    (hret : ∀ p p', k.env p p' → p'.mem.readW (p.gpr .rsp) 64 = p.mem.readW (p.gpr .rsp) 64)
    {s s' : State} (hs : k.pre s) (hr : Restarts target c k s s') :
    k.pre s' ∧ (∀ r ∈ rs, s'.gpr r = s.gpr r) ∧
      ((∃ s'', Exec isa c s' s'' ∧ abiPreserved s s'' ∧ k.post s' s'') ∨
        (∃ t s'', ExecExit isa c s' t s'' ∧ abiPreserved s s'' ∧ k.exit t s' s'')) := by
  obtain ⟨hab, hrs⟩ := restarts_target h hargs hret hs hr
  obtain ⟨hs', hrun⟩ := h.restarts hs hr
  refine ⟨hs', hrs, ?_⟩
  rcases hrun with ⟨s'', he, ha, hp⟩ | ⟨t, s'', he, ha, hx⟩
  · exact .inl ⟨s'', he, abiPreserved_trans hab ha, hp⟩
  · exact .inr ⟨t, s'', he, abiPreserved_trans hab ha, hx⟩

/-- Inline: the last attempt starts with every register but the clobbers
`cl` as on entry, if the routine keeps them wherever it may be aborted
(`Artifact.clobbersOk`). -/
theorem restarts_inline {c : Prog isa} {k : Contract isa} (h : Verified inlineTarget c k)
    {cl : List String} (hcl : ∀ s, k.pre s → ∀ p ∈ c.abortStates isa s, preservedExcept cl s p)
    {s s' : State} (hs : k.pre s) (hr : Restarts inlineTarget c k s s') :
    preservedExcept cl s s' ∧ s'.gpr .rsp = s.gpr .rsp := by
  refine h.restarts_keep (fun s s' => preservedExcept cl s s' ∧ s'.gpr .rsp = s.gpr .rsp)
    (fun _ => ⟨fun _ _ => rfl, rfl⟩) ?_ hs hr
  intro s p p' s'' hs hp hrely _ _ ⟨hpe, hsp⟩
  have hab : p.gpr .rsp = s.gpr .rsp := (h.abortOk hs hp).2.1
  have hg : p'.gpr = p.gpr := hrely.1
  exact ⟨preservedExcept_trans (hcl s hs p hp) (fun r hr => by rw [hpe r hr, hg]),
    by rw [hsp, hg, hab]⟩

/-- Inline, the whole of a routine's run: its last attempt starts in a state
satisfying the precondition, and ends, or exits, with every register but
the clobbers `cl` (and, if it completes, those of `cl'`) as on entry and
the stack pointer as on entry, given that every run that is not aborted
keeps them (`Artifact.clobbersOk`). -/
theorem restarts_inline_run {c : Prog isa} {k : Contract isa} (h : Verified inlineTarget c k)
    {cl cl' : List String} (hsub : ∀ n ∈ cl, n ∈ cl')
    (hcl : ∀ s, k.pre s → (∀ s', Exec isa c s s' → preservedExcept cl' s s') ∧
      (∀ t s', ExecExit isa c s t s' → preservedExcept cl s s') ∧
      (∀ p ∈ c.abortStates isa s, preservedExcept cl s p))
    {s s' : State} (hs : k.pre s) (hr : Restarts inlineTarget c k s s') :
    k.pre s' ∧
      ((∃ s'', Exec isa c s' s'' ∧ preservedExcept cl' s s'' ∧ s''.gpr .rsp = s.gpr .rsp ∧
          k.post s' s'') ∨
        (∃ t s'', ExecExit isa c s' t s'' ∧ preservedExcept cl s s'' ∧ s''.gpr .rsp = s.gpr .rsp ∧
          k.exit t s' s'')) := by
  obtain ⟨hpe, hsp⟩ := restarts_inline h (fun s hs => (hcl s hs).2.2) hs hr
  obtain ⟨hs', hrun⟩ := h.restarts hs hr
  refine ⟨hs', ?_⟩
  rcases hrun with ⟨s'', he, ha, hp⟩ | ⟨t, s'', he, ha, hx⟩
  · exact .inl ⟨s'', he, preservedExcept_trans (preservedExcept_mono hpe hsub) ((hcl s' hs').1 s'' he),
      by rw [show s''.gpr .rsp = s'.gpr .rsp from ha, hsp], hp⟩
  · exact .inr ⟨t, s'', he, preservedExcept_trans hpe ((hcl s' hs').2.1 t s'' he),
      by rw [show s''.gpr .rsp = s'.gpr .rsp from ha, hsp], hx⟩

/-! ## As `Artifact.restartsOk` states it -/

/-- Arguments in the argument registers, which `s'` holds as `s` does. -/
theorem argsKept_of {sig : Sig} (hlen : (sig.words 64).length ≤ 6) {s s' : State}
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
    (h : Verified target c k) (hlen : (sig.words 64).length ≤ 6)
    (hargs : ∀ s, k.pre s → ∀ p ∈ c.abortStates isa s, ∀ r ∈ argRegs.take (sig.words 64).length,
      p.gpr r = s.gpr r)
    (hret : ∀ p p', k.env p p' → p'.mem.readW (p.gpr .rsp) 64 = p.mem.readW (p.gpr .rsp) 64) :
    ∀ s s', k.pre s → Restarts target c k s s' →
      target.abiPreserved s s' ∧ ArgsKept target.abi sig s s' ∧
      (target.inline = true → target.preservedExcept cl s s') := fun s s' hs hr =>
  have ⟨hab, hrs⟩ := restarts_target h hargs hret hs hr
  ⟨hab, argsKept_of hlen hrs, fun h' => absurd h' (by decide)⟩

/-- `Artifact.restartsOk` inline, from what `restarts_inline` needs: the
argument registers are none of the clobbers. -/
theorem restartsOk_inline {c : Prog isa} {k : Contract isa} {sig : Sig} {cl : List String}
    (h : Verified inlineTarget c k) (hlen : (sig.words 64).length ≤ 6)
    (hcl : ∀ s, k.pre s → ∀ p ∈ c.abortStates isa s, preservedExcept cl s p)
    (hargs : ∀ r ∈ argRegs.take (sig.words 64).length, r.name ∉ cl) :
    ∀ s s', k.pre s → Restarts inlineTarget c k s s' →
      inlineTarget.abiPreserved s s' ∧ ArgsKept inlineTarget.abi sig s s' ∧
      (inlineTarget.inline = true → inlineTarget.preservedExcept cl s s') := fun s s' hs hr =>
  have ⟨hpe, hsp⟩ := restarts_inline h hcl hs hr
  ⟨hsp, (argsKept_of hlen fun r hr => hpe r (hargs r hr) : ArgsKept abi sig s s'), fun _ => hpe⟩

end VG.X86_64
