//! tikv-jemallocator's own jemalloc, which this crate uses as the global allocator unchanged.
//! Its symbols carry the `_rjem_` prefix that tikv-jemalloc-sys configures, so naming them
//! here also checks at link time that the prefixed static build is the one linked.
use std::ffi::{c_char, c_int, c_void};

unsafe extern "C" {
    fn _rjem_mallocx(size: usize, flags: c_int) -> *mut c_void;
    fn _rjem_mallctl(
        name: *const c_char,
        old: *mut c_void,
        oldlen: *mut usize,
        new: *mut c_void,
        newlen: usize,
    ) -> c_int;
}

pub const NAME: &str = "jemalloc";

pub fn entry() -> *const c_void {
    _rjem_mallocx as *const c_void
}

pub fn version() -> String {
    // SAFETY: mallctl with the signature jemalloc documents.
    unsafe { crate::je_version(_rjem_mallctl) }
}
