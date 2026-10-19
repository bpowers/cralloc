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
//! [`HEADROOM`] times the room it asked for, so a buffer that doubles is copied once in every
//! five doublings, not at each one; but only while that is a small part of a region, and
//! [`FALLBACK_HEADROOM`] times past it while that fits a region, since a run commits all its
//! memory at once ([`growth_rooms`]). When that slot cannot be had, it takes less, then none.
//! Small objects get no headroom: their class comes from the layout, so their slot must be exactly
//! their class.
//!
//! A large allocation or free is a slow path of its own and runs a heartbeat
//! ([`Bound::heartbeat`]), so a program that allocates only large blocks still keeps the caches,
//! the depot and the back end maintained.
//!
//! Fresh slots and runs read as zero; the back end says when a reused large slot is known to, so
//! zeroed allocation clears only what it must.
//!
//! Every allocation and in-place resize of a large object records the size asked for
//! (`Back::alloc`, `Back::alloc_run`, `Back::resize_in_place`). What the caller may use is that size rounded
//! up to a page, so a block with headroom never reports, or makes anyone touch, the room it has not
//! been asked for.

use super::{Bound, Grant, Heap, Zero, small};
use crate::back::policy::{STAGGER_ROOM, STAGGER_STEP};
use crate::classes::Class;
use crate::front::Routines;
use crate::os::Os;
use crate::space::Plan;
use core::alloc::Layout;
use core::ptr::NonNull;

/// How many times the room it asked for an object gets when it outgrows its block. A buffer
/// that doubles is then copied once in every five doublings (the class above sixteen times the
/// size, with the stagger's room, is twenty times it, which holds four doublings), and copies
/// about a fifth of its final size on average over final sizes, against a third at four times;
/// that copy was half of the time of a buffer grown to 4 MiB and freed, over and over. The room
/// costs no memory, since the slot is charged, kept and purged by the pages that are not clean
/// and a move copies dense pieces whole, but it does cost address space and, under strict
/// overcommit, commit charge, both in proportion to the factor, where the copying saved falls
/// only with its logarithm: 32 would copy a sixth instead of a fifth for twice the room
/// (`docs/research/memory.md`, "Headroom for growing blocks").
pub(crate) const HEADROOM: usize = 16;

/// The headroom a growing block gets where [`HEADROOM`] times its size is past
/// [`HEADROOM_SHARE`], and falls back to when no slot with that much room can be had (the
/// reservation's regions are all assigned, or the OS refused to commit the slot), before it
/// takes a block of its size: the headroom the earlier rule gave every growing block, so that
/// nothing that rule could serve fails for want of the larger slot.
const FALLBACK_HEADROOM: usize = 4;

/// [`HEADROOM`] times a block's size is room only while it is at most this fraction of a
/// region: a sixteenth, so that the class it lands in has a dozen slots or more to a region.
/// Regions are assigned to a class for good, so a room that left a class one slot to a region
/// took a region for every buffer that grew there, and when they were gone nothing of another
/// class could be had: under an address-space limit of 16 GiB, whose regions are 2 MiB, 3,939
/// buffers grown from 40 to 64 KiB used every region, where four times the room fits 23,634.
/// Past it the room is four times, where that fits a region, which is the earlier rule and fits
/// as many buffers as it did.
const HEADROOM_SHARE: usize = 16;

/// A large object that shrinks to under this fraction of its slot or run moves: a region has
/// few slots of a large class, and one should not be held by an object a small fraction of its
/// size.
pub(crate) const SHRINK_MOVES_BELOW: usize = shrink_moves_below(HEADROOM);

/// The shrink threshold for a headroom factor: twice it. A block that moved in with headroom
/// fills more than `1 / (headroom * 5 / 4)` of its slot (a class is less than a quarter above the
/// size it was chosen for), and a block that shrinks back to the size it had before the move
/// must stay, or a block resized back and forth across a class boundary is copied at every
/// call; twice the factor leaves that gap with room to spare.
const fn shrink_moves_below(headroom: usize) -> usize {
    2 * headroom
}

const _: () = assert!(4 * SHRINK_MOVES_BELOW > 5 * HEADROOM);
const _: () = assert!(HEADROOM >= FALLBACK_HEADROOM);

/// The rooms to try, most first, for a block growing out of its slot to `size` bytes, in a heap
/// whose regions are `region` bytes: [`HEADROOM`] times the size where that is at most
/// [`HEADROOM_SHARE`] of a region, then [`FALLBACK_HEADROOM`] times where that fits a region,
/// since a run commits all its memory at once. With none, the caller takes a block of the size.
pub(crate) fn growth_rooms(size: usize, region: usize) -> impl Iterator<Item = usize> {
    rooms(size, region, HEADROOM)
}

fn rooms(size: usize, region: usize, headroom: usize) -> impl Iterator<Item = usize> {
    let room = move |factor: usize, most: usize| {
        size.checked_mul(factor)
            .filter(|&room| room <= most && room > size)
    };
    let first = room(headroom, region / HEADROOM_SHARE);
    let second = room(FALLBACK_HEADROOM, region).filter(move |&r| Some(r) != first);
    first.into_iter().chain(second)
}

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

impl<O: Os> Heap<O> {
    /// Allocates a large block for `layout`, with the bytes `zero` names zeroed. `None` if there
    /// is no memory.
    #[cold]
    #[inline(never)]
    pub(crate) fn alloc_large(&self, layout: Layout, zero: Zero) -> Option<Grant> {
        self.alloc_large_using(layout, layout.size(), zero)
    }

    /// [`Heap::alloc_large`] with the room of `layout` for a block that uses `used` bytes of it,
    /// at most `layout.size()`: a block that grows and takes headroom. The slot is chosen for
    /// the room and the block records `used`, so the back end knows from the claim which of the
    /// slot's pages the block may write (`back::region::settle_slot`). A run records the room.
    pub(crate) fn alloc_large_using(
        &self,
        layout: Layout,
        used: usize,
        zero: Zero,
    ) -> Option<Grant> {
        let back = self.start()?;
        let used = used.min(layout.size());
        match place(layout, back.space().plan()) {
            Place::Slot(class) => {
                let block = if layout.align() <= STAGGER_STEP {
                    back.alloc_staggered(class, used)?
                } else {
                    back.alloc(class, used)?
                };
                let len = block.len;
                let clear = match zero {
                    _ if block.zeroed => 0,
                    Zero::No => 0,
                    Zero::Requested => used,
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

    /// Frees a large block, finding its class or run from its address. Aborts if no large
    /// block starts there.
    ///
    /// # Safety
    ///
    /// `ptr` is a large block of this heap that the caller owns and gives up: the back end
    /// may purge its pages before it releases the slot (BACK-CLASS-01).
    #[cold]
    #[inline(never)]
    pub(crate) unsafe fn free_large(&self, ptr: NonNull<u8>) {
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
        let whole = self.back().and_then(|back| back.room(ptr.as_ptr()));
        match whole {
            Some(len) => len,
            None => self
                .os()
                .abort("resize of a pointer that is not a block of the heap"),
        }
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
            // SAFETY: the block is the caller's and `new` fits it; a resize gives up only what
            // lies past the new size.
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

impl<O: Os, R: Routines + Copy> Bound<'_, O, R> {
    /// [`Heap::alloc_large`] as the slow path it counts as: the block, then a heartbeat.
    pub(crate) fn alloc_large_slow(self, layout: Layout, zero: Zero) -> Option<Grant> {
        self.alloc_large_slow_using(layout, layout.size(), zero)
    }

    /// [`Heap::alloc_large_using`] as the slow path it counts as.
    pub(crate) fn alloc_large_slow_using(
        self,
        layout: Layout,
        used: usize,
        zero: Zero,
    ) -> Option<Grant> {
        let grant = self.heap().alloc_large_using(layout, used, zero);
        self.heartbeat();
        grant
    }

    /// [`Heap::free_large`] as the slow path it counts as: the free, then a heartbeat.
    ///
    /// # Safety
    ///
    /// As for [`Heap::free_large`].
    pub(crate) unsafe fn free_large_slow(self, ptr: NonNull<u8>) {
        // SAFETY: the caller gives up the large block.
        unsafe { self.heap().free_large(ptr) };
        self.heartbeat();
    }

    /// Resizes the block at `ptr` from `old` to `new`, in place when the next free will still
    /// find its class, else by moving it. With `zero`, the bytes of the result past the old
    /// block read as zero. `None` if a move found no memory; the old block is then untouched.
    ///
    /// # Safety
    ///
    /// As for [`Bound::dealloc`] with `ptr` and `old`, except that the caller gives the block
    /// up only if the resize succeeds; `new` has a non-zero size.
    pub(crate) unsafe fn resize(
        self,
        ptr: NonNull<u8>,
        old: Layout,
        new: Layout,
        zero: bool,
    ) -> Option<Grant> {
        let heap = self.heap();
        let len = heap.block_len(ptr, old);
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
            // SAFETY: the block is the caller's, large, and `new` fits it (`in_place`).
            return Some(unsafe { heap.resize_large_in_place(ptr, new, zero) });
        }
        // Growing a large object out of its block: take headroom. A zeroed grow takes none, so
        // that it never clears room nobody asked for.
        // Not for a run: it commits all its regions at once, so headroom would be commit charge,
        // not just address space.
        let region = heap
            .back()
            .map_or(0, |back| back.space().plan().region_size());
        let roomy = !zero && new.size() > len && small(new).is_none();
        // A zeroed grow clears only what lies past the bytes it keeps, and only when the new
        // block is not known to read as zero.
        // A block with headroom records the size asked for, not the room it was given.
        let grant = growth_rooms(new.size(), region)
            .filter(|_| roomy)
            .filter_map(|room| Layout::from_size_align(room, new.align()).ok())
            .find_map(|layout| self.alloc_large_slow_using(layout, new.size(), Zero::No))
            .or_else(|| self.alloc_grant(new, Zero::No))?;
        let kept = old.size().min(new.size());
        // SAFETY: HEAP-04.
        unsafe {
            match heap.back() {
                // A large block is copied in pieces, its old pages returned behind the copy,
                // and freed, all by the back end.
                Some(back) if small(old).is_none() => {
                    back.move_large(ptr, grant.ptr, kept, grant.zeroed)
                }
                _ => {
                    ptr.as_ptr()
                        .copy_to_nonoverlapping(grant.ptr.as_ptr(), kept);
                    self.dealloc(ptr.as_ptr(), old);
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
    fn growth_takes_headroom_while_it_is_a_small_part_of_a_region_then_less() {
        let region = 1 << 32;
        let rooms = |size| growth_rooms(size, region).collect::<std::vec::Vec<_>>();
        // Full headroom while it is a small part of a region, the fallback's after it; the
        // fallback is left out where it is the same room.
        let share = region / HEADROOM_SHARE;
        let both = [HEADROOM << 20, FALLBACK_HEADROOM << 20];
        assert_eq!(
            rooms(1 << 20),
            both[..if HEADROOM == FALLBACK_HEADROOM { 1 } else { 2 }]
        );
        assert_eq!(rooms(share / HEADROOM)[0], share);
        let past = share / HEADROOM + 1;
        assert_eq!(rooms(past), [FALLBACK_HEADROOM * past]);
        // Never past a region.
        assert_eq!(rooms(region / FALLBACK_HEADROOM), [region]);
        assert_eq!(rooms(region / FALLBACK_HEADROOM + 1), []);
        assert_eq!(rooms(region), []);
        assert_eq!(rooms(usize::MAX / 2), []);
    }

    /// The settling property, for every factor the experiment tried and for the fallback: a
    /// block that fills its slot, outgrows it by a byte and moves into a slot with headroom, and
    /// is then cut back to the size it had, stays in place, so a block resized back and forth
    /// across a class boundary is copied once, not at every call. The worst case is a slot with
    /// no stagger, whose whole size counts against the shrink.
    #[test]
    fn a_block_that_moved_in_with_headroom_stays_when_it_shrinks_back() {
        let plan = Geometry::DESIGN.largest_plan();
        let region = plan.region_size();
        for factor in [4, 8, HEADROOM, 32] {
            let below = shrink_moves_below(factor);
            for class in Class::all().filter(|&c| !c.is_small() && plan.fits_region(c)) {
                let size = class.size();
                for room in rooms(size + 1, region, factor) {
                    let Place::Slot(slot) = place(layout(room, 16), plan) else {
                        panic!("a room of at most a region is a slot: {room}");
                    };
                    assert!(slot.size() > size);
                    assert!(
                        size >= slot.size() / below,
                        "factor {factor}: {size} moved to {} and would move back",
                        slot.size()
                    );
                }
            }
        }
    }
}
