//! `errno`, through the C library's accessor: with glibc's `__rseq_offset` (`front`), and on
//! musl the pthread key calls that retire a thread's own rseq area (`front::area`), asmalloc's
//! only reference to a C library.
//!
//! errno belongs to the C library: its location is a thread-local of the C library's, so the
//! only way to set the value a C caller reads is the library's own `__errno_location`. The `link` attribute
//! also brings in the C library for the compiler's own `memcpy` and `memset` calls, which a
//! `no_std` shared library has no other way to get. The ledger entry is
//! `docs/safety/asmalloc/c/errno-safety.md`.

use core::ffi::c_int;

/// `ENOMEM` on Linux.
pub(crate) const ENOMEM: c_int = 12;
/// `EINVAL` on Linux.
pub(crate) const EINVAL: c_int = 22;

// SAFETY: ERRNO-01. glibc and musl declare `extern int *__errno_location (void)` (`errno.h`); it takes
// nothing and returns a pointer, which `*mut c_int` represents exactly.
#[link(name = "c")]
unsafe extern "C" {
    fn __errno_location() -> *mut c_int;
}

/// The calling thread's `errno`.
#[cfg(any(test, feature = "background"))]
pub(crate) fn get() -> c_int {
    // SAFETY: ERRNO-02.
    unsafe { __errno_location().read() }
}

/// Sets the calling thread's `errno` to `value`.
#[cold]
pub(crate) fn set(value: c_int) {
    // SAFETY: ERRNO-02.
    unsafe { __errno_location().write(value) }
}
