//! Hand-written aarch64 routines, until Lean emits them.
//!
//! Each is the direct translation of its x86-64 routine in `generated/`, with the same module and
//! function names that `generated/aarch64/` and `generated/aarch64_inline/` will have, so that the
//! swap to Lean's output is the one `use` in `front`. They keep the design's rules (docs/design.md,
//! "Front end: magazines under rseq"), which their proofs will have to find:
//!
//! - a routine changes nothing another thread could see before its one commit store, its last
//!   instruction in the sequence, and a read of the CPU number is the sequence's first;
//! - an abort restarts it from the arming store, with its arguments as they were, so nothing
//!   before the commit writes a register the restart reads: an output lives in a scratch register
//!   until the commit is done, and an inline routine's operands are early-clobber;
//! - a miss leaves the arguments intact, so that a C routine reaches its slow path with the C
//!   function's own, by one branch.
//!
//! What this architecture changes:
//!
//! - The abort handler is preceded by the signature glibc registered, `brk #0x45e0`, and the thread
//!   pointer is `tpidr_el0`.
//! - A conditional branch reaches 1 MiB, and a linker cannot put a veneer on one. So every miss stub
//!   and abort handler stays in its function's own section, after the function (`.subsection 1`), and
//!   nothing a condition targets is in another section. A function larger than 1 MiB is out of the
//!   reach of its own stubs, and fails to assemble rather than to run. `b` reaches 128 MiB and a
//!   linker may add a veneer, which may overwrite x16 and x17: only a C routine's tail call to its
//!   slow path crosses sections, and those registers then hold nothing.
//! - Displacements are small and `adr` reaches 1 MiB, so what x86-64 reads at a displacement from
//!   `SLABS` it reaches here by `adrp`, which reaches 4 GiB. The displacements are the specification's,
//!   the same on every target.
//! - The `executable` feature changes nothing: a linker will not copy a C library's variable into
//!   a position-independent executable for `adrp` to reach, so every routine reads through a word.
//! - The return register is the first argument's, so a routine that returns a value and restarts
//!   with its arguments moves the value there after the sequence.
//!
//! The ledger for all of this is `docs/safety/cralloc/front/provisional/mod-safety.md`.

#[macro_use]
mod text;

pub(crate) mod aarch64;
pub(crate) mod aarch64_inline;
