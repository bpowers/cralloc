import Asmalloc.TCB.Emit

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
proofs. Then it writes a program that imports every registration file
(`.lake/emit/Driver.lean`, `VG.Emit.driver`) and runs it with the same
arguments: the program audits the artifacts and emits them (`VG.Emit.run`).
-/

def main (args : List String) : IO UInt32 := do
  let some _ := VG.Emit.parseArgs args
    | IO.eprintln VG.Emit.usage; return 2
  let mods ← VG.Emit.registrations
  let build ← IO.Process.spawn
    { cmd := "lake",
      args := #["build", "--log-level=warning", "+Asmalloc.TCB.Audit", "+Asmalloc.TCB.Axioms",
        "+Asmalloc.TCB.Emit"] ++ (mods.map ("+" ++ ·)).toArray }
  let code ← build.wait
  unless code == 0 do return code
  let emitDir : System.FilePath := ".lake" / "emit"
  IO.FS.createDirAll emitDir
  let driver := emitDir / "Driver.lean"
  IO.FS.writeFile driver (VG.Emit.driver mods)
  let lean ← IO.appPath
  let child ← IO.Process.spawn
    { cmd := lean.toString, args := #["--run", driver.toString] ++ args.toArray }
  child.wait
