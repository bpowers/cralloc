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
        .chain([
            (1, 1),
            (24, 8),
            (100, 64),
            (3000, 1024),
            (MAX_SMALL_SIZE + 1, 8),
        ])
        .collect();
    // Under Miri every byte costs; one large size there.
    if cfg!(miri) {
        sizes.retain(|&(size, _)| size <= 4096 || size == MAX_SMALL_SIZE + 1);
    } else {
        sizes.extend([(40 << 10, 16), (60 << 10, 4096), (200 << 10, 8)]);
    }
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
        assert!(
            ranges.windows(2).all(|w| w[0].1 <= w[1].0),
            "blocks overlap"
        );
        for i in (0..live.len())
            .rev()
            .step_by(2)
            .chain((0..live.len()).step_by(2))
        {
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
/// contents survive and that large growth moves rarely while headroom fits in a `region`.
fn realloc_walk<H: Inspect>(heap: &H, top: usize, region: usize) {
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
        if next > MAX_SMALL_SIZE && size > MAX_SMALL_SIZE && next * HEADROOM <= region {
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
    // Shrinking keeps the contents, in place or not.
    while size > 1 {
        let next = size / 2;
        // SAFETY: as above.
        let q = unsafe { heap.realloc(p, layout(size, align), next) };
        assert!(!q.is_null());
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
    if cfg!(miri) {
        // Past one magazine of 254 objects, so every exchange still runs.
        bursts(&heap, 48, 300, 2);
        return;
    }
    for size in [8, 48, 512, MAX_SMALL_SIZE] {
        bursts(&heap, size, 600, 40);
    }
}

#[test]
#[cfg_attr(miri, ignore)]
fn a_simulated_heap_resizes_in_place_and_with_headroom() {
    let heap = SimHeap::<Shape>::new();
    realloc_walk(&heap, 8 << 20, 1 << 20);
}

#[test]
fn a_simulated_heap_zeroes() {
    let heap = SimHeap::<Shape>::new();
    let sizes: &[usize] = if cfg!(miri) {
        &[8, 100, 40 << 10]
    } else {
        &[8, 100, 4096, 40 << 10, 300 << 10]
    };
    zeroed(&heap, sizes);
}

#[test]
fn a_simulated_heap_serves_threads() {
    let heap = SimHeap::<Shape>::new();
    threads(&heap, if cfg!(miri) { 40 } else { 4000 });
}

#[test]
fn threads_that_start_a_fresh_heap_together_agree_on_it() {
    let rounds = if cfg!(miri) { 1 } else { 200 };
    // The simulated OS has room for one heap, so every thread but the winner sees its
    // reservation fail while the winner's is in flight; none of them may settle for less.
    let largest = <crate::os::sim::Standard as crate::os::sim::Shape>::GEOMETRY.largest_plan();
    for _ in 0..rounds {
        let heap = SimHeap::<crate::os::sim::Standard>::new();
        threads(&heap, if cfg!(miri) { 20 } else { 300 });
        assert_eq!(heap.heap.back().unwrap().space().plan(), largest);
    }
}

#[test]
fn the_first_allocations_of_a_fresh_heap_from_several_threads_succeed() {
    let rounds = if cfg!(miri) { 1 } else { 300 };
    for _ in 0..rounds {
        let heap = SimHeap::<crate::os::sim::Standard>::new();
        thread::scope(|scope| {
            for t in 0..4 {
                let heap = &heap;
                scope.spawn(move || {
                    let l = layout(1 + t, 2);
                    // SAFETY: a valid layout; freed once.
                    unsafe {
                        let p = heap.alloc_zeroed(l);
                        assert!(!p.is_null(), "thread {t}'s first allocation failed");
                        heap.dealloc(p, l);
                    }
                });
            }
        });
    }
}

/// Every live block holds its owner's tag, checked before it is freed: two owners of one block
/// show as a wrong tag.
///
/// The first `quiet` threads stay on the fast paths: one size, and a live set that hovers within
/// one magazine. Their slabs take no slow path, so the idle check, which the other threads' slow
/// paths run, finds them unchanged and drains them while they are in use.
fn tagged_churn<H: GlobalAlloc + Sync>(heap: &H, threads: usize, quiet: usize, rounds: usize) {
    let pool = std::sync::Mutex::new(Vec::<Sent>::new());
    thread::scope(|scope| {
        for t in 0..threads {
            let pool = &pool;
            scope.spawn(move || {
                let quiet = t < quiet;
                let most = if quiet { 100 } else { 400 };
                let mut live: Vec<(*mut u8, Layout, usize)> = Vec::new();
                let mut x = 0x9e37_79b9_7f4a_7c15u64 ^ t as u64;
                for i in 0..rounds {
                    x ^= x << 13;
                    x ^= x >> 7;
                    x ^= x << 17;
                    // One block in 32 is big. At one in 16, about one run in 350 on a busy
                    // machine filled all 244 regions of the simulated heap, a third of them
                    // with blocks over 40 KiB at four to six a region, and an allocation failed.
                    let size = if quiet {
                        64
                    } else if (x >> 30).is_multiple_of(32)
                        && std::env::var_os("SMALL_ONLY").is_none()
                    {
                        (x as usize >> 8) % (64 << 10) + 8
                    } else {
                        8 + (x as usize % 12) * 8
                    };
                    let l = layout(size, 8);
                    if live.len() < most && !(x >> 40).is_multiple_of(3) {
                        // SAFETY: a valid layout.
                        let p = unsafe { heap.alloc(l) };
                        assert!(!p.is_null());
                        let tag = t << 32 | i;
                        // SAFETY: live, at least 8 bytes.
                        unsafe { p.cast::<usize>().write(tag) };
                        live.push((p, l, tag));
                    } else if !quiet && (x >> 50).is_multiple_of(4) && !live.is_empty() {
                        // Hand a block to whichever thread takes it next; its tag goes along
                        // as the block's size field.
                        let (p, l, tag) = live.swap_remove((x >> 20) as usize % live.len());
                        // SAFETY: still ours; the tag is in its first word.
                        assert_eq!(unsafe { p.cast::<usize>().read() }, tag);
                        pool.lock().unwrap().push(Sent(p, l.size()));
                    } else if !quiet
                        && (x >> 50) % 4 == 1
                        && let Some(Sent(p, size)) = pool.lock().unwrap().pop()
                    {
                        // SAFETY: given up by its sender; its tag is in its first word.
                        let tag = unsafe { p.cast::<usize>().read() };
                        live.push((p, layout(size, 8), tag));
                    } else if !live.is_empty() {
                        let (p, l, tag) = live.swap_remove((x >> 20) as usize % live.len());
                        // SAFETY: still ours; freed once with its layout.
                        unsafe {
                            assert_eq!(p.cast::<usize>().read(), tag, "a block had two owners");
                            heap.dealloc(p, l);
                        }
                    }
                }
                for (p, l, tag) in live {
                    // SAFETY: as above.
                    unsafe {
                        assert_eq!(p.cast::<usize>().read(), tag, "a block had two owners");
                        heap.dealloc(p, l);
                    }
                }
            });
        }
    });
    for Sent(p, size) in pool.into_inner().unwrap() {
        // SAFETY: given up by its sender, never taken.
        unsafe { heap.dealloc(p, layout(size, 8)) };
    }
}

/// Runs `body` while another thread drains the heap's slabs, in two ways.
///
/// It moves the clock, which stands still unless a test moves it: a slab is drained by the idle
/// check only when its activity counter has stood still for an interval of the clock. That
/// exercises the heartbeat's own path, but rarely: a slab is looked at once in many slow
/// paths, and one that takes slow paths itself is never found quiet (about a dozen drains in
/// five million operations, none in a short round).
///
/// So it also asks for a drain of each slab in turn, whatever the slab is doing, a few
/// thousand times a second. A drain is safe at any moment, and these are the ones that race
/// the routines of the threads using the slab.
fn with_drains_running<S: crate::os::sim::Shape, T>(
    heap: &SimHeap<S>,
    body: impl FnOnce() -> T,
) -> T {
    use core::sync::atomic::{AtomicBool, Ordering};
    /// Stops the drainer when the body ends, by a panic too, or the scope would never end.
    struct Stop<'a>(&'a AtomicBool);
    impl Drop for Stop<'_> {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }
    let stop = AtomicBool::new(false);
    thread::scope(|scope| {
        scope.spawn(|| {
            let mut cpu = 0;
            while !stop.load(Ordering::Acquire) {
                heap.os().advance_clock(crate::depot::IDLE_TICKS / 4);
                heap.drain(cpu);
                cpu = (cpu + 1) % SIM_THREADS;
                // Miri runs one thread at a time and a drain is some hundred atomic
                // instructions, so there the drainer mostly yields.
                if cfg!(miri) {
                    for _ in 0..2000 {
                        thread::yield_now();
                    }
                } else {
                    for _ in 0..200 {
                        core::hint::spin_loop();
                    }
                }
            }
        });
        let _stop = Stop(&stop);
        body()
    })
}

/// This caught the simulator's fence returning while a twin stood between its `Access` check and
/// its store (`os::sim::cpus`): the store landed in a magazine a drain had passed on, and a block
/// got two owners. It showed most with more threads than cores. The switches (`NO_DRAIN`, `NO_PURGE`, `NO_CPU`) turn parts of the heap off
/// to bisect a failure.
#[test]
fn blocks_have_one_owner_while_threads_churn_and_drains_run() {
    let rounds = if cfg!(miri) { 1 } else { 30 };
    for _ in 0..rounds {
        let heap = SimHeap::<crate::os::sim::Standard>::new();
        if std::env::var_os("NO_DRAIN").is_some() {
            heap.os().set_faults(Call::RegisterFence, Faults::ALWAYS);
        }
        if std::env::var_os("NO_PURGE").is_some() {
            heap.os().set_faults(Call::Purge, Faults::ALWAYS);
        }
        let held: Vec<_> = if std::env::var_os("NO_CPU").is_some() {
            (0..SIM_THREADS).filter_map(|_| heap.lease()).collect()
        } else {
            Vec::new()
        };
        with_drains_running(&heap, || {
            tagged_churn(
                &heap,
                if cfg!(miri) { 3 } else { 10 },
                if cfg!(miri) { 1 } else { 5 },
                if cfg!(miri) { 200 } else { 50_000 },
            )
        });
        drop(held);
    }
}

#[test]
fn back_end_slots_have_one_owner_while_threads_churn() {
    let heap = SimHeap::<crate::os::sim::Standard>::new();
    let l = layout(8, 8);
    // SAFETY: a valid layout; starts the heap.
    unsafe { heap.dealloc(heap.alloc(l), l) };
    let classes: Vec<Class> = (1..=12).map(|w| Class::of_size(8 * w).unwrap()).collect();
    let pool = std::sync::Mutex::new(Vec::<Sent>::new());
    thread::scope(|scope| {
        for t in 0..10usize {
            let (pool, classes, heap) = (&pool, &classes, &heap);
            scope.spawn(move || {
                let back = heap.heap.back().unwrap();
                let mut live: Vec<(*mut u8, Class, usize)> = Vec::new();
                let mut x = 0x9e37_79b9_7f4a_7c15u64 ^ t as u64;
                for i in 0..400_000usize {
                    x ^= x << 13;
                    x ^= x >> 7;
                    x ^= x << 17;
                    let class = classes[x as usize % classes.len()];
                    let pick = (x >> 50) % 4;
                    if live.len() < 400 && !(x >> 40).is_multiple_of(3) {
                        let p = back.alloc(class, class.size()).unwrap().ptr.as_ptr();
                        let tag = t << 32 | i;
                        // SAFETY: a slot this thread now holds.
                        unsafe { p.cast::<usize>().write(tag) };
                        live.push((p, class, tag));
                    } else if pick == 0 && !live.is_empty() {
                        let (p, class, _) = live.swap_remove((x >> 20) as usize % live.len());
                        pool.lock().unwrap().push(Sent(p, class.size()));
                    } else if pick == 1
                        && let Some(Sent(p, size)) = pool.lock().unwrap().pop()
                    {
                        // SAFETY: given up by its sender.
                        let tag = unsafe { p.cast::<usize>().read() };
                        live.push((p, Class::of_size(size).unwrap(), tag));
                    } else if !live.is_empty() {
                        let (p, class, tag) = live.swap_remove((x >> 20) as usize % live.len());
                        // SAFETY: still ours.
                        assert_eq!(unsafe { p.cast::<usize>().read() }, tag, "two owners");
                        back.free(core::ptr::NonNull::new(p).unwrap(), class);
                        if std::env::var_os("HEARTBEAT").is_some() {
                            back.heartbeat();
                        }
                    }
                }
            });
        }
    });
}

#[test]
fn a_thread_without_a_lease_is_served_by_the_back_end() {
    let heap = SimHeap::<Shape>::new();
    // Bounded, so that a lease that is handed out twice fails here instead of growing forever.
    let leases: Vec<_> = core::iter::from_fn(|| heap.lease())
        .take(SIM_THREADS + 1)
        .collect();
    assert_eq!(
        leases.len(),
        SIM_THREADS,
        "every simulated thread is leased once"
    );
    assert!(heap.lease().is_none());
    let burst = if cfg!(miri) { 40 } else { 300 };
    bursts(&heap, 64, burst, 2);
    every_class(&heap, 1);
    drop(leases);
    bursts(&heap, 64, burst, 2);
}

#[test]
fn a_large_shrink_stays_in_place_unless_it_leaves_under_an_eighth_of_the_block() {
    let heap = SimHeap::<Shape>::new();
    let l = layout(200 << 10, 8);
    // SAFETY: valid layouts; each block is freed once with the layout it has.
    unsafe {
        let p = heap.alloc(l);
        let q = heap.realloc(p, l, 120 << 10);
        assert_eq!(q, p, "a shrink to more than an eighth of the slot stays");
        let r = heap.realloc(q, layout(120 << 10, 8), 40 << 10);
        assert_eq!(r, q, "a shrink to more than an eighth of the slot stays");
        heap.dealloc(r, layout(40 << 10, 8));
        // Regions of the native shape hold a megabyte slot, whose eighth is a large size.
        if !cfg!(miri) {
            let l = layout(1 << 20, 8);
            let p = heap.alloc(l);
            let q = heap.realloc(p, l, 100 << 10);
            assert_ne!(q, p, "a shrink to less than an eighth of the slot moves");
            heap.dealloc(q, layout(100 << 10, 8));
        }
    }
}

/// A zeroed grow into a slot an earlier block left dirty: the kept bytes arrive, and the rest
/// reads as zero, though the move skipped the old block's zero pages and allocated unzeroed.
#[test]
fn a_zeroed_grow_into_a_dirty_slot_reads_as_zero_past_what_it_kept() {
    let heap = SimHeap::<Shape>::new();
    // Under the large cap of the simulated shape, so the freed slot stays resident and dirty.
    let (small, big) = (layout(80 << 10, 8), layout(200 << 10, 8));
    // SAFETY: valid layouts; each block is freed once with the layout it has.
    unsafe {
        let dirty = heap.alloc(big);
        dirty.write_bytes(0xaa, big.size());
        heap.dealloc(dirty, big);
        let p = heap.alloc(small);
        // Half of it written, the other half left as zero pages the move will skip.
        fill(p, small.size() / 2, 7);
        let (q, len) = heap.grow_zeroed_for_test(p, small, big).unwrap();
        assert_eq!(q, dirty, "the grow reuses the dirty slot");
        assert!(holds(q, small.size() / 2, 7));
        assert!((small.size() / 2..len).all(|i| q.add(i).read() == 0));
        heap.dealloc(q, big);
    }
}

#[test]
fn growth_in_small_steps_stays_in_the_room_a_move_gave() {
    let heap = SimHeap::<Shape>::new();
    let page = 4 << 10;
    let mut size = 40 << 10;
    // SAFETY: valid layouts; the block is freed once with the layout it has.
    unsafe {
        let mut p = heap.alloc(layout(size, 8));
        // Outgrow the slot once, into a block with headroom (which fits a region of every shape).
        p = heap.realloc(p, layout(size, 8), 50 << 10);
        size = 50 << 10;
        for _ in 0..16 {
            let q = heap.realloc(p, layout(size, 8), size + page);
            assert_eq!(q, p, "growth within the headroom stays put at {size}");
            size += page;
        }
        // An over-aligned layout grows in place too.
        let small = layout(8, 64 << 10);
        let a = heap.alloc(small);
        assert_eq!(heap.realloc(a, small, 16), a);
        heap.dealloc(a, layout(16, 64 << 10));
        heap.dealloc(p, layout(size, 8));
    }
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
        let q = heap.realloc(p, l, 50 << 10);
        assert!(!q.is_null());
        let slot = match heap.heap.back().unwrap().lookup(q).unwrap() {
            crate::back::Kind::Slot(class) => class.size(),
            crate::back::Kind::Run(regions) => {
                regions << heap.heap.back().unwrap().space().plan().region_shift()
            }
        };
        assert!(slot >= HEADROOM * (50 << 10), "{slot}");
        // What the caller may use is what it asked for, to a page, not the headroom.
        let page = heap
            .heap
            .back()
            .unwrap()
            .space()
            .plan()
            .geometry()
            .page_size();
        assert_eq!(heap.usable(q), Some((50usize << 10).next_multiple_of(page)));
        heap.dealloc(q, layout(50 << 10, 8));
    }
    assert!(!Class::of_size(40 << 10).unwrap().is_small());
}

/// Where a heap's parked objects are, by `inspect`.
#[derive(Default, Debug)]
struct Parked {
    /// In loaded magazines and spares.
    on_cpus: usize,
    /// In the depot.
    in_depot: usize,
}

impl crate::stats::Visitor for Parked {
    fn cached(&mut self, place: crate::stats::Place, _: SmallClass, _: *const u8) {
        match place {
            crate::stats::Place::Depot => self.in_depot += 1,
            _ => self.on_cpus += 1,
        }
    }
}

fn parked<S: crate::os::sim::Shape>(heap: &SimHeap<S>) -> Parked {
    let mut seen = Parked::default();
    heap.inspect(&mut seen);
    seen
}

/// Allocates and frees `count` small objects of mixed sizes on each of `threads` threads, which
/// leaves objects parked on several CPUs and in the depot.
fn park<H: GlobalAlloc + Sync>(heap: &H, threads: usize, count: usize) {
    thread::scope(|scope| {
        for t in 0..threads {
            scope.spawn(move || {
                let blocks: Vec<_> = (0..count)
                    .map(|i| {
                        let l = layout(8 + (i * 7 + t) % 600, 8);
                        // SAFETY: a valid layout.
                        let p = unsafe { heap.alloc(l) };
                        assert!(!p.is_null());
                        (p, l)
                    })
                    .collect();
                for (p, l) in blocks {
                    // SAFETY: allocated above with this layout, freed once.
                    unsafe { heap.dealloc(p, l) };
                }
            });
        }
    });
}

#[test]
fn collect_leaves_nothing_parked_and_returns_the_free_pages() {
    let heap = SimHeap::<Shape>::new();
    park(&heap, 4, if cfg!(miri) { 300 } else { 3000 });
    let before = parked(&heap);
    assert!(before.on_cpus > 0 && before.in_depot > 0, "{before:?}");
    let purged = heap.collect();
    let after = parked(&heap);
    assert_eq!((after.on_cpus, after.in_depot), (0, 0), "{after:?}");
    assert!(purged > 0, "nothing was purged");
    // The heap goes on serving.
    park(&heap, 2, 100);
}

#[test]
fn large_allocations_and_frees_run_the_heartbeat() {
    let heap = SimHeap::<Shape>::new();
    let small = layout(64, 8);
    // SAFETY: valid layouts; each block is freed once.
    unsafe {
        let blocks: Vec<_> = (0..100).map(|_| heap.alloc(small)).collect();
        for p in blocks {
            heap.dealloc(p, small);
        }
    }
    assert!(parked(&heap).on_cpus > 0);
    // Only large blocks from here: their heartbeats stamp the loaded magazines, then take
    // them once they have been loaded for an interval, and the spares of classes left with
    // none loaded, a class at a time.
    let large = layout(64 << 10, 8);
    for _ in 0..3 {
        for _ in 0..400 {
            // SAFETY: as above.
            unsafe { heap.dealloc(heap.alloc(large), large) };
        }
        heap.os().advance_clock(crate::depot::IDLE_TICKS);
    }
    assert_eq!(parked(&heap).on_cpus, 0, "{:?}", parked(&heap));
}

/// As `blocks_have_one_owner_while_threads_churn_and_drains_run`, with the clock moving an
/// interval every few milliseconds, so that the age limit's takes, the depot's sheds and the
/// idle check's drains all run among the churn.
#[test]
fn blocks_have_one_owner_while_threads_churn_and_the_clock_runs() {
    let rounds = if cfg!(miri) { 1 } else { 4 };
    for _ in 0..rounds {
        let heap = SimHeap::<crate::os::sim::Standard>::new();
        let done = std::sync::atomic::AtomicBool::new(false);
        thread::scope(|scope| {
            scope.spawn(|| {
                while !done.load(std::sync::atomic::Ordering::Relaxed) {
                    heap.os().advance_clock(crate::depot::IDLE_TICKS / 4);
                    thread::sleep(std::time::Duration::from_millis(1));
                }
            });
            tagged_churn(
                &heap,
                if cfg!(miri) { 3 } else { 10 },
                0,
                if cfg!(miri) { 200 } else { 50_000 },
            );
            done.store(true, std::sync::atomic::Ordering::Relaxed);
        });
    }
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
        realloc_walk(&Asmalloc, 32 << 20, 1 << 32);
    }

    #[test]
    fn the_process_heap_zeroes() {
        zeroed(&Asmalloc, &[8, 100, 4096, 40 << 10, 3 << 20]);
    }

    #[test]
    fn the_process_heap_serves_threads() {
        threads(&Asmalloc, 20_000);
    }

    /// Collect drains the process heap's slabs with real fences while other tests use it; the
    /// heap must go on serving everyone.
    #[test]
    fn the_process_heap_collects_and_goes_on() {
        bursts(&Asmalloc, 64, 600, 4);
        let _ = Asmalloc::collect();
        every_class(&Asmalloc, 1);
        let _ = Asmalloc::collect();
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

    /// The design: what a large block may use is the size asked for, to a page, never the
    /// room it sits in, so that a caller that fills what it is told it may use touches about
    /// what it asked for. A layout aligned beyond a region takes a run, which records the size
    /// asked for, not the size rounded up to the alignment (8 GiB for these eight bytes).
    #[test]
    fn an_over_aligned_run_reports_what_was_asked_for() {
        let l = layout(8, 1 << 33);
        // SAFETY: a valid layout; the block is freed once.
        unsafe {
            let p = Asmalloc.alloc(l);
            assert!(!p.is_null());
            assert_eq!(p.addr() % (1 << 33), 0);
            let usable = Asmalloc::usable_size(p);
            Asmalloc.dealloc(p, l);
            assert_eq!(usable, Some(4096));
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
