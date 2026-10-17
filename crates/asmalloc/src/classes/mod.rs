//! Size classes: which slot size serves a request, and how the small ones are cached.
//!
//! A class is a slot size. The classes are every multiple of 8 bytes up to 64, then four per
//! doubling up to 4 GiB: 8, 16, ..., 64, 80, 96, 112, 128, 160, 192, .... A request is rounded
//! up to a multiple of its alignment and served by the smallest class at least that large.
//!
//! Every class size is `q << e` with `q` one of 1, 3, 5 or 7 and `e >= 3`. The Kani harnesses in
//! `verify.rs` (in the merge gate) and `verify_slow.rs` (on demand) prove, for every input:
//!
//! - **Alignment by construction.** If a size is a multiple of a power of two, so is its class.
//!   Regions are aligned to their size and slots sit at multiples of the class size, so a slot
//!   is aligned to every power of two that divides its class size, up to the region size. A
//!   request needs no padding and no interior pointer to be aligned.
//! - **The fit rule.** `Allocator::deallocate` may be called with any size from the requested
//!   one up to the length `allocate` returned, which is the class size. Every size in that
//!   interval maps to the same class.
//! - **Division without a divide.** [`Divisor`] turns an offset within a region into a slot
//!   index with two shifts and a multiply.
//! - **The magazine tests.** [`MagazineSize`] holds the mask the fast paths test, and the two
//!   tests mean exactly what the design says.
//!
//! Classes up to [`MAX_SMALL_SIZE`] are small ([`SmallClass`]): they are cached in magazines and
//! the Rust entry points compute them from the `Layout`. The rest are large.
//!
//! Everything here is a `const fn`, so a constant `Layout` folds to a constant class.

mod divisor;
mod magazine;
mod small;
#[cfg(kani)]
mod verify;
#[cfg(kani)]
mod verify_slow;

pub use divisor::Divisor;
pub use magazine::{MAGAZINE_BUDGET, MARKER, MARKER_SHIFT, MagazineSize, SLOT_BYTES, is_marker};
pub use small::SmallClass;

use core::alloc::Layout;
use core::fmt;
use core::num::NonZeroU8;

/// The smallest class, and the spacing of the classes up to 64 bytes. Every class size is a
/// multiple of it.
pub const QUANTUM: usize = 8;

/// Classes up to 64 bytes are every multiple of [`QUANTUM`]; there are this many of them.
const EXACT_COUNT: usize = 8;

/// The largest class. In the design's geometry one slot of it fills a region; anything larger
/// takes a run of whole regions.
pub const MAX_SIZE: usize = 1 << 32;

/// The number of classes. Their indexes run from 1 to `COUNT`; index 0 means "no class" in the
/// class-of-region table, so that an all-zero table is valid.
pub const COUNT: usize = 112;

/// The largest small class.
///
/// The design asks for "a few tens of kilobytes". At this size a magazine still holds a few
/// objects within [`MAGAZINE_BUDGET`]; above it the magazine would be mostly bookkeeping, and the
/// object's own cost (touching tens of kilobytes) dwarfs the few atomic instructions a large
/// allocation costs.
pub const MAX_SMALL_SIZE: usize = 32 << 10;

/// The number of small classes: those with indexes 1 to `SMALL_COUNT`.
pub const SMALL_COUNT: usize = match Class::of_size(MAX_SMALL_SIZE) {
    Some(class) => class.index() as usize,
    None => panic!("MAX_SMALL_SIZE is beyond the classes"),
};

/// C requires a block of this many bytes or more to be aligned to this many bytes (C17
/// 7.22.3: aligned for any object type that fits, and `max_align_t` is 16 bytes on x86-64).
pub const C_ALIGN: usize = 16;

const _: () = {
    assert!(Class::LARGEST.size() == MAX_SIZE);
    assert!(Class::LARGEST.index() as usize == COUNT);
    assert!(SMALL_COUNT <= COUNT);
    assert!(Class::LARGEST_SMALL.size() == MAX_SMALL_SIZE);
};

/// A size class.
///
/// The value is the class index, 1 to [`COUNT`], so a `Class` is always a real class and
/// `Option<Class>` is one byte with `None` as zero. Classes order by size.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct Class(NonZeroU8);

impl Class {
    /// The smallest class, [`QUANTUM`] bytes.
    pub const SMALLEST: Class = Class(NonZeroU8::MIN);

    /// The largest class, [`MAX_SIZE`] bytes.
    pub const LARGEST: Class = Class::from_index(COUNT as u8).unwrap();

    /// The largest small class, [`MAX_SMALL_SIZE`] bytes.
    pub const LARGEST_SMALL: Class = Class::from_index(SMALL_COUNT as u8).unwrap();

    /// The class with this index, if there is one.
    #[inline(always)]
    pub const fn from_index(index: u8) -> Option<Class> {
        match NonZeroU8::new(index) {
            Some(nonzero) if index as usize <= COUNT => Some(Class(nonzero)),
            _ => None,
        }
    }

    /// This class's index, 1 to [`COUNT`].
    #[inline(always)]
    pub const fn index(self) -> u8 {
        self.0.get()
    }

    /// The smallest class of at least `size` bytes, or `None` above [`MAX_SIZE`]. A size of zero
    /// gets the smallest class.
    #[inline(always)]
    pub const fn of_size(size: usize) -> Option<Class> {
        if size > MAX_SIZE {
            return None;
        }
        let words = size.div_ceil(QUANTUM);
        let index = if words <= EXACT_COUNT {
            if words == 0 { 1 } else { words }
        } else {
            // Four classes per doubling. With `w = words - 1` and `b` the position of its top
            // bit, the two bits below the top bit pick the quarter, and the `- 3` lines the
            // count up with the exact classes below 64 bytes. This is mimalloc's bin formula
            // (`page-queue.c`, `mi_bin`) with an 8-byte word.
            let w = words - 1;
            let b = (usize::BITS - 1 - w.leading_zeros()) as usize;
            4 * b + ((w >> (b - 2)) & 3) - 3
        };
        // `index` is at most COUNT here (proved by `every_size_has_the_smallest_class_that_fits`),
        // so the cast is exact.
        Class::from_index(index as u8)
    }

    /// The class for a Rust request: the size rounded up to the alignment, then classed.
    ///
    /// A zero size counts as one byte, so that a zero-size request with a large alignment still
    /// gets an aligned slot. `None` means the rounded size is above [`MAX_SIZE`].
    #[inline(always)]
    pub const fn of_layout(layout: Layout) -> Option<Class> {
        Class::of_aligned(layout.size(), layout.align())
    }

    /// The class for a C request: as [`Class::of_layout`], except that a block of [`C_ALIGN`]
    /// bytes or more is [`C_ALIGN`]-aligned, as C requires. So `malloc(24)` gets a 32-byte class
    /// where a Rust `Layout` of size 24 and alignment 8 gets the 24-byte one.
    #[inline(always)]
    pub const fn of_c(layout: Layout) -> Option<Class> {
        let align = if layout.size() >= C_ALIGN && layout.align() < C_ALIGN {
            C_ALIGN
        } else {
            layout.align()
        };
        Class::of_aligned(layout.size(), align)
    }

    /// The class of `size` rounded up to `align`, a power of two, counting zero as one. The
    /// caller guarantees the rounding cannot overflow, as `Layout` does: it bounds the size so
    /// that rounding to its own alignment stays within `isize::MAX`, and `of_c` raises the
    /// alignment by at most 15 bytes' worth.
    #[inline(always)]
    const fn of_aligned(size: usize, align: usize) -> Option<Class> {
        let size = if size == 0 { 1 } else { size };
        let mask = align - 1;
        Class::of_size((size + mask) & !mask)
    }

    /// The slot size in bytes.
    #[inline(always)]
    pub const fn size(self) -> usize {
        let index = self.0.get() as usize;
        if index <= EXACT_COUNT {
            index * QUANTUM
        } else {
            // The inverse of the formula in `of_size`: `index + 3 = 4b + m`, and the largest
            // word count in the class is `(5 + m) << (b - 2)`.
            let k = index + 3;
            let b = k >> 2;
            let m = k & 3;
            ((5 + m) << (b - 2)) * QUANTUM
        }
    }

    /// The slot size as `odd << shift`, with `odd` one of 1, 3, 5 or 7 and `shift` at least 3:
    /// `(odd, shift)`.
    #[inline(always)]
    pub const fn factors(self) -> (usize, u32) {
        let size = self.size();
        let shift = size.trailing_zeros();
        (size >> shift, shift)
    }

    /// The largest power of two that divides the slot size. Every slot of this class is aligned
    /// to it, or to the region size if that is smaller.
    #[inline(always)]
    pub const fn align(self) -> usize {
        1 << self.factors().1
    }

    /// The offset of slot `index` from its region's base: `index * size`, computed as
    /// `(index * odd) << shift` so that the alignment is evident to a reader and to Kani.
    #[inline(always)]
    pub const fn slot_offset(self, index: u32) -> usize {
        let (odd, shift) = self.factors();
        (index as usize * odd) << shift
    }

    /// The class as a small class, if it is one.
    #[inline(always)]
    pub const fn small(self) -> Option<SmallClass> {
        SmallClass::new(self)
    }

    /// Whether the class is small, that is, cached in magazines.
    #[inline(always)]
    pub const fn is_small(self) -> bool {
        self.index() as usize <= SMALL_COUNT
    }

    /// Division by this class's size, for turning offsets into slot indexes.
    #[inline(always)]
    pub const fn divisor(self) -> Divisor {
        Divisor::of(self.size())
    }

    /// The next larger class, if there is one.
    #[inline]
    pub const fn next(self) -> Option<Class> {
        Class::from_index(self.index() + 1)
    }

    /// Every class, smallest first.
    pub fn all() -> impl Iterator<Item = Class> {
        (1..=COUNT as u8).filter_map(Class::from_index)
    }
}

impl fmt::Debug for Class {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Class({}: {} B)", self.index(), self.size())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    /// mimalloc's `MI_PAGE_QUEUES_EMPTY` sizes (init.c) read with an 8-byte word, extended four
    /// per doubling to 4 GiB: an independent statement of the class sizes.
    fn reference_sizes() -> Vec<usize> {
        let mut sizes: Vec<usize> = (1..=8).map(|w| w * 8).collect();
        let mut base = 64;
        while base < MAX_SIZE {
            for quarter in 1..=4 {
                sizes.push(base + quarter * base / 4);
            }
            base *= 2;
        }
        sizes
    }

    #[test]
    fn class_sizes_match_the_reference_list() {
        let reference = reference_sizes();
        assert_eq!(reference.len(), COUNT);
        let sizes: Vec<usize> = Class::all().map(Class::size).collect();
        assert_eq!(sizes, reference);
        assert_eq!(
            &sizes[..13],
            &[8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160]
        );
    }

    #[test]
    fn indexes_round_trip_and_zero_is_not_a_class() {
        assert_eq!(Class::from_index(0), None);
        assert_eq!(Class::from_index(COUNT as u8 + 1), None);
        for class in Class::all() {
            assert_eq!(Class::from_index(class.index()), Some(class));
            assert_eq!(Class::of_size(class.size()), Some(class));
        }
        assert_eq!(core::mem::size_of::<Option<Class>>(), 1);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn every_size_up_to_a_megabyte_gets_the_tightest_class() {
        let mut previous = Class::SMALLEST;
        for size in 0..=(1 << 20) {
            let class = Class::of_size(size).unwrap();
            assert!(class.size() >= size, "size {size} -> {class:?}");
            assert!(class >= previous, "not monotone at {size}");
            if class != Class::SMALLEST {
                let below = Class::from_index(class.index() - 1).unwrap();
                assert!(
                    below.size() < size,
                    "size {size} -> {class:?} but {below:?} fits"
                );
            }
            previous = class;
        }
    }

    #[test]
    fn sizes_beyond_the_largest_class_have_none() {
        assert_eq!(Class::of_size(MAX_SIZE), Some(Class::LARGEST));
        assert_eq!(Class::of_size(MAX_SIZE + 1), None);
        assert_eq!(Class::of_size(usize::MAX), None);
    }

    #[test]
    fn layouts_round_to_their_alignment_first() {
        let layout = |size, align| Layout::from_size_align(size, align).unwrap();
        assert_eq!(Class::of_layout(layout(24, 8)).unwrap().size(), 24);
        assert_eq!(Class::of_layout(layout(1, 1)).unwrap().size(), 8);
        assert_eq!(Class::of_layout(layout(0, 1)).unwrap().size(), 8);
        // A zero size counts as one byte, so the alignment still holds.
        assert_eq!(Class::of_layout(layout(0, 16)).unwrap().size(), 16);
        assert_eq!(Class::of_layout(layout(0, 1 << 32)), Some(Class::LARGEST));
        assert_eq!(Class::of_layout(layout(0, 1 << 33)), None);
        assert_eq!(Class::of_layout(layout(1, 64)).unwrap().size(), 64);
        assert_eq!(Class::of_layout(layout(65, 64)).unwrap().size(), 128);
        assert_eq!(Class::of_layout(layout(100, 4096)).unwrap().size(), 4096);
        assert_eq!(Class::of_layout(layout(1, 1 << 32)), Some(Class::LARGEST));
        assert_eq!(Class::of_layout(layout(1, 1 << 33)), None);
        // 3 << 20 is a class, and a multiple of 1 MiB.
        assert_eq!(
            Class::of_layout(layout(5 << 19, 1 << 20)).unwrap().size(),
            3 << 20
        );
    }

    #[test]
    fn the_c_rule_aligns_blocks_of_sixteen_bytes_or_more() {
        let c = |size| {
            Class::of_c(Layout::from_size_align(size, 1).unwrap())
                .unwrap()
                .size()
        };
        assert_eq!(c(0), 8);
        assert_eq!(c(8), 8);
        assert_eq!(c(9), 16);
        assert_eq!(c(16), 16);
        assert_eq!(c(24), 32);
        assert_eq!(c(40), 48);
        assert_eq!(c(56), 64);
        assert_eq!(c(72), 80);
        for size in 16..=(64 << 10) {
            assert_eq!(c(size) % C_ALIGN, 0, "malloc({size})");
        }
        let aligned = Class::of_c(Layout::from_size_align(24, 64).unwrap()).unwrap();
        assert_eq!(aligned.size(), 64);
        let empty = Class::of_c(Layout::from_size_align(0, 64).unwrap()).unwrap();
        assert_eq!(empty.size(), 64);
    }

    #[test]
    fn the_small_classes_end_at_the_boundary() {
        assert_eq!(SMALL_COUNT, 44);
        assert!(Class::LARGEST_SMALL.is_small());
        assert!(!Class::LARGEST_SMALL.next().unwrap().is_small());
        assert_eq!(Class::all().filter(|c| c.is_small()).count(), SMALL_COUNT);
    }

    #[test]
    fn class_alignment_is_the_largest_power_of_two_dividing_the_size() {
        for class in Class::all() {
            let (odd, shift) = class.factors();
            assert!(matches!(odd, 1 | 3 | 5 | 7) && shift >= 3, "{class:?}");
            assert_eq!(odd << shift, class.size());
            for index in [0u32, 1, 2, 1000, u32::MAX] {
                assert_eq!(class.slot_offset(index), index as usize * class.size());
            }
            let align = class.align();
            assert!(align.is_power_of_two());
            assert_eq!(class.size() % align, 0);
            assert_ne!(class.size() % (2 * align), 0);
        }
        assert_eq!(Class::of_size(24).unwrap().align(), 8);
        assert_eq!(Class::of_size(96).unwrap().align(), 32);
    }

    #[test]
    fn a_constant_layout_folds_to_a_constant_class() {
        const BOX_U64: Class = Class::of_layout(Layout::new::<u64>()).unwrap();
        const SMALL: SmallClass = BOX_U64.small().unwrap();
        const MASK: usize = SMALL.magazine().mask();
        assert_eq!(BOX_U64.size(), 8);
        assert_eq!(MASK, 8 * MagazineSize::MAX.slots() - 16);
    }
}
