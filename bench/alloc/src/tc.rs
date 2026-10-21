//! google/tcmalloc, as `tcmalloc-better` 0.1.19 calls it (aligned nothrow `new`, sized and
//! aligned `delete`, the generic zeroing `alloc_zeroed`, and a `realloc` that copies every
//! time).
//!
//! Everything stays in the `new`/`delete` family. tcmalloc records which family made a
//! sampled allocation and aborts on a mismatched free, so `calloc` and `realloc`, which are
//! `malloc`'s family, cannot be paired with a sized `delete`.
//!
//! `realloc` never keeps a block in place. tcmalloc records the requested size of a sampled
//! allocation and aborts when a sized delete passes another: less after a block shrank in
//! place (ruff and boa, which shrink buffers with `shrink_to_fit`: "Mismatched-size-delete of
//! 9948 bytes (expected 12288 bytes)"), and more after one grew in place within its class
//! (Meilisearch, whose tokenizer grows a 2-byte string to 8: "of 8 bytes (expected 2 bytes)",
//! in every start; with copying, in none). A block cannot be known to be unsampled from
//! outside, so the in-place growth this wrapper had until 2026-10-21 is gone, and `realloc` is
//! the crate's again.
use std::alloc::{GlobalAlloc, Layout};
use std::ffi::c_void;

unsafe extern "C" {
    // The third argument is `const std::nothrow_t&`, an empty type that is never read.
    fn TCMallocInternalNewAlignedNothrow(size: usize, align: usize, nothrow: *const u8) -> *mut c_void;
    fn TCMallocInternalDeleteSizedAligned(p: *mut c_void, size: usize, align: usize);
    fn MallocExtension_Internal_GetPerCpuCachesActive() -> bool;
}

static NOTHROW: u8 = 0;

pub const NAME: &str = "tcmalloc";

pub fn entry() -> *const c_void {
    TCMallocInternalNewAlignedNothrow as *const c_void
}

/// tcmalloc has no version of its own; build.sh records the commit.
pub fn version() -> String {
    String::new()
}

pub fn per_cpu_caches_active() -> bool {
    // SAFETY: a query with no preconditions.
    unsafe { MallocExtension_Internal_GetPerCpuCachesActive() }
}

pub struct Tcmalloc;

// SAFETY: forwards to tcmalloc, which meets the GlobalAlloc contract for these calls; see
// the module comment for why realloc always copies.
unsafe impl GlobalAlloc for Tcmalloc {
    #[inline]
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        unsafe { TCMallocInternalNewAlignedNothrow(l.size(), l.align(), &NOTHROW).cast() }
    }
    #[inline]
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { TCMallocInternalDeleteSizedAligned(p.cast(), l.size(), l.align()) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: the caller guarantees new_size is a valid size for this alignment.
        let new_l = unsafe { Layout::from_size_align_unchecked(new_size, l.align()) };
        let q = unsafe { self.alloc(new_l) };
        if !q.is_null() {
            unsafe {
                std::ptr::copy_nonoverlapping(p, q, l.size().min(new_size));
                self.dealloc(p, l);
            }
        }
        q
    }
}
