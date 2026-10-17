module

public import Asmalloc.TCB.Rust

/-!
# The emitter

**Trusted.** What `Emit.lean` runs: which artifacts are emitted, and how the
generated files are written or checked.

The artifacts emitted are those of `Artifacts.lean` and of every
*registration file*: each `.lean` file under `Asmalloc/Artifacts/`,
say `Asmalloc/Artifacts/Sha3/X86_64.lean`, must define the list
`VG.Artifacts.Sha3.X86_64.artifacts : List Artifact`. Adding functions for
an algorithm or a target adds such a file, so that parallel changes don't
all edit one list.

## Variants and generic callers

Some functions have several implementations with the same contract (e.g. a
compression function in scalar code and with the SHA extensions), and other
functions call them. A caller whose proof holds for any implementation is
*generic*: it is emitted once for each implementation, each a direct call
of that implementation. Adding an implementation must not edit its callers,
so both are found by their files:

* a *variant* file, `Asmalloc/Variants/<Iface>/<Target>/<Name>.lean`,
  defines `VG.Variants.<Iface>.<Target>.<Name>.variant`, one implementation
  of the interface `<Iface>` on `<Target>`;
* a *generic* file, `Asmalloc/Generic/<Iface>/<Target>/<Alg>.lean`,
  defines `VG.Generic.<Iface>.<Target>.<Alg>.artifacts`, a function from a
  variant of `<Iface>` on `<Target>` to a list of artifacts.

A caller of functions of several interfaces (e.g. a key derivation calling
both a hash's compression function and a block function, each with its own
implementations) is generic over all of them: its file is
`Asmalloc/Generic/<Iface₁>/…/<Ifaceₙ>/<Target>/<Alg>.lean`, and its
`artifacts` takes a variant of each interface on `<Target>`, in that order.
Variants of different interfaces may have the same suffix (e.g. `_avx2`), so
the instances of such a caller are named by `qualifiedName`: its name, then
for each interface whose variant has a non-empty suffix, in that order, a tag
for the interface and the suffix (`vg_argon2_blake2b_avx2_g_avx512`;
the instance of the baseline variants keeps the plain name, `vg_argon2`).
Callers of one interface append the suffix alone (`vg_sha256_update_shani`).

The artifacts emitted include each generic function applied to each variant
of its interface and target (each combination of a variant of each of its
interfaces), in order of their module names. What a variant is (its type)
is up to the interface, as long as every variant and every generic function
of it agree, which elaboration checks. Nothing here needs trusting beyond
the files found: every artifact carries its own proof, and
the emitter checks its calls and features (`Rust.files`) as for any other.
A variant's own functions (e.g. the implementation itself) are listed in a
registration file as usual.

`Emit.lean` finds the registration, variant and generic files
(`registrations`, `grouped`, `genericGroups`), writes a program that
imports them all, checks that the whole list depends only on the standard
axioms, that what it runs is what the kernel checked and that the `Spec/`
names it uses come from `Spec/` (`TCB/Audit.lean`), and emits it
(`driver`), and runs that program, whose `main` is `run`.
-/

@[expose] public section

namespace VG.Emit

open System

/-- The directory of the registration files, relative to `lean/`. -/
def registrationDir : FilePath := "Asmalloc" / "Artifacts"

/-- The module of the Lean file `path` (relative to `lean/`), e.g.
`Asmalloc.Artifacts.Sha3.X86_64`. -/
def moduleOf (path : FilePath) : String :=
  ".".intercalate ((path.withExtension "").components.filter (· != "."))

/-- The directory of the variant files, relative to `lean/`. -/
def variantDir : FilePath := "Asmalloc" / "Variants"

/-- The directory of the generic files, relative to `lean/`. -/
def genericDir : FilePath := "Asmalloc" / "Generic"

/-- The modules of the `.lean` files under `dir`, in order of their names. -/
def modulesUnder (dir : FilePath) : IO (List String) := do
  unless ← dir.pathExists do return []
  let files ← dir.walkDir
  let mods := files.toList.filter (·.extension == some "lean") |>.map moduleOf
  return mods.mergeSort (· ≤ ·)

/-- The registration modules: the `.lean` files under `registrationDir`, in
order of their module names. -/
def registrations : IO (List String) := modulesUnder registrationDir

/-- The interface and target of a variant module `mod`, e.g.
`Sha256Compress.X86_64` for `Asmalloc.Variants.Sha256Compress.X86_64.ShaNi`;
`none` unless it is exactly three levels below the directory. -/
def groupOf (mod : String) : Option String :=
  match mod.splitOn "." with
  | [_, _, i, t, _] => some s!"{i}.{t}"
  | _ => none

/-- The modules under `dir`, grouped by interface and target (`groupOf`), in
order of the groups' names; fails on a module at any other depth. -/
def grouped (dir : FilePath) : IO (List (String × List String)) := do
  let mods ← modulesUnder dir
  let mut groups : List (String × List String) := []
  for m in mods do
    let some g := groupOf m
      | throw <| IO.userError s!"{m} must be at {dir}/<Iface>/<Target>/<Name>.lean"
    groups := match groups.find? (·.1 == g) with
      | some _ => groups.map fun (h, ms) => if h == g then (h, ms ++ [m]) else (h, ms)
      | none => groups ++ [(g, [m])]
  return groups.mergeSort (·.1 ≤ ·.1)

/-- The name of the list a registration module defines:
`Asmalloc.Artifacts.Sha3.X86_64` defines
`VG.Artifacts.Sha3.X86_64.artifacts`. -/
def listOf (mod : String) : String :=
  "VG." ++ (mod.dropPrefix "Asmalloc.").toString ++ ".artifacts"

/-- The variant a variant module defines:
`Asmalloc.Variants.Sha256Compress.X86_64.ShaNi` defines
`VG.Variants.Sha256Compress.X86_64.ShaNi.variant`. -/
def variantOf (mod : String) : String :=
  "VG." ++ (mod.dropPrefix "Asmalloc.").toString ++ ".variant"

/-- The interfaces of a generic module `mod`, each with its target:
`["Sha256Compress.X86_64"]` for
`Asmalloc.Generic.Sha256Compress.X86_64.Pbkdf2`, and
`["Blake2b.X86_64", "Argon2Compress.X86_64"]` for
`Asmalloc.Generic.Blake2b.Argon2Compress.X86_64.Argon2`; `none`
unless there is at least one interface, a target and a name below the
directory. -/
def interfacesOf (mod : String) : Option (List String) :=
  match mod.splitOn "." with
  | _ :: _ :: rest =>
    match rest.reverse with
    | _ :: t :: i :: is => some ((i :: is).reverse.map (· ++ "." ++ t))
    | _ => none
  | _ => none

/-- The modules under `dir` (generic files), grouped by their interfaces
(`interfacesOf`), in order of the groups' names; fails on a module with no
interface. -/
def genericGroups (dir : FilePath) : IO (List (List String × List String)) := do
  let mods ← modulesUnder dir
  let mut groups : List (List String × List String) := []
  for m in mods do
    let some g := interfacesOf m
      | throw <| IO.userError s!"{m} must be at {dir}/<Iface>/…/<Target>/<Name>.lean"
    groups := match groups.find? (·.1 == g) with
      | some _ => groups.map fun (h, ms) => if h == g then (h, ms ++ [m]) else (h, ms)
      | none => groups ++ [(g, [m])]
  return groups.mergeSort fun a b => ".".intercalate a.1 ≤ ".".intercalate b.1

/-- The name of an instance of a generic function of several interfaces
(see above): `base`, then `_<tag><suffix>` for each `(tag, suffix)` of
`parts` (one per interface, in order) whose suffix is not empty. Suffixes
start with `_`, so `qualifiedName "vg_argon2" [("blake2b", "_avx2"),
("g", "")]` is `vg_argon2_blake2b_avx2`. Two variants with the same
suffix in different interfaces give different names, as long as the tags
differ. -/
def qualifiedName (base : String) (parts : List (String × String)) : String :=
  base ++ String.join (parts.map fun (tag, suffix) =>
    if suffix.isEmpty then "" else "_" ++ tag ++ suffix)

/-- The name of the variable bound to a variant of the `i`th interface of a
generic function: `v`, then `v2`, `v3`, …. -/
def varName (i : Nat) : String := if i == 0 then "v" else s!"v{i + 1}"

/-- The artifacts of the generic modules `gens` of the same interfaces and
target, for each combination of a variant module of each interface, in
`vars` (one list per interface, in order): one term of type
`List Artifact`. -/
def instances (gens : List String) (vars : List (List String)) : String :=
  let names := (List.range vars.length).map varName
  let apply (g : String) : String := listOf g ++ String.join (names.map (" " ++ ·))
  let body := "List.flatten [" ++ ", ".intercalate (gens.map apply) ++ "]"
  (names.zip vars).foldr (fun (v, ms) acc =>
    s!"(List.flatMap (fun {v} => {acc}) [" ++ ", ".intercalate (ms.map variantOf) ++ "])") body

/-- The program that emits every artifact: those of each registration
module `mods`, in order, then each generic function of `gens` applied to
each variant of its interface and target in `vars` (each combination of
variants of its interfaces), then those of
`Artifacts.lean`. It fails to elaborate unless each module defines its list
(or variant), with types that agree, unless everything emitted depends
only on the standard axioms (`#assert_standard_axioms`), unless the compiled
code of everything emitted and of `run` is what the kernel checked
(`#assert_no_compiler_overrides`) and unless every `VG.Spec` definition is
declared in `Spec/` (`#assert_spec_origin`). -/
def driver (mods : List String) (gens : List (List String × List String) := [])
    (vars : List (String × List String) := []) : String :=
  let groups := gens.map fun (is, gs) =>
    (gs, is.map fun i => ((vars.find? (·.1 == i)).map (·.2)).getD [])
  "-- Generated by Emit.lean, which runs it. DO NOT EDIT.\n" ++
  "import Asmalloc.TCB.Audit\n" ++
  "import Asmalloc.TCB.Axioms\n" ++
  "import Asmalloc.TCB.Emit\n" ++
  "import Asmalloc.Artifacts\n" ++
  String.join ((mods ++ gens.flatMap (·.2) ++ vars.flatMap (·.2)).map fun m =>
    s!"import {m}\n") ++
  "\nnamespace VG.Emit\n\n" ++
  "def all : List Artifact := List.flatten [\n" ++
  String.join (mods.map fun m => s!"  {listOf m},\n") ++
  String.join (groups.map fun (gs, vs) => s!"  {instances gs vs},\n") ++
  "  VG.artifacts]\n\n" ++
  "#assert_standard_axioms all\n" ++
  "#assert_no_compiler_overrides all run\n" ++
  "#assert_spec_origin\n\n" ++
  "end VG.Emit\n\n" ++
  "def main (args : List String) : IO UInt32 := VG.Emit.run VG.Emit.all args\n"

/-- The target directory a registration, generic or variant module's path
names: `X86_64` for `Asmalloc.Artifacts.Sha3.X86_64`,
`Asmalloc.Generic.Sha256Compress.X86_64.Pbkdf2` and
`Asmalloc.Variants.Sha256Compress.X86_64.ShaNi`. -/
def targetDirOf (mod : String) : Option String :=
  match mod.splitOn "." with
  | ["Asmalloc", "Artifacts", _, t] => some t
  | "Asmalloc" :: d :: rest =>
    if (d == "Generic" || d == "Variants") && rest.length ≥ 3 then rest[rest.length - 2]?
    else none
  | _ => none

/-- The registration modules `mods`, generic modules `gens` and variant
modules `vars` (as `driver` takes them) of each target directory
(`targetDirOf`), in order of its name; fails on a module without one.
`Emit.lean` runs a driver for each (a *part*): where they are split only
decides how much each process does, as `finish` checks that no target's
artifacts are in two parts. -/
def parts (mods : List String) (gens : List (List String × List String))
    (vars : List (String × List String)) :
    Except String (List (String × List String × List (List String × List String) ×
      List (String × List String))) := do
  let dirOf (m : String) : Except String String :=
    match targetDirOf m with
    | some t => pure t
    | none => throw s!"{m} names no target directory"
  let mut dirs : List String := []
  for m in mods ++ gens.flatMap (·.2) ++ vars.flatMap (·.2) do
    let t ← dirOf m
    unless dirs.contains t do dirs := dirs ++ [t]
  dirs.mergeSort (· ≤ ·) |>.mapM fun t => do
    let ms ← mods.filterM fun m => return (← dirOf m) == t
    let gs ← gens.filterMapM fun (is, gs) => do
      let gs ← gs.filterM fun g => return (← dirOf g) == t
      return if gs.isEmpty then none else some (is, gs)
    let vs ← vars.filterMapM fun (i, vs) => do
      let vs ← vs.filterM fun v => return (← dirOf v) == t
      return if vs.isEmpty then none else some (i, vs)
    return (t, ms, gs, vs)

def usage : String := "usage: lake env lean --run Emit.lean [--check] [DIR]"

/-- `[--check] [--part FILE] [DIR]`: whether to check rather than write, the
file a part writes its targets to, and the directory (by default
`../src/asm`). -/
def parseArgs (args : List String) : Option (Bool × Option String × FilePath) :=
  let (check, rest) := match args with
    | "--check" :: rest => (true, rest)
    | rest => (false, rest)
  let (part, rest) := match rest with
    | "--part" :: f :: rest => (some f, rest)
    | rest => (none, rest)
  match rest with
  | [] => some (check, part, "../src/asm")
  | [d] => some (check, part, d)
  | _ => none

/-- Writes `files` (paths relative to `dir` and their contents) under `dir`,
deleting any other `.rs` file under the directories `under` (relative to
`dir`, `""` for all of it) but those of `others`; or, if `check`, writes
nothing and fails if the files on disk differ from `files`. -/
def sync (dir : FilePath) (check : Bool) (files : List (String × String))
    (under others : List String) : IO Bool := do
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
  -- Any other `.rs` file there would be unverified code.
  let expectedPaths := files.map fun (n, _) => (dir / n).normalize
  let otherDirs := others.map fun (o : String) => (dir / o).normalize.toString ++ "/"
  for u in under do
    let d := if u.isEmpty then dir else dir / u
    unless ← d.pathExists do continue
    for path in ← d.walkDir do
      if path.extension == some "rs" && !expectedPaths.contains path.normalize &&
          !otherDirs.any (path.normalize.toString.startsWith ·) then
        if check then
          IO.eprintln s!"{path} is not generated from any artifact"
          ok := false
        else
          IO.FS.removeFile path
          IO.println s!"removed stale {path}"
  return ok

/-- Writes the files generated from `as` under the directory in `args` (by
default `../src/asm`), deleting any other `.rs` file there; or, if `args`
starts with `--check`, writes nothing and fails if the files on disk differ
from what would be generated.

With `--part FILE`, `as` are the artifacts of a part (`parts`): only the
files of their targets, `<target>/…`, are written or checked, and the
other `.rs` files under their directories; then `FILE` gets their targets,
for `finish`, which checks the root module and the rest of the
directory. -/
def run (as : List Artifact) (args : List String) : IO UInt32 := do
  let some (check, part, dir) := parseArgs args | do IO.eprintln usage; return 2
  let files ← match Rust.files as with
    | .ok files => pure files
    | .error e => do IO.eprintln e; return 1
  let targets := Rust.targetsOf as
  let ok ← match part with
    | none => sync dir check files [""] []
    | some _ =>
      let ts := targets.map (·.1)
      sync dir check (files.filter fun (n, _) => ts.any (n.startsWith <| · ++ "/")) ts []
  if !ok then
    IO.eprintln "run `lake build && lake env lean --run Emit.lean` in lean/ and commit the result"
    return 1
  if let some f := part then
    IO.FS.writeFile f (String.join (targets.map fun (t, c) => s!"{t}\t{c}\n"))
  return 0

/-- A target of two of the parts whose targets are `targets`, if any. -/
def inTwoParts (targets : List (List (String × String))) : Option String :=
  let names := targets.flatMap (·.map (·.1))
  names.find? fun t => (names.filter (· == t)).length > 1

/-- After `run` succeeded on every part, given the targets each wrote: no
target is in two parts (so each part generated all of its targets' files,
from all of their artifacts), the root module `mod.rs` is that of all the
targets (`Rust.rootFile`), and no other `.rs` file is outside their
directories. Writes or checks, as `run`. -/
def finish (targets : List (List (String × String))) (args : List String) : IO UInt32 := do
  let some (check, _, dir) := parseArgs args | do IO.eprintln usage; return 2
  if let some t := inTwoParts targets then
    IO.eprintln s!"the artifacts of the target {t} are in two parts"
    return 1
  let all := targets.flatten
  unless ← sync dir check [("mod.rs", Rust.rootFile all)] [""] (all.map (·.1)) do
    IO.eprintln "run `lake build && lake env lean --run Emit.lean` in lean/ and commit the result"
    return 1
  return 0

end VG.Emit
