//! The dynamic loader's symbol lookups, for the C++ layer: the new-handler, and whether a
//! program replaced an operator. With `errno.rs`, the only references to a C library in
//! asmalloc. The ledger entry is `docs/safety/asmalloc/c/loader-safety.md`.
//!
//! Every function here takes the loader's lock, which is recursive, and may allocate (an error
//! string for a missing symbol), which reaches asmalloc again. They run only on slow paths that
//! hold nothing of the allocator's, so a nested call is an ordinary one. They are the one wait
//! on another thread in the allocator: a thread in the middle of a `dlopen` holds the lock.

use core::ffi::{CStr, c_char, c_int, c_void};
use core::ptr;

/// `Dl_info`.
#[repr(C)]
struct Info {
    file: *const c_char,
    base: *mut c_void,
    symbol: *const c_char,
    address: *mut c_void,
}

// SAFETY: LOADER-01. glibc 2.34 and later define both in libc.so.6.
#[link(name = "c")]
unsafe extern "C" {
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn dladdr(address: *const c_void, info: *mut Info) -> c_int;
}

/// `RTLD_DEFAULT`: the global scope, in load order.
const RTLD_DEFAULT: *mut c_void = ptr::null_mut();
/// `RTLD_NEXT`: the objects after the caller's.
const RTLD_NEXT: *mut c_void = ptr::without_provenance_mut(usize::MAX);

fn lookup(handle: *mut c_void, name: &CStr) -> Option<usize> {
    // SAFETY: LOADER-02.
    let found = unsafe { dlsym(handle, name.as_ptr()) };
    (!found.is_null()).then(|| found.expose_provenance())
}

/// The first definition of `name` in the process: what a call the program makes binds to.
pub(super) fn first(name: &CStr) -> Option<usize> {
    lookup(RTLD_DEFAULT, name)
}

/// The first definition of `name` in the objects loaded after this one: what the program
/// would bind to if this object did not define it.
pub(super) fn next(name: &CStr) -> Option<usize> {
    lookup(RTLD_NEXT, name)
}

/// The base of the loaded object that holds `address`, if any does.
fn object_of(address: usize) -> Option<usize> {
    let mut info = Info {
        file: ptr::null(),
        base: ptr::null_mut(),
        symbol: ptr::null(),
        address: ptr::null_mut(),
    };
    // SAFETY: LOADER-03.
    let found = unsafe { dladdr(ptr::without_provenance(address), &mut info) };
    (found != 0).then(|| info.base.addr())
}

/// Whether the function at `address` is defined in the object that holds the allocator.
pub(super) fn is_ours(address: usize) -> bool {
    let here = is_ours as fn(usize) -> bool as usize;
    object_of(address).is_some_and(|base| object_of(here) == Some(base))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookups_find_the_c_library_outside_this_test_binary() {
        let malloc = first(c"malloc").expect("the C library's malloc");
        assert!(!is_ours(malloc));
        assert_eq!(next(c"malloc"), Some(malloc));
        assert_eq!(first(c"asmalloc_no_such_symbol"), None);
        assert!(is_ours(object_of as fn(usize) -> Option<usize> as usize));
    }
}
