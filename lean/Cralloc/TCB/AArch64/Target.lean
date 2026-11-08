module

public import Cralloc.TCB.AArch64.Print
public import Cralloc.TCB.Artifact

/-!
# The AArch64 target (AAPCS64)

**Trusted.** Functions are emitted as Rust `extern "C"` naked functions.

AAPCS64 ("Procedure Call Standard for the Arm 64-bit Architecture"): integer
and pointer arguments arrive in `x0`–`x7`, and the integer result is returned
in `x0`; `x19`–`x28`, `x29` and the stack pointer are callee-saved. The return
address is in `x30`, and the printer ends every function with `ret`, so `x30`
must be unchanged on exit. `x18` and `x29` cannot be named (`TCB/AArch64/Isa.lean`).

The SIMD registers, FPCR and FPSR are not modelled, and no modelled instruction
reads or writes them, so a function keeps whatever its caller left in them, as
the convention requires of the callee-saved parts (AAPCS64 §6.1.2). NZCV is
undefined on entry to and return from a public interface (AAPCS64 §6.1.1).
There is no red zone on Linux: nothing below the stack pointer belongs to the
caller, and no modelled instruction touches it.

The Rust `cfg` excludes `aarch64_be` targets, which are big-endian, and ILP32
ones, whose pointers have 32 bits.
-/

@[expose] public section

namespace VG.AArch64

/-- The registers a function must leave as it found them: AAPCS64 §6.1.1, and
`x30`, which `ret` returns to. -/
def calleeSaved : List Reg :=
  [.x19, .x20, .x21, .x22, .x23, .x24, .x25, .x26, .x27, .x28, .x30]

/-- Calling-convention obligations on return. The return address is in `x30`,
not in memory. -/
def abiPreserved (s s' : State) : Prop :=
  (∀ r ∈ calleeSaved, s'.gpr r = s.gpr r) ∧ s'.sp = s.sp

/-- AAPCS64 argument registers, in order. -/
def argRegs : List Reg := [.x0, .x1, .x2, .x3, .x4, .x5, .x6, .x7]

/-! ## The calling convention, for `Sig`

AAPCS64, "Parameter passing": each integer or pointer argument takes the next of
`x0`–`x7` (a 32-bit argument in the low half; the upper half is unspecified).
Further arguments, on the stack, are not modelled: a signature with more has a
precondition no state satisfies (`Abi.args`). The result is in `x0`. -/

def abi : Abi isa where
  ptrBits := 64
  args ws := if ws.length ≤ argRegs.length then
    some fun s => (argRegs.take ws.length).map s.gpr
  else none
  argArea _ _ := []
  reserved n s := stackBelow s.sp n
  wf _ n s := match n with
    | 0 => True
    | n => n ≤ s.sp.toNat
  mem s := s.mem
  rd s := s.rd
  wr s := s.wr
  ret s := s.gpr .x0
  argAreaDoc _ := none
  reservedDoc n := if n = 0 then none else some s!"the {n} bytes of stack below the stack pointer"
  sym := some fun s name => s.syms name
  tp := some fun s => s.tpidr
  setupFindsArea := true
  slabsOnPage := true
  maskBytes := 8

/-! ## Restartable sequences

glibc registers a `struct rseq` for every thread at the thread pointer plus the
value of its variable `__rseq_offset`, which is signed (glibc manual,
"Restartable Sequences"); on AArch64 the thread pointer is TPIDR_EL0. The field
`rseq_cs`, the address of the descriptor of the sequence the thread is in, is at
offset 8 and `cpu_id` at 4 (`include/uapi/linux/rseq.h`, `struct rseq`), read and
written through a register holding the area's address, there being no segment
to name it. The model reads `__rseq_offset` as the symbol of that name
(`State.syms`). The kernel reads `rseq_cs` as one 64-bit access, and an aligned
store of 64 bits is single-copy atomic (DDI 0487, "Single-copy atomicity").

A routine forms the descriptor's and the area's addresses once, in registers
(`Artifact.setup`), and its arm is the one store, as the emitter requires
(`Printer.armOneStore`): the abort handler jumps back to it, so a retry finds those
registers as the abort left them, which is what a retry's precondition asks of it. -/

/-- The address of the thread's rseq area. -/
def rseqArea (s : State) : Addr := s.tpidr + s.mem.readW (s.syms rseqOffsetSym) 64

/-- The address of the area's `rseq_cs` field. -/
def rseqCsAddr (s : State) : Addr := rseqArea s + 8

/-- `rseq_cs` points at the routine's descriptor. -/
def armed (s : State) : Prop := s.mem.readW (rseqCsAddr s) 64 = s.csDesc

/-- The instruction, run in `a`, stores the descriptor's address into
`rseq_cs`: a 64-bit store of a register holding it, to the field's address. -/
def arms : Instr → State → Prop
  | .str .x r m, a => a.ea m = rseqCsAddr a ∧ a.gpr r = a.csDesc
  | _, _ => False

/-- What an abort keeps (`Target.rely`). The kernel aborts a sequence on its way
back to user mode, by setting the saved program counter to the abort handler and
changing nothing else the thread saved (`rseq_entry.h`, the abort path;
docs/research/linux-rseq-contract.md, "Aborts"): the thread resumes with the
registers and PSTATE it was interrupted with, and its TPIDR_EL0. If a signal
handler ran first, `rt_sigreturn` restores them from the signal frame. The
program's symbols and descriptor are the same. The memory may differ; the
contract's `env` says how. So may the thread's permissions: they are what the
contract's precondition makes of the state the thread resumes in. -/
def rely (p p' : State) : Prop :=
  p'.gpr = p.gpr ∧ p'.sp = p.sp ∧ p'.nf = p.nf ∧ p'.zf = p.zf ∧ p'.cf = p.cf ∧ p'.vf = p.vf ∧
  p'.tpidr = p.tpidr ∧ p'.syms = p.syms ∧ p'.csDesc = p.csDesc

/-! ## The rseq area on musl

musl registers no rseq area for its threads, so the crate defines its own in the executable's
thread-local data and registers it itself (`crates/cralloc/src/front/area.rs`). A routine has
the area's distance from the thread pointer in its instructions (`Instr.addAreaHi`,
`TCB/AArch64/Isa.lean`), as there is no way to write it as data.

On this architecture the static thread-local data starts above the thread pointer, at the 16
bytes the ABI keeps there rounded up to the data's alignment, and the C library aligns the thread
pointer to it too: a page here, so the area lies at the same page offset in every thread, away
from the slab words the routines store to (docs/research/portability.md, "The area's page
offset"). -/

/-- The area, for musl: data, with the layout of `VG.X86_64.ownRseqArea`, whose constants the crate
asserts against. -/
def ownRseqArea : OwnArea where
  cfg := [.targetEnv "musl"]
  lines := [
    ".pushsection .tdata.cralloc_rseq_area,\"awT\",%progbits",
    s!".balign {ownAreaAlign}",
    s!".skip {ownAreaPageOffset}",
    s!".globl {ownAreaSym}",
    s!".hidden {ownAreaSym}",
    s!".type {ownAreaSym},%object",
    s!".size {ownAreaSym}, {ownAreaLen}",
    s!"{ownAreaSym}:",
    -- cpu_id_start, cpu_id; rseq_cs; flags, node_id, mm_cid and padding.
    s!".long 0, {hexLit ownAreaUnregistered}",
    ".quad 0",
    ".long 0, 0, 0, 0",
    ".popsection"]
  consts := ownAreaConsts

abbrev target : Target where
  name := "aarch64"
  isa := isa
  printer := printer
  abiPreserved := abiPreserved
  rustCfg := "all(target_arch = \"aarch64\", target_endian = \"little\", \
    target_pointer_width = \"64\")"
  rustAbi := "C"
  abi := abi
  arms := arms
  armed := armed
  rely := rely
  ownArea := some ownRseqArea
  consts := [("RSEQ_SIGNATURE_AARCH64", .u32, rseqSignature)]

/-! ## Inline functions

`inlineTarget` emits functions inline (`Target.inline`): an `#[inline(always)]`
Rust function whose body is one `asm!` block with `options(nostack)`. The
arguments are in registers of the routine's choosing, named as explicit register
operands (`inlineAbi`): a restart re-reads them, and the block's result, in `x0`,
may be written before the commit, so the first argument is not in `x0`. `asm!`
requires the stack pointer, and every register the block does not declare as an
output, to hold at its end what it held at its start (the Rust Reference,
"Inline assembly", "Rules for inline assembly"). There is no red zone to keep the
block from: it reserves no memory the calling convention does not. -/

/-- The registers an inline function's arguments are in, in order. -/
def inlineArgRegs : List Reg := [.x3, .x2, .x0]

/-- The calling convention as an inline function with its result in `r` sees it:
`abi`, but for where the arguments are and the result. -/
def inlineAbiOf (r : Reg) : Abi isa :=
  { abi with
    args ws := if ws.length ≤ inlineArgRegs.length then
      some fun s => (inlineArgRegs.take ws.length).map s.gpr
    else none
    ret s := s.gpr r }

/-- An inline function's, with its result in `x0`, where most have it. -/
abbrev inlineAbi : Abi isa := inlineAbiOf .x0

/-- An inline function's with its result in `x4`: one that has `x0` as an argument it
must keep until its commit (the inline push). -/
abbrev inlineAbiX4 : Abi isa := inlineAbiOf .x4

/-- Every register but those named in `rs` holds in `s'` what it held in `s`. -/
def preservedExcept (rs : List String) (s s' : State) : Prop :=
  ∀ r : Reg, r.x ∉ rs → s'.gpr r = s.gpr r

/-- The registers an `asm!` block may declare as clobbers: all it may name. LLVM
keeps `x19` for itself, and `asm!` refuses it as an operand. -/
def clobberable : List Reg := [.x0, .x1, .x2, .x3, .x4, .x5, .x6, .x7, .x8, .x9, .x10, .x11, .x12,
  .x13, .x14, .x15, .x16, .x17, .x20, .x21, .x22, .x23, .x24, .x25, .x26, .x27, .x28, .x30]

/-- The inline target, for a function with its result in `r`: the same model and
printer, with the obligations of `asm!` in place of a call's. Its conditional
branches may not leave the function, which the compiler may make any size. -/
abbrev inlineFor (r : Reg) : Target :=
  { target with
    name := "aarch64_inline"
    abi := inlineAbiOf r
    abiPreserved := fun s s' => s'.sp = s.sp
    inline := true
    argRegs := inlineArgRegs.map Reg.x
    retReg := r.x
    regs := clobberable.map Reg.x
    preservedExcept := preservedExcept
    -- The area is the target's; emitted once, with `target`.
    ownArea := none
    printer := { printer with stubExits := false, coldFar := true } }

abbrev inlineTarget : Target := inlineFor .x0

/-- For a function whose result is in `x4`. -/
abbrev inlineTargetX4 : Target := inlineFor .x4

end VG.AArch64
