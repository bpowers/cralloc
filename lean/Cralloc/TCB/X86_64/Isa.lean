module

public import Cralloc.TCB.Code
public import Cralloc.TCB.X86_64.Gpr

/-!
# x86-64 machine model

**Trusted.** A model of the subset of x86-64 used by our implementations.
Each instruction's semantics here must agree with the Intel SDM; when adding
an instruction, cite the SDM pseudocode it transcribes.

The model is split: `State.lean` holds the registers, the state and memory
operands; `Gpr.lean` the semantics of the general-purpose (integer)
instructions; and this file the instructions themselves, the CPU features
they require, and `exec`, which dispatches to those semantics.

Modelling choices:
* Only general-purpose registers are modelled. No modelled instruction reads
  or writes a vector register, the x87 state or MXCSR.
* Only 64-bit and 32-bit operand sizes are modelled, plus byte loads
  (`movzx`, zero-extending) and byte stores. SDM Vol. 1 §3.4.1.1: "32-bit
  operands generate a 32-bit result, zero-extended to a 64-bit result in the
  destination general-purpose register."
* Only CF, ZF, SF and OF are modelled. Each is an `Option Bool`; `none` means
  "undefined" (as the SDM specifies for some instructions). Evaluating a
  branch on an undefined flag faults, so verified code never depends on one.
  PF and AF are not modelled, and no modelled instruction reads them.
* Memory accesses must lie within the state's permitted regions: loads within
  `rd ++ wr`, stores within `wr`; otherwise the instruction faults.
* A memory operand may name the FS segment (`MemOp.seg`), which adds the
  segment's base, `State.fsBase`, to its address (`State.ea`). Thread-local
  memory is therefore one more region, at an address relative to `fsBase`;
  the model knows nothing else about it. (`Sig.contract` grants no such
  region yet: a contract that lets code access thread-local memory needs a
  calling convention that adds one, as `Abi.withConsts` adds the tables.)
  GS, and the instructions that read
  or write the segment bases (`rdfsbase`, `wrfsbase`, `swapgs`), are not
  modelled.
* Calls (`call`) and returns (`ret`) are near and direct (SDM Vol. 2, "CALL",
  "RET"). The return addresses are the next of the state's `unknowns`,
  which nothing constrains (see `TCB/Code.lean`).
* A `static` the code reads (`Artifact.consts`) is at the address
  `State.syms name`, where `name` is its name, which `leaSym` (`lea` of a
  RIP-relative operand, resolved by the linker) puts in a register, or that
  address plus a constant; `movGot` reads the address from a word holding it,
  as from a slot in the global offset table, for a symbol of another shared
  object (glibc's `__rseq_offset`), and `movSym` reads the 8 bytes at the symbol's address
  directly, as an executable may for such a symbol (a copy relocation puts
  the variable in the executable). No
  instruction changes `syms`: a static's address is fixed for the run of
  the program.
* A compile-time constant the code names (a `const` operand: an immediate,
  `Src.immP`, or an offset of a symbol, `leaSym`) has the value
  `State.params name`, fixed for the run. The compiler writes the constant's
  value into the instruction's text; the contract says what it may be.
* `push` and `pop` (of 64-bit registers other than `rsp`) only occur as the
  push and pop of a frame (see `push`), as a sequence of them. A caller
  passes arguments on the stack by pushing them, the last first, just
  before the call: on the callee's entry the first is then at `[rsp + 8]`,
  above the return address.
* A frame may instead allocate a buffer of `bytes` bytes on the stack
  (`alloc`, `lea rsp, [rsp - bytes]`), released by its pop (`free`, `lea
  rsp, [rsp + bytes]`); `lea` writes only `rsp`, so neither changes the
  flags or memory. `bytes` is positive, a multiple of 8 and less than 4096:
  less than a page, so that the allocation does not move `rsp` past a guard
  page below the stack.
-/

@[expose] public section

namespace VG.X86_64

/-- Branch conditions (`jcc` suffixes), also those of `cmovcc`. -/
inductive Cond
  /-- `je`: ZF = 1 -/
  | e
  /-- `jne`: ZF = 0 -/
  | ne
  /-- `jb`: CF = 1 -/
  | b
  /-- `jae`: CF = 0 -/
  | ae
  deriving DecidableEq, Repr

/-- Whether a condition holds (SDM Vol. 2, "Jcc" and "CMOVcc", the
description of each condition: E, "equal (ZF=1)"; NE, "not equal (ZF=0)";
B, "below (CF=1)"; AE, "above or equal (CF=0)"); `none`, and so a fault, if
the flag it reads is undefined. -/
def eval : Cond → State → Option Bool
  | .e, s => s.zf
  | .ne, s => s.zf.map (!·)
  | .b, s => s.cf
  | .ae, s => s.cf.map (!·)

/-- SDM Vol. 2, "CMOVcc—Conditional Move", for 64-bit operands (`REX.W + 0F
4x /r`, `CMOVcc r64, r/m64`): `temp := SRC; IF condition TRUE THEN DEST :=
temp; FI;` (its `ELSE IF (OperandSize = 32 and IA-32e mode active) THEN
DEST[63:32] := 0` is for 32-bit operands only); "Flags Affected: None." The
source is read whether or not the condition holds, so a memory source
outside the readable regions faults either way. An immediate source does not
exist, and a condition on an undefined flag faults, as a branch on it does. -/
def execCmov (cc : Cond) (dst : Reg) (src : Src) (s : State) : Option State :=
  match src with
  | .imm _ | .immP _ => none
  | _ => (readSrc s src).bind fun temp => (eval cc s).map fun c =>
    if c then s.setReg dst temp else s

inductive Instr
  /-- `mov dst, src` (64-bit) -/
  | mov (dst : Reg) (src : Src)
  /-- `mov QWORD PTR [dst], src` -/
  | store (dst : MemOp) (src : Reg)
  /-- Two-operand ALU instruction `op dst, src` (64-bit). With `test` and an
  immediate, `TEST r/m64, imm32` (`REX.W + F7 /0 id`), the immediate
  sign-extended. -/
  | alu (op : AluOp) (dst : Reg) (src : Src)
  /-- `inc r64` (`REX.W + FF /0`): `dst := dst + 1`, CF unchanged (`execInc`). -/
  | inc (dst : Reg)
  /-- `dec r64` (`REX.W + FF /1`): `dst := dst - 1`, CF unchanged (`execDec`). -/
  | dec (dst : Reg)
  /-- `mov r32, src` (32-bit; `DWORD PTR` for a memory source). An immediate
  is used as is. The result is zero-extended into the 64-bit register. -/
  | mov32 (dst : Reg) (src : Src)
  /-- `mov DWORD PTR [dst], r32` -/
  | store32 (dst : MemOp) (src : Reg)
  /-- Two-operand ALU instruction `op r32, src` (32-bit; `DWORD PTR` for a
  memory source). An immediate is used as is: with `test`, `TEST r/m32,
  imm32` (`F7 /0 id`). -/
  | alu32 (op : AluOp) (dst : Reg) (src : Src)
  /-- `op r32, count` (32-bit) with an immediate count. Only counts `1 ≤ count ≤ 31`
  are modelled; any other count faults. -/
  | shift32 (op : ShiftOp) (dst : Reg) (count : Nat)
  /-- `bswap r32` -/
  | bswap32 (dst : Reg)
  /-- `rorx r32, r32, count` (`VEX.LZ.F2.0F3A.W0 F0 /r ib`, BMI2): `src`
  rotated right, into `dst`, without affecting the flags. Only counts
  `1 ≤ count ≤ 31` are modelled; any other count faults. -/
  | rorx32 (dst src : Reg) (count : Nat)
  /-- `andn r32, r32, r32` (`VEX.LZ.0F38.W0 F2 /r`, BMI1): `dst := ¬src1 ∧ src2`. -/
  | andn32 (dst src1 src2 : Reg)
  /-- `rorx r64, r64, count` (`VEX.LZ.F2.0F3A.W1 F0 /r ib`, BMI2): `src`
  rotated right, into `dst`, without affecting the flags. Only counts
  `1 ≤ count ≤ 63` are modelled; any other count faults. -/
  | rorx (dst src : Reg) (count : Nat)
  /-- `andn r64, r64, r64` (`VEX.LZ.0F38.W1 F2 /r`, BMI1): `dst := ¬src1 ∧ src2`. -/
  | andn (dst src1 src2 : Reg)
  /-- `movzx r32, BYTE PTR [src]`: the byte, zero-extended into the 64-bit register. -/
  | movzx8 (dst : Reg) (src : MemOp)
  /-- `mov BYTE PTR [dst], r8`: the low byte of `src`. -/
  | store8 (dst : MemOp) (src : Reg)
  /-- `movzx r32, r16` (`0F B7 /r` with a register operand): the low 16 bits
  of `src`, zero-extended into the 64-bit register. -/
  | movzx16 (dst src : Reg)
  /-- `cmp r8, BYTE PTR [src]` (`3A /r`): the low byte of `dst` compared
  with the byte at `src`, setting the flags as `cmp` does on 8-bit operands
  (`execCmp8`). -/
  | cmp8 (dst : Reg) (src : MemOp)
  /-- `bswap r64` -/
  | bswap (dst : Reg)
  /-- `op r64, count` (64-bit) with an immediate count. Only counts `1 ≤ count ≤ 63`
  are modelled; any other count faults. -/
  | shift (op : ShiftOp) (dst : Reg) (count : Nat)
  /-- `op r64, cl` (64-bit; `REX.W + D3 /4`, `/5`, `/7`): `dst` shifted by
  `cl` masked to 6 bits (`execShiftCl`). -/
  | shiftCl (op : VarShiftOp) (dst : Reg)
  /-- `shlx` or `shrx r64, r64, r64` (`VEX.LZ.66.0F38.W1 F7 /r`,
  `VEX.LZ.F2.0F38.W1 F7 /r`, BMI2): `src` shifted by `cnt` masked to 6
  bits, into `dst`, without affecting the flags (`execShiftx`). -/
  | shiftx (op : ShiftxOp) (dst src cnt : Reg)
  /-- `movabs r64, imm64`: `MOV r64, imm64` (REX.W + B8+rd io), a full 64-bit
  immediate. -/
  | movImm64 (dst : Reg) (v : BitVec 64)
  /-- `lea r64, [rip + name + off + offP]` (REX.W + 8D /r, with a
  RIP-relative memory operand): the address of the symbol `name`,
  `State.syms name`, plus `off`, plus the compile-time constant `offP` if
  given (`State.params`). -/
  | leaSym (dst : Reg) (name : String) (off : Int := 0) (offP : Option String := none)
  /-- `mov r64, QWORD PTR [rip + word]` (REX.W + 8B /r, with a RIP-relative
  memory operand), where `word` holds the address of the symbol `name`: a
  global offset table slot of the routine's own, which the printer emits
  beside it (`gotLines`). -/
  | movGot (dst : Reg) (name : String)
  /-- `mov r64, QWORD PTR [rip + name]` (REX.W + 8B /r, with a RIP-relative
  memory operand): the 8 bytes at the address of the symbol `name`. -/
  | movSym (dst : Reg) (name : String)
  /-- `lea r64, [rip + 3f]` (REX.W + 8D /r, with a RIP-relative memory
  operand): the address of the routine's restartable-sequence descriptor,
  `State.csDesc`, which the printer labels `3` (`Printer.rseqLines`). -/
  | leaRseqCs (dst : Reg)
  /-- `lea r64, [base + index * scale + disp]` (REX.W + 8D /r): the operand's
  offset (`State.offset`), computed without accessing memory. The operand
  names no segment (the printer refuses one). -/
  | lea (dst : Reg) (src : MemOp)
  /-- `mul r64` (REX.W + F7 /4): the unsigned product `RDX:RAX := RAX * r64`. -/
  | mul (src : Reg)
  /-- `imul r64, r64` (`REX.W 0F AF /r`): the low 64 bits of the product
  `dst := dst * src`, the same for signed and unsigned operands. -/
  | imul (dst src : Reg)
  /-- `mulx hi, lo, src` (`VEX.LZ.F2.0F38.W1 F6 /r`, BMI2): the unsigned
  product `hi:lo := RDX * src`, without affecting the flags. `src` is a
  register or memory. -/
  | mulx (hi lo : Reg) (src : Src)
  /-- `adcx r64, src` (`66 REX.w 0F 38 F6 /r`, ADX): `CF:dst := dst + src +
  CF`, the other flags unchanged. `src` is a register or memory. -/
  | adcx (dst : Reg) (src : Src)
  /-- `adox r64, src` (`F3 REX.w 0F 38 F6 /r`, ADX): `OF:dst := dst + src +
  OF`, the other flags unchanged. `src` is a register or memory. -/
  | adox (dst : Reg) (src : Src)
  /-- `cmovcc r64, src` (`REX.W + 0F 4x /r`: `0F 42` CMOVB, `0F 43` CMOVAE,
  `0F 44` CMOVE, `0F 45` CMOVNE): `dst := src` if the condition `cc`
  holds, the flags unchanged. `src` is a register or memory, read whether or
  not the condition holds. -/
  | cmov (cc : Cond) (dst : Reg) (src : Src)
  /-- `push r64` (50+rd) for each `r` of `rs`, in order: the push of a frame
  (see `push`); `rs` must not be empty or contain `rsp` -/
  | push (rs : List Reg)
  /-- `pop r64` (58+rd), `k` times: the pop of a frame of `8 * k` bytes (see
  `pop`); `k > 0`, and `r` is not `rsp` -/
  | pop (r : Reg) (k : Nat)
  /-- `lea rsp, [rsp - bytes]` (REX.W + 8D /r): the push of a frame of
  `bytes` bytes that it does not write (see `push`); `0 < bytes < 4096`, a
  multiple of 8 -/
  | alloc (bytes : Nat)
  /-- `lea rsp, [rsp + bytes]` (REX.W + 8D /r): the pop of a frame of `bytes`
  bytes (see `pop`) -/
  | free (bytes : Nat)
  deriving DecidableEq, Repr

/-- The CPU features an instruction needs beyond the x86-64 baseline
(x86-64-v1: System V AMD64 psABI, "Micro-Architecture Levels"), named as
Rust's target features. SDM Vol. 2, the "CPUID Feature Flag" column of each
instruction's opcode table: BMI2 for RORX (`VEX.LZ.F2.0F3A.W0 F0 /r ib`,
`VEX.LZ.F2.0F3A.W1 F0 /r ib`), MULX (`VEX.LZ.F2.0F38.W1 F6 /r`), SHLX and
SHRX (`VEX.LZ.66.0F38.W1 F7 /r`, `VEX.LZ.F2.0F38.W1 F7 /r`); ADX for ADCX
and ADOX (`66 REX.w 0F 38 F6 /r`, `F3 REX.w 0F 38 F6 /r`); and BMI1 for
ANDN (`VEX.LZ.0F38.W0 F2 /r`, `VEX.LZ.0F38.W1 F2 /r`). CMOVcc (`REX.W + 0F
4x /r`), whose "CPUID Feature Flag" CMOV (CPUID.01H:EDX.CMOV[bit 15]) is
part of x86-64-v1, is in the baseline, as are the other instructions. -/
def Instr.requires : Instr → List String
  | .rorx32 .. | .rorx .. | .mulx .. | .shiftx .. => ["bmi2"]
  | .adcx .. | .adox .. => ["adx"]
  | .andn32 .. | .andn .. => ["bmi1"]
  | _ => []

/-- SDM Vol. 2, "CMP—Compare Two Operands", `CMP r8, r/m8` (`3A /r`):
`temp := SRC1 − SignExtend(SRC2); ModifyStatusFlags;` "The CF, OF, SF, ZF,
AF, and PF flags are set according to the result" of the 8-bit
subtraction `a − b`, as for the wider forms (`execAlu`'s `cmp`): CF is the
borrow, OF the signed overflow. No register is written. (AF and PF are not
modelled.) -/
def execCmp8 (a b : BitVec 8) (s : State) : State :=
  arithFlags s (a - b) (a.toNat < b.toNat) (subOverflow a b (a - b))

/-- Semantics of an instruction. The byte forms: SDM Vol. 2, "MOVZX":
`DEST := ZeroExtend(SRC)` (with a 32-bit destination, zero-extended to 64
bits, SDM Vol. 1 §3.4.1.1), and "MOV": `DEST := SRC`, where the source of a
byte store is the register's low byte (AL, CL, DL, BL, SPL, BPL, SIL, DIL,
R8B–R15B; SDM Vol. 1 §3.4.1.1). Neither affects the flags. The 16-bit zero extension: SDM Vol. 2,
"MOVZX", `MOVZX r32, r/m16` (`0F B7 /r`), `DEST := ZeroExtend(SRC)` with a
16-bit source register's low 16 bits, zero-extended to 64 bits (SDM Vol. 1
§3.4.1.1); it affects no flags. -/
def exec : Instr → State → Option State
  | .mov d src, s => (readSrc s src).map fun v => s.setReg d v
  | .store m r, s => s.store64 (s.ea m) (s.gpr r)
  | .alu op d src, s => execAlu op d src s
  | .inc d, s => some (execInc d s)
  | .dec d, s => some (execDec d s)
  | .mov32 d src, s => (readSrc32 s src).map fun v => s.setReg32 d v
  | .store32 m r, s => s.store32 (s.ea m) ((s.gpr r).setWidth 32)
  | .alu32 op d src, s => execAlu32 op d src s
  | .shift32 op d n, s => execShift32 op d n s
  | .bswap32 d, s => some (s.setReg32 d (bswap32 ((s.gpr d).setWidth 32)))
  | .rorx32 d r n, s => execRorx32 d r n s
  | .andn32 d a b, s => some (execAndn32 d a b s)
  | .rorx d r n, s => execRorx d r n s
  | .andn d a b, s => some (execAndn d a b s)
  | .movzx8 d m, s => (s.load8 (s.ea m)).map fun v => s.setReg d (v.setWidth 64)
  | .store8 m r, s => s.store8 (s.ea m) ((s.gpr r).setWidth 8)
  | .movzx16 d r, s => some (s.setReg d ((s.gpr r).setWidth 16 |>.setWidth 64))
  | .cmp8 d m, s => (s.load8 (s.ea m)).map fun b => execCmp8 ((s.gpr d).setWidth 8) b s
  | .bswap d, s => some (s.setReg d (bswap64 (s.gpr d)))
  | .shift op d n, s => execShift op d n s
  | .shiftCl op d, s => some (execShiftCl op d s)
  | .shiftx op d r c, s => some (execShiftx op d r c s)
  -- SDM Vol. 2, "MOV": `DEST := SRC`; no flags are affected.
  | .movImm64 d v, s => some (s.setReg d v)
  -- `leaSym`: SDM Vol. 2, "LEA—Load Effective Address", with a 64-bit operand
  -- size and address size: `DEST := EffectiveAddress(SRC)`; "Flags Affected:
  -- None"; it accesses no memory. Its source is RIP-relative (SDM Vol. 2
  -- §2.2.1.6, "RIP-Relative Addressing": ModR/M `mod = 00`, `r/m = 101` in
  -- 64-bit mode is `RIP + disp32`, Table 2-7: "An effective address is
  -- formed by adding displacement to the 64-bit RIP of the next
  -- instruction"), its `disp32` written by the linker so that the sum is the
  -- static's address `S` plus the constant `off` of the operand (`S + A - P`,
  -- with the addend `A = off - 4`: System V AMD64 psABI §4.4, "Relocation
  -- Types", `R_X86_64_PC32`): `DEST := S + off`, plus the value of the
  -- compile-time constant `offP`, which the compiler writes into the operand
  -- as a number. A sum beyond `disp32`'s range (±2 GB from the instruction)
  -- does not link.
  | .leaSym d name off offP, s =>
    some (s.setReg d (s.syms name + BitVec.ofInt 64 off + ((offP.map s.params).getD 0)))
  -- `movGot`: SDM Vol. 2, "MOV—Move", `MOV r64, r/m64` (`REX.W + 8B /r`),
  -- `DEST := SRC`, with a RIP-relative source (§2.2.1.6) whose `disp32` the
  -- assembler writes so that it addresses a word the printer emits beside
  -- the instruction (`gotLines`), in relocated read-only data, holding the
  -- address of `name` by an absolute relocation (`R_X86_64_64`: System V
  -- AMD64 psABI §4.4). The linker or, for a symbol another shared object
  -- defines, the dynamic linker fills the word with the symbol's address `S`
  -- before any code of the program runs, as it fills a global offset table
  -- slot, and with RELRO maps it read-only, so the load reads `S` and never
  -- faults: `DEST := S`. No flags are affected.
  | .movGot d name, s => some (s.setReg d (s.syms name))
  -- `movSym`: "MOV" as for `movGot`, its RIP-relative source's `disp32`
  -- written by the linker so that it addresses the symbol itself
  -- (`R_X86_64_PC32`, as for `leaSym`): `DEST := SRC`, the 8 bytes at `S`,
  -- a load that faults outside the permitted regions as any other does.
  -- In an executable, a symbol another shared object defines is copied
  -- into the executable (a copy relocation) before any code of the program
  -- runs, and `S` is the copy's address. No flags are affected.
  | .movSym d name, s => (s.load64 (s.syms name)).map (s.setReg d ·)
  -- `leaRseqCs`: "LEA" as for `leaSym`, of the local label the descriptor
  -- has (`Printer.rseqLines`), which the assembler resolves: `DEST :=` its
  -- address.
  | .leaRseqCs d, s => some (s.setReg d s.csDesc)
  -- `lea`: SDM Vol. 2, "LEA—Load Effective Address", `LEA r64, m` (`REX.W + 8D
  -- /r`), with a 64-bit operand size and address size: "Computes the
  -- effective address of the second operand (the source operand) and stores
  -- it in the first operand", where "the source operand is a memory address
  -- (offset part)"; `IF OperandSize = 64 and AddressSize = 64 THEN DEST :=
  -- EffectiveAddress(SRC)`, its offset `base + index * scale + disp` modulo
  -- 2⁶⁴ (`State.offset`); "Flags Affected: None"; it accesses no memory.
  | .lea d m, s => some (s.setReg d (s.offset m))
  | .mul r, s => some (execMul r s)
  | .imul d r, s => some (execImul d r s)
  | .mulx hi lo src, s => execMulx hi lo src s
  | .adcx d src, s => execAdcx d src s
  | .adox d src, s => execAdox d src s
  | .cmov cc d src, s => execCmov cc d src s
  -- Only the push and pop of a frame (`push`, `pop`).
  | .push _, _ | .pop .., _ | .alloc _, _ | .free _, _ => none

/-- SDM Vol. 2, "CALL", near call: `RSP := RSP − 8; Memory[RSP] := RIP`
(`Push(RIP)`, where `RIP` is the address of the next instruction), then the
jump. No flags are affected. The return address is the next of the state's. -/
def call (s : State) : Option State :=
  let sp := s.gpr .rsp - 8
  some { s.setReg .rsp sp with
    mem := s.mem.writeW sp (s.unknowns 0)
    unknowns := fun n => s.unknowns (n + 1) }

/-- SDM Vol. 2, "RET", near return: `RIP := Pop()`, i.e. `RIP :=
Memory[RSP]; RSP := RSP + 8`. No flags are affected. It returns after the
call instruction if `RSP` and the return address at `[RSP]` are those the
call left (`s₁`); otherwise the model faults. -/
def ret (s₁ s₂ : State) : Option State :=
  if s₂.gpr .rsp = s₁.gpr .rsp ∧ s₂.mem.readW (s₂.gpr .rsp) 64 = s₁.mem.readW (s₁.gpr .rsp) 64 then
    some (s₂.setReg .rsp (s₂.gpr .rsp + 8))
  else none

/-- `push r` for each of `rs`, in order, where `r ≠ rsp`: SDM Vol. 2,
"PUSH—Push Word, Doubleword, or Quadword Onto the Stack", with a 64-bit
stack address size and operand size (the default for `PUSH r64` in 64-bit
mode): `RSP := RSP − 8; Memory[SS:RSP] := SRC; (* push quadword *)`. No
flags are affected. -/
def pushRegs (s : State) : List Reg → State
  | [] => s
  | r :: rs =>
    let sp := s.gpr .rsp - 8
    pushRegs { s.setReg .rsp sp with mem := s.mem.writeW sp (s.gpr r) } rs

/-- `pop r`, `k` times, where `r ≠ rsp`: SDM Vol. 2, "POP—Pop a Value From
the Stack", with a 64-bit stack address size and operand size (the default
for `POP r64` in 64-bit mode): `DEST := Memory[SS:RSP]; (* Copy quadword *)
RSP := RSP + 8;`. No flags are affected. -/
def popReg (s : State) (r : Reg) : Nat → State
  | 0 => s
  | k + 1 =>
    popReg ((s.setReg r (s.mem.readW (s.gpr .rsp) 64)).setReg .rsp (s.gpr .rsp + 8)) r k

/-- The push of a frame: `push r` for each `r` of `rs` (`pushRegs`). The
`8 * rs.length` bytes it stores become a writable region, at the head of
`wr`. Faults if `rs` is empty or contains `rsp`, or if the frame would wrap
around the address space.

Or `lea rsp, [rsp - bytes]` (`alloc`): SDM Vol. 2, "LEA—Load Effective
Address", with a 64-bit operand size and address size: `DEST :=
EffectiveAddress(SRC)`, the address computed modulo 2⁶⁴; "Flags Affected:
None". The `bytes` bytes below `rsp` become a writable region, at the head
of `wr`, which it does not write: they hold what memory held there, as the
bytes a call or a push stores below `rsp` before it does (and a contract
says nothing of them: they are in the stack below the caller's stack
pointer, `Abi.reserved`). Faults unless `0 < bytes < 4096` and `bytes` is a
multiple of 8, or if the frame would wrap around the address space. -/
def push : Instr → State → Option State
  | .alloc bytes, s =>
    if 0 < bytes ∧ bytes < 4096 ∧ bytes % 8 = 0 ∧ bytes ≤ (s.gpr .rsp).toNat then
      let sp := s.gpr .rsp - BitVec.ofNat 64 bytes
      some { s.setReg .rsp sp with wr := ⟨sp, bytes⟩ :: s.wr }
    else none
  | .push rs, s =>
    let n := 8 * rs.length
    if rs ≠ [] ∧ .rsp ∉ rs ∧ n ≤ (s.gpr .rsp).toNat then
      some { pushRegs s rs with wr := ⟨s.gpr .rsp - BitVec.ofNat 64 n, n⟩ :: s.wr }
    else none
  | _, _ => none

/-- The pop of a frame: `pop r`, `k` times (`popReg`), so that `r` holds the
last quadword of the frame. Faults if `k = 0` or `r` is `rsp`, and unless
`rsp` and the writable regions are those the push left (`s₁`), and the
frame, the region at their head, has `8 * k` bytes; it removes the frame.

Or `lea rsp, [rsp + bytes]` (`free`, "LEA" as for `alloc`), with the same
conditions on `bytes` as `alloc` and on `rsp` and the writable regions as
`pop`, the frame having `bytes` bytes; it changes neither memory nor any
other register. -/
def pop : Instr → State → State → Option State
  | .free bytes, s₁, s₂ =>
    if 0 < bytes ∧ bytes < 4096 ∧ bytes % 8 = 0 ∧ s₂.gpr .rsp = s₁.gpr .rsp ∧ s₂.wr = s₁.wr ∧
        s₁.wr.head? = some ⟨s₁.gpr .rsp, bytes⟩ then
      some { s₂.setReg .rsp (s₂.gpr .rsp + BitVec.ofNat 64 bytes) with wr := s₂.wr.tail }
    else none
  | .pop r k, s₁, s₂ =>
    if k ≠ 0 ∧ r ≠ .rsp ∧ s₂.gpr .rsp = s₁.gpr .rsp ∧ s₂.wr = s₁.wr ∧
        s₁.wr.head? = some ⟨s₁.gpr .rsp, 8 * k⟩ then
      some { popReg s₂ r k with wr := s₂.wr.tail }
    else none
  | _, _, _ => none

/-- The general-purpose register an instruction writes, if it writes exactly
one (the pop of a frame also moves `rsp`, as the push does): `mul` writes
two, `rax` and `rdx`, `mulx` two of its choosing, and stores none. -/
def Instr.dst : Instr → Option Reg
  | .mov d _ | .alu _ d _ | .inc d | .dec d | .mov32 d _ | .alu32 _ d _ | .shift32 _ d _ | .bswap32 d
  | .rorx32 d .. | .andn32 d .. | .rorx d .. | .andn d .. | .movzx8 d _ | .movzx16 d _ | .bswap d | .shift _ d _
  | .shiftCl _ d | .shiftx _ d .. | .movImm64 d _ | .leaSym d .. | .movGot d _ | .movSym d _ | .leaRseqCs d | .lea d _ | .adcx d _ | .adox d _
  | .cmov _ d _ | .pop d _ | .imul d _ => some d
  | .store .. | .store32 .. | .store8 .. | .cmp8 .. | .mul _ | .mulx .. | .push _ | .alloc _ | .free _ => none

/-- The memory accesses of an instruction run from `s` to `s'` (`Event`):
a load of the bytes of its memory source (8 for a 64-bit operand, 4 for a
32-bit one, 1 for `movzx8` and `cmp8`), with the value `s` holds there; a store of the
bytes of its memory destination, with the value `s'` holds there. Each such
instruction makes exactly one access (SDM Vol. 2, each instruction's
"Operation"). The pushes, pops, calls and returns of frames and calls are
not listed: a restartable sequence, whose accesses a contract constrains,
has none (`Rust.checkRseq`). -/
def Instr.events (i : Instr) (s s' : State) : List Event :=
  let rd (m : MemOp) (n : Nat) : List Event := [⟨s.ea m, n, false, (s.mem.read (s.ea m) n).toNat⟩]
  let wr (m : MemOp) (n : Nat) : List Event := [⟨s.ea m, n, true, (s'.mem.read (s.ea m) n).toNat⟩]
  match i with
  | .mov _ (.mem m) | .alu _ _ (.mem m) | .mulx _ _ (.mem m) | .adcx _ (.mem m) | .adox _ (.mem m)
  | .cmov _ _ (.mem m) => rd m 8
  | .mov32 _ (.mem m) | .alu32 _ _ (.mem m) => rd m 4
  | .movSym _ name => [⟨s.syms name, 8, false, (s.mem.read (s.syms name) 8).toNat⟩]
  | .movzx8 _ m | .cmp8 _ m => rd m 1
  | .store m _ => wr m 8
  | .store32 m _ => wr m 4
  | .store8 m _ => wr m 1
  | _ => []

abbrev isa : ISA where
  State := State
  Instr := Instr
  Cond := Cond
  exec := exec
  events := Instr.events
  eval := eval
  call := call
  ret := ret
  -- Other than as the push and pop of a frame. `mul` writes `rax` and
  -- `rdx`, never `rsp`; `mulx` writes both of its destinations.
  writesSp i := match i with
    | .mulx hi lo _ => hi == .rsp || lo == .rsp
    | _ => i.dst == some .rsp
  push := push
  pop := pop
  requires := Instr.requires

end VG.X86_64
