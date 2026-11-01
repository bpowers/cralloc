import Cralloc.TCB.X86_64.Target

/-!
# Tests of the x86-64 semantics on concrete states

The model is part of the TCB and transcribes the Intel SDM by hand; these
tests evaluate instructions on concrete states and compare the results with
values worked out from the SDM, so that a change to the semantics shows up
here.
-/

namespace VG.Test.Semantics

open X86_64

/-- A thread block of 32 bytes at `fsBase - 320` (where glibc's rseq area
is in one binary), writable; each byte of memory holds the low byte of its
address; `rdx` holds the block's offset from the FS base, `r10` a word to
store, and the other registers all ones. -/
def st : State where
  gpr r := match r with
    | .rdx => BitVec.ofInt 64 (-320)
    | .r10 => 0x1122334455667788
    | _ => -1
  cf := some true
  zf := some false
  sf := some false
  of := some false
  mem a := a.setWidth 8
  rd := []
  wr := [⟨0x7000 - 320, 32⟩]
  fsBase := 0x7000

/-! ## FS-relative memory operands -/

-- `fs:[rdx+8]` is the byte `0x7000 - 320 + 8 = 0x6EC8` and the seven after it.
#guard (exec (.mov .rax (.mem { base := .rdx, disp := 8, seg := some .fs })) st).map (·.gpr .rax) ==
  some 0xCFCECDCCCBCAC9C8
-- Without the segment, the same operand is the address `-312`, outside every region.
#guard (exec (.mov .rax (.mem { base := .rdx, disp := 8 })) st).isNone
-- Past the end of the block: the access faults.
#guard (exec (.mov .rax (.mem { base := .rdx, disp := 25, seg := some .fs })) st).isNone
#guard (exec (.mov .rax (.mem { base := .rdx, disp := 24, seg := some .fs })) st).isSome
-- A store writes the eight bytes, little-endian, and changes neither the
-- registers, the flags nor the segment base.
#guard match exec (.store { base := .rdx, disp := 8, seg := some .fs } .r10) st with
  | none => false
  | some s => s.mem.readW 0x6EC8 64 == 0x1122334455667788 && s.mem 0x6EC7 == 0xC7 &&
      s.mem 0x6ED0 == 0xD0 && s.fsBase == 0x7000 && s.gpr .rax == -1 && s.cf == some true
/-- `st` with `rcx = -2`. -/
def stIdx : State := st.setReg .rcx (-2)

-- An index register (negative, times 8) and a displacement, FS-relative:
-- `-320 - 16 + 24 = -312`, the same word as `fs:[rdx+8]`.
def fsIdx : MemOp := { base := .rdx, index := some .rcx, scale := 8, disp := 24, seg := some .fs }

#guard (exec (.mov .rax (.mem fsIdx)) stIdx).map (·.gpr .rax) == some 0xCFCECDCCCBCAC9C8

/-! ## 32-bit loads zero-extend

`r8` starts as all ones; a 32-bit load leaves the four bytes in its low half
and zero in its high half. -/

-- `fs:[rdx+24]` is `0x6ED8` to `0x6EDB`.
#guard (exec (.mov32 .r8 (.mem { base := .rdx, disp := 24, seg := some .fs })) st).map (·.gpr .r8) ==
  some 0x00000000DBDAD9D8
-- A 32-bit register move clears the high half too.
#guard (exec (.mov32 .r8 (.reg .r8)) st).map (·.gpr .r8) == some 0x00000000FFFFFFFF
-- The last four bytes of the block, and one past them.
#guard (exec (.mov32 .r8 (.mem { base := .rdx, disp := 28, seg := some .fs })) st).isSome
#guard (exec (.mov32 .r8 (.mem { base := .rdx, disp := 29, seg := some .fs })) st).isNone

/-! ## `lea` -/

/-- The flags of a state, to check that an instruction keeps them. -/
def flags (s : State) : List (Option Bool) := [s.cf, s.zf, s.sf, s.of]

-- The offset, modulo 2⁶⁴, with no memory access (the address is outside
-- every region) and the flags unchanged.
#guard match exec (.lea .rax { base := .rdx, disp := 8 }) st with
  | none => false
  | some s => s.gpr .rax == BitVec.ofInt 64 (-312) && flags s == flags st
-- Base, index times scale, and a negative displacement: `-320 + (-2) * 8 - 8`.
#guard (exec (.lea .rcx { base := .rdx, index := some .rcx, scale := 8, disp := -8 }) stIdx).map
  (·.gpr .rcx) == some (BitVec.ofInt 64 (-344))
-- Wrapping: `(2⁶⁴ - 1) + 1 = 0`.
#guard (exec (.lea .rax { base := .rax, disp := 1 }) st).map (·.gpr .rax) == some 0

/-- `st` with a static `T` at `0x1000`. -/
def stSym : State := { st with syms := fun n => if n = "T" then 0x1000 else 0 }

#guard (exec (.leaSym .r9 "T") stSym).map (·.gpr .r9) == some 0x1000
#guard (exec (.leaSym .r9 "T" 24) stSym).map (·.gpr .r9) == some 0x1018
#guard (exec (.leaSym .r9 "T" (-8)) stSym).map (·.gpr .r9) == some 0xFF8
#guard (exec (.leaSym .r9 "T" 24) stSym).map flags == some (flags st)

/-! ## Compile-time constants and the global offset table -/

/-- `stSym` with the constants `off = 0x30` and `mask = 0x1_0000_01f0` (whose
low 32 bits are `0x1f0`). -/
def stPar : State := { stSym with params := fun n =>
  if n = "off" then 0x30 else if n = "mask" then 0x1000001f0 else 0 }

-- A symbol plus a literal and a constant offset.
#guard (exec (.leaSym .r9 "T" 8 (some "off")) stPar).map (·.gpr .r9) == some 0x1038
-- An immediate from a constant: its low 32 bits, as an immediate is.
#guard (exec (.alu32 .test .rcx (.immP "mask")) (stPar.setReg .rcx 0x10008)).map flags ==
  some [some false, some true, some false, some false]
#guard (exec (.mov32 .rax (.immP "mask")) stPar).map (·.gpr .rax) == some 0x1f0
-- A 64-bit operation sign-extends the 32-bit immediate.
#guard (exec (.mov .rax (.immP "mask")) { stPar with params := fun _ => 0x80000000 }).map
  (·.gpr .rax) == some 0xffffffff80000000
-- Instructions without an immediate form refuse one.
#guard (exec (.cmov .e .rax (.immP "mask")) stPar).isNone
-- The GOT slot of a symbol holds its address; nothing is read from the regions.
#guard (exec (.movGot .rdx "T") stPar).map (·.gpr .rdx) == some 0x1000
-- A direct load of a symbol reads the 8 bytes at its address, only within the
-- regions.
#guard (exec (.movSym .rdx "T") { stPar with
    mem := fun a => if a = 0x1000 then 0x2a else 0, rd := [⟨0x1000, 8⟩] }).map (·.gpr .rdx) ==
  some 0x2a
#guard (exec (.movSym .rdx "T") { stPar with rd := [⟨0x1001, 8⟩] }).isNone
-- The descriptor's address is the state's.
#guard (exec (.leaRseqCs .r10) { st with csDesc := 0x4000 }).map (·.gpr .r10) == some 0x4000

/-! ## The body of a restartable sequence

An exit whose condition holds runs its code and continues after the
sequence, or ends at its external target; one whose condition does not hold
falls through; one on an undefined flag faults. -/

def body : List (Step Instr Cond) :=
  [.instr (.mov .rax (.imm 1)), .exit .e [.mov .rax (.imm 2)] none, .exit .b [] (some "slow"),
    .instr (.mov .rax (.imm 3))]

/-- What a run of `body` ends with: whether it exited to `slow`, and `rax`. -/
def outcome (s : State) : Option (Bool × BitVec 64) :=
  (runBody isa body s).map fun | .done s => (false, s.gpr .rax) | .exit _ s => (true, s.gpr .rax)

#guard outcome { st with zf := some true } == some (false, 2)
#guard outcome { st with zf := some false, cf := some true } == some (true, 1)
#guard outcome { st with zf := some false, cf := some false } == some (false, 3)
#guard outcome { st with zf := none } == none

/-- `rax` in each state in which the body may be aborted: before each step,
until an exit is taken, up to and including before its last step. -/
def aborts (s : State) : List (BitVec 64) := (bodyPrefixes isa body s).map (·.gpr .rax)

#guard aborts { st with zf := some true } == [-1, 1]
#guard aborts { st with zf := some false, cf := some true } == [-1, 1, 1]
#guard aborts { st with zf := some false, cf := some false } == [-1, 1, 1, 1]
/-! The memory accesses of a sequence: the arm's store, the body's load,
and its commit, each with its value; an exit taken ends them. -/

def evSeq : Prog isa := .rseq
  [.store { base := .rdx, disp := 8, seg := some .fs } .r10]
  [.instr (.mov32 .r8 (.mem { base := .rdx, disp := 24, seg := some .fs })),
    .exit .e [] none,
    .instr (.store { base := .rdx, disp := 16, seg := some .fs } .r8)]

#guard Code.events isa evSeq { st with zf := some false } ==
  [⟨0x6EC8, 8, true, 0x1122334455667788⟩, ⟨0x6ED8, 4, false, 0xDBDAD9D8⟩,
    ⟨0x6ED0, 8, true, 0xDBDAD9D8⟩]
#guard Code.events isa evSeq { st with zf := some true } ==
  [⟨0x6EC8, 8, true, 0x1122334455667788⟩, ⟨0x6ED8, 4, false, 0xDBDAD9D8⟩]

-- A fault ends the run, and with it the states.
#guard aborts { st with zf := none } == [-1, 1]
-- Nowhere after the arm faults.
#guard (Code.abortStates isa (.rseq [.mov .rax (.mem { base := .rax })] body) st).isEmpty
#guard (Code.abortStates isa (.rseq [.mov .rax (.imm 7)] body) { st with zf := some true }).map
  (·.gpr .rax) == [7, 1]

/-! ## `inc`, `dec` and `test` with an immediate

`flags` lists CF, ZF, SF and OF. `inc` and `dec` keep CF (set in `st`) and
set the others by the result; `test` clears CF and OF and writes no
register. -/

/-- `st` with `rax = v`. -/
def withRax (v : BitVec 64) : State := st.setReg .rax v

-- -1 + 1 = 0.
#guard (exec (.inc .rax) st).map (fun s => (s.gpr .rax, flags s)) ==
  some (0, [some true, some true, some false, some false])
-- 2⁶³ - 1 + 1 overflows to -2⁶³.
#guard (exec (.inc .rax) (withRax 0x7fffffffffffffff)).map (fun s => (s.gpr .rax, flags s)) ==
  some (0x8000000000000000, [some true, some false, some true, some true])
-- 0 - 1 = -1, without overflow.
#guard (exec (.dec .rax) (withRax 0)).map (fun s => (s.gpr .rax, flags s)) ==
  some (-1, [some true, some false, some true, some false])
-- -2⁶³ - 1 overflows to 2⁶³ - 1.
#guard (exec (.dec .rax) (withRax 0x8000000000000000)).map (fun s => (s.gpr .rax, flags s)) ==
  some (0x7fffffffffffffff, [some true, some false, some false, some true])
-- 1 - 1 = 0.
#guard (exec (.dec .rax) (withRax 1)).map (fun s => (s.gpr .rax, flags s)) ==
  some (0, [some true, some true, some false, some false])

/-- `st` with `rcx = v`. -/
def withRcx (v : BitVec 64) : State := st.setReg .rcx v

-- The low 32 bits of `rcx` against `0x1f0`: some bit in common, then none.
#guard (exec (.alu32 .test .rcx (.imm 0x1f0)) st).map (fun s => (s.gpr .rcx, flags s)) ==
  some (-1, [some false, some false, some false, some false])
#guard (exec (.alu32 .test .rcx (.imm 0x1f0)) (withRcx 0xffffffff00010008)).map flags ==
  some [some false, some true, some false, some false]
-- The 32-bit form's sign is bit 31 of the result.
#guard (exec (.alu32 .test .rcx (.imm 0x80000000)) st).map flags ==
  some [some false, some false, some true, some false]
-- The 64-bit form sign-extends its immediate: `0xfffffff0` tests bits 63:4.
#guard (exec (.alu .test .rax (.imm 0xfffffff0)) (withRax 0x8000000000000005)).map flags ==
  some [some false, some false, some true, some false]
#guard (exec (.alu .test .rax (.imm 0xfffffff0)) (withRax 0xf)).map flags ==
  some [some false, some true, some false, some false]

/-! ## `movzx r32, r16` and `cmp r8, m8` -/

-- The low 16 bits of `rax` (all ones), zero-extended; the flags unchanged.
#guard (exec (.movzx16 .rcx .rax) st).map (fun s => (s.gpr .rcx, flags s)) ==
  some (0xffff, flags st)
#guard (exec (.movzx16 .rcx .rax) (withRax 0x8000000012345678)).map (·.gpr .rcx) == some 0x5678
-- `cl` (0xff) against the byte at `fs:[rdx+8]`, 0xC8: equal no, below no, sign of 0x37 clear.
#guard (exec (.cmp8 .rcx { base := .rdx, disp := 8, seg := some .fs }) st).map
    (fun s => (s.gpr .rcx, flags s)) ==
  some (-1, [some false, some false, some false, some false])
-- 0xC8 against itself: equal.
#guard (exec (.cmp8 .rcx { base := .rdx, disp := 8, seg := some .fs }) (withRcx 0xC8)).map flags ==
  some [some false, some true, some false, some false]
-- 0x01 against 0xC8: a borrow, sign of 0x39 clear, no signed overflow (1 - (-56) = 57).
#guard (exec (.cmp8 .rcx { base := .rdx, disp := 8, seg := some .fs }) (withRcx 0x101)).map flags ==
  some [some true, some false, some false, some false]
-- Outside every region the byte load faults.
#guard (exec (.cmp8 .rcx { base := .rdx, disp := 8 }) st).isNone

end VG.Test.Semantics
