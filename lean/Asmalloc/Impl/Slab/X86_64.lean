module

public import Asmalloc.TCB.X86_64.Isa

/-! # The front end's pop and push on x86-64

The listing of docs/design.md, "Front end: magazines under rseq". Every
routine arms its sequence, then reads the CPU number and leaves for the slow path
if it has no slab:

```
    mov   r10, QWORD PTR [rip + __rseq_offset@GOTPCREL]
    mov   r10, QWORD PTR [r10]          ; the rseq area's offset from fs
    lea   r11, [rip + 3f]               ; the descriptor
    mov   QWORD PTR fs:[r10+8], r11     ; arm
6:  mov   r8d, DWORD PTR fs:[r10+4]    ; cpu_id
    cmp   r8d, 8192
    jae   slow
    shl   r8, 12
```

then reads the slab word at `[w + r8]` into `rcx`, `w` holding the address
of the class's word in slab 0, tests it with the mask and pops (`rax :=
[rcx - 8]`, commit `rcx - 8`) or pushes (`[rcx] := p`, commit `rcx + 8`).
No routine names a class or the slabs: inline, `w` is `rdi` and the mask is
in `rsi`; out of line, `w` is `rdi` and the mask is in the table entry
`MASK_DISP` bytes above the word, past the slabs. The out-of-line steps after the CPU
number's (`popSteps`, `pushSteps`) take `w` and the exits as parameters, so
that a longer routine can end with them. -/

@[expose] public section

namespace VG.Impl.Slab.X86_64

open VG.X86_64

/-- Arming, in `r10` and `r11` so that the third argument's register, `rdx`,
is free: `r10` holds the rseq area's offset from the FS base, and
`rseq_cs` the descriptor's address. -/
def arm : List Instr := [
  .movGot .r10 "__rseq_offset",
  .mov .r10 (.mem { base := .r10 }),
  .leaRseqCs .r11,
  .store { base := .r10, disp := 8, seg := some .fs } .r11]

/-- The CPU number into `r8`, and the exit for one without a slab. -/
def cpuSteps (cold : List Instr) (ext : Option String) : List (Step Instr Cond) := [
  .instr (.mov32 .r8 (.mem { base := .r10, disp := 4, seg := some .fs })),
  .instr (.alu32 .cmp .r8 (.imm 8192)),
  .exit .ae cold ext,
  .instr (.shift .shl .r8 12)]

/-- `[r9 + r8]`: the slow path's routines' slab word. -/
def slabWord : MemOp := { base := .r9, index := some .r8 }

/-- `[w + r8]`: the slab word, `w` holding the address of the class's word in
slab 0 and `r8` the CPU's slab's offset. -/
def wordAt (w : Reg) : MemOp := { base := w, index := some .r8 }

/-- The class's mask, for the C entry points: the entry `MASK_DISP` (33556480) bytes above
the class's word in slab 0. -/
def maskAt (w : Reg) : Src := .mem { base := w, disp := 33556480 }

/-- A miss of the inline pop: it returns 0. -/
def zero : List Instr := [.mov32 .rax (.imm 0)]

/-- `slab_pop` inline, of the class whose word in slab 0 is at `rdi`, with
the mask in `rsi`: 11 fused micro-ops, 12 with the caller's test of the
result. -/
def popInline : Prog isa := .rseq arm (cpuSteps zero none ++ [
  .instr (.mov .rcx (.mem (wordAt .rdi))),
  .instr (.alu32 .test .rcx (.reg .rsi)),
  .exit .e zero none,
  .instr (.mov .rax (.mem { base := .rcx, disp := -8 })),
  .instr (.lea .rcx { base := .rcx, disp := -8 }),
  .instr (.store (wordAt .rdi) .rcx)])

/-- `slab_push` inline of `p` in `rdx`, to the class whose word in slab 0 is
at `rdi`, with the mask in `rsi`: 11 fused micro-ops. -/
def pushInline : Prog isa := .rseq arm (cpuSteps [] (some "miss") ++ [
  .instr (.mov .rcx (.mem (wordAt .rdi))),
  .instr (.lea .rax { base := .rcx, disp := 8 }),
  .instr (.alu32 .test .rax (.reg .rsi)),
  .exit .e [] (some "miss"),
  .instr (.store { base := .rcx } .rdx),
  .instr (.store (wordAt .rdi) .rax)])

/-- An out-of-line pop's steps after the CPU number's, of the class whose
word in slab 0 is at `w`, exiting with `cold` to `ext` on a miss. The slot
address is formed before the load from it, so that `mov rax, [rcx]` is a
byte shorter than `mov rax, [rcx - 8]`. -/
def popSteps (w : Reg) (cold : List Instr) (ext : Option String) : List (Step Instr Cond) := [
  .instr (.mov .rcx (.mem (wordAt w))),
  .instr (.alu32 .test .rcx (maskAt w)),
  .exit .e cold ext,
  .instr (.lea .rcx { base := .rcx, disp := -8 }),
  .instr (.mov .rax (.mem { base := .rcx })),
  .instr (.store (wordAt w) .rcx)]

/-- An out-of-line push's steps after the CPU number's, of `p` to the class
whose word in slab 0 is at `w`. -/
def pushSteps (w p : Reg) (cold : List Instr) (ext : Option String) : List (Step Instr Cond) := [
  .instr (.mov .rcx (.mem (wordAt w))),
  .instr (.lea .rax { base := .rcx, disp := 8 }),
  .instr (.alu32 .test .rax (maskAt w)),
  .exit .e cold ext,
  .instr (.store { base := .rcx } p),
  .instr (.store (wordAt w) .rax)]

/-! ## The slow path's routines

Out of line, each on the word at the byte offset in `rdi`, rounded down to a
word of the slab (`and r9d, 4088`), so that the word's access is within the
slab whatever the offset: the store-if-equal touches nothing else, but the
stack routines' access to a link slot rests on the invariant for that
offset. `rax` is the word read, or `NO_CPU` (2) without a slab. -/

/-- The word at the offset in `rdi`, into `rax`; `[r9 + r8]` addresses it. -/
def offsetWord : List (Step Instr Cond) := [
  .instr (.mov32 .r9 (.reg .rdi)),
  .instr (.alu32 .and .r9 (.imm 4088)),
  .instr (.alu .add .r8 (.reg .r9)),
  .instr (.leaSym .r9 "SLABS"),
  .instr (.mov .rax (.mem slabWord))]

/-- No slab: return 2. -/
def noCpu : List Instr := [.mov32 .rax (.imm 2)]

/-- `slab_cas(offset, expected, new)`: store `new` (`rdx`) if the word is
`expected` (`rsi`); return the word. -/
def cas : Prog isa := .rseq arm (
  cpuSteps noCpu none ++ offsetWord ++ [
  .instr (.alu .cmp .rax (.reg .rsi)),
  .exit .ne [] none,
  .instr (.store slabWord .rdx)])

/-- `slab_stack_push(offset, magazine)`: unless the head is a marker, link
it from the magazine (`rsi`) and commit the magazine; return the head. -/
def stackPush : Prog isa := .rseq arm (
  cpuSteps noCpu none ++ offsetWord ++ [
  .instr (.alu32 .test .rax (.imm 1)),
  .exit .ne [] none,
  .instr (.store { base := .rsi } .rax),
  .instr (.store slabWord .rsi)])

/-- `slab_stack_pop(offset)`: unless the head is a marker or zero, commit
its link; return the head. -/
def stackPop : Prog isa := .rseq arm (
  cpuSteps noCpu none ++ offsetWord ++ [
  .instr (.alu32 .test .rax (.imm 1)),
  .exit .ne [] none,
  .instr (.alu .test .rax (.reg .rax)),
  .exit .e [] none,
  .instr (.mov .rcx (.mem { base := .rax })),
  .instr (.store slabWord .rcx)])

/-! ## The CPU number

Not a restartable sequence: `__rseq_offset` through its GOT slot, then the
rseq area's `cpu_id` through `fs`, zero-extended into `rax`. -/

/-- `current_cpu()`. -/
def currentCpu : Prog isa := .block [
  .movGot .rax "__rseq_offset",
  .mov .rax (.mem { base := .rax }),
  .mov32 .rax (.mem { base := .rax, disp := 4, seg := some .fs })]

/-! ## In an executable

An executable may read `__rseq_offset` directly (`movSym`, one load) rather
than through its GOT slot (`movGot` and a load): the linker copies the
variable into the executable before any code of the program runs. A shared
library cannot, so the GOT form is the default and this the cargo feature
`executable`'s. The routines are the same but for that. -/

/-- Arming with `__rseq_offset` read directly. -/
def armDirect : List Instr := [
  .movSym .r10 "__rseq_offset",
  .leaRseqCs .r11,
  .store { base := .r10, disp := 8, seg := some .fs } .r11]

/-- A sequence armed with `armDirect`. -/
def direct : Prog isa → Prog isa
  | .rseq _ body => .rseq armDirect body
  | c => c

/-- `current_cpu()`, reading `__rseq_offset` directly. -/
def currentCpuDirect : Prog isa := .block [
  .movSym .rax "__rseq_offset",
  .mov32 .rax (.mem { base := .rax, disp := 4, seg := some .fs })]

end VG.Impl.Slab.X86_64
