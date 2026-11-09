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
use crate::stress::{FULL, Gate, LEDGER_BYTES, QUICK, Rounds, count, failed, jitter, own, spread};
use core::ptr::{self, NonNull};
use core::sync::atomic::Ordering::Relaxed;
use core::sync::atomic::{AtomicPtr, AtomicUsize};
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

/// The same two writes and two reads with a purge's claim, whose clear is one of a group: a free of
/// the word's other slot against the purge's atomic AND of the summary word and its re-read.
fn a_free_and_a_purges_clear_leave_the_slot_visible(time: Duration) {
    let host: TestHost<Atomic> = TestHost::new(Standard::GEOMETRY.largest_plan(), class(64), 1);
    let region = host.region_at(0);
    host.carve_all(region, 64);
    let gate = Gate::new(2);
    let rounds = Rounds::of(time, 100);
    spread(2, |me| {
        let mut noise = 0x9e37_79b9_7f4a_7c15u64 + me as u64;
        let mut round = 0;
        loop {
            // Slot 0 free, the only one, and every summary bit above it set.
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
                let mut claims = tree::ExactRun::new();
                assert!(claims.claim(&host, region, 0, 1));
                claims.finish(&host);
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
fn a_free_and_a_purges_clear_leave_the_slot_visible_quickly() {
    a_free_and_a_purges_clear_leave_the_slot_visible(QUICK);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn a_free_and_a_purges_clear_leave_the_slot_visible_fully() {
    a_free_and_a_purges_clear_leave_the_slot_visible(FULL);
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

/// The same for a large class, whose free and claim are the single-slot path.
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
/// break: the tag, the carving word, the region's header and the published byte. One more thread
/// walks the heap meanwhile, as a purge does, learning each region's class from that byte and then
/// reading its words.
fn racing_classes_keep_their_regions_apart(time: Duration) {
    let rounds = Rounds::of(time, 3);
    let mut round = 0;
    while rounds.go_on(round) {
        round += 1;
        let state = Box::new(State::new(SimOs::<Standard>::new()));
        let back = state.get_or_reserve().unwrap();
        let classes = [class(2048), class(4096), class(40 << 10), class(48 << 10)];
        let gate = Gate::new(classes.len() + 1);
        let done = AtomicUsize::new(0);
        let got = spread(classes.len() + 1, |me| {
            let back = state.get().unwrap();
            gate.pass();
            let mut got = Vec::new();
            let Some(&class) = classes.get(me) else {
                // Under Miri a walk is slow, and two are enough.
                let mut walks = 0;
                while done.load(Relaxed) < classes.len() && !failed() && (!cfg!(miri) || walks < 2)
                {
                    back.trim();
                    walks += 1;
                }
                return got;
            };
            for _ in 0..if cfg!(miri) { 4 } else { 64 } {
                let block = back.alloc(class, class.size()).expect("a slot");
                assert_eq!(back.lookup(block.ptr.as_ptr()), Some(Kind::Slot(class)));
                got.push((block.ptr.addr().get(), class));
            }
            done.fetch_add(1, Relaxed);
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

/// A program may pass a block to another thread with a relaxed store and have that thread free
/// it, if nothing touches the block after its allocation: C asks a free to be ordered only after
/// the block's last use. The free then runs with no happens-before edge from the claim or the carve
/// that made the block, on a fresh region of a fresh class, so that it races the tag, the watermark
/// and the summary words. What it reads it reads through the pointer, and an address dependency
/// orders those loads behind the load of the pointer (`docs/research/memory-ordering.md`, "A
/// block passed with no order"). The heap itself is published first: that word has no such
/// dependency. That is the hardware's order, not Rust's, so Miri fails this as it should.
fn a_block_passed_with_no_order_can_be_freed(time: Duration) {
    let rounds = Rounds::of(time, 0);
    let mut round = 0;
    while rounds.go_on(round) {
        round += 1;
        let state = Box::new(State::new(SimOs::<Standard>::new()));
        state.get_or_reserve().unwrap();
        let passed = AtomicPtr::<u8>::new(ptr::null_mut());
        let classes = [class(48), class(2048), class(40 << 10)];
        spread(2, |me| {
            let back = state.get().unwrap();
            for class in classes {
                if me == 0 {
                    let block = back.alloc(class, class.size()).expect("a slot");
                    passed.store(block.ptr.as_ptr(), Relaxed);
                    while !passed.load(Relaxed).is_null() {
                        assert!(!failed(), "another thread of this test failed");
                        core::hint::spin_loop();
                    }
                } else {
                    let block = loop {
                        if let Some(block) = NonNull::new(passed.load(Relaxed)) {
                            break block;
                        }
                        assert!(!failed(), "another thread of this test failed");
                        core::hint::spin_loop();
                    };
                    assert_eq!(back.lookup(block.as_ptr()), Some(Kind::Slot(class)));
                    back.free_by_address(block);
                    passed.store(ptr::null_mut(), Relaxed);
                }
            }
        });
    }
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Miri has no address dependencies, and in Rust's model the free may read an old tag"
)]
fn a_block_passed_with_no_order_can_be_freed_quickly() {
    a_block_passed_with_no_order_can_be_freed(QUICK);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn a_block_passed_with_no_order_can_be_freed_fully() {
    a_block_passed_with_no_order_can_be_freed(FULL);
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
            assert_eq!(std::format!("{:?}", memo.layout(|| plan, class)), want);
        });
    }
}
