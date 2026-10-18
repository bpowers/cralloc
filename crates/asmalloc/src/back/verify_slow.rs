//! Kani proofs too slow for the merge gate: the layout for every geometry, purge-unit and
//! leaf-commit arithmetic (they divide by symbolic class sizes), and `lowest`. Run with
//! `scripts/kani-all back::verify_slow`.
//!
//! Which slots a unit touches, and which lie wholly inside it, is proved for every class and
//! position on the tiny heap's regions. For the larger geometries CBMC does not finish the
//! 64-bit multiply in `Divisor::divide` within the hour; there it rests on `touching` being
//! `(start / size, (end - 1) / size)` by definition, `Divisor::divide` being proved exact for
//! every input (`classes::verify`, `classes::verify_slow`), and the unit test
//! `layout::tests::purge_passes_tile_the_carved_pages`.
//!
//! Runs of operations on a heap are not here. A harness running the real back end on a tiny
//! simulated heap (fill, launder, fill, with two slots) exceeds the 6 GiB cap within two
//! minutes: the protocols' retry loops nest (a search restarts around a descent, a clear and a
//! walk up), and Kani unrolls each to the bound. Those runs are checked by the interleaving
//! explorer (`back::models`) and the shadow-model tests (`back::tests`).

use super::class::{Fast, odd_inverse};
use super::layout::{Layout, Memo, Step, WORD_BYTES, slots_in_region};
use super::verify::{any_class, any_plan_of, any_used_plan, layouts_fit};
use super::word::{BITS, lowest};
use crate::classes::MagazineSize;
use crate::space::{Geometry, Plan};

/// Any plan of any geometry `Geometry::try_new` accepts.
fn any_plan() -> Plan {
    let ordinal: usize = kani::any();
    kani::assume(ordinal < MagazineSize::COUNT);
    let geometry = Geometry::try_new(
        kani::any(),
        kani::any(),
        kani::any(),
        kani::any(),
        kani::any(),
        kani::any(),
        MagazineSize::from_ordinal(ordinal).unwrap(),
    );
    kani::assume(geometry.is_some());
    any_plan_of(geometry.unwrap())
}

/// Seconds rather than a fraction of one, so not in the merge gate.
#[kani::proof]
#[kani::unwind(6)]
fn layouts_fit_the_used_geometries() {
    layouts_fit(any_used_plan());
}

#[kani::proof]
#[kani::unwind(6)]
fn layouts_fit_every_geometry() {
    layouts_fit(any_plan());
}

/// The layout `Back::host` reads back from a class's memo is the one that was worked out, and
/// so the one the harnesses above are about: packing it into the memo's words loses nothing.
/// The bound is for the comparison, which goes over the bytes of the layout's arrays.
#[kani::proof]
#[kani::unwind(21)]
fn layouts_come_back_from_the_memo() {
    let plan = any_used_plan();
    let class = any_class();
    let memo = Memo::new();
    let Some(layout) = memo.layout(plan, class) else {
        return;
    };
    assert!(memo.layout(plan, class) == Some(layout));
}

/// The slot count without a divide is the quotient.
#[kani::proof]
#[kani::solver(kissat)]
fn slot_counts_are_the_quotient() {
    let plan = any_plan();
    let class = any_class();
    assert!(slots_in_region(plan, class) == plan.slots_per_region(class));
}

/// A carve that commits up to page `committed` also commits the leaf words of every slot that
/// starts below it, as whole pages within the leaf bitmap's part of the block.
#[kani::proof]
#[kani::unwind(6)]
fn leaf_commits_cover_the_committed_slots() {
    let plan = any_used_plan();
    let class = any_class();
    let Some(layout) = Layout::new(plan, class) else {
        return;
    };
    let committed: u32 = kani::any();
    kani::assume(committed <= layout.region_pages());
    let bytes = layout.leaf_commit(committed);
    let page = plan.geometry().page_size();
    assert!(bytes % page == 0);
    assert!(bytes <= plan.metadata_size());
    let slot: u32 = kani::any();
    kani::assume(slot < layout.slots());
    if layout.slot_bytes(slot) < (committed as usize) << layout.page_shift() {
        assert!((layout.tree_word(0, slot / BITS) + 1) as usize * WORD_BYTES <= bytes);
    }
}

/// A purge step moves down: a unit lies wholly below the watermark and below the cursor, ends
/// on a unit boundary, starts below where it ends, and stays in one part of the region (whole
/// huge pages in the huge-page part); a skip moves the cursor down.
///
/// For classes whose size is a power of two, where `Divisor::divide` is a shift: with an odd
/// factor of 3, 5 or 7, CBMC does not finish its 64-bit multiply within 20 minutes. The unit
/// test `layout::tests::purge_passes_tile_the_carved_pages` checks every class.
#[kani::proof]
#[kani::unwind(6)]
fn purge_steps_move_down_within_one_part() {
    let plan = any_used_plan();
    let class = any_class();
    kani::assume(class.size().is_power_of_two());
    let Some(layout) = Layout::new(plan, class) else {
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

/// The unit a large free purges lies inside its slot, so below the watermark whenever the slot
/// is, and touches no other slot: for every class and slot of the tiny heap's regions.
#[kani::proof]
#[kani::unwind(6)]
fn a_slot_unit_touches_only_its_slot_on_the_tiny_heap() {
    use crate::os::sim::{Shape, Tiny};
    let plan = Tiny::GEOMETRY.largest_plan();
    let class = any_class();
    let Some(layout) = Layout::new(plan, class) else {
        return;
    };
    let slot: u32 = kani::any();
    kani::assume(slot < layout.slots());
    let Some(unit) = layout.slot_unit(slot) else {
        return;
    };
    let page = layout.page_size();
    let (start, end) = (unit.start as usize * page, unit.end as usize * page);
    assert!(start < end);
    assert!(layout.slot_bytes(slot) <= start && end <= layout.slot_bytes(slot + 1));
    assert!(unit.end <= layout.carved_pages(slot + 1));
    assert!(layout.touching(unit) == (slot, slot));
    // A slot of whole pages is covered entirely.
    if layout.size() % page == 0 {
        assert!(start == layout.slot_bytes(slot) && end == layout.slot_bytes(slot + 1));
    }
}

/// The slots a unit touches are exactly those overlapping its pages, and those `inside` reports
/// are exactly those wholly within them, for every class, watermark, cursor and huge-page part
/// of the tiny heap's regions.
#[kani::proof]
#[kani::unwind(6)]
fn units_touch_exactly_their_slots_on_the_tiny_heap() {
    use crate::os::sim::{Shape, Tiny};
    let plan = Tiny::GEOMETRY.largest_plan();
    let class = any_class();
    let Some(layout) = Layout::new(plan, class) else {
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
    let Step::Unit(unit) = layout.unit_below(carved, huge_from, cursor) else {
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

/// A launder's fast check (`class::Fast`), for every class and every pointer: it passes a
/// pointer only if the pointer is the start of a slot below the limit, and gives that slot;
/// and it passes every such start. The limit is any watermark a region of at most 4 GiB can
/// have. A 64-bit multiply each way, so not in the gate.
#[kani::proof]
#[kani::solver(kissat)]
fn the_fast_check_passes_exactly_the_slot_starts_below_the_limit() {
    let class = any_class();
    let size = class.size() as u64;
    let shift = size.trailing_zeros();
    let limit: u64 = kani::any();
    kani::assume(limit <= (1u64 << 32) / size);
    let base: usize = kani::any();
    let fast = Fast {
        base,
        inverse: odd_inverse(size >> shift),
        shift,
        limit,
    };
    let offset: u64 = kani::any();
    let object = core::ptr::without_provenance::<u8>(base.wrapping_add(offset as usize));
    let k = fast.slot(object);
    if k < limit {
        assert!(k.wrapping_mul(size) == offset);
    }
    let slot: u64 = kani::any();
    kani::assume(slot < limit);
    let start = core::ptr::without_provenance::<u8>(base.wrapping_add((slot * size) as usize));
    assert!(fast.slot(start) == slot);
}
