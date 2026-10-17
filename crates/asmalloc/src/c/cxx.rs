//! The C++ allocation operators: every replaceable form of `operator new` and `operator
//! delete` (C++17 [new.delete], as libstdc++'s and libc++'s `<new>` declare them), on the
//! process's heap. [`export_cxx_abi!`](crate::export_cxx_abi) exports them under their mangled
//! names.
//!
//! - **new** is `malloc`, or the aligned allocation for the `std::align_val_t` forms, with the
//!   same classes and alignment. When it fails it runs the installed new-handler in a loop, as
//!   the standard requires, until an allocation succeeds or no handler is installed. The
//!   handler comes from `std::get_new_handler`, found at the time of the failure with `dlsym`,
//!   so the library needs no C++ runtime to load.
//! - **Failure.** A nothrow form returns null once no handler is installed, and a handler that
//!   throws ends the process, since the exception cannot pass (below). A throwing form ends the process: it cannot
//!   throw `std::bad_alloc`. The allocator is built with `panic = "abort"`, and Rust then gives
//!   every call that may unwind a landing pad that aborts, so an exception thrown below one of
//!   these functions (by `std::__throw_bad_alloc`, or by a new-handler) can never leave
//!   through its frame: the unwinder reaches the landing pad and the process aborts. Only a routine that leaves no Rust frame on the stack, a generated one
//!   whose failure path jumps to the C++ runtime's thrower, can throw from here.
//! - **delete** takes the class from the address, as `free` does. A size passed to a sized
//!   delete is a prediction, never the truth: a wrong one (an object deleted through a base
//!   pointer whose destructor is not virtual) frees exactly what an unsized delete would. The
//!   sized forms go through the sized-free seam (`seam::free_sized`), which a generated
//!   routine will replace with one that checks the size against the address.
//!
//! The ledger entry is `docs/safety/asmalloc/c/cxx-safety.md`.

use super::{seam, slow};
use crate::heap::process;
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

/// The end of a throwing `new` that failed.
#[cold]
fn cannot_throw() -> ! {
    process::abort("operator new: out of memory, and asmalloc cannot throw std::bad_alloc")
}

/// A block of `size` bytes aligned to `align`, or null; null at once if `align` is not a
/// power of two, which the standard leaves undefined.
fn aligned(size: usize, align: usize) -> *mut c_void {
    if !align.is_power_of_two() {
        return ptr::null_mut();
    }
    slow::allocate(size, align, false).cast()
}

/// Completes a throwing `operator new(size)` after the fast path failed.
#[cold]
#[inline(never)]
pub(crate) extern "C" fn new_slow(size: usize) -> *mut c_void {
    let block = retry(|| slow::malloc_slow(size));
    if block.is_null() {
        cannot_throw();
    }
    block
}

/// Completes a nothrow `operator new(size, std::nothrow)` after the fast path failed.
#[cold]
#[inline(never)]
pub(crate) extern "C" fn new_nothrow_slow(size: usize) -> *mut c_void {
    retry(|| slow::malloc_slow(size))
}

/// `operator new(size)` and `operator new[](size)`.
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
    let block = seam::malloc(size);
    if block.is_null() {
        return new_nothrow_slow(size);
    }
    block
}

/// `operator new(size, std::align_val_t(align))` and its array form.
pub extern "C" fn operator_new_aligned(size: usize, align: usize) -> *mut c_void {
    let block = aligned(size, align);
    if !block.is_null() {
        return block;
    }
    let block = if align.is_power_of_two() {
        retry(|| aligned(size, align))
    } else {
        ptr::null_mut()
    };
    if block.is_null() {
        cannot_throw();
    }
    block
}

/// `operator new(size, std::align_val_t(align), std::nothrow)` and its array form.
pub extern "C" fn operator_new_aligned_nothrow(size: usize, align: usize) -> *mut c_void {
    let block = aligned(size, align);
    if !block.is_null() || !align.is_power_of_two() {
        return block;
    }
    retry(|| aligned(size, align))
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

/// Exports the C++ allocation operators under their mangled names (Itanium C++ ABI, x86-64:
/// `size_t` is `m`) from the crate that invokes it, so that C++ code in the program allocates
/// from asmalloc without going through `malloc`, and sized deletes reach it with their size.
/// Invoke it once per program, at the item level, alongside
/// [`export_c_abi!`](crate::export_c_abi).
///
/// It is separate from `export_c_abi!` because C++ lets a program define these operators
/// itself; such a program can take the C interface without a clash of definitions.
///
/// The set is every replaceable form `<new>` declares: `new` and `new[]`, each plain,
/// `std::nothrow`, `std::align_val_t` and both; `delete` and `delete[]`, each plain, sized,
/// `std::nothrow`, `std::align_val_t`, aligned `std::nothrow`, and sized aligned. The placement
/// forms are inline and not replaceable.
#[macro_export]
macro_rules! export_cxx_abi {
    () => {
        // The C++ ABI's names are not snake case.
        #[allow(non_snake_case)]
        const _: () = {
            use ::core::ffi::c_void;
            use $crate::c::cxx;

            // new
            #[unsafe(no_mangle)]
            extern "C" fn _Znwm(size: usize) -> *mut c_void {
                cxx::operator_new(size)
            }
            #[unsafe(no_mangle)]
            extern "C" fn _Znam(size: usize) -> *mut c_void {
                cxx::operator_new(size)
            }
            #[unsafe(no_mangle)]
            extern "C" fn _ZnwmRKSt9nothrow_t(size: usize, _: *const c_void) -> *mut c_void {
                cxx::operator_new_nothrow(size)
            }
            #[unsafe(no_mangle)]
            extern "C" fn _ZnamRKSt9nothrow_t(size: usize, _: *const c_void) -> *mut c_void {
                cxx::operator_new_nothrow(size)
            }
            #[unsafe(no_mangle)]
            extern "C" fn _ZnwmSt11align_val_t(size: usize, align: usize) -> *mut c_void {
                cxx::operator_new_aligned(size, align)
            }
            #[unsafe(no_mangle)]
            extern "C" fn _ZnamSt11align_val_t(size: usize, align: usize) -> *mut c_void {
                cxx::operator_new_aligned(size, align)
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
            unsafe extern "C" fn _ZdlPv(ptr: *mut c_void) {
                // SAFETY: CXX-EXPORT-01.
                unsafe { cxx::operator_delete(ptr) }
            }
            #[unsafe(no_mangle)]
            unsafe extern "C" fn _ZdaPv(ptr: *mut c_void) {
                // SAFETY: CXX-EXPORT-01.
                unsafe { cxx::operator_delete(ptr) }
            }
            #[unsafe(no_mangle)]
            unsafe extern "C" fn _ZdlPvm(ptr: *mut c_void, size: usize) {
                // SAFETY: CXX-EXPORT-01.
                unsafe { cxx::operator_delete_sized(ptr, size) }
            }
            #[unsafe(no_mangle)]
            unsafe extern "C" fn _ZdaPvm(ptr: *mut c_void, size: usize) {
                // SAFETY: CXX-EXPORT-01.
                unsafe { cxx::operator_delete_sized(ptr, size) }
            }
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

#[cfg(feature = "cxx-abi")]
export_cxx_abi!();
