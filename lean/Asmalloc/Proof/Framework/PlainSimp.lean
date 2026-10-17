module

public meta import Lean.Elab.Tactic.Basic

/-!
# `plain_simp_names`: the simp lemmas a macro writes, by their constants

Untrusted: this only changes how the names of simp lemmas are elaborated, not
which lemmas they are.

In a module with many imports, `simp` elaborates a lemma name that carries
macro scopes (any name a hygienic macro writes) an order of magnitude slower
than the plain name of a constant. `plain_simp_names t` runs the tactic `t`
with each such name replaced by the constant the quotation already resolved
it to. It imports only Lean core, so that any macro can use it without
importing the framework.
-/

public meta section

namespace VG

open Lean Elab Tactic in
/-- The simp lemma `l` with its name, if a macro wrote it (with macro scopes),
replaced by the global constant it resolves to. In a module with many imports,
`simp` elaborates a name with macro scopes an order of magnitude slower than
the plain name of a constant (about 13K heartbeats more each: 1.2M for the
lemmas of `sig_eval`, at every use). -/
def plainSimpLemma (l : Syntax) : TacticM Syntax := do
  let t := l[2]
  let .ident _ _ n pre := t | return l
  unless n.hasMacroScopes do return l
  -- The constant the quotation resolved the name to, if it is the only one.
  let c ← match pre with
    | [.decl c []] => pure c
    | _ =>
      let cs ← try realizeGlobalConstWithInfos t catch _ => pure []
      let [c] := cs | return l
      pure c
  if isPrivateName c || !(← getEnv).contains c then return l
  return l.setArg 2 (mkIdentFrom t (`_root_ ++ c))

open Lean Elab Tactic in
/-- `plainSimpLemma` on every simp lemma in `stx`. -/
partial def plainSimpNames (stx : Syntax) : TacticM Syntax := do
  if stx.getKind == ``Lean.Parser.Tactic.simpLemma then return ← plainSimpLemma stx
  match stx with
  | .node info k args => return .node info k (← args.mapM plainSimpNames)
  | _ => return stx

open Lean Elab Tactic in
/-- The simp lemmas `xs`, their names elaborated as plain names (`plainSimpLemma`). -/
def unhygienicSimpArgs (xs : Array (TSyntax ``Lean.Parser.Tactic.simpLemma)) :
    TacticM (Array (TSyntax ``Lean.Parser.Tactic.simpLemma)) :=
  xs.mapM fun l => return ⟨← plainSimpLemma l⟩

/-- `plain_simp_names t` runs the tactic `t` (any `simp`, `simp_all`, `dsimp`,
`simpa`, or a block of tactics) with the names of the simp lemmas a macro
wrote replaced by the constants they resolve to (`plainSimpLemma`): the same
lemmas, elaborated an order of magnitude faster. Wrap a macro's expansion in
it: `` `(tactic| plain_simp_names (simp only [foo, bar] at h)) ``. -/
syntax "plain_simp_names " tactic : tactic

open Lean Elab Tactic in
elab_rules : tactic
  | `(tactic| plain_simp_names $t) => do evalTactic (← plainSimpNames t)

end VG
