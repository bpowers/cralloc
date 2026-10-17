module

public import Asmalloc.TCB.Registration
public meta import Asmalloc.TCB.Registration
public import Asmalloc.Proof.Framework.Lit
public meta import Asmalloc.Proof.Framework.Lit
public import Asmalloc.Proof.Framework.Semantics
public meta import Asmalloc.Proof.Framework.Semantics
public meta import Lean.Elab.Tactic.Basic
public meta import Lean.Elab.Tactic.ElabTerm

/-!
# The default proofs of a registration

Untrusted: this only changes how proofs are found; the kernel checks them.

The tactics `TCB/Registration.lean` declares for the default proofs of a
`Registration`, which a registration file imports this module for:

* `of_sig` proves `ofSig`, `∃ pre post leak, contract = sig.contract A pre
  post writeArgs stack leak`, by unfolding the head of `contract` until it is
  `Sig.contract` (e.g. `nttInvContract A` to `inPlaceContract A nttInv` to
  `Sig.contract A (inPlaceSig "f") …`), and then `⟨_, _, _, rfl⟩`, whose
  unification does not always find the arguments of `Sig.contract` itself.
* `sp_safe` proves `spSafe`, `code.all (fun i => !T.isa.writesSp i) = true`:
  on the ISAs whose `writesSp` is always `false` (ARMv7, AArch64) by
  `Code.all_of_forall`, which evaluates nothing, and on the others (x86,
  x86-64) by `Code.all_of_allInstrs (by lit_decide)`, which evaluates the
  code's literal if the module imports the one that materializes it
  (`Lit.lean`), or, where that exceeds the recursion limit (rewriting some
  code to its literal), by `Code.all_of_allInstrs (by decide +kernel)`.
  Code that depends on a variable of an enumeration (`adx : Bool`, a
  variant's `FieldVariant`) is proven for each of its values, by cases.
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
    let some c := eqLhs ty | throwError "of_sig: the goal is not `∃ pre post leak, _ = _`: {ty}"
    if c.hasLooseBVars then throwError "of_sig: the contract depends on `pre`, `post` or `leak`: {ty}"
    let c' ← toSigContract c
    setGoals [← g.replaceTargetDefEq (ty.replace fun e => if e == c then some c' else none)]
    evalTactic (← `(tactic| exact ⟨_, _, _, rfl⟩))

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
    let p := lhs.getArg! 2
    -- Whether `p` holds of every instruction by evaluation alone.
    let always ← withLocalDeclD `i (lhs.getArg! 0) fun i => do
      return (← whnf (mkApp p i)).isConstOf ``Bool.true
    if always then
      evalTactic (← `(tactic| exact Code.all_of_forall (fun _ => rfl) _))
    else if let some fv ← enumFVar? lhs then
      let gs ← (← getMainGoal).cases fv
      for g in gs do
        setGoals [g.mvarId]
        evalTactic (← `(tactic| sp_safe))
    else
      let s ← saveState
      -- The recursion limit is a runtime exception, which `first` does not catch.
      tryCatchRuntimeEx (evalTactic (← `(tactic| exact Code.all_of_allInstrs (by lit_decide))))
        fun _ => do
          s.restore
          evalTactic (← `(tactic| exact Code.all_of_allInstrs (by decide +kernel)))

end VG.RegistrationTactics
