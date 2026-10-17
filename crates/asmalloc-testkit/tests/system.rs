//! The allocator under test (`asmalloc_testkit::target`) through every interface, profile and
//! threading pattern. The model must accept a correct allocator on every step, so a failure
//! here is a bug in the allocator or in the kit.

mod common;

use asmalloc_testkit::entropy::Rng;
use asmalloc_testkit::subject::Subject;
use asmalloc_testkit::{Config, Handoff, Profile, Script, check, generate, replay, run, run_bytes};
use asmalloc_testkit::{Stats, target};
use common::{balanced, fit, ops, ops_for, profiles, workers};

fn every_profile(subject: &dyn Subject) {
    for p in profiles() {
        for seed in 1..=2 {
            let stats = check(&subject, &Config::single(p, seed, ops_for(&p)));
            balanced(&stats, p.name);
            assert_eq!(stats.ops, ops_for(&p), "{}", p.name);
        }
    }
}

#[test]
fn global_alloc_serves_every_profile() {
    every_profile(&target::global_alloc());
}

#[test]
fn c_abi_serves_every_profile() {
    every_profile(&target::c_abi());
}

fn every_handoff(subject: &dyn Subject, profile: Profile) -> Vec<Stats> {
    [Handoff::Pairs, Handoff::AllToAll, Handoff::Steal]
        .into_iter()
        .map(|handoff| {
            let cfg = Config::threads(fit(profile), workers(), handoff, 11, ops(6000, 120));
            let stats = check(&subject, &cfg);
            balanced(&stats, &format!("{handoff:?}"));
            assert!(
                stats.sends > 0 && stats.receives > 0,
                "{handoff:?}: {stats:?}"
            );
            stats
        })
        .collect()
}

#[test]
fn global_alloc_frees_on_other_threads() {
    every_handoff(&target::global_alloc(), Profile::MIXED.with_migrate(5));
    every_handoff(&target::global_alloc(), Profile::SMALL_FIXED);
}

#[test]
fn c_abi_frees_on_other_threads() {
    every_handoff(&target::c_abi(), Profile::MIXED.with_migrate(5));
    every_handoff(&target::c_abi(), Profile::CHURN);
}

#[test]
fn threads_exit_mid_stream_holding_blocks() {
    let p = fit(Profile::SMALL_FIXED.with_migrate(40));
    let cfg = Config::threads(p, workers(), Handoff::AllToAll, 5, ops(4000, 100));
    let stats = check(&target::global_alloc(), &cfg);
    balanced(&stats, "migrating");
    assert!(stats.migrations > workers(), "{stats:?}");
}

#[test]
fn a_generated_script_replays_exactly_like_the_seeded_run() {
    let s = target::global_alloc();
    let p = fit(Profile::MIXED);
    let n = ops(3000, 100);
    let script = generate(p, s.zero_size(), 9, n);
    assert_eq!(script.0.len(), n);
    assert_eq!(script.to_string().parse::<Script>(), Ok(script.clone()));
    let replayed = replay(&s, &script).unwrap();
    let ran = run(&s, &Config::single(p, 9, n)).unwrap();
    assert_eq!(replayed, ran);
}

#[test]
fn arbitrary_bytes_decode_into_runs_the_model_accepts() {
    let s = target::global_alloc();
    let c = target::c_abi();
    let mut rng = Rng::new(0xF022);
    for round in 0..ops(24, 3) {
        let len = (rng.next_u64() % 4000) as usize;
        let bytes: Vec<u8> = (0..len).map(|_| rng.next_u64() as u8).collect();
        let p = Profile::ALL[round % Profile::ALL.len()].capped(1 << 20, 1 << 16, 16 << 20);
        let p = fit(p);
        for subject in [&s as &dyn Subject, &c] {
            let stats = run_bytes(&subject, p, &bytes).unwrap_or_else(|f| panic!("{f}"));
            assert_eq!(stats.frees, stats.blocks_allocated());
        }
    }
}

#[cfg(all(target_os = "linux", not(miri)))]
#[test]
fn fork_while_other_threads_allocate_then_allocate_in_the_child() {
    use asmalloc_testkit::ForkPlan;
    use std::time::Duration;

    let plan = ForkPlan {
        at: 1500,
        child_workers: 3,
        child_ops: 2000,
        timeout: Duration::from_secs(60),
    };
    for subject in [&target::global_alloc() as &dyn Subject, &target::c_abi()] {
        for handoff in [Handoff::AllToAll, Handoff::Steal] {
            let cfg = Config::threads(Profile::MIXED, 4, handoff, 3, 4000).with_fork(plan);
            let stats = check(&subject, &cfg);
            assert_eq!(stats.forks, 1);
            balanced(&stats, "fork");
        }
    }
}
