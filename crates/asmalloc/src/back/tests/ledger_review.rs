//! Evidence for the ledger review of `back/class.rs`, `back/host.rs` and `back/region.rs` at
//! `d36e0cb`.
//!
//! The `ClassHost` invariant (`docs/safety/asmalloc/back/host-safety.md`) says a host is only
//! given regions the table tags with its class, and that the one tag change away from a class,
//! `Back::assign` giving back a region it took and could not publish, is of a region "no other
//! thread has named". The walks that find regions by reading the table (`Back::hold_region`,
//! `Back::lower_walked`, `Back::raise_marked`, `Back::inspect`) can name it: they read the tag
//! while the loser holds it. A walk that is then descheduled until another class has taken the
//! region goes on with the first class's host.
//!
//! No test can put a thread between two loads of `hold_region`, so the test here plays the
//! steps by hand: the loser's as `Back::assign` makes them, and the look's as `hold_region`
//! makes them after its read of the tag.

use super::super::hold::{Reach, held_for};
use super::super::policy::{FINE_TICKS, HOLD_STEPS};
use super::super::purge::Scope;
use super::super::region;
use super::super::tree::Host;
use super::super::word::Word;
use super::*;
use core::sync::atomic::Ordering::SeqCst;

/// A look of the hold reads a region's tag while a thread that lost the race for its class's
/// carving word still holds the region, and runs again once another large class has taken the
/// region, allocated from it and freed a slot. It walks the region with the first class's
/// layout. Two large classes with a leaf word each keep their bitmaps and size words at the
/// same offsets, so the look takes the second class's free slot 1 for the first class's, and
/// returns the first class's slot 1: the pages of the second class's slots 2 and 3, which are
/// live.
///
/// Ignored because it fails (`scripts/gate --all` runs ignored tests and will fail on it until
/// the fix). It replays `hold_region` as it stood at `d36e0cb`: the look takes its class from
/// the table. A fix that gives scanners another source for a region's class changes the step
/// marked "the look's first step" below, which should then find no class for the region.
#[test]
#[cfg_attr(miri, ignore)]
#[ignore = "fails: a look with a stale class returns another class's live slots"]
fn a_look_that_read_the_tag_of_a_region_given_back_returns_another_classes_live_slots() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let (big, little) = (large(128 << 10), large(64 << 10));
    assert_eq!(big.size(), 2 * little.size());
    // The winner of the race for `big`'s carving word publishes its region.
    let _first = back.alloc(big, big.size()).unwrap().ptr;
    // The loser, as `Back::assign` goes: it commits the fixed metadata and the first slot of
    // the lowest unassigned region, raises the walks' bound and takes the tag.
    let host = back.host(big).unwrap();
    let region = back
        .space()
        .plan()
        .regions()
        .find(|&r| back.state.table.get(r) == RegionTag::Unassigned)
        .unwrap();
    let (start, end) = host.layout().fixed_commit();
    assert!(host.commit_meta(region, start, end - start));
    assert!(region::commit_first(&host, region).is_some());
    back.state
        .walk
        .top
        .fetch_max(region.index() as u32 + 1, SeqCst);
    back.state
        .table
        .replace(region, RegionTag::Unassigned, RegionTag::Class(big))
        .unwrap();
    // The look's first step in `hold_region`: the tag. Then it is descheduled.
    let RegionTag::Class(seen) = back.state.table.get(region) else {
        panic!("the loser holds the tag");
    };
    assert_eq!(seen, big);
    // The loser finds the carving word published and gives the region back. Its header is
    // zero again (`unprepare`); here nothing wrote it.
    back.state
        .table
        .replace(region, RegionTag::Class(big), RegionTag::Unassigned)
        .unwrap();
    // Another large class takes the region, fills four slots and frees the second.
    let size = little.size();
    let blocks: Vec<NonNull<u8>> = (0..4)
        .map(|_| back.alloc(little, size).unwrap().ptr)
        .collect();
    for b in &blocks {
        assert_eq!(back.space().locate(b.as_ptr()).unwrap().region, region);
        paint(b.as_ptr(), size, 7);
    }
    for _ in 0..2 {
        os.advance_clock(FINE_TICKS);
        back.heartbeat();
    }
    back.free(blocks[1], little);
    // The look runs again, with the class it read: the rest of `hold_region`.
    let host = back.host(seen).unwrap();
    let word = host.meta(region, host.layout().hold_word().unwrap());
    let stamp = word.load();
    assert_ne!(stamp, 0, "the other class's stamp, at the same word");
    let count = back.hold_count().wrapping_add(HOLD_STEPS);
    let reach = Reach::beat(Native::GEOMETRY);
    assert!(held_for(stamp, count, reach.steps));
    // `hold_region` asks only whether this is `None`, and it is the other class's region.
    assert_eq!(back.purgeable(region), Some(little));
    let (purged, _, _, _) = back.purge_region(
        seen,
        region,
        usize::MAX,
        usize::MAX,
        true,
        Scope::All,
        false,
        true,
    );
    // The look's release walked up the big class's tree, so the big class now finds a free
    // slot in the little class's region.
    let again = back.alloc(big, big.size()).unwrap().ptr;
    let mut wrong = Vec::new();
    if purged != 0 {
        wrong.push("the look returned pages of another class's region");
    }
    if (0..4).any(|i| i != 1 && !painted(blocks[i].as_ptr(), size, 7)) {
        wrong.push("a live block of the little class was returned");
    }
    if back.space().locate(again.as_ptr()).unwrap().region == region {
        wrong.push("the big class was handed a slot in the little class's region");
    }
    assert!(wrong.is_empty(), "{purged} bytes returned: {wrong:?}");
}
