//! The C++ allocation operators (C++17 [new.delete], as libstdc++'s and libc++'s `<new>`
//! declare them) on the process's heap. [`export_cxx_abi!`](crate::export_cxx_abi) exports the
//! nothrow forms of `new` and every form of `delete` under their mangled names.
//!
//! - **new** is `malloc`, or the aligned allocation for the `std::align_val_t` forms, with the
//!   same classes and alignment. When it fails it runs the installed new-handler in a loop, as
//!   the standard requires, until an allocation succeeds or no handler is installed. The
//!   handler comes from `std::get_new_handler`, found at the time of the failure with `dlsym`,
//!   so the library needs no C++ runtime to load. A nothrow form then returns null.
//! - **The throwing forms are not exported.** Their failure must throw `std::bad_alloc`, and
//!   no Rust function here can: the allocator is built with `panic = "abort"`, and Rust then
//!   gives every call that may unwind a landing pad that aborts, so an exception thrown below
//!   one of these functions never leaves through its frame. The design's answer is a generated
//!   routine whose failure path jumps to the C++ runtime's thrower with nothing of ours on the
//!   stack. Until it exists, the C++ runtime's own throwing `new` serves those four forms by
//!   calling `malloc` and `aligned_alloc`, which keeps the exception catchable; its blocks are
//!   asmalloc's, and the `delete` here frees them. [`operator_new`] and
//!   [`operator_new_aligned`] are written as what the generated routine will do short of the
//!   throw, around `new_slow` and `new_aligned_slow`, the slow paths it will call: each
//!   returns the block, or null once the new-handler loop is over, and the routine throws.
//! - **A new-handler that throws** ends the process when the new that ran it is ours (a nothrow
//!   form), for the same reason: the exception cannot pass our frame.
//! - **delete** takes the class from the address, as `free` does. A size passed to a sized
//!   delete is a prediction, never the truth: a wrong one (an object deleted through a base
//!   pointer whose destructor is not virtual) frees exactly what an unsized delete would. The
//!   sized forms go through the sized-free seam (`seam::free_sized`), which a generated
//!   routine will replace with one that checks the size against the address.
//!
//! The ledger entry is `docs/safety/asmalloc/c/cxx-safety.md`.

use super::{seam, slow};
use core::ffi::{c_char, c_void};
use core::ptr;

/// `std::new_handler`: `void (*)()`. A handler may throw, so calls to it may unwind.
type NewHandler = unsafe extern "C-unwind" fn();

/// `std::get_new_handler`: `std::new_handler get_new_handler() noexcept`.
type GetNewHandler = unsafe extern "C" fn() -> Option<NewHandler>;

/// `std::get_new_handler`'s mangled name, the same in libstdc++ and libc++.
const GET_NEW_HANDLER: &core::ffi::CStr = c"_ZSt15get_new_handlerv";

// SAFETY: CXX-01. glibc 2.34 and later define `void *dlsym(void *, const char *)` in
// libc.so.6.
#[link(name = "c")]
unsafe extern "C" {
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

/// `RTLD_DEFAULT` on glibc: search the global scope.
const RTLD_DEFAULT: *mut c_void = ptr::null_mut();

/// The installed new-handler, if the process has a C++ runtime and a handler is installed.
fn new_handler() -> Option<NewHandler> {
    // SAFETY: CXX-02.
    let getter = unsafe { dlsym(RTLD_DEFAULT, GET_NEW_HANDLER.as_ptr()) };
    if getter.is_null() {
        return None;
    }
    // SAFETY: CXX-03.
    let getter = unsafe { core::mem::transmute::<*mut c_void, GetNewHandler>(getter) };
    // SAFETY: CXX-03.
    unsafe { getter() }
}

/// After `allocate` has failed once: runs the new-handler and tries again, until an attempt
/// succeeds (the block) or no handler is installed (null).
#[cold]
#[inline(never)]
fn retry(allocate: impl Fn() -> *mut c_void) -> *mut c_void {
    loop {
        let Some(handler) = new_handler() else {
            return ptr::null_mut();
        };
        // SAFETY: CXX-04.
        unsafe { handler() };
        let block = allocate();
        if !block.is_null() {
            return block;
        }
    }
}

/// A block of `size` bytes aligned to `align`, or null; null at once if `align` is not a
/// power of two, which the standard leaves undefined.
fn aligned(size: usize, align: usize) -> *mut c_void {
    if !align.is_power_of_two() {
        return ptr::null_mut();
    }
    slow::allocate(size, align, false).cast()
}

/// Completes an `operator new(size)` of any form whose first attempt failed: the new-handler
/// loop, then the block or null. A nothrow form returns what it returns; a throwing form's
/// routine will throw `std::bad_alloc` on null.
#[cold]
#[inline(never)]
pub(crate) extern "C" fn new_slow(size: usize) -> *mut c_void {
    retry(|| slow::malloc_slow(size))
}

/// Completes an `operator new(size, std::align_val_t(align))` of any form, as [`new_slow`].
#[cold]
#[inline(never)]
pub(crate) extern "C" fn new_aligned_slow(size: usize, align: usize) -> *mut c_void {
    if !align.is_power_of_two() {
        return ptr::null_mut();
    }
    retry(|| aligned(size, align))
}

/// `operator new(size)` and `operator new[](size)` short of the throw: the block, or null where
/// the C++ operator must throw `std::bad_alloc`. Not exported (see the module documentation):
/// the throw needs a generated routine with no frame of ours below it.
#[inline]
pub extern "C" fn operator_new(size: usize) -> *mut c_void {
    let block = seam::malloc(size);
    if block.is_null() {
        return new_slow(size);
    }
    block
}

/// `operator new(size, std::nothrow)` and its array form.
#[inline]
pub extern "C" fn operator_new_nothrow(size: usize) -> *mut c_void {
    operator_new(size)
}

/// `operator new(size, std::align_val_t(align))` and its array form short of the throw, as
/// [`operator_new`]. Not exported.
pub extern "C" fn operator_new_aligned(size: usize, align: usize) -> *mut c_void {
    let block = aligned(size, align);
    if block.is_null() {
        return new_aligned_slow(size, align);
    }
    block
}

/// `operator new(size, std::align_val_t(align), std::nothrow)` and its array form.
pub extern "C" fn operator_new_aligned_nothrow(size: usize, align: usize) -> *mut c_void {
    operator_new_aligned(size, align)
}

/// Completes a plain `operator delete(ptr)` or `delete[]` the generated `c_delete` routine did
/// not: a foreign pointer's handling belongs to the `delete` family, not to `free`.
///
/// # Safety
///
/// As for [`operator_delete`].
#[doc(hidden)]
#[cold]
#[inline(never)]
pub unsafe extern "C" fn operator_delete_slow(ptr: *mut c_void) {
    // SAFETY: the caller's promise.
    unsafe { slow::free_slow(ptr) }
}

/// Completes a sized `operator delete(ptr, size)` or `delete[]` the generated `c_delete_sized`
/// routine did not, as [`operator_delete_slow`].
///
/// # Safety
///
/// As for [`operator_delete`]. A wrong `size` is safe.
#[doc(hidden)]
#[cold]
#[inline(never)]
pub unsafe extern "C" fn operator_delete_sized_slow(ptr: *mut c_void, size: usize) {
    // SAFETY: the caller's promise.
    unsafe { slow::free_sized_slow(ptr, size) }
}

/// `operator delete(ptr)`, its array form, and the `std::nothrow` and `std::align_val_t`
/// forms, none of which needs its extra argument.
///
/// # Safety
///
/// `ptr` is null or a block of the heap that the caller owns and gives up.
#[inline]
pub unsafe extern "C" fn operator_delete(ptr: *mut c_void) {
    // SAFETY: the caller's promise.
    unsafe { seam::free(ptr) }
}

/// `operator delete(ptr, size)` and its array form. `size` is a prediction; the class comes
/// from the address.
///
/// # Safety
///
/// As for [`operator_delete`]. A wrong `size` is safe.
#[inline]
pub unsafe extern "C" fn operator_delete_sized(ptr: *mut c_void, size: usize) {
    // SAFETY: the caller's promise.
    unsafe { seam::free_sized(ptr, size) }
}

/// `operator delete(ptr, size, std::align_val_t(align))` and its array form. Neither `size`
/// nor `align` is trusted.
///
/// # Safety
///
/// As for [`operator_delete`]. A wrong `size` or `align` is safe.
#[inline]
pub unsafe extern "C" fn operator_delete_sized_aligned(
    ptr: *mut c_void,
    size: usize,
    align: usize,
) {
    // SAFETY: the caller's promise.
    unsafe { seam::free_aligned_sized(ptr, align, size) }
}

/// Exports C++ allocation operators under their mangled names (Itanium C++ ABI, x86-64:
/// `size_t` is `m`) from the crate that invokes it, so that sized deletes reach asmalloc with
/// their size and nothrow `new` skips a call layer.
/// Invoke it once per program, at the item level, alongside
/// [`export_c_abi!`](crate::export_c_abi).
///
/// It is separate from `export_c_abi!` because C++ lets a program define these operators
/// itself; such a program can take the C interface without a clash of definitions. The reverse
/// is not supported: without the C interface, the C++ runtime's throwing `new` would get its
/// blocks from the C library's `malloc`, and this `delete` would be handed them.
///
/// The set is 16 of the 20 replaceable forms `<new>` declares: `new` and `new[]` with
/// `std::nothrow`, plain and with `std::align_val_t`; `delete` and `delete[]`, each plain,
/// sized, `std::nothrow`, `std::align_val_t`, aligned `std::nothrow`, and sized aligned. The
/// four throwing forms of `new` are left to the C++ runtime, which can throw `std::bad_alloc`
/// (see the [module documentation](self)); they allocate through `malloc` and `aligned_alloc`,
/// so their blocks are asmalloc's and the `delete` exported here frees them. The placement
/// forms are inline and not replaceable.
#[macro_export]
macro_rules! export_cxx_abi {
    () => {
        // Without the C interface the C++ runtime's throwing `new` would take its blocks from the
        // C library's `malloc` and hand them to this `delete`: refuse to build. `export_c_abi!`,
        // invoked in the same module, defines this.
        const _: () = ASMALLOC_C_ABI_EXPORTED;

        // Plain and sized `delete` and `delete[]`: the generated `c_delete` and `c_delete_sized`
        // routines under the C++ names, defined in this crate.
        $crate::asmalloc_exports_cxx_x86_64!();

        // The C++ ABI's names are not snake case.
        #[allow(non_snake_case)]
        const _: () = {
            use ::core::ffi::c_void;
            use $crate::c::cxx;

            // new: the nothrow forms only. The throwing forms stay the C++ runtime's, which
            // serves them through `malloc` and `aligned_alloc` and can throw `std::bad_alloc`
            // (see `asmalloc::c::cxx`).
            #[unsafe(no_mangle)]
            extern "C" fn _ZnwmRKSt9nothrow_t(size: usize, _: *const c_void) -> *mut c_void {
                cxx::operator_new_nothrow(size)
            }
            #[unsafe(no_mangle)]
            extern "C" fn _ZnamRKSt9nothrow_t(size: usize, _: *const c_void) -> *mut c_void {
                cxx::operator_new_nothrow(size)
            }
            #[unsafe(no_mangle)]
            extern "C" fn _ZnwmSt11align_val_tRKSt9nothrow_t(
                size: usize,
                align: usize,
                _: *const c_void,
            ) -> *mut c_void {
                cxx::operator_new_aligned_nothrow(size, align)
            }
            #[unsafe(no_mangle)]
            extern "C" fn _ZnamSt11align_val_tRKSt9nothrow_t(
                size: usize,
                align: usize,
                _: *const c_void,
            ) -> *mut c_void {
                cxx::operator_new_aligned_nothrow(size, align)
            }

            // delete
            #[unsafe(no_mangle)]
            unsafe extern "C" fn _ZdlPvRKSt9nothrow_t(ptr: *mut c_void, _: *const c_void) {
                // SAFETY: CXX-EXPORT-01.
                unsafe { cxx::operator_delete(ptr) }
            }
            #[unsafe(no_mangle)]
            unsafe extern "C" fn _ZdaPvRKSt9nothrow_t(ptr: *mut c_void, _: *const c_void) {
                // SAFETY: CXX-EXPORT-01.
                unsafe { cxx::operator_delete(ptr) }
            }
            #[unsafe(no_mangle)]
            unsafe extern "C" fn _ZdlPvSt11align_val_t(ptr: *mut c_void, _: usize) {
                // SAFETY: CXX-EXPORT-01.
                unsafe { cxx::operator_delete(ptr) }
            }
            #[unsafe(no_mangle)]
            unsafe extern "C" fn _ZdaPvSt11align_val_t(ptr: *mut c_void, _: usize) {
                // SAFETY: CXX-EXPORT-01.
                unsafe { cxx::operator_delete(ptr) }
            }
            #[unsafe(no_mangle)]
            unsafe extern "C" fn _ZdlPvSt11align_val_tRKSt9nothrow_t(
                ptr: *mut c_void,
                _: usize,
                _: *const c_void,
            ) {
                // SAFETY: CXX-EXPORT-01.
                unsafe { cxx::operator_delete(ptr) }
            }
            #[unsafe(no_mangle)]
            unsafe extern "C" fn _ZdaPvSt11align_val_tRKSt9nothrow_t(
                ptr: *mut c_void,
                _: usize,
                _: *const c_void,
            ) {
                // SAFETY: CXX-EXPORT-01.
                unsafe { cxx::operator_delete(ptr) }
            }
            #[unsafe(no_mangle)]
            unsafe extern "C" fn _ZdlPvmSt11align_val_t(
                ptr: *mut c_void,
                size: usize,
                align: usize,
            ) {
                // SAFETY: CXX-EXPORT-01.
                unsafe { cxx::operator_delete_sized_aligned(ptr, size, align) }
            }
            #[unsafe(no_mangle)]
            unsafe extern "C" fn _ZdaPvmSt11align_val_t(
                ptr: *mut c_void,
                size: usize,
                align: usize,
            ) {
                // SAFETY: CXX-EXPORT-01.
                unsafe { cxx::operator_delete_sized_aligned(ptr, size, align) }
            }
        };
    };
}
