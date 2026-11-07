# Safety ledger: `crates/cralloc/src/front/provisional/`

The aarch64 routines are hand-written until Lean emits them. Each is the translation of the x86-64
routine of its name and has that routine's contract, so what `Real` and the C layer owe them is
already discharged (`real-safety.md`, `c/routines-safety.md`), but for the red zone, which this
architecture does not have. What x86-64's proofs give the generated routines, these have only by the
argument below and by test: the twins' differential test and the breakpoint tests run them. Nobody
has reviewed these entries, and the Lean proofs replace them.

## PROVISIONAL-01: the slab routines (`aarch64/slab.rs`, `aarch64_inline/slab.rs`, the self-test)

**Blocks.** The `naked_asm!` of `slab_cas`, `slab_stack_push`, `slab_stack_pop` and
`vg_selftest_add`, and the `asm!` of `slab_pop`, `slab_push` and `current_cpu`.

**Preconditions.** Those of the routine's x86-64 contract, `VG.Spec.Slab.*Contract`.

**Proof.**

1. *The area.* `mrs tpidr_el0` plus `__rseq_offset` is the calling thread's rseq area, by glibc's
   definition of the variable (signed; the routine adds it). On musl the area is the crate's own
   (PROVISIONAL-03), and the offset the linker's: `:tprel_hi12:` and `:tprel_lo12_nc:` of it.
2. *The sequence.* The descriptor names `6..7` and the abort handler `4`. The handler is preceded by
   the signature glibc registers on aarch64, `0xd428bc00` (`sysdeps/unix/sysv/linux/aarch64/bits/rseq.h`):
   the kernel reads the four bytes before the abort address and kills a thread on a mismatch. The
   handler jumps to `5`, which reloads the area and arms again, since the kernel clears `rseq_cs`
   when it aborts.
3. *One commit.* Between `6` and `7` the only stores are to a slot of the magazine the word names,
   which nothing can observe until the commit, and the commit, the last instruction before `7`.
   So an abort at any instruction leaves no change another thread can see. A store is a single
   64-bit `str` to an 8-aligned address, which is single-copy atomic, so the drain's exchange on
   another CPU sees the word before or after it.
4. *The restart.* Everything `5` or later reads is an input: an operand no instruction writes. An
   inlined routine's outputs and scratch operands are `out`, which the compiler keeps apart from
   its inputs, and a naked routine writes only x9 to x14, and x0 only after `7`.
5. *The branch.* Each conditional branch's target is in the same section, in the same function or
   after it (`.subsection 1`), so that it is resolved by the assembler and needs no veneer, which
   could overwrite x16 or x17, where an inlined routine's input may live. The abort handler's `b`
   is the same.
6. *The miss.* A CPU number of 8192 or more, as an unregistered thread's -1 or -2 reads unsigned,
   branches before any address is formed from it. Nothing has been stored.
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

1. *The tables.* A table's address comes by `adrp` and `:lo12:`, with the displacements of the
   specification, which `aarch64/mod.rs` asserts. Each index is in range as on x86-64: the size
   compared first for `malloc` and `calloc`, the low 16 bits for the sized free's table, and the
   compare with 4096 for the region table. A position read from a table is a byte, below the 256
   entries of the masks' table, and a zero mask misses.
2. *The arguments.* x0 and x1 are not written before a commit, so a restart finds them, and a miss
   reaches its slow path with them by one `b`, whose veneer, if the linker adds one, may overwrite
   only x16 and x17, which hold nothing the slow path takes. `c_calloc`'s block goes in x2, which
   neither restart nor miss reads, and `c_malloc` returns its block through x10.
3. *The mirror.* A stale region shift, base or byte gives a position whose mask is zero, an index
   out of range or a class that the slow path corrects, as on x86-64: the routine's contract holds
   for every content of those words. So their order on a weak machine is no matter of safety.

## PROVISIONAL-03: the area on musl (`aarch64/area.rs`)

**Blocks.** The `global_asm!` that defines the area, and the `asm!` of `offset`.

**Proof.** The first is data: 32 bytes of thread-local data, as `generated/x86_64/area.rs` defines
them, read through nothing but the symbol. The second moves into one register the 32-bit value the
linker computes for the symbol, the area's distance from the thread pointer under the local-exec
model, which is valid in an executable, the only kind of musl program the crate supports; it reads
no memory and touches neither flags nor stack. What rests on the offset is in `front/area-safety.md`.
