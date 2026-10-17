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

use super::explore::{Config, Outcome, TWord, explore};
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

/// Claims up to `n` slots and returns them, with their region.
fn claim(host: &Host8, n: u32) -> Vec<(RegionId, u32)> {
    let Some(taken) = tree::claim(host, n) else {
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

fn run(host: &Host8, op: Op) -> Vec<(RegionId, u32)> {
    match op {
        Op::ClaimRelease(n) => {
            let got = claim(host, n);
            release_all(host, &got);
            Vec::new()
        }
        Op::Claim(n) => claim(host, n),
        Op::Free(slot) => {
            free_now(host, host.region_at(0), slot);
            Vec::new()
        }
    }
}

/// Runs two single operations against each other under every schedule, from
/// `summary_setup`, and checks `Partition` and `Visible`.
fn summary_pair(a: Op, b: Op) -> usize {
    let ops = [a, b];
    let freed: Vec<u64> = ops
        .iter()
        .filter_map(|op| match *op {
            Op::Free(slot) => Some(slot as u64),
            _ => None,
        })
        .collect();
    explore(
        Config::ALL,
        2,
        || (summary_setup(), Mutex::new(Vec::new())),
        |(host, kept), me| {
            let got = run(host, ops[me]);
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

/// `Summary.cfg` and `Summary_heal.cfg`, two threads, every schedule: each pair of single
/// operations that can meet on a word. A claim that empties a word races a free into it (the
/// clear-then-reread against the healing walk), two claims contend for one word, and frees race
/// into an empty word and an empty region.
#[test]
#[cfg_attr(miri, ignore)]
fn summary_pairs_every_schedule() {
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
        schedules += summary_pair(a, b);
    }
    assert!(schedules > 1000, "{schedules}");
}

/// Longer sequences on two threads, preemptions bounded to three.
#[test]
#[cfg_attr(miri, ignore)]
fn summary_two_threads_bounded() {
    explore(
        Config {
            preemptions: Some(3),
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

/// `Summary_heal.cfg`: three threads, preemptions bounded to two.
#[test]
#[cfg_attr(miri, ignore)]
fn summary_three_threads_bounded() {
    explore(
        Config {
            preemptions: Some(2),
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

/// `Summary_heal_deep.cfg`: a region with two summary levels (8-byte slots in a 64 KiB region:
/// 128 leaf words, two words above them, one top word). Slots in leaf words 0 and 64 sit under
/// different level-1 words. Preemptions bounded to three.
#[test]
#[cfg_attr(miri, ignore)]
fn summary_deep_bounded() {
    explore(
        Config {
            preemptions: Some(3),
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

/// `Summary_heal_crash.cfg`: as `summary_three_threads_bounded`, but threads 1 and 2 may vanish
/// at any step, as in a forked child. What a vanished thread held is lost; nothing is ever
/// handed out twice; and once the survivor is done, a free into any word makes that word
/// reachable (the healing free), even where a vanished thread left a summary bit clear.
#[test]
#[cfg_attr(miri, ignore)]
fn summary_crash_then_heal() {
    explore(
        Config {
            preemptions: Some(2),
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
#[ignore = "slow: a few minutes; run with --ignored"]
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
            let header = host.meta(region, host.layout().grow_word()).load();
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
    let carved = super::layout::Grow::unpack(host.meta(region, layout.grow_word()).load()).carved;
    let huge_from =
        super::layout::Hints::unpack(host.meta(region, layout.hints_word()).load()).huge_from;
    let mut cursor = 0;
    let mut purged = 0;
    loop {
        let next = match layout.unit_below(carved, huge_from, cursor) {
            Step::Done => 0,
            Step::Skip(page) => page,
            Step::Unit(unit) => {
                if let (Purge::Done(_), _) = region::purge_unit(host, region, unit) {
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
    let taken = tree::claim(host, 1)?;
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
                host.meta(region, host.layout().hints_word())
                    .fetch_or(hints.pack());
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
            let layout = *host.layout();
            let carved =
                super::layout::Grow::unpack(host.meta(region, layout.grow_word()).load()).carved;
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
#[test]
#[cfg_attr(miri, ignore)]
fn purge_in_huge_pages() {
    purge_roles(
        tiny(),
        32,
        16,
        &[3],
        2,
        true,
        [Role::Purge, Role::Program, Role::FreeHeld],
    );
}

/// Two purgers over the same units, against a program.
#[test]
#[cfg_attr(miri, ignore)]
fn two_purgers_on_the_same_units() {
    purge_roles(
        tiny(),
        48,
        4,
        &[1],
        2,
        false,
        [Role::Purge, Role::Purge, Role::FreeHeld],
    );
}

/// A carve racing a purge: the purge reads the watermark once and must never return a page a
/// carve hands out. The carver checks that its slots read as zero.
#[test]
#[cfg_attr(miri, ignore)]
fn a_carve_racing_a_purge() {
    purge_roles(
        tiny(),
        48,
        3,
        &[],
        3,
        false,
        [Role::Purge, Role::Carve, Role::Program],
    );
}

/// `Purge.cfg`: two 32-byte slots to a 64-byte page.
#[test]
#[cfg_attr(miri, ignore)]
fn purge_aligned_slots() {
    purge_scenario(tiny(), 32, 4, &[1], 2);
}

/// `Purge_straddle.cfg`: 48-byte slots on 64-byte pages, so slot 1 straddles pages 0 and 1.
#[test]
#[cfg_attr(miri, ignore)]
fn purge_straddling_slots() {
    purge_scenario(tiny(), 48, 3, &[1], 2);
}

/// `Purge_big.cfg` with the zero bitmap: 64 KiB slots, two 32 KiB pages each, a large class.
#[test]
#[cfg_attr(miri, ignore)]
fn purge_big_slots_and_zero_bits() {
    let host = Host8::new(big_plan(), class(64 << 10), 1);
    assert!(host.layout().zero_word(0).is_some());
    purge_scenario(big_plan(), 64 << 10, 2, &[0], 2);
}

/// A large class whose slots straddle pages (48 KiB slots on 32 KiB pages): zero bits are set
/// only for slots wholly inside a purged unit.
#[test]
#[cfg_attr(miri, ignore)]
fn purge_big_straddling_slots_and_zero_bits() {
    purge_scenario(big_plan(), 48 << 10, 3, &[2], 2);
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
