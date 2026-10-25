//! The control functions tikv-jemalloc-ctl 0.7 calls, each failing with `ENOENT` (no such
//! control), as jemalloc's own do for a name they do not know. See Cargo.toml.
#![allow(clippy::missing_safety_doc)]
use core::ffi::{c_char, c_int, c_void};

const ENOENT: c_int = 2;

pub unsafe fn mallctl(
    _name: *const c_char,
    _oldp: *mut c_void,
    _oldlenp: *mut usize,
    _newp: *mut c_void,
    _newlen: usize,
) -> c_int {
    ENOENT
}

pub unsafe fn mallctlnametomib(_name: *const c_char, _mibp: *mut usize, _miblenp: *mut usize) -> c_int {
    ENOENT
}

pub unsafe fn mallctlbymib(
    _mib: *const usize,
    _miblen: usize,
    _oldp: *mut c_void,
    _oldlenp: *mut usize,
    _newp: *mut c_void,
    _newlen: usize,
) -> c_int {
    ENOENT
}

pub unsafe fn malloc_stats_print(
    _write_cb: Option<unsafe extern "C" fn(*mut c_void, *const c_char)>,
    _cbopaque: *mut c_void,
    _opts: *const c_char,
) {
}
