//! google/tcmalloc, as `tcmalloc-better` 0.1.19 calls it (aligned nothrow `new`, sized and
//! aligned `delete`, the generic zeroing `alloc_zeroed`), with an in-place `realloc` where
//! that crate copies every time.
//!
//! Everything stays in the `new`/`delete` family. tcmalloc records which family made a
//! sampled allocation and aborts on a mismatched free, so `calloc` and `realloc`, which are
//! `malloc`'s family, cannot be paired with a sized `delete`.
//!
//! A sized delete takes the size class from the size it is given, so a block may only stay
//! in place on `realloc` when the new size maps to the block's current class; otherwise
//! the next `dealloc`, which passes the new size, would return it to the wrong class. That
//! is why `realloc` here is not tcmalloc's C `realloc`, which keeps a block in place when it
//! shrinks by less than half.
use std::alloc::{GlobalAlloc, Layout};
use std::ffi::{c_int, c_void};

unsafe extern "C" {
    // The third argument is `const std::nothrow_t&`, an empty type that is never read.
    fn TCMallocInternalNewAlignedNothrow(size: usize, align: usize, nothrow: *const u8) -> *mut c_void;
    fn TCMallocInternalDeleteSizedAligned(p: *mut c_void, size: usize, align: usize);
    fn TCMallocInternalMallocSize(p: *mut c_void) -> usize;
    fn nallocx(size: usize, flags: c_int) -> usize;
    fn MallocExtension_Internal_GetPerCpuCachesActive() -> bool;
}

static NOTHROW: u8 = 0;
const MIN_ALIGN: usize = 16;

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
// the module comment for why realloc keeps a block only when its class is unchanged.
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
        let usable = unsafe { TCMallocInternalMallocSize(p.cast()) };
        if l.align() <= MIN_ALIGN && new_size <= usable && unsafe { nallocx(new_size, 0) } == usable {
            return p;
        }
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
