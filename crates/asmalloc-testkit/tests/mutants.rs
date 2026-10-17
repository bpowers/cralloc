//! The model catches every mutant, on one thread and on many, through each interface, within a
//! bounded number of operations; and a caught failure minimizes to a short script that
//! replays to the same failure.

mod common;

use asmalloc_testkit::mutants::Kind;
use asmalloc_testkit::subject::Subject;
use asmalloc_testkit::{Config, Failure, Handoff, Profile, Trace, minimize, replay, run, target};
use common::fit;

/// How each mutant is hunted: the profile that exposes it, and the operations of one worker
/// within which every seed must catch it. Each bound is about three times the latest detection
/// over 50 seeds, through either interface, on one thread or four (2026-10-16, against glibc).
fn hunt(kind: Kind) -> (Profile, usize) {
    match kind {
        Kind::Overlapping => (Profile::SMALL_FIXED, 500),
        Kind::Misaligned => (Profile::ALIGN_HEAVY, 30),
        Kind::Short => (Profile::SMALL_FIXED, 50),
        Kind::NoZeroFill => (Profile::SMALL_FIXED, 250),
        Kind::LosesContents => (Profile::MIXED, 100),
        // Its bug needs frees on another thread: on one worker, thread migration provides them.
        Kind::DoubleHandout => (Profile::MIXED.with_migrate(30), 500),
        Kind::Scribbler => (Profile::SMALL_FIXED, 300),
        Kind::PrematureReuse => (Profile::SMALL_FIXED, 1000),
    }
}

/// Mutants Miri can run: it reports reading uninitialized memory, and data races, as
/// undefined behaviour before the model sees the bug.
fn runnable(kind: Kind, threads: bool) -> bool {
    !cfg!(miri) || !(kind.reads_uninit() || threads && kind.races())
}

fn seeds() -> u64 {
    if cfg!(miri) { 1 } else { 5 }
}

fn expect_caught(
    kind: Kind,
    result: Result<asmalloc_testkit::Stats, Failure>,
    bound: usize,
) -> Failure {
    let f = match result {
        Ok(stats) => panic!("{kind:?} was not caught within {bound} ops: {stats:?}"),
        Err(f) => f,
    };
    assert!(
        kind.caught_as().contains(&f.kind),
        "{kind:?} caught as the wrong kind: {f}"
    );
    assert!(
        f.op_index < bound,
        "{kind:?} caught only at the final drain: {f}"
    );
    f
}

fn single_threaded<S: Subject + 'static>(make: impl Fn() -> S, interface: &str) {
    for kind in Kind::ALL.into_iter().filter(|&k| runnable(k, false)) {
        let (profile, bound) = hunt(kind);
        let profile = fit(profile);
        let mut worst = 0;
        for seed in 1..=seeds() {
            let mutant = kind.wrap(make());
            let f = expect_caught(
                kind,
                run(&mutant, &Config::single(profile, seed, bound)),
                bound,
            );
            worst = worst.max(f.op_index + 1);
        }
        eprintln!("{interface}: {} caught within {worst} ops", kind.name());
    }
}

fn multithreaded<S: Subject + 'static>(make: impl Fn() -> S, interface: &str) {
    let workers = if cfg!(miri) { 2 } else { 4 };
    for kind in Kind::ALL.into_iter().filter(|&k| runnable(k, true)) {
        let (profile, bound) = hunt(kind);
        let profile = fit(profile.with_migrate(0));
        let mut worst = 0;
        for seed in 1..=seeds() {
            let mutant = kind.wrap(make());
            let cfg = Config::threads(profile, workers, Handoff::AllToAll, seed, bound);
            let f = expect_caught(kind, run(&mutant, &cfg), bound);
            worst = worst.max(f.op_index + 1);
        }
        eprintln!(
            "{interface}, {workers} threads: {} caught within {worst} ops of a worker",
            kind.name()
        );
    }
}

#[test]
fn every_mutant_is_caught_through_global_alloc() {
    single_threaded(target::global_alloc, "GlobalAlloc");
}

#[test]
fn every_mutant_is_caught_through_the_c_abi() {
    if cfg!(miri) {
        return;
    }
    single_threaded(target::c_abi, "C");
}

#[test]
fn every_mutant_is_caught_across_threads_through_global_alloc() {
    multithreaded(target::global_alloc, "GlobalAlloc");
}

#[test]
fn every_mutant_is_caught_across_threads_through_the_c_abi() {
    if cfg!(miri) {
        return;
    }
    multithreaded(target::c_abi, "C");
}

#[test]
fn caught_failures_minimize_to_scripts_that_replay_to_the_same_failure() {
    for kind in Kind::ALL.into_iter().filter(|&k| runnable(k, false)) {
        let (profile, bound) = hunt(kind);
        let mutant = kind.wrap(target::global_alloc());
        let f = expect_caught(
            kind,
            run(&mutant, &Config::single(fit(profile), 1, bound)),
            bound,
        );
        let Some(Trace::Complete(script)) = &f.trace else {
            panic!("a single-worker failure carries its whole script: {f}");
        };
        let small = minimize(|| kind.wrap(target::global_alloc()), script, f.kind);
        assert!(small.0.len() <= script.0.len());
        let again = replay(&kind.wrap(target::global_alloc()), &small)
            .expect_err("the minimized script still fails");
        assert_eq!(again.kind, f.kind, "{again}");
        let text = small.to_string();
        let parsed = text.parse().expect("scripts print as they parse");
        let once_more = replay(&kind.wrap(target::global_alloc()), &parsed)
            .expect_err("the printed script still fails");
        assert_eq!(once_more.kind, f.kind);
        eprintln!(
            "{}: {} ops minimized to {}: {text}",
            kind.name(),
            script.0.len(),
            small.0.len()
        );
    }
}
