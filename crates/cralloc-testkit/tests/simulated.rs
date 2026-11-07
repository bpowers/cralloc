//! The model against cralloc's simulated heap (twins on the simulated OS), natively: the
//! configuration Miri runs, at a speed that lets it run long. Under Miri, `target` already is
//! this heap, so `system.rs` covers it there.
#![cfg(not(miri))]

mod common;

use common::{balanced, ops, ops_for, profiles, with_drains};
use cralloc::classes::MagazineSize;
use cralloc::os::sim::{Shape, SimOs};
use cralloc::space::Geometry;
use cralloc::{SimCralloc, SimHeap};
use cralloc_testkit::subject::ViaGlobalAlloc;
use cralloc_testkit::{Config, Handoff, Profile, check};

/// The simulated heap holds 64 MiB in all, shared by every test here, which run at once and
/// with up to six workers each: keep what they hold well inside it.
fn fit(p: Profile) -> Profile {
    p.capped(64 << 10, 4096, 2 << 20)
}

// Each test runs a few seeds in the merge gate and more on demand, in its `_fully` form.

fn the_simulated_heap_serves_every_profile(seeds: u64) {
    let s = ViaGlobalAlloc(SimCralloc);
    for p in profiles() {
        let p = fit(p);
        for seed in 1..=seeds {
            balanced(&check(&s, &Config::single(p, seed, ops_for(&p))), p.name);
        }
    }
}

#[test]
fn the_simulated_heap_serves_every_profile_quickly() {
    the_simulated_heap_serves_every_profile(1);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn the_simulated_heap_serves_every_profile_fully() {
    the_simulated_heap_serves_every_profile(2);
}

fn the_simulated_heap_frees_on_other_threads_with_miri_sizes(seeds: u64) {
    let s = ViaGlobalAlloc(SimCralloc);
    let p = Profile::MIXED.with_migrate(5).capped(4096, 4096, 1 << 20);
    with_drains(SimCralloc::heap(), || {
        for handoff in [Handoff::Pairs, Handoff::AllToAll, Handoff::Steal] {
            for seed in 1..=seeds {
                let cfg = Config::threads(p, 3, handoff, seed, ops(6000, 0));
                balanced(&check(&s, &cfg), &format!("{handoff:?}"));
            }
        }
    });
}

#[test]
fn the_simulated_heap_frees_on_other_threads_with_miri_sizes_quickly() {
    the_simulated_heap_frees_on_other_threads_with_miri_sizes(2);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn the_simulated_heap_frees_on_other_threads_with_miri_sizes_fully() {
    the_simulated_heap_frees_on_other_threads_with_miri_sizes(20);
}

/// A fresh simulated heap, by reference.
struct Fresh<S: Shape = cralloc::os::sim::Standard>(&'static SimHeap<S>);

impl<S: Shape> Clone for Fresh<S> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<S: Shape> Copy for Fresh<S> {}

// SAFETY: forwards to the heap's own implementation.
unsafe impl<S: Shape> core::alloc::GlobalAlloc for Fresh<S> {
    unsafe fn alloc(&self, l: core::alloc::Layout) -> *mut u8 {
        // SAFETY: the caller's contract, passed on.
        unsafe { self.0.alloc(l) }
    }
    unsafe fn alloc_zeroed(&self, l: core::alloc::Layout) -> *mut u8 {
        // SAFETY: as above.
        unsafe { self.0.alloc_zeroed(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: core::alloc::Layout) {
        // SAFETY: as above.
        unsafe { self.0.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: core::alloc::Layout, n: usize) -> *mut u8 {
        // SAFETY: as above.
        unsafe { self.0.realloc(p, l, n) }
    }
}

/// Pages of 16 KiB, huge pages of 1 MiB, regions of 4 MiB down to 512 KiB.
enum HugePageOver16 {}

impl Shape for HugePageOver16 {
    const GEOMETRY: Geometry = Geometry::new(14, 20, 22, 19, 256, 16, MagazineSize::MAX);
}

/// Pages of 64 KiB, huge pages of 4 MiB, regions of 8 MiB down to 2 MiB.
enum HugePageOver64 {}

impl Shape for HugePageOver64 {
    const GEOMETRY: Geometry = Geometry::new(16, 22, 23, 21, 256, 16, MagazineSize::MAX);
}

/// The model on a heap that got only its smallest regions, which are smaller than a huge page, as
/// a limit on address space leaves one on a kernel with large pages. The heap asks for no huge page
/// at any step, and serves every profile and hand-off all the same.
fn a_heap_of_regions_below_a_huge_page_serves_the_model<S: Shape>(seeds: u64, each: &[Profile]) {
    let smallest = S::GEOMETRY.smallest_plan();
    assert!(smallest.region_size() < S::GEOMETRY.huge_page_size());
    assert!(!smallest.geometry().huge_pages_usable());
    let largest = S::GEOMETRY.largest_plan().region_size();
    let room = smallest.size().next_multiple_of(largest);
    for seed in 1..=seeds {
        let os = SimOs::<S>::with_capacity(room);
        let heap = Fresh::<S>(Box::leak(Box::new(SimHeap::with_os(os))));
        let s = ViaGlobalAlloc(heap);
        for &p in each {
            let p = fit(p);
            balanced(&check(&s, &Config::single(p, seed, 4000)), p.name);
        }
        let threads = Config::threads(fit(Profile::MIXED), 4, Handoff::AllToAll, seed, 2000);
        balanced(&check(&s, &threads), "threads");
        heap.0.collect();
        assert_eq!(heap.0.os().advised_bytes(), 0);
    }
}

#[test]
fn a_heap_of_regions_below_a_huge_page_serves_the_model_quickly() {
    a_heap_of_regions_below_a_huge_page_serves_the_model::<HugePageOver16>(1, &[Profile::MIXED]);
    a_heap_of_regions_below_a_huge_page_serves_the_model::<HugePageOver64>(1, &[Profile::MIXED]);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn a_heap_of_regions_below_a_huge_page_serves_the_model_fully() {
    let all: Vec<Profile> = profiles().into_iter().collect();
    a_heap_of_regions_below_a_huge_page_serves_the_model::<HugePageOver16>(4, &all);
    a_heap_of_regions_below_a_huge_page_serves_the_model::<HugePageOver64>(4, &all);
}

fn fresh_simulated_heaps_serve_threads_that_hand_blocks_over(seeds: u64) {
    for seed in 0..seeds {
        let heap = Fresh(Box::leak(Box::new(SimHeap::new())));
        let s = ViaGlobalAlloc(heap);
        let cfg = Config::threads(
            fit(Profile::MIXED.with_migrate(5)),
            6,
            Handoff::AllToAll,
            seed,
            6000,
        );
        with_drains(heap.0, || balanced(&check(&s, &cfg), "fresh"));
    }
}

#[test]
fn fresh_simulated_heaps_serve_threads_that_hand_blocks_over_quickly() {
    fresh_simulated_heaps_serve_threads_that_hand_blocks_over(4);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn fresh_simulated_heaps_serve_threads_that_hand_blocks_over_fully() {
    fresh_simulated_heaps_serve_threads_that_hand_blocks_over(40);
}

fn the_simulated_heap_frees_on_other_threads(seeds: u64) {
    let s = ViaGlobalAlloc(SimCralloc);
    with_drains(SimCralloc::heap(), || {
        for profile in [Profile::MIXED.with_migrate(5), Profile::SMALL_FIXED] {
            for handoff in [Handoff::Pairs, Handoff::AllToAll, Handoff::Steal] {
                for seed in 11..11 + seeds {
                    let cfg = Config::threads(fit(profile), 6, handoff, seed, ops(6000, 0));
                    balanced(&check(&s, &cfg), &format!("{handoff:?}"));
                }
            }
        }
    });
}

#[test]
fn the_simulated_heap_frees_on_other_threads_quickly() {
    the_simulated_heap_frees_on_other_threads(1);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn the_simulated_heap_frees_on_other_threads_fully() {
    the_simulated_heap_frees_on_other_threads(3);
}
