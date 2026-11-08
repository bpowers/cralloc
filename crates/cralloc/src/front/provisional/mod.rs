//! Hand-written aarch64 routines, until Lean emits them.
//!
//! Each does what its x86-64 routine in `generated/` does, by the instructions measured as best in
//! `docs/research/firestorm.md`, with the same module and function names that `generated/aarch64/`
//! and `generated/aarch64_inline/` will have, so that the swap to Lean's output is the one `use` in
//! `front`. They keep the design's rules (docs/design.md, "Front end: magazines under rseq"), which
//! their proofs will have to find:
//!
//! - a routine changes nothing another thread could see before its one commit store, its last
//!   instruction in the sequence, and a read of the CPU number comes first where it decides anything;
//! - an abort returns to the arming store with the registers as they were, so nothing after it
//!   writes the rseq area's register, the descriptor's, or an argument, and a register that a
//!   routine writes and returns is set again after the arming store;
//! - a miss leaves the arguments intact, so that a C routine reaches its slow path with the C
//!   function's own, by one branch.
//!
//! What this architecture changes:
//!
//! - The abort handler is preceded by the signature glibc registered, `brk #0x45e0`, and the thread
//!   pointer is `tpidr_el0`.
//! - A conditional branch reaches 1 MiB, and a linker cannot put a veneer on one. So a condition only
//!   targets its own routine: an inlined routine's end, with the result already set, or a stub after
//!   a function's `ret`. The abort handler holds an unconditional branch, in `.subsection 1`, past
//!   the end of its function, with no veneer: `b` reaches 128 MiB, and only a C routine's tail call to
//!   its slow path crosses sections, where a veneer's x16 and x17 hold nothing.
//! - Displacements are small and `adr` reaches 1 MiB, so what x86-64 reads at a displacement from
//!   `SLABS` it reaches here by `adrp`, which reaches 4 GiB. The displacements are the specification's,
//!   the same on every target.
//! - A sequence holds no `adrp` or other relocation: every address is formed before the arming store,
//!   which is the only thing a retry repeats outside the sequence. rustc links with
//!   `--fix-cortex-a53-843419`, which can replace a load after an `adrp` near a page's end with a
//!   branch to a stub, and a preemption in a stub out of the sequence's range does not abort it.
//! - The `executable` feature changes nothing: GNU ld will not copy a C library's variable into a
//!   position-independent executable for `adrp` to reach, so every routine reads through a word.
//! - The return register is the first argument's, so a routine that returns a value and restarts
//!   with its arguments moves the value there after the sequence.
//!
//! The ledger for all of this is `docs/safety/cralloc/front/provisional/mod-safety.md`.

#[macro_use]
mod text;

pub(crate) mod aarch64;
pub(crate) mod aarch64_inline;
