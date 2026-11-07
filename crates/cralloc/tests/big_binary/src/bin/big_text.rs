//! A program with 400 MB of text between its cold functions and the rest: the linker orders hot
//! text after cold, and the filler is hot, so a routine's slow path lies beyond what a branch
//! reaches. The program takes every routine through its hit and its miss: both C entry points and
//! the inlined routines.
use std::alloc::{GlobalAlloc, Layout};

#[global_allocator]
static ALLOCATOR: cralloc::Cralloc = cralloc::Cralloc;

mod exported {
    cralloc::export_c_abi!();
}

core::arch::global_asm!(
    ".section .text.hot.filler,\"axR\",%progbits",
    ".skip 400000000",
    ".text",
);

unsafe extern "C" {
    fn malloc(size: usize) -> *mut u8;
    fn calloc(n: usize, size: usize) -> *mut u8;
    fn free(p: *mut u8);
    fn free_sized(p: *mut u8, size: usize);
}

fn main() {
    let mut sum = 0usize;
    for round in 0..20_000 {
        let size = 8 + round % 2000;
        let l = Layout::from_size_align(size, 8).unwrap();
        // SAFETY: valid layouts and sizes, and each block is freed once as it was allocated.
        unsafe {
            let a = ALLOCATOR.alloc(l);
            a.write(1);
            let b = malloc(size);
            b.write(2);
            let c = calloc(3, size);
            assert_eq!(c.read(), 0);
            sum += a.read() as usize + b.read() as usize;
            ALLOCATOR.dealloc(a, l);
            free(b);
            free_sized(c, 3 * size);
        }
    }
    // Enough at once to take every class through its slow paths.
    let blocks: Vec<Vec<u8>> = (0..200_000).map(|i| vec![i as u8; 1 + i % 5000]).collect();
    sum += blocks.iter().map(|b| b[0] as usize).sum::<usize>();
    drop(blocks);
    println!("ok {sum}");
}
