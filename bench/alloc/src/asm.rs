//! asmalloc, used as its users use it: `asmalloc::Asmalloc` as the global allocator.
use std::alloc::{GlobalAlloc, Layout};
use std::ffi::c_void;

pub const NAME: &str = "asmalloc";

pub fn entry() -> *const c_void {
    <asmalloc::Asmalloc as GlobalAlloc>::alloc as unsafe fn(&asmalloc::Asmalloc, Layout) -> *mut u8 as *const c_void
}

pub fn version() -> String {
    String::new()
}

/// Whether asmalloc really serves this process's allocations: a fresh `Box` must be on its
/// heap. The executable holds asmalloc's code whether or not it is the global allocator, so
/// the library check alone cannot tell.
pub fn serves() -> bool {
    let b = Box::new(0u64);
    asmalloc::Asmalloc::contains(std::ptr::from_ref(&*b).cast())
}
