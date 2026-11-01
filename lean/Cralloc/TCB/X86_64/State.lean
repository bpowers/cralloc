module

public import Cralloc.TCB.Code

/-!
# x86-64 machine state

**Trusted.** The registers, the machine state and memory operands of the
x86-64 model, and the helpers that read and write them. The modelling choices
are described in `TCB/X86_64/Isa.lean`.
-/

@[expose] public section

namespace VG.X86_64

inductive Reg
  | rax | rcx | rdx | rbx | rsp | rbp | rsi | rdi
  | r8 | r9 | r10 | r11 | r12 | r13 | r14 | r15
  deriving DecidableEq, Repr, Inhabited

structure State where
  gpr : Reg → BitVec 64
  cf : Option Bool
  zf : Option Bool
  sf : Option Bool
  of : Option Bool
  mem : Mem
  /-- Regions the code may read (in addition to `wr`). -/
  rd : List Region
  /-- Regions the code may read and write. -/
  wr : List Region
  /-- Values the model does not know, used in order: the return address each
  call stores (see `TCB/Code.lean`). -/
  unknowns : Nat → BitVec 64 := fun _ => 0
  /-- The address of each `static` the code can name (`leaSym`), by name. -/
  syms : String → BitVec 64 := fun _ => 0
  /-- The base address of the FS segment, which a memory operand with an FS
  segment override adds to its address (`State.ea`). The operating system
  sets it (on Linux, to the thread's thread-local storage); no modelled
  instruction changes it. -/
  fsBase : Addr := 0
  /-- The value of each compile-time constant the code names (a `const`
  operand, `Operand.const`), by name: fixed for the run, as the instance of
  a generic function the compiler emits is. -/
  params : String → BitVec 64 := fun _ => 0
  /-- The address of the routine's restartable-sequence descriptor
  (`leaRseqCs`), where the linker places it: the model does not know it, and
  no instruction changes it. -/
  csDesc : Addr := 0

/-- A segment override of a memory operand. Only FS is modelled. -/
inductive Seg
  | fs
  deriving DecidableEq, Repr

/-- A memory operand `seg:[base + index * scale + disp]`. -/
structure MemOp where
  base : Reg
  index : Option Reg := none
  /-- 1, 2, 4 or 8. -/
  scale : Nat := 1
  disp : Int := 0
  /-- The segment override, if any. -/
  seg : Option Seg := none
  deriving DecidableEq, Repr

namespace State

def setReg (s : State) (r : Reg) (v : BitVec 64) : State :=
  { s with gpr := fun r' => if r' = r then v else s.gpr r' }

/-- The effective address of a memory operand, its offset in its segment:
`base + index * scale + disp`, modulo 2⁶⁴ (SDM Vol. 1 §3.7.5, "Specifying
an Offset"). -/
def offset (s : State) (m : MemOp) : Addr :=
  match m.index with
  | none => s.gpr m.base + BitVec.ofInt 64 m.disp
  | some i => s.gpr m.base + s.gpr i * BitVec.ofNat 64 m.scale + BitVec.ofInt 64 m.disp

/-- The address a memory operand accesses: its offset, plus the base of its
segment if that is FS. In 64-bit mode the processor treats the bases of the
CS, DS, ES and SS segments as zero, while FS and GS "may be used as
additional base registers in linear address calculations" (SDM Vol. 1
§3.4.2.1, "Segment Registers in 64-Bit Mode"); the sum is modulo 2⁶⁴, like
the offset's. Paging is not modelled: the address is the one the state's
memory and regions are indexed by. -/
def ea (s : State) (m : MemOp) : Addr :=
  match m.seg with
  | none => s.offset m
  | some .fs => s.fsBase + s.offset m

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

/-- Write a 32-bit result, zero-extended to 64 bits (SDM Vol. 1 §3.4.1.1). -/
def setReg32 (s : State) (r : Reg) (v : BitVec 32) : State := s.setReg r (v.setWidth 64)

/-- Set CF, OF, ZF and SF. -/
def setFlags (s : State) (cf of zf sf : Option Bool) : State :=
  { s with cf := cf, of := of, zf := zf, sf := sf }

end State

end VG.X86_64
