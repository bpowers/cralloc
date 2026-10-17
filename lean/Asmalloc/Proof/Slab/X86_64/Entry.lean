module

public import Asmalloc.Proof.Slab.X86_64.Reg
public import Asmalloc.Proof.Framework.X86_64.Retry

/-!
# The routines, aborted any number of times, against the caller's entry

Each routine's last attempt meets its contract relative to the state it
starts in (`Verified.restarts`); here what that state keeps of the caller's
entry, by `restarts_target_run` and `restarts_inline_run`: out of line, the
arguments, the callee-saved registers, the stack pointer and the return
address, which `env` keeps out of line (`Spec.Slab.contract`); inline,
every register but the clobbers.
-/

@[expose] public section

namespace VG.Proof.Slab.X86_64

open VG.X86_64 VG.Spec.Slab VG.Impl.Slab.X86_64

/-- `env` keeps the return address's slot. -/
theorem env_ret {c : Spec.Slab.Ctx isa} (hA : X86 c.A) {post : State → State → Prop}
    {exit : String → State → State → Prop} {abort : State → State → Prop}
    {trace : State → List Event → Prop} {p p' : State} (hf : c.form ≠ .inline)
    (he : (contract c post exit abort trace).env p p') :
    p'.mem.readW (p.gpr .rsp) 64 = p.mem.readW (p.gpr .rsp) 64 := by
  have hk := he.2.2.1 ⟨p.gpr .rsp, 8⟩ (List.mem_cons_of_mem _ (by
    obtain ⟨A, f, sig⟩ := c
    cases f <;> first | exact absurd rfl hf | exact hA.retSlot p))
  rw [mem_of hA, mem_of hA] at hk
  exact Mem.readW_congr fun i hi => hk _ (rsVar_contains _ (by omega))

theorem popTable_run {s s' : State} (hs : (popContract abi).pre s)
    (hr : Restarts target popTable (popContract abi) s s') :
    (popContract abi).pre s' ∧ (∀ r ∈ [Reg.rdi], s'.gpr r = s.gpr r) ∧
      ((∃ s'', Exec isa popTable s' s'' ∧ abiPreserved s s'' ∧ (popContract abi).post s' s'') ∨
        (∃ t s'', ExecExit isa popTable s' t s'' ∧ abiPreserved s s'' ∧
          (popContract abi).exit t s' s'')) :=
  restarts_target_run PopTable.verified
    (fun s hs p hp => by
      have := ((abort_iff PopTable.cx PopTable.hv s p).mp
        (PopTable.verified.abortOk hs hp).2.2.1).1
      simpa using this)
    (fun _ _ he => env_ret PopTable.hX (by decide) he) hs hr

theorem pushTable_run {s s' : State} (hs : (pushContract abi).pre s)
    (hr : Restarts target pushTable (pushContract abi) s s') :
    (pushContract abi).pre s' ∧ (∀ r ∈ [Reg.rdi, Reg.rsi], s'.gpr r = s.gpr r) ∧
      ((∃ s'', Exec isa pushTable s' s'' ∧ abiPreserved s s'' ∧ (pushContract abi).post s' s'') ∨
        (∃ t s'', ExecExit isa pushTable s' t s'' ∧ abiPreserved s s'' ∧
          (pushContract abi).exit t s' s'')) :=
  restarts_target_run PushTable.verified
    (fun s hs p hp => by
      have := ((abort_iff PushTable.cx PushTable.hv s p).mp
        (PushTable.verified.abortOk hs hp).2.2.1).1
      simp only [List.cons.injEq, and_true] at this
      simpa using this)
    (fun _ _ he => env_ret PushTable.hX (by decide) he) hs hr

theorem popInline_run {s s' : State} (hs : (popInlineContract inlineAbi).pre s)
    (hr : Restarts inlineTarget popInline (popInlineContract inlineAbi) s s') :
    (popInlineContract inlineAbi).pre s' ∧
      ((∃ s'', Exec isa popInline s' s'' ∧ preservedExcept (PopInline.clobbers ++ ["rax"]) s s'' ∧
          s''.gpr .rsp = s.gpr .rsp ∧ (popInlineContract inlineAbi).post s' s'') ∨
        (∃ t s'', ExecExit isa popInline s' t s'' ∧ preservedExcept PopInline.clobbers s s'' ∧
          s''.gpr .rsp = s.gpr .rsp ∧ (popInlineContract inlineAbi).exit t s' s'')) :=
  restarts_inline_run PopInline.verified (fun _ h => List.mem_append_left _ h)
    (fun _ hs => PopInline.clobbersOk hs) hs hr

theorem pushInline_run {s s' : State} (hs : (pushInlineContract inlineAbi).pre s)
    (hr : Restarts inlineTarget pushInline (pushInlineContract inlineAbi) s s') :
    (pushInlineContract inlineAbi).pre s' ∧
      ((∃ s'', Exec isa pushInline s' s'' ∧ preservedExcept PushInline.clobbers s s'' ∧
          s''.gpr .rsp = s.gpr .rsp ∧ (pushInlineContract inlineAbi).post s' s'') ∨
        (∃ t s'', ExecExit isa pushInline s' t s'' ∧ preservedExcept PushInline.clobbers s s'' ∧
          s''.gpr .rsp = s.gpr .rsp ∧ (pushInlineContract inlineAbi).exit t s' s'')) := by
  have := restarts_inline_run (cl' := PushInline.clobbers ++ []) PushInline.verified
    (fun _ h => List.mem_append_left _ h) (fun _ hs => PushInline.clobbersOk hs) hs hr
  simpa only [List.append_nil] using this

theorem cas_run {s s' : State} (hs : (casContract abi).pre s)
    (hr : Restarts target cas (casContract abi) s s') :
    (casContract abi).pre s' ∧ (∀ r ∈ [Reg.rdi, Reg.rsi, Reg.rdx], s'.gpr r = s.gpr r) ∧
      ((∃ s'', Exec isa cas s' s'' ∧ abiPreserved s s'' ∧ (casContract abi).post s' s'') ∨
        (∃ t s'', ExecExit isa cas s' t s'' ∧ abiPreserved s s'' ∧
          (casContract abi).exit t s' s'')) :=
  restarts_target_run Cas.verified
    (fun s hs p hp => by
      have := ((abort_iff Cas.cx Cas.hv s p).mp (Cas.verified.abortOk hs hp).2.2.1).1
      simp only [List.cons.injEq, and_true] at this
      simpa using this)
    (fun _ _ he => env_ret Cas.hX (by decide) he) hs hr

theorem stackPush_run {s s' : State} (hs : (stackPushContract abi).pre s)
    (hr : Restarts target stackPush (stackPushContract abi) s s') :
    (stackPushContract abi).pre s' ∧ (∀ r ∈ [Reg.rdi, Reg.rsi], s'.gpr r = s.gpr r) ∧
      ((∃ s'', Exec isa stackPush s' s'' ∧ abiPreserved s s'' ∧ (stackPushContract abi).post s' s'') ∨
        (∃ t s'', ExecExit isa stackPush s' t s'' ∧ abiPreserved s s'' ∧
          (stackPushContract abi).exit t s' s'')) :=
  restarts_target_run StackPush.verified
    (fun s hs p hp => by
      have := ((abort_iff StackPush.cx StackPush.hv s p).mp (StackPush.verified.abortOk hs hp).2.2.1).1
      simp only [List.cons.injEq, and_true] at this
      simpa using this)
    (fun _ _ he => env_ret StackPush.hX (by decide) he) hs hr

theorem stackPop_run {s s' : State} (hs : (stackPopContract abi).pre s)
    (hr : Restarts target stackPop (stackPopContract abi) s s') :
    (stackPopContract abi).pre s' ∧ (∀ r ∈ [Reg.rdi], s'.gpr r = s.gpr r) ∧
      ((∃ s'', Exec isa stackPop s' s'' ∧ abiPreserved s s'' ∧ (stackPopContract abi).post s' s'') ∨
        (∃ t s'', ExecExit isa stackPop s' t s'' ∧ abiPreserved s s'' ∧
          (stackPopContract abi).exit t s' s'')) :=
  restarts_target_run StackPop.verified
    (fun s hs p hp => by
      have := ((abort_iff StackPop.cx StackPop.hv s p).mp (StackPop.verified.abortOk hs hp).2.2.1).1
      simp only [List.cons.injEq, and_true] at this
      simpa using this)
    (fun _ _ he => env_ret StackPop.hX (by decide) he) hs hr

/-! ## As each artifact states it (`Artifact.restartsOk`) -/

theorem popInline_restartsOk : ∀ s s', (popInlineContract inlineAbi).pre s →
    Restarts inlineTarget popInline (popInlineContract inlineAbi) s s' →
      inlineTarget.abiPreserved s s' ∧ ArgsKept inlineTarget.abi popInlineSig s s' ∧
      (inlineTarget.inline = true → inlineTarget.preservedExcept PopInline.clobbers s s') :=
  restartsOk_inline (sig := popInlineSig) PopInline.verified
  (by decide) (fun _ hs => (PopInline.clobbersOk hs).2.2) (by decide)

theorem pushInline_restartsOk : ∀ s s', (pushInlineContract inlineAbi).pre s →
    Restarts inlineTarget pushInline (pushInlineContract inlineAbi) s s' →
      inlineTarget.abiPreserved s s' ∧ ArgsKept inlineTarget.abi pushInlineSig s s' ∧
      (inlineTarget.inline = true → inlineTarget.preservedExcept PushInline.clobbers s s') :=
  restartsOk_inline (sig := pushInlineSig) PushInline.verified
  (by decide) (fun _ hs => (PushInline.clobbersOk hs).2.2) (by decide)

theorem popTable_restartsOk : ∀ s s', (popContract abi).pre s →
    Restarts target popTable (popContract abi) s s' →
      target.abiPreserved s s' ∧ ArgsKept target.abi popSig s s' ∧
      (target.inline = true → target.preservedExcept [] s s') :=
  restartsOk_target (sig := popSig) PopTable.verified (by decide)
    (fun s hs p hp r hr => by
      have h := ((abort_iff PopTable.cx PopTable.hv s p).mp (PopTable.verified.abortOk hs hp).2.2.1).1
      have hr' : r ∈ [Reg.rdi] := hr
      simp only [List.cons.injEq, and_true] at h
      simp only [List.mem_cons, List.not_mem_nil, or_false] at hr'
      rcases hr' with rfl <;> simp_all)
    (fun _ _ he => env_ret PopTable.hX (by decide) he)

theorem pushTable_restartsOk : ∀ s s', (pushContract abi).pre s →
    Restarts target pushTable (pushContract abi) s s' →
      target.abiPreserved s s' ∧ ArgsKept target.abi pushSig s s' ∧
      (target.inline = true → target.preservedExcept [] s s') :=
  restartsOk_target (sig := pushSig) PushTable.verified (by decide)
    (fun s hs p hp r hr => by
      have h := ((abort_iff PushTable.cx PushTable.hv s p).mp (PushTable.verified.abortOk hs hp).2.2.1).1
      have hr' : r ∈ [Reg.rdi, Reg.rsi] := hr
      simp only [List.cons.injEq, and_true] at h
      simp only [List.mem_cons, List.not_mem_nil, or_false] at hr'
      rcases hr' with rfl | rfl <;> simp_all)
    (fun _ _ he => env_ret PushTable.hX (by decide) he)

theorem cas_restartsOk : ∀ s s', (casContract abi).pre s →
    Restarts target cas (casContract abi) s s' →
      target.abiPreserved s s' ∧ ArgsKept target.abi casSig s s' ∧
      (target.inline = true → target.preservedExcept [] s s') :=
  restartsOk_target (sig := casSig) Cas.verified (by decide)
    (fun s hs p hp r hr => by
      have h := ((abort_iff Cas.cx Cas.hv s p).mp (Cas.verified.abortOk hs hp).2.2.1).1
      have hr' : r ∈ [Reg.rdi, Reg.rsi, Reg.rdx] := hr
      simp only [List.cons.injEq, and_true] at h
      simp only [List.mem_cons, List.not_mem_nil, or_false] at hr'
      rcases hr' with rfl | rfl | rfl <;> simp_all)
    (fun _ _ he => env_ret Cas.hX (by decide) he)

theorem stackPush_restartsOk : ∀ s s', (stackPushContract abi).pre s →
    Restarts target stackPush (stackPushContract abi) s s' →
      target.abiPreserved s s' ∧ ArgsKept target.abi stackPushSig s s' ∧
      (target.inline = true → target.preservedExcept [] s s') :=
  restartsOk_target (sig := stackPushSig) StackPush.verified (by decide)
    (fun s hs p hp r hr => by
      have h := ((abort_iff StackPush.cx StackPush.hv s p).mp (StackPush.verified.abortOk hs hp).2.2.1).1
      have hr' : r ∈ [Reg.rdi, Reg.rsi] := hr
      simp only [List.cons.injEq, and_true] at h
      simp only [List.mem_cons, List.not_mem_nil, or_false] at hr'
      rcases hr' with rfl | rfl <;> simp_all)
    (fun _ _ he => env_ret StackPush.hX (by decide) he)

theorem stackPop_restartsOk : ∀ s s', (stackPopContract abi).pre s →
    Restarts target stackPop (stackPopContract abi) s s' →
      target.abiPreserved s s' ∧ ArgsKept target.abi stackPopSig s s' ∧
      (target.inline = true → target.preservedExcept [] s s') :=
  restartsOk_target (sig := stackPopSig) StackPop.verified (by decide)
    (fun s hs p hp r hr => by
      have h := ((abort_iff StackPop.cx StackPop.hv s p).mp (StackPop.verified.abortOk hs hp).2.2.1).1
      have hr' : r ∈ [Reg.rdi] := hr
      simp only [List.cons.injEq, and_true] at h
      simp only [List.mem_cons, List.not_mem_nil, or_false] at hr'
      rcases hr' with rfl <;> simp_all)
    (fun _ _ he => env_ret StackPop.hX (by decide) he)

end VG.Proof.Slab.X86_64
