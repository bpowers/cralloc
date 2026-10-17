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

then forms the slab word's address in `r9 + r8`, reads the word into `rcx`,
tests it with the mask and pops (`rax := [rcx - 8]`, commit `rcx - 8`) or
pushes (`[rcx] := p`, commit `rcx + 8`). Inline, the class's offset and the
mask are constants; out of line, the class is in `rdi` and the mask in the
`u32` table `MAG_MASKS`. -/

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

/-- `[r9 + r8]`: the slab word. -/
def slabWord : MemOp := { base := .r9, index := some .r8 }

/-- The slab word into `rcx`, `r9` holding the address of the class's word in
slab 0: inline, the class's offset is the constant `coff`. -/
def wordInline : List (Step Instr Cond) := [
  .instr (.leaSym .r9 "SLABS" 0 (some "coff")),
  .instr (.mov .rcx (.mem slabWord))]

/-- The same out of line, the class in `rdi`, and `r11` the masks' table. -/
def wordTable : List (Step Instr Cond) := [
  .instr (.leaSym .r9 "SLABS"),
  .instr (.lea .r9 { base := .r9, index := some .rdi, scale := 8 }),
  .instr (.mov .rcx (.mem slabWord)),
  .instr (.leaSym .r11 "MAG_MASKS")]

/-- The class's mask, out of line. -/
def maskEntry : Src := .mem { base := .r11, index := some .rdi, scale := 4 }

/-- Pop's tail: the slot below the word into `rax`, and the commit. -/
def popTail : List (Step Instr Cond) := [
  .instr (.mov .rax (.mem { base := .rcx, disp := -8 })),
  .instr (.lea .rcx { base := .rcx, disp := -8 }),
  .instr (.store slabWord .rcx)]

/-- Push's tail: `p` into the free slot, and the commit of `rax`, the word
plus 8. -/
def pushTail (p : Reg) : List (Step Instr Cond) := [
  .instr (.store { base := .rcx } p),
  .instr (.store slabWord .rax)]

/-- A miss of the inline pop: it returns 0. -/
def zero : List Instr := [.mov32 .rax (.imm 0)]

/-- `slab_pop` inline: 13 fused micro-ops. -/
def popInline : Prog isa := .rseq arm (
  cpuSteps zero none ++ wordInline ++ [
  .instr (.alu32 .test .rcx (.immP "mask")),
  .exit .e zero none] ++ popTail)

/-- `slab_push` inline of `p` in `rdi`: 13 fused micro-ops. -/
def pushInline : Prog isa := .rseq arm (
  cpuSteps [] (some "miss") ++ wordInline ++ [
  .instr (.lea .rax { base := .rcx, disp := 8 }),
  .instr (.alu32 .test .rax (.immP "mask")),
  .exit .e [] (some "miss")] ++ pushTail .rdi)

/-- `slab_pop` out of line, of the class in `rdi`. -/
def popTable : Prog isa := .rseq arm (
  cpuSteps [] (some "slow") ++ wordTable ++ [
  .instr (.alu32 .test .rcx maskEntry),
  .exit .e [] (some "slow")] ++ popTail)

/-- `slab_push` out of line, of `p` in `rsi` to the class in `rdi`. -/
def pushTable : Prog isa := .rseq arm (
  cpuSteps [] (some "slow") ++ wordTable ++ [
  .instr (.lea .rax { base := .rcx, disp := 8 }),
  .instr (.alu32 .test .rax maskEntry),
  .exit .e [] (some "slow")] ++ pushTail .rsi)

end VG.Impl.Slab.X86_64
