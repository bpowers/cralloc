module

public import Asmalloc.Proof.Framework.Semantics

/-!
# A restartable sequence's body, one step at a time

The semantics folds over a sequence's body four ways: its run
(`runBody`), the states it may be aborted in (`bodyPrefixes`), its memory
accesses (`bodyEvents`) and where it ends (`bodyEnd`). Each lemma here takes
one step of one fold, given the step's result (`exec i s = some s'`, or the
condition `eval c s`), so that a proof unfolds a body by rewriting with the
steps it has proven once.
-/

@[expose] public section

namespace VG

variable {M : ISA} {i : M.Instr} {c : M.Cond} {cold : List M.Instr} {ext : Option String}
  {rest : List (Step M.Instr M.Cond)} {s s' : M.State}

theorem execBlock_cons_some (h : M.exec i s = some s') (is : List M.Instr) :
    execBlock M (i :: is) s = execBlock M is s' := by
  simp only [execBlock, h]

theorem execBlock_nil' : execBlock M [] s = some s := rfl

theorem blockEvents_cons_some (h : M.exec i s = some s') (is : List M.Instr) :
    blockEvents M (i :: is) s = M.events i s s' ++ blockEvents M is s' := by
  simp only [blockEvents, h]

theorem blockEvents_nil' : blockEvents M [] s = [] := rfl

/-! ## An instruction -/

theorem runBody_instr (h : M.exec i s = some s') :
    runBody M (.instr i :: rest) s = runBody M rest s' := by
  simp only [runBody, h]

theorem bodyPrefixes_instr (h : M.exec i s = some s') :
    bodyPrefixes M (.instr i :: rest) s = s :: bodyPrefixes M rest s' := by
  simp only [bodyPrefixes, h]

theorem bodyEvents_instr (h : M.exec i s = some s') :
    bodyEvents M (.instr i :: rest) s = M.events i s s' ++ bodyEvents M rest s' := by
  simp only [bodyEvents, h]

theorem bodyEnd_instr (h : M.exec i s = some s') :
    bodyEnd M (.instr i :: rest) s = (bodyEnd M rest s').map (· + 1) := by
  simp only [bodyEnd, h]

/-! ## An exit not taken -/

theorem runBody_skip (h : M.eval c s = some false) :
    runBody M (.exit c cold ext :: rest) s = runBody M rest s := by
  simp only [runBody, h]

theorem bodyPrefixes_skip (h : M.eval c s = some false) :
    bodyPrefixes M (.exit c cold ext :: rest) s = s :: bodyPrefixes M rest s := by
  simp only [bodyPrefixes, h]

theorem bodyEvents_skip (h : M.eval c s = some false) :
    bodyEvents M (.exit c cold ext :: rest) s = bodyEvents M rest s := by
  simp only [bodyEvents, h]

theorem bodyEnd_skip (h : M.eval c s = some false) :
    bodyEnd M (.exit c cold ext :: rest) s = (bodyEnd M rest s).map (· + 1) := by
  simp only [bodyEnd, h]

/-! ## An exit taken -/

theorem runBody_take (h : M.eval c s = some true) (hc : execBlock M cold s = some s') :
    runBody M (.exit c cold ext :: rest) s =
      some (match ext with | none => .done s' | some t => .exit t s') := by
  simp only [runBody, h, hc]
  cases ext <;> rfl

theorem bodyPrefixes_take (h : M.eval c s = some true) :
    bodyPrefixes M (.exit c cold ext :: rest) s = [s] := by
  simp only [bodyPrefixes, h]

theorem bodyEvents_take (h : M.eval c s = some true) :
    bodyEvents M (.exit c cold ext :: rest) s = blockEvents M cold s := by
  simp only [bodyEvents, h]

theorem bodyEnd_take (h : M.eval c s = some true) :
    bodyEnd M (.exit c cold ext :: rest) s = some 0 := by
  simp only [bodyEnd, h]

/-! ## The end -/

theorem runBody_nil' : runBody M [] s = some (.done s) := rfl
theorem bodyPrefixes_last (h : M.exec i s = some s') :
    bodyPrefixes M [.instr i] s = [s] := by
  simp only [bodyPrefixes, h]
theorem bodyEvents_nil' : bodyEvents M [] s = [] := rfl
theorem bodyEnd_nil' : bodyEnd M [] s = some 0 := rfl

end VG
