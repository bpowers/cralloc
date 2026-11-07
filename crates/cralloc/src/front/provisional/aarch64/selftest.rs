//! The pipeline self-test for aarch64.
#![allow(dead_code)]

/// Returns `a.wrapping_add(b)`.
///
/// # Safety
///
/// None: it reads two registers and writes one.
// SAFETY: PROVISIONAL-01.
#[unsafe(naked)]
pub(crate) unsafe extern "C" fn vg_selftest_add(a: u64, b: u64) -> u64 {
    core::arch::naked_asm!(
        ".cfi_startproc\n",
        "add x0, x0, x1\n",
        "ret\n",
        ".cfi_endproc\n",
    )
}
