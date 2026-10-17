import Asmalloc.TCB.X86_64.Print
import Asmalloc.TCB.Rust
import Asmalloc.TCB.X86_64.Target

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
    | .ops ps => String.join (ps.map fun | .text s => s | .ref n => s!"<{n}>")

open X86_64

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
  .movzx8 .rax (.mk .rsi (some .rcx) 1 32 none),
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

/-- Memory operands naming the FS segment, of every width and form. -/
def sampleFs : Prog isa := .block [
  .mov .rax (.mem { base := .rdi, seg := some .fs }),
  .mov32 .r8 (.mem { base := .rdx, disp := 24, seg := some .fs }),
  .store { base := .rdx, disp := 8, seg := some .fs } .r10,
  .store32 { base := .rdi, index := some .rsi, scale := 4, disp := -320, seg := some .fs } .rcx,
  .alu .add .rax (.mem { base := .rdi, index := some .rsi, scale := 8, disp := -8, seg := some .fs }),
  .movzx8 .rcx { base := .rdx, disp := 3, seg := some .fs },
  .store8 { base := .rdx, seg := some .fs } .rcx
]

#guard text (printer.function sampleFs) == [
  "mov rax, QWORD PTR fs:[rdi]",
  "mov r8d, DWORD PTR fs:[rdx+24]",
  "mov QWORD PTR fs:[rdx+8], r10",
  "mov DWORD PTR fs:[rdi+rsi*4-320], ecx",
  "add rax, QWORD PTR fs:[rdi+rsi*8-8]",
  "movzx ecx, BYTE PTR fs:[rdx+3]",
  "mov BYTE PTR fs:[rdx], cl",
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
#guard printer.unencodable (.store { base := .rdx, disp := 2147483648, seg := some .fs } .rax) ==
  some "the displacement of the memory operand fs:[rdx+2147483648] does not fit in 32 bits"
#guard (printer.unencodable (.store8 { base := .rsi, disp := 4294967304 } .rax)).isSome
#guard (printer.unencodable (.adox .rax (.mem { base := .rsi, disp := -4294967304 }))).isSome
#guard printer.unencodable (.alu .add .rax (.imm 0x7fffffff)) == none
-- A scale the SIB byte cannot encode.
#guard printer.unencodable (.mov .rax (.mem { base := .rdi, index := some .rcx, scale := 3 })) ==
  some "the scale of the memory operand [rdi+rcx*3] is not 1, 2, 4 or 8"
#guard printer.unencodable (.lea .rax { base := .rdi, index := some .rcx, scale := 8 }) == none

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

-- `inc`, `dec`, and `test` with an immediate in both widths (the 64-bit
-- immediate is 32 bits, sign-extended, printed as its signed value).
#guard text (printer.function (.block [.inc .rax, .dec .r9, .alu32 .test .rcx (.imm 0x1f0),
    .alu .test .rax (.imm 0xfffffff0), .alu32 .test .r8 (.imm 0xfffffff0)] : Prog isa)) ==
  ["inc rax", "dec r9", "test ecx, 496", "test rax, -16", "test r8d, -16", "ret"]

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

-- The address of a static, or of a constant offset from it: a RIP-relative
-- operand, which `Rust.line` writes with the static's symbol. `asm` gives
-- the same text, for reading.
#guard text (printer.function (.block [.leaSym .r9 "VG_TABLE", .leaSym .r9 "VG_TABLE" 24,
    .leaSym .rax "VG_TABLE" (-8)] : Prog isa)) ==
  ["lea r9, [rip + <VG_TABLE>]", "lea r9, [rip + <VG_TABLE> + 24]", "lea rax, [rip + <VG_TABLE> - 8]",
    "ret"]
#guard Rust.line printer.call (.ops [.text "lea r9, [rip + ", .ref "VG_TABLE", .text "]"]) ==
  "        r\"lea r9, [rip + {VG_TABLE}]\",\n"
#guard Rust.line printer.call (.ops [.text "lea r9, [rip + ", .ref "VG_TABLE", .text " + 24]"]) ==
  "        r\"lea r9, [rip + {VG_TABLE} + 24]\",\n"
#guard Rust.line printer.call (.ops [.text "lea r9, [rip + ", .ref "VG_TABLE", .text " - 8]"]) ==
  "        r\"lea r9, [rip + {VG_TABLE} - 8]\",\n"
#guard Instr.asm (.leaSym .r9 "VG_TABLE") == ["lea r9, [rip + VG_TABLE]"]
#guard Instr.asm (.leaSym .r9 "VG_TABLE" 24) == ["lea r9, [rip + VG_TABLE + 24]"]
-- The offset joins a 32-bit displacement.
#guard printer.unencodable (.leaSym .r9 "VG_TABLE" (2 ^ 31 - 1)) == none
#guard printer.unencodable (.leaSym .r9 "VG_TABLE" (-2 ^ 31)) == none
#guard printer.unencodable (.leaSym .r9 "VG_TABLE" (2 ^ 31)) ==
  some "the offset 2147483648 from VG_TABLE does not fit in 32 bits"

-- A compile-time constant as an immediate or a symbol's offset, and a GOT
-- load: each refers to the operand by name.
#guard text (printer.function (.block [.alu32 .test .rcx (.immP "mask"),
    .leaSym .r9 "SLABS" 8 (some "off"), .movGot .rdx "rs", .mov .rax (.imm 1)] : Prog isa)) ==
  ["test ecx, <mask>", "lea r9, [rip + <SLABS> + 8 + <off>]",
    "mov rdx, QWORD PTR [rip + <rs>@GOTPCREL]", "mov rax, 1", "ret"]
#guard Rust.line printer.call (.ops [.text "test ecx, ", .ref "mask"]) ==
  "        r\"test ecx, {mask}\",\n"
#guard (printer.function (.block [.alu32 .test .rcx (.immP "mask")] : Prog isa)).flatMap Line.refs ==
  ["mask"]

-- `lea` of a memory operand: its address, with no size.
#guard text (printer.function (.block [.lea .rcx { base := .rcx, disp := -8 },
    .lea .rax { base := .rdi, index := some .rsi, scale := 8, disp := 16 },
    .lea .r9 { base := .r9, index := some .r8 }] : Prog isa)) ==
  ["lea rcx, [rcx-8]", "lea rax, [rdi+rsi*8+16]", "lea r9, [r9+r8*1]", "ret"]
-- No segment, and the displacement fits in 32 bits.
#guard printer.unencodable (.lea .rax { base := .rdx, seg := some .fs }) ==
  some "a segment override on `lea`, which computes an offset"
#guard printer.unencodable (.lea .rax { base := .rdx, disp := 2147483648 }) ==
  some "the displacement of the memory operand [rdx+2147483648] does not fit in 32 bits"
#guard printer.unencodable (.lea .rax { base := .rdx, disp := -8 }) == none

#guard Rust.escape "and {x}, [x1] \\ \"q\"" == "and {{x}}, [x1] \\\\ \\\"q\\\""

/-! ## Calls

A call is its own line, whatever the callee's code; the emitter renders it as
the call instruction with the callee's symbol as a `sym` operand. -/

/-- A call between two blocks. -/
def callSample : Prog X86_64.isa :=
  .seq (.block [.mov .rdi (.reg .rbx)]) (.seq (.call "vg_f" (.block [.alu .add .rax (.imm 1)]))
    (.block [.mov .rax (.reg .rdx)]))

#guard printer.function callSample == [
  .text "mov rdi, rbx", .call "vg_f", .text "mov rax, rdx", .text "ret"]

#guard Rust.line printer.call (.call "vg_f") == "        r\"call {vg_f}\",\n"
#guard Rust.line printer.call (.text "mov rax, QWORD PTR [rdi]") ==
  "        r\"mov rax, QWORD PTR [rdi]\",\n"

-- A raw string literal, unless the text has a `"` or a carriage return.
#guard Rust.literal "mov rax, rbx\nret" == "r\"mov rax, rbx\nret\""
#guard Rust.literal ".ascii \"a\\b\"\nret" == "\".ascii \\\"a\\\\b\\\"\\nret\""
#guard Rust.literal "a\rb" == "\"a\\rb\""

-- The lines are one string literal, separated by newlines (as `naked_asm!`
-- joins its arguments); calls and statics are named as operands.
#guard Rust.template "call" [.text "mov rax, rbx", .call "vg_f", .ops [.text "lea rcx, [rip + ", .ref "VG_T", .text "]"],
    .text "ret", .text ".p2align 6"] ==
  "        r\"mov rax, rbx\ncall {vg_f}\nlea rcx, [rip + {VG_T}]\nret\n.p2align 6\",\n"
#guard Rust.template "call" [] == ""

/-! ## Frames

A frame is its push, its body and its pop, each printed as the target's
instructions. -/

-- Pass two arguments on the stack, inside a frame saving `rbx`.
#guard text (printer.function
    (.frame (.push [.rbx])
      (.frame (.push [.rcx, .rax]) (.call "vg_f" (.block [])) (.pop .rdx 2)) (.pop .rbx 1) :
      Prog X86_64.isa)) == [
  "push rbx", "push rcx", "push rax", "<call vg_f>", "pop rdx", "pop rdx", "pop rbx", "ret"]

-- A stack buffer.
#guard text (printer.function (.frame (.alloc 64) (.block []) (.free 64) : Prog X86_64.isa)) ==
  ["lea rsp, [rsp-64]", "lea rsp, [rsp+64]", "ret"]

/-! ## Restartable sequences

The arm at label 5, the sequence from 6 to 7, the descriptor at 3 in its own
section, and out of line the signature, the abort handler at 4 jumping back
to the arm, and each exit's code. An exit straight to an external target
branches to it. -/

def rseqSample : Prog isa := .rseq
  [.movGot .rdx "rs", .mov .rdx (.mem { base := .rdx }), .leaRseqCs .r10,
    .store { base := .rdx, disp := 8, seg := some .fs } .r10]
  [.instr (.mov32 .r8 (.mem { base := .rdx, disp := 24, seg := some .fs })),
    .instr (.alu32 .cmp .r8 (.imm 8192)),
    .exit .ae [] (some "slow"),
    .instr (.mov .rcx (.mem { base := .r9, index := some .r8 })),
    .exit .e [.mov32 .rax (.imm 0)] none,
    .exit .b [] none,
    .instr (.store { base := .r9, index := some .r8 } .rcx)]

#guard text (printer.function rseqSample) == [
  "5:",
  "mov rdx, QWORD PTR [rip + <rs>@GOTPCREL]",
  "mov rdx, QWORD PTR [rdx]",
  "lea r10, [rip + 3f]",
  "mov QWORD PTR fs:[rdx+8], r10",
  "6:",
  "mov r8d, DWORD PTR fs:[rdx+24]",
  "cmp r8d, 8192",
  "jae <slow>",
  "mov rcx, QWORD PTR [r9+r8*1]",
  "je 20f",
  "jb 7f",
  "mov QWORD PTR [r9+r8*1], rcx",
  "7:",
  ".pushsection __rseq_cs, \"aw\"",
  ".balign 32",
  "3:",
  ".long 0, 0",
  ".quad 6b, 7b - 6b, 4f",
  ".popsection",
  ".pushsection .text.unlikely, \"ax\"",
  ".byte 0x0f, 0xb9, 0x3d",
  ".long 0x53053053",
  "4:",
  "jmp 5b",
  "20:",
  "mov eax, 0",
  "jmp 7b",
  ".popsection",
  "ret"
]

-- The line of an exit to an external target refers to it as an operand.
#guard (printer.function rseqSample).flatMap Line.refs == ["rs", "slow"]

-- Its hot path: the arm's 4, the load, the compare fused with its branch, the
-- load, two branches after a load (not fused), the commit; and the return.
#guard printer.hotUops false rseqSample == some 11
#guard printer.hotUops true rseqSample == some 10
#guard printer.uopLimit == some 24
#guard inlineTarget.printer.uopLimit == some 13

/-! ## CPU features

`Code.requires` collects what every instruction needs, through calls and
frames; `Rust.featureCheck` accepts exactly the declared set; the emitted
`# Safety` item and constant list it. -/

/-- A stand-in for an ISA's `requires`, over instructions named by strings. -/
def req : String → List String
  | "mulx" => ["bmi2"]
  | "adcx" => ["adx"]
  | "andn" => ["bmi1"]
  | _ => []

def featureSample : Code String Unit :=
  .seq (.block ["mov", "mulx", "mulx"])
    (.ite () (.loop (.call "vg_f" (.block ["adcx"])) ())
      (.frame "push" (.block ["andn"]) "pop"))

#guard featureSample.requires req == ["bmi2", "bmi2", "adx", "bmi1"]
#guard (Code.block ["mov", "add"] : Code String Unit).requires req == []

-- The model's own: only the BMI1, BMI2 and ADX instructions need any.
#guard Instr.requires (.mulx .rdx .rax (.reg .rcx)) == ["bmi2"]
#guard Instr.requires (.adox .rax (.reg .rcx)) == ["adx"]
#guard Instr.requires (.andn .rax .rcx .rdx) == ["bmi1"]
#guard Instr.requires (.cmov .e .rax (.reg .rcx)) == []

/-! `Rust.requiresDedup` and `Rust.firstSome?` go through the code of each
call once (`Rust.distinctCalls`): the features of `Code.requires` each once,
and an instruction through any call. -/

/-- Calls of `f` and `g` nested in a call of `k`, and a later call of `f`
whose code requires something else. -/
def nestedSample : Code String Unit :=
  .seq (.call "k" (.seq (.block ["mov"]) (.call "g" (.call "f" (.block ["mulx"])))))
    (.seq (.call "f" (.block ["mulx"])) (.call "f" (.block ["andn"])))

#guard Rust.requiresDedup req featureSample == ["bmi2", "bmi1", "adx"]
#guard Rust.requiresDedup req nestedSample == ["bmi2", "bmi1"]
#guard Rust.requiresDedup req (Code.block ["mov", "add"] : Code String Unit) == []
-- The value of each call's code, but for a later call of a function whose
-- value is the first call's.
#guard Rust.distinctCalls (Rust.ownRequires req []) nestedSample.calls ==
  [[], [], ["bmi2"], ["bmi1"]]

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
#guard (Rust.featureCheck "f" safeDoc (featureSample.requires req) ["bmi1", "bmi2", "adx"]).toBool
-- An undeclared feature, an unneeded one, and a doc the item can't be added to.
#guard err (Rust.featureCheck "f" safeDoc ["bmi2", "adx"] ["bmi2"]) ==
  some "f requires the CPU feature adx but does not declare it"
#guard err (Rust.featureCheck "f" safeDoc ["bmi2"] ["bmi2", "bmi1"]) ==
  some "f declares the CPU feature bmi1, which none of its code requires"
#guard err (Rust.featureCheck "f" "No safety requirements." ["bmi2"] ["bmi2"]) ==
  some "f needs CPU features but its doc has no `# Safety` section"
#guard err (Rust.featureCheck "f" (safeDoc ++ "\n\n# Panics\n\nNever.") ["bmi2"] ["bmi2"]) ==
  some "f needs CPU features but its doc does not end with its `# Safety` section"

#guard Rust.featureDoc safeDoc [] == safeDoc
#guard Rust.featureDoc safeDoc ["bmi2"] ==
  safeDoc ++ "\n* The CPU must support the `bmi2` target feature."
#guard Rust.featureDoc safeDoc ["bmi2", "adx"] ==
  safeDoc ++ "\n* The CPU must support the `bmi2` and `adx` target features."
#guard Rust.featureDoc safeDoc ["bmi2", "adx", "bmi1"] ==
  safeDoc ++ "\n* The CPU must support the `bmi2`, `adx` and `bmi1` target features."

#guard Rust.featuresConst "vg_f" [] == ""
#guard Rust.featuresConst "vg_mul_bmi2" ["bmi2", "adx"] ==
  "/// The CPU features `vg_mul_bmi2` requires (`Artifact.features`).\n\
  pub(crate) const VG_MUL_BMI2_FEATURES: &[&str] = &[\"bmi2\", \"adx\"];\n\n"

end VG.Test
