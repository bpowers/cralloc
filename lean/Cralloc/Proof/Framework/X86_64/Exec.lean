module

public import Cralloc.TCB.X86_64.Isa
public import Cralloc.Proof.Framework.Block

/-!
# x86-64: lemmas for symbolic execution
-/

@[expose] public section

namespace VG.X86_64

/-! Symbolic execution of a block, one instruction at a time (see `runStep`).
These are deliberately not proved by `rfl`: `simp` would use an `rfl` lemma
as a definitional unfolding, which the kernel then re-checks by unfolding the
structural recursion of `runBlock` over the whole remaining block, at every
instruction. -/

theorem runBlock_nil {s : State} : runBlock isa ([] : List Instr) s = some s := by
  rw [runBlock]

theorem runBlock_cons {i : Instr} {is : List Instr} {s : State} :
    runBlock isa (i :: is : List Instr) s = runStep isa (exec i s) is := by
  rw [runBlock]; rfl

theorem runStep_some {s : State} {is : List Instr} :
    runStep isa (some s : Option State) is = runBlock isa is s := by
  rw [runStep]; rfl

end VG.X86_64
