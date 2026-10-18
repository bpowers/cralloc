//! The copy, beside the slabs, of what the generated C frees read about the heap: its region
//! shift and base index, and the class byte of each region that holds small blocks
//! (`front::Front`, `docs/research/zen5-roofline.md` section 14). The routines read the copy at a
//! fixed distance from `SLABS` with no lookup of their own; the back end's region table and the
//! published heap word stay the truth, which every slow path uses.
//!
//! The copy is written only here, from the C allocation slow paths, and only ever in one
//! direction: a zero becomes the right value. Every value is all-zero at load, and zero sends
//! every C free to its slow path (a zero shift or base gives an index of the pointer itself,
//! 4096 or more for any heap address; a zero class byte has a zero mask). So the copy may lag
//! the truth, which costs a slow path, but it never disagrees with it in a harmful direction:
//!
//! - The shift and base index are written once, the base index first, from the published heap,
//!   which never changes for the life of the process (a forked child inherits both). A free that
//!   sees the shift sees the base index too (stores are not reordered on x86-64, and the shift's
//!   store is a release); one that sees only the base index computes an index of at least the
//!   pointer's region number above zero minus the base's, far out of range.
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

/// Records that `block`, a small block of `class` just allocated for C, lives in its region.
pub(super) fn note(block: *mut u8, class: SmallClass) {
    let (shift_word, base_word) = SLABS.published();
    let mut shift = shift_word.load(Ordering::Acquire);
    if shift == 0 {
        let Some((base, region_shift)) = process::layout() else {
            return;
        };
        base_word.store((base >> region_shift) as u64, Ordering::Relaxed);
        shift_word.store(u64::from(region_shift), Ordering::Release);
        shift = u64::from(region_shift);
    }
    let base_index = base_word.load(Ordering::Relaxed) as usize;
    let index = (block.addr() >> shift).wrapping_sub(base_index);
    if let Some(entry) = SLABS.regions().get(index) {
        let byte = class.class().index();
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
            class.class().index()
        );
        // SAFETY: a live block, given up once.
        unsafe { crate::c::free(block.cast()) };
    }
}
