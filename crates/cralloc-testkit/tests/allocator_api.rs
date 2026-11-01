//! The `Allocator` interface of the allocator under test: every profile, hand-off between
//! threads, and every mutant. Needs the `allocator-api` feature and a toolchain where the trait
//! is stable (1.100, or nightly today).
#![cfg(feature = "allocator-api")]

mod common;

use common::{balanced, fit, ops, ops_for, profiles, workers};
use cralloc_testkit::mutants::Kind;
use cralloc_testkit::{Config, Handoff, Profile, check, run, target};

#[test]
fn allocator_serves_every_profile_including_zero_sizes() {
    let s = target::allocator();
    let mut zero_sized = 0;
    for p in profiles() {
        let stats = check(&s, &Config::single(p, 4, ops_for(&p)));
        balanced(&stats, p.name);
        zero_sized += stats.null_allocs;
    }
    assert_eq!(
        zero_sized, 0,
        "Allocator never returns nothing for a zero-size layout"
    );
}

#[test]
fn allocator_frees_on_other_threads() {
    let s = target::allocator();
    for handoff in [Handoff::Pairs, Handoff::AllToAll, Handoff::Steal] {
        let cfg = Config::threads(fit(Profile::MIXED), workers(), handoff, 8, ops(5000, 100));
        balanced(&check(&s, &cfg), &format!("{handoff:?}"));
    }
}

#[test]
fn every_mutant_is_caught_through_the_allocator_trait() {
    for kind in Kind::ALL {
        if cfg!(miri) && kind.reads_uninit() {
            continue;
        }
        let profile = match kind {
            Kind::Misaligned => Profile::ALIGN_HEAVY,
            Kind::LosesContents => Profile::MIXED,
            Kind::DoubleHandout => Profile::MIXED.with_migrate(30),
            _ => Profile::SMALL_FIXED,
        };
        let bound = 1000;
        for seed in 1..=3 {
            let m = kind.wrap(target::allocator());
            let f = run(&m, &Config::single(fit(profile), seed, bound))
                .expect_err("every mutant is caught");
            assert!(kind.caught_as().contains(&f.kind), "{kind:?}: {f}");
        }
    }
}
