//! cralloc, used as its users use it: `cralloc::Cralloc` as the global allocator.
use std::alloc::{GlobalAlloc, Layout};
use std::ffi::c_void;

pub const NAME: &str = "cralloc";

pub fn entry() -> *const c_void {
    <cralloc::Cralloc as GlobalAlloc>::alloc as unsafe fn(&cralloc::Cralloc, Layout) -> *mut u8 as *const c_void
}

pub fn version() -> String {
    String::new()
}

/// Whether cralloc really serves this process's allocations: a fresh `Box` must be on its
/// heap. The executable holds cralloc's code whether or not it is the global allocator, so
/// the library check alone cannot tell.
pub fn serves() -> bool {
    let b = Box::new(0u64);
    cralloc::Cralloc::contains(std::ptr::from_ref(&*b).cast())
}
