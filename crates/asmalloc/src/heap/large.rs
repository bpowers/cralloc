//! Large objects, and resizing any object (design: "Large objects").
//!
//! A large object has no magazine. It is a slot of its class taken straight from the region
//! bitmaps, or, when its class does not fit in a region (or its alignment is larger than a region),
//! a run of whole regions. Its class always comes from its address, because it may sit in a slot
//! bigger than its layout says.
//!
//! A resize stays in place only when the caller's next free will still find the right class: for a
//! small object, when the new layout has the same class; for a large one, when the new layout is
//! still large and fits the slot or run, and, when it shrinks (a decision beyond the design), keeps
//! at least a [`HEADROOM`]th of it, so that a big buffer shrunk to a sliver does not keep its
//! pages. Anything else moves the object. An object that outgrows its block moves to one with
//! [`HEADROOM`] times the room it asked for, so a buffer that doubles is copied every other
//! doubling, not at each one; but not past a region, since a run commits all its memory at once.
//! Small objects get no headroom: their class comes from the layout, so their slot must be exactly
//! their class.
//!
//! Fresh slots and runs read as zero; the back end says when a reused large slot is known to, so
//! zeroed allocation clears only what it must.
//!
//! Every allocation and in-place resize of a large object records the size asked for
//! (`Back::alloc`, `Back::alloc_run`, `Back::resize`). What the caller may use is that size rounded
//! up to a page, so a block with headroom never reports, or makes anyone touch, the room it has not
//! been asked for.

use super::{Grant, Heap, Zero, small};
use crate::back::Kind;
use crate::classes::Class;
use crate::front::Routines;
use crate::os::Os;
use crate::space::Plan;
use core::alloc::Layout;
use core::ptr::NonNull;

/// How many times the room it asked for an object gets when it outgrows its block.
pub(crate) const HEADROOM: usize = 4;

/// Where a large object goes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Place {
    /// A slot of this class, which fits in a region.
    Slot(Class),
    /// A run of whole regions holding `size` bytes, aligned to `align`.
    Run { size: usize, align: usize },
}

/// Where an object with a layout that has no small class goes in a heap laid out as `plan`.
fn place(layout: Layout, plan: Plan) -> Place {
    match Class::of_layout(layout) {
        Some(class) if plan.fits_region(class) => Place::Slot(class),
        // `Layout` keeps the size rounded up to its alignment within `isize::MAX`.
        _ => Place::Run {
            size: layout.size().max(1).next_multiple_of(layout.align()),
            align: layout.align(),
        },
    }
}

/// The layout with headroom for an object that grows to `layout`, if it can be described.
fn with_headroom(layout: Layout) -> Option<Layout> {
    let size = layout.size().checked_mul(HEADROOM)?;
    Layout::from_size_align(size, layout.align()).ok()
}

impl<O: Os> Heap<O> {
    /// Allocates a large block for `layout`, with the bytes `zero` names zeroed. `None` if there
    /// is no memory.
    #[cold]
    #[inline(never)]
    pub(crate) fn alloc_large(&self, layout: Layout, zero: Zero) -> Option<Grant> {
        let back = self.start()?;
        match place(layout, back.space().plan()) {
            Place::Slot(class) => {
                let block = back.alloc(class, layout.size())?;
                let len = self
                    .usable_size(block.ptr.as_ptr())
                    .unwrap_or(layout.size());
                let clear = match zero {
                    _ if block.zeroed => 0,
                    Zero::No => 0,
                    Zero::Requested => layout.size(),
                    Zero::Whole => len,
                };
                // SAFETY: HEAP-03.
                unsafe { block.ptr.as_ptr().write_bytes(0, clear) };
                Some(Grant {
                    ptr: block.ptr,
                    len,
                })
            }
            // A run is always fresh or purged memory, so it reads as zero.
            Place::Run { size, align } => {
                let ptr = back.alloc_run(size, align)?;
                let len = self.usable_size(ptr.as_ptr()).unwrap_or(layout.size());
                Some(Grant { ptr, len })
            }
        }
    }

    /// Frees a large block, finding its class or run from its address.
    #[cold]
    #[inline(never)]
    pub(crate) fn free_large(&self, ptr: NonNull<u8>) {
        match self.back() {
            Some(back) => back.free_by_address(ptr),
            None => self.os().abort("free of a block when no heap exists"),
        }
    }

    /// The bytes the block at `ptr`, described by `layout`, occupies: its class for a small
    /// layout, else its whole slot or run. Aborts if no large block starts there.
    fn block_len(&self, ptr: NonNull<u8>, layout: Layout) -> usize {
        if let Some(class) = small(layout) {
            return class.size();
        }
        let whole = self
            .back()
            .and_then(|back| match back.lookup(ptr.as_ptr())? {
                Kind::Slot(class) => Some(class.size()),
                Kind::Run(regions) => Some(regions << back.space().plan().region_shift()),
            });
        match whole {
            Some(len) => len,
            None => self
                .os()
                .abort("resize of a pointer that is not a block of the heap"),
        }
    }

    /// Resizes the block at `ptr` from `old` to `new`, in place when the next free will still
    /// find its class, else by moving it. With `zero`, the bytes of the result past the old
    /// block read as zero. `None` if a move found no memory; the old block is then untouched.
    ///
    /// # Safety
    ///
    /// As for [`Heap::dealloc`] with `ptr` and `old`; `new` has a non-zero size.
    pub(crate) unsafe fn resize<R: Routines + Copy>(
        &self,
        routines: R,
        ptr: NonNull<u8>,
        old: Layout,
        new: Layout,
        zero: bool,
    ) -> Option<Grant> {
        let len = self.block_len(ptr, old);
        let in_place = match (small(old), Class::of_layout(new)) {
            (Some(class), Some(next)) => next == class.class(),
            (Some(_), None) => false,
            // Still large, fitting the block, and aligned as the new layout asks: the next free
            // reads the block from the address and finds it whole.
            // A shrink to less than a [`HEADROOM`]th of the block moves, so the pages it gives
            // up go back with the old block instead of staying resident in a block that is
            // mostly empty.
            (None, next) => {
                let needed = new.size().next_multiple_of(new.align());
                next.is_none_or(|c| !c.is_small())
                    && needed <= len
                    && (new.size() >= old.size() || needed >= len / HEADROOM)
                    && ptr.addr().get() & (new.align() - 1) == 0
            }
        };
        if in_place {
            if small(old).is_some() {
                return Some(Grant { ptr, len });
            }
            return Some(self.resize_large_in_place(ptr, new, zero));
        }
        // Growing a large object out of its block: take headroom. A zeroed grow takes none, so
        // that it never clears room nobody asked for.
        // Not for a run: it commits all its regions at once, so headroom would be commit charge,
        // not just address space.
        let region = self
            .back()
            .map_or(0, |back| back.space().plan().region_size());
        let roomy = (!zero && new.size() > len && small(new).is_none())
            .then(|| with_headroom(new))
            .flatten()
            .filter(|layout| layout.size() <= region);
        let fill = if zero { Zero::Whole } else { Zero::No };
        // SAFETY: passed through from the caller.
        let grant = unsafe {
            roomy
                .and_then(|layout| self.alloc_grant(routines, layout, Zero::No))
                .or_else(|| self.alloc_grant(routines, new, fill))?
        };
        // A block with headroom records the size asked for, not the room it was given.
        let grant = match self.back() {
            Some(back) if small(new).is_none() => {
                back.resize(grant.ptr, new.size());
                let len = self.usable_size(grant.ptr.as_ptr()).unwrap_or(new.size());
                Grant { len, ..grant }
            }
            _ => grant,
        };
        let kept = old.size().min(new.size());
        // SAFETY: HEAP-04.
        unsafe {
            match self.back() {
                // A large block is copied in pieces, its old pages returned behind the copy,
                // and freed, all by the back end.
                Some(back) if small(old).is_none() => back.move_large(ptr, grant.ptr, kept),
                _ => {
                    ptr.as_ptr()
                        .copy_to_nonoverlapping(grant.ptr.as_ptr(), kept);
                    self.dealloc(routines, ptr.as_ptr(), old);
                }
            }
        }
        Some(grant)
    }

    /// Records a large block's new size after an in-place resize, clearing what it gains if
    /// `zero`, and returns what the caller may use now. A block that shrinks gives the whole
    /// pages past its new usable size back to the OS at once (`Back::resize_in_place`), so
    /// that a buffer cut down by `realloc` does not keep its old pages until it is freed.
    fn resize_large_in_place(&self, ptr: NonNull<u8>, new: Layout, zero: bool) -> Grant {
        let before = self.usable_size(ptr.as_ptr()).unwrap_or(0);
        if let Some(back) = self.back() {
            back.resize_in_place(ptr, new.size());
        }
        let len = self.usable_size(ptr.as_ptr()).unwrap_or(new.size());
        if zero && len > before {
            // SAFETY: HEAP-05.
            unsafe { ptr.as_ptr().add(before).write_bytes(0, len - before) };
        }
        Grant { ptr, len }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::space::Geometry;

    fn layout(size: usize, align: usize) -> Layout {
        Layout::from_size_align(size, align).unwrap()
    }

    #[test]
    fn large_layouts_take_slots_until_they_outgrow_a_region() {
        let plan = Geometry::DESIGN.largest_plan();
        let big = Class::of_size(40 << 10).unwrap();
        assert_eq!(place(layout(40 << 10, 8), plan), Place::Slot(big));
        assert_eq!(place(layout(1 << 32, 8), plan), Place::Slot(Class::LARGEST));
        assert_eq!(
            place(layout((1 << 32) + 1, 8), plan),
            Place::Run {
                size: (1 << 32) + 8,
                align: 8
            }
        );
        // An alignment above the region size is a run aligned to it.
        assert_eq!(
            place(layout(8, 1 << 33), plan),
            Place::Run {
                size: 1 << 33,
                align: 1 << 33
            }
        );
    }

    #[test]
    fn headroom_multiplies_the_size_and_keeps_the_alignment() {
        assert_eq!(with_headroom(layout(100, 16)), Some(layout(400, 16)));
        assert_eq!(with_headroom(layout(usize::MAX / 2, 1)), None);
    }
}
