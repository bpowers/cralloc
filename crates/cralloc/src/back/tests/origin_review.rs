//! Review of the small classes' origins (`Plan::slot_origin`, merged as `f361b53`): slot `i`
//! of a small class is at `region_base + origin + i * size`, and the pages below the origin
//! are meant never to be committed, purged or handed out.
//!
//! The review found no input for which the arithmetic is wrong. What it found is that most of
//! the tests cannot see an origin at all: the `Standard` and `Tiny` shapes, which the bulk of
//! the back end's tests, Miri, the testkit's simulated suites and the tiny-heap Kani harnesses
//! run on, have regions too small for one, so every origin there is zero
//! (`every_simulated_shape_has_a_class_with_an_origin`, ignored because it fails). Mutating
//! the origin out of one use at a time showed which uses no test noticed; the tests here are
//! the ones that notice, on the shapes that do have origins: the design's geometry at every
//! region size (arithmetic only, and on the real OS), and `Native`.
//!
//! - nothing checked that the pages below an origin stay uncommitted, the claim the design
//!   makes for them (committing from the region's base passed every test):
//!   `a_class_with_an_origin_keeps_everything_above_it`,
//!   `a_later_region_keeps_everything_above_its_origin`, and `design::` on the real OS;
//! - no test handed the back end a pointer below an origin:
//!   `freeing_*_below_the_origin_aborts`, `lookups_below_the_origin_find_nothing`;
//! - the purge walk's units were checked on `Native` only, whose origins are at most three
//!   pages against huge pages of sixteen, and the check allowed gaps between units:
//!   `purge_passes_cover_every_carved_page_above_the_origin_in_the_design_geometry`;
//! - the slot arithmetic of `Layout`, which is what the allocator runs (`Plan::slot_at` is
//!   used by tests and proofs only), was checked against `Plan`'s at the design's largest and
//!   smallest region sizes only:
//!   `every_slot_is_found_from_its_address_at_every_design_region_size`.

use super::super::layout::{Grow, Hints, Layout, Step};
use super::super::tree::Host;
use super::super::word::Word;
use super::*;
use crate::os::sim::Tiny;
use crate::space::{Geometry, Plan, RegionId};

/// Every region size the design's geometry allows.
fn design_plans() -> impl Iterator<Item = Plan> {
    let g = Geometry::DESIGN;
    (g.min_region_shift()..=g.max_region_shift()).filter_map(move |shift| g.plan(shift))
}

/// Finding, fixed: the shapes most tests run on had no origins. `Standard` (the default of the
/// back end's tests, of `SimHeap`, and so of Miri and the testkit's simulated suites) has
/// regions of 256 KiB, and `Tiny` (the Kani harnesses that run `touching`, `inside` and
/// `slot_unit` for every class) of 2 KiB; a sixty-fourth of either was at most one page, so
/// every origin rounded to zero. Origins now lie in the first `ORIGIN_SHARE`th of a region,
/// a few pages in both.
#[test]
fn every_simulated_shape_has_a_class_with_an_origin() {
    for (name, plan) in [
        ("Standard", Standard::GEOMETRY.largest_plan()),
        ("Native", Native::GEOMETRY.largest_plan()),
        ("Tiny", Tiny::GEOMETRY.largest_plan()),
    ] {
        let with = Class::all()
            .filter(|&c| plan.fits_region(c) && plan.slot_origin(c) != 0)
            .count();
        assert!(with > 0, "{name}: every class's origin is zero");
    }
}

/// `Layout`'s slot arithmetic, which every allocation, free and purge uses, agrees with
/// `Plan`'s and inverts itself, for every class at every region size of the design: slot
/// starts are found again, no other byte of a small slot is a block, nothing below the origin
/// is, and the last slot ends inside the region.
#[test]
fn every_slot_is_found_from_its_address_at_every_design_region_size() {
    for plan in design_plans() {
        let page = plan.geometry().page_size();
        let region = plan.region(2).unwrap();
        let at_region = plan.region_offset(region);
        for class in Class::all().filter(|&c| plan.fits_region(c)) {
            let layout = Layout::new(plan, class).unwrap();
            let (size, origin, slots) = (class.size(), plan.slot_origin(class), layout.slots());
            let what = std::format!("{class:?} in regions of 2^{}", plan.region_shift());
            assert_eq!(layout.slot_bytes(0), origin, "{what}");
            assert_eq!(layout.origin_page() as usize * page, origin, "{what}");
            assert_eq!(origin % class.align(), 0, "{what}");
            assert!(origin == 0 || class.is_small(), "{what}");
            assert!(
                origin == 0 || origin < plan.region_size() / crate::space::ORIGIN_SHARE,
                "{what}"
            );
            assert_eq!(slots, plan.slots_per_region(class), "{what}");
            assert!(slots >= 1, "{what}");
            assert!(layout.slot_bytes(slots) <= plan.region_size(), "{what}");
            assert!(
                layout.slot_bytes(slots) + size > plan.region_size(),
                "{what}"
            );
            let picks = [
                0,
                1,
                2,
                63,
                64,
                65,
                slots / 2,
                slots.saturating_sub(2),
                slots - 1,
            ];
            for index in picks.into_iter().filter(|&i| i < slots) {
                let at = layout.slot_bytes(index);
                assert_eq!(
                    plan.slot_offset(region, class, index),
                    at_region + at,
                    "{what}"
                );
                assert_eq!(at % class.align().min(plan.region_size()), 0, "{what}");
                assert!(plan.slot_fits(class, index), "{what}");
                assert_eq!(layout.slot_at(at), index, "{what}");
                assert_eq!(layout.slot_at(at + size - 1), index, "{what}");
                assert_eq!(layout.block_at(at), Some((index, 0)), "{what}");
                let location = plan.locate(at_region + at).unwrap();
                assert_eq!(plan.slot_at(location, class), Some(index), "{what}");
                if class.is_small() {
                    for inside in [1, 8, size / 2, size - 8, size - 1] {
                        if inside != 0 && inside < size {
                            assert_eq!(layout.block_at(at + inside), None, "{what} +{inside}");
                        }
                    }
                }
            }
            assert!(!plan.slot_fits(class, slots), "{what}");
            let end = layout.slot_bytes(slots);
            if end < plan.region_size() {
                // The tail is no slot: `block_at` leaves the count to its callers.
                assert!(layout.block_at(end).is_none_or(|(slot, _)| slot >= slots));
                let location = plan.locate(at_region + end).unwrap();
                assert_eq!(plan.slot_at(location, class), None, "{what}");
            }
            if origin == 0 {
                continue;
            }
            let below = [
                0,
                8,
                size,
                page,
                origin.saturating_sub(size),
                origin - page,
                origin - 8,
                origin - 1,
            ];
            for offset in below.into_iter().filter(|&o| o < origin) {
                assert_eq!(layout.block_at(offset), None, "{what} at {offset}");
                let location = plan.locate(at_region + offset).unwrap();
                assert_eq!(plan.slot_at(location, class), None, "{what} at {offset}");
            }
        }
    }
}

/// One top-down pass over a region with `carved` slots: the units lie at or above the origin
/// and below the watermark, touch only carved slots, and leave no gap. A unit ends where the
/// one above it started; the only pages a pass may leave are the part of a huge page under
/// the watermark, and the part of one that the huge-page part does not wholly hold.
fn check_pass(layout: &Layout, carved: u32, huge_from: Option<u32>, what: &str) {
    let top = layout.carved_pages(carved);
    let granule = layout.huge_pages();
    let origin = layout.origin_page();
    let page = layout.page_size();
    let mut cursor = 0;
    let mut covered_to = top;
    loop {
        match layout.unit_below(carved, huge_from, cursor) {
            Step::Done => {
                assert!(
                    covered_to <= origin,
                    "{what}: done with pages {origin}..{covered_to} not looked at"
                );
                break;
            }
            Step::Skip(next) => {
                assert_eq!(Some(next), huge_from, "{what}");
                assert!(next < covered_to, "{what}");
                // Only less than a huge page is passed over.
                assert!(
                    covered_to & !(granule - 1) <= next,
                    "{what}: skipped a huge page"
                );
                covered_to = next;
                if next == 0 {
                    break;
                }
                cursor = next;
            }
            Step::Unit(unit) => {
                assert!(unit.start < unit.end, "{what}: {unit:?}");
                assert!(unit.start >= origin, "{what}: {unit:?} below the origin");
                let in_huge = huge_from.is_some_and(|h| unit.end > h);
                assert!(
                    unit.end == covered_to || in_huge && unit.end == covered_to & !(granule - 1),
                    "{what}: {unit:?} leaves a gap below {covered_to}"
                );
                if let Some(h) = huge_from.filter(|_| in_huge) {
                    assert!(unit.start >= h, "{what}: {unit:?}");
                    assert_eq!(unit.end % granule, 0, "{what}: {unit:?}");
                    assert!(
                        unit.start == h || unit.start == origin || unit.start % granule == 0,
                        "{what}: {unit:?}"
                    );
                }
                let (first, last) = layout.touching(unit);
                assert!(first <= last && last < carved, "{what}: {unit:?}");
                // A unit starts at the boundary at or below the start of the slot holding
                // its last byte, and no lower, or a live slot would hold more pages than it
                // is on.
                let (step, floor) = match huge_from {
                    Some(h) if in_huge => (granule, h.max(origin)),
                    _ => (1, origin),
                };
                let slot_page = (layout.slot_bytes(last) / page) as u32;
                assert_eq!(
                    unit.start,
                    (slot_page & !(step - 1)).max(floor),
                    "{what}: {unit:?}"
                );
                let (start, end) = (unit.start as usize * page, unit.end as usize * page);
                assert!(layout.slot_bytes(first) <= start, "{what}: {unit:?}");
                assert!(layout.slot_bytes(first + 1) > start, "{what}: {unit:?}");
                assert!(layout.slot_bytes(last) < end, "{what}: {unit:?}");
                assert!(layout.slot_bytes(last + 1) >= end, "{what}: {unit:?}");
                let (lo, hi) = layout.inside(unit);
                for slot in [first, first + 1, last.saturating_sub(1), last] {
                    if slot < first || slot > last {
                        continue;
                    }
                    let wholly =
                        layout.slot_bytes(slot) >= start && layout.slot_bytes(slot + 1) <= end;
                    assert_eq!(wholly, slot >= lo && slot < hi, "{what}: {unit:?} {slot}");
                }
                covered_to = unit.start;
                if unit.start == 0 {
                    break;
                }
                cursor = unit.start;
            }
        }
    }
}

/// The purge walk in the design's geometry, where an origin is up to 255 pages and a huge
/// page 512, so a huge-page part that starts at the region's base (every region of a dense
/// class after its first, `Back::prepare`) has its first huge page cut by the origin. The
/// existing walk check (`layout::tests::purge_passes_tile_the_carved_pages`) covers the
/// simulated shapes only and does not ask the units to meet.
#[test]
fn purge_passes_cover_every_carved_page_above_the_origin_in_the_design_geometry() {
    let g = Geometry::DESIGN;
    for plan in [
        g.largest_plan(),
        g.plan(26).unwrap(),
        g.smallest_plan(),
        Native::GEOMETRY.largest_plan(),
    ] {
        let page = plan.geometry().page_size();
        for class in Class::all().filter(|c| c.is_small()) {
            let layout = Layout::new(plan, class).unwrap();
            let huge = layout.huge_pages();
            let slots = layout.slots();
            // Watermarks this many pages past the origin, and a slot more and less.
            let pages = [0, 1, 2, 300, huge - 1, huge, huge + 3, 9 * huge + 7];
            for carved in pages.into_iter().flat_map(|p| {
                let at = (p as usize * page).div_ceil(class.size()) as u32;
                [at.saturating_sub(1), at, at + 1]
            }) {
                let carved = carved.min(slots);
                for huge_from in [None, Some(0), Some(huge), Some(8 * huge)] {
                    let huge_from = huge_from.filter(|&h| h < layout.region_pages());
                    let what = std::format!(
                        "{class:?}, regions of 2^{}, {carved} carved, huge from {huge_from:?}",
                        plan.region_shift()
                    );
                    check_pass(&layout, carved, huge_from, &what);
                }
            }
        }
    }
}

/// A small class of the `Native` shape whose origin is not zero, with its origin.
fn class_with_an_origin(plan: Plan, size: usize) -> (SmallClass, usize) {
    let class = small(size);
    let origin = plan.slot_origin(class.class());
    assert!(origin > 0, "pick a class with an origin");
    (class, origin)
}

/// Asserts that no page of a region below `origin` is writable or has been purged: nothing
/// committed it and no purge reached it.
fn nothing_below<S: Shape>(os: &Sim<S>, base: NonNull<u8>, origin: usize, when: &str) {
    let page = S::GEOMETRY.page_size();
    for at in (0..origin).step_by(page) {
        let start = base.map_addr(|a| a.saturating_add(at));
        assert!(
            !os.is_writable(start, 1),
            "{when}: the page {at} bytes into the region, below its origin of {origin}, is \
             writable"
        );
        // SAFETY: no other thread runs; the page is not writable, so nothing was stored in it.
        let purged = unsafe { os.is_purged(start, 1) };
        assert!(
            !purged,
            "{when}: the page {at} bytes into the region, below its origin, was purged"
        );
    }
}

/// The first region of a class with an origin, end to end on the simulated OS: slots are
/// handed out from the origin up, nothing below it is ever writable or purged, whatever the
/// region goes through (carving through the small-page part into the huge-page part, a
/// launder, a trim that purges all of it, reuse), and the retained count stays exact, which
/// it would not if a purge counted pages below the origin.
#[test]
#[cfg_attr(miri, ignore)]
fn a_class_with_an_origin_keeps_everything_above_it() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let (class, origin) = class_with_an_origin(back.space().plan(), 64);
    let size = class.size();
    let slots = fill(&back, class, 5000);
    assert_eq!(slots.len(), 5000);
    let region = back.space().locate(slots[0]).unwrap().region;
    let base = back.space().region(region);
    nothing_below(os, base, origin, "after carving");
    for (i, &p) in slots.iter().enumerate() {
        assert_eq!(p.addr(), base.addr().get() + origin + i * size, "slot {i}");
        assert_eq!(back.lookup(p), Some(Kind::Slot(class.class())));
        paint(p, size, 0x6b);
    }
    assert_eq!(
        back.space().slot(region, class.class(), 0).as_ptr(),
        slots[0]
    );
    // Scattered and dense batches, so that both the fast check and the full one run.
    let (odd, even): (Vec<_>, Vec<_>) = slots
        .iter()
        .copied()
        .enumerate()
        .partition(|&(i, _)| i % 2 == 1);
    for chunk in odd.chunks(64).chain(even.chunks(64).rev()) {
        let batch: Vec<*mut u8> = chunk.iter().map(|&(_, p)| p).collect();
        // SAFETY: the test holds these slots and gives them up.
        unsafe { back.launder(class, &batch) };
    }
    assert_eq!(back.retained(), 5000 * size);
    nothing_below(os, base, origin, "after laundering");
    back.trim();
    assert_eq!(back.retained(), 0);
    nothing_below(os, base, origin, "after a trim");
    // SAFETY: no other thread runs, and the back end and this test store only integers.
    unsafe {
        assert!(os.is_purged(NonNull::new(slots[0]).unwrap(), 5000 * size));
    }
    let again = fill(&back, class, 5000);
    assert_eq!(again, slots);
    assert!(again.iter().all(|&p| painted(p, size, 0)));
    // The count is still exact: a purge that had counted pages below the origin would have
    // taken more out of it than was ever put in. What is retained now is the rest of the last
    // purged page the slots touch, which their reuse made resident again.
    let page = Native::GEOMETRY.page_size();
    let revived = (5000 * size).next_multiple_of(page) - 5000 * size;
    assert_eq!(back.retained(), revived);
    // SAFETY: the test holds these slots and gives them up.
    unsafe { back.launder(class, &again[..100]) };
    assert_eq!(back.retained(), revived + 100 * size);
    nothing_below(os, base, origin, "after reuse");
    // A second round: the reuse cleared the clean bits of the pages the slots are on, and
    // not of pages an origin lower, so the pages written now are purged again and counted.
    for &p in &again {
        paint(p, size, 0x2e);
    }
    // SAFETY: the test holds these slots and gives them up.
    unsafe { back.launder(class, &again[100..]) };
    let purges = os.calls(Call::Purge);
    back.trim();
    assert!(os.calls(Call::Purge) > purges);
    assert_eq!(back.retained(), 0);
    // SAFETY: as above.
    unsafe {
        assert!(os.is_purged(NonNull::new(slots[0]).unwrap(), 5000 * size));
    }
    nothing_below(os, base, origin, "after the second trim");
}

/// The watermark's huge page with exactly one carved slot in it, which starts the page: the
/// purge brings the watermark up to the page's end and takes the page. (Whether a slot begins
/// in the page is asked of the slot's offset in the region, origin included.)
#[test]
#[cfg_attr(miri, ignore)]
fn a_huge_page_holding_only_its_first_slot_is_carved_up_and_purged() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let (class, origin) = class_with_an_origin(back.space().plan(), 64);
    let (page, huge) = (
        Native::GEOMETRY.page_size(),
        Native::GEOMETRY.huge_page_size(),
    );
    // The last slot carved is the first of the huge page five huge pages up, in the
    // huge-page part.
    let count = (5 * huge - origin) / 64 + 1;
    assert!(origin.is_multiple_of(page) && (5 * huge - origin).is_multiple_of(64));
    let slots = fill(&back, class, count);
    assert_eq!(slots.len(), count);
    let region = back.space().locate(slots[0]).unwrap().region;
    let base = back.space().region(region).addr().get();
    assert_eq!(slots[count - 1].addr(), base + 5 * huge);
    let host = back.host(class.class()).unwrap();
    assert!(
        Hints::unpack(host.hints(region).load())
            .huge_from
            .is_some_and(|from| from as usize * page <= 5 * huge)
    );
    for &p in &slots {
        paint(p, 64, 0x19);
    }
    for chunk in slots.chunks(64) {
        // SAFETY: the test holds these slots and gives them up.
        unsafe { back.launder(class, chunk) };
    }
    back.trim();
    assert_eq!(
        Grow::unpack(host.grow(region).load()).carved as usize,
        (6 * huge - origin) / 64
    );
    assert_eq!(back.retained(), 0);
    // SAFETY: no other thread runs, and the back end and this test store only integers.
    unsafe {
        assert!(os.is_purged(NonNull::new(slots[0]).unwrap(), 6 * huge - origin));
    }
}

/// The heap walk reports a class's free runs where its slots are, and its watermark from the
/// region's base, on a shape where the class has an origin (the walk's own test runs on
/// `Standard`, where it has none).
#[test]
#[cfg_attr(miri, ignore)]
fn the_walk_reports_free_runs_past_the_origin() {
    use crate::stats::{Region, Visitor};
    #[derive(Default)]
    struct Seen {
        regions: Vec<(usize, usize, usize)>,
        free: Vec<(usize, usize)>,
    }
    impl Visitor for Seen {
        fn region(&mut self, region: &Region) {
            self.regions
                .push((region.base.addr(), region.carved, region.committed));
        }
        fn free(&mut self, _: Class, start: *const u8, len: usize) {
            self.free.push((start.addr(), len));
        }
    }
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let (class, origin) = class_with_an_origin(back.space().plan(), 64);
    let slots = fill(&back, class, 200);
    // SAFETY: the test holds these slots and gives them up.
    unsafe { back.launder(class, &slots[10..150]) };
    let mut seen = Seen::default();
    back.inspect(&mut seen);
    let region = back.space().locate(slots[0]).unwrap().region;
    let base = back.space().region(region).addr().get();
    assert_eq!(seen.regions.len(), 1);
    let (seen_base, carved, committed) = seen.regions[0];
    assert_eq!((seen_base, carved), (base, origin + 200 * 64));
    assert!(committed >= carved);
    assert_eq!(seen.free, [(slots[10].addr(), 140 * 64)]);
    assert_eq!(slots[10].addr(), base + origin + 10 * 64);
}

/// The region a dense class takes after it has filled one has its huge-page part from the
/// region's base (`Back::prepare`), so the origin cuts the first huge page: commits, the
/// carve that brings the watermark up to the huge page's end, and the purge of that page all
/// stay above the origin there too.
#[test]
#[cfg_attr(miri, ignore)]
fn a_later_region_keeps_everything_above_its_origin() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let (class, origin) = class_with_an_origin(back.space().plan(), 8);
    let size = class.size();
    let host = back.host(class.class()).unwrap();
    let layout = *host.layout();
    let region_of = |p: *mut u8| back.space().locate(p).unwrap().region;
    // Fill the first region.
    let mut first: Vec<*mut u8> = Vec::new();
    while first.len() < layout.slots() as usize {
        let want = (layout.slots() as usize - first.len()).min(4096);
        let got = fill(&back, class, want);
        assert_eq!(got.len(), want);
        first.extend(got);
    }
    let one = region_of(first[0]);
    assert!(first.iter().all(|&p| region_of(p) == one));
    let last = *first.last().unwrap();
    let end = back.space().region(one).addr().get() + back.space().plan().region_size();
    assert!(last.addr() + size <= end && last.addr() + 2 * size > end);
    // A few slots of the second, well inside its first huge page.
    let second = fill(&back, class, 300);
    assert_eq!(second.len(), 300);
    let two: RegionId = region_of(second[0]);
    assert_ne!(two, one);
    let base = back.space().region(two);
    assert_eq!(
        Hints::unpack(host.hints(two).load()).huge_from,
        Some(0),
        "a later region of a dense class is all huge-page part"
    );
    assert!((layout.origin_page()) < layout.huge_pages());
    for (i, &p) in second.iter().enumerate() {
        assert_eq!(p.addr(), base.addr().get() + origin + i * size, "slot {i}");
        paint(p, size, 0x3c);
    }
    nothing_below(os, base, origin, "after carving the second region");
    let committed = Grow::unpack(host.grow(two).load()).committed as usize;
    assert!(committed * layout.page_size() > origin);
    assert!(os.is_writable(
        NonNull::new(second[0]).unwrap(),
        committed * layout.page_size() - origin
    ));
    // SAFETY: the test holds these slots and gives them up.
    unsafe { back.launder(class, &second) };
    assert_eq!(back.retained(), 300 * size);
    back.trim();
    nothing_below(os, base, origin, "after a trim of the second region");
    // The trim carved the rest of the cut huge page, freed it, and purged from the origin.
    let huge = Native::GEOMETRY.huge_page_size();
    assert_eq!(
        Grow::unpack(host.grow(two).load()).carved as usize,
        (huge - origin) / size
    );
    assert_eq!(back.retained(), 0);
    // SAFETY: no other thread runs, and the back end and this test store only integers.
    unsafe {
        assert!(os.is_purged(NonNull::new(second[0]).unwrap(), huge - origin));
    }
    let again = fill(&back, class, 300);
    assert_eq!(again, second);
    assert!(again.iter().all(|&p| painted(p, size, 0)));
    // As in the first region: the count holds the rest of the purged page the slots touch.
    let revived = (300 * size).next_multiple_of(layout.page_size()) - 300 * size;
    assert_eq!(back.retained(), revived);
    // SAFETY: the test holds these slots and gives them up.
    unsafe { back.launder(class, &again[..10]) };
    assert_eq!(back.retained(), revived + 10 * size);
}

/// A pointer below a class's origin is no block: the lookups the C entry points make find
/// nothing there, as they find nothing inside a slot.
#[test]
#[cfg_attr(miri, ignore)]
fn lookups_below_the_origin_find_nothing() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let (class, origin) = class_with_an_origin(back.space().plan(), 64);
    let slots = fill(&back, class, 4);
    let region = back.space().locate(slots[0]).unwrap().region;
    let base = back.space().region(region).as_ptr();
    assert_eq!(slots[0], base.wrapping_add(origin));
    for below in [0, 64, origin - 4096, origin - 64, origin - 8] {
        let p = base.wrapping_add(below);
        assert_eq!(back.lookup(p), None, "{below}");
        assert_eq!(back.size(p), None, "{below}");
        assert_eq!(back.room(p), None, "{below}");
        assert_eq!(back.usable_size(p), None, "{below}");
    }
}

/// The region's base was slot 0 before origins, and is a multiple of the slot size from the
/// origin for a class whose size divides its origin: a free of it must not pass for a slot.
#[test]
#[cfg_attr(miri, ignore)]
#[should_panic(expected = "not the start of a slot")]
fn freeing_the_region_base_below_the_origin_aborts() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let (class, _) = class_with_an_origin(back.space().plan(), 64);
    let slots = fill(&back, class, 4096);
    let region = back.space().locate(slots[0]).unwrap().region;
    let base = back.space().region(region).as_ptr();
    // SAFETY: the test passes a bad pointer to see the abort.
    unsafe { back.launder(class, &[base]) };
}

/// The same in a batch, where the fast check (`class::Fast`) sees the pointer first: the
/// offset from the first slot is negative and a multiple of the slot size.
#[test]
#[cfg_attr(miri, ignore)]
#[should_panic(expected = "not the start of a slot")]
fn freeing_a_slot_size_below_the_origin_in_a_batch_aborts() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let (class, _) = class_with_an_origin(back.space().plan(), 64);
    let slots = fill(&back, class, 4096);
    let batch = [slots[0], slots[1], slots[0].wrapping_sub(64), slots[2]];
    // SAFETY: the test passes a bad pointer to see the abort.
    unsafe { back.launder(class, &batch) };
}

/// And with the region's base in a batch whose slots lie far enough up that the base's old
/// slot index, `origin / size` slots below the first, is under the watermark.
#[test]
#[cfg_attr(miri, ignore)]
#[should_panic(expected = "not the start of a slot")]
fn freeing_the_region_base_in_a_batch_aborts() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let (class, origin) = class_with_an_origin(back.space().plan(), 64);
    let slots = fill(&back, class, 4096);
    assert!(origin / 64 < slots.len());
    let region = back.space().locate(slots[0]).unwrap().region;
    let base = back.space().region(region).as_ptr();
    let batch = [slots[3000], slots[3001], base, slots[3002]];
    // SAFETY: the test passes a bad pointer to see the abort.
    unsafe { back.launder(class, &batch) };
}

/// On the real OS, in the design's geometry, where origins are up to a megabyte.
#[cfg(not(miri))]
mod design {
    use super::*;
    use crate::os::linux::LinuxOs;
    use std::fs::File;
    use std::os::unix::fs::FileExt;

    /// The pages of the `len` bytes at `start` that have a page-table entry: touched, or read
    /// and mapped to the shared zero page.
    fn present(start: *const u8, len: usize) -> Vec<usize> {
        const PRESENT: u64 = 1 << 63;
        let page = Geometry::DESIGN.page_size();
        let pagemap = File::open("/proc/self/pagemap").unwrap();
        let mut entries = std::vec![0u8; len / page * 8];
        pagemap
            .read_exact_at(&mut entries, (start.addr() / page * 8) as u64)
            .unwrap();
        entries
            .as_chunks::<8>()
            .0
            .iter()
            .enumerate()
            .filter(|&(_, &entry)| u64::from_ne_bytes(entry) & PRESENT != 0)
            .map(|(i, _)| i)
            .collect()
    }

    /// The permissions of the mapping that holds `at`, as `/proc/self/maps` prints them.
    fn permissions(at: usize) -> std::string::String {
        let maps = std::fs::read_to_string("/proc/self/maps").unwrap();
        for line in maps.lines() {
            let mut fields = line.split_whitespace();
            let (Some(range), Some(perms)) = (fields.next(), fields.next()) else {
                continue;
            };
            let Some((start, end)) = range.split_once('-') else {
                continue;
            };
            let start = usize::from_str_radix(start, 16).unwrap();
            let end = usize::from_str_radix(end, 16).unwrap();
            if start <= at && at < end {
                return perms.into();
            }
        }
        panic!("no mapping holds {at:#x}");
    }

    /// Asserts that the pages of `region` below `origin` are mapped without access and none
    /// has ever been touched.
    fn untouched_below(back: &Back<'_, LinuxOs>, region: RegionId, origin: usize, when: &str) {
        let base = back.space().region(region).as_ptr();
        let page = Geometry::DESIGN.page_size();
        assert_eq!(present(base, origin), [0usize; 0], "{when}");
        for at in [0, (origin / 2) & !(page - 1), origin - page] {
            assert_eq!(
                permissions(base.addr() + at),
                "---p",
                "{when}: {at} bytes in"
            );
        }
        assert_eq!(permissions(base.addr() + origin), "rw-p", "{when}");
    }

    /// Every small class in its first region: its slots start at its origin, a different
    /// page for each class, aligned as the class promises, and the pages below are without
    /// access and untouched through carving, use, a launder, a trim and reuse.
    #[test]
    fn nothing_below_an_origin_is_ever_accessible_or_touched() {
        let state = Box::new(State::new(LinuxOs));
        let back = state.get_or_reserve().unwrap();
        let plan = back.space().plan();
        let page = Geometry::DESIGN.page_size();
        let mut starts = BTreeSet::new();
        for class in SmallClass::all() {
            let size = class.size();
            let origin = plan.slot_origin(class.class());
            let slots = fill(&back, class, 96);
            assert_eq!(slots.len(), 96);
            let region = back.space().locate(slots[0]).unwrap().region;
            let base = back.space().region(region).as_ptr();
            for (i, &p) in slots.iter().enumerate() {
                assert_eq!(
                    p.addr(),
                    base.addr() + origin + i * size,
                    "{size}: slot {i}"
                );
                assert_eq!(p.addr() % class.class().align(), 0, "{size}");
                paint(p, size, 0x4d);
            }
            if plan.region_size() == Geometry::DESIGN.largest_plan().region_size() {
                assert!(
                    starts.insert(slots[0].addr() / page % 256),
                    "{size} starts on a page another class starts on"
                );
            }
            if origin == 0 {
                continue;
            }
            assert_eq!(back.lookup(base), None, "{size}");
            assert_eq!(
                back.lookup(base.wrapping_add(origin - size)),
                None,
                "{size}"
            );
            untouched_below(&back, region, origin, &std::format!("{size}: carved"));
            // SAFETY: the test holds these slots and gives them up.
            unsafe { back.launder(class, &slots) };
            back.trim();
            untouched_below(&back, region, origin, &std::format!("{size}: trimmed"));
            assert_eq!(fill(&back, class, 96), slots, "{size}");
            untouched_below(&back, region, origin, &std::format!("{size}: reused"));
        }
    }

    /// A sink that keeps nothing, so that a region can be carved to its end without its
    /// memory or a pointer to each slot.
    struct Discard(usize);

    impl crate::back::Sink for Discard {
        fn positions(&self) -> usize {
            self.0
        }
        fn run(&mut self, _: usize, _: *mut u8, _: usize, _: usize) {}
        fn put(&mut self, _: usize, _: *mut u8) {}
    }

    /// The second region of a dense class, whose huge-page part starts at its base and whose
    /// first huge page the origin cuts: slots start at the origin, the purge that takes the
    /// cut page carves its rest and returns it from the origin up, and nothing below the
    /// origin is touched or made accessible.
    #[test]
    fn a_later_region_starts_at_its_origin_and_purges_from_it() {
        let state = Box::new(State::new(LinuxOs));
        let back = state.get_or_reserve().unwrap();
        let plan = back.space().plan();
        let class = small(8);
        let origin = plan.slot_origin(class.class());
        let huge = Geometry::DESIGN.huge_page_size();
        if plan.region_size() != Geometry::DESIGN.largest_plan().region_size() {
            // Under an address-space limit the regions are smaller; nothing to check here.
            return;
        }
        assert!(origin > 0 && origin < huge);
        let host = back.host(class.class()).unwrap();
        let slots = host.layout().slots() as usize;
        // The whole first region, carved and never touched. Where the OS refuses to make
        // 4 GiB writable (strict overcommit), there is nothing to check.
        if back.fill(class, &mut Discard(slots)) != slots {
            return;
        }
        let second = fill(&back, class, 50_000);
        assert_eq!(second.len(), 50_000);
        let first = back.space().locate(second[0]).unwrap().region;
        let base = back.space().region(first).as_ptr();
        assert_eq!(
            Hints::unpack(host.hints(first).load()).huge_from,
            Some(0),
            "a later region of a dense class is all huge-page part"
        );
        assert!(origin + 50_000 * 8 < huge);
        for (i, &p) in second.iter().enumerate() {
            assert_eq!(p.addr(), base.addr() + origin + i * 8, "slot {i}");
            paint(p, 8, 0x4d);
        }
        untouched_below(&back, first, origin, "carved");
        assert!(!present(base.wrapping_add(origin), huge - origin).is_empty());
        // SAFETY: the test holds these slots and gives them up.
        unsafe { back.launder(class, &second) };
        back.trim();
        assert_eq!(back.retained(), 0);
        assert_eq!(
            present(base, huge),
            [0usize; 0],
            "the cut huge page went back"
        );
        untouched_below(&back, first, origin, "trimmed");
        assert_eq!(
            Grow::unpack(host.grow(first).load()).carved as usize,
            (huge - origin) / 8
        );
        let again = fill(&back, class, 50_000);
        assert_eq!(again, second);
        assert!(again.iter().all(|&p| painted(p, 8, 0)));
    }
}
