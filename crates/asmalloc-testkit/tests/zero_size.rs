//! C's `malloc(0)` returns null or a distinct object; one that hands the same pointer to two
//! live zero-size requests is caught, though the model never reads or writes such a block.
#![cfg(not(miri))]

use core::ffi::c_void;
use core::ptr;
use std::sync::atomic::{AtomicPtr, Ordering};

use asmalloc_testkit::subject::{CFns, ViaC};
use asmalloc_testkit::{Config, FailureKind, Profile, run};

static SHARED: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());

/// `malloc`, except that every zero-size request gets the same block.
unsafe extern "C" fn malloc_sharing_zero(size: usize) -> *mut c_void {
    if size > 0 {
        // SAFETY: the C library's own function.
        return unsafe { (CFns::LIBC.malloc)(size) };
    }
    let p = SHARED.load(Ordering::Relaxed);
    if !p.is_null() {
        return p;
    }
    // SAFETY: as above; the block is never freed, so handing it out again stays memory-safe.
    let fresh = unsafe { (CFns::LIBC.malloc)(8) };
    SHARED.store(fresh, Ordering::Relaxed);
    fresh
}

/// `free`, except that the shared zero-size block is kept.
unsafe extern "C" fn free_keeping_zero(p: *mut c_void) {
    if p != SHARED.load(Ordering::Relaxed) {
        // SAFETY: any other pointer came from the C library.
        unsafe { (CFns::LIBC.free)(p) }
    }
}

#[test]
fn a_zero_size_pointer_handed_out_twice_is_an_overlap() {
    let s = ViaC(CFns {
        malloc: malloc_sharing_zero,
        free: free_keeping_zero,
        sized_free: None,
        ..CFns::LIBC
    });
    let f = run(&s, &Config::single(Profile::SMALL_FIXED, 1, 20_000))
        .expect_err("two live zero-size blocks share an address");
    assert_eq!(f.kind, FailureKind::Overlap, "{f}");
}
