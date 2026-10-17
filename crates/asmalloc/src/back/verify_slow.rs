//! Kani proofs too slow for the merge gate: the layout for every geometry, the slot arithmetic of
//! purge units (it divides by symbolic class sizes), and runs of operations on a small heap. Run
//! with `scripts/kani-all back::verify_slow`.

use super::layout::{Layout, Step, WORD_BYTES, slots_in_region};
use super::region::{self, Purge};
use super::testhost::TestHost;
use super::tree::{self, Host};
use super::verify::{any_class, any_used_geometry, layouts_fit};
use super::word::{Atomic, BITS, Word, lowest};
use crate::classes::Class;
use crate::os::sim::{Shape, Tiny};
use crate::space::Geometry;

fn any_geometry() -> Geometry {
    let geometry = Geometry::try_new(
        kani::any(),
        kani::any(),
        kani::any(),
        kani::any(),
        kani::any(),
    );
    kani::assume(geometry.is_some());
    geometry.unwrap()
}

#[kani::proof]
#[kani::unwind(6)]
fn layouts_fit_every_geometry() {
    layouts_fit(any_geometry());
}

/// The slot count without a divide is the quotient.
#[kani::proof]
#[kani::solver(kissat)]
fn slot_counts_are_the_quotient() {
    let geometry = any_geometry();
    let class = any_class();
    assert!(slots_in_region(geometry, class) == geometry.slots_per_region(class));
}

/// The slots a unit touches are exactly those overlapping its pages, and those `inside` reports
/// are exactly those wholly within them.
#[kani::proof]
#[kani::unwind(6)]
#[kani::solver(kissat)]
fn units_touch_exactly_their_slots() {
    let geometry = any_used_geometry();
    let class = any_class();
    let Some(layout) = Layout::new(geometry, class) else {
        return;
    };
    let carved: u32 = kani::any();
    kani::assume(carved <= layout.slots());
    let cursor: u32 = kani::any();
    let Step::Unit(unit) = layout.unit_below(carved, None, cursor) else {
        return;
    };
    let (first, last) = layout.touching(unit);
    let page = layout.page_size();
    let (start, end) = (unit.start as usize * page, unit.end as usize * page);
    assert!(last < carved);
    assert!(layout.slot_bytes(first) <= start && layout.slot_bytes(first + 1) > start);
    assert!(layout.slot_bytes(last) < end && layout.slot_bytes(last + 1) >= end);
    let (lo, hi) = layout.inside(unit);
    let slot: u32 = kani::any();
    kani::assume(first <= slot && slot <= last);
    let wholly = layout.slot_bytes(slot) >= start && layout.slot_bytes(slot + 1) <= end;
    assert!(wholly == (lo <= slot && slot < hi));
}

/// Three operations, each any of claim, release and purge with any arguments, on one region of
/// eight 256-byte slots, all carved and held at the start. After each: every slot is either free
/// or held, never both; every word with free slots is visible from the top; and the zero bits
/// (none: a small class) and the purge's claims leave the bitmap as it was.
#[kani::proof]
#[kani::unwind(70)]
fn short_runs_keep_the_bitmap_and_its_summary_consistent() {
    let class = Class::of_size(256).unwrap();
    let host = TestHost::<Atomic>::new(Tiny::GEOMETRY, class, 1);
    let region = host.region_at(0);
    host.carve_all(region, 8);
    let all = 0xffu64;
    let mut held = all;
    let leaf = tree::Node::leaf(region, 0);
    for _ in 0..3 {
        match kani::any::<u8>() % 3 {
            0 => {
                let n: u32 = kani::any();
                kani::assume((1..=8).contains(&n));
                if let Some(taken) = tree::claim(&host, n) {
                    assert!(taken.region == region && taken.word == 0);
                    assert!(taken.bits & held == 0);
                    assert!(taken.bits.count_ones() <= n);
                    held |= taken.bits;
                } else {
                    assert!(held == all);
                }
            }
            1 => {
                let bits: u64 = kani::any();
                kani::assume(bits != 0 && bits & !held == 0);
                tree::release(&host, region, 0, bits);
                held &= !bits;
            }
            _ => {
                let cursor: u32 = kani::any();
                if let Step::Unit(unit) = host.layout().unit_below(8, None, cursor) {
                    let (outcome, _) = region::purge_unit(&host, region, unit);
                    let (first, last) = host.layout().touching(unit);
                    let touching = super::word::bits_in_word(0, first, last);
                    if matches!(outcome, Purge::Done(_)) {
                        assert!(touching & held == 0);
                    }
                }
            }
        }
        let free = tree::word(&host, leaf).load();
        assert!(free & held == 0 && free | held == all);
        assert!(free == 0 || host.path_set(leaf));
    }
}

/// A carve that commits up to page `committed` also commits the leaf words of every slot that
/// starts below it, as whole pages within the leaf bitmap's part of the block.
#[kani::proof]
#[kani::unwind(6)]
fn leaf_commits_cover_the_committed_slots() {
    let geometry = any_used_geometry();
    let class = any_class();
    let Some(layout) = Layout::new(geometry, class) else {
        return;
    };
    let committed: u32 = kani::any();
    kani::assume(committed <= layout.region_pages());
    let bytes = layout.leaf_commit(committed);
    let page = geometry.page_size();
    assert!(bytes % page == 0);
    assert!(bytes <= geometry.metadata_size());
    let slot: u32 = kani::any();
    kani::assume(slot < layout.slots());
    if layout.slot_bytes(slot) < (committed as usize) << layout.page_shift() {
        assert!((slot / BITS + 1) as usize * WORD_BYTES <= bytes);
    }
}

/// A purge step moves down: a unit lies wholly below the watermark and below the cursor, ends
/// on a unit boundary, starts below where it ends, and stays in one part of the region (whole
/// huge pages in the huge-page part); a skip moves the cursor down.
#[kani::proof]
#[kani::unwind(6)]
fn purge_steps_move_down_within_one_part() {
    let geometry = any_used_geometry();
    let class = any_class();
    let Some(layout) = Layout::new(geometry, class) else {
        return;
    };
    let carved: u32 = kani::any();
    kani::assume(carved <= layout.slots());
    let huge: u32 = kani::any();
    let huge_from = if kani::any() {
        kani::assume(huge < layout.region_pages() && huge % layout.huge_pages() == 0);
        Some(huge)
    } else {
        None
    };
    let cursor: u32 = kani::any();
    let top = layout.carved_pages(carved);
    let bound = if cursor == 0 || cursor > top {
        top
    } else {
        cursor
    };
    match layout.unit_below(carved, huge_from, cursor) {
        Step::Done => assert!(bound == 0),
        Step::Skip(next) => {
            assert!(next < bound);
            assert!(huge_from == Some(next));
        }
        Step::Unit(unit) => {
            assert!(unit.start < unit.end && unit.end <= bound);
            match huge_from {
                Some(h) if unit.end > h => {
                    assert!(unit.start >= h);
                    assert!(unit.end % layout.huge_pages() == 0);
                    assert!(unit.start % layout.huge_pages() == 0);
                }
                _ => {}
            }
        }
    }
}

/// `lowest` takes `min(n, popcount)` bits of the word, and they are its lowest set bits.
#[kani::proof]
#[kani::unwind(8)]
fn lowest_takes_the_lowest_set_bits() {
    let word: u64 = kani::any();
    let n: u32 = kani::any();
    kani::assume(n <= 64);
    let taken = lowest(word, n);
    assert!(taken & !word == 0);
    let want = if n < word.count_ones() {
        n
    } else {
        word.count_ones()
    };
    assert!(taken.count_ones() == want);
    if taken != 0 {
        let top = 63 - taken.leading_zeros();
        let below = u64::MAX >> (63 - top);
        assert!(word & below == taken);
    }
}
