module

public meta import Lean

/-!
# Axiom audit

**Trusted.** `#assert_standard_axioms c` fails unless the constant `c` (and so
everything it depends on, transitively) uses only Lean's three standard
axioms. In particular this rejects `sorry` (`sorryAx`) and anything proven by
`native_decide`/`bv_decide`, which rely on `Lean.ofReduceBool` and thus trust
the compiler. It refuses to run in a module, which sees the theorems it
imports without their proofs.
-/

public meta section

namespace VG

open Lean Elab Command

def standardAxioms : List Name := [``propext, ``Classical.choice, ``Quot.sound]

/-- The axioms the constants `roots` depend on in `env`, transitively,
through the types and values of constants (proofs of theorems included) and
the constructors of inductive types; and the constants they mention that
`env` does not have. It walks the declarations themselves, not the axiom
lists Lean records in each module, which code run while elaborating a module
could alter. -/
def axiomsOf (env : Environment) (roots : List Name) : List Name × List Name := Id.run do
  let mut seen : NameSet := {}
  let mut todo := roots
  let mut axioms : List Name := []
  let mut missing : List Name := []
  -- Visits each constant once, so it ends.
  while !todo.isEmpty do
    let n := todo.head!
    todo := todo.tail
    if seen.contains n then continue
    seen := seen.insert n
    match env.find? n with
    | none => missing := n :: missing
    | some ci =>
      if ci matches .axiomInfo _ then axioms := n :: axioms
      let value := (ci.value? (allowOpaque := true)).elim [] (·.getUsedConstants.toList)
      let ctors := match ci with
        | .inductInfo v => v.ctors
        | _ => []
      todo := ci.type.getUsedConstants.toList ++ value ++ ctors ++ todo
  return (axioms.reverse, missing.reverse)

elab "#assert_standard_axioms " id:ident : command => do
  -- In a module, a theorem imported from another module is an axiom, without
  -- its proof: `collectAxioms` would not see what the proof depends on.
  if (← getEnv).header.isModule then
    throwError "#assert_standard_axioms must run in a file that is not a module"
  let n ← liftCoreM <| realizeGlobalConstNoOverloadWithInfo id
  let axs ← liftCoreM <| collectAxioms n
  let bad := axs.filter (!standardAxioms.contains ·)
  unless bad.isEmpty do
    throwError m!"{n} depends on non-standard axioms: {bad.toList}"
  logInfo m!"{n} depends only on the standard axioms {axs.toList}"

end VG
