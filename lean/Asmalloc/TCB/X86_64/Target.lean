module

public import Asmalloc.TCB.X86_64.Print
public import Asmalloc.TCB.Artifact

/-!
# The x86-64 System V target

**Trusted.** Functions are emitted as Rust `extern "sysv64"` naked functions.

System V AMD64 ABI: integer/pointer arguments arrive in `rdi, rsi, rdx, rcx,
r8, r9`; the integer result is returned in `rax`; `rbx, rbp, rsp, r12–r15` are
callee-saved. The return address is at `[rsp]` on entry; the printer ends
every function with `ret`, so `abiPreserved` demands that `rsp` and the
return-address slot are unchanged on exit.

The SSE registers `xmm0`–`xmm15` are all caller-saved (System V AMD64
psABI §3.2.1, Figure 3.4: "No" under "callee-saved"; also on Windows, whose
own convention the functions do not use), and so are the upper halves of
the `ymm` registers that contain them and `zmm16`–`zmm31` (the psABI makes
no vector register callee-saved), so `abiPreserved` says nothing about them.

The control bits of MXCSR (15:6; bits 5:0 are the status flags, SDM Vol. 1
§10.2.3) are callee-saved: "The control bits of the MXCSR register are
callee-saved (preserved across calls), while the status bits are
caller-saved (not preserved)" (System V AMD64 psABI §3.4.1, "Special
Registers").

Not modelled: the direction flag (no modelled instruction changes it; it is
clear on entry and exit), the x87 control word (never modified), and the
red zone: a contract that grants write access below `rsp` must keep it within
the 128-byte red zone.
-/

@[expose] public section

namespace VG.X86_64

def calleeSaved : List Reg := [.rbx, .rbp, .rsp, .r12, .r13, .r14, .r15]

/-- Calling-convention obligations on return. -/
def abiPreserved (s s' : State) : Prop :=
  (∀ r ∈ calleeSaved, s'.gpr r = s.gpr r) ∧
  s'.mem.readW (s.gpr .rsp) 64 = s.mem.readW (s.gpr .rsp) 64 ∧
  s'.mxcsr.extractLsb' 6 10 = s.mxcsr.extractLsb' 6 10

/-- System V argument registers, in order. -/
def argRegs : List Reg := [.rdi, .rsi, .rdx, .rcx, .r8, .r9]

/-! ## The calling convention, for `Sig`

Each integer or pointer argument takes the next of `rdi, rsi,
rdx, rcx, r8, r9` (a 32-bit argument in the low half; the upper half is
unspecified). Once they are used up, each further argument takes the next
eightbyte on the stack, in order, the first at `[rsp + 8]` on entry, just
above the return address at `[rsp]`. System V AMD64 psABI §3.2.3,
"Parameter Passing": for class INTEGER "the next available register of the
sequence %rdi, %rsi, %rdx, %rcx, %r8 and %r9 is used"; "if there are no
registers available for any eightbyte of an argument, the whole argument is
passed on the stack", the arguments passed in memory being "pushed on the
stack in reversed (right-to-left) order"; Figure 3.3, "Stack Frame with Base
Pointer": `8n+16(%rbp)` holds "memory argument eightbyte n", which is
`8n+8(%rsp)` on entry, before the callee pushes `rbp`. A 32-bit argument is
in the low half of its eightbyte; the upper half is unspecified. `abi` only
grants reading the stack arguments, and assumes they do not wrap around the
end of the address space, nor the stack below `rsp` that the function's calls
and frames use (a frame's push faults if it would wrap: `Isa.lean`). The
integer result is in `rax`.
-/

/-- The address of the `i`-th (from 0) stack argument, on entry. -/
def stackArgAddr (s : State) (i : Nat) : Addr := s.gpr .rsp + BitVec.ofNat 64 (8 * (i + 1))

/-- The eightbyte of the `i`-th (from 0) stack argument, on entry. -/
def stackArg (s : State) (i : Nat) : BitVec 64 := s.mem.readW (stackArgAddr s i) 64

def abi : Abi isa where
  ptrBits := 64
  args ws := if ws.length ≤ argRegs.length then
    some fun s => (argRegs.take ws.length).map s.gpr
  else some fun s => argRegs.map s.gpr ++ (List.range (ws.length - argRegs.length)).map (stackArg s)
  argArea ws s := let n := ws.length - argRegs.length
    if n = 0 then [] else [(⟨stackArgAddr s 0, 8 * n⟩, false)]
  reserved n s := ⟨s.gpr .rsp, 8⟩ :: stackBelow (s.gpr .rsp) n
  -- The stack the function's calls and frames use, and its stack arguments,
  -- if any, do not wrap around.
  wf ws n s := if ws.length ≤ argRegs.length then
      match n with
      | 0 => True
      | n => n ≤ (s.gpr .rsp).toNat
    else
      (match n with
      | 0 => True
      | n => n ≤ (s.gpr .rsp).toNat) ∧
      (s.gpr .rsp).toNat + 8 * (ws.length - argRegs.length + 1) ≤ 2 ^ 64
  pub s₁ s₂ := s₁.gpr .rsp = s₂.gpr .rsp
  mem s := s.mem
  rd s := s.rd
  wr s := s.wr
  ret s := s.gpr .rax
  argAreaDoc ws := if ws.length - argRegs.length = 0 then none else
    some ("the arguments on the stack", false)
  reservedDoc n := some (if n = 0 then "the return address on the stack" else
    s!"the return address on the stack or the {n} bytes of stack below it")
  sym := some fun s name => s.syms name

abbrev target : Target where
  name := "x86_64"
  isa := isa
  printer := printer
  abiPreserved := abiPreserved
  -- x32 targets (`x86_64-unknown-linux-gnux32`) have 32-bit pointers. The
  -- model's baseline includes SSE2 (see `Instr.requires`), which
  -- `x86_64-unknown-none` and `x86_64-unknown-uefi` turn off; there the Rust
  -- caller would also not save the SSE registers, of which UEFI's convention
  -- (the Microsoft x64 one) makes `xmm6`–`xmm15` callee-saved.
  rustCfg := "all(target_arch = \"x86_64\", target_pointer_width = \"64\", \
    target_feature = \"sse2\")"
  rustAbi := "sysv64"
  abi := abi

end VG.X86_64
