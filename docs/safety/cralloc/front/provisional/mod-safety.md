# Safety ledger: `crates/cralloc/src/front/provisional/`

The aarch64 routines are hand-written until Lean emits them. Each does what the x86-64 routine of its
name does, by the instructions `docs/research/firestorm.md` chose, and has that routine's contract, so
what `Real` and the C layer owe them is already discharged (`real-safety.md`, `c/routines-safety.md`), but for the red zone, which this
architecture does not have. What x86-64's proofs give the generated routines, these have only by the
argument below and by test: the twins' differential test and the breakpoint tests run them. Nobody
has reviewed these entries, and the Lean proofs replace them.

## PROVISIONAL-01: the slab routines (`aarch64/slab.rs`, `aarch64_inline/slab.rs`, the self-test)

**Blocks.** The `naked_asm!` of `slab_cas`, `slab_stack_push`, `slab_stack_pop` and
`vg_selftest_add`, and the `asm!` of `slab_pop`, `slab_push` and `current_cpu`.

**Preconditions.** Those of the routine's x86-64 contract, `VG.Spec.Slab.*Contract`.

**Proof.**

1. *The area.* `mrs tpidr_el0` plus `__rseq_offset` is the calling thread's rseq area, by glibc's
   definition of the variable (signed; the routine adds it). The routine finds the variable's address
   in a word of its own, 32 bytes past its descriptor (`.quad __rseq_offset`), or, for `current_cpu`,
   in a word in `.data.rel.ro`. On musl the area is the crate's own (PROVISIONAL-03), and the offset
   the linker's: `:tprel_hi12:` and `:tprel_lo12_nc:` of it.
2. *The sequence.* The descriptor names `6..7` and the abort handler `4`. The handler is preceded by
   the signature glibc registers on aarch64, `0xd428bc00` (`sysdeps/unix/sysv/linux/aarch64/bits/rseq.h`):
   the kernel reads the four bytes before the abort address and kills a thread on a mismatch. The
   handler jumps to `5`, the arming store, since the kernel clears `rseq_cs` when it aborts. x10 and
   x11 hold the area and the descriptor for it: the thread is the same one, and nothing writes them
   after `5`.
3. *One commit.* Between `6` and `7` the only stores are to a slot of the magazine the word names,
   which nothing can observe until the commit, and the commit, the last instruction before `7`.
   So an abort at any instruction leaves no change another thread can see. A store is a single
   64-bit `str` to an 8-aligned address, which is single-copy atomic, so the drain's exchange on
   another CPU sees the word before or after it.
4. *The restart.* Everything from `5` reads an input no instruction writes, x10 or x11, or something
   the sequence sets after `6`: an inlined pop's x0, a push's x4, the slab routines' x3 (`NO_CPU`),
   set first of all. The operands are in fixed registers, which the compiler keeps apart. A naked
   routine writes the result's register only after `7` and otherwise only x1 to x15 that are not
   its arguments: `c_calloc` and `c_free_sized` leave x1, their second argument, alone.
5. *The branch.* Each conditional branch's target is in its own routine: its end `7`, where the
   result is as the routine set it first, or the stub after a naked routine's `ret`. The assembler
   resolves each, and none needs a veneer. The abort handler's `b` is in the same section, past the
   function's end, and a function does not reach 128 MiB. A naked routine's tail calls may get a
   veneer, whose x16 and x17 hold nothing the slow path takes.
6. *The miss.* A CPU number of 8192 or more, as an unregistered thread's -1 or -2 reads unsigned,
   branches before any address is formed from it. Nothing has been stored. A miss leaves the
   sequence, so `rseq_cs` names a sequence the thread is not in, which the kernel clears when it next
   looks.
7. *The ABI.* The naked routines write only registers AAPCS64 lets a callee write and the flags;
   none uses the stack or x16, x17 or x18. The `asm!` blocks declare every register they write, and
   leave `nostack` true: no stack, and the kernel builds a signal frame below `sp`, there being no
   red zone.
8. *Ordering.* A slab's words are read and written by one CPU at a time, and a thread that moves
   does so through the kernel, whose context switch orders its memory accesses, so plain loads and
   stores see each other in program order as they do on x86-64. The drain's reads rest on the fence
   (KERNEL-02), which runs a full barrier on the CPU it names.

## PROVISIONAL-02: the C entry points (`aarch64/centry.rs`, `aarch64/exports.rs`)

**Blocks.** The `naked_asm!` of `c_malloc`, `c_calloc`, `c_free` and `c_free_sized`, and their copies
under the C names.

**Preconditions.** Those of the routine's x86-64 contract, `VG.Spec.CEntry.*Contract`.

**Proof.** As PROVISIONAL-01, with these differences.

1. *The tables.* A table's address comes by `adrp` before the arming store, with the displacements
   of the specification, which `aarch64/mod.rs` asserts, and `SLABS` is page-aligned. Each index is
   in range as on x86-64: the size compared first for `malloc` and `calloc`, the low 16 bits for the
   sized free's table, and the compare with 4096 for the region table. `calloc`'s product has its
   high word checked by `umulh`, as x86-64's multiply checks overflow, and is compared with 1024
   before it indexes. A position read from a table is a byte, below the 256 entries of the masks'
   table, and a zero mask misses. The hot word is at `[x3, x5, lsl #3]`, this CPU's slab indexed by
   that position.
2. *The arguments and the setup.* x0 and x1 are not written before a commit by a routine that takes
   them, and the registers formed before the arming store (x2 in `c_free_sized`, x10 to x15) are not
   written in the sequence at all, so a restart finds them, and a miss reaches its slow path with the
   arguments by one `b`. `c_malloc` writes x1, which is not its argument, and holds its block in x4
   until after the commit, and `c_calloc` moves its block to x2 after the commit for the tail, which
   does all the zeroing: no store follows a commit. Neither a restart nor a miss reads a register the
   sequence writes.
3. *The linker.* A sequence holds no `adrp` or other relocation, and no `adrp` before one is directly
   followed by a load or store, so the pattern of Cortex-A53 erratum 843419 occurs nowhere in a
   routine and no linker replaces a load in a sequence with a branch out of its range.
4. *The mirror.* A stale region shift, base or byte gives a position whose mask is zero, an index
   out of range or a class that the slow path corrects, as on x86-64: the routine's contract holds
   for every content of those words. So their order on a weak machine is no matter of safety.

## PROVISIONAL-03: the area on musl (`aarch64/area.rs`)

**Blocks.** The `global_asm!` that defines the area, and the `asm!` of `offset`.

**Proof.** The first is data: 32 bytes of thread-local data, as `generated/x86_64/area.rs` defines
them, read through nothing but the symbol. The second moves into one register the 32-bit value the
linker computes for the symbol, the area's distance from the thread pointer under the local-exec
model, which is valid in an executable, the only kind of musl program the crate supports; it reads
no memory and touches neither flags nor stack. What rests on the offset is in `front/area-safety.md`.
