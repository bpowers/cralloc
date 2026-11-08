module

public import Cralloc.TCB.Code

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
* `jump is t` ⟶  `is; jmp t` (the emitter allows it only after a sequence)

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
    <the word after it>       if the routine reads one (`Printer.descWord`)
    .popsection
    <coldOpen>                where its out-of-line code goes
    <rseqSig>                 ends with the 4-byte signature before abort_ip
4:  jmp 5b                    the abort handler
    <exits' code>             out of line (`lowerBody`)
    <coldClose>
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

A routine may run instructions once before its sequence (`Artifact.setup`):
they are printed first, ahead of the label `5`, so that the abort handler does
not run them again (`Printer.routine`). A word it reads after the descriptor
(`Printer.descWord`) is placed there by this printer, since the descriptor is
printed with the sequence, after the instruction that reads it.

Together with each ISA's instruction printer this is part of the trusted
base; it is small enough to check by inspection and is covered by the golden
tests in `CrallocTest/Print.lean`.
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
  /-- Assembler directives that open and close a function's unwind
  information, for a function that never moves the stack pointer
  (`Rust.unwindLines`); empty on a target that emits none. -/
  unwindStart : List String := []
  unwindEnd : List String := []
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
  /-- The assembler directives around a function's out-of-line code: its
  abort handler, its exits' code, and inline its cold branches. -/
  coldOpen : List String := [".pushsection .text.unlikely, \"ax\""]
  coldClose : List String := [".popsection"]
  /-- Whether the out-of-line code of an inlined function may be out of reach
  of a conditional branch in it, since the compiler may make the function any
  size (AArch64's reaches 1 MiB, and a linker cannot put a veneer on one). The
  emitter then allows an inlined sequence no exit with code of its own or to
  an external target (`Rust.checkRseq`). -/
  coldFar : Bool := false
  /-- The symbol whose address an instruction reads from the word after the
  routine's descriptor, if it is such a load (`Rust.checkDescWord`). The printer
  places the word there (`Printer.rseqLines`). -/
  descWord : M.Instr → Option String := fun _ => none
  /-- Why an instruction may not be in a restartable sequence's arm or a step of its
  body, if so (`Rust.checkRseq`). The model has one state before each step, where an
  abort may come, and the range of addresses it covers is the sequence's as the assembler
  laid it out. So a step must be one instruction that stays where it was printed, with at
  most one store, which a misaligned one may not be. -/
  bodyRefuses : M.Instr → Option String := fun _ => none
  /-- Whether an instruction reads or writes memory another thread can change, and so may
  not run outside a restartable sequence, after its commit or in an exit's code
  (`Rust.checkRseq`): nothing there stops another thread running first, which `Exec` does
  not model. A word the dynamic linker fills before any code runs is not such memory. -/
  accessesMemory : M.Instr → Bool := fun _ => true
  /-- Whether an instruction names the routine's descriptor by its label, `3f`, which is
  this routine's only ahead of the descriptor (`Printer.rseqLines`); anywhere else it is
  the next one the assembler sees (`Rust.checkRseq`). -/
  namesDesc : M.Instr → Bool := fun _ => false
  /-- Whether a sequence's arm must be exactly one store (`Rust.checkRseq`): on a target
  whose abort returns to it alone (`Artifact.setup`), so that nothing is read there,
  outside the range an abort covers, and then relied on. -/
  armOneStore : Bool := false
  /-- Whether, after an instruction, a linker may move a later one out of line, up to
  `moveReach` instructions on. Such an instruction may not be so near a sequence that
  what moves is in it (`Rust.checkRseq`). -/
  movesAfter : M.Instr → Bool := fun _ => false
  moveReach : Nat := 0
  /-- Text that, in a line of a function, means it may move the stack pointer
  or the return address, so that it gets no unwind information
  (`Rust.unwindLines`). -/
  stackText : List String := ["rsp", "esp", "push ", "pop ", "call"]

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
docs), given its body's lines, its exits' code (`lowerBody`) and the lines of
the word after its descriptor. -/
def Printer.rseqLines (arm : List M.Instr) (body cold desc : List Line) : List Line :=
  [.text "5:"] ++ arm.flatMap P.instr ++ [.text "6:"] ++ body ++ [.text "7:"] ++
  ([".pushsection __rseq_cs, \"aw\"", ".balign 32", "3:", ".long 0, 0",
    ".quad 6b, 7b - 6b, 4f"]).map .text ++ desc ++
  ([".popsection"] ++ P.coldOpen ++ P.rseqSig.getD [] ++ ["4:", P.jump "5b"]).map .text ++
  cold ++ P.coldClose.map .text

/-- Lower code to lines of assembly, using labels `labelNum n`: the lines in
place, the lines of its cold branches (`Code.iteCold`), placed after the
function's return, and the next unused label number. `desc` is the word after
a restartable sequence's descriptor (`Printer.rseqLines`). -/
def Printer.lowerWith (desc : List Line) : Code M.Instr M.Cond → Nat → List Line × List Line × Nat
  | .block is, n => (is.flatMap P.instr, [], n)
  | .seq c₁ c₂, n =>
    let (l₁, k₁, n) := lowerWith desc c₁ n
    let (l₂, k₂, n) := lowerWith desc c₂ n
    (l₁ ++ l₂, k₁ ++ k₂, n)
  | .ite c (.seq (.seq (.block []) (.block [])) t) e, n =>
    let lCold := labelNum n
    let lBack := labelNum (n + 1)
    let (le, ke, n) := lowerWith desc e (n + 2)
    let (lt, kt, n) := lowerWith desc t n
    ([.text (P.branch c (lCold ++ "f"))] ++ le ++ [.text (lBack ++ ":")],
      ke ++ [.text (lCold ++ ":")] ++ lt ++ [.text (P.jump (lBack ++ "b"))] ++ kt, n)
  | .ite c t e, n =>
    let lThen := labelNum n
    let lEnd := labelNum (n + 1)
    let (le, ke, n) := lowerWith desc e (n + 2)
    let (lt, kt, n) := lowerWith desc t n
    ([.text (P.branch c (lThen ++ "f"))] ++ le ++ [.text (P.jump (lEnd ++ "f")), .text (lThen ++ ":")] ++
      lt ++ [.text (lEnd ++ ":")], ke ++ kt, n)
  | .loop body c, n =>
    let lTop := labelNum n
    let (lb, kb, n) := lowerWith desc body (n + 1)
    ([.text (lTop ++ ":")] ++ lb ++ [.text (P.branch c (lTop ++ "b"))], kb, n)
  | .call name _, n => ([.call name], [], n)
  | .frame i body j, n =>
    let (lb, kb, n) := lowerWith desc body n
    (P.instr i ++ lb ++ P.instr j, kb, n)
  | .rseq arm body, n =>
    let (lb, kb, st, n) := P.lowerBody body n
    (P.rseqLines arm lb kb desc, st, n)
  | .jump is t, n => (is.flatMap P.instr ++ [.ops [.text (P.jump ""), .ref t]], [], n)

/-- `lowerWith` for code with no word after its descriptor. -/
abbrev Printer.lower : Code M.Instr M.Cond → Nat → List Line × List Line × Nat := P.lowerWith []

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
return, unless `inline`; or a sequence followed by a `jump`, whose
instructions and jump replace the return. Each instruction counts one. -/
def Printer.hotUops (inline : Bool) : Code M.Instr M.Cond → Option Nat
  | .block is => some (is.length + if inline then 0 else P.ret.length)
  | .rseq arm body => some (arm.length + P.stepUops body + if inline then 0 else P.ret.length)
  -- A sequence that ends with a tail call: the instructions after the commit and the jump, in
  -- place of the return.
  | .seq (.rseq arm body) (.jump is _) => some (arm.length + P.stepUops body + is.length + 1)
  | _ => none

/-- The word after the descriptor of a routine whose setup is `setup`: its
first `descWord`, of which `Rust.checkDescWord` allows only the one. -/
def Printer.descLines (setup : List M.Instr) : List Line :=
  match setup.findSome? P.descWord with
  | some n => [.ops [.text ".quad ", .ref n]]
  | none => []

/-- The lines of the instructions a routine runs once on entry, `setup`, and
of its lowered code `body`, without its return: the lines in place, those of
its cold branches, and the next unused label number. -/
def Printer.lowerRoutine (setup : List M.Instr) (body : Code M.Instr M.Cond) :
    List Line × List Line × Nat :=
  let (l, k, n) := P.lowerWith (P.descLines setup) body 0
  (setup.flatMap P.instr ++ l, k, n)

/-- The complete body of a routine: `setup`, the lowered code, the return, and
the cold branches' code after it. -/
def Printer.routine (setup : List M.Instr) (body : Code M.Instr M.Cond) : List Line :=
  let (l, k, _) := P.lowerRoutine setup body
  -- Code that ends in a tail call never reaches a return.
  let ret := match body with
    | .seq _ (.jump ..) => []
    | _ => P.ret.map .text
  l ++ ret ++ k

/-- The complete body of a function with no setup. -/
abbrev Printer.function (body : Code M.Instr M.Cond) : List Line := P.routine [] body

end VG
