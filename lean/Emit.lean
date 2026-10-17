import Asmalloc.TCB.Emit

/-!
# The emitter

Renders every artifact (those of `Asmalloc/Artifacts.lean`, of
each registration file under `Asmalloc/Artifacts/`, and of each
generic file under `Asmalloc/Generic/` for each variant under
`Asmalloc/Variants/`, see `Asmalloc/TCB/Emit.lean`) into Rust, under `src/asm/` of the crate
(by default `../src/asm`, relative to `lean/`). Run it from `lean/` after
`lake build` (which checks every proof):

* `lake env lean --run Emit.lean [DIR]` writes the files (and deletes stale
  `.rs` files).
* `lake env lean --run Emit.lean --check [DIR]` writes nothing and fails if
  the files on disk differ from what would be generated; CI runs this.

It writes a program for each target directory that imports its
registration files (`.lake/emit/Driver<Target>.lean`, `VG.Emit.parts`) and
runs them at once, as processes with the same arguments, each auditing and
emitting the artifacts of its targets (`VG.Emit.run`'s `--part`); and then,
if they all pass, checks that no target is in two of them and writes or
checks the root module (`VG.Emit.finish`). (They are run by the
interpreter rather than as a `lean_exe` so that CI does not have to compile
Mathlib to native code.) The interpreter runs the trusted base, which
does the printing and checking, as the compiled code of the `NativeTCB`
library (`lakefile.toml`), which `lake build` builds, and so an order of
magnitude faster.
-/

/-- The shared library of `NativeTCB`, as Lake names it. -/
def nativeTCB : System.FilePath :=
  ".lake" / "build" / "lib" /
    if System.Platform.isWindows then "Asmalloc_NativeTCB.dll"
    else s!"libAsmalloc_NativeTCB.{if System.Platform.isOSX then "dylib" else "so"}"

/-- How many `.olean` files the driver may load: those of this project and
of its packages. -/
partial def oleanCount (dir : System.FilePath) : IO Nat := do
  if !(← dir.isDir) then return 0
  let mut n := 0
  for e in ← dir.readDir do
    if ← e.path.isDir then n := n + (← oleanCount e.path)
    else if e.path.extension == some "olean" then n := n + 1
  return n

/-- On Linux, a process may map at most `vm.max_map_count` regions of memory,
and the driver maps each module it imports, with its `.olean.server` and
`.olean.private` parts, since it is not a module and sees every module's
private part. Past the limit, Lean fails with `failed to read file
'….olean.private'`, naming whichever module it was loading. The driver
needed about six regions per `.olean` (80,144 for 13,700); require eight.
`none` if the limit is high enough, or not Linux. -/
def mapCountProblem : IO (Option String) := do
  let limitFile : System.FilePath := "/proc/sys/vm/max_map_count"
  unless System.Platform.isLinux && (← limitFile.pathExists) do return none
  let some limit := (← IO.FS.readFile limitFile).trimAscii.toString.toNat? | return none
  let mut oleans ← oleanCount (".lake" / "build" / "lib" / "lean")
  let packages : System.FilePath := ".lake" / "packages"
  if ← packages.isDir then
    for e in ← packages.readDir do
      oleans := oleans + (← oleanCount (e.path / ".lake" / "build" / "lib" / "lean"))
  let needed := 8 * oleans
  if limit ≥ needed then return none
  return some s!"vm.max_map_count is {limit}, and the driver may map about {needed} \
    regions of memory ({oleans} .olean files): raise it, e.g. \
    `sudo sysctl -w vm.max_map_count=262144` (see lean/README.md)"

def main (args : List String) : IO UInt32 := do
  unless ← nativeTCB.pathExists do
    IO.eprintln s!"{nativeTCB} is missing: run `lake build` first"
    return 1
  if let some problem ← mapCountProblem then
    IO.eprintln problem
    return 1
  let some (check, none, _) := VG.Emit.parseArgs args
    | IO.eprintln VG.Emit.usage; return 2
  let parts ← match VG.Emit.parts (← VG.Emit.registrations)
      (← VG.Emit.genericGroups VG.Emit.genericDir) (← VG.Emit.grouped VG.Emit.variantDir) with
    | .ok parts => pure parts
    | .error e => do IO.eprintln e; return 1
  let emitDir : System.FilePath := ".lake" / "emit"
  IO.FS.createDirAll emitDir
  let lean ← IO.appPath
  -- Each part's driver, run at once.
  let mut children : Array (System.FilePath × IO.Process.Child {}) := #[]
  for (t, mods, gens, vars) in parts do
    let driver := emitDir / s!"Driver{t}.lean"
    let targets := emitDir / s!"{t}.targets"
    IO.FS.writeFile driver (VG.Emit.driver mods gens vars)
    if ← targets.pathExists then IO.FS.removeFile targets
    let partArgs := (if check then ["--check"] else []) ++ ["--part", targets.toString] ++
      (if check then args.drop 1 else args)
    children := children.push (targets, ← IO.Process.spawn
      { cmd := lean.toString,
        args := #[s!"--load-dynlib={nativeTCB}", "--run", driver.toString] ++ partArgs.toArray })
  let mut code := 0
  for (_, child) in children do
    let c ← child.wait
    if code == 0 then code := c
  unless code == 0 do return code
  let targets ← children.toList.mapM fun (f, _) => do
    let lines := ((← IO.FS.readFile f).splitOn "\n").filter fun (l : String) => !l.isEmpty
    return lines.map fun (l : String) =>
      match l.splitOn "\t" with
      | [t, c] => (t, c)
      | _ => (l, "")
  VG.Emit.finish targets args
