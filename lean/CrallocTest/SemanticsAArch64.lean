import Cralloc.TCB.AArch64.Target

/-!
# Tests of the AArch64 semantics on concrete states

The model is part of the TCB and transcribes the Arm ARM by hand; these tests
evaluate instructions on concrete states and compare the results with values
worked out from the manual, so that a change to the semantics shows up here.
`VectorsAArch64.lean` and `scripts/check_aarch64.py` compare the same model with
the processor, where one is at hand.
-/

namespace VG.Test.SemanticsAArch64

open VG.AArch64

/-- A block of 64 bytes at `0x7000`, writable; each byte of memory holds the low
byte of its address; `x10` holds the block's address, `x2` a word to store, and
the other registers all ones. -/
def st : State where
  gpr r := match r with
    | .x10 => 0x7000
    | .x2 => 0x1122334455667788
    | _ => -1
  nf := true
  zf := false
  cf := true
  vf := false
  mem a := a.setWidth 8
  rd := []
  wr := [⟨0x7000, 64⟩]
  tpidr := 0xABC000
  syms n := if n = "s" then 0x12345678 else 0
  csDesc := 0x9000

/-- The flags of a state, as `NZCV`. -/
def flags (s : State) : List Bool := [s.nf, s.zf, s.cf, s.vf]

def stFlags : List Bool := flags st

/-- `st` with register `r` holding `v`. -/
def withReg (r : Reg) (v : BitVec 64) : State := st.setReg r v

/-! ## Loads and stores -/

-- A 64-bit load, little-endian, the flags unchanged.
#guard (exec (.ldr .x .x0 { base := .x10, off := .imm 8 }) st).map (fun s => (s.gpr .x0, flags s)) ==
  some (0x0F0E0D0C0B0A0908, stFlags)
-- A 32-bit load zero-extends: `x0` starts as all ones.
#guard (exec (.ldr .w .x0 { base := .x10, off := .imm 4 }) st).map (·.gpr .x0) == some 0x07060504
-- So does a byte load.
#guard (exec (.ldr .b .x0 { base := .x10, off := .imm 63 }) st).map (·.gpr .x0) == some 0x3F
-- A negative offset, and the one past the block's start, faults.
#guard (exec (.ldr .x .x0 { base := .x10, off := .imm (-8) }) st).isNone
#guard (exec (.ldr .x .x0 { base := .x10, off := .imm 56 }) st).isSome
#guard (exec (.ldr .x .x0 { base := .x10, off := .imm 57 }) st).isNone
-- A register offset, and a scaled one: `[x10, x3, lsl #3]` with `x3 = 2` is `+16`.
#guard (exec (.ldr .x .x0 { base := .x10, off := .reg .x3 3 }) (withReg .x3 2)).map (·.gpr .x0) ==
  some 0x1716151413121110
#guard (exec (.ldr .x .x0 { base := .x10, off := .reg .x3 }) (withReg .x3 2)).map (·.gpr .x0) ==
  some 0x0908070605040302
-- `:lo12:` adds the low 12 bits of the address only: `0x678` of `s`.
#guard (exec (.ldr .b .x0 { base := .x3, off := .lo12 (.sym "s") }) (withReg .x3 (0x7000 - 0x678))).map
    (·.gpr .x0) == some 0
-- A store writes the low bytes of the register, the rest of memory unchanged.
#guard match exec (.str .x .x2 { base := .x10, off := .imm 8 }) st with
  | none => false
  | some s => s.mem.readW 0x7008 64 == 0x1122334455667788 && s.mem 0x7007 == 0x07 &&
      s.mem 0x7010 == 0x10 && s.gpr .x2 == st.gpr .x2 && flags s == stFlags
#guard match exec (.str .w .x2 { base := .x10, off := .imm 8 }) st with
  | none => false
  | some s => s.mem.readW 0x7008 32 == 0x55667788 && s.mem 0x700C == 0x0C
#guard match exec (.str .b .x2 { base := .x10, off := .imm 8 }) st with
  | none => false
  | some s => s.mem 0x7008 == 0x88 && s.mem 0x7009 == 0x09
-- A store to a region only read faults.
#guard (exec (.str .x .x2 { base := .x10 }) { st with rd := st.wr, wr := [] }).isNone
#guard (exec (.ldr .x .x0 { base := .x10 }) { st with rd := st.wr, wr := [] }).isSome
-- `stp xzr, xzr`: both words, or a fault if either is out of bounds.
#guard match exec (.stpz .x10 16) st with
  | none => false
  | some s => s.mem.readW 0x7010 64 == 0 && s.mem.readW 0x7018 64 == 0 && s.mem 0x7020 == 0x20
#guard (exec (.stpz .x10 48) st).isSome
#guard (exec (.stpz .x10 49) st).isNone

/-! ## The memory accesses

One event for each, with the value read from the first state, or written to the
second. -/

#guard match exec (.ldr .w .x0 { base := .x10, off := .imm 4 }) st with
  | some s => Instr.events (.ldr .w .x0 { base := .x10, off := .imm 4 }) st s ==
      [⟨0x7004, 4, false, 0x07060504⟩]
  | none => false
#guard match exec (.str .x .x2 { base := .x10 }) st with
  | some s => Instr.events (.str .x .x2 { base := .x10 }) st s ==
      [⟨0x7000, 8, true, 0x1122334455667788⟩]
  | none => false
#guard match exec (.stpz .x10 0) st with
  | some s => Instr.events (.stpz .x10 0) st s == [⟨0x7000, 8, true, 0⟩, ⟨0x7008, 8, true, 0⟩]
  | none => false
#guard Instr.events (.add .x .x0 .x0 (.imm 1)) st st == []

/-! ## Arithmetic and logic

No flags: `st`'s stay. A 32-bit result clears the high half. -/

#guard (exec (.add .x .x0 .x10 (.imm 8)) st).map (fun s => (s.gpr .x0, flags s)) == some (0x7008, stFlags)
#guard (exec (.add .x .x0 .x10 (.reg .x2)) st).map (·.gpr .x0) == some 0x112233445566E788
#guard (exec (.add .x .x0 .x10 (.lsl .x3 12)) (withReg .x3 2)).map (·.gpr .x0) == some 0x9000
-- Wraps at the operand size, and a 32-bit result is zero-extended.
#guard (exec (.add .x .x0 .x1 (.imm 1)) st).map (·.gpr .x0) == some 0
#guard (exec (.add .w .x0 .x1 (.imm 1)) st).map (·.gpr .x0) == some 0
#guard (exec (.add .w .x0 .x2 (.imm 0)) st).map (·.gpr .x0) == some 0x55667788
#guard (exec (.sub .x .x0 .x10 (.imm 8)) st).map (·.gpr .x0) == some 0x6FF8
#guard (exec (.and .x .x0 .x2 (.imm 0xff8)) st).map (fun s => (s.gpr .x0, flags s)) == some (0x788, stFlags)
#guard (exec (.orr .x .x0 .x10 (.imm 1)) st).map (·.gpr .x0) == some 0x7001
#guard (exec (.mov .x .x0 (.imm 2)) st).map (·.gpr .x0) == some 2
#guard (exec (.mov .w .x0 (.reg .x1)) st).map (·.gpr .x0) == some 0xFFFFFFFF
#guard (exec (.mov .x .x0 (.reg .x10)) st).map (·.gpr .x0) == some 0x7000
#guard (exec (.lsl .x .x0 .x10 4) st).map (·.gpr .x0) == some 0x70000
#guard (exec (.lsl .w .x0 .x1 4) st).map (·.gpr .x0) == some 0xFFFFFFF0
#guard (exec (.lsr .x .x0 .x10 4) st).map (·.gpr .x0) == some 0x700
#guard (exec (.lsr .w .x0 .x1 28) st).map (·.gpr .x0) == some 0xF
-- A count in a register is taken modulo the operand size.
#guard (exec (.lsrv .x .x0 .x10 .x3) (withReg .x3 68)).map (·.gpr .x0) == some 0x700
#guard (exec (.lslv .w .x0 .x10 .x3) (withReg .x3 36)).map (·.gpr .x0) == some 0x70000
#guard (exec (.mul .x .x0 .x10 .x3) (withReg .x3 2)).map (·.gpr .x0) == some 0xE000
#guard (exec (.mul .w .x0 .x1 .x1) st).map (·.gpr .x0) == some 1
-- The high half of the product: none below 2^64, and of all ones squared, all ones but the low bit.
#guard (exec (.umulh .x0 .x10 .x10) st).map (·.gpr .x0) == some 0
#guard (exec (.umulh .x0 .x1 .x1) st).map (·.gpr .x0) == some 0xFFFFFFFFFFFFFFFE
-- `calloc`'s check: 2^60 + 1 times 16 has 16 in its low word, and 1 in its high.
#guard (exec (.umulh .x0 .x3 .x4) ((withReg .x3 0x1000000000000001).setReg .x4 16)).map (·.gpr .x0) == some 1
#guard (exec (.mul .x .x0 .x3 .x4) ((withReg .x3 0x1000000000000001).setReg .x4 16)).map (·.gpr .x0) == some 16

/-! ## Compares

DDI 0487 J1, `AddWithCarry`: of `n`, `NOT(m)` and a carry of 1. C is "no
borrow", V the signed overflow. The flags alone: the registers stay. -/

def cmpFlags (a b : BitVec 64) : Option (List Bool) :=
  (exec (.cmp .x .x3 (.reg .x4)) ((withReg .x3 a).setReg .x4 b)).map flags

def cmp32Flags (a b : BitVec 64) : Option (List Bool) :=
  (exec (.cmp .w .x3 (.reg .x4)) ((withReg .x3 a).setReg .x4 b)).map flags

-- Equal: Z and C (no borrow).
#guard cmpFlags 5 5 == some [false, true, true, false]
-- Above: C, unsigned; positive.
#guard cmpFlags 7 5 == some [false, false, true, false]
-- Below: borrow, negative.
#guard cmpFlags 5 7 == some [true, false, false, false]
-- The largest negative minus one overflows.
#guard cmpFlags 0x8000000000000000 1 == some [false, false, true, true]
-- Zero against all ones: a borrow, and the result 1.
#guard cmpFlags 0 (-1) == some [false, false, false, false]
-- 32 bits look at the low halves alone.
#guard cmp32Flags 0xFFFFFFFF00000005 5 == some [false, true, true, false]
#guard cmp32Flags 0x80000000 1 == some [false, false, true, true]
-- The CPU check: `-1` and `-2` are above the slab count, and a number below it is
-- not (`b.hs` taken on C).
#guard (exec (.cmp .w .x8 (.imm 8192)) (withReg .x8 0xFFFFFFFF)).map (·.cf) == some true
#guard (exec (.cmp .w .x8 (.imm 8192)) (withReg .x8 0xFFFFFFFE)).map (·.cf) == some true
#guard (exec (.cmp .w .x8 (.imm 8192)) (withReg .x8 8191)).map (·.cf) == some false
#guard (exec (.cmp .w .x8 (.imm 8192)) (withReg .x8 8192)).map (·.cf) == some true
-- A compare leaves its registers alone.
#guard (exec (.cmp .x .x3 (.imm 1)) st).map (·.gpr .x3) == some (-1)

/-! ## Tests

N and Z of the AND, C and V cleared. -/

#guard (exec (.tst .x .x1 (.reg .x1)) st).map flags == some [true, false, false, false]
#guard (exec (.tst .w .x1 (.reg .x1)) st).map flags == some [true, false, false, false]
#guard (exec (.tst .x .x10 (.imm 0xfff)) st).map flags == some [false, true, false, false]
-- Bit 32 of a word is out of a 32-bit test.
#guard (exec (.tst .w .x3 (.reg .x3)) (withReg .x3 0x100000000)).map flags ==
  some [false, true, false, false]

/-! ## Conditions -/

/-- `st` with the flags `NZCV`. -/
def withFlags (n z c v : Bool) : State := st.setFlags n z c v

#guard eval .eq (withFlags false true false false) == some true
#guard eval .ne (withFlags false true false false) == some false
#guard eval .hs (withFlags false false true false) == some true
#guard eval .lo (withFlags false false true false) == some false
-- HI is C and not Z, LS its opposite.
#guard eval .hi (withFlags false false true false) == some true
#guard eval .hi (withFlags false true true false) == some false
#guard eval .ls (withFlags false true true false) == some true
#guard eval .ls (withFlags false false false false) == some true
#guard eval .ls (withFlags false false true false) == some false
-- The compare-and-branches read the register at their size.
#guard eval (.cbz .x .x3) (withReg .x3 0x100000000) == some false
#guard eval (.cbz .w .x3) (withReg .x3 0x100000000) == some true
#guard eval (.cbnz .w .x3) (withReg .x3 0x100000000) == some false
#guard eval (.cbnz .x .x3) (withReg .x3 0x100000000) == some true
-- The test-and-branches read their bit.
#guard eval (.tbnz .x3 0) (withReg .x3 1) == some true
#guard eval (.tbz .x3 0) (withReg .x3 1) == some false
#guard eval (.tbnz .x3 63) (withReg .x3 0x8000000000000000) == some true
#guard eval (.tbnz .x3 62) (withReg .x3 0x8000000000000000) == some false

/-! ## Symbols and the thread pointer -/

-- `s` is at `0x12345678`: its page, and the low 12 bits.
#guard page 0x12345678 == 0x12345000
#guard lo12 0x12345678 == 0x678
#guard page 0x12345000 == 0x12345000
#guard lo12 0x12345000 == 0
#guard (exec (.adrp .x0 (.sym "s")) st).map (·.gpr .x0) == some 0x12345000
#guard (exec (.adrp .x0 (.sym "s" 0x1000)) st).map (·.gpr .x0) == some 0x12346000
-- An offset into the next page moves the page, and is the page's low 12 bits.
#guard (exec (.adrp .x0 (.sym "s" 0x1000)) st).map (·.gpr .x0) == some 0x12346000
#guard (exec (.addLo12 .x0 .x0 (.sym "s")) (withReg .x0 0x12345000)).map (·.gpr .x0) == some 0x12345678
-- A negative offset goes down.
#guard (exec (.addLo12 .x0 .x3 (.sym "s" (-0x679))) (withReg .x3 0x12345000)).map (·.gpr .x0) ==
  some 0x12345fff
-- The pair makes the address, of a symbol and of the descriptor.
#guard (do
    let s₁ ← exec (.adrp .x11 .desc) { st with csDesc := 0x9860 }
    let s₂ ← exec (.addLo12 .x11 .x11 .desc) s₁
    pure (s₁.gpr .x11, s₂.gpr .x11)) == some (0x9000, 0x9860)
#guard (exec (.mrsTp .x10) st).map (·.gpr .x10) == some 0xABC000
-- The word after the descriptor: only from a register holding its address.
#guard (exec (.ldrWord .x9 .x11 "s") (withReg .x11 0x9000)).map (·.gpr .x9) == some 0x12345678
#guard (exec (.ldrWord .x9 .x11 "s") st).isNone
#guard (exec (.movGot .x9 "s") st).map (·.gpr .x9) == some 0x12345678

/-! ## Calls and frames are not modelled -/

#guard (isa.call st).isNone
#guard (isa.push (.str .x .x1 { base := .x2 }) st).isNone
#guard isa.writesSp (.add .x .x0 .x0 (.imm 1)) == false

/-! ## What an abort keeps, and what arming is -/

-- The arming store, in the state a setup leaves: `rseq_cs` holds the descriptor.
def armState : State :=
  { st with
    gpr := fun r => if r = .x10 then 0x7010 else if r = .x11 then 0x9000 else 0
    syms := fun n => if n = rseqOffsetSym then 0x7000 else 0
    tpidr := 0x7010 - 0x0706050403020100
    wr := [⟨0x7018, 8⟩] }

#guard rseqArea armState == 0x7010
#guard match exec (.str .x .x11 { base := .x10, off := .imm 8 }) armState with
  | none => false
  | some s => s.mem.readW (rseqCsAddr s) 64 == s.csDesc && rseqCsAddr s == 0x7018

end VG.Test.SemanticsAArch64
