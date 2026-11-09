//! The simulated OS's memory: one zeroed range that reservations are carved from.
//!
//! Natively, on Linux, the memory a `SimOs` asks for is a real reservation from [`LinuxOs`], which
//! the host commits only as it is touched. A shape whose pages are whole host pages gets it
//! *guarded*: inaccessible until the simulator commits it, so a read or write of memory the
//! allocator never committed faults, as it would in a real process. A write that the program later
//! undoes (a bitmap bit set and cleared again) leaves no trace in the contents, so only a fault
//! catches it. A shape with smaller pages than the host's cannot change the protection of one of
//! them, so its memory is all accessible, and the simulator checks contents instead, as it does
//! under Miri and Kani, which cannot change page protections and get an ordinary zeroed
//! allocation.

#[cfg(cralloc_real)]
use crate::os::Os;
#[cfg(cralloc_real)]
use crate::os::linux::{LinuxOs, protect_none};
use alloc::alloc::Layout;
#[cfg(not(cralloc_real))]
use alloc::alloc::{alloc_zeroed, dealloc, handle_alloc_error};
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

/// The host's page size.
#[cfg(cralloc_real)]
fn host_page() -> usize {
    LinuxOs::geometry().page_size()
}

/// Whether `len` bytes of address space aligned to `align` can be had now.
#[cfg(cralloc_real)]
pub(super) fn room_for(len: usize, align: usize) -> bool {
    let host = host_page();
    let whole = len.next_multiple_of(host);
    let Ok(base) = LinuxOs.reserve(whole, align.max(host)) else {
        return false;
    };
    // SAFETY: the reservation just made, whole, that nothing else knows of.
    unsafe { LinuxOs.unmap(base, whole) }.is_ok()
}

/// Where the memory is an ordinary allocation, a shape's address space is its memory.
#[cfg(not(cralloc_real))]
pub(super) fn room_for(_len: usize, _align: usize) -> bool {
    false
}

impl Memory {
    /// `len` zero bytes aligned to `align`, for a shape whose pages are `page` bytes: guarded if
    /// those are whole host pages and the platform allows it. Panics if `len` is zero, the layout
    /// is invalid, or the memory cannot be had.
    #[cfg(cralloc_real)]
    pub(super) fn new(len: usize, align: usize, page: usize) -> Memory {
        assert!(len > 0, "simulated memory must not be empty");
        let layout = Layout::from_size_align(len, align).expect("simulated memory layout");
        let host = host_page();
        let whole = len.next_multiple_of(host);
        let base = LinuxOs
            .reserve(whole, align.max(host))
            .expect("simulated memory");
        let guarded = page.is_multiple_of(host);
        if !guarded {
            // SAFETY: the whole of a reservation just made, that nothing else knows of.
            let opened = unsafe { LinuxOs.commit(base, whole) };
            opened.expect("simulated memory: mprotect");
        }
        Memory {
            base,
            layout,
            guarded,
        }
    }

    /// `len` zero bytes aligned to `align`. Panics if `len` is zero or the layout is invalid.
    #[cfg(not(cralloc_real))]
    pub(super) fn new(len: usize, align: usize, _page: usize) -> Memory {
        assert!(len > 0, "simulated memory must not be empty");
        let layout = Layout::from_size_align(len, align).expect("simulated memory layout");
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
    /// is whole host pages: the simulator's pages are whole host pages when guarded.
    ///
    /// # Safety
    ///
    /// `offset + len <= self.len()`.
    pub(super) unsafe fn open(&self, offset: usize, len: usize) {
        #[cfg(cralloc_real)]
        if self.guarded && len != 0 {
            // SAFETY: the range is inside this value's own `LinuxOs` reservation (caller), and
            // whole host pages (the simulator's page size is a multiple of the host's when
            // guarded).
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
        if len != 0 {
            // The host pages wholly inside the range go back, which costs no memory where
            // writing zeros would commit them, and MADV_DONTNEED zero-fills whether or not they
            // are accessible. What is left at either end shares a host page with a simulated page
            // outside the range, so only unguarded memory has any: it is accessible.
            let host = host_page();
            let end = offset + len;
            let (first, last) = (offset.next_multiple_of(host), end & !(host - 1));
            if first < last {
                // SAFETY: the range is inside this value's own reservation and whole host pages;
                // the caller promises nothing touches it, so discarding its contents is safe.
                let zeroed = unsafe { LinuxOs.purge(self.at(first), last - first) };
                zeroed.expect("simulated memory: madvise");
                // SAFETY: as below, for the two ends of the range.
                unsafe {
                    self.base
                        .as_ptr()
                        .add(offset)
                        .write_bytes(0, first - offset);
                    self.base.as_ptr().add(last).write_bytes(0, end - last);
                }
            } else {
                // SAFETY: as below.
                unsafe { self.base.as_ptr().add(offset).write_bytes(0, len) };
            }
        }
        // SAFETY: the range is inside the allocation (caller), `base` has provenance for all of
        // it, `u8` has no alignment requirement and no invalid values, and the caller promises
        // that nothing else accesses the bytes meanwhile.
        #[cfg(not(cralloc_real))]
        unsafe {
            self.base.as_ptr().add(offset).write_bytes(0, len)
        }
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
    #[cfg(cralloc_real)]
    fn drop(&mut self) {
        let len = self.layout.size().next_multiple_of(host_page());
        // SAFETY: the whole reservation `new` made, released only here; nothing uses it after
        // the simulator is dropped.
        let _ = unsafe { LinuxOs.unmap(self.base, len) };
    }

    #[cfg(not(cralloc_real))]
    fn drop(&mut self) {
        // SAFETY: `base` came from `alloc_zeroed` with this exact layout and is freed only here.
        unsafe { dealloc(self.base.as_ptr(), self.layout) }
    }
}
