//! The model against asmalloc's simulated heap (twins on the simulated OS), natively: the
//! configuration Miri runs, at a speed that lets it run long. Under Miri, `target` already is
//! this heap, so `system.rs` covers it there.
#![cfg(not(miri))]

mod common;

use asmalloc::SimAsmalloc;
use asmalloc_testkit::subject::ViaGlobalAlloc;
use asmalloc_testkit::{Config, Handoff, Profile, check};
use common::{balanced, ops, ops_for, profiles, with_drains};

/// The simulated heap holds 64 MiB in all, shared by every test here, which run at once and
/// with up to six workers each: keep what they hold well inside it.
fn fit(p: Profile) -> Profile {
    p.capped(64 << 10, 4096, 2 << 20)
}

// Each test runs a few seeds in the merge gate and more on demand, in its `_fully` form.

fn the_simulated_heap_serves_every_profile(seeds: u64) {
    let s = ViaGlobalAlloc(SimAsmalloc);
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
    let s = ViaGlobalAlloc(SimAsmalloc);
    let p = Profile::MIXED.with_migrate(5).capped(4096, 4096, 1 << 20);
    with_drains(SimAsmalloc::heap(), || {
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
#[derive(Clone, Copy)]
struct Fresh(&'static asmalloc::SimHeap);

// SAFETY: forwards to the heap's own implementation.
unsafe impl core::alloc::GlobalAlloc for Fresh {
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

fn fresh_simulated_heaps_serve_threads_that_hand_blocks_over(seeds: u64) {
    for seed in 0..seeds {
        let heap = Fresh(Box::leak(Box::new(asmalloc::SimHeap::new())));
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
    let s = ViaGlobalAlloc(SimAsmalloc);
    with_drains(SimAsmalloc::heap(), || {
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
