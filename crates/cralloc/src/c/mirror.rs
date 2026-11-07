//! The copies, beside the slabs, of what the generated C frees read about the heap: its region
//! shift and base index, and the class byte of
//! each region that holds small blocks (`front::Front`, `docs/research/roofline.md`, "The C
//! entry points"). The routines read them at fixed distances from `SLABS` with no lookup of
//! their own; the back end's region table and the published heap word stay the truth, which
//! every slow path uses.
//!
//! The copies are written only here, and only ever in one direction: a zero becomes the right
//! value. Every value is all-zero at load, and zero sends every free that reads it to its slow
//! path (a zero shift or base gives an index of the pointer itself, 4096 or more for any heap
//! address; a zero class byte has a zero mask). So a copy may lag the truth, which costs a slow
//! path, but it never disagrees with it in a harmful direction:
//!
//! - The shift and base index are written once, the base index first, from the published
//!   heap, which never changes for the life of the process (a forked child inherits both). A free
//!   that sees the shift sees the base index too (stores are not reordered on x86-64, and the
//!   shift's store is a release); one that sees only the base index computes an index of at
//!   least the pointer's region number above zero minus the base's, far out of range.
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
    let base_index = base >> region_shift;
    base_word.store(base_index as u64, Ordering::Relaxed);
    shift_word.store(u64::from(region_shift), Ordering::Release);
    Some((u64::from(region_shift), base_index))
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
