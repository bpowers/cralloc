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

A restartable sequence (`Code.rseq arm body`) is printed with fixed labels,
which the emitter allows only once per function (`Rust.checkRseq`):

```
5:  arm                       restart point: the abort handler jumps here
6:  body                      start of the sequence (start_ip)
7:                            just past the commit
    .pushsection __rseq_cs, "aw"
    .balign 32
3:  .long 0, 0                the descriptor, struct rseq_cs: version, flags,
    .quad 6b, 7b - 6b, 4f     start_ip, post_commit_offset, abort_ip
    .popsection
    .pushsection .text.unlikely, "ax"
    <rseqSig>                 ends with the 4-byte signature before abort_ip
4:  jmp 5b                    the abort handler
    <exits' code>             out of line (`lowerBody`)
    .popsection
```

An exit straight to an external target, with no code of its own, is a
branch to it; or, if the printer has `stubExits`, a branch to a stub
`N: jmp target` placed after the function's return, so that the assembler
can give the branch its short form. Either way the branch leaves the
sequence for the target with nothing else run in between.

So the abort handler lies outside the sequence, after the signature, and
the arming code (x86-64's `leaRseqCs`, `lea r, [rip + 3f]`) names this
descriptor. The descriptor holds absolute addresses, which the dynamic
linker relocates, so its section is writable.

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
  /-- The lines just before a restartable sequence's abort handler, which
  must end with the signature the C library registers its area with; `none`
  on a target that cannot print a restartable sequence (`Rust.checkRseq`). -/
  rseqSig : Option (List String) := none
  /-- Whether an instruction is a plain store of a register, which is what
  a restartable sequence's commit must be (`Rust.checkRseq`). -/
  plainStore : M.Instr → Bool := fun _ => false
  /-- Whether an instruction fuses with a conditional branch right after it
  into one micro-op (a compare or a test). -/
  fuses : M.Instr → Bool := fun _ => false
  /-- The most fused micro-ops a function's hot path may have (`Rust.checkUops`),
  if the target has a limit. -/
  uopLimit : Option Nat := none
  /-- Whether a restartable sequence's exit straight to an external target
  branches to a stub after the function's return, `N: jmp target`, rather
  than to the target itself. The assembler can then encode the branch in its
  short form, the stub's jump taking the long one off the hot path. Only for
  a function that has a return to put the stubs after. -/
  stubExits : Bool := false

variable {M : ISA} (P : Printer M)

/-- The number of the `n`-th label of a function. -/
def labelNum (n : Nat) : String := s!"2{n}"

/-- The lines of a restartable sequence's body, of its exits' code out of
line, and of its exits' stubs (`stubExits`), using labels `labelNum n`: an
exit with no code of its own branches straight to its external target, or
to a stub of its own that jumps there, or to the end of the sequence (label
`7`); any other branches to a label of its own, whose code ends with a jump
to the end of the sequence or to its external target. -/
def Printer.lowerBody : List (Step M.Instr M.Cond) → Nat → List Line × List Line × List Line × Nat
  | [], n => ([], [], [], n)
  | .instr i :: rest, n =>
    let (l, k, st, n) := lowerBody rest n
    (P.instr i ++ l, k, st, n)
  | .exit c [] (some t) :: rest, n =>
    if P.stubExits then
      let lab := labelNum n
      let (l, k, st, n) := lowerBody rest (n + 1)
      (.text (P.branch c (lab ++ "f")) :: l, k,
        [.text (lab ++ ":"), .ops [.text (P.jump ""), .ref t]] ++ st, n)
    else
      let (l, k, st, n) := lowerBody rest n
      (.ops [.text (P.branch c ""), .ref t] :: l, k, st, n)
  | .exit c [] none :: rest, n =>
    let (l, k, st, n) := lowerBody rest n
    (.text (P.branch c "7f") :: l, k, st, n)
  | .exit c cold ext :: rest, n =>
    let lab := labelNum n
    let (l, k, st, n) := lowerBody rest (n + 1)
    let back : Line := match ext with
      | none => .text (P.jump "7b")
      | some t => .ops [.text (P.jump ""), .ref t]
    (.text (P.branch c (lab ++ "f")) :: l,
      [.text (lab ++ ":")] ++ cold.flatMap P.instr ++ [back] ++ k, st, n)

/-- The lines of the restartable sequence `rseq arm body` (see the module
docs), given its body's lines and its exits' code (`lowerBody`). -/
def Printer.rseqLines (arm : List M.Instr) (body cold : List Line) : List Line :=
  [.text "5:"] ++ arm.flatMap P.instr ++ [.text "6:"] ++ body ++ [.text "7:"] ++
  ([".pushsection __rseq_cs, \"aw\"", ".balign 32", "3:", ".long 0, 0",
    ".quad 6b, 7b - 6b, 4f", ".popsection", ".pushsection .text.unlikely, \"ax\""] ++
    P.rseqSig.getD [] ++ ["4:", P.jump "5b"]).map .text ++
  cold ++ [.text ".popsection"]

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
  | .rseq arm body, n =>
    let (lb, kb, st, n) := P.lowerBody body n
    (P.rseqLines arm lb kb, st, n)

/-- The fused micro-ops of a restartable sequence's body when no exit is
taken: one per instruction and per exit, but an exit right after an
instruction that fuses with it (`fuses`) counts with that instruction. -/
def Printer.stepUops : List (Step M.Instr M.Cond) → Nat
  | [] => 0
  | .instr i :: .exit _ _ _ :: rest => (if P.fuses i then 1 else 2) + stepUops rest
  | _ :: rest => 1 + stepUops rest

/-- The fused micro-ops of a function's hot path, if the code has one the
count is defined for: a block (every instruction), or a restartable
sequence when no exit is taken (its arm and its body, `stepUops`); and the
return, unless `inline`. Each instruction counts one. -/
def Printer.hotUops (inline : Bool) : Code M.Instr M.Cond → Option Nat
  | .block is => some (is.length + if inline then 0 else P.ret.length)
  | .rseq arm body => some (arm.length + P.stepUops body + if inline then 0 else P.ret.length)
  | _ => none

/-- The complete body of a function: the lowered code, the return, and the
cold branches' code after it. -/
def Printer.function (body : Code M.Instr M.Cond) : List Line :=
  let (l, k, _) := P.lower body 0
  l ++ P.ret.map .text ++ k

end VG
