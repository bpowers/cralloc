module

public meta import Lean.Meta.Tactic.Simp.Simproc
public meta import Lean.Elab.Tactic.Location

/-!
# Evaluating a contract's signature by unfolding

The result is checked by the kernel, as a definitional unfolding.

`sig_eval` (`Proof/Framework/Sig.lean`) first evaluates the data of a
contract built with `Sig.contract`: the argument words of the signature, the
calling convention's argument locations, the buffers, which of them are
writable. This is computation, so the result is definitionally equal to the
contract and needs no proof term. `dsimp` can do it, but it visits every
subterm bottom-up (the signature's parameter names, every field of the calling
convention and of the contract), tries its simp set on each, and unfolds
definitions whose results are then thrown away. `sigReduce` evaluates
top-down instead, only what the result needs:

* the head of a term is reduced by `whnf`, unfolding only the given
  definitions (the contract, the signature, the calling convention's
  functions; and instances and structure projections, when that reaches a
  constructor);
* a closed term of a data type (a list, a pair, a number, …: the signature,
  the argument widths, the argument slots) is reduced by `whnf` with no
  restriction;
* `if c then … else …`, `decide c` and `cond` on a closed `c` are decided;
* arithmetic on numerals is evaluated (not `^`, which stays readable);
* then the arguments, and the bodies of binders, are evaluated in turn.
-/

public meta section

namespace VG.Sig.Eval

open Lean Meta

/-- The types whose closed terms `sigReduce` evaluates. -/
def dataTypes : List Name :=
  [``List, ``Prod, ``Option, ``Bool, ``Nat, `VG.ArgWord, `VG.Param, `VG.IntTy, `VG.Elem, `VG.Sig,
    `VG.Arm.Loc]

/-- The binary operations on `Nat` that `sigReduce` evaluates on numerals. -/
def natOps : List (Name × (Nat → Nat → Nat)) :=
  [(``HAdd.hAdd, (· + ·)), (``HSub.hSub, (· - ·)), (``HMul.hMul, (· * ·)), (``HDiv.hDiv, (· / ·)),
    (``HMod.hMod, (· % ·))]

/-- The definitions that `sigReduce` unfolds only to a value. -/
def valueOnly : List Name := [``and, ``or, ``not, ``cond]

structure Ctx where
  /-- The definitions to unfold. -/
  unfold : NameSet

abbrev M := ReaderT Ctx <| StateRefT (Std.HashMap Expr Expr) MetaM

/-- Whether `c` may be unfolded while reducing the head of a term. -/
def canUnfold (u : NameSet) (c : Name) : CoreM Bool := do
  if u.contains c then return true
  -- Auxiliary definitions of the equation compiler (`_sparseCasesOn`, …).
  if c.isInternal || isAuxRecursor (← getEnv) c then return true
  if (← isInstance c) then return true
  if (← getProjectionFnInfo? c).isSome then return true
  return false

/-- The value of a closed decidable proposition. -/
def decide? (c i : Expr) : MetaM (Option (Bool × Expr)) := do
  if c.hasFVar || c.hasMVar || c.hasLooseBVars then return none
  match_expr ← whnfD i with
  | Decidable.isTrue _ h => return some (true, h)
  | Decidable.isFalse _ h => return some (false, h)
  | _ => return none

def isClosed (e : Expr) : Bool := !(e.hasFVar || e.hasMVar || e.hasLooseBVars)

/-- Whether the head of `e` is a constructor or a literal. -/
def isValue (e : Expr) : MetaM Bool := do
  if e.isRawNatLit || e.nat?.isSome then return true
  let .const c _ := e.getAppFn | return false
  return (← getEnv).isConstructor c

/-- One step at the head of `e`: `some e'` if `e` reduces to `e'`. -/
def step (e : Expr) : M (Option Expr) := do
  let f := e.getAppFn
  if f.isLambda then return some e.headBeta
  let .const c _ := f | return none
  let args := e.getAppArgs
  -- Decisions on closed conditions.
  if c == ``ite && args.size ≥ 5 then
    let some (v, _) ← decide? args[1]! args[2]! | return none
    return some (mkAppN (if v then args[3]! else args[4]!) args[5:])
  if c == ``dite && args.size ≥ 5 then
    let some (v, h) ← decide? args[1]! args[2]! | return none
    return some (mkAppN (mkApp (if v then args[3]! else args[4]!) h).headBeta args[5:])
  if c == ``Decidable.decide && args.size == 2 then
    let some (v, _) ← decide? args[0]! args[1]! | return none
    return some (toExpr v)
  -- An element of a list at a numeral index.
  if c == ``List.getD && args.size == 4 then
    let some i := args[2]!.nat? <|> args[2]!.rawNatLit? | return none
    let mut l := args[1]!
    for _ in [0:i] do
      let_expr List.cons _ _ t := l | return (if l.isAppOfArity ``List.nil 1 then some args[3]! else none)
      l := t
    match_expr l with
    | List.cons _ a _ => return some a
    | List.nil _ => return some args[3]!
    | _ => return none
  -- Arithmetic on numerals.
  if let some (_, op) := natOps.find? (·.1 == c) then
    if args.size == 6 then
      if let (some a, some b) := (args[4]!.nat? <|> args[4]!.rawNatLit?,
          args[5]!.nat? <|> args[5]!.rawNatLit?) then
        if (← whnfR args[0]!).isConstOf ``Nat then
          return some (mkNatLit (op a b))
    return none
  if c == ``HPow.hPow then return none
  -- Closed data (the signature's words, the argument slots, …).
  if e.nat?.isSome then return none
  if isClosed e && (← read).unfold.contains c then
    let ty ← whnfR (← inferType e)
    if let .const tc _ := ty.getAppFn then
      if dataTypes.contains tc then
        let e' ← whnfD e
        if let some n := e'.rawNatLit? then return some (mkNatLit n)
        if e' != e && (← isValue e') then return some e'
        return none
  -- A projection of a structure (not a class) that evaluates to a constructor.
  if let some info ← getProjectionFnInfo? c then
    if !info.fromClass then
      if args.size ≤ info.numParams then return none
      let some v ← projectCore? (← whnfD args[info.numParams]!) info.i | return none
      return some (mkAppN v args[info.numParams + 1:]).headBeta
  -- Never an instance itself (`instLENat` stays).
  if (← isInstance c) then
    if (← isClass? (← inferType e)).isSome then return none
  let u := (← read).unfold
  -- A `match` whose discriminants evaluate (if they are closed, with no restriction).
  if let some info ← getMatcherInfo? c then
    if let .reduced e' ← withCanUnfoldPred (fun _ ci => canUnfold u ci.name) <| reduceMatcher? e then
      return some e'
    let discrs := args[info.numParams + 1:info.numParams + 1 + info.numDiscrs]
    if discrs.all isClosed then
      if let .reduced e' ← reduceMatcher? e then return some e'
    return none
  -- Unfolding: one step at the head (the next steps are `step`'s again).
  let unf := u.contains c || isCasesOnRecursor (← getEnv) c
  if unf || (← isInstance c) || (← getProjectionFnInfo? c).isSome then
    let e' ← withCanUnfoldPred (fun _ ci => canUnfold u ci.name) do
      if unf && !isCasesOnRecursor (← getEnv) c then
        let some e' ← unfoldDefinition? e | return e
        whnfCore e'
      else
        whnf e
    if e' == e then return none
    -- An instance or a class's projection only if that reaches a value (`x ++ y` on bit
    -- vectors stays), and so a Boolean operation (`!x` on a variable stays).
    if (!unf || valueOnly.contains c) && !(← isValue e') then return none
    -- A stuck projection back to the projection function (`s.1` to `s.gpr`).
    if let .proj S i x := e'.getAppFn then
      let some field := (getStructureFields (← getEnv) S)[i]? | return none
      return some (mkAppN (← mkProjection x field) e'.getAppArgs)
    return some e'
  return none

mutual

/-- Evaluates `e` (see the module documentation). -/
partial def visit (e : Expr) : M Expr := do
  if let some r := (← get)[e]? then return r
  let r ← visitCore e
  modify (·.insert e r)
  return r

partial def visitCore (e : Expr) : M Expr := do
  match e with
  | .mdata _ b => visit b
  | .letE _ _ v b _ => visit (b.instantiate1 v)
  | .lam n d b bi => do
      let d ← visit d
      withLocalDecl n bi d fun x => do mkLambdaFVars #[x] (← visit (b.instantiate1 x))
  | .forallE n d b bi => do
      let d ← visit d
      withLocalDecl n bi d fun x => do mkForallFVars #[x] (← visit (b.instantiate1 x))
  | .proj _ i s => do
      let s' ← visit s
      if let some v ← projectCore? s' i then visit v else return e.updateProj! s'
  | .app .. | .const .. =>
      let f := e.getAppFn
      -- A definition to unfold on evaluated arguments (so that its recursion reduces at once).
      if let .const c _ := f then
        let classProj := (← getProjectionFnInfo? c).any (·.fromClass)
        if ((← read).unfold.contains c && !valueOnly.contains c) || classProj then
          let e' := mkAppN f (← e.getAppArgs.mapM visit)
          if let some e'' ← step e' then return (← visit e'')
          return e'
      if let some e' ← step e then return (← visit e')
      let args ← e.getAppArgs.mapM visit
      let e' := mkAppN f args
      if e' == e then return e
      if let some e'' ← step e' then return (← visit e'')
      return e'
  | _ => return e

end

/-- The definitions `sigReduce` always unfolds: those of `Sig.contract` and of
the lists it computes with. -/
def defaultUnfold : List Name :=
  [`VG.Sig.contract, `VG.Sig.words, `VG.Param.words, `VG.Param.pubs, `VG.Sig.bufs,
    `VG.Sig.lists, `VG.Sig.descs, `VG.Sig.descRegion,
    `VG.Sig.retBits, `VG.ArgWord.bits, `VG.ArgWord.ofRaw, `VG.IntTy.bits, `VG.Elem.size,
    `VG.stackBelow, `VG.Curry.apply, `VG.Curry.const, `VG.Sig.wfPre, ``List.map, ``List.filter,
    ``List.flatMap, ``List.flatten, ``List.append, ``List.zip, ``List.zipWith, ``List.sum,
    ``List.foldr, ``List.foldl, ``List.all, ``List.any, ``List.length, ``List.take, ``List.drop,
    ``List.head?, ``List.headD, ``List.tail, ``List.filterMap, `VG.Sig.conj, `VG.Sig.pairFacts,
    `VG.Sig.pubFacts, `VG.Sig.preE, `VG.Sig.pubE,
    ``and, ``or, ``not,
    ``cond]

/-- Evaluates `e`, unfolding the definitions `unfold` (and `defaultUnfold`). -/
def sigReduce (unfold : NameSet) (e : Expr) : MetaM Expr :=
  (visit e).run { unfold := defaultUnfold.foldl (·.insert ·) unfold } |>.run' {}

open Elab Tactic in
/-- `sig_reduce [defs] (at h)?` evaluates the goal (or `h`) with
`sigReduce`, unfolding the definitions among `defs` (other names, e.g. of
lemmas, are ignored), and replaces it by the result, which the kernel checks
to be definitionally equal. -/
elab "sig_reduce " "[" ls:Lean.Parser.Tactic.simpLemma,* "]" loc:(Lean.Parser.Tactic.location)? :
    tactic => do
  let mut u : NameSet := {}
  for l in ls.getElems do
    let t := l.raw[2]
    unless t.isIdent do continue
    let cs ← try realizeGlobalConstWithInfos t catch _ => pure []
    for c in cs do
      if let some (.defnInfo _) := (← getEnv).find? c then u := u.insert c
  let unfold := u
  let loc := expandOptLocation (mkOptionalNode loc)
  withLocation loc
    (fun fvarId => liftMetaTactic1 fun g => g.withContext do
      let ty ← instantiateMVars (← fvarId.getType)
      let ty' ← sigReduce unfold ty
      if ty' == ty then return g
      g.replaceLocalDeclDefEq fvarId ty')
    (liftMetaTactic1 fun g => g.withContext do
      let ty ← instantiateMVars (← g.getType)
      let ty' ← sigReduce unfold ty
      if ty' == ty then return g
      g.replaceTargetDefEq ty')
    (fun _ => pure ())

open Elab Tactic in
/-- Closes each goal that is a hypothesis, syntactically, or the symmetric
`Region.Disjoint` of one (`VG.Region.Disjoint.symm`), with one lookup per goal
in a table of the hypotheses; leaves the other goals. -/
elab "sig_close" : tactic => do
  let mut rest := #[]
  for g in ← getGoals do
    if ← g.isAssigned then continue
    let closed ← g.withContext do
      let mut hyps : Std.HashMap Expr Expr := {}
      for d in ← getLCtx do
        if d.isImplementationDetail then continue
        hyps := hyps.insert (← instantiateMVars d.type).consumeMData d.toExpr
      let t := (← instantiateMVars (← g.getType)).consumeMData
      if let some h := hyps[t]? then
        g.assign h
        return true
      if t.isAppOfArity `VG.Region.Disjoint 2 then
        let t' := mkApp2 t.appFn!.appFn! t.appArg! t.appFn!.appArg!
        if let some h := hyps[t']? then
          g.assign (mkApp3 (mkConst `VG.Region.Disjoint.symm) t.appArg! t.appFn!.appArg! h)
          return true
      return false
    unless closed do rest := rest.push g
  setGoals rest.toList

open Elab Tactic in
/-- Closes a goal `Region.Disjoint a b` with `Region.Disjoint.symm h` for a
hypothesis `h : Region.Disjoint b a` (up to reducible unfolding), as
`with_reducible exact Region.Disjoint.symm ‹_›` does, but fails at once on any
other goal, where elaborating that term searches the hypotheses first. -/
elab "sig_disjoint_symm" : tactic => liftMetaTactic fun g => g.withContext do
  let t ← whnfR (← instantiateMVars (← g.getType))
  unless t.isAppOfArity `VG.Region.Disjoint 2 do throwError "sig_disjoint_symm: not a disjointness"
  let a := t.appFn!.appArg!
  let b := t.appArg!
  let t' := mkApp2 t.appFn!.appFn! b a
  for d in ← getLCtx do
    if d.isImplementationDetail then continue
    if ← withReducible (isDefEq d.type t') then
      g.assign (mkApp3 (mkConst `VG.Region.Disjoint.symm) b a d.toExpr)
      return []
  throwError "sig_disjoint_symm: no hypothesis"

open Elab Tactic in
/-- On a goal `(k).pre s` where `k` unfolds to `Sig.contract …`, applies
`VG.Sig.contract_pre_of_check` (`Proof/Framework/Contract.lean`) to the
arguments of `Sig.contract` directly, rather than finding them by
unification; leaves its two hypotheses as goals. -/
elab "sig_apply_check" : tactic => liftMetaTactic fun g => g.withContext do
  let t ← instantiateMVars (← g.getType)
  unless t.isAppOfArity `VG.Contract.pre 3 do throwError "sig_apply_check: not a precondition"
  let s := t.appArg!
  let mut k := t.appFn!.appArg!
  for _ in [0:32] do
    if k.isAppOfArity `VG.Sig.contract 8 then break
    let some k' ← unfoldDefinition? k | break
    k := k'.headBeta
  unless k.isAppOfArity `VG.Sig.contract 8 do
    throwError "sig_apply_check: not a contract built with `Sig.contract`"
  let f := mkAppN (mkConst `VG.Sig.contract_pre_of_check) (k.getAppArgs.push s)
  let .forallE _ hcTy (.forallE _ hTy _ _) _ ← whnfR (← inferType f)
    | throwError "sig_apply_check: unexpected type"
  let hc ← mkFreshExprSyntheticOpaqueMVar hcTy
  let h ← mkFreshExprSyntheticOpaqueMVar hTy
  g.assign (mkApp2 f hc h)
  return [hc.mvarId!, h.mvarId!]

/-- Splits the hypothesis `h`, a conjunction `a₁ ∧ … ∧ aₙ` (up to unfolding,
along the right), into hypotheses `a₁`, …, `aₙ₋₁` (anonymous) and `aₙ` (named
`h`), in one step. -/
def splitAnds (g : MVarId) (h : FVarId) : MetaM MVarId := g.withContext do
  let d ← h.getDecl
  let mut t ← instantiateMVars d.type
  let mut prf := mkFVar h
  let mut hyps : Array Hypothesis := #[]
  for _ in [0:10000] do
    let t' ← whnfD t
    unless t'.isAppOfArity ``And 2 do break
    let a := t'.appFn!.appArg!
    let b := t'.appArg!
    hyps := hyps.push { userName := ← mkFreshUserName `h, type := a, value := mkApp3 (mkConst ``And.left) a b prf }
    prf := mkApp3 (mkConst ``And.right) a b prf
    t := b
  if hyps.isEmpty then return g
  hyps := hyps.push { userName := d.userName, type := t, value := prf }
  let (_, g) ← g.assertHypotheses hyps
  g.tryClear h

/-- Splits a goal `a₁ ∧ … ∧ aₙ` (nested either way) into goals `a₁`, …, `aₙ`. -/
partial def andIntros (g : MVarId) : MetaM (List MVarId) := g.withContext do
  let t ← whnfR (← instantiateMVars (← g.getType))
  unless t.isAppOfArity ``And 2 do return [g]
  let a := t.appFn!.appArg!
  let b := t.appArg!
  let ga ← mkFreshExprSyntheticOpaqueMVar a (← g.getTag)
  let gb ← mkFreshExprSyntheticOpaqueMVar b (← g.getTag)
  g.assign (mkApp4 (mkConst ``And.intro) a b ga gb)
  return (← andIntros ga.mvarId!) ++ (← andIntros gb.mvarId!)

open Elab Tactic in
/-- `sig_split h`: `splitAnds`. -/
elab "sig_split " h:ident : tactic => withMainContext do
  let fv ← getFVarId h
  liftMetaTactic1 fun g => return some (← splitAnds g fv)

open Elab Tactic in
/-- `sig_and_intros`: `andIntros` on every goal. -/
elab "sig_and_intros" : tactic => do
  let gs ← getGoals
  let mut out := []
  for g in gs do
    if ← g.isAssigned then continue
    out := out ++ (← andIntros g)
  setGoals out

/-- The arguments of `Sig.contract` that `k` unfolds to. -/
def contractArgs? (k : Expr) : MetaM (Option (Array Expr)) := do
  let mut k := k
  for _ in [0:32] do
    if k.isAppOfArity `VG.Sig.contract 8 then return some k.getAppArgs
    let some k' ← unfoldDefinition? k | return none
    k := k'.headBeta
  return none

/-- For `t` the precondition or public data of a contract built with
`Sig.contract`: the proof of `t ↔ t'` (`VG.Sig.contract_pre_iff`,
`VG.Sig.contract_pub_iff`) and `t'`. `some none` for its postcondition. -/
def contractIff? (t : Expr) : MetaM (Option (Option (Expr × Expr))) := do
  let t ← instantiateMVars t
  if t.isAppOfArity `VG.Contract.pre 3 then
    let some a ← contractArgs? t.appFn!.appArg! | return none
    let s := t.appArg!
    return some (some (mkAppN (mkConst `VG.Sig.contract_pre_iff) (a.push s),
      mkAppN (mkConst `VG.Sig.preE) #[a[0]!, a[1]!, a[2]!, a[3]!, a[5]!, a[6]!, s]))
  if t.isAppOfArity `VG.Contract.pub 4 then
    let some a ← contractArgs? t.appFn!.appFn!.appArg! | return none
    let s₁ := t.appFn!.appArg!
    let s₂ := t.appArg!
    return some (some (mkAppN (mkConst `VG.Sig.contract_pub_iff) ((a.push s₁).push s₂),
      mkAppN (mkConst `VG.Sig.pubE) #[a[0]!, a[1]!, a[2]!, a[7]!, s₁, s₂]))
  if t.isAppOfArity `VG.Contract.post 4 then
    if (← contractArgs? t.appFn!.appFn!.appArg!).isSome then return some none
  return none

open Elab Tactic in
/-- `sig_iff (at h)?` restates the precondition or public data of a contract
built with `Sig.contract` (the goal or `h`) with its facts about lists as
`Sig.conj`s, which `sig_reduce` evaluates (`Sig.preE`, `Sig.pubE`); does
nothing to its postcondition, and fails on anything else. -/
elab "sig_iff" loc:(Lean.Parser.Tactic.location)? : tactic => do
  let loc := expandOptLocation (mkOptionalNode loc)
  withLocation loc
    (fun fvarId => liftMetaTactic1 fun g => g.withContext do
      let some r ← contractIff? (← fvarId.getType) | throwError "sig_iff: not a contract"
      let some (prf, t') := r | return g
      let r ← g.replace fvarId (mkApp4 (mkConst ``Iff.mp) (← fvarId.getType) t' prf (mkFVar fvarId))
        (some t')
      return r.mvarId)
    (liftMetaTactic1 fun g => g.withContext do
      let t ← g.getType
      let some r ← contractIff? t | throwError "sig_iff: not a contract"
      let some (prf, t') := r | return g
      let g' ← mkFreshExprSyntheticOpaqueMVar t' (← g.getTag)
      g.assign (mkApp4 (mkConst ``Iff.mpr) t t' prf g')
      return g'.mvarId!)
    (fun _ => throwError "sig_iff: not a contract")

end VG.Sig.Eval
