//! Small classes: the ones cached in magazines.

use super::{Class, MAX_SMALL_SIZE, MagazineSize, QUANTUM, SMALL_COUNT};
use core::alloc::Layout;

/// The small class of every rounded size, by `(size - 1) / QUANTUM`: entry `j` is the class of
/// `QUANTUM * (j + 1)` bytes, which serves every size from `QUANTUM * j + 1` up to that.
static BY_SIZE: [SmallClass; MAX_SMALL_SIZE / QUANTUM] = {
    let mut table = [SmallClass(Class::SMALLEST); MAX_SMALL_SIZE / QUANTUM];
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
};

/// A class of at most `MAX_SMALL_SIZE` bytes.
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
    pub(crate) const fn new(class: Class) -> Option<SmallClass> {
        if class.is_small() {
            Some(SmallClass(class))
        } else {
            None
        }
    }

    /// The small class with this ordinal, if there is one.
    #[inline(always)]
    pub(crate) const fn from_ordinal(ordinal: usize) -> Option<SmallClass> {
        if ordinal < SMALL_COUNT {
            match Class::from_index(ordinal as u8 + 1) {
                Some(class) => Some(SmallClass(class)),
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
            Some(BY_SIZE[less_one / QUANTUM])
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
    pub(crate) const fn ordinal(self) -> usize {
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
                (8, 256),
                (16, 256),
                (32, 256),
                (64, 256),
                (128, 256),
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
