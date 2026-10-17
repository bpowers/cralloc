//! Magazine sizes and the slot-number tests the fast paths make.
//!
//! A magazine of `n` slots is `8n` bytes, `n` a power of two, aligned to its size. Slot 0 is the
//! depot's link word, slot `n - 1` is never used, and slots 1 to `n - 2` hold pointers to free
//! objects. A slab word is the address of the next free slot of the loaded magazine, so its low
//! bits are the slot number times 8, and the fast paths can bound-check with one mask:
//! `8n - 16` covers every slot-number bit except the lowest.
//!
//! - Allocation goes to the slow path when `word & mask` is zero: slot 0 or 1, meaning no
//!   magazine or an empty one.
//! - Free goes to the slow path when `(word + 8) & mask` is zero: slot 0 or `n - 1`, meaning no
//!   magazine or a full one.
//!
//! A zero word, which is what a never-written slab holds, fails both, and so does [`MARKER`].
//! `verify.rs` proves all of this for every magazine size and every magazine address.

/// Bytes per magazine slot: one pointer.
pub const SLOT_BYTES: usize = 8;

/// The most bytes of objects a magazine holds, unless that would leave it fewer than
/// [`MagazineSize::MIN`] slots.
///
/// The design asks that no magazine hold more than a few hundred kilobytes. This budget is a
/// starting point, not a measurement: it gives 254-object magazines up to 512-byte classes and
/// two-object magazines at 32 KiB. It bounds the memory one cached magazine can pin against the
/// cost of exchanging magazines, two atomic instructions each.
pub const MAGAZINE_BUDGET: usize = 128 << 10;

/// The slab word that marks a class's slot as being drained by another thread (see "Idle
/// caches" in the design). No slot address has low bits `001`, so it is never a real word.
pub const MARKER: usize = 1;

const MIN_LOG2: u8 = 2;
const MAX_LOG2: u8 = 8;

/// A magazine size: a power-of-two number of slots, from 4 to 256.
///
/// Four is the least for which the two tests above work: with two slots the mask is zero.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[repr(transparent)]
pub struct MagazineSize(u8);

impl MagazineSize {
    /// The smallest magazine: 4 slots, 2 objects.
    pub const MIN: MagazineSize = MagazineSize(MIN_LOG2);

    /// The largest magazine: 256 slots, 254 objects, 2 KiB.
    pub const MAX: MagazineSize = MagazineSize(MAX_LOG2);

    /// The number of magazine sizes.
    pub const COUNT: usize = (MAX_LOG2 - MIN_LOG2 + 1) as usize;

    /// The magazine size with this many slots, if there is one.
    #[inline]
    pub const fn from_slots(slots: usize) -> Option<MagazineSize> {
        if slots.is_power_of_two()
            && slots >= MagazineSize::MIN.slots()
            && slots <= Self::MAX.slots()
        {
            Some(MagazineSize(slots.trailing_zeros() as u8))
        } else {
            None
        }
    }

    /// The magazine size with this ordinal, `0..COUNT`, if there is one.
    #[inline]
    pub const fn from_ordinal(ordinal: usize) -> Option<MagazineSize> {
        if ordinal < Self::COUNT {
            Some(MagazineSize(MIN_LOG2 + ordinal as u8))
        } else {
            None
        }
    }

    /// The size for a class of `size` bytes: the most slots whose objects fit in
    /// [`MAGAZINE_BUDGET`], within [`MagazineSize::MIN`] and [`MagazineSize::MAX`]. `size` must
    /// not be zero. Evaluated at compile time into a table; it divides.
    pub(super) const fn for_class_size(size: usize) -> MagazineSize {
        let fit = MAGAZINE_BUDGET / size + 2;
        let log2 = (usize::BITS - 1 - fit.leading_zeros()) as u8;
        if log2 < MIN_LOG2 {
            Self::MIN
        } else if log2 > MAX_LOG2 {
            Self::MAX
        } else {
            MagazineSize(log2)
        }
    }

    /// The number of slots, `n`.
    #[inline(always)]
    pub const fn slots(self) -> usize {
        1 << self.0
    }

    /// The number of objects a full magazine holds, `n - 2`.
    #[inline(always)]
    pub const fn capacity(self) -> usize {
        self.slots() - 2
    }

    /// The size in bytes, `8n`, which is also the magazine's alignment.
    #[inline(always)]
    pub const fn bytes(self) -> usize {
        SLOT_BYTES << self.0
    }

    /// The position among the magazine sizes, `0..COUNT`, smallest first.
    #[inline(always)]
    pub const fn ordinal(self) -> usize {
        (self.0 - MIN_LOG2) as usize
    }

    /// The mask the fast paths test, `8n - 16`.
    #[inline(always)]
    pub const fn mask(self) -> usize {
        self.bytes() - 2 * SLOT_BYTES
    }

    /// The slot number a word points at, for a word that is a slot address of a magazine of this
    /// size.
    #[inline(always)]
    pub const fn slot_of(self, word: usize) -> usize {
        (word & (self.bytes() - 1)) / SLOT_BYTES
    }

    /// Whether allocation must take the slow path: the word names no magazine or an empty one.
    #[inline(always)]
    pub const fn alloc_slow(self, word: usize) -> bool {
        word & self.mask() == 0
    }

    /// Whether free must take the slow path: the word names no magazine or a full one.
    #[inline(always)]
    pub const fn free_slow(self, word: usize) -> bool {
        word.wrapping_add(SLOT_BYTES) & self.mask() == 0
    }

    /// Every magazine size, smallest first.
    pub fn all() -> impl Iterator<Item = MagazineSize> {
        (0..Self::COUNT).filter_map(MagazineSize::from_ordinal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_round_trip() {
        for (ordinal, size) in MagazineSize::all().enumerate() {
            assert_eq!(size.ordinal(), ordinal);
            assert_eq!(MagazineSize::from_slots(size.slots()), Some(size));
            assert_eq!(size.bytes(), size.slots() * SLOT_BYTES);
        }
        assert_eq!(MagazineSize::all().count(), MagazineSize::COUNT);
        assert_eq!(MagazineSize::from_slots(2), None);
        assert_eq!(MagazineSize::from_slots(12), None);
        assert_eq!(MagazineSize::from_slots(512), None);
        assert_eq!(MagazineSize::from_ordinal(MagazineSize::COUNT), None);
    }

    #[test]
    fn the_tests_pick_out_the_boundary_slots() {
        for size in MagazineSize::all() {
            let n = size.slots();
            for base in [0x7f00_0000_0000usize, 0x1234_5000, size.bytes() * 3] {
                for slot in 0..n {
                    let word = base + slot * SLOT_BYTES;
                    assert_eq!(size.slot_of(word), slot);
                    assert_eq!(size.alloc_slow(word), slot <= 1, "{size:?} slot {slot}");
                    assert_eq!(size.free_slow(word), slot == 0 || slot == n - 1);
                }
            }
            for word in [0, MARKER] {
                assert!(
                    size.alloc_slow(word) && size.free_slow(word),
                    "{size:?} {word}"
                );
            }
        }
    }

    #[test]
    fn the_class_rule_respects_the_bounds() {
        assert_eq!(MagazineSize::for_class_size(8), MagazineSize::MAX);
        assert_eq!(MagazineSize::for_class_size(1 << 20), MagazineSize::MIN);
        assert_eq!(MagazineSize::for_class_size(1024).slots(), 128);
    }
}
