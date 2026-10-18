//! The slow paths of `malloc`, `free` and `calloc`, each of which completes its call from
//! scratch, and the zeroing tail of `calloc`.
//!
//! They take the C function's own arguments and are `extern "C"`, so that a generated routine
//! that misses can leave for one with a `jmp` and its argument registers intact
//! (`docs/research/zen5-roofline.md` section 14): `malloc_slow(size)` for any size, `free_slow(ptr)`
//! for any pointer, `calloc_slow(total)` for any total (`usize::MAX` for one that overflowed),
//! and `calloc_zero(block, total)` after a routine popped a block; and for the sized frees,
//! `free_sized_slow(ptr, size)` and `free_aligned_sized_slow(ptr, align, size)`, which complete
//! one whatever the size says. The ledger entry is
//! `docs/safety/asmalloc/c/slow-safety.md`.

use super::{errno, seam, table};
use crate::classes::Class;
use crate::heap::{Found, process};
use core::alloc::Layout;
use core::ffi::c_void;
use core::ptr::{self, NonNull};

/// A block of at least `size` bytes aligned to `align`, a power of two, by C's rules (16 bytes
/// for a block of 16 or more, whatever is asked). With `zero`, all of it that
/// `malloc_usable_size` reports reads as zero, as glibc's `calloc` makes it: a program that
/// sizes a block by its usable size may read the slack. Null if there is no memory, or if no
/// block can have that size and alignment; errno is not set.
pub(super) fn allocate(size: usize, align: usize, zero: bool) -> *mut u8 {
    let Ok(layout) = Layout::from_size_align(size, align) else {
        return ptr::null_mut();
    };
    match Class::of_c(layout).and_then(Class::small) {
        Some(class) => {
            let object = seam::alloc_small(class);
            if zero && !object.is_null() {
                // SAFETY: SLOW-01.
                unsafe { object.write_bytes(0, class.size()) };
            }
            object
        }
        None => process::alloc_large(size, align, zero),
    }
}

/// `p`, or null with errno set to `ENOMEM` if `p` is null.
#[inline]
pub(super) fn or_enomem(p: *mut u8) -> *mut c_void {
    if p.is_null() {
        errno::set(errno::ENOMEM);
    }
    p.cast()
}

/// Completes `malloc(size)`.
#[cold]
#[inline(never)]
pub(crate) extern "C" fn malloc_slow(size: usize) -> *mut c_void {
    or_enomem(allocate(size, 1, false))
}

/// Completes a `calloc` of `total` bytes in all.
#[cold]
#[inline(never)]
pub(crate) extern "C" fn calloc_slow(total: usize) -> *mut c_void {
    or_enomem(allocate(total, 1, true))
}

/// Zeroes `block`, all that `malloc_usable_size` reports of it (the class of `total`), and
/// returns it: the tail of a `calloc` of `total` bytes whose block a pop found.
///
/// # Safety
///
/// `block` is a slot of the C class of `total` bytes (`table::calloc_class`), or for a total
/// that table does not cover a block of at least `total` bytes, that the caller owns.
#[inline(never)]
pub(crate) unsafe extern "C" fn calloc_zero(block: *mut c_void, total: usize) -> *mut c_void {
    let len = table::calloc_class(total).map_or(total, |class| class.size());
    // SAFETY: SLOW-02.
    unsafe { block.cast::<u8>().write_bytes(0, len) };
    block
}

/// Completes `free(ptr)`: nothing for null, the push for a small block, the back end for a
/// large one, and an abort for anything that is not a block of the heap.
///
/// # Safety
///
/// `ptr` is null or a block of the heap that the caller owns and gives up.
#[cold]
#[inline(never)]
pub(crate) unsafe extern "C" fn free_slow(ptr: *mut c_void) {
    let Some(object) = NonNull::new(ptr.cast::<u8>()) else {
        return;
    };
    match process::find(object.as_ptr()) {
        // SAFETY: SLOW-03.
        Some(Found::Small(class)) => unsafe { seam::free_small(class, object) },
        Some(Found::Large { .. }) => process::free_large(object),
        None => process::abort("free of a pointer that is not a block of asmalloc's heap"),
    }
}

/// Completes a sized free, `free_sized(ptr, size)` or C++'s sized `delete`: the size is a
/// prediction a routine may have acted on, and this frees exactly what `free_slow(ptr)` would.
///
/// # Safety
///
/// As for [`free_slow`]. A wrong `size` is safe.
#[cold]
#[inline(never)]
pub(crate) unsafe extern "C" fn free_sized_slow(ptr: *mut c_void, _size: usize) {
    // SAFETY: the caller's promise.
    unsafe { free_slow(ptr) }
}

/// Completes `free_aligned_sized(ptr, align, size)` or C++'s sized aligned `delete`, as
/// [`free_sized_slow`].
///
/// # Safety
///
/// As for [`free_slow`]. A wrong `align` or `size` is safe.
#[cold]
#[inline(never)]
pub(crate) unsafe extern "C" fn free_aligned_sized_slow(
    ptr: *mut c_void,
    _align: usize,
    _size: usize,
) {
    // SAFETY: the caller's promise.
    unsafe { free_slow(ptr) }
}
