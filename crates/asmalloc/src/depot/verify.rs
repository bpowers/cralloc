//! Kani proofs of the depot's word-level arithmetic.

use super::age::{Age, Stamp};
use super::generation::Generation;
use super::heartbeat::{Look, Sighting};
use super::magazine::MagazineId;
use super::magazine::admits;
use super::shed::Water;
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

/// The age limit never takes a magazine its stamp does not name, a fresh stamp reads as young,
/// and a stamp makes a magazine old only once an interval has passed: for every stamp, magazine
/// number and clock.
#[kani::proof]
fn a_stamp_ages_only_its_own_magazine_and_only_after_an_interval() {
    let number: u32 = kani::any();
    kani::assume(number != 0);
    let id = MagazineId::new(number).unwrap();
    let stamp = Stamp(kani::any());
    let now: u64 = kani::any();
    if stamp.0 as u32 != number {
        assert!(matches!(stamp.age(id, now), Age::Unstamped(_)));
    }
    let fresh = Stamp::new(id, now);
    assert!(fresh.0 != 0);
    let elapsed: u64 = kani::any();
    kani::assume(elapsed < super::IDLE_TICKS - (1 << 20));
    assert!(fresh.age(id, now.wrapping_add(elapsed)) == Age::Young);
    assert!(fresh.age(id, now.wrapping_add(super::IDLE_TICKS)) == Age::Old);
}

/// A look never leaves the mark above the depth it saw, a new debt is never more than the old
/// mark or the depth now, a note without the clock never raises the mark or changes the debt,
/// and paying never raises a debt: for every word, depth and interval.
#[kani::proof]
fn a_low_water_mark_owes_at_most_what_was_never_used() {
    let water = Water(kani::any());
    let depth: u32 = kani::any();
    let interval: u64 = kani::any();
    let next = water.look(depth, interval);
    assert!(next.low() <= depth);
    assert!(next.owed() <= depth.max(water.owed()));
    if next.owed() > water.owed() {
        assert!(next.owed() <= water.low());
    }
    assert!(Water(0).look(depth, interval).owed() == 0);
    // A note without the clock never changes the debt, and never raises the mark.
    let noted = water.lower(depth);
    assert!(noted.owed() == water.owed());
    assert!(noted.low() <= water.low());
    let paid: u32 = kani::any();
    assert!(water.paid(paid).owed() <= water.owed());
}

/// The idle check never drains on a first sighting, never drains a slab twice without a change
/// in its counter between, and drains only a sighting at least an interval old: for every
/// sighting, counter and pair of clock readings.
#[kani::proof]
fn a_sighting_drains_once_and_only_after_an_interval() {
    let sighting = Sighting(kani::any());
    let activity: u64 = kani::any();
    let now: u64 = kani::any();
    match sighting.look(activity, now) {
        Look::Record(fresh) => {
            assert!(fresh.look(activity, now) == Look::Wait);
        }
        Look::Drain(marked) => {
            assert!(sighting.0 != 0);
            assert!(marked.look(activity, kani::any()) == Look::Wait);
        }
        Look::Wait => {}
    }
    assert!(!matches!(Sighting(0).look(activity, now), Look::Drain(_)));
    // A slab first seen at `then` is not drained before an interval has passed.
    let then: u64 = kani::any();
    let elapsed: u64 = kani::any();
    kani::assume(elapsed < super::IDLE_TICKS - (1 << 20));
    if let Look::Record(fresh) = Sighting(0).look(activity, then) {
        assert!(fresh.look(activity, then.wrapping_add(elapsed)) == Look::Wait);
        assert!(matches!(
            fresh.look(activity, then.wrapping_add(super::IDLE_TICKS)),
            Look::Drain(_)
        ));
    }
}
