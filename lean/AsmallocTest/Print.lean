import Asmalloc.TCB.X86_64.Print
import Asmalloc.TCB.Rust
import Asmalloc.TCB.AArch64.Print
import Asmalloc.TCB.X86.Print
import Asmalloc.TCB.Arm.Print

/-!
# Golden tests for the trusted printers

The printers are part of the TCB but not verified; these tests pin down their
output for every construct, so that changes to them are deliberate and show
up in review.
-/

namespace VG.Test

/-- The lines of a printed function, a call shown as `<call name>`. -/
def text (ls : List Line) : List String :=
  ls.map fun
    | .text s => s
    | .call n => s!"<call {n}>"
    | .sym s .page n => s!"{s}<page {n}>"
    | .sym s .pageOff n => s!"{s}<pageoff {n}>"
    | .sym s .ripRel n => s!"{s}<riprel {n}>"
    | .sym s .x86PcRel n => s!"{s}<pcrel {n}>"
    | .sym s .armPcRel n => s!"{s}<armpcrel {n}>"

open X86_64

-- IA-32's position-independent static address is an explicit four-byte
-- frame. The ADD sets flags; releasing its scratch slot preserves them.
#guard text (X86.printer.function
    (.frame (.symPush .edx "VG_TABLE") (.block []) (.free 4) : Prog X86.isa)) ==
  ["call 2f", "2:", "mov edx, DWORD PTR [esp]", ".byte 0x81, 194", ".long <pcrel VG_TABLE>",
    "lea esp, [esp+4]", "ret"]

#guard Rust.line "call" (.sym ".long " .x86PcRel "VG_TABLE") ==
  "        r\".long {VG_TABLE} - 2b\",\n"

-- Neither a plain block nor an invalid frame can use symPush.
def symState : X86.State :=
  { gpr := fun r => if r = .esp then 4096 else 17
    cf := some true, zf := some false, sf := none, of := some false
    mem := fun _ => 0, rd := [], wr := [], unknowns := fun n => BitVec.ofNat 32 (8192 + n)
    syms := fun _ => 12288 }

#guard (X86.exec (.symPush .eax "VG_TABLE") symState).isNone
#guard (X86.push (.symPush .esp "VG_TABLE") symState).isNone
#guard (X86.push (.symPush .eax "VG_TABLE") (symState.setReg .esp 3)).isNone
#guard match X86.push (.symPush .eax "VG_TABLE") symState with
  | none => false
  | some s => s.gpr .esp == 4092 && s.gpr .eax == 12288 && s.gpr .edx == 17 &&
      s.mem.readW 4092 32 == 8192 && s.unknowns 0 == 8193 &&
      s.cf == some false && s.zf == some false && s.sf == some false && s.of == some false &&
      s.syms "VG_TABLE" == 12288 && s.wr == [⟨4092, 4⟩] &&
      match X86.pop (.free 4) s s with
      | none => false
      | some t => t.gpr .esp == 4096 && t.gpr .eax == 12288 && t.wr.isEmpty &&
          t.mem.readW 4092 32 == 8192

-- The ModR/M /0 encoding follows Intel's register encoding, not an
-- accidental order inferred by the assembler from the instruction text.
#guard ([X86.Reg.eax, .ecx, .edx, .ebx, .esp, .ebp, .esi, .edi].map fun r =>
    (X86.Instr.asm (.symPush r "VG_TABLE")).getD 3 "") ==
  [".byte 0x81, 192", ".byte 0x81, 193", ".byte 0x81, 194", ".byte 0x81, 195",
    ".byte 0x81, 196", ".byte 0x81, 197", ".byte 0x81, 198", ".byte 0x81, 199"]

-- A backwards displacement carries; crossing the signed boundary overflows.
#guard match X86.push (.symPush .esi "VG_TABLE") { symState with syms := fun _ => 4096 } with
  | none => false
  | some s => s.gpr .esi == 4096 && s.cf == some true && s.of == some false
#guard match X86.push (.symPush .edi "VG_TABLE")
    { symState with unknowns := fun _ => 0x7fff0000, syms := fun _ => 0x80000000 } with
  | none => false
  | some s => s.gpr .edi == 0x80000000 && s.cf == some false && s.of == some true && s.sf == some true

/-- Every `Code` constructor, nested. -/
def sample : Prog isa :=
  .seq (.block [.alu .xor .rax (.reg .rax)])
    (.ite .ne
      (.loop (.block [.alu .add .rax (.mem { base := .rdi, index := some .rcx, scale := 8, disp := -8 }),
                      .alu .sub .rcx (.imm 1)]) .ne)
      (.block [.store { base := .rsi, disp := 16 } .rax, .mov .rdx (.imm (-1))]))

#guard text (printer.function sample) == [
  "xor rax, rax",
  "jne 20f",
  "mov QWORD PTR [rsi+16], rax",
  "mov rdx, -1",
  "jmp 21f",
  "20:",
  "22:",
  "add rax, QWORD PTR [rdi+rcx*8-8]",
  "sub rcx, 1",
  "jne 22b",
  "21:",
  "ret"
]

/-! `Rust.badCall`: the first call that is not of the code it runs. -/

/-- The printed code of the functions `f`, `g` and `k`, which calls `g`. -/
def printedFG : String → Option (List Line)
  | "f" => some (printer.function (.block [.mov .rax (.imm 1)]))
  | "g" => some (printer.function (.block [.mov .rax (.imm 2)]))
  | "k" => some (printer.function (.call "g" (.block [.mov .rax (.imm 2)])))
  | _ => none

-- The same code written twice is two values in memory: the later call of
-- `f` is compared by its printed code.
#guard Rust.badCall printer printedFG
  [("f", .block [.mov .rax (.imm 1)]), ("g", .block [.mov .rax (.imm 2)]),
   ("f", .block [.mov .rax (.imm 1)])] == none
#guard Rust.badCall printer printedFG [("h", .block [])] == some ("h", false)
#guard Rust.badCall printer printedFG
  [("f", .block [.mov .rax (.imm 1)]), ("g", .block [.mov .rax (.imm 1)])] == some ("g", true)
-- A later call of a function whose first call is of its code.
#guard Rust.badCall printer printedFG
  [("f", .block [.mov .rax (.imm 1)]), ("f", .block [.mov .rax (.imm 2)])] == some ("f", true)
-- Calls nested in a call's code are in `Code.calls`: `k` prints as it should,
-- but the code its call of `g` runs does not.
#guard Rust.badCall printer printedFG
  (Code.calls (.seq (.call "k" (.call "g" (.block [.mov .rax (.imm 2)])))
    (.call "f" (.block [.mov .rax (.imm 1)])) : Prog isa)) == none
#guard Rust.badCall printer printedFG
  (Code.calls (.call "k" (.call "g" (.block [.mov .rax (.imm 3)])) : Prog isa)) ==
  some ("g", true)

/-- Every 32-bit instruction form. -/
def sample32 : Prog isa := .block [
  .mov32 .rax (.reg .r8),
  .mov32 .r15 (.imm 0xfffffffe),
  .mov32 .rcx (.mem { base := .rsi, disp := 60 }),
  .store32 { base := .rdi, index := some .rdx, scale := 4 } .r11,
  .alu32 .add .rbp (.imm 0x428a2f98),
  .alu32 .xor .r12 (.reg .rsp),
  .alu32 .and .r13 (.mem { base := .rcx, disp := -4 }),
  .shift32 .ror .rbx 25,
  .shift32 .shr .r9 3,
  .bswap32 .r10
]

#guard text (printer.function sample32) == [
  "mov eax, r8d",
  "mov r15d, -2",
  "mov ecx, DWORD PTR [rsi+60]",
  "mov DWORD PTR [rdi+rdx*4], r11d",
  "add ebp, 1116352408",
  "xor r12d, esp",
  "and r13d, DWORD PTR [rcx-4]",
  "ror ebx, 25",
  "shr r9d, 3",
  "bswap r10d",
  "ret"
]

/-- The byte instructions, every 8-bit register name, and 64-bit `bswap`. -/
def sample8 : Prog isa := .block ([
  .movzx8 .rax (.mk .rsi (some .rcx) 1 32),
  .movzx8 .r15 { base := .r12 },
  .bswap .rax,
  .bswap .r9] ++
  [Reg.rax, .rcx, .rdx, .rbx, .rsp, .rbp, .rsi, .rdi,
   .r8, .r9, .r10, .r11, .r12, .r13, .r14, .r15].map (.store8 { base := .rdi, disp := -3 }))

#guard text (printer.function sample8) == [
  "movzx eax, BYTE PTR [rsi+rcx*1+32]",
  "movzx r15d, BYTE PTR [r12]",
  "bswap rax",
  "bswap r9",
  "mov BYTE PTR [rdi-3], al",
  "mov BYTE PTR [rdi-3], cl",
  "mov BYTE PTR [rdi-3], dl",
  "mov BYTE PTR [rdi-3], bl",
  "mov BYTE PTR [rdi-3], spl",
  "mov BYTE PTR [rdi-3], bpl",
  "mov BYTE PTR [rdi-3], sil",
  "mov BYTE PTR [rdi-3], dil",
  "mov BYTE PTR [rdi-3], r8b",
  "mov BYTE PTR [rdi-3], r9b",
  "mov BYTE PTR [rdi-3], r10b",
  "mov BYTE PTR [rdi-3], r11b",
  "mov BYTE PTR [rdi-3], r12b",
  "mov BYTE PTR [rdi-3], r13b",
  "mov BYTE PTR [rdi-3], r14b",
  "mov BYTE PTR [rdi-3], r15b",
  "ret"
]

-- A displacement is encoded in 32 bits, sign-extended: the printer refuses
-- one outside `[-2^31, 2^31)`, for every kind of memory operand, and accepts
-- the bounds.
#guard printer.unencodable (.mov .rax (.mem { base := .rdi, disp := 2147483647 })) == none
#guard printer.unencodable (.store { base := .rdi, index := some .rcx, disp := -2147483648 } .rax) ==
  none
#guard printer.unencodable (.mov .rax (.mem { base := .rdi, disp := 2147483648 })) ==
  some "the displacement of the memory operand [rdi+2147483648] does not fit in 32 bits"
#guard printer.unencodable (.store { base := .rdi, index := some .rcx, disp := -2147483649 } .rax) ==
  some "the displacement of the memory operand [rdi+rcx*1-2147483649] does not fit in 32 bits"
#guard (printer.unencodable (.vmovdqu32Store { base := .rsi, disp := 4294967304 } .xmm0)).isSome
#guard (printer.unencodable (.adox .rax (.mem { base := .rsi, disp := -4294967304 }))).isSome
#guard printer.unencodable (.alu .add .rax (.imm 0x7fffffff)) == none

-- A cold branch (`Code.iteCold`): `je` to its code after the `ret`, which
-- jumps back to the code after the `ite`; the else branch falls through, with
-- no jump, so the code around the `ite` is as long as without `t`.
#guard text (printer.function
    (.seq (Code.iteCold .e (.block [.mov .rax (.imm 1)]) (.block [.mov .rax (.imm 2)]))
      (.block [.mov .rcx (.imm 3)]) : Prog isa)) ==
  ["je 20f", "mov rax, 2", "21:", "mov rcx, 3", "ret", "20:", "mov rax, 1", "jmp 21b"]

-- In a loop, and nested: each cold branch's code follows the return, in the
-- order of the branches, the inner one's after the outer one's jump back.
#guard text (printer.function
    (.loop (Code.iteCold .ne
      (Code.iteCold .b (.block [.mov .rax (.imm 1)]) (.block [.mov .rax (.imm 2)]))
      (.block [.mov .rax (.imm 3)])) .ae : Prog isa)) ==
  ["20:", "jne 21f", "mov rax, 3", "22:", "jae 20b", "ret",
    "21:", "jb 23f", "mov rax, 2", "24:", "jmp 22b", "23:", "mov rax, 1", "jmp 24b"]

-- An ordinary `ite` and a cold branch in one loop body: the numbers stay
-- distinct (the loop's 20, the `ite`'s 21 and 22, the cold branch's 23 and
-- 24), the `ite`'s references are forward, and the cold code, after the
-- return, jumps backward to its place in the loop.
#guard text (printer.function
    (.loop (.seq (.ite .e (.block [.mov .rax (.imm 1)]) (.block [.mov .rax (.imm 2)]))
      (Code.iteCold .ne (.block [.mov .rcx (.imm 3)]) (.block [.mov .rcx (.imm 4)]))) .ae : Prog isa)) ==
  ["20:", "je 21f", "mov rax, 2", "jmp 22f", "21:", "mov rax, 1", "22:",
    "jne 23f", "mov rcx, 4", "24:", "jae 20b", "ret", "23:", "mov rcx, 3", "jmp 24b"]

-- An `ite` whose `then` branch merely starts with one empty block is not cold.
#guard text (printer.function
    (.ite .e (.seq (.block []) (.block [.mov .rax (.imm 1)])) (.block [.mov .rax (.imm 2)]) : Prog isa)) ==
  ["je 20f", "mov rax, 2", "jmp 21f", "20:", "mov rax, 1", "21:", "ret"]

/-- The 64-bit shifts and `movabs`, including an immediate ≥ 2⁶³. -/
def sample64 : Prog isa := .block [
  .shift .ror .rax 28,
  .shift .ror .r15 1,
  .shift .shr .rbx 63,
  .shift .shr .r9 7,
  .movImm64 .rcx 0x428a2f98d728ae22,
  .movImm64 .r13 0xb5c0fbcfec4d3b2f,
  .movImm64 .rsi 1
]

#guard text (printer.function sample64) == [
  "ror rax, 28",
  "ror r15, 1",
  "shr rbx, 63",
  "shr r9, 7",
  "movabs rcx, 4794697086780616226",
  "movabs r13, -5349999486874862801",
  "movabs rsi, 1",
  "ret"
]

/-- Every AArch64 instruction form, and both branch conditions. -/
def sampleA64 : Prog AArch64.isa :=
  .ite (.zero .x .x2) (.block [])
    (.loop (.block [
      .add .w .x4 .x5 .x6, .add .x .x0 .x1 .x30,
      .addImm .x .x1 .x1 64, .subImm .x .x2 .x2 1,
      .logic .and .w .x7 .x8 .x9, .logic .orr .w .x7 .x8 .x9, .logic .eor .x .x7 .x8 .x9,
      .ror .w .x13 .x8 25, .lsr .w .x14 .x15 10, .rev32 .x12 .x12,
      .movz .w .x13 0x2f98 0, .movk .w .x13 0x428a 1,
      .ldr .w .x12 .x1 60, .str .w .x12 .x3 4, .ldr .x .x16 .x17 8, .str .x .x20 .x19 16,
      .sub .w .x4 .x5 .x6, .sub .x .x0 .x1 .x30, .rev .x9 .x10,
      .ldrb .x11 .x20 0, .ldrb .x21 .x22 4095, .strb .x23 .x24 7, .strb .x25 .x26 4095,
      .ldrSp .x9 0, .ldrSp .x10 32760])
      (.nonzero .x .x2))

#guard text (AArch64.printer.function sampleA64) == [
  "cbz x2, 20f",
  "22:",
  "add w4, w5, w6",
  "add x0, x1, x30",
  "add x1, x1, #64",
  "sub x2, x2, #1",
  "and w7, w8, w9",
  "orr w7, w8, w9",
  "eor x7, x8, x9",
  "ror w13, w8, #25",
  "lsr w14, w15, #10",
  "rev w12, w12",
  "movz w13, #12184, lsl #0",
  "movk w13, #17034, lsl #16",
  "ldr w12, [x1, #60]",
  "str w12, [x3, #4]",
  "ldr x16, [x17, #8]",
  "str x20, [x19, #16]",
  "sub w4, w5, w6",
  "sub x0, x1, x30",
  "rev x9, x10",
  "ldrb w11, [x20, #0]",
  "ldrb w21, [x22, #4095]",
  "strb w23, [x24, #7]",
  "strb w25, [x26, #4095]",
  "ldr x9, [sp, #0]",
  "ldr x10, [sp, #32760]",
  "cbnz x2, 22b",
  "b 21f",
  "20:",
  "21:",
  "ret"
]

/-- Every x86 (32-bit) instruction form, every operand form, and both conditions. -/
def sampleX86 : Prog X86.isa :=
  .seq (.block [.mov .eax (.mem { base := .esp, disp := 16 }), .alu .test .ebp (.reg .ebp)])
    (.ite .e (.block [])
      (.loop (.block [
        .mov .ebx (.reg .eax), .mov .ecx (.imm 0xfffffffe), .mov .edx (.mem { base := .esi }),
        .store { base := .eax, disp := 96 } .ebx,
        .alu .add .ecx (.imm 0x428a2f98), .alu .adc .ecx (.reg .edx), .alu .sub .ebp (.imm 1),
        .alu .sbb .ecx (.reg .edx), .alu .and .ebx (.mem { base := .esi, disp := 28 }),
        .alu .or .ebx (.reg .edx), .alu .xor .ebx (.reg .ecx), .alu .cmp .edi (.imm 64),
        .shift .ror .ebx 6, .shift .shr .eax 10, .bswap .eax]) .ne))

#guard text (X86.printer.function sampleX86) == [
  "mov eax, DWORD PTR [esp+16]",
  "test ebp, ebp",
  "je 20f",
  "22:",
  "mov ebx, eax",
  "mov ecx, -2",
  "mov edx, DWORD PTR [esi]",
  "mov DWORD PTR [eax+96], ebx",
  "add ecx, 1116352408",
  "adc ecx, edx",
  "sub ebp, 1",
  "sbb ecx, edx",
  "and ebx, DWORD PTR [esi+28]",
  "or ebx, edx",
  "xor ebx, ecx",
  "cmp edi, 64",
  "ror ebx, 6",
  "shr eax, 10",
  "bswap eax",
  "jne 22b",
  "jmp 21f",
  "20:",
  "21:",
  "ret"
]

/-- The x86 (32-bit) byte instructions, every 8-bit register name, and `jb`/`jae`. -/
def sampleX86Bytes : Prog X86.isa :=
  .seq (.block ([.movzx8 .eax { base := .esi, disp := 32 }, .movzx8 .edi { base := .ebp }] ++
      [X86.Reg8.al, .cl, .dl, .bl].map (.store8 { base := .ebx, disp := 5 })))
    (.seq (.ite .b (.block []) (.block [])) (.ite .ae (.block []) (.block [])))

#guard text (X86.printer.function sampleX86Bytes) == [
  "movzx eax, BYTE PTR [esi+32]",
  "movzx edi, BYTE PTR [ebp]",
  "mov BYTE PTR [ebx+5], al",
  "mov BYTE PTR [ebx+5], cl",
  "mov BYTE PTR [ebx+5], dl",
  "mov BYTE PTR [ebx+5], bl",
  "jb 20f",
  "jmp 21f",
  "20:",
  "21:",
  "jae 22f",
  "jmp 23f",
  "22:",
  "23:",
  "ret"
]

/-- Every ARMv7 instruction form, every second-operand form, and both conditions. -/
def sampleArm : Prog Arm.isa :=
  .seq (.block [.cmp .r2 (.imm 0)])
    (.ite .eq (.block [])
      (.loop (.block [
        .mov .r12 (.imm 255), .mov .lr (.reg .r4), .mov .r12 (.shifted .r8 .ror 6),
        .mov .r12 (.shifted .r8 .lsr 10), .mov .r12 (.shifted .r8 .lsl 3),
        .dp .add .r1 .r1 (.imm 64), .dp .sub .r4 .r5 (.reg .r6),
        .dp .and .r7 .r8 (.reg .lr), .dp .orr .r9 .r10 (.reg .r11),
        .dp .eor .r12 .r12 (.shifted .r8 .ror 11),
        .movw .r12 0x2f98, .movt .r12 0x428a, .rev .r12 .r12,
        .ldr .r12 .r1 60, .str .lr .r3 100, .ldrb .r5 .r6 4095, .strb .lr .r7 3, .ldrSp .r4 8,
        .subs .r2 .r2 (.imm 1)]) .ne))

#guard text (Arm.printer.function sampleArm) == [
  "cmp r2, #0",
  "beq 20f",
  "22:",
  "mov r12, #255",
  "mov lr, r4",
  "ror r12, r8, #6",
  "lsr r12, r8, #10",
  "lsl r12, r8, #3",
  "add r1, r1, #64",
  "sub r4, r5, r6",
  "and r7, r8, lr",
  "orr r9, r10, r11",
  "eor r12, r12, r8, ror #11",
  "movw r12, #12184",
  "movt r12, #17034",
  "rev r12, r12",
  "ldr r12, [r1, #60]",
  "str lr, [r3, #100]",
  "ldrb r5, [r6, #4095]",
  "strb lr, [r7, #3]",
  "ldr r4, [sp, #8]",
  "subs r2, r2, #1",
  "bne 22b",
  "b 21f",
  "20:",
  "21:",
  "bx lr"
]

#guard Arm.encodable 0xff000000 && Arm.encodable 0x3fc && !Arm.encodable 0x101 && !Arm.encodable 0x1fe00

#guard Rust.escape "ld1 {v0.4s}, [x1] \\ \"q\"" == "ld1 {{v0.4s}}, [x1] \\\\ \\\"q\\\""

/-! ## Calls

A call is its own line, whatever the callee's code; the emitter renders it as
the call instruction with the callee's symbol as a `sym` operand. -/

/-- A call between two blocks, on every target. -/
def callSample : Prog X86_64.isa :=
  .seq (.block [.mov .rdi (.reg .rbx)]) (.seq (.call "vg_f" (.block [.alu .add .rax (.imm 1)]))
    (.block [.mov .rax (.reg .rdx)]))

#guard printer.function callSample == [
  .text "mov rdi, rbx", .call "vg_f", .text "mov rax, rdx", .text "ret"]
#guard AArch64.printer.function (.call "vg_f" (.block []) : Prog AArch64.isa) == [.call "vg_f", .text "ret"]
#guard X86.printer.function (.call "vg_f" (.block []) : Prog X86.isa) == [.call "vg_f", .text "ret"]
#guard Arm.printer.function (.call "vg_f" (.block []) : Prog Arm.isa) == [.call "vg_f", .text "bx lr"]

#guard Rust.line printer.call (.call "vg_f") == "        r\"call {vg_f}\",\n"
#guard Rust.line Arm.printer.call (.call "vg_f") == "        r\"bl {vg_f}\",\n"
#guard Rust.line printer.call (.text "mov rax, QWORD PTR [rdi]") ==
  "        r\"mov rax, QWORD PTR [rdi]\",\n"

-- A raw string literal, unless the text has a `"` or a carriage return.
#guard Rust.literal "mov rax, rbx\nret" == "r\"mov rax, rbx\nret\""
#guard Rust.literal ".ascii \"a\\b\"\nret" == "\".ascii \\\"a\\\\b\\\"\\nret\""
#guard Rust.literal "a\rb" == "\"a\\rb\""

-- A run of lines that are text alone is one string literal, its lines
-- separated by newlines (as `naked_asm!` joins its arguments); the page of a
-- static's address, which a macro writes, is an argument of its own.
#guard Rust.template "bl" [.text "mov x0, x1", .call "vg_f", .sym "adrp x16, " .page "VG_T",
    .text "ret", .text ".p2align 6"] ==
  "        r\"mov x0, x1\nbl {vg_f}\",\n" ++
  "        concat!(\"adrp x16, \", vg_sym_page!(\"{VG_T}\")),\n" ++
  "        r\"ret\n.p2align 6\",\n"
#guard Rust.template "bl" [] == ""

/-! ## Frames

A frame is its push, its body and its pop, each printed as the target's
instructions. -/

-- Save the link register around a call (AArch64).
#guard text (AArch64.printer.function
    (.frame (.push .x30) (.call "vg_f" (.block [])) (.pop .x30) : Prog AArch64.isa)) == [
  "str x30, [sp, #-16]!", "<call vg_f>", "ldr x30, [sp], #16", "ret"]

-- The address of a static (AArch64): its page and the offset in the page, in the
-- syntax of the object format (`TCB/Rust.lean`).
#guard text (AArch64.printer.function (.block [.adrSym .x16 "VG_TABLE"] : Prog AArch64.isa)) == [
  "adrp x16, <page VG_TABLE>", "add x16, x16, <pageoff VG_TABLE>", "ret"]

-- The address of a static (ARMv7): `movw` and `movt` of its distance from the
-- `pc` the `add` between labels 2 and 3 reads, the same text in ARM and Thumb
-- code. `asm` gives the text `Rust.line` writes, for reading.
#guard text (Arm.printer.function (.block [.adrSym .r4 "VG_TABLE"] : Prog Arm.isa)) == [
  "movw r4, #:lower16:(<armpcrel VG_TABLE>", "movt r4, #:upper16:(<armpcrel VG_TABLE>", "2:",
  "add r4, pc", "3:", "bx lr"]
#guard Rust.line Arm.printer.call (.sym "movw r4, #:lower16:(" .armPcRel "VG_TABLE") ==
  "        r\"movw r4, #:lower16:({VG_TABLE} - (2f + 2 * (3f - 2f)))\",\n"
#guard Arm.Instr.asm (.adrSym .r4 "VG_TABLE") == [
  "movw r4, #:lower16:(VG_TABLE - (2f + 2 * (3f - 2f)))",
  "movt r4, #:upper16:(VG_TABLE - (2f + 2 * (3f - 2f)))", "2:", "add r4, pc", "3:"]
-- It writes the static's address to its register alone.
#guard match Arm.exec (.adrSym .lr "VG_TABLE")
    { gpr := fun _ => 17, sp := 4096, n := true, z := false, c := true, v := false,
      mem := fun _ => 0, rd := [], wr := [], syms := fun _ => 12288 } with
  | none => false
  | some s => s.gpr .lr == 12288 && s.gpr .r4 == 17 && s.sp == 4096 && s.n && !s.z && s.c && !s.v

-- Pass two arguments on the stack, inside a frame saving `lr` (ARMv7). A pop
-- loads the lowest word of its frame.
#guard text (Arm.printer.function
    (.frame (.push [.lr])
      (.frame (.push [.r0, .r1]) (.call "vg_f" (.block [])) (.pop .r2 8)) (.pop .lr 4) :
      Prog Arm.isa)) == [
  "push {lr}", "push {r0, r1}", "<call vg_f>", "ldr r2, [sp], #8", "ldr lr, [sp], #4", "bx lr"]

-- Pass two arguments on the stack (x86): the first pushed is the higher
-- address.
#guard text (X86.printer.function
    (.frame (.push [.ecx, .eax]) (.call "vg_f" (.block [])) (.pop .edx 2) : Prog X86.isa)) == [
  "push ecx", "push eax", "<call vg_f>", "pop edx", "pop edx", "ret"]

-- Pass two arguments on the stack, inside a frame saving `rbx` (x86-64).
#guard text (printer.function
    (.frame (.push [.rbx])
      (.frame (.push [.rcx, .rax]) (.call "vg_f" (.block [])) (.pop .rdx 2)) (.pop .rbx 1) :
      Prog X86_64.isa)) == [
  "push rbx", "push rcx", "push rax", "<call vg_f>", "pop rdx", "pop rdx", "pop rbx", "ret"]

#guard Rust.line Arm.printer.call (.text "push {r4, lr}") == "        r\"push {{r4, lr}}\",\n"

/-! ## CPU features

`Code.requires` collects what every instruction needs, through calls and
frames; `Rust.featureCheck` accepts exactly the declared set; the emitted
`# Safety` item and constant list it. -/

/-- A stand-in for an ISA's `requires`, over instructions named by strings. -/
def req : String → List String
  | "sha256rnds2" => ["sha"]
  | "pshufb" => ["ssse3"]
  | "pblendw" => ["sse4.1"]
  | _ => []

def featureSample : Code String Unit :=
  .seq (.block ["mov", "sha256rnds2", "sha256rnds2"])
    (.ite () (.loop (.call "vg_f" (.block ["pshufb"])) ())
      (.frame "push" (.block ["pblendw"]) "pop"))

#guard featureSample.requires req == ["sha", "sha", "ssse3", "sse4.1"]
#guard (Code.block ["mov", "add"] : Code String Unit).requires req == []

/-! `Rust.requiresDedup` and `Rust.firstSome?` go through the code of each
call once (`Rust.distinctCalls`): the features of `Code.requires` each once,
and an instruction through any call. -/

/-- Calls of `f` and `g` nested in a call of `k`, and a later call of `f`
whose code requires something else. -/
def nestedSample : Code String Unit :=
  .seq (.call "k" (.seq (.block ["mov"]) (.call "g" (.call "f" (.block ["sha256rnds2"])))))
    (.seq (.call "f" (.block ["sha256rnds2"])) (.call "f" (.block ["pblendw"])))

#guard Rust.requiresDedup req featureSample == ["sha", "sse4.1", "ssse3"]
#guard Rust.requiresDedup req nestedSample == ["sha", "sse4.1"]
#guard Rust.requiresDedup req (Code.block ["mov", "add"] : Code String Unit) == []
-- The value of each call's code, but for a later call of a function whose
-- value is the first call's.
#guard Rust.distinctCalls (Rust.ownRequires req []) nestedSample.calls ==
  [[], [], ["sha"], ["sse4.1"]]

/-- The instruction `bad`, which a printer cannot encode. -/
def badInstr (i : String) : Option String := if i == "bad" then some i else none

#guard Rust.firstSome? badInstr nestedSample == none
#guard Rust.firstSome? badInstr (.seq nestedSample (.call "f" (.block ["bad"]))) == some "bad"
#guard Rust.firstSome? badInstr
  (.call "k" (.call "g" (.call "f" (.block ["bad"]))) : Code String Unit) == some "bad"
#guard Rust.firstSome? badInstr (.frame "bad" (.block []) "pop" : Code String Unit) == some "bad"

/-- The error of a failed check. -/
def err : Except String Unit → Option String
  | .error e => some e
  | .ok _ => none

def safeDoc : String := "Does things.\n\n# Safety\n\n* `p` must be valid."

#guard (Rust.featureCheck "f" "No safety requirements." [] []).toBool
#guard (Rust.featureCheck "f" safeDoc (featureSample.requires req) ["sse4.1", "sha", "ssse3"]).toBool
-- An undeclared feature, an unneeded one, and a doc the item can't be added to.
#guard err (Rust.featureCheck "f" safeDoc ["sha", "ssse3"] ["sha"]) ==
  some "f requires the CPU feature ssse3 but does not declare it"
#guard err (Rust.featureCheck "f" safeDoc ["sha"] ["sha", "avx2"]) ==
  some "f declares the CPU feature avx2, which none of its code requires"
#guard err (Rust.featureCheck "f" "No safety requirements." ["sha"] ["sha"]) ==
  some "f needs CPU features but its doc has no `# Safety` section"
#guard err (Rust.featureCheck "f" (safeDoc ++ "\n\n# Panics\n\nNever.") ["sha"] ["sha"]) ==
  some "f needs CPU features but its doc does not end with its `# Safety` section"

#guard Rust.featureDoc safeDoc [] == safeDoc
#guard Rust.featureDoc safeDoc ["sha"] ==
  safeDoc ++ "\n* The CPU must support the `sha` target feature."
#guard Rust.featureDoc safeDoc ["sha", "ssse3"] ==
  safeDoc ++ "\n* The CPU must support the `sha` and `ssse3` target features."
#guard Rust.featureDoc safeDoc ["sha", "ssse3", "sse4.1"] ==
  safeDoc ++ "\n* The CPU must support the `sha`, `ssse3` and `sse4.1` target features."

#guard Rust.featuresConst "vg_f" [] == ""
#guard Rust.featuresConst "vg_sha256_compress_shani" ["sha", "ssse3"] ==
  "/// The CPU features `vg_sha256_compress_shani` requires (`Artifact.features`).\n\
  pub(crate) const VG_SHA256_COMPRESS_SHANI_FEATURES: crate::cpu::Features = \
  crate::cpu::Features::of(&[\"sha\", \"ssse3\"]);\n\n"

end VG.Test
