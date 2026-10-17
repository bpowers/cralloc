module

public import Asmalloc.TCB.X86_64.Isa
public import Asmalloc.TCB.Print

/-!
# Intel-syntax printer for the x86-64 model

**Trusted.** Emits Intel syntax without register prefixes, which is the
default dialect of Rust's `asm!`/`naked_asm!` on x86-64.
-/

@[expose] public section

namespace VG.X86_64

def Reg.name : Reg → String
  | .rax => "rax" | .rcx => "rcx" | .rdx => "rdx" | .rbx => "rbx"
  | .rsp => "rsp" | .rbp => "rbp" | .rsi => "rsi" | .rdi => "rdi"
  | .r8 => "r8" | .r9 => "r9" | .r10 => "r10" | .r11 => "r11"
  | .r12 => "r12" | .r13 => "r13" | .r14 => "r14" | .r15 => "r15"

/-- The 32-bit name of a register (its low 32 bits). -/
def Reg.name32 : Reg → String
  | .rax => "eax" | .rcx => "ecx" | .rdx => "edx" | .rbx => "ebx"
  | .rsp => "esp" | .rbp => "ebp" | .rsi => "esi" | .rdi => "edi"
  | .r8 => "r8d" | .r9 => "r9d" | .r10 => "r10d" | .r11 => "r11d"
  | .r12 => "r12d" | .r13 => "r13d" | .r14 => "r14d" | .r15 => "r15d"

/-- The 8-bit name of a register (its low byte). -/
def Reg.name8 : Reg → String
  | .rax => "al" | .rcx => "cl" | .rdx => "dl" | .rbx => "bl"
  | .rsp => "spl" | .rbp => "bpl" | .rsi => "sil" | .rdi => "dil"
  | .r8 => "r8b" | .r9 => "r9b" | .r10 => "r10b" | .r11 => "r11b"
  | .r12 => "r12b" | .r13 => "r13b" | .r14 => "r14b" | .r15 => "r15b"

/-- `[base+index*scale+disp]`, after `fs:` if the operand names the FS
segment: the segment-override syntax of Intel's assembler, which LLVM's
accepts and encodes with the FS segment-override prefix, `64H` (SDM Vol. 2
§2.1.1, "Instruction Prefixes"). -/
def MemOp.addr (m : MemOp) : String :=
  let seg := match m.seg with
    | none => ""
    | some .fs => "fs:"
  let idx := match m.index with
    | none => ""
    | some i => s!"+{i.name}*{m.scale}"
  let d := if m.disp = 0 then "" else if m.disp > 0 then s!"+{m.disp}" else s!"{m.disp}"
  s!"{seg}[{m.base.name}{idx}{d}]"

def MemOp.str (m : MemOp) : String := s!"QWORD PTR {m.addr}"

def MemOp.str32 (m : MemOp) : String := s!"DWORD PTR {m.addr}"

def MemOp.str8 (m : MemOp) : String := s!"BYTE PTR {m.addr}"

def Src.str : Src → String
  | .reg r => r.name
  | .imm v => toString v.toInt
  | .immP n => "{" ++ n ++ "}"
  | .mem m => m.str

/-- A 32-bit source operand. -/
def Src.str32 : Src → String
  | .reg r => r.name32
  | .imm v => toString v.toInt
  | .immP n => "{" ++ n ++ "}"
  | .mem m => m.str32

def AluOp.name : AluOp → String
  | .add => "add" | .adc => "adc" | .sub => "sub" | .sbb => "sbb" | .and => "and"
  | .or => "or" | .xor => "xor" | .cmp => "cmp" | .test => "test"

def ShiftOp.name : ShiftOp → String
  | .ror => "ror" | .shr => "shr" | .shl => "shl"

def VarShiftOp.name : VarShiftOp → String
  | .shl => "shl" | .shr => "shr" | .sar => "sar"

def ShiftxOp.name : ShiftxOp → String
  | .shlx => "shlx" | .shrx => "shrx"

def Cond.name : Cond → String
  | .e => "e" | .ne => "ne" | .b => "b" | .ae => "ae"

def Instr.asm : Instr → List String
  | .mov d s => [s!"mov {d.name}, {s.str}"]
  | .store m r => [s!"mov {m.str}, {r.name}"]
  | .alu op d s => [s!"{op.name} {d.name}, {s.str}"]
  | .inc d => [s!"inc {d.name}"]
  | .dec d => [s!"dec {d.name}"]
  | .mov32 d s => [s!"mov {d.name32}, {s.str32}"]
  | .store32 m r => [s!"mov {m.str32}, {r.name32}"]
  | .alu32 op d s => [s!"{op.name} {d.name32}, {s.str32}"]
  | .shift32 op d n => [s!"{op.name} {d.name32}, {n}"]
  | .bswap32 d => [s!"bswap {d.name32}"]
  | .rorx32 d r n => [s!"rorx {d.name32}, {r.name32}, {n}"]
  | .andn32 d a b => [s!"andn {d.name32}, {a.name32}, {b.name32}"]
  | .rorx d r n => [s!"rorx {d.name}, {r.name}, {n}"]
  | .andn d a b => [s!"andn {d.name}, {a.name}, {b.name}"]
  | .movzx8 d m => [s!"movzx {d.name32}, {m.str8}"]
  | .store8 m r => [s!"mov {m.str8}, {r.name8}"]
  | .bswap d => [s!"bswap {d.name}"]
  | .shift op d n => [s!"{op.name} {d.name}, {n}"]
  | .shiftCl op d => [s!"{op.name} {d.name}, cl"]
  | .shiftx op d r c => [s!"{op.name} {d.name}, {r.name}, {c.name}"]
  -- `movabs` always selects the `REX.W + B8+rd io` encoding, whatever the value.
  | .movImm64 d v => [s!"movabs {d.name}, {v.toInt}"]
  | .leaSym d name off offP =>
    [s!"lea {d.name}, [rip + {name}{symOffset off}{(offP.map (" + {" ++ · ++ "}")).getD ""}]"]
  | .movGot d name => [s!"mov {d.name}, QWORD PTR [rip + {name}@GOTPCREL]"]
  | .leaRseqCs d => [s!"lea {d.name}, [rip + 3f]"]
  | .lea d m => [s!"lea {d.name}, {m.addr}"]
  | .mul r => [s!"mul {r.name}"]
  | .imul d r => [s!"imul {d.name}, {r.name}"]
  | .mulx hi lo s => [s!"mulx {hi.name}, {lo.name}, {s.str}"]
  | .adcx d s => [s!"adcx {d.name}, {s.str}"]
  | .adox d s => [s!"adox {d.name}, {s.str}"]
  | .cmov c d s => [s!"cmov{c.name} {d.name}, {s.str}"]
  | .push rs => rs.map fun r => s!"push {r.name}"
  | .pop r k => List.replicate k s!"pop {r.name}"
  | .alloc bytes => [s!"lea rsp, [rsp-{bytes}]"]
  | .free bytes => [s!"lea rsp, [rsp+{bytes}]"]

/-- Whether the displacement of `m` fits in the 32-bit field it is encoded
in, which the processor sign-extends: `-2^31 ≤ disp < 2^31`. SDM Vol. 2
§2.1.5, "Addressing-Mode Encoding of ModR/M and SIB Bytes" (Tables 2-2 and
2-3: the displacement of a ModR/M or SIB memory operand is `disp8` or
`disp32`), and §2.2.1.3, "Displacement", on 64-bit mode: "The ModR/M and
SIB displacement sizes do not change. They remain 8 bits or 32 bits and are
sign-extended to 64 bits." The model adds `disp` to the address as an
unbounded integer (`State.ea`), and an assembler given a wider one may
silently truncate it (LLVM before 22 does), so such an operand must not be
printed. -/
def MemOp.dispOk (m : MemOp) : Bool := decide (-2 ^ 31 ≤ m.disp ∧ m.disp < 2 ^ 31)

/-- Whether the scale of `m` is one the SIB byte encodes: 1, 2, 4 or 8 (SDM
Vol. 2 §2.1.5, Table 2-3, the `SS` field). The model multiplies by any
number (`State.offset`); only these may be printed. -/
def MemOp.scaleOk (m : MemOp) : Bool := m.scale == 1 || m.scale == 2 || m.scale == 4 || m.scale == 8

def Src.memOps : Src → List MemOp
  | .reg _ | .imm _ | .immP _ => []
  | .mem m => [m]

/-- The memory operands of an instruction. -/
def Instr.memOps : Instr → List MemOp
  | .mov _ s | .alu _ _ s | .mov32 _ s | .alu32 _ _ s | .mulx _ _ s | .adcx _ s | .adox _ s
  | .cmov _ _ s => s.memOps
  | .store m _ | .store32 m _ | .movzx8 _ m | .store8 m _ | .lea _ m => [m]
  | .inc _ | .dec _ | .shift32 .. | .bswap32 _ | .rorx32 .. | .andn32 .. | .rorx .. | .andn .. | .bswap _
  | .shift .. | .shiftCl .. | .shiftx .. | .movImm64 .. | .leaSym .. | .movGot .. | .leaRseqCs _ | .mul _ | .imul ..
  | .push _ | .pop .. | .alloc _ | .free _ => []

/-- The lines of an instruction, as `TCB/Rust.lean` writes them: those of
`asm`, but for the instructions that name a symbol or a compile-time
constant, whose lines refer to it as an operand of the `asm!` block
(`Piece.ref`). `asm` gives the same text, for reading. -/
def Instr.lines : Instr → List Line
  | .leaSym d name off offP =>
    [.ops ([.text s!"lea {d.name}, [rip + ", .ref name, .text (symOffset off)] ++
      (match offP with | some p => [.text " + ", .ref p] | none => []) ++ [.text "]"])]
  | .movGot d name => [.ops [.text s!"mov {d.name}, QWORD PTR [rip + ", .ref name, .text "@GOTPCREL]"]]
  | .mov d (.immP n) => [.ops [.text s!"mov {d.name}, ", .ref n]]
  | .alu op d (.immP n) => [.ops [.text s!"{op.name} {d.name}, ", .ref n]]
  | .mov32 d (.immP n) => [.ops [.text s!"mov {d.name32}, ", .ref n]]
  | .alu32 op d (.immP n) => [.ops [.text s!"{op.name} {d.name32}, ", .ref n]]
  | i => i.asm.map .text

def printer : Printer isa where
  instr := Instr.lines
  branch c l := s!"j{c.name} {l}"
  jump l := s!"jmp {l}"
  ret := ["ret"]
  call := "call"
  /- Every function starts on a 64-byte boundary, a cache line: the unit
  AMD's op cache builds its entries from (Zen 4 Software Optimization Guide,
  57647, §2.9.1), and one of the 16- and 32-byte windows Intel's decoders and
  decoded ICache work in. So where a function's code, and its loops, lie
  relative to them is fixed by the function alone: a change to other code
  cannot move them, and with them its performance. -/
  funcAlign := [".p2align 6"]
  /- glibc registers its threads' rseq areas with the signature `0x53053053`
  on x86 (`RSEQ_SIG`, sysdeps/unix/sysv/linux/x86/bits/rseq.h), which the
  kernel requires in the four bytes before an abort handler. The three bytes
  before it make the seven decode as `ud1 edi, DWORD PTR [rip + 0x53053053]`
  (`0F B9 /r`, SDM Vol. 2 "UD"), as the kernel's selftests do, so that a
  disassembler stays in step and a stray jump into them traps. -/
  rseqSig := some [".byte 0x0f, 0xb9, 0x3d", ".long 0x53053053"]
  plainStore | .store .. => true | _ => false
  unencodable i := match i with
    -- `lea` computes an offset, which a segment does not change (`State.offset`);
    -- one written on it is a mistake.
    | .lea _ { seg := some _, .. } => some "a segment override on `lea`, which computes an offset"
    -- The constant joins the linker's 32-bit displacement (`exec`); refuse one an
    -- assembler might truncate before the linker sees it, as for `dispOk`.
    | .leaSym _ name off _ =>
      if -2 ^ 31 ≤ off ∧ off < 2 ^ 31 then none
      else some s!"the offset {off} from {name} does not fit in 32 bits"
    | _ => match i.memOps.filter (!·.dispOk), i.memOps.filter (!·.scaleOk) with
      | m :: _, _ => some s!"the displacement of the memory operand {m.addr} does not fit in 32 bits"
      | [], m :: _ => some s!"the scale of the memory operand {m.addr} is not 1, 2, 4 or 8"
      | [], [] => none

end VG.X86_64
