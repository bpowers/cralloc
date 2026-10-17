module

public import Asmalloc.TCB.Registration
public meta import Asmalloc.TCB.Registration
public import Asmalloc.Proof.Framework.Semantics
public meta import Asmalloc.Proof.Framework.Semantics
public meta import Lean.Elab.Tactic.Basic
public meta import Lean.Elab.Tactic.ElabTerm
public meta import Lean.Meta.Tactic.Cases

/-!
# The default proofs of a registration

Untrusted: this only changes how proofs are found; the kernel checks them.

The tactics `TCB/Registration.lean` declares for the default proofs of a
`Registration`, which a registration file imports this module for:

* `of_sig` proves `ofSig`, `∃ pre post x, contract = sig.contract A pre post
  writeArgs stack x`, by unfolding the head of `contract` until it is
  `Sig.contract`, and then `⟨none, _, _, _, rfl⟩` (or with the extra memory
  found by unification), whose unification does not always
  find the arguments of `Sig.contract` itself.
* `sp_safe` proves `spSafe`, `code.all (fun i => !T.isa.writesSp i) = true`,
  by `Code.all_of_allInstrs (by decide +kernel)`. Code that depends on a
  variable of an enumeration (`Bool`, an `inductive` of constructors without
  fields) is proven for each of its values, by cases.
-/

public meta section

namespace VG.RegistrationTactics

open Lean Meta Elab Tactic

/-- The left-hand side of the equation under the existential quantifiers
of `e`. -/
partial def eqLhs : Expr → Option Expr
  | .app (.app (.const ``Exists _) _) (.lam _ _ b _) => eqLhs b
  | e => e.eq?.map (·.2.1)

/-- `e` with its head unfolded until it is an application of `Sig.contract`. -/
partial def toSigContract (e : Expr) : MetaM Expr := do
  let e ← whnfCore e
  if e.isAppOf ``Sig.contract then return e
  match ← unfoldDefinition? e with
  | some e' => toSigContract e'
  | none => throwError "of_sig: {e} does not unfold to an application of `Sig.contract`"

elab_rules : tactic
  | `(tactic| of_sig) => do
    let g ← getMainGoal
    let ty ← instantiateMVars (← g.getType)
    let some c := eqLhs ty | throwError "of_sig: the goal is not `∃ ext pre post x, _ = _`: {ty}"
    if c.hasLooseBVars then throwError "of_sig: the contract depends on `pre` or `post`: {ty}"
    let c' ← toSigContract c
    setGoals [← g.replaceTargetDefEq (ty.replace fun e => if e == c then some c' else none)]
    evalTactic (← `(tactic| first | exact ⟨none, _, _, _, rfl⟩ | exact ⟨_, _, _, _, rfl⟩))

/-- A free variable of `e` whose type is an enumeration (`Bool`, an
`inductive` of constructors without fields). -/
def enumFVar? (e : Expr) : MetaM (Option FVarId) := do
  for fv in (collectFVars {} e).fvarIds do
    let ty ← whnf (← fv.getType)
    let .const n _ := ty | continue
    let some (.inductInfo i) := (← getEnv).find? n | continue
    if i.isRec || i.numIndices > 0 then continue
    if ← i.ctors.allM fun c => return (← getConstInfoCtor c).numFields == 0 then
      return some fv
  return none

elab_rules : tactic
  | `(tactic| sp_safe) => do
    let ty ← instantiateMVars (← getMainTarget)
    let some (_, lhs, _) := ty.eq? | throwError "sp_safe: the goal is not `Code.all p c = true`: {ty}"
    unless lhs.isAppOfArity ``Code.all 4 do
      throwError "sp_safe: the goal is not `Code.all p c = true`: {ty}"
    if let some fv ← enumFVar? lhs then
      let gs ← (← getMainGoal).cases fv
      for g in gs do
        setGoals [g.mvarId]
        evalTactic (← `(tactic| sp_safe))
    else
      evalTactic (← `(tactic| exact Code.all_of_allInstrs (by decide +kernel)))

end VG.RegistrationTactics
