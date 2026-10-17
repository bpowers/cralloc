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

The vector registers, the x87 state and MXCSR are not modelled, and no
modelled instruction reads or writes them (`TCB/X86_64/Isa.lean`), so a
function keeps whatever its caller left in them, as the convention requires
of the callee-saved parts (System V AMD64 psABI §3.2.1 and §3.4.1: MXCSR's
control bits and the x87 control word).

Not modelled: the direction flag (no modelled instruction changes it; it is
clear on entry and exit) and the red zone: a contract that grants write
access below `rsp` must keep it within the 128-byte red zone.
-/

@[expose] public section

namespace VG.X86_64

def calleeSaved : List Reg := [.rbx, .rbp, .rsp, .r12, .r13, .r14, .r15]

/-- Calling-convention obligations on return. -/
def abiPreserved (s s' : State) : Prop :=
  (∀ r ∈ calleeSaved, s'.gpr r = s.gpr r) ∧
  s'.mem.readW (s.gpr .rsp) 64 = s.mem.readW (s.gpr .rsp) 64

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
  mem s := s.mem
  rd s := s.rd
  wr s := s.wr
  ret s := s.gpr .rax
  argAreaDoc ws := if ws.length - argRegs.length = 0 then none else
    some ("the arguments on the stack", false)
  reservedDoc n := some (if n = 0 then "the return address on the stack" else
    s!"the return address on the stack or the {n} bytes of stack below it")
  sym := some fun s name => s.syms name
  tp := some fun s => s.fsBase
  param := some fun s name => s.params name

/-! ## Restartable sequences

glibc registers a `struct rseq` for every thread at the thread pointer plus
the value of its variable `__rseq_offset` (glibc manual, "Restartable
Sequences": "the offset of the area relative to the thread pointer"); on
x86-64 the thread pointer is the FS segment's base (System V AMD64 psABI,
"Thread-Local Storage"). The field `rseq_cs`, the address of the
descriptor of the sequence the thread is in, is at offset 8, `cpu_id` at 4
and `mm_cid` at 24 (`include/uapi/linux/rseq.h`, `struct rseq`). The model
reads `__rseq_offset` as the symbol of that name (`State.syms`).
-/

/-- The name of glibc's variable holding the rseq area's offset from the
thread pointer. -/
def rseqOffsetSym : String := "__rseq_offset"

/-- The address of the thread's rseq area. -/
def rseqArea (s : State) : Addr := s.fsBase + s.mem.readW (s.syms rseqOffsetSym) 64

/-- The address of the area's `rseq_cs` field. -/
def rseqCsAddr (s : State) : Addr := rseqArea s + 8

/-- `rseq_cs` points at the routine's descriptor. -/
def armed (s : State) : Prop := s.mem.readW (rseqCsAddr s) 64 = s.csDesc

/-- The instruction, run in `a`, stores the descriptor's address into
`rseq_cs`: a 64-bit store of a register holding it, to the field's address.
(It then also changes nothing else, `store`.) -/
def arms : Instr → State → Prop
  | .store m r, a => a.ea m = rseqCsAddr a ∧ a.gpr r = a.csDesc
  | _, _ => False

/-- What an abort keeps (`Target.rely`). The kernel aborts a sequence on its
way back to user mode, by setting the saved instruction pointer to the abort
handler and changing nothing else the thread saved (`rseq_entry.h`, the
abort path; docs/research/linux-rseq-contract.md, "Aborts"): the thread
resumes with the registers and flags it was interrupted with, and its FS
base. If a signal handler ran first, `sigreturn` restores them from the
signal frame, which the handler leaves alone. The program's symbols,
constants and descriptor are the same. The memory may differ; the
contract's `env` says how. So may the thread's permissions: they are what
the contract's precondition makes of the state the thread resumes in (a
retry may find a different magazine, say, after a migration). -/
def rely (p p' : State) : Prop :=
  p'.gpr = p.gpr ∧ p'.cf = p.cf ∧ p'.zf = p.zf ∧ p'.sf = p.sf ∧ p'.of = p.of ∧
  p'.fsBase = p.fsBase ∧ p'.syms = p.syms ∧ p'.params = p.params ∧ p'.csDesc = p.csDesc ∧
  p'.unknowns = p.unknowns

abbrev target : Target where
  name := "x86_64"
  isa := isa
  printer := printer
  abiPreserved := abiPreserved
  -- x32 targets (`x86_64-unknown-linux-gnux32`) have 32-bit pointers.
  rustCfg := "all(target_arch = \"x86_64\", target_pointer_width = \"64\")"
  rustAbi := "sysv64"
  abi := abi
  arms := arms
  armed := armed
  rely := rely

end VG.X86_64
