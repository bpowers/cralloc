//! The pop, the push and the read of the CPU number on aarch64, inlined into their callers. Their
//! contracts are those of `generated/x86_64_inline/slab.rs`, but for the red zone, which this
//! architecture does not have.
//!
//! Every operand a sequence writes is `out`, not `lateout`: the register cannot be one a restart
//! reads.
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
            crate::cralloc_a64_arm!(),
            "b.hs 20f",
            "add x10, {word}, x10, lsl #12",
            "ldr x11, [x10]",
            "tst w11, {mask:w}",
            "b.eq 20f",
            "ldr {ret}, [x11, #-8]",
            "sub x11, x11, #8",
            "str x11, [x10]",
            crate::cralloc_a64_end!(),
            ".subsection 1",
            "20:",
            "mov {ret}, xzr",
            "b 7b",
            ".subsection 0",
            word = in(reg) word,
            mask = in(reg) mask,
            ret = out(reg) ret,
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
    // SAFETY: PROVISIONAL-01; the caller's promise.
    unsafe {
        core::arch::asm!(
            crate::cralloc_a64_arm!(),
            "b.hs {miss}",
            "add x10, {word}, x10, lsl #12",
            "ldr x11, [x10]",
            "add {next}, x11, #8",
            "tst {next:w}, {mask:w}",
            "b.eq {miss}",
            "str {p}, [x11]",
            "str {next}, [x10]",
            crate::cralloc_a64_end!(),
            word = in(reg) word,
            mask = in(reg) mask,
            p = in(reg) p,
            next = out(reg) _,
            out("x9") _,
            out("x10") _,
            out("x11") _,
            miss = label { return false },
            options(nostack),
        );
    }
    true
}

/// Returns the calling thread's CPU number as the slab routines read it: its rseq area's
/// `cpu_id`, zero-extended. Not a restartable sequence.
///
/// # Safety
///
/// As the x86-64 routine of this name, whose contract is `VG.Spec.Slab.cpuContract`.
#[cfg(not(target_env = "musl"))]
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
            "adrp {area}, 8b",
            "ldr {area}, [{area}, :lo12:8b]",
            "ldr {area}, [{area}]",
            "mrs {tp}, tpidr_el0",
            "add {area}, {area}, {tp}",
            "ldr {ret:w}, [{area}, #4]",
            area = out(reg) _,
            tp = out(reg) _,
            ret = out(reg) ret,
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
            "mrs {area}, tpidr_el0",
            "add {area}, {area}, :tprel_hi12:cralloc_rseq_area",
            "add {area}, {area}, :tprel_lo12_nc:cralloc_rseq_area",
            "ldr {ret:w}, [{area}, #4]",
            area = out(reg) _,
            ret = out(reg) ret,
            options(nostack),
        );
    }
    ret
}
