module

public import Asmalloc.TCB.Code

/-!
# Lowering structured code to assembly text

**Trusted.** Structured control flow is lowered to local labels and
conditional branches:

* `ite c t e`  ⟶  `b<c> Lthen; e; b Lend; Lthen: t; Lend:`
* `loop body c` ⟶  `Ltop: body; b<c> Ltop`
* `call name body` ⟶  `<call> name` (the call instruction of the function
  `name`, which is emitted separately: see `VG.Rust.files`)
* an instruction naming a symbol or a compile-time parameter ⟶ a line that
  refers to it as an operand of the `asm!` block (`Line.ops`, `Piece.ref`),
  which `VG.Rust.files` declares (`sym` or `const`)
* `frame push body pop` ⟶  `push; body; pop`

Structured-control-flow labels are numeric local labels (`N:`, referenced
as `Nf` forward or `Nb` backward), the only kind Rust allows in inline
assembly (the `named_asm_labels` lint). Each has a distinct number, so each
reference has exactly one target. The numbers are `20`, `21`, `22`, …
(`2` followed by a counter): a number of only `0`s and `1`s could be read as a
binary literal in Intel syntax.

A printer's `funcAlign` is an assembler alignment directive (e.g.
`.p2align 6`) that `VG.Rust.function` emits after everything else in the
function, so that where the function's code lies relative to the processor's
fetch blocks and decoded-instruction cache lines depends on the function
alone, not on the size of the code the linker happens to place before it.
The directive raises the alignment of the function's section, so the
function's first instruction lies on the boundary (`naked_asm!` gives each
function a section of its own on ELF targets). Whatever padding the
assembler inserts at the directive comes after the return that ends the
function (`Printer.function`), so it is never executed: the printed code is
the same instructions, at the same distances from one another.

A cold branch (`Code.iteCold c t e`, an `ite` whose `then` branch is
marked by two empty blocks) is laid out out of line: `if c` branches to
`t`'s code after the function's return, `e` falls through to the code after
the `ite`, and `t`'s code ends with a jump back to it. So the instructions
of the code around the `ite` and of `e`, which the hot path runs, are the
same, at the same addresses, whatever `t` is. The cold code comes before the
`funcAlign` directives. The printer keys on exactly that shape: a `then`
branch `.seq (.seq (.block []) (.block [])) t` is reserved for cold branches.

Together with each ISA's instruction printer this is part of the trusted
base; it is small enough to check by inspection and is covered by the golden
tests in `AsmallocTest/Print.lean`.
-/

@[expose] public section

namespace VG

/-- The text of a constant added to a symbol in an operand: nothing for 0,
` + k` or ` - k` otherwise. -/
def symOffset (off : Int) : String :=
  if off = 0 then "" else if off > 0 then s!" + {off}" else s!" - {-off}"

/-- Part of a line of assembly: text, or a reference to an operand of the
`asm!` block by its name (a symbol, a compile-time constant, or a label),
which the template writes as `{name}`. -/
inductive Piece
  | text (s : String)
  | ref (name : String)
  deriving DecidableEq, Repr

/-- A line of assembly: text, a call instruction of the function it names,
or text with references to operands (`Piece`). -/
inductive Line
  | text (s : String)
  | call (name : String)
  | ops (ps : List Piece)
  deriving DecidableEq, Repr

/-- The operands a line refers to (`Piece.ref`). -/
def Line.refs : Line → List String
  | .ops ps => ps.filterMap fun | .ref n => some n | .text _ => none
  | _ => []

structure Printer (M : ISA) where
  /-- The lines of one instruction. -/
  instr : M.Instr → List Line
  /-- Conditional branch to a label, taken when the condition is true. -/
  branch : M.Cond → String → String
  /-- Unconditional branch to a label. -/
  jump : String → String
  /-- Return to the caller. -/
  ret : List String
  /-- The mnemonic of the call instruction whose operand is a function's
  symbol (`ISA.call`), e.g. `call`. -/
  call : String
  /-- Why the assembler cannot encode an instruction as the model describes
  it (e.g. an x86-64 displacement too wide for its field, which some
  assemblers silently truncate), or `none` if it can. The emitter refuses
  code containing such an instruction (`Rust.checkEncodable`). -/
  unencodable : M.Instr → Option String := fun _ => none
  /-- Assembler directives after the end of each function, aligning its
  start (see the module docs). They come after its return, so any padding
  they insert is never executed. -/
  funcAlign : List String := []

variable {M : ISA} (P : Printer M)

/-- The number of the `n`-th label of a function. -/
def labelNum (n : Nat) : String := s!"2{n}"

/-- Lower code to lines of assembly, using labels `labelNum n`: the lines in
place, the lines of its cold branches (`Code.iteCold`), placed after the
function's return, and the next unused label number. -/
def Printer.lower : Code M.Instr M.Cond → Nat → List Line × List Line × Nat
  | .block is, n => (is.flatMap P.instr, [], n)
  | .seq c₁ c₂, n =>
    let (l₁, k₁, n) := lower c₁ n
    let (l₂, k₂, n) := lower c₂ n
    (l₁ ++ l₂, k₁ ++ k₂, n)
  | .ite c (.seq (.seq (.block []) (.block [])) t) e, n =>
    let lCold := labelNum n
    let lBack := labelNum (n + 1)
    let (le, ke, n) := lower e (n + 2)
    let (lt, kt, n) := lower t n
    ([.text (P.branch c (lCold ++ "f"))] ++ le ++ [.text (lBack ++ ":")],
      ke ++ [.text (lCold ++ ":")] ++ lt ++ [.text (P.jump (lBack ++ "b"))] ++ kt, n)
  | .ite c t e, n =>
    let lThen := labelNum n
    let lEnd := labelNum (n + 1)
    let (le, ke, n) := lower e (n + 2)
    let (lt, kt, n) := lower t n
    ([.text (P.branch c (lThen ++ "f"))] ++ le ++ [.text (P.jump (lEnd ++ "f")), .text (lThen ++ ":")] ++
      lt ++ [.text (lEnd ++ ":")], ke ++ kt, n)
  | .loop body c, n =>
    let lTop := labelNum n
    let (lb, kb, n) := lower body (n + 1)
    ([.text (lTop ++ ":")] ++ lb ++ [.text (P.branch c (lTop ++ "b"))], kb, n)
  | .call name _, n => ([.call name], [], n)
  | .frame i body j, n =>
    let (lb, kb, n) := lower body n
    (P.instr i ++ lb ++ P.instr j, kb, n)

/-- The complete body of a function: the lowered code, the return, and the
cold branches' code after it. -/
def Printer.function (body : Code M.Instr M.Cond) : List Line :=
  let (l, k, _) := P.lower body 0
  l ++ P.ret.map .text ++ k

end VG
