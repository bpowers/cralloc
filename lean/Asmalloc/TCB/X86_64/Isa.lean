module

public import Asmalloc.TCB.Code
public import Asmalloc.TCB.X86_64.Gpr
public import Asmalloc.TCB.X86_64.Evex

/-!
# x86-64 machine model

**Trusted.** A model of the subset of x86-64 used by our implementations.
Each instruction's semantics here must agree with the Intel SDM; when adding
an instruction, cite the SDM pseudocode it transcribes.

The model is split by instruction family: `State.lean` holds the registers,
the state and memory operands; `Gpr.lean`, `Sse.lean`, `Avx.lean` and
`Avx512.lean` the semantics of the general-purpose, SSE, AVX and AVX-512
instructions; and this file the
instructions themselves, the CPU features they require, and `exec`, which
dispatches to those semantics.

Modelling choices:
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
* Instructions whose timing depends on their operands (e.g. `div`) must never
  be added: the constant-time leakage model assumes they do not exist. `mul`
  is one of the instructions whose timing Intel documents as independent of
  their data operands ("Data Operand Independent Timing Instruction Set
  Architecture (ISA) Guidance", which lists `MUL`), and so are BMI2's `mulx`
  and ADX's `adcx` and `adox` (it lists `MULX`, `ADCX` and `ADOX`), `shl`
  (`SHL`), the shifts by CL `shl`, `shr` and `sar` (`SHL`, `SHR` and `SAR`,
  each with opcode `0xd3`), BMI2's `shlx` and `shrx` (`SHLX` and `SHRX`; it
  does not list `SARX`, which the model leaves out), `cmovcc` (`CMOVcc`),
  and SSE2's `pcmpeqd` and its VEX forms `vpcmpeqd` (`PCMPEQD`,
  `VPCMPEQD`). `cmovcc` is not a branch: whether the condition holds
  changes the destination, not which instructions run or for how long, so
  the model's leakage of it is only its memory source's address (`addrs`),
  which it reads whether or not the condition holds.
* Assumed, not proven: the model's instructions take a time independent of
  their data operands on every processor that runs this code. Intel's
  guidance, the only vendor statement the model cites, is narrower. It
  covers Intel Core and Atom processors only, not those of other vendors
  (AMD, VIA, Zhaoxin), which document no such list. On Intel Core
  processors from Ice Lake and Intel Atom processors from Gracemont on,
  which enumerate DOITM, it holds only while the DOITM bit
  (IA32_UARCH_MISC_CTL[0], MSR 1B01H) is set; the bit resets to 0 and only
  privileged software can set it, so user code (and so this library)
  cannot, and runs with it clear unless the operating system sets it
  ("Data Operand Independent Timing ISA Guidance", "DOITM";
  https://www.intel.com/content/www/us/en/developer/articles/technical/software-security-guidance/best-practices/data-operand-independent-timing-isa-guidance.html).
  And Intel's list of the instructions it covers ("Data Operand Independent
  Timing Instructions", updated 2/23/2026;
  https://www.intel.com/content/www/us/en/developer/articles/technical/software-security-guidance/resources/data-operand-independent-timing-instructions.html)
  does not list `VSHA512RNDS2`, `VSHA512MSG1` or `VSHA512MSG2` (`Avx.lean`),
  which take secret data in `vg_sha512_compress_shani`: that they, like
  the `SHA1*` and `SHA256*` instructions it does list, take a time
  independent of their data is an assumption no vendor statement covers.
* Calls (`call`) and returns (`ret`) are near and direct (SDM Vol. 2, "CALL",
  "RET"). The return addresses are the next of the state's `unknowns`,
  which nothing constrains (see `TCB/Code.lean`).
* A `static` the code reads (`Artifact.consts`) is at the address
  `State.syms name`, where `name` is its name, which `leaSym` (`lea` of a
  RIP-relative operand, resolved by the linker) puts in a register. No
  instruction changes `syms`: a static's address is fixed for the run of
  the program.
* A table of constants an instruction carries (`aluTbl` and `vmovdquTbl`,
  its name and its words) is the `static` of that name the emitter writes from it
  (`TCB/Rust.lean`), so that for the whole run of the program it is mapped
  readable at `State.syms name` and holds exactly those words, 8 bytes each,
  little-endian, and does not wrap around the address space; and no memory
  any code writes overlaps it: no region a contract lets a function write,
  nor the stack. This is what a Rust `static` without interior mutability
  is: an immutable object, which the linker places in read-only memory,
  apart from every other object and from the stack, and which no `&mut`
  (safe Rust cannot make one to it) nor any other pointer the program may
  write through can reach. So the model reads the words from the
  instruction rather than from `State.mem`, whose writes never reach it,
  and no contract states any of this. Its address, fixed when the program
  is loaded, does not depend on the data the code works on: an access to it
  is observed (`addrs`) as its offset in the table.
* `push` and `pop` (of 64-bit registers other than `rsp`) only occur as the
  push and pop of a frame (see `push`), as a sequence of them. A caller
  passes arguments on the stack by pushing them, the last first, just
  before the call: on the callee's entry the first is then at `[rsp + 8]`,
  above the return address.
* A frame may instead allocate a buffer of `bytes` bytes on the stack
  (`alloc`, `lea rsp, [rsp - bytes]`), released by its pop (`free`, `lea
  rsp, [rsp + bytes]`); `lea` writes only `rsp`, so neither changes the
  flags or memory. `bytes` is positive, a multiple of 8 and less than 4096,
  as for the frames of the AArch64 model: less than a page, so that the
  allocation does not move `rsp` past a guard page below the stack.
* The SSE registers `xmm0`–`xmm15` are modelled as 128 bits each (SDM Vol. 1
  §10.2.2), and separately the upper halves (bits 255:128) of the AVX
  registers `ymm0`–`ymm15` that alias them (SDM Vol. 1 §14.1.1) and bits
  511:256 of the AVX-512 registers `zmm0`–`zmm15` that alias those (SDM
  Vol. 1 §15.5). The legacy SSE instructions leave bits 511:128 unmodified;
  the VEX-encoded ones zero every bit above their vector length (SDM Vol. 1
  §14.1.3: "VEX.128 encoded … the upper bits (MAXVL-1:128) of the
  destination are zeroed", and each VEX.256 form's pseudocode ends with
  `DEST[MAXVL-1:256] := 0`), and so do the EVEX-encoded ones, of 256 or 512
  bits, unmasked (each EVEX form's pseudocode ends with `DEST[MAXVL-1:VL] :=
  0`). Registers `zmm16`–`zmm31`, which only EVEX-encoded instructions name,
  are held in `ymmH` (bits 255:0) and `zmmHiH` (bits 511:256). EVEX.256 and
  EVEX.128 writes zero their upper bits; VZEROUPPER only changes registers
  0–15 (Intel SDM Vol. 2, "VZEROUPPER").
* The opmask registers `k0`–`k7` (SDM Vol. 1, Chapter 15) are not modelled.
  The only masked instructions are the masked 512-bit moves `kLoad` and
  `kStore`, each of which the model treats as one instruction together with
  the `kmovw k1, r32` before it that sets its mask, as `push` is the
  sequence of a frame's pushes: no modelled instruction reads `k1`
  otherwise, and only these write it. The value they leave in `k1` is
  never read, and the opmask registers are caller-saved (System V AMD64
  psABI §3.2.1, Figure 3.4: `%k0`–`%k7`, "No" under "callee-saved"), so the
  model need not know it. A masked move accesses exactly the quadwords its
  mask enables: the others are neither read nor written, and may be
  outside the permitted regions, as their faults are suppressed (SDM Vol. 2,
  "MOVDQU,VMOVDQU8/16/32/64", whose EVEX forms have the exceptions of Type
  E4.nb, Vol. 2A §2.8, Table 2-49, which suppress the memory faults of the
  elements whose mask bit is 0). Where an
  access is and which quadwords it enables can affect its timing (a
  masked-off quadword on an unmapped page may take a microcode assist), so
  it leaks both: its address and the mask, bits 7:0 of the register it
  moves to `k1` (`addrs`).
  No SSE, AVX or AVX-512 instruction has a memory operand except the
  unaligned `movdqu`/`vmovdqu`/`vmovdqu32` loads and stores,
  `vbroadcasti128`/`vbroadcasti32x4`, the `vmovdqu64` loads and stores
  (`evLoad`, `evStore`), the quadword source of the
  embedded-broadcast forms `zbcst` (`m64bcst`), the second source of
  `vbinLoad` (`xmm3/m128` or `ymm3/m256`), the 256-bit second
  source of `vpmadd52Load` and `evMadd52Load` (`m256`), and the masked
  moves `kLoad` and `kStore` (`m512`), which (like every VEX or
  EVEX memory operand but those of the aligned moves) need no alignment, so
  no alignment fault needs modelling.
* MXCSR (SDM Vol. 1 §10.2.3) is modelled as a 32-bit value that only
  `ldmxcsr` and `stmxcsr` access: no other modelled instruction reads or
  writes it (integer instructions raise no SIMD floating-point exceptions).
  Its control bits (15:6) are callee-saved (see `Target.lean`). `lfence`
  has no architectural effect, so the model treats it as a no-op. As in
  the SDM, bits 31:16 are reserved: `ldmxcsr` of a value with any of them
  set faults. AMD processors with misaligned SSE mode define bit 17 as the
  control bit MM, "Misaligned Exception Mask" (AMD64 Architecture
  Programmer's Manual Vol. 1, "MXCSR Register"), which the model does not
  know, so code that saves and restores MXCSR clears bits 31:16 of the
  value it restores (`and r32, 65535`): a caller's MM = 1 would be cleared
  on return. Nothing is known to set it.
* MXCSR-configuration-dependent timing (MCDT): on some Intel processors,
  the multiplies of the model but `mul` and `mulx` (`pmuludq`, `vpmuludq`,
  `pmullw`, `vpmullw`, `pmulhw`, `vpmulhw`, `vpmadd52luq` and
  `vpmadd52huq`), although on Intel's DOIT
  list, may take up to a cycle longer to retire for specific data values
  unless MXCSR holds `0x1FBF` (Intel, "MXCSR Configuration Dependent Timing", and
  its list of the instructions affected, "MCDT Data Operand Independent
  Timing Instructions"; the processors that enumerate `MCDT_NO`,
  CPUID.(EAX=7H,ECX=2):EDX[5], are not affected). The leakage model does
  not see this, so code must only give these instructions secret operands
  between Intel's prologue and epilogue:
  `stmxcsr` (save the caller's MXCSR), `ldmxcsr` of `0x1FBF`, `lfence`, then
  the code that uses them, then `lfence` and `ldmxcsr` of the saved value.
  `ci/check_mcdt.py` checks this in the generated code; `lfence` and the
  MXCSR instructions exist in the model for it.
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
outside the readable regions faults either way (and its address is
accessed, `addrs`). An immediate source does not exist, and a condition on
an undefined flag faults, as a branch on it does. -/
def execCmov (cc : Cond) (dst : Reg) (src : Src) (s : State) : Option State :=
  match src with
  | .imm _ => none
  | _ => (readSrc s src).bind fun temp => (eval cc s).map fun c =>
    if c then s.setReg dst temp else s

inductive Instr
  /-- `mov dst, src` (64-bit) -/
  | mov (dst : Reg) (src : Src)
  /-- `mov QWORD PTR [dst], src` -/
  | store (dst : MemOp) (src : Reg)
  /-- Two-operand ALU instruction `op dst, src` (64-bit). -/
  | alu (op : AluOp) (dst : Reg) (src : Src)
  /-- `mov r32, src` (32-bit; `DWORD PTR` for a memory source). An immediate
  is used as is. The result is zero-extended into the 64-bit register. -/
  | mov32 (dst : Reg) (src : Src)
  /-- `mov DWORD PTR [dst], r32` -/
  | store32 (dst : MemOp) (src : Reg)
  /-- Two-operand ALU instruction `op r32, src` (32-bit; `DWORD PTR` for a
  memory source). An immediate is used as is. -/
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
  /-- `lea r64, [rip + name]` (REX.W + 8D /r, with a RIP-relative memory
  operand): the address of the `static` `name`, `State.syms name`. -/
  | leaSym (dst : Reg) (name : String)
  /-- `op r64, QWORD PTR [m]` (REX.W + 03 /r, 2B /r, 23 /r, 0B /r, 33 /r,
  3B /r, 85 /r, 13 /r, 1B /r: the memory forms of `alu`), whose base register
  holds the address of the `static` `name`, whose words are `words`: the
  emitter writes that static from this instruction (`TCB/Rust.lean`). -/
  | aluTbl (op : AluOp) (dst : Reg) (m : MemOp) (name : String) (words : List (BitVec 64))
  /-- `movdqu xmm, XMMWORD PTR [src]` (`F3 0F 6F /r`) -/
  | movdquLoad (dst : XReg) (src : MemOp)
  /-- `movdqu XMMWORD PTR [dst], xmm` (`F3 0F 7F /r`) -/
  | movdquStore (dst : MemOp) (src : XReg)
  /-- `movq r64, xmm` (`66 REX.W 0F 7E /r`, SSE2): the low quadword of `src`. -/
  | movqR (dst : Reg) (src : XReg)
  /-- An SSE instruction that writes only an SSE register. -/
  | xop (op : XOp)
  /-- An AVX instruction that writes only vector registers. -/
  | vop (op : VOp)
  /-- `vmovdqu xmm, XMMWORD PTR [src]` (`VEX.128.F3.0F.WIG 6F /r`) or
  `vmovdqu ymm, YMMWORD PTR [src]` (`VEX.256.F3.0F.WIG 6F /r`) -/
  | vmovdquLoad (len : VLen) (dst : XReg) (src : MemOp)
  /-- `vmovdqu XMMWORD PTR [dst], xmm` (`VEX.128.F3.0F.WIG 7F /r`) or
  `vmovdqu YMMWORD PTR [dst], ymm` (`VEX.256.F3.0F.WIG 7F /r`) -/
  | vmovdquStore (len : VLen) (dst : MemOp) (src : XReg)
  /-- `vmovdqu ymm, YMMWORD PTR [m]` (`VEX.256.F3.0F.WIG 6F /r`), whose base
  register holds the address of the `static` `name`, whose words are
  `words`: the emitter writes that static from this instruction
  (`TCB/Rust.lean`), as for `aluTbl`. -/
  | vmovdquTbl (dst : XReg) (m : MemOp) (name : String) (words : List (BitVec 64))
  /-- `vbroadcasti128 ymm, XMMWORD PTR [src]` (`VEX.256.66.0F38.W0 5A /r`) -/
  | vbroadcasti128 (dst : XReg) (src : MemOp)
  /-- `op xmm1, xmm2, XMMWORD PTR [src2]` or `op ymm1, ymm2, YMMWORD PTR
  [src2]`: the form of `VOp.vbin op` whose second source is the 16 or 32
  bytes at `src2` (`xmm3/m128`, `ymm3/m256`, the same opcode as the register
  form with a memory `ModRM.rm`). -/
  | vbinLoad (op : VBinOp) (len : VLen) (dst src1 : XReg) (src2 : MemOp)
  /-- `vpmovmskb r32, xmm` (`VEX.128.66.0F.WIG D7 /r`) or `vpmovmskb r32,
  ymm` (`VEX.256.66.0F.WIG D7 /r`): the most significant bit of each byte of
  `src`, into `dst`. -/
  | vpmovmskb (len : VLen) (dst : Reg) (src : XReg)
  /-- An AVX-512 instruction with 512-bit operands that writes only vector registers. -/
  | zop (op : ZOp)
  /-- `vmovdqu32 zmm, ZMMWORD PTR [src]` (`EVEX.512.F3.0F.W0 6F /r`) -/
  | vmovdqu32Load (dst : XReg) (src : MemOp)
  /-- `vmovdqu32 ZMMWORD PTR [dst], zmm` (`EVEX.512.F3.0F.W0 7F /r`) -/
  | vmovdqu32Store (dst : MemOp) (src : XReg)
  /-- `vbroadcasti32x4 zmm, XMMWORD PTR [src]` (`EVEX.512.66.0F38.W0 5A /r`) -/
  | vbroadcasti32x4 (dst : XReg) (src : MemOp)
  /-- The same EVEX.512 broadcast, with its destination in `zmm16`–`zmm31`.
  Intel SDM Vol. 2, "VBROADCAST": `EVEX.512.66.0F38.W0 5A /r`, AVX512F;
  `DEST[i+127:i] := SRC[127:0]` for all four lanes, with no write mask.
  Vol. 2 §2.7.1 describes the five-bit EVEX destination register specifier. -/
  | vbroadcasti32x4H (dst : HReg) (src : MemOp)
  /-- `op zmm1, zmm2, QWORD PTR [src2]{1to8}`, an AVX-512 instruction whose
  second source is the quadword at `src2` broadcast to every quadword
  (`m64bcst`, `EVEX.b = 1`): `vpmuludq` (`EVEX.512.66.0F.W1 F4 /r`),
  `vpandq` (`EVEX.512.66.0F.W1 DB /r`) or `vporq` (`EVEX.512.66.0F.W1 EB
  /r`). -/
  | zbcst (op : ZBcstOp) (dst src1 : XReg) (src2 : MemOp)
  /-- `vpmadd52luq ymm1, ymm2, YMMWORD PTR [src2]` (`EVEX.256.66.0F38.W1 B4
  /r`), or with `hi` `vpmadd52huq ymm1, ymm2, YMMWORD PTR [src2]`
  (`EVEX.256.66.0F38.W1 B5 /r`), AVX512_IFMA and AVX512VL: the forms of
  `VOp.vpmadd52luq` and `VOp.vpmadd52huq` whose second source is the 32
  bytes at `src2` (`ymm3/m256`), unmasked and without embedded broadcast. -/
  | vpmadd52Load (hi : Bool) (dst src1 : XReg) (src2 : MemOp)
  /-- An `EVEX.256` instruction, of any of `ymm0`–`ymm31`, that writes only
  a vector register (`Evex.lean`). -/
  | eop (op : EOp)
  /-- `vmovdqu64 ymm1, YMMWORD PTR [src]` (`EVEX.256.F3.0F.W1 6F /r`) -/
  | evLoad (dst : VReg) (src : MemOp)
  /-- `vmovdqu64 YMMWORD PTR [dst], ymm1` (`EVEX.256.F3.0F.W1 7F /r`) -/
  | evStore (dst : MemOp) (src : VReg)
  /-- `vpmadd52luq ymm1, ymm2, YMMWORD PTR [src2]` (`EVEX.256.66.0F38.W1 B4
  /r`), or with `hi` `vpmadd52huq` (`EVEX.256.66.0F38.W1 B5 /r`): as
  `vpmadd52Load`, of any of `ymm0`–`ymm31`. -/
  | evMadd52Load (hi : Bool) (dst src1 : VReg) (src2 : MemOp)
  /-- `kmovw k1, r32` (`VEX.L0.0F.W0 92 /r`), then `vmovdqu64 zmm1 {k1}{z},
  ZMMWORD PTR [src]` (`EVEX.512.F3.0F.W1 6F /r`), with zeroing-masking: the
  quadwords of the 64 bytes at `src` that bits 7:0 of `mask` enable, the
  others zero. -/
  | kLoad (mask : Reg) (dst : XReg) (src : MemOp)
  /-- `kmovw k1, r32` (`VEX.L0.0F.W0 92 /r`), then `vmovdqu64 ZMMWORD PTR
  [dst] {k1}, zmm1` (`EVEX.512.F3.0F.W1 7F /r`), with merging-masking: the
  quadwords of `src` that bits 7:0 of `mask` enable, stored to the 64 bytes
  at `dst`. -/
  | kStore (dst : MemOp) (mask : Reg) (src : XReg)
  /-- `stmxcsr DWORD PTR [dst]` (`NP 0F AE /3`) -/
  | stmxcsr (dst : MemOp)
  /-- `ldmxcsr DWORD PTR [src]` (`NP 0F AE /2`) -/
  | ldmxcsr (src : MemOp)
  /-- `lfence` (`NP 0F AE E8`) -/
  | lfence
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
(x86-64-v1, which includes SSE and SSE2: System V AMD64 psABI,
"Micro-Architecture Levels"), named as Rust's target features. SDM Vol. 2,
the "CPUID Feature Flag" column of each instruction's opcode table: SSSE3
for PSHUFB and PALIGNR (`66 0F 38 00 /r`, `66 0F 3A 0F /r ib`), SHA for
SHA256RNDS2, SHA256MSG1 and SHA256MSG2 (`NP 0F 38 CB /r`, `NP 0F 38 CC /r`,
`NP 0F 38 CD /r`) and for SHA1RNDS4, SHA1NEXTE, SHA1MSG1 and SHA1MSG2
(`NP 0F 3A CC /r ib`, `NP 0F 38 C8 /r`, `NP 0F 38 C9 /r`, `NP 0F 38 CA /r`); AES for AESENC, AESENCLAST, AESDEC, AESDECLAST, AESIMC
and AESKEYGENASSIST (`66 0F 38 DC /r`, `66 0F 38 DD /r`, `66 0F 38 DE /r`,
`66 0F 38 DF /r`, `66 0F 38 DB /r`, `66 0F 3A DF /r ib`); PCLMULQDQ for
PCLMULQDQ (`66 0F 3A 44 /r ib`); SSE2 for MOVQ xmm, r64 (`66 REX.W 0F 6E /r`),
PAND, PANDN, PADDQ, PMULUDQ, PSLLQ, PSRLQ, PSLLDQ, PSRLDQ and PCMPEQD
(`66 0F 76 /r`); AVX for the
VEX.128 forms of the lane-wise instructions (e.g. `VEX.128.66.0F.WIG FE /r`
VPADDD, `VEX.128.66.0F.WIG 76 /r` VPCMPEQD), for VMOVDQA, VMOVDQU (both
lengths), VMOVQ and VZEROUPPER; AVX2 for their VEX.256 forms (e.g.
`VEX.256.66.0F.WIG FE /r` VPADDD, `VEX.256.66.0F.WIG 76 /r` VPCMPEQD) and
for VPBLENDD, VPSLLVD/Q, VPSRLVD/Q, VPBROADCASTD/Q, VPERMQ, VPERM2I128,
VINSERTI128, VEXTRACTI128 and VBROADCASTI128 at any length; AVX for VPMOVMSKB reg, xmm1
(`VEX.128.66.0F.WIG D7 /r`) and AVX2 for VPMOVMSKB reg, ymm1
(`VEX.256.66.0F.WIG D7 /r`) and VPERMD (`VEX.256.66.0F38.W0 36 /r`). LDMXCSR and STMXCSR (SSE,
`NP 0F AE /2`, `NP 0F AE /3`) and LFENCE (SSE2, `NP 0F AE E8`) are in the
baseline, and so is CMOVcc (`REX.W + 0F 4x /r`), whose "CPUID Feature Flag"
CMOV (CPUID.01H:EDX.CMOV[bit 15]) is part of x86-64-v1; SHL (`C1 /4 ib`,
`REX.W + C1 /4 ib`, and by 1 `D1 /4`, `REX.W + D1 /4`) needs none. BMI2 for
RORX (`VEX.LZ.F2.0F3A.W0 F0 /r ib`, `VEX.LZ.F2.0F3A.W1 F0 /r ib`) and for
MULX (`VEX.LZ.F2.0F38.W1 F6 /r`), ADX for ADCX and ADOX (`66 REX.w 0F 38 F6
/r`, `F3 REX.w 0F 38 F6 /r`), and BMI1 for ANDN (`VEX.LZ.0F38.W0 F2 /r`,
`VEX.LZ.0F38.W1 F2 /r`). AVX512F for the EVEX.512 forms of VPADDD
(`EVEX.512.66.0F.W0 FE /r`), VPXORD (`EVEX.512.66.0F.W0 EF /r`),
VPUNPCKLDQ, VPUNPCKHDQ, VPUNPCKLQDQ and VPUNPCKHQDQ (`EVEX.512.66.0F.W0 62
/r`, `EVEX.512.66.0F.W0 6A /r`, `EVEX.512.66.0F.W1 6C /r`,
`EVEX.512.66.0F.W1 6D /r`), VPROLD (`EVEX.512.66.0F.W0 72 /1 ib`), VPSHUFD
(`EVEX.512.66.0F.W0 70 /r ib`), VSHUFI32X4 (`EVEX.512.66.0F3A.W0 43 /r
ib`), VPADDQ (`EVEX.512.66.0F.W1 D4 /r`), VPMULUDQ (`EVEX.512.66.0F.W1 F4
/r`), VPANDQ (`EVEX.512.66.0F.W1 DB /r`), VPORQ (`EVEX.512.66.0F.W1 EB
/r`), VPANDNQ (`EVEX.512.66.0F.W1 DF /r`), VPSLLQ and VPSRLQ
(`EVEX.512.66.0F.W1 73 /6 ib`, `EVEX.512.66.0F.W1 73 /2 ib`), VPBROADCASTQ
(`EVEX.512.66.0F38.W1 59 /r`), VMOVDQA64 (`EVEX.512.66.0F.W1 6F /r`),
VPTERNLOGD (`EVEX.512.66.0F3A.W0 25 /r ib`), VPRORQ (`EVEX.512.66.0F.W1 72 /0
ib`), VPERMQ (`EVEX.512.66.0F3A.W1 00 /r ib`), VMOVDQU32 (`EVEX.512.F3.0F.W0 6F /r`, `EVEX.512.F3.0F.W0 7F /r`) and
VBROADCASTI32X4 (`EVEX.512.66.0F38.W0 5A /r`), and for those of VPMULUDQ,
VPANDQ and VPORQ with an `m64bcst` source (`EVEX.512.66.0F.W1 F4 /r`,
`EVEX.512.66.0F.W1 DB /r`, `EVEX.512.66.0F.W1 EB /r`). SHA512 for VSHA512RNDS2,
VSHA512MSG1 and VSHA512MSG2 (`VEX.256.F2.0F38.W0 CB /r`, `VEX.256.F2.0F38.W0
CC /r`, `VEX.256.F2.0F38.W0 CD /r`). AVX512_IFMA and AVX512VL for the
EVEX.128 and EVEX.256 forms of VPMADD52LUQ and VPMADD52HUQ
(`EVEX.256.66.0F38.W1 B4 /r`, `EVEX.256.66.0F38.W1 B5 /r`, with a register
or an `m256` second source; the SDM's
"CPUID Feature Flag" column lists both, AVX512VL for the vector lengths
below 512 bits). AVX512VL and AVX512F for the EVEX.128 and EVEX.256 forms
of VPROLD (`EVEX.128.66.0F.W0 72 /1 ib`, `EVEX.256.66.0F.W0 72 /1 ib`),
VPTERNLOGD (`EVEX.128.66.0F3A.W0 25 /r ib`, `EVEX.256.66.0F3A.W0 25 /r ib`)
and VPRORQ (`EVEX.128.66.0F.W1 72 /0 ib`, `EVEX.256.66.0F.W1 72 /0 ib`),
likewise. AVX512F and AVX512VL for the EVEX.128 and EVEX.256 forms of
`Evex.lean` (the SDM's "CPUID Feature Flag" column lists both for each:
VPADDQ `EVEX.256.66.0F.W1 D4 /r`, VPXORQ `EVEX.256.66.0F.W1 EF /r`, VPANDQ
`EVEX.256.66.0F.W1 DB /r`, VPORQ `EVEX.256.66.0F.W1 EB /r`, VPSLLQ and
VPSRLQ `EVEX.256.66.0F.W1 73 /6 ib` and `/2 ib`, VPERMQ
`EVEX.256.66.0F3A.W1 00 /r ib`, VALIGNQ `EVEX.256.66.0F3A.W1 03 /r ib`,
VPBROADCASTQ `EVEX.256.66.0F38.W1 59 /r`, VMOVDQU64 `EVEX.256.F3.0F.W1 6F
/r` and `7F /r`), but AVX512F alone for VMOVQ xmm1, r64
(`EVEX.128.66.0F.W1 6E /r`) and VMOVQ xmm1, xmm2 (`EVEX.128.F3.0F.W1 7E /r`), and AVX512_IFMA and AVX512VL for the EVEX.256
VPMADD52LUQ and VPMADD52HUQ of any register. AVX512F for KMOVW k1, r32
(`VEX.L0.0F.W0 92 /r`) and for the EVEX.512 VMOVDQU64 load and store with
a write mask (`EVEX.512.F3.0F.W1 6F /r`, `7F /r`), and so for `kLoad` and
`kStore`.

Vector AES/GCM additions: SDM Vol. 2, "AESENC", "AESENCLAST", "AESDEC",
"AESDECLAST", "PCLMULQDQ", "PSHUFB", "PSLLDQ" and "PSRLDQ", opcode tables'
"CPUID Feature Flag": VEX.128 VAESENC/VAESENCLAST (`VEX.128.66.0F38.WIG
DC/DD /r`) and VAESDEC/VAESDECLAST (`VEX.128.66.0F38.WIG DE/DF /r`) require
AES and AVX; VEX.256 (`VEX.256.66.0F38.WIG DC/DD/DE/DF /r`) require VAES and
AVX. VEX.128 VPCLMULQDQ (`VEX.128.66.0F3A.WIG 44 /r ib`) requires PCLMULQDQ
and AVX; VEX.256 requires VPCLMULQDQ and AVX. EVEX.512 VAESENC/VAESENCLAST
and VAESDEC/VAESDECLAST (`EVEX.512.66.0F38.WIG DC/DD/DE/DF /r`) require
VAES and AVX512F; EVEX.512
VPCLMULQDQ (`EVEX.512.66.0F3A.WIG 44 /r ib`) requires VPCLMULQDQ and
AVX512F. EVEX.512 VPSHUFB (`EVEX.512.66.0F38.WIG 00 /r`), VPSLLDQ and
VPSRLDQ (`EVEX.512.66.0F.WIG 73 /7 ib`, `/3 ib`) require AVX512BW.
AVX and AVX512F requirements also ensure the vector state is enabled,
following the model's existing feature convention. -/
def Instr.requires : Instr → List String
  | .zop (.zbinH .vaesenc ..) | .zop (.zbinH .vaesenclast ..) => ["vaes", "avx512f"]
  | .xop (.bin .pshufb ..) | .xop (.palignr ..) => ["ssse3"]
  | .xop (.bin .sha256msg1 ..) | .xop (.bin .sha256msg2 ..) | .xop (.sha256rnds2 ..) => ["sha"]
  | .xop (.bin .sha1msg1 ..) | .xop (.bin .sha1msg2 ..) | .xop (.bin .sha1nexte ..)
  | .xop (.sha1rnds4 ..) => ["sha"]
  | .xop (.bin .aesenc ..) | .xop (.bin .aesenclast ..) | .xop (.bin .aesdec ..)
  | .xop (.bin .aesdeclast ..) | .xop (.bin .aesimc ..) | .xop (.aeskeygenassist ..) => ["aes"]
  | .xop (.pclmulqdq ..) => ["pclmulqdq"]
  | .vop (.vbin .vaesenc .l128 ..) | .vop (.vbin .vaesenclast .l128 ..)
  | .vbinLoad .vaesenc .l128 .. | .vbinLoad .vaesenclast .l128 ..
  | .vop (.vbin .vaesdec .l128 ..) | .vop (.vbin .vaesdeclast .l128 ..)
  | .vbinLoad .vaesdec .l128 .. | .vbinLoad .vaesdeclast .l128 .. => ["aes", "avx"]
  | .vop (.vbin .vaesenc .l256 ..) | .vop (.vbin .vaesenclast .l256 ..)
  | .vbinLoad .vaesenc .l256 .. | .vbinLoad .vaesenclast .l256 ..
  | .vop (.vbin .vaesdec .l256 ..) | .vop (.vbin .vaesdeclast .l256 ..)
  | .vbinLoad .vaesdec .l256 .. | .vbinLoad .vaesdeclast .l256 .. => ["vaes", "avx"]
  | .vop (.vpclmulqdq .l128 ..) => ["pclmulqdq", "avx"]
  | .vop (.vpclmulqdq .l256 ..) => ["vpclmulqdq", "avx"]
  | .vop (.vbin _ .l256 ..) | .vbinLoad _ .l256 .. | .vop (.vshift _ .l256 ..)
  | .vop (.vpshufd .l256 ..) | .vop (.vpalignr .l256 ..) => ["avx2"]
  | .vop (.vbin _ .l128 ..) | .vbinLoad _ .l128 .. | .vop (.vshift _ .l128 ..)
  | .vop (.vpshufd .l128 ..)
  | .vop (.vpalignr .l128 ..) | .vop (.vmovdqa ..) | .vop (.vmovq ..) | .vop .vzeroupper
  | .vmovdquLoad .. | .vmovdquStore .. | .vmovdquTbl .. => ["avx"]
  | .vop (.vpblendd ..) | .vop (.vvar ..) | .vop (.vpbroadcastd ..) | .vop (.vpbroadcastq ..)
  | .vop (.vpermq ..) | .vop (.vpermd ..) | .vop (.vperm2i128 ..) | .vop (.vinserti128 ..) | .vop (.vextracti128 ..)
  | .vbroadcasti128 .. | .vpmovmskb .l256 .. => ["avx2"]
  | .vpmovmskb .l128 .. => ["avx"]
  | .rorx32 .. | .rorx .. | .mulx .. | .shiftx .. => ["bmi2"]
  | .adcx .. | .adox .. => ["adx"]
  | .andn32 .. | .andn .. => ["bmi1"]
  | .zop (.zbin .vaesenc ..) | .zop (.zbin .vaesenclast ..)
  | .zop (.zbin .vaesdec ..) | .zop (.zbin .vaesdeclast ..) => ["vaes", "avx512f"]
  | .zop (.vpclmulqdq ..) => ["vpclmulqdq", "avx512f"]
  | .zop (.vpmadd52 ..) => ["avx512ifma", "avx512f"]
  | .zop (.zbin .vpshufb ..) | .zop (.vpslldq ..) | .zop (.vpsrldq ..) =>
    ["avx512bw"]
  | .zop _ | .vmovdqu32Load .. | .vmovdqu32Store .. | .vbroadcasti32x4 .. | .vbroadcasti32x4H .. | .zbcst .. =>
    ["avx512f"]
  | .kLoad .. | .kStore .. => ["avx512f"]
  | .vop (.vsha512rnds2 ..) | .vop (.vsha512msg1 ..) | .vop (.vsha512msg2 ..) => ["sha512"]
  | .vop (.vpmadd52luq ..) | .vop (.vpmadd52huq ..) | .vpmadd52Load .. =>
    ["avx512ifma", "avx512vl"]
  | .vop (.vprold ..) | .vop (.vpternlogd ..) | .vop (.vprorq ..) => ["avx512f", "avx512vl"]
  | .eop (.vpmadd52 ..) | .evMadd52Load .. => ["avx512ifma", "avx512vl"]
  | .eop (.vmovq ..) | .eop (.vmovqx ..) => ["avx512f"]
  | .eop _ | .evLoad .. | .evStore .. => ["avx512f", "avx512vl"]
  | _ => []

/-- Semantics of an instruction. The byte forms: SDM Vol. 2, "MOVZX":
`DEST := ZeroExtend(SRC)` (with a 32-bit destination, zero-extended to 64
bits, SDM Vol. 1 §3.4.1.1), and "MOV": `DEST := SRC`, where the source of a
byte store is the register's low byte (AL, CL, DL, BL, SPL, BPL, SIL, DIL,
R8B–R15B; SDM Vol. 1 §3.4.1.1). Neither affects the flags. -/
def exec : Instr → State → Option State
  | .mov d src, s => (readSrc s src).map fun v => s.setReg d v
  | .store m r, s => s.store64 (s.ea m) (s.gpr r)
  | .alu op d src, s => execAlu op d src s
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
  -- static's address `S` (`S + A - P`, with the addend `A = -4`: System V
  -- AMD64 psABI §4.4, "Relocation Types", `R_X86_64_PC32`; Mach-O's
  -- `X86_64_RELOC_SIGNED`; COFF's `IMAGE_REL_AMD64_REL32`): `DEST := S`. A
  -- static beyond `disp32`'s range (±2 GB) does not link.
  | .leaSym d name, s => some (s.setReg d (s.syms name))
  -- `aluTbl`: as `alu op d (.mem m)` (SDM Vol. 2, ADD, SUB, AND, OR, XOR,
  -- CMP, TEST, ADC, SBB, the `r64, r/m64` forms; `execAlu`), its source the
  -- 8 bytes at `m`, little-endian: when `m`'s base register holds the
  -- static's address and `m` is `off` bytes past it, a multiple of 8 within
  -- the table, word `off / 8` of the static `name`; otherwise it faults. The
  -- static is the immutable object of these words that the emitter writes,
  -- which no writable memory overlaps (see the module docs), so the bytes
  -- there are always these.
  | .aluTbl op d m name ws, s =>
    let off := (s.ea m - s.gpr m.base).toNat
    if s.gpr m.base = s.syms name ∧ off % 8 = 0 ∧ off / 8 < ws.length then
      execAluW op d (ws.getD (off / 8) 0) s
    else none
  -- SDM Vol. 2, "MOVDQU": `DEST[127:0] := SRC[127:0]`, with memory in
  -- little-endian byte order (SDM Vol. 1 §1.3.1); no alignment is required
  -- and no flags are affected.
  | .movdquLoad d m, s => (s.load128 (s.ea m)).map fun v => s.setXmm d v
  | .movdquStore m r, s => s.store128 (s.ea m) (s.xmm r)
  -- SDM Vol. 2, "MOVQ—Move Quadword", `MOVQ r/m64, xmm` (66 REX.W 0F 7E /r),
  -- "MOVQ instruction when destination operand is r/m64": `DEST[63:0] :=
  -- SRC[63:0]`; "Flags Affected: None".
  | .movqR d r, s => some (s.setReg d ((s.xmm r).extractLsb' 0 64))
  | .xop op, s => some (op.exec s)
  | .vop op, s => some (op.exec s)
  -- SDM Vol. 2, "MOVDQU" (VEX.128 and VEX.256 versions): `DEST[127:0] :=
  -- SRC[127:0]; DEST[MAXVL-1:128] := 0`, respectively `DEST[255:0] :=
  -- SRC[255:0]`, and for a store the 16 or 32 bytes of the source; no
  -- alignment is required.
  | .vmovdquLoad .l128 d m, s => (s.load128 (s.ea m)).map fun v => s.setV .l128 d v 0
  | .vmovdquLoad .l256 d m, s =>
    (s.load256 (s.ea m)).map fun v => s.setV .l256 d (v.extractLsb' 0 128) (v.extractLsb' 128 128)
  | .vmovdquStore .l128 m r, s => s.store128 (s.ea m) (s.xmm r)
  -- `vmovdquTbl`: as `vmovdquLoad .l256 d m`, its source the 32 bytes at
  -- `m`, little-endian: when `m`'s base register holds the static's address
  -- and `m` is `off` bytes past it, a multiple of 8 with four words of the
  -- table from there, words `off / 8` to `off / 8 + 3` of the static `name`
  -- (bits 63:0 the first); otherwise it faults. As for `aluTbl`, the static
  -- is the immutable object of these words the emitter writes, so the bytes
  -- there are always these.
  | .vmovdquTbl d m name ws, s =>
    let off := (s.ea m - s.gpr m.base).toNat
    let w := fun j => ws.getD (off / 8 + j) 0
    if s.gpr m.base = s.syms name ∧ off % 8 = 0 ∧ off / 8 + 4 ≤ ws.length then
      some (s.setV .l256 d (w 1 ++ w 0) (w 3 ++ w 2))
    else none
  | .vmovdquStore .l256 m r, s => s.store256 (s.ea m) (s.ymm r)
  -- SDM Vol. 2, "VBROADCAST": `DEST[127:0] := SRC[127:0]; DEST[255:128] :=
  -- SRC[127:0]`; no alignment is required.
  | .vbroadcasti128 d m, s => (s.load128 (s.ea m)).map fun v => s.setV .l256 d v v
  -- SDM Vol. 2, each instruction of `VBinOp` (VEX.128 and VEX.256 encoded,
  -- `SRC2` a memory operand): as the register form (`VOp.exec`'s `vbin`),
  -- with `SRC2` the 16 or 32 bytes at the address, in little-endian byte
  -- order (bits 127:0 the lower lane); no alignment is required.
  | .vbinLoad op .l128 d a m, s => (s.load128 (s.ea m)).map fun v =>
    s.setV .l128 d (op.sse.eval (s.lane a 0) v) 0
  | .vbinLoad op .l256 d a m, s => (s.load256 (s.ea m)).map fun v =>
    s.setV .l256 d (op.sse.eval (s.lane a 0) (v.extractLsb' 0 128))
      (op.sse.eval (s.lane a 1) (v.extractLsb' 128 128))
  -- SDM Vol. 2, "PMOVMSKB" (VEX.128 and VEX.256 encoded VPMOVMSKB): see
  -- `byteMask`; "The upper bits of r32 or r64 are filled with zeros." No
  -- flags are affected.
  | .vpmovmskb len d r, s =>
    some (s.setReg d (byteMask (s.ymm r) (match len with | .l128 => 16 | .l256 => 32)))
  | .zop op, s => some (op.exec s)
  -- SDM Vol. 2, "MOVDQU,VMOVDQU8/16/32/64" (EVEX.512 encoded VMOVDQU32,
  -- without a write mask): `DEST[511:0] := SRC[511:0]`, and for a store the
  -- 64 bytes of the source; no alignment is required.
  | .vmovdqu32Load d m, s => (s.load512 (s.ea m)).map fun v =>
    s.setZ d (v.extractLsb' 0 128) (v.extractLsb' 128 128) (v.extractLsb' 256 128)
      (v.extractLsb' 384 128)
  | .vmovdqu32Store m r, s => s.store512 (s.ea m) (s.zmm r)
  -- SDM Vol. 2, "VBROADCAST" (EVEX.512 encoded VBROADCASTI32X4, without a
  -- write mask): each 128-bit lane of `DEST` is `SRC[127:0]`; no alignment
  -- is required.
  | .vbroadcasti32x4 d m, s => (s.load128 (s.ea m)).map fun v => s.setZ d v v v v
  | .vbroadcasti32x4H d m, s => (s.load128 (s.ea m)).map fun v => s.setZH d v v v v
  -- See `ZBcstOp.sse`: on each 128-bit lane, the SSE operation of the lane
  -- of `SRC1` and the loaded quadword `SRC2[63:0]` in both quadwords; no
  -- alignment is required.
  | .zbcst op d a m, s => (s.load64 (s.ea m)).map fun v =>
    let f (i : Nat) := op.sse.eval (s.zlane a i) (v ++ v)
    s.setZ d (f 0) (f 1) (f 2) (f 3)
  -- SDM Vol. 2, "VPMADD52LUQ" and "VPMADD52HUQ" (EVEX.256 encoded, without a
  -- write mask or embedded broadcast): as the register form (see `madd52`,
  -- each lane), with `SRC3` the 32 bytes at the address, in little-endian
  -- byte order (bits 127:0 the lower lane), and `DEST[MAXVL-1:256] := 0`
  -- (`setV`); no alignment is required.
  | .vpmadd52Load hi d a m, s => (s.load256 (s.ea m)).map fun v =>
    s.setV .l256 d (madd52 hi (s.lane d 0) (s.lane a 0) (v.extractLsb' 0 128))
      (madd52 hi (s.lane d 1) (s.lane a 1) (v.extractLsb' 128 128))
  | .eop op, s => some (op.exec s)
  -- SDM Vol. 2, "MOVDQA,VMOVDQA32/64" and "MOVDQU,VMOVDQU8/16/32/64"
  -- (EVEX.256 encoded VMOVDQU64, without a write mask): `DEST[255:0] :=
  -- SRC[255:0]; DEST[MAXVL-1:256] := 0`, and for a store the 32 bytes of the
  -- source, in little-endian byte order; no alignment is required.
  | .evLoad d m, s => (s.load256 (s.ea m)).map fun v => s.setVy d v
  | .evStore m r, s => s.store256 (s.ea m) (s.vy r)
  -- As `vpmadd52Load`, with `SRC3` the 32 bytes at the address.
  | .evMadd52Load hi d a m, s => (s.load256 (s.ea m)).map fun v =>
    let dv := s.vy d
    let av := s.vy a
    s.setVy d (madd52 hi (lane256 dv 1) (lane256 av 1) (lane256 v 1) ++
      madd52 hi (lane256 dv 0) (lane256 av 0) (lane256 v 0))
  -- SDM Vol. 2, "KMOVW—Move From and to Mask Registers" (`KMOVW k1, r32`):
  -- `DEST := ZeroExtension(SRC[15:0])`, so `k1[j]` is bit `j` of `mask`;
  -- "Flags Affected: None". Then "MOVDQU,VMOVDQU8/16/32/64", VMOVDQU64
  -- (EVEX encoded versions, load-form), with `KL = 8`, `VL = 512` and
  -- zeroing-masking: `FOR j := 0 TO KL-1; i := j * 64; IF k1[j] THEN
  -- DEST[i+63:i] := SRC[i+63:i] ELSE DEST[i+63:i] := 0`, and the store-form,
  -- merging-masking: `IF k1[j] THEN DEST[i+63:i] := SRC[i+63:i] ELSE
  -- *DEST[i+63:i] remains unchanged*`. No alignment is required, and a
  -- masked-off quadword is not accessed (its faults are suppressed: see the
  -- opmask registers above). Only bits 7:0 of `k1` are used.
  | .kLoad k d m, s => (s.loadMasked512 (s.ea m) (s.gpr k)).map fun v =>
    s.setZ d (v.extractLsb' 0 128) (v.extractLsb' 128 128) (v.extractLsb' 256 128)
      (v.extractLsb' 384 128)
  | .kStore m k r, s => s.storeMasked512 (s.ea m) (s.gpr k) (s.zmm r)
  -- SDM Vol. 2, "STMXCSR": `m32 := MXCSR`. "LDMXCSR": `MXCSR := m32`, with
  -- #GP(0) "for an attempt to set reserved bits in MXCSR", which are bits
  -- 31:16 (SDM Vol. 1 §10.2.3); on processors without DAZ, bit 6 is
  -- reserved too (§11.6.6, `MXCSR_MASK`), which the model does not know: it
  -- is loaded only with values without it, or saved from MXCSR itself.
  -- "LFENCE": it orders instructions and has no architectural effect.
  -- None of them affects the flags.
  | .stmxcsr m, s => s.store32 (s.ea m) s.mxcsr
  | .ldmxcsr m, s => (s.load32 (s.ea m)).bind fun v =>
    if v.extractLsb' 16 16 = 0 then some { s with mxcsr := v } else none
  | .lfence, s => some s
  | .mul r, s => some (execMul r s)
  | .imul d r, s => some (execImul d r s)
  | .mulx hi lo src, s => execMulx hi lo src s
  | .adcx d src, s => execAdcx d src s
  | .adox d src, s => execAdox d src s
  | .cmov cc d src, s => execCmov cc d src s
  -- Only the push and pop of a frame (`push`, `pop`).
  | .push _, _ | .pop .., _ | .alloc _, _ | .free _, _ => none

def addrs : Instr → State → List Addr
  | .mov _ src, s => srcAddrs s src
  | .store m _, s => [s.ea m]
  | .alu _ _ src, s => srcAddrs s src
  | .mov32 _ src, s => srcAddrs s src
  | .store32 m _, s => [s.ea m]
  | .alu32 _ _ src, s => srcAddrs s src
  | .shift32 .., _ => []
  | .bswap32 _, _ => []
  | .rorx32 .., _ => []
  | .andn32 .., _ => []
  | .rorx .., _ => []
  | .andn .., _ => []
  | .movzx8 _ m, s => [s.ea m]
  | .store8 m _, s => [s.ea m]
  | .bswap _, _ => []
  | .shift .., _ => []
  | .shiftCl .., _ => []
  | .shiftx .., _ => []
  | .movImm64 .., _ => []
  | .leaSym .., _ => []
  -- The offset in the table: the static's address does not depend on the
  -- data (see the module docs).
  | .aluTbl _ _ m .., s => [s.ea m - s.gpr m.base]
  | .vmovdquTbl _ m .., s => [s.ea m - s.gpr m.base]
  | .movdquLoad _ m, s => [s.ea m]
  | .movdquStore m _, s => [s.ea m]
  | .movqR .., _ => []
  | .xop _, _ => []
  | .vop _, _ => []
  | .vmovdquLoad _ _ m, s => [s.ea m]
  | .vmovdquStore _ m _, s => [s.ea m]
  | .vbroadcasti128 _ m, s => [s.ea m]
  | .vbinLoad _ _ _ _ m, s => [s.ea m]
  | .vpmovmskb .., _ => []
  | .zop _, _ => []
  | .vmovdqu32Load _ m, s => [s.ea m]
  | .vmovdqu32Store m _, s => [s.ea m]
  | .vbroadcasti32x4 _ m, s => [s.ea m]
  | .vbroadcasti32x4H _ m, s => [s.ea m]
  | .zbcst _ _ _ m, s => [s.ea m]
  | .vpmadd52Load _ _ _ m, s => [s.ea m]
  | .eop _, _ => []
  | .evLoad _ m, s => [s.ea m]
  | .evStore m _, s => [s.ea m]
  | .evMadd52Load _ _ _ m, s => [s.ea m]
  -- Not only an address: the second observation is the mask (see the
  -- opmask registers above), as a number, so that every run of an
  -- instruction makes the same number of observations.
  | .kLoad k _ m, s | .kStore m k _, s => [s.ea m, (s.gpr k).setWidth 8 |>.setWidth 64]
  | .stmxcsr m, s => [s.ea m]
  | .ldmxcsr m, s => [s.ea m]
  | .lfence, _ => []
  | .mul _, _ => []
  | .imul .., _ => []
  | .mulx _ _ src, s => srcAddrs s src
  | .adcx _ src, s => srcAddrs s src
  | .adox _ src, s => srcAddrs s src
  | .cmov _ _ src, s => srcAddrs s src
  | .push rs, s => (List.range rs.length).map fun i => s.gpr .rsp - BitVec.ofNat 64 (8 * (i + 1))
  | .pop _ k, s => (List.range k).map fun i => s.gpr .rsp + BitVec.ofNat 64 (8 * i)
  | .alloc _, _ | .free _, _ => []

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
two, `rax` and `rdx`, and stores and SSE instructions none. -/
def Instr.dst : Instr → Option Reg
  | .mov d _ | .alu _ d _ | .mov32 d _ | .alu32 _ d _ | .shift32 _ d _ | .bswap32 d
  | .rorx32 d .. | .andn32 d .. | .rorx d .. | .andn d .. | .movzx8 d _ | .bswap d | .shift _ d _
  | .shiftCl _ d | .shiftx _ d .. | .movImm64 d _ | .leaSym d _ | .aluTbl _ d .. | .adcx d _ | .adox d _
  | .cmov _ d _ | .pop d _ | .vpmovmskb _ d _ | .movqR d _ | .imul d _ => some d
  | .store .. | .store32 .. | .store8 .. | .movdquLoad .. | .movdquStore .. | .xop _
  | .vop _ | .vmovdquLoad .. | .vmovdquStore .. | .vmovdquTbl .. | .vbroadcasti128 .. | .vbinLoad .. | .zop _
  | .vmovdqu32Load .. | .vmovdqu32Store .. | .vbroadcasti32x4 .. | .vbroadcasti32x4H .. | .zbcst .. | .vpmadd52Load ..
  | .eop _ | .evLoad .. | .evStore .. | .evMadd52Load .. | .kLoad .. | .kStore ..
  | .stmxcsr _ | .ldmxcsr _ | .lfence | .mul _ | .mulx .. | .push _ | .alloc _ | .free _ => none

abbrev isa : ISA where
  State := State
  Instr := Instr
  Cond := Cond
  exec := exec
  addrs := addrs
  eval := eval
  call := call
  callAddrs s := [s.gpr .rsp - 8]
  ret := ret
  retAddrs s := [s.gpr .rsp]
  -- Other than as the push and pop of a frame. `mul` writes `rax` and
  -- `rdx`, never `rsp`; `mulx` writes both of its destinations.
  writesSp i := match i with
    | .mulx hi lo _ => hi == .rsp || lo == .rsp
    | _ => i.dst == some .rsp
  push := push
  pop := pop
  requires := Instr.requires

end VG.X86_64
