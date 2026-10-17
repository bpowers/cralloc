//! The C functions under test against every conformance case. Miri's C allocation functions
//! are its own, not an implementation we ship or judge, so this runs natively only.
#![cfg(not(miri))]

use core::ffi::{c_int, c_void};

use asmalloc_testkit::conformance::{CASES, Verdict, run_all, run_case};
use asmalloc_testkit::subject::CFns;
use asmalloc_testkit::target;

#[test]
fn the_c_functions_meet_every_conformance_case() {
    let results = run_all(&target::c_fns());
    let mut failures = Vec::new();
    for (case, verdict) in &results {
        match verdict {
            Verdict::Pass => {}
            Verdict::Skip(why) => eprintln!("skipped {}: {why}", case.name),
            Verdict::Fail(how) => failures.push(format!("{}: {how}", case.name)),
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} cases failed:\n{}",
        failures.len(),
        results.len(),
        failures.join("\n")
    );
}

// Broken implementations, each wrong in one way, to show the cases notice.

unsafe extern "C" fn calloc_unzeroed(n: usize, size: usize) -> *mut c_void {
    let Some(total) = n.checked_mul(size) else {
        return core::ptr::null_mut();
    };
    // SAFETY: plain calls of the C library's functions.
    unsafe {
        let p = (CFns::LIBC.malloc)(total);
        if !p.is_null() {
            p.cast::<u8>().write_bytes(0xA5, total);
        }
        p
    }
}

unsafe extern "C" fn posix_memalign_lax(out: *mut *mut c_void, align: usize, size: usize) -> c_int {
    let align = align.next_power_of_two().max(size_of::<*mut c_void>());
    // SAFETY: a valid alignment for the C library's own function.
    unsafe { (CFns::LIBC.posix_memalign)(out, align, size) }
}

unsafe extern "C" fn malloc_clearing_errno(size: usize) -> *mut c_void {
    // SAFETY: plain calls of the C library's functions.
    unsafe {
        let p = (CFns::LIBC.malloc)(size);
        *(CFns::LIBC.errno_location)() = 0;
        p
    }
}

unsafe extern "C" fn realloc_short_copy(p: *mut c_void, size: usize) -> *mut c_void {
    // SAFETY: plain calls of the C library's functions on a live block (caller contract).
    unsafe {
        if p.is_null() || size == 0 {
            return (CFns::LIBC.realloc)(p, size);
        }
        let old = (CFns::LIBC.malloc_usable_size.unwrap())(p);
        let q = (CFns::LIBC.malloc)(size);
        if q.is_null() {
            return q;
        }
        core::ptr::copy_nonoverlapping(p.cast::<u8>(), q.cast::<u8>(), old.min(size) / 2);
        (CFns::LIBC.free)(p);
        q
    }
}

fn failing(fns: &CFns) -> Vec<&'static str> {
    CASES
        .iter()
        .filter(|case| matches!(run_case(fns, case), Verdict::Fail(_)))
        .map(|case| case.name)
        .collect()
}

#[test]
fn the_cases_catch_broken_implementations() {
    let unzeroed = failing(&CFns {
        calloc: calloc_unzeroed,
        ..CFns::LIBC
    });
    assert!(
        unzeroed.contains(&"calloc(16, 16) is zeroed"),
        "{unzeroed:?}"
    );
    assert!(
        unzeroed.contains(&"calloc of recycled 4 KiB is zeroed"),
        "{unzeroed:?}"
    );
    assert!(
        unzeroed.contains(&"calloc(SIZE_MAX, 2) overflows to ENOMEM"),
        "errno not set: {unzeroed:?}"
    );

    let lax = failing(&CFns {
        posix_memalign: posix_memalign_lax,
        ..CFns::LIBC
    });
    assert!(
        lax.contains(&"posix_memalign alignment 24 is EINVAL"),
        "{lax:?}"
    );
    assert!(
        lax.contains(&"posix_memalign alignment 0 is EINVAL"),
        "{lax:?}"
    );
    assert!(!lax.contains(&"posix_memalign(64, 64)"), "{lax:?}");

    let clearing = failing(&CFns {
        malloc: malloc_clearing_errno,
        ..CFns::LIBC
    });
    assert_eq!(
        clearing,
        [
            "malloc(SIZE_MAX) fails with ENOMEM",
            "malloc(PTRDIFF_MAX + 1) fails with ENOMEM",
            "malloc(2^62) fails with ENOMEM"
        ]
    );

    let short_copy = failing(&CFns {
        realloc: realloc_short_copy,
        ..CFns::LIBC
    });
    assert!(
        short_copy.contains(&"realloc 24 to 25 keeps 24"),
        "{short_copy:?}"
    );
    assert!(
        !short_copy.contains(&"realloc(NULL, 24) is malloc(24)"),
        "{short_copy:?}"
    );
}
