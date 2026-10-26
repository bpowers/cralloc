//! Free large slots that age on their own (`hold::walk_aged`): a slot freed long ago goes back
//! while frees keep its region young, wherever it lies in the region and past a look's budget,
//! in bounded looks and down to the floor; a kept outgrown slot leaves its count as it goes; a
//! slot its free returned whole is passed without a call; and a stamp ages across its wrap.

use super::super::hold::{until_aged, walk_aged, walk_due};
use super::super::layout::{AgeWalk, Recorded, STAMP_MASK};
use super::super::policy::{FINE_TICKS, HOLD_STEPS, allowance_floor, hold_beat};
use super::super::region;
use super::super::testhost::TestHost;
use super::super::tree::{self, Host as _};
use super::super::word::{Atomic, BITS, Word as _};
use super::*;
use crate::space::Geometry;
use core::sync::atomic::Ordering::Relaxed;

/// One heartbeat a fine step after the last.
fn beat<O: Os>(back: &Back<'_, O>, os: &SimOs<Native>) {
    os.advance_clock(FINE_TICKS);
    back.heartbeat();
}

/// The hold returns free large memory down to nothing, so that a test whose heap is a region
/// or two, no more than the floor in the simulator, sees what it does.
fn unfloor<O: Os>(back: &Back<'_, O>) {
    back.state.hold.unfloored.store(true, Relaxed);
}

/// Whether the free slot of `len` bytes at `ptr` has gone back to the OS.
fn purged(os: &SimOs<Native>, ptr: NonNull<u8>, len: usize) -> bool {
    // SAFETY: free slots of the test's heap, which nothing else uses meanwhile.
    unsafe { os.is_purged(ptr, len) }
}

/// `n` live slots of `class`, painted, all in one region.
fn one_region<O: Os>(back: &Back<'_, O>, class: Class, n: usize) -> Vec<NonNull<u8>> {
    let blocks: Vec<_> = (0..n)
        .map(|_| back.alloc(class, class.size()).unwrap().ptr)
        .collect();
    let region = back.space().locate(blocks[0].as_ptr()).unwrap().region;
    for b in &blocks {
        assert_eq!(back.space().locate(b.as_ptr()).unwrap().region, region);
        paint(b.as_ptr(), class.size(), 7);
    }
    blocks
}

/// Frees the slot at `churn` and takes it again, as a program that keeps reusing one buffer
/// does: the free leaves the slot resident and stamps its region, so the region never has a
/// hold without such a free.
fn churn<O: Os>(back: &Back<'_, O>, class: Class, churn: NonNull<u8>) {
    back.free(churn, class);
    let again = back.alloc(class, class.size()).unwrap().ptr;
    assert_eq!(again, churn, "the lowest free slot");
    paint(churn.as_ptr(), class.size(), 7);
}

/// Slots freed together, in a region that a free of another slot keeps young every fine step,
/// go back once they have been free for the hold, at the first walk after that (a walk starts
/// once a hold, so within two holds of the free), a slot a heartbeat (the look's bound in bytes),
/// and not before; the region's stamp stays young throughout, so the hold of the region alone
/// would never have returned them.
#[test]
#[cfg_attr(miri, ignore)]
fn old_free_slots_go_back_while_frees_keep_their_region_young() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    unfloor(&back);
    let class = large(64 << 10);
    let size = class.size();
    let blocks = one_region(&back, class, 16);
    beat(&back, os);
    beat(&back, os);
    for b in &blocks[1..] {
        back.free(*b, class);
    }
    let mut first = None;
    for step in 0..2 * HOLD_STEPS + 16 {
        churn(&back, class, blocks[0]);
        let before = back.retained_large();
        beat(&back, os);
        assert!(before.saturating_sub(back.retained_large()) <= hold_beat(Native::GEOMETRY));
        if first.is_none() && blocks[1..].iter().any(|b| purged(os, *b, size)) {
            first = Some(step);
        }
        let stamp = back.hold_stamp_at(blocks[0].as_ptr()) as u32;
        assert!(
            back.hold_count().wrapping_sub(stamp) <= 1,
            "the region is young"
        );
    }
    let first = first.expect("no old slot went back");
    assert!(first + 3 >= HOLD_STEPS, "a slot went back at step {first}");
    assert!(blocks[1..].iter().all(|b| purged(os, *b, size)));
    assert_eq!(back.retained_large(), 0);
    assert!(state.hold.aged_returned.load(Relaxed) >= 15 * size);
    back.free(blocks[0], class);
}

/// A region of thousands of slots, its old free slots at the bottom below thousands of young
/// ones, and a look's budget a small part of the region: the walk carries on across looks from
/// where the last stopped, returns every old slot and no young one, and then says its next walk
/// is due when the youngest it passed will be old enough. A look whose bytes are fewer than a
/// slot's returns the slot a piece at a time and stays at it.
#[test]
#[cfg_attr(miri, ignore)]
fn the_walk_reaches_old_slots_below_young_ones_past_its_budget() {
    let plan = Geometry::DESIGN.largest_plan();
    let class = large(40 << 10);
    for (limit, old) in [(usize::MAX, 100u32), (4096, 3)] {
        let host = TestHost::<Atomic>::new(plan, class, 1);
        let region = host.region_at(0);
        let slots = 3000;
        host.carve_all(region, slots);
        let count = 5000u32;
        for slot in 0..slots {
            host.write(region, slot);
            let freed = if slot < old {
                count - HOLD_STEPS - 5
            } else {
                count - 3
            };
            let recorded = Recorded {
                size: class.size(),
                huge: false,
                unwritten: false,
                outgrown: false,
                freed,
            };
            region::record(&host, region, slot, recorded);
            tree::release_one(&host, region, slot / BITS, slot % BITS);
        }
        let (mut looks, mut returned) = (0, 0);
        loop {
            let out = walk_aged(&host, region, count, HOLD_STEPS, limit, 256);
            assert!(out.read <= 256 + 8, "a look read {} words", out.read);
            assert!(out.returned <= limit.max(4096));
            looks += 1;
            returned += out.returned;
            if out.done {
                break;
            }
            assert!(looks < 2000, "the walk never reached the bottom");
        }
        assert!(looks > 10, "{looks} looks: the budget did not bound them");
        assert_eq!(returned, old as usize * class.size());
        for slot in 0..slots {
            let zero = host
                .meta(region, host.layout().zero_word(slot / BITS).unwrap())
                .load();
            assert_eq!(zero >> (slot % BITS) & 1 != 0, slot < old, "slot {slot}");
            assert_eq!(host.is_zero(region, slot), slot < old, "slot {slot}");
        }
        let word = AgeWalk::unpack(host.meta(region, host.layout().age_word().unwrap()).load());
        assert_eq!(
            word,
            AgeWalk {
                cursor: 0,
                began: count
            }
        );
        let idle = walk_aged(
            &host,
            region,
            count + HOLD_STEPS - 2,
            HOLD_STEPS,
            limit,
            256,
        );
        assert!(idle.done && !idle.walked, "walked again within the hold");
        assert!(
            walk_aged(
                &host,
                region,
                count + HOLD_STEPS - 1,
                HOLD_STEPS,
                limit,
                256
            )
            .walked
        );
    }
}

/// A stamp keeps the count's low 28 bits and reads as its true age less a multiple of 2^28:
/// never older than it is, so a stamp left for a wrap of its bits is a keep of at most two
/// holds, never a slot returned early. Up to a hold ahead of the count is a free that read the
/// count after the look did. A walk may start a hold after the last began, and one under way
/// may always go on.
#[test]
fn stamps_age_by_their_bits_and_walks_are_due_by_their_count() {
    let h = HOLD_STEPS;
    for count in [
        0u32,
        1 << 27,
        STAMP_MASK - 3,
        STAMP_MASK,
        u32::MAX - 5,
        0x1234_5678,
    ] {
        let freed = count & STAMP_MASK;
        let at = |steps: u32| count.wrapping_add(steps);
        assert_eq!(until_aged(freed, at(0), h), h);
        assert_eq!(until_aged(freed, at(1), h), h - 1);
        assert_eq!(until_aged(freed, at(h - 1), h), 1);
        assert_eq!(until_aged(freed, at(h), h), 0);
        assert_eq!(until_aged(freed, at(1 << 27), h), 0);
        assert_eq!(until_aged(freed, at(STAMP_MASK - h), h), 0);
        // A wrap of the stamp's bits: read as younger, never as older.
        for late in [STAMP_MASK - h + 1, STAMP_MASK, STAMP_MASK + 1] {
            assert_eq!(until_aged(freed, at(late), h), h, "{count:#x} + {late:#x}");
        }
        assert_eq!(until_aged(freed, at(STAMP_MASK + 1 + 5), h), h - 5);
        // Ahead of the count.
        assert_eq!(until_aged(at(1) & STAMP_MASK, count, h), h);
        assert_eq!(until_aged(at(h) & STAMP_MASK, count, h), h);
        // A shorter hold, as `maintain` takes in a silence.
        assert_eq!(until_aged(freed, at(2), 2), 0);
        assert_eq!(until_aged(freed, at(1), 2), 1);
        // Walks.
        let due = |began: u32, cursor: u32| walk_due(AgeWalk { cursor, began }, count, h);
        assert!(
            !due(count, 0) && !due(count.wrapping_sub(h - 2), 0),
            "within the hold"
        );
        assert!(due(count.wrapping_sub(h - 1), 0) && due(count.wrapping_sub(5 * h), 0));
        assert!(due(count, 7), "under way");
        assert!(
            due(at(1), 0),
            "a start the count has not reached reads as long ago"
        );
        // `maintain`'s shorter hold in a silence.
        assert!(walk_due(
            AgeWalk {
                cursor: 0,
                began: count
            },
            at(1),
            2
        ));
    }
    assert!(
        walk_due(
            AgeWalk {
                cursor: 0,
                began: 0
            },
            h - 1,
            h
        ),
        "never walked"
    );
}

/// Ageing returns old slots of young regions only down to the floor, as the hold does. Two
/// classes, a region each, both kept young.
#[test]
#[cfg_attr(miri, ignore)]
fn ageing_stops_at_the_floor() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let classes = [large(64 << 10), large(128 << 10)];
    let regions = classes.map(|class| {
        let slots = back.space().plan().region_size() / class.size();
        one_region(&back, class, slots)
    });
    beat(&back, os);
    beat(&back, os);
    for (class, blocks) in classes.iter().zip(&regions) {
        for b in &blocks[1..] {
            back.free(*b, *class);
        }
    }
    let floor = allowance_floor(Native::GEOMETRY);
    assert!(back.retained_large() > floor + 4 * classes[1].size());
    for _ in 0..3 * HOLD_STEPS {
        for (class, blocks) in classes.iter().zip(&regions) {
            churn(&back, *class, blocks[0]);
        }
        beat(&back, os);
    }
    let left = back.retained_large();
    assert!(
        left <= floor && left + classes[1].size() > floor,
        "{left} against {floor}"
    );
    assert_ne!(state.hold.aged_returned.load(Relaxed), 0);
    for (class, blocks) in classes.iter().zip(&regions) {
        back.free(blocks[0], *class);
    }
}

/// A slot that a block outgrew, kept, in a region frees keep young, goes back once it has been
/// free for the hold, and the count of kept outgrown slots falls by what the keep added.
#[test]
#[cfg_attr(miri, ignore)]
fn a_kept_outgrown_slot_ages_and_leaves_its_count() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    unfloor(&back);
    let (class, wider) = (large(192 << 10), large(512 << 10));
    let live = back.alloc(class, class.size()).unwrap().ptr;
    paint(live.as_ptr(), class.size(), 7);
    let grown = back.alloc(class, 40 << 10).unwrap().ptr;
    // SAFETY: the test holds the block and uses none of it during the call.
    unsafe { back.resize_in_place(grown, 120 << 10) };
    paint(grown.as_ptr(), 120 << 10, 6);
    let to = back.alloc(wider, wider.size()).unwrap();
    back.resize(to.ptr, 240 << 10);
    // SAFETY: both are live blocks the test holds, `grown` given up.
    unsafe {
        back.move_large(
            grown,
            to.ptr,
            120 << 10,
            to.zeroed,
            crate::back::Move::Outgrown,
        )
    };
    assert!(back.kept_outgrown_at(grown.as_ptr()));
    assert_ne!(back.outgrown(), 0);
    for _ in 0..2 * HOLD_STEPS {
        churn(&back, class, live);
        beat(&back, os);
    }
    assert!(purged(os, grown, 120 << 10));
    assert_eq!(back.outgrown(), 0);
    back.free(live, class);
    back.free(to.ptr, wider);
}

/// A slot its free returned whole, a block's advised for huge pages or one left mostly
/// unwritten, is clean: a walk of its young region passes it at a look and makes no call.
#[test]
#[cfg_attr(miri, ignore)]
fn slots_returned_at_their_free_are_passed_without_a_call() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let _ballast = spend_kept_budget(&back);
    let os = state.os();
    unfloor(&back);
    let huge = Native::GEOMETRY.huge_page_size();
    let class = large(512 << 10);
    let live = back.alloc(class, class.size()).unwrap().ptr;
    paint(live.as_ptr(), class.size(), 7);
    // Advised: a block that doubled in place over a huge page, densely.
    let block = back.alloc(class, huge).unwrap().ptr;
    paint(block.as_ptr(), huge, 3);
    // SAFETY: the test holds the block and uses none of it during the call.
    unsafe { back.resize_in_place(block, 2 * huge) };
    assert_eq!(os.huge_pages(block), Some(HugePages::Prefer));
    back.free(block, class);
    assert!(purged(os, block, class.size()));
    let walk = |what: &str| {
        let purges = os.calls(Call::Purge);
        let aged = state.hold.aged_returned.load(Relaxed);
        for _ in 0..2 * HOLD_STEPS {
            churn(&back, class, live);
            beat(&back, os);
        }
        assert_eq!(os.calls(Call::Purge), purges, "{what}: a call");
        assert_eq!(state.hold.aged_returned.load(Relaxed), aged, "{what}");
    };
    walk("advised");
    // Unwritten: a zeroed block from the zero slot, marked, written in one page.
    let block = back.alloc(class, class.size()).unwrap();
    assert!(block.zeroed);
    back.mark_unwritten(block.ptr);
    paint(block.ptr.as_ptr(), 4096, 4);
    back.free(block.ptr, class);
    assert!(purged(os, block.ptr, class.size()));
    walk("unwritten");
    back.free(live, class);
}
