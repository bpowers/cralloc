//! The TLA+ models' properties, checked on the production protocols under the interleaving
//! explorer (`explore`).
//!
//! Each scenario names the configuration in `tla/` it follows. The threads run `tree` and
//! `region` code unchanged over a `TestHost` whose words are scheduling points, and every
//! schedule is checked for the model's invariants:
//!
//! - `Partition` (no slot handed out twice, none lost): the ghost owner record fails a schedule
//!   in which a thread claims a slot another holds, and the final check counts every slot;
//! - `Visible` (a free slot is never permanently invisible): once the threads finish, every leaf
//!   word with free bits is reachable from the class's top word through set summary bits;
//! - `Healed` (after a fork): the next free into a word makes it reachable again;
//! - `PurgeSafe` (no live slot's memory purged): the ghost record fails a schedule in which a
//!   purge returns pages touched by a slot it does not hold;
//! - `ZeroBitSound` and `ZeroedIsZero`: a set zero bit describes a free slot that reads as zero,
//!   and an allocation told "zeroed" gets a slot that reads as zero;
//! - `NoFalseDoubleFree`: a release that finds its bit set aborts, which fails the schedule.
//!
//! Two-thread scenarios explore every schedule. Three-thread scenarios, and the purge
//! scenarios, whose operations take a dozen steps each, bound preemptions to two or three.
//! Those bounds take seconds a scenario, so they run in the `_fully` tests, on demand
//! (`scripts/gate --all`); the merge gate runs each scenario one preemption lower.

use super::explore::{Config, Outcome, TWord, explore};
use super::group::{Direct, Gather};
use super::layout::{Step, Unit};
use super::region::{self, Carve, Purge};
use super::testhost::TestHost;
use super::tree::{self, Host, Node};
use super::word::{BITS, Word};
use crate::classes::Class;
use crate::classes::MagazineSize;
use crate::os::sim::{Shape, Standard, Tiny};
use crate::space::{Geometry, Plan, RegionId};
use std::collections::BTreeSet;
use std::sync::Mutex;
use std::vec::Vec;

type Host8 = TestHost<TWord>;

fn class(size: usize) -> Class {
    Class::of_size(size).unwrap()
}

/// A geometry with 32 KiB pages and 1 MiB regions, so that large classes have several slots
/// and slots of a few pages: what `Purge_big` and the zero bitmap need.
fn big_plan() -> Plan {
    const BIG: Geometry = Geometry::new(15, 16, 20, 20, 4, 1, MagazineSize::MAX);
    BIG.largest_plan()
}

fn tiny() -> Plan {
    Tiny::GEOMETRY.largest_plan()
}

fn standard() -> Plan {
    Standard::GEOMETRY.largest_plan()
}

/// Frees `slot`, as the program would, from outside any schedule.
fn free_now(host: &Host8, region: RegionId, slot: u32) {
    tree::release(host, region, slot / BITS, 1 << (slot % BITS));
}

/// Frees `slot` the way a single large free does (`tree::release_one`).
fn free_one(host: &Host8, region: RegionId, slot: u32) {
    tree::release_one(host, region, slot / BITS, slot % BITS);
}

/// Claims up to `n` slots and returns them, with their region.
fn claim(host: &Host8, n: u32) -> Vec<(RegionId, u32)> {
    claim_with(host, n, false)
}

/// [`claim`], leaving the summary bits of a word it empties set if `keep`.
fn claim_with(host: &Host8, n: u32, keep: bool) -> Vec<(RegionId, u32)> {
    let Some(taken) = tree::claim(host, n, keep) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut bits = taken.bits;
    while bits != 0 {
        out.push((taken.region, taken.word * BITS + bits.trailing_zeros()));
        bits &= bits - 1;
    }
    out
}

fn release_all(host: &Host8, slots: &[(RegionId, u32)]) {
    for &(region, slot) in slots {
        free_now(host, region, slot);
    }
}

/// The free slots of every region, as heap-wide numbers.
fn free_set(host: &Host8, regions: usize) -> BTreeSet<u64> {
    (0..regions)
        .flat_map(|r| {
            let region = host.region_at(r);
            host.free_slots(region)
                .into_iter()
                .map(move |s| host.global(region, s))
        })
        .collect()
}

/// A host of 8-byte slots in `Tiny` regions (four leaf words, one summary level), with slots 0,
/// 1, 64 and 65 of region 0 and slot 0 of region 1 free, and slots 128 and 192 of region 0 held
/// by the program.
fn summary_setup() -> Host8 {
    let host = Host8::new(tiny(), class(8), 2);
    let (r0, r1) = (host.region_at(0), host.region_at(1));
    host.carve_all(r0, 256);
    host.carve_all(r1, 256);
    for slot in [0, 1, 64, 65] {
        free_now(&host, r0, slot);
    }
    free_now(&host, r1, 0);
    host
}

/// Every slot that was free at the start, or freed by a thread, is free at the end, and every
/// word is visible.
fn summary_check(host: &Host8, outcome: &Outcome, extra: &[u64]) {
    assert!(outcome.killed.is_empty());
    let mut expected: BTreeSet<u64> = [0, 1, 64, 65].into_iter().collect();
    expected.insert(host.global(host.region_at(1), 0));
    expected.extend(extra.iter().copied());
    assert_eq!(free_set(host, 2), expected);
    assert!(host.all_visible(), "a free word is invisible");
}

/// One operation of a summary scenario.
#[derive(Clone, Copy, Debug)]
enum Op {
    /// Claim up to this many slots and release them again.
    ClaimRelease(u32),
    /// Claim up to this many slots and keep them.
    Claim(u32),
    /// Free this held slot of region 0.
    Free(u32),
}

/// Runs `op`. With `one`, claims are the large classes' (`keep`) and every free is a single
/// free (`tree::release_one`).
fn run(host: &Host8, op: Op, one: bool) -> Vec<(RegionId, u32)> {
    let free = |region, slot| {
        if one {
            free_one(host, region, slot)
        } else {
            free_now(host, region, slot)
        }
    };
    match op {
        Op::ClaimRelease(n) => {
            let got = claim_with(host, n, one);
            for &(region, slot) in &got {
                free(region, slot);
            }
            Vec::new()
        }
        Op::Claim(n) => claim_with(host, n, one),
        Op::Free(slot) => {
            free(host.region_at(0), slot);
            Vec::new()
        }
    }
}

/// Runs two single operations against each other under the schedules `config` allows, from
/// `summary_setup`, and checks `Partition` and `Visible`.
fn summary_pair(config: Config, a: Op, b: Op, one: bool) -> usize {
    let ops = [a, b];
    let freed: Vec<u64> = ops
        .iter()
        .filter_map(|op| match *op {
            Op::Free(slot) => Some(slot as u64),
            _ => None,
        })
        .collect();
    explore(
        config,
        2,
        || (summary_setup(), Mutex::new(Vec::new())),
        |(host, kept), me| {
            let got = run(host, ops[me], one);
            kept.lock().unwrap().extend(got);
        },
        |(host, kept), outcome| {
            // Release what the threads kept, from outside the schedule, then check.
            let kept = kept.lock().unwrap().clone();
            release_all(host, &kept);
            summary_check(host, outcome, &freed);
        },
    )
    .schedules
}

/// `Summary.cfg` and `Summary_heal.cfg`, two threads: each pair of single operations that can
/// meet on a word. A claim that empties a word races a free into it (the clear-then-reread
/// against the healing walk), two claims contend for one word, and frees race into an empty
/// word and an empty region. Every schedule in the full run, two preemptions in the quick one.
fn summary_pairs(config: Config, at_least: usize, one: bool) {
    let pairs = [
        (Op::Claim(2), Op::Free(2)),
        (Op::ClaimRelease(2), Op::Free(3)),
        (Op::Claim(1), Op::Claim(1)),
        (Op::Claim(5), Op::Claim(2)),
        (Op::Free(128), Op::Free(129)),
        (Op::Free(128), Op::Claim(5)),
        (Op::Free(192), Op::Claim(4)),
        (Op::ClaimRelease(5), Op::Free(130)),
    ];
    let mut schedules = 0;
    for (a, b) in pairs {
        schedules += summary_pair(config, a, b, one);
    }
    assert!(schedules > at_least, "{schedules}");
}

#[test]
#[cfg_attr(miri, ignore)]
fn summary_pairs_quickly() {
    summary_pairs(
        Config {
            preemptions: Some(2),
            ..Config::ALL
        },
        500,
        false,
    );
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn summary_pairs_every_schedule() {
    summary_pairs(Config::ALL, 1000, false);
}

/// The same pairs as large classes run them: claims that leave the summary bits of a word they
/// empty set, and single frees. A held clear is the model's (a thread may hold it for as long
/// as it likes), so `Partition` and `Visible` hold as before.
#[test]
#[cfg_attr(miri, ignore)]
fn summary_pairs_with_kept_bits_and_single_frees_quickly() {
    summary_pairs(
        Config {
            preemptions: Some(2),
            ..Config::ALL
        },
        500,
        true,
    );
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn summary_pairs_with_kept_bits_and_single_frees_every_schedule() {
    summary_pairs(Config::ALL, 1000, true);
}

/// Longer sequences on two threads, preemptions bounded to three (two in the quick run).
fn summary_two_threads_bounded(preemptions: usize) {
    explore(
        Config {
            preemptions: Some(preemptions),
            ..Config::ALL
        },
        2,
        summary_setup,
        |host, me| {
            let r0 = host.region_at(0);
            if me == 0 {
                let got = claim(host, 2);
                assert!(!got.is_empty());
                release_all(host, &got);
            } else {
                free_now(host, r0, 128);
                let got = claim(host, 3);
                release_all(host, &got);
            }
        },
        |host, outcome| summary_check(host, outcome, &[128]),
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn summary_two_threads_bounded_quickly() {
    summary_two_threads_bounded(2);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn summary_two_threads_bounded_fully() {
    summary_two_threads_bounded(3);
}

/// `Summary_heal.cfg`: three threads, preemptions bounded to two (one in the quick run).
fn summary_three_threads_bounded(preemptions: usize) {
    explore(
        Config {
            preemptions: Some(preemptions),
            ..Config::ALL
        },
        3,
        summary_setup,
        |host, me| {
            let r0 = host.region_at(0);
            match me {
                0 => {
                    let got = claim(host, 2);
                    release_all(host, &got);
                }
                1 => {
                    let got = claim(host, 1);
                    release_all(host, &got);
                }
                _ => {
                    free_now(host, r0, 192);
                    free_now(host, r0, 128);
                }
            }
        },
        |host, outcome| summary_check(host, outcome, &[128, 192]),
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn summary_three_threads_bounded_quickly() {
    summary_three_threads_bounded(1);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn summary_three_threads_bounded_fully() {
    summary_three_threads_bounded(2);
}

/// `Summary_heal_deep.cfg`: a region with two summary levels (8-byte slots in a 64 KiB region:
/// 128 leaf words, two words above them, one top word). Slots in leaf words 0 and 64 sit under
/// different level-1 words. Preemptions bounded to three (two in the quick run).
fn summary_deep_bounded(preemptions: usize) {
    explore(
        Config {
            preemptions: Some(preemptions),
            ..Config::ALL
        },
        2,
        || {
            let host = Host8::new(standard(), class(8), 1);
            assert_eq!(host.layout().height(), 2);
            let r0 = host.region_at(0);
            host.carve_all(r0, host.layout().slots());
            free_now(&host, r0, 0);
            free_now(&host, r0, 64 * 64);
            host
        },
        |host, me| {
            let r0 = host.region_at(0);
            if me == 0 {
                let got = claim(host, 1);
                release_all(host, &got);
                let got = claim(host, 1);
                release_all(host, &got);
            } else {
                free_now(host, r0, 64 * 64 + 1);
            }
        },
        |host, outcome| {
            assert!(outcome.killed.is_empty());
            let expected: BTreeSet<u64> = [0, 64 * 64, 64 * 64 + 1].into_iter().collect();
            assert_eq!(free_set(host, 1), expected);
            assert!(host.all_visible());
        },
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn summary_deep_bounded_quickly() {
    summary_deep_bounded(2);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn summary_deep_bounded_fully() {
    summary_deep_bounded(3);
}

/// The deep host of `summary_deep_bounded`: 8-byte slots in a 64 KiB region, two summary
/// levels, slots 0 and `64 * 64` free.
fn deep_setup() -> Host8 {
    let host = Host8::new(standard(), class(8), 1);
    assert_eq!(host.layout().height(), 2);
    let r0 = host.region_at(0);
    host.carve_all(r0, host.layout().slots());
    free_now(&host, r0, 0);
    free_now(&host, r0, 64 * 64);
    host
}

/// Frees `slots` of region 0 as one batch: one OR per slot, one walk up for all.
fn free_batch(host: &Host8, slots: &[u32]) {
    let r0 = host.region_at(0);
    let mut release = tree::Release::new(host, r0);
    for &slot in slots {
        release.leaf(r0, slot / BITS, 1 << (slot % BITS));
    }
    release.finish();
}

/// Claims `times` times through one cursor, as a fill does, keeping what it gets.
fn claim_run(host: &Host8, times: usize) -> Vec<(RegionId, u32)> {
    claim_run_of(host, times, 1)
}

/// Claims up to `n` slots `times` times through one cursor, then ends the run, as a fill does.
fn claim_run_of(host: &Host8, times: usize, n: u32) -> Vec<(RegionId, u32)> {
    let mut cursor = None;
    let mut out = Vec::new();
    for _ in 0..times {
        let Some(taken) = tree::claim_next(host, n, &mut cursor, &mut |_| {}) else {
            break;
        };
        let mut bits = taken.bits;
        while bits != 0 {
            out.push((taken.region, taken.word * BITS + bits.trailing_zeros()));
            bits &= bits - 1;
        }
    }
    tree::finish_run(host, cursor);
    out
}

/// A host of 8-byte slots in one `Tiny` region (four leaf words under one summary word, so a
/// run batches its clears), with slots 0, 64, 65 and 128 free and the rest held.
fn run_setup() -> Host8 {
    let host = Host8::new(tiny(), class(8), 1);
    let r0 = host.region_at(0);
    host.carve_all(r0, 256);
    for slot in [0, 64, 65, 128] {
        free_now(&host, r0, slot);
    }
    host
}

/// Two runs of claims race each other and a free into a word one of them empties: each run
/// empties words and clears their bits as a batch, moves past the other, and leaves words the
/// other emptied to it. Nothing is handed out twice or lost, and every word is visible at the
/// end (`Summary_run.cfg`). Preemptions bounded to two (one in the quick run).
fn two_runs_race_a_free(preemptions: usize) {
    explore(
        Config {
            preemptions: Some(preemptions),
            ..Config::ALL
        },
        3,
        || (run_setup(), Mutex::new(Vec::new())),
        |(host, kept), me| {
            let got = match me {
                0 => claim_run_of(host, 3, 2),
                1 => claim_run_of(host, 2, 1),
                _ => {
                    free_now(host, host.region_at(0), 1);
                    Vec::new()
                }
            };
            kept.lock().unwrap().extend(got);
        },
        |(host, kept), outcome| {
            assert!(outcome.killed.is_empty());
            let kept = kept.lock().unwrap().clone();
            let distinct: BTreeSet<_> = kept.iter().copied().collect();
            assert_eq!(distinct.len(), kept.len(), "a slot claimed twice");
            // Before the releases below, whose healing walks would repair a lost bit.
            assert!(host.all_visible(), "a free word is invisible");
            release_all(host, &kept);
            let expected: BTreeSet<u64> = [0, 1, 64, 65, 128].into_iter().collect();
            assert_eq!(free_set(host, 1), expected);
            assert!(host.all_visible(), "a free word is invisible");
        },
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn two_runs_race_a_free_quickly() {
    two_runs_race_a_free(1);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn two_runs_race_a_free_fully() {
    two_runs_race_a_free(2);
}

/// `Summary_run_crash.cfg`: a run may vanish, as in a forked child, while it holds clears it has
/// not made, leaving bits set over words it emptied. The survivor then fills from the top: it
/// must still find every free slot before it reports none (the stale bits cost it a clear, not
/// a carve), and the words stay visible. Preemptions bounded to two (one in the quick run).
fn a_run_lost_with_held_clears(preemptions: usize) {
    explore(
        Config {
            preemptions: Some(preemptions),
            crash: true,
            ..Config::ALL
        },
        3,
        || (run_setup(), Mutex::new(Vec::new())),
        |(host, kept), me| {
            let got = match me {
                // The survivor: frees into a word a run may be emptying, then fills.
                0 => {
                    free_now(host, host.region_at(0), 129);
                    claim_run_of(host, 2, 1)
                }
                1 => claim_run_of(host, 3, 1),
                _ => claim_run_of(host, 2, 2),
            };
            kept.lock().unwrap().extend(got);
        },
        |(host, kept), _| {
            let kept: Vec<_> = kept.lock().unwrap().clone();
            let distinct: BTreeSet<_> = kept.iter().copied().collect();
            assert_eq!(distinct.len(), kept.len(), "a slot claimed twice");
            // A vanished thread may have cleared a bit and not read its word again, which
            // only the next free into the word repairs (`Summary_crash.cfg`): free a slot the
            // program held in every word.
            let r0 = host.region_at(0);
            for w in 0..host.layout().leaf_words() {
                free_now(host, r0, w * BITS + 3);
            }
            // Now a fill from the top finds every free slot before it reports none: bits a
            // lost run left set over words it emptied cost a clear, not a missed slot.
            let free = free_set(host, 1);
            let got = claim_run_of(host, 16, 64);
            let got: BTreeSet<u64> = got.iter().map(|&(_, s)| s as u64).collect();
            assert_eq!(got, free, "a fill missed a free slot");
            for &slot in &got {
                free_now(host, r0, slot as u32);
            }
            assert!(host.all_visible(), "a free word is invisible");
        },
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn a_run_lost_with_held_clears_quickly() {
    a_run_lost_with_held_clears(1);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn a_run_lost_with_held_clears_fully() {
    a_run_lost_with_held_clears(2);
}

/// A batch of releases under three summary words, whose walk is shared and deferred, races a
/// run of claims through a cursor and a claim from the top. Every slot ends up free once and
/// every word visible (`Partition`, `Visible`). Preemptions bounded to two (one in the quick
/// run).
fn a_batch_release_races_claims_through_a_cursor(preemptions: usize) {
    let freed = [64 + 1, 64 * 65 + 2, 64 * 2 + 3, 64 + 4];
    explore(
        Config {
            preemptions: Some(preemptions),
            ..Config::ALL
        },
        3,
        || (deep_setup(), Mutex::new(Vec::new())),
        |(host, kept), me| {
            let got = match me {
                0 => {
                    free_batch(host, &freed);
                    Vec::new()
                }
                1 => claim_run(host, 3),
                _ => claim(host, 1),
            };
            kept.lock().unwrap().extend(got);
        },
        |(host, kept), outcome| {
            assert!(outcome.killed.is_empty());
            let kept = kept.lock().unwrap().clone();
            let distinct: BTreeSet<_> = kept.iter().copied().collect();
            assert_eq!(distinct.len(), kept.len(), "a slot claimed twice");
            // Before the releases below, whose healing walks would repair a lost bit.
            assert!(host.all_visible(), "a free word is invisible");
            release_all(host, &kept);
            let mut expected: BTreeSet<u64> = [0, 64 * 64].into_iter().collect();
            expected.extend(freed.iter().map(|&s| s as u64));
            assert_eq!(free_set(host, 1), expected);
            assert!(host.all_visible(), "a free word is invisible");
        },
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn a_batch_release_races_claims_through_a_cursor_quickly() {
    a_batch_release_races_claims_through_a_cursor(1);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn a_batch_release_races_claims_through_a_cursor_fully() {
    a_batch_release_races_claims_through_a_cursor(2);
}

/// A run of claims through a cursor while another thread frees below and above it: nothing is
/// handed out twice or lost, and every word stays visible. Preemptions bounded to three (two in
/// the quick run).
fn a_cursor_run_races_frees_below_and_above_it(preemptions: usize) {
    explore(
        Config {
            preemptions: Some(preemptions),
            ..Config::ALL
        },
        2,
        || (deep_setup(), Mutex::new(Vec::new())),
        |(host, kept), me| {
            if me == 0 {
                let got = claim_run(host, 3);
                kept.lock().unwrap().extend(got);
            } else {
                free_now(host, host.region_at(0), 1);
                free_now(host, host.region_at(0), 64 * 70);
            }
        },
        |(host, kept), outcome| {
            assert!(outcome.killed.is_empty());
            let kept = kept.lock().unwrap().clone();
            // Before the releases below, whose healing walks would repair a lost bit.
            assert!(host.all_visible(), "a free word is invisible");
            release_all(host, &kept);
            let expected: BTreeSet<u64> = [0, 1, 64 * 64, 64 * 70].into_iter().collect();
            assert_eq!(free_set(host, 1), expected);
            assert!(host.all_visible(), "a free word is invisible");
        },
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn a_cursor_run_races_frees_below_and_above_it_quickly() {
    a_cursor_run_races_frees_below_and_above_it(2);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn a_cursor_run_races_frees_below_and_above_it_fully() {
    a_cursor_run_races_frees_below_and_above_it(3);
}

/// The check of the run scenarios below: nothing was claimed twice; every word is visible before
/// any release that would heal a lost bit; a fill alone then finds every free slot before it
/// reports none; and with everything released, exactly `expected` is free.
fn run_check(host: &Host8, regions: usize, kept: &Mutex<Vec<(RegionId, u32)>>, expected: &[u64]) {
    let kept = kept.lock().unwrap().clone();
    let distinct: BTreeSet<_> = kept.iter().copied().collect();
    assert_eq!(distinct.len(), kept.len(), "a slot claimed twice");
    assert!(host.all_visible(), "a free word is invisible");
    let free = free_set(host, regions);
    let got = claim_run_of(host, 4096, 64);
    let found: BTreeSet<u64> = got.iter().map(|&(r, s)| host.global(r, s)).collect();
    assert_eq!(found, free, "a fill missed a free slot");
    assert!(host.all_visible(), "a free word is invisible after a fill");
    release_all(host, &got);
    release_all(host, &kept);
    let expected: BTreeSet<u64> = expected.iter().copied().collect();
    assert_eq!(free_set(host, regions), expected);
    assert!(host.all_visible(), "a free word is invisible");
}

/// `deep_setup` with slot 64 free as well: two words with free slots under the first level-1
/// word, one under the second.
fn deep_run_setup() -> Host8 {
    let host = deep_setup();
    free_now(&host, host.region_at(0), 64);
    host
}

/// Two summary levels: a run empties both words of the first group, so its batch's AND empties
/// the level-1 word and it goes on to clear the level-2 bit, then moves to the second group.
/// Frees land in the words it holds and in the group it moves to, and a claim searches from
/// the top. Preemptions bounded to two (one in the quick run).
fn a_deep_batch_that_goes_up_races_frees(preemptions: usize) {
    explore(
        Config {
            preemptions: Some(preemptions),
            ..Config::ALL
        },
        3,
        || (deep_run_setup(), Mutex::new(Vec::new())),
        |(host, kept), me| {
            let r0 = host.region_at(0);
            let got = match me {
                0 => claim_run_of(host, 3, 1),
                1 => {
                    free_now(host, r0, 1);
                    free_now(host, r0, 65);
                    Vec::new()
                }
                _ => {
                    free_now(host, r0, 64 * 64 + 1);
                    claim(host, 1)
                }
            };
            kept.lock().unwrap().extend(got);
        },
        |(host, kept), outcome| {
            assert!(outcome.killed.is_empty());
            run_check(host, 1, kept, &[0, 1, 64, 65, 64 * 64, 64 * 64 + 1]);
        },
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn a_deep_batch_that_goes_up_races_frees_quickly() {
    a_deep_batch_that_goes_up_races_frees(1);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn a_deep_batch_that_goes_up_races_frees_fully() {
    a_deep_batch_that_goes_up_races_frees(2);
}

/// Two summary levels: two runs empty the same words of one group, so their batches overlap,
/// while a free refills a word between them. Preemptions bounded to two (one in the quick run).
fn deep_runs_with_overlapping_batches(preemptions: usize) {
    explore(
        Config {
            preemptions: Some(preemptions),
            ..Config::ALL
        },
        3,
        || (deep_run_setup(), Mutex::new(Vec::new())),
        |(host, kept), me| {
            let r0 = host.region_at(0);
            let got = match me {
                0 | 1 => claim_run_of(host, 3, 1),
                _ => {
                    free_now(host, r0, 1);
                    free_now(host, r0, 66);
                    Vec::new()
                }
            };
            kept.lock().unwrap().extend(got);
        },
        |(host, kept), outcome| {
            assert!(outcome.killed.is_empty());
            run_check(host, 1, kept, &[0, 1, 64, 66, 64 * 64]);
        },
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn deep_runs_with_overlapping_batches_quickly() {
    deep_runs_with_overlapping_batches(1);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn deep_runs_with_overlapping_batches_fully() {
    deep_runs_with_overlapping_batches(2);
}

/// Two regions with one summary level each: a batch's AND empties a region's top word, whose
/// own bit is in a class word shared with the other region, while frees land in both regions
/// and a second run claims. Preemptions bounded to two (one in the quick run).
fn a_batch_empties_a_region_under_a_shared_class_word(preemptions: usize) {
    explore(
        Config {
            preemptions: Some(preemptions),
            ..Config::ALL
        },
        3,
        || {
            let host = Host8::new(tiny(), class(8), 2);
            let (r0, r1) = (host.region_at(0), host.region_at(1));
            host.carve_all(r0, 256);
            host.carve_all(r1, 256);
            free_now(&host, r0, 0);
            free_now(&host, r0, 64);
            free_now(&host, r1, 0);
            (host, Mutex::new(Vec::new()))
        },
        |(host, kept), me| {
            let (r0, r1) = (host.region_at(0), host.region_at(1));
            let got = match me {
                0 => claim_run_of(host, 3, 1),
                1 => {
                    free_now(host, r0, 1);
                    free_now(host, r1, 70);
                    Vec::new()
                }
                _ => claim_run_of(host, 2, 2),
            };
            kept.lock().unwrap().extend(got);
        },
        |(host, kept), outcome| {
            assert!(outcome.killed.is_empty());
            let first = host.global(host.region_at(1), 0);
            run_check(host, 2, kept, &[0, 1, 64, first, first + 70]);
        },
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn a_batch_empties_a_region_under_a_shared_class_word_quickly() {
    a_batch_empties_a_region_under_a_shared_class_word(1);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn a_batch_empties_a_region_under_a_shared_class_word_fully() {
    a_batch_empties_a_region_under_a_shared_class_word(2);
}

/// Regions whose tree is a single word: a run has no group to batch in, and clears the word's
/// bit, which is a class word's, as soon as it empties the word. Preemptions bounded to two
/// (one in the quick run).
fn runs_over_regions_of_one_word(preemptions: usize) {
    explore(
        Config {
            preemptions: Some(preemptions),
            ..Config::ALL
        },
        3,
        || {
            let host = Host8::new(tiny(), class(256), 2);
            assert_eq!(host.layout().height(), 0);
            let (r0, r1) = (host.region_at(0), host.region_at(1));
            host.carve_all(r0, 8);
            host.carve_all(r1, 8);
            free_now(&host, r0, 0);
            free_now(&host, r1, 0);
            (host, Mutex::new(Vec::new()))
        },
        |(host, kept), me| {
            let (r0, r1) = (host.region_at(0), host.region_at(1));
            let got = match me {
                0 => claim_run_of(host, 3, 1),
                1 => {
                    free_now(host, r0, 1);
                    free_now(host, r1, 1);
                    Vec::new()
                }
                _ => claim_run_of(host, 2, 2),
            };
            kept.lock().unwrap().extend(got);
        },
        |(host, kept), outcome| {
            assert!(outcome.killed.is_empty());
            let first = host.global(host.region_at(1), 0);
            run_check(host, 2, kept, &[0, 1, first, first + 1]);
        },
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn runs_over_regions_of_one_word_quickly() {
    runs_over_regions_of_one_word(1);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn runs_over_regions_of_one_word_fully() {
    runs_over_regions_of_one_word(2);
}

/// A run against a purge's claims, which take exact bits and give them back, and a free.
/// Preemptions bounded to two (one in the quick run).
fn a_run_races_exact_claims(preemptions: usize) {
    explore(
        Config {
            preemptions: Some(preemptions),
            ..Config::ALL
        },
        3,
        || (run_setup(), Mutex::new(Vec::new())),
        |(host, kept), me| {
            let r0 = host.region_at(0);
            let got = match me {
                0 => claim_run_of(host, 3, 2),
                1 => {
                    for (word, bits) in [(1, 0b11), (0, 0b1)] {
                        if tree::claim_exact(host, r0, word, bits) {
                            tree::release(host, r0, word, bits);
                        }
                    }
                    Vec::new()
                }
                _ => {
                    free_now(host, r0, 1);
                    free_now(host, r0, 129);
                    Vec::new()
                }
            };
            kept.lock().unwrap().extend(got);
        },
        |(host, kept), outcome| {
            assert!(outcome.killed.is_empty());
            run_check(host, 1, kept, &[0, 1, 64, 65, 128, 129]);
        },
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn a_run_races_exact_claims_quickly() {
    a_run_races_exact_claims(1);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn a_run_races_exact_claims_fully() {
    a_run_races_exact_claims(2);
}

/// Two fills by one thread over the same words while another thread frees into them in
/// batches, whose walks up are shared and deferred. Preemptions bounded to three (two in the
/// quick run).
fn two_fills_race_batched_releases(preemptions: usize) {
    explore(
        Config {
            preemptions: Some(preemptions),
            ..Config::ALL
        },
        2,
        || (run_setup(), Mutex::new(Vec::new())),
        |(host, kept), me| {
            let got = if me == 0 {
                let mut got = claim_run_of(host, 4, 1);
                got.extend(claim_run_of(host, 4, 1));
                got
            } else {
                free_batch(host, &[1, 66, 129, 192]);
                free_batch(host, &[2, 130]);
                Vec::new()
            };
            kept.lock().unwrap().extend(got);
        },
        |(host, kept), outcome| {
            assert!(outcome.killed.is_empty());
            run_check(host, 1, kept, &[0, 1, 2, 64, 65, 66, 128, 129, 130, 192]);
        },
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn two_fills_race_batched_releases_quickly() {
    two_fills_race_batched_releases(2);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn two_fills_race_batched_releases_fully() {
    two_fills_race_batched_releases(3);
}

/// `a_run_lost_with_held_clears` with two summary levels: a run may vanish holding clears, or
/// between its batch's AND and its re-reads, or on its way up. Afterwards a free into each
/// word makes the word reachable, and a fill finds every free slot. Preemptions bounded to two.
fn a_deep_run_lost_at_any_step(preemptions: usize) {
    explore(
        Config {
            preemptions: Some(preemptions),
            crash: true,
            ..Config::ALL
        },
        3,
        || (deep_run_setup(), Mutex::new(Vec::new())),
        |(host, kept), me| {
            let got = match me {
                0 => {
                    free_now(host, host.region_at(0), 1);
                    claim_run_of(host, 2, 1)
                }
                1 => claim_run_of(host, 3, 1),
                _ => claim_run_of(host, 2, 2),
            };
            kept.lock().unwrap().extend(got);
        },
        |(host, kept), _| {
            let kept: Vec<_> = kept.lock().unwrap().clone();
            let distinct: BTreeSet<_> = kept.iter().copied().collect();
            assert_eq!(distinct.len(), kept.len(), "a slot claimed twice");
            let r0 = host.region_at(0);
            for w in [0, 1, 64] {
                free_now(host, r0, w * BITS + 3);
                assert!(
                    host.path_set(Node::leaf(r0, w)),
                    "word {w} stays invisible after a free into it"
                );
            }
            let free = free_set(host, 1);
            let got = claim_run_of(host, 4096, 64);
            let found: BTreeSet<u64> = got.iter().map(|&(_, s)| s as u64).collect();
            assert_eq!(found, free, "a fill missed a free slot");
        },
    );
}

/// No quick run: one preemption already takes several seconds, and two take minutes.
#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn a_deep_run_lost_at_any_step_fully() {
    a_deep_run_lost_at_any_step(2);
}

/// Claims as a large class makes them, one slot at a time and keeping the summary bits of a
/// word they empty, until a search from the top finds nothing: every slot they got.
fn sweep_keeping_bits(host: &Host8) -> Vec<(RegionId, u32)> {
    let mut out = Vec::new();
    loop {
        let got = claim_with(host, 1, true);
        if got.is_empty() {
            return out;
        }
        out.extend(got);
    }
}

/// The check of the kept-bit scenarios: nothing was claimed twice; every word is visible
/// before any release that would heal a lost bit; claims that keep bits then find every free
/// slot before one reports none, however many bits are set over empty words; and with
/// everything released by single frees, exactly `expected` is free.
fn kept_check(host: &Host8, regions: usize, kept: &Mutex<Vec<(RegionId, u32)>>, expected: &[u64]) {
    let kept = kept.lock().unwrap().clone();
    let distinct: BTreeSet<_> = kept.iter().copied().collect();
    assert_eq!(distinct.len(), kept.len(), "a slot claimed twice");
    assert!(host.all_visible(), "a free word is invisible");
    let free = free_set(host, regions);
    let got = sweep_keeping_bits(host);
    let found: BTreeSet<u64> = got.iter().map(|&(r, s)| host.global(r, s)).collect();
    assert_eq!(found, free, "a claim missed a free slot");
    assert_eq!(got.len(), free.len(), "a slot claimed twice");
    for &(region, slot) in got.iter().chain(&kept) {
        free_one(host, region, slot);
    }
    let expected: BTreeSet<u64> = expected.iter().copied().collect();
    assert_eq!(free_set(host, regions), expected);
    assert!(host.all_visible(), "a free word is invisible");
}

/// A class whose every word is empty under set summary bits, as claims that keep bits leave
/// it: a claim clears them all and reports nothing free, and after one free anywhere the next
/// claim finds that slot. On a tree of two summary levels and on regions of one word.
#[test]
fn a_claim_gets_past_every_kept_bit() {
    let deep = deep_run_setup();
    let flat = Host8::new(tiny(), class(256), 2);
    assert_eq!(flat.layout().height(), 0);
    for r in 0..2 {
        flat.carve_all(flat.region_at(r), 8);
        free_now(&flat, flat.region_at(r), 0);
    }
    for (host, regions, slot) in [(&deep, 1, 64 * 64 + 5), (&flat, 2, 5)] {
        let all = sweep_keeping_bits(host);
        assert_eq!(all.len(), 4 - regions);
        assert!(free_set(host, regions).is_empty());
        // The last claim of the sweep cleared every bit on its way to finding nothing.
        assert_eq!(host.class_top().load(), 0);
        // Keep the bits set over the empty words again, then free one slot under them.
        for &(region, slot) in &all {
            free_one(host, region, slot);
        }
        for _ in 0..all.len() {
            assert_eq!(claim_with(host, 1, true).len(), 1);
        }
        assert_ne!(host.class_top().load(), 0);
        let last = host.region_at(regions - 1);
        free_one(host, last, slot);
        assert_eq!(claim_with(host, 1, true), [(last, slot)]);
        assert!(claim_with(host, 1, true).is_empty());
    }
}

/// `Summary_keep_deep.cfg`: two summary levels, as a large class with many slots. Claims that
/// keep the bits of the words they empty race single frees into those words and others.
/// Preemptions bounded to two (one in the quick run).
fn kept_bits_race_single_frees_on_a_deep_tree(preemptions: usize) {
    explore(
        Config {
            preemptions: Some(preemptions),
            ..Config::ALL
        },
        3,
        || (deep_run_setup(), Mutex::new(Vec::new())),
        |(host, kept), me| {
            let r0 = host.region_at(0);
            let got = match me {
                0 => {
                    let mut got = claim_with(host, 1, true);
                    got.extend(claim_with(host, 1, true));
                    got
                }
                1 => {
                    free_one(host, r0, 1);
                    free_one(host, r0, 64 * 64 + 1);
                    Vec::new()
                }
                _ => {
                    for (region, slot) in claim_with(host, 1, true) {
                        free_one(host, region, slot);
                    }
                    Vec::new()
                }
            };
            kept.lock().unwrap().extend(got);
        },
        |(host, kept), outcome| {
            assert!(outcome.killed.is_empty());
            kept_check(host, 1, kept, &[0, 1, 64, 64 * 64, 64 * 64 + 1]);
        },
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn kept_bits_race_single_frees_on_a_deep_tree_quickly() {
    kept_bits_race_single_frees_on_a_deep_tree(1);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn kept_bits_race_single_frees_on_a_deep_tree_fully() {
    kept_bits_race_single_frees_on_a_deep_tree(2);
}

/// Regions of one word, as most large classes have, all of them empty under set bits at the
/// start: single frees race claims that must get past the kept bits to find them. Preemptions
/// bounded to two (one in the quick run).
fn single_frees_under_kept_bits_of_one_word_regions(preemptions: usize) {
    explore(
        Config {
            preemptions: Some(preemptions),
            ..Config::ALL
        },
        3,
        || {
            let host = Host8::new(tiny(), class(256), 2);
            for r in 0..2 {
                host.carve_all(host.region_at(r), 8);
                free_now(&host, host.region_at(r), 0);
            }
            assert_eq!(sweep_keeping_bits(&host).len(), 2);
            for r in 0..2 {
                free_one(&host, host.region_at(r), 0);
                assert_eq!(claim_with(&host, 1, true).len(), 1);
            }
            (host, Mutex::new(Vec::new()))
        },
        |(host, kept), me| {
            let got = match me {
                0 => {
                    free_one(host, host.region_at(1), 0);
                    claim_with(host, 1, true)
                }
                1 => claim_with(host, 1, true),
                _ => {
                    free_one(host, host.region_at(0), 1);
                    Vec::new()
                }
            };
            kept.lock().unwrap().extend(got);
        },
        |(host, kept), outcome| {
            assert!(outcome.killed.is_empty());
            let last = host.global(host.region_at(1), 0);
            kept_check(host, 2, kept, &[1, last]);
        },
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn single_frees_under_kept_bits_of_one_word_regions_quickly() {
    single_frees_under_kept_bits_of_one_word_regions(1);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn single_frees_under_kept_bits_of_one_word_regions_fully() {
    single_frees_under_kept_bits_of_one_word_regions(2);
}

/// `Summary_keep_crash.cfg`, on two summary levels: a claimer or a single free may vanish at
/// any step, as in a forked child. Afterwards a single free into each word makes the word reachable,
/// and claims find every free slot. The second group starts empty, so a free into it sets a
/// bit at each level, and one lost between two of them leaves the upper bit for the next free
/// to set though the lower is set already. Preemptions bounded to two (one in the quick run).
fn kept_bits_and_single_frees_lost_at_any_step(preemptions: usize) {
    explore(
        Config {
            preemptions: Some(preemptions),
            crash: true,
            ..Config::ALL
        },
        3,
        || {
            let host = deep_run_setup();
            assert!(tree::claim_exact(&host, host.region_at(0), 64, 1));
            (host, Mutex::new(Vec::new()))
        },
        |(host, kept), me| {
            let r0 = host.region_at(0);
            let got = match me {
                0 => {
                    free_one(host, r0, 1);
                    claim_with(host, 1, true)
                }
                1 => {
                    let got = claim_with(host, 1, true);
                    free_one(host, r0, 65);
                    got
                }
                _ => {
                    free_one(host, r0, 64 * 64 + 1);
                    claim_with(host, 1, false)
                }
            };
            kept.lock().unwrap().extend(got);
        },
        |(host, kept), _| {
            let kept: Vec<_> = kept.lock().unwrap().clone();
            let distinct: BTreeSet<_> = kept.iter().copied().collect();
            assert_eq!(distinct.len(), kept.len(), "a slot claimed twice");
            let r0 = host.region_at(0);
            for w in [0, 1, 64] {
                free_one(host, r0, w * BITS + 3);
                assert!(
                    host.path_set(Node::leaf(r0, w)),
                    "word {w} stays invisible after a free into it"
                );
            }
            let free = free_set(host, 1);
            let got = sweep_keeping_bits(host);
            let found: BTreeSet<u64> = got.iter().map(|&(_, s)| s as u64).collect();
            assert_eq!(found, free, "a claim missed a free slot");
        },
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn kept_bits_and_single_frees_lost_at_any_step_quickly() {
    kept_bits_and_single_frees_lost_at_any_step(1);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn kept_bits_and_single_frees_lost_at_any_step_fully() {
    kept_bits_and_single_frees_lost_at_any_step(2);
}

/// Frees `slots` as a launder does: gathered by word in a window of `SPAN` words and a map,
/// then one OR per word and one debt per summary word (`Gather`, `Release::owe_leaves`).
fn free_gathered<const SPAN: usize>(host: &Host8, slots: &[(RegionId, u32)]) {
    let key = |region: RegionId, slot: u32| (region.index() as u64) << 32 | (slot / BITS) as u64;
    let Some(&(region, slot)) = slots.first() else {
        return;
    };
    let mut release = tree::Release::new(host, region);
    let mut gather = Gather::<32, SPAN>::new(key(region, slot));
    for &(region, slot) in slots {
        gather.end_one(&mut release, key(region, slot), 1 << (slot % BITS));
    }
    gather.finish(&mut release);
    release.finish();
}

/// A launder whose words owe merged debts, on two regions of two summary levels: two words in
/// its window under one summary word, one in the map under another, and two in the map in the
/// other region under one. It races a run of claims and a claim from the top. Preemptions
/// bounded to two (one in the quick run).
fn merged_debts_race_claims_on_deep_trees(preemptions: usize) {
    explore(
        Config {
            preemptions: Some(preemptions),
            ..Config::ALL
        },
        3,
        || {
            let host = Host8::new(standard(), class(8), 2);
            assert_eq!(host.layout().height(), 2);
            for r in 0..2 {
                host.carve_all(host.region_at(r), host.layout().slots());
            }
            free_now(&host, host.region_at(0), 0);
            free_now(&host, host.region_at(0), 64 * 64);
            (host, Mutex::new(Vec::new()))
        },
        |(host, kept), me| {
            let (r0, r1) = (host.region_at(0), host.region_at(1));
            let got = match me {
                0 => {
                    let slots = [(r0, 65), (r0, 1), (r0, 64 * 64 + 2), (r1, 3), (r1, 64 + 3)];
                    free_gathered::<64>(host, &slots);
                    Vec::new()
                }
                1 => claim_run_of(host, 3, 1),
                _ => claim(host, 1),
            };
            kept.lock().unwrap().extend(got);
        },
        |(host, kept), outcome| {
            assert!(outcome.killed.is_empty());
            let first = host.global(host.region_at(1), 0);
            let expected = [0, 1, 65, 64 * 64, 64 * 64 + 2, first + 3, first + 67];
            run_check(host, 2, kept, &expected);
        },
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn merged_debts_race_claims_on_deep_trees_quickly() {
    merged_debts_race_claims_on_deep_trees(1);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn merged_debts_race_claims_on_deep_trees_fully() {
    merged_debts_race_claims_on_deep_trees(2);
}

/// A launder into regions of one word, whose bits are a class word's: the window holds one
/// region's word and the map the other's, and their debts merge in the class word. It races a
/// run of claims and a claim that keeps bits. Preemptions bounded to two (one in the quick
/// run).
fn merged_debts_race_claims_on_one_word_regions(preemptions: usize) {
    explore(
        Config {
            preemptions: Some(preemptions),
            ..Config::ALL
        },
        3,
        || {
            let host = Host8::new(tiny(), class(256), 2);
            for r in 0..2 {
                host.carve_all(host.region_at(r), 8);
            }
            free_now(&host, host.region_at(0), 0);
            (host, Mutex::new(Vec::new()))
        },
        |(host, kept), me| {
            let (r0, r1) = (host.region_at(0), host.region_at(1));
            let got = match me {
                0 => {
                    free_gathered::<64>(host, &[(r1, 1), (r0, 1), (r1, 2)]);
                    Vec::new()
                }
                1 => claim_run_of(host, 2, 1),
                _ => claim_with(host, 1, true),
            };
            kept.lock().unwrap().extend(got);
        },
        |(host, kept), outcome| {
            assert!(outcome.killed.is_empty());
            let first = host.global(host.region_at(1), 0);
            run_check(host, 2, kept, &[0, 1, first + 1, first + 2]);
        },
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn merged_debts_race_claims_on_one_word_regions_quickly() {
    merged_debts_race_claims_on_one_word_regions(1);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn merged_debts_race_claims_on_one_word_regions_fully() {
    merged_debts_race_claims_on_one_word_regions(2);
}

/// Frees `slots` as a scattered launder does: each with its own OR, owing its summary bit
/// with the bit read before it is set (`Look`), the debts merged per summary word (`Direct`).
///
/// With `wide`, as a batch judged wide does: each slot's summary word paid at once, only what
/// that owes the level above merged.
fn free_direct(host: &Host8, slots: &[(RegionId, u32)], wide: bool) {
    let Some(&(region, _)) = slots.first() else {
        return;
    };
    let mut release = tree::Release::new(host, region);
    let mut direct = Direct::new();
    for &(region, slot) in slots {
        let leaves = release.leaves(region, slot / BITS + 1);
        if wide {
            let levels = release.summary_levels(region);
            direct.free_slot_wide(&mut release, leaves, &levels, slot);
        } else {
            direct.free_slot(&mut release, leaves, region, slot);
        }
    }
    direct.finish(&mut release);
    release.finish();
}

/// `Summary_look_deep.cfg`: a scattered launder on two regions of two summary levels, into
/// words that were empty (so with `must` the summary bits would be set unread) under two
/// summary words of one region and one of the other, races a run of claims and a claim from
/// the top. Preemptions bounded to two (one in the quick run).
fn direct_frees_race_claims_on_deep_trees(preemptions: usize, wide: bool) {
    explore(
        Config {
            preemptions: Some(preemptions),
            ..Config::ALL
        },
        3,
        || {
            let host = Host8::new(standard(), class(8), 2);
            assert_eq!(host.layout().height(), 2);
            for r in 0..2 {
                host.carve_all(host.region_at(r), host.layout().slots());
            }
            free_now(&host, host.region_at(0), 0);
            free_now(&host, host.region_at(0), 64 * 64);
            (host, Mutex::new(Vec::new()))
        },
        |(host, kept), me| {
            let (r0, r1) = (host.region_at(0), host.region_at(1));
            let got = match me {
                0 => {
                    let slots = [(r0, 65), (r1, 3), (r0, 64 * 64 + 2), (r0, 1), (r1, 64 + 3)];
                    free_direct(host, &slots, wide);
                    Vec::new()
                }
                1 => claim_run_of(host, 3, 1),
                _ => claim(host, 1),
            };
            kept.lock().unwrap().extend(got);
        },
        |(host, kept), outcome| {
            assert!(outcome.killed.is_empty());
            let first = host.global(host.region_at(1), 0);
            let expected = [0, 1, 65, 64 * 64, 64 * 64 + 2, first + 3, first + 67];
            run_check(host, 2, kept, &expected);
        },
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn direct_frees_race_claims_on_deep_trees_quickly() {
    direct_frees_race_claims_on_deep_trees(1, false);
}

#[test]
#[cfg_attr(miri, ignore)]
fn wide_direct_frees_race_claims_on_deep_trees_quickly() {
    direct_frees_race_claims_on_deep_trees(1, true);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn wide_direct_frees_race_claims_on_deep_trees_fully() {
    direct_frees_race_claims_on_deep_trees(2, true);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn direct_frees_race_claims_on_deep_trees_fully() {
    direct_frees_race_claims_on_deep_trees(2, false);
}

/// `Summary_look_crash.cfg`, on two summary levels: a scattered launder, a claimer and a run of
/// claims may vanish at any step, as in a forked child. Afterwards a single free into each
/// word makes it reachable, and claims find every free slot. Preemptions bounded to two (one
/// in the quick run).
fn direct_frees_lost_at_any_step(preemptions: usize, wide: bool) {
    explore(
        Config {
            preemptions: Some(preemptions),
            crash: true,
            ..Config::ALL
        },
        3,
        || {
            let host = deep_run_setup();
            (host, Mutex::new(Vec::new()))
        },
        |(host, kept), me| {
            let r0 = host.region_at(0);
            let got = match me {
                0 => {
                    free_direct(host, &[(r0, 65), (r0, 64 * 64 + 1), (r0, 1)], wide);
                    Vec::new()
                }
                1 => claim_run_of(host, 2, 1),
                _ => claim(host, 1),
            };
            kept.lock().unwrap().extend(got);
        },
        |(host, kept), _| {
            let kept: Vec<_> = kept.lock().unwrap().clone();
            let distinct: BTreeSet<_> = kept.iter().copied().collect();
            assert_eq!(distinct.len(), kept.len(), "a slot claimed twice");
            let r0 = host.region_at(0);
            for w in [0, 1, 64] {
                free_one(host, r0, w * BITS + 3);
                assert!(
                    host.path_set(Node::leaf(r0, w)),
                    "word {w} stays invisible after a free into it"
                );
            }
        },
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn direct_frees_lost_at_any_step_quickly() {
    direct_frees_lost_at_any_step(1, false);
}

#[test]
#[cfg_attr(miri, ignore)]
fn wide_direct_frees_lost_at_any_step_quickly() {
    direct_frees_lost_at_any_step(1, true);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn wide_direct_frees_lost_at_any_step_fully() {
    direct_frees_lost_at_any_step(2, true);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn direct_frees_lost_at_any_step_fully() {
    direct_frees_lost_at_any_step(2, false);
}

/// `Summary_heal_crash.cfg`: as `summary_three_threads_bounded`, but threads 1 and 2 may vanish
/// at any step, as in a forked child. What a vanished thread held is lost; nothing is ever
/// handed out twice; and once the survivor is done, a free into any word makes that word
/// reachable (the healing free), even where a vanished thread left a summary bit clear.
/// Preemptions bounded to two (one in the quick run).
fn summary_crash_then_heal(preemptions: usize) {
    explore(
        Config {
            preemptions: Some(preemptions),
            crash: true,
            ..Config::ALL
        },
        3,
        summary_setup,
        |host, me| {
            let r0 = host.region_at(0);
            match me {
                0 => {
                    let got = claim(host, 1);
                    release_all(host, &got);
                }
                1 => {
                    let got = claim(host, 2);
                    release_all(host, &got);
                }
                _ => {
                    free_now(host, r0, 128);
                    let got = claim(host, 1);
                    release_all(host, &got);
                }
            }
        },
        |host, _| {
            // Slot 2 of each word is held by the program throughout: free it now, from the one
            // thread left, and check that its word can be found.
            for r in 0..2 {
                let region = host.region_at(r);
                for w in 0..host.layout().leaf_words() {
                    let slot = w * BITS + 2;
                    free_now(host, region, slot);
                    assert!(
                        host.path_set(Node::leaf(region, w)),
                        "word {w} of region {r} stays invisible after a free into it"
                    );
                }
            }
        },
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn summary_crash_then_heal_quickly() {
    summary_crash_then_heal(1);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn summary_crash_then_heal_fully() {
    summary_crash_then_heal(2);
}

/// The carve protocol, which no TLA+ model covers: two threads carve from a region of eight
/// slots while committing as they go, and each slot is handed out exactly once. Preemptions
/// bounded to three; `carving_every_schedule` explores all 1.5 million schedules on demand.
#[test]
#[cfg_attr(miri, ignore)]
fn carving_threads_get_disjoint_slots() {
    carving(Config {
        preemptions: Some(3),
        ..Config::ALL
    });
}

/// `carving_threads_get_disjoint_slots` under every schedule: a few million of them, a few
/// minutes in a debug build.
#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn carving_every_schedule() {
    carving(Config {
        limit: 20_000_000,
        ..Config::ALL
    });
}

fn carving(config: Config) {
    let geometry = tiny();
    explore(
        config,
        2,
        || {
            // 256-byte slots: eight to a 2 KiB region.
            let host = Host8::new(geometry, class(256), 1);
            assert_eq!(host.layout().slots(), 8);
            (host, Mutex::new(Vec::new()))
        },
        |(host, carved), me| {
            let region = host.region_at(0);
            for _ in 0..3 {
                match region::carve(host, region, 2 + me as u32) {
                    Carve::Got { first, count } => {
                        carved.lock().unwrap().extend(first..first + count);
                    }
                    Carve::Exhausted => break,
                    Carve::Failed => panic!("commit failed"),
                }
            }
        },
        |(host, carved), _| {
            let mut slots = carved.lock().unwrap().clone();
            slots.sort_unstable();
            assert_eq!(slots, (0..8).collect::<Vec<u32>>());
            let region = host.region_at(0);
            assert!(host.free_slots(region).is_empty(), "carving frees nothing");
            let header = host.grow(region).load();
            let grow = super::layout::Grow::unpack(header);
            assert_eq!(grow.carved, 8);
            assert!(grow.committed as usize * geometry.geometry().page_size() >= 8 * 256);
        },
    );
}

/// A purge over the whole carved part of region 0, top down.
fn purge_pass(host: &Host8) -> usize {
    let region = host.region_at(0);
    let layout = *host.layout();
    let carved = super::layout::Grow::unpack(host.grow(region).load()).carved;
    let huge_from = super::layout::Hints::unpack(host.hints(region).load()).huge_from;
    let mut cursor = 0;
    let mut purged = 0;
    loop {
        let next = match layout.unit_below(carved, huge_from, cursor) {
            Step::Done => 0,
            Step::Skip(page) => page,
            Step::Unit(unit) => {
                if let (Purge::Done { .. }, _) = region::purge_unit(host, region, unit) {
                    purged += 1;
                }
                unit.start
            }
        };
        if next == 0 {
            return purged;
        }
        cursor = next;
    }
}

/// An allocation as the back end does one: claim, then settle the zero and clean bits.
/// Returns the slot and whether it was said to be zero.
fn alloc_one(host: &Host8) -> Option<(RegionId, u32, bool)> {
    let taken = tree::claim(host, 1, false)?;
    let settled = region::settle(host, taken.region, taken.word, taken.bits);
    let slot = taken.word * BITS + taken.bits.trailing_zeros();
    Some((taken.region, slot, settled.zeroed != 0))
}

/// `Purge.tla`'s program thread: allocate, check a zeroed answer, write, free.
fn program(host: &Host8) {
    if let Some((region, slot, zeroed)) = alloc_one(host) {
        if zeroed {
            assert!(
                host.is_zero(region, slot),
                "a zeroed allocation reads non-zero"
            );
        }
        host.write(region, slot);
        free_now(host, region, slot);
    }
}

/// Sets up `slots` carved slots of `size` bytes in `geometry`, all written and free except
/// `held`, which the program holds.
fn purge_setup(geometry: Plan, size: usize, slots: u32, held: &[u32]) -> Host8 {
    let host = Host8::new(geometry, class(size), 1);
    let region = host.region_at(0);
    host.carve_all(region, slots);
    for slot in 0..slots {
        host.write(region, slot);
        if !held.contains(&slot) {
            free_now(&host, region, slot);
        }
    }
    host
}

/// `ZeroBitSound`: every set zero bit is a free slot that reads as zero.
fn zero_bits_sound(host: &Host8) {
    let region = host.region_at(0);
    let layout = *host.layout();
    let free: BTreeSet<u32> = host.free_slots(region).into_iter().collect();
    for w in 0..layout.leaf_words() {
        let Some(at) = layout.zero_word(w) else {
            return;
        };
        let mut bits = host.meta(region, at).load();
        while bits != 0 {
            let slot = w * BITS + bits.trailing_zeros();
            bits &= bits - 1;
            assert!(
                free.contains(&slot),
                "slot {slot} has a zero bit but is not free"
            );
            assert!(
                host.is_zero(region, slot),
                "slot {slot} has a zero bit but holds data"
            );
        }
    }
}

/// What one thread of a purge scenario does.
#[derive(Clone, Copy, Debug)]
enum Role {
    /// A top-down purge pass over the carved part of the region.
    Purge,
    /// Allocate a slot, check a zeroed answer, write it, free it.
    Program,
    /// Free the slots the program held at the start, then act as `Program`.
    FreeHeld,
    /// Carve two more slots, write them and free them: a carve racing a purge.
    Carve,
}

fn purge_scenario(geometry: Plan, size: usize, slots: u32, held: &[u32], bound: usize) {
    purge_roles(
        geometry,
        size,
        slots,
        held,
        bound,
        false,
        [Role::Purge, Role::Program, Role::FreeHeld],
    );
}

/// A purge scenario with the given thread roles; with `huge`, the whole region is its
/// huge-page part, so purges work in whole huge pages.
fn purge_roles(
    geometry: Plan,
    size: usize,
    slots: u32,
    held: &[u32],
    bound: usize,
    huge: bool,
    roles: [Role; 3],
) {
    let held: Vec<u32> = held.to_vec();
    explore(
        Config {
            preemptions: Some(bound),
            ..Config::ALL
        },
        3,
        || {
            let host = purge_setup(geometry, size, slots, &held);
            if huge {
                let hints = super::layout::Hints {
                    huge_from: Some(0),
                    cursor: 0,
                };
                let region = host.region_at(0);
                host.hints(region).fetch_or(hints.pack());
            }
            host
        },
        |host, me| match roles[me] {
            Role::Purge => {
                purge_pass(host);
            }
            Role::Program => program(host),
            Role::FreeHeld => {
                for &slot in &held {
                    free_now(host, host.region_at(0), slot);
                }
                program(host);
            }
            Role::Carve => {
                let region = host.region_at(0);
                if let Carve::Got { first, count } = region::carve(host, region, 2) {
                    for slot in first..first + count {
                        assert!(host.is_zero(region, slot), "a carved slot reads non-zero");
                        host.write(region, slot);
                        free_now(host, region, slot);
                    }
                }
            }
        },
        |host, _| {
            let region = host.region_at(0);
            let carved = super::layout::Grow::unpack(host.grow(region).load()).carved;
            let mut expected: BTreeSet<u32> = (0..carved).collect();
            if !roles.iter().any(|r| matches!(r, Role::FreeHeld)) {
                for slot in &held {
                    expected.remove(slot);
                }
            }
            let free: BTreeSet<u32> = host.free_slots(region).into_iter().collect();
            assert_eq!(free, expected, "a slot was lost or invented");
            assert!(host.all_visible());
            zero_bits_sound(host);
        },
    );
}

/// The huge-page part: 32-byte slots on 64-byte pages, purged in whole 256-byte huge pages.
fn purge_in_huge_pages(bound: usize) {
    purge_roles(
        tiny(),
        32,
        16,
        &[3],
        bound,
        true,
        [Role::Purge, Role::Program, Role::FreeHeld],
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn purge_in_huge_pages_quickly() {
    purge_in_huge_pages(1);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn purge_in_huge_pages_fully() {
    purge_in_huge_pages(2);
}

/// Two purgers over the same units, against a program.
fn two_purgers_on_the_same_units(bound: usize) {
    purge_roles(
        tiny(),
        48,
        4,
        &[1],
        bound,
        false,
        [Role::Purge, Role::Purge, Role::FreeHeld],
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn two_purgers_on_the_same_units_quickly() {
    two_purgers_on_the_same_units(1);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn two_purgers_on_the_same_units_fully() {
    two_purgers_on_the_same_units(2);
}

/// A carve racing a purge: the purge reads the watermark once and must never return a page a
/// carve hands out. The carver checks that its slots read as zero.
fn a_carve_racing_a_purge(bound: usize) {
    purge_roles(
        tiny(),
        48,
        3,
        &[],
        bound,
        false,
        [Role::Purge, Role::Carve, Role::Program],
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn a_carve_racing_a_purge_quickly() {
    a_carve_racing_a_purge(2);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn a_carve_racing_a_purge_fully() {
    a_carve_racing_a_purge(3);
}

/// The page a claim of `slot` raises its region's frontier to (`region::reach`).
fn reach_page(host: &Host8, slot: u32) -> u32 {
    let layout = host.layout();
    layout.slot_bytes(slot + 1).div_ceil(layout.page_size()) as u32
}

/// Claims across an interval boundary race on their region's frontier (`region::reach`). From
/// `run_setup`, with the frontier holding `now` 2 and `last` 9 under interval E - 1: thread 0
/// claims two slots through a run, as a fill does, and records the higher in interval E;
/// thread 1 claims one and records it in E + 1, as an allocation that read the interval
/// number after it moved; thread 2 claims one and records it in E, as one that read it
/// before. In every schedule, read in E + 1:
///
/// - no claim is hidden: the floor is at or above the page of every slot a thread claimed and
///   recorded, whichever record landed first, including a record of E that finds the word
///   already of E + 1;
/// - nothing is invented or kept: the floor is zero or a claim's page, and the earlier
///   intervals' 2 and 9 are gone;
/// - nothing sticks: read two intervals later, the word gives nothing.
///
/// With `crash`, one thread may vanish at any step, as in a fork: a claim lost between claim
/// and record is unrecorded (its slots are lost with it, so no purge can take them), and the
/// word is one of the values above all the same.
fn claims_race_across_an_interval(preemptions: Option<usize>, crash: bool) {
    use super::layout::Frontier;
    const E: u64 = 7;
    explore(
        Config {
            preemptions,
            crash,
            ..Config::ALL
        },
        3,
        || {
            let host = run_setup();
            let r0 = host.region_at(0);
            let format = host.layout().frontier_format();
            let earlier = Frontier {
                epoch: E - 1,
                now: 2,
                last: 9,
            };
            host.frontier(r0).store(format.pack(earlier));
            (host, Mutex::new(Vec::new()))
        },
        |(host, recorded), me| {
            let (got, epoch) = match me {
                0 => (claim_run_of(host, 2, 1), E),
                1 => (claim(host, 1), E + 1),
                _ => (claim(host, 1), E),
            };
            if let Some(&(region, high)) = got.iter().max_by_key(|&&(_, slot)| slot) {
                region::reach(host, region, high, epoch);
            }
            recorded.lock().unwrap().extend(got);
        },
        |(host, recorded), _| {
            let r0 = host.region_at(0);
            let floor = |epoch| region::floor(host, r0, epoch);
            let recorded = recorded.lock().unwrap();
            for &(_, slot) in recorded.iter() {
                let page = reach_page(host, slot);
                assert!(
                    floor(E + 1) >= page,
                    "the floor {} hides page {page}",
                    floor(E + 1)
                );
            }
            // Pages any thread could have recorded, finished or not.
            let possible: BTreeSet<u32> = [0, 64, 65, 128]
                .into_iter()
                .map(|slot| reach_page(host, slot))
                .collect();
            let now = floor(E + 1);
            assert!(now == 0 || possible.contains(&now), "{now}");
            assert_eq!(floor(E + 3), 0, "a frontier stuck");
        },
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn claims_race_across_an_interval_quickly() {
    claims_race_across_an_interval(Some(2), false);
    claims_race_across_an_interval(Some(1), true);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn claims_race_across_an_interval_fully() {
    claims_race_across_an_interval(Some(3), false);
    claims_race_across_an_interval(Some(2), true);
}

/// `Purge.cfg`: two 32-byte slots to a 64-byte page.
fn purge_aligned_slots(bound: usize) {
    purge_scenario(tiny(), 32, 4, &[1], bound);
}

#[test]
#[cfg_attr(miri, ignore)]
fn purge_aligned_slots_quickly() {
    purge_aligned_slots(1);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn purge_aligned_slots_fully() {
    purge_aligned_slots(2);
}

/// `Purge_straddle.cfg`: 48-byte slots on 64-byte pages, so slot 1 straddles pages 0 and 1.
fn purge_straddling_slots(bound: usize) {
    purge_scenario(tiny(), 48, 3, &[1], bound);
}

#[test]
#[cfg_attr(miri, ignore)]
fn purge_straddling_slots_quickly() {
    purge_straddling_slots(1);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn purge_straddling_slots_fully() {
    purge_straddling_slots(2);
}

/// `Purge_big.cfg` with the zero bitmap: 64 KiB slots, two 32 KiB pages each, a large class.
fn purge_big_slots_and_zero_bits(bound: usize) {
    let host = Host8::new(big_plan(), class(64 << 10), 1);
    assert!(host.layout().zero_word(0).is_some());
    purge_scenario(big_plan(), 64 << 10, 2, &[0], bound);
}

#[test]
#[cfg_attr(miri, ignore)]
fn purge_big_slots_and_zero_bits_quickly() {
    purge_big_slots_and_zero_bits(1);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn purge_big_slots_and_zero_bits_fully() {
    purge_big_slots_and_zero_bits(2);
}

/// A large class whose slots straddle pages (48 KiB slots on 32 KiB pages): zero bits are set
/// only for slots wholly inside a purged unit.
fn purge_big_straddling_slots_and_zero_bits(bound: usize) {
    purge_scenario(big_plan(), 48 << 10, 3, &[2], bound);
}

#[test]
#[cfg_attr(miri, ignore)]
fn purge_big_straddling_slots_and_zero_bits_quickly() {
    purge_big_straddling_slots_and_zero_bits(1);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn purge_big_straddling_slots_and_zero_bits_fully() {
    purge_big_straddling_slots_and_zero_bits(2);
}

/// A purge alone, 48 KiB slots on 32 KiB pages with slot 2 held: the top unit (page 3) touches
/// slot 2 and is busy; the next (pages 1 and 2) holds slot 1 wholly and the start of nothing
/// else, so slot 1 gets a zero bit; the last (page 0) holds only part of slot 0, so slot 0,
/// though all its pages are now purged, gets none.
#[test]
fn purge_sets_zero_bits_only_inside() {
    let host = purge_setup(big_plan(), 48 << 10, 3, &[2]);
    let layout = *host.layout();
    let region = host.region_at(0);
    assert_eq!(layout.carved_pages(3), 4);
    assert_eq!(layout.touching(Unit { start: 1, end: 3 }), (0, 1));
    assert_eq!(purge_pass(&host), 2);
    let zero = host.meta(region, layout.zero_word(0).unwrap()).load();
    assert_eq!(zero & 0b111, 0b010);
    zero_bits_sound(&host);
    assert!(host.is_zero(region, 0) && host.is_zero(region, 1));
    assert!(!host.is_zero(region, 2));
}

/// Two regions of two summary levels in which words 1 and 64 of region 0 and word 0 of region
/// 1 are empty under summary bits set at every level, as claims that keep the bits leave
/// them, and nothing is free. The slots the claims took (64 and `64 * 64` of region 0, 0 of
/// region 1) are the program's.
fn stale_bits_setup() -> Host8 {
    let host = Host8::new(standard(), class(8), 2);
    assert_eq!(host.layout().height(), 2);
    for r in 0..2 {
        host.carve_all(host.region_at(r), host.layout().slots());
    }
    let (r0, r1) = (host.region_at(0), host.region_at(1));
    // Highest first: a search stops at the lowest word with a free slot, so it never reaches
    // the bits already left over the empty words above, which it would clear.
    for (region, slot) in [(r1, 0), (r0, 64 * 64), (r0, 64)] {
        free_now(&host, region, slot);
        assert_eq!(claim_with(&host, 1, true), [(region, slot)]);
    }
    assert!(free_set(&host, 2).is_empty());
    for (region, word) in [(r0, 1), (r0, 64), (r1, 0)] {
        assert!(host.path_set(Node::leaf(region, word)));
    }
    host
}

/// Review of the direct path (`Look` against clears that go up the tree). A scattered launder
/// frees into words that are empty under set summary bits, so at every level it reads a bit
/// that is set and writes nothing, while searches from the top find those words empty, clear
/// their bits level by level, and read each word again. `Summary_look_deep.cfg` with the
/// stale bits of `Summary_keep_deep.cfg`, on the production code.
fn direct_frees_under_stale_bits_race_clears(preemptions: usize, wide: bool) {
    explore(
        Config {
            preemptions: Some(preemptions),
            ..Config::ALL
        },
        3,
        || (stale_bits_setup(), Mutex::new(Vec::new())),
        |(host, kept), me| {
            let (r0, r1) = (host.region_at(0), host.region_at(1));
            let got = match me {
                0 => {
                    free_direct(host, &[(r0, 64), (r1, 0), (r0, 64 * 64), (r0, 65)], wide);
                    Vec::new()
                }
                1 => claim(host, 1),
                _ => claim_run_of(host, 2, 1),
            };
            kept.lock().unwrap().extend(got);
        },
        |(host, kept), outcome| {
            assert!(outcome.killed.is_empty());
            let first = host.global(host.region_at(1), 0);
            run_check(host, 2, kept, &[64, 65, 64 * 64, first]);
        },
    );
}

/// One preemption, which takes most of a minute for this scenario and the next together. Two
/// take about twelve minutes, and passed when the direct path was reviewed.
#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn direct_frees_under_stale_bits_race_clears_fully() {
    direct_frees_under_stale_bits_race_clears(1, false);
}

/// The same with each slot's walk made at once, as a batch judged wide makes it.
#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn wide_direct_frees_under_stale_bits_race_clears_fully() {
    direct_frees_under_stale_bits_race_clears(1, true);
}

/// The same with a thread lost at any step, as in a forked child: afterwards a scattered
/// launder into each word, which is the direct path's own healing walk (a look at each level,
/// a write only where the bit is clear), makes the word reachable.
fn direct_frees_heal_what_a_fork_left(preemptions: usize, wide: bool) {
    explore(
        Config {
            preemptions: Some(preemptions),
            crash: true,
            ..Config::ALL
        },
        3,
        || (stale_bits_setup(), Mutex::new(Vec::new())),
        |(host, kept), me| {
            let (r0, r1) = (host.region_at(0), host.region_at(1));
            let got = match me {
                0 => claim(host, 1),
                1 => {
                    free_direct(host, &[(r0, 64), (r1, 0), (r0, 64 * 64), (r0, 65)], wide);
                    Vec::new()
                }
                _ => claim_run_of(host, 2, 1),
            };
            kept.lock().unwrap().extend(got);
        },
        |(host, kept), _| {
            let kept: Vec<_> = kept.lock().unwrap().clone();
            let distinct: BTreeSet<_> = kept.iter().copied().collect();
            assert_eq!(distinct.len(), kept.len(), "a slot claimed twice");
            let (r0, r1) = (host.region_at(0), host.region_at(1));
            free_direct(host, &[(r0, 64 + 7), (r1, 7), (r0, 64 * 64 + 7)], wide);
            for (region, word) in [(r0, 1), (r1, 0), (r0, 64)] {
                assert!(
                    host.path_set(Node::leaf(region, word)),
                    "word {word} stays invisible after a scattered launder into it"
                );
            }
        },
    );
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn direct_frees_heal_what_a_fork_left_fully() {
    direct_frees_heal_what_a_fork_left(1, false);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn wide_direct_frees_heal_what_a_fork_left_fully() {
    direct_frees_heal_what_a_fork_left(1, true);
}

/// Review of the single free that looks at every level (`tree::release_one`, main `5c38865`).
/// The step it takes that `Up` in `tla/Summary.tla` does not: its OR of a level-1 bit finds
/// the level-1 word empty, and it still reads the level-2 bit before setting it, while the
/// thread that emptied that word is clearing the same bit on its way up
/// (`tla/SummaryLookUp.tla`). Two summary levels and the class's two; the claim is a purge's
/// (`claim_exact`), which clears and reads again at every level. With `alone`, nothing else
/// in the class is free, so the clears go on through the class word to its top word and the
/// free looks at each of those behind an OR that found the word below empty. Every schedule,
/// or those with at most `preemptions` preemptions.
fn a_single_free_looks_above_a_word_it_found_empty(alone: bool, preemptions: Option<usize>) {
    explore(
        Config {
            preemptions,
            ..Config::ALL
        },
        2,
        || {
            let host = Host8::new(standard(), class(8), 1);
            assert_eq!(host.layout().height(), 2);
            let r0 = host.region_at(0);
            host.carve_all(r0, host.layout().slots());
            free_now(&host, r0, 64 * 64);
            if !alone {
                free_now(&host, r0, 0);
            }
            host
        },
        |host, me| {
            let r0 = host.region_at(0);
            if me == 0 {
                assert!(tree::claim_exact(host, r0, 64, 1));
            } else {
                free_one(host, r0, 65 * 64 + 1);
            }
        },
        |host, outcome| {
            assert!(outcome.killed.is_empty());
            let mut expected: BTreeSet<u64> = [65 * 64 + 1].into_iter().collect();
            if !alone {
                expected.insert(0);
            }
            assert_eq!(free_set(host, 1), expected);
            assert!(host.all_visible(), "a free word is invisible");
        },
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn a_single_free_looks_above_a_word_it_found_empty_quickly() {
    a_single_free_looks_above_a_word_it_found_empty(false, Some(3));
    a_single_free_looks_above_a_word_it_found_empty(true, Some(3));
}

/// Every schedule: about ten seconds beside other free slots and forty alone.
#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn a_single_free_looks_above_a_word_it_found_empty_fully() {
    a_single_free_looks_above_a_word_it_found_empty(false, None);
    a_single_free_looks_above_a_word_it_found_empty(true, None);
}

/// `direct_frees_under_stale_bits_race_clears` for single frees: they land in words that are
/// empty under summary bits set at every level, so each reads bits that are set and writes
/// nothing above its leaf word, while a search from the top and a run find those words empty,
/// clear their bits level by level and read each word again.
fn single_frees_under_stale_bits_race_clears(preemptions: usize) {
    explore(
        Config {
            preemptions: Some(preemptions),
            ..Config::ALL
        },
        3,
        || (stale_bits_setup(), Mutex::new(Vec::new())),
        |(host, kept), me| {
            let (r0, r1) = (host.region_at(0), host.region_at(1));
            let got = match me {
                0 => {
                    for (region, slot) in [(r0, 64), (r1, 0), (r0, 64 * 64), (r0, 65)] {
                        free_one(host, region, slot);
                    }
                    Vec::new()
                }
                1 => claim(host, 1),
                _ => claim_run_of(host, 2, 1),
            };
            kept.lock().unwrap().extend(got);
        },
        |(host, kept), outcome| {
            assert!(outcome.killed.is_empty());
            let first = host.global(host.region_at(1), 0);
            run_check(host, 2, kept, &[64, 65, 64 * 64, first]);
        },
    );
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn single_frees_under_stale_bits_race_clears_fully() {
    single_frees_under_stale_bits_race_clears(1);
}

/// The fine count the age walk scenarios look at, and a stamp that is a hold old by it.
const AGE_COUNT: u32 = 1000;
const AGE_OLD: u32 = AGE_COUNT - super::policy::HOLD_STEPS - 1;

/// Writes `slot`'s size word as a tenant of `size` bytes whose free leaves it resident leaves
/// it: its size kept, and the fine count it was freed at.
fn stamp(host: &Host8, region: RegionId, slot: u32, size: usize, freed: u32) {
    let recorded = super::layout::Recorded {
        size,
        huge: false,
        unwritten: false,
        outgrown: false,
        freed,
    };
    region::record(host, region, slot, recorded);
}

/// A large tenant as the back end serves one: claim a slot, settle it with the size word its
/// last tenant left, return the slack past both extents, write it, stamp it young and free it.
fn large_tenant(host: &Host8) {
    let Some(taken) = tree::claim(host, 1, true) else {
        return;
    };
    let region = taken.region;
    let slot = taken.word * BITS + taken.bits.trailing_zeros();
    let last = region::recorded(host, region, slot);
    let size = host.layout().size() / 3;
    let settled = region::settle_slot(host, region, slot, size, last);
    if settled.zeroed != 0 {
        assert!(host.is_zero(region, slot), "a zeroed claim reads non-zero");
    }
    if let Some(slack) = settled.slack {
        let page = host.layout().page_size();
        let from = slack.start as usize * page - host.layout().slot_bytes(slot);
        let len = (slack.end - slack.start) as usize * page;
        // SAFETY: the tenant holds the slot and the range is past what it writes.
        if unsafe { region::purge_part(host, region, slot, from, len) } {
            region::mark_clean(host, region, slot, from, len);
        }
    }
    host.write(region, slot);
    stamp(host, region, slot, size, AGE_COUNT);
    tree::release_one(host, region, slot / BITS, slot % BITS);
}

/// The hold's walk of a young region by its slots' own ages (`hold::walk_aged`) racing large
/// tenants: the walk reads each stamp with no claim, but returns a slot only through
/// `purge_unit`'s claim, so in every schedule it holds every slot it returns (`TestHost::purge`
/// fails the schedule otherwise), a tenant's claim and its trim touch only its own slot, and
/// no slot is lost, invented, or said to be zero while it holds data. Two slots start free,
/// written and a hold old, one held; the tenants take and free them while the walk runs.
fn an_age_walk_races_large_tenants(threads: usize, bound: usize) {
    let plan = big_plan();
    explore(
        Config {
            preemptions: Some(bound),
            ..Config::ALL
        },
        threads,
        || {
            let host = Host8::new(plan, class(64 << 10), 1);
            let region = host.region_at(0);
            host.carve_all(region, 3);
            for slot in 0..3 {
                host.write(region, slot);
                stamp(&host, region, slot, host.layout().size(), AGE_OLD);
                if slot != 1 {
                    free_one(&host, region, slot);
                }
            }
            host
        },
        |host, me| match me {
            0 => {
                let region = host.region_at(0);
                let walk = super::hold::walk_aged(
                    host,
                    region,
                    AGE_COUNT,
                    super::policy::HOLD_STEPS,
                    usize::MAX,
                    usize::MAX,
                );
                assert!(walk.done);
            }
            1 => large_tenant(host),
            _ => {
                free_one(host, host.region_at(0), 1);
                large_tenant(host);
            }
        },
        |host, _| {
            let region = host.region_at(0);
            let mut expected: BTreeSet<u32> = (0..3).collect();
            if threads < 3 {
                expected.remove(&1);
            }
            let free: BTreeSet<u32> = host.free_slots(region).into_iter().collect();
            assert_eq!(free, expected, "a slot was lost or invented");
            assert!(host.all_visible());
            zero_bits_sound(host);
        },
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn an_age_walk_races_large_tenants_quickly() {
    an_age_walk_races_large_tenants(2, 2);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn an_age_walk_races_large_tenants_fully() {
    an_age_walk_races_large_tenants(3, 2);
}
