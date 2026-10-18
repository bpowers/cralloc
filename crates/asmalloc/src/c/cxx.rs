//! The C++ allocation operators (C++17 [new.delete], as libstdc++'s and libc++'s `<new>`
//! declare them) on the process's heap. [`export_cxx_abi!`](crate::export_cxx_abi) exports the
//! nothrow forms of `new` and every form of `delete` under their mangled names. Nothing exports
//! them yet, the preload library included: exporting some forms without the throwing `new` is
//! unsound when another library defines the operators (design: "Interfaces", C++). They stay
//! built and tested (`tests/programs/cxx_exports` in the preload crate) for when it can.
//!
//! - **new** is `malloc`, or the aligned allocation for the `std::align_val_t` forms, with the
//!   same classes and alignment. When it fails it runs the installed new-handler in a loop, as
//!   the standard requires, until an allocation succeeds or no handler is installed. The
//!   handler comes from `std::get_new_handler`, found at the time of the failure (`loader.rs`),
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
//!   pointer whose destructor is not virtual) frees exactly what an unsized delete would.
//! - **A program may replace operators** (design: "Interfaces", C++). The forms it did not
//!   define stay ours, and behave as the standard's defaults would: if the first
//!   `operator delete(void*)` in the process is not ours, every unaligned `delete` of ours
//!   forwards every pointer to it, and likewise the aligned forms to a replaced
//!   `operator delete(void*, std::align_val_t)`; a replacement `new` built on our `malloc` hands
//!   out pointers inside our heap that no check could tell from ours. If the program replaced
//!   `operator new(size)`, our nothrow `new` hands off to the C++ runtime's, which calls the
//!   program's in a `try` block. With no `delete` replaced, a pointer outside the heap goes to
//!   the program's `free` if it has its own `malloc`, and aborts otherwise. Each answer is
//!   looked up once (`loader.rs`) and cached. The array forms take the scalar forms' answers.
//!
//! The ledger entry is `docs/safety/asmalloc/c/cxx-safety.md`.

use super::{loader, seam, slow};
use crate::heap::{Found, process};
use core::ffi::{CStr, c_void};
use core::ptr::{self, NonNull};
use core::sync::atomic::{AtomicUsize, Ordering};

/// `std::new_handler`: `void (*)()`. A handler may throw, so calls to it may unwind.
type NewHandler = unsafe extern "C-unwind" fn();

/// `std::get_new_handler`: `std::new_handler get_new_handler() noexcept`.
type GetNewHandler = unsafe extern "C" fn() -> Option<NewHandler>;

/// `operator delete(void*)`, and the nothrow `new` forms of the C++ runtime.
type Delete = unsafe extern "C" fn(*mut c_void);
type DeleteAligned = unsafe extern "C" fn(*mut c_void, usize);
type NewNothrow = unsafe extern "C" fn(usize, *const u8) -> *mut c_void;
type NewAlignedNothrow = unsafe extern "C" fn(usize, usize, *const u8) -> *mut c_void;

/// `std::get_new_handler`'s mangled name, the same in libstdc++ and libc++.
const GET_NEW_HANDLER: &CStr = c"_ZSt15get_new_handlerv";

/// The installed new-handler, if the process has a C++ runtime and a handler is installed.
fn new_handler() -> Option<NewHandler> {
    let getter = loader::first(GET_NEW_HANDLER)?;
    // SAFETY: CXX-03.
    let getter = unsafe { core::mem::transmute::<usize, GetNewHandler>(getter) };
    // SAFETY: CXX-03.
    unsafe { getter() }
}

/// A function of the program's that takes the place of one of ours, looked up once.
///
/// The word is zero until the first look, [`NONE`] when there is nothing to forward to, and
/// otherwise the function's address. Racing first looks find the same answer and store it.
struct Replacement {
    word: AtomicUsize,
    look: fn() -> Option<usize>,
}

/// The cached answer for "nothing to forward to".
const NONE: usize = 1;

impl Replacement {
    const fn new(look: fn() -> Option<usize>) -> Replacement {
        Replacement {
            word: AtomicUsize::new(0),
            look,
        }
    }

    /// The program's function, if it replaced ours.
    #[inline]
    fn get(&self) -> Option<usize> {
        let mut word = self.word.load(Ordering::Relaxed);
        if word == 0 {
            word = (self.look)().filter(|&f| f > NONE).unwrap_or(NONE);
            self.word.store(word, Ordering::Relaxed);
        }
        (word != NONE).then_some(word)
    }
}

/// The first definition of `name`, if it is not ours.
fn theirs(name: &CStr) -> Option<usize> {
    loader::first(name).filter(|&f| !loader::is_ours(f))
}

/// The C++ runtime's definition of the nothrow form `nothrow` when the program replaced the
/// basic `new` it calls: what the program would bind to without ours.
fn runtime_nothrow(basic: &CStr, nothrow: &CStr) -> Option<usize> {
    if loader::first(basic) == loader::next(basic) {
        return None;
    }
    loader::next(nothrow).filter(|&f| !loader::is_ours(f))
}

/// The program's `operator delete(void*)`, to which every unaligned `delete` of ours forwards
/// when it exists, as the standard's defaults do.
static DELETE: Replacement = Replacement::new(|| theirs(c"_ZdlPv"));
/// The program's `operator delete(void*, std::align_val_t)`, likewise for the aligned forms.
static DELETE_ALIGNED: Replacement = Replacement::new(|| theirs(c"_ZdlPvSt11align_val_t"));
/// The program's `free`, for a pointer outside the heap when no `delete` was replaced.
static FREE: Replacement = Replacement::new(|| theirs(c"free"));
/// The C++ runtime's `operator new(size, std::nothrow)`, when the program replaced
/// `operator new(size)`: it calls the program's in a `try` block, which no frame of ours can.
static NEW_NOTHROW: Replacement =
    Replacement::new(|| runtime_nothrow(c"_Znwm", c"_ZnwmRKSt9nothrow_t"));
/// The same for the aligned forms.
static NEW_ALIGNED_NOTHROW: Replacement = Replacement::new(|| {
    runtime_nothrow(
        c"_ZnwmSt11align_val_t",
        c"_ZnwmSt11align_val_tRKSt9nothrow_t",
    )
});

/// What the `std::nothrow_t` reference we pass on points at. The runtime never reads it.
static NOTHROW: u8 = 0;

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

/// `operator new(size, std::nothrow)` and its array form: the C++ runtime's when the program
/// replaced `operator new(size)`, so that the block comes from the program's `new` and suits
/// its `delete`.
#[inline]
pub extern "C" fn operator_new_nothrow(size: usize) -> *mut c_void {
    if let Some(runtime) = NEW_NOTHROW.get() {
        // SAFETY: CXX-05.
        return unsafe { core::mem::transmute::<usize, NewNothrow>(runtime)(size, &NOTHROW) };
    }
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
    if let Some(runtime) = NEW_ALIGNED_NOTHROW.get() {
        // SAFETY: CXX-05.
        let runtime = unsafe { core::mem::transmute::<usize, NewAlignedNothrow>(runtime) };
        // SAFETY: CXX-05.
        return unsafe { runtime(size, align, &NOTHROW) };
    }
    operator_new_aligned(size, align)
}

/// Frees `ptr` as our own `delete` with no replacement to forward to: a block of the heap is
/// ours to free, and a pointer outside it goes to the program's `free` if the program has
/// one, else the process aborts.
///
/// # Safety
///
/// `ptr` is null, a block of the heap that the caller owns and gives up, or a block from the
/// program's own `malloc` that its `free` takes.
unsafe fn release(ptr: *mut c_void) {
    let Some(object) = NonNull::new(ptr.cast::<u8>()) else {
        return;
    };
    match process::find(object.as_ptr()) {
        // SAFETY: CXX-06.
        Some(Found::Small(class)) => unsafe { seam::free_small(class, object) },
        Some(Found::Large { .. }) => process::free_large(object),
        None => match FREE.get() {
            // SAFETY: CXX-07.
            Some(free) => unsafe { core::mem::transmute::<usize, Delete>(free)(ptr) },
            None => process::abort("delete of a pointer that is not a block of asmalloc's heap"),
        },
    }
}

/// Completes a plain `operator delete(ptr)` or `delete[]` the generated `c_delete` routine did
/// not. If the program replaced `operator delete(void*)`, every pointer goes to it, as the
/// standard's default for every other form would send it: its blocks may sit inside our heap
/// (a replacement `new` on top of `malloc`), where no check can tell them from ours. Otherwise
/// the routines may serve our blocks from now on (`mirror::publish_delete`).
///
/// # Safety
///
/// `ptr` is null or a block that a `new` of the process returned, which the caller gives up.
#[doc(hidden)]
#[cold]
#[inline(never)]
pub unsafe extern "C" fn operator_delete_slow(ptr: *mut c_void) {
    if let Some(delete) = DELETE.get() {
        // SAFETY: CXX-07.
        return unsafe { core::mem::transmute::<usize, Delete>(delete)(ptr) };
    }
    // The basic `delete` is ours, so from now on the generated routines may free our blocks
    // themselves; until this, they send every pointer here.
    super::mirror::publish_delete();
    // SAFETY: CXX-06: with no replaced `delete`, every `new` of the process is ours or the
    // C++ runtime's, whose blocks come from `malloc`.
    unsafe { release(ptr) }
}

/// Completes a sized `operator delete(ptr, size)` or `delete[]` the generated `c_delete_sized`
/// routine did not, as [`operator_delete_slow`]: the standard's default for the sized forms
/// forwards to the unsized one.
///
/// # Safety
///
/// As for [`operator_delete_slow`]. A wrong `size` is safe.
#[doc(hidden)]
#[cold]
#[inline(never)]
pub unsafe extern "C" fn operator_delete_sized_slow(ptr: *mut c_void, _size: usize) {
    // SAFETY: the caller's promise.
    unsafe { operator_delete_slow(ptr) }
}

/// `operator delete(ptr, std::nothrow)` and its array form: as [`operator_delete_slow`].
///
/// # Safety
///
/// As for [`operator_delete_slow`].
#[inline]
pub unsafe extern "C" fn operator_delete(ptr: *mut c_void) {
    // SAFETY: the caller's promise.
    unsafe { operator_delete_slow(ptr) }
}

/// `operator delete(ptr, size)` and its array form, as [`operator_delete_sized_slow`].
///
/// # Safety
///
/// As for [`operator_delete_slow`]. A wrong `size` is safe.
#[inline]
pub unsafe extern "C" fn operator_delete_sized(ptr: *mut c_void, size: usize) {
    // SAFETY: the caller's promise.
    unsafe { operator_delete_sized_slow(ptr, size) }
}

/// `operator delete(ptr, std::align_val_t(align))`, its array form and their `std::nothrow`
/// forms. If the program replaced `operator delete(void*, std::align_val_t)`, every pointer goes
/// to it with the alignment, as the standard's defaults do; otherwise the alignment is not
/// trusted and the block is freed as by [`operator_delete_slow`].
///
/// # Safety
///
/// As for [`operator_delete_slow`]. A wrong `align` is safe.
#[inline]
pub unsafe extern "C" fn operator_delete_aligned(ptr: *mut c_void, align: usize) {
    if let Some(delete) = DELETE_ALIGNED.get() {
        // SAFETY: CXX-07.
        return unsafe { core::mem::transmute::<usize, DeleteAligned>(delete)(ptr, align) };
    }
    // SAFETY: as in `operator_delete_slow`.
    unsafe { release(ptr) }
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
    let _ = size;
    // SAFETY: the caller's promise.
    unsafe { operator_delete_aligned(ptr, align) }
}

/// **Not for use yet.** Exporting some of the operators without the throwing `new` forms is
/// unsound when another library defines the operators: its blocks then reach our `delete`
/// (design: "Interfaces", C++). The macro is kept built and tested for when every form can
/// be exported; the preload library exports none.
///
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
#[doc(hidden)]
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
            unsafe extern "C" fn _ZdlPvSt11align_val_t(ptr: *mut c_void, align: usize) {
                // SAFETY: CXX-EXPORT-01.
                unsafe { cxx::operator_delete_aligned(ptr, align) }
            }
            #[unsafe(no_mangle)]
            unsafe extern "C" fn _ZdaPvSt11align_val_t(ptr: *mut c_void, align: usize) {
                // SAFETY: CXX-EXPORT-01.
                unsafe { cxx::operator_delete_aligned(ptr, align) }
            }
            #[unsafe(no_mangle)]
            unsafe extern "C" fn _ZdlPvSt11align_val_tRKSt9nothrow_t(
                ptr: *mut c_void,
                align: usize,
                _: *const c_void,
            ) {
                // SAFETY: CXX-EXPORT-01.
                unsafe { cxx::operator_delete_aligned(ptr, align) }
            }
            #[unsafe(no_mangle)]
            unsafe extern "C" fn _ZdaPvSt11align_val_tRKSt9nothrow_t(
                ptr: *mut c_void,
                align: usize,
                _: *const c_void,
            ) {
                // SAFETY: CXX-EXPORT-01.
                unsafe { cxx::operator_delete_aligned(ptr, align) }
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
