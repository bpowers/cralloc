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
//! is still large, fits the slot or run, and, when it shrinks, keeps a part of it
//! (`stays_after_shrink`); a block that outgrows its
//! slot moves to one with headroom (`growth_rooms`), or with less when that cannot be had.
//!
//! Small blocks are not served here: the C layer allocates and frees them with the generated C
//! routines and, when those miss, the heap's slow paths (`c_alloc_small`, `c_free_small`).
//!
//! The ledger for the unsafe blocks here is `docs/safety/cralloc/heap/address-safety.md`.

use super::large::{growth_rooms, stays_after_shrink};
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

    /// The small class of the slot that starts at `ptr`, or `None` if no slot of a small class
    /// starts there (including when the heap has not started): [`Heap::find`]'s
    /// `Found::Small` answer, for less (`Back::small_slot`).
    #[inline]
    pub(crate) fn find_small(&self, ptr: *const u8) -> Option<SmallClass> {
        self.back()?.small_slot(ptr)
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

    /// Allocates a large block for an object growing to `size` bytes: with
    /// [`HEADROOM`](super::large::HEADROOM) times the room, at most a region, or less if that
    /// cannot be had (`growth_rooms`), recording `size` as the size in use; else exactly. The
    /// request's C class must not be small.
    pub(crate) fn alloc_large_grown(&self, size: usize) -> Option<NonNull<u8>> {
        let plan = self.start()?.space().plan();
        growth_rooms(size, plan)
            .filter_map(|room| Layout::from_size_align(room, C_ALIGN).ok())
            .find_map(|layout| self.alloc_large_using(layout, size, Zero::No))
            .map(|grant| grant.ptr)
            .or_else(|| self.alloc_large_aligned(size, C_ALIGN, false))
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
            Some(Kind::Slot(class)) if class.is_small() => return false,
            Some(_) => back.room(ptr.as_ptr()).unwrap_or(0),
            None => return false,
        };
        let plan = back.space().plan();
        let fits =
            is_large(new) && new <= room && (new >= old || stays_after_shrink(new, room, plan));
        if fits {
            // SAFETY: ADDRESS-01. the block is the caller's and live, and `new` fits its room,
            // so what the back end gives up past `new` is the caller's to give.
            unsafe { back.resize_in_place(ptr, new) };
        }
        fits
    }
}

/// The process's heap, for the C layer: the operations above on the one static heap. A large
/// allocation or free here is a slow path, and runs a heartbeat as the Rust ones do.
#[cfg(cralloc_real)]
pub(crate) mod process {
    use super::super::global;
    use super::super::resize::Caller;
    use super::Found;
    use crate::classes::SmallClass;
    use crate::os::Os;
    use core::ptr::{self, NonNull};

    /// The block that starts at `ptr`, if one does.
    #[inline]
    pub(crate) fn find(ptr: *const u8) -> Option<Found> {
        global::process().heap().find(ptr)
    }

    /// The small class of the slot that starts at `ptr`, if one does (`Heap::find_small`).
    #[inline(always)]
    pub(crate) fn find_small(ptr: *const u8) -> Option<SmallClass> {
        global::process().heap().find_small(ptr)
    }

    /// A large block of `size` bytes aligned to `align`, zeroed if `zero`; null if there is no
    /// memory.
    #[cold]
    pub(crate) fn alloc_large(size: usize, align: usize, zero: bool) -> *mut u8 {
        let heap = global::process();
        let block = heap.heap().alloc_large_aligned(size, align, zero);
        heap.heartbeat();
        block.map_or(ptr::null_mut(), NonNull::as_ptr)
    }

    /// A large block for an object growing to `size` bytes, with headroom; null if there is
    /// no memory.
    #[cold]
    pub(crate) fn alloc_large_grown(size: usize) -> *mut u8 {
        let heap = global::process();
        let block = heap.heap().alloc_large_grown(size);
        heap.heartbeat();
        block.map_or(ptr::null_mut(), NonNull::as_ptr)
    }

    /// Frees the large block at `ptr`, finding its class or run from the address. Aborts if no
    /// large block starts there.
    ///
    /// # Safety
    ///
    /// `ptr` is a large block of the heap that the caller owns and gives up.
    #[cold]
    pub(crate) unsafe fn free_large(ptr: NonNull<u8>) {
        // SAFETY: ADDRESS-02. the contract of `Bound::free_large_slow` is this function's.
        unsafe { global::process().free_large_slow(ptr) }
    }

    /// Whether the large slot `ptr` starts is free and kept as the slot a block outgrew
    /// (`Back::kept_outgrown_at`), on the process's heap; for the C layer's tests.
    #[cfg(test)]
    pub(crate) fn kept_outgrown_at(ptr: *const u8) -> bool {
        global::process()
            .heap()
            .back()
            .is_some_and(|back| back.kept_outgrown_at(ptr))
    }

    /// Moves `len` bytes of the large block at `from` to `to` and frees `from`, a piece at a
    /// time with the old pages returned behind the copy (`Back::move_large`); `why` says whether
    /// the block outgrew its slot.
    ///
    /// # Safety
    ///
    /// As for `Back::move_large`: `from` is a large block of the heap that the caller owns
    /// and gives up, readable for `len` bytes; `to` is writable for `len` bytes and does not
    /// overlap it.
    #[cold]
    pub(crate) unsafe fn move_large(
        from: NonNull<u8>,
        to: NonNull<u8>,
        len: usize,
        why: crate::back::Move,
    ) {
        let heap = global::process().heap();
        match heap.back() {
            // SAFETY: ADDRESS-03. the contract of `Back::move_large` is this function's, and
            // `zeroed` is false, which never lets it skip a page.
            Some(back) => unsafe { back.move_large(from, to, len, false, why) },
            None => heap.os().abort("a move of a block when no heap exists"),
        }
    }

    /// The lean path of C's `realloc` (`Bound::resize_small`): the block at `ptr`, of class
    /// `from`, resized to class `to` with its first `kept` bytes kept; `None`, having changed
    /// nothing, when this CPU's magazine of `to` has no block to give.
    ///
    /// # Safety
    ///
    /// As for `Bound::resize_small`.
    #[inline(always)]
    pub(crate) unsafe fn resize_small(
        ptr: NonNull<u8>,
        from: SmallClass,
        to: SmallClass,
        kept: usize,
    ) -> Option<NonNull<u8>> {
        // SAFETY: ADDRESS-02. the contract of `Bound::resize_small` is this function's, for the
        // process's heap.
        unsafe { global::process().resize_small(ptr, from, to, kept, Caller::C) }
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
        // SAFETY: ADDRESS-02. the contract of `Heap::resize_in_place_by_address` is this
        // function's, for the process's heap.
        unsafe {
            global::process()
                .heap()
                .resize_in_place_by_address(ptr, old, new)
        }
    }

    /// The bytes the block at `ptr` may use, if a block starts there.
    pub(crate) fn usable_size(ptr: *const u8) -> Option<usize> {
        global::process().heap().usable_size(ptr)
    }

    /// See `Cralloc::collect`.
    pub(crate) fn collect() -> usize {
        global::process().collect()
    }

    /// The published heap's base address and region shift, once it has started: what the C
    /// layer copies beside the slabs for the C frees (`c::mirror`).
    pub(crate) fn layout() -> Option<(usize, u32)> {
        global::process().heap().back().map(|back| {
            (
                back.space().base().addr().get(),
                back.space().plan().region_shift(),
            )
        })
    }

    /// Ends the process with `message`.
    #[cold]
    pub(crate) fn abort(message: &str) -> ! {
        global::process().heap().os().abort(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SimHeap;
    use crate::classes::{MAX_SMALL_SIZE, SmallClass};
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

    /// `find_small`, which C's `realloc` takes the old class from, gives `find`'s small answer
    /// and nothing else: the class of a block of every small class, and `None` for every
    /// address around those blocks that starts no slot (so that `realloc` falls back to `find`,
    /// which aborts for it), for a large block, before the heap and outside it.
    #[test]
    fn the_small_lookup_agrees_with_the_full_one() {
        let heap = SimHeap::<crate::os::sim::Standard>::new();
        assert_eq!(heap.heap.find_small(ptr_of(&0u8)), None);
        let mut blocks = std::vec::Vec::new();
        for class in SmallClass::all() {
            let l = layout(class.size(), 8);
            // SAFETY: a valid layout; freed below.
            let p = unsafe { heap.alloc(l) };
            assert!(!p.is_null());
            blocks.push((p, l));
            assert_eq!(heap.heap.find_small(p), Some(class), "{class:?}");
            // Every 8-byte step through the block and a little either side.
            let span = class.size().min(4096) as isize;
            for d in (-64..span + 64).step_by(8) {
                let q = p.wrapping_offset(d);
                let want = match heap.heap.find(q) {
                    Some(Found::Small(c)) => Some(c),
                    _ => None,
                };
                assert_eq!(heap.heap.find_small(q), want, "{class:?} at {d}");
            }
        }
        let big = heap.heap.alloc_large_aligned(50 << 10, 16, false).unwrap();
        assert_eq!(heap.heap.find_small(big.as_ptr()), None);
        assert_eq!(heap.heap.find_small(ptr_of(&0u8)), None);
        // SAFETY: the test's blocks, each freed once.
        unsafe {
            heap.heap.free_large(big);
            for (p, l) in blocks {
                heap.dealloc(p, l);
            }
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
        // Growth past the room from the block to its slot's end moves; within it, and a shrink
        // to a large part of it, stays.
        let room = heap.heap.back().unwrap().room(big.as_ptr()).unwrap();
        assert!(room >= 40 << 10);
        assert!(!resize(40 << 10, room + 1));
        assert!(resize(40 << 10, room));
        assert!(resize(room, 33 << 10));
        assert_eq!(
            heap.heap.find(big.as_ptr()),
            Some(Found::Large { size: 33 << 10 })
        );
        // A size with a small class never stays in a large slot.
        assert!(!resize(33 << 10, 32 << 10));
        // SAFETY: the test's block, freed once.
        unsafe { heap.heap.free_large(big) };
    }

    #[test]
    fn a_growing_block_gets_headroom_but_records_its_size() {
        let heap = SimHeap::<crate::os::sim::Standard>::new();
        let grown = heap.heap.alloc_large_grown(40 << 10).unwrap();
        assert_eq!(
            heap.heap.find(grown.as_ptr()),
            Some(Found::Large { size: 40 << 10 })
        );
        // Room for headroom times as much: growth to 150 KiB stays.
        // SAFETY: the test holds the block and uses none of it meanwhile.
        assert!(unsafe {
            heap.heap
                .resize_in_place_by_address(grown, 40 << 10, 150 << 10)
        });
        // SAFETY: the test's block, freed once.
        unsafe { heap.heap.free_large(grown) };
    }

    /// A tenant of no size in a slot an earlier one left resident
    /// (`back::tests::large_charge`), reached through the heap as C's `aligned_alloc(huge, 0)`
    /// reaches it: a zero-size request aligned to a huge page gets a large slot and records a
    /// size of zero. The slot stays counted, and a trim returns it.
    #[test]
    fn a_trim_returns_the_slot_of_a_zero_size_aligned_block() {
        use crate::os::sim::{Shape, Standard};
        let heap = SimHeap::<Standard>::new();
        let huge = Standard::GEOMETRY.huge_page_size();
        let full = heap.heap.alloc_large_aligned(huge, huge, false).unwrap();
        // SAFETY: the test's block, of this size, freed once.
        unsafe {
            full.as_ptr().write_bytes(0x77, huge);
            heap.heap.free_large(full);
        }
        let empty = heap.heap.alloc_large_aligned(0, huge, false).unwrap();
        assert_eq!(empty, full);
        // SAFETY: the test's block, freed once.
        unsafe { heap.heap.free_large(empty) };
        let back = heap.heap.back().unwrap();
        // Counted as resident, which it is.
        assert_eq!(back.retained(), huge);
        back.trim();
        // SAFETY: no other thread runs, and the heap and this test store only integers.
        let purged = unsafe { heap.heap.os().is_purged(full, huge) };
        assert!(purged, "a free slot is still resident after a trim");
    }

    /// When the reservation's regions are all assigned, a growing block that cannot have a slot
    /// with full headroom takes one with less, then one of its size, and fails only where a
    /// plain allocation of its size fails too. In the native shape a region is a megabyte, so
    /// 40 KiB gets four times the room, never sixteen.
    #[test]
    #[cfg_attr(miri, ignore)]
    fn a_growing_block_takes_less_headroom_once_the_regions_run_out() {
        let heap = SimHeap::<crate::os::sim::Native>::new();
        let size = 40 << 10;
        let back = heap.heap.start().unwrap();
        // Each room the growth may take, most first, and then the size itself.
        let steps: std::vec::Vec<usize> = growth_rooms(size, back.space().plan())
            .chain([size])
            .collect();
        // A block of each room but the first, freed, so that each of their classes has a
        // region with free slots when the regions run out.
        for &room in &steps[1..] {
            let layout = layout(room, C_ALIGN);
            let block = heap.heap.alloc_large_using(layout, size, Zero::No).unwrap();
            // SAFETY: the test's block, freed once.
            unsafe { heap.heap.free_large(block.ptr) };
        }
        let (mut held, mut taken) = (std::vec::Vec::new(), std::vec::Vec::new());
        while let Some(block) = heap.heap.alloc_large_grown(size) {
            let Some(Kind::Slot(class)) = back.lookup(block.as_ptr()) else {
                panic!("a growing block of {size} took a run");
            };
            // The largest room the block's slot holds.
            let step = steps.iter().copied().find(|&s| class.size() >= s).unwrap();
            if taken.last() != Some(&step) {
                taken.push(step);
            }
            held.push(block);
        }
        assert_eq!(taken, steps, "{} blocks", held.len());
        assert_eq!(heap.heap.alloc_large_aligned(size, C_ALIGN, false), None);
        for block in held {
            // SAFETY: the test's blocks, each freed once.
            unsafe { heap.heap.free_large(block) };
        }
    }

    fn ptr_of(x: &u8) -> *const u8 {
        x
    }
}
