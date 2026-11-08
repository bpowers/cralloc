import Cralloc.TCB.Emit
import Cralloc.TCB.Axioms
import Cralloc.TCB.Audit

/-!
# The emitter

Renders every artifact (those of each registration file under
`Cralloc/Artifacts/`, see `Cralloc/TCB/Emit.lean`) into Rust, by default
under `../crates/cralloc/src/front/generated/` (relative to `lean/`), where
the allocator compiles them. Run it from `lean/`:

* `lake env lean --run Emit.lean [DIR]` writes the files (and deletes stale
  `.rs` files).
* `lake env lean --run Emit.lean --check [DIR]` writes nothing and fails if
  the files on disk differ from what would be generated.

It first builds the registration files and what they import (`lake build`,
which is quick when nothing changed), so that it never emits from stale
proofs. Then, in this process, it imports them with no extension loaded, which runs none of
their code, and audits the lists of artifacts they define; and only then imports them again to
run them, and evaluates and emits those same lists (`VG.Emit.run`). Nothing it runs is elaborated
in an environment that imports them, so no syntax they define (a macro rewriting a command, say)
can change what is audited or emitted. The audits:

* everything the lists depend on uses only Lean's standard axioms
  (`VG.axiomsOf`, which walks the declarations themselves);
* no override remains, after the build, on the lists and `VG.Emit.run`, and no module declares an
  initializer (`VG.compilerOverrides`);
* every `VG.Spec` definition is declared in `Spec/` (`VG.specOutsideSpec`);
* every target is built in `TCB/` and every `Api` in `Spec/` (`VG.builtOutside`);
* every module loaded is Lean's or under `Cralloc` (`VG.foreignModules`).

That each declaration was checked by the kernel at all (and not, say,
skipped with `debug.skipKernelTC`) is checked separately, by replaying every
module under `Cralloc` through the kernel (`leanchecker`, in `scripts/check.sh`), and so every
module loaded but Lean's. What this does not defend against is in `README.md`, "What the audits
defend against".
-/

open Lean

/-- The problems the audits find with the artifact lists `roots` in `env`,
one line each. -/
def audit (env : Environment) (roots : List Name) : List String :=
  let (axioms, missing) := VG.axiomsOf env roots
  (axioms.filter (!VG.standardAxioms.contains ·)).map (s!"non-standard axiom: {·}") ++
  missing.map (s!"unknown constant: {·}") ++
  VG.compilerOverrides env (roots ++ [``VG.Emit.run]) ++
  (VG.specOutsideSpec env).map (fun (n, m) => s!"`{n}` is declared outside `Spec/`, in {m}") ++
  (VG.builtOutside env).map (fun (n, c, m) => s!"`{n}` builds a `{c}` outside what may, in {m}") ++
  (VG.foreignModules env).map fun m => s!"`{m}` is loaded, and is neither Lean's nor under `Cralloc`"

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
      args := #["build", "--log-level=warning", "+Cralloc.TCB.Emit"] ++
        (mods.map ("+" ++ ·)).toArray }
  let code ← build.wait
  unless code == 0 do return code
  initSearchPath (← findSysroot)
  let imports := ("Cralloc.TCB.Emit" :: mods).map fun m => ({ module := m.toName } : Import)
  let roots := mods.map fun m => (VG.Emit.listOf m).toName
  -- The audit comes first, on an import with no extension loaded, which runs no code of what it
  -- imports: an initializer it would refuse must not get to run before it.
  let plain ← importModules imports.toArray {} (loadExts := false)
  let problems := audit plain roots
  unless problems.isEmpty do
    for p in problems do IO.eprintln p
    IO.eprintln "the audit failed: nothing was emitted"
    return 1
  IO.println s!"audited {roots.length} registration files: standard axioms only, no compiler \
    override remains after the build, no initializer, every `VG.Spec` definition is from \
    `Spec/`, every target and `Api` is built where it may be, and every module loaded is Lean's \
    or under `Cralloc`"
  -- Evaluating the lists needs the extensions, whose loading runs the modules' initializers, and
  -- the audit found none but Lean's.
  enableInitializersExecution
  let env ← importModules imports.toArray {} (loadExts := true)
  match evalAll env roots with
  | .ok arts => VG.Emit.run arts args
  | .error e =>
    IO.eprintln e
    return 1
