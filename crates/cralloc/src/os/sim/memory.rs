//! The simulated OS's memory: one zeroed range that reservations are carved from.
//!
//! Natively, on Linux, the memory a `SimOs` asks for is *guarded*: a real reservation from
//! [`LinuxOs`], inaccessible until the simulator commits it, so a read or write of memory the
//! allocator never committed faults, as it would in a real process. A write that the program later
//! undoes (a bitmap bit set and cleared again) leaves no trace in the contents, so only a fault
//! catches it. Under Miri and Kani, which cannot change page protections, the memory is an
//! ordinary zeroed allocation, and the simulator checks contents instead.

#[cfg(cralloc_real)]
use crate::os::Os;
#[cfg(cralloc_real)]
use crate::os::linux::{LinuxOs, protect_none};
use alloc::alloc::{Layout, alloc_zeroed, dealloc, handle_alloc_error};
use core::ptr::NonNull;

/// A zeroed range of memory, released on drop.
///
/// Every pointer the simulated OS hands out is derived from `base`, so it carries the range's
/// provenance, which keeps Miri's strict-provenance checks and Kani's pointer model able to
/// follow the allocator.
pub(super) struct Memory {
    base: NonNull<u8>,
    layout: Layout,
    guarded: bool,
}

// SAFETY: `Memory` owns its range outright and exposes it only as raw pointers; it holds no
// references and no thread-affine state, so moving it to another thread or sharing it is
// harmless. Callers synchronize their own accesses through the pointers, as `zero` requires.
unsafe impl Send for Memory {}
// SAFETY: as above.
unsafe impl Sync for Memory {}

impl Memory {
    /// `len` zero bytes aligned to `align`, guarded if `guard` is set and the platform allows it.
    /// Panics if `len` is zero, the layout is invalid, or the memory cannot be had.
    pub(super) fn new(len: usize, align: usize, guard: bool) -> Memory {
        assert!(len > 0, "simulated memory must not be empty");
        let layout = Layout::from_size_align(len, align).expect("simulated memory layout");
        #[cfg(cralloc_real)]
        if guard {
            let base = LinuxOs
                .reserve(len.next_multiple_of(4096), align.max(4096))
                .expect("guarded simulated memory");
            return Memory {
                base,
                layout,
                guarded: true,
            };
        }
        let _ = guard;
        // SAFETY: the layout's size is not zero.
        let ptr = unsafe { alloc_zeroed(layout) };
        let Some(base) = NonNull::new(ptr) else {
            handle_alloc_error(layout)
        };
        Memory {
            base,
            layout,
            guarded: false,
        }
    }

    pub(super) fn len(&self) -> usize {
        self.layout.size()
    }

    /// Whether memory the simulator has not committed faults when touched.
    pub(super) fn guarded(&self) -> bool {
        self.guarded
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

    /// Makes `len` bytes at `offset` readable and writable, if the memory is guarded. The range
    /// is whole 4 KiB host pages: the simulator's pages are whole host pages when guarded.
    ///
    /// # Safety
    ///
    /// `offset + len <= self.len()`.
    pub(super) unsafe fn open(&self, offset: usize, len: usize) {
        #[cfg(cralloc_real)]
        if self.guarded && len != 0 {
            // SAFETY: the range is inside this value's own `LinuxOs` reservation (caller), and
            // whole host pages (the simulator's page size is a multiple of 4 KiB when guarded).
            let opened = unsafe { LinuxOs.commit(self.at(offset), len) };
            opened.expect("guarded simulated memory: mprotect");
        }
        let _ = (offset, len);
    }

    /// Makes `len` bytes at `offset` inaccessible again, if the memory is guarded.
    ///
    /// # Safety
    ///
    /// `offset + len <= self.len()`, and nothing reads or writes the range until it is opened
    /// again.
    pub(super) unsafe fn close(&self, offset: usize, len: usize) {
        #[cfg(cralloc_real)]
        if self.guarded && len != 0 {
            // SAFETY: as in `open`; the caller promises nothing touches the range meanwhile.
            let closed = unsafe { protect_none(self.at(offset), len) };
            closed.expect("guarded simulated memory: mprotect");
        }
        let _ = (offset, len);
    }

    /// Makes `len` bytes at `offset` read as zero.
    ///
    /// # Safety
    ///
    /// `offset + len <= self.len()`, and no other thread reads or writes those bytes during the
    /// call.
    pub(super) unsafe fn zero(&self, offset: usize, len: usize) {
        debug_assert!(offset <= self.len() && len <= self.len() - offset);
        #[cfg(cralloc_real)]
        if self.guarded {
            if len != 0 {
                // SAFETY: the range is inside this value's own reservation and whole host pages;
                // the caller promises nothing touches it, so discarding its contents is safe.
                // MADV_DONTNEED zero-fills whether or not the pages are accessible.
                let zeroed = unsafe { LinuxOs.purge(self.at(offset), len) };
                zeroed.expect("guarded simulated memory: madvise");
            }
            return;
        }
        // SAFETY: the range is inside the allocation (caller), `base` has provenance for all of
        // it, `u8` has no alignment requirement and no invalid values, and the caller promises
        // that nothing else accesses the bytes meanwhile.
        unsafe { self.base.as_ptr().add(offset).write_bytes(0, len) }
    }

    /// Whether the `len` bytes at `offset` are all zero.
    ///
    /// # Safety
    ///
    /// `offset + len <= self.len()`, the range is accessible (always, unless the memory is
    /// guarded, when it must be open), every byte in it is initialized, and no thread writes
    /// those bytes during the call.
    pub(super) unsafe fn is_zero(&self, offset: usize, len: usize) -> bool {
        debug_assert!(offset <= self.len() && len <= self.len() - offset);
        // SAFETY: the range is inside the memory, accessible and initialized (caller); `base`
        // has provenance for all of it; and nothing writes it while the shared slice lives
        // (caller).
        let bytes = unsafe { core::slice::from_raw_parts(self.base.as_ptr().add(offset), len) };
        // Byte slices compare with one `memcmp`, which Miri runs natively. A loop over the
        // words was interpreted step by step and cost Miri tens of minutes for each test that
        // commits a whole simulated heap, since every commit checks its pages.
        static ZEROS: [u8; 4096] = [0; 4096];
        bytes
            .chunks(ZEROS.len())
            .all(|chunk| chunk == &ZEROS[..chunk.len()])
    }
}

impl Drop for Memory {
    fn drop(&mut self) {
        #[cfg(cralloc_real)]
        if self.guarded {
            let len = self.layout.size().next_multiple_of(4096);
            // SAFETY: the whole reservation `new` made, released only here; nothing uses it
            // after the simulator is dropped.
            let _ = unsafe { LinuxOs.unmap(self.base, len) };
            return;
        }
        // SAFETY: `base` came from `alloc_zeroed` with this exact layout and is freed only here.
        unsafe { dealloc(self.base.as_ptr(), self.layout) }
    }
}
