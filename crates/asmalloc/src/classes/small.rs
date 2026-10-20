//! Small classes: the ones cached in magazines.

use super::{Class, MAX_SMALL_SIZE, MagazineSize, QUANTUM, SMALL_COUNT};
use core::alloc::Layout;
use core::num::NonZeroU8;

/// The small class of every rounded size, by `(size - 1) / QUANTUM`: entry `j` is the class of
/// `QUANTUM * (j + 1)` bytes, which serves every size from `QUANTUM * j + 1` up to that.
static BY_SIZE: PlacedClasses = PlacedClasses {
    _pad: [0; BY_SIZE_PAGE_OFFSET],
    classes: {
        let mut table = [SmallClass::of(Class::SMALLEST); MAX_SMALL_SIZE / QUANTUM];
        let mut j = 0;
        while j < table.len() {
            table[j] = match Class::of_size(QUANTUM * (j + 1)) {
                Some(class) => match class.small() {
                    Some(small) => small,
                    None => panic!("a size up to MAX_SMALL_SIZE has a large class"),
                },
                None => panic!("a size up to MAX_SMALL_SIZE has no class"),
            };
            j += 1;
        }
        table
    },
};

/// Where [`BY_SIZE`]'s first entry lies within its page. An inlined allocation or free loads its
/// entry right after the previous operation stored a slab's hot word, and on Zen 5 a load whose
/// page offset overlaps a store still in flight can be held until that store is done; since the
/// store's address comes from the previous lookup, a loop of them then runs one at a time
/// (`docs/research/zen5-roofline.md`, section 16). From here, the entries of every size up to
/// [`BY_SIZE_CLEAR`] bytes have page offsets past every slab field (`front` asserts it), and
/// only larger sizes wrap round onto them.
pub(crate) const BY_SIZE_PAGE_OFFSET: usize = 2048;

/// The largest size whose [`BY_SIZE`] entry lies between [`BY_SIZE_PAGE_OFFSET`] and the end
/// of its page.
pub(crate) const BY_SIZE_CLEAR: usize = (4096 - BY_SIZE_PAGE_OFFSET) * QUANTUM;

/// [`BY_SIZE`]'s table, [`BY_SIZE_PAGE_OFFSET`] bytes into a page-aligned static.
#[repr(C, align(4096))]
struct PlacedClasses {
    _pad: [u8; BY_SIZE_PAGE_OFFSET],
    classes: [SmallClass; MAX_SMALL_SIZE / QUANTUM],
}

const _: () = assert!(core::mem::offset_of!(PlacedClasses, classes) == BY_SIZE_PAGE_OFFSET);

/// The address of the entry for `size` bytes, for the placement tests.
#[cfg(test)]
pub(crate) fn by_size_entry(size: usize) -> usize {
    core::ptr::from_ref(&BY_SIZE.classes[(size - 1) / QUANTUM]).addr()
}

/// A class of at most `MAX_SMALL_SIZE` bytes.
///
/// Small classes have a slab word and a magazine size. Their ordinal, `index - 1`, numbers them
/// from zero for arrays with one entry per small class.
///
/// It is held as the position of the class's hot word among a slab's words,
/// `WORD_STRIDE * index + WORD_START` (`front::slab` lays the slab out so): the position is what
/// the fast path needs, an address `SLABS + 8 * position` and the mask at the same position, so a
/// class found at run time by [`of_nonzero_layout`](Self::of_nonzero_layout) gives the address
/// with one scaled add, and the class index is a shift away, for the slow paths.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[repr(transparent)]
pub struct SmallClass(NonZeroU8);

/// A small class's hot word is at position `WORD_STRIDE * index + WORD_START` of its slab: one
/// 32-byte chunk per class, the word at its bytes 24 to 31.
pub(crate) const WORD_STRIDE: usize = 4;
pub(crate) const WORD_START: usize = 3;
const _: () = assert!(WORD_STRIDE.is_power_of_two() && WORD_START < WORD_STRIDE);
const _: () = assert!(WORD_STRIDE * SMALL_COUNT + WORD_START <= u8::MAX as usize);

/// The magazine size of each small class, by ordinal. A table, not the formula, because the
/// formula divides by the class size.
const MAGAZINES: [MagazineSize; SMALL_COUNT] = {
    let mut table = [MagazineSize::MIN; SMALL_COUNT];
    let mut ordinal = 0;
    while ordinal < SMALL_COUNT {
        let class = Class::from_index(ordinal as u8 + 1).unwrap();
        table[ordinal] = MagazineSize::for_class_size(class.size());
        ordinal += 1;
    }
    table
};

impl SmallClass {
    /// The class as a small class, if it is one.
    #[inline(always)]
    pub(crate) const fn new(class: Class) -> Option<SmallClass> {
        if class.is_small() {
            Some(SmallClass::of(class))
        } else {
            None
        }
    }

    /// `class`, which is small.
    const fn of(class: Class) -> SmallClass {
        match NonZeroU8::new((WORD_STRIDE * class.index() as usize + WORD_START) as u8) {
            Some(word) => SmallClass(word),
            None => panic!("a word position is never zero"),
        }
    }

    /// The position of the class's hot word among a slab's words.
    #[inline(always)]
    pub(crate) const fn word(self) -> usize {
        self.0.get() as usize
    }

    /// The small class with this ordinal, if there is one.
    #[inline(always)]
    pub(crate) const fn from_ordinal(ordinal: usize) -> Option<SmallClass> {
        if ordinal < SMALL_COUNT {
            match Class::from_index(ordinal as u8 + 1) {
                Some(class) => Some(SmallClass::of(class)),
                None => None,
            }
        } else {
            None
        }
    }

    /// The small class of a Rust request of at least one byte, as [`Class::of_layout`] gives
    /// it, with one compare, a shift and a table load; `None` for a zero size and for every
    /// request without a small class.
    ///
    /// `(size - 1) | (align - 1)` is the size rounded up to the alignment, less one, for any
    /// size of at least one byte and power-of-two alignment. A zero size wraps it to the top, so
    /// one compare turns away both a zero size, which [`Class::of_layout`] counts as one byte,
    /// and a request too large to be small. A constant layout folds to a constant class.
    #[inline(always)]
    pub(crate) fn of_nonzero_layout(layout: Layout) -> Option<SmallClass> {
        let less_one = layout.size().wrapping_sub(1) | (layout.align() - 1);
        if less_one < MAX_SMALL_SIZE {
            Some(BY_SIZE.classes[less_one / QUANTUM])
        } else {
            None
        }
    }

    /// The class.
    #[inline(always)]
    pub const fn class(self) -> Class {
        match Class::from_index((self.word() / WORD_STRIDE) as u8) {
            Some(class) => class,
            None => Class::SMALLEST,
        }
    }

    /// The position among the small classes, `0..SMALL_COUNT`.
    #[inline(always)]
    pub(crate) const fn ordinal(self) -> usize {
        self.word() / WORD_STRIDE - 1
    }

    /// The slot size in bytes.
    #[inline(always)]
    pub const fn size(self) -> usize {
        self.class().size()
    }

    /// The size of this class's magazines.
    #[inline(always)]
    pub const fn magazine(self) -> MagazineSize {
        let ordinal = self.ordinal();
        // Always in bounds; the test lets the compiler drop the bounds check and its panic.
        if ordinal < SMALL_COUNT {
            MAGAZINES[ordinal]
        } else {
            MagazineSize::MIN
        }
    }

    /// Every small class, smallest first.
    pub fn all() -> impl Iterator<Item = SmallClass> {
        (0..SMALL_COUNT).filter_map(SmallClass::from_ordinal)
    }
}

#[cfg(test)]
mod tests {
    use super::super::{MAGAZINE_BUDGET, MAX_SMALL_SIZE};
    use super::*;
    use std::vec::Vec;

    #[test]
    fn ordinals_round_trip() {
        for (ordinal, small) in SmallClass::all().enumerate() {
            assert_eq!(small.ordinal(), ordinal);
            assert_eq!(SmallClass::from_ordinal(ordinal), Some(small));
            assert_eq!(small.class().small(), Some(small));
        }
        assert_eq!(SmallClass::from_ordinal(SMALL_COUNT), None);
        assert_eq!(SmallClass::all().count(), SMALL_COUNT);
        assert_eq!(SmallClass::all().last().unwrap().size(), MAX_SMALL_SIZE);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn the_table_lookup_gives_the_computed_class_of_every_small_request() {
        for log in 0..=16 {
            let align = 1usize << log;
            for size in 0..=(MAX_SMALL_SIZE + 2 * align) {
                let Ok(layout) = Layout::from_size_align(size, align) else {
                    continue;
                };
                let computed = Class::of_layout(layout).and_then(Class::small);
                let looked_up = SmallClass::of_nonzero_layout(layout);
                if size == 0 {
                    assert_eq!(looked_up, None);
                } else {
                    assert_eq!(looked_up, computed, "size {size} align {align}");
                }
            }
        }
        for (size, align) in [
            (usize::MAX / 2, 1),
            (1, 1 << 40),
            (isize::MAX as usize - 7, 8),
        ] {
            let layout = Layout::from_size_align(size, align).unwrap();
            assert_eq!(SmallClass::of_nonzero_layout(layout), None);
        }
    }

    #[test]
    fn magazines_shrink_as_classes_grow_and_hold_at_most_the_budget() {
        let mut previous = MagazineSize::MAX;
        for small in SmallClass::all() {
            let magazine = small.magazine();
            assert!(magazine <= previous, "{small:?}");
            assert!(
                magazine.capacity() * small.size() <= MAGAZINE_BUDGET,
                "{small:?}"
            );
            previous = magazine;
        }
    }

    #[test]
    fn magazine_sizes_follow_the_budget() {
        let slots: Vec<(usize, usize)> = SmallClass::all()
            .map(|s| (s.size(), s.magazine().slots()))
            .filter(|&(size, _)| size.is_power_of_two())
            .collect();
        assert_eq!(
            slots,
            [
                (8, 1024),
                (16, 1024),
                (32, 1024),
                (64, 1024),
                (128, 512),
                (256, 256),
                (512, 128),
                (1024, 64),
                (2048, 32),
                (4096, 16),
                (8192, 8),
                (16384, 4),
                (32768, 4),
            ]
        );
    }
}
