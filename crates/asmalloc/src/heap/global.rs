//! The process's heap, and [`Asmalloc`], the handle Rust programs install as their allocator.
//!
//! One heap per process: [`HEAP`] over Linux, run with the generated routines on the static
//! `front::SLABS`. All three are zero until the first slow path starts the heap, so the fast
//! paths never check. This module also gives the generated out-of-line routines their slow
//! paths, [`pop_slow`] and [`push_slow`]. The ledger for the unsafe blocks here is
//! `docs/safety/asmalloc/heap/global-safety.md`.

use super::Heap;
use crate::classes::{Class, SmallClass};
use crate::front::Real;
use crate::os::Os;
use crate::os::linux::LinuxOs;
use core::alloc::{GlobalAlloc, Layout};
use core::ptr::{self, NonNull};

/// The heap. Its slab array is `front::SLABS`, which only [`Real`] writes, and nothing else
/// uses that array, so the routines' slabs and this heap always go together (HEAP-02).
static HEAP: Heap<LinuxOs> = Heap::new(LinuxOs);

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

    /// The bytes the block starting at `ptr` may use: its whole slot or run. `None` if no slot
    /// or run of the heap starts at `ptr`.
    pub fn usable_size(ptr: *const u8) -> Option<usize> {
        HEAP.usable_size(ptr)
    }

    /// Pops an object of `class` with the out-of-line routine (for callers that cannot inline,
    /// such as the C entry points). Null if there is no memory.
    #[inline]
    pub fn alloc_small(class: SmallClass) -> *mut u8 {
        Real.pop_out_of_line(class)
    }

    /// Pushes an object of `class` with the out-of-line routine.
    ///
    /// # Safety
    ///
    /// `object` is a block of `class` this heap handed out, which the caller owns and gives up.
    #[inline]
    pub unsafe fn free_small(class: SmallClass, object: NonNull<u8>) {
        // SAFETY: the caller's promise is the routine's.
        unsafe { Real.push_out_of_line(class, object) }
    }
}

// SAFETY: GLOBAL-01. Every method is the heap's, run with `Real` on `front::SLABS`.
unsafe impl GlobalAlloc for Asmalloc {
    #[inline]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: GLOBAL-02.
        unsafe { HEAP.alloc(Real, layout) }
    }

    #[inline]
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: GLOBAL-02.
        unsafe { HEAP.alloc_grant(Real, layout, super::Zero::Requested) }
            .map_or(ptr::null_mut(), |grant| grant.ptr.as_ptr())
    }

    #[inline]
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: GLOBAL-02.
        unsafe { HEAP.dealloc(Real, ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let (Some(block), Ok(new)) = (
            NonNull::new(ptr),
            Layout::from_size_align(new_size, layout.align()),
        ) else {
            return ptr::null_mut();
        };
        // SAFETY: GLOBAL-02.
        unsafe { HEAP.resize(Real, block, layout, new, false) }
            .map_or(ptr::null_mut(), |grant| grant.ptr.as_ptr())
    }
}

#[cfg(feature = "allocator-api")]
// SAFETY: GLOBAL-03.
unsafe impl core::alloc::Allocator for Asmalloc {
    #[inline]
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, core::alloc::AllocError> {
        // SAFETY: GLOBAL-02.
        unsafe { HEAP.allocate(Real, layout, false) }
    }

    #[inline]
    fn allocate_zeroed(&self, layout: Layout) -> Result<NonNull<[u8]>, core::alloc::AllocError> {
        // SAFETY: GLOBAL-02.
        unsafe { HEAP.allocate(Real, layout, true) }
    }

    #[inline]
    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        // SAFETY: GLOBAL-02.
        unsafe { HEAP.deallocate(Real, ptr, layout) }
    }

    unsafe fn grow(
        &self,
        ptr: NonNull<u8>,
        old: Layout,
        new: Layout,
    ) -> Result<NonNull<[u8]>, core::alloc::AllocError> {
        // SAFETY: GLOBAL-02.
        unsafe { HEAP.grow(Real, ptr, old, new, false) }
    }

    unsafe fn grow_zeroed(
        &self,
        ptr: NonNull<u8>,
        old: Layout,
        new: Layout,
    ) -> Result<NonNull<[u8]>, core::alloc::AllocError> {
        // SAFETY: GLOBAL-02.
        unsafe { HEAP.grow(Real, ptr, old, new, true) }
    }

    unsafe fn shrink(
        &self,
        ptr: NonNull<u8>,
        old: Layout,
        new: Layout,
    ) -> Result<NonNull<[u8]>, core::alloc::AllocError> {
        // SAFETY: GLOBAL-02.
        unsafe { HEAP.shrink(Real, ptr, old, new) }
    }
}

/// The small class with index `class`, or an abort: the out-of-line routines take any index
/// below 512, and only a small class's has a magazine.
fn small_class(class: usize) -> SmallClass {
    match u8::try_from(class)
        .ok()
        .and_then(Class::from_index)
        .and_then(Class::small)
    {
        Some(small) => small,
        None => HEAP
            .os()
            .abort("an out-of-line routine was called with no small class"),
    }
}

/// Where the out-of-line pop routine jumps when it misses, with its own arguments: the object's
/// address, or 0 if there is no memory. The thread's `rseq_cs` still names the routine's
/// sequence, which it has left, so an interruption only clears it.
#[cold]
#[inline(never)]
pub(crate) extern "sysv64" fn pop_slow(class: usize) -> usize {
    // SAFETY: GLOBAL-02.
    unsafe { HEAP.alloc_slow(Real, small_class(class)) }.expose_provenance()
}

/// Where the out-of-line push routine jumps when it misses, with its own arguments.
#[cold]
#[inline(never)]
pub(crate) extern "sysv64" fn push_slow(class: usize, object: usize) {
    let class = small_class(class);
    let Some(object) = NonNull::new(ptr::with_exposed_provenance_mut::<u8>(object)) else {
        HEAP.os().abort("free of a null pointer through a routine");
    };
    // SAFETY: GLOBAL-02 and GLOBAL-04.
    unsafe { HEAP.free_slow(Real, class, object) }
}
