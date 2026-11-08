module

public import Cralloc.TCB.AArch64.Isa
public import Cralloc.TCB.Print

/-!
# Printer for the AArch64 model

**Trusted.** Emits the LLVM assembler syntax of Rust's `asm!` and `naked_asm!`
on AArch64. The model describes an instruction with an operand of any value; an
assembler given one that does not fit rewrites it into another instruction or
refuses it. So the printer refuses first what no instruction can encode
(`Instr.unencodable`), and `lean/scripts/check_aarch64.py` assembles what it
prints and runs it against the model.
-/

@[expose] public section

namespace VG.AArch64

/-- The signature of the threads' rseq registrations on AArch64 (glibc's
`RSEQ_SIG`, sysdeps/unix/sysv/linux/aarch64/bits/rseq.h), which the kernel requires
in the four bytes before every abort handler (`Printer.rseqSig`). It is the
encoding of `brk #0x45e0`, so that a stray jump into it traps. -/
def rseqSignature : Nat := 0xd428bc00

/-- `n` in hexadecimal, as `0x…`. -/
def hexLit (n : Nat) : String := "0x" ++ String.ofList (Nat.toDigits 16 n)

/-- A register's 64-bit name. -/
def Reg.x : Reg → String
  | .x0 => "x0" | .x1 => "x1" | .x2 => "x2" | .x3 => "x3" | .x4 => "x4" | .x5 => "x5"
  | .x6 => "x6" | .x7 => "x7" | .x8 => "x8" | .x9 => "x9" | .x10 => "x10" | .x11 => "x11"
  | .x12 => "x12" | .x13 => "x13" | .x14 => "x14" | .x15 => "x15" | .x16 => "x16" | .x17 => "x17"
  | .x19 => "x19" | .x20 => "x20" | .x21 => "x21" | .x22 => "x22" | .x23 => "x23" | .x24 => "x24"
  | .x25 => "x25" | .x26 => "x26" | .x27 => "x27" | .x28 => "x28" | .x30 => "x30"

/-- A register's 32-bit name. -/
def Reg.w : Reg → String
  | .x0 => "w0" | .x1 => "w1" | .x2 => "w2" | .x3 => "w3" | .x4 => "w4" | .x5 => "w5"
  | .x6 => "w6" | .x7 => "w7" | .x8 => "w8" | .x9 => "w9" | .x10 => "w10" | .x11 => "w11"
  | .x12 => "w12" | .x13 => "w13" | .x14 => "w14" | .x15 => "w15" | .x16 => "w16" | .x17 => "w17"
  | .x19 => "w19" | .x20 => "w20" | .x21 => "w21" | .x22 => "w22" | .x23 => "w23" | .x24 => "w24"
  | .x25 => "w25" | .x26 => "w26" | .x27 => "w27" | .x28 => "w28" | .x30 => "w30"

/-- A register's name at an operand size. -/
def Reg.name (sz : Size) (r : Reg) : String :=
  match sz with
  | .x => r.x
  | .w => r.w

/-- A line of text and operands: plain text if it refers to no operand. -/
def line (ps : List Piece) : Line :=
  if ps.any (fun | .ref _ => true | .text _ => false) then .ops ps
  else .text (String.join (ps.map fun | .text s => s | .ref n => n))

/-- An address relative to the program counter, as an operand: `t + off` for a
symbol's, of which the linker writes the part an instruction holds; label `3`
for the descriptor's. -/
def Rel.pieces : Rel → List Piece
  | .sym n off => [.ref n, .text (symOffset off)]
  | .desc => [.text "3f"]

/-- A memory operand. -/
def MemOp.pieces (m : MemOp) : List Piece :=
  match m.off with
  | .imm 0 => [.text s!"[{m.base.x}]"]
  | .imm d => [.text s!"[{m.base.x}, #{d}]"]
  | .lo12 t => [.text s!"[{m.base.x}, :lo12:"] ++ t.pieces ++ [.text "]"]
  | .reg r 0 => [.text s!"[{m.base.x}, {r.x}]"]
  | .reg r sh => [.text s!"[{m.base.x}, {r.x}, lsl #{sh}]"]

/-- An `add`, `sub` or `cmp` immediate: 12 bits, or 12 bits shifted by 12, which
is written out rather than left for the assembler to choose. -/
def addImmText (v : Nat) : String :=
  if v < 4096 then s!"#{v}" else s!"#{v / 4096}, lsl #12"

/-- A second operand, with `imm` the text of an immediate. -/
def Src.text (sz : Size) (imm : Nat → String) : Src → String
  | .reg r => r.name sz
  | .lsl r n => s!"{r.name sz}, lsl #{n}"
  | .imm v => imm v

/-- The second operand of an `add`, `sub` or `cmp`. -/
def Src.arith (sz : Size) : Src → String := Src.text sz addImmText

/-- The second operand of a logical instruction: an immediate in hexadecimal. -/
def Src.logical (sz : Size) : Src → String := Src.text sz fun v => s!"#{hexLit v}"

/-- A word of the routine's own holding the symbol's address, and a load of it
through its page: a global offset table slot done by hand, as on x86-64
(`VG.X86_64.gotLines`): the dynamic linker fills it (`R_AARCH64_ABS64`) in the
same pass it would fill such a slot, in relocated read-only data, and the
16-aligned word overlaps no hot word's page offset. The label `8` is local to
these lines. -/
def gotLines (d : Reg) (sym : String) : List Line :=
  [".pushsection .data.rel.ro.cralloc_got,\"aw\",%progbits", ".balign 16", "8:"].map .text ++
  [.ops [.text ".quad ", .ref sym], .text ".popsection", .text s!"adrp {d.x}, 8b",
    .text s!"ldr {d.x}, [{d.x}, :lo12:8b]"]

/-- The lines of an instruction. -/
def Instr.lines : Instr → List Line
  | .ldr .x t m => [line (.text s!"ldr {t.x}, " :: m.pieces)]
  | .ldr .w t m => [line (.text s!"ldr {t.w}, " :: m.pieces)]
  | .ldr .b t m => [line (.text s!"ldrb {t.w}, " :: m.pieces)]
  | .str .x t m => [line (.text s!"str {t.x}, " :: m.pieces)]
  | .str .w t m => [line (.text s!"str {t.w}, " :: m.pieces)]
  | .str .b t m => [line (.text s!"strb {t.w}, " :: m.pieces)]
  | .stpz n off => [line (.text "stp xzr, xzr, " :: MemOp.pieces { base := n, off := .imm off })]
  | .add sz d n s => [.text s!"add {d.name sz}, {n.name sz}, {s.arith sz}"]
  | .sub sz d n s => [.text s!"sub {d.name sz}, {n.name sz}, {s.arith sz}"]
  | .and sz d n s => [.text s!"and {d.name sz}, {n.name sz}, {s.logical sz}"]
  | .orr sz d n s => [.text s!"orr {d.name sz}, {n.name sz}, {s.logical sz}"]
  | .mov sz d s => [.text s!"mov {d.name sz}, {s.text sz (s!"#{·}")}"]
  | .cmp sz n s => [.text s!"cmp {n.name sz}, {s.arith sz}"]
  | .tst sz n s => [.text s!"tst {n.name sz}, {s.logical sz}"]
  | .lsl sz d n sh => [.text s!"lsl {d.name sz}, {n.name sz}, #{sh}"]
  | .lsr sz d n sh => [.text s!"lsr {d.name sz}, {n.name sz}, #{sh}"]
  | .lslv sz d n m => [.text s!"lsl {d.name sz}, {n.name sz}, {m.name sz}"]
  | .lsrv sz d n m => [.text s!"lsr {d.name sz}, {n.name sz}, {m.name sz}"]
  | .mul sz d n m => [.text s!"mul {d.name sz}, {n.name sz}, {m.name sz}"]
  | .umulh d n m => [.text s!"umulh {d.x}, {n.x}, {m.x}"]
  | .adrp d t => [line (.text s!"adrp {d.x}, " :: t.pieces)]
  | .addLo12 d n t => [line (.text s!"add {d.x}, {n.x}, :lo12:" :: t.pieces)]
  | .mrsTp d => [.text s!"mrs {d.x}, tpidr_el0"]
  | .ldrWord d n _ => [.text s!"ldr {d.x}, [{n.x}, #32]"]
  | .movGot d name => gotLines d name
  | .addAreaHi d n => [.text s!"add {d.x}, {n.x}, :tprel_hi12:{ownAreaSym}"]
  | .addAreaLo d n => [.text s!"add {d.x}, {n.x}, :tprel_lo12_nc:{ownAreaSym}"]

/-- The branch of a condition, to the label `l`. A `tbz` or `tbnz` names the
register at the size whose bits hold the bit tested, as the assembler requires. -/
def Cond.branch (c : Cond) (l : String) : String :=
  match c with
  | .eq => s!"b.eq {l}"
  | .ne => s!"b.ne {l}"
  | .hs => s!"b.hs {l}"
  | .lo => s!"b.lo {l}"
  | .hi => s!"b.hi {l}"
  | .ls => s!"b.ls {l}"
  | .cbz sz r => s!"cbz {r.name sz}, {l}"
  | .cbnz sz r => s!"cbnz {r.name sz}, {l}"
  | .tbz r b => s!"tbz {r.name (if b.val < 32 then .w else .x)}, #{b.val}, {l}"
  | .tbnz r b => s!"tbnz {r.name (if b.val < 32 then .w else .x)}, #{b.val}, {l}"

/-! ## What an instruction can encode

DDI 0487 C6.2, the fields of each instruction's encoding. -/

/-- An `add`, `sub` or `cmp` immediate: "ADD (immediate)", `imm12` shifted left by
0 or 12 (`sh`). -/
def addImmOk (v : Nat) : Bool := v < 4096 || (v % 4096 == 0 && v / 4096 < 4096)

/-- A wide immediate of `mov`: "MOVZ", `imm16` with no shift (`hw` 0). Larger
values have other encodings, which no routine needs. -/
def movImmOk (v : Nat) : Bool := v < 65536

/-- Whether the `e`-bit pattern `x`, with `0 < x < 2 ^ e - 1`, is a run of ones,
rotated: its bits change exactly twice around the circle. -/
def rotatedRun (e x : Nat) : Bool :=
  (List.range e |>.filter fun i => x.testBit i != x.testBit ((i + 1) % e)).length == 2

/-- A logical immediate of a `bits`-bit instruction: DDI 0487 J1,
`DecodeBitMasks`: a pattern of 2, 4, 8, 16, 32 or 64 bits, not all ones or all
zeros and a rotated run of ones, repeated to fill the operand. -/
def logicalImmOk (bits v : Nat) : Bool :=
  v != 0 && v + 1 < 2 ^ bits &&
  [2, 4, 8, 16, 32, 64].any fun e =>
    e ≤ bits && rotatedRun e (v % 2 ^ e) &&
    (List.range (bits / e)).all fun i => (v / 2 ^ (i * e)) % 2 ^ e == v % 2 ^ e

/-- `log2` of an access's bytes, the one shift the register form allows besides
none: "LDR (register)", `S` selects a shift of `scale`; for a byte access it
selects a shift of 0, so only the plain form is needed. -/
def Width.scale : Width → Nat
  | .b => 0
  | .w => 2
  | .x => 3

/-- A symbol's offset the linker can resolve, as on x86-64 (the printer refuses
one an assembler might truncate). -/
def Rel.ok : Rel → Bool
  | .sym _ off => decide (-2 ^ 31 ≤ off ∧ off < 2 ^ 31)
  | .desc => true

/-- A memory operand of an access of width `wd`: "LDR (immediate)", unsigned offset,
`imm12` scaled by the access's size; "LDUR", `imm9` signed, which an assembler
selects for `ldr` when the other cannot encode the offset; "LDR (register)";
or a `:lo12:` offset, whose alignment the linker checks. -/
def MemOp.ok (wd : Width) (m : MemOp) : Bool :=
  match m.off with
  | .imm d =>
    let n : Int := wd.bytes
    decide ((0 ≤ d ∧ d % n = 0 ∧ d / n < 4096) ∨ (-256 ≤ d ∧ d < 256))
  | .lo12 t => t.ok
  | .reg _ sh => sh == 0 || sh == wd.scale

/-- A second operand, with `imm` the immediates the instruction can encode:
"ADD (shifted register)", `imm6`, a shift below the operand size. An operand
wider than the operand size is refused by `imm` too. -/
def Src.ok (sz : Size) (imm : Nat → Bool) : Src → Bool
  | .reg _ => true
  | .lsl _ n => n < sz.bits
  | .imm v => imm v && v < 2 ^ sz.bits

/-- Why no instruction encodes this one as the model describes it, or `none`. -/
def Instr.unencodable (i : Instr) : Option String :=
  let check (ok : Bool) (what : String) : Option String := if ok then none else some what
  match i with
  | .ldr wd t m => check (m.ok wd) s!"the memory operand {repr m} of a load of {repr wd} into {t.x}"
  | .str wd t m => check (m.ok wd) s!"the memory operand {repr m} of a store of {repr wd} from {t.x}"
  | .stpz n off =>
    check (decide (off % 8 = 0 ∧ -512 ≤ off ∧ off ≤ 504)) s!"the offset {off} of `stp` from {n.x}"
  | .add sz d _ s | .sub sz d _ s => check (s.ok sz addImmOk) s!"the operand {repr s} into {d.x}"
  | .cmp sz n s => check (s.ok sz addImmOk) s!"the operand {repr s} compared with {n.x}"
  | .and sz d _ s | .orr sz d _ s =>
    check (s.ok sz (logicalImmOk sz.bits)) s!"the logical operand {repr s} into {d.x}"
  | .tst sz n s =>
    check (s.ok sz (logicalImmOk sz.bits)) s!"the logical operand {repr s} tested with {n.x}"
  | .mov sz d s =>
    check (match s with | .lsl .. => false | _ => s.ok sz movImmOk) s!"the operand {repr s} into {d.x}"
  | .lsl sz d _ sh | .lsr sz d _ sh => check (sh < sz.bits) s!"the shift {sh} into {d.x}"
  | .adrp d t | .addLo12 d _ t => check t.ok s!"the address {repr t} into {d.x}"
  | _ => none

/-! ## What a restartable sequence may hold

`--fix-cortex-a53-843419`, which rustc passes on every AArch64 link, works around
Cortex-A53 erratum 843419: where an `adrp` lands at page offset 0xff8 or 0xffc and a load or
store follows, then within one more instruction a load or store through its register with
an unsigned immediate, the linker puts that last instruction in a stub out of line and
branches to it. In a sequence's body the stub is outside the range an abort covers, so a
preemption there does not abort it. Rather than recognize the pattern and its variants,
which depend on where the function lands, a body has no `adrp` at all, and nothing else a
linker writes: what it needs is formed in the setup, where a retry keeps the registers. An
`adrp` at the end of the setup could still reach into the sequence, so none is among its
last `moveReach` instructions. -/

/-- A memory operand whose offset the linker writes. -/
def MemOp.linked (m : MemOp) : Bool :=
  match m.off with
  | .lo12 _ => true
  | _ => false

/-- A memory operand whose immediate is not a multiple of the access's size. Through a register, an
address's alignment is what the proof of the routine says it is. -/
def MemOp.misaligned (wd : Width) (m : MemOp) : Bool :=
  match m.off with
  | .imm d => d % (wd.bytes : Int) != 0
  | _ => false

/-- Why an instruction may not be a step of a sequence's body, or `none`. -/
def Instr.notInBody : Instr → Option String
  | .adrp .. | .addLo12 .. | .addAreaHi .. | .addAreaLo .. =>
    some "an instruction the linker writes, which it may also move"
  | .movGot .. => some "`movGot`, one step of the model and two instructions"
  | .stpz .. => some "`stp`, two stores in one step of the model"
  | .ldrWord .. => some "a read of the word after its descriptor, which is for its setup"
  | .str wd _ m =>
    if m.linked then some "a store whose offset the linker writes"
    else if m.misaligned wd then some "a misaligned store, which may not be one access"
    else none
  | .ldr _ _ m => if m.linked then some "a load whose offset the linker writes" else none
  | _ => none

/-- Whether an instruction accesses memory another thread can change. `ldrWord` and `movGot` read
words no program writes, and `addAreaHi` and `addAreaLo` none (`TCB/AArch64/Isa.lean`). -/
def Instr.accessesMemory : Instr → Bool
  | .ldr .. | .str .. | .stpz .. => true
  | .add .. | .sub .. | .and .. | .orr .. | .mov .. | .cmp .. | .tst .. | .lsl .. | .lsr ..
  | .lslv .. | .lsrv .. | .mul .. | .umulh .. | .adrp .. | .addLo12 .. | .mrsTp _ | .ldrWord ..
  | .movGot .. | .addAreaHi .. | .addAreaLo .. => false

/-- Whether an instruction stores to memory. -/
def Instr.storesMemory : Instr → Bool
  | .str .. | .stpz .. => true
  | .ldr .. | .add .. | .sub .. | .and .. | .orr .. | .mov .. | .cmp .. | .tst .. | .lsl .. | .lsr ..
  | .lslv .. | .lsrv .. | .mul .. | .umulh .. | .adrp .. | .addLo12 .. | .mrsTp _ | .ldrWord ..
  | .movGot .. | .addAreaHi .. | .addAreaLo .. => false

/-- Whether an instruction names the descriptor by its label. -/
def Instr.namesDesc : Instr → Bool
  | .adrp _ .desc | .addLo12 _ _ .desc => true
  | .ldr _ _ { off := .lo12 .desc, .. } | .str _ _ { off := .lo12 .desc, .. } => true
  | _ => false

def printer : Printer isa where
  instr := Instr.lines
  branch := Cond.branch
  jump l := s!"b {l}"
  ret := ["ret"]
  call := "bl"
  unencodable := Instr.unencodable
  /- Every function starts on a 64-byte boundary, a cache line, on every core
  this runs on. So where a function's code lies relative to the fetch blocks
  and lines is fixed by the function alone: a change to other code cannot move
  it, and with it its performance. There is no front-end limit to check beyond
  this (docs/research/firestorm.md). -/
  funcAlign := [".p2align 6"]
  /- A function that neither calls nor touches the stack pointer or `x30` keeps
  the return address in `x30` from its first instruction to its last, which is
  what `.cfi_startproc` with no other directive says on this architecture (the
  CIE: the frame address is the stack pointer, the return address `x30`). -/
  unwindStart := [".cfi_startproc"]
  unwindEnd := [".cfi_endproc"]
  stackText := ["[sp", " sp", "x30", "w30", "bl "]
  /- The instruction form of the signature, so that the disassembly reads
  `brk #0x45e0`. -/
  rseqSig := some [s!".inst {hexLit rseqSignature}"]
  plainStore | .str .x .. => true | _ => false
  /- A branch to an external symbol has a 26-bit reach, and a linker may add a
  veneer for it; a conditional branch has 19 bits, 1 MiB, and no veneer. So a
  conditional branch to a slow path goes to a stub after the return, in the
  function's own section. -/
  stubExits := true
  /- Out-of-line code stays in the function's own section, so that a conditional
  branch to it in a naked function, whose section holds nothing else, is always
  in reach: the abort handler's jump back needs none, but a linker cannot move
  what a conditional branch names into another section. The assembler lays
  subsection 1 after subsection 0. -/
  coldOpen := [".subsection 1"]
  coldClose := [".subsection 0"]
  descWord | .ldrWord _ _ n => some n | _ => none
  bodyRefuses := Instr.notInBody
  accessesMemory := Instr.accessesMemory
  storesMemory := Instr.storesMemory
  namesDesc := Instr.namesDesc
  /- An abort returns to the arm alone, so that a retry keeps what the setup formed. -/
  armOneStore := true
  /- The erratum's last instruction is at most three after the `adrp`, and `movGot` starts with
  one. -/
  movesAfter | .adrp .. | .movGot .. => true | _ => false
  moveReach := 3

end VG.AArch64
