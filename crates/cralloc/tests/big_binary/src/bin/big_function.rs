//! One function of more than a megabyte, past what a conditional branch reaches, with the routines
//! inlined at both ends of it: a program a compiler is free to make, from generated code or from
//! inlining. Whatever a routine branches to on a miss has to lie within its reach all the same.
use std::alloc::{GlobalAlloc, Layout};

#[global_allocator]
static ALLOCATOR: cralloc::Cralloc = cralloc::Cralloc;

/// Jumps over two megabytes of the function's own text.
macro_rules! gap {
    () => {
        // SAFETY: jumps over bytes that nothing runs, and changes nothing else.
        unsafe {
            #[cfg(target_arch = "x86_64")]
            core::arch::asm!("jmp 9f", ".skip 2000000", "9:", options(nomem, nostack));
            #[cfg(target_arch = "aarch64")]
            core::arch::asm!("b 9f", ".skip 2000000", "9:", options(nomem, nostack));
        }
    };
}

/// The routines inlined before and after the gap, each through its hit and its miss: every
/// allocation is freed in the other half, so that magazines fill and empty.
#[inline(never)]
fn across(l: Layout, n: usize) -> usize {
    let mut sum = 0;
    let mut held = Vec::with_capacity(n);
    for i in 0..n {
        // SAFETY: a valid layout of a non-zero size.
        let p = unsafe { ALLOCATOR.alloc(l) };
        // SAFETY: the block holds at least a byte.
        unsafe { p.write(i as u8) };
        held.push(p);
    }
    gap!();
    for p in held {
        // SAFETY: the block allocated above, with its layout, freed once.
        unsafe {
            sum += p.read() as usize;
            ALLOCATOR.dealloc(p, l);
        }
    }
    sum
}

fn main() {
    let sum: usize = [8, 64, 1024, 32 << 10]
        .into_iter()
        .map(|size| across(Layout::from_size_align(size, 8).unwrap(), 5000))
        .sum();
    println!("ok {sum}");
}
