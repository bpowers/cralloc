module

public import Asmalloc.Impl.Slab.X86_64
public import Asmalloc.Spec.CEntry

/-! # The C entry points on x86-64

docs/research/zen5-roofline.md, section 14, recommendations 1, 2 and 5,
with the front half inside the sequence: every routine arms first (`arm`),
reads the CPU number and leaves for the slow path if it has no slab
(`cpuSteps`), then finds its class into `r9`, the address of the class's
word in slab 0, `SLABS + 8 * class`, and ends with the out-of-line pop's or
push's steps on `r9` (`popSteps`, `pushSteps`). An abort restarts the whole
of it, so the arguments, in `rdi` and `rsi`, are never written: every exit
tail-calls a slow path with them as they arrived. The scratch registers are
`rax rcx rdx r8 r9 r10 r11`.

`malloc(size)`:

```
    cmp   rdi, 32769 ; jae slow
    lea   rax, [rdi + 7]
    shr   eax, 3
    lea   rcx, [rip + C_SIZES + 2048]
    movzx edx, BYTE PTR [rcx + rax]
    lea   r9, [rip + SLABS]
    lea   r9, [r9 + rdx*8]
```

`free(ptr)`, the region index from the shift and base index of the free's
mirror (`Spec.CEntry.Mirror`), at `D = Mirror.disp` above `SLABS`:

```
    test  rdi, rdi ; je done
    lea   r9, [rip + SLABS]
    mov   ecx, DWORD PTR [r9 + D]
    mov   rax, rdi
    shr   rax, cl
    sub   rax, QWORD PTR [r9 + D + 8]
    cmp   rax, 4096 ; jae slow
    lea   rdx, [r9 + REGION_DISP]
    movzx edx, BYTE PTR [rdx + rax]
    lea   r9, [r9 + rdx*8]
```

The sized free, the class predicted by the size and checked by the region:

```
    lea   r9, [rip + SLABS]
    lea   rcx, [rip + C_SIZED_SIZES + 2048]
    movzx eax, si
    movzx edx, BYTE PTR [rcx + rax]
    mov   ecx, DWORD PTR [r9 + D]
    mov   rax, rdi
    shr   rax, cl
    sub   rax, QWORD PTR [r9 + D + 8]
    cmp   rax, 4096 ; jae slow
    cmp   dl, BYTE PTR [r9 + rax + REGION_DISP] ; jne slow
    lea   r9, [r9 + rdx*8]
```
-/

@[expose] public section

namespace VG.Impl.CEntry.X86_64

open VG.X86_64 VG.Impl.Slab.X86_64
open VG.Spec.CEntry (Mirror)

/-- The slow path's name, an operand of each routine. -/
def slow : Option String := some "slow"

/-- `malloc`'s steps that find the class. -/
def mallocFront : List (Step Instr Cond) := [
  .instr (.alu .cmp .rdi (.imm 32769)),
  .exit .ae [] slow,
  .instr (.lea .rax { base := .rdi, disp := 7 }),
  .instr (.shift32 .shr .rax 3),
  .instr (.leaSym .rcx "C_SIZES" 2048),
  .instr (.movzx8 .rdx { base := .rcx, index := some .rax }),
  .instr (.leaSym .r9 "SLABS"),
  .instr (.lea .r9 { base := .r9, index := some .rdx, scale := 8 })]

/-- The steps that find the region index of `rdi` into `rax`, with `r9`
holding `SLABS`, from mirror `m`, and leave for the slow path if it is out of range. -/
def regionIndex (m : Mirror) : List (Step Instr Cond) := [
  .instr (.mov32 .rcx (.mem { base := .r9, disp := m.disp })),
  .instr (.mov .rax (.reg .rdi)),
  .instr (.shiftCl .shr .rax),
  .instr (.alu .sub .rax (.mem { base := .r9, disp := m.disp + 8 })),
  .instr (.alu .cmp .rax (.imm 4096)),
  .exit .ae [] slow]

/-- `free`'s steps that find the class. -/
def freeFront (m : Mirror) : List (Step Instr Cond) := [
  .instr (.alu .test .rdi (.reg .rdi)),
  .exit .e [] none,
  .instr (.leaSym .r9 "SLABS")] ++ regionIndex m ++ [
  .instr (.lea .rdx { base := .r9, disp := 33560576 }),
  .instr (.movzx8 .rdx { base := .rdx, index := some .rax }),
  .instr (.lea .r9 { base := .r9, index := some .rdx, scale := 8 })]

/-- The sized free's steps that find the class. -/
def freeSizedFront (m : Mirror) : List (Step Instr Cond) := [
  .instr (.leaSym .r9 "SLABS"),
  .instr (.leaSym .rcx "C_SIZED_SIZES" 2048),
  .instr (.movzx16 .rax .rsi),
  .instr (.movzx8 .rdx { base := .rcx, index := some .rax })] ++ regionIndex m ++ [
  .instr (.cmp8 .rdx { base := .r9, index := some .rax, disp := 33560576 }),
  .exit .ne [] slow,
  .instr (.lea .r9 { base := .r9, index := some .rdx, scale := 8 })]

/-- `calloc`'s steps that find the class: the product of `n` and `size` (`mul` writes `rdx`,
which no argument uses), leaving for the slow path if it overflows or is above 1024. -/
def callocFront : List (Step Instr Cond) := [
  .instr (.mov .rax (.reg .rdi)),
  .instr (.mul .rsi),
  .exit .b [] slow,
  .instr (.alu .cmp .rax (.imm 1025)),
  .exit .ae [] slow,
  .instr (.leaSym .rcx "C_CALLOC_SIZES" 2048),
  .instr (.movzx8 .rdx { base := .rcx, index := some .rax }),
  .instr (.leaSym .r9 "SLABS"),
  .instr (.lea .r9 { base := .r9, index := some .rdx, scale := 8 })]

/-- `c_calloc(n, size)`: the pop, then, after the commit and outside the sequence, the block
into `rdx` and a jump to the zeroing tail, with `n` and `size` as they arrived. -/
def cCalloc : Prog isa :=
  .seq (.rseq arm (cpuSteps [] slow ++ callocFront ++ popSteps .r9 [] slow))
    (.jump [.mov .rdx (.reg .rax)] "zero")

/-- `c_malloc(size)`. -/
def cMalloc : Prog isa := .rseq arm (cpuSteps [] slow ++ mallocFront ++ popSteps .r9 [] slow)

/-- `c_free(ptr)`, and the plain `delete` forms, by their mirrors. -/
def cFree (m : Mirror) : Prog isa :=
  .rseq arm (cpuSteps [] slow ++ freeFront m ++ pushSteps .r9 .rdi [] slow)

/-- `c_free_sized(ptr, size)`, and the sized `delete` forms. -/
def cFreeSized (m : Mirror) : Prog isa :=
  .rseq arm (cpuSteps [] slow ++ freeSizedFront m ++ pushSteps .r9 .rdi [] slow)

end VG.Impl.CEntry.X86_64
