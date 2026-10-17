//! [`Asmalloc`] under Miri: Miri's own allocator.
//!
//! Miri cannot run the generated routines or make system calls, and a program checked under
//! Miri is better served by Miri's allocator, which tracks each block on its own (design:
//! "Under Miri"). asmalloc's own tests run the allocator under Miri with twins on the simulated
//! OS instead (`SimHeap`, `SimAsmalloc`). The ledger entry is
//! `docs/safety/asmalloc/heap/miri-safety.md`.

use core::alloc::{GlobalAlloc, Layout};

// SAFETY: MIRI-01. These are Miri's allocator shims, with the signatures Miri gives them.
unsafe extern "Rust" {
    fn miri_alloc(size: usize, align: usize) -> *mut u8;
    fn miri_dealloc(ptr: *mut u8, size: usize, align: usize);
}

/// asmalloc as Rust's allocator. Under Miri, a handle to Miri's allocator.
#[derive(Clone, Copy, Debug, Default)]
pub struct Asmalloc;

impl Asmalloc {
    /// Whether `ptr` lies in the heap. Under Miri there is no heap of asmalloc's.
    pub fn contains(_ptr: *const u8) -> bool {
        false
    }

    /// The usable size of a block of the heap. Under Miri there is none.
    pub fn usable_size(_ptr: *const u8) -> Option<usize> {
        None
    }
}

// SAFETY: MIRI-02. Miri's allocator keeps `GlobalAlloc`'s promises.
unsafe impl GlobalAlloc for Asmalloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: MIRI-01; the caller's layout is valid.
        unsafe { miri_alloc(layout.size(), layout.align()) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: MIRI-01; the caller passes a block `alloc` returned for `layout`.
        unsafe { miri_dealloc(ptr, layout.size(), layout.align()) }
    }
}

#[cfg(feature = "allocator-api")]
// SAFETY: MIRI-02.
unsafe impl core::alloc::Allocator for Asmalloc {
    fn allocate(
        &self,
        layout: Layout,
    ) -> Result<core::ptr::NonNull<[u8]>, core::alloc::AllocError> {
        let ptr = if layout.size() == 0 {
            core::ptr::NonNull::new(core::ptr::without_provenance_mut::<u8>(layout.align()))
        } else {
            // SAFETY: MIRI-01; the size is not zero.
            core::ptr::NonNull::new(unsafe { miri_alloc(layout.size(), layout.align()) })
        };
        ptr.map(|ptr| core::ptr::NonNull::slice_from_raw_parts(ptr, layout.size()))
            .ok_or(core::alloc::AllocError)
    }

    unsafe fn deallocate(&self, ptr: core::ptr::NonNull<u8>, layout: Layout) {
        if layout.size() != 0 {
            // SAFETY: MIRI-01; `allocate` returned the block for this exact layout.
            unsafe { miri_dealloc(ptr.as_ptr(), layout.size(), layout.align()) }
        }
    }
}
