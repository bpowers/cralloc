//! mimalloc, as the `mimalloc` crate 0.1.52 calls it. With `mi-sized` the free passes the
//! layout back. (mimalloc v3 ignores the size in release builds, so the two differ only in
//! the call path; the comparison shows whether that matters.)
use std::alloc::{GlobalAlloc, Layout};
use std::ffi::c_void;

unsafe extern "C" {
    fn mi_malloc_aligned(size: usize, align: usize) -> *mut c_void;
    fn mi_zalloc_aligned(size: usize, align: usize) -> *mut c_void;
    fn mi_realloc_aligned(p: *mut c_void, new_size: usize, align: usize) -> *mut c_void;
    #[cfg(not(feature = "mi-sized"))]
    fn mi_free(p: *mut c_void);
    #[cfg(feature = "mi-sized")]
    fn mi_free_size_aligned(p: *mut c_void, size: usize, align: usize);
    fn mi_version() -> std::ffi::c_int;
}

pub const NAME: &str = if cfg!(feature = "mi-sized") { "mimalloc-sized" } else { "mimalloc" };

pub fn entry() -> *const c_void {
    mi_malloc_aligned as *const c_void
}

pub fn version() -> String {
    // SAFETY: a query with no preconditions.
    unsafe { mi_version() }.to_string()
}

pub struct Mimalloc;

// SAFETY: forwards to mimalloc, which meets the GlobalAlloc contract for these calls.
unsafe impl GlobalAlloc for Mimalloc {
    #[inline]
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        unsafe { mi_malloc_aligned(l.size(), l.align()).cast() }
    }
    #[inline]
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        unsafe { mi_zalloc_aligned(l.size(), l.align()).cast() }
    }
    #[inline]
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        #[cfg(not(feature = "mi-sized"))]
        unsafe {
            let _ = l;
            mi_free(p.cast())
        }
        #[cfg(feature = "mi-sized")]
        unsafe {
            mi_free_size_aligned(p.cast(), l.size(), l.align())
        }
    }
    #[inline]
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new_size: usize) -> *mut u8 {
        unsafe { mi_realloc_aligned(p.cast(), new_size, l.align()).cast() }
    }
}
