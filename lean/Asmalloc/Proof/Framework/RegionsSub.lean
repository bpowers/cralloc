module

public import Lean.Elab.Tactic.Simp
public meta import Lean.Elab.Tactic.Simp

/-!
# `regions_sub`: one list's elements among another's

`regions_sub` proves `∀ r ∈ l₁, r ∈ l₂` when `l₁` and `l₂` are built from
list literals and `++`, and each element of `l₁` is written in `l₂` as it is
in `l₁` (as `Frame.mono` and the like need). It finds each element and builds
the membership proof directly (`List.forall_mem_cons`, `List.mem_cons_self`,
…), which costs next to nothing where `simp` took 0.1–0.4M heartbeats; any
other goal it leaves to `simp`.
-/

public meta section

namespace VG.RegionsSub

open Lean Elab Tactic Meta

/-- `l₁ ++ l₂`, as its two lists. -/
def append? (e : Expr) : Option (Expr × Expr) :=
  if e.isAppOfArity ``HAppend.hAppend 6 then some (e.getArg! 4, e.getArg! 5) else none

/-- A proof of `a ∈ l`, if `a` is written in `l`. -/
partial def mem (u : Level) (α a l : Expr) : Option Expr :=
  if l.isAppOfArity ``List.cons 3 then
    let y := l.getArg! 1; let t := l.getArg! 2
    if y == a then some (mkApp3 (mkConst ``List.mem_cons_self [u]) α a t)
    else (mkApp5 (mkConst ``List.mem_cons_of_mem [u]) α y a t ·) <$> mem u α a t
  else if let some (l₁, l₂) := append? l then
    match mem u α a l₁ with
    | some h => some (mkApp5 (mkConst ``List.mem_append_left [u]) α a l₁ l₂ h)
    | none => (mkApp5 (mkConst ``List.mem_append_right [u]) α a l₁ l₂ ·) <$> mem u α a l₂
  else none

/-- `Iff.mpr` of an `iff` whose sides its type gives. -/
def iffMpr (iff h : Expr) : MetaM Expr := do
  let_expr Iff a b := ← inferType iff | throwError "regions_sub: not an iff"
  return mkApp4 (mkConst ``Iff.mpr) a b iff h

/-- A proof of `∀ x ∈ l, x ∈ l₂` (`p := fun x => x ∈ l₂`), if each element of
`l` is written in `l₂`. -/
partial def all (u : Level) (α p l₂ l : Expr) : MetaM (Option Expr) := do
  if l.isAppOfArity ``List.nil 1 then
    return some (mkApp2 (mkConst ``List.forall_mem_nil [u]) α p)
  if l.isAppOfArity ``List.cons 3 then
    let a := l.getArg! 1; let t := l.getArg! 2
    let some ha := mem u α a l₂ | return none
    let some ht ← all u α p l₂ t | return none
    let iff := mkApp4 (mkConst ``List.forall_mem_cons [u]) α p a t
    let_expr Iff _ b := ← inferType iff | return none
    let_expr And pa pt := b | return none
    return some (← iffMpr iff (mkApp4 (mkConst ``And.intro) pa pt ha ht))
  if let some (l₁, l₁') := append? l then
    let some h₁ ← all u α p l₂ l₁ | return none
    let some h₂ ← all u α p l₂ l₁' | return none
    let iff := mkApp4 (mkConst ``List.forall_mem_append [u]) α p l₁ l₁'
    let_expr Iff _ b := ← inferType iff | return none
    let_expr And p₁ p₂ := b | return none
    return some (← iffMpr iff (mkApp4 (mkConst ``And.intro) p₁ p₂ h₁ h₂))
  return none

/-- The proof `regions_sub` builds for the goal `t`, if it can. -/
def proof (t : Expr) : MetaM (Option Expr) := do
  let .forallE _ α (.forallE _ h₁ h₂ _) _ := t | return none
  let_expr Membership.mem _ _ _ l₁ x₁ := h₁ | return none
  let_expr Membership.mem _ _ _ l₂ x₂ := h₂ | return none
  unless x₁ == .bvar 0 && x₂ == .bvar 1 && !l₁.hasLooseBVars && !l₂.hasLooseBVars do return none
  let .succ u ← getLevel α | return none
  let p := mkLambda `x .default α (h₂.lowerLooseBVars 1 1)
  all u α p l₂ l₁

elab "regions_sub" : tactic => do
  let g ← getMainGoal
  let t ← instantiateMVars (← g.getType)
  if let some pf ← proof t then
    if ← withReducible (isDefEq (← inferType pf) t) then
      g.assign pf
      replaceMainGoal []
      return
  evalTactic (← `(tactic| simp))

end VG.RegionsSub
