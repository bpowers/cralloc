//! The copies, beside the slabs, of what the generated C frees read about the heap: its region
//! shift and base index, and the class byte of
//! each region that holds small blocks (`front::Front`, `docs/research/roofline.md`, "The C
//! entry points"). The routines read them at fixed distances from `SLABS` with no lookup of
//! their own; the back end's region table and the published heap word stay the truth, which
//! every slow path uses.
//!
//! The copies are written only here, and only ever in one direction: a zero becomes the right
//! value. Every value is all-zero at load, and zero sends every free that reads it to its slow
//! path (a zero class byte has a zero mask, and a zero shift or base is below). So a copy may lag
//! the truth, which costs a slow path, but it never disagrees with it in a harmful direction.
//!
//! A routine's loads have no order of their own, so a free may read any of these words old and any
//! other new, on a weakly ordered CPU for as long as a stale read lasts. Every mix has to be safe,
//! and is:
//!
//! - The shift and base index are written once, from the published heap, which never changes for
//!   the life of the process (a forked child inherits both). A free computes `(ptr >> shift) -
//!   base index`, and takes the slow path if that is not below [`REGION_COUNT`]. For a pointer of
//!   the heap, whose base index is at least [`REGION_COUNT`] or it is not mirrored at all
//!   ([`words_for`]): both old gives the pointer, a heap address; the shift new and the base index
//!   old gives its region number counted from address zero, at least the base index; the shift old
//!   and the base index new gives a heap address less a small number. All are out of range. Both
//!   new is right. Each try of a sequence reads each word once, and a restart begins again with
//!   nothing kept, so each try is one of these mixes. In `publish` the base index's store comes
//!   first and the shift's is a release, so that a slow path that finds the shift has the base
//!   index; a routine relies on none of that, on x86-64 or aarch64.
//! - They are written from the C allocation slow paths (`note`).
//! - A region's byte is written only when a small block of that region has just been handed out
//!   to C, so the region is assigned to that class and has blocks. The back end never takes a
//!   region holding blocks from its class (it unassigns a region only when undoing an assignment
//!   no block was ever carved from), so the class the copy names is the region's for as long as
//!   any block of it can be freed. A region never named here reads zero.
//!
//! Each write is one relaxed or release store of a value every writer agrees on, so racing
//! writers are harmless and no write needs a lock.

use crate::classes::SmallClass;
use crate::front::{REGION_COUNT, SLABS};
use crate::heap::process;
use core::sync::atomic::Ordering;

/// Publishes the heap's shift and base index beside the slabs, if the heap has started, and
/// returns them as `(shift, base index)`. Idempotent.
fn publish() -> Option<(u64, usize)> {
    let (shift_word, base_word) = SLABS.published();
    let shift = shift_word.load(Ordering::Acquire);
    if shift != 0 {
        return Some((shift, base_word.load(Ordering::Relaxed) as usize));
    }
    let (base, region_shift) = process::layout()?;
    let (shift, base_index) = words_for(base, region_shift)?;
    base_word.store(base_index as u64, Ordering::Relaxed);
    shift_word.store(shift, Ordering::Release);
    Some((shift, base_index))
}

/// The shift and base index to publish for a heap at `base`, or `None` if it is so low that its base
/// index is in range for the table, as a small reservation may be on a small address space: a free
/// that read the shift and not yet the base index would take the pointer's number from zero for
/// an index, and name another region's class. Such a heap is not mirrored, and its C frees take the
/// slow path.
fn words_for(base: usize, region_shift: u32) -> Option<(u64, usize)> {
    let base_index = base >> region_shift;
    (base_index >= REGION_COUNT).then_some((u64::from(region_shift), base_index))
}

/// Records that `block`, a small block of `class` just allocated for C, lives in its region.
pub(super) fn note(block: *mut u8, class: SmallClass) {
    let Some((shift, base_index)) = publish() else {
        return;
    };
    let index = (block.addr() >> shift).wrapping_sub(base_index);
    if let Some(entry) = SLABS.regions().get(index) {
        let byte = super::table::word_byte(class);
        if entry.load(Ordering::Relaxed) != byte {
            entry.store(byte, Ordering::Relaxed);
        }
    }
    debug_assert!(
        index < REGION_COUNT,
        "a heap block outside the region table"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::heap::{Found, process};

    /// The index a generated C free works out from what it read of the two words.
    fn index(ptr: usize, shift: u64, base_index: usize) -> usize {
        (ptr >> shift).wrapping_sub(base_index)
    }

    #[test]
    fn a_heap_whose_base_index_is_in_range_is_not_mirrored() {
        for shift in [20, 26, 30] {
            let at = |base_index: usize| base_index << shift;
            assert_eq!(words_for(0, shift), None);
            assert_eq!(words_for(at(1), shift), None);
            assert_eq!(words_for(at(REGION_COUNT - 1), shift), None);
            assert_eq!(words_for(at(REGION_COUNT) - 1, shift), None);
            assert_eq!(
                words_for(at(REGION_COUNT), shift),
                Some((u64::from(shift), REGION_COUNT))
            );
        }
    }

    /// Whatever a free reads of the two words, for any pointer of a heap that is mirrored, it gets
    /// the right index or one out of range, and never another region's.
    #[test]
    fn every_mix_of_the_words_is_right_or_out_of_range() {
        for region_shift in 20..=36 {
            let lowest = REGION_COUNT << region_shift;
            let high = lowest.max(0x7f00_0000_0000);
            for base in [lowest, lowest + (1 << region_shift), high] {
                // A reservation starts on a region.
                let base = base & !((1usize << region_shift) - 1);
                let Some((shift, base_index)) = words_for(base, region_shift) else {
                    panic!("a heap at {base:#x} is high enough");
                };
                for region in [0, 1, REGION_COUNT / 2, REGION_COUNT - 1] {
                    for inside in [0, 8, (1usize << region_shift) - 8] {
                        let ptr = base + (region << region_shift) + inside;
                        assert!(index(ptr, 0, 0) >= REGION_COUNT, "both old");
                        assert!(index(ptr, shift, 0) >= REGION_COUNT, "the shift new");
                        assert!(index(ptr, 0, base_index) >= REGION_COUNT, "the base new");
                        assert_eq!(index(ptr, shift, base_index), region, "both new");
                    }
                }
            }
        }
    }

    #[test]
    fn a_noted_block_names_its_class_where_the_c_free_reads_it() {
        let block = crate::c::malloc(48).cast::<u8>();
        let Some(Found::Small(class)) = process::find(block) else {
            panic!("a small block");
        };
        note(block, class);
        let (shift, base) = SLABS.published();
        let (heap_base, region_shift) = process::layout().unwrap();
        assert_eq!(shift.load(Ordering::Relaxed), u64::from(region_shift));
        assert_eq!(
            base.load(Ordering::Relaxed) as usize,
            heap_base >> region_shift
        );
        let index = (block.addr() >> region_shift) - (heap_base >> region_shift);
        assert_eq!(
            SLABS.regions()[index].load(Ordering::Relaxed),
            crate::c::table::word_byte(class)
        );
        // SAFETY: a live block, given up once.
        unsafe { crate::c::free(block.cast()) };
    }
}
