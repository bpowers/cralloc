//! The only things the allocator takes from the C library: where glibc put the thread's rseq
//! area, and how much of it the kernel maintains.
//!
//! glibc 2.35 and later export both from the dynamic loader. They are set before any user code
//! runs and never change afterwards. The ledger entry for the declaration is
//! `docs/safety/asmalloc/os/linux/glibc-safety.md`.

use crate::os::RseqArea;

// SAFETY: glibc declares these as `extern const ptrdiff_t __rseq_offset;` and
// `extern const unsigned int __rseq_size;` (`sys/rseq.h`). `isize` and `u32` have the same
// size, alignment and representation on x86-64 Linux, every bit pattern is valid for both, and
// glibc writes them only during start-up, before any Rust code can read them, so reading them as
// immutable statics is sound.
unsafe extern "C" {
    safe static __rseq_offset: isize;
    safe static __rseq_size: u32;
}

/// glibc's rseq registration facts, or `None` if the process has none (valgrind,
/// `glibc.pthread.rseq=0`, or a kernel without rseq). They describe the process: a thread whose
/// own registration failed after start-up still sees them, with its `cpu_id` field at -2.
pub(super) fn registration() -> Option<RseqArea> {
    RseqArea::new(__rseq_offset, __rseq_size)
}
