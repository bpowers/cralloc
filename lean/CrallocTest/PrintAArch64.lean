import Cralloc.TCB.AArch64.Target
import Cralloc.TCB.X86_64.Target
import Cralloc.TCB.Rust

/-!
# Golden tests for the AArch64 printer

The printer is part of the TCB but not verified; these tests pin down its output
for every construct, so that changes to it are deliberate and show up in review.
`golden` also gives each instruction its encoding, from the Arm ARM's tables, and
`scripts/check_aarch64.py` assembles the text with `llvm-mc` and compares: an
assembler free to pick among encodings has not picked another instruction.
-/

namespace VG.Test.AArch64

open VG.AArch64

/-- The lines of a printed function, a call shown as `<call name>`. -/
def text (ls : List Line) : List String :=
  ls.map fun
    | .text s => s
    | .call n => s!"<call {n}>"
    | .ops ps => String.join (ps.map fun | .text s => s | .ref n => s!"<{n}>")

/-- The text of an instruction's lines. -/
def lines (i : Instr) : List String := text (printer.instr i)

/-- Every instruction form, each with the one line it prints and that line's
encoding (DDI 0487 C6.2). `vg_sym`, an operand of the block, leaves the bits a
relocation fills at 0. -/
def golden : List (Instr × String × Nat) := [
  -- Loads, 64-bit: scaled and unscaled immediates, register and scaled register.
  (.ldr .x .x0 { base := .x1 }, "ldr x0, [x1]", 0xf9400020),
  (.ldr .x .x9 { base := .x9 }, "ldr x9, [x9]", 0xf9400129),
  (.ldr .x .x0 { base := .x1, off := .imm 8 }, "ldr x0, [x1, #8]", 0xf9400420),
  (.ldr .x .x0 { base := .x1, off := .imm 32760 }, "ldr x0, [x1, #32760]", 0xf97ffc20),
  (.ldr .x .x0 { base := .x1, off := .imm (-8) }, "ldr x0, [x1, #-8]", 0xf85f8020),
  (.ldr .x .x0 { base := .x1, off := .imm 255 }, "ldr x0, [x1, #255]", 0xf84ff020),
  (.ldr .x .x1 { base := .x3, off := .reg .x8 }, "ldr x1, [x3, x8]", 0xf8686861),
  (.ldr .x .x1 { base := .x3, off := .reg .x5 3 }, "ldr x1, [x3, x5, lsl #3]", 0xf8657861),
  -- Loads, 32-bit: the `w` register.
  (.ldr .w .x8 { base := .x10, off := .imm 4 }, "ldr w8, [x10, #4]", 0xb9400548),
  (.ldr .w .x8 { base := .x10, off := .imm (-4) }, "ldr w8, [x10, #-4]", 0xb85fc148),
  (.ldr .w .x1 { base := .x3, off := .reg .x8 }, "ldr w1, [x3, x8]", 0xb8686861),
  (.ldr .w .x1 { base := .x3, off := .reg .x5 2 }, "ldr w1, [x3, x5, lsl #2]", 0xb8657861),
  -- Byte loads.
  (.ldr .b .x5 { base := .x12 }, "ldrb w5, [x12]", 0x39400185),
  (.ldr .b .x5 { base := .x12, off := .imm 4095 }, "ldrb w5, [x12, #4095]", 0x397ffd85),
  (.ldr .b .x5 { base := .x12, off := .imm (-1) }, "ldrb w5, [x12, #-1]", 0x385ff185),
  (.ldr .b .x5 { base := .x12, off := .reg .x2 }, "ldrb w5, [x12, x2]", 0x38626985),
  -- Stores.
  (.str .x .x11 { base := .x10, off := .imm 8 }, "str x11, [x10, #8]", 0xf900054b),
  (.str .x .x0 { base := .x1 }, "str x0, [x1]", 0xf9000020),
  (.str .x .x0 { base := .x1, off := .imm (-8) }, "str x0, [x1, #-8]", 0xf81f8020),
  (.str .x .x1 { base := .x3, off := .reg .x8 }, "str x1, [x3, x8]", 0xf8286861),
  (.str .x .x1 { base := .x3, off := .reg .x5 3 }, "str x1, [x3, x5, lsl #3]", 0xf8257861),
  (.str .w .x1 { base := .x3, off := .imm 4 }, "str w1, [x3, #4]", 0xb9000461),
  (.str .w .x1 { base := .x3, off := .reg .x5 2 }, "str w1, [x3, x5, lsl #2]", 0xb8257861),
  (.str .b .x1 { base := .x3, off := .imm 1 }, "strb w1, [x3, #1]", 0x39000461),
  (.str .b .x1 { base := .x3, off := .reg .x5 }, "strb w1, [x3, x5]", 0x38256861),
  -- Sixteen zero bytes: register 31 is the zero register here.
  (.stpz .x0 0, "stp xzr, xzr, [x0]", 0xa9007c1f),
  (.stpz .x0 16, "stp xzr, xzr, [x0, #16]", 0xa9017c1f),
  (.stpz .x0 (-16), "stp xzr, xzr, [x0, #-16]", 0xa93f7c1f),
  -- Arithmetic: a register, a shifted register, 12 bits, 12 bits shifted by 12.
  (.add .x .x8 .x8 (.reg .x9), "add x8, x8, x9", 0x8b090108),
  (.add .x .x3 .x13 (.lsl .x5 3), "add x3, x13, x5, lsl #3", 0x8b050da3),
  (.add .x .x9 .x1 (.imm 8), "add x9, x1, #8", 0x91002029),
  (.add .x .x9 .x1 (.imm 4095), "add x9, x1, #4095", 0x913ffc29),
  (.add .x .x9 .x1 (.imm 4096), "add x9, x1, #1, lsl #12", 0x91400429),
  (.add .w .x9 .x1 (.reg .x2), "add w9, w1, w2", 0x0b020029),
  (.sub .x .x1 .x1 (.imm 8), "sub x1, x1, #8", 0xd1002021),
  (.sub .x .x4 .x4 (.reg .x7), "sub x4, x4, x7", 0xcb070084),
  (.sub .w .x4 .x4 (.imm 1), "sub w4, w4, #1", 0x51000484),
  -- Logic: a register, a bitmask immediate.
  (.and .x .x9 .x0 (.imm 0xff8), "and x9, x0, #0xff8", 0x927d2009),
  (.and .x .x9 .x1 (.imm 0xffff), "and x9, x1, #0xffff", 0x92403c29),
  (.and .x .x9 .x1 (.reg .x2), "and x9, x1, x2", 0x8a020029),
  (.and .w .x9 .x1 (.imm 0xf0), "and w9, w1, #0xf0", 0x121c0c29),
  (.orr .x .x2 .x0 (.reg .x1), "orr x2, x0, x1", 0xaa010002),
  (.orr .x .x2 .x0 (.imm 1), "orr x2, x0, #0x1", 0xb2400002),
  -- Moves: of an immediate, `movz`; of a register, `orr` with the zero register.
  (.mov .x .x0 (.imm 0), "mov x0, #0", 0xd2800000),
  (.mov .x .x3 (.imm 2), "mov x3, #2", 0xd2800043),
  (.mov .x .x3 (.imm 65535), "mov x3, #65535", 0xd29fffe3),
  (.mov .w .x3 (.imm 2), "mov w3, #2", 0x52800043),
  (.mov .x .x0 (.reg .x3), "mov x0, x3", 0xaa0303e0),
  (.mov .w .x0 (.reg .x3), "mov w0, w3", 0x2a0303e0),
  -- Compares: `subs` with the zero register as destination.
  (.cmp .w .x8 (.imm 8192), "cmp w8, #2, lsl #12", 0x7140091f),
  (.cmp .x .x0 (.imm 32768), "cmp x0, #8, lsl #12", 0xf140201f),
  (.cmp .x .x9 (.imm 1024), "cmp x9, #1024", 0xf110013f),
  (.cmp .x .x3 (.reg .x1), "cmp x3, x1", 0xeb01007f),
  (.cmp .w .x5 (.reg .x7), "cmp w5, w7", 0x6b0700bf),
  -- Tests: `ands` with the zero register as destination.
  (.tst .w .x1 (.reg .x2), "tst w1, w2", 0x6a02003f),
  (.tst .x .x1 (.reg .x2), "tst x1, x2", 0xea02003f),
  (.tst .w .x1 (.imm 0x1f0), "tst w1, #0x1f0", 0x721c103f),
  (.tst .x .x1 (.imm 0xff8), "tst x1, #0xff8", 0xf27d203f),
  -- Shifts.
  (.lsl .x .x8 .x8 12, "lsl x8, x8, #12", 0xd374cd08),
  (.lsr .x .x2 .x2 3, "lsr x2, x2, #3", 0xd343fc42),
  (.lsl .w .x8 .x8 12, "lsl w8, w8, #12", 0x53144d08),
  (.lsr .w .x2 .x2 3, "lsr w2, w2, #3", 0x53037c42),
  (.lsrv .x .x4 .x0 .x6, "lsr x4, x0, x6", 0x9ac62404),
  (.lslv .x .x4 .x0 .x6, "lsl x4, x0, x6", 0x9ac62004),
  (.lsrv .w .x4 .x0 .x6, "lsr w4, w0, w6", 0x1ac62404),
  (.mul .x .x9 .x0 .x1, "mul x9, x0, x1", 0x9b017c09),
  (.mul .w .x9 .x0 .x1, "mul w9, w0, w1", 0x1b017c09),
  (.umulh .x2 .x0 .x1, "umulh x2, x0, x1", 0x9bc17c02),
  (.mrsTp .x10, "mrs x10, tpidr_el0", 0xd53bd04a),
  (.ldrWord .x9 .x11 "rs", "ldr x9, [x11, #32]", 0xf9401169)]

-- The text of each form, one line each.
#guard golden.all fun (i, t, _) => lines i == [t]

/-- An instruction the printer refuses. -/
def refused (i : Instr) : Bool := (printer.unencodable i).isSome

-- Each form above is one the printer accepts.
#guard golden.all fun (i, _, _) => !refused i

/-! ## Symbols and the descriptor

Named by operands, the part the instruction holds written by the linker. -/

#guard lines (.adrp .x9 (.sym "SLABS")) == ["adrp x9, <SLABS>"]
#guard lines (.adrp .x12 (.sym "SLABS" 33556480)) == ["adrp x12, <SLABS> + 33556480"]
#guard lines (.addLo12 .x12 .x12 (.sym "SLABS" 33556480)) == ["add x12, x12, :lo12:<SLABS> + 33556480"]
#guard lines (.ldr .w .x6 { base := .x14, off := .lo12 (.sym "SLABS" 1024) }) ==
  ["ldr w6, [x14, :lo12:<SLABS> + 1024]"]
#guard lines (.ldr .x .x7 { base := .x14, off := .lo12 (.sym "SLABS" (-8)) }) ==
  ["ldr x7, [x14, :lo12:<SLABS> - 8]"]
#guard lines (.ldr .b .x7 { base := .x14, off := .lo12 (.sym "T") }) == ["ldrb w7, [x14, :lo12:<T>]"]
#guard lines (.str .x .x7 { base := .x14, off := .lo12 (.sym "T") }) == ["str x7, [x14, :lo12:<T>]"]
#guard lines (.adrp .x11 .desc) == ["adrp x11, 3f"]
#guard lines (.addLo12 .x11 .x11 .desc) == ["add x11, x11, :lo12:3f"]
-- The word of the routine's own, in relocated read-only data, 16-aligned.
#guard lines (.movGot .x9 "rs") ==
  [".pushsection .data.rel.ro.cralloc_got,\"aw\",%progbits", ".balign 16", "8:", ".quad <rs>",
    ".popsection", "adrp x9, 8b", "ldr x9, [x9, :lo12:8b]"]
-- Each refers to its symbol as an operand, and a descriptor's is a label.
#guard (printer.instr (.adrp .x9 (.sym "SLABS"))).flatMap Line.refs == ["SLABS"]
#guard (printer.instr (.adrp .x11 .desc)).flatMap Line.refs == []
#guard (printer.instr (.movGot .x9 "rs")).flatMap Line.refs == ["rs"]
-- `ldrWord` names no operand itself: the word after the descriptor does.
#guard (printer.instr (.ldrWord .x9 .x11 "rs")).flatMap Line.refs == []

/-! ## Branches -/

#guard Cond.branch .eq "7f" == "b.eq 7f"
#guard Cond.branch .ne "7f" == "b.ne 7f"
#guard Cond.branch .hs "7f" == "b.hs 7f"
#guard Cond.branch .lo "7f" == "b.lo 7f"
#guard Cond.branch .hi "20f" == "b.hi 20f"
#guard Cond.branch .ls "20f" == "b.ls 20f"
#guard Cond.branch (.cbz .x .x3) "7f" == "cbz x3, 7f"
#guard Cond.branch (.cbz .w .x3) "7f" == "cbz w3, 7f"
#guard Cond.branch (.cbnz .x .x3) "7f" == "cbnz x3, 7f"
-- A bit below 32 names the `w` register, as `roofline` writes it; above, the `x`.
#guard Cond.branch (.tbnz .x3 0) "7f" == "tbnz w3, #0, 7f"
#guard Cond.branch (.tbz .x3 31) "7f" == "tbz w3, #31, 7f"
#guard Cond.branch (.tbnz .x3 32) "7f" == "tbnz x3, #32, 7f"
#guard printer.jump "5b" == "b 5b"
#guard printer.ret == ["ret"]

/-! ## What no instruction encodes

The printer refuses each, naming it, where an assembler would refuse it or pick
something else. -/

-- 12 bits, or 12 bits shifted by 12.
#guard !refused (.add .x .x0 .x1 (.imm 0xfff000))
#guard refused (.add .x .x0 .x1 (.imm 4097))
#guard refused (.sub .x .x0 .x1 (.imm 0x1000000))
#guard refused (.cmp .w .x8 (.imm 8193))
-- A 32-bit operand holds no wider immediate.
#guard refused (.cmp .w .x8 (.imm (2 ^ 32)))
-- Bitmask immediates: a rotated run of ones, repeated; not zero, not all ones.
#guard !refused (.and .x .x0 .x1 (.imm 0x5555555555555555))
#guard !refused (.and .x .x0 .x1 (.imm 0xf0f0f0f0f0f0f0f0))
#guard !refused (.and .w .x0 .x1 (.imm 0xfffffffe))
#guard !refused (.tst .x .x0 (.imm 0x8000000000000001))
#guard refused (.and .x .x0 .x1 (.imm 0))
#guard refused (.and .x .x0 .x1 (.imm 0xffffffffffffffff))
#guard refused (.and .w .x0 .x1 (.imm 0xffffffff))
#guard refused (.and .x .x0 .x1 (.imm 0x5))
#guard refused (.tst .w .x0 (.imm 0x1000f))
-- 64-bit patterns do not fit a 32-bit instruction.
#guard refused (.and .w .x0 .x1 (.imm 0x100000000))
-- A wide immediate: this printer knows only `movz` with no shift.
#guard refused (.mov .x .x0 (.imm 65536))
#guard refused (.mov .x .x0 (.lsl .x1 3))
-- Shifts below the operand size.
#guard !refused (.add .x .x0 .x1 (.lsl .x2 63))
#guard refused (.add .x .x0 .x1 (.lsl .x2 64))
#guard refused (.add .w .x0 .x1 (.lsl .x2 32))
#guard !refused (.lsl .x .x0 .x1 63)
#guard refused (.lsl .x .x0 .x1 64)
#guard refused (.lsr .w .x0 .x1 32)
-- Scaled unsigned offsets to 4095 times the size, or signed 9 bits.
#guard !refused (.ldr .x .x0 { base := .x1, off := .imm 32760 })
#guard refused (.ldr .x .x0 { base := .x1, off := .imm 32768 })
#guard !refused (.ldr .x .x0 { base := .x1, off := .imm 255 })
#guard refused (.ldr .x .x0 { base := .x1, off := .imm 257 })
#guard !refused (.ldr .x .x0 { base := .x1, off := .imm (-256) })
#guard refused (.ldr .x .x0 { base := .x1, off := .imm (-257) })
#guard !refused (.ldr .w .x0 { base := .x1, off := .imm 16380 })
#guard refused (.ldr .w .x0 { base := .x1, off := .imm 16384 })
#guard !refused (.ldr .b .x0 { base := .x1, off := .imm 4095 })
#guard refused (.ldr .b .x0 { base := .x1, off := .imm 4096 })
-- A register offset: no shift, or the access's size.
#guard refused (.ldr .x .x0 { base := .x1, off := .reg .x2 2 })
#guard refused (.ldr .w .x0 { base := .x1, off := .reg .x2 3 })
#guard refused (.ldr .b .x0 { base := .x1, off := .reg .x2 1 })
-- `stp`: a multiple of 8 in 7 signed bits.
#guard !refused (.stpz .x0 504)
#guard !refused (.stpz .x0 (-512))
#guard refused (.stpz .x0 512)
#guard refused (.stpz .x0 (-520))
#guard refused (.stpz .x0 4)
-- A symbol's offset: 32 bits, as on x86-64.
#guard !refused (.adrp .x0 (.sym "S" (2 ^ 31 - 1)))
#guard refused (.adrp .x0 (.sym "S" (2 ^ 31)))
#guard refused (.addLo12 .x0 .x0 (.sym "S" (-2 ^ 31 - 1)))
#guard refused (.ldr .x .x0 { base := .x1, off := .lo12 (.sym "S" (2 ^ 31)) })

-- The crate's own area's distance from the thread pointer, which the linker writes.
#guard lines (.addAreaHi .x10 .x10) == ["add x10, x10, :tprel_hi12:cralloc_rseq_area"]
#guard lines (.addAreaLo .x10 .x10) == ["add x10, x10, :tprel_lo12_nc:cralloc_rseq_area"]
#guard !refused (.addAreaHi .x10 .x10) && !refused (.addAreaLo .x10 .x10)
#guard (Instr.notInBody (.addAreaHi .x10 .x10)).isSome && (Instr.notInBody (.addAreaLo .x10 .x10)).isSome

/-! ## Restartable sequences

A setup first, the arm at label 5, the sequence from 6 to 7, the descriptor at 3
with the word after it, and in the function's own section, after it, the
signature and the abort handler at 4, which branches back to the arm alone. An
exit straight to an external target branches to a stub after the return. -/

def setup : List Instr := [
  .adrp .x11 .desc, .addLo12 .x11 .x11 .desc, .ldrWord .x9 .x11 "rs", .ldr .x .x9 { base := .x9 },
  .mrsTp .x10, .add .x .x10 .x10 (.reg .x9)]

def arm : List Instr := [.str .x .x11 { base := .x10, off := .imm 8 }]

def body : List (Step Instr Cond) := [
  .instr (.ldr .w .x8 { base := .x10, off := .imm 4 }),
  .instr (.cmp .w .x8 (.imm 8192)),
  .exit .hs [] (some "slow"),
  .instr (.lsl .x .x8 .x8 12),
  .instr (.ldr .x .x1 { base := .x3, off := .reg .x8 }),
  .instr (.tst .w .x1 (.reg .x2)),
  .exit .eq [] none,
  .exit .ne [.mov .x .x4 (.imm 1)] none,
  .instr (.str .x .x1 { base := .x3, off := .reg .x8 })]

-- What a routine prints: its setup, in place, is not behind the label 5.
#guard text (printer.routine setup (.rseq arm body)) == [
  "adrp x11, 3f",
  "add x11, x11, :lo12:3f",
  "ldr x9, [x11, #32]",
  "ldr x9, [x9]",
  "mrs x10, tpidr_el0",
  "add x10, x10, x9",
  "5:",
  "str x11, [x10, #8]",
  "6:",
  "ldr w8, [x10, #4]",
  "cmp w8, #2, lsl #12",
  "b.hs 20f",
  "lsl x8, x8, #12",
  "ldr x1, [x3, x8]",
  "tst w1, w2",
  "b.eq 7f",
  "b.ne 21f",
  "str x1, [x3, x8]",
  "7:",
  ".pushsection __rseq_cs, \"aw\"",
  ".balign 32",
  "3:",
  ".long 0, 0",
  ".quad 6b, 7b - 6b, 4f",
  ".quad <rs>",
  ".popsection",
  ".subsection 1",
  ".inst 0xd428bc00",
  "4:",
  "b 5b",
  "21:",
  "mov x4, #1",
  "b 7b",
  ".subsection 0",
  "ret",
  "20:",
  "b <slow>"
]

-- The operands it names: the word after the descriptor's, and the stub's.
#guard (printer.routine setup (.rseq arm body)).flatMap Line.refs == ["rs", "slow"]

-- Without stubs, as inline, the branch names the target itself.
#guard inlineTarget.printer.stubExits == false
#guard inlineTarget.printer.coldFar == true
#guard target.printer.coldFar == false
#guard ((text (Printer.lowerWith { printer with stubExits := false } [] (.rseq arm body) 0).1).take 6).drop 4 ==
  ["cmp w8, #2, lsl #12", "b.hs <slow>"]

-- The result moved after the commit, as the slow path's routines do: the tail is
-- in place, ahead of the return.
#guard (text (printer.routine setup (.seq (.rseq arm [.instr (.str .x .x1 { base := .x3 })])
    (.block [.mov .x .x0 (.reg .x3)])))).getLast? == some "ret"
#guard (text (printer.routine setup (.seq (.rseq arm [.instr (.str .x .x1 { base := .x3 })])
    (.block [.mov .x .x0 (.reg .x3)])))).contains "mov x0, x3"

-- A tail call after a sequence: no return.
#guard (text (printer.routine setup (.seq (.rseq arm [.instr (.str .x .x1 { base := .x3 })])
    (.jump [.mov .x .x0 (.reg .x3)] "zero")))).getLast? == some "b <zero>"

/-! ## What the emitter refuses, and adds -/

/-- The error of a failed check. -/
def err : Except String Unit → Option String
  | .error e => some e
  | .ok _ => none

-- The word after the descriptor: one symbol, read in the setup, of a routine with a descriptor.
#guard (Rust.descWordCheck printer "f" setup (.rseq arm body)).toBool
#guard (Rust.descWordCheck printer "f" (setup ++ [.ldrWord .x9 .x11 "rs"]) (.rseq arm body)).toBool
#guard err (Rust.descWordCheck printer "f" (setup ++ [.ldrWord .x9 .x11 "other"]) (.rseq arm body)) ==
  some "f reads two symbols' addresses after its descriptor"
#guard err (Rust.descWordCheck printer "f" [] (.rseq (.ldrWord .x9 .x11 "rs" :: arm) body)) ==
  some "f reads the word after its descriptor outside its setup"
#guard err (Rust.descWordCheck printer "f" setup (.block [])) ==
  some "f reads the word after its descriptor, but has no descriptor"
-- Inlined, a conditional branch may reach the end of the sequence only: the function
-- may be any size, and a linker cannot put a veneer on one.
#guard (Rust.nearExits inlineTarget.printer "f" [.exit .hs [] none, .instr (.str .x .x1 { base := .x3 })]).toBool
#guard err (Rust.nearExits inlineTarget.printer "f" [.exit .hs [] (some "miss")]) ==
  some "f is inline, where a conditional branch may not reach an exit with code of its own or an external target"
#guard (Rust.nearExits inlineTarget.printer "f" [.exit .hs [.mov .x .x0 (.imm 0)] none]).toBool == false
-- A naked function's section holds nothing else, and x86-64's exits have a stub's reach.
#guard (Rust.nearExits printer "f" [.exit .hs [] (some "slow")]).toBool
#guard (Rust.nearExits X86_64.inlineTarget.printer "f" [.exit .e [] (some "miss")]).toBool

-- The signature `.inst` writes is glibc's for this architecture.
#guard hexLit rseqSignature == "0xd428bc00"
-- The commit must be a plain 64-bit store.
#guard printer.plainStore (.str .x .x1 { base := .x3, off := .reg .x8 })
#guard !printer.plainStore (.str .w .x1 { base := .x3 })
#guard !printer.plainStore (.stpz .x0 0)
-- A leaf gets unwind information; text naming the stack pointer or `x30`, a call,
-- does not. x86-64's text means nothing here, and this printer's nothing there.
#guard printer.unwindStart == [".cfi_startproc"]
#guard (Rust.unwindLines printer (printer.function (.block [.add .x .x0 .x0 (.reg .x1)]))).head? ==
  some (.text ".cfi_startproc")
#guard (Rust.unwindLines printer [.text "add x0, x1, x2", .text "ret"]).length == 4
#guard (Rust.unwindLines printer [.text "ldr x0, [sp, #8]"]).length == 1
#guard (Rust.unwindLines printer [.text "add x0, sp, #8"]).length == 1
#guard (Rust.unwindLines printer [.text "mov x30, x0"]).length == 1
#guard (Rust.unwindLines printer [.call "f"]).length == 1
-- The directive that places a function's out-of-line code.
#guard printer.coldOpen == [".subsection 1"]
#guard printer.coldClose == [".subsection 0"]
-- No limit on micro-ops; the function's alignment alone.
#guard printer.uopLimit == none
#guard inlineTarget.printer.uopLimit == none
#guard printer.funcAlign == [".p2align 6"]

-- An inline function's operands, in the order of the signature.
#guard inlineTarget.argRegs == ["x3", "x2", "x0"]
#guard inlineTarget.retReg == "x0"
-- No inline function may declare what `asm!` refuses as an operand.
#guard !inlineTarget.regs.contains "x18"
#guard !inlineTarget.regs.contains "x19"
#guard !inlineTarget.regs.contains "x29"
#guard inlineTarget.regs.contains "x30"

end VG.Test.AArch64
