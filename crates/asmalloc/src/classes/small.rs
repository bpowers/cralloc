//! Small classes: the ones cached in magazines.

use super::{Class, MagazineSize, SMALL_COUNT};

/// A class of at most [`super::MAX_SMALL_SIZE`] bytes.
///
/// Small classes have a slab word and a magazine size. Their ordinal, `index - 1`, numbers them
/// from zero for arrays with one entry per small class, such as the slab's hot words.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[repr(transparent)]
pub struct SmallClass(Class);

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
    pub const fn new(class: Class) -> Option<SmallClass> {
        if class.is_small() {
            Some(SmallClass(class))
        } else {
            None
        }
    }

    /// The small class with this ordinal, if there is one.
    #[inline(always)]
    pub const fn from_ordinal(ordinal: usize) -> Option<SmallClass> {
        if ordinal < SMALL_COUNT {
            match Class::from_index(ordinal as u8 + 1) {
                Some(class) => Some(SmallClass(class)),
                None => None,
            }
        } else {
            None
        }
    }

    /// The class.
    #[inline(always)]
    pub const fn class(self) -> Class {
        self.0
    }

    /// The position among the small classes, `0..SMALL_COUNT`.
    #[inline(always)]
    pub const fn ordinal(self) -> usize {
        self.0.index() as usize - 1
    }

    /// The slot size in bytes.
    #[inline(always)]
    pub const fn size(self) -> usize {
        self.0.size()
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
                (8, 256),
                (16, 256),
                (32, 256),
                (64, 256),
                (128, 256),
                (256, 256),
                (512, 256),
                (1024, 128),
                (2048, 64),
                (4096, 32),
                (8192, 16),
                (16384, 8),
                (32768, 4),
            ]
        );
    }
}
