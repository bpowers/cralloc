//! The life of a zeroed large block, for docs/research/meili-peak.md: allocate it zeroed, write
//! a fraction of its pages, free it, over and over, with asmalloc as the global allocator and
//! the prototype's switch (`ASMALLOC_ZPURGE`) as the environment sets it.
//!
//!   zbench [REPS]
//!
//! For each size (1, 4, 16, 64 MiB), slot state and fraction written (0, 1/16, 1/4, 1/2, 1 of
//! the pages, one store a page from the start), the median cycles of one life (allocate, write,
//! free) and the minor faults a life. Slot states: `resident`, the slot the last life left, its
//! pages still resident (a block freed and taken again within the hold); `returned`, the same
//! slot after the program returned the block's pages itself before freeing it (a slot the
//! hold or a payment purged in part, so not known to be zero). The program's own return is
//! not timed.
use std::alloc::{GlobalAlloc, Layout};

#[global_allocator]
static A: asmalloc::Asmalloc = asmalloc::Asmalloc;

fn minflt() -> i64 {
    // SAFETY: getrusage writes one rusage.
    let mut r: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut r) };
    r.ru_minflt
}

fn rdtsc() -> u64 {
    // SAFETY: rdtsc has no preconditions on x86-64.
    unsafe { core::arch::x86_64::_rdtsc() }
}

fn main() {
    let reps: usize = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(40);
    let mode = std::env::var("ASMALLOC_ZPURGE").unwrap_or_default();
    let page = 4096;
    println!("zpurge={mode}");
    println!("{:>6} {:>9} {:>6} {:>12} {:>10}", "MiB", "slot", "frac", "kcycles", "faults");
    for mib in [1usize, 4, 16, 64] {
        let size = mib << 20;
        let layout = Layout::from_size_align(size, 8).unwrap();
        for returned in [false, true] {
            for (num, den) in [(0usize, 1usize), (1, 16), (1, 4), (1, 2), (1, 1)] {
                let pages = size / page * num / den;
                let mut cycles = Vec::with_capacity(reps);
                let mut faults = 0i64;
                // Two lives to settle the slot first.
                for i in 0..reps + 2 {
                    let f0 = minflt();
                    let t0 = rdtsc();
                    // SAFETY: a valid layout of non-zero size.
                    let p = unsafe { A.alloc_zeroed(layout) };
                    assert!(!p.is_null());
                    for k in 0..pages {
                        // SAFETY: inside the block.
                        unsafe { p.add(k * page).write_volatile(1) };
                    }
                    let t1 = rdtsc();
                    let f1 = minflt();
                    if returned {
                        // SAFETY: the program's own block, whole pages of it, which it does not
                        // read again before freeing it.
                        let start = (p as usize).next_multiple_of(page);
                        let end = (p as usize + size) & !(page - 1);
                        unsafe { libc::madvise(start as *mut libc::c_void, end - start, libc::MADV_DONTNEED) };
                    }
                    let t2 = rdtsc();
                    // SAFETY: allocated above with this layout.
                    unsafe { A.dealloc(p, layout) };
                    let t3 = rdtsc();
                    if i >= 2 {
                        cycles.push((t1 - t0) + (t3 - t2));
                        faults += f1 - f0;
                    }
                }
                cycles.sort_unstable();
                println!(
                    "{:>6} {:>9} {:>6} {:>12.1} {:>10.0}",
                    mib,
                    if returned { "returned" } else { "resident" },
                    format!("{num}/{den}"),
                    cycles[reps / 2] as f64 / 1e3,
                    faults as f64 / reps as f64
                );
            }
        }
    }
}
