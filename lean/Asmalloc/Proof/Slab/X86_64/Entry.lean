module

public import Asmalloc.Proof.Slab.X86_64.Reg
public import Asmalloc.Proof.Framework.X86_64.Retry

/-!
# The routines, aborted any number of times, against the caller's entry

Each routine's last attempt meets its contract relative to the state it
starts in (`Verified.restarts`); here what that state keeps of the caller's
entry, by `restarts_target_run` and `restarts_inline_run`: out of line, the
arguments, the callee-saved registers, the stack pointer and the return
address, which `env` keeps (`Spec.Slab.contract`); inline, every register
but the clobbers.
-/

@[expose] public section

namespace VG.Proof.Slab.X86_64

open VG.X86_64 VG.Spec.Slab VG.Impl.Slab.X86_64

/-- `env` keeps the return address's slot. -/
theorem env_ret {c : Spec.Slab.Ctx isa} (hA : X86 c.A) {post : State → State → Prop}
    {exit : String → State → State → Prop} {abort : State → State → Prop}
    {trace : State → List Event → Prop} {p p' : State}
    (he : (contract c post exit abort trace).env p p') :
    p'.mem.readW (p.gpr .rsp) 64 = p.mem.readW (p.gpr .rsp) 64 := by
  have hk := he.2.2.1 ⟨p.gpr .rsp, 8⟩ (List.mem_cons_of_mem _ (hA.retSlot p))
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
    (fun _ _ he => env_ret PopTable.hX he) hs hr

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
    (fun _ _ he => env_ret PushTable.hX he) hs hr

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

end VG.Proof.Slab.X86_64
