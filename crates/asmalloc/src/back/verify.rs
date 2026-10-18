//! Kani proofs of the back end's word operations and index arithmetic, for every input, in the
//! geometries in use. `verify_slow.rs` has the properties that need division by a symbolic class
//! size, every geometry, or a run of operations on a small heap.

use super::layout::{Grow, Hints, Layout, MAX_LEVELS, WORD_BYTES, slots_in_region};
use super::word::{BITS, bits_in_word, position, span};
use crate::classes::{COUNT, Class};
use crate::os::sim::{Native, Shape, Standard, Tiny};
use crate::space::{Geometry, Plan};

pub(super) fn any_class() -> Class {
    let index: u8 = kani::any();
    kani::assume(index >= 1 && index as usize <= COUNT);
    Class::from_index(index).unwrap()
}

/// One of the geometries in use, at any region size it allows.
pub(super) fn any_used_plan() -> Plan {
    let geometry = match kani::any::<u8>() {
        0 => Geometry::DESIGN,
        1 => Standard::GEOMETRY,
        2 => Native::GEOMETRY,
        _ => Tiny::GEOMETRY,
    };
    any_plan_of(geometry)
}

/// A plan of `geometry` at any region size it allows.
pub(super) fn any_plan_of(geometry: Geometry) -> Plan {
    let shift: u32 = kani::any();
    let plan = geometry.plan(shift);
    kani::assume(plan.is_some());
    plan.unwrap()
}

/// `span` and `bits_in_word` name exactly the bits of a word that lie in a range of items, and
/// `position` inverts `64 * word + bit`.
#[kani::proof]
fn word_bits_cover_exactly_their_range() {
    let first: u32 = kani::any();
    let last: u32 = kani::any();
    let word: u32 = kani::any();
    let bit: u32 = kani::any();
    kani::assume(first <= last && last < 1 << 29 && word < 1 << 23 && bit < BITS);
    let item = word * BITS + bit;
    let in_range = first <= item && item <= last;
    assert!((bits_in_word(word, first, last) >> bit) & 1 == in_range as u64);
    assert!(position(item) == (word, bit));
    let (lo, hi): (u32, u32) = (kani::any(), kani::any());
    kani::assume(lo <= hi && hi < 64);
    assert!((span(lo, hi) >> bit) & 1 == (lo <= bit && bit <= hi) as u64);
}

/// The arithmetic of a run of claims (`tree::claim_next`). Moving past a group lands on the
/// first word of the next group of 64, always forward and never more than a group. A claim of
/// `bits` from a word read as `seen` leaves exactly the other bits, and empties the word exactly
/// when it took all of them; and a compare-and-swap that fails against a word some free has
/// only added bits to is not counted as meeting another claimer.
#[kani::proof]
fn a_run_moves_one_group_on_and_claims_exactly() {
    use super::tree::past_group;
    let index: u32 = kani::any();
    kani::assume(index < u32::MAX - BITS);
    let next = past_group(index);
    assert!(next > index && next - index <= BITS && next % BITS == 0);
    assert!(next / BITS == index / BITS + 1);
    let (seen, bits): (u64, u64) = (kani::any(), kani::any());
    kani::assume(bits != 0 && bits & !seen == 0);
    let left = seen & !bits;
    assert!(left | bits == seen && left & bits == 0);
    assert!((left == 0) == (bits == seen));
    let freed: u64 = kani::any();
    let now = seen | freed;
    assert!(seen & !now == 0);
}

/// The header words round-trip, and all zero means nothing carved, nothing committed, no
/// huge-page part and a purge cursor at the top.
#[kani::proof]
fn headers_round_trip() {
    let grow = Grow {
        carved: kani::any(),
        committed: kani::any(),
    };
    assert!(Grow::unpack(grow.pack()) == grow);
    let huge: u32 = kani::any();
    kani::assume(huge < u32::MAX);
    let hints = Hints {
        huge_from: if kani::any() { Some(huge) } else { None },
        cursor: kani::any(),
    };
    assert!(Hints::unpack(hints.pack()) == hints);
    assert!(Grow::unpack(0).carved == 0 && Grow::unpack(0).committed == 0);
    assert!(Hints::unpack(0).huge_from.is_none() && Hints::unpack(0).cursor == 0);
}

/// Every class that fits a region of a plan has a layout, and the layout fits in the
/// region's metadata block: the leaf bitmap at offset zero covers every slot, each summary level
/// has a bit for every word below and ends in one top word, the sections do not overlap, the
/// clean bitmap covers every page, large classes have a zero bitmap and a size word for every
/// slot, and the
/// part committed at assignment covers everything after the leaf bitmap.
pub(super) fn layouts_fit(plan: Plan) {
    let class = any_class();
    let slots = slots_in_region(plan, class);
    let layout = Layout::new(plan, class);
    if slots == 0 {
        assert!(layout.is_none());
        return;
    }
    let layout = layout.unwrap();
    assert!(layout.slots() == slots);
    assert!(layout.total_words() as usize * WORD_BYTES <= plan.metadata_size());
    assert!(layout.leaf_words() * BITS >= slots && (layout.leaf_words() - 1) * BITS < slots);
    assert!(layout.tree_word(0, 0) == 0);
    // The header is the first whole cache line after the leaf bitmap, alone on it, or, where
    // the block has no room for that, straight after the leaf bitmap.
    let line = (crate::lined::LINE / WORD_BYTES) as u32;
    assert!(layout.hints_word() == layout.grow_word() + 1);
    let after = if layout.height() >= 1 {
        layout.tree_word(1, 0)
    } else {
        layout.clean_word(0)
    };
    let mut end = if after == layout.hints_word() + 1 {
        assert!(layout.grow_word() == layout.leaf_words());
        after
    } else {
        assert!(layout.grow_word() % line == 0);
        assert!(layout.grow_word() >= layout.leaf_words());
        assert!(layout.grow_word() < layout.leaf_words() + line);
        assert!(after == layout.grow_word() + line);
        after
    };
    let height = layout.height() as usize;
    assert!(height < MAX_LEVELS);
    assert!(layout.level_words(layout.height()) == 1);
    let mut level = 1;
    while level <= height {
        let below = layout.level_words(level as u8 - 1);
        let words = layout.level_words(level as u8);
        assert!(words * BITS >= below && (words - 1) * BITS < below);
        assert!(layout.tree_word(level as u8, 0) == end);
        end += words;
        level += 1;
    }
    assert!(layout.clean_word(0) == end);
    assert!(layout.clean_words() * BITS >= layout.region_pages());
    end += layout.clean_words();
    match layout.zero_word(0) {
        Some(at) => {
            assert!(!class.is_small() && at == end);
            end += layout.leaf_words();
            assert!(layout.size_word(0) == Some(end));
            end += slots;
        }
        None => assert!(class.is_small() && layout.size_word(0).is_none()),
    }
    assert!(end == layout.total_words());
    let (start, stop) = layout.fixed_commit();
    let page = plan.geometry().page_size();
    assert!(start % page == 0 && stop % page == 0);
    assert!(start <= layout.grow_word() as usize * WORD_BYTES);
    assert!(stop >= end as usize * WORD_BYTES && stop <= plan.metadata_size());
    assert!(layout.region_pages() as usize * page == plan.region_size());
}

/// The decay word: a pass under way is stepped whatever the clock says. Otherwise a pass
/// starts exactly when the clock is an interval or more past the word's time; a clock behind
/// the word re-stamps it with the time now, after which a pass starts one interval later; and
/// the word that starts a pass is marked active and keeps the time.
#[kani::proof]
fn a_decay_pass_starts_only_after_an_interval() {
    use super::policy::DECAY_TICKS;
    use super::purge::{Decay, decay_look};
    let word: u64 = kani::any();
    let now: u64 = kani::any();
    let since = (now & !1).wrapping_sub(word);
    match decay_look(word, now) {
        Decay::Step => assert!(word & 1 == 1),
        Decay::Start(next) => {
            assert!(word & 1 == 0 && next & 1 == 1);
            assert!(next | 1 == now | 1);
            assert!(decay_look(next, kani::any()) == Decay::Step);
            assert!(since >= DECAY_TICKS && since < 1 << 63);
        }
        Decay::Restamp(next) => {
            assert!(word & 1 == 0 && since >= 1 << 63);
            assert!(next == now & !1);
            // From the new stamp the wait is one interval, wherever the clock goes forward.
            assert!(decay_look(next, now) == Decay::Wait);
            assert!(
                decay_look(next, now.wrapping_add(DECAY_TICKS))
                    == Decay::Start(now.wrapping_add(DECAY_TICKS) | 1)
            );
        }
        Decay::Wait => assert!(word & 1 == 0 && since < DECAY_TICKS),
    }
    // A pass that has just ended at `now` does not start again at once.
    assert!(decay_look(now & !1, now) == Decay::Wait);
}

/// The cap's decision at growth: a grower purges nothing for less than a step of debt or with
/// the count at or under the cap; otherwise it purges no more than it grew and no more than
/// is over the cap, and exactly the smaller of the two, so that growth over the cap pays for
/// itself and never takes the count under the cap.
///
/// This is the decision alone. `Back::grow` adds to the debt and takes it in separate steps,
/// so a racing grower may take less than a step and purge nothing: one purge later.
#[kani::proof]
fn growth_purges_the_smaller_of_its_debt_and_the_excess() {
    use super::policy::growth_goal;
    let (debt, retained, cap, step): (usize, usize, usize, usize) =
        (kani::any(), kani::any(), kani::any(), kani::any());
    let goal = growth_goal(debt, retained, cap, step);
    assert!(goal <= debt);
    assert!(goal <= retained.saturating_sub(cap));
    if debt < step || retained <= cap {
        assert!(goal == 0);
    } else {
        assert!(goal == debt || goal == retained - cap);
        assert!(goal == debt.min(retained - cap));
    }
}

/// A pass word keeps its three parts apart, and a stepper can tell its pass from another: the
/// tag of a pass's decay word is the only one it acts on.
#[kani::proof]
fn a_pass_word_round_trips() {
    use super::purge::Pass;
    let pass = Pass {
        tag: kani::any(),
        lap: kani::any(),
        pages: kani::any(),
    };
    assert!(Pass::unpack(pass.pack()) == pass);
    let word: u64 = kani::any();
    assert!(Pass::unpack(word).pack() == word);
    // The all-zero word, which is what the state starts as, is a pass with nothing to purge.
    assert!(Pass::unpack(0).pages == 0);
}

/// A large slot is whole pages in every geometry in use, so the unit a large free returns
/// (`Layout::slot_unit`) is exactly the slot's bytes: it starts and ends on the slot's own
/// boundaries and holds no byte of another slot.
#[kani::proof]
fn a_large_slot_is_whole_pages() {
    let geometry = match kani::any::<u8>() {
        0 => Geometry::DESIGN,
        1 => Standard::GEOMETRY,
        2 => Native::GEOMETRY,
        _ => Tiny::GEOMETRY,
    };
    let class = any_class();
    kani::assume(!class.is_small());
    let page = geometry.page_size();
    assert!(class.size() & (page - 1) == 0);
    // With the size a page multiple, `slot_unit`'s two roundings are exact for any slot.
    let slot: u32 = kani::any();
    // A region holds at most 2^32 bytes, so the products below are slot offsets within one.
    kani::assume((slot as u128 + 1) * class.size() as u128 <= 1 << 32);
    let (start, end) = (
        slot as usize * class.size(),
        (slot as usize + 1) * class.size(),
    );
    assert!(start.div_ceil(page) * page == start && (end >> page.trailing_zeros()) * page == end);
}

/// A decay pass's goal is never more than is retained, is all of it once that is within the
/// floor, at least a quarter otherwise, and zero only when nothing is retained.
#[kani::proof]
fn a_decay_goal_is_a_quarter_or_the_floor() {
    use super::policy::{DECAY_SHIFT, decay_goal};
    let geometry = match kani::any::<u8>() {
        0 => Geometry::DESIGN,
        1 => Standard::GEOMETRY,
        2 => Native::GEOMETRY,
        _ => Tiny::GEOMETRY,
    };
    let retained: usize = kani::any();
    let goal = decay_goal(geometry, retained);
    assert!(goal <= retained);
    assert!(goal >= retained >> DECAY_SHIFT);
    assert!((goal == 0) == (retained == 0));
    if retained <= geometry.huge_page_size() {
        assert!(goal == retained);
    }
}
