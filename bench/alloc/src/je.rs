//! jemalloc, as `tikv-jemallocator` 0.6 calls it: plain `malloc`, `calloc` and `realloc`
//! when the alignment is what malloc guarantees anyway, the `*allocx` forms otherwise, and
//! always the sized `sdallocx` to free.
use std::alloc::{GlobalAlloc, Layout};
use std::ffi::{c_int, c_void};

unsafe extern "C" {
    #[link_name = "malloc"]
    fn je_malloc(size: usize) -> *mut c_void;
    #[link_name = "calloc"]
    fn je_calloc(n: usize, size: usize) -> *mut c_void;
    #[link_name = "realloc"]
    fn je_realloc(p: *mut c_void, size: usize) -> *mut c_void;
    fn mallocx(size: usize, flags: c_int) -> *mut c_void;
    fn rallocx(p: *mut c_void, size: usize, flags: c_int) -> *mut c_void;
    fn sdallocx(p: *mut c_void, size: usize, flags: c_int);
}

/// alignof(max_align_t) on x86-64.
const MIN_ALIGN: usize = 16;
const MALLOCX_ZERO: c_int = 0x40;

/// MALLOCX_LG_ALIGN when the alignment is more than malloc gives for this size, else 0.
#[inline(always)]
fn flags(align: usize, size: usize) -> c_int {
    if align <= MIN_ALIGN && align <= size { 0 } else { align.trailing_zeros() as c_int }
}

pub struct Jemalloc;

// SAFETY: forwards to jemalloc, which meets the GlobalAlloc contract for these calls.
unsafe impl GlobalAlloc for Jemalloc {
    #[inline]
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let f = flags(l.align(), l.size());
        unsafe { if f == 0 { je_malloc(l.size()) } else { mallocx(l.size(), f) }.cast() }
    }
    #[inline]
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        let f = flags(l.align(), l.size());
        unsafe { if f == 0 { je_calloc(1, l.size()) } else { mallocx(l.size(), f | MALLOCX_ZERO) }.cast() }
    }
    #[inline]
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { sdallocx(p.cast(), l.size(), flags(l.align(), l.size())) }
    }
    #[inline]
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new_size: usize) -> *mut u8 {
        let f = flags(l.align(), new_size);
        unsafe { if f == 0 { je_realloc(p.cast(), new_size) } else { rallocx(p.cast(), new_size, f) }.cast() }
    }
}
