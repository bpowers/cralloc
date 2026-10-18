//! The generated C entry points (`front/generated/x86_64/centry.rs`), callable from Rust:
//! `malloc`, `free` and `free_sized` as the exported C symbols run them, for the tests and the
//! conformance suite, which cannot export `malloc` from their own processes. Each is a call of
//! the routine; the exported symbols are the routines themselves (`export_c_abi!`). The ledger
//! entry is `docs/safety/asmalloc/c/routines-safety.md`.

use crate::front::generated_centry as centry;
use core::ffi::c_void;
use core::ptr;

/// The generated `malloc`.
pub extern "C" fn malloc(size: usize) -> *mut c_void {
    // SAFETY: ROUTINES-01.
    ptr::with_exposed_provenance_mut(unsafe { centry::c_malloc(size) })
}

/// The generated `free`.
///
/// # Safety
///
/// As for [`super::free`].
pub unsafe extern "C" fn free(ptr: *mut c_void) {
    // SAFETY: ROUTINES-01; the pointer's provenance is exposed for the push to take it back.
    unsafe { centry::c_free(ptr.expose_provenance()) }
}

/// The generated `free_sized`.
///
/// # Safety
///
/// As for [`super::free`]. A wrong `size` is safe.
pub unsafe extern "C" fn free_sized(ptr: *mut c_void, size: usize) {
    // SAFETY: as for `free`.
    unsafe { centry::c_free_sized(ptr.expose_provenance(), size) }
}
