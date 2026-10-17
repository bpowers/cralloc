import Asmalloc.TCB.Emit
import Asmalloc.TCB.Axioms
import Asmalloc.TCB.Audit

/-!
# The emitter

Renders every artifact (those of each registration file under
`Asmalloc/Artifacts/`, see `Asmalloc/TCB/Emit.lean`) into Rust, by default
under `generated/` (relative to `lean/`). Run it from `lean/`:

* `lake env lean --run Emit.lean [DIR]` writes the files (and deletes stale
  `.rs` files).
* `lake env lean --run Emit.lean --check [DIR]` writes nothing and fails if
  the files on disk differ from what would be generated.

It first builds the registration files and what they import (`lake build`,
which is quick when nothing changed), so that it never emits from stale
proofs. Then, in this process, it imports them (`importModules`), audits
the lists of artifacts they define, and evaluates and emits those same
lists (`VG.Emit.run`). Nothing it runs is elaborated in an environment that
imports them, so no syntax they define (a macro rewriting a command, say)
can change what is audited or emitted. The audits:

* everything the lists depend on uses only Lean's standard axioms
  (`VG.axiomsOf`, which walks the declarations themselves);
* the compiled code of the lists and of `VG.Emit.run` is what the kernel
  checked (`VG.compilerOverrides`);
* every `VG.Spec` definition is declared in `Spec/` (`VG.specOutsideSpec`).

That each declaration was checked by the kernel at all (and not, say,
skipped with `debug.skipKernelTC`) is checked separately, by replaying every
module through the kernel (`leanchecker`, in `scripts/check.sh`).
-/

open Lean

/-- The problems the audits find with the artifact lists `roots` in `env`,
one line each. -/
def audit (env : Environment) (roots : List Name) : List String :=
  let (axioms, missing) := VG.axiomsOf env roots
  (axioms.filter (!VG.standardAxioms.contains ·)).map (s!"non-standard axiom: {·}") ++
  missing.map (s!"unknown constant: {·}") ++
  VG.compilerOverrides env (roots ++ [``VG.Emit.run]) ++
  (VG.specOutsideSpec env).map fun (n, m) => s!"`{n}` is declared outside `Spec/`, in {m}"

/-- The artifact lists `roots`, evaluated in `env`, in order. Each is a
constant of type `List VG.Artifact` (`registrations` names them, and the
audit found each): `evalConst` takes a type of `Type` only, so it returns
the object as an opaque one, which is then cast back to its type. -/
unsafe def evalAll (env : Environment) (roots : List Name) : Except String (List VG.Artifact) :=
  roots.foldlM (init := []) fun acc r => do
    let some ci := env.find? r | .error s!"{r} is not defined"
    unless ci.type == mkApp (mkConst ``List [1]) (mkConst ``VG.Artifact) do
      .error s!"{r} is not a `List VG.Artifact`"
    match env.evalConst NonScalar {} r (checkMeta := false) with
    | .ok v => .ok (acc ++ (unsafeCast v : List VG.Artifact))
    | .error e => .error s!"cannot evaluate {r}: {e}"

unsafe def main (args : List String) : IO UInt32 := do
  let some _ := VG.Emit.parseArgs args
    | IO.eprintln VG.Emit.usage; return 2
  let mods ← VG.Emit.registrations
  let build ← IO.Process.spawn
    { cmd := "lake",
      args := #["build", "--log-level=warning", "+Asmalloc.TCB.Emit"] ++
        (mods.map ("+" ++ ·)).toArray }
  let code ← build.wait
  unless code == 0 do return code
  initSearchPath (← findSysroot)
  -- Evaluating the lists needs the imported environment's extensions, whose
  -- loading runs the imported modules' initializers. No module of this
  -- project may declare one: `scripts/check_lean_imports.py` refuses the
  -- syntax, and `audit` (`VG.compilerOverrides`) the declarations.
  enableInitializersExecution
  let imports := ("Asmalloc.TCB.Emit" :: mods).map fun m => ({ module := m.toName } : Import)
  let env ← importModules imports.toArray {} (loadExts := true)
  let roots := mods.map fun m => (VG.Emit.listOf m).toName
  let problems := audit env roots
  unless problems.isEmpty do
    for p in problems do IO.eprintln p
    IO.eprintln "the audit failed: nothing was emitted"
    return 1
  IO.println s!"audited {roots.length} registration files: standard axioms only, the compiled \
    code is what the kernel checked, and every `VG.Spec` definition is from `Spec/`"
  match evalAll env roots with
  | .ok arts => VG.Emit.run arts args
  | .error e =>
    IO.eprintln e
    return 1
