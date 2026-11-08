module

public import Cralloc.TCB.AArch64.Isa

/-! # The front end's pop and push on AArch64

The sequences of docs/research/firestorm.md. A routine finds its sequence's
registers once, in its setup (`Artifact.setup`), and its arm is one store, to which
an abort returns, so that a retry reads no more than it must:

```
    adrp  x11, 3f
    add   x11, x11, :lo12:3f            ; the descriptor
    ldr   x9, [x11, #32]                ; the word after it: &__rseq_offset
    ldr   x9, [x9]
    mrs   x10, tpidr_el0
    add   x10, x10, x9                  ; the rseq area
5:  str   x11, [x10, #8]                ; arm; an abort returns here
```

A conditional branch may not leave a routine, which the compiler may make any size,
so a miss's result is set first: the sequence's first step writes it, a write a
restart repeats. So does every other register the body writes, and none of the
registers a retry reads: `x10`, `x11` and the arguments. -/

@[expose] public section

namespace VG.Impl.Slab.AArch64

open VG.AArch64

/-- What a routine runs once: the descriptor's address into `x11`, the rseq area's
into `x10`. -/
def setup : List Instr := [
  .adrp .x11 .desc,
  .addLo12 .x11 .x11 .desc,
  .ldrWord .x9 .x11 "__rseq_offset",
  .ldr .x .x9 { base := .x9 },
  .mrsTp .x10,
  .add .x .x10 .x10 (.reg .x9)]

/-- Arming: `rseq_cs` holds the descriptor's address. -/
def arm : List Instr := [.str .x .x11 { base := .x10, off := .imm 8 }]

/-- The CPU number into `x8`, and out of the sequence if it has no slab. -/
def cpuSteps : List (Step Instr Cond) := [
  .instr (.ldr .w .x8 { base := .x10, off := .imm 4 }),
  .instr (.cmp .w .x8 (.imm 8192)),
  .exit .hs [] none]

/-- `[x3, x8]`: the slab word of the class whose word in slab 0 is at `x3`, `x8`
holding the CPU's slab's offset. -/
def wordAt : MemOp := { base := .x3, off := .reg .x8 }

/-- `slab_pop` inline, of the class whose word in slab 0 is at `x3`, with the mask
in `w2`; the result is in `x0`, zero on a miss. -/
def popInline : Prog isa := .rseq arm (.instr (.mov .x .x0 (.imm 0)) :: cpuSteps ++ [
  .instr (.lsl .x .x8 .x8 12),
  .instr (.ldr .x .x1 wordAt),
  .instr (.tst .w .x1 (.reg .x2)),
  .exit .eq [] none,
  .instr (.ldr .x .x0 { base := .x1, off := .imm (-8) }),
  .instr (.sub .x .x1 .x1 (.imm 8)),
  .instr (.str .x .x1 wordAt)])

/-- `slab_push` inline of `p` in `x0`, to the class whose word in slab 0 is at `x3`,
with the mask in `w2`; the result is in `x4`, zero on a miss and the new word, which
is not zero, on a push. -/
def pushInline : Prog isa := .rseq arm (.instr (.mov .x .x4 (.imm 0)) :: cpuSteps ++ [
  .instr (.lsl .x .x8 .x8 12),
  .instr (.ldr .x .x1 wordAt),
  .instr (.add .x .x9 .x1 (.imm 8)),
  .instr (.tst .w .x9 (.reg .x2)),
  .exit .eq [] none,
  .instr (.str .x .x0 { base := .x1 }),
  .instr (.mov .x .x4 (.reg .x9)),
  .instr (.str .x .x9 wordAt)])

end VG.Impl.Slab.AArch64
