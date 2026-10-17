module

public import Asmalloc.TCB.X86_64.State

/-!
# x86-64 general-purpose instructions

**Trusted.** The semantics of the general-purpose (integer) instructions of
the x86-64 model in `TCB/X86_64/Isa.lean`: source operands, the ALU
operations and their flags, shifts (by an immediate, by CL, and BMI2's
`shlx` and `shrx`), rotates (including BMI2's `rorx`), BMI1's
`andn`, byte swaps, `mul`, `imul`, BMI2's `mulx` and ADX's `adcx` and `adox`.
(`cmovcc`, which reads the flags through a branch condition, is in
`Isa.lean`, with the conditions.)
-/

@[expose] public section

namespace VG.X86_64

inductive Src
  | reg (r : Reg)
  /-- A 32-bit immediate, sign-extended to 64 bits. -/
  | imm (v : BitVec 32)
  | mem (m : MemOp)
  deriving DecidableEq, Repr

inductive AluOp | add | adc | sub | sbb | and | or | xor | cmp | test
  deriving DecidableEq, Repr

/-- Shifts and rotates by an immediate count. -/
inductive ShiftOp | ror | shr | shl
  deriving DecidableEq, Repr

/-- Shifts by a count in a register: `shl`, `shr` and `sar` by CL. -/
inductive VarShiftOp | shl | shr | sar
  deriving DecidableEq, Repr

/-- BMI2's shifts by a count in a register that do not affect the flags
(`sarx` is not modelled). -/
inductive ShiftxOp | shlx | shrx
  deriving DecidableEq, Repr

/-- The shift of `VarShiftOp` a `ShiftxOp` performs. -/
def ShiftxOp.var : ShiftxOp → VarShiftOp
  | .shlx => .shl
  | .shrx => .shr

/-- Read a source operand. Immediates are sign-extended from 32 bits. -/
def readSrc (s : State) : Src → Option (BitVec 64)
  | .reg r => some (s.gpr r)
  | .imm v => some (v.signExtend 64)
  | .mem m => s.load64 (s.ea m)

/-- Read a 32-bit source operand: the low 32 bits of a register, the
immediate itself, or 4 bytes of memory. -/
def readSrc32 (s : State) : Src → Option (BitVec 32)
  | .reg r => some ((s.gpr r).setWidth 32)
  | .imm v => some v
  | .mem m => s.load32 (s.ea m)

/-- Flags after the result `r` of an arithmetic or logic operation with carry
`c` and signed overflow `o`: ZF and SF are computed from `r`. -/
def arithFlags {w : Nat} (s : State) (r : BitVec w) (c o : Bool) : State :=
  s.setFlags (some c) (some o) (some (r == 0)) (some r.msb)

/-- Signed overflow of `a + b (+ carry) = r`. -/
def addOverflow {w : Nat} (a b r : BitVec w) : Bool := a.msb == b.msb && r.msb != a.msb
/-- Signed overflow of `a - b (- borrow) = r`. -/
def subOverflow {w : Nat} (a b r : BitVec w) : Bool := a.msb != b.msb && r.msb != a.msb

/-- SDM Vol. 2: ADD, ADC, SUB, SBB, CMP set CF/OF/ZF/SF by the result; AND,
OR, XOR, TEST clear CF and OF and set ZF/SF by the result. -/
def execAlu (op : AluOp) (dst : Reg) (src : Src) (s : State) : Option State :=
  (readSrc s src).bind fun b =>
  let a := s.gpr dst
  match op with
  | .add => let r := a + b
    some ((arithFlags s r (2 ^ 64 ≤ a.toNat + b.toNat) (addOverflow a b r)).setReg dst r)
  | .adc => s.cf.map fun c =>
    let r := a + b + (BitVec.ofBool c).setWidth 64
    (arithFlags s r (2 ^ 64 ≤ a.toNat + b.toNat + c.toNat) (addOverflow a b r)).setReg dst r
  | .sub => let r := a - b
    some ((arithFlags s r (a.toNat < b.toNat) (subOverflow a b r)).setReg dst r)
  | .sbb => s.cf.map fun c =>
    let r := a - b - (BitVec.ofBool c).setWidth 64
    (arithFlags s r (a.toNat < b.toNat + c.toNat) (subOverflow a b r)).setReg dst r
  | .cmp => let r := a - b
    some (arithFlags s r (a.toNat < b.toNat) (subOverflow a b r))
  | .and => let r := a &&& b; some ((arithFlags s r false false).setReg dst r)
  | .or => let r := a ||| b; some ((arithFlags s r false false).setReg dst r)
  | .xor => let r := a ^^^ b; some ((arithFlags s r false false).setReg dst r)
  | .test => let r := a &&& b; some (arithFlags s r false false)

/-- The 32-bit forms of `execAlu`: the same operations on the low 32 bits,
with the flags computed from the 32-bit result. CMP and TEST write no
register; every other operation zero-extends its result (SDM Vol. 1 §3.4.1.1). -/
def execAlu32 (op : AluOp) (dst : Reg) (src : Src) (s : State) : Option State :=
  (readSrc32 s src).bind fun b =>
  let a := (s.gpr dst).setWidth 32
  match op with
  | .add => let r := a + b
    some ((arithFlags s r (2 ^ 32 ≤ a.toNat + b.toNat) (addOverflow a b r)).setReg32 dst r)
  | .adc => s.cf.map fun c =>
    let r := a + b + (BitVec.ofBool c).setWidth 32
    (arithFlags s r (2 ^ 32 ≤ a.toNat + b.toNat + c.toNat) (addOverflow a b r)).setReg32 dst r
  | .sub => let r := a - b
    some ((arithFlags s r (a.toNat < b.toNat) (subOverflow a b r)).setReg32 dst r)
  | .sbb => s.cf.map fun c =>
    let r := a - b - (BitVec.ofBool c).setWidth 32
    (arithFlags s r (a.toNat < b.toNat + c.toNat) (subOverflow a b r)).setReg32 dst r
  | .cmp => let r := a - b
    some (arithFlags s r (a.toNat < b.toNat) (subOverflow a b r))
  | .and => let r := a &&& b; some ((arithFlags s r false false).setReg32 dst r)
  | .or => let r := a ||| b; some ((arithFlags s r false false).setReg32 dst r)
  | .xor => let r := a ^^^ b; some ((arithFlags s r false false).setReg32 dst r)
  | .test => let r := a &&& b; some (arithFlags s r false false)

/-- SDM Vol. 2, "RCL/RCR/ROL/ROR" and "SAL/SAR/SHL/SHR", for a 32-bit
operand and a count `n` with `1 ≤ n ≤ 31` (so the masked count `n AND 1FH`
is `n`; other counts fault):

* ROR: the operand is rotated right by `n`; CF := MSB of the result; OF :=
  MSB XOR MSB−1 of the result if `n = 1`, otherwise undefined; SF and ZF are
  unaffected.
* SHR: the operand is shifted right (logically) by `n`; CF := the last bit
  shifted out (bit `n − 1` of the operand); OF := MSB of the original operand
  if `n = 1`, otherwise undefined; SF and ZF are set according to the result.
* SHL: the operand is shifted left by `n` (`DEST := DEST ∗ 2`, `n` times,
  each time after `CF := MSB(DEST)`); CF := the last bit shifted out (bit
  `32 − n` of the operand); OF := `MSB(DEST) XOR CF` (the MSB of the result
  XOR CF) if `n = 1`, otherwise undefined; SF and ZF are set according to
  the result.

(AF and PF are not modelled.) -/
def execShift32 (op : ShiftOp) (dst : Reg) (n : Nat) (s : State) : Option State :=
  if 1 ≤ n ∧ n ≤ 31 then
    let a := (s.gpr dst).setWidth 32
    match op with
    | .ror => let r := a.rotateRight n
      some ((s.setFlags (some r.msb) (if n = 1 then some (r.msb ^^ r.getMsbD 1) else none)
        s.zf s.sf).setReg32 dst r)
    | .shr => let r := a >>> n
      some ((s.setFlags (some (a.getLsbD (n - 1))) (if n = 1 then some a.msb else none)
        (some (r == 0)) (some r.msb)).setReg32 dst r)
    | .shl => let r := a <<< n
      let c := a.getLsbD (32 - n)
      some ((s.setFlags (some c) (if n = 1 then some (r.msb ^^ c) else none)
        (some (r == 0)) (some r.msb)).setReg32 dst r)
  else none

/-- SDM Vol. 2, "RORX—Rotate Right Logical Without Affecting Flags", for a
32-bit operand: `y := imm8 AND 1FH; DEST := (SRC >> y) | (SRC << (32 - y))`,
zero-extended (SDM Vol. 1 §3.4.1.1); "This instruction does not update any
flags." Only counts `1 ≤ n ≤ 31` are modelled (so the masked count is `n`);
other counts fault. -/
def execRorx32 (dst src : Reg) (n : Nat) (s : State) : Option State :=
  if 1 ≤ n ∧ n ≤ 31 then some (s.setReg32 dst (((s.gpr src).setWidth 32).rotateRight n))
  else none

/-- SDM Vol. 2, "ANDN—Logical AND NOT", for 32-bit operands: `DEST := (NOT
SRC1) bitwiseAND SRC2`, zero-extended (SDM Vol. 1 §3.4.1.1); "SF and ZF
flags are updated based on result. OF and CF flags are cleared. AF and PF
flags are undefined." (AF and PF are not modelled.) -/
def execAndn32 (dst src1 src2 : Reg) (s : State) : State :=
  let r := ~~~((s.gpr src1).setWidth 32) &&& (s.gpr src2).setWidth 32
  (arithFlags s r false false).setReg32 dst r

/-- SDM Vol. 2, "RORX—Rotate Right Logical Without Affecting Flags", for a
64-bit operand: `y := imm8 AND 3FH; DEST := (SRC >> y) | (SRC << (64 - y))`;
"This instruction does not update any flags." Only counts `1 ≤ n ≤ 63` are
modelled (so the masked count is `n`); other counts fault. -/
def execRorx (dst src : Reg) (n : Nat) (s : State) : Option State :=
  if 1 ≤ n ∧ n ≤ 63 then some (s.setReg dst ((s.gpr src).rotateRight n)) else none

/-- SDM Vol. 2, "ANDN—Logical AND NOT", for 64-bit operands: `DEST := (NOT
SRC1) bitwiseAND SRC2`; "SF and ZF flags are updated based on result. OF and
CF flags are cleared. AF and PF flags are undefined." (AF and PF are not
modelled.) -/
def execAndn (dst src1 src2 : Reg) (s : State) : State :=
  let r := ~~~(s.gpr src1) &&& s.gpr src2
  (arithFlags s r false false).setReg dst r

/-- SDM Vol. 2, "BSWAP": `DEST[7:0] := TEMP[31:24]; DEST[15:8] := TEMP[23:16];
DEST[23:16] := TEMP[15:8]; DEST[31:24] := TEMP[7:0]` for a 32-bit operand
(zero-extended, SDM Vol. 1 §3.4.1.1). No flags are affected. -/
def bswap32 (a : BitVec 32) : BitVec 32 :=
  a.extractLsb' 0 8 ++ a.extractLsb' 8 8 ++ a.extractLsb' 16 8 ++ a.extractLsb' 24 8

/-- The 64-bit form of `execShift32` (the same SDM pseudocode), for a count
`n` with `1 ≤ n ≤ 63` (so the masked count `n AND 3FH` is `n`; other counts
fault):

* ROR: the operand is rotated right by `n`; CF := MSB of the result; OF :=
  MSB XOR MSB−1 of the result if `n = 1`, otherwise undefined; SF and ZF are
  unaffected.
* SHR: the operand is shifted right (logically) by `n`; CF := the last bit
  shifted out (bit `n − 1` of the operand); OF := MSB of the original operand
  if `n = 1`, otherwise undefined; SF and ZF are set according to the result.
* SHL: the operand is shifted left by `n` (`DEST := DEST ∗ 2`, `n` times,
  each time after `CF := MSB(DEST)`); CF := the last bit shifted out (bit
  `64 − n` of the operand); OF := `MSB(DEST) XOR CF` (the MSB of the result
  XOR CF) if `n = 1`, otherwise undefined; SF and ZF are set according to
  the result.

(AF and PF are not modelled.) -/
def execShift (op : ShiftOp) (dst : Reg) (n : Nat) (s : State) : Option State :=
  if 1 ≤ n ∧ n ≤ 63 then
    let a := s.gpr dst
    match op with
    | .ror => let r := a.rotateRight n
      some ((s.setFlags (some r.msb) (if n = 1 then some (r.msb ^^ r.getMsbD 1) else none)
        s.zf s.sf).setReg dst r)
    | .shr => let r := a >>> n
      some ((s.setFlags (some (a.getLsbD (n - 1))) (if n = 1 then some a.msb else none)
        (some (r == 0)) (some r.msb)).setReg dst r)
    | .shl => let r := a <<< n
      let c := a.getLsbD (64 - n)
      some ((s.setFlags (some c) (if n = 1 then some (r.msb ^^ c) else none)
        (some (r == 0)) (some r.msb)).setReg dst r)
  else none

/-- A 64-bit operand `a` shifted by `n` bits, `n < 64`: SDM Vol. 2,
"SAL/SAR/SHL/SHR", `tempCOUNT` times `DEST := DEST ∗ 2` (SHL), `DEST := DEST
/ 2; (* Unsigned divide *)` (SHR) or `DEST := DEST / 2; (* Signed divide,
rounding toward negative infinity *)` (SAR): a logical shift left, a
logical shift right, and an arithmetic shift right ("The SAR instruction
sets or clears the most significant bit to correspond to the sign (most
significant bit) of the original value in the destination operand"). BMI2's
SHLX and SHRX ("SARX/SHLX/SHRX—Shift Without Affecting Flags") shift as SHL
and SHR. -/
def VarShiftOp.eval : VarShiftOp → BitVec 64 → Nat → BitVec 64
  | .shl, a, n => a <<< n
  | .shr, a, n => a >>> n
  | .sar, a, n => a.sshiftRight n

/-- SDM Vol. 2, "SAL/SAR/SHL/SHR", the forms `SHL r/m64, CL` (`REX.W + D3
/4`), `SHR r/m64, CL` (`REX.W + D3 /5`) and `SAR r/m64, CL` (`REX.W + D3
/7`), with a register destination. The count is CL masked to 6 bits: `IF
64-Bit Mode and using REX.W THEN countMASK := 3FH`, `tempCOUNT := (COUNT AND
countMASK)`; the low 6 bits of CL are those of RCX, so the count `n` is
`RCX mod 64`. Then (see `VarShiftOp.eval` for the result):

* If `n = 0`, nothing changes: the loop does not run, so the destination
  keeps its value (written back unchanged: `VarShiftOp.eval a 0 = a`), and
  "If the count is 0, the flags are not affected" (`ELSE IF (COUNT AND
  countMASK) = 0 THEN All flags unchanged`).
* Otherwise CF := the last bit shifted out ("The CF flag contains the value
  of the last bit shifted out of the destination operand"; it is undefined
  only for counts of at least the operand size, 64, which the mask rules
  out): bit `64 − n` of the operand for SHL (each step does `CF :=
  MSB(DEST)`), and bit `n − 1` for SHR and SAR (`CF := LSB(DEST)`). OF, "if
  (COUNT AND countMASK) = 1": `OF := MSB(DEST) XOR CF` (of the result) for
  SHL, `OF := MSB(tempDEST)` (of the original operand) for SHR, and `OF :=
  0` for SAR; for other counts, undefined. "The SF, ZF, and PF flags are set
  according to the result." (AF, undefined for a non-zero count, and PF are
  not modelled.)

The count is read before the destination is written, so `rcx` may be the
destination. -/
def execShiftCl (op : VarShiftOp) (dst : Reg) (s : State) : State :=
  let n := (s.gpr .rcx).toNat % 64
  let a := s.gpr dst
  let r := op.eval a n
  let c := match op with
    | .shl => a.getLsbD (64 - n)
    | .shr | .sar => a.getLsbD (n - 1)
  let o := if n = 1 then
      some (match op with | .shl => r.msb ^^ c | .shr => a.msb | .sar => false)
    else none
  (s.setFlags (if n = 0 then s.cf else some c) (if n = 0 then s.of else o)
    (if n = 0 then s.zf else some (r == 0)) (if n = 0 then s.sf else some r.msb)).setReg dst r

/-- SDM Vol. 2, "SARX/SHLX/SHRX—Shift Without Affecting Flags", for 64-bit
operands with a register source: `SHLX r64a, r/m64, r64b`
(`VEX.LZ.66.0F38.W1 F7 /r`) and `SHRX r64a, r/m64, r64b`
(`VEX.LZ.F2.0F38.W1 F7 /r`): `src` (the second operand) shifted by `cnt`
(the third, `r64b`, in VEX.vvvv) masked to 6 bits (`IF VEX.W1 and CS.L = 1 THEN countMASK := 3FH`), into
`dst` ("Flags Affected: None"). The memory form of `src` is not modelled.
The count is masked, not checked: every count is modelled. -/
def execShiftx (op : ShiftxOp) (dst src cnt : Reg) (s : State) : State :=
  s.setReg dst (op.var.eval (s.gpr src) ((s.gpr cnt).toNat % 64))

/-- SDM Vol. 2, "BSWAP", for a 64-bit operand: `DEST[7:0] := TEMP[63:56];
DEST[15:8] := TEMP[55:48]; DEST[23:16] := TEMP[47:40]; DEST[31:24] :=
TEMP[39:32]; DEST[39:32] := TEMP[31:24]; DEST[47:40] := TEMP[23:16];
DEST[55:48] := TEMP[15:8]; DEST[63:56] := TEMP[7:0]`. No flags are affected. -/
def bswap64 (a : BitVec 64) : BitVec 64 :=
  a.extractLsb' 0 8 ++ a.extractLsb' 8 8 ++ a.extractLsb' 16 8 ++ a.extractLsb' 24 8 ++
    a.extractLsb' 32 8 ++ a.extractLsb' 40 8 ++ a.extractLsb' 48 8 ++ a.extractLsb' 56 8

/-- SDM Vol. 2, "MUL—Unsigned Multiply", for a 64-bit operand: `RDX:RAX :=
RAX ∗ SRC` (the 128-bit product of the unsigned operands, its high half in
RDX and its low half in RAX). "The OF and CF flags are set to 0 if the upper
half of the result is 0; otherwise, they are set to 1. The SF, ZF, AF, and PF
flags are undefined." (AF and PF are not modelled.) -/
def execMul (src : Reg) (s : State) : State :=
  let p := (s.gpr .rax).toNat * (s.gpr src).toNat
  let hi : BitVec 64 := BitVec.ofNat 64 (p / 2 ^ 64)
  ((s.setFlags (some (hi != 0)) (some (hi != 0)) none none).setReg .rax (BitVec.ofNat 64 p)).setReg
    .rdx hi

/-- SDM Vol. 2, "IMUL—Signed Multiply", the two-operand form `IMUL r64, r/m64`
(`REX.W + 0F AF /r`) with a register source: `TMP_XP := DEST ∗ SRC; DEST :=
TruncateToOperandSize(TMP_XP); IF SignExtend(DEST) ≠ TMP_XP THEN CF := 1;
OF := 1; ELSE CF := 0; OF := 0; FI;` (signed operands). "The SF, ZF, AF, and
PF flags are undefined." (AF and PF are not modelled.) The low 64 bits of the
product are those of the unsigned product as well. -/
def execImul (dst src : Reg) (s : State) : State :=
  let t := (s.gpr dst).toInt * (s.gpr src).toInt
  let r : BitVec 64 := BitVec.ofInt 64 t
  let o := r.toInt != t
  (s.setFlags (some o) (some o) none none).setReg dst r

/-- SDM Vol. 2, "MULX—Unsigned Multiply Without Affecting Flags", for 64-bit
operands (`VEX.LZ.F2.0F38.W1 F6 /r`, `MULX r64a, r64b, r/m64`): `SRC1 :=
RDX; SRC2 := r/m64; TEMP := SRC1 * SRC2; DEST2 := TEMP[OperandSize-1:0];
DEST1 := TEMP[2*OperandSize-1:OperandSize]`, where `DEST1` is the first
operand (`hi`) and `DEST2` the second (`lo`); "Flags Affected: None." The
high half is written last: "If the first and second operand are identical,
it will contain the high half of the multiplication result." An immediate
source does not exist: it faults. -/
def execMulx (hi lo : Reg) (src : Src) (s : State) : Option State :=
  match src with
  | .imm _ => none
  | _ => (readSrc s src).map fun b =>
    let p := (s.gpr .rdx).toNat * b.toNat
    (s.setReg lo (BitVec.ofNat 64 p)).setReg hi (BitVec.ofNat 64 (p / 2 ^ 64))

/-- SDM Vol. 2, "ADCX—Unsigned Integer Addition of Two Operands With Carry
Flag", for 64-bit operands (`66 REX.w 0F 38 F6 /r`, `ADCX r64, r/m64`):
`CF:DEST[63:0] := DEST[63:0] + SRC[63:0] + CF`; "CF is updated based on
result. OF, SF, ZF, AF and PF flags are unmodified." (AF and PF are not
modelled.) It faults if CF is undefined, as `adc` does, and on an immediate
source, which does not exist. -/
def execAdcx (dst : Reg) (src : Src) (s : State) : Option State :=
  match src with
  | .imm _ => none
  | _ => (readSrc s src).bind fun b => s.cf.map fun c =>
    let a := s.gpr dst
    let r := a + b + (BitVec.ofBool c).setWidth 64
    (s.setFlags (some (2 ^ 64 ≤ a.toNat + b.toNat + c.toNat)) s.of s.zf s.sf).setReg dst r

/-- SDM Vol. 2, "ADOX—Unsigned Integer Addition of Two Operands With
Overflow Flag", for 64-bit operands (`F3 REX.w 0F 38 F6 /r`, `ADOX r64,
r/m64`): `OF:DEST[63:0] := DEST[63:0] + SRC[63:0] + OF`; "OF is updated
based on result. CF, SF, ZF, AF and PF flags are unmodified." (AF and PF are
not modelled.) It faults if OF is undefined, and on an immediate source,
which does not exist. -/
def execAdox (dst : Reg) (src : Src) (s : State) : Option State :=
  match src with
  | .imm _ => none
  | _ => (readSrc s src).bind fun b => s.of.map fun o =>
    let a := s.gpr dst
    let r := a + b + (BitVec.ofBool o).setWidth 64
    (s.setFlags s.cf (some (2 ^ 64 ≤ a.toNat + b.toNat + o.toNat)) s.zf s.sf).setReg dst r

end VG.X86_64
