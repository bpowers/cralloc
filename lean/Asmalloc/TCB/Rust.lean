module

public import Asmalloc.TCB.Artifact

/-!
# Rendering artifacts as Rust

**Trusted.** Every artifact becomes a Rust naked function whose entire body
is the printed assembly:

```rust
#[unsafe(naked)]
pub(crate) unsafe extern "<abi>" fn <name><sig> {
    core::arch::naked_asm!(
        r"<line>
<line>
…",
    )
}
```

A naked function has no compiler-generated prologue or epilogue, so the
machine code that runs is exactly the code that was verified (plus the
final `ret` from the printer). The functions of each target are collected
in `src/asm/<target>/`, one file per `module`, compiled only under the
target's `cfg`.

A call (`Code.call name body`) is a call instruction whose operand is the
symbol of the Rust function `name` (a `sym` operand). The model runs `body`
for it, so `files` checks that `name` is an artifact of the same target
whose printed code is exactly that of `body`, for every call, including the
calls in `body` itself; otherwise nothing is emitted.

The documentation of each function is its `doc`, with what its contract
requires of the memory each buffer is valid for and of where its buffers are
added to its `# Safety` section (`fullDoc`, `Sig.validDoc`, `Sig.layoutDoc`):
the latter depends on the target's calling convention and on the stack the
code uses, and both are generated from the same signature, calling
convention, `writeArgs` and `stack` as the contract (`Artifact.ofSig`), so
the `doc` must not state them itself (`checkLayout`).

An artifact that reads tables of constants (`Artifact.consts`, and those
its instructions carry, `Printer.tables`: `tablesOf`) names each
by the name of a `static` holding it, which `files` writes once per target,
in the module `consts` (`src/asm/<target>/consts.rs`), after checking that
every artifact of the target with a table of that name gives the same words
and that the code names only its own tables (`checkConsts`):

```rust
#[repr(C, align(64))]
pub(crate) struct Table<const N: usize>(pub(crate) [u8; N]);

pub(crate) static <NAME>: Table<<8 n>> = Table(*b"\
    <the 8 bytes of word 0, least significant first, as \xHH>\
    …
");
```

`Table` holds the bytes of the words at its start (`repr(C)`, one field),
each word little-endian as the contract's memory holds it
(`Abi.constsHeld`), so the static's address is that of word 0. It aligns each table to 64 bytes, a
cache line on every target, so that where a table's entries fall in cache
lines depends only on the table, not on the size of whatever the linker puts
before it (a change to one table would otherwise move the others, and their
functions' timing with them). Its name cannot be a table's (`tableName`).

The code forms a table's address from the static's symbol (a `sym`
operand), on AArch64 by `adrp` and `add` (`Line.sym`), whose operands
`vg_sym_page!` and `vg_sym_pageoff!` (`src/asm/mod.rs`) write in the syntax
of the object format: `<sym>@PAGE` and `<sym>@PAGEOFF` for Mach-O, on
Apple's platforms, and `<sym>` and `:lo12:<sym>` for ELF and COFF; on
x86-64 by `lea` of the RIP-relative operand `[rip + <sym>]`, which is the
same in every object format (`SymPart.ripRel`); on IA-32, by a call to the
next instruction and an ADD of the difference between its return address
and the static (`SymPart.x86PcRel`, the ADD immediate emitted as `.long`
because LLVM's Intel-syntax operand parser rejects symbol differences); on
ARMv7, by `movw` and `movt` of the difference between the static and the
`pc` an `add` then adds (`SymPart.armPcRel`, the same text in ARM and Thumb
code, and position independent; ELF only, as COFF, for Windows' Thumb
targets, cannot represent the difference, so such code does not assemble
there). The
linker resolves them to the static's address, which the model gives
(`State.syms`, fixed for the run as a static's address is) and at which the
contract (`Abi.withConsts`) says the memory holds the table, readable and
apart from every writable region: a `static` is a distinct immutable object.

An artifact whose code needs CPU features beyond the target's baseline
(`Artifact.features`) also gets a last `# Safety` item saying the CPU must
support them, and a constant of them, for the Rust that checks for them
before calling it: a `Features` set (`src/cpu.rs`), made from their names by
`Features::of`, which does not compile with a name `src/cpu.rs` does not
detect:

```rust
/// The CPU features `<name>` requires (`Artifact.features`).
pub(crate) const <NAME>_FEATURES: crate::cpu::Features = crate::cpu::Features::of(&["<feature>", …]);
```

`files` checks that `features` are exactly the features the code requires.
(The functions get no `#[target_feature]` attribute: Rust does not allow one
on naked functions. The x86 assemblers accept every instruction without it;
on AArch64 the printer's `enableFeature`/`disableFeature` directives enable
each feature for the function's body only.)
-/

@[expose] public section

namespace VG.Rust

universe u

/-- Escape a string for a Rust string literal that is also an `asm!`
template: `{`/`}` are template placeholders and must be doubled. -/
def escape (s : String) : String :=
  s.foldl (init := "") fun acc c =>
    match c with
    | '{' => acc ++ "{{"
    | '}' => acc ++ "}}"
    | '\\' => acc ++ "\\\\"
    | '"' => acc ++ "\\\""
    | '\n' => acc ++ "\\n"
    | c => acc.push c

/-- `s` as an `asm!` template: `{`/`}` are placeholders and must be doubled. -/
def escapeBraces (s : String) : String :=
  s.foldl (init := "") fun acc c =>
    match c with
    | '{' => acc ++ "{{"
    | '}' => acc ++ "}}"
    | c => acc.push c

/-- A Rust string literal whose value is `s`: a raw string literal, which
rustc reads faster (it maps each character of an ordinary literal's
template back to the source by unescaping it), unless `s` has a `"` or a
carriage return, which a raw string literal cannot hold. -/
def literal (s : String) : String :=
  if s.any (fun c => c == '"' || c == '\r') then
    "\"" ++ s.foldl (init := "") (fun acc c =>
      match c with
      | '\\' => acc ++ "\\\\"
      | '"' => acc ++ "\\\""
      | '\n' => acc ++ "\\n"
      | '\r' => acc ++ "\\r"
      | c => acc.push c) ++ "\""
  else "r\"" ++ s ++ "\""

def header : String :=
  "// @generated by lean/Emit.lean. DO NOT EDIT.\n"

def docComment (indent doc : String) : String :=
  String.join ((doc.splitOn "\n").map fun l =>
    if l.isEmpty then s!"{indent}///\n" else s!"{indent}/// {l}\n")

/-- The distinct values of `as`, in first-seen order. -/
def dedup (as : List String) : List String :=
  as.foldl (init := []) fun acc a => if acc.contains a then acc else acc ++ [a]

/-- The text of one line of the `naked_asm!` template (`escapeBraces`): a
call names its function as the operand of the same name. `none` for the
page of a static's address or its offset in the page, which a macro writes
in the syntax of the object format (`line`). -/
def lineText (call : String) : Line → Option String
  | .text l => some (escapeBraces l)
  | .call n => some (escapeBraces call ++ " {" ++ n ++ "}")
  | .sym s .ripRel n => some (escapeBraces s ++ "[rip + {" ++ n ++ "}]")
  | .sym s .x86PcRel n => some (escapeBraces s ++ "{" ++ n ++ "} - 2b")
  | .sym s .armPcRel n => some (escapeBraces s ++ "{" ++ n ++ "} - (2f + 2 * (3f - 2f)))")
  | .sym _ .page _ | .sym _ .pageOff _ => none

/-- One line of the `naked_asm!` template as an argument of its own. -/
def line (call : String) : Line → String
  | .sym s .page n => s!"        concat!(\"{escape s}\", vg_sym_page!(\"" ++ "{" ++ n ++ "}\")),\n"
  | .sym s .pageOff n =>
    s!"        concat!(\"{escape s}\", vg_sym_pageoff!(\"" ++ "{" ++ n ++ "}\")),\n"
  | l => "        " ++ literal ((lineText call l).getD "") ++ ",\n"

/-- The arguments of `naked_asm!` for the template lines `ls`: each run of
lines that are text alone (`lineText`) as one string literal (`literal`) of
those lines separated by newlines, the others as arguments of their own
(`line`).
`naked_asm!` joins its arguments with newlines, so the template is the same
as with one argument per line. A function's template is then a few tokens
for rustc to parse and expand, not one per line. -/
def template (call : String) (ls : List Line) : String :=
  go ls []
where
  /-- The arguments for `ls`, after the run of text lines `run` (reversed). -/
  go : List Line → List String → String
    | [], run => flush run
    | l :: ls, run => match lineText call l with
      | some t => go ls (t :: run)
      | none => flush run ++ line call l ++ go ls []
  /-- The run of text lines `run` (reversed) as one string literal. -/
  flush : List String → String
    | [] => ""
    | run => "        " ++ literal ("\n".intercalate run.reverse) ++ ",\n"

/-- `fs` as an English list of code spans: "`a`, `b` and `c`". -/
def codeList : List String → String
  | [] => ""
  | [f] => s!"`{f}`"
  | [f, g] => s!"`{f}` and `{g}`"
  | f :: fs => s!"`{f}`, " ++ codeList fs

/-- `doc`, with the CPU features `fs` added as the last item of its
`# Safety` section (which `checkFeatures` requires it to end with). -/
def featureDoc (doc : String) : List String → String
  | [] => doc
  | fs => doc ++ s!"\n* The CPU must support the {codeList fs} target \
    feature{if fs.length == 1 then "" else "s"}."

/-- The constant of the CPU features `fs` that the function `name`
requires; nothing if there are none. -/
def featuresConst (name : String) : List String → String
  | [] => ""
  | fs =>
    s!"/// The CPU features `{name}` requires (`Artifact.features`).\n" ++
    s!"pub(crate) const {name.toUpper}_FEATURES: crate::cpu::Features = \
      crate::cpu::Features::of(&[" ++
    ", ".intercalate (fs.map fun f => s!"\"{escape f}\"") ++ "]);\n\n"

/-- The name of the constant of how much of its parameter `param` the
function `name` writes (`Artifact.written`). -/
def writtenName (name param : String) : String := s!"{name.toUpper}_{param.toUpper}_WRITTEN"

/-- The constants of how much of its parameters the function `name` writes
(`Artifact.written`), one per parameter; nothing if there are none. -/
def writtenConsts (name : String) (written : List (String × Nat)) : String :=
  String.join (written.map fun (p, n) =>
    s!"/// The bytes of `{p}` that `{name}` may write: it changes no byte of `{p}` at this \
      offset or beyond (`Artifact.written`).\n" ++
    s!"pub(crate) const {writtenName name p}: usize = {n};\n\n")

/-- The paragraphs documenting how much of its parameters the function `name`
writes (`Artifact.written`). -/
def writtenDoc (name : String) (written : List (String × Nat)) : List String :=
  written.map fun (p, n) =>
    s!"It writes at most the first {n} bytes of `{p}` (`{writtenName name p}`): no byte of \
      `{p}` beyond them changes."

/-- The features that the function `name`, with documentation `doc`, declares
(`declared`) are exactly those its code requires (`required`, with
repetitions); and if there are any, `doc` ends with its `# Safety`
section, for `featureDoc` to add them to. -/
def featureCheck (name doc : String) (required declared : List String) :
    Except String Unit := do
  let required := dedup required
  for f in required do
    unless declared.contains f do
      throw s!"{name} requires the CPU feature {f} but does not declare it"
  for f in declared do
    unless required.contains f do
      throw s!"{name} declares the CPU feature {f}, which none of its code requires"
  unless declared.isEmpty do
    match doc.splitOn "\n# " with
    | [_] => throw s!"{name} needs CPU features but its doc has no `# Safety` section"
    | parts => unless parts.getLast!.startsWith "Safety\n" do
        throw s!"{name} needs CPU features but its doc does not end with its `# Safety` section"

/-- The value of `f` on the code of each call of `calls` (`Code.calls`), in
order, but for a call whose value is that of the first call of the same
function.

`calls` holds a function once per call of it, through every function that
calls it, and its code is usually the same value in memory at each of them:
`withPtrEq` finds that at once, without computing `f` again (which, on the
code of every call, was most of the time of the checks using this). -/
def distinctCalls {I C β : Type} [BEq β] [LawfulBEq β] (f : Code I C → β)
    (calls : List (String × Code I C)) : List β := Id.run do
  -- The first call of each function so far, with its code and its value.
  let mut first : List {p : String × Code I C × β // p.2.2 = f p.2.1} := []
  let mut values : List β := []
  for (n, body) in calls do
    match first.find? (·.1.1 == n) with
    | some ⟨(_, body₀, v₀), h₀⟩ =>
      unless withPtrEq body body₀ (fun _ => f body == v₀) (fun h => by simp_all) do
        values := f body :: values
    | none =>
      let v := f body
      first := ⟨(n, body, v), rfl⟩ :: first
      values := v :: values
  return values.reverse

/-- The features that the instructions of `c` require, but not those of the
functions it calls, each once, in first-seen order after those of `acc`. -/
def ownRequires {I C : Type} (req : I → List String) : List String → Code I C → List String
  | acc, .block is => is.foldl (fun acc i => add acc (req i)) acc
  | acc, .seq a b => ownRequires req (ownRequires req acc a) b
  | acc, .ite _ t e => ownRequires req (ownRequires req acc t) e
  | acc, .loop b _ => ownRequires req acc b
  | acc, .call _ _ => acc
  | acc, .frame i b j => add (ownRequires req (add acc (req i)) b) (req j)
where
  add (acc fs : List String) : List String :=
    fs.foldl (fun acc f => if acc.contains f then acc else acc ++ [f]) acc

/-- The features that the instructions of `c`, and of the functions it
calls, require (`Code.requires`), each once: those of the instructions of
`c` outside its calls (`ownRequires`), then those of the code of each call
(`Code.calls`, which has every call, those in the code of a call too),
skipping a call whose code requires what the first call of the same
function's does (`distinctCalls`). These are the features of
`dedup (c.requires req)`, without building the list with repetitions or
going through the code of a function once per call of it. -/
def requiresDedup {I C : Type} (req : I → List String) (c : Code I C) : List String :=
  (distinctCalls (ownRequires req []) c.calls).foldl ownRequires.add (ownRequires req [] c)

/-- The first `some` of `f` over the instructions of `c`, but not those of
the functions it calls. -/
def ownFirstSome? {I C : Type} (f : I → Option String) : Code I C → Option String
  | .block is => is.findSome? f
  | .seq a b => ownFirstSome? f a <|> ownFirstSome? f b
  | .ite _ t e => ownFirstSome? f t <|> ownFirstSome? f e
  | .loop b _ => ownFirstSome? f b
  | .call _ _ => none
  | .frame i b j => f i <|> ownFirstSome? f b <|> f j

/-- A `some` of `f` over the instructions of `c`, and of the functions it
calls, if there is one: the first of those of `c` outside its calls
(`ownFirstSome?`), or else that of the code of the first call that has one
(`Code.calls`, which has every call, those in the code of a call too). A
call whose value is the first call of the same function's is skipped
(`distinctCalls`): that value is `none`, or the first call's was found. -/
def firstSome? {I C : Type} (f : I → Option String) (c : Code I C) : Option String :=
  ownFirstSome? f c <|> (distinctCalls (ownFirstSome? f) c.calls).findSome? id

/-- `doc` with the paragraphs `notes` inserted before its `# Safety` section. -/
def insertNotes (doc : String) : List String → String
  | [] => doc
  | notes =>
    let sep := "\n\n# Safety\n\n"
    match (doc.splitOn sep).reverse with
    | safety :: rest@(_ :: _) =>
      sep.intercalate rest.reverse ++ String.join (notes.map ("\n\n" ++ ·)) ++ sep ++ safety
    | _ => doc

/-- `doc` with the items `items` first in its `# Safety` section (after
`"\n\n# Safety\n\n"`, which `checkLayout` requires it to have if there are
any), which may have no items of its own. -/
def prependSafety (doc : String) : List String → String
  | [] => doc
  | items =>
    let sep := "\n\n# Safety\n\n"
    match (doc.splitOn sep).reverse with
    | safety :: rest@(_ :: _) =>
      sep.intercalate rest.reverse ++ sep ++ "\n".intercalate (items.map ("* " ++ ·)) ++
        (if safety.isEmpty then "" else "\n" ++ safety)
    | _ => doc

/-- The documentation of `a` as emitted: its `doc`, with what its signature's
contract requires of the memory each buffer is valid for and of where its
buffers are (`Sig.layoutNote` before its `# Safety` section, `Sig.validDoc`
first in it and `Sig.layoutDoc` at the end), and the CPU features it needs
(`featureDoc`). -/
def fullDoc (a : Artifact) : String :=
  let A := a.target.abi
  let items := Sig.layoutDoc A a.sig a.writeArgs a.stack
  featureDoc (prependSafety (insertNotes a.doc
      (Sig.layoutNote A a.sig a.writeArgs ++ writtenDoc a.name a.written))
    (Sig.validDoc a.sig) ++ String.join (items.map ("\n* " ++ ·))) a.features

/-- If the emitter adds anything to the documentation of `a` from its
signature (`fullDoc`), `doc` ends with its `# Safety` section, which starts
with `"\n\n# Safety\n\n"`; and `doc` does not itself state what memory a
buffer must be valid for, which the emitter generates (`Sig.validDoc`). -/
def checkLayout (a : Artifact) : Except String Unit := do
  if (a.doc.splitOn "valid for").length > 1 then
    throw s!"{a.target.name}: {a.name}'s doc states what memory a buffer must be valid for, \
      which the emitter generates from its signature (`Sig.validDoc`)"
  let A := a.target.abi
  if (Sig.layoutDoc A a.sig a.writeArgs a.stack).isEmpty &&
      (Sig.layoutNote A a.sig a.writeArgs).isEmpty && (Sig.validDoc a.sig).isEmpty then return
  let ok := match a.doc.splitOn "\n# " with
    | [_] => false
    | parts => parts.getLast!.startsWith "Safety\n" &&
        (a.doc.splitOn "\n\n# Safety\n\n").length > 1
  unless ok do
    throw s!"{a.target.name}: {a.name} has buffers but its doc does not end with its \
      `# Safety` section"

/-- One artifact as a Rust naked function; `moduleOf` gives the module of
each function it calls. The printer's `funcAlign` comes last, after the
return (`Printer.function`) and any directives that follow it. -/
def function (a : Artifact) (moduleOf : String → String) : String :=
  functionOf a (a.target.printer.function a.code) moduleOf
where
  /-- `function a`, given `a`'s printed code `lines`
  (`a.target.printer.function a.code`). -/
  functionOf (a : Artifact) (lines : List Line) (moduleOf : String → String) : String :=
  let P := a.target.printer
  let body := (a.features.flatMap P.enableFeature).map .text ++ lines ++
    (a.features.flatMap P.disableFeature).map .text ++ P.funcAlign.map .text
  let callees := dedup (body.filterMap fun | .call n => some n | _ => none)
  let syms := dedup (body.filterMap fun | .sym _ _ n => some n | _ => none)
  featuresConst a.name a.features ++
  writtenConsts a.name a.written ++
  docComment "" (fullDoc a) ++
  "#[unsafe(naked)]\n" ++
  s!"pub(crate) unsafe extern \"{a.target.rustAbi}\" fn {a.name}{a.sig.rust} " ++ "{\n" ++
  "    core::arch::naked_asm!(\n" ++
  template a.target.printer.call body ++
  String.join (callees.map fun n => s!"        {n} = sym super::{moduleOf n}::{n},\n") ++
  String.join (syms.map fun n => s!"        {n} = sym super::consts::{n},\n") ++
  "    )\n" ++
  "}\n"

/-- The tables of constants of `a`: those its contract gives it
(`Artifact.consts`) and those its instructions carry (`Printer.codeTables`,
x86-64's `aluTbl`). -/
def tablesOf (a : Artifact) : List (String × List (BitVec 64)) :=
  a.consts ++ a.target.printer.codeTables a.code

/-- The distinct values of `f` over `as`, in first-seen order. -/
def distinct (as : List Artifact) (f : Artifact → String) : List String :=
  dedup (as.map f)

/-- The artifact that a call of `name` in `a` calls. -/
def callee (as : List Artifact) (a : Artifact) (name : String) : Option Artifact :=
  as.find? fun b => b.target.name == a.target.name && b.name == name

/-- The first of `calls` (`Code.calls`: the name of each function called and
the code the model runs for it) that is not of a function `printed` gives the
printed code of (`some (n, false)`), or whose code `P` does not print as that
(`some (n, true)`); `none` if every call is of the code it runs.

A function can be called hundreds of times in one artifact's `calls`, so
only the first call of each name is printed and compared with `printed`;
every later one is compared with the first one's code, by equal printed
code, which `withPtrEq` returns at once if the two are the same value in
memory, as they usually are. -/
def badCall {M : ISA} (P : Printer M) (printed : String → Option (List Line))
    (calls : List (String × Prog M)) : Option (String × Bool) := Id.run do
  -- The first call of each function checked so far, with its code.
  let mut first : List (String × Prog M) := []
  for (n, body) in calls do
    match first.find? (·.1 == n) with
    | some (_, body₀) =>
      unless withPtrEq body body₀ (fun _ => P.function body == P.function body₀)
          (fun h => by simp [h]) do
        return some (n, true)
    | none =>
      match printed n with
      | none => return some (n, false)
      | some lines =>
        unless P.function body == lines do return some (n, true)
        first := (n, body) :: first
  return none

/-- An artifact with what the checks read of it more than once, each computed
once: its printed code (`Printer.function`, which a call of it is checked
against in every function that calls it, `checkCalls`) and its tables of
constants (`tablesOf`, which every artifact of the target is checked
against, `checkConsts`). -/
structure Prepared where
  art : Artifact
  lines : List Line
  tables : List (String × List (BitVec 64))
  lines_eq : lines = art.target.printer.function art.code
  tables_eq : tables = tablesOf art

def Prepared.of (a : Artifact) : Prepared := ⟨a, _, _, rfl, rfl⟩

/-- `p`'s artifact declares exactly the CPU features its code requires
(`featureCheck`). -/
def checkFeatures (p : Prepared) : Except String Unit :=
  let a := p.art
  featureCheck s!"{a.target.name}: {a.name}" a.doc (requiresDedup a.target.isa.requires a.code)
    a.features

/-- The target's printer can encode every instruction of the code of `p`'s
artifact, and of the functions it calls, as the model describes it
(`Printer.unencodable`). -/
def checkEncodable (p : Prepared) : Except String Unit :=
  let a := p.art
  match firstSome? a.target.printer.unencodable a.code with
  | none => pure ()
  | some e => throw s!"{a.target.name}: {a.name}: {e}"

/-- Every call in the code of `p`'s artifact, and in the code of the functions
it calls, is of an artifact of `ps` of the same target whose printed code is
that of the called code (`badCall`). -/
def checkCalls (ps : List Prepared) (p : Prepared) : Except String Unit :=
  let a := p.art
  match badCall a.target.printer
      (fun n => (ps.find? fun q => q.art.target.name == a.target.name && q.art.name == n).map
        (·.lines)) a.code.calls with
  | none => pure ()
  | some (n, false) => throw s!"{a.target.name}: {a.name} calls {n}, which is not an artifact"
  | some (n, true) =>
    throw s!"{a.target.name}: {a.name} calls {n}, whose code is not what the call runs"

/-- The artifact carries the contract of the `Api` it is made from
(`Artifact.contracts`), which it is proven against (`Artifact.ofApi`): so its
name, signature and documentation are those `Spec/` gives that contract. -/
def checkApi (a : Artifact) : Except String Unit :=
  unless a.contracts.isSome do
    throw s!"{a.target.name}: {a.name} is not made from an `Api` with a contract \
      (`Api.contracts`)"

/-- Each bound of `a.written` names a writable array parameter of its
signature, no parameter twice, and is at most the parameter's size in bytes:
so the constant `writtenConsts` exports for it is a bound on the bytes of
that buffer the function may change. -/
def checkWritten (a : Artifact) : Except String Unit := do
  let names := a.written.map (·.1)
  for (p, n) in a.written do
    unless (names.filter (· == p)).length == 1 do
      throw s!"{a.target.name}: {a.name} bounds what it writes of `{p}` more than once"
    match a.sig.params.lookup p with
    | some (.array true e k) =>
      unless n ≤ k * e.size do
        throw s!"{a.target.name}: {a.name} bounds what it writes of `{p}` by {n} bytes, \
          more than its {k * e.size}"
    | some _ =>
      throw s!"{a.target.name}: {a.name} bounds what it writes of `{p}`, which is not a \
        writable array"
    | none => throw s!"{a.target.name}: {a.name} bounds what it writes of `{p}`, which is not \
        one of its parameters"

/-- No two artifacts of a target have the same name. -/
def checkUnique (as : List Artifact) : Except String Unit :=
  as.forM fun a =>
    unless (as.filter fun b => b.target.name == a.target.name && b.name == a.name).length == 1 do
      throw s!"{a.target.name}: {a.name} is defined more than once"

/-- A table's name is a Rust constant's: an uppercase ASCII letter, then
uppercase ASCII letters, digits and underscores. -/
def tableName (n : String) : Bool :=
  match n.toList with
  | [] => false
  | c :: cs => ('A' ≤ c && c ≤ 'Z') && cs.all fun c => ('A' ≤ c && c ≤ 'Z') || c.isDigit || c == '_'

/-- Tables of the same name in `cs` and `ds` have the same words. -/
def consistent (cs ds : List (String × List (BitVec 64))) : Bool :=
  cs.all fun (n, ws) => ds.all fun (m, vs) => m != n || vs == ws

/-- The tables of constants of `a` (`Artifact.consts`) have distinct names,
and so do those its instructions carry but for repeats of the same table;
every name is a Rust constant's, none is named `consts` or the module of an
artifact; every artifact of the target with a table of one of these names,
given or carried, gives the same words; its code names only these tables;
and its target's code can name a `static` (`Abi.sym`), if it has any. Of
`p`'s artifact, checked against the artifacts of `ps`. -/
def checkConsts (ps : List Prepared) (p : Prepared) : Except String Unit := do
  let a := p.art
  let ts := p.tables
  unless ts.isEmpty || a.target.abi.sym.isSome do
    throw s!"{a.target.name}: {a.name} reads tables of constants, which its target's code cannot name"
  let names := a.consts.map (·.1)
  unless (dedup names).length == names.length do
    throw s!"{a.target.name}: {a.name} has two tables of constants of the same name"
  for (n, _) in ts do
    unless tableName n do
      throw s!"{a.target.name}: {a.name}'s table of constants {n} is not named as a Rust constant"
  unless consistent ts ts do
    throw s!"{a.target.name}: {a.name} gives a table of constants two lists of words"
  for q in ps do
    unless q.art.target.name != a.target.name || consistent ts q.tables do
      throw s!"{a.target.name}: {a.name} and {q.art.name} give a table of constants different words"
  if (ps.any fun q => q.art.target.name == a.target.name && q.art.module == "consts") then
    throw s!"{a.target.name}: an artifact's module is `consts`, the module of the tables of constants"
  let used := p.lines.filterMap fun | .sym _ _ n => some n | _ => none
  for n in used do
    unless (ts.map (·.1)).contains n do
      throw s!"{a.target.name}: {a.name} names the table {n}, which it does not have \
        (`Artifact.consts`) or carry"

/-- The macros that write the page of a static's address and its offset in
the page in the syntax of the object format (`Line.sym`): Mach-O's on
Apple's platforms, ELF's (which COFF's is too) elsewhere. -/
def symMacros : String :=
  "\n/// The page of a `static`'s address, in the assembler syntax of the object format.\n" ++
  "#[cfg(target_vendor = \"apple\")]\n#[allow(unused_macros)]\n" ++
  "macro_rules! vg_sym_page {\n    ($s:literal) => {\n        concat!($s, \"@PAGE\")\n    };\n}\n" ++
  "#[cfg(not(target_vendor = \"apple\"))]\n#[allow(unused_macros)]\n" ++
  "macro_rules! vg_sym_page {\n    ($s:literal) => {\n        $s\n    };\n}\n" ++
  "\n/// The offset of a `static`'s address in its page, in the assembler syntax of the object format.\n" ++
  "#[cfg(target_vendor = \"apple\")]\n#[allow(unused_macros)]\n" ++
  "macro_rules! vg_sym_pageoff {\n    ($s:literal) => {\n        concat!($s, \"@PAGEOFF\")\n    };\n}\n" ++
  "#[cfg(not(target_vendor = \"apple\"))]\n#[allow(unused_macros)]\n" ++
  "macro_rules! vg_sym_pageoff {\n    ($s:literal) => {\n        concat!(\":lo12:\", $s)\n    };\n}\n"

/-- The tables of constants of the artifacts `arts`, by name, each once. -/
def tables (arts : List Artifact) : List (String × List (BitVec 64)) :=
  let all := arts.flatMap tablesOf
  ((dedup (all.map (·.1))).mergeSort (· ≤ ·)).filterMap fun n => all.find? (·.1 == n)

/-- The byte `b < 256` as an escape of a Rust byte string literal, `\xHH`. -/
def byteEscape (b : Nat) : String :=
  let hex (d : Nat) : Char := Nat.digitChar d
  "\\x" ++ (String.singleton (hex (b / 16))) ++ (String.singleton (hex (b % 16)))

/-- The word `w` as the escapes of its 8 bytes, least significant first. -/
def wordBytes (w : BitVec 64) : String :=
  String.join ((List.range 8).map fun i => byteEscape (w.toNat / 2 ^ (8 * i) % 256))

/-- The module of a target's tables of constants: each table's words, as the
memory holds them (`Abi.constsHeld`: little-endian), in a byte string
literal, one word per line (each line of the literal ends with `\`, which
continues it on the next line without the newline and the indentation).
rustc evaluates a byte string literal at once, faster than an array of a
word per element. -/
def tablesFile (t : String) (ts : List (String × List (BitVec 64))) : String :=
  header ++ s!"//! The tables of constants of the verified functions for `{t}` (`Artifact.consts`).\n" ++
  "#![allow(dead_code)]\n" ++
  "\n/// A table of constants, aligned to a cache line.\n" ++
  "#[repr(C, align(64))]\npub(crate) struct Table<const N: usize>(pub(crate) [u8; N]);\n" ++
  String.join (ts.map fun (n, ws) =>
    s!"\n/// The table of constants `{n}` (`Artifact.consts`).\n" ++
    s!"pub(crate) static {n}: Table<{8 * ws.length}> = Table(*b\"\\\n" ++
    String.join (ws.map fun w => "    " ++ wordBytes w ++ "\\\n") ++ "\");\n")

/-- `mod` declarations for generated child modules (never reformatted by rustfmt). -/
def modDecls (ms : List String) (cfg : String → Option String) : String :=
  String.join (ms.map fun m =>
    let c := match cfg m with
      | some c => s!"#[cfg({c})]\n"
      | none => ""
    s!"\n{c}#[rustfmt::skip]\npub(crate) mod {m};\n")

/-- How many tasks `files` checks the artifacts on: the CPUs of CI's
runners. The checks, and `render`, are pure functions of the artifacts, so
running them on tasks (`Task.spawn`) changes how long they take, never what
they return. -/
def tasks : Nat := 4

/-- `as.forM check` on `tasks` tasks, each checking every `tasks`-th
artifact: the error of the first artifact of `as` that fails, as `forM`
gives it, or none. -/
def forMPar {α : Type u} (as : List α) (check : α → Except String Unit) : Except String Unit :=
  let indexed := as.zipIdx
  let parts := (List.range tasks).map fun j => Task.spawn fun _ =>
    (indexed.filter (·.2 % tasks == j)).findSome? fun (a, i) =>
      match check a with
      | .error e => some (i, e)
      | .ok _ => none
  let first := (parts.filterMap Task.get).foldl (init := none) fun
    | some m, e => if e.1 < m.1 then some e else some m
    | none, e => some e
  match first with
  | some (_, e) => throw e
  | none => pure ()

/-- `as.map Prepared.of`, on `tasks` tasks, each preparing a run of consecutive
artifacts. -/
def prepareAll (as : List Artifact) : List Prepared :=
  let size := (as.length + tasks - 1) / tasks
  let parts := (List.range tasks).map fun j =>
    Task.spawn fun _ => ((as.drop (j * size)).take size).map Prepared.of
  (parts.map Task.get).flatten

/-- The targets of the artifacts `as`, each once, with its Rust `cfg`
(`Target.rustCfg`), in first-seen order. -/
def targetsOf (as : List Artifact) : List (String × String) :=
  (distinct as (·.target.name)).map fun t =>
    (t, ((as.find? (·.target.name == t)).map (·.target.rustCfg)).getD "")

/-- The root module, `mod.rs`: the macros its targets' modules use and a
`mod` declaration of each target of `ts` (its name and Rust `cfg`), in order
of their names. -/
def rootFile (ts : List (String × String)) : String :=
  header ++
  "//! Formally verified assembly, emitted from Lean. See `lean/README.md`.\n" ++
  "//!\n" ++
  "//! Every function in these modules is the direct rendering of an `Artifact`\n" ++
  "//! whose machine code has been proven correct, memory safe and constant time\n" ++
  "//! against its contract.\n" ++
  symMacros ++
  modDecls ((ts.map (·.1)).mergeSort (· ≤ ·)) fun t => (ts.find? (·.1 == t)).map (·.2)

/-- The generated files of the artifacts of `ps`, given the module of each
function each artifact calls; each artifact's code as `ps` printed it
(`Prepared.lines`). -/
def render (ps : List Prepared) (moduleOf : Artifact → String → String) : List (String × String) :=
  let as := ps.map (·.art)
  let targets := distinct as (·.target.name)
  let root := rootFile (targetsOf as)
  let perTarget (t : String) : List (String × String) :=
    let arts := ps.filter (·.art.target.name == t)
    -- In order of their names, wherever their artifacts come in the list.
    let modules := (distinct (arts.map (·.art)) (·.module)).mergeSort (· ≤ ·)
    let ts := tables (arts.map (·.art))
    let mods := if ts.isEmpty then modules else (modules ++ ["consts"]).mergeSort (· ≤ ·)
    (s!"{t}/mod.rs", header ++ s!"//! Verified functions for `{t}`.\n" ++
      modDecls mods (fun _ => none)) ::
    (if ts.isEmpty then [] else [(s!"{t}/consts.rs", tablesFile t ts)]) ++
    -- Each module's file on a task of its own: a part (`Emit.run`) has one target.
    ((modules.map fun m => Task.spawn fun _ =>
      (s!"{t}/{m}.rs",
        header ++ s!"//! Verified `{m}` functions for `{t}`.\n" ++
        "#![allow(dead_code)]\n" ++
        String.join ((arts.filter (·.art.module == m)).map fun p =>
          "\n" ++ function.functionOf p.art p.lines (moduleOf p.art)))).map Task.get)
  -- Each target's files on a task of their own.
  ("mod.rs", root) :: ((targets.map fun t => Task.spawn fun _ => perTarget t).map Task.get).flatten

/-- The generated files, as paths relative to `src/asm/` and their contents:
`mod.rs`, and for each target `<target>/mod.rs` and one `<target>/<module>.rs`
per module; an error if an artifact is not made from an `Api` with a
contract (`checkApi`), two artifacts of a target have the same name
(`checkUnique`), a call is not of the code it runs (`checkCalls`), its
tables of constants are not as `checkConsts` requires, an
artifact's features are not those its code requires (`checkFeatures`), its
code has an instruction its target's printer cannot encode
(`checkEncodable`), its doc has no `# Safety` section at its end for what
`fullDoc` adds to it (`checkLayout`) or a bound on what it writes is not of a
writable array parameter, at most its size (`checkWritten`). Each check of
every artifact runs on `tasks` tasks (`forMPar`), one check after the other,
so the error is the
one checking them in order would give; what they read of an artifact more
than once is computed once, on `tasks` tasks (`Prepared`, `prepareAll`). -/
def files (as : List Artifact) : Except String (List (String × String)) := do
  -- The same artifacts, with equal parts the same value in memory
  -- (`ShareCommon.shareCommon'` returns its argument): the code of every call
  -- of a function in an artifact is then the same value as its first call's,
  -- which `badCall` and `distinctCalls` skip at once. Most calls' code was
  -- an equal value made again, not the same one. First, as sharing values
  -- that tasks have read takes several times longer.
  let as := ShareCommon.shareCommon' as
  forMPar as checkApi
  checkUnique as
  let ps := prepareAll as
  forMPar ps (checkCalls ps)
  forMPar ps (checkConsts ps)
  forMPar ps checkFeatures
  forMPar ps checkEncodable
  forMPar as checkLayout
  forMPar as checkWritten
  return render ps fun a n => ((callee as a n).map (·.module)).getD ""

end VG.Rust
