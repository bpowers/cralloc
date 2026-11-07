//! The size tables the C entry points read: a request's small class from its size, with C's
//! 16-byte rule built in (design: "Size classes and alignment").
//!
//! The shapes are the ones `docs/research/roofline.md`, "The C entry points", measured as
//! fastest for the generated `c_malloc` and `c_calloc` routines: a byte per 8-byte step of size
//! for `malloc`, and a byte per size for the smaller range `calloc` serves inline. Each byte is
//! the position of the class's hot word in a slab (`Field::index`), from which the routines
//! form the word's address and find its mask; it is not the class index, since hot words sit
//! only at bytes 24 to 31 of each 32-byte chunk (`front::slab`). They are constants built from
//! `Class::of_c`, so they cannot drift from the classes; the tests check every entry against it
//! anyway, from the size side.

use crate::classes::{Class, MAX_SMALL_SIZE, SmallClass};
use crate::front::Field;
use core::alloc::Layout;

/// The largest size `malloc`'s table covers: every small class.
pub(crate) const C_TABLE_MAX: usize = MAX_SMALL_SIZE;

/// The largest size `calloc`'s table covers.
pub(crate) const CALLOC_TABLE_MAX: usize = crate::spec::CALLOC_MAX;

/// The small class of a C request for `size` bytes, for a size with one.
const fn c_class(size: usize) -> SmallClass {
    match Layout::from_size_align(size, 1) {
        Ok(layout) => match Class::of_c(layout) {
            Some(class) => match class.small() {
                Some(small) => small,
                None => panic!("a size in the table has no small class"),
            },
            None => panic!("a size in the table has no class"),
        },
        Err(_) => panic!("a size in the table has no layout"),
    }
}

/// The byte the tables hold for a C request for `size` bytes: the position of its class's hot
/// word in a slab (`Field::index`), from which the C routines form the word's address.
const fn class_index(size: usize) -> u8 {
    word_byte(c_class(size))
}

/// The position of `class`'s hot word in a slab, as a table byte.
pub(crate) const fn word_byte(class: SmallClass) -> u8 {
    let word = class.word();
    assert!(word < 256 && word == Field::hot(class).index());
    word as u8
}

/// The small class whose hot word is at position `byte`, if any: the inverse of [`word_byte`].
#[cfg(test)]
pub(crate) fn class_of_byte(byte: u8) -> Option<SmallClass> {
    SmallClass::all().find(|&class| word_byte(class) == byte)
}

/// `C_SIZE_CLASSES[(size + 7) >> 3]` is the word byte (`word_byte`) of the class of `malloc(size)` for every size up to
/// [`C_TABLE_MAX`]. Entry `i` is the class of `8 * i` bytes, which every size from `8 * i - 7`
/// shares: below 16 bytes a class step is 8 bytes, and from 16 the 16-byte rule rounds a whole
/// 8-byte step to the same multiple of 16. Entry 0 is size 0, which gets the smallest class.
pub(crate) const C_SIZE_CLASSES: [u8; C_TABLE_MAX / 8 + 1] = {
    let mut table = [0u8; C_TABLE_MAX / 8 + 1];
    let mut i = 0;
    while i < table.len() {
        table[i] = class_index(8 * i);
        i += 1;
    }
    table
};

/// `C_CALLOC_CLASSES[size]` is the word byte of the class of a `calloc` of `size` bytes in all, for every
/// size up to [`CALLOC_TABLE_MAX`].
pub(crate) const C_CALLOC_CLASSES: [u8; CALLOC_TABLE_MAX + 1] = {
    let mut table = [0u8; CALLOC_TABLE_MAX + 1];
    let mut size = 0;
    while size < table.len() {
        table[size] = class_index(size);
        size += 1;
    }
    table
};

/// `C_CALLOC_LENS[size]` is the size of the class of a `calloc` of `size` bytes in all, which
/// it zeroes, for every size up to [`CALLOC_TABLE_MAX`].
pub(crate) const C_CALLOC_LENS: [u16; CALLOC_TABLE_MAX + 1] = {
    let mut table = [0u16; CALLOC_TABLE_MAX + 1];
    let mut size = 0;
    while size < table.len() {
        let len = c_class(size).size();
        assert!(len <= u16::MAX as usize);
        table[size] = len as u16;
        size += 1;
    }
    table
};

/// The largest size the sized free's prediction covers by its table index: the low 16 bits of
/// the size.
pub(crate) const C_SIZED_LEN: usize = crate::spec::SIZED_LEN;

/// The sized free's table: entry `n` is the word byte of the class of `malloc(n)` for every size up to
/// [`C_TABLE_MAX`], and 0 (no class) above it: the class a sized free predicts from the low 16
/// bits of its size. A size of 2^16 or more predicts the class of its low 16 bits, which the
/// region's class then refutes, so a wrong prediction costs the slow path and nothing else.
const fn sized_classes() -> [u8; C_SIZED_LEN] {
    let mut table = [0u8; C_SIZED_LEN];
    let mut size = 0;
    while size <= C_TABLE_MAX {
        table[size] = C_SIZE_CLASSES[(size + 7) >> 3];
        size += 1;
    }
    table
}

/// Where a table the C routines read starts within its static: 2048 bytes into a page, so that
/// the bytes of the sizes programs use most have page offsets no slab word has (a load at the page
/// offset of a store in flight waits for it: `docs/research/roofline.md`, "A load at the page
/// offset of a store in flight can stall the whole loop").
pub(crate) const TABLE_OFF: usize = crate::spec::TABLE_OFF;

/// A table of class bytes `TABLE_OFF` bytes into a page-aligned static.
#[doc(hidden)]
#[repr(C, align(4096))]
pub struct PlacedTable<const N: usize> {
    _pad: [u8; TABLE_OFF],
    bytes: [u8; N],
}

impl<const N: usize> PlacedTable<N> {
    const fn new(bytes: [u8; N]) -> PlacedTable<N> {
        PlacedTable {
            _pad: [0; TABLE_OFF],
            bytes,
        }
    }

    /// The table.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn bytes(&self) -> &[u8; N] {
        &self.bytes
    }
}

/// [`C_SIZE_CLASSES`] as `c_malloc` reads it (`C_SIZES` in the Lean specification). Public,
/// hidden and immutable, so that the C routines the export macros define in another crate can
/// name it.
#[doc(hidden)]
pub static C_SIZES: PlacedTable<{ C_TABLE_MAX / 8 + 1 }> = PlacedTable::new(C_SIZE_CLASSES);

/// [`C_CALLOC_CLASSES`] as `c_calloc` reads it, as [`C_SIZES`].
#[doc(hidden)]
pub static C_CALLOC_SIZES: PlacedTable<{ CALLOC_TABLE_MAX + 1 }> =
    PlacedTable::new(C_CALLOC_CLASSES);

/// The sized free's table (`sized_classes`) as `c_free_sized` reads it, as [`C_SIZES`].
#[doc(hidden)]
pub static C_SIZED_SIZES: PlacedTable<C_SIZED_LEN> = PlacedTable::new(sized_classes());

// The tables are where and how long the routines were proved to read them.
const _: () = {
    use crate::spec;
    assert!(C_TABLE_MAX == spec::MALLOC_MAX && C_TABLE_MAX / 8 + 1 == spec::MALLOC_LEN);
    assert!(CALLOC_TABLE_MAX + 1 == spec::CALLOC_LEN);
    assert!(core::mem::offset_of!(PlacedTable<1>, bytes) == TABLE_OFF);
};

/// The small class of `malloc(size)` from the table, or `None` above it.
#[cfg(test)]
pub(crate) fn malloc_class(size: usize) -> Option<crate::classes::SmallClass> {
    if size > C_TABLE_MAX {
        return None;
    }
    class_of_byte(*C_SIZE_CLASSES.get((size + 7) >> 3)?)
}

/// The small class of a `calloc` of `size` bytes in all from its table, or `None` above it.
#[cfg(test)]
pub(crate) fn calloc_class(size: usize) -> Option<crate::classes::SmallClass> {
    class_of_byte(*C_CALLOC_CLASSES.get(size)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn of_c(size: usize) -> Class {
        Class::of_c(Layout::from_size_align(size, 1).unwrap()).unwrap()
    }

    #[test]
    fn the_malloc_table_gives_the_c_class_of_every_small_size() {
        for size in 0..=C_TABLE_MAX {
            let class = malloc_class(size).unwrap();
            assert_eq!(class.class(), of_c(size), "malloc({size})");
            assert!(class.size() >= size);
            if size >= 16 {
                assert_eq!(class.size() % 16, 0, "malloc({size})");
            }
        }
        assert_eq!(malloc_class(C_TABLE_MAX + 1), None);
        assert_eq!(malloc_class(usize::MAX), None);
        assert_eq!(C_SIZE_CLASSES.len(), 4097);
    }

    #[test]
    fn the_calloc_table_gives_the_c_class_of_every_size_it_covers() {
        for size in 0..=CALLOC_TABLE_MAX {
            assert_eq!(calloc_class(size).unwrap().class(), of_c(size), "{size}");
        }
        assert_eq!(calloc_class(CALLOC_TABLE_MAX + 1), None);
        assert_eq!(C_CALLOC_CLASSES.len(), 1025);
        for (size, &len) in C_CALLOC_LENS.iter().enumerate() {
            assert_eq!(usize::from(len), calloc_class(size).unwrap().size());
        }
    }

    #[test]
    fn the_sized_table_predicts_mallocs_class_and_nothing_above_it() {
        for size in 0..C_SIZED_LEN {
            let expected = if size <= C_TABLE_MAX {
                word_byte(of_c(size).small().unwrap())
            } else {
                0
            };
            assert_eq!(C_SIZED_SIZES.bytes()[size], expected, "{size}");
        }
        assert_eq!(&C_SIZES.bytes()[..], &C_SIZE_CLASSES[..]);
        assert_eq!(&C_CALLOC_SIZES.bytes()[..], &C_CALLOC_CLASSES[..]);
        let base = core::ptr::from_ref(&C_SIZES).addr();
        assert_eq!(
            core::ptr::from_ref(C_SIZES.bytes()).addr(),
            base + TABLE_OFF
        );
        assert_eq!(base % 4096, 0);
    }

    #[test]
    fn the_last_small_size_is_the_last_small_class() {
        assert_eq!(
            malloc_class(C_TABLE_MAX).unwrap().class(),
            Class::LARGEST_SMALL
        );
        assert_eq!(malloc_class(0).unwrap().class(), Class::SMALLEST);
        assert_eq!(malloc_class(24).unwrap().size(), 32);
    }
}
