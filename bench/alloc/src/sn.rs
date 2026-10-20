//! The `snmalloc-rs` crate itself as the global allocator, default features: the snmalloc
//! that snmalloc-sys bundles, built with cmake, called with sizes on free and realloc.
use std::ffi::c_void;

unsafe extern "C" {
    // snmalloc-sys's entry for Rust's `alloc`, which `snmalloc_rs::SnMalloc` calls; naming it
    // here also checks at link time that the bundled build is the one linked.
    fn sn_rust_alloc(alignment: usize, size: usize) -> *mut c_void;
}

pub const NAME: &str = "snmalloc";

pub fn entry() -> *const c_void {
    sn_rust_alloc as *const c_void
}

/// snmalloc reports no version at run time; the lock file pins the crate, which pins the
/// bundled source.
pub fn version() -> String {
    String::new()
}
