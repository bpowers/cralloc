//! `benchrun layout`'s padN variants: N bytes of code that never runs, ahead of the allocator
//! and the program in the linker's order, so that everything placed after it moves by exactly
//! N bytes and nothing else changes (`docs/research/measuring.md`, "One number from each of two
//! builds measures where the code landed"). Built only when `BENCH_LAYOUT_PAD` is set at build
//! time (`build.rs`).

const fn parse(s: &str) -> usize {
    let b = s.as_bytes();
    let (mut i, mut n) = (0, 0);
    while i < b.len() {
        n = n * 10 + (b[i] - b'0') as usize;
        i += 1;
    }
    n
}

const BYTES: usize = parse(env!("BENCH_LAYOUT_PAD_BYTES"));

/// A byte whose repetition traps: int3 on x86-64, and on aarch64 the permanently undefined
/// `udf #0`.
const TRAP: u8 = if cfg!(target_arch = "aarch64") { 0 } else { 0xcc };

/// Never called: a run of trapping instructions the size of the pad.
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub extern "C" fn bench_layout_pad() {
    core::arch::naked_asm!(".skip {n}, {trap}", n = const BYTES, trap = const TRAP);
}

// Kept by the linker, which would otherwise drop a function nothing calls.
#[used]
static KEEP: extern "C" fn() = bench_layout_pad;
