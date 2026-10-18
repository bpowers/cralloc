//! Many threads make the process heap's first allocation at once. The program's own allocator
//! is the system's, so nothing has started asmalloc's heap before they are released.
use asmalloc::Asmalloc;
use asmalloc::stats::{Region, Visitor};
use std::alloc::{GlobalAlloc, Layout};
use std::sync::{Arc, Barrier};

struct Seen(usize);
impl Visitor for Seen {
    fn region(&mut self, region: &Region) {
        self.0 = region.len;
    }
}

fn main() {
    let threads: usize = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(64);
    let barrier = Arc::new(Barrier::new(threads));
    let layout = Layout::from_size_align(64, 8).unwrap();
    let handles: Vec<_> = (0..threads)
        .map(|_| {
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                // SAFETY: a valid, non-empty layout.
                let p = unsafe { Asmalloc.alloc(layout) };
                p.is_null()
            })
        })
        .collect();
    let nulls = handles
        .into_iter()
        .map(|h| h.join().unwrap())
        .filter(|&null| null)
        .count();
    // SAFETY: as above.
    let _ = unsafe { Asmalloc.alloc(layout) };
    let mut seen = Seen(0);
    Asmalloc::inspect(&mut seen);
    println!("nulls {nulls} region_shift {}", seen.0.trailing_zeros());
}
