//! The slab routines that are not inlined, on aarch64: `slab_cas` and the stack routines. Their
//! contracts are those of `generated/x86_64/slab.rs`.
#![allow(dead_code)]

/// Stores `new` in the calling thread's CPU's slab word at `offset` if it holds `expected`, and
/// returns the word it read there; returns `NO_CPU` (2) if the CPU number has no slab.
///
/// # Safety
///
/// As the x86-64 routine of this name, whose contract is `VG.Spec.Slab.casContract`.
// SAFETY: PROVISIONAL-01.
#[unsafe(naked)]
pub(crate) unsafe extern "C" fn slab_cas(offset: usize, expected: usize, new: usize) -> usize {
    core::arch::naked_asm!(
        concat!(
            ".cfi_startproc\n",
            crate::cralloc_a64_arm!(),
            a64_slab_field!(),
            "cmp x13, x1\n",
            "b.ne 7f\n",
            "str x2, [x12]\n",
            crate::cralloc_a64_end!(),
            a64_slab_return!(),
        ),
        SLABS = sym crate::front::SLABS,
    )
}

/// Pushes `magazine` on the calling thread's CPU's stack whose head is at `offset`, unless the
/// head is a marker, and returns the head it read; returns `NO_CPU` (2) if the CPU number has no
/// slab.
///
/// # Safety
///
/// As the x86-64 routine of this name, whose contract is `VG.Spec.Slab.stackPushContract`.
// SAFETY: PROVISIONAL-01.
#[unsafe(naked)]
pub(crate) unsafe extern "C" fn slab_stack_push(offset: usize, magazine: usize) -> usize {
    core::arch::naked_asm!(
        concat!(
            ".cfi_startproc\n",
            a64_arm_direct!(),
            a64_slab_field!(),
            "tbnz x13, #0, 7f\n",
            "str x13, [x1]\n",
            "str x1, [x12]\n",
            crate::cralloc_a64_end!(),
            a64_slab_return!(),
        ),
        SLABS = sym crate::front::SLABS,
    )
}

/// Pops the top magazine off the calling thread's CPU's stack whose head is at `offset`, unless
/// the stack is empty (0) or the head a marker, and returns the head it read; returns `NO_CPU` (2)
/// if the CPU number has no slab.
///
/// # Safety
///
/// As the x86-64 routine of this name, whose contract is `VG.Spec.Slab.stackPopContract`.
// SAFETY: PROVISIONAL-01.
#[unsafe(naked)]
pub(crate) unsafe extern "C" fn slab_stack_pop(offset: usize) -> usize {
    core::arch::naked_asm!(
        concat!(
            ".cfi_startproc\n",
            a64_arm_direct!(),
            a64_slab_field!(),
            "tbnz x13, #0, 7f\n",
            "cbz x13, 7f\n",
            "ldr x14, [x13]\n",
            "str x14, [x12]\n",
            crate::cralloc_a64_end!(),
            a64_slab_return!(),
        ),
        SLABS = sym crate::front::SLABS,
    )
}
