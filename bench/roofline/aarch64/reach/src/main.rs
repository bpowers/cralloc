//! Does an inlined fast path reach its cold stub and its data, wherever the build puts them?
//!
//! `pop` is a restartable sequence's shape reduced to what matters here: a conditional branch to a
//! stub, an abort handler behind a signature, a descriptor, and the address of `__rseq_offset`. It
//! is inlined at the site in `hot`, and `check.sh` builds this file every way that matters and
//! reads where the stubs landed.
use std::arch::asm;
use std::sync::atomic::{AtomicU64, Ordering};

extern "C" {
    static __rseq_offset: isize;
}

static WORD: AtomicU64 = AtomicU64::new(8);

/// The stub sits in `.subsection 1`: past the end of the function, in its own section.
#[cfg(not(any(feature = "stub_label", feature = "stub_unlikely", feature = "stub_none")))]
#[inline(always)]
fn pop(word: &AtomicU64) -> u64 {
    let out: u64;
    unsafe {
        asm!(
            "5:",
            "ldr {out}, [{word}]",
            "tst {out}, #0x1f0",
            "b.eq 20f",
            "6:",
            ".pushsection __rseq_cs, \"aw\"",
            ".balign 32",
            "3:",
            ".long 0, 0",
            ".quad 5b, 6b - 5b, 4f",
            ".popsection",
            ".subsection 1",
            ".inst 0xd428bc00",
            "4:",
            "b 5b",
            "20:",
            "mov {out}, #0",
            "b 6b",
            ".subsection 0",
            word = in(reg) word,
            out = out(reg) out,
            options(nostack),
        );
    }
    out
}

/// The stub in `.text.unlikely`, the x86-64 routines' placement, reached by a conditional branch.
#[cfg(feature = "stub_unlikely")]
#[inline(always)]
fn pop(word: &AtomicU64) -> u64 {
    let out: u64;
    unsafe {
        asm!(
            "6:",
            "ldr {out}, [{word}]",
            "tst {out}, #0x1f0",
            "b.eq 20f",
            "7:",
            ".pushsection .text.unlikely, \"ax\"",
            "20:",
            "mov {out}, #0",
            "b 7b",
            ".popsection",
            word = in(reg) word,
            out = out(reg) out,
            options(nostack),
        );
    }
    out
}

/// The miss through a `label` operand, as a push has it: the compiler places the block. A `label`
/// cannot be combined with an output on stable, so the result goes through memory here.
#[cfg(feature = "stub_label")]
#[inline(always)]
fn pop(word: &AtomicU64) -> u64 {
    unsafe {
        asm!(
            "ldr x9, [{word}]",
            "tst x9, #0x1f0",
            "b.eq {miss}",
            word = in(reg) word,
            out("x9") _,
            miss = label { return miss() },
            options(nostack),
        );
    }
    word.load(Ordering::Relaxed)
}

/// Cold, so that the compiler moves the miss's block past whatever else the function holds.
#[cfg(feature = "stub_label")]
#[cold]
#[inline(never)]
fn miss() -> u64 {
    std::hint::black_box(0)
}

/// No stub: a miss leaves the sequence with its result still zero. Only the abort handler needs a
/// place of its own. The kernel reaches it by its address and it holds only an unconditional
/// branch, whose 128 MiB reach no function exceeds, so `.subsection 1` holds it with no veneer.
#[cfg(feature = "stub_none")]
#[inline(always)]
fn pop(word: &AtomicU64) -> u64 {
    let out: u64;
    unsafe {
        asm!(
            "5:",
            "mov {out}, xzr",
            "ldr {tmp}, [{word}]",
            "tst {tmp}, #0x1f0",
            "b.eq 6f",
            "mov {out}, {tmp}",
            "6:",
            ".pushsection __rseq_cs, \"aw\"",
            ".balign 32",
            "3:",
            ".long 0, 0",
            ".quad 5b, 6b - 5b, 4f",
            ".popsection",
            ".subsection 1",
            ".inst 0xd428bc00",
            "4:",
            "b 5b",
            ".subsection 0",
            word = in(reg) word,
            out = out(reg) out,
            tmp = out(reg) _,
            options(nostack),
        );
    }
    out
}

/// Pads the function it is inlined into past a conditional branch's reach.
#[cfg(feature = "big_function")]
#[inline(always)]
fn pad() {
    unsafe { asm!(".rept 300000", "nop", ".endr", options(nostack, nomem, preserves_flags)) };
}
#[cfg(not(feature = "big_function"))]
#[inline(always)]
fn pad() {}

// A lot of text between the site and a stub elsewhere: the linker's script orders .text.unlikely,
// then .text.hot, then the rest, so this lies between a stub in .text.unlikely and `hot`.
#[cfg(feature = "big_text")]
std::arch::global_asm!(
    ".section .text.hot.rf_pad, \"ax\"",
    ".globl rf_pad",
    "rf_pad:",
    ".skip 300 * 1024 * 1024",
    ".text"
);

#[cfg(feature = "big_text")]
extern "C" {
    static rf_pad: u8;
}

/// Keeps the padding in the link.
#[cfg(feature = "big_text")]
fn keep_pad() -> usize {
    std::hint::black_box(&raw const rf_pad) as usize & 1
}
#[cfg(not(feature = "big_text"))]
fn keep_pad() -> usize {
    0
}

#[cfg(feature = "direct_offset")]
fn rseq_offset() -> isize {
    let v: isize;
    unsafe {
        asm!(
            "adrp {v}, {sym}",
            "ldr {v}, [{v}, :lo12:{sym}]",
            v = out(reg) v,
            sym = sym __rseq_offset,
            options(nostack, readonly),
        );
    }
    v
}

#[cfg(not(feature = "direct_offset"))]
fn rseq_offset() -> isize {
    let v: isize;
    unsafe {
        asm!(
            "adrp {v}, :got:{sym}",
            "ldr {v}, [{v}, :got_lo12:{sym}]",
            "ldr {v}, [{v}]",
            v = out(reg) v,
            sym = sym __rseq_offset,
            options(nostack, readonly),
        );
    }
    v
}

#[inline(never)]
fn hot(n: u64) -> u64 {
    let mut sum = 0;
    for _ in 0..n {
        sum += pop(&WORD);
    }
    pad();
    sum
}

fn main() {
    WORD.store(16, Ordering::Relaxed);
    let n: u64 = std::env::args().count() as u64;
    println!("offset {} sum {} pad {}", rseq_offset(), hot(n), keep_pad());
}
