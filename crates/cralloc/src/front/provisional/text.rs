//! The text of the routines, as macros, since `naked_asm!` takes a literal and one text serves a
//! routine and its exported copy. Some are exported only for what `export_c_abi!` expands to in
//! another crate.
//!
//! Labels: `5` the arming sequence, `6` the sequence's first instruction, `7` the end of the
//! sequence, `3` its descriptor, `4` its abort handler, `8` the word holding the address of
//! `__rseq_offset`, `20` the miss. Registers: x9 the rseq area, then x10 to x15 and for the larger
//! routines x2 as scratch, which no routine that reads them after a restart writes before its
//! commit.

/// Finds the rseq area through a word of the routine's own, arms the sequence, reads the CPU
/// number into x10 and compares it with the slab count, leaving the flags for `b.hs`. The word is
/// 16-aligned, as x86-64's is, off the page offsets of the hot words.
#[cfg(not(target_env = "musl"))]
#[doc(hidden)]
#[macro_export]
macro_rules! cralloc_a64_arm {
    () => {
        concat!(
            "5:\n",
            ".pushsection .data.rel.ro.cralloc_got,\"aw\",%progbits\n",
            ".balign 16\n",
            "8:\n",
            ".quad __rseq_offset\n",
            ".popsection\n",
            "adrp x9, 8b\n",
            "ldr x9, [x9, :lo12:8b]\n",
            "ldr x9, [x9]\n",
            "mrs x10, tpidr_el0\n",
            "add x9, x9, x10\n",
            $crate::cralloc_a64_arm_area!(),
        )
    };
}

/// [`cralloc_a64_arm`] on musl, where the area is the crate's own in the executable's thread-local
/// data: its distance from the thread pointer is fixed at link time, and no assembler for this
/// architecture can write that distance as data for a word to hold, so it is the instructions'.
#[cfg(target_env = "musl")]
#[doc(hidden)]
#[macro_export]
macro_rules! cralloc_a64_arm {
    () => {
        concat!(
            "5:\n",
            "mrs x10, tpidr_el0\n",
            "add x9, x10, :tprel_hi12:cralloc_rseq_area\n",
            "add x9, x9, :tprel_lo12_nc:cralloc_rseq_area\n",
            $crate::cralloc_a64_arm_area!(),
        )
    };
}

/// [`cralloc_a64_arm`] from x9, the rseq area.
#[doc(hidden)]
#[macro_export]
macro_rules! cralloc_a64_arm_area {
    () => {
        concat!(
            "adrp x10, 3f\n",
            "add x10, x10, :lo12:3f\n",
            "str x10, [x9, #8]\n",
            "6:\n",
            "ldr w10, [x9, #4]\n",
            "cmp w10, #2, lsl #12\n",
        )
    };
}

/// [`cralloc_a64_arm`] for the routines that read `__rseq_offset` directly where they can: an
/// executable, which has its own copy of glibc's.
#[cfg(all(feature = "executable", not(target_env = "musl")))]
macro_rules! a64_arm_direct {
    () => {
        concat!(
            "5:\n",
            "adrp x9, __rseq_offset\n",
            "ldr x9, [x9, :lo12:__rseq_offset]\n",
            "mrs x10, tpidr_el0\n",
            "add x9, x9, x10\n",
            $crate::cralloc_a64_arm_area!(),
        )
    };
}

#[cfg(any(not(feature = "executable"), target_env = "musl"))]
macro_rules! a64_arm_direct {
    () => {
        $crate::cralloc_a64_arm!()
    };
}

/// The sequence's descriptor and, after the function, its abort handler, which starts the sequence
/// again.
#[doc(hidden)]
#[macro_export]
macro_rules! cralloc_a64_end {
    () => {
        concat!(
            "7:\n",
            ".pushsection __rseq_cs, \"aw\"\n",
            ".balign 32\n",
            "3:\n",
            ".long 0, 0\n",
            ".quad 6b, 7b - 6b, 4f\n",
            ".popsection\n",
            ".subsection 1\n",
            ".inst 0xd428bc00\n",
            "4:\n",
            "b 5b\n",
            ".subsection 0\n",
        )
    };
}

/// From the word position in x11, with the CPU number in x10: the mask into x12, and the address
/// of the word in this CPU's slab into x11, its value into x13.
#[doc(hidden)]
#[macro_export]
macro_rules! cralloc_a64_c_word {
    () => {
        concat!(
            "adrp x12, {SLABS}+33556480\n",
            "add x12, x12, :lo12:{SLABS}+33556480\n",
            "ldr x12, [x12, x11, lsl #3]\n",
            "adrp x13, {SLABS}\n",
            "add x13, x13, :lo12:{SLABS}\n",
            "add x11, x13, x11, lsl #3\n",
            "add x11, x11, x10, lsl #12\n",
            "ldr x13, [x11]\n",
        )
    };
}

/// The position of the hot word of the class of `ptr` in x0's region into x11, missing if the
/// region is out of range. Reads the mirror of the heap words.
#[doc(hidden)]
#[macro_export]
macro_rules! cralloc_a64_c_region {
    () => {
        concat!(
            "adrp x11, {SLABS}+33555456\n",
            "add x11, x11, :lo12:{SLABS}+33555456\n",
            "ldp x12, x13, [x11]\n",
            "lsr x11, x0, x12\n",
            "sub x11, x11, x13\n",
            "cmp x11, #1, lsl #12\n",
            "b.hs 20f\n",
            "adrp x12, {SLABS}+33560576\n",
            "add x12, x12, :lo12:{SLABS}+33560576\n",
            "ldrb w11, [x12, x11]\n",
        )
    };
}

/// A push from the word and mask of [`cralloc_a64_c_word`], of the pointer in x0.
#[doc(hidden)]
#[macro_export]
macro_rules! cralloc_a64_c_push {
    () => {
        concat!(
            $crate::cralloc_a64_c_word!(),
            "add x14, x13, #8\n",
            "tst w14, w12\n",
            "b.eq 20f\n",
            "str x0, [x13]\n",
            "str x14, [x11]\n",
        )
    };
}

/// The miss: to the slow path, with the arguments as they came.
#[doc(hidden)]
#[macro_export]
macro_rules! cralloc_a64_c_miss {
    () => {
        concat!(
            ".subsection 1\n",
            "20:\n",
            "b {slow}\n",
            ".subsection 0\n",
            ".cfi_endproc\n",
        )
    };
}

/// `c_malloc`.
#[doc(hidden)]
#[macro_export]
macro_rules! cralloc_a64_c_malloc {
    () => {
        concat!(
            ".cfi_startproc\n",
            $crate::cralloc_a64_arm!(),
            "b.hs 20f\n",
            "cmp x0, #8, lsl #12\n",
            "b.hi 20f\n",
            "add x11, x0, #7\n",
            "lsr x11, x11, #3\n",
            "adrp x12, {C_SIZES}+2048\n",
            "add x12, x12, :lo12:{C_SIZES}+2048\n",
            "ldrb w11, [x12, x11]\n",
            $crate::cralloc_a64_c_word!(),
            "tst w13, w12\n",
            "b.eq 20f\n",
            "sub x13, x13, #8\n",
            "ldr x10, [x13]\n",
            "str x13, [x11]\n",
            $crate::cralloc_a64_end!(),
            "mov x0, x10\n",
            "ret\n",
            $crate::cralloc_a64_c_miss!(),
        )
    };
}

/// `c_calloc`.
#[doc(hidden)]
#[macro_export]
macro_rules! cralloc_a64_c_calloc {
    () => {
        concat!(
            ".cfi_startproc\n",
            $crate::cralloc_a64_arm!(),
            "b.hs 20f\n",
            "mul x11, x0, x1\n",
            "umulh x12, x0, x1\n",
            "cbnz x12, 20f\n",
            "cmp x11, #1024\n",
            "b.hi 20f\n",
            "adrp x12, {C_CALLOC_SIZES}+2048\n",
            "add x12, x12, :lo12:{C_CALLOC_SIZES}+2048\n",
            "ldrb w11, [x12, x11]\n",
            $crate::cralloc_a64_c_word!(),
            "tst w13, w12\n",
            "b.eq 20f\n",
            "sub x13, x13, #8\n",
            "ldr x2, [x13]\n",
            "str x13, [x11]\n",
            $crate::cralloc_a64_end!(),
            "b {zero}\n",
            $crate::cralloc_a64_c_miss!(),
        )
    };
}

/// `c_free`.
#[doc(hidden)]
#[macro_export]
macro_rules! cralloc_a64_c_free {
    () => {
        concat!(
            ".cfi_startproc\n",
            $crate::cralloc_a64_arm!(),
            "b.hs 20f\n",
            "cbz x0, 7f\n",
            $crate::cralloc_a64_c_region!(),
            $crate::cralloc_a64_c_push!(),
            $crate::cralloc_a64_end!(),
            "ret\n",
            $crate::cralloc_a64_c_miss!(),
        )
    };
}

/// `c_free_sized`.
#[doc(hidden)]
#[macro_export]
macro_rules! cralloc_a64_c_free_sized {
    () => {
        concat!(
            ".cfi_startproc\n",
            $crate::cralloc_a64_arm!(),
            "b.hs 20f\n",
            $crate::cralloc_a64_c_region!(),
            "adrp x12, {C_SIZED_SIZES}+2048\n",
            "add x12, x12, :lo12:{C_SIZED_SIZES}+2048\n",
            "and x13, x1, #0xffff\n",
            "ldrb w12, [x12, x13]\n",
            "cmp w11, w12\n",
            "b.ne 20f\n",
            $crate::cralloc_a64_c_push!(),
            $crate::cralloc_a64_end!(),
            "ret\n",
            $crate::cralloc_a64_c_miss!(),
        )
    };
}

/// The field of this CPU's slab at the offset in x0: its address into x12 and its value into x13,
/// after the compare and its branch to 20.
macro_rules! a64_slab_field {
    () => {
        concat!(
            "b.hs 20f\n",
            "and x11, x0, #0xff8\n",
            "add x11, x11, x10, lsl #12\n",
            "adrp x12, {SLABS}\n",
            "add x12, x12, :lo12:{SLABS}\n",
            "add x12, x12, x11\n",
            "ldr x13, [x12]\n",
        )
    };
}

/// After a slab routine's commit: returns what it read, with `NO_CPU` (2) for the miss.
macro_rules! a64_slab_return {
    () => {
        concat!(
            "mov x0, x13\n",
            "ret\n",
            ".subsection 1\n",
            "20:\n",
            "mov x13, #2\n",
            "b 7b\n",
            ".subsection 0\n",
            ".cfi_endproc\n",
        )
    };
}
