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
/// region's metadata block. A small class's block: the summary levels from word zero, top
/// first, each with a bit for every word below and ending in one top word; the clean bitmap,
/// covering every page; the leaf bitmap, covering every slot, last and less than a page after
/// the rest; committed at assignment up to the leaf bitmap. A large class's block: its header
/// (`HEADER_WORDS` words, `header_word` zero) alone on a line where the block has room; the summary
/// levels; the leaf, zero and clean bitmaps; a size word for every slot; committed whole at
/// assignment. The sections do not overlap.
pub(super) fn layouts_fit(plan: Plan) {
    let class = any_class();
    let slots = slots_in_region(plan, class);
    let layout = Layout::new(plan, class);
    if slots == 0 {
        assert!(layout.is_none());
        return;
    }
    let layout = layout.unwrap();
    let page = plan.geometry().page_size();
    let line = (crate::lined::LINE / WORD_BYTES) as u32;
    assert!(layout.slots() == slots);
    assert!(layout.total_words() as usize * WORD_BYTES <= plan.metadata_size());
    assert!(layout.block_bytes() <= plan.metadata_size());
    assert!(layout.region_pages() as usize * page == plan.region_size());
    assert!(layout.leaf_words() * BITS >= slots && (layout.leaf_words() - 1) * BITS < slots);
    // A large class's header first.
    let mut end = match layout.header_word() {
        Some(at) => {
            assert!(!class.is_small() && at == 0);
            let first = if layout.height() >= 1 {
                layout.tree_word(layout.height(), 0)
            } else {
                layout.tree_word(0, 0)
            };
            assert!(first == line || first == super::layout::HEADER_WORDS);
            first
        }
        None => {
            assert!(class.is_small());
            0
        }
    };
    // The summary levels from the top down.
    let height = layout.height() as usize;
    assert!(height < MAX_LEVELS);
    assert!(layout.level_words(layout.height()) == 1);
    let mut level = height;
    while level >= 1 {
        let below = layout.level_words(level as u8 - 1);
        let words = layout.level_words(level as u8);
        assert!(words * BITS >= below && (words - 1) * BITS < below);
        assert!(layout.tree_word(level as u8, 0) == end);
        end += words;
        level -= 1;
    }
    let (start, stop) = layout.fixed_commit();
    assert!(start == 0 && stop % page == 0);
    if class.is_small() {
        assert!(layout.zero_word(0).is_none() && layout.size_word(0).is_none());
        assert!(layout.clean_word(0) == end);
        assert!(layout.clean_words() * BITS >= layout.region_pages());
        end += layout.clean_words();
        // Then the leaf bitmap, last, starting at most a page after the rest.
        let leaf = layout.tree_word(0, 0);
        assert!(leaf >= end && ((leaf - end) as usize) * WORD_BYTES < page);
        assert!(leaf + layout.leaf_words() == layout.total_words());
        // Committed at assignment: everything before the leaf bitmap, as whole pages; the leaf
        // bitmap's first commit starts in the page it starts in.
        assert!(stop >= end as usize * WORD_BYTES && stop < (leaf as usize) * WORD_BYTES + page);
        assert!(layout.leaf_commit(0) <= leaf as usize * WORD_BYTES);
        assert!(layout.leaf_commit(0) + page > leaf as usize * WORD_BYTES);
    } else {
        assert!(layout.tree_word(0, 0) == end);
        end += layout.leaf_words();
        assert!(layout.zero_word(0) == Some(end));
        end += layout.leaf_words();
        assert!(layout.clean_word(0) == end);
        assert!(layout.clean_words() * BITS >= layout.region_pages());
        end += layout.clean_words();
        assert!(layout.size_word(0) == Some(end));
        end += slots;
        assert!(end == layout.total_words());
        // Committed whole at assignment, so no carve commits more.
        assert!(stop == layout.block_bytes() && stop >= end as usize * WORD_BYTES);
        assert!(layout.leaf_commit(0) == stop);
    }
}

/// The decay word: a pass under way is stepped whatever the clock says. Otherwise a pass
/// starts exactly when the clock is in a later interval than the word's time; a clock behind
/// the word re-stamps it with the time now, after which a pass starts in the next interval;
/// and the word that starts a pass is marked active and keeps the time.
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
            assert!(since < 1 << 63 && (now & !1) / DECAY_TICKS > word / DECAY_TICKS);
        }
        Decay::Restamp(next) => {
            assert!(word & 1 == 0 && since >= 1 << 63);
            assert!(next == now & !1);
            // From the new stamp a pass waits for the next interval, wherever the clock goes
            // forward.
            assert!(decay_look(next, now) == Decay::Wait);
            let later = now.wrapping_add(DECAY_TICKS);
            if later > now {
                assert!(decay_look(next, later) == Decay::Start(later | 1));
            }
        }
        Decay::Wait => assert!(
            word & 1 == 0 && since < 1 << 63 && (now & !1) / DECAY_TICKS <= word / DECAY_TICKS
        ),
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

/// A pass word keeps its parts apart, and a stepper can tell its pass from another: the
/// tag of a pass's decay word is the only one it acts on.
#[kani::proof]
fn a_pass_word_round_trips() {
    use super::purge::Pass;
    let pass = Pass {
        tag: kani::any(),
        lap: kani::any(),
        pages: kani::any(),
        any: kani::any(),
        count: kani::any(),
    };
    kani::assume(pass.pages < 1 << 30);
    assert!(Pass::unpack(pass.pack()) == pass);
    let word: u64 = kani::any();
    assert!(Pass::unpack(word).pack() == word);
    // The all-zero word, which is what the state starts as, is a pass with nothing to purge.
    assert!(Pass::unpack(0).pages == 0);
}

/// A frontier word, in the format of any region size a geometry allows, round-trips. Read in
/// interval E it gives the higher of both frontiers if it is E's, its `now` if it is E - 1's,
/// and nothing otherwise. A claim in E that reaches page p leaves a word of E (or of E + 1, if
/// it found one of E + 1) that reads at least p in its interval and exactly p in the next,
/// nothing in the one after, and never less than the old word read in its interval; it writes
/// nothing only when the word, of E or E + 1, already reads at least p. The all-zero word
/// reads nothing in any interval but the first two.
#[kani::proof]
fn a_frontier_word_names_its_interval() {
    use super::layout::FrontierFormat;
    let pages: u32 = kani::any();
    kani::assume(pages >= 1 && pages <= 1 << 26);
    let format = FrontierFormat::of(pages);
    let word: u64 = kani::any();
    assert!(format.pack(format.unpack(word)) == word);
    let epoch: u64 = kani::any();
    let page: u32 = kani::any();
    kani::assume(page <= pages);
    let f = format.unpack(word);
    let floor = format.floor(word, epoch);
    assert!(floor == 0 || floor == f.now || floor == f.last);
    let mask = u64::MAX >> (64 - format.epoch_bits());
    match format.record(word, epoch, page) {
        Some(next) => {
            // Recorded under this interval, or the next if the word is already of the next.
            let at = format.unpack(next).epoch;
            assert!(at == epoch & mask || at == epoch.wrapping_add(1) & mask);
            assert!(format.floor(next, at) >= page);
            assert!(format.floor(next, at) >= format.floor(word, at));
            assert!(format.floor(next, at.wrapping_add(1)) == page);
            if format.epoch_bits() > 1 {
                assert!(format.floor(next, at.wrapping_add(2)) == 0);
            }
        }
        None => {
            assert!(f.epoch == epoch & mask || f.epoch == epoch.wrapping_add(1) & mask);
            assert!(format.floor(word, f.epoch) >= page);
        }
    }
    if format.epoch_bits() > 1 && epoch & (u64::MAX >> (64 - format.epoch_bits())) > 1 {
        assert!(format.floor(0, epoch) == 0);
    }
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

/// The refresh's raise is at most `REFRESH_RAISE_MAX`, none under a share of 1 in 64 or with
/// nothing carved, the most from 1 in 16, and never less for more fragmentation.
#[kani::proof]
fn a_refresh_raise_follows_the_share_within_its_bounds() {
    use super::policy::{REFRESH_RAISE_MAX, refresh_raise};
    let (fragmented, more, carved): (usize, usize, usize) = (kani::any(), kani::any(), kani::any());
    let raise = refresh_raise(fragmented, carved);
    assert!(raise <= REFRESH_RAISE_MAX);
    // A share of at least 1 in n is `fragmented >= carved.div_ceil(n)`.
    if carved == 0 || fragmented < carved.div_ceil(64) {
        assert!(raise == 0);
    }
    if carved != 0 && fragmented >= carved.div_ceil(16) {
        assert!(raise == REFRESH_RAISE_MAX);
    }
    assert!(refresh_raise(fragmented.saturating_add(more), carved) >= raise);
}

/// A clocked pass's goal is never more than the last lap found returnable, leaves exactly the
/// floor of it when there is more, and is zero, a pass that only counts, exactly when there is
/// no more than the floor.
#[kani::proof]
fn a_decay_goal_is_everything_over_the_floor() {
    use super::policy::{allowance_floor, decay_goal};
    let geometry = match kani::any::<u8>() {
        0 => Geometry::DESIGN,
        1 => Standard::GEOMETRY,
        2 => Native::GEOMETRY,
        _ => Tiny::GEOMETRY,
    };
    let floor = if kani::any() {
        allowance_floor(geometry)
    } else {
        0
    };
    let returnable: usize = kani::any();
    let goal = decay_goal(returnable, floor);
    assert!(goal <= returnable);
    assert!((goal == 0) == (returnable <= floor));
    if returnable > floor {
        assert!(returnable - goal == floor);
    }
}

/// A large slot's size word round-trips with every size a slot can hold, every stamp and every
/// flag, and the region's walk word round-trips; all zero is a size word with nothing set and
/// a walk that is due and not under way.
#[kani::proof]
fn a_size_word_and_a_walk_word_round_trip() {
    use super::layout::{AgeWalk, Recorded, STAMP_MASK};
    let size: usize = kani::any();
    kani::assume(size <= 1 << 32);
    let recorded = Recorded {
        size,
        huge: kani::any(),
        unwritten: kani::any(),
        outgrown: kani::any(),
        freed: kani::any::<u32>() & STAMP_MASK,
    };
    assert!(Recorded::unpack(recorded.pack()) == recorded);
    let zero = Recorded::unpack(0);
    assert!(zero.size == 0 && !zero.huge && !zero.unwritten && !zero.outgrown && zero.freed == 0);
    let walk = AgeWalk {
        cursor: kani::any(),
        began: kani::any(),
    };
    assert!(AgeWalk::unpack(walk.pack()) == walk);
    let zero = AgeWalk::unpack(0);
    assert!(zero.cursor == 0 && zero.began == 0);
}

/// A free slot's stamp never reads older than it is. Freed at fine count `freed` and looked at
/// `age` steps later, at any count and with any hold up to the hold's own: the slot is old only
/// if it has really been free for the hold, it waits at most the hold, and until its stamp's
/// bits wrap it reads exactly its age, so it goes back as soon as it is old.
#[kani::proof]
fn a_stamp_never_reads_older_than_it_is() {
    use super::hold::until_aged;
    use super::layout::STAMP_MASK;
    use super::policy::HOLD_STEPS;
    let (freed, age, steps): (u32, u32, u32) = (kani::any(), kani::any(), kani::any());
    kani::assume(steps >= 1 && steps <= HOLD_STEPS);
    let left = until_aged(freed & STAMP_MASK, freed.wrapping_add(age), steps);
    assert!(left <= steps);
    if left == 0 {
        assert!(age >= steps);
    }
    if age < STAMP_MASK - HOLD_STEPS {
        assert!(left == steps.saturating_sub(age) || (age == 0 && left == steps));
    }
}

/// A young region's walk starts at most once a hold: one that began at `began` may not start
/// again before `steps - 1` have passed, may then or later, and one under way may always go
/// on.
#[kani::proof]
fn a_walk_starts_at_most_once_a_hold() {
    use super::hold::walk_due;
    use super::layout::AgeWalk;
    use super::policy::HOLD_STEPS;
    let (began, since, steps): (u32, u32, u32) = (kani::any(), kani::any(), kani::any());
    kani::assume(steps >= 1 && steps <= HOLD_STEPS);
    let walk = AgeWalk { cursor: 0, began };
    assert!(walk_due(walk, began.wrapping_add(since), steps) == (since.saturating_add(1) >= steps));
    let under_way = AgeWalk {
        cursor: kani::any(),
        ..walk
    };
    kani::assume(under_way.cursor != 0);
    assert!(walk_due(under_way, kani::any(), steps));
}
