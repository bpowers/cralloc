//! The C entry points on aarch64. Their contracts are those of `generated/x86_64/centry.rs`.
#![allow(dead_code)]

/// C's `calloc`, as the x86-64 routine of this name.
///
/// # Safety
///
/// As that routine, whose contract is `VG.Spec.CEntry.callocContract`.
// SAFETY: PROVISIONAL-02.
#[unsafe(naked)]
pub(crate) unsafe extern "C" fn c_calloc(n: usize, size: usize) -> usize {
    core::arch::naked_asm!(
        crate::cralloc_a64_c_calloc!(),
        C_CALLOC_SIZES = sym crate::c::table::C_CALLOC_SIZES,
        SLABS = sym crate::front::SLABS,
        zero = sym crate::c::slow::calloc_zero_tail,
        slow = sym crate::c::slow::calloc_slow_args,
    )
}

/// C's `malloc`, as the x86-64 routine of this name.
///
/// # Safety
///
/// As that routine, whose contract is `VG.Spec.CEntry.mallocContract`.
// SAFETY: PROVISIONAL-02.
#[unsafe(naked)]
pub(crate) unsafe extern "C" fn c_malloc(size: usize) -> usize {
    core::arch::naked_asm!(
        crate::cralloc_a64_c_malloc!(),
        C_SIZES = sym crate::c::table::C_SIZES,
        SLABS = sym crate::front::SLABS,
        slow = sym crate::c::slow::malloc_slow,
    )
}

/// For C's `free`, as the x86-64 routine of this name.
///
/// # Safety
///
/// As that routine, whose contract is `VG.Spec.CEntry.freeContract`.
// SAFETY: PROVISIONAL-02.
#[unsafe(naked)]
pub(crate) unsafe extern "C" fn c_free(ptr: usize) {
    core::arch::naked_asm!(
        crate::cralloc_a64_c_free!(),
        SLABS = sym crate::front::SLABS,
        slow = sym crate::c::slow::free_slow,
    )
}

/// For C's `free_sized`, as the x86-64 routine of this name.
///
/// # Safety
///
/// As that routine, whose contract is `VG.Spec.CEntry.freeSizedContract`.
// SAFETY: PROVISIONAL-02.
#[unsafe(naked)]
pub(crate) unsafe extern "C" fn c_free_sized(ptr: usize, size: usize) {
    core::arch::naked_asm!(
        crate::cralloc_a64_c_free_sized!(),
        C_SIZED_SIZES = sym crate::c::table::C_SIZED_SIZES,
        SLABS = sym crate::front::SLABS,
        slow = sym crate::c::slow::free_sized_slow,
    )
}
