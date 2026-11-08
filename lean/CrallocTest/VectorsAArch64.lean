import CrallocTest.PrintAArch64

/-!
# What the model says, for the processor to check

`lake env lean --run CrallocTest/VectorsAArch64.lean` prints, for
`scripts/check_aarch64.py`:

* `G <encoding> <text>`: the golden instructions (`golden`).
* `L <bits> <value> <0|1>`: logical immediates and whether the printer accepts
  each, for the assembler to agree.
* `T <id> <kind> <line>;<line>…`: a test, its instruction's lines as printed.
* `V <id> <nzcv> <regs> | <nzcv> <regs> | <addr>:<byte>,…`: a vector of the test:
  the flags and registers it starts with, those the model ends with, and the bytes
  of memory the model changed, every number in hexadecimal. A branch's test
  (`kind` B) ends with `x17` holding 1 if it is taken.

The script runs each test on the processor with each vector's registers, and
requires what the model says, in memory laid out as it is here (`pat`, `BUF`).
-/

namespace VG.Test.VectorsAArch64

open VG.AArch64
open VG.Test.AArch64 (text lines golden)

/-- Memory the tests may read and write: 64 KiB, at an address the script maps. -/
def BUF : Addr := 0x7000000000
def BUF_LEN : Nat := 0x10000

/-- Where the script links the symbol `vg_sym`, the routine's descriptor, and
the value the thread pointer holds. -/
def SYM : Addr := 0x20001238
def DESC : Addr := 0x20002460
def TP : Addr := 0x7654321000

/-- A byte of memory, as a function of its address; the script fills `BUF` alike. -/
def pat (a : Addr) : Byte :=
  BitVec.ofNat 8 ((a.toNat * 0x9d + a.toNat / 256 * 0x3b) % 251)

def allRegs : List Reg := [.x0, .x1, .x2, .x3, .x4, .x5, .x6, .x7, .x8, .x9, .x10, .x11, .x12, .x13,
  .x14, .x15, .x16, .x17, .x19, .x20, .x21, .x22, .x23, .x24, .x25, .x26, .x27, .x28, .x30]

/-- Each register's value, unless `inp` says: distinct for every register. -/
def regsOf (inp : List (Reg × BitVec 64)) (r : Reg) : BitVec 64 :=
  match inp.lookup r with
  | some v => v
  | none => BitVec.ofNat 64 (0x0101010101010101 * ((allRegs.findIdx? (· == r)).getD 0 + 1))

def st (inp : List (Reg × BitVec 64)) (nzcv : Nat) : State where
  gpr := regsOf inp
  sp := 0
  nf := nzcv.testBit 3
  zf := nzcv.testBit 2
  cf := nzcv.testBit 1
  vf := nzcv.testBit 0
  mem := pat
  rd := []
  wr := [⟨BUF, BUF_LEN⟩]
  syms n := if n = "vg_sym" then SYM else 0
  tpidr := TP
  csDesc := DESC

def hex (n : Nat) : String := String.ofList (Nat.toDigits 16 n)

def nzcvOf (s : State) : Nat :=
  (if s.nf then 8 else 0) + (if s.zf then 4 else 0) + (if s.cf then 2 else 0) + (if s.vf then 1 else 0)

def regsText (s : State) : String :=
  " ".intercalate (allRegs.map fun r => hex (s.gpr r).toNat)

/-- The bytes `m` holds differently from `pat`, in the part of `BUF` the tests'
stores may reach. -/
def changed (m : Mem) : String :=
  ",".intercalate ((List.range 0x2000).filterMap fun k =>
    let a := BUF + BitVec.ofNat 64 (0x0800 + k)
    if m a != pat a then some s!"{hex a.toNat}:{hex (m a).toNat}" else none)

/-- A test: its id and the lines it runs, and its vectors. -/
def vector (id : Nat) (s : State) (s' : Option State) (scan : Bool := false) : String :=
  match s' with
  | some s' =>
    s!"V {id} {hex (nzcvOf s)} {regsText s} | {hex (nzcvOf s')} {regsText s'} | \
      {if scan then changed s'.mem else ""}"
  | none => s!"F {id} {regsText s}"

/-- The text of `i`, with its operands as the symbols the script defines. -/
def plain (ls : List Line) : String :=
  ";".intercalate (ls.map fun
    | .text s => s
    | .call n => s!"bl {n}"
    | .ops ps => String.join (ps.map fun | .text s => s | .ref n => n))

/-- Values on the edges of what 32-bit and 64-bit arithmetic does. -/
def edges : List (BitVec 64) := [0, 1, 2, 8, 0xff8, 0x1000, 0x7fffffff, 0x80000000, 0xffffffff,
  0x100000000, 0x7fffffffffffffff, 0x8000000000000000, 0xfffffffffffffff8, 0xffffffffffffffff,
  0x123456789abcdef0, 0xedcba98765432110]

/-- Every assignment of `edges` to `x1` and `x2`. -/
def twoRegs : List (List (Reg × BitVec 64)) :=
  edges.flatMap fun a => edges.map fun b => [(.x1, a), (.x2, b)]

/-- Every value of `edges` in `x1`. -/
def oneReg : List (List (Reg × BitVec 64)) := edges.map fun a => [(.x1, a)]

/-- The instructions on two registers. -/
def binary : List Instr := .umulh .x0 .x1 .x2 :: [Size.x, .w].flatMap fun sz => [
  .add sz .x0 .x1 (.reg .x2), .sub sz .x0 .x1 (.reg .x2), .and sz .x0 .x1 (.reg .x2),
  .orr sz .x0 .x1 (.reg .x2), .cmp sz .x1 (.reg .x2), .tst sz .x1 (.reg .x2),
  .add sz .x0 .x1 (.lsl .x2 3), .sub sz .x0 .x1 (.lsl .x2 12), .and sz .x0 .x1 (.lsl .x2 1),
  .orr sz .x0 .x1 (.lsl .x2 7), .cmp sz .x1 (.lsl .x2 4), .tst sz .x1 (.lsl .x2 2),
  .mov sz .x0 (.reg .x2), .lslv sz .x0 .x1 .x2, .lsrv sz .x0 .x1 .x2, .mul sz .x0 .x1 .x2]

/-- Immediates an `add`, `sub` or `cmp` can encode. -/
def addImms : List Nat := [0, 1, 8, 4095, 4096, 8192, 32768, 0x7ff000, 0xfff000]

/-- Bitmask immediates for 64 bits and for 32. -/
def logical64 : List Nat := [1, 0xff8, 0xffff, 0x1f0, 0x5555555555555555, 0xf0f0f0f0f0f0f0f0,
  0x8000000000000001, 0xfffffffffffffffe, 0xffffffff, 0x7fffffffffffffff]

def logical32 : List Nat := [1, 0xff8, 0xffff, 0x1f0, 0x55555555, 0xf0f0f0f0, 0x80000001, 0xfffffffe,
  0x7fffffff]

/-- The instructions with an immediate, on the value in `x1`. -/
def withImm : List Instr :=
  ([Size.x, .w].flatMap fun sz => addImms.flatMap fun v =>
    [.add sz .x0 .x1 (.imm v), .sub sz .x0 .x1 (.imm v), .cmp sz .x1 (.imm v)]) ++
  (logical64.flatMap fun v => [.and .x .x0 .x1 (.imm v), .orr .x .x0 .x1 (.imm v), .tst .x .x1 (.imm v)]) ++
  (logical32.flatMap fun v => [.and .w .x0 .x1 (.imm v), .orr .w .x0 .x1 (.imm v), .tst .w .x1 (.imm v)]) ++
  ([Size.x, .w].flatMap fun sz => [0, 1, 2, 8192, 65535].map fun v => .mov sz .x0 (.imm v)) ++
  ([1, 3, 12, 31, 32, 63].flatMap fun n => [.lsl .x .x0 .x1 n, .lsr .x .x0 .x1 n]) ++
  ([1, 3, 12, 31].flatMap fun n => [.lsl .w .x0 .x1 n, .lsr .w .x0 .x1 n])

/-- The memory operands of a width: its immediates `ds`, a register, a register
scaled by `scale`, and a symbol's low 12 bits at the offset `lo`. -/
def memOps (ds : List Int) (scale : Nat) (lo : Int) : List MemOp :=
  ds.map (fun d => { base := .x1, off := .imm d }) ++
  [{ base := .x1, off := .reg .x2 }, { base := .x1, off := .lo12 (.sym "vg_sym" lo) }] ++
  (if scale == 0 then [] else [{ base := .x1, off := .reg .x2 scale }])

/-- Every load and store of every width and operand form. -/
def memTests : List Instr :=
  let forms : List (Width × List Int × Nat × Int) := [
    (.x, [0, 8, 16, -8, 3, 255, -256, 2040], 3, 0),
    (.w, [0, 4, 8, -4, 3, 255, -256, 2044], 2, 4),
    (.b, [0, 1, 7, -1, 255, -256, 2047], 0, 1)]
  forms.flatMap fun (wd, ds, sc, lo) =>
    (memOps ds sc lo).flatMap fun m => [.ldr wd .x0 m, .str wd .x5 m]

/-- The registers a memory test starts with: the base, on a boundary and not,
an index, and a word to store. -/
def memInputs : List (List (Reg × BitVec 64)) :=
  [0x1000, 0x1001, 0x1ff8].flatMap fun b => [0, 0x10, 3].map fun i =>
    [(.x1, BUF + BitVec.ofNat 64 b), (.x2, i), (.x5, 0x1122334455667788)]

/-- `stp xzr, xzr`, and a load whose base is its destination. -/
def memExtra : List Instr := [.stpz .x1 0, .stpz .x1 16, .stpz .x1 (-16), .stpz .x1 504, .stpz .x1 (-512),
  .ldr .x .x1 { base := .x1 }, .ldr .w .x1 { base := .x1, off := .imm 4 }, .ldr .b .x1 { base := .x1 },
  .str .x .x1 { base := .x1 }]

/-- Addresses, by symbol and by label. -/
def addrTests : List Instr := [.adrp .x0 (.sym "vg_sym"), .adrp .x0 (.sym "vg_sym" 0x10000),
  .adrp .x0 (.sym "vg_sym" (-0x10000)), .adrp .x0 (.sym "vg_sym" 0xdc8), .adrp .x11 .desc,
  .addLo12 .x0 .x1 (.sym "vg_sym"), .addLo12 .x0 .x1 (.sym "vg_sym" 8), .addLo12 .x0 .x1 (.sym "vg_sym" 0xdc8),
  .addLo12 .x11 .x1 .desc, .mrsTp .x10, .movGot .x0 "vg_sym"]

/-- The crate's own area's distance from the thread pointer, in the script's link: its section's
alignment, past the 16 bytes the ABI keeps above the thread pointer, and the 768 bytes it skips. -/
def AREA_TPREL : BitVec 64 := 0x1300

/-- `st`, with the model's word holding that distance. -/
def stArea (inp : List (Reg × BitVec 64)) (nzcv : Nat) : State :=
  let s := st inp nzcv
  { s with
    mem := s.mem.writeW BUF AREA_TPREL
    syms := fun n => if n = rseqOffsetSym then BUF else s.syms n }

/-- A test of an addition of that distance, on the edges of `x1` and from `x10` to itself. -/
def areaTest (id : Nat) (i : Instr) (inputs : List (List (Reg × BitVec 64))) : List String :=
  let t := s!"T {id} I {plain (printer.instr i)}"
  t :: inputs.map fun inp => let s := stArea inp 0xA; vector id s (exec i s)

/-- Each branch condition. -/
def conds : List Cond := [.eq, .ne, .hs, .lo, .hi, .ls, .cbz .x .x1, .cbz .w .x1, .cbnz .x .x1, .cbnz .w .x1,
  .tbz .x1 0, .tbz .x1 31, .tbz .x1 32, .tbz .x1 63, .tbnz .x1 0, .tbnz .x1 31, .tbnz .x1 32, .tbnz .x1 63]

/-- Whether an instruction may write memory: only for these is the model's
memory scanned, which takes long enough to want to skip. -/
def writes : Instr → Bool
  | .str .. | .stpz .. => true
  | _ => false

/-- Run a test of an instruction, on its vectors. -/
def instrTest (id : Nat) (i : Instr) (inputs : List (List (Reg × BitVec 64))) : List String :=
  let t := s!"T {id} I {plain (printer.instr i)}"
  t :: inputs.flatMap fun inp => [0xA, 0x5].map fun nz =>
    let s := st inp nz
    vector id s (exec i s) (writes i)

/-- Run a test of a branch, on every flag and the edge values. Taken, `x17` is 1. -/
def condTest (id : Nat) (c : Cond) : List String :=
  let t := s!"T {id} B {Cond.branch c "97f"}"
  t :: (List.range 16).flatMap fun nz => oneReg.map fun inp =>
    let s := st inp nz
    let s' := (eval c s).map fun b => s.setReg .x17 (if b then 1 else 0)
    vector id s s'

/-- Run the test of the word after the descriptor: only from its address. -/
def wordTest (id : Nat) : List String :=
  [s!"T {id} I {plain (printer.instr (.ldrWord .x9 .x11 "vg_sym"))}",
    vector id (st [(.x11, DESC)] 0xA) (exec (.ldrWord .x9 .x11 "vg_sym") (st [(.x11, DESC)] 0xA))]

/-- Sample values to check `logicalImmOk` against: every bit pattern below 2^12,
then a run of ones at every position and width, and other patterns. -/
def logicalSample : List (Nat × Nat) :=
  let small := List.range 4096
  let runs := (List.range 64).flatMap fun pos => (List.range 64).map fun len =>
    ((2 ^ (len + 1) - 1) * 2 ^ pos) % 2 ^ 64
  let wide := [0x5555555555555555, 0xAAAAAAAAAAAAAAAA, 0x3333333333333333, 0x0f0f0f0f0f0f0f0f,
    0x00ff00ff00ff00ff, 0x0000ffff0000ffff, 0x00000000ffffffff, 0xffffffff00000000, 0x5555555555555554,
    0x1234567812345678, 0xffffffffffffffff, 0, 0x8000000000000000, 0xc000000000000001]
  [(64 : Nat), 32].flatMap fun bits => (small ++ runs ++ wide).filterMap fun v =>
    if v < 2 ^ bits then some (bits, v) else none

def main : IO Unit := do
  for (_, t, e) in golden do
    IO.println s!"G {hex e} {t}"
  for (bits, v) in logicalSample do
    IO.println s!"L {bits} {hex v} {if logicalImmOk bits v then 1 else 0}"
  let mut id := 0
  let emit (ls : List String) : IO Unit := ls.forM IO.println
  for i in binary do
    emit (instrTest id i twoRegs); id := id + 1
  for i in withImm do
    emit (instrTest id i oneReg); id := id + 1
  for i in memTests ++ memExtra do
    emit (instrTest id i memInputs); id := id + 1
  for i in addrTests do
    emit (instrTest id i [[(.x1, 0x123456789abcd000)]]); id := id + 1
  for c in conds do
    emit (condTest id c); id := id + 1
  for i in [Instr.addAreaHi .x0 .x1, .addAreaLo .x0 .x1] do
    emit (areaTest id i oneReg); id := id + 1
  for i in [Instr.addAreaHi .x10 .x10, .addAreaLo .x10 .x10] do
    emit (areaTest id i [[(.x10, 0x123456789abcd000)]]); id := id + 1
  emit (wordTest id)

end VG.Test.VectorsAArch64

def main : IO Unit := VG.Test.VectorsAArch64.main
