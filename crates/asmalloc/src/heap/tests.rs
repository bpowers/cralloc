//! The heap through `GlobalAlloc`: the process's heap with the generated routines where they
//! run, and simulated heaps with twins everywhere.

use super::large::{HEADROOM, growth_rooms, shrink_moves_below_for};
use crate::SimHeap;
use crate::classes::{Class, MAX_SMALL_SIZE, SmallClass};
use crate::depot::siblings::{SIBLINGS, stride};
use crate::os::OsError;
use crate::os::sim::{Call, Faults, SIM_THREADS, SimOs};
use crate::space::Plan;

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
    /// The heap's plan, once it has started.
    fn plan(&self) -> Option<Plan>;
}

impl<S: crate::os::sim::Shape> Inspect for SimHeap<S> {
    fn contains(&self, ptr: *const u8) -> bool {
        SimHeap::contains(self, ptr)
    }

    fn usable(&self, ptr: *const u8) -> Option<usize> {
        self.usable_size(ptr)
    }

    fn plan(&self) -> Option<Plan> {
        self.heap.back().map(|back| back.space().plan())
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
/// contents survive and that large growth moves as rarely as its headroom allows: a block that
/// moved with `f` times its room has `log2 f` doublings in place before it moves again. With
/// `full`, the heap must give some sizes sixteen times, and those must move fewer times than
/// four times would make them.
fn realloc_walk<H: Inspect>(heap: &H, top: usize, full: bool) {
    let align = 8;
    let mut size = 1;
    // SAFETY: a valid, non-empty layout.
    let mut p = unsafe { heap.alloc(layout(size, align)) };
    assert!(!p.is_null());
    // SAFETY: the block is live and `size` long.
    unsafe { fill(p, size, 7) };
    let plan = heap.plan().expect("the heap has started");
    // Doublings and moves by the factor of headroom the doubling's size gets; none is 1.
    let mut by_factor = std::collections::BTreeMap::<usize, (usize, usize)>::new();
    while size < top {
        let next = size * 2;
        // SAFETY: `p` is live with this layout; `next` is valid.
        let q = unsafe { heap.realloc(p, layout(size, align), next) };
        assert!(!q.is_null());
        // SAFETY: the first `size` bytes were carried over.
        assert!(unsafe { holds(q, size, 7) }, "at {size}");
        if next > MAX_SMALL_SIZE && size > MAX_SMALL_SIZE {
            let factor = growth_rooms(next, plan)
                .next()
                .map_or(1, |room| room / next);
            let (steps, moves) = by_factor.entry(factor).or_default();
            *steps += 1;
            *moves += usize::from(q != p);
        }
        // SAFETY: the block is live and `next` long.
        unsafe { fill(q, next, 7) };
        p = q;
        size = next;
    }
    // At most one move in each run of `log2 f + 1` doublings, the walk entering each factor's
    // sizes with a move.
    let most = |factor: usize, steps: usize| steps.div_ceil(factor.ilog2() as usize + 1);
    for (&factor, &(steps, moves)) in &by_factor {
        if factor > 1 {
            assert!(
                moves <= most(factor, steps),
                "{moves} moves in {steps} doublings with {factor} times the room: {by_factor:?}"
            );
        }
    }
    if full {
        let (steps, _) = by_factor[&HEADROOM];
        assert!(
            most(HEADROOM, steps) < most(4, steps),
            "{steps} doublings do not tell sixteen times from four: {by_factor:?}"
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
        // Past one magazine of 1,022 objects, so every exchange still runs.
        bursts(&heap, 48, 1100, 2);
        return;
    }
    // Past the largest magazine, so that every class exchanges.
    let burst = 2 * crate::classes::MagazineSize::MAX.capacity() + 100;
    for size in [8, 48, 512, MAX_SMALL_SIZE] {
        bursts(&heap, size, burst, 40);
    }
}

/// A fresh heap's first objects of a class smaller than a line, siblings made in a loop, are
/// handed out a stride apart, so that none shares a cache line with the one before; the rest of
/// the class's first magazine, and its next, filled in the same interval, are dense.
#[test]
fn a_fresh_heaps_first_objects_of_a_small_class_lie_on_lines_of_their_own() {
    let heap = SimHeap::<Shape>::new();
    let sizes: Vec<usize> = if cfg!(miri) {
        [40].to_vec()
    } else {
        (8..64).step_by(8).collect()
    };
    for size in sizes {
        let l = layout(size, 8);
        let capacity = Class::of_size(size)
            .and_then(Class::small)
            .unwrap()
            .magazine()
            .capacity();
        // SAFETY: a valid, non-empty layout.
        let got: Vec<*mut u8> = (0..2 * capacity)
            .map(|_| unsafe { heap.alloc(l) })
            .collect();
        assert!(got.iter().all(|p| !p.is_null()));
        let siblings = SIBLINGS * stride(size).unwrap();
        for pair in got[..=siblings].windows(2) {
            let (low, high) = (pair[0].min(pair[1]).addr(), pair[0].max(pair[1]).addr());
            assert_ne!((low + size - 1) / 64, high / 64, "{size}: {pair:?}");
        }
        for pair in got[siblings..].windows(2) {
            assert_eq!(pair[1].addr() - pair[0].addr(), size, "{size}: {pair:?}");
        }
        for p in got {
            // SAFETY: allocated above with `l`, freed once.
            unsafe { heap.dealloc(p, l) };
        }
    }
}

/// A class opens one run at start-up whatever the cycle counter reads. On a machine up for
/// more than an interval the clock's first reading steps its count from zero; if that came at
/// the first heartbeat, every frontier the first fills recorded would read as an earlier
/// interval's and the class would open a second run at once. Only the first magazine is
/// strided, here as when the clock starts at zero.
#[test]
fn a_class_opens_one_run_at_start_whatever_the_clock_reads() {
    for start in [0u64, 5 * crate::depot::IDLE_TICKS] {
        let heap = SimHeap::<Shape>::new();
        heap.os().advance_clock(start);
        let size = 40;
        let l = layout(size, 8);
        let capacity = Class::of_size(size)
            .and_then(Class::small)
            .unwrap()
            .magazine()
            .capacity();
        // SAFETY: a valid, non-empty layout.
        let got: Vec<*mut u8> = (0..6 * capacity)
            .map(|_| unsafe { heap.alloc(l) })
            .collect();
        let strided: Vec<usize> = got
            .chunks(capacity)
            .enumerate()
            .filter(|(_, m)| m[1].addr().abs_diff(m[0].addr()) != size)
            .map(|(i, _)| i)
            .collect();
        assert_eq!(strided, [0], "clock starts at {start:#x}");
    }
}

#[test]
#[cfg_attr(miri, ignore)]
fn a_simulated_heap_resizes_in_place_and_with_headroom() {
    let heap = SimHeap::<Shape>::new();
    realloc_walk(&heap, 8 << 20, false);
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

fn threads_that_start_a_fresh_heap_together_agree_on_it(rounds: usize) {
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
fn threads_that_start_a_fresh_heap_together_agree_on_it_quickly() {
    threads_that_start_a_fresh_heap_together_agree_on_it(if cfg!(miri) { 1 } else { 20 });
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn threads_that_start_a_fresh_heap_together_agree_on_it_fully() {
    threads_that_start_a_fresh_heap_together_agree_on_it(200);
}

fn the_first_allocations_of_a_fresh_heap_from_several_threads_succeed(rounds: usize) {
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

#[test]
fn the_first_allocations_of_a_fresh_heap_from_several_threads_succeed_quickly() {
    the_first_allocations_of_a_fresh_heap_from_several_threads_succeed(if cfg!(miri) {
        1
    } else {
        30
    });
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn the_first_allocations_of_a_fresh_heap_from_several_threads_succeed_fully() {
    the_first_allocations_of_a_fresh_heap_from_several_threads_succeed(300);
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
fn blocks_have_one_owner_while_threads_churn_and_drains_run(rounds: usize) {
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
        // No magazine is lost: every one the area made is loaded, a spare, or on a depot
        // stack. The area may run out here, and calls are then served from the back end, which
        // is correct: on a machine with more threads than cores the drainer, which drains a CPU
        // every few hundred spins, parks magazines on the depot faster than the starved
        // churners' heartbeats shed them, and in 480 oversubscribed rounds 5 filled the area
        // (561 to 595 magazines on depot stacks against 201 to 358 in the others), none with a
        // magazine missing. Production drains a slab at most once an interval.
        let (created, reached) = magazine_audit(&heap);
        assert_eq!(created, reached, "a magazine was lost");
    }
}

/// Magazines the heap's area made, and magazines the inspection walk reaches: loaded, spare,
/// on a depot stack, or on an empty stack. Equal when none is lost, once nothing runs.
fn magazine_audit<S: crate::os::sim::Shape>(heap: &SimHeap<S>) -> (usize, usize) {
    #[derive(Default)]
    struct Count(usize);
    impl crate::stats::Visitor for Count {
        fn magazine(&mut self, _: crate::stats::Place, _: SmallClass, _: usize) {
            self.0 += 1;
        }
        fn empties(&mut self, _: crate::classes::MagazineSize, count: usize) {
            self.0 += count;
        }
    }
    let mut count = Count::default();
    heap.inspect(&mut count);
    let created = heap
        .heap
        .depot
        .created
        .load(core::sync::atomic::Ordering::Relaxed);
    (created, count.0)
}

#[test]
fn blocks_have_one_owner_while_threads_churn_and_drains_run_quickly() {
    blocks_have_one_owner_while_threads_churn_and_drains_run(if cfg!(miri) { 1 } else { 3 });
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn blocks_have_one_owner_while_threads_churn_and_drains_run_fully() {
    blocks_have_one_owner_while_threads_churn_and_drains_run(30);
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

/// A large shrink stays in place unless it leaves a sliver of the block
/// (`stays_after_shrink`). The simulated regions are too small for a large size to be that little of a slot;
/// `the_process_heap_moves_a_large_shrink_to_a_sliver` takes the other side.
#[test]
fn a_large_shrink_stays_in_place_unless_it_leaves_a_sliver_of_the_block() {
    let heap = SimHeap::<Shape>::new();
    let l = layout(200 << 10, 8);
    // SAFETY: valid layouts; each block is freed once with the layout it has.
    unsafe {
        let p = heap.alloc(l);
        let q = heap.realloc(p, l, 120 << 10);
        assert_eq!(q, p, "a shrink to a large part of the slot stays");
        let r = heap.realloc(q, layout(120 << 10, 8), 40 << 10);
        assert_eq!(r, q, "a shrink to a large part of the slot stays");
        heap.dealloc(r, layout(40 << 10, 8));
    }
}

/// A block that outgrows its slot by a byte moves once, into a slot with headroom; cut back
/// to the size it had and grown again across the same boundary, over and over, it stays: the
/// shrink threshold is well below what a block fills of a slot it moved into.
#[test]
fn a_block_resized_back_and_forth_across_its_slot_boundary_settles() {
    let heap = SimHeap::<Shape>::new();
    let mut size = 40 << 10;
    // SAFETY: valid layouts; the block is live where it is resized and freed once with the
    // layout it has.
    unsafe {
        let mut p = heap.alloc(layout(size, 8));
        let edge = heap.heap.back().unwrap().room(p).unwrap();
        let mut moves = 0;
        for _ in 0..10 {
            for next in [edge + 1, edge] {
                let q = heap.realloc(p, layout(size, 8), next);
                assert!(!q.is_null());
                moves += usize::from(q != p);
                (p, size) = (q, next);
            }
        }
        assert_eq!(moves, 1, "the block moved {moves} times in 20 resizes");
        heap.dealloc(p, layout(size, 8));
    }
}

/// The room a block moves in with is never written, so it is never charged, kept or purged: a
/// grown block freed with huge-page advice adds to the retained count only the pages it may
/// have written, is kept on that charge within the budget for kept grown blocks, and a trim
/// takes back exactly what was counted.
#[test]
#[cfg_attr(miri, ignore)]
fn the_room_a_grown_block_never_wrote_is_never_charged_kept_or_purged() {
    use crate::back::policy::{STAGGER_ROOM, kept_grown};
    // The block's charge must fit the budget for the slot to be kept.
    const _: () = assert!(kept_grown(crate::os::sim::Native::GEOMETRY) >= 2 * (128 << 10));
    use crate::os::HugePages;
    use crate::os::sim::Shape as _;
    let heap = SimHeap::<crate::os::sim::Native>::new();
    let geometry = crate::os::sim::Native::GEOMETRY;
    let huge = geometry.huge_page_size();
    let (small, grown) = (layout(40 << 10, 8), layout(80 << 10, 8));
    // SAFETY: valid layouts; each block is written within its size and freed once with the
    // layout it has.
    unsafe {
        let p = heap.alloc(small);
        fill(p, small.size(), 1);
        // Doubling past a huge page, densely: a move into a slot with headroom, and advice.
        let q = heap.realloc(p, small, grown.size());
        fill(q, grown.size(), 2);
        let back = heap.heap.back().unwrap();
        let slot = match back.lookup(q) {
            Some(crate::back::Kind::Slot(class)) => class.size(),
            other => panic!("{other:?}"),
        };
        let most = (grown.size() + STAGGER_ROOM).next_multiple_of(huge);
        assert!(slot >= 2 * most, "a slot of {slot}");
        let advised = || heap.os().huge_pages(ptr::NonNull::new(q).unwrap());
        assert_eq!(advised(), Some(HugePages::Prefer));
        let before = back.retained_large();
        heap.dealloc(q, grown);
        let charge = back.retained_large() - before;
        assert!(
            (grown.size()..=most).contains(&charge),
            "charged {charge} for a block of {} in a slot of {slot}",
            grown.size()
        );
        assert_eq!(advised(), Some(HugePages::Prefer), "the slot was kept");
        let held = back.retained();
        assert_eq!(back.trim(), held, "the purge takes back what was counted");
        assert_eq!(back.retained(), 0);
    }
}

/// A zeroed grow into a slot an earlier block left dirty: the kept bytes arrive, and the rest
/// reads as zero, though the move skipped the old block's zero pages and allocated unzeroed.
#[test]
fn a_zeroed_grow_into_a_dirty_slot_reads_as_zero_past_what_it_kept() {
    let heap = SimHeap::<Shape>::new();
    // Not grown, so not advised for huge pages: the freed slot stays resident and dirty.
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
        let plan = heap.heap.back().unwrap().space().plan();
        let room = growth_rooms(50 << 10, plan).next().unwrap();
        assert!(slot >= room, "{slot}");
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

/// The twelve sizes of a quiet program's light work.
const LIGHT: [usize; 12] = [16, 24, 32, 40, 48, 64, 80, 96, 128, 160, 192, 256];

/// What a quiet program does: a few objects of each of the light sizes, allocated and freed,
/// which once the magazines are loaded never leaves them.
fn light_work<H: GlobalAlloc>(heap: &H) {
    for size in LIGHT {
        let l = layout(size, 8);
        // SAFETY: a valid layout; each block is freed once with it.
        unsafe {
            let held: Vec<_> = (0..4).map(|_| heap.alloc(l)).collect();
            for p in held {
                assert!(!p.is_null());
                heap.dealloc(p, l);
            }
        }
    }
}

/// Allocates `count` objects of the light sizes, writes them, and frees them in random order.
fn burst<H: GlobalAlloc>(heap: &H, count: usize) {
    let mut x = 0x2545_f491_4f6c_dd1du64;
    let mut next = move || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    let mut held: Vec<_> = (0..count)
        .map(|_| {
            let l = layout(LIGHT[next() as usize % LIGHT.len()], 8);
            // SAFETY: a valid layout; the write stays inside the block.
            let p = unsafe { heap.alloc(l) };
            assert!(!p.is_null());
            // SAFETY: as above.
            unsafe { p.write_bytes(1, l.size()) };
            (p, l)
        })
        .collect();
    for i in (1..held.len()).rev() {
        held.swap(i, next() as usize % (i + 1));
    }
    for (p, l) in held {
        // SAFETY: allocated above with this layout, freed once.
        unsafe { heap.dealloc(p, l) };
    }
}

fn resident<S: crate::os::sim::Shape>(heap: &SimHeap<S>) -> usize {
    // SAFETY: nothing else uses the heap during the call, and every byte the heap and the
    // test wrote is initialized.
    unsafe { heap.os().resident_bytes() }
}

/// The case `maintain` is for: a burst of frees, then light work that never leaves the loaded
/// magazines, so that no slow path runs. Without `maintain` the heap keeps what it held. With
/// it once an interval, the idle slab is drained and laundered, the depot sheds, and the
/// clocked purge returns what is free from the top down, a quarter of what is retained an
/// interval once under the cap: resident memory comes down to what `collect` leaves, within
/// what that rate leaves after the intervals run. A second call in an interval does nothing.
#[test]
#[cfg_attr(miri, ignore = "reads every simulated page dozens of times")]
fn maintain_brings_a_quiet_heap_down_to_what_collect_leaves() {
    let heap = SimHeap::<Shape>::new();
    burst(&heap, if cfg!(miri) { 3000 } else { 100_000 });
    light_work(&heap);
    let quiet = resident(&heap);
    let purges = heap.os().calls(Call::Purge);
    for _ in 0..3 {
        heap.os().advance_clock(crate::depot::IDLE_TICKS);
        light_work(&heap);
    }
    assert_eq!(
        heap.os().calls(Call::Purge),
        purges,
        "the light work took a slow path"
    );
    assert_eq!(resident(&heap), quiet);
    let intervals = 14;
    for _ in 0..intervals {
        heap.os().advance_clock(crate::depot::IDLE_TICKS);
        light_work(&heap);
        heap.maintain();
        let calls = (
            heap.os().calls(Call::Purge),
            heap.os().calls(Call::Fence),
            resident(&heap),
        );
        heap.maintain();
        let again = (
            heap.os().calls(Call::Purge),
            heap.os().calls(Call::Fence),
            resident(&heap),
        );
        assert_eq!(again, calls, "a second call in an interval did something");
    }
    let maintained = resident(&heap);
    heap.collect();
    let collected = resident(&heap);
    // Two intervals before the purge has what was parked (a look, then the drain and shed).
    // Frees do not purge, so what the drain launders past the cap waits for the passes that
    // follow, which take it down to the cap; then a quarter of the rest each interval. Two
    // intervals are allowed for reaching the cap, since a pass ends after one lap whatever
    // parked objects still pin. Passes carry on round the heap from where the last one
    // stopped, so no free page waits behind the pages light work revives each interval: an
    // earlier walk that began every pass at the top left the bottom of the lowest region
    // resident for ever, under a slack of two huge pages that hid it.
    let geometry = <Shape as crate::os::sim::Shape>::GEOMETRY;
    // The passes stop at the floor, which `collect` does not keep.
    let cap = crate::back::policy::retained_cap(geometry);
    let floor = crate::back::policy::allowance_floor(geometry);
    let left = floor as f64 + (cap - floor) as f64 * 0.75f64.powi(intervals - 5);
    assert!(
        maintained <= collected + left as usize,
        "after {intervals} intervals of maintain: {maintained} bytes resident, collect leaves \
         {collected}, the clocked rules allow {left:.0} more"
    );
    assert!(quiet > 4 * maintained, "{quiet} {maintained}");
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
        // Every call went through magazines: none was served from the back end because the
        // magazine area ran out.
        assert_eq!(heap.refused_magazines(), 0);
    }
}

/// The hold on free large slots (`back::hold`) through the heap, whose large calls beat the
/// heart one in sixteen.
mod hold {
    use super::*;
    use crate::back::policy::{FINE_TICKS, HOLD_STEPS, allowance_floor};
    use crate::os::sim::{Native, Shape as _};

    /// Allocates, writes and frees twice the hold's floor in blocks of 256 KiB. Returns the
    /// heap's free large memory then.
    fn free_a_phase(heap: &SimHeap<Native>) -> usize {
        let big = layout(256 << 10, 8);
        let n = 2 * allowance_floor(Native::GEOMETRY) / big.size();
        // SAFETY: a valid layout; each block is written within its size and freed once.
        unsafe {
            let blocks: Vec<*mut u8> = (0..n).map(|_| heap.alloc(big)).collect();
            for &b in &blocks {
                b.write_bytes(7, big.size());
            }
            for &b in &blocks {
                heap.dealloc(b, big);
            }
        }
        let freed = heap.heap.back().unwrap().retained_large();
        assert!(freed >= 2 * allowance_floor(Native::GEOMETRY));
        freed
    }

    /// A phase's free blocks are held while the next phase works in another large class, and
    /// go back down to the floor once their hold has ended, counted in the steps the next
    /// phase's heartbeats take.
    #[test]
    #[cfg_attr(miri, ignore)]
    fn a_phases_free_large_blocks_go_back_while_the_next_runs() {
        let heap = SimHeap::<Native>::new();
        let freed = free_a_phase(&heap);
        let back = heap.heap.back().unwrap();
        let small = layout(64 << 10, 8);
        // Two large calls an iteration, so a heartbeat every eight, a fine step apart.
        let work = |iterations: u32| {
            for _ in 0..iterations {
                heap.os().advance_clock(FINE_TICKS / 8);
                // SAFETY: a valid layout; written within its size and freed once.
                unsafe {
                    let p = heap.alloc(small);
                    p.write_bytes(3, small.size());
                    heap.dealloc(p, small);
                }
            }
        };
        // The first carve in the new class pays its growth against the retained cap, which
        // twice the floor is over in this shape.
        work(1);
        let held = back.retained_large();
        assert!(
            held > allowance_floor(Native::GEOMETRY) && held <= freed + small.size(),
            "held {held} freed {freed}"
        );
        work(8 * (HOLD_STEPS - 4));
        assert!(back.retained_large() >= held, "held for the hold");
        work(8 * 2 * HOLD_STEPS);
        let left = back.retained_large();
        let floor = allowance_floor(Native::GEOMETRY);
        assert!(
            left <= floor + small.size(),
            "{left} left over a floor of {floor}"
        );
    }

    /// `maintain`, in a silence, returns a quiet program's free large blocks down to the floor
    /// at its second round: the first takes the whole hold, as no earlier round looked. The
    /// clocked pass alone, which runs in the same rounds, returns at most a quarter of what is
    /// retained a round, and nothing the last two intervals' claims reached.
    #[test]
    #[cfg_attr(miri, ignore)]
    fn maintain_returns_a_quiet_programs_free_large_blocks() {
        let heap = SimHeap::<Native>::new();
        let freed = free_a_phase(&heap);
        let back = heap.heap.back().unwrap();
        let floor = allowance_floor(Native::GEOMETRY);
        let round = || {
            heap.os().advance_clock(crate::depot::IDLE_TICKS);
            heap.maintain();
            back.retained_large()
        };
        let first = round();
        assert!(
            first > floor + (freed - floor) / 2,
            "{first} left after the first round"
        );
        let second = round();
        assert!(second <= floor, "{second} left over a floor of {floor}");
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

        fn plan(&self) -> Option<Plan> {
            crate::heap::global::process()
                .heap()
                .back()
                .map(|back| back.space().plan())
        }
    }

    #[test]
    fn the_process_heap_serves_every_class() {
        every_class(&Asmalloc, 3);
    }

    #[test]
    fn the_process_heap_exchanges_magazines() {
        // Past the largest magazine, so that every class exchanges.
        let burst = 2 * crate::classes::MagazineSize::MAX.capacity() + 100;
        for size in [8, 48, 512, MAX_SMALL_SIZE] {
            bursts(&Asmalloc, size, burst, 40);
        }
    }

    #[test]
    fn the_process_heap_resizes_in_place_and_with_headroom() {
        // Whether the process's heap got its full reservation is the machine's to say: under
        // an address-space limit its regions are halved, and it gives no size sixteen times.
        // SAFETY: a valid layout; the block is freed once.
        unsafe { Asmalloc.dealloc(Asmalloc.alloc(layout(8, 8)), layout(8, 8)) };
        let plan = Asmalloc.plan().expect("the heap has started");
        let full = plan.region_shift() == plan.geometry().max_region_shift();
        realloc_walk(&Asmalloc, 32 << 20, full);
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

    /// `maintain` on the process heap while other tests use it, and its thread: started
    /// twice, it is one thread, which then maintains the heap under every test that follows.
    #[test]
    fn the_process_heap_maintains_and_its_thread_starts_once() {
        bursts(&Asmalloc, 64, 600, 4);
        Asmalloc::maintain();
        Asmalloc::start_background().expect("a thread starts");
        Asmalloc::start_background().expect("a second start is harmless");
        let named = || {
            std::fs::read_dir("/proc/self/task")
                .unwrap()
                .filter_map(|task| std::fs::read_to_string(task.ok()?.path().join("comm")).ok())
                .filter(|name| name.trim() == "asmalloc")
                .count()
        };
        // The new thread names itself once it runs.
        let mut tries = 0;
        while named() == 0 && tries < 200 {
            thread::sleep(std::time::Duration::from_millis(5));
            tries += 1;
        }
        assert_eq!(named(), 1);
        every_class(&Asmalloc, 1);
        Asmalloc::maintain();
    }

    /// What the simulated heaps cannot show: a large block cut to a sliver of its slot, under its
    /// threshold, moves to a block of its size, and one cut to just above that stays.
    #[test]
    fn the_process_heap_moves_a_large_shrink_to_a_sliver() {
        let l = layout(4 << 20, 8);
        // SAFETY: valid layouts; each block is freed once with the layout it has.
        unsafe {
            let p = Asmalloc.alloc(l);
            let back = crate::heap::global::process().heap().back().unwrap();
            let room = back.room(p).unwrap();
            let sliver = room / shrink_moves_below_for(room, back.space().plan());
            let q = Asmalloc.realloc(p, l, sliver + 4096);
            assert_eq!(q, p, "a shrink to just above the threshold stays");
            let r = Asmalloc.realloc(q, layout(sliver + 4096, 8), sliver - 4096);
            assert_ne!(r, q, "a shrink to under the threshold moves");
            Asmalloc.dealloc(r, layout(sliver - 4096, 8));
        }
    }

    /// The threshold follows the slot, in a heap with its full reservation: a block in a slot
    /// of at most a sixteenth of a region stays down to a thirty-second of it, and one in a
    /// larger slot moves once it is under an eighth, as it did before growing blocks got
    /// sixteen times the room. The fractions are spelled out here, not taken from
    /// `shrink_moves_below_for`. A heap whose regions were halved to fit an address-space limit
    /// has no slot of these sizes on either side, and the test asks nothing of it.
    #[test]
    fn the_process_heap_keeps_a_thirty_second_of_a_small_slot_and_an_eighth_of_a_large_one() {
        // SAFETY: valid layouts; each block is freed once with the layout it has. The blocks
        // are never written, so they cost address space and no memory.
        unsafe {
            let under = layout(250 << 20, 8);
            let p = Asmalloc.alloc(under);
            let back = crate::heap::global::process().heap().back().unwrap();
            let plan = back.space().plan();
            if plan.region_shift() != plan.geometry().max_region_shift() {
                Asmalloc.dealloc(p, under);
                return;
            }
            let share = plan.region_size() / 16;
            let room = back.room(p).unwrap();
            assert!(room <= share, "{room}");
            let q = Asmalloc.realloc(p, under, room / 16);
            assert_eq!(q, p, "a sixteenth of a small slot stays");
            let cut = room / 32 - 4096;
            let r = Asmalloc.realloc(q, layout(room / 16, 8), cut);
            assert_ne!(r, q, "under a thirty-second of a small slot moves");
            Asmalloc.dealloc(r, layout(cut, 8));

            let over = layout(257 << 20, 8);
            let p = Asmalloc.alloc(over);
            let room = back.room(p).unwrap();
            assert!(room > share, "{room}");
            let kept = room / 8 + 4096;
            let q = Asmalloc.realloc(p, over, kept);
            assert_eq!(q, p, "an eighth of a large slot stays");
            let r = Asmalloc.realloc(q, layout(kept, 8), room / 16);
            assert_ne!(r, q, "a sixteenth of a large slot moves");
            Asmalloc.dealloc(r, layout(room / 16, 8));
        }
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
