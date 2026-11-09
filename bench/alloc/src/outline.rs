//! cralloc's fast path out of line: the same allocator behind calls the optimizer may not inline,
//! at a cost that is part of what it measures. A call carries the size at run time, so it has no
//! constant-size path, and it takes up no room at the caller. For `docs/research/firestorm.md`,
//! "What inlining costs a program".
use cralloc::Cralloc;
use std::alloc::{GlobalAlloc, Layout};

pub struct Outline;

// SAFETY: each method passes its arguments to `Cralloc`'s, under the same contract.
unsafe impl GlobalAlloc for Outline {
    #[inline(never)]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller upholds `GlobalAlloc::alloc`'s contract.
        unsafe { Cralloc.alloc(layout) }
    }

    #[inline(never)]
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the caller upholds `GlobalAlloc::dealloc`'s contract.
        unsafe { Cralloc.dealloc(ptr, layout) }
    }

    #[inline(never)]
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller upholds `GlobalAlloc::alloc_zeroed`'s contract.
        unsafe { Cralloc.alloc_zeroed(layout) }
    }

    #[inline(never)]
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: the caller upholds `GlobalAlloc::realloc`'s contract.
        unsafe { Cralloc.realloc(ptr, layout, new_size) }
    }
}
