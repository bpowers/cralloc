//! Evidence for the adversarial review of the shorter large-object path (main `6ff0a8e`,
//! `5c38865`): the release store of a size word, the unlocked activity count, the single-slot
//! release that looks at every level, the retained count as two tallies, the clock-led decay
//! step, `Space::metadata_at`, and the one-pass claim.
//!
//! No schedule, input or state was found that breaks one of them, so nothing here is ignored.
//! The tests pin what the review checked by reading, each against a reference written out
//! here, so that a later change to the same code has something to fail:
//!
//! - the claim's one pass over a slot's clean words counts and clears what the two passes it
//!   replaced did, for every shape of clean word, a word with gaps included, and touches no
//!   bit outside its slot;
//! - `Space::metadata_at` is `Space::metadata` plus the offset for every region of every plan,
//!   a reservation that settled for smaller regions included;
//! - a single free sets every level's bit from an all-clear path and repairs a bit cleared at
//!   any one level, at the region's levels and the class's;
//! - the two tallies wrap, and a subtraction ahead of its addition reads as zero;
//! - a pass that is due with nothing retained ends at once, and memory freed after it waits
//!   an interval, where it was returned by the next heartbeat before.
//!
//! The step the single-slot release takes that `tla/Summary.tla` does not have, a look at the
//! level above a summary word its own OR found empty, is checked in `tla/SummaryLookUp.tla`.

use super::super::region;
use super::super::tree::Host;
use super::super::word::{BITS, Word};
use super::*;
use core::sync::atomic::Ordering::Relaxed;

/// A clean word for a trial: the shapes a slot's words take (nothing, everything, one run)
/// and the ones they rarely do (scattered bits, scattered gaps).
fn clean_shape(rng: &mut Rng) -> u64 {
    match rng.below(6) {
        0 => 0,
        1 => u64::MAX,
        2 => {
            let (a, b) = (rng.below(64) as u32, rng.below(64) as u32);
            super::super::word::span(a.min(b), a.max(b))
        }
        3 => rng.next() & rng.next() & rng.next(),
        4 => !(rng.next() & rng.next() & rng.next()),
        _ => rng.next(),
    }
}

/// Whether page `p` is marked in `words`.
fn marked(words: &[u64], p: u32) -> bool {
    words[(p / BITS) as usize] >> (p % BITS) & 1 != 0
}

/// `region::settle_slot` against the two passes it replaced, worked out a page at a time: the
/// count is of the slot's clean pages as they stood before the claim, the pages under the
/// block lose their bits, and no other bit of the region's clean bitmap changes. Slots of one
/// clean word, of several, and of words shared with their neighbours.
#[test]
#[cfg_attr(miri, ignore)]
fn the_claim_counts_and_clears_what_two_passes_did() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let page = Native::GEOMETRY.page_size();
    let mut rng = Rng(0x5c38_865d);
    for size in [40 << 10, 112 << 10, 320 << 10, 1 << 20] {
        let class = large(size);
        let blocks: Vec<NonNull<u8>> = (0..3)
            .map(|_| back.alloc_staggered(class, class.size() / 2).unwrap().ptr)
            .collect();
        let host = back.host(class).unwrap();
        let layout = *host.layout();
        for &block in &blocks {
            let location = back.space().locate(block.as_ptr()).unwrap();
            let region = location.region;
            let (slot, _) = layout.block_at(location.offset as usize).unwrap();
            let unit = layout.slot_unit(slot).unwrap();
            let word = |w: u32| host.meta(region, layout.clean_word(w));
            let saved: Vec<u64> = (0..layout.clean_words()).map(|w| word(w).load()).collect();
            for trial in 0..400 {
                let before: Vec<u64> = (0..layout.clean_words())
                    .map(|w| {
                        let bits = clean_shape(&mut rng);
                        word(w).store(bits);
                        bits
                    })
                    .collect();
                let asked = match rng.below(8) {
                    0 => 0,
                    1 => 1,
                    2 => class.size(),
                    _ => rng.below(class.size() + 1),
                };
                let clean = (unit.start..unit.end)
                    .filter(|&p| marked(&before, p))
                    .count();
                let written = layout.slot_bytes(slot) + layout.stagger(slot) + asked;
                let extent = (written.div_ceil(page) as u32).clamp(unit.start, unit.end);
                assert_eq!(
                    region::charge(&host, region, slot),
                    class.size() - clean * page,
                    "{size} slot {slot} trial {trial}: the charge"
                );
                // The last tenant's size, and so what the claim keeps: the slack is the pages
                // past both extents that were not clean, from the lowest to the highest.
                let last = super::super::layout::Recorded {
                    size: match rng.below(4) {
                        0 => 0,
                        1 => class.size(),
                        _ => rng.below(class.size() + 1),
                    },
                    huge: false,
                    unwritten: false,
                    outgrown: false,
                    freed: 0,
                };
                let kept = layout.slot_bytes(slot) + layout.stagger(slot) + last.size;
                let keep = (kept.div_ceil(page) as u32)
                    .clamp(unit.start, unit.end)
                    .max(extent);
                let dirty: Vec<u32> = (keep..unit.end).filter(|&p| !marked(&before, p)).collect();
                let slack = dirty.first().map(|&low| (low, dirty[dirty.len() - 1] + 1));
                let settled = region::settle_slot(&host, region, slot, asked, Some(last));
                assert_eq!(
                    (settled.resident, settled.revived, settled.zeroed),
                    (class.size() - clean * page, 0, 0),
                    "{size} slot {slot} trial {trial} for {asked}: {before:x?}"
                );
                assert_eq!(
                    settled.slack.map(|u| (u.start, u.end)),
                    slack,
                    "{size} slot {slot} trial {trial} for {asked} after {}: {before:x?}",
                    last.size
                );
                let after: Vec<u64> = (0..layout.clean_words()).map(|w| word(w).load()).collect();
                for p in 0..layout.region_pages() {
                    let under = p >= unit.start && p < extent;
                    assert_eq!(
                        marked(&after, p),
                        marked(&before, p) && !under,
                        "{size} slot {slot} trial {trial} for {asked}: page {p} of unit \
                         {unit:?}, extent {extent}"
                    );
                }
            }
            for (w, &bits) in saved.iter().enumerate() {
                word(w as u32).store(bits);
            }
        }
        for block in blocks {
            back.free(block, class);
        }
    }
}

/// `Space::metadata_at` is the region's block plus the offset, for every region of the plan,
/// the last one's last word still inside the metadata blocks; on a reservation that got its
/// full region size and on one that settled for the next size down.
fn metadata_words_are_the_blocks<S: Shape>(failed_reserves: u32) {
    let state = state::<S>();
    state.os().set_faults(
        Call::Reserve,
        Faults {
            pass: 0,
            fail: failed_reserves,
            partial: false,
        },
    );
    let back = state.get_or_reserve().unwrap();
    let space = back.space();
    let plan = space.plan();
    assert_eq!(
        plan.region_shift(),
        S::GEOMETRY.max_region_shift() - failed_reserves
    );
    let size = plan.metadata_size();
    let base = space.base().addr().get();
    let mut next = base + plan.metadata_start();
    for region in plan.regions() {
        let block = space.metadata(region).addr().get();
        assert_eq!(block, next, "the blocks are back to back");
        for offset in [0, 8, size / 2, size - 8] {
            let at = space.metadata_at(region, offset);
            assert_eq!(at.addr(), block + offset, "{region:?} + {offset}");
            assert!(at.addr() + 8 <= base + plan.metadata_end());
        }
        next = block + size;
    }
    assert_eq!(next, base + plan.metadata_end());
}

#[test]
fn metadata_words_are_the_blocks_in_every_plan() {
    metadata_words_are_the_blocks::<Standard>(0);
    metadata_words_are_the_blocks::<Standard>(1);
    metadata_words_are_the_blocks::<crate::os::sim::Tiny>(0);
}

#[test]
#[cfg_attr(miri, ignore)]
fn metadata_words_are_the_blocks_in_the_native_plans() {
    metadata_words_are_the_blocks::<Native>(0);
    metadata_words_are_the_blocks::<Native>(1);
    metadata_words_are_the_blocks::<Native>(3);
}

/// The summary words above leaf word `leaf` of `region`, lowest first, each with the bit that
/// stands for the word below: the region's own levels, the class word and the class's top.
fn path<H: Host>(host: &H, region: crate::space::RegionId, leaf: u32) -> Vec<(&H::W, u64)> {
    let layout = host.layout();
    let mut path = Vec::new();
    let mut index = leaf;
    for level in 1..=layout.height() {
        let bit = 1u64 << (index % BITS);
        index /= BITS;
        path.push((host.meta(region, layout.tree_word(level, index)), bit));
    }
    let r = region.index() as u32;
    path.push((host.class_word(r / BITS), 1 << (r % BITS)));
    path.push((host.class_top(), 1 << (r / BITS % BITS)));
    path
}

/// A single free (`tree::release_one`) on a tree with two summary levels of its own and the
/// class's two: from an all-clear path it sets every level's bit and no other; and with any
/// one level's bit cleared behind it, as a thread lost in a fork leaves it, the next single
/// free below sets it again, whether or not its leaf word was empty.
#[test]
#[cfg_attr(miri, ignore)]
fn a_single_free_sets_and_repairs_every_level() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let class = small(16);
    let host = back.host(class.class()).unwrap();
    let layout = *host.layout();
    assert!(
        layout.height() >= 2,
        "two summary levels of the region's own"
    );
    // Past one level-1 word's worth of leaf words, so that the path is not all first bits.
    let n = (BITS * BITS + 3 * BITS + 5) as usize;
    let slots = fill(&back, class, n);
    assert_eq!(slots.len(), n);
    let location = back.space().locate(slots[n - 1]).unwrap();
    let region = location.region;
    assert_eq!(back.space().locate(slots[0]).unwrap().region, region);
    let leaf = (n as u32 - 1) / BITS;
    let levels = path(&host, region, leaf);
    assert_eq!(levels.len(), layout.height() as usize + 2);
    let free = |p: *mut u8| {
        // SAFETY: the test holds the slot and gives it up.
        unsafe { back.launder(class, &[p]) }
    };
    for (word, _) in &levels {
        assert_eq!(word.load(), 0, "nothing is free, so no summary bit is set");
    }
    free(slots[n - 1]);
    for (level, (word, bit)) in levels.iter().enumerate() {
        assert_eq!(word.load(), *bit, "level {}", level + 1);
    }
    let whole = |what: &str, lost: usize| {
        for (level, (word, bit)) in levels.iter().enumerate() {
            assert_eq!(
                word.load(),
                *bit,
                "level {} after level {} lost its bit, {what}",
                level + 1,
                lost + 1
            );
        }
    };
    // One level's bit lost at a time. Here the last slot is free, the one before it held, and
    // the path set.
    for lost in 0..levels.len() {
        // A free into a leaf word that is not empty.
        let (word, bit) = levels[lost];
        word.fetch_and(!bit);
        free(slots[n - 2]);
        whole("the leaf word not empty", lost);
        assert_eq!(fill(&back, class, 1), [slots[n - 2]], "the slot is found");
        // A free into an empty leaf word under bits that are set but for the lost one: stale
        // hints, which the protocol allows over nothing free.
        assert_eq!(fill(&back, class, 1), [slots[n - 1]]);
        for (at, (word, bit)) in levels.iter().enumerate() {
            assert_eq!(word.load(), 0, "the claim cleared its path");
            if at != lost {
                word.fetch_or(*bit);
            }
        }
        free(slots[n - 2]);
        whole("the leaf word empty", lost);
        assert_eq!(fill(&back, class, 1), [slots[n - 2]], "the slot is found");
        for (word, _) in &levels {
            assert_eq!(word.load(), 0, "the claim cleared its path");
        }
        free(slots[n - 1]);
        whole("and was set again", lost);
    }
}

/// The retained count is what was counted in less what was counted out, as two tallies that
/// wrap: the difference is right across the wrap of either, and a claim counted out before
/// the free it answers is counted in reads as nothing retained, not as a huge count.
#[test]
fn the_tallies_wrap_and_a_subtraction_ahead_reads_as_zero() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let class = large(40 << 10);
    let size = class.size();
    for large in [false, true] {
        state
            .retained
            .added
            .of(large)
            .store(usize::MAX - 7, Relaxed);
        state
            .retained
            .taken
            .of(large)
            .store(usize::MAX - 7, Relaxed);
    }
    assert_eq!((back.retained(), back.retained_large()), (0, 0));
    let block = back.alloc(class, size).unwrap().ptr;
    paint(block.as_ptr(), size, 1);
    back.free(block, class);
    assert_eq!(state.retained.added.of(true).load(Relaxed), size - 8);
    assert_eq!((back.retained(), back.retained_large()), (size, size));
    assert_eq!(back.alloc(class, size).unwrap().ptr, block);
    assert_eq!(state.retained.taken.of(true).load(Relaxed), size - 8);
    assert_eq!((back.retained(), back.retained_large()), (0, 0));
    // A claim counted out first: the large count is negative, and reads as zero beside a
    // small count that is not.
    back.unretain(class, size);
    let slots = fill(&back, small(1024), 4);
    // SAFETY: the test holds these slots and gives them up.
    unsafe { back.launder(small(1024), &slots) };
    assert_eq!(state.retained.signed(true), -(size as isize));
    assert_eq!((back.retained(), back.retained_large()), (4 * 1024, 0));
    back.retain(class, size);
    assert_eq!((back.retained(), back.retained_large()), (4 * 1024, 0));
}

/// A heartbeat that finds a pass due and nothing retained ends the pass as it starts: no pass
/// is left under way, the heartbeats of the interval after it change nothing, and memory freed
/// in that interval is returned by the first pass an interval after the empty one, not by the
/// next heartbeat. (Before `6ff0a8e` a heartbeat with nothing retained left the decay word
/// alone, so the first heartbeat after a free found a pass long overdue and purged at once.)
#[test]
#[cfg_attr(miri, ignore)]
fn an_empty_pass_ends_at_once_and_the_next_is_an_interval_later() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let word = || state.decay.word.load(Relaxed);
    let class = small(1024);
    let slots = fill(&back, class, 256);
    for &p in &slots {
        paint(p, 1024, 9);
    }
    // Five intervals of the clock with no heartbeat are the first counted interval
    // (`crate::clock`), so the decay word holds the counted time, one interval.
    os.advance_clock(5 * DECAY_TICKS);
    back.heartbeat();
    assert_eq!(word(), DECAY_TICKS, "the empty pass ended when it began");
    os.advance_clock(DECAY_TICKS / 2);
    beat(&back);
    assert_eq!(word(), DECAY_TICKS, "nothing is due inside the interval");
    // SAFETY: the test holds these slots and gives them up.
    unsafe { back.launder(class, &slots) };
    assert_eq!(back.retained(), 256 * 1024);
    let purges = os.calls(Call::Purge);
    beat(&back);
    os.advance_clock(DECAY_TICKS / 2 - 1);
    beat(&back);
    assert_eq!(os.calls(Call::Purge), purges, "purged inside the interval");
    assert_eq!(word(), DECAY_TICKS);
    os.advance_clock(1);
    beat(&back);
    assert!(os.calls(Call::Purge) > purges, "the pass an interval later");
    assert!(!pass_under_way(&state));
    assert!(back.retained() <= 256 * 1024 - 256 * 1024 / 4);
    // With nothing left, every interval is one empty pass and no walk.
    for _ in 0..40 {
        os.advance_clock(DECAY_TICKS);
        beat(&back);
    }
    assert_eq!(back.retained(), 0);
    let laps = state.walk.laps.load(Relaxed);
    let now = word();
    for round in 1..=3 {
        os.advance_clock(DECAY_TICKS);
        beat(&back);
        assert!(!pass_under_way(&state));
        assert_eq!(word(), now + round * DECAY_TICKS, "one stamp an interval");
        assert_eq!(
            state.walk.laps.load(Relaxed),
            laps,
            "an empty pass walks nothing"
        );
    }
}
