module

public import Cralloc.TCB.Code
public import Cralloc.TCB.AArch64.State

/-!
# AArch64 machine model

**Trusted.** A model of the subset of A64 the routines use. Each instruction's
semantics here must agree with the Arm Architecture Reference Manual for
A-profile (DDI 0487), chapter C6, "A64 Base Instruction Descriptions", and its
shared pseudocode (chapter J1); when adding an instruction, cite the
pseudocode it transcribes. Relocations are those of AAELF64, "ELF for the Arm
64-bit Architecture".

Modelling choices:

* Registers are `x0`–`x17`, `x19`–`x28` and `x30`. `x18` is the platform
  register (AAPCS64 §6.1.1: "software developers creating platform-independent
  code are advised to avoid using r18 if at all possible"), and `x29` the frame
  pointer, which a frame record may be walked through at any time: neither can
  be named, so no code changes them.
* Register number 31 is the stack pointer in some operands and the zero
  register in others, the same encoding. It is not a `Reg`, so no operand
  can stand for it; the one instruction here that uses it, `stpz`, stores the
  zero register. The stack pointer is a field of the state, and no modelled
  instruction reads or writes it: `ISA.writesSp` is false, and calls and frames
  fault (`ISA.call`, `ISA.push`), so code that needs them cannot be verified
  until they are modelled.
* A 32-bit result is zero-extended into the 64-bit register (`State.write`).
* The flags are PSTATE.N, Z, C and V, each a plain bit. They hold whatever they
  held when the function was entered until an instruction sets them, and
  AAPCS64 §6.1.1 leaves them undefined across a call. A contract does not
  constrain them, so a proof covers every value: reading one before setting it
  is not a fault, as it is on x86-64, but no proof can rely on what it reads.
* Memory accesses must lie within the state's permitted regions: loads within
  `rd ++ wr`, stores within `wr`; otherwise the instruction faults. Alignment is
  not modelled: Linux leaves SCTLR_EL1.A clear for user code, so a misaligned
  access to Normal memory does not fault (DDI 0487, "Alignment of data
  accesses"), though it may not be single-copy atomic. So a sequence's commit, the
  store other threads and the kernel must see whole, must be aligned, which holds
  this way: its contract's trace says where it is (`Spec.Slab.Ctx.PopTrace` and its kin),
  a slab word, `SLABS` plus a multiple of 8, and `SLABS` starts on a page
  (`Spec.Slab.Ctx.Layout.page`, `Proof.Slab.wordAddr_aligned`). The store to `rseq_cs`
  is 8 bytes into the thread's area, which the kernel refuses to register unless it is
  32-byte aligned. In a body the emitter refuses an immediate offset that is not a
  multiple of the access's size (`Printer.bodyRefuses`).
* The model is sequential (`TCB/Code.lean`, "What the model leaves out"). On this
  architecture's weaker memory order, the routines rest on what
  docs/research/memory-ordering.md argues.
* The model describes every instruction with an operand of any value. Which
  immediates, offsets and shifts an instruction can encode is the printer's to
  check (`Printer.unencodable`, `TCB/AArch64/Print.lean`): the emitter refuses
  code it cannot print as the model describes, before an assembler sees it.
* A `Rel` names an address, of a symbol or of the routine's descriptor. `adrp`
  and `addLo12` are what the linker makes of `adrp` and `add` at `:lo12:`, two
  real instructions each modelled as itself, so that a sequence may be aborted
  between them and a routine may share an `adrp`. No instruction changes
  `syms` or `csDesc`: the addresses are fixed for the run of the program.
* `ldrWord` and `movGot` read a word that holds a symbol's address and that the
  dynamic linker fills before any code of the program runs, like a global offset
  table slot, so neither is a memory access of the program's: neither needs a
  region, and neither faults, but for the address `ldrWord` is given.
* On a C library that registers no rseq area for the thread (musl), the crate defines
  its own, in the executable's thread-local data, and there is no word `__rseq_offset`:
  no assembler for this architecture can write the area's distance from the thread
  pointer as data, so the linker writes it into two instructions, `addAreaHi` and
  `addAreaLo`. The contracts still say what the area is by the word
  (`Spec.Slab.Ctx.area`). So it is a ghost there: a word of the model that holds the
  distance, which only these two read, and no region or trace needs to name for them,
  since no real instruction loads it. It is at an address nothing else uses, so the
  region a contract gives it overlaps nothing it must not, and a routine's proof that it
  never writes the regions it only reads keeps it constant.
-/

@[expose] public section

namespace VG.AArch64

/-- The symbol of the rseq area the crate defines itself (`ownRseqArea`). -/
def ownAreaSym : String := "cralloc_rseq_area"

/-- The name of glibc's variable holding the rseq area's offset from the
thread pointer. -/
def rseqOffsetSym : String := "__rseq_offset"

/-- Branch conditions: the condition codes of `b.cond` and the tests of
`cbz`, `cbnz`, `tbz` and `tbnz`. -/
inductive Cond
  /-- `b.eq`: Z set. -/
  | eq
  /-- `b.ne`: Z clear. -/
  | ne
  /-- `b.hs`: C set, unsigned higher or same. -/
  | hs
  /-- `b.lo`: C clear, unsigned lower. -/
  | lo
  /-- `b.hi`: C set and Z clear, unsigned higher. -/
  | hi
  /-- `b.ls`: C clear or Z set, unsigned lower or same. -/
  | ls
  /-- `cbz r`: the register, at the operand size, is zero. -/
  | cbz (sz : Size) (r : Reg)
  /-- `cbnz r`: the register, at the operand size, is not zero. -/
  | cbnz (sz : Size) (r : Reg)
  /-- `tbz r, #bit`: the bit of the register is clear. -/
  | tbz (r : Reg) (bit : Fin 64)
  /-- `tbnz r, #bit`: the bit of the register is set. -/
  | tbnz (r : Reg) (bit : Fin 64)
  deriving DecidableEq, Repr

/-- DDI 0487 J1, `ConditionHolds(cond)`: `cond<3:1>` selects the test (`'000'`:
`PSTATE.Z == '1'`; `'001'`: `PSTATE.C == '1'`; `'100'`: `PSTATE.C == '1' &&
PSTATE.Z == '0'`), and `cond<0>` inverts it. C6.2, "CBZ", "CBNZ": `operand1 =
X[t, datasize]; if IsZero(operand1) == iszero then BranchTo(...)`; "TBZ", "TBNZ":
`operand = X[t, datasize]; if operand<bit_pos> == op then BranchTo(...)`. -/
def eval : Cond → State → Option Bool
  | .eq, s => some s.zf
  | .ne, s => some !s.zf
  | .hs, s => some s.cf
  | .lo, s => some !s.cf
  | .hi, s => some (s.cf && !s.zf)
  | .ls, s => some !(s.cf && !s.zf)
  | .cbz sz r, s => some (s.read sz r == 0)
  | .cbnz sz r, s => some (s.read sz r != 0)
  | .tbz r b, s => some !((s.gpr r).getLsbD b)
  | .tbnz r b, s => some ((s.gpr r).getLsbD b)

/-- The second operand of a data-processing instruction. -/
inductive Src
  | reg (r : Reg)
  /-- A register shifted left by a constant (the shifted-register forms). -/
  | lsl (r : Reg) (n : Nat)
  /-- An immediate, of the form the instruction can encode. -/
  | imm (v : Nat)
  deriving DecidableEq, Repr

/-- The value of a second operand at the operand size. DDI 0487 C6.2, "ADD
(shifted register)": `operand2 = ShiftReg(m, shift_type, shift_amount,
datasize)`, here with LSL; "ADD (immediate)": `imm = ZeroExtend(imm12, datasize)`
or `ZeroExtend(imm12:Zeros(12), datasize)`; "AND (immediate)": `imm =
DecodeBitMasks(...)`. The printer refuses an immediate the instruction cannot
encode, or one that does not fit the operand size. -/
def State.src (s : State) (sz : Size) : Src → BitVec sz.bits
  | .reg r => s.read sz r
  | .lsl r n => s.read sz r <<< n
  | .imm v => BitVec.ofNat sz.bits v

/-- DDI 0487 J1, `AddWithCarry(x, y, carry_in)`: `result = unsigned_sum<N-1:0>;
n = result<N-1>; z = IsZero(result); c = if UInt(result) == unsigned_sum then '0'
else '1'; v = if SInt(result) == signed_sum then '0' else '1'`, as flags. -/
def State.addFlags {w : Nat} (s : State) (x y : BitVec w) (carry : Bool) : State :=
  let r := x + y + BitVec.ofNat w carry.toNat
  s.setFlags r.msb (r == 0) (decide (2 ^ w ≤ x.toNat + y.toNat + carry.toNat))
    (decide (r.toInt ≠ x.toInt + y.toInt + carry.toNat))

inductive Instr
  /-- `ldr xt, m`, `ldr wt, m` or `ldrb wt, m`: the bytes at `m`, zero-extended
  into the 64-bit register. -/
  | ldr (wd : Width) (t : Reg) (m : MemOp)
  /-- `str xt, m`, `str wt, m` or `strb wt, m`: the low bytes of the register. -/
  | str (wd : Width) (t : Reg) (m : MemOp)
  /-- `stp xzr, xzr, [n, #off]`: sixteen zero bytes, in two 64-bit stores. -/
  | stpz (n : Reg) (off : Int)
  /-- `add d, n, src` -/
  | add (sz : Size) (d n : Reg) (src : Src)
  /-- `sub d, n, src` -/
  | sub (sz : Size) (d n : Reg) (src : Src)
  /-- `and d, n, src` -/
  | and (sz : Size) (d n : Reg) (src : Src)
  /-- `orr d, n, src` -/
  | orr (sz : Size) (d n : Reg) (src : Src)
  /-- `mov d, src`: of a register, `orr d, zr, m`; of an immediate, `movz`. -/
  | mov (sz : Size) (d : Reg) (src : Src)
  /-- `cmp n, src`: `subs zr, n, src`, the flags alone. -/
  | cmp (sz : Size) (n : Reg) (src : Src)
  /-- `tst n, src`: `ands zr, n, src`, the flags alone. -/
  | tst (sz : Size) (n : Reg) (src : Src)
  /-- `lsl d, n, #sh`: an alias of `ubfm`. -/
  | lsl (sz : Size) (d n : Reg) (sh : Nat)
  /-- `lsr d, n, #sh`: an alias of `ubfm`. -/
  | lsr (sz : Size) (d n : Reg) (sh : Nat)
  /-- `lsl d, n, m`: `lslv`, the count `m` modulo the operand size. -/
  | lslv (sz : Size) (d n m : Reg)
  /-- `lsr d, n, m`: `lsrv`, the count `m` modulo the operand size. -/
  | lsrv (sz : Size) (d n m : Reg)
  /-- `mul d, n, m`: `madd d, n, m, zr`. -/
  | mul (sz : Size) (d n m : Reg)
  /-- `umulh d, n, m`: the high 64 bits of the unsigned 128-bit product. -/
  | umulh (d n m : Reg)
  /-- `adrp d, t`: the 4 KiB page of the address `t`. -/
  | adrp (d : Reg) (t : Rel)
  /-- `add d, n, :lo12:t`: `n` plus the low 12 bits of the address `t`. -/
  | addLo12 (d n : Reg) (t : Rel)
  /-- `mrs d, tpidr_el0`: the thread pointer. -/
  | mrsTp (d : Reg)
  /-- `ldr d, [n, #32]`, where `n` holds the address of the routine's
  descriptor: the word after the descriptor, which holds the address of the
  symbol `name` (`Printer.descWord`). -/
  | ldrWord (d n : Reg) (name : String)
  /-- `adrp d, 8f` and `ldr d, [d, :lo12:8f]`, of a word of the routine's own
  holding the address of the symbol `name` (`gotLines`). -/
  | movGot (d : Reg) (name : String)
  /-- `add d, n, :tprel_hi12:cralloc_rseq_area`: `n` plus the crate's own area's distance from
  the thread pointer, but for its low 12 bits. -/
  | addAreaHi (d n : Reg)
  /-- `add d, n, :tprel_lo12_nc:cralloc_rseq_area`: `n` plus that distance's low 12 bits. -/
  | addAreaLo (d n : Reg)
  deriving DecidableEq, Repr

/-- The distance the linker writes into `addAreaHi` and `addAreaLo`, as the model's word holds
it. -/
def State.areaOffset (s : State) : Addr := s.mem.readW (s.syms rseqOffsetSym) 64

/-- Semantics of an instruction. DDI 0487 C6.2:

* "LDR (immediate)", "LDUR", "LDR (register)": `data = Mem[address, datasize DIV
  8]; X[t, regsize] = ZeroExtend(data, regsize)`; "LDRB": `X[t, 32] =
  ZeroExtend(data, 32)`, zero-extended again to 64 by the accessor. "STR",
  "STRB": `data = X[t, datasize]; Mem[address, datasize DIV 8] = data`, the low
  bytes. None affects the flags. (`State.ea` gives the address.)
* "STP", 64-bit, signed offset, with `t` and `t2` the zero register:
  `Mem[address, dbytes] = data1; Mem[address + dbytes, dbytes] = data2`, each
  all zero. The two stores are in order in the model; STP does not make them
  one access to other threads.
* "ADD (immediate)", "ADD (shifted register)": `(result, -) =
  AddWithCarry(operand1, operand2, '0')`, the sum modulo `2 ^ datasize`; "SUB":
  `(result, -) = AddWithCarry(operand1, NOT(operand2), '1')`, the difference.
  Neither sets the flags. Register 31 is the stack pointer in the immediate
  forms and zero in the others; neither is a `Reg`.
* "AND", "ORR": `result = operand1 AND operand2`, `OR`, without the flags.
* "MOV (register)" is `ORR Xd, XZR, Xm`; "MOV (wide immediate)" is `MOVZ`:
  `result = Zeros(); result<pos+15:pos> = imm16`. Neither sets the flags.
* "CMP" is `SUBS` with the zero register as destination: `(-, nzcv) =
  AddWithCarry(operand1, NOT(operand2), '1'); PSTATE.<N,Z,C,V> = nzcv`.
* "TST" is `ANDS` with the zero register as destination: `result = operand1 AND
  operand2; PSTATE.<N,Z,C,V> = result<datasize-1>:IsZeroBit(result):'00'`.
* "LSL (immediate)", "LSR (immediate)" are `UBFM`: a shift by `sh`, zeros shifted
  in, without the flags. "LSLV", "LSRV": `result = ShiftReg(n, shift_type,
  UInt(operand2) MOD datasize, datasize)`.
* "MUL" is `MADD` with the zero register as addend: `result =
  (UInt(operand1) * UInt(operand2))<destsize-1:0>`.
* "UMULH": `result = UInt(operand1) * UInt(operand2); X[d, 64] = result<127:64>`,
  without the flags.
* "ADRP": `base = PC64; base<11:0> = Zeros(12); X[d, 64] = base + imm`, where the
  linker writes `imm` as `Page(S + A) - Page(P)` (AAELF64,
  `R_<CLS>_ADR_PREL_PG_HI21`), `P` the instruction's address: `Page(S + A)`. A
  distance beyond ±4 GiB does not link.
* `addLo12`: "ADD (immediate)" with the `imm12` the linker writes as `S + A`'s
  low 12 bits (AAELF64, `R_<CLS>_ADD_ABS_LO12_NC`).
* "MRS" of TPIDR_EL0 (DDI 0487, "TPIDR_EL0, EL0 Read/Write Software Thread ID
  Register"): `X[t, 64] = AArch64.SysRegRead(...)`, allowed at EL0, the flags
  unchanged.
* `ldrWord`: "LDR (immediate)" from the word `Printer.descWord` places after the
  descriptor. The assembler writes `.quad name` there, and the linker or the
  dynamic linker fills it with the symbol's address `S` before any code of the
  program runs (AAELF64, `R_<CLS>_ABS64`): `X[d, 64] = S`. It faults unless `n`
  holds the descriptor's address, the only thing the word is known to follow.
* `movGot`: "ADRP" and "LDR (immediate)" of a word in relocated read-only data,
  filled as `ldrWord`'s is: `X[d, 64] = S`.
* `addAreaHi`, `addAreaLo`: "ADD (immediate)" with the `imm12`, shifted left by 12 for the
  first, that the linker writes as bits 23:12 and bits 11:0 of `X = TPREL(S + A)` (AAELF64,
  `R_<CLS>_TLSLE_ADD_TPREL_HI12`, which does not link unless `0 ≤ X < 2 ^ 24`, and
  `R_<CLS>_TLSLE_ADD_TPREL_LO12_NC`). So for every program that links, the first adds `X` with
  its low 12 bits clear, and the two add `X`. -/
def exec : Instr → State → Option State
  | .ldr .x t m, s => (s.load64 (s.ea m)).map fun v => s.setReg t v
  | .ldr .w t m, s => (s.load32 (s.ea m)).map fun v => s.setReg t (v.setWidth 64)
  | .ldr .b t m, s => (s.load8 (s.ea m)).map fun v => s.setReg t (v.setWidth 64)
  | .str .x t m, s => s.store64 (s.ea m) (s.gpr t)
  | .str .w t m, s => s.store32 (s.ea m) ((s.gpr t).setWidth 32)
  | .str .b t m, s => s.store8 (s.ea m) ((s.gpr t).setWidth 8)
  | .stpz n off, s =>
    let a := s.gpr n + BitVec.ofInt 64 off
    (s.store64 a 0).bind fun s₁ => s₁.store64 (a + 8) 0
  | .add sz d n src, s => some (s.write sz d (s.read sz n + s.src sz src))
  | .sub sz d n src, s => some (s.write sz d (s.read sz n - s.src sz src))
  | .and sz d n src, s => some (s.write sz d (s.read sz n &&& s.src sz src))
  | .orr sz d n src, s => some (s.write sz d (s.read sz n ||| s.src sz src))
  | .mov sz d src, s => some (s.write sz d (s.src sz src))
  | .cmp sz n src, s => some (s.addFlags (s.read sz n) (~~~(s.src sz src)) true)
  | .tst sz n src, s =>
    let r := s.read sz n &&& s.src sz src
    some (s.setFlags r.msb (r == 0) false false)
  | .lsl sz d n sh, s => some (s.write sz d (s.read sz n <<< sh))
  | .lsr sz d n sh, s => some (s.write sz d (s.read sz n >>> sh))
  | .lslv sz d n m, s => some (s.write sz d (s.read sz n <<< ((s.read sz m).toNat % sz.bits)))
  | .lsrv sz d n m, s => some (s.write sz d (s.read sz n >>> ((s.read sz m).toNat % sz.bits)))
  | .mul sz d n m, s => some (s.write sz d (s.read sz n * s.read sz m))
  | .umulh d n m, s =>
    some (s.setReg d (BitVec.ofNat 64 ((s.gpr n).toNat * (s.gpr m).toNat / 2 ^ 64)))
  | .adrp d t, s => some (s.setReg d (page (s.relAddr t)))
  | .addLo12 d n t, s => some (s.setReg d (s.gpr n + lo12 (s.relAddr t)))
  | .mrsTp d, s => some (s.setReg d s.tpidr)
  | .ldrWord d n name, s => if s.gpr n = s.csDesc then some (s.setReg d (s.syms name)) else none
  | .movGot d name, s => some (s.setReg d (s.syms name))
  | .addAreaHi d n, s => some (s.setReg d (s.gpr n + page s.areaOffset))
  | .addAreaLo d n, s => some (s.setReg d (s.gpr n + lo12 s.areaOffset))

/-- The memory accesses of an instruction run from `s` to `s'` (`Event`): a load
of the bytes of its memory source, with the value `s` holds there; a store of
the bytes of its memory destination, with the value `s'` holds there. `ldrWord`
and `movGot` read words no program writes, and `addAreaHi` and `addAreaLo` none at all, so
none is listed. -/
def Instr.events (i : Instr) (s s' : State) : List Event :=
  let rd (a : Addr) (n : Nat) : List Event := [⟨a, n, false, (s.mem.read a n).toNat⟩]
  let wr (a : Addr) (n : Nat) : List Event := [⟨a, n, true, (s'.mem.read a n).toNat⟩]
  match i with
  | .ldr wd _ m => rd (s.ea m) wd.bytes
  | .str wd _ m => wr (s.ea m) wd.bytes
  | .stpz n off =>
    let a := s.gpr n + BitVec.ofInt 64 off
    wr a 8 ++ wr (a + 8) 8
  | _ => []

abbrev isa : ISA where
  State := State
  Instr := Instr
  Cond := Cond
  exec := exec
  events := Instr.events
  eval := eval
  -- Calls and frames are not modelled, so code with one cannot be verified.
  call _ := none
  ret _ _ := none
  -- No modelled instruction names the stack pointer.
  writesSp _ := false
  push _ _ := none
  pop _ _ _ := none
  -- The routines use only the base instruction set.
  requires _ := []

end VG.AArch64
