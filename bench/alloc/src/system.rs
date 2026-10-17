//! Rust's `System` allocator, which is glibc's malloc. No `#[global_allocator]` is needed.
use std::ffi::c_void;

pub const NAME: &str = "glibc";

pub fn entry() -> *const c_void {
    libc::malloc as *const c_void
}

pub fn version() -> String {
    // SAFETY: glibc returns a static NUL-terminated string.
    unsafe { std::ffi::CStr::from_ptr(libc::gnu_get_libc_version()) }.to_string_lossy().into_owned()
}
