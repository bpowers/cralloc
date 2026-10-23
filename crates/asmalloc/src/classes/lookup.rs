//! The tables the inlined fast paths read when a class is known only at run time: the small
//! class of every rounded size, each class's magazine mask, and each class's slot size.
//!
//! They share one page-aligned static so that one address serves every load (an inlined call
//! site that is not in a loop otherwise forms the address of each table with an instruction of
//! its own), and they sit at page offsets no slab hot word has: an inlined allocation or free
//! loads its entries right after the previous operation stored a hot word, and on Zen 5 a load
//! whose page offset overlaps a store still in flight can be held until that store is done;
//! since the store's address comes from the previous lookup, a loop of them then runs one at a
//! time (`docs/research/zen5-roofline.md`, section 16). `front` asserts the placement for every
//! class.
//!
//! The mask stays a table entry handed to the routine in a register, not something the routine
//! loads: a mask that comes from memory costs a loop of constant-size calls up to a cycle a pair,
//! where a constant class folds its entry to an immediate (`docs/research/fast-lookup.md`).
//! Each entry is a constant built from the class functions, so a constant layout folds every
//! load away.

use super::{Class, MAX_SMALL_SIZE, QUANTUM, SMALL_COUNT, SmallClass, WORD_STRIDE};

/// Where the masks lie within their page: after the slot sizes, in the static's second page, at
/// page offsets past every field of a slab (`front` asserts it).
pub(crate) const MASKS_PAGE_OFFSET: usize = LENGTHS_PAGE_OFFSET + 2 * LENGTH_ENTRIES;

/// Entries in the masks' table: one per value of a word position, so that indexing it by a
/// class's position needs no bounds check.
const MASK_ENTRIES: usize = 256;

/// Where the class bytes start within their page. From here the entries of every size up to
/// [`BY_SIZE_CLEAR`] bytes lie past every hot word's page offset, and only larger sizes wrap
/// round onto them.
pub(crate) const BY_SIZE_PAGE_OFFSET: usize = 2048;

/// The largest size whose class byte lies between [`BY_SIZE_PAGE_OFFSET`] and the end of its
/// page.
pub(crate) const BY_SIZE_CLEAR: usize = (4096 - BY_SIZE_PAGE_OFFSET) * QUANTUM;

/// Entries in the slot sizes' table: one per value of `word / WORD_STRIDE`, so that indexing it by a
/// class's index needs no bounds check.
const LENGTH_ENTRIES: usize = 64;

/// Where the slot sizes lie within their page: right after the class bytes, which end 2048
/// bytes into the static's second page.
pub(crate) const LENGTHS_PAGE_OFFSET: usize =
    (BY_SIZE_PAGE_OFFSET + MAX_SMALL_SIZE / QUANTUM) % 4096;

#[repr(C, align(4096))]
struct Lookup {
    _pad: [u8; BY_SIZE_PAGE_OFFSET],
    /// By `(size - 1) / QUANTUM`: entry `j` is the class of `QUANTUM * (j + 1)` bytes, which
    /// serves every size from `QUANTUM * j + 1` up to that.
    classes: [SmallClass; MAX_SMALL_SIZE / QUANTUM],
    /// By class index: the slot size, zero for every index that is not a small class. For the
    /// `Allocator` interface, which returns it.
    lengths: [u16; LENGTH_ENTRIES],
    /// By word position (`SmallClass::word`): the class's magazine mask, zero for every other
    /// position. Every mask fits 16 bits (`verify.rs`), and half the width puts twice the classes
    /// on a line.
    masks: [u16; MASK_ENTRIES],
}

const _: () = {
    use core::mem::offset_of;
    assert!(align_of::<Lookup>() == 4096);
    assert!(offset_of!(Lookup, masks) % 4096 == MASKS_PAGE_OFFSET);
    assert!(offset_of!(Lookup, classes) == BY_SIZE_PAGE_OFFSET);
    assert!(offset_of!(Lookup, lengths) % 4096 == LENGTHS_PAGE_OFFSET);
    assert!(MASKS_PAGE_OFFSET + 2 * MASK_ENTRIES <= 4096);
    assert!(WORD_LIMIT <= MASK_ENTRIES && WORD_LIMIT / WORD_STRIDE <= LENGTH_ENTRIES);
    assert!(MAX_SMALL_SIZE <= u16::MAX as usize);
};

/// Each small class's magazine mask at its word position, zero at every other: what the
/// inlined routines are handed (from [`LOOKUP`]) and what the C routines read from their copy
/// past the slabs (`front::publish_masks`).
pub(crate) const MASKS_BY_WORD: [u16; MASK_ENTRIES] = {
    let mut masks = [0; MASK_ENTRIES];
    let mut ordinal = 0;
    while ordinal < SMALL_COUNT {
        if let Some(class) = SmallClass::from_ordinal(ordinal) {
            let mask = class.magazine().mask();
            assert!(mask <= u16::MAX as usize);
            masks[class.word()] = mask as u16;
        }
        ordinal += 1;
    }
    masks
};

/// One more than the largest value a word position can take.
const WORD_LIMIT: usize = u8::MAX as usize + 1;

static LOOKUP: Lookup = Lookup {
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
    lengths: {
        let mut lengths = [0; LENGTH_ENTRIES];
        let mut ordinal = 0;
        while ordinal < SMALL_COUNT {
            if let Some(class) = SmallClass::from_ordinal(ordinal) {
                lengths[class.class().index() as usize] = class.size() as u16;
            }
            ordinal += 1;
        }
        lengths
    },
    masks: MASKS_BY_WORD,
};

/// The small class of a rounded size less one, `less_one < MAX_SMALL_SIZE`.
#[inline(always)]
pub(super) fn class_of(less_one: usize) -> SmallClass {
    LOOKUP.classes[less_one / QUANTUM]
}

/// The magazine mask of `class`, from the table.
#[inline(always)]
pub(super) fn mask(class: SmallClass) -> u16 {
    LOOKUP.masks[class.word()]
}

/// The slot size of `class`, from the table.
#[inline(always)]
pub(super) fn length(class: SmallClass) -> u16 {
    LOOKUP.lengths[class.word() / WORD_STRIDE]
}

/// The table entry of word position `position`, for the proofs.
#[cfg(kani)]
pub(super) fn mask_entry(position: u8) -> u16 {
    LOOKUP.masks[position as usize]
}

/// The table entry of class index `index` (by its word position's upper bits), for the proofs.
#[cfg(kani)]
pub(super) fn length_entry(index: usize) -> u16 {
    LOOKUP.lengths[index % LENGTH_ENTRIES]
}

/// The addresses of the entries an inlined call reads, for the placement tests.
#[cfg(test)]
pub(crate) mod addresses {
    use super::*;

    pub(crate) fn class_byte(size: usize) -> usize {
        core::ptr::from_ref(&LOOKUP.classes[(size - 1) / QUANTUM]).addr()
    }

    pub(crate) fn mask(class: SmallClass) -> usize {
        core::ptr::from_ref(&LOOKUP.masks[class.word()]).addr()
    }

    pub(crate) fn length(class: SmallClass) -> usize {
        core::ptr::from_ref(&LOOKUP.lengths[class.word() / WORD_STRIDE]).addr()
    }

    pub(crate) fn every_mask() -> &'static [u16] {
        &LOOKUP.masks
    }

    pub(crate) fn every_length() -> &'static [u16] {
        &LOOKUP.lengths
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_masks_and_lengths_are_each_classs_and_zero_elsewhere() {
        for class in SmallClass::all() {
            assert_eq!(class.mask(), class.magazine().mask(), "{class:?}");
            assert_eq!(class.length(), class.size(), "{class:?}");
        }
        let classes = SmallClass::all().count();
        let masks = addresses::every_mask().iter().filter(|&&m| m != 0).count();
        let lengths = addresses::every_length()
            .iter()
            .filter(|&&n| n != 0)
            .count();
        assert_eq!((masks, lengths), (classes, classes));
    }

    #[test]
    fn the_tables_share_one_page_aligned_static() {
        let base = core::ptr::from_ref(&LOOKUP).addr();
        assert_eq!(base % 4096, 0);
        let first = SmallClass::all().next().unwrap();
        assert_eq!(
            addresses::mask(first),
            base + 4096 + MASKS_PAGE_OFFSET + 2 * first.word()
        );
        assert_eq!(addresses::class_byte(1), base + BY_SIZE_PAGE_OFFSET);
        assert_eq!(addresses::length(first) % 4096, LENGTHS_PAGE_OFFSET + 2);
    }
}
