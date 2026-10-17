//! The simulated OS's memory: one zeroed host allocation that reservations are carved from.

use alloc::alloc::{Layout, alloc_zeroed, dealloc, handle_alloc_error};
use core::ptr::NonNull;

/// A zero-initialized host allocation, freed on drop.
///
/// Every pointer the simulated OS hands out is derived from `base`, so it carries the
/// allocation's provenance, which keeps Miri's strict-provenance checks and Kani's pointer model
/// able to follow the allocator.
pub(super) struct Memory {
    base: NonNull<u8>,
    layout: Layout,
}

// SAFETY: `Memory` owns its allocation outright and exposes it only as raw pointers; it holds no
// references and no thread-affine state, so moving it to another thread or sharing it is
// harmless. Callers synchronize their own accesses through the pointers, as `zero` requires.
unsafe impl Send for Memory {}
// SAFETY: as above.
unsafe impl Sync for Memory {}

impl Memory {
    /// `len` zero bytes aligned to `align`. Panics if `len` is zero or the layout is invalid,
    /// and reports allocation failure as the global allocator does.
    pub(super) fn new(len: usize, align: usize) -> Memory {
        assert!(len > 0, "simulated memory must not be empty");
        let layout = Layout::from_size_align(len, align).expect("simulated memory layout");
        // SAFETY: the layout's size is not zero.
        let ptr = unsafe { alloc_zeroed(layout) };
        let Some(base) = NonNull::new(ptr) else {
            handle_alloc_error(layout)
        };
        Memory { base, layout }
    }

    pub(super) fn len(&self) -> usize {
        self.layout.size()
    }

    /// The pointer `offset` bytes in. Pure address arithmetic; the result is only valid to
    /// dereference if `offset` is in bounds.
    pub(super) fn at(&self, offset: usize) -> NonNull<u8> {
        self.base.map_addr(|base| base.saturating_add(offset))
    }

    /// The offset of `ptr` from the start, whether or not it is in bounds.
    pub(super) fn offset_of(&self, ptr: NonNull<u8>) -> usize {
        ptr.addr().get().wrapping_sub(self.base.addr().get())
    }

    /// Writes `len` zero bytes at `offset`.
    ///
    /// # Safety
    ///
    /// `offset + len <= self.len()`, and no other thread reads or writes those bytes during the
    /// call.
    pub(super) unsafe fn zero(&self, offset: usize, len: usize) {
        debug_assert!(offset <= self.len() && len <= self.len() - offset);
        // SAFETY: the range is inside the allocation (caller), `base` has provenance for all of
        // it, `u8` has no alignment requirement and no invalid values, and the caller promises
        // that nothing else accesses the bytes meanwhile.
        unsafe { self.base.as_ptr().add(offset).write_bytes(0, len) }
    }
}

impl Drop for Memory {
    fn drop(&mut self) {
        // SAFETY: `base` came from `alloc_zeroed` with this exact layout and is freed only here.
        unsafe { dealloc(self.base.as_ptr(), self.layout) }
    }
}
