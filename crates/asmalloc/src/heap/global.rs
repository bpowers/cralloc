//! The process's heap, and [`Asmalloc`], the handle Rust programs install as their allocator.
//!
//! One heap per process: [`HEAP`] over Linux, run with the generated routines on the static
//! `front::SLABS` ([`process`]). All three are zero until the first slow path starts the heap,
//! so the fast paths never check. This module also gives the C slow paths their way into the
//! heap, [`c_alloc_small`] and [`c_free_small`]. The ledger for the unsafe blocks here is
//! `docs/safety/asmalloc/heap/global-safety.md`.

use super::{Bound, Heap, Zero};
use crate::classes::SmallClass;
use crate::front::{Real, publish_masks};
use crate::os::linux::LinuxOs;
use core::alloc::{GlobalAlloc, Layout};
use core::ptr::{self, NonNull};

/// The heap. Its slab array is `front::SLABS`, which only [`Real`] writes.
static HEAP: Heap<LinuxOs> = Heap::new(LinuxOs);

/// The process's heap with its routines: everything below runs on this.
#[inline(always)]
pub(super) fn process() -> Bound<'static, LinuxOs, Real> {
    // SAFETY: HEAP-02, the process's heap.
    unsafe { Bound::new(&HEAP, Real) }
}

/// asmalloc as Rust's allocator: a zero-sized handle to the process's one heap.
///
/// ```ignore
/// #[global_allocator]
/// static ALLOCATOR: asmalloc::Asmalloc = asmalloc::Asmalloc;
/// ```
///
/// Every handle reaches the same heap. Build with `lto = "fat"` (or thin LTO with the
/// declaration in a crate of its own) so that the fast paths inline into their callers.
#[derive(Clone, Copy, Debug, Default)]
pub struct Asmalloc;

impl Asmalloc {
    /// Whether `ptr` lies in the heap's object regions. Every block the heap hands out does;
    /// so does any pointer into one, or into a slot that is free.
    pub fn contains(ptr: *const u8) -> bool {
        HEAP.contains(ptr)
    }

    /// The bytes the block starting at `ptr` may use: the class size of a small block; for a
    /// large one, the size last asked for rounded up to a page, not the whole slot or run.
    /// `None` if no slot or run of the heap starts at `ptr`.
    pub fn usable_size(ptr: *const u8) -> Option<usize> {
        HEAP.usable_size(ptr)
    }

    /// Walks the heap for `visitor`: every region, free run and cached object. Read-only; a
    /// racy snapshot when other threads are allocating. See [`crate::stats`].
    pub fn inspect(visitor: &mut impl crate::stats::Visitor) {
        process().inspect(visitor);
    }

    /// Returns everything the heap can return now, without waiting for its clock: empties the
    /// magazines of every CPU it has run on (one `membarrier` per CPU other than the caller's),
    /// sends every object parked in magazines and the depot back to the bitmaps, and returns
    /// every wholly free page to the OS. For a program that knows it is idle; while other
    /// threads allocate it is a racy sweep, safe but incomplete. Returns the bytes returned to
    /// the OS that the heap believed resident. `malloc_trim` calls it.
    pub fn collect() -> usize {
        process().collect()
    }

    /// One round of the heap's clocked maintenance, which otherwise runs only on slow paths:
    /// drains the magazines of every CPU that has taken no slow path for an interval (about a
    /// second; one `membarrier` each), launders what the depot's stacks did not need during
    /// the last interval, and takes the purge pass that is due to its end. It respects the
    /// clock as the slow paths do, so calling it more often than about once a second does next
    /// to nothing, and it is not [`Asmalloc::collect`], which empties every cache at once.
    ///
    /// A process whose threads all stay inside their loaded magazines never takes a slow
    /// path, and keeps what it held when it went quiet until something calls this. The
    /// `background` feature's [`Asmalloc::start_background`] calls it from a thread of its
    /// own; a program with an idle hook can call it from there instead. It never waits, and
    /// any thread may call it at any time.
    pub fn maintain() {
        process().maintain();
    }

    /// How long the background thread sleeps between calls of [`Asmalloc::maintain`]: under
    /// the shortest interval the clock gives on a plausible machine (`depot::IDLE_TICKS`, 0.7 s
    /// at 6 GHz), so that every interval gets a call.
    #[doc(hidden)]
    pub const MAINTAIN_EVERY: core::time::Duration = core::time::Duration::from_millis(500);
}

// SAFETY: GLOBAL-01. Every method is the process heap's.
unsafe impl GlobalAlloc for Asmalloc {
    #[inline]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        process().alloc(layout)
    }

    #[inline]
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        process()
            .alloc_grant(layout, Zero::Requested)
            .map_or(ptr::null_mut(), |grant| grant.ptr.as_ptr())
    }

    #[inline]
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: GLOBAL-05.
        unsafe { process().dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let (Some(block), Ok(new)) = (
            NonNull::new(ptr),
            Layout::from_size_align(new_size, layout.align()),
        ) else {
            return ptr::null_mut();
        };
        // SAFETY: GLOBAL-05; `new_size` is not zero, by `realloc`'s contract.
        unsafe { process().resize(block, layout, new, false) }
            .map_or(ptr::null_mut(), |grant| grant.ptr.as_ptr())
    }
}

#[cfg(feature = "allocator-api")]
// SAFETY: GLOBAL-03.
unsafe impl core::alloc::Allocator for Asmalloc {
    #[inline]
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, core::alloc::AllocError> {
        process().allocate(layout, false)
    }

    #[inline]
    fn allocate_zeroed(&self, layout: Layout) -> Result<NonNull<[u8]>, core::alloc::AllocError> {
        process().allocate(layout, true)
    }

    #[inline]
    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        // SAFETY: GLOBAL-05.
        unsafe { process().deallocate(ptr, layout) }
    }

    unsafe fn grow(
        &self,
        ptr: NonNull<u8>,
        old: Layout,
        new: Layout,
    ) -> Result<NonNull<[u8]>, core::alloc::AllocError> {
        // SAFETY: GLOBAL-05.
        unsafe { process().grow(ptr, old, new, false) }
    }

    unsafe fn grow_zeroed(
        &self,
        ptr: NonNull<u8>,
        old: Layout,
        new: Layout,
    ) -> Result<NonNull<[u8]>, core::alloc::AllocError> {
        // SAFETY: GLOBAL-05.
        unsafe { process().grow(ptr, old, new, true) }
    }

    unsafe fn shrink(
        &self,
        ptr: NonNull<u8>,
        old: Layout,
        new: Layout,
    ) -> Result<NonNull<[u8]>, core::alloc::AllocError> {
        // SAFETY: GLOBAL-05.
        unsafe { process().shrink(ptr, old, new) }
    }
}

/// A small block of `class` for the C slow paths, from the heap's own slow path (which pops
/// first): reached only after a generated C routine missed, so no second pop in front of it.
/// Also where the masks the generated C routines read are written: until they are, every call of
/// those routines misses and arrives here. Null if there is no memory.
#[cold]
pub(crate) fn c_alloc_small(class: SmallClass) -> *mut u8 {
    publish_masks();
    process().alloc_slow(class)
}

/// Frees a small block of `class` for the C slow paths, by the heap's own slow path (which
/// pushes first), and writes the masks as [`c_alloc_small`] does.
///
/// # Safety
///
/// `object` is a block of `class` this heap handed out, which the caller owns and gives up.
#[cold]
pub(crate) unsafe fn c_free_small(class: SmallClass, object: NonNull<u8>) {
    publish_masks();
    // SAFETY: GLOBAL-04.
    unsafe { process().free_slow(class, object) }
}
