//! The model against asmalloc's simulated heap (twins on the simulated OS), natively: the
//! configuration Miri runs, at a speed that lets it run long. Under Miri, `target` already is
//! this heap, so `system.rs` covers it there.
#![cfg(not(miri))]

mod common;

use asmalloc::SimAsmalloc;
use asmalloc_testkit::subject::ViaGlobalAlloc;
use asmalloc_testkit::{Config, Handoff, Profile, check};
use common::{balanced, ops, ops_for, profiles};

/// The simulated heap holds 64 MiB in all.
fn fit(p: Profile) -> Profile {
    p.capped(64 << 10, 4096, 8 << 20)
}

#[test]
fn the_simulated_heap_serves_every_profile() {
    let s = ViaGlobalAlloc(SimAsmalloc);
    for p in profiles() {
        let p = fit(p);
        for seed in 1..=2 {
            balanced(&check(&s, &Config::single(p, seed, ops_for(&p))), p.name);
        }
    }
}

#[test]
fn the_simulated_heap_frees_on_other_threads_with_miri_sizes() {
    let s = ViaGlobalAlloc(SimAsmalloc);
    let p = Profile::MIXED.with_migrate(5).capped(4096, 4096, 1 << 20);
    for handoff in [Handoff::Pairs, Handoff::AllToAll, Handoff::Steal] {
        for seed in 1..=20 {
            let cfg = Config::threads(p, 3, handoff, seed, ops(6000, 0));
            balanced(&check(&s, &cfg), &format!("{handoff:?}"));
        }
    }
}

#[test]
fn the_simulated_heap_frees_on_other_threads() {
    let s = ViaGlobalAlloc(SimAsmalloc);
    for profile in [Profile::MIXED.with_migrate(5), Profile::SMALL_FIXED] {
        for handoff in [Handoff::Pairs, Handoff::AllToAll, Handoff::Steal] {
            for seed in [11, 12, 13] {
                let cfg = Config::threads(fit(profile), 6, handoff, seed, ops(6000, 0));
                balanced(&check(&s, &cfg), &format!("{handoff:?}"));
            }
        }
    }
}
