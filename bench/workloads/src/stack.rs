//! How much stack the allocator's slow paths take below a caller's frame: a probe, not a
//! benchmark. A slow path runs on whatever stack called `free`, a small thread's or an
//! alternate signal stack among them.
//!
//!   workload stack [n=1000000] [bound=BYTES]
//!
//! On a thread of its own, it paints a stretch of stack below the loop's frame, allocates `n`
//! objects of a few sizes, and frees them in a random order (scattered launders, wide ones
//! once `n` is large) and then allocates and frees them in order (dense ones). The deepest
//! painted byte overwritten is the slow paths' peak, measured from the loop's frame, so the
//! loop's own few dozen bytes are counted too. Prints `peak_stack_bytes`, and exits with 1 if
//! it is above `bound`. Built with the release profile, as the allocator ships.

use bench_harness::{Args, Rng};
use std::hint::black_box;

/// Stack painted below the loop's frame: far more than any slow path takes.
const PAINT: usize = 128 << 10;
const MARK: u8 = 0xa5;

pub fn stack(args: &Args) {
    let n = args.usize("n", 1_000_000);
    let bound = args.usize("bound", 0);
    let peak = std::thread::Builder::new()
        .stack_size(PAINT + (1 << 20))
        .spawn(move || probe(n))
        .expect("spawn the probe thread")
        .join()
        .expect("the probe thread");
    println!("peak_stack_bytes={peak}");
    if bound > 0 && peak > bound {
        eprintln!("the slow paths took {peak} bytes of stack, more than {bound}");
        std::process::exit(1);
    }
}

/// Runs the frees with a painted stack below this frame and returns how deep they reached.
#[inline(never)]
fn probe(n: usize) -> usize {
    let mut objects: Vec<Box<[u64]>> = Vec::with_capacity(n);
    let mut order: Vec<usize> = (0..n).collect();
    let mut rng = Rng::new(1);
    for i in (1..n).rev() {
        order.swap(i, rng.below(i as u64 + 1) as usize);
    }
    let here = black_box(0u8);
    let top = std::ptr::addr_of!(here) as usize;
    paint();
    // Scattered: allocate in order, free in a random order.
    for i in 0..n {
        objects.push(vec![i as u64; 1 + i % 6].into_boxed_slice());
    }
    let mut slots: Vec<Option<Box<[u64]>>> = objects.drain(..).map(Some).collect();
    for &i in &order {
        drop(black_box(slots[i].take()));
    }
    // Dense: allocate and free in order.
    for i in 0..n {
        objects.push(vec![i as u64; 1 + i % 6].into_boxed_slice());
    }
    for object in objects.drain(..) {
        drop(black_box(object));
    }
    top - deepest(top)
}

/// Fills `PAINT` bytes of stack below the caller's frame with `MARK`.
#[inline(never)]
fn paint() {
    let mut area = [0u8; PAINT];
    for byte in area.iter_mut() {
        // SAFETY: a byte of a local array.
        unsafe { std::ptr::write_volatile(byte, MARK) };
    }
    black_box(&area);
}

/// The lowest address below `top`, within the painted stretch, whose byte is no longer the
/// mark.
fn deepest(top: usize) -> usize {
    let low = top - PAINT;
    for addr in low..top {
        // SAFETY: the stretch is this thread's own stack, mapped for the thread's life: the
        // frames that used it have returned, and reading their old bytes is all this does.
        let byte = unsafe { std::ptr::read_volatile(addr as *const u8) };
        if byte != MARK {
            return addr;
        }
    }
    top
}
