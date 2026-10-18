//! `errno`, through the C library's accessor. With the loader's lookups in `loader.rs`, these
//! are the only references to a C library in asmalloc.
//!
//! errno belongs to the C library: its location is a thread-local of glibc's, so the only way
//! to set the value a C caller reads is glibc's own `__errno_location`. The `link` attribute
//! also brings in the C library for the compiler's own `memcpy` and `memset` calls, which a
//! `no_std` shared library has no other way to get. The ledger entry is
//! `docs/safety/asmalloc/c/errno-safety.md`.

use core::ffi::c_int;

/// `ENOMEM` on Linux.
pub(crate) const ENOMEM: c_int = 12;
/// `EINVAL` on Linux.
pub(crate) const EINVAL: c_int = 22;

// SAFETY: ERRNO-01. glibc declares `extern int *__errno_location (void)` (`errno.h`); it takes
// nothing and returns a pointer, which `*mut c_int` represents exactly.
#[link(name = "c")]
unsafe extern "C" {
    fn __errno_location() -> *mut c_int;
}

/// Sets the calling thread's `errno` to `value`.
#[cold]
pub(crate) fn set(value: c_int) {
    // SAFETY: ERRNO-02.
    unsafe { __errno_location().write(value) }
}
