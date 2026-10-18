//! Large objects, and resizing any object (design: "Large objects").
//!
//! A large object has no magazine. It is a slot of its class taken straight from the region
//! bitmaps, or, when its class does not fit in a region (or its alignment is larger than a region),
//! a run of whole regions. Its class always comes from its address, because it may sit in a slot
//! bigger than its layout says.
//!
//! A resize stays in place only when the caller's next free will still find the right class: for a
//! small object, when the new layout has the same class; for a large one, when the new layout is
//! still large and fits the slot or run, and, when it shrinks, keeps at least a
//! [`SHRINK_MOVES_BELOW`]th of it, so that a big buffer shrunk to a sliver does not hold its
//! slot. Anything else moves the object. An object that outgrows its block moves to one with
//! [`HEADROOM`] times the room it asked for, so a buffer that doubles is copied every other
//! doubling, not at each one; but not past a region, since a run commits all its memory at once.
//! Small objects get no headroom: their class comes from the layout, so their slot must be exactly
//! their class.
//!
//! A large allocation or free is a slow path of its own and runs a heartbeat
//! ([`Heap::heartbeat`]), so a program that allocates only large blocks still keeps the caches,
//! the depot and the back end maintained.
//!
//! Fresh slots and runs read as zero; the back end says when a reused large slot is known to, so
//! zeroed allocation clears only what it must.
//!
//! Every allocation and in-place resize of a large object records the size asked for
//! (`Back::alloc`, `Back::alloc_run`, `Back::resize`). What the caller may use is that size rounded
//! up to a page, so a block with headroom never reports, or makes anyone touch, the room it has not
//! been asked for.

use super::{Grant, Heap, Zero, small};
use crate::back::policy::{STAGGER_ROOM, STAGGER_STEP};
use crate::classes::Class;
use crate::front::Routines;
use crate::os::Os;
use crate::space::Plan;
use core::alloc::Layout;
use core::ptr::NonNull;

/// How many times the room it asked for an object gets when it outgrows its block: it then
/// fills between a fifth and a quarter of its new slot (a class is less than a quarter above
/// the size it was chosen for).
pub(crate) const HEADROOM: usize = 4;

/// A large object that shrinks to under this fraction of its slot or run moves: a region has
/// few slots of a large class, and one should not be held by an object a small fraction of its
/// size.
pub(crate) const SHRINK_MOVES_BELOW: usize = 8;

// The two rules must leave a gap: a block that moved in with headroom and then shrinks back to
// the size it had before must stay, or a block resized back and forth across a class boundary
// is copied at every call. It fills more than 1 / (HEADROOM * 5 / 4) of its slot.
const _: () = assert!(4 * SHRINK_MOVES_BELOW > 5 * HEADROOM);

/// Whether a large block of `len` bytes stays in place when it shrinks to `size` bytes.
pub(crate) const fn stays_after_shrink(size: usize, len: usize) -> bool {
    size >= len / SHRINK_MOVES_BELOW
}

/// Where a large object goes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Place {
    /// A slot of this class, which fits in a region.
    Slot(Class),
    /// A run of whole regions holding `size` bytes, aligned to `align`.
    Run { size: usize, align: usize },
}

/// Where an object with a layout that has no small class goes in a heap laid out as `plan`.
/// A layout that may be staggered (`policy::STAGGER_STEP`) gets a slot with
/// `policy::STAGGER_ROOM` to spare, so that its stagger always fits, unless only a slot
/// without the room fits a region.
fn place(layout: Layout, plan: Plan) -> Place {
    let slot = |layout: Layout| Class::of_layout(layout).filter(|&c| plan.fits_region(c));
    let roomy = Layout::from_size_align(layout.size().saturating_add(STAGGER_ROOM), layout.align())
        .ok()
        .filter(|_| layout.align() <= STAGGER_STEP)
        .and_then(slot);
    match roomy.or_else(|| slot(layout)) {
        Some(class) => Place::Slot(class),
        // The size asked for, which the run records: the alignment is met by where the run
        // starts, so rounding the size up to it would only report, and commit, room nobody
        // asked for.
        _ => Place::Run {
            size: layout.size().max(1),
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
                let block = if layout.align() <= STAGGER_STEP {
                    back.alloc_staggered(class, layout.size())?
                } else {
                    back.alloc(class, layout.size())?
                };
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
                    zeroed: block.zeroed || zero == Zero::Whole,
                })
            }
            // A run is always fresh or purged memory, so it reads as zero.
            Place::Run { size, align } => {
                let ptr = back.alloc_run(size, align)?;
                let len = self.usable_size(ptr.as_ptr()).unwrap_or(layout.size());
                Some(Grant {
                    ptr,
                    len,
                    zeroed: true,
                })
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

    /// [`Heap::alloc_large`] as the slow path it counts as: the block, then a heartbeat.
    ///
    /// # Safety
    ///
    /// As for [`Heap::alloc`].
    pub(crate) unsafe fn alloc_large_slow<R: Routines + Copy>(
        &self,
        routines: R,
        layout: Layout,
        zero: Zero,
    ) -> Option<Grant> {
        let grant = self.alloc_large(layout, zero);
        // SAFETY: passed through from the caller.
        unsafe { self.heartbeat(routines) };
        grant
    }

    /// [`Heap::free_large`] as the slow path it counts as: the free, then a heartbeat.
    ///
    /// # Safety
    ///
    /// As for [`Heap::alloc`].
    pub(crate) unsafe fn free_large_slow<R: Routines + Copy>(&self, routines: R, ptr: NonNull<u8>) {
        self.free_large(ptr);
        // SAFETY: passed through from the caller.
        unsafe { self.heartbeat(routines) };
    }

    /// The bytes the block at `ptr`, described by `layout`, occupies: its class for a small
    /// layout, else its whole slot or run. Aborts if no large block starts there.
    fn block_len(&self, ptr: NonNull<u8>, layout: Layout) -> usize {
        if let Some(class) = small(layout) {
            return class.size();
        }
        let whole = self.back().and_then(|back| back.room(ptr.as_ptr()));
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
            // A shrink to less than a [`SHRINK_MOVES_BELOW`]th of the block moves.
            (None, next) => {
                let needed = new.size().next_multiple_of(new.align());
                next.is_none_or(|c| !c.is_small())
                    && needed <= len
                    && (new.size() >= old.size() || stays_after_shrink(needed, len))
                    && ptr.addr().get() & (new.align() - 1) == 0
            }
        };
        if in_place {
            if small(old).is_some() {
                return Some(Grant {
                    ptr,
                    len,
                    zeroed: false,
                });
            }
            // SAFETY: the caller's promise for `ptr`, and `new` fits the block (`in_place`).
            return Some(unsafe { self.resize_large_in_place(ptr, new, zero) });
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
        // A zeroed grow clears only what lies past the bytes it keeps, and only when the new
        // block is not known to read as zero.
        // SAFETY: passed through from the caller.
        let grant = unsafe {
            roomy
                .and_then(|layout| self.alloc_grant(routines, layout, Zero::No))
                .or_else(|| self.alloc_grant(routines, new, Zero::No))?
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
                Some(back) if small(old).is_none() => {
                    back.move_large(ptr, grant.ptr, kept, grant.zeroed)
                }
                _ => {
                    ptr.as_ptr()
                        .copy_to_nonoverlapping(grant.ptr.as_ptr(), kept);
                    self.dealloc(routines, ptr.as_ptr(), old);
                }
            }
            if zero && !grant.zeroed && grant.len > kept {
                grant
                    .ptr
                    .as_ptr()
                    .add(kept)
                    .write_bytes(0, grant.len - kept);
            }
        }
        Some(grant)
    }

    /// Records a large block's new size after an in-place resize, clearing what it gains if
    /// `zero`, and returns what the caller may use now. A block that shrinks gives the whole
    /// pages past its new usable size back to the OS at once (`Back::resize_in_place`), so
    /// that a buffer cut down by `realloc` does not keep its old pages until it is freed.
    ///
    /// # Safety
    ///
    /// `ptr` is a live large block of this heap that the caller owns and is resizing to `new`,
    /// which fits it.
    unsafe fn resize_large_in_place(&self, ptr: NonNull<u8>, new: Layout, zero: bool) -> Grant {
        let before = self.usable_size(ptr.as_ptr()).unwrap_or(0);
        if let Some(back) = self.back() {
            // SAFETY: the caller's promise; a resize gives up what lies past the new size.
            unsafe { back.resize_in_place(ptr, new.size()) };
        }
        let len = self.usable_size(ptr.as_ptr()).unwrap_or(new.size());
        if zero && len > before {
            // SAFETY: HEAP-05.
            unsafe { ptr.as_ptr().add(before).write_bytes(0, len - before) };
        }
        Grant {
            ptr,
            len,
            zeroed: false,
        }
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
        // Room for a stagger: a 40 KiB request takes the class that holds 44 KiB, and one too
        // aligned to be staggered takes its own.
        let roomy = Class::of_size((40 << 10) + STAGGER_ROOM).unwrap();
        assert_eq!(place(layout(40 << 10, 8), plan), Place::Slot(roomy));
        let exact = Class::of_size(40 << 10).unwrap();
        assert_eq!(place(layout(40 << 10, 8192), plan), Place::Slot(exact));
        assert_eq!(
            place(layout(1 << 32, 8192), plan),
            Place::Slot(Class::LARGEST)
        );
        // A region-sized request takes the region-sized slot without the room.
        assert_eq!(place(layout(1 << 32, 8), plan), Place::Slot(Class::LARGEST));
        assert_eq!(
            place(layout((1 << 32) + 1, 8), plan),
            Place::Run {
                size: (1 << 32) + 1,
                align: 8
            }
        );
        // An alignment above the region size is a run aligned to it, holding what was asked.
        assert_eq!(
            place(layout(8, 1 << 33), plan),
            Place::Run {
                size: 8,
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
