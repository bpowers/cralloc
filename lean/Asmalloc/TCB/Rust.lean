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
in `<target>/` under the output directory (`TCB/Emit.lean`), one file per
`module`, compiled only under the target's `cfg`.

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

An artifact that reads tables of constants (`Artifact.consts`) names each
by the name of a `static` holding it, which `files` writes once per target,
in the module `consts` (`<target>/consts.rs`), after checking that every
artifact of the target with a table of that name gives the same words and
that the code names only its own tables (`checkConsts`):

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
cache line, so that where a table's entries fall in cache lines depends
only on the table, not on the size of whatever the linker puts before it.
Its name cannot be a table's (`tableName`).

The code forms a table's address, or an address at a constant offset from
it, from the static's symbol (a `sym` operand), on x86-64 by `lea` of the
RIP-relative operand `[rip + <sym> + <off>]`. The linker resolves it to the
static's address plus the offset, which
the model gives (`State.syms`, fixed for the run as a static's address is)
and at which the contract (`Abi.withConsts`) says the memory holds the table,
readable and apart from every writable region: a `static` is a distinct
immutable object.

Any other symbol or compile-time constant the code names (`Piece.ref`) is
an operand its `Api` declares (`Operand`): `<name> = sym <path>` or
`<name> = const <expr>` (`operandText`, `checkOperands`).

An artifact whose code needs CPU features beyond the target's baseline
(`Artifact.features`) also gets a last `# Safety` item saying the CPU must
support them, and a constant naming them, for the Rust that checks for them
before calling it:

```rust
/// The CPU features `<name>` requires (`Artifact.features`).
pub(crate) const <NAME>_FEATURES: &[&str] = &["<feature>", …];
```

`files` checks that `features` are exactly the features the code requires.
(The functions get no `#[target_feature]` attribute: Rust does not allow one
on naked functions, and the x86 assemblers accept every instruction without
it.)
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
call names its function, and a reference to an operand (`Piece.ref`) the
operand, by the same name. -/
def lineText (call : String) : Line → String
  | .text l => escapeBraces l
  | .call n => escapeBraces call ++ " {" ++ n ++ "}"
  | .ops ps => String.join (ps.map fun | .text s => escapeBraces s | .ref n => "{" ++ n ++ "}")

/-- One line of the `naked_asm!` template as an argument of its own. -/
def line (call : String) (l : Line) : String :=
  "        " ++ literal (lineText call l) ++ ",\n"

/-- The arguments of `naked_asm!` for the template lines `ls`: one string
literal (`literal`) of the lines separated by newlines, or nothing if there
are none. `naked_asm!` joins its arguments with newlines, so the template is
the same as with one argument per line. -/
def template (call : String) : List Line → String
  | [] => ""
  | ls => "        " ++ literal ("\n".intercalate (ls.map (lineText call))) ++ ",\n"

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
    s!"pub(crate) const {name.toUpper}_FEATURES: &[&str] = &[" ++
    ", ".intercalate (fs.map fun f => s!"\"{escape f}\"") ++ "];\n\n"

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
`withPtrEq` finds that at once, without computing `f` again. -/
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
  | acc, .rseq arm body =>
    (arm ++ body.flatMap Step.instrs).foldl (fun acc i => add acc (req i)) acc
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
  | .rseq arm body => (arm ++ body.flatMap Step.instrs).findSome? f

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
  featureDoc (prependSafety (insertNotes a.doc (Sig.layoutNote A a.sig a.writeArgs))
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

/-- The declaration of the operand `n` that `a`'s code names: a table of
constants, which the emitter writes (`tablesFile`), or an operand of
`Artifact.operands` (`checkOperands` checks it is one of them). -/
def operandText (a : Artifact) (n : String) : String :=
  if (a.consts.map (·.1)).contains n then s!"sym super::consts::{n}" else
  match a.operands.lookup n with
  | some (.sym p) => s!"sym {p}"
  | some (.const e) => s!"const {e}"
  | none => ""

/-- One artifact as a Rust naked function; `moduleOf` gives the module of
each function it calls. The printer's `funcAlign` comes last, after the
return (`Printer.function`) and any directives that follow it. -/
def function (a : Artifact) (moduleOf : String → String) : String :=
  functionOf a (a.target.printer.function a.code) moduleOf
where
  /-- `function a`, given `a`'s printed code `lines`
  (`a.target.printer.function a.code`). -/
  functionOf (a : Artifact) (lines : List Line) (moduleOf : String → String) : String :=
  let body := lines ++ a.target.printer.funcAlign.map .text
  let callees := dedup (body.filterMap fun | .call n => some n | _ => none)
  let refs := dedup (body.flatMap Line.refs)
  featuresConst a.name a.features ++
  docComment "" (fullDoc a) ++
  "#[unsafe(naked)]\n" ++
  s!"pub(crate) unsafe extern \"{a.target.rustAbi}\" fn {a.name}{a.sig.rust} " ++ "{\n" ++
  "    core::arch::naked_asm!(\n" ++
  template a.target.printer.call body ++
  String.join (callees.map fun n => s!"        {n} = sym super::{moduleOf n}::{n},\n") ++
  String.join (refs.map fun n => s!"        {n} = {operandText a n},\n") ++
  "    )\n" ++
  "}\n"

/-- The external targets the code exits to (`Step.exit`), each once. -/
def exitNames {I C : Type} : Code I C → List String
  | .rseq _ body => dedup (body.filterMap fun | .exit _ _ (some t) => some t | _ => none)
  | _ => []

/-- The Rust names of the machine-level arguments of `sig`, in order: each
parameter's, and a slice's length's after its pointer's. -/
def argNames (sig : Sig) : List String :=
  sig.params.flatMap fun (n, p) => match p with
    | .slice _ _ len => [n, len]
    | _ => [n]

/-- The lines of an inline artifact's `asm!` block: its code, without a
return, and the code of its cold branches in a section of their own. -/
def inlineLines (a : Artifact) : List Line :=
  let (l, k, _) := a.target.printer.lower a.code 0
  l ++ (if k.isEmpty then [] else
    [.text ".pushsection .text.unlikely, \"ax\""] ++ k ++ [.text ".popsection"])

/-- One artifact as an inline Rust function whose body is one `asm!` block:

```rust
#[inline(always)]
pub(crate) unsafe fn <name><<generics>><sig> {
    let ret: <ty>;                       // if it returns a value
    unsafe {
        core::arch::asm!(
            r"<lines>",
            in("<reg>") <arg>, …           // inout("<reg>") <arg> => _ if a clobber
            lateout("<ret>") ret,          // if it returns a value
            out("<reg>") _, …              // the other clobbers
            <name> = sym <path> | const <expr>, …
            <exit> = label { return false }, …
            options(nostack),
        );
    }
    ret                                  // or `true` if it has exits
}
```

The arguments are in the target's argument registers, in order (the same as
its calling convention, `Target.abi`, so the contract reads them where they
are), and the result in its result register. Every register the code may
change is declared, from the same list the proof covers
(`Artifact.clobbersOk`); `asm!` keeps the others. A function with exits
returns `bool`: `false` from each exit's label, `true` from the end of the
block. -/
def inlineFunction (a : Artifact) (lines : List Line) : String :=
  let T := a.target
  let args := (T.argRegs.zip (argNames a.sig))
  let ret := a.sig.ret
  let exits := exitNames a.code
  let refs := dedup (lines.flatMap Line.refs) |>.filter (!exits.contains ·)
  let retTy := match ret with | some ty => ty.rust | none => "bool"
  let sig := "(" ++ ", ".intercalate (a.sig.params.map fun p => p.2.rust p.1) ++ ")" ++
    (if exits.isEmpty && ret.isNone then "" else s!" -> {retTy}")
  let ins := args.map fun (r, n) =>
    if a.clobbers.contains r then s!"inout(\"{r}\") {n} => _" else s!"in(\"{r}\") {n}"
  let outs := (if ret.isSome then [s!"lateout(\"{T.retReg}\") ret"] else []) ++
    -- The result register is an output of its own only if there is a result.
    (a.clobbers.filter fun c => !(args.map (·.1)).contains c && (ret.isNone || c != T.retReg)).map
      fun c => s!"out(\"{c}\") _"
  let gen := if a.generics.isEmpty then "" else s!"<{a.generics}>"
  featuresConst a.name a.features ++
  docComment "" (fullDoc a) ++
  "#[inline(always)]\n" ++
  s!"pub(crate) unsafe fn {a.name}{gen}{sig} " ++ "{\n" ++
  (match ret with | some ty => s!"    let ret: {ty.rust};\n" | none => "") ++
  "    unsafe {\n" ++
  "        core::arch::asm!(\n" ++
  "    " ++ template a.target.printer.call lines ++
  String.join ((ins ++ outs).map fun o => s!"            {o},\n") ++
  String.join (refs.map fun n => s!"            {n} = {operandText a n},\n") ++
  String.join (exits.map fun n => s!"            {n} = label \{ return false },\n") ++
  "            options(nostack),\n" ++
  "        );\n" ++
  "    }\n" ++
  (if ret.isSome then "    ret\n" else if exits.isEmpty then "" else "    true\n") ++
  "}\n"

/-- Whether code calls a function or has a frame, which an inline function,
whose `asm!` block may not touch the stack, cannot. -/
def usesStack {I C : Type} : Code I C → Bool
  | .block _ | .rseq .. => false
  | .seq a b => usesStack a || usesStack b
  | .ite _ t e => usesStack t || usesStack e
  | .loop b _ => usesStack b
  | .call .. | .frame .. => true

/-- An artifact of an inline target uses no stack, names only the target's
registers as clobbers, each once, has no more arguments than argument
registers, and returns no value if it has exits (`asm!` cannot have a label
and an output: rust-lang/rust#119364); one of a naked target declares no
clobbers and no generic parameters. -/
def checkInline (a : Artifact) : Except String Unit := do
  let T := a.target
  let n := s!"{T.name}: {a.name}"
  if T.inline then
    if usesStack a.code then throw s!"{n} is inline but calls a function or has a frame"
    for c in a.clobbers do
      unless T.regs.contains c do throw s!"{n} clobbers `{c}`, which is not a register it may name"
    unless (dedup a.clobbers).length == a.clobbers.length do throw s!"{n} declares a clobber twice"
    unless (argNames a.sig).length ≤ T.argRegs.length do throw s!"{n} has too many arguments"
    unless (exitNames a.code).isEmpty || a.sig.ret.isNone do
      throw s!"{n} has exits and returns a value, which `asm!` cannot combine"
  else
    unless a.clobbers.isEmpty && a.generics.isEmpty do
      throw s!"{n} is a naked function but declares clobbers or generic parameters"

/-- On a target whose printer `P` has a limit of fused micro-ops
(`Printer.uopLimit`), the hot path of the code `c` of the function `n` has
at most that many (`Printer.hotUops`); code whose hot path the count is not
defined for is refused there, rather than let through uncounted. -/
def uopCheck {M : ISA} (P : Printer M) (inline : Bool) (n : String) (c : Prog M) :
    Except String Unit := do
  if let some lim := P.uopLimit then
    match P.hotUops inline c with
    | some k =>
      unless k ≤ lim do
        throw s!"{n}'s hot path has {k} fused micro-ops, more than {lim}"
    | none =>
      throw s!"{n} is neither a block nor a restartable sequence, so its hot path's fused \
        micro-ops cannot be counted against the limit of {lim}"

def checkUops (a : Artifact) : Except String Unit :=
  uopCheck a.target.printer a.target.inline s!"{a.target.name}: {a.name}" a.code

/-- The tables of constants of `a`, which its contract gives it
(`Artifact.consts`). -/
def tablesOf (a : Artifact) : List (String × List (BitVec 64)) := a.consts

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

Only the first call of each name is printed and compared with `printed`;
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

/-- Whether `n` is a plain Rust identifier: an ASCII letter or `_`, then
ASCII letters, digits and `_`. -/
def rustIdent (n : String) : Bool :=
  match n.toList with
  | [] => false
  | c :: cs => (c.isAlpha || c == '_') && cs.all fun c => c.isAlphanum || c == '_'

/-- The artifact's name and module are Rust identifiers: the name is a
function's, and the module names the file `<target>/<module>.rs`, which must
stay inside the output directory. -/
def checkNames (a : Artifact) : Except String Unit := do
  unless rustIdent a.name do
    throw s!"{a.target.name}: `{a.name}` is not a Rust identifier"
  unless rustIdent a.module && a.module != "mod" do
    throw s!"{a.target.name}: {a.name}'s module `{a.module}` is not a Rust identifier (or is `mod`)"

/-- Whether `c`, or the code of a function it calls, has a restartable
sequence. -/
def hasRseq {I C : Type} : Code I C → Bool
  | .block _ => false
  | .seq a b => hasRseq a || hasRseq b
  | .ite _ t e => hasRseq t || hasRseq e
  | .loop b _ => hasRseq b
  | .call _ b => hasRseq b
  | .frame _ b _ => hasRseq b
  | .rseq .. => true

/-- The code of `p`'s artifact has a restartable sequence only as the whole
of its code (`Code.rseq arm body`): the printer gives every sequence the
same labels and its abort handler restarts it at its arm
(`Printer.rseqLines`), which is then the function's first instruction, as
`Code.abortStates` assumes. Then the target can print a sequence
(`Printer.rseqSig`), the arm is not empty (`Verified` checks that its last
instruction arms the sequence), the body ends with its commit, a plain
store (`Printer.plainStore`): a store of a value the code computed, never a
read-modify-write of memory another thread may have changed meanwhile; and
each external exit jumps to a function the artifact declares (a `sym`
operand), which it tail-calls with its arguments as they arrived
(`Sig.contract`'s `exit`). -/
def checkRseq (p : Prepared) : Except String Unit := do
  let a := p.art
  let P := a.target.printer
  match a.code with
  | .rseq arm body =>
    unless P.rseqSig.isSome do
      throw s!"{a.target.name}: {a.name} has a restartable sequence, which its target cannot print"
    if arm.isEmpty then
      throw s!"{a.target.name}: {a.name}'s restartable sequence has no arming code"
    match body.getLast? with
    | some (.instr i) =>
      unless P.plainStore i do
        throw s!"{a.target.name}: {a.name}'s restartable sequence does not commit with a plain store"
    | _ => throw s!"{a.target.name}: {a.name}'s restartable sequence does not end with its commit"
    for st in body do
      if let .exit _ _ (some t) := st then
        if a.target.inline then
          if (a.operands.lookup t).isSome then
            throw s!"{a.target.name}: {a.name} exits to `{t}`, which inline is a label, not an operand"
        else unless a.operands.lookup t matches some (.sym _) do
          throw s!"{a.target.name}: {a.name} exits to `{t}`, which is not a function it declares \
            (`Operand.sym`)"
  | c =>
    if hasRseq c then
      throw s!"{a.target.name}: {a.name} has a restartable sequence that is not the whole of its code"

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

/-- The constants of the artifacts `as` (`Artifact.rustConsts`), by name,
each once, in order of their names. -/
def rustConstsOf (as : List Artifact) : List (String × IntTy × Nat) :=
  let all := as.flatMap (·.rustConsts)
  ((dedup (all.map (·.1))).mergeSort (· ≤ ·)).filterMap fun n => all.find? (·.1 == n)

/-- A constant's name is a Rust constant's, its value fits its type, and the
artifacts of a module that give a constant of that name give the same type
and value. -/
def checkRustConsts (as : List Artifact) (a : Artifact) : Except String Unit := do
  for (n, ty, v) in a.rustConsts do
    unless tableName n do
      throw s!"{a.target.name}: {a.name}'s constant {n} is not named as a Rust constant"
    unless v < 2 ^ ty.bits a.target.abi.ptrBits do
      throw s!"{a.target.name}: {a.name}'s constant {n} does not fit in a {ty.rust}"
    for b in as do
      if b.target.name == a.target.name && b.module == a.module then
        for (m, ty', v') in b.rustConsts do
          if m == n && (ty' != ty || v' != v) then
            throw s!"{a.target.name}: {a.name} and {b.name} give the constant {n} different values"

/-- The constants of a module's artifacts, as Rust. -/
def rustConstsText (cs : List (String × IntTy × Nat)) : String :=
  String.join (cs.map fun (n, ty, v) =>
    s!"\n/// From the specification (`Api.rustConsts`).\npub(crate) const {n}: {ty.rust} = {v};\n")

/-- Tables of the same name in `cs` and `ds` have the same words. -/
def consistent (cs ds : List (String × List (BitVec 64))) : Bool :=
  cs.all fun (n, ws) => ds.all fun (m, vs) => m != n || vs == ws

/-- The tables of constants of `a` (`Artifact.consts`) have distinct names;
every name is a Rust constant's, none is named `consts` or the module of an
artifact; every artifact of the target with a table of one of these names
gives the same words; its code names only these tables; and its target's
code can name a `static` (`Abi.sym`), if it has any. Of `p`'s artifact,
checked against the artifacts of `ps`. -/
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

/-- Every operand `p`'s code names (`Line.refs`) is one of its tables of
constants or of its declared operands (`Artifact.operands`), and none is
both; each declared operand has a distinct name, which is a Rust identifier
(the name of an `asm!` operand) and not one the emitter declares itself. -/
def checkOperands (p : Prepared) : Except String Unit := do
  let a := p.art
  let tables := a.consts.map (·.1)
  let names := a.operands.map (·.1)
  unless (dedup names).length == names.length do
    throw s!"{a.target.name}: {a.name} declares two operands of the same name"
  for n in names do
    unless rustIdent n do
      throw s!"{a.target.name}: {a.name}'s operand `{n}` is not a Rust identifier"
    if tables.contains n then
      throw s!"{a.target.name}: {a.name}'s operand `{n}` is also a table of constants"
    if (a.code.calls.map (·.1)).contains n then
      throw s!"{a.target.name}: {a.name}'s operand `{n}` is also a function it calls"
  let labels := if a.target.inline then exitNames a.code else []
  for n in dedup (p.lines.flatMap Line.refs) do
    unless tables.contains n || names.contains n || labels.contains n do
      throw s!"{a.target.name}: {a.name} names `{n}`, which is neither a table of constants \
        (`Artifact.consts`) nor an operand (`Artifact.operands`)"

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

/-- How many tasks `files` checks the artifacts on. The checks, and
`render`, are pure functions of the artifacts, so running them on tasks
(`Task.spawn`) changes how long they take, never what they return. -/
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

/-- The root module, `mod.rs`: a `mod` declaration of each target of `ts`
(its name and Rust `cfg`), in order of their names. -/
def rootFile (ts : List (String × String)) : String :=
  header ++
  "//! Formally verified assembly, emitted from Lean. See `lean/README.md`.\n" ++
  "//!\n" ++
  "//! Every function in these modules is the direct rendering of an `Artifact`\n" ++
  "//! whose machine code has been proven correct and memory safe against its\n" ++
  "//! contract.\n" ++
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
    -- Each module's file on a task of its own.
    ((modules.map fun m => Task.spawn fun _ =>
      (s!"{t}/{m}.rs",
        header ++ s!"//! Verified `{m}` functions for `{t}`.\n" ++
        "#![allow(dead_code)]\n" ++
        rustConstsText (rustConstsOf ((arts.filter (·.art.module == m)).map (·.art))) ++
        String.join ((arts.filter (·.art.module == m)).map fun p =>
          "\n" ++ (if p.art.target.inline then inlineFunction p.art (inlineLines p.art)
            else function.functionOf p.art p.lines (moduleOf p.art))))).map Task.get)
  -- Each target's files on a task of their own.
  ("mod.rs", root) :: ((targets.map fun t => Task.spawn fun _ => perTarget t).map Task.get).flatten

/-- The generated files, as paths relative to the output directory and their
contents: `mod.rs`, and for each target `<target>/mod.rs` and one
`<target>/<module>.rs` per module; an error if an artifact is not made from
an `Api` with a contract (`checkApi`), its name or module is not a Rust
identifier (`checkNames`), two artifacts of a target have the
same name (`checkUnique`), a call is not of the code it runs (`checkCalls`),
its tables of constants are not as `checkConsts` requires, its code names
an operand it does not declare (`checkOperands`), a restartable sequence is
not as `checkRseq` requires, an artifact's
features are not those its code requires (`checkFeatures`), its code has an
instruction its target's printer cannot encode (`checkEncodable`), or its
doc has no `# Safety` section at its end for what `fullDoc` adds to it
(`checkLayout`). Each check of every artifact runs on `tasks` tasks
(`forMPar`), one check after the other, so the error is the one checking
them in order would give; what they read of an artifact more than once is
computed once, on `tasks` tasks (`Prepared`, `prepareAll`). -/
def files (as : List Artifact) : Except String (List (String × String)) := do
  -- The same artifacts, with equal parts the same value in memory
  -- (`ShareCommon.shareCommon'` returns its argument): the code of every call
  -- of a function in an artifact is then the same value as its first call's,
  -- which `badCall` and `distinctCalls` skip at once.
  let as := ShareCommon.shareCommon' as
  forMPar as checkApi
  forMPar as checkNames
  checkUnique as
  let ps := prepareAll as
  forMPar ps (checkCalls ps)
  forMPar ps (checkConsts ps)
  forMPar ps checkOperands
  forMPar ps checkRseq
  forMPar ps checkFeatures
  forMPar ps checkEncodable
  forMPar as checkLayout
  forMPar as checkInline
  forMPar as checkUops
  forMPar as (checkRustConsts as)
  return render ps fun a n => ((callee as a n).map (·.module)).getD ""

end VG.Rust
