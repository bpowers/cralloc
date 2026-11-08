module

public meta import Lean

/-!
# Environment audit

**Trusted.** Two checks the emitter runs (`Emit.driver`) on everything it
imports, besides the axiom audit (`TCB/Axioms.lean`):

* `#assert_no_compiler_overrides all run` fails if the compiler may run
  something other than what the kernel checked in the code the emitter runs.
  The emitter is compiled code: it evaluates the artifacts `all` (their code,
  signatures and documentation) and runs `run`, which prints and checks them
  (`Rust.files`), by running what the compiler made of their definitions.
  So nothing of this project they depend on, transitively, may carry
  `implemented_by`, `extern` or `export`, which substitute other code for a
  definition (or give one to a constant the kernel knows nothing of); what
  Lean implements so is trusted, as Lean is. And no module of this
  project (`Cralloc` or a module under it) may declare a `csimp`
  theorem (or axiom), which the compiler rewrites by in whatever it compiles
  later, or an `init`/`builtin_init` (`initialize`), which runs when the
  emitter imports its module. Any of these could make the emitter write code
  other than the code that was proven, with every proof intact. Code the
  emitter does not run may use them: e.g. a tactic's `unsafe` term, whose
  results the kernel checks.
* `#assert_spec_origin` fails unless every constant named `VG.Spec.…` that
  is not a theorem is declared in a module under `Cralloc.Spec`. A
  reviewer reads a registration file's `Spec.<Alg>.fooContract` or
  `Spec.<Alg>.fooApi` as the reviewed one in `Spec/`, but a namespace is
  not a directory: an implementation's own files could declare such a name.
  Theorems (e.g. the equation lemmas Lean generates for a `Spec/` definition
  in the module that first unfolds it) carry no data and are exempt.
* `#assert_built_inside` fails if a definition outside `Cralloc.TCB` uses the
  constructor of a `Target`, `ISA`, `Printer`, `Abi` or `Artifact`, or one outside
  `Cralloc.Spec` that of an `Api`. A registration names a target and an `Api`, and what
  is emitted is theirs: a target of a registration's own, with another printer, or
  another `rely`, would have every proof go through and write other code. A safe
  definition has no other way to make a value of those types: so every target an
  emitted artifact holds was made in `TCB/`, whatever the registration's text says. That
  covers structure updates (`{ t with … }` is the constructor), and any definition, not
  only the emitted lists. Theorems carry no data and are exempt.

All three refuse to run in a module, which sees what it imports from other
modules without their proofs, and without the values of definitions they
do not expose: the emitter's driver is not a module.
-/

public meta section

namespace VG

open Lean Elab Command

/-- The modules of this project, `Cralloc` and those under it, with
their indices in `env`. -/
def projectModules (env : Environment) : List (ModuleIdx × Name) :=
  -- `moduleNames` builds a new array of every module on each call: read it once.
  let names := env.header.moduleNames
  (List.range names.size).filterMap fun i =>
    let m := names[i]!
    if (`Cralloc).isPrefixOf m then some (i, m) else none

/-- Whether `env`, whose modules are `names` (`env.header.moduleNames`, which
callers read once), declares `n` in the module being elaborated or in a
module of this project, rather than in Lean or a dependency (whose constants
never depend on this project's). -/
def isProjectConst (env : Environment) (names : Array Name) (n : Name) : Bool :=
  match env.getModuleIdxFor? n with
  | none => true
  | some i => (`Cralloc).isPrefixOf names[i.toNat]!

/-- The constants of this project (`isProjectConst`) that the compiled code
of the constants `roots` may run in `env`: those their values use,
transitively (including `roots`). Types and proofs are erased from compiled
code, so it follows neither the types of constants nor the values of
theorems (whose names it still includes). -/
def dependencies (env : Environment) (roots : List Name) : NameSet := Id.run do
  let names := env.header.moduleNames
  let mut seen : NameSet := {}
  let mut todo := roots
  -- Visits each constant once, so it ends.
  while !todo.isEmpty do
    let n := todo.head!
    todo := todo.tail
    if seen.contains n || !isProjectConst env names n then continue
    seen := seen.insert n
    if let some ci := env.find? n then
      unless ci matches .thmInfo _ do
        if let some v := ci.value? (allowOpaque := true) then
          todo := v.getUsedConstants.toList ++ todo
  return seen

/-- What makes the compiled code of the constants `roots` depend on in `env`
differ from what the kernel checked (see the module docs): one line each. -/
def compilerOverrides (env : Environment) (roots : List Name) : List String :=
  let deps := (dependencies env roots).toList.mergeSort (·.toString ≤ ·.toString)
  deps.flatMap (fun n =>
    (if (Compiler.getImplementedBy? env n).isSome then [s!"@[implemented_by] on {n}"] else []) ++
    (if isExtern env n then [s!"@[extern] on {n}"] else []) ++
    (if (getExportNameFor? env n).isSome then [s!"@[export] on {n}"] else [])) ++
  (projectModules env).flatMap fun (i, m) =>
    let param {α} (attr : ParametricAttribute α) (what : String) : List String :=
      (attr.ext.getModuleEntries env i).toList.map fun (d, _) => s!"{m}: {what} {d}"
    param regularInitAttr "@[init] on" ++ param builtinInitAttr "@[builtin_init] on" ++
    (Compiler.CSimp.ext.ext.getModuleEntries env i).toList.map fun
      | .global e | .scoped _ e => s!"{m}: @[csimp] {e.thmName}"

elab "#assert_no_compiler_overrides " ids:ident* : command => do
  -- In a module, a definition imported from another module may be opaque,
  -- without its value: `dependencies` would not follow what it runs.
  if (← getEnv).header.isModule then
    throwError "#assert_no_compiler_overrides must run in a file that is not a module"
  let roots ← ids.toList.mapM fun id => liftCoreM <| realizeGlobalConstNoOverloadWithInfo id.raw
  let bad := compilerOverrides (← getEnv) roots
  unless bad.isEmpty do
    throwError m!"the compiled code of {roots} is not what the kernel checked:\n\
      {"\n".intercalate bad}"
  logInfo m!"the compiled code of {roots} is what the kernel checked"

/-- The constants named `VG.Spec.…` in `env` that are not theorems and are
declared outside the modules under `Cralloc.Spec` (in another
module, or in the one being elaborated), with their modules. -/
def specOutsideSpec (env : Environment) : List (Name × Name) :=
  let names := env.header.moduleNames
  env.constants.fold (init := []) fun acc n c =>
    if !(`VG.Spec).isPrefixOf n || c matches .thmInfo _ then acc else
      let m := match env.getModuleIdxFor? n with
        | none => env.mainModule
        | some i => names[i.toNat]!
      if (`Cralloc.Spec).isPrefixOf m then acc else (n, m) :: acc

elab "#assert_spec_origin" : command => do
  -- In a module, a theorem imported from another module is an axiom, which
  -- the check would not exempt as one.
  if (← getEnv).header.isModule then
    throwError "#assert_spec_origin must run in a file that is not a module"
  let bad := specOutsideSpec (← getEnv)
  unless bad.isEmpty do
    throwError m!"these `VG.Spec` names are not declared in `Spec/`:\n\
      {"\n".intercalate (bad.map fun (n, m) => s!"{n} (in {m})")}"
  logInfo m!"every `VG.Spec` definition is declared in `Spec/`"

/-- The constructors only a part of the project may use: each with the prefixes of the
modules that may. `Api`'s own module declares what Lean generates for a structure, whose
types name its constructor. -/
def guardedConstructors : List (Name × List Name) :=
  [(`VG.Target.mk, [`Cralloc.TCB]), (`VG.ISA.mk, [`Cralloc.TCB]), (`VG.Printer.mk, [`Cralloc.TCB]),
    (`VG.Abi.mk, [`Cralloc.TCB]), (`VG.Artifact.mk, [`Cralloc.TCB]),
    (`VG.Api.mk, [`Cralloc.Spec, `Cralloc.TCB.Artifact])]

/-- The definitions of this project in `env` that use a guarded constructor outside the modules
that may, each with the constructor and its module. -/
def builtOutside (env : Environment) : List (Name × Name × Name) :=
  let names := env.header.moduleNames
  env.constants.fold (init := []) fun acc n c =>
    if c matches .thmInfo _ then acc else
      let m := match env.getModuleIdxFor? n with
        | none => env.mainModule
        | some i => names[i.toNat]!
      if !(`Cralloc).isPrefixOf m && (env.getModuleIdxFor? n).isSome then acc else
        let used := c.type.getUsedConstants ++
          (c.value? (allowOpaque := true)).elim #[] (·.getUsedConstants)
        guardedConstructors.foldl (init := acc) fun acc (g : Name × List Name) =>
          if !g.2.any (·.isPrefixOf m) && used.contains g.1 then (n, g.1, m) :: acc else acc

elab "#assert_built_inside" : command => do
  -- In a module, a definition imported from another module may have no value to look in.
  if (← getEnv).header.isModule then
    throwError "#assert_built_inside must run in a file that is not a module"
  let bad := builtOutside (← getEnv)
  unless bad.isEmpty do
    throwError m!"these definitions build what only the trusted files may:\n\
      {"\n".intercalate (bad.map fun (n, c, m) => s!"{n} (in {m}) uses {c}")}"
  logInfo m!"every target and `Api` is built where it may be"

end VG
