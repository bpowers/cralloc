//! A stand-in allocator around the proved slab routines, to see what they cost once rustc has
//! inlined them (or called them) at real allocation sites. The fast paths are the sequences in
//! `crates/cralloc/src/front/generated/` (inline: `x86_64_inline/slab.rs`, out of line:
//! `x86_64/slab.rs`), copied instruction for instruction, or the variant a feature names:
//!
//!   slab           inline pop and push, as generated
//!   slab_direct    the same, reading glibc's `__rseq_offset` with one rip-relative load
//!   slab_call      the inline pop calls its slow path from inside the `asm!` block, so the caller
//!                  does not test the result (combine with slab_direct for both)
//!   slab_nonnull   with slab_call: also tells the compiler the result is never null
//!   slab_ool       out-of-line pop and push, as generated: entry on a 64-byte boundary
//!   slab_pc        out of line, one entry per class with the class and mask as immediates
//!
//! It is NOT a correct allocator. The slow paths are plain Rust, not restartable, so they assume
//! the thread is not preempted or migrated while they run (the benchmark pins its threads);
//! magazines that fill up are dropped with their contents; only classes of 8 to 512 bytes with
//! alignment up to 8 are cached, everything else goes to the system allocator. Each class has
//! magazines of 64 slots (mask 496).
#![allow(clippy::missing_safety_doc, dead_code, unused_macros)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::arch::asm;
use std::cell::UnsafeCell;

const SLAB_COUNT: usize = 8192;
const SLAB_BYTES: usize = 4096;
const SLOTS: usize = 64;
const MASK: u32 = (8 * SLOTS - 16) as u32;
const SMALL: usize = 512;

/// The slab array, `SLABS`. All zero, so it lives in .bss and costs nothing until touched.
#[repr(C, align(4096))]
pub struct Slabs(UnsafeCell<[u8; SLAB_COUNT * SLAB_BYTES]>);
unsafe impl Sync for Slabs {}
pub static SLABS: Slabs = Slabs(UnsafeCell::new([0; SLAB_COUNT * SLAB_BYTES]));

/// The generated out-of-line routines' mask table, `MAG_MASKS`.
#[repr(C, align(64))]
pub struct Masks([u32; 512]);
pub static MAG_MASKS: Masks = Masks([MASK; 512]);

extern "C" {
    /// Offset of glibc's registered `struct rseq` from the thread pointer.
    static __rseq_offset: isize;
}

pub struct Slab;

unsafe impl GlobalAlloc for Slab {
    #[inline]
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        if l.size().wrapping_sub(1) < SMALL && l.align() <= 8 {
            return imp::pop((l.size() + 7) >> 3) as *mut u8;
        }
        System.alloc(l)
    }

    #[inline]
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        if l.size().wrapping_sub(1) < SMALL && l.align() <= 8 {
            return imp::push((l.size() + 7) >> 3, p as usize);
        }
        System.dealloc(p, l)
    }
}

/// The calling thread's slab word for `class`, or `None` if its CPU number has no slab.
unsafe fn word(class: usize) -> Option<*mut usize> {
    let cpu: u32;
    asm!("mov {c:e}, dword ptr fs:[{o} + 4]", c = out(reg) cpu, o = in(reg) __rseq_offset, options(nostack, readonly, preserves_flags));
    let cpu = cpu as usize;
    (cpu < SLAB_COUNT).then(|| (SLABS.0.get() as *mut u8).add(cpu * SLAB_BYTES + class * 8) as *mut usize)
}

unsafe fn magazine() -> *mut usize {
    System.alloc_zeroed(Layout::from_size_align_unchecked(8 * SLOTS, 8 * SLOTS)) as *mut usize
}

/// Refills the class's magazine on this CPU with fresh objects and returns one more.
#[cold]
#[inline(never)]
pub unsafe extern "sysv64" fn pop_slow(class: usize) -> usize {
    let size = class * 8;
    let chunk = System.alloc(Layout::from_size_align_unchecked(size * (SLOTS - 3), 8));
    let Some(w) = word(class) else { return chunk as usize };
    let mag = if *w & (8 * SLOTS - 8) == 0 { magazine() } else { (*w & !(8 * SLOTS - 1)) as *mut usize };
    for i in 1..SLOTS - 3 {
        mag.add(i).write(chunk.add(i * size) as usize);
    }
    *w = mag.add(SLOTS - 3) as usize;
    chunk as usize
}

/// Starts a new magazine holding `p` (the old one, if full, is dropped).
#[cold]
#[inline(never)]
pub unsafe extern "sysv64" fn push_slow(class: usize, p: usize) {
    let Some(w) = word(class) else { return };
    let mag = magazine();
    mag.add(1).write(p);
    *w = mag.add(2) as usize;
}

/// Expands to a match that calls `$f::<C>` for the constant class `C` equal to `$c`, so that a
/// constant layout selects one inlined sequence.
macro_rules! by_class {
    ($c:expr, $f:ident ( $($arg:expr),* )) => {
        by_class!(@ $c, $f, ($($arg),*), 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30 31 32 33 34 35 36 37 38 39 40 41 42 43 44 45 46 47 48 49 50 51 52 53 54 55 56 57 58 59 60 61 62 63 64)
    };
    (@ $c:expr, $f:ident, $args:tt, $($n:literal)*) => {
        match $c {
            $($n => by_class!(@call $f, $n, $args),)*
            _ => std::hint::unreachable_unchecked(),
        }
    };
    (@call $f:ident, $n:literal, ($($arg:expr),*)) => { $f::<$n>($($arg),*) };
}

#[cfg(not(feature = "slab_direct"))]
macro_rules! load_rseq {
    () => {
        "mov r10, QWORD PTR [rip + {__rseq_offset}@GOTPCREL]\nmov r10, QWORD PTR [r10]\n"
    };
}
#[cfg(feature = "slab_direct")]
macro_rules! load_rseq {
    () => {
        "mov r10, QWORD PTR [rip + {__rseq_offset}]\n"
    };
}

/// The rseq descriptor and abort handler, as generated.
macro_rules! descriptor {
    () => {
        concat!(
            "7:\n.pushsection __rseq_cs, \"aw\"\n.balign 32\n3:\n.long 0, 0\n.quad 6b, 7b - 6b, 4f\n.popsection\n",
            ".pushsection .text.unlikely, \"ax\"\n.byte 0x0f, 0xb9, 0x3d\n.long 0x53053053\n4:\njmp 5b\n"
        )
    };
}

#[cfg(not(any(feature = "slab_ool", feature = "slab_pc")))]
mod imp {
    use super::*;

    /// The generated inline pop: returns 0 on a miss.
    #[cfg(not(feature = "slab_call"))]
    #[inline(always)]
    unsafe fn pop_inline<const C: usize>() -> usize {
        let ret: usize;
        asm!(
            concat!(
                "5:\n", load_rseq!(), "lea r11, [rip + 3f]\nmov QWORD PTR fs:[r10+8], r11\n6:\n",
                "mov r8d, DWORD PTR fs:[r10+4]\ncmp r8d, 8192\njae 20f\nshl r8, 12\n",
                "lea r9, [rip + {SLABS} + {coff}]\nmov rcx, QWORD PTR [r9+r8*1]\ntest ecx, {mask}\nje 21f\n",
                "mov rax, QWORD PTR [rcx-8]\nlea rcx, [rcx-8]\nmov QWORD PTR [r9+r8*1], rcx\n",
                descriptor!(),
                "20:\nmov eax, 0\njmp 7b\n21:\nmov eax, 0\njmp 7b\n.popsection"
            ),
            lateout("rax") ret,
            out("rcx") _,
            out("r8") _,
            out("r9") _,
            out("r10") _,
            out("r11") _,
            __rseq_offset = sym __rseq_offset,
            SLABS = sym SLABS,
            coff = const C * 8,
            mask = const MASK,
            options(nostack),
        );
        ret
    }

    /// The inline pop calling its slow path from inside the block. A `call` needs the block not
    /// to be `nostack` (the compiler then aligns the stack and keeps the red zone free), and the
    /// block must declare everything the callee may change: `clobber_abi("C")` does, for whatever
    /// registers the target features enable.
    #[cfg(feature = "slab_call")]
    #[inline(always)]
    unsafe fn pop_inline<const C: usize>() -> usize {
        let ret: usize;
        asm!(
            concat!(
                "5:\n", load_rseq!(), "lea r11, [rip + 3f]\nmov QWORD PTR fs:[r10+8], r11\n6:\n",
                "mov r8d, DWORD PTR fs:[r10+4]\ncmp r8d, 8192\njae 20f\nshl r8, 12\n",
                "lea r9, [rip + {SLABS} + {coff}]\nmov rcx, QWORD PTR [r9+r8*1]\ntest ecx, {mask}\nje 20f\n",
                "mov rax, QWORD PTR [rcx-8]\nlea rcx, [rcx-8]\nmov QWORD PTR [r9+r8*1], rcx\n",
                descriptor!(),
                "20:\nmov edi, {class}\ncall {slow}\njmp 7b\n.popsection"
            ),
            lateout("rax") ret,
            __rseq_offset = sym __rseq_offset,
            SLABS = sym SLABS,
            coff = const C * 8,
            mask = const MASK,
            class = const C,
            slow = sym pop_slow,
            clobber_abi("C"),
        );
        ret
    }

    /// The generated inline push: branches to the miss label instead of committing.
    #[inline(always)]
    unsafe fn push_inline<const C: usize>(p: usize) -> bool {
        asm!(
            concat!(
                "5:\n", load_rseq!(), "lea r11, [rip + 3f]\nmov QWORD PTR fs:[r10+8], r11\n6:\n",
                "mov r8d, DWORD PTR fs:[r10+4]\ncmp r8d, 8192\njae {miss}\nshl r8, 12\n",
                "lea r9, [rip + {SLABS} + {coff}]\nmov rcx, QWORD PTR [r9+r8*1]\nlea rax, [rcx+8]\n",
                "test eax, {mask}\nje {miss}\nmov QWORD PTR [rcx], rdi\nmov QWORD PTR [r9+r8*1], rax\n",
                descriptor!(),
                ".popsection"
            ),
            in("rdi") p,
            out("rax") _,
            out("rcx") _,
            out("r8") _,
            out("r9") _,
            out("r10") _,
            out("r11") _,
            __rseq_offset = sym __rseq_offset,
            SLABS = sym SLABS,
            coff = const C * 8,
            mask = const MASK,
            miss = label { return false },
            options(nostack),
        );
        true
    }

    #[inline(always)]
    pub unsafe fn pop(c: usize) -> usize {
        let p = by_class!(c, pop_inline());
        if cfg!(feature = "slab_call") || p != 0 {
            // Box and Vec test the result for null whatever the allocator does; only a promise
            // that it is never null (an allocator that aborts when memory runs out) removes that.
            if cfg!(feature = "slab_nonnull") {
                std::hint::assert_unchecked(p != 0);
            }
            return p;
        }
        pop_slow(c)
    }

    #[inline(always)]
    pub unsafe fn push(c: usize, p: usize) {
        if !by_class!(c, push_inline(p)) {
            push_slow(c, p)
        }
    }
}

/// The generated out-of-line routines, as naked functions: the template is the generated one.
#[cfg(feature = "slab_ool")]
mod imp {
    use super::*;

    #[unsafe(naked)]
    unsafe extern "sysv64" fn slab_pop(class: usize) -> usize {
        core::arch::naked_asm!(
            "5:\nmov r10, QWORD PTR [rip + {__rseq_offset}@GOTPCREL]\nmov r10, QWORD PTR [r10]\nlea r11, [rip + 3f]\nmov QWORD PTR fs:[r10+8], r11\n6:\nmov r8d, DWORD PTR fs:[r10+4]\ncmp r8d, 8192\njae {slow}\nshl r8, 12\nlea r9, [rip + {SLABS}]\nlea r9, [r9+rdi*8]\nmov rcx, QWORD PTR [r9+r8*1]\nlea r11, [rip + {MAG_MASKS}]\ntest ecx, DWORD PTR [r11+rdi*4]\nje {slow}\nmov rax, QWORD PTR [rcx-8]\nlea rcx, [rcx-8]\nmov QWORD PTR [r9+r8*1], rcx\n7:\n.pushsection __rseq_cs, \"aw\"\n.balign 32\n3:\n.long 0, 0\n.quad 6b, 7b - 6b, 4f\n.popsection\n.pushsection .text.unlikely, \"ax\"\n.byte 0x0f, 0xb9, 0x3d\n.long 0x53053053\n4:\njmp 5b\n.popsection\nret\n.p2align 6",
            __rseq_offset = sym __rseq_offset,
            slow = sym pop_slow,
            SLABS = sym SLABS,
            MAG_MASKS = sym MAG_MASKS,
        )
    }

    #[unsafe(naked)]
    unsafe extern "sysv64" fn slab_push(class: usize, p: usize) {
        core::arch::naked_asm!(
            "5:\nmov r10, QWORD PTR [rip + {__rseq_offset}@GOTPCREL]\nmov r10, QWORD PTR [r10]\nlea r11, [rip + 3f]\nmov QWORD PTR fs:[r10+8], r11\n6:\nmov r8d, DWORD PTR fs:[r10+4]\ncmp r8d, 8192\njae {slow}\nshl r8, 12\nlea r9, [rip + {SLABS}]\nlea r9, [r9+rdi*8]\nmov rcx, QWORD PTR [r9+r8*1]\nlea r11, [rip + {MAG_MASKS}]\nlea rax, [rcx+8]\ntest eax, DWORD PTR [r11+rdi*4]\nje {slow}\nmov QWORD PTR [rcx], rsi\nmov QWORD PTR [r9+r8*1], rax\n7:\n.pushsection __rseq_cs, \"aw\"\n.balign 32\n3:\n.long 0, 0\n.quad 6b, 7b - 6b, 4f\n.popsection\n.pushsection .text.unlikely, \"ax\"\n.byte 0x0f, 0xb9, 0x3d\n.long 0x53053053\n4:\njmp 5b\n.popsection\nret\n.p2align 6",
            __rseq_offset = sym __rseq_offset,
            slow = sym push_slow,
            SLABS = sym SLABS,
            MAG_MASKS = sym MAG_MASKS,
        )
    }

    #[inline(always)]
    pub unsafe fn pop(c: usize) -> usize {
        slab_pop(c)
    }

    #[inline(always)]
    pub unsafe fn push(c: usize, p: usize) {
        slab_push(c, p)
    }
}

/// Out of line, one pop and one push per class with the class and mask as immediates: the inline
/// sequences plus a `ret`, entry on a 64-byte boundary. A miss tail-calls the slow path through a
/// cold stub that sets the class argument.
#[cfg(feature = "slab_pc")]
mod imp {
    use super::*;

    std::arch::global_asm!(
        ".irp c, 1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31,32,33,34,35,36,37,38,39,40,41,42,43,44,45,46,47,48,49,50,51,52,53,54,55,56,57,58,59,60,61,62,63,64",
        ".text", ".balign 64", ".globl rk_pc_pop_\\c", "rk_pc_pop_\\c:",
        "5:", "mov r10, QWORD PTR [rip + {rseq}@GOTPCREL]", "mov r10, QWORD PTR [r10]", "lea r11, [rip + 3f]",
        "mov QWORD PTR fs:[r10+8], r11", "6:", "mov r8d, DWORD PTR fs:[r10+4]", "cmp r8d, 8192", "jae 20f", "shl r8, 12",
        "lea r9, [rip + {slabs} + \\c*8]", "mov rcx, QWORD PTR [r9+r8*1]", "test ecx, {mask}", "je 20f",
        "mov rax, QWORD PTR [rcx-8]", "lea rcx, [rcx-8]", "mov QWORD PTR [r9+r8*1], rcx",
        "7:", ".pushsection __rseq_cs, \"aw\"", ".balign 32", "3:", ".long 0, 0", ".quad 6b, 7b - 6b, 4f", ".popsection",
        ".pushsection .text.unlikely, \"ax\"", ".byte 0x0f, 0xb9, 0x3d", ".long 0x53053053", "4:", "jmp 5b",
        "20:", "mov edi, \\c", "jmp {pop_slow}", ".popsection", "ret",
        ".balign 64", ".globl rk_pc_push_\\c", "rk_pc_push_\\c:",
        "5:", "mov r10, QWORD PTR [rip + {rseq}@GOTPCREL]", "mov r10, QWORD PTR [r10]", "lea r11, [rip + 3f]",
        "mov QWORD PTR fs:[r10+8], r11", "6:", "mov r8d, DWORD PTR fs:[r10+4]", "cmp r8d, 8192", "jae 20f", "shl r8, 12",
        "lea r9, [rip + {slabs} + \\c*8]", "mov rcx, QWORD PTR [r9+r8*1]", "lea rax, [rcx+8]", "test eax, {mask}",
        "je 20f", "mov QWORD PTR [rcx], rdi", "mov QWORD PTR [r9+r8*1], rax",
        "7:", ".pushsection __rseq_cs, \"aw\"", ".balign 32", "3:", ".long 0, 0", ".quad 6b, 7b - 6b, 4f", ".popsection",
        ".pushsection .text.unlikely, \"ax\"", ".byte 0x0f, 0xb9, 0x3d", ".long 0x53053053", "4:", "jmp 5b",
        "20:", "mov rsi, rdi", "mov edi, \\c", "jmp {push_slow}", ".popsection", "ret",
        ".endr",
        rseq = sym __rseq_offset,
        slabs = sym SLABS,
        mask = const MASK,
        pop_slow = sym pop_slow,
        push_slow = sym push_slow,
    );

    /// A call to the class's entry. The entries are ordinary System V functions, so the block
    /// declares what a call clobbers and is not `nostack`.
    macro_rules! pc_pop {
        ($n:literal) => {{
            let r: usize;
            asm!(concat!("call rk_pc_pop_", stringify!($n)), lateout("rax") r, clobber_abi("C"));
            r
        }};
    }
    macro_rules! pc_push {
        ($n:literal, $p:expr) => {{
            asm!(concat!("call rk_pc_push_", stringify!($n)), in("rdi") $p, clobber_abi("C"));
        }};
    }
    macro_rules! by_class_m {
        ($c:expr, $m:ident $($rest:tt)*) => {
            by_class_m!(@ $c, $m, ($($rest)*), 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30 31 32 33 34 35 36 37 38 39 40 41 42 43 44 45 46 47 48 49 50 51 52 53 54 55 56 57 58 59 60 61 62 63 64)
        };
        (@ $c:expr, $m:ident, $rest:tt, $($n:literal)*) => {
            match $c {
                $($n => by_class_m!(@call $m, $n, $rest),)*
                _ => std::hint::unreachable_unchecked(),
            }
        };
        (@call $m:ident, $n:literal, ($($rest:tt)*)) => { $m!($n $($rest)*) };
    }

    #[inline(always)]
    pub unsafe fn pop(c: usize) -> usize {
        by_class_m!(c, pc_pop)
    }

    #[inline(always)]
    pub unsafe fn push(c: usize, p: usize) {
        by_class_m!(c, pc_push, p)
    }
}
