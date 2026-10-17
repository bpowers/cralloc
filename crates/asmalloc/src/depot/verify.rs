//! Kani proofs of the depot's word-level arithmetic.

use super::generation::Generation;
use super::magazine::admits;
use super::stack::Packed;
use crate::classes::{MagazineSize, SLOT_BYTES, is_marker};

fn any_magazine_size() -> MagazineSize {
    let ordinal: usize = kani::any();
    kani::assume(ordinal < MagazineSize::COUNT);
    MagazineSize::from_ordinal(ordinal).unwrap()
}

/// A head or a link packs a number and a counter into one word and gives them back, and the
/// empty head is the zero word.
#[kani::proof]
fn heads_and_links_round_trip() {
    let packed = Packed {
        number: kani::any(),
        high: kani::any(),
    };
    assert!(Packed::from_word(packed.to_word()) == packed);
    let word: u64 = kani::any();
    assert!(Packed::from_word(word).to_word() == word);
    assert!(Packed::from_word(0).number == 0);
}

/// Every generation is a non-zero marker, so a claim of one is never "no claim", every slot
/// test misses it, and no slot address can be mistaken for it.
#[kani::proof]
fn every_generation_is_a_marker_the_routines_reject() {
    let generation = Generation::from_random(kani::any());
    let word = generation.get() as usize;
    assert!(word != 0 && is_marker(word));
    assert!(Generation::from_word(generation.get()) == Some(generation));
    assert!(generation.is_mine(generation.marker()));
    let size = any_magazine_size();
    assert!(size.alloc_slow(word) && size.free_slow(word));
    let slot_address: usize = kani::any();
    kani::assume(slot_address % SLOT_BYTES == 0);
    assert!(!generation.is_mine(core::ptr::without_provenance_mut(slot_address)));
}

/// A slab word that names slot `s >= 1` of a magazine gives back the magazine's base and the
/// level `s - 1`, and the word for that level is the word: the arithmetic `Area::loaded` and
/// `Magazine::word` use, for every size and every aligned base.
#[kani::proof]
fn slab_words_and_levels_round_trip() {
    let size = any_magazine_size();
    let base: usize = kani::any();
    kani::assume(base % size.bytes() == 0 && base <= usize::MAX - size.bytes());
    let level: usize = kani::any();
    kani::assume(level <= size.capacity());
    let word = base + (level + 1) * SLOT_BYTES;
    let slot = size.slot_of(word);
    assert!(slot == level + 1);
    assert!(word - slot * SLOT_BYTES == base);
    // An empty magazine's word sends allocation to the slow path, a full one's sends free.
    assert!(size.alloc_slow(word) == (level == 0));
    assert!(size.free_slow(word) == (level == size.capacity()));
}

/// The record in an unloaded magazine's last slot gives back its level and depth.
#[kani::proof]
fn records_round_trip() {
    let level: u32 = kani::any();
    let depth: u32 = kani::any();
    let word = (depth as u64) << 32 | level as u64;
    assert!(word as u32 == level && (word >> 32) as u32 == depth);
}

/// What makes a magazine handle's slot accesses safe: an admitted offset puts every slot of the
/// magazine, slot 0 to the last, past the start and inside the committed bytes, aligned to 8.
#[kani::proof]
fn an_admitted_magazine_lies_inside_the_committed_area() {
    let size = any_magazine_size();
    let offset: usize = kani::any();
    let start: usize = kani::any();
    let committed: usize = kani::any();
    let slot: usize = kani::any();
    kani::assume(slot < size.slots());
    if admits(offset, size, start, committed) {
        let at = offset + slot * SLOT_BYTES;
        assert!(at >= start);
        assert!(at + SLOT_BYTES <= committed);
        assert!(at % SLOT_BYTES == 0);
    }
}
