//! The C interface: `malloc` and its family on the process's heap (design: "Interfaces").
//!
//! The functions here have their C names and signatures but Rust (mangled) symbols, so a
//! program can call them without replacing its C library's allocator.
//! [`export_c_abi!`](crate::export_c_abi) exports them under their C names from the crate that
//! invokes it, which is how the
//! `LD_PRELOAD` library (`crates/asmalloc-preload`) and the cargo feature `c-abi` give a
//! process asmalloc as its `malloc`. A macro rather than a feature alone, because cargo unifies
//! features across a workspace: a feature that the preload library turned on would also export
//! `malloc` from every test binary built with it.
//!
//! What the functions promise, beyond the C standard and POSIX:
//!
//! - A block of 16 bytes or more is 16-aligned; a smaller one is aligned to its 8-byte class.
//!   Any alignment the family asks for is met by the class alone (no padding, no interior
//!   pointer), up to the region size, and by a run of regions beyond it.
//! - `free`, `realloc`, `malloc_usable_size` and the sized frees find the block's class from
//!   its address. The sized frees use the size only as a prediction that the address must
//!   confirm: a wrong one frees exactly what `free` would. A pointer outside the heap, or into
//!   the middle of a large block or a run, aborts the process. A pointer into the middle of a
//!   small block is the caller's bug that the fast path does not look for: it is caught only if
//!   it reaches the back end's bitmaps before it is handed out again (docs/design.md,
//!   "Interfaces").
//! - `realloc(p, 0)` frees `p` and returns null, as glibc's does. `realloc` keeps a small block
//!   in place when the new size has the same class, and a large one when the new size is still
//!   large, fits the slot or run and, shrinking, keeps enough of it (`heap::large`); a large
//!   block that grows out of its slot moves to one with headroom. A block that moves keeps
//!   every byte its usable size covered, up to the new size, as glibc's does.
//! - `malloc_usable_size` is the class size of a small block, and the size last asked for,
//!   rounded up to a page, of a large one.
//! - `calloc` makes all of the block's usable size read as zero, as glibc's does, checks the
//!   product for overflow, and does not clear memory the back end knows is zero.
//! - `malloc_trim` is `collect`: it empties the magazines and the depot back to the bitmaps and
//!   returns every wholly free page.
//! - A failure returns null (or an error number from `posix_memalign`) and sets `errno` through
//!   the C library's own accessor (`errno.rs`). `free` and a successful call leave it alone.
//!   Nothing here hands a request to another allocator.
//!
//! `malloc`, `calloc`, `free` and `free_sized` are the routines generated from Lean
//! (`routines.rs`), whose misses complete in the slow paths (`slow.rs`), which also serve every
//! other function here. The ledger is under `docs/safety/asmalloc/c/`.

mod errno;
mod mirror;
mod routines;
#[doc(hidden)]
pub mod slow;
#[doc(hidden)]
pub mod table;

use self::errno::{EINVAL, ENOMEM};
use self::slow::{allocate, or_enomem};
use crate::classes::{C_ALIGN, Class, SmallClass};
use crate::heap::{Found, process};
use crate::os::Os;
use crate::os::linux::LinuxOs;
use core::alloc::Layout;
use core::ffi::{CStr, c_char, c_int, c_void};
use core::ptr::{self, NonNull};

/// The page size, for `valloc` and `pvalloc`.
const PAGE: usize = <LinuxOs as Os>::GEOMETRY.page_size();

/// C's `malloc`.
#[inline]
pub extern "C" fn malloc(size: usize) -> *mut c_void {
    routines::malloc(size)
}

/// C's `calloc`.
#[inline]
pub extern "C" fn calloc(n: usize, size: usize) -> *mut c_void {
    routines::calloc(n, size)
}

/// C's `free`.
///
/// # Safety
///
/// `ptr` is null or a block of the heap, from any of its interfaces, that the caller owns and
/// gives up.
#[inline]
pub unsafe extern "C" fn free(ptr: *mut c_void) {
    // SAFETY: MOD-03. the caller's promise.
    unsafe { routines::free(ptr) }
}

/// C23's `free_sized`. The size is not trusted: the class comes from the address.
///
/// # Safety
///
/// As for [`free`].
#[inline]
pub unsafe extern "C" fn free_sized(ptr: *mut c_void, size: usize) {
    // SAFETY: MOD-03. the caller's promise.
    unsafe { routines::free_sized(ptr, size) }
}

/// C23's `free_aligned_sized`: [`free`], since neither the alignment nor the size is trusted.
///
/// # Safety
///
/// As for [`free`].
#[inline]
pub unsafe extern "C" fn free_aligned_sized(ptr: *mut c_void, _align: usize, _size: usize) {
    // SAFETY: MOD-03. the caller's promise.
    unsafe { routines::free(ptr) }
}

/// The obsolete `cfree`, which glibc still exports for old binaries: [`free`].
///
/// # Safety
///
/// As for [`free`].
#[inline]
pub unsafe extern "C" fn cfree(ptr: *mut c_void) {
    // SAFETY: MOD-03. the caller's promise.
    unsafe { routines::free(ptr) }
}

/// The block at `ptr`, or an abort naming `call` if no block of the heap starts there.
fn find_or_abort(ptr: NonNull<u8>, what: &str) -> Found {
    match process::find(ptr.as_ptr()) {
        Some(found) => found,
        None => process::abort(what),
    }
}

/// C's `realloc`.
///
/// # Safety
///
/// `ptr` is null or a block of the heap that the caller owns. Unless the call fails, the
/// caller gives it up.
pub unsafe extern "C" fn realloc(ptr: *mut c_void, size: usize) -> *mut c_void {
    let Some(old) = NonNull::new(ptr.cast::<u8>()) else {
        return malloc(size);
    };
    if size == 0 {
        // SAFETY: MOD-03. the caller's promise; glibc's choice for C17's implementation-defined case.
        unsafe { free(ptr) };
        return ptr::null_mut();
    }
    // The lean path (`heap::resize`): a small block to a small class. The old class comes from
    // the region table and the slot arithmetic alone (`find_small`); anything else, a pointer
    // that starts no small slot included, takes the general path, which finds it or aborts.
    let found = match (process::find_small(old.as_ptr()), small_class(size)) {
        (Some(from), Some(to)) => {
            let kept = from.size().min(size);
            // SAFETY: MOD-05.
            if let Some(new) = unsafe { process::resize_small(old, from, to, kept) } {
                return new.as_ptr().cast();
            }
            Found::Small(from)
        }
        _ => find_or_abort(
            old,
            "realloc of a pointer that is not a block of asmalloc's heap",
        ),
    };
    // SAFETY: MOD-03. the caller's promise, which the lean path left as it found it.
    unsafe { realloc_other(old, found, size) }
}

/// The small class of `malloc(size)`, for a size of at least one byte, from the inlined lookup
/// the Rust fast path uses: C's rule that a block of 16 bytes or more is 16-aligned is that
/// alignment (`Class::of_c`). `None` for a size with no small class.
#[inline(always)]
fn small_class(size: usize) -> Option<SmallClass> {
    let align = if size >= C_ALIGN { C_ALIGN } else { 1 };
    SmallClass::of_nonzero(size, align)
}

/// C's `realloc` of what its lean path does not take: a large block on either side, or a pop
/// that missed.
///
/// # Safety
///
/// As for [`realloc`], with `old` not null, `found` what the heap found there, and `size` not
/// zero.
#[cold]
#[inline(never)]
unsafe fn realloc_other(old: NonNull<u8>, found: Found, size: usize) -> *mut c_void {
    let ptr = old.as_ptr().cast::<c_void>();
    let Ok(layout) = Layout::from_size_align(size, 1) else {
        errno::set(ENOMEM);
        return ptr::null_mut();
    };
    let next = Class::of_c(layout);
    let kept = match found {
        Found::Small(class) if next == Some(class.class()) => return ptr,
        Found::Small(class) => class.size(),
        Found::Large { size: used } => {
            // SAFETY: MOD-04.
            if unsafe { process::resize_in_place_by_address(old, used, size) } {
                return ptr;
            }
            // Everything `malloc_usable_size` reported, not just the size asked for: programs
            // write into that room (redis's strings do) and glibc's `realloc` keeps it.
            process::usable_size(old.as_ptr()).unwrap_or(used)
        }
    };
    let new = match next.and_then(Class::small) {
        Some(_) => malloc(size).cast(),
        None if size > kept => process::alloc_large_grown(size),
        None => process::alloc_large(size, C_ALIGN, false),
    };
    if new.is_null() {
        errno::set(ENOMEM);
        return ptr::null_mut();
    }
    let (n, to) = (kept.min(size), NonNull::new(new));
    // SAFETY: MOD-01.
    unsafe {
        match (found, to) {
            // A large block is copied in pieces, its old pages returned behind the copy, and
            // freed, all by the back end.
            (Found::Large { .. }, Some(to)) => {
                // `kept` is all the block could use: past it, the block outgrew its slot.
                let why = if size > kept {
                    crate::back::Move::Outgrown
                } else {
                    crate::back::Move::Other
                };
                process::move_large(old, to, n, why)
            }
            _ => {
                old.as_ptr().copy_to_nonoverlapping(new, n);
                free(ptr);
            }
        }
    }
    new.cast()
}

/// `reallocarray`, from the BSDs and glibc: `realloc(ptr, n * size)`, or `ENOMEM` if the
/// product overflows, leaving `ptr` as it was.
///
/// # Safety
///
/// As for [`realloc`].
pub unsafe extern "C" fn reallocarray(ptr: *mut c_void, n: usize, size: usize) -> *mut c_void {
    match n.checked_mul(size) {
        // SAFETY: MOD-03. the caller's promise.
        Some(total) => unsafe { realloc(ptr, total) },
        None => {
            errno::set(ENOMEM);
            ptr::null_mut()
        }
    }
}

/// POSIX's `posix_memalign`: stores a block of `size` bytes aligned to `align` at `*out` and
/// returns 0; returns `EINVAL` if `align` is not a power of two and a multiple of the pointer
/// size, `ENOMEM` if there is no memory. On failure `*out` and `errno` are left alone.
///
/// # Safety
///
/// `out` is valid for a write of a pointer.
pub unsafe extern "C" fn posix_memalign(out: *mut *mut c_void, align: usize, size: usize) -> c_int {
    if !align.is_power_of_two() || align < size_of::<*mut c_void>() {
        return EINVAL;
    }
    let block = allocate(size, align, false);
    if block.is_null() {
        return ENOMEM;
    }
    // SAFETY: MOD-02.
    unsafe { out.write(block.cast()) };
    0
}

/// C's `aligned_alloc`. An alignment that is not a power of two fails with `EINVAL`, as in
/// glibc 2.38 and later; any size is accepted, as C17 allows.
pub extern "C" fn aligned_alloc(align: usize, size: usize) -> *mut c_void {
    if !align.is_power_of_two() {
        errno::set(EINVAL);
        return ptr::null_mut();
    }
    or_enomem(allocate(size, align, false))
}

/// The alignment `memalign` uses for `align`, as glibc reads it: zero is no constraint, and
/// any other value is rounded up to a power of two. `None` above the largest power of two.
fn memalign_alignment(align: usize) -> Option<usize> {
    align.max(1).checked_next_power_of_two()
}

/// The obsolete `memalign`, with glibc's lenient reading of the alignment.
pub extern "C" fn memalign(align: usize, size: usize) -> *mut c_void {
    match memalign_alignment(align) {
        Some(align) => or_enomem(allocate(size, align, false)),
        None => {
            errno::set(EINVAL);
            ptr::null_mut()
        }
    }
}

/// The obsolete `valloc`: a page-aligned block.
pub extern "C" fn valloc(size: usize) -> *mut c_void {
    or_enomem(allocate(size, PAGE, false))
}

/// The obsolete `pvalloc`: a page-aligned block of whole pages.
pub extern "C" fn pvalloc(size: usize) -> *mut c_void {
    match size.checked_next_multiple_of(PAGE) {
        Some(pages) => or_enomem(allocate(pages, PAGE, false)),
        None => {
            errno::set(ENOMEM);
            ptr::null_mut()
        }
    }
}

/// glibc's `malloc_usable_size`: 0 for null, the class size of a small block, and the size
/// last asked for, rounded up to a page, of a large one. Aborts for a pointer that is not a
/// block of the heap.
///
/// # Safety
///
/// `ptr` is null or a live block of the heap.
pub unsafe extern "C" fn malloc_usable_size(ptr: *mut c_void) -> usize {
    if ptr.is_null() {
        return 0;
    }
    match process::usable_size(ptr.cast::<u8>()) {
        Some(size) => size,
        None => {
            process::abort("malloc_usable_size of a pointer that is not a block of asmalloc's heap")
        }
    }
}

/// The crate's version, as a C string.
const VERSION: &CStr =
    match CStr::from_bytes_with_nul(concat!(env!("CARGO_PKG_VERSION"), "\0").as_bytes()) {
        Ok(version) => version,
        Err(_) => panic!("the crate version has an interior NUL"),
    };

/// asmalloc's own `const char *asmalloc_version(void)`: the crate version as a static,
/// NUL-terminated string. Exported with the C interface, so a program (or a benchmark probe)
/// can tell that asmalloc serves its `malloc`, and which version.
pub extern "C" fn asmalloc_version() -> *const c_char {
    VERSION.as_ptr()
}

/// glibc's `malloc_trim`: sends every parked object back to the bitmaps and returns the wholly
/// free pages to the OS (`collect`). The padding argument has no meaning here. Returns 1 if any
/// memory was released, else 0.
pub extern "C" fn malloc_trim(_pad: usize) -> c_int {
    c_int::from(process::collect() > 0)
}

/// Exports the C interface under its C names (`malloc`, `free` and the rest) from the crate
/// that invokes it, for a program or shared library that wants asmalloc as its C allocator.
/// Invoke it once per program, at the item level. `malloc`, `free` and `free_sized` are the
/// generated routines themselves (`front/generated/x86_64/exports.rs`, a copy of each under its C
/// name), so a call reaches the routine with no jump in between; every other export is a call of
/// the function of the same name in the crate's hidden `c` module, which inlines into it. What
/// the functions promise beyond C and POSIX is in that module's documentation and in
/// `docs/design.md`, "Interfaces".
///
/// Besides the C standard's functions and POSIX's `posix_memalign`, it exports every other
/// allocation function glibc 2.43 exports that takes or returns a block: `reallocarray`,
/// `memalign`, `valloc`, `pvalloc`, `malloc_usable_size`, `malloc_trim`, C23's sized frees and
/// the obsolete `cfree`. Otherwise glibc would serve one of them from its own heap, or be handed
/// a block of asmalloc's, and either is a crash. `mallinfo`, `mallinfo2`, `mallopt`,
/// `malloc_stats` and `malloc_info` take no block and are left to glibc, where they describe
/// glibc's own, unused heap. It also exports `asmalloc_version`, which tells a program (or a
/// benchmark probe) that asmalloc serves its `malloc`, and which version.
#[macro_export]
macro_rules! export_c_abi {
    () => {
        // `malloc`, `calloc`, `free` and `free_sized`: the generated routines, defined in this
        // crate. Their unsafe code is generated, so its ledger entry is cited here: C-EXPORT-02.
        $crate::asmalloc_exports_c_x86_64!();

        const _: () = {
            use ::core::ffi::{c_int, c_void};

            // SAFETY: C-EXPORT-01, the exported name.
            /// # Safety
            ///
            /// As for `asmalloc::c::realloc`.
            #[unsafe(no_mangle)]
            unsafe extern "C" fn realloc(ptr: *mut c_void, size: usize) -> *mut c_void {
                // SAFETY: C-EXPORT-01.
                unsafe { $crate::c::realloc(ptr, size) }
            }

            // SAFETY: C-EXPORT-01, the exported name.
            /// # Safety
            ///
            /// As for `asmalloc::c::reallocarray`.
            #[unsafe(no_mangle)]
            unsafe extern "C" fn reallocarray(
                ptr: *mut c_void,
                n: usize,
                size: usize,
            ) -> *mut c_void {
                // SAFETY: C-EXPORT-01.
                unsafe { $crate::c::reallocarray(ptr, n, size) }
            }

            // SAFETY: C-EXPORT-01, the exported name.
            /// # Safety
            ///
            /// As for `asmalloc::c::cfree`.
            #[unsafe(no_mangle)]
            unsafe extern "C" fn cfree(ptr: *mut c_void) {
                // SAFETY: C-EXPORT-01.
                unsafe { $crate::c::cfree(ptr) }
            }

            // SAFETY: C-EXPORT-01, the exported name.
            /// # Safety
            ///
            /// As for `asmalloc::c::free_aligned_sized`.
            #[unsafe(no_mangle)]
            unsafe extern "C" fn free_aligned_sized(ptr: *mut c_void, align: usize, size: usize) {
                // SAFETY: C-EXPORT-01.
                unsafe { $crate::c::free_aligned_sized(ptr, align, size) }
            }

            // SAFETY: C-EXPORT-01, the exported name.
            /// # Safety
            ///
            /// As for `asmalloc::c::posix_memalign`.
            #[unsafe(no_mangle)]
            unsafe extern "C" fn posix_memalign(
                out: *mut *mut c_void,
                align: usize,
                size: usize,
            ) -> c_int {
                // SAFETY: C-EXPORT-01.
                unsafe { $crate::c::posix_memalign(out, align, size) }
            }

            // SAFETY: C-EXPORT-01, the exported name.
            /// # Safety
            ///
            /// As for `asmalloc::c::aligned_alloc`.
            #[unsafe(no_mangle)]
            unsafe extern "C" fn aligned_alloc(align: usize, size: usize) -> *mut c_void {
                $crate::c::aligned_alloc(align, size)
            }

            // SAFETY: C-EXPORT-01, the exported name.
            /// # Safety
            ///
            /// As for `asmalloc::c::memalign`.
            #[unsafe(no_mangle)]
            unsafe extern "C" fn memalign(align: usize, size: usize) -> *mut c_void {
                $crate::c::memalign(align, size)
            }

            // SAFETY: C-EXPORT-01, the exported name.
            /// # Safety
            ///
            /// As for `asmalloc::c::valloc`.
            #[unsafe(no_mangle)]
            unsafe extern "C" fn valloc(size: usize) -> *mut c_void {
                $crate::c::valloc(size)
            }

            // SAFETY: C-EXPORT-01, the exported name.
            /// # Safety
            ///
            /// As for `asmalloc::c::pvalloc`.
            #[unsafe(no_mangle)]
            unsafe extern "C" fn pvalloc(size: usize) -> *mut c_void {
                $crate::c::pvalloc(size)
            }

            // SAFETY: C-EXPORT-01, the exported name.
            /// # Safety
            ///
            /// As for `asmalloc::c::malloc_usable_size`.
            #[unsafe(no_mangle)]
            unsafe extern "C" fn malloc_usable_size(ptr: *mut c_void) -> usize {
                // SAFETY: C-EXPORT-01.
                unsafe { $crate::c::malloc_usable_size(ptr) }
            }

            // SAFETY: C-EXPORT-01, the exported name.
            /// # Safety
            ///
            /// As for `asmalloc::c::malloc_trim`.
            #[unsafe(no_mangle)]
            unsafe extern "C" fn malloc_trim(pad: usize) -> c_int {
                $crate::c::malloc_trim(pad)
            }

            // SAFETY: C-EXPORT-01, the exported name.
            #[unsafe(no_mangle)]
            extern "C" fn asmalloc_version() -> *const ::core::ffi::c_char {
                $crate::c::asmalloc_version()
            }
        };
    };
}

#[cfg(feature = "c-abi")]
export_c_abi!();

#[cfg(test)]
mod tests;
