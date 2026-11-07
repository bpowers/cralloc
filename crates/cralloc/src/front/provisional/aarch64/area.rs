//! The rseq area the crate defines and registers itself on musl, which registers none: the data
//! of `generated/x86_64/area.rs`, for aarch64, without its word holding the area's offset from the
//! thread pointer, which no assembler for this architecture can write as data. The routines have
//! the offset in their instructions, and [`offset`] gives it to Rust.
//!
//! On this architecture the static thread-local data starts above the thread pointer, at the 16
//! bytes the ABI keeps there rounded up to the data's alignment, and the C library aligns the
//! thread pointer to it too: a page here, so the area keeps its page offset in every thread.

// SAFETY: PROVISIONAL-03.
core::arch::global_asm!(
    ".pushsection .tdata.cralloc_rseq_area,\"awT\",%progbits",
    ".balign 4096",
    ".skip 768",
    ".globl cralloc_rseq_area",
    ".hidden cralloc_rseq_area",
    ".type cralloc_rseq_area,%object",
    ".size cralloc_rseq_area, 32",
    "cralloc_rseq_area:",
    ".long 0, 0xffffffff",
    ".quad 0",
    ".long 0, 0, 0, 0",
    ".popsection",
);

/// The area's offset from the thread pointer, as glibc's `__rseq_offset` holds its own.
#[inline]
pub(crate) fn offset() -> isize {
    let offset: isize;
    // SAFETY: PROVISIONAL-03. Two moves of constants into a register.
    unsafe {
        core::arch::asm!(
            "movz {offset}, #:tprel_g1:cralloc_rseq_area",
            "movk {offset}, #:tprel_g0_nc:cralloc_rseq_area",
            offset = out(reg) offset,
            options(nomem, nostack, preserves_flags),
        );
    }
    offset
}
