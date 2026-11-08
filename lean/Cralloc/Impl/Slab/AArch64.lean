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

/-- `setup` on musl, where the area is the crate's own and the linker writes its distance from the
thread pointer into two instructions (`Instr.addAreaHi`). -/
def muslSetup : List Instr := [
  .adrp .x11 .desc,
  .addLo12 .x11 .x11 .desc,
  .mrsTp .x10,
  .addAreaHi .x10 .x10,
  .addAreaLo .x10 .x10]

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

/-! ## The slow path's routines

Out of line, each on the word at the byte offset in `x0`, rounded down to a word of the
slab (`and x9, x0, #0xff8`), so that the word's access is within the slab whatever the
offset. `x3` holds the result: `NO_CPU` (2) until the word is read, and the word then. It
leaves for `x0` only after the sequence, since a restart reads the offset in `x0`. -/

/-- A slow path's routine's setup: also the slabs' address, into `x13`. `adrp` alone makes
it, as `SLABS` starts on a page (`Spec.Slab.Ctx.Layout`), and in the setup, as a linker may
move a load after an `adrp` out of a sequence (`TCB/AArch64/Print.lean`). -/
def slowOf (l : List Instr) : List Instr := .adrp .x13 (.sym "SLABS") :: l

def slowSetup : List Instr := slowOf setup
def slowMuslSetup : List Instr := slowOf muslSetup

/-- `[x13, x8]`: the slow path's routines' slab word. -/
def slabWord : MemOp := { base := .x13, off := .reg .x8 }

/-- What follows the sequence: the result, where the convention returns it. -/
def retWord : List Instr := [.mov .x .x0 (.reg .x3)]

/-- The word at the offset in `x0`, into `x3`; `[x13, x8]` addresses it. -/
def offsetWord : List (Step Instr Cond) := [
  .instr (.lsl .x .x8 .x8 12),
  .instr (.and .x .x9 .x0 (.imm 0xff8)),
  .instr (.add .x .x8 .x8 (.reg .x9)),
  .instr (.ldr .x .x3 slabWord)]

/-- A slow path's routine, of the steps `steps` after its word's. -/
def slowRoutine (steps : List (Step Instr Cond)) : Prog isa :=
  .seq (.rseq arm (.instr (.mov .x .x3 (.imm 2)) :: cpuSteps ++ offsetWord ++ steps)) (.block retWord)

/-- `slab_cas(offset, expected, new)`: store `new` (`x2`) if the word is `expected` (`x1`);
return the word. -/
def cas : Prog isa := slowRoutine [
  .instr (.cmp .x .x3 (.reg .x1)),
  .exit .ne [] none,
  .instr (.str .x .x2 slabWord)]

/-- `slab_stack_push(offset, magazine)`: unless the head is a marker, link it from the
magazine (`x1`) and commit the magazine; return the head. -/
def stackPush : Prog isa := slowRoutine [
  .exit (.tbnz .x3 0) [] none,
  .instr (.str .x .x3 { base := .x1 }),
  .instr (.str .x .x1 slabWord)]

/-- `slab_stack_pop(offset)`: unless the head is a marker or zero, commit its link; return
the head. -/
def stackPop : Prog isa := slowRoutine [
  .exit (.tbnz .x3 0) [] none,
  .exit (.cbz .x .x3) [] none,
  .instr (.ldr .x .x4 { base := .x3 }),
  .instr (.str .x .x4 slabWord)]

/-! ## The CPU number

Not a restartable sequence: `__rseq_offset` through a word holding its address, then the
rseq area's `cpu_id`, zero-extended into `x0`. -/

/-- `current_cpu()`. -/
def currentCpu : Prog isa := .block [
  .movGot .x9 "__rseq_offset",
  .ldr .x .x9 { base := .x9 },
  .mrsTp .x10,
  .add .x .x10 .x10 (.reg .x9),
  .ldr .w .x0 { base := .x10, off := .imm 4 }]

/-- `current_cpu()` on musl (`muslSetup`). -/
def currentCpuMusl : Prog isa := .block [
  .mrsTp .x10,
  .addAreaHi .x10 .x10,
  .addAreaLo .x10 .x10,
  .ldr .w .x0 { base := .x10, off := .imm 4 }]

/-- `area_offset()`, on musl: what the crate needs to find the area it registers. -/
def areaOffset : Prog isa := .block [
  .mov .x .x0 (.imm 0),
  .addAreaHi .x0 .x0,
  .addAreaLo .x0 .x0]

end VG.Impl.Slab.AArch64
