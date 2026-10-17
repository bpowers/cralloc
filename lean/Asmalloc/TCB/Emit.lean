module

public import Asmalloc.TCB.Rust

/-!
# The emitter

**Trusted.** What `Emit.lean` runs: which artifacts are emitted, and how the
generated files are written or checked.

The artifacts emitted are those of every *registration file*: each `.lean`
file under `Asmalloc/Artifacts/`, say `Asmalloc/Artifacts/Selftest/X86_64.lean`,
must define the list `VG.Artifacts.Selftest.X86_64.artifacts : List Artifact`.
Adding functions for a routine or a target adds such a file, so that
parallel changes don't all edit one list.

`Emit.lean` finds the registration files (`registrations`), imports them
into its own process, audits the lists they define (`TCB/Axioms.lean`,
`TCB/Audit.lean`), and evaluates and emits those lists (`run`).
-/

@[expose] public section

namespace VG.Emit

open System

/-- The directory of the registration files, relative to `lean/`. -/
def registrationDir : FilePath := "Asmalloc" / "Artifacts"

/-- The module of the Lean file `path` (relative to `lean/`), e.g.
`Asmalloc.Artifacts.Selftest.X86_64`. -/
def moduleOf (path : FilePath) : String :=
  ".".intercalate ((path.withExtension "").components.filter (· != "."))

/-- The modules of the `.lean` files under `dir`, in order of their names. -/
def modulesUnder (dir : FilePath) : IO (List String) := do
  unless ← dir.pathExists do return []
  let files ← dir.walkDir
  let mods := files.toList.filter (·.extension == some "lean") |>.map moduleOf
  return mods.mergeSort (· ≤ ·)

/-- The registration modules: the `.lean` files under `registrationDir`, in
order of their module names. -/
def registrations : IO (List String) := modulesUnder registrationDir

/-- The name of the list a registration module defines:
`Asmalloc.Artifacts.Selftest.X86_64` defines
`VG.Artifacts.Selftest.X86_64.artifacts`. -/
def listOf (mod : String) : String :=
  "VG." ++ (mod.dropPrefix "Asmalloc.").toString ++ ".artifacts"

def usage : String := "usage: lake env lean --run Emit.lean [--check] [DIR]"

/-- The directory the generated files go to, relative to `lean/`, unless
the command line names another. -/
def defaultDir : FilePath := "generated"

/-- `[--check] [DIR]`: whether to check rather than write, and the
directory (by default `defaultDir`). -/
def parseArgs (args : List String) : Option (Bool × FilePath) :=
  let (check, rest) := match args with
    | "--check" :: rest => (true, rest)
    | rest => (false, rest)
  match rest with
  | [] => some (check, defaultDir)
  -- An empty path would make `dir / name` absolute (`"" / "mod.rs"` is `/mod.rs`).
  | [d] => if d.startsWith "-" || d.isEmpty then none else some (check, d)
  | _ => none

/-- The `.rs` files under `dir` but those of `expected`, each with whether
this emitter wrote it (it starts with `Rust.header`). It does not follow a
symbolic link to a directory, which could lead outside `dir`. -/
def strays (dir : FilePath) (expected : List FilePath) : IO (List (FilePath × Bool)) := do
  unless ← dir.pathExists do return []
  let mut out := []
  let enter (p : FilePath) : IO Bool := do return (← p.symlinkMetadata).type != .symlink
  for path in ← dir.walkDir enter do
    if path.extension == some "rs" && !expected.contains path.normalize then
      out := out ++ [(path, (← IO.FS.readFile path).startsWith Rust.header)]
  return out

/-- Writes `files` (paths relative to `dir` and their contents) under `dir`,
deleting any other `.rs` file under `dir` that it wrote before; or, if
`check`, writes nothing and fails if the files on disk differ from `files`.
Any other `.rs` file there would be unverified code; one that it did not
write (without `Rust.header`) is not deleted but refused, before anything is
written, since a wrong `dir` must not destroy hand-written source. -/
def sync (dir : FilePath) (check : Bool) (files : List (String × String)) : IO Bool := do
  let others ← strays dir (files.map fun (n, _) => (dir / n).normalize)
  let foreign := others.filter (!·.2)
  unless foreign.isEmpty do
    for (path, _) in foreign do
      IO.eprintln s!"{path} was not written by Emit.lean: the output directory must hold only \
        generated files"
    return false
  let mut ok := true
  unless check do IO.FS.createDirAll dir
  for (name, text) in files do
    let path := dir / name
    if let some parent := path.parent then
      unless check do IO.FS.createDirAll parent
    if check then
      let current ← if ← path.pathExists then IO.FS.readFile path else pure ""
      if current != text then
        IO.eprintln s!"{path} is out of date"
        ok := false
    else
      IO.FS.writeFile path text
      IO.println s!"wrote {path}"
  for (path, _) in others do
    if check then
      IO.eprintln s!"{path} is not generated from any artifact"
      ok := false
    else
      IO.FS.removeFile path
      IO.println s!"removed stale {path}"
  return ok

/-- Writes the files generated from `as` under the directory in `args` (by
default `defaultDir`), deleting any other `.rs` file there that it wrote
(`sync`); or, if `args`
starts with `--check`, writes nothing and fails if the files on disk differ
from what would be generated. -/
def run (as : List Artifact) (args : List String) : IO UInt32 := do
  let some (check, dir) := parseArgs args | do IO.eprintln usage; return 2
  let files ← match Rust.files as with
    | .ok files => pure files
    | .error e => do IO.eprintln e; return 1
  unless ← sync dir check files do
    IO.eprintln "run `lake build && lake env lean --run Emit.lean` in lean/ and commit the result"
    return 1
  return 0

end VG.Emit
