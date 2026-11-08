module

public import Cralloc.TCB.Code

/-!
# AArch64 machine state

**Trusted.** The registers, the machine state and memory operands of the
AArch64 model, and the helpers that read and write them. The modelling choices
are described in `TCB/AArch64/Isa.lean`.
-/

@[expose] public section

namespace VG.AArch64

/-- The general-purpose registers code may name. Register number 31 is not
one: it is the zero register or the stack pointer, by instruction (DDI 0487,
"Registers in AArch64 Execution state"), and each use of it is a form of its
own (`Instr.stpz`), so no operand can mean the wrong one. `x18` and `x29` are
not modelled either (`TCB/AArch64/Isa.lean`). -/
inductive Reg
  | x0 | x1 | x2 | x3 | x4 | x5 | x6 | x7 | x8 | x9 | x10 | x11 | x12 | x13 | x14 | x15
  | x16 | x17 | x19 | x20 | x21 | x22 | x23 | x24 | x25 | x26 | x27 | x28 | x30
  deriving DecidableEq, Repr, Inhabited

/-- The size of a data-processing instruction's operands: `w` registers or
`x` registers. -/
inductive Size | w | x
  deriving DecidableEq, Repr

abbrev Size.bits : Size → Nat
  | .w => 32
  | .x => 64

/-- The width of a load or store: a byte, a 32-bit word or a 64-bit word. -/
inductive Width | b | w | x
  deriving DecidableEq, Repr

abbrev Width.bytes : Width → Nat
  | .b => 1
  | .w => 4
  | .x => 8

/-- What an address relative to the program counter names: a symbol plus a
constant, or the routine's restartable-sequence descriptor, label `3`
(`Printer.rseqLines`). -/
inductive Rel
  | sym (name : String) (off : Int := 0)
  | desc
  deriving DecidableEq, Repr

/-- The offset of a memory operand from its base register. -/
inductive Off
  /-- `[base, #d]` -/
  | imm (d : Int)
  /-- `[base, :lo12:t]`: the low 12 bits of the address `t`. -/
  | lo12 (t : Rel)
  /-- `[base, r]`, or `[base, r, lsl #sh]` if `sh` is not 0. -/
  | reg (r : Reg) (sh : Nat := 0)
  deriving DecidableEq, Repr

/-- A memory operand. -/
structure MemOp where
  base : Reg
  off : Off := .imm 0
  deriving DecidableEq, Repr

structure State where
  gpr : Reg → BitVec 64
  /-- The stack pointer. No modelled instruction names it. -/
  sp : BitVec 64 := 0
  /-- PSTATE.N: the result's top bit. -/
  nf : Bool := false
  /-- PSTATE.Z: the result is zero. -/
  zf : Bool := false
  /-- PSTATE.C: a carry, or no borrow after a subtraction. -/
  cf : Bool := false
  /-- PSTATE.V: signed overflow. -/
  vf : Bool := false
  mem : Mem
  /-- Regions the code may read (in addition to `wr`). -/
  rd : List Region
  /-- Regions the code may read and write. -/
  wr : List Region
  /-- The address of each symbol the code can name, by name: fixed for the
  run of the program. -/
  syms : String → BitVec 64 := fun _ => 0
  /-- TPIDR_EL0, the thread pointer: set by the C library when the thread
  starts. No modelled instruction changes it. -/
  tpidr : Addr := 0
  /-- The address of the routine's restartable-sequence descriptor, where the
  linker places it: the model does not know it, and no instruction changes
  it. -/
  csDesc : Addr := 0

/-- `Page(a)` (AAELF64, "Relocation operations"): `a` with its low 12 bits
clear. -/
def page (a : Addr) : Addr := BitVec.ofNat 64 (a.toNat / 4096 * 4096)

/-- The low 12 bits of `a`, which `:lo12:` selects. -/
def lo12 (a : Addr) : Addr := BitVec.ofNat 64 (a.toNat % 4096)

namespace State

def setReg (s : State) (r : Reg) (v : BitVec 64) : State :=
  { s with gpr := fun r' => if r' = r then v else s.gpr r' }

/-- A register at the operand size: DDI 0487, the pseudocode accessor `X[n,
width]`, the low `width` bits of `_R[n]`. -/
def read (s : State) : (sz : Size) → Reg → BitVec sz.bits
  | .x, r => s.gpr r
  | .w, r => (s.gpr r).setWidth 32

/-- A write at the operand size: DDI 0487, the accessor `X[n, width] = value`
sets `_R[n] = ZeroExtend(value, 64)`, so a 32-bit write clears the high half. -/
def write (s : State) : (sz : Size) → Reg → BitVec sz.bits → State
  | .x, r, v => s.setReg r v
  | .w, r, v => s.setReg r (v.setWidth 64)

/-- The address a `Rel` names. -/
def relAddr (s : State) : Rel → Addr
  | .sym n off => s.syms n + BitVec.ofInt 64 off
  | .desc => s.csDesc

/-- The address a memory operand accesses, modulo 2⁶⁴. DDI 0487 C6.2, "LDR
(immediate)" and "LDUR": `address = X[n, 64] + offset`, with `offset =
LSL(ZeroExtend(imm12, 64), scale)` or `SignExtend(imm9, 64)`; "LDR (register)":
`offset = ExtendReg(m, extend_type, shift, 64)`, with `extend_type` LSL and
`shift` 0 or `scale`. A `:lo12:` offset is the immediate the linker writes:
the low 12 bits of the address (AAELF64, `R_<CLS>_LDST<n>_ABS_LO12_NC`), which
it refuses to link if they are not a multiple of the access's size. -/
def ea (s : State) (m : MemOp) : Addr :=
  match m.off with
  | .imm d => s.gpr m.base + BitVec.ofInt 64 d
  | .lo12 t => s.gpr m.base + lo12 (s.relAddr t)
  | .reg r sh => s.gpr m.base + s.gpr r <<< sh

/-- Load 8 bytes, faulting if not permitted. -/
def load64 (s : State) (a : Addr) : Option (BitVec 64) :=
  if InRegions (s.rd ++ s.wr) a 8 then some (s.mem.readW a 64) else none

/-- Store 8 bytes, faulting if not permitted. -/
def store64 (s : State) (a : Addr) (v : BitVec 64) : Option State :=
  if InRegions s.wr a 8 then some { s with mem := s.mem.writeW a v } else none

/-- Load 4 bytes, faulting if not permitted. -/
def load32 (s : State) (a : Addr) : Option (BitVec 32) :=
  if InRegions (s.rd ++ s.wr) a 4 then some (s.mem.readW a 32) else none

/-- Store 4 bytes, faulting if not permitted. -/
def store32 (s : State) (a : Addr) (v : BitVec 32) : Option State :=
  if InRegions s.wr a 4 then some { s with mem := s.mem.writeW a v } else none

/-- Load 1 byte, faulting if not permitted. -/
def load8 (s : State) (a : Addr) : Option Byte :=
  if InRegions (s.rd ++ s.wr) a 1 then some (s.mem a) else none

/-- Store 1 byte, faulting if not permitted. -/
def store8 (s : State) (a : Addr) (v : Byte) : Option State :=
  if InRegions s.wr a 1 then some { s with mem := s.mem.writeW a v } else none

/-- Set PSTATE.N, Z, C and V. -/
def setFlags (s : State) (n z c v : Bool) : State :=
  { s with nf := n, zf := z, cf := c, vf := v }

end State

end VG.AArch64
