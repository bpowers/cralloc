//! Allocation sites of each kind, for `check.py` to disassemble. Each `site_*` function is a
//! loop of one allocation and its free; `check.py` asserts that the loop holds the inlined
//! sequences and no call. Running the binary exercises each once. Each block's address
//! escapes through `black_box`, or LLVM, which knows what the allocator's functions do, removes
//! the pair altogether.

use std::alloc::{alloc, dealloc, Layout};
use std::hint::black_box;

#[global_allocator]
static A: cralloc::Cralloc = cralloc::Cralloc;

const L32: Layout = match Layout::from_size_align(32, 8) {
    Ok(layout) => layout,
    Err(_) => panic!(),
};

/// A layout known at compile time.
#[inline(never)]
pub fn site_constant(n: u64) -> u64 {
    let mut acc = 0u64;
    for i in 0..n {
        // SAFETY: a valid layout; each block is written, read and freed once.
        unsafe {
            let p = black_box(alloc(L32)).cast::<u64>();
            if p.is_null() {
                return acc;
            }
            p.write(i);
            acc = acc.wrapping_add(p.read());
            dealloc(p.cast(), L32);
        }
    }
    acc
}

/// `Box`, whose layout is a constant too, but which reaches the allocator through std.
#[inline(never)]
pub fn site_box(n: u64) -> u64 {
    let mut acc = 0u64;
    for i in 0..n {
        let b = black_box(Box::new([i, 1, 2, 3]));
        acc = acc.wrapping_add(b[0]);
    }
    acc
}

/// A size known only at run time.
#[inline(never)]
pub fn site_runtime(n: u64, size: usize) -> u64 {
    let mut acc = 0u64;
    for i in 0..n {
        // SAFETY: as above; `size` is not zero (the caller's).
        unsafe {
            let l = Layout::from_size_align_unchecked(black_box(size), 8);
            let p = black_box(alloc(l)).cast::<u64>();
            if p.is_null() {
                return acc;
            }
            p.write(i);
            acc = acc.wrapping_add(p.read());
            dealloc(p.cast(), l);
        }
    }
    acc
}

fn main() {
    let a = site_constant(black_box(1000));
    let b = site_box(black_box(1000));
    let c = site_runtime(black_box(1000), black_box(48));
    println!("{}", a.wrapping_add(b).wrapping_add(c));
}
