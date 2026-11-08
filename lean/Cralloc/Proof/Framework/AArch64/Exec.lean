module

public import Cralloc.TCB.AArch64.Isa
public import Cralloc.Proof.Framework.Block

/-!
# AArch64: lemmas for symbolic execution

As on x86-64 (`Proof/Framework/X86_64/Exec.lean`): stated at the concrete types,
and not proved by `rfl`, so that `simp` does not unfold the rest of a block.
-/

@[expose] public section

namespace VG.AArch64

theorem runBlock_nil {s : State} : runBlock isa ([] : List Instr) s = some s := by
  rw [runBlock]

theorem runBlock_cons {i : Instr} {is : List Instr} {s : State} :
    runBlock isa (i :: is : List Instr) s = runStep isa (exec i s) is := by
  rw [runBlock]; rfl

theorem runStep_some {s : State} {is : List Instr} :
    runStep isa (some s : Option State) is = runBlock isa is s := by
  rw [runStep]; rfl

end VG.AArch64
