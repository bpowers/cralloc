//! The `mimalloc` crate itself as the global allocator. Its mimalloc is the one
//! libmimalloc-sys bundles; `version()` tells it apart from third_party's.
use std::ffi::{c_int, c_void};

unsafe extern "C" {
    fn mi_malloc_aligned(size: usize, align: usize) -> *mut c_void;
    fn mi_version() -> c_int;
}

pub const NAME: &str = "mimalloc";

pub fn entry() -> *const c_void {
    mi_malloc_aligned as *const c_void
}

pub fn version() -> String {
    // SAFETY: a query with no preconditions.
    unsafe { mi_version() }.to_string()
}
