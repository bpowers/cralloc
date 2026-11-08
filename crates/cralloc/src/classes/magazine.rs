//! Magazine sizes and the slot-number tests the fast paths make.
//!
//! A magazine of `n` slots is `8n` bytes, `n` a power of two, aligned to its size. Slot 0 is the
//! depot's link word, slot `n - 1` holds the fill level while the magazine is not loaded, and
//! slots 1 to `n - 2` hold pointers to free objects. A slab word is the address of the next free
//! slot of the loaded magazine, so its low
//! bits are the slot number times 8, and the fast paths can bound-check with one mask:
//! `8n - 16` covers every slot-number bit except the lowest.
//!
//! - Allocation goes to the slow path when `word & mask` is zero: slot 0 or 1, meaning no
//!   magazine or an empty one.
//! - Free goes to the slow path when `(word + 8) & mask` is zero: slot 0 or `n - 1`, meaning no
//!   magazine or a full one.
//!
//! A zero word, which is what a never-written slab holds, fails both, and so does every marker
//! (a word whose low [`MARKER_SHIFT`] bits are exactly 1; see [`is_marker`]). `verify.rs` proves
//! all of this for every magazine size, every magazine address and every marker.

/// Bytes per magazine slot: one pointer.
pub const SLOT_BYTES: usize = crate::spec::WORD_BYTES;

/// The most bytes of objects a magazine holds, unless that would leave it fewer than
/// `MagazineSize::MIN` slots.
///
/// It bounds the memory one cached magazine can pin against the cost of exchanging magazines.
/// With [`MagazineSize::MAX`] it is the whole rule for a class's magazine: up to 64 KiB of
/// objects, at most 1,024 slots, at least 4. That gives 1,022-object magazines up to 64-byte
/// classes, 254 objects to 256-byte ones and two objects from 16 KiB. Halving the budget from
/// 128 KiB brought the mixed-size pipeline down to jemalloc's memory for a few percent on that
/// job and no resolvable cost elsewhere (`docs/research/memory.md`, "Magazine budget").
pub const MAGAZINE_BUDGET: usize = 64 << 10;

/// The low bits of a marker: a marker is `(generation << MARKER_SHIFT) | MARKER`.
///
/// A marker stands in a slab word or a spare-stack head while another thread drains it (see
/// "Idle caches" in the design). No slot address has its lowest bit set, so a marker is never a
/// real word. The generation sits above every slot-number bit of every magazine the routines
/// accept (up to `2^16` slots, `2^19` bytes), so a marker fails both slot tests whatever the
/// generation. Setting only the lowest bit would not do: a marker with a slot-number bit set
/// passes the allocation test.
pub const MARKER: usize = 1;

/// The number of low bits that are exactly [`MARKER`] in every marker: the specification's,
/// `MAG_LOG_MAX + 3` (`IsMarker` in `lean/Cralloc/Spec/Slab.lean`).
pub const MARKER_SHIFT: u32 = crate::spec::MARKER_SHIFT;

/// Whether a slab word or stack head is a marker.
#[inline(always)]
pub const fn is_marker(word: usize) -> bool {
    word & ((1 << MARKER_SHIFT) - 1) == MARKER
}

const _: () = assert!(MagazineSize::MAX.bytes() <= 1 << MARKER_SHIFT);

/// A marker: a slab word or stack head that stands for "being drained by the process of this
/// generation". Its word is `(generation << MARKER_SHIFT) | MARKER`, so its low
/// [`MARKER_SHIFT`] bits are exactly 1 and it fails both slot tests whatever the generation.
///
/// The generation keeps its low `64 - MARKER_SHIFT` bits; generation 0 gives the word 1, which
/// a child forked before drawing its generation would write.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[repr(transparent)]
pub struct Marker(usize);

impl Marker {
    /// The marker of a generation.
    #[inline(always)]
    pub const fn new(generation: u64) -> Marker {
        Marker(((generation as usize) << MARKER_SHIFT) | MARKER)
    }

    /// The marker a word is, if it is one.
    #[inline(always)]
    pub const fn from_word(word: usize) -> Option<Marker> {
        if is_marker(word) {
            Some(Marker(word))
        } else {
            None
        }
    }

    /// The word to store.
    #[inline(always)]
    pub const fn word(self) -> usize {
        self.0
    }

    /// The generation, in the bits it keeps.
    #[inline(always)]
    pub const fn generation(self) -> u64 {
        (self.0 >> MARKER_SHIFT) as u64
    }
}

const MIN_LOG2: u8 = 2;
/// `log2` of the most slots a magazine has: 1,024, so that the smallest classes' magazines hold
/// what a 64-byte class's does, 64 KiB of objects, and no more objects than that. Up to 1,024
/// slots, larger magazines halved sh8bench's gap and took 10 to 18% off the cross-thread
/// pipelines; past it the cross-thread gains stopped while the memory parked in loaded
/// magazines, and the work one slow path does with one, kept doubling
/// (`docs/research/magazines.md`, "What magazine size buys"). The routines accept
/// up to `2^MAG_LOG_MAX` slots.
const MAX_LOG2: u8 = 10;
const _: () = assert!(MAX_LOG2 as u32 <= crate::spec::MAG_LOG_MAX);

/// A magazine size: a power-of-two number of slots, from 4 to 1,024.
///
/// Four is the least for which the two tests above work: with two slots the mask is zero.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[repr(transparent)]
pub struct MagazineSize(u8);

impl MagazineSize {
    /// The smallest magazine: 4 slots, 2 objects.
    pub(crate) const MIN: MagazineSize = MagazineSize(MIN_LOG2);

    /// The largest magazine: 1,024 slots, 1,022 objects, 8 KiB.
    pub const MAX: MagazineSize = MagazineSize(MAX_LOG2);

    /// The number of magazine sizes.
    pub(crate) const COUNT: usize = (MAX_LOG2 - MIN_LOG2 + 1) as usize;

    /// The magazine size with this many slots, if there is one.
    #[inline]
    #[cfg(any(test, kani))]
    pub(crate) const fn from_slots(slots: usize) -> Option<MagazineSize> {
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
    pub(crate) const fn from_ordinal(ordinal: usize) -> Option<MagazineSize> {
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
    pub(crate) const fn ordinal(self) -> usize {
        (self.0 - MIN_LOG2) as usize
    }

    /// The mask the fast paths test, `8n - 16`.
    #[inline(always)]
    pub(crate) const fn mask(self) -> usize {
        self.bytes() - 2 * SLOT_BYTES
    }

    /// The slot number a word points at, for a word that is a slot address of a magazine of this
    /// size.
    #[inline(always)]
    pub(crate) const fn slot_of(self, word: usize) -> usize {
        (word & (self.bytes() - 1)) / SLOT_BYTES
    }

    /// Whether allocation must take the slow path: the word names no magazine or an empty one.
    #[inline(always)]
    pub(crate) const fn alloc_slow(self, word: usize) -> bool {
        word & self.mask() == 0
    }

    /// Whether free must take the slow path: the word names no magazine or a full one.
    #[inline(always)]
    pub(crate) const fn free_slow(self, word: usize) -> bool {
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
    use crate::spec;

    /// Every magazine size is one the routines accept, with the mask they were proved for.
    #[test]
    fn the_masks_match_the_routines() {
        for size in MagazineSize::all() {
            let proved = match size.slots() {
                4 => spec::MAG_MASK_4,
                8 => spec::MAG_MASK_8,
                16 => spec::MAG_MASK_16,
                32 => spec::MAG_MASK_32,
                64 => spec::MAG_MASK_64,
                128 => spec::MAG_MASK_128,
                256 => spec::MAG_MASK_256,
                512 => spec::MAG_MASK_512,
                1024 => spec::MAG_MASK_1024,
                2048 => spec::MAG_MASK_2048,
                4096 => spec::MAG_MASK_4096,
                8192 => spec::MAG_MASK_8192,
                16384 => spec::MAG_MASK_16384,
                32768 => spec::MAG_MASK_32768,
                65536 => spec::MAG_MASK_65536,
                n => panic!("no proved mask for {n} slots"),
            };
            assert_eq!(size.mask(), proved as usize, "{} slots", size.slots());
            let log = size.slots().trailing_zeros();
            assert!((spec::MAG_LOG_MIN..=spec::MAG_LOG_MAX).contains(&log));
        }
        assert_eq!(MARKER_SHIFT, spec::MAG_LOG_MAX + 3);
    }

    /// `is_marker` and `Marker` transcribe Lean's `IsMarker`: the low `MARKER_SHIFT` bits are 1.
    #[test]
    fn markers_are_what_the_specification_says() {
        let is_marker_in_lean = |w: usize| w % (1 << MARKER_SHIFT) == 1;
        let low = (1usize << MARKER_SHIFT) - 1;
        for high in [0, 1, 0xdead_beef, usize::MAX >> MARKER_SHIFT] {
            for low_bits in [0, 1, 2, 3, 8, 9, low - 1, low] {
                let word = high << MARKER_SHIFT | low_bits;
                assert_eq!(is_marker(word), is_marker_in_lean(word), "{word:#x}");
                assert_eq!(Marker::from_word(word).is_some(), is_marker_in_lean(word));
            }
            assert!(is_marker_in_lean(Marker::new(high as u64).word()));
        }
    }

    #[test]
    fn markers_carry_their_generation_and_fail_both_tests() {
        for generation in [0u64, 1, 0xdead_beef, u64::MAX] {
            let marker = Marker::new(generation);
            assert!(is_marker(marker.word()));
            assert_eq!(Marker::from_word(marker.word()), Some(marker));
            assert_eq!(
                marker.generation(),
                generation & ((1 << (64 - MARKER_SHIFT)) - 1)
            );
            for size in MagazineSize::all() {
                assert!(size.alloc_slow(marker.word()) && size.free_slow(marker.word()));
            }
        }
        assert_eq!(Marker::new(0).word(), MARKER);
        // Odd words that are not markers can pass a test, which is why a marker's low bits are
        // fixed: 0x11 passes the allocation test of a 4-slot magazine, 0x9 its free test.
        assert_eq!(Marker::from_word(0x11), None);
        assert!(!MagazineSize::MIN.alloc_slow(0x11));
        assert!(!MagazineSize::MIN.free_slow(0x9));
    }

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
        assert_eq!(MagazineSize::from_slots(2048), None);
        assert_eq!(MagazineSize::from_ordinal(MagazineSize::COUNT), None);
    }

    #[test]
    fn the_tests_pick_out_the_boundary_slots() {
        for size in MagazineSize::all() {
            let n = size.slots();
            for base in [0x7f00_0000_0000usize, 0x1234_0000, size.bytes() * 3] {
                for slot in 0..n {
                    let word = base + slot * SLOT_BYTES;
                    assert_eq!(size.slot_of(word), slot);
                    assert_eq!(size.alloc_slow(word), slot <= 1, "{size:?} slot {slot}");
                    assert_eq!(size.free_slow(word), slot == 0 || slot == n - 1);
                }
            }
            for word in [
                0,
                MARKER,
                0x1234_5678 << MARKER_SHIFT | MARKER,
                usize::MAX << 19 | 1,
            ] {
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
        assert_eq!(MagazineSize::for_class_size(1024).slots(), 64);
        assert_eq!(MagazineSize::for_class_size(64).slots(), 1024);
        assert_eq!(MagazineSize::for_class_size(80).slots(), 512);
    }
}
