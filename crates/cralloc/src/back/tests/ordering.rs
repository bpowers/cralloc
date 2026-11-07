//! The back end's memory orders on real threads: what `docs/research/memory-ordering.md` says the
//! bitmaps, the summary bits and the assignment of regions each rely on, checked where a store can
//! be late. The interleaving explorer (`back::models`) covers these protocols under sequential
//! consistency; these look for what only a weaker order would let through.

use super::super::layout::Memo;
use super::super::testhost::TestHost;
use super::super::tree;
use super::super::word::Atomic;
use super::super::{Kind, State};
use crate::classes::{Class, SmallClass};
use crate::os::sim::{Shape, SimOs, Standard};
use crate::stress::{FULL, Gate, LEDGER_BYTES, QUICK, Rounds, count, jitter, own, spread};
use core::ptr;
use std::boxed::Box;
use std::collections::BTreeSet;
use std::time::Duration;
use std::vec::Vec;

fn class(size: usize) -> Class {
    Class::of_size(size).unwrap()
}

fn small(size: usize) -> SmallClass {
    class(size).small().unwrap()
}

/// A free sets its word's bit and then reads the summary bit; a claim that empties a word clears
/// the summary bit and then reads the word (`tree`). Each writes one word and then reads the other,
/// so only sequential consistency keeps both reads from missing, which would leave a free slot
/// under a clear summary bit, where no search finds it until the next free into that word
/// (`tla/Summary.tla`, `Visible`).
fn a_free_and_an_emptying_claim_leave_the_slot_visible(time: Duration) {
    let host: TestHost<Atomic> = TestHost::new(Standard::GEOMETRY.largest_plan(), class(64), 1);
    let region = host.region_at(0);
    host.carve_all(region, 64);
    let gate = Gate::new(2);
    let rounds = Rounds::of(time, 100);
    spread(2, |me| {
        let mut noise = 0x9e37_79b9_7f4a_7c15u64 + me as u64;
        let mut round = 0;
        loop {
            // One slot free, the lowest, and every summary bit above it set.
            if me == 0 {
                while tree::claim(&host, 64, false).is_some() {}
                tree::release(&host, region, 0, 1);
            }
            gate.pass();
            if !rounds.go_on(round) {
                return;
            }
            round += 1;
            jitter(&mut noise);
            if me == 0 {
                tree::release_one(&host, region, 0, 1);
            } else {
                let taken = tree::claim(&host, 1, false).expect("a slot is free");
                assert_eq!(taken.bits, 1);
            }
            gate.pass();
            if me == 0 {
                assert!(
                    host.all_visible(),
                    "a free slot lies under a clear summary bit"
                );
            }
            gate.pass();
        }
    });
}

#[test]
fn a_free_and_an_emptying_claim_leave_the_slot_visible_quickly() {
    a_free_and_an_emptying_claim_leave_the_slot_visible(QUICK);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn a_free_and_an_emptying_claim_leave_the_slot_visible_fully() {
    a_free_and_an_emptying_claim_leave_the_slot_visible(FULL);
}

/// A launder's atomic OR and a fill's claiming compare-and-swap are all that carries a block from
/// its last owner to its next. Threads fill and launder one class, so that each mostly takes what
/// another has just returned, and every block's ledger must show every earlier owner.
fn a_launder_and_a_fill_hand_over_what_the_last_owner_wrote(time: Duration) {
    let state = Box::new(State::new(SimOs::<Standard>::new()));
    let back = state.get_or_reserve().unwrap();
    let class = small(512);
    assert!(class.size() >= LEDGER_BYTES);
    let rounds = Rounds::of(time, 100);
    let results = spread(4, |_| {
        let back = state.get().unwrap();
        let mut slots = [ptr::null_mut(); 32];
        let mut seen = BTreeSet::new();
        let mut owned = 0u64;
        let mut round = 0;
        while rounds.go_on(round) {
            round += 1;
            let got = back.fill(class, &mut slots[..]);
            assert_ne!(got, 0);
            for &slot in &slots[..got] {
                // SAFETY: a slot this thread now holds, of at least the ledger's size.
                unsafe { own(slot) };
                seen.insert(slot.addr());
            }
            owned += got as u64;
            // SAFETY: the slots this thread filled, given up once.
            unsafe { back.launder(class, &slots[..got]) };
        }
        (owned, seen)
    });
    let owned: u64 = results.iter().map(|r| r.0).sum();
    let blocks: BTreeSet<usize> = results.into_iter().flat_map(|r| r.1).collect();
    let base = back.space().base();
    let counted: u64 = blocks
        .iter()
        // SAFETY: slots of the heap, all free, and no thread runs.
        .map(|&block| unsafe { count(base.with_addr(block.try_into().unwrap()).as_ptr()) })
        .sum();
    assert_eq!(counted, owned, "a new owner read a stale ledger");
}

#[test]
fn a_launder_and_a_fill_hand_over_what_the_last_owner_wrote_quickly() {
    a_launder_and_a_fill_hand_over_what_the_last_owner_wrote(QUICK);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn a_launder_and_a_fill_hand_over_what_the_last_owner_wrote_fully() {
    a_launder_and_a_fill_hand_over_what_the_last_owner_wrote(FULL);
}

/// The same for a large class, whose free and claim are the single-slot path, and whose slot's size
/// word goes from holder to holder with the block.
fn a_large_free_and_claim_hand_over_what_the_last_owner_wrote(time: Duration) {
    let state = Box::new(State::new(SimOs::<Standard>::new()));
    let back = state.get_or_reserve().unwrap();
    let class = class(40 << 10);
    assert!(!class.is_small());
    let rounds = Rounds::of(time, 50);
    let results = spread(2, |me| {
        let back = state.get().unwrap();
        let mut seen = BTreeSet::new();
        let (mut owned, mut round) = (0u64, 0);
        let size = class.size() - 8 * me;
        while rounds.go_on(round) {
            round += 1;
            let block = back.alloc(class, size).expect("a slot");
            // SAFETY: a slot this thread now holds, of at least the ledger's size.
            unsafe { own(block.ptr.as_ptr()) };
            assert_eq!(back.size(block.ptr.as_ptr()), Some(size), "a stale size word");
            seen.insert(block.ptr.addr().get());
            owned += 1;
            back.free(block.ptr, class);
        }
        (owned, seen)
    });
    let owned: u64 = results.iter().map(|r| r.0).sum();
    let blocks: BTreeSet<usize> = results.into_iter().flat_map(|r| r.1).collect();
    let base = back.space().base();
    let counted: u64 = blocks
        .iter()
        // SAFETY: slots of the heap, all free, and no thread runs.
        .map(|&block| unsafe { count(base.with_addr(block.try_into().unwrap()).as_ptr()) })
        .sum();
    assert_eq!(counted, owned, "a new owner read a stale ledger");
}

#[test]
fn a_large_free_and_claim_hand_over_what_the_last_owner_wrote_quickly() {
    a_large_free_and_claim_hand_over_what_the_last_owner_wrote(QUICK);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn a_large_free_and_claim_hand_over_what_the_last_owner_wrote_fully() {
    a_large_free_and_claim_hand_over_what_the_last_owner_wrote(FULL);
}

/// Threads race two classes through the assignment of regions, on a fresh heap each round: every
/// slot handed out lies in a region tagged for its class, what the walks read (`tags`) names no
/// other class, and no slot goes out twice. Each round's start is the part that a late store would
/// break: the tag, the carving word, the region's header and the published byte.
fn racing_classes_keep_their_regions_apart(time: Duration) {
    let rounds = Rounds::of(time, 3);
    let mut round = 0;
    while rounds.go_on(round) {
        round += 1;
        let state = Box::new(State::new(SimOs::<Standard>::new()));
        let back = state.get_or_reserve().unwrap();
        let classes = [class(2048), class(4096), class(40 << 10), class(48 << 10)];
        let gate = Gate::new(4);
        let got = spread(4, |me| {
            let back = state.get().unwrap();
            let class = classes[me % classes.len()];
            gate.pass();
            let mut got = Vec::new();
            for _ in 0..if cfg!(miri) { 4 } else { 64 } {
                let block = back.alloc(class, class.size()).expect("a slot");
                assert_eq!(back.lookup(block.ptr.as_ptr()), Some(Kind::Slot(class)));
                got.push((block.ptr.addr().get(), class));
            }
            got
        });
        let mut slots = BTreeSet::new();
        for (address, class) in got.into_iter().flatten() {
            assert!(slots.insert(address), "a slot handed out twice");
            let location = back
                .space()
                .locate(ptr::without_provenance(address))
                .unwrap();
            assert_eq!(state.tags.published(location.region), Some(class));
        }
    }
}

#[test]
fn racing_classes_keep_their_regions_apart_quickly() {
    racing_classes_keep_their_regions_apart(QUICK);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn racing_classes_keep_their_regions_apart_fully() {
    racing_classes_keep_their_regions_apart(FULL);
}

/// A class's layout, kept the first time any thread works it out: a reader that sees the last
/// word sees the rest.
#[test]
fn a_kept_layout_is_whole_once_its_last_word_is_there() {
    let plan = Standard::GEOMETRY.largest_plan();
    let class = class(512);
    let want = std::format!("{:?}", super::super::layout::Layout::new(plan, class));
    let rounds = Rounds::of(QUICK / 3, 20);
    let mut round = 0;
    while rounds.go_on(round) {
        round += 1;
        let memo = Memo::new();
        let gate = Gate::new(2);
        spread(2, |_| {
            gate.pass();
            assert_eq!(std::format!("{:?}", memo.layout(plan, class)), want);
        });
    }
}
