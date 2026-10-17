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
