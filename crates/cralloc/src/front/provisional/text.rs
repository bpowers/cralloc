//! The text of the routines, as macros, since `naked_asm!` takes a literal and one text serves a
//! routine and its exported copy. Some are exported only for what `export_c_abi!` expands to in
//! another crate.
//!
//! Labels: `5` the arming store, where the abort handler returns; `6` the sequence's first
//! instruction; `7` the end of the sequence; `3` its descriptor; `4` its abort handler; `8` a word
//! holding the address of `__rseq_offset`, for a routine with no descriptor; `20` a C routine's
//! miss. Registers: x10 the rseq area and x11 the descriptor, which nothing writes after `5`, x8 the
//! CPU number, and the rest as each routine says.

/// Finds the rseq area through the word after the routine's descriptor, which saves an `adrp` on
/// a core where they compete with branches, and arms the sequence. The word is 32-aligned, off the
/// page offsets of the hot words.
#[cfg(not(target_env = "musl"))]
#[doc(hidden)]
#[macro_export]
macro_rules! cralloc_a64_arm {
    () => {
        concat!(
            "adrp x11, 3f\n",
            "add x11, x11, :lo12:3f\n",
            "ldr x9, [x11, #32]\n",
            "ldr x9, [x9]\n",
            "mrs x10, tpidr_el0\n",
            "add x10, x10, x9\n",
            "5:\n",
            "str x11, [x10, #8]\n",
            "6:\n",
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
            "adrp x11, 3f\n",
            "add x11, x11, :lo12:3f\n",
            "mrs x10, tpidr_el0\n",
            "add x10, x10, :tprel_hi12:cralloc_rseq_area\n",
            "add x10, x10, :tprel_lo12_nc:cralloc_rseq_area\n",
            "5:\n",
            "str x11, [x10, #8]\n",
            "6:\n",
        )
    };
}

/// The end of a sequence begun by [`cralloc_a64_arm`]: its descriptor, with the word holding the
/// address of `__rseq_offset` after it, and the abort handler, which holds only an unconditional
/// branch and so needs no veneer from `.subsection 1`.
#[cfg(not(target_env = "musl"))]
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
            ".quad __rseq_offset\n",
            ".popsection\n",
            ".subsection 1\n",
            ".inst 0xd428bc00\n",
            "4:\n",
            "b 5b\n",
            ".subsection 0\n",
        )
    };
}

/// [`cralloc_a64_end`] for a sequence with no word to hold.
#[cfg(target_env = "musl")]
#[doc(hidden)]
#[macro_export]
macro_rules! cralloc_a64_end {
    () => {
        $crate::cralloc_a64_end_bare!()
    };
}

/// A sequence's end with no word after the descriptor.
#[doc(hidden)]
#[macro_export]
macro_rules! cralloc_a64_end_bare {
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

/// [`cralloc_a64_arm`] with `__rseq_offset` read directly, under the configuration
/// `cralloc_a64_direct`, for comparing it on real jobs with the form that stays. A direct `adrp` to
/// glibc's variable needs a copy of it in the executable, which GNU ld refuses to make in a PIE and
/// `rust-lld` makes, so the `executable` feature cannot mean this here.
#[cfg(all(cralloc_a64_direct, not(target_env = "musl")))]
macro_rules! a64_arm_direct {
    () => {
        concat!(
            "adrp x11, 3f\n",
            "add x11, x11, :lo12:3f\n",
            "adrp x9, __rseq_offset\n",
            "ldr x9, [x9, :lo12:__rseq_offset]\n",
            "mrs x10, tpidr_el0\n",
            "add x10, x10, x9\n",
            "5:\n",
            "str x11, [x10, #8]\n",
            "6:\n",
        )
    };
}

#[cfg(all(cralloc_a64_direct, not(target_env = "musl")))]
macro_rules! a64_end_direct {
    () => {
        $crate::cralloc_a64_end_bare!()
    };
}

#[cfg(any(not(cralloc_a64_direct), target_env = "musl"))]
macro_rules! a64_arm_direct {
    () => {
        $crate::cralloc_a64_arm!()
    };
}

#[cfg(any(not(cralloc_a64_direct), target_env = "musl"))]
macro_rules! a64_end_direct {
    () => {
        $crate::cralloc_a64_end!()
    };
}

/// The CPU number into x8, and to the miss if it has no slab.
#[doc(hidden)]
#[macro_export]
macro_rules! cralloc_a64_c_cpu {
    () => {
        concat!("ldr w8, [x10, #4]\n", "cmp w8, #2, lsl #12\n", "b.hs 20f\n",)
    };
}

/// This CPU's slab into x3: indexing it by a class's position leaves no ALU instruction between a
/// table's load and the hot word's, which is what a free's commit waits on.
#[doc(hidden)]
#[macro_export]
macro_rules! cralloc_a64_c_slab {
    () => {
        concat!(
            "lsl x8, x8, #12\n",
            "adrp x13, {SLABS}\n",
            "add x3, x13, x8\n",
        )
    };
}

/// The page holding the published words and the masks, into x14, and the masks into x15.
#[doc(hidden)]
#[macro_export]
macro_rules! cralloc_a64_c_masks {
    () => {
        concat!("adrp x14, {SLABS}+33554432\n", "add x15, x14, #2048\n",)
    };
}

/// The region index of `ptr` in x0 into x4, to the miss if it is out of range, with the masks in
/// x15 and the region table in x12, 4096 bytes past them.
#[doc(hidden)]
#[macro_export]
macro_rules! cralloc_a64_c_region {
    () => {
        concat!(
            "adrp x14, {SLABS}+33554432\n",
            "ldr w6, [x14, #1024]\n",
            "ldr x7, [x14, #1032]\n",
            "lsr x4, x0, x6\n",
            "sub x4, x4, x7\n",
            "cmp x4, #1, lsl #12\n",
            "b.hs 20f\n",
            "add x15, x14, #2048\n",
            "add x12, x15, #1, lsl #12\n",
        )
    };
}

/// The miss: to the slow path, with the arguments as they came.
#[doc(hidden)]
#[macro_export]
macro_rules! cralloc_a64_c_miss {
    () => {
        concat!("20:\n", "b {slow}\n", ".cfi_endproc\n", ".p2align 6\n",)
    };
}

/// `c_malloc`. The block stays in x4 until the commit, since x0 is the size a restart reads.
#[doc(hidden)]
#[macro_export]
macro_rules! cralloc_a64_c_malloc {
    () => {
        concat!(
            ".cfi_startproc\n",
            $crate::cralloc_a64_arm!(),
            $crate::cralloc_a64_c_cpu!(),
            "cmp x0, #8, lsl #12\n",
            "b.hi 20f\n",
            "add x2, x0, #7\n",
            "lsr x2, x2, #3\n",
            "adrp x12, {C_SIZES}+2048\n",
            "add x12, x12, :lo12:{C_SIZES}+2048\n",
            "ldrb w5, [x12, x2]\n",
            $crate::cralloc_a64_c_slab!(),
            "ldr x1, [x3, x5, lsl #3]\n",
            $crate::cralloc_a64_c_masks!(),
            "ldr x6, [x15, x5, lsl #3]\n",
            "tst w1, w6\n",
            "b.eq 20f\n",
            "ldr x4, [x1, #-8]\n",
            "sub x1, x1, #8\n",
            "str x1, [x3, x5, lsl #3]\n",
            $crate::cralloc_a64_end!(),
            "mov x0, x4\n",
            "ret\n",
            $crate::cralloc_a64_c_miss!(),
        )
    };
}

/// `c_calloc`. x0 and x1 stay as they came, for a restart, the slow path and the zeroing tail: both
/// below 65536 is a product that cannot overflow, and anything else is the slow path's. The hot
/// word is in x2, and the block goes there for the tail.
#[doc(hidden)]
#[macro_export]
macro_rules! cralloc_a64_c_calloc {
    () => {
        concat!(
            ".cfi_startproc\n",
            $crate::cralloc_a64_arm!(),
            $crate::cralloc_a64_c_cpu!(),
            "orr x2, x0, x1\n",
            "cmp x2, #16, lsl #12\n",
            "b.hs 20f\n",
            "mul x9, x0, x1\n",
            "cmp x9, #1024\n",
            "b.hi 20f\n",
            "adrp x12, {C_CALLOC_SIZES}+2048\n",
            "add x12, x12, :lo12:{C_CALLOC_SIZES}+2048\n",
            "ldrb w5, [x12, x9]\n",
            $crate::cralloc_a64_c_slab!(),
            "ldr x2, [x3, x5, lsl #3]\n",
            $crate::cralloc_a64_c_masks!(),
            "ldr x6, [x15, x5, lsl #3]\n",
            "tst w2, w6\n",
            "b.eq 20f\n",
            "ldr x4, [x2, #-8]\n",
            "sub x2, x2, #8\n",
            "str x2, [x3, x5, lsl #3]\n",
            $crate::cralloc_a64_end!(),
            "mov x2, x4\n",
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
            $crate::cralloc_a64_c_cpu!(),
            "cbz x0, 7f\n",
            $crate::cralloc_a64_c_slab!(),
            $crate::cralloc_a64_c_region!(),
            "ldrb w5, [x12, x4]\n",
            "ldr x1, [x3, x5, lsl #3]\n",
            "ldr x6, [x15, x5, lsl #3]\n",
            "add x2, x1, #8\n",
            "tst w2, w6\n",
            "b.eq 20f\n",
            "str x0, [x1]\n",
            "str x2, [x3, x5, lsl #3]\n",
            $crate::cralloc_a64_end!(),
            "ret\n",
            $crate::cralloc_a64_c_miss!(),
        )
    };
}

/// `c_free_sized`. x1 is the size a restart and the slow path read, so the hot word is in x9.
#[doc(hidden)]
#[macro_export]
macro_rules! cralloc_a64_c_free_sized {
    () => {
        concat!(
            ".cfi_startproc\n",
            $crate::cralloc_a64_arm!(),
            $crate::cralloc_a64_c_cpu!(),
            $crate::cralloc_a64_c_slab!(),
            "adrp x2, {C_SIZED_SIZES}+2048\n",
            "add x2, x2, :lo12:{C_SIZED_SIZES}+2048\n",
            "and x9, x1, #0xffff\n",
            "ldrb w5, [x2, x9]\n",
            $crate::cralloc_a64_c_region!(),
            "ldrb w7, [x12, x4]\n",
            "cmp w5, w7\n",
            "b.ne 20f\n",
            "ldr x9, [x3, x5, lsl #3]\n",
            "ldr x6, [x15, x5, lsl #3]\n",
            "add x2, x9, #8\n",
            "tst w2, w6\n",
            "b.eq 20f\n",
            "str x0, [x9]\n",
            "str x2, [x3, x5, lsl #3]\n",
            $crate::cralloc_a64_end!(),
            "ret\n",
            $crate::cralloc_a64_c_miss!(),
        )
    };
}

/// The field of this CPU's slab at the offset in x0: its operand `[x9, x8]`, and its value into
/// x3. x3 starts as `NO_CPU` (2), the answer if the number has no slab.
macro_rules! a64_slab_field {
    () => {
        concat!(
            "mov x3, #2\n",
            "ldr w8, [x10, #4]\n",
            "cmp w8, #2, lsl #12\n",
            "b.hs 7f\n",
            "lsl x8, x8, #12\n",
            "and x9, x0, #0xff8\n",
            "add x8, x8, x9\n",
            "adrp x9, {SLABS}\n",
            "ldr x3, [x9, x8]\n",
        )
    };
}

/// After a slab routine's commit: returns what it read.
macro_rules! a64_slab_return {
    () => {
        concat!("mov x0, x3\n", "ret\n", ".cfi_endproc\n", ".p2align 6\n",)
    };
}
