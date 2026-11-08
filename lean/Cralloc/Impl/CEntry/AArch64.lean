module

public import Cralloc.Impl.Slab.AArch64

/-! # The C entry points on AArch64

docs/research/firestorm.md, with the front half inside the sequence, as on x86-64. A routine's
setup forms the addresses of the tables it reads (`mallocTables` and its kin), then finds the
sequence's registers as every routine does (`setup`), and its arm is one store. So a retry forms
no address: it reads `x10`, `x11`, the arguments and the tables' registers, which the body writes
none of.

Every routine reads the CPU number and leaves for the slow path if it has no slab (`cpuC`), finds
its class into `x5` and its slab into `x3` (`slabOf`), in the order that leaves no ALU instruction
between a table's load and the hot word's, which is what a free's commit waits on, and ends with
the pop's or push's steps through `[x3, x5, lsl #3]` (`popSteps`, `pushSteps`). Every exit
tail-calls a slow path with the arguments as they arrived, through a stub after the routine's
return, in its own section.

`x13` holds `SLABS`, which starts on a page; `x14` the page past the slabs, whose words 1024 and
1032 bytes in are the published shift and base index; `x15` the masks' table, 2048 bytes into that
page; and a free's `x12` the region table, a page past the masks. A table's page offsets are
none of a slab word's. No `adrp` is in a body, or among the last instructions of a setup
(`Instr.notInBody`). -/

@[expose] public section

namespace VG.Impl.CEntry.AArch64

open VG.AArch64 VG.Impl.Slab.AArch64

/-- The slow path's name, an operand of each routine. -/
def slow : Option String := some "slow"

/-! ## The setups -/

/-- The slabs' address, from its page alone. -/
def slabsAddr : Instr := .adrp .x13 (.sym "SLABS")

/-- The page past the slabs, into `x14`, and the masks' table into `x15`. -/
def masksAddr : List Instr := [.adrp .x14 (.sym "SLABS" 33554432), .add .x .x15 .x14 (.imm 2048)]

/-- A size table's first entry, 2048 bytes into its static, into `r`. -/
def tableAddr (r : Reg) (name : String) : List Instr :=
  [.adrp r (.sym name 2048), .addLo12 r r (.sym name 2048)]

def mallocTables : List Instr := tableAddr .x12 "C_SIZES" ++ slabsAddr :: masksAddr
def callocTables : List Instr := tableAddr .x12 "C_CALLOC_SIZES" ++ slabsAddr :: masksAddr
/-- A free's tables: the region table too, into `x12`. -/
def freeTables : List Instr := slabsAddr :: masksAddr ++ [.add .x .x12 .x15 (.imm 4096)]
/-- The sized free's size table is in `x2`, there being no third argument for it to hold. -/
def freeSizedTables : List Instr := tableAddr .x2 "C_SIZED_SIZES" ++ freeTables

/-- Each routine's setup: its tables, then what every routine runs. -/
def mallocSetup : List Instr := mallocTables ++ setup
def callocSetup : List Instr := callocTables ++ setup
def freeSetup : List Instr := freeTables ++ setup
def freeSizedSetup : List Instr := freeSizedTables ++ setup

/-- Each on musl (`muslSetup`). -/
def mallocMuslSetup : List Instr := mallocTables ++ muslSetup
def callocMuslSetup : List Instr := callocTables ++ muslSetup
def freeMuslSetup : List Instr := freeTables ++ muslSetup
def freeSizedMuslSetup : List Instr := freeSizedTables ++ muslSetup

/-! ## The steps -/

/-- The CPU number into `x8`, and to the slow path if it has no slab. -/
def cpuC : List (Step Instr Cond) := [
  .instr (.ldr .w .x8 { base := .x10, off := .imm 4 }),
  .instr (.cmp .w .x8 (.imm 8192)),
  .exit .hs [] slow]

/-- The CPU's slab into `x3`. -/
def slabOf : List (Step Instr Cond) := [
  .instr (.lsl .x .x8 .x8 12),
  .instr (.add .x .x3 .x13 (.reg .x8))]

/-- `[x3, x5, lsl #3]`: the word of the class in `x5` in the CPU's slab. -/
def slotWord : MemOp := { base := .x3, off := .reg .x5 3 }

/-- `[x15, x5, lsl #3]`: the class's entry in the masks' table. -/
def maskEntry : MemOp := { base := .x15, off := .reg .x5 3 }

/-- A pop, with the word in `w` and the block in `x4`, for after the commit, since a restart reads
the arguments. -/
def popSteps (w : Reg) : List (Step Instr Cond) := [
  .instr (.ldr .x w slotWord),
  .instr (.ldr .x .x6 maskEntry),
  .instr (.tst .w w (.reg .x6)),
  .exit .eq [] slow,
  .instr (.ldr .x .x4 { base := w, off := .imm (-8) }),
  .instr (.sub .x w w (.imm 8)),
  .instr (.str .x w slotWord)]

/-- A push of `x0`, with the word in `w` and the new word in `n`. -/
def pushSteps (w n : Reg) : List (Step Instr Cond) := [
  .instr (.ldr .x w slotWord),
  .instr (.ldr .x .x6 maskEntry),
  .instr (.add .x n w (.imm 8)),
  .instr (.tst .w n (.reg .x6)),
  .exit .eq [] slow,
  .instr (.str .x .x0 { base := w }),
  .instr (.str .x n slotWord)]

/-- `malloc`'s steps that find the class. -/
def mallocFront : List (Step Instr Cond) := [
  .instr (.cmp .x .x0 (.imm 32768)),
  .exit .hi [] slow,
  .instr (.add .x .x2 .x0 (.imm 7)),
  .instr (.lsr .x .x2 .x2 3),
  .instr (.ldr .b .x5 { base := .x12, off := .reg .x2 })]

/-- `calloc`'s steps that find the class: the slow path if the product overflows or is above
1024. -/
def callocFront : List (Step Instr Cond) := [
  .instr (.umulh .x2 .x0 .x1),
  .exit (.cbnz .x .x2) [] slow,
  .instr (.mul .x .x9 .x0 .x1),
  .instr (.cmp .x .x9 (.imm 1024)),
  .exit .hi [] slow,
  .instr (.ldr .b .x5 { base := .x12, off := .reg .x9 })]

/-- The region index of the pointer in `x0` into `x4`, from the mirror's words, and to the slow
path if it is out of range. There is one mirror, C's (`Spec.CEntry.Mirror`). -/
def regionIndex : List (Step Instr Cond) := [
  .instr (.ldr .w .x6 { base := .x14, off := .imm 1024 }),
  .instr (.ldr .x .x7 { base := .x14, off := .imm 1032 }),
  .instr (.lsrv .x .x4 .x0 .x6),
  .instr (.sub .x .x4 .x4 (.reg .x7)),
  .instr (.cmp .x .x4 (.imm 4096)),
  .exit .hs [] slow]

/-- `free` of null returns. -/
def freeNull : List (Step Instr Cond) := [.exit (.cbz .x .x0) [] none]

/-- `free`'s class: the region's byte. -/
def freeByte : List (Step Instr Cond) := [.instr (.ldr .b .x5 { base := .x12, off := .reg .x4 })]

/-- The sized free's class: the byte for the size's low 16 bits. -/
def sizedPredict : List (Step Instr Cond) := [
  .instr (.and .x .x9 .x1 (.imm 0xffff)),
  .instr (.ldr .b .x5 { base := .x2, off := .reg .x9 })]

/-- The region's byte must confirm it. -/
def sizedConfirm : List (Step Instr Cond) := [
  .instr (.ldr .b .x7 { base := .x12, off := .reg .x4 }),
  .instr (.cmp .w .x5 (.reg .x7)),
  .exit .ne [] slow]

/-! ## The routines -/

/-- `c_malloc(size)`: the block leaves for `x0` only after the sequence, since a restart reads the
size there. -/
def cMalloc : Prog isa :=
  .seq (.rseq arm (cpuC ++ mallocFront ++ slabOf ++ popSteps .x1)) (.block [.mov .x .x0 (.reg .x4)])

/-- `c_calloc(n, size)`: the pop, then, after the commit and outside the sequence, the block into
`x2` and a jump to the zeroing tail, with `n` and `size` as they arrived. -/
def cCalloc : Prog isa :=
  .seq (.rseq arm (cpuC ++ callocFront ++ slabOf ++ popSteps .x2)) (.jump [.mov .x .x2 (.reg .x4)] "zero")

/-- `c_free(ptr)`. -/
def cFree : Prog isa :=
  .rseq arm (cpuC ++ freeNull ++ slabOf ++ regionIndex ++ freeByte ++ pushSteps .x1 .x2)

/-- `c_free_sized(ptr, size)`: `x1` is the size a restart reads and `x2` its table, so the hot word
is in `x9` and the new word in `x4`. -/
def cFreeSized : Prog isa :=
  .rseq arm (cpuC ++ slabOf ++ sizedPredict ++ regionIndex ++ sizedConfirm ++ pushSteps .x9 .x4)

end VG.Impl.CEntry.AArch64
