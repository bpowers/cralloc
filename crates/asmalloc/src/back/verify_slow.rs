//! Kani proofs too slow for the merge gate: the layout for every geometry, purge-unit and
//! leaf-commit arithmetic (they divide by symbolic class sizes), and `lowest`. Run with
//! `scripts/kani-all back::verify_slow`.
//!
//! Runs of operations on a small heap are not here: a harness of two symbolic operations on one
//! region of eight slots ran past twelve minutes under the 6 GiB cap, because Kani cannot bound
//! the summary walks' loops. Those runs are checked instead by the interleaving explorer
//! (`back::models`) and the shadow-model tests (`back::tests`).
//!
//! Which slots a unit touches, and which lie wholly inside it, is not proved here: CBMC does not
//! finish the 64-bit multiply in `Divisor::divide` for a symbolic class within the hour.
//! `touching` is `(start / size, (end - 1) / size)` by definition, with `Divisor::divide` proved
//! exact for every input in `classes::verify` and `classes::verify_slow`; the unit test
//! `layout::tests::purge_passes_tile_the_carved_pages` checks both against the definitions for
//! every class, many watermarks and huge-page boundaries, in three geometries.

use super::layout::{Layout, Step, WORD_BYTES, slots_in_region};
use super::verify::{any_class, any_used_geometry, layouts_fit};
use super::word::{BITS, lowest};
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
///
/// For classes whose size is a power of two, where `Divisor::divide` is a shift: with an odd
/// factor of 3, 5 or 7, CBMC does not finish its 64-bit multiply within 20 minutes. The unit
/// test `layout::tests::purge_passes_tile_the_carved_pages` checks every class.
#[kani::proof]
#[kani::unwind(6)]
fn purge_steps_move_down_within_one_part() {
    let geometry = any_used_geometry();
    let class = any_class();
    kani::assume(class.size().is_power_of_two());
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
