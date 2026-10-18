//! What the C entry points need beyond the Rust ones: blocks known only by their address, and
//! large blocks asked for by a size and an alignment instead of a `Layout`.
//!
//! C passes a bare pointer to `free`, `realloc` and `malloc_usable_size`, so the block's class
//! and size come from the address: the region table gives the class, and a large block's
//! recorded size says how much of it is in use (`Back::lookup`, `Back::size`). A size from C is
//! never trusted.
//!
//! The in-place and headroom rules for large blocks are the ones `large.rs` applies to Rust
//! resizes, restated for a block described by its address: a block stays put when the new size
//! is still large, fits the slot or run, and, when it shrinks, keeps at least a
//! [`SHRINK_MOVES_BELOW`](super::large::SHRINK_MOVES_BELOW)th of it; a block that outgrows its
//! slot moves to one with [`HEADROOM`] times the room, but not past a region.
//!
//! Small blocks are not served here: the C layer allocates and frees them with the
//! out-of-line routines (`Asmalloc::alloc_small`, `Asmalloc::free_small`).

use super::large::{HEADROOM, stays_after_shrink};
use super::{Heap, Zero};
use crate::back::Kind;
use crate::classes::{C_ALIGN, Class, SmallClass};
use crate::os::Os;
use core::alloc::Layout;
use core::ptr::NonNull;

/// A block of the heap, as its address describes it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Found {
    /// A slot of a small class. All of it is the caller's.
    Small(SmallClass),
    /// A large slot or a run, holding `size` bytes in use: the size last asked for.
    Large {
        /// The recorded size.
        size: usize,
    },
}

/// Whether a C request of `size` bytes takes a large block: its class is not small, or it has
/// no class at all.
fn is_large(size: usize) -> bool {
    match Layout::from_size_align(size, 1) {
        Ok(layout) => Class::of_c(layout).is_none_or(|class| !class.is_small()),
        Err(_) => true,
    }
}

impl<O: Os> Heap<O> {
    /// The block that starts at `ptr`, or `None` if no slot or run of the heap starts there
    /// (including when the heap has not started). A slot that is free, or was never handed
    /// out, is still found; the back end catches a free of one when the slot reaches its
    /// bitmap.
    pub(crate) fn find(&self, ptr: *const u8) -> Option<Found> {
        let back = self.back()?;
        match back.lookup(ptr)? {
            Kind::Slot(class) => match class.small() {
                Some(small) => Some(Found::Small(small)),
                None => back.size(ptr).map(|size| Found::Large { size }),
            },
            Kind::Run(_) => back.size(ptr).map(|size| Found::Large { size }),
        }
    }

    /// Allocates a large block of `size` bytes aligned to `align` (a power of two; C's 16 at
    /// least, whatever is asked). With `zero`, all of it the caller may use (its usable size)
    /// reads as zero, skipping memory the back end knows is zero. The request's C
    /// class must not be small. `None` if there is no memory or the size and alignment
    /// describe no block.
    pub(crate) fn alloc_large_aligned(
        &self,
        size: usize,
        align: usize,
        zero: bool,
    ) -> Option<NonNull<u8>> {
        let layout = Layout::from_size_align(size, align.max(C_ALIGN)).ok()?;
        debug_assert!(Class::of_layout(layout).is_none_or(|class| !class.is_small()));
        let fill = if zero { Zero::Whole } else { Zero::No };
        self.alloc_large(layout, fill).map(|grant| grant.ptr)
    }

    /// Allocates a large block for an object growing to `size` bytes: with [`HEADROOM`] times
    /// the room if that fits in a region, recording `size` as the size in use, else exactly.
    /// The request's C class must not be small.
    pub(crate) fn alloc_large_grown(&self, size: usize) -> Option<NonNull<u8>> {
        let roomy = size.checked_mul(HEADROOM).filter(|&room| {
            self.back()
                .is_none_or(|back| room <= back.space().plan().region_size())
        });
        if let Some(room) = roomy
            && let Some(ptr) = self.alloc_large_aligned(room, C_ALIGN, false)
        {
            if let Some(back) = self.back() {
                back.resize(ptr, size);
            }
            return Some(ptr);
        }
        self.alloc_large_aligned(size, C_ALIGN, false)
    }

    /// Resizes the large block at `ptr`, which holds `old` bytes in use, to `new` bytes in
    /// place if the rules in the module documentation allow it, and says whether it did. A
    /// block that stays records `new` as its size.
    ///
    /// # Safety
    ///
    /// `ptr` is a live large block of this heap that the caller owns and is resizing.
    pub(crate) unsafe fn resize_in_place_by_address(
        &self,
        ptr: NonNull<u8>,
        old: usize,
        new: usize,
    ) -> bool {
        let Some(back) = self.back() else {
            return false;
        };
        let room = match back.lookup(ptr.as_ptr()) {
            Some(Kind::Slot(class)) if !class.is_small() => class.size(),
            Some(Kind::Run(regions)) => regions << back.space().plan().region_shift(),
            _ => return false,
        };
        let fits = is_large(new) && new <= room && (new >= old || stays_after_shrink(new, room));
        if fits {
            // SAFETY: the caller's promise; a resize gives up what lies past the new size.
            unsafe { back.resize_in_place(ptr, new) };
        }
        fits
    }
}

/// The process's heap, for the C layer: the operations above on the one static heap. A large
/// allocation or free here is a slow path, and runs a heartbeat as the Rust ones do.
#[cfg(all(target_os = "linux", target_arch = "x86_64", not(miri), not(kani)))]
pub(crate) mod process {
    use super::super::global::{HEAP, large_heartbeat};
    use super::Found;
    use crate::Asmalloc;
    use crate::os::Os;
    use core::ptr::{self, NonNull};

    /// The block that starts at `ptr`, if one does.
    #[inline]
    pub(crate) fn find(ptr: *const u8) -> Option<Found> {
        HEAP.find(ptr)
    }

    /// A large block of `size` bytes aligned to `align`, zeroed if `zero`; null if there is no
    /// memory.
    #[cold]
    pub(crate) fn alloc_large(size: usize, align: usize, zero: bool) -> *mut u8 {
        let block = HEAP.alloc_large_aligned(size, align, zero);
        large_heartbeat();
        block.map_or(ptr::null_mut(), NonNull::as_ptr)
    }

    /// A large block for an object growing to `size` bytes, with headroom; null if there is
    /// no memory.
    #[cold]
    pub(crate) fn alloc_large_grown(size: usize) -> *mut u8 {
        let block = HEAP.alloc_large_grown(size);
        large_heartbeat();
        block.map_or(ptr::null_mut(), NonNull::as_ptr)
    }

    /// Frees the large block at `ptr`, finding its class or run from the address. Aborts if no
    /// large block starts there, or if it is free.
    #[cold]
    pub(crate) fn free_large(ptr: NonNull<u8>) {
        HEAP.free_large(ptr);
        large_heartbeat();
    }

    /// Moves `len` bytes of the large block at `from` to `to` and frees `from`, a piece at a
    /// time with the old pages returned behind the copy (`Back::move_large`).
    ///
    /// # Safety
    ///
    /// As for `Back::move_large`: `from` is a large block of the heap that the caller owns
    /// and gives up, readable for `len` bytes; `to` is writable for `len` bytes and does not
    /// overlap it.
    #[cold]
    pub(crate) unsafe fn move_large(from: NonNull<u8>, to: NonNull<u8>, len: usize) {
        match HEAP.back() {
            // SAFETY: the caller's promise, passed on.
            Some(back) => unsafe { back.move_large(from, to, len) },
            None => HEAP.os().abort("a move of a block when no heap exists"),
        }
    }

    /// See `Heap::resize_in_place_by_address`.
    ///
    /// # Safety
    ///
    /// As for `Heap::resize_in_place_by_address`.
    pub(crate) unsafe fn resize_in_place_by_address(
        ptr: NonNull<u8>,
        old: usize,
        new: usize,
    ) -> bool {
        // SAFETY: the caller's promise, passed on.
        unsafe { HEAP.resize_in_place_by_address(ptr, old, new) }
    }

    /// The bytes the block at `ptr` may use, if a block starts there.
    pub(crate) fn usable_size(ptr: *const u8) -> Option<usize> {
        HEAP.usable_size(ptr)
    }

    /// See `Asmalloc::collect`.
    pub(crate) fn collect() -> usize {
        Asmalloc::collect()
    }

    /// The published heap's base address and region shift, once it has started: what the C
    /// layer copies beside the slabs for the C frees (`c::mirror`).
    pub(crate) fn layout() -> Option<(usize, u32)> {
        HEAP.back().map(|back| {
            (
                back.space().base().addr().get(),
                back.space().plan().region_shift(),
            )
        })
    }

    /// Ends the process with `message`.
    #[cold]
    pub(crate) fn abort(message: &str) -> ! {
        HEAP.os().abort(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SimHeap;
    use crate::classes::MAX_SMALL_SIZE;
    use core::alloc::GlobalAlloc;

    fn layout(size: usize, align: usize) -> Layout {
        Layout::from_size_align(size, align).unwrap()
    }

    #[test]
    fn requests_above_the_small_classes_are_large() {
        assert!(!is_large(0));
        assert!(!is_large(MAX_SMALL_SIZE));
        assert!(is_large(MAX_SMALL_SIZE + 1));
        assert!(is_large(usize::MAX));
        // 24 bytes is a 32-byte class in C, still small.
        assert!(!is_large(24));
    }

    #[test]
    fn a_block_is_found_from_its_address() {
        let heap = SimHeap::<crate::os::sim::Standard>::new();
        assert_eq!(heap.heap.find(ptr_of(&0u8)), None);
        // SAFETY: valid layouts; each block is freed once.
        unsafe {
            let small = heap.alloc(layout(100, 8));
            let class = Class::of_size(112).unwrap().small().unwrap();
            assert_eq!(heap.heap.find(small), Some(Found::Small(class)));
            assert_eq!(heap.heap.find(small.add(8)), None);
            let big = heap.heap.alloc_large_aligned(50 << 10, 16, false).unwrap();
            assert_eq!(
                heap.heap.find(big.as_ptr()),
                Some(Found::Large { size: 50 << 10 })
            );
            heap.heap.free_large(big);
            heap.dealloc(small, layout(100, 8));
        }
    }

    #[test]
    fn large_blocks_resize_in_place_within_their_room() {
        let heap = SimHeap::<crate::os::sim::Standard>::new();
        let big = heap.heap.alloc_large_aligned(40 << 10, 16, false).unwrap();
        let resize = |old, new| {
            // SAFETY: the test holds the block and uses none of it meanwhile.
            unsafe { heap.heap.resize_in_place_by_address(big, old, new) }
        };
        // The slot is the 40 KiB class: growth past it moves, a shrink to an eighth stays.
        assert!(!resize(40 << 10, 41 << 10));
        assert!(resize(40 << 10, 33 << 10));
        assert_eq!(
            heap.heap.find(big.as_ptr()),
            Some(Found::Large { size: 33 << 10 })
        );
        // A size with a small class never stays in a large slot.
        assert!(!resize(33 << 10, 32 << 10));
        heap.heap.free_large(big);
    }

    #[test]
    fn a_growing_block_gets_headroom_but_records_its_size() {
        let heap = SimHeap::<crate::os::sim::Standard>::new();
        let grown = heap.heap.alloc_large_grown(40 << 10).unwrap();
        assert_eq!(
            heap.heap.find(grown.as_ptr()),
            Some(Found::Large { size: 40 << 10 })
        );
        // Room for four times as much: growth to 150 KiB stays.
        // SAFETY: the test holds the block and uses none of it meanwhile.
        assert!(unsafe {
            heap.heap
                .resize_in_place_by_address(grown, 40 << 10, 150 << 10)
        });
        heap.heap.free_large(grown);
    }

    fn ptr_of(x: &u8) -> *const u8 {
        x
    }
}
