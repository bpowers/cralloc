//! Kani proofs of the back end's word operations and index arithmetic, for every input, in the
//! geometries in use. `verify_slow.rs` has the properties that need division by a symbolic class
//! size, every geometry, or a run of operations on a small heap.

use super::layout::{Grow, Hints, Layout, MAX_LEVELS, WORD_BYTES, slots_in_region};
use super::word::{BITS, bits_in_word, position, span};
use crate::classes::{COUNT, Class};
use crate::os::sim::{Shape, Standard, Tiny};
use crate::space::Geometry;

pub(super) fn any_class() -> Class {
    let index: u8 = kani::any();
    kani::assume(index >= 1 && index as usize <= COUNT);
    Class::from_index(index).unwrap()
}

pub(super) fn any_used_geometry() -> Geometry {
    match kani::any::<u8>() {
        0 => Geometry::DESIGN,
        1 => Standard::GEOMETRY,
        _ => Tiny::GEOMETRY,
    }
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

/// Every class that fits a region of a used geometry has a layout, and the layout fits in the
/// region's metadata block: the leaf bitmap at offset zero covers every slot, each summary level
/// has a bit for every word below and ends in one top word, the sections do not overlap, the
/// clean bitmap covers every page, large classes have a zero bitmap covering every slot, and the
/// part committed at assignment covers everything after the leaf bitmap.
pub(super) fn layouts_fit(geometry: Geometry) {
    let class = any_class();
    let slots = slots_in_region(geometry, class);
    let layout = Layout::new(geometry, class);
    if slots == 0 {
        assert!(layout.is_none());
        return;
    }
    let layout = layout.unwrap();
    assert!(layout.slots() == slots);
    assert!(layout.total_words() as usize * WORD_BYTES <= geometry.metadata_size());
    assert!(layout.leaf_words() * BITS >= slots && (layout.leaf_words() - 1) * BITS < slots);
    assert!(layout.tree_word(0, 0) == 0);
    assert!(layout.grow_word() == layout.leaf_words());
    assert!(layout.hints_word() == layout.leaf_words() + 1);
    let height = layout.height() as usize;
    assert!(height < MAX_LEVELS);
    assert!(layout.level_words(layout.height()) == 1);
    let mut end = layout.hints_word() + 1;
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
        }
        None => assert!(class.is_small()),
    }
    assert!(end == layout.total_words());
    let (start, stop) = layout.fixed_commit();
    let page = geometry.page_size();
    assert!(start % page == 0 && stop % page == 0);
    assert!(start <= layout.grow_word() as usize * WORD_BYTES);
    assert!(stop >= end as usize * WORD_BYTES && stop <= geometry.metadata_size());
    assert!(layout.region_pages() as usize * page == geometry.region_size());
}

#[kani::proof]
#[kani::unwind(6)]
fn layouts_fit_the_used_geometries() {
    layouts_fit(any_used_geometry());
}
