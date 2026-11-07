//! The pop, the push and the read of the CPU number on aarch64, inlined into their callers. Their
//! contracts are those of `generated/x86_64_inline/slab.rs`, but for the red zone, which this
//! architecture does not have.
//!
//! Each works in fixed registers, where its contract will read them. A miss leaves the sequence
//! with the routine's result as it set it first, so no conditional branch leaves the routine: a
//! stub would be past the end of the function it is inlined into, out of a conditional branch's
//! reach in a function over 1 MiB.
#![allow(dead_code)]

/// Pops a pointer from the magazine of the calling thread's CPU of the class whose word in slab 0
/// is at `word`, or returns 0 if the CPU number has no slab or the magazine none to give.
///
/// # Safety
///
/// As the x86-64 routine of this name, whose contract is `VG.Spec.Slab.popInlineContract`.
#[inline(always)]
pub(crate) unsafe fn slab_pop(word: usize, mask: usize) -> usize {
    let ret: usize;
    // SAFETY: PROVISIONAL-01; the caller's promise.
    unsafe {
        core::arch::asm!(
            a64_arm_direct!(),
            "mov x0, #0",
            "ldr w8, [x10, #4]",
            "cmp w8, #2, lsl #12",
            "b.hs 7f",
            "lsl x8, x8, #12",
            "ldr x1, [x3, x8]",
            "tst w1, w2",
            "b.eq 7f",
            "ldr x0, [x1, #-8]",
            "sub x1, x1, #8",
            "str x1, [x3, x8]",
            a64_end_direct!(),
            in("x3") word,
            in("x2") mask,
            out("x0") ret,
            out("x1") _,
            out("x8") _,
            out("x9") _,
            out("x10") _,
            out("x11") _,
            options(nostack),
        );
    }
    ret
}

/// Pushes `p` on the magazine of the calling thread's CPU of the class whose word in slab 0 is at
/// `word` and returns `true`, or returns `false` if the CPU number has no slab or the magazine no
/// free slot to spare.
///
/// # Safety
///
/// As the x86-64 routine of this name, whose contract is `VG.Spec.Slab.pushInlineContract`.
#[inline(always)]
pub(crate) unsafe fn slab_push(word: usize, mask: usize, p: usize) -> bool {
    let pushed: usize;
    // SAFETY: PROVISIONAL-01; the caller's promise.
    unsafe {
        core::arch::asm!(
            a64_arm_direct!(),
            "mov x4, #0",
            "ldr w8, [x10, #4]",
            "cmp w8, #2, lsl #12",
            "b.hs 7f",
            "lsl x8, x8, #12",
            "ldr x1, [x3, x8]",
            "add x9, x1, #8",
            "tst w9, w2",
            "b.eq 7f",
            "str x0, [x1]",
            "mov x4, x9",
            "str x9, [x3, x8]",
            a64_end_direct!(),
            in("x0") p,
            in("x3") word,
            in("x2") mask,
            out("x4") pushed,
            out("x1") _,
            out("x8") _,
            out("x9") _,
            out("x10") _,
            out("x11") _,
            options(nostack),
        );
    }
    pushed != 0
}

/// Returns the calling thread's CPU number as the slab routines read it: its rseq area's
/// `cpu_id`, zero-extended. Not a restartable sequence.
///
/// # Safety
///
/// As the x86-64 routine of this name, whose contract is `VG.Spec.Slab.cpuContract`.
#[cfg(not(any(feature = "executable", target_env = "musl")))]
#[inline(always)]
pub(crate) unsafe fn current_cpu() -> u32 {
    let ret: u32;
    // SAFETY: PROVISIONAL-01; the caller's promise.
    unsafe {
        core::arch::asm!(
            ".pushsection .data.rel.ro.cralloc_got,\"aw\",%progbits",
            ".balign 16",
            "8:",
            ".quad __rseq_offset",
            ".popsection",
            "adrp x9, 8b",
            "ldr x9, [x9, :lo12:8b]",
            "ldr x9, [x9]",
            "mrs x10, tpidr_el0",
            "add x10, x10, x9",
            "ldr w0, [x10, #4]",
            lateout("x0") ret,
            out("x9") _,
            out("x10") _,
            options(nostack),
        );
    }
    ret
}

/// [`current_cpu`] in an executable, which has its own copy of glibc's `__rseq_offset`.
///
/// # Safety
///
/// As above.
#[cfg(all(feature = "executable", not(target_env = "musl")))]
#[inline(always)]
pub(crate) unsafe fn current_cpu() -> u32 {
    let ret: u32;
    // SAFETY: PROVISIONAL-01; the caller's promise.
    unsafe {
        core::arch::asm!(
            "adrp x9, __rseq_offset",
            "ldr x9, [x9, :lo12:__rseq_offset]",
            "mrs x10, tpidr_el0",
            "add x10, x10, x9",
            "ldr w0, [x10, #4]",
            lateout("x0") ret,
            out("x9") _,
            out("x10") _,
            options(nostack),
        );
    }
    ret
}

/// [`current_cpu`] on musl, where the area is the crate's own.
///
/// # Safety
///
/// As above.
#[cfg(target_env = "musl")]
#[inline(always)]
pub(crate) unsafe fn current_cpu() -> u32 {
    let ret: u32;
    // SAFETY: PROVISIONAL-01; the caller's promise.
    unsafe {
        core::arch::asm!(
            "mrs x10, tpidr_el0",
            "add x10, x10, :tprel_hi12:cralloc_rseq_area",
            "add x10, x10, :tprel_lo12_nc:cralloc_rseq_area",
            "ldr w0, [x10, #4]",
            lateout("x0") ret,
            out("x10") _,
            options(nostack),
        );
    }
    ret
}
