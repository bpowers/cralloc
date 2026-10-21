//! The hold on free large slots (`back::hold`): what it returns and when, the bound on a
//! heartbeat's look, the count's wrap, a look abandoned part way, what a free writes, the
//! walk's cursor, the frees that stamp and those that do not, and `maintain`'s pass.

use super::super::hold::{Reach, fine_read, held_for};
use super::super::policy::{
    FINE_TICKS, HOLD_STEPS, PURGE_MAINTAIN_STEP_PAGES, PURGE_MAX_RUN_PAGES, hold_beat, hold_floor,
};
use super::super::word::Word;
use super::*;

/// Heartbeats `n` times, the clock moving `ticks` before each.
fn beat_for<O: Os>(back: &Back<'_, O>, os: &SimOs<Native>, n: u32, ticks: u64) {
    for _ in 0..n {
        os.advance_clock(ticks);
        back.heartbeat();
    }
}

/// Heartbeats with the clock standing still until a beat returns nothing, as the beats that
/// carry on a look bounded in bytes do; at most `most` beats. Returns how many returned
/// something, and the most one returned.
fn beat_until_still<O: Os>(back: &Back<'_, O>, most: u32) -> (u32, usize) {
    let (mut beats, mut largest) = (0, 0);
    for _ in 0..most {
        let before = back.retained_large();
        back.heartbeat();
        let returned = before.saturating_sub(back.retained_large());
        if returned == 0 {
            break;
        }
        beats += 1;
        largest = largest.max(returned);
    }
    (beats, largest)
}

/// Allocates and paints enough slots of `class` to hold twice the hold's floor, and one more
/// that stays live above them, then frees the first ones after starting the fine count.
/// Returns the freed blocks and the live one.
fn free_twice_the_floor<O: Os>(
    back: &Back<'_, O>,
    os: &SimOs<Native>,
    class: Class,
) -> (Vec<NonNull<u8>>, NonNull<u8>) {
    free_slots(back, os, class, 2 * hold_floor(Native::GEOMETRY))
}

/// As [`free_twice_the_floor`], for `bytes` of slots.
fn free_slots<O: Os>(
    back: &Back<'_, O>,
    os: &SimOs<Native>,
    class: Class,
    bytes: usize,
) -> (Vec<NonNull<u8>>, NonNull<u8>) {
    let size = class.size();
    let n = bytes / size;
    let blocks: Vec<NonNull<u8>> = (0..n)
        .map(|_| back.alloc(class, size).unwrap().ptr)
        .collect();
    let top = back.alloc(class, size).unwrap().ptr;
    for b in &blocks {
        paint(b.as_ptr(), size, 7);
    }
    // Start the fine count, so that the frees are stamped in a counted step.
    beat_for(back, os, 2, FINE_TICKS);
    let before = back.retained_large();
    for b in &blocks {
        back.free(*b, class);
    }
    assert_eq!(back.retained_large() - before, n * size);
    (blocks, top)
}

/// The most one heartbeat's look returns: its bound, and the rest of the run it is on.
fn beat_bound() -> usize {
    hold_beat(Native::GEOMETRY) + PURGE_MAX_RUN_PAGES as usize * Native::GEOMETRY.page_size()
}

/// Runs the hold to its end from the frees of `free_twice_the_floor` and checks what it
/// returned: nothing for the hold, then everything over the floor, within the bound a beat.
fn hold_then_return<O: Os>(back: &Back<'_, O>, os: &SimOs<Native>, class: Class) {
    let (blocks, top) = free_twice_the_floor(back, os, class);
    let freed = back.retained_large();
    beat_for(back, os, HOLD_STEPS - 1, FINE_TICKS);
    assert_eq!(back.retained_large(), freed, "held for the hold");
    assert!(blocks.iter().all(|b| painted(b.as_ptr(), class.size(), 7)));
    beat_for(back, os, 1, FINE_TICKS);
    let first = freed - back.retained_large();
    assert!(first > 0, "the hold has ended");
    assert!(first <= beat_bound(), "one beat returned {first}");
    let (_, largest) = beat_until_still(back, 1000);
    assert!(largest <= beat_bound(), "one beat returned {largest}");
    let floor = hold_floor(Native::GEOMETRY);
    let left = back.retained_large();
    assert!(
        left <= floor && left + class.size() > floor,
        "down to the floor: {left}"
    );
    let purged = blocks
        .iter()
        // SAFETY: the slots are free and nothing else uses the simulated heap meanwhile.
        .filter(|b| unsafe { os.is_purged(**b, class.size()) })
        .count();
    assert_eq!(purged * class.size(), freed - left);
    back.free(top, class);
}

/// Free large slots stay resident while their regions' hold lasts, and when it ends what is
/// over the floor goes back to the OS, at most a bound a heartbeat, the beats after the first
/// carrying on without waiting for the next fine step.
#[test]
#[cfg_attr(miri, ignore)]
fn free_large_slots_over_the_floor_go_back_when_their_hold_ends() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    hold_then_return(&back, state.os(), large(256 << 10));
}

/// The same with the fine count past half its range, where a process beating every 2 ms is
/// after about 50 days, and across its wrap.
#[test]
#[cfg_attr(miri, ignore)]
fn the_hold_works_with_the_count_anywhere_in_its_range() {
    for start in [1 << 31, (1 << 31) + 1, u32::MAX - HOLD_STEPS / 2] {
        let state = state::<Native>();
        let back = state.get_or_reserve().unwrap();
        back.hold_set_count(start);
        hold_then_return(&back, state.os(), large(256 << 10));
    }
}

/// Stamps compare with the count by wrapping difference: a hold old and more is old, up to a
/// hold ahead is fresh (a free that read the count after the look), and anything else is old,
/// a stamp left over for more than half the range included.
#[test]
fn stamps_age_by_wrapping_difference() {
    let stamp = |count: u32| 1u64 << 32 | u64::from(count);
    for base in [0, 1 << 31, u32::MAX - 3] {
        let at = |steps: u32| base.wrapping_add(steps);
        assert!(!held_for(stamp(base), at(0), HOLD_STEPS));
        assert!(!held_for(stamp(base), at(HOLD_STEPS - 1), HOLD_STEPS));
        assert!(held_for(stamp(base), at(HOLD_STEPS), HOLD_STEPS));
        assert!(held_for(stamp(base), at(1 << 31), HOLD_STEPS));
        assert!(held_for(stamp(base), at((1 << 31) + 1), HOLD_STEPS));
        assert!(held_for(stamp(base), at(u32::MAX - HOLD_STEPS), HOLD_STEPS));
        // Ahead of the count, by a free that read it after the look.
        assert!(!held_for(stamp(at(1)), at(0), HOLD_STEPS));
        assert!(!held_for(stamp(at(HOLD_STEPS)), at(0), HOLD_STEPS));
        // A shorter hold, as `maintain` takes in a silence.
        assert!(!held_for(stamp(base), at(1), 2));
        assert!(held_for(stamp(base), at(2), 2));
    }
}

/// The fine count steps once for a step of time or more, and a reading less than a step behind
/// its time (another thread's, a moment older) writes nothing; one further behind is a clock
/// that went back, and moves the time without a step.
#[test]
fn the_fine_count_steps_once_and_ignores_slightly_older_readings() {
    let unit = 20;
    let step = FINE_TICKS >> unit;
    let word = |count: u64, at: u64| count << 32 | at;
    let at = 1000;
    let time = |units: u64| units << unit;
    assert_eq!(fine_read(word(5, at), time(at + step - 1)), None);
    assert_eq!(
        fine_read(word(5, at), time(at + step)),
        Some(word(6, at + step))
    );
    assert_eq!(
        fine_read(word(5, at), time(at + 100 * step)),
        Some(word(6, at + 100 * step))
    );
    assert_eq!(fine_read(word(5, at), time(at - 1)), None);
    assert_eq!(fine_read(word(5, at), time(at - step + 1)), None);
    assert_eq!(
        fine_read(word(5, at), time(at - step)),
        Some(word(5, at - step))
    );
}

/// Under the floor nothing is returned, however long it waits, and no look walks the heap.
#[test]
#[cfg_attr(miri, ignore)]
fn free_large_slots_under_the_floor_stay() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let class = large(256 << 10);
    let a = back.alloc(class, class.size()).unwrap();
    let top = back.alloc(class, class.size()).unwrap();
    beat_for(&back, os, 2, FINE_TICKS);
    back.free(a.ptr, class);
    let stamp = back.hold_stamp_at(a.ptr.as_ptr());
    assert_ne!(stamp, 0);
    beat_for(&back, os, 8 * HOLD_STEPS, FINE_TICKS);
    assert_eq!(back.retained_large(), class.size());
    assert_eq!(back.hold_at(), 0, "no look walked");
    assert_eq!(back.hold_stamp_at(a.ptr.as_ptr()), stamp);
    assert_eq!(back.hold_looked(), back.hold_count(), "looked once a step");
    back.free(top.ptr, class);
}

/// A silence ages a hold by one fine step however long it lasts: the first heartbeats after
/// it do not return what the program is about to take again (design: "Heartbeat").
#[test]
#[cfg_attr(miri, ignore)]
fn a_silence_ages_a_hold_by_one_step() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let class = large(256 << 10);
    let (blocks, top) = free_twice_the_floor(&back, os, class);
    let freed = back.retained_large();
    // Ten seconds of silence, then a few heartbeats close together.
    os.advance_clock(10 * DECAY_TICKS);
    beat_for(&back, os, 4, FINE_TICKS / 4);
    assert_eq!(back.retained_large(), freed, "a silence is one step");
    assert!(blocks.iter().all(|b| painted(b.as_ptr(), class.size(), 7)));
    let again = back.alloc(class, class.size()).unwrap();
    assert!(!again.zeroed, "the slot came back resident");
    back.free(again.ptr, class);
    back.free(top, class);
}

/// A free into a region whose hold is running restarts it.
#[test]
#[cfg_attr(miri, ignore)]
fn a_free_restarts_its_regions_hold() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let class = large(256 << 10);
    let (blocks, top) = free_twice_the_floor(&back, os, class);
    let freed = back.retained_large();
    for _ in 0..4 {
        beat_for(&back, os, HOLD_STEPS / 2, FINE_TICKS);
        let again: Vec<_> = (0..blocks.len())
            .map(|_| back.alloc(class, class.size()).unwrap().ptr)
            .collect();
        for b in &again {
            paint(b.as_ptr(), class.size(), 7);
            back.free(*b, class);
        }
    }
    assert_eq!(back.retained_large(), freed);
    back.free(top, class);
}

/// A look taken and never finished (its heartbeat vanished after taking it, as a fork leaves
/// it in the child) costs that step's look and nothing more: the next step looks again.
#[test]
#[cfg_attr(miri, ignore)]
fn an_abandoned_look_is_taken_up_at_the_next_step() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let class = large(256 << 10);
    let (_blocks, top) = free_twice_the_floor(&back, os, class);
    let freed = back.retained_large();
    beat_for(&back, os, HOLD_STEPS - 1, FINE_TICKS);
    os.advance_clock(FINE_TICKS);
    back.hold_take_and_vanish(back.now());
    back.heartbeat();
    assert_eq!(back.retained_large(), freed, "that step's look is lost");
    beat_for(&back, os, 1, FINE_TICKS);
    beat_until_still(&back, 1000);
    assert!(back.retained_large() <= hold_floor(Native::GEOMETRY));
    back.free(top, class);
}

/// Frees within one fine step take no look, and leave their region's stamp the count: a free
/// that finds it so writes nothing.
#[test]
#[cfg_attr(miri, ignore)]
fn frees_within_a_step_take_no_look() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let class = large(256 << 10);
    let (_blocks, top) = free_twice_the_floor(&back, os, class);
    beat_for(&back, os, HOLD_STEPS, FINE_TICKS);
    beat_until_still(&back, 1000);
    let mut looks = 0;
    for _ in 0..32 {
        let looked = back.hold_looked();
        beat_for(&back, os, 1, FINE_TICKS / 64);
        looks += u32::from(back.hold_looked() != looked);
        let b = back.alloc(class, class.size()).unwrap().ptr;
        paint(b.as_ptr(), class.size(), 7);
        back.free(b, class);
        assert_eq!(
            back.hold_stamp_at(b.as_ptr()),
            1 << 32 | u64::from(back.hold_count())
        );
    }
    // Half a fine step of beats: the count moves once at most.
    assert!(looks <= 1, "{looks} looks in half a step");
    back.free(top, class);
}

/// A look over the floor walks past a small class's region without writing its header.
#[test]
#[cfg_attr(miri, ignore)]
fn a_look_leaves_a_small_class_region_alone() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let class = small(4096);
    let objects = fill(&back, class, 64);
    let region = back.space().locate(objects[0]).unwrap().region.index();
    let header = &back.state.headers[region];
    let before = (header.grow.load(), header.hints.load());
    let large_class = large(256 << 10);
    let (_blocks, top) = free_twice_the_floor(&back, os, large_class);
    beat_for(&back, os, HOLD_STEPS, FINE_TICKS);
    beat_until_still(&back, 1000);
    assert!(back.retained_large() <= hold_floor(Native::GEOMETRY));
    assert_eq!((header.grow.load(), header.hints.load()), before);
    back.free(top, large_class);
    // SAFETY: the test's objects, given back once.
    unsafe { back.launder(class, &objects) };
}

/// Free slots of two classes: the look takes both down to the floor between them.
#[test]
#[cfg_attr(miri, ignore)]
fn a_look_returns_from_every_class_down_to_the_floor() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let floor = hold_floor(Native::GEOMETRY);
    let (a, b) = (large(256 << 10), large(128 << 10));
    let (from_a, top_a) = free_slots(&back, os, a, 3 * floor / 2);
    let (from_b, top_b) = free_slots(&back, os, b, 3 * floor / 2);
    beat_for(&back, os, HOLD_STEPS, FINE_TICKS);
    beat_until_still(&back, 1000);
    let left = back.retained_large();
    assert!(left <= floor && left + a.size() > floor, "{left}");
    let purged = |blocks: &[NonNull<u8>], class: Class| {
        blocks
            .iter()
            // SAFETY: the slots are free and nothing else uses the simulated heap meanwhile.
            .filter(|p| unsafe { os.is_purged(**p, class.size()) })
            .count()
    };
    assert!(purged(&from_a, a) > 0 && purged(&from_b, b) > 0);
    back.free(top_a, a);
    back.free(top_b, b);
}

/// A slot the hold returned comes back zeroed, and reads as zero.
#[test]
#[cfg_attr(miri, ignore)]
fn a_slot_the_hold_returned_comes_back_zeroed() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let class = large(256 << 10);
    let (blocks, top) = free_twice_the_floor(&back, os, class);
    beat_for(&back, os, HOLD_STEPS, FINE_TICKS);
    beat_until_still(&back, 1000);
    // SAFETY: the slots are free and nothing else uses the simulated heap meanwhile.
    let purged: Vec<bool> = blocks
        .iter()
        .map(|b| unsafe { os.is_purged(*b, class.size()) })
        .collect();
    assert!(purged.iter().any(|&p| p));
    let again: Vec<_> = (0..blocks.len())
        .map(|_| back.alloc(class, class.size()).unwrap())
        .collect();
    for block in &again {
        let at = blocks.iter().position(|b| *b == block.ptr).unwrap();
        if purged[at] {
            assert!(block.zeroed);
            assert!(painted(block.ptr.as_ptr(), class.size(), 0));
        } else {
            assert!(painted(block.ptr.as_ptr(), class.size(), 7));
        }
    }
    for block in again {
        back.free(block.ptr, class);
    }
    back.free(top, class);
}

/// A region whose free slots lie below more live ones than a look's budget reads is still
/// reached: the walk carries on from the region's cursor, where a walk from its top each time
/// never got past the live slots.
#[test]
#[cfg_attr(miri, ignore)]
fn the_walk_gets_past_live_slots_across_looks() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let class = large(64 << 10);
    let size = class.size();
    let blocks: Vec<_> = (0..12)
        .map(|_| back.alloc(class, size).unwrap().ptr)
        .collect();
    let region = back.space().locate(blocks[0].as_ptr()).unwrap().region;
    assert!(
        blocks
            .iter()
            .all(|b| back.space().locate(b.as_ptr()).unwrap().region == region)
    );
    for b in &blocks {
        paint(b.as_ptr(), size, 7);
    }
    beat_for(&back, os, 2, FINE_TICKS);
    // The lowest slot is free and resident; the eleven above it are live.
    back.free(blocks[0], class);
    let count = back.hold_count().wrapping_add(HOLD_STEPS);
    let mut calls = 0;
    loop {
        let (read, purged, done) = back.hold_region(
            region.index(),
            count,
            usize::MAX,
            4,
            Reach::beat(Native::GEOMETRY),
        );
        calls += 1;
        if purged != 0 {
            assert_eq!(purged, size);
            break;
        }
        assert!(!done, "done without reaching slot 0");
        assert!(read >= 4);
        assert!(
            calls < 1000,
            "1000 looks of 4 words each never reached slot 0"
        );
    }
    for b in &blocks[1..] {
        back.free(*b, class);
    }
}

/// The frees that leave a slot resident stamp its region; a free that returns the slot first
/// (a grown block past the budget for kept ones), and a move that returns the old slot, do not.
#[test]
#[cfg_attr(miri, ignore)]
fn only_frees_that_leave_a_slot_resident_stamp_it() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let huge = Native::GEOMETRY.huge_page_size();
    beat_for(&back, os, 2, FINE_TICKS);
    // A move within the budget keeps the old slot, and stamps its region.
    let (kept_class, to_class) = (large(192 << 10), large(512 << 10));
    let old = back.alloc(kept_class, 80 << 10).unwrap().ptr;
    pattern(old.as_ptr(), 80 << 10);
    let new = back.alloc(to_class, 160 << 10).unwrap().ptr;
    // SAFETY: both are live blocks that nothing else uses, and `new` is fresh.
    unsafe { back.move_large(old, new, 80 << 10, true) };
    assert_ne!(back.hold_stamp_at(old.as_ptr()), 0, "kept, and stamped");
    back.free(new, to_class);
    // Past the budget, a move returns the old slot, and a grown block's free returns it.
    let class = large(256 << 10);
    let grown = back.alloc(class, class.size()).unwrap().ptr;
    let moved_class = large(96 << 10);
    let from = back.alloc(moved_class, huge).unwrap().ptr;
    pattern(from.as_ptr(), huge);
    back.resize(grown, 2 * huge);
    let _ballast = spend_kept_budget(&back);
    // SAFETY: both are live blocks that nothing else uses, and `grown` is fresh.
    unsafe { back.move_large(from, grown, huge, true) };
    // SAFETY: the slot is free and nothing else uses the simulated heap meanwhile.
    assert!(unsafe { os.is_purged(from, moved_class.size()) });
    assert_eq!(
        back.hold_stamp_at(from.as_ptr()),
        0,
        "returned, not stamped"
    );
    assert_eq!(os.huge_pages(grown), Some(HugePages::Prefer));
    back.free(grown, class);
    assert_eq!(os.huge_pages(grown), Some(HugePages::Avoid));
    assert_eq!(
        back.hold_stamp_at(grown.as_ptr()),
        0,
        "returned, not stamped"
    );
}

/// One round of `maintain`'s looks, after a counted interval's silence: steps until one says
/// there is no more. Returns the most one step returned.
fn maintain_round<O: Os>(back: &Back<'_, O>, os: &SimOs<Native>) -> usize {
    os.advance_clock(DECAY_TICKS);
    let mut largest = 0;
    loop {
        let before = back.retained_large();
        let more = back.hold_maintain_step(&|| back.now());
        largest = largest.max(before.saturating_sub(back.retained_large()));
        if !more {
            return largest;
        }
    }
}

/// In a silence `maintain`'s looks are what step the fine count, once a round: the first
/// round's look takes the whole hold and returns nothing, and the second takes a stamp from
/// before the first look as old and returns everything over the floor, a bounded step at a
/// time.
#[test]
#[cfg_attr(miri, ignore)]
fn maintain_returns_a_quiet_programs_held_slots_at_its_second_round() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let class = large(256 << 10);
    let (_blocks, top) = free_twice_the_floor(&back, os, class);
    let freed = back.retained_large();
    maintain_round(&back, os);
    assert_eq!(back.retained_large(), freed, "the first round holds them");
    let largest = maintain_round(&back, os);
    let page = Native::GEOMETRY.page_size();
    assert!(largest <= (PURGE_MAINTAIN_STEP_PAGES + PURGE_MAX_RUN_PAGES) as usize * page);
    let floor = hold_floor(Native::GEOMETRY);
    let left = back.retained_large();
    assert!(left <= floor && left + class.size() > floor, "{left}");
    back.free(top, class);
}

/// Where heartbeats step the fine count, `maintain`'s looks, an interval apart, hold slots as
/// the heartbeats' do: a free shortly before a look is not taken.
#[test]
#[cfg_attr(miri, ignore)]
fn maintain_holds_a_busy_programs_slots_for_the_hold() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let class = large(256 << 10);
    let maintain = || {
        os.advance_clock(FINE_TICKS);
        let count = back.hold_count();
        while back.hold_maintain_step(&|| back.now()) {}
        assert_ne!(
            back.hold_count(),
            count,
            "the look's own reading stepped the count"
        );
    };
    maintain();
    // An interval of heartbeats, a fine step apart, then frees, and a few more heartbeats.
    beat_for(&back, os, (DECAY_TICKS / FINE_TICKS) as u32, FINE_TICKS);
    let (_blocks, top) = free_twice_the_floor(&back, os, class);
    let freed = back.retained_large();
    beat_for(&back, os, 10, FINE_TICKS);
    maintain();
    assert_eq!(back.retained_large(), freed, "held for the hold");
    beat_for(&back, os, HOLD_STEPS, FINE_TICKS);
    beat_until_still(&back, 1000);
    assert!(back.retained_large() <= hold_floor(Native::GEOMETRY));
    back.free(top, class);
}
