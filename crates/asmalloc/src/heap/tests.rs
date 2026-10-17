//! The heap through `GlobalAlloc`: the process's heap with the generated routines where they
//! run, and simulated heaps with twins everywhere.

use super::large::HEADROOM;
use crate::SimHeap;
use crate::classes::{Class, MAX_SMALL_SIZE, SmallClass};
use crate::os::OsError;
use crate::os::sim::{Call, Faults, SIM_THREADS, SimOs};

/// The simulated shape: one with large classes natively, the small one Miri can afford.
#[cfg(not(miri))]
type Shape = crate::os::sim::Native;
#[cfg(miri)]
type Shape = crate::os::sim::Standard;
use core::alloc::{GlobalAlloc, Layout};
use core::ptr;
use std::thread;
use std::vec::Vec;

fn layout(size: usize, align: usize) -> Layout {
    Layout::from_size_align(size, align).unwrap()
}

/// What a test needs of a heap besides `GlobalAlloc`.
trait Inspect: GlobalAlloc + Sync {
    fn contains(&self, ptr: *const u8) -> bool;
    fn usable(&self, ptr: *const u8) -> Option<usize>;
}

impl<S: crate::os::sim::Shape> Inspect for SimHeap<S> {
    fn contains(&self, ptr: *const u8) -> bool {
        SimHeap::contains(self, ptr)
    }

    fn usable(&self, ptr: *const u8) -> Option<usize> {
        self.usable_size(ptr)
    }
}

/// Fills `len` bytes at `p` with a pattern derived from `seed`.
///
/// # Safety
///
/// `p..p + len` is a live block.
unsafe fn fill(p: *mut u8, len: usize, seed: u8) {
    for i in 0..len {
        // SAFETY: the caller's.
        unsafe { p.add(i).write(seed.wrapping_add(i as u8)) };
    }
}

/// Whether `len` bytes at `p` hold the pattern for `seed`.
///
/// # Safety
///
/// `p..p + len` is a live block.
unsafe fn holds(p: *const u8, len: usize, seed: u8) -> bool {
    // SAFETY: the caller's.
    (0..len).all(|i| unsafe { p.add(i).read() } == seed.wrapping_add(i as u8))
}

/// Allocates every small class and a few large sizes, checks alignment, usable size and
/// disjointness, writes every byte, and frees in a scrambled order.
fn every_class<H: Inspect>(heap: &H, rounds: usize) {
    let mut sizes: Vec<(usize, usize)> = SmallClass::all()
        .map(|c| (c.size(), 8))
        .chain([(1, 1), (24, 8), (100, 64), (3000, 1024), (MAX_SMALL_SIZE + 1, 8)])
        .collect();
    sizes.extend([(40 << 10, 16), (60 << 10, 4096), (200 << 10, 8)]);
    for round in 0..rounds {
        let mut live = Vec::new();
        for &(size, align) in &sizes {
            let l = layout(size, align);
            // SAFETY: a valid, non-empty layout.
            let p = unsafe { heap.alloc(l) };
            assert!(!p.is_null(), "{l:?}");
            assert_eq!(p.addr() % align, 0, "{l:?}");
            assert!(heap.contains(p));
            let usable = heap.usable(p).expect("a block starts here");
            assert!(usable >= size, "{l:?}: {usable}");
            // SAFETY: the block is live and `size` long.
            unsafe { fill(p, size, round as u8) };
            live.push((p, l));
        }
        let mut ranges: Vec<(usize, usize)> = live
            .iter()
            .map(|&(p, l)| (p.addr(), p.addr() + l.size()))
            .collect();
        ranges.sort_unstable();
        assert!(ranges.windows(2).all(|w| w[0].1 <= w[1].0), "blocks overlap");
        for i in (0..live.len()).rev().step_by(2).chain((0..live.len()).step_by(2)) {
            let (p, l) = live[i];
            if p.is_null() {
                continue;
            }
            // SAFETY: still live, as written.
            assert!(unsafe { holds(p, l.size(), round as u8) });
            // SAFETY: allocated with `l` above and freed once.
            unsafe { heap.dealloc(p, l) };
            live[i].0 = ptr::null_mut();
        }
    }
}

/// Allocates and frees one class far past a magazine's worth, in bursts, so every slow path
/// runs: exchanges with spares and the depot, fills and launders.
fn bursts<H: GlobalAlloc>(heap: &H, size: usize, burst: usize, rounds: usize) {
    let l = layout(size, 8);
    let mut held = Vec::with_capacity(burst);
    for round in 0..rounds {
        for _ in 0..burst {
            // SAFETY: a valid, non-empty layout.
            let p = unsafe { heap.alloc(l) };
            assert!(!p.is_null());
            // SAFETY: the block is live and at least 8 bytes.
            unsafe { p.cast::<usize>().write(round) };
            held.push(p);
        }
        for p in held.drain(..) {
            // SAFETY: still live.
            assert_eq!(unsafe { p.cast::<usize>().read() }, round);
            // SAFETY: allocated with `l` and freed once.
            unsafe { heap.dealloc(p, l) };
        }
    }
}

/// Grows a buffer from one byte to `top`, then shrinks it, through `realloc`, checking the
/// contents survive and that large growth moves rarely.
fn realloc_walk<H: Inspect>(heap: &H, top: usize) {
    let align = 8;
    let mut size = 1;
    // SAFETY: a valid, non-empty layout.
    let mut p = unsafe { heap.alloc(layout(size, align)) };
    assert!(!p.is_null());
    // SAFETY: the block is live and `size` long.
    unsafe { fill(p, size, 7) };
    let mut large_moves = 0;
    let mut large_steps = 0;
    while size < top {
        let next = size * 2;
        // SAFETY: `p` is live with this layout; `next` is valid.
        let q = unsafe { heap.realloc(p, layout(size, align), next) };
        assert!(!q.is_null());
        // SAFETY: the first `size` bytes were carried over.
        assert!(unsafe { holds(q, size, 7) }, "at {size}");
        if next > MAX_SMALL_SIZE && size > MAX_SMALL_SIZE {
            large_steps += 1;
            large_moves += usize::from(q != p);
        }
        // SAFETY: the block is live and `next` long.
        unsafe { fill(q, next, 7) };
        p = q;
        size = next;
    }
    if large_steps >= 4 {
        // With headroom of HEADROOM times, at most every other doubling moves.
        assert!(
            large_moves * 2 <= large_steps + 1,
            "{large_moves} moves in {large_steps} doublings with headroom {HEADROOM}"
        );
    }
    // Shrinking stays in place while the object is large and keeps its contents.
    while size > 1 {
        let next = size / 2;
        // SAFETY: as above.
        let q = unsafe { heap.realloc(p, layout(size, align), next) };
        assert!(!q.is_null());
        if next > MAX_SMALL_SIZE {
            assert_eq!(q, p, "a large shrink that fits stays in place");
        }
        // SAFETY: the first `next` bytes were carried over.
        assert!(unsafe { holds(q, next, 7) }, "at {next}");
        p = q;
        size = next;
    }
    // SAFETY: allocated with this layout.
    unsafe { heap.dealloc(p, layout(size, align)) };
}

/// Zeroed allocations read as zero, also when they reuse a block that was written.
fn zeroed<H: GlobalAlloc>(heap: &H, sizes: &[usize]) {
    for &size in sizes {
        let l = layout(size, 16);
        for _ in 0..3 {
            // SAFETY: a valid, non-empty layout.
            let p = unsafe { heap.alloc_zeroed(l) };
            assert!(!p.is_null());
            // SAFETY: the block is live and `size` long.
            unsafe {
                assert!((0..size).all(|i| p.add(i).read() == 0), "size {size}");
                p.write_bytes(0xa5, size);
                heap.dealloc(p, l);
            }
        }
    }
}

/// A block on its way to another thread.
struct Sent(*mut u8, usize);

// SAFETY: the sender gives the block up; only the receiver uses it.
unsafe impl Send for Sent {}

/// Several threads allocate and free, and hand blocks to each other.
fn threads<H: GlobalAlloc + Sync>(heap: &H, ops: usize) {
    let (sender, receiver) = std::sync::mpsc::channel::<Sent>();
    let receiver = std::sync::Mutex::new(receiver);
    thread::scope(|scope| {
        for t in 0..4 {
            let sender = sender.clone();
            let receiver = &receiver;
            scope.spawn(move || {
                for i in 0..ops {
                    let size = 8 << ((i + t) % 9);
                    // SAFETY: a valid, non-empty layout.
                    let p = unsafe { heap.alloc(layout(size, 8)) };
                    assert!(!p.is_null());
                    // SAFETY: live, at least 8 bytes.
                    unsafe { p.cast::<usize>().write(size) };
                    if i % 3 == 0 {
                        sender.send(Sent(p, size)).unwrap();
                    } else {
                        // SAFETY: allocated with this layout.
                        unsafe { heap.dealloc(p, layout(size, 8)) };
                    }
                    let got = receiver.lock().unwrap().try_recv();
                    if let Ok(Sent(p, size)) = got {
                        // SAFETY: the sender gave the block up; it is live and holds its size.
                        unsafe {
                            assert_eq!(p.cast::<usize>().read(), size);
                            heap.dealloc(p, layout(size, 8));
                        }
                    }
                }
            });
        }
    });
    drop(sender);
    for Sent(p, size) in receiver.into_inner().unwrap().try_iter() {
        // SAFETY: as above.
        unsafe { heap.dealloc(p, layout(size, 8)) };
    }
}

#[test]
fn a_simulated_heap_serves_every_class() {
    let heap = SimHeap::<Shape>::new();
    every_class(&heap, if cfg!(miri) { 1 } else { 3 });
}

#[test]
fn a_simulated_heap_exchanges_magazines() {
    let heap = SimHeap::<Shape>::new();
    let rounds = if cfg!(miri) { 3 } else { 40 };
    for size in [8, 48, 512, MAX_SMALL_SIZE] {
        bursts(&heap, size, 600, rounds);
    }
}

#[test]
#[cfg_attr(miri, ignore)]
fn a_simulated_heap_resizes_in_place_and_with_headroom() {
    let heap = SimHeap::<Shape>::new();
    realloc_walk(&heap, 8 << 20);
}

#[test]
fn a_simulated_heap_zeroes() {
    let heap = SimHeap::<Shape>::new();
    zeroed(&heap, &[8, 100, 4096, 40 << 10, 300 << 10]);
}

#[test]
fn a_simulated_heap_serves_threads() {
    let heap = SimHeap::<Shape>::new();
    threads(&heap, if cfg!(miri) { 40 } else { 4000 });
}

#[test]
fn a_thread_without_a_lease_is_served_by_the_back_end() {
    let heap = SimHeap::<Shape>::new();
    // Bounded, so that a lease that is handed out twice fails here instead of growing forever.
    let leases: Vec<_> = core::iter::from_fn(|| heap.lease())
        .take(SIM_THREADS + 1)
        .collect();
    assert_eq!(leases.len(), SIM_THREADS, "every simulated thread is leased once");
    assert!(heap.lease().is_none());
    bursts(&heap, 64, 300, 2);
    every_class(&heap, 1);
    drop(leases);
    bursts(&heap, 64, 300, 2);
}

#[test]
fn a_failed_lease_releases_nothing() {
    let heap = SimHeap::<Shape>::new();
    let held: Vec<_> = (0..SIM_THREADS).filter_map(|_| heap.lease()).collect();
    assert_eq!(held.len(), SIM_THREADS);
    for _ in 0..3 {
        assert!(heap.lease().is_none(), "a lease is never handed out twice");
    }
    drop(held);
    assert!(heap.lease().is_some());
}

#[test]
fn a_heap_that_cannot_reserve_returns_null() {
    let os = SimOs::<Shape>::new();
    os.set_faults(Call::Reserve, Faults::ALWAYS);
    let heap = SimHeap::with_os(os);
    // SAFETY: valid, non-empty layouts.
    unsafe {
        assert!(heap.alloc(layout(16, 8)).is_null());
        assert!(heap.alloc(layout(1 << 20, 8)).is_null());
    }
    assert!(!heap.contains(ptr::null()));
    assert_eq!(OsError::ENOMEM.errno(), 12);
}

#[test]
fn large_frees_find_the_class_from_the_address() {
    let heap = SimHeap::<Shape>::new();
    // A layout far smaller than the block still frees the whole slot, and a block moved into
    // a roomier slot by `realloc` is freed with the caller's layout.
    let l = layout(40 << 10, 8);
    // SAFETY: valid layouts; each block is freed once with the layout it has.
    unsafe {
        let p = heap.alloc(l);
        let q = heap.realloc(p, l, 100 << 10);
        assert!(!q.is_null());
        let slot = heap.heap.back().unwrap().usable_size(q).unwrap();
        assert!(slot >= HEADROOM * (100 << 10), "{slot}");
        // What the caller may use is what it asked for, to a page, not the headroom.
        let page = heap.heap.back().unwrap().space().plan().geometry().page_size();
        assert_eq!(heap.usable(q), Some((100usize << 10).next_multiple_of(page)));
        heap.dealloc(q, layout(100 << 10, 8));
    }
    assert!(!Class::of_size(40 << 10).unwrap().is_small());
}

#[cfg(all(target_os = "linux", target_arch = "x86_64", not(miri)))]
mod process {
    use super::*;
    use crate::{Asmalloc, SimAsmalloc};

    impl Inspect for Asmalloc {
        fn contains(&self, ptr: *const u8) -> bool {
            Asmalloc::contains(ptr)
        }

        fn usable(&self, ptr: *const u8) -> Option<usize> {
            Asmalloc::usable_size(ptr)
        }
    }

    #[test]
    fn the_process_heap_serves_every_class() {
        every_class(&Asmalloc, 3);
    }

    #[test]
    fn the_process_heap_exchanges_magazines() {
        for size in [8, 48, 512, MAX_SMALL_SIZE] {
            bursts(&Asmalloc, size, 600, 40);
        }
    }

    #[test]
    fn the_process_heap_resizes_in_place_and_with_headroom() {
        realloc_walk(&Asmalloc, 32 << 20);
    }

    #[test]
    fn the_process_heap_zeroes() {
        zeroed(&Asmalloc, &[8, 100, 4096, 40 << 10, 3 << 20]);
    }

    #[test]
    fn the_process_heap_serves_threads() {
        threads(&Asmalloc, 20_000);
    }

    #[test]
    fn objects_larger_than_a_region_take_runs() {
        let l = layout(5 << 30, 8);
        // SAFETY: a valid layout; the block is freed once.
        unsafe {
            let p = Asmalloc.alloc_zeroed(l);
            assert!(!p.is_null());
            // Two regions, of which the caller asked for five gibibytes.
            assert_eq!(Asmalloc::usable_size(p), Some(5 << 30));
            assert_eq!(p.add((5 << 30) - 1).read(), 0);
            Asmalloc.dealloc(p, l);
        }
    }

    #[test]
    fn the_out_of_line_routines_reach_the_slow_paths() {
        let class = Class::of_size(96).unwrap().small().unwrap();
        let mut held = Vec::new();
        for round in 0..20 {
            for _ in 0..700 {
                let p = Asmalloc::alloc_small(class);
                assert!(!p.is_null());
                assert_eq!(Asmalloc::usable_size(p), Some(96));
                held.push(p);
            }
            for p in held.drain(..) {
                // SAFETY: each block came from `alloc_small` of this class and is freed once.
                unsafe { Asmalloc::free_small(class, core::ptr::NonNull::new(p).unwrap()) };
            }
            let _ = round;
        }
        // A block from the inlined routines frees through the out-of-line ones, and back.
        let l = layout(96, 8);
        // SAFETY: as above.
        unsafe {
            let p = Asmalloc.alloc(l);
            Asmalloc::free_small(class, core::ptr::NonNull::new(p).unwrap());
            let q = Asmalloc::alloc_small(class);
            Asmalloc.dealloc(q, l);
        }
    }

    #[test]
    fn pointers_outside_the_heap_are_not_in_it() {
        let local = 0u8;
        assert!(!Asmalloc::contains(&local));
        assert_eq!(Asmalloc::usable_size(&local), None);
        let p = std::boxed::Box::into_raw(std::boxed::Box::new(1u64)).cast::<u8>();
        assert!(!Asmalloc::contains(p));
        // SAFETY: made by `Box::into_raw` just above.
        drop(unsafe { std::boxed::Box::from_raw(p.cast::<u64>()) });
    }

    #[test]
    fn the_shared_simulated_heap_is_one_heap() {
        let l = layout(32, 8);
        // SAFETY: a valid layout; freed once.
        unsafe {
            let p = SimAsmalloc.alloc(l);
            assert!(SimAsmalloc::heap().contains(p));
            SimAsmalloc.dealloc(p, l);
        }
        assert!(ptr::eq(SimAsmalloc::heap(), SimAsmalloc::heap()));
    }
}
