//! The process's heap, and [`Cralloc`], the handle Rust programs install as their allocator.
//!
//! One heap per process: [`HEAP`] over Linux, run with the generated routines on the static
//! `front::SLABS` ([`process`]). All three are zero until the first slow path starts the heap,
//! so the fast paths never check. This module also gives the C slow paths their way into the
//! heap, [`c_alloc_small`] and [`c_free_small`]. The ledger for the unsafe blocks here is
//! `docs/safety/cralloc/heap/global-safety.md`.

use super::{Bound, Heap, Zero};
use crate::classes::SmallClass;
use crate::front::{Real, publish_masks};
use crate::os::linux::LinuxOs;
use core::alloc::{GlobalAlloc, Layout};
use core::ptr::{self, NonNull};

/// The heap. Its slab array is `front::SLABS`, which only [`Real`] writes.
static HEAP: Heap<LinuxOs> = Heap::starting_with(LinuxOs, START);

/// What starts the background thread when a heartbeat wants one (`background`).
#[cfg(feature = "background")]
const START: Option<fn()> = Some(super::background::start_wanted);
#[cfg(not(feature = "background"))]
const START: Option<fn()> = None;

/// Notes that the program maintains the heap itself, so it gets no default thread.
#[inline]
fn opt_out() {
    #[cfg(feature = "background")]
    {
        use core::sync::atomic::Ordering::Relaxed;
        let opted = &super::background::OPTED_OUT;
        if !opted.load(Relaxed) {
            opted.store(true, Relaxed);
        }
    }
}

/// The process's heap with its routines: everything below runs on this.
#[inline(always)]
pub(super) fn process() -> Bound<'static, LinuxOs, Real> {
    // SAFETY: HEAP-02, the process's heap.
    unsafe { Bound::new(&HEAP, Real) }
}

/// cralloc as Rust's allocator: a zero-sized handle to the process's one heap.
///
/// ```ignore
/// #[global_allocator]
/// static ALLOCATOR: cralloc::Cralloc = cralloc::Cralloc;
/// ```
///
/// Every handle reaches the same heap. Build with `lto = "fat"` (or thin LTO with the
/// declaration in a crate of its own) so that the fast paths inline into their callers.
#[derive(Clone, Copy, Debug, Default)]
pub struct Cralloc;

impl Cralloc {
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
    /// the last interval, and takes the purge pass that is due to its end. It runs one round
    /// in each interval and returns at once for every other call, after one clock read, so it
    /// may be called as often as a caller likes; it is not [`Cralloc::collect`], which
    /// empties every cache at once. Intervals are counted, not read off the clock: a call is
    /// what counts the intervals of a silence, so with nothing calling it a silence of any
    /// length ages the heap by one interval.
    ///
    /// A process whose threads all stay inside their loaded magazines never takes a slow
    /// path, and keeps what it held when it went quiet until something calls this. The
    /// `background` feature's `Cralloc::start_background` calls it from a thread of its
    /// own; the `cralloc-tokio` crate from a task on a tokio runtime. It never waits, and any
    /// thread may call it at any time. On a kernel whose huge pages are over 2 MiB, its purge
    /// pass may split them, which takes 1 to 2 ms each ([`Cralloc::maintain_step`]).
    pub fn maintain() {
        opt_out();
        process().maintain();
    }

    /// One bounded step of [`Cralloc::maintain`]'s round: one slab's idle look, the looks at
    /// the depot's stacks, one magazine laundered, or one step of the purge pass, each a few
    /// microseconds to tens of microseconds. Returns whether the round has more steps; `false`
    /// at once when this interval's round is done. For a caller that must not hold its thread
    /// for a whole round, such as a task on an async runtime, which yields between steps.
    ///
    /// One kind of step takes longer. On a kernel whose huge pages are over 2 MiB and at most
    /// 64 MiB (32 MiB, on pages of 16 KiB), a step of the purge pass may split one huge page, one a
    /// step, and that holds the caller for 1 to 2 ms. It is the price of ever returning that
    /// memory without the background thread, since no slow path splits one that large.
    pub fn maintain_step() -> bool {
        opt_out();
        process().maintain_step()
    }

    /// How long the background thread, or a timer, sleeps between calls of
    /// [`Cralloc::maintain`]: under the shortest interval the clock gives on a plausible
    /// machine (`depot::IDLE_TICKS`, 0.7 s at 6 GHz), so that every interval gets a call.
    /// `maintain` runs one round an interval, so the calls in between cost a clock read.
    #[doc(hidden)]
    pub const MAINTAIN_EVERY: core::time::Duration = core::time::Duration::from_millis(500);

    /// Puts CPU number `c` in cache domain `c / width` instead of the domain its L3 gives it,
    /// so that a benchmark can run with more domains than the machine has L3s. For
    /// measurement only: the hidden feature `internals` is never part of a build that serves
    /// a program.
    #[cfg(feature = "internals")]
    #[doc(hidden)]
    pub fn split_domains(width: usize) {
        HEAP.split_domains(width);
    }
}

// SAFETY: GLOBAL-01. Every method is the process heap's.
unsafe impl GlobalAlloc for Cralloc {
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
        // SAFETY: GLOBAL-05.
        unsafe { process().realloc(ptr, layout, new_size) }
    }
}

#[cfg(feature = "allocator-api")]
// SAFETY: GLOBAL-03.
unsafe impl core::alloc::Allocator for Cralloc {
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
    let _ = unsafe { process().free_slow(class, object) };
}
