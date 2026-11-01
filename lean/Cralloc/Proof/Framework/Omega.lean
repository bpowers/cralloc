module

public meta import Lean.Elab.Tactic.Basic
public meta import Lean.Elab.Tactic.Omega
public meta import Lean.Elab.SyntheticMVars
public meta import Lean.Meta.Closure
public meta import Lean.Meta.Tactic.Assert
public meta import Lean.Meta.Tactic.Clear

/-!
# `omega` in large contexts

Untrusted: this only changes how proofs are found.

`omega` and `bv_omega` use every hypothesis in the local context (`bv_omega`
first rewrites every one of them with `simp … at *`). In the middle of a long
proof about machine states most hypotheses are facts about memory, regions
or states that they cannot use, and looking at them costs a tenth of a second
or more per call; and preprocessing each fact about `/` and `%` introduces
new variables and constraints, which every call pays for and the kernel
checks. Two ways to run them on less:

* `omega_arith` selects (in)equations between natural numbers, integers or
  bit vectors, and propositional combinations of them, that are connected to
  the goal (see `relevantFacts`). `bv_omega_arith` clears other hypotheses
  before preprocessing;
* `omega_using [h₁, …, hₙ]` and `bv_omega_using [h₁, …, hₙ]` use only the
  facts `h₁, …, hₙ` (any terms). `omega_using` passes them directly to
  the solver; `bv_omega_using` clears other hypotheses before preprocessing.
-/

public meta section

namespace VG.Omega

open Lean Meta Elab Tactic

/-- Whether `e` is a type `omega` reasons about: `Nat`, `Int`, `Fin n` or `BitVec w`. -/
def isArithType (e : Expr) : Bool :=
  e.isConstOf ``Nat || e.isConstOf ``Int || e.isAppOfArity ``BitVec 1 || e.isConstOf ``Fin ||
    e.isAppOfArity ``Fin 1

/-- Whether `p` is a fact `omega` may use: (in)equations over `Nat`, `Int` or
`BitVec`, `Dvd`, and `¬`, `∧`, `∨`, `↔`, `→` of those. -/
partial def isArithProp (p : Expr) : MetaM Bool := do
  let p ← instantiateMVars p
  match p.getAppFn.constName?, p.getAppNumArgs with
  | some ``Eq, 3 | some ``Ne, 3 | some ``LE.le, 4 | some ``LT.lt, 4 | some ``GE.ge, 4
  | some ``GT.gt, 4 | some ``Dvd.dvd, 4 => return isArithType (p.getArg! 0)
  | some ``Not, 1 => isArithProp (p.getArg! 0)
  | some ``And, 2 | some ``Or, 2 | some ``Iff, 2 =>
    return (← isArithProp (p.getArg! 0)) && (← isArithProp (p.getArg! 1))
  | _, _ =>
    if p.isArrow then return (← isArithProp p.bindingDomain!) && (← isArithProp p.bindingBody!)
    else return p.isConstOf ``False || p.isConstOf ``True

/-- Clears every hypothesis `omega` cannot use (and that nothing else depends on). -/
def clearNonArith : TacticM Unit := withMainContext do
  let mut g ← getMainGoal
  for d in (← getLCtx).getFVarIds.reverse do
    let some ld := (← getLCtx).find? d | continue
    if ld.isImplementationDetail || ld.isLet then continue
    let ty ← instantiateMVars ld.type
    unless ← isProp ty do continue
    if ← isArithProp ty then continue
    try g ← g.clear d catch _ => pure ()
  replaceMainGoal [g]

/-- Adds the facts `hs` to the main goal as new hypotheses, and clears every
other hypothesis that is a proposition. -/
def keepOnly (hs : Array Term) : TacticM Unit := do
  let mut g ← getMainGoal
  -- Elaborate every fact in the original context, then add them as new hypotheses.
  let facts ← g.withContext do
    hs.mapM fun h => do
      let e ← Term.elabTerm h none
      Term.synthesizeSyntheticMVarsNoPostponing
      let e ← instantiateMVars e
      pure (e, ← instantiateMVars (← inferType e))
  let mut keep : Array FVarId := #[]
  for (e, ty) in facts do
    let (fv, g') ← (← g.assert (← mkFreshUserName `h) ty e).intro1P
    keep := keep.push fv
    g := g'
  let g' ← g.withContext do
    let mut g := g
    for fv in (← getLCtx).getFVarIds.reverse do
      if keep.contains fv then continue
      let d ← fv.getDecl
      if d.isImplementationDetail then continue
      if ← isProp d.type then
        g ← g.tryClear fv
    pure g
  replaceMainGoal [g']

/-- Run the arithmetic solver on these facts and the negation of the goal. -/
def omegaFromFacts (facts : Array Expr) (exactTypeOnly := false) : TacticM Unit := do
  recordExtraModUse (isMeta := false) `Init.Omega
  liftMetaFinishingTactic fun g => g.withContext do
    let target ← g.getType
    for fact in facts do
      let factType ← inferType fact
      let sameType ← if exactTypeOnly then pure (factType == target)
        else withReducibleAndInstances <| isDefEq factType target
      if sameType then
        g.assign fact
        return
    let oldCtx ← getLCtx
    let some g ← g.falseOrByContra | return ()
    g.withContext do
      -- Include the negated goal and any hypotheses introduced from it.
      let newFacts := (← getLocalHyps).filter fun h => !oldCtx.contains h.fvarId!
      let type ← g.getType
      let result ← mkFreshExprSyntheticOpaqueMVar type
      Lean.Elab.Tactic.Omega.omega (facts.toList ++ newFacts.toList) result.mvarId!
      -- Keep the same auxiliary-theorem boundary as Lean's `omega` tactic.
      let proof ← mkAuxTheorem type (← instantiateMVarsProfiling result) (zetaDelta := true)
      g.assign proof

/-- Elaborate exactly the facts selected by the caller. A fact that is the
goal closes it only if its type is the goal's (unifying every fact with the
goal first costs more than the solver); if the solver then fails, try again
with a fact the goal is equal to up to unfolding. -/
def omegaWith (hs : Array Term) : TacticM Unit := do
  let facts ← withMainContext do
    hs.mapM fun h => do
      let e ← Term.elabTerm h none
      Term.synthesizeSyntheticMVarsNoPostponing
      instantiateMVars e
  let saved ← saveState
  try omegaFromFacts facts (exactTypeOnly := true)
  catch _ =>
    saved.restore
    omegaFromFacts facts

/-- Adds the atoms of the arithmetic term or fact `e` to `acc`: its maximal
subterms that are not arithmetic operations, casts or numerals. -/
partial def atoms (e : Expr) (acc : Std.HashSet Expr) : MetaM (Std.HashSet Expr) := do
  let e := (← instantiateMVars e).consumeMData
  match e.getAppFn.constName?, e.getAppNumArgs with
  | some ``HAdd.hAdd, 6 | some ``HSub.hSub, 6 | some ``HMul.hMul, 6 | some ``HDiv.hDiv, 6
  | some ``HMod.hMod, 6 | some ``HPow.hPow, 6 | some ``Min.min, 4 | some ``Max.max, 4
  | some ``Eq, 3 | some ``Ne, 3 | some ``LE.le, 4 | some ``LT.lt, 4 | some ``GE.ge, 4
  | some ``GT.gt, 4 | some ``Dvd.dvd, 4 | some ``And, 2 | some ``Or, 2 | some ``Iff, 2 =>
    atoms e.appArg! (← atoms e.appFn!.appArg! acc)
  | some ``Not, 1 | some ``Nat.succ, 1 | some ``Nat.cast, 3 | some ``Int.ofNat, 1
  | some ``BitVec.toNat, 2 | some ``BitVec.ofNat, 2 | some ``Int.toNat, 1 | some ``Int.natAbs, 1
  | some ``Neg.neg, 3 => atoms e.appArg! acc
  | some ``OfNat.ofNat, 3 => return acc
  | _, _ =>
    if e.isRawNatLit || e.isConstOf ``False || e.isConstOf ``True then return acc
    if e.isArrow then return ← atoms e.bindingBody! (← atoms e.bindingDomain! acc)
    return acc.insert e

/-- The atom `x` of a fact `x = t`, if `x` is an atom. -/
def definedAtom? (ty : Expr) : MetaM (Option Expr) := do
  let ty := (← instantiateMVars ty).consumeMData
  let_expr Eq _ x _ := ty | return none
  let x := x.consumeMData
  let s ← atoms x {}
  return if s.size == 1 && s.contains x then some x else none

/-- The facts among `hs` connected to the goal: starting from the goal's
atoms, a fact is taken when it shares an atom with those taken so far (an
equation `x = t` of an atom `x`, only when `x` is among them), and adds its
atoms. Every fact `omega` adds to its problem makes its proof term larger,
which the kernel checks; equations such as a layout's offsets (`l.oE = …`),
in the context of every goal about one, made each proof several times
larger. -/
def relevantFacts (hs : Array Expr) (goal : Expr) : MetaM (Array Expr) := do
  let mut seen ← atoms goal {}
  if seen.isEmpty then return hs
  let mut infos : Array (Std.HashSet Expr × Option Expr) := #[]
  for h in hs do
    let ty ← inferType h
    infos := infos.push (← atoms ty {}, ← definedAtom? ty)
  let mut taken := Array.replicate hs.size false
  let mut changed := true
  while changed do
    changed := false
    for i in [:infos.size] do
      if taken[i]! then continue
      let (as, d) := infos[i]!
      if match d with | some x => seen.contains x | none => as.any seen.contains then
        taken := taken.set! i true
        seen := as.fold (·.insert ·) seen
        changed := true
  return (hs.zip taken).filterMap fun (h, t) => if t then some h else none

/-- Select arithmetic hypotheses without repeatedly clearing the context. -/
def omegaArith : TacticM Unit := do
  let (facts, all) ← withMainContext do
    let all ← (← getLocalHyps).filterM fun h => do isArithProp (← inferType h)
    pure (← relevantFacts all (← getMainTarget), all)
  -- Most arithmetic goals need the solver, so avoid unifying every fact first.
  -- Retry the original path for goals that need a definitionally equal fact,
  -- then with every arithmetic fact.
  let saved ← saveState
  try omegaFromFacts facts (exactTypeOnly := true)
  catch _ =>
    saved.restore
    if facts.size == all.size then omegaFromFacts facts
    else
      try omegaFromFacts facts
      catch _ =>
        saved.restore
        omegaFromFacts all

/-- `omega` on the arithmetic facts connected to the goal, once: `omega_arith`
without its retries, for index bounds (`Framework/GetElem.lean`), where Lean
core's rules follow if it fails. -/
def omegaArithOnce : TacticM Unit := do
  let facts ← withMainContext do
    let all ← (← getLocalHyps).filterM fun h => do isArithProp (← inferType h)
    relevantFacts all (← getMainTarget)
  omegaFromFacts facts (exactTypeOnly := true)

/-- Clears every hypothesis `omega` cannot use. -/
elab "clear_non_arith" : tactic => clearNonArith

/-- `omega` using only the given facts, not the local context. -/
syntax (name := omegaUsing) "omega_using " "[" term,* "]" : tactic

elab_rules : tactic
  | `(tactic| omega_using [$hs,*]) => omegaWith hs.getElems

/-- `bv_omega` using only the given facts, not the local context. -/
syntax (name := bvOmegaUsing) "bv_omega_using " "[" term,* "]" : tactic

elab_rules : tactic
  | `(tactic| bv_omega_using [$hs,*]) => do
    keepOnly hs.getElems
    evalTactic (← `(tactic| bv_omega))

end VG.Omega

/-- `omega` using the arithmetic hypotheses in the local context. -/
elab "omega_arith" : tactic => VG.Omega.omegaArith

/-- `omega_arith` without its retries (`omegaArithOnce`). -/
elab "omega_arith_once" : tactic => VG.Omega.omegaArithOnce

/-- `bv_omega`, after clearing the hypotheses it cannot use. -/
macro "bv_omega_arith" : tactic => `(tactic| (clear_non_arith; bv_omega))

/-- `decide` if the goal is about numerals only (as the side conditions of
lemmas about literal offsets are, once instantiated), and `omega` otherwise.
`omega` first collects every arithmetic hypothesis in the context, a tenth of
a second or more deep in a proof about states; `decide` does not look at the
context. -/
macro "lit_omega" : tactic => `(tactic| first | decide | omega)

/-! ## Index identities by evaluation

Proofs about lanes and words are full of small identities between indices,
with `/` and `%` by literals, about variables with literal bounds (`e < 8`,
`l < 2`): `4 * ((e - 4) / 2) + 8 = 4 * (e / 2)` when `4 ≤ e`. `omega`
proves each, but its proof of a division or remainder is a large term,
which the kernel checks in tens of milliseconds every time.

`bdd_omega` proves such a goal by evaluation instead: when every variable of
the goal is a natural number with a literal bound among the hypotheses, it
states the goal for all values below the bounds, assuming the hypotheses
about those variables alone, and has the kernel evaluate it
(`decide +kernel`). Otherwise, or if that fails, it is `omega`.
-/

namespace VG

open Lean Meta Elab Tactic

/-- The literal `n` of a hypothesis `x < n` (or `x ≤ n - 1` as `x ≤ n`), for the
free variable `x`. -/
private def litBound? (x : FVarId) (ty : Expr) : MetaM (Option Nat) := do
  let ty ← instantiateMVars ty
  match ty.getAppFnArgs with
  | (``LT.lt, #[.const ``Nat [], _, a, b]) =>
    if a.isFVarOf x then return (← getNatValue? b) else return none
  | (``LE.le, #[.const ``Nat [], _, a, b]) =>
    if a.isFVarOf x then return (← getNatValue? b).map (· + 1) else return none
  | _ => return none
where
  getNatValue? (e : Expr) : MetaM (Option Nat) := do
    let e ← instantiateMVars e
    return e.nat? <|> e.rawNatLit?

/-- Whether `ty` is an arithmetic fact `bdd_omega` may assume: a comparison of
natural numbers, or its negation. -/
private partial def isArith (ty : Expr) : Bool :=
  match ty.getAppFnArgs with
  | (``Not, #[p]) => isArith p
  | (``LT.lt, #[.const ``Nat [], _, _, _]) | (``LE.le, #[.const ``Nat [], _, _, _]) => true
  | (``Eq, #[.const ``Nat [], _, _]) | (``Ne, #[.const ``Nat [], _, _]) => true
  | _ => false

/-- Tries to prove the main goal by evaluation over the bounded variables (see
"Index identities by evaluation"); returns whether it did. -/
private def bddDecide (maxCases : Nat) : TacticM Bool := withMainContext do
  let g ← getMainGoal
  let tgt ← instantiateMVars (← g.getType)
  if tgt.hasMVar then return false
  let xs := (collectFVars {} tgt).fvarIds
  if xs.isEmpty then return false
  let lctx ← getLCtx
  -- Each variable: a natural number with a literal bound.
  let mut bounds : Array (FVarId × FVarId × Nat) := #[]
  for x in xs do
    let some d := lctx.find? x | return false
    unless (← whnfR d.type).isConstOf ``Nat do return false
    let mut best : Option (FVarId × Nat) := none
    for h in lctx do
      if h.isImplementationDetail then continue
      if let some n ← litBound? x h.type then
        if best.all (n < ·.2) then best := some (h.fvarId, n)
    let some (h, n) := best | return false
    bounds := bounds.push (x, h, n)
  if bounds.foldl (fun p (_, _, n) => p * n) 1 > maxCases then return false
  -- The other facts about these variables alone.
  let xset := xs.foldl (fun s x => s.insert x) ({} : Std.HashSet FVarId)
  let used := bounds.foldl (fun s (_, h, _) => s.insert h) ({} : Std.HashSet FVarId)
  let mut facts : Array FVarId := #[]
  for h in lctx do
    if h.isImplementationDetail || used.contains h.fvarId || xset.contains h.fvarId then continue
    let ty ← instantiateMVars h.type
    unless isArith ty do continue
    let fv := (collectFVars {} ty).fvarIds
    if !fv.isEmpty && fv.all xset.contains then facts := facts.push h.fvarId
  -- `∀ x₁, x₁ < n₁ → … → facts → goal`, by evaluation.
  let binders := bounds.foldl (fun a (x, h, _) => a.push (.fvar x) |>.push (.fvar h)) #[]
  let binders := binders ++ facts.map Expr.fvar
  let stmt ← mkForallFVars binders tgt
  if stmt.hasFVar then return false
  let m ← mkFreshExprSyntheticOpaqueMVar stmt
  let saved ← saveState
  try
    setGoals [m.mvarId!]
    evalTactic (← `(tactic| decide +kernel))
    let p ← instantiateMVars m
    g.assign (mkAppN p binders)
    setGoals []
    return true
  catch _ =>
    saved.restore
    return false

/-- `omega`, or, for a goal whose variables all have literal bounds among the
hypotheses (with at most `n` values in all, 64 if not given), the kernel's
evaluation of the goal for every value (see "Index identities by evaluation"). -/
elab "bdd_omega" n:(ppSpace num)? : tactic => do
  unless ← bddDecide (n.map (·.getNat) |>.getD 64) do evalTactic (← `(tactic| omega))

end VG
