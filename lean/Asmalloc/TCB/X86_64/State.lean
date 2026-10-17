module

public import Asmalloc.TCB.Code

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

/-- The SSE registers. -/
inductive XReg
  | xmm0 | xmm1 | xmm2 | xmm3 | xmm4 | xmm5 | xmm6 | xmm7
  | xmm8 | xmm9 | xmm10 | xmm11 | xmm12 | xmm13 | xmm14 | xmm15
  deriving DecidableEq, Repr, Inhabited

/-- The vector registers `xmm16`–`xmm31` (and the `ymm` and `zmm` registers
that contain them), which only EVEX-encoded instructions can name (SDM Vol. 1
§15.5: "the AVX-512 programming environment ... 32 vector registers"; Vol. 2
§2.7.1, the EVEX prefix's `R'` and `V'` bits). -/
inductive HReg
  | xmm16 | xmm17 | xmm18 | xmm19 | xmm20 | xmm21 | xmm22 | xmm23
  | xmm24 | xmm25 | xmm26 | xmm27 | xmm28 | xmm29 | xmm30 | xmm31
  deriving DecidableEq, Repr, Inhabited

structure State where
  gpr : Reg → BitVec 64
  cf : Option Bool
  zf : Option Bool
  sf : Option Bool
  of : Option Bool
  /-- The low 128 bits of each SSE register. -/
  xmm : XReg → BitVec 128 := fun _ => 0
  /-- Bits 255:128 of each AVX register. -/
  ymmHi : XReg → BitVec 128 := fun _ => 0
  /-- Bits 511:256 of each AVX-512 register `zmm0`–`zmm15`. -/
  zmmHi : XReg → BitVec 256 := fun _ => 0
  /-- Bits 255:0 of each AVX-512 register `zmm16`–`zmm31` (the `ymm`
  registers `ymm16`–`ymm31`). -/
  ymmH : HReg → BitVec 256 := fun _ => 0
  /-- Bits 511:256 of `zmm16`–`zmm31`. -/
  zmmHiH : HReg → BitVec 256 := fun _ => 0
  /-- The SSE control and status register (SDM Vol. 1 §10.2.3). -/
  mxcsr : BitVec 32 := 0x1F80
  mem : Mem
  /-- Regions the code may read (in addition to `wr`). -/
  rd : List Region
  /-- Regions the code may read and write. -/
  wr : List Region
  /-- Values the model does not know, used in order: the return address each
  call stores and, on the ARM targets, what a linker veneer may leave in the
  intra-procedure-call scratch registers (see `TCB/Code.lean`). -/
  unknowns : Nat → BitVec 64 := fun _ => 0
  /-- The address of each `static` the code can name (`leaSym`), by name. -/
  syms : String → BitVec 64 := fun _ => 0

/-- A memory operand `[base + index * scale + disp]`. -/
structure MemOp where
  base : Reg
  index : Option Reg := none
  /-- 1, 2, 4 or 8. -/
  scale : Nat := 1
  disp : Int := 0
  deriving DecidableEq, Repr

/-- The vector length of a VEX-encoded instruction: 128 bits (`xmm`
operands, `VEX.L = 0`) or 256 bits (`ymm` operands, `VEX.L = 1`). -/
inductive VLen | l128 | l256
  deriving DecidableEq, Repr

namespace State

/-- A 128-bit lane of `zmm16`–`zmm31`, least significant first. -/
def zlaneH (s : State) (r : HReg) (i : Nat) : BitVec 128 :=
  if i < 2 then (s.ymmH r).extractLsb' (128 * i) 128
  else (s.zmmHiH r).extractLsb' (128 * (i - 2)) 128

/-- Write all four lanes of an unmasked EVEX.512 destination. -/
def setZH (s : State) (r : HReg) (l0 l1 l2 l3 : BitVec 128) : State :=
  { s with
    ymmH := fun r' => if r' = r then l1 ++ l0 else s.ymmH r'
    zmmHiH := fun r' => if r' = r then l3 ++ l2 else s.zmmHiH r' }

def setReg (s : State) (r : Reg) (v : BitVec 64) : State :=
  { s with gpr := fun r' => if r' = r then v else s.gpr r' }

/-- Effective address of a memory operand. -/
def ea (s : State) (m : MemOp) : Addr :=
  match m.index with
  | none => s.gpr m.base + BitVec.ofInt 64 m.disp
  | some i => s.gpr m.base + s.gpr i * BitVec.ofNat 64 m.scale + BitVec.ofInt 64 m.disp

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

/-- Load 16 bytes, faulting if not permitted. -/
def load128 (s : State) (a : Addr) : Option (BitVec 128) :=
  if InRegions (s.rd ++ s.wr) a 16 then some (s.mem.readW a 128) else none

/-- Store 16 bytes, faulting if not permitted. -/
def store128 (s : State) (a : Addr) (v : BitVec 128) : Option State :=
  if InRegions s.wr a 16 then some { s with mem := s.mem.writeW a v } else none

def setXmm (s : State) (r : XReg) (v : BitVec 128) : State :=
  { s with xmm := fun r' => if r' = r then v else s.xmm r' }

/-- Load 32 bytes, faulting if not permitted. -/
def load256 (s : State) (a : Addr) : Option (BitVec 256) :=
  if InRegions (s.rd ++ s.wr) a 32 then some (s.mem.readW a 256) else none

/-- Store 32 bytes, faulting if not permitted. -/
def store256 (s : State) (a : Addr) (v : BitVec 256) : Option State :=
  if InRegions s.wr a 32 then some { s with mem := s.mem.writeW a v } else none

/-- Load 64 bytes, faulting if not permitted. -/
def load512 (s : State) (a : Addr) : Option (BitVec 512) :=
  if InRegions (s.rd ++ s.wr) a 64 then some (s.mem.readW a 512) else none

/-- Store 64 bytes, faulting if not permitted. -/
def store512 (s : State) (a : Addr) (v : BitVec 512) : Option State :=
  if InRegions s.wr a 64 then some { s with mem := s.mem.writeW a v } else none

/-- The address of quadword `j` of a 64-byte access at `a`. -/
def qAddr (a : Addr) (j : Nat) : Addr := a + BitVec.ofNat 64 (8 * j)

/-- The quadwords of a 64-byte access that the mask `k` enables: bit `j`
of `k` for quadword `j`, from 0 to 7. -/
def qOn (k : BitVec 64) : List Nat := (List.range 8).filter k.getLsbD

/-- Load the quadwords of the 64 bytes at `a` that `k` enables, the others
0 (zeroing-masking); faults unless each enabled quadword may be read, and
reads no other. -/
def loadMasked512 (s : State) (a : Addr) (k : BitVec 64) : Option (BitVec 512) :=
  if ∀ j ∈ qOn k, InRegions (s.rd ++ s.wr) (qAddr a j) 8 then
    let q (j : Nat) : BitVec 64 := if k.getLsbD j then s.mem.readW (qAddr a j) 64 else 0
    some (q 7 ++ q 6 ++ q 5 ++ q 4 ++ q 3 ++ q 2 ++ q 1 ++ q 0)
  else none

/-- Store the quadwords of `v` that `k` enables to the 64 bytes at `a`,
leaving the others' bytes unchanged (merging-masking); faults unless each
enabled quadword may be written, and writes no other. -/
def storeMasked512 (s : State) (a : Addr) (k : BitVec 64) (v : BitVec 512) : Option State :=
  if ∀ j ∈ qOn k, InRegions s.wr (qAddr a j) 8 then
    some { s with mem := (qOn k).foldl (fun m j => m.writeW (qAddr a j) (v.extractLsb' (64 * j) 64)) s.mem }
  else none

/-- Lane `i` (bits `128i+127:128i`, for `i` 0 or 1) of an AVX register. -/
def lane (s : State) (r : XReg) (i : Nat) : BitVec 128 := if i = 0 then s.xmm r else s.ymmHi r

/-- The 256 bits of an AVX register. -/
def ymm (s : State) (r : XReg) : BitVec 256 := s.ymmHi r ++ s.xmm r

/-- Write a VEX-encoded instruction's result to `r`: lane 0 is `lo`, and
lane 1 is `hi` for 256-bit operands and 0 for 128-bit ones, and bits 511:256
are zeroed (SDM Vol. 1 §14.1.3: "VEX.128 encoded … the upper bits
(MAXVL-1:128) of the destination are zeroed"; the SDM Vol. 2 pseudocode of
each VEX.256 form ends with `DEST[MAXVL-1:256] := 0`). -/
def setV (s : State) (len : VLen) (r : XReg) (lo hi : BitVec 128) : State :=
  { s with
    xmm := fun r' => if r' = r then lo else s.xmm r'
    ymmHi := fun r' => if r' = r then (match len with | .l128 => 0 | .l256 => hi) else s.ymmHi r'
    zmmHi := fun r' => if r' = r then 0 else s.zmmHi r' }

/-- Lane `i` (bits `128i+127:128i`, for `i` from 0 to 3) of an AVX-512
register. -/
def zlane (s : State) (r : XReg) (i : Nat) : BitVec 128 :=
  if i < 2 then s.lane r i else (s.zmmHi r).extractLsb' (128 * (i - 2)) 128

/-- The 512 bits of an AVX-512 register. -/
def zmm (s : State) (r : XReg) : BitVec 512 := s.zmmHi r ++ s.ymm r

/-- Write the result of an EVEX-encoded instruction with 512-bit operands (and
no masking) to `r`, lanes 0 to 3 from `l0` to `l3`. -/
def setZ (s : State) (r : XReg) (l0 l1 l2 l3 : BitVec 128) : State :=
  { s with
    xmm := fun r' => if r' = r then l0 else s.xmm r'
    ymmHi := fun r' => if r' = r then l1 else s.ymmHi r'
    zmmHi := fun r' => if r' = r then l3 ++ l2 else s.zmmHi r' }

/-- Write a 32-bit result, zero-extended to 64 bits (SDM Vol. 1 §3.4.1.1). -/
def setReg32 (s : State) (r : Reg) (v : BitVec 32) : State := s.setReg r (v.setWidth 64)

/-- Set CF, OF, ZF and SF. -/
def setFlags (s : State) (cf of zf sf : Option Bool) : State :=
  { s with cf := cf, of := of, zf := zf, sf := sf }

end State

end VG.X86_64
