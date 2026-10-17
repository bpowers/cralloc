//! The C layer's fast path, in one place so that the generated C routines can replace it.
//!
//! `docs/research/zen5-roofline.md` section 14 settles what will sit here: generated
//! `c_malloc`, `c_free` and `c_calloc` routines, each exported as the C symbol itself, that
//! find the class from a table or the region table and leave by `jmp` to the slow paths in
//! `slow.rs` with their own arguments. Until they exist, the front halves below do the same in
//! Rust: find the small class, run the existing out-of-line pop or push (which completes its
//! own magazine misses), and send everything else to the same slow paths. The ledger entry is
//! `docs/safety/asmalloc/c/seam-safety.md`.

use super::slow;
use super::table;
use crate::Asmalloc;
use crate::classes::SmallClass;
use crate::heap::{Found, process};
use core::ffi::c_void;
use core::ptr::NonNull;

/// An object of `class`, by the out-of-line pop and, when it misses, the heap's slow path.
/// Null if there is no memory; errno is not set.
#[inline(always)]
pub(super) fn alloc_small(class: SmallClass) -> *mut u8 {
    Asmalloc::alloc_small(class)
}

/// Frees an object of `class`, by the out-of-line push and, when it misses, the heap's slow
/// path.
///
/// # Safety
///
/// `object` starts a slot of `class` that the heap handed out, which the caller owns and gives
/// up.
#[inline(always)]
pub(super) unsafe fn free_small(class: SmallClass, object: NonNull<u8>) {
    // SAFETY: SEAM-01.
    unsafe { Asmalloc::free_small(class, object) }
}

/// `malloc`: a small class from the size table and a pop; anything else is the slow path's.
#[inline(always)]
pub(super) fn malloc(size: usize) -> *mut c_void {
    if let Some(class) = table::malloc_class(size) {
        let object = alloc_small(class);
        if !object.is_null() {
            return object.cast();
        }
    }
    slow::malloc_slow(size)
}

/// `free`: a small class from the region table and a push; anything else (null, a pointer
/// that is not the heap's, a large block) is the slow path's.
///
/// # Safety
///
/// `ptr` is null or a block of the heap that the caller owns and gives up.
#[inline(always)]
pub(super) unsafe fn free(ptr: *mut c_void) {
    if let Some(object) = NonNull::new(ptr.cast::<u8>())
        && let Some(Found::Small(class)) = process::find(object.as_ptr())
    {
        // SAFETY: SEAM-02.
        return unsafe { free_small(class, object) };
    }
    // SAFETY: passed through from the caller.
    unsafe { slow::free_slow(ptr) }
}

/// `calloc`: for totals up to the calloc table's end, a pop and the zeroing tail; anything
/// else, an overflow included, is the slow path's.
#[inline(always)]
pub(super) fn calloc(n: usize, size: usize) -> *mut c_void {
    let Some(total) = n.checked_mul(size) else {
        // A total no block can have: the slow path fails it with ENOMEM.
        return slow::calloc_slow(usize::MAX);
    };
    if let Some(class) = table::calloc_class(total) {
        let object = alloc_small(class);
        if !object.is_null() {
            // SAFETY: SEAM-03.
            return unsafe { slow::calloc_zero(object.cast(), total) };
        }
    }
    slow::calloc_slow(total)
}
