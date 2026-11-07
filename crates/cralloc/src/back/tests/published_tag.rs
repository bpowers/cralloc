//! The published tag (`back::tags`): the walks that find regions by their number
//! (`Back::hold_region`, the hold's plan, `Back::lower_walked`, `Back::raise_marked`,
//! `Back::inspect`) take a region's class from it, and only the thread whose carving swing named
//! the region writes it, after the swing.
//!
//! A walk that read the class from the table could read it while a thread that lost the race for
//! its class's carving word held the region, before it gave the region back. Descheduled from
//! there until another class had the region, it would go on with the first class's host and
//! layout.
//!
//! No test can put a thread between two loads of a walk, so the tests play the steps by hand: the
//! loser's as `Back::assign` makes them, and the walk's read of the class, then the walk itself
//! once another class has the region.

use super::super::hold::{Reach, held_for};
use super::super::policy::{FINE_TICKS, HOLD_STEPS};
use super::super::region;
use super::super::tree::Host;
use super::super::word::Word;
use super::*;
use crate::space::RegionId;
use core::sync::atomic::Ordering::SeqCst;

/// The loser's steps in `Back::assign` for `class`, up to its tag: the fixed metadata (and a
/// large class's first slot) committed for the lowest unassigned region, the walks' bound
/// raised, and the tag taken. Returns the region, which the loser then gives back.
fn lose_a_region<O: Os>(back: &Back<'_, O>, class: Class) -> RegionId {
    let host = back.host(class).unwrap();
    let region = back
        .space()
        .plan()
        .regions()
        .find(|&r| back.tag(r) == RegionTag::Unassigned)
        .unwrap();
    let (start, end) = host.layout().fixed_commit();
    assert!(host.commit_meta(region, start, end - start));
    if !class.is_small() {
        assert!(region::commit_first(&host, region).is_some());
    }
    back.state
        .walk
        .top
        .fetch_max(region.index() as u32 + 1, SeqCst);
    assert!(back.state.tags.take(region, RegionTag::Class(class)));
    region
}

/// A look of the hold reads a region's class while a thread that lost the race for its class's
/// carving word still holds the region. It finds no class for the region, and the hold's look, run
/// once another large class has the region, walks it as that class's. A look that read the table
/// would find the loser's class and run again once the other class had taken the region, allocated
/// from it and freed a slot: two large classes with a leaf word each keep their bitmaps and size
/// words at the same offsets, so it would take the second class's free slot 1 for the first
/// class's and return the first class's slot 1, the pages of the second class's live slots 2 and 3.
#[test]
#[cfg_attr(miri, ignore)]
fn a_look_at_a_region_given_back_returns_none_of_the_next_classes_live_slots() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let (big, little) = (large(128 << 10), large(64 << 10));
    assert_eq!(big.size(), 2 * little.size());
    // The winner of the race for `big`'s carving word publishes its region.
    let _first = back.alloc(big, big.size()).unwrap().ptr;
    let region = lose_a_region(&back, big);
    // The look's first step in `hold_region`: the region's class. The loser has not named the
    // region, so nothing is published, and the look passes it by.
    assert_eq!(back.state.tags.published(region), None);
    // The loser finds the carving word published and gives the region back. Its header is
    // zero again (`unprepare`); here nothing wrote it.
    back.state.tags.give_back(region, RegionTag::Class(big));
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
    assert_eq!(back.state.tags.published(region), Some(little));
    // The look itself, at a count by which the region's stamp is held for the hold.
    let host = back.host(little).unwrap();
    let stamp = host.meta(region, host.layout().hold_word().unwrap()).load();
    assert_ne!(stamp, 0);
    let count = back.hold_count().wrapping_add(HOLD_STEPS);
    let reach = Reach::beat(Native::GEOMETRY);
    assert!(held_for(stamp, count, reach.steps));
    let _ = back.hold_region(region.index(), count, usize::MAX, usize::MAX, reach);
    let again = back.alloc(big, big.size()).unwrap().ptr;
    let mut wrong = Vec::new();
    if (0..4).any(|i| i != 1 && !painted(blocks[i].as_ptr(), size, 7)) {
        wrong.push("a live block of the little class was returned");
    }
    if back.space().locate(again.as_ptr()).unwrap().region == region {
        wrong.push("the big class was handed a slot in the little class's region");
    }
    assert!(wrong.is_empty(), "{wrong:?}");
}

/// The same race with a small class taking the region the large class's loser gave back: a
/// look that took the large class would have walked the small class's bitmap as the large
/// class's, setting and clearing its summary and clean bits and returning pages of its live
/// objects. The look finds no class while the loser holds the region, and the small class
/// once it has the region, which the hold passes by (small classes have no stamp).
#[test]
#[cfg_attr(miri, ignore)]
fn a_region_given_back_to_a_small_class_is_the_small_classs_to_the_walks() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let (big, tiny) = (large(128 << 10), small(64));
    let _first = back.alloc(big, big.size()).unwrap().ptr;
    let region = lose_a_region(&back, big);
    assert_eq!(back.state.tags.published(region), None);
    back.state.tags.give_back(region, RegionTag::Class(big));
    let objects: Vec<NonNull<u8>> = (0..64)
        .map(|_| back.alloc(tiny.class(), tiny.size()).unwrap().ptr)
        .collect();
    assert!(
        objects
            .iter()
            .all(|o| back.space().locate(o.as_ptr()).unwrap().region == region)
    );
    for o in &objects {
        paint(o.as_ptr(), tiny.size(), 7);
    }
    assert_eq!(back.state.tags.published(region), Some(tiny.class()));
    let count = back.hold_count().wrapping_add(HOLD_STEPS);
    let reach = Reach::beat(Native::GEOMETRY);
    let (_, purged, done, _) =
        back.hold_region(region.index(), count, usize::MAX, usize::MAX, reach);
    assert_eq!((purged, done), (0, true));
    for _ in 0..4 {
        os.advance_clock(DECAY_TICKS);
        back.heartbeat();
    }
    assert!(objects.iter().all(|o| painted(o.as_ptr(), tiny.size(), 7)));
}

/// The inspection is a walk too. With two small classes, one that took the class of a region
/// its loser held would read the leaf words of a class that never committed them once the
/// other class had the region. It sees the region only once a class has named it, and then
/// as that class's.
#[test]
#[cfg_attr(miri, ignore)]
fn the_inspection_sees_a_region_only_as_the_class_that_named_it() {
    use crate::stats::{Region, Visitor};
    #[derive(Default)]
    struct Seen(Vec<(Class, usize)>);
    impl Visitor for Seen {
        fn region(&mut self, region: &Region) {
            self.0.push((region.class, region.index));
        }
    }
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let (first, second) = (small(16), small(1024));
    let _a = back.alloc(first.class(), first.size()).unwrap().ptr;
    let region = lose_a_region(&back, first.class());
    let mut seen = Seen::default();
    back.inspect(&mut seen);
    assert!(seen.0.iter().all(|&(_, index)| index != region.index()));
    back.state
        .tags
        .give_back(region, RegionTag::Class(first.class()));
    let b = back.alloc(second.class(), second.size()).unwrap().ptr;
    assert_eq!(back.space().locate(b.as_ptr()).unwrap().region, region);
    let mut seen = Seen::default();
    back.inspect(&mut seen);
    assert!(seen.0.contains(&(second.class(), region.index())));
    assert!(!seen.0.contains(&(first.class(), region.index())));
}

/// In a fork's child the repair publishes each region a carving word named whose namer the
/// fork cut off before it published: the class's carving region, and one it has carved from
/// whose carving word has moved on. A region tagged and never named stays unpublished.
#[test]
#[cfg_attr(miri, ignore)]
fn a_fork_child_publishes_the_regions_named_and_not_published() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let class = large(64 << 10);
    let host = back.host(class).unwrap();
    // A region named by the carving word, carved from, its publication undone as a fork that
    // cut its namer off after the swing would leave it.
    let first = back.alloc(class, class.size()).unwrap().ptr;
    let named = back.space().locate(first.as_ptr()).unwrap().region;
    back.state.tags.unpublish(named);
    // A second region, tagged by a thread that has not swung the carving word.
    let tagged = lose_a_region(&back, class);
    assert_eq!(
        host.class.carving.load(SeqCst),
        named.index() as u32 + 1,
        "the carving word still names the first region"
    );
    back.publish_named();
    assert_eq!(back.state.tags.published(named), Some(class));
    assert_eq!(back.state.tags.published(tagged), None);
    // The carving word moves on to a third region and the first, carved from, loses its byte
    // again: the repair still publishes it.
    back.state.tags.give_back(tagged, RegionTag::Class(class));
    back.state.tags.unpublish(named);
    let next = back.state.tags.published(named);
    assert_eq!(next, None);
    host.class.carving.store(0, SeqCst);
    back.publish_named();
    assert_eq!(back.state.tags.published(named), Some(class));
}

/// The fork child's repair beside a thread of the child's own that loses an assignment
/// (`tla/Assign.tla`, `Assign_repair`): the repair reads the loser's tag, and by the time it asks
/// whether the region was carved from, another class has named the region and carved, and its namer
/// has not yet published. Read through the first class's host, the carve is seen: two small classes
/// share the region's header, and a large class's header is where a small class's top summary word
/// sits, set once one of its slots is released. The repair reads the tag again and publishes
/// nothing for the first class.
#[test]
#[cfg_attr(miri, ignore)]
fn the_repair_does_not_publish_a_region_for_a_class_whose_tag_it_read_stale() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let (first, big) = (small(16), large(64 << 10));
    let _a = back.alloc(first.class(), first.size()).unwrap().ptr;
    let _b = back.alloc(big, big.size()).unwrap().ptr;
    // A second small class for each case, which has no region yet.
    for (stale, second) in [(first.class(), small(1024)), (big, small(512))] {
        let region = lose_a_region(&back, stale);
        // The repair's read of the tag.
        assert_eq!(back.tag(region), RegionTag::Class(stale));
        back.state.tags.give_back(region, RegionTag::Class(stale));
        // The second class names the region and carves; a slot of it is released, which sets
        // its summary words; its namer has not published yet.
        let objects: Vec<NonNull<u8>> = (0..4)
            .map(|_| back.alloc(second.class(), second.size()).unwrap().ptr)
            .collect();
        let regions: Vec<usize> = objects
            .iter()
            .map(|o| back.space().locate(o.as_ptr()).unwrap().region.index())
            .collect();
        assert!(
            regions.iter().all(|&r| r == region.index()),
            "{stale:?}: the second class's objects in {regions:?}, not {}",
            region.index()
        );
        back.free(objects[0], second.class());
        back.state.tags.unpublish(region);
        // What the first class's host reads there is a carve.
        let host = back.host(stale).unwrap();
        assert_ne!(
            super::super::layout::Grow::unpack(host.grow(region).load()).carved,
            0,
            "{stale:?}'s host sees a carve"
        );
        back.publish_if_read(region, stale);
        assert_eq!(
            back.state.tags.published(region),
            None,
            "published for {stale:?}"
        );
        // The namer, or the repair reading the tag as it is, publishes the right class.
        back.publish_named();
        assert_eq!(back.state.tags.published(region), Some(second.class()));
    }
}
