import Asmalloc.TCB.X86_64.Isa

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

end VG.Test.Semantics
