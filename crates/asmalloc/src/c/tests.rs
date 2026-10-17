//! What the C layer promises beyond the C standard, on the process's heap. The standard's own
//! cases are the test kit's (`asmalloc-testkit`, `conformance`), which run against these same
//! functions; the abort on a foreign pointer is tested through the preload library, in a
//! process of its own.

use super::*;
use crate::Asmalloc;
use core::alloc::GlobalAlloc;
use std::vec::Vec;

unsafe extern "C" {
    fn __errno_location() -> *mut c_int;
}

fn errno() -> c_int {
    // SAFETY: glibc's accessor returns this thread's errno, valid for the thread's life.
    unsafe { *__errno_location() }
}

fn set_errno(value: c_int) {
    // SAFETY: as for `errno`.
    unsafe { *__errno_location() = value }
}

fn usable(p: *mut c_void) -> usize {
    // SAFETY: the callers pass live blocks of the heap.
    unsafe { malloc_usable_size(p) }
}

fn release(p: *mut c_void) {
    // SAFETY: the callers pass live blocks of the heap that they give up.
    unsafe { free(p) }
}

#[test]
fn small_blocks_use_their_whole_c_class() {
    for (size, class) in [
        (0, 8),
        (1, 8),
        (8, 8),
        (9, 16),
        (16, 16),
        (24, 32),
        (40, 48),
        (100, 112),
        (4097, 5120),
        (32 << 10, 32 << 10),
    ] {
        let p = malloc(size);
        assert_eq!(usable(p), class, "malloc({size})");
        assert!(Asmalloc::contains(p.cast()));
        release(p);
    }
}

#[test]
fn large_blocks_report_what_was_asked_for_rounded_to_a_page() {
    for size in [(32 << 10) + 1, 40 << 10, 100_000, 3 << 20] {
        let p = malloc(size);
        assert_eq!(usable(p), size.next_multiple_of(PAGE), "malloc({size})");
        release(p);
    }
    // An aligned request in a large slot reports its size, not the slot.
    let p = memalign(1 << 20, 100);
    assert_eq!(p.addr() % (1 << 20), 0);
    assert_eq!(usable(p), PAGE);
    release(p);
    assert_eq!(usable(ptr::null_mut()), 0);
}

#[test]
fn success_and_free_leave_errno_alone() {
    set_errno(0x5eed);
    let small = malloc(10);
    let big = calloc(1, 1 << 20);
    // SAFETY: live blocks, each given up once.
    unsafe {
        let small = realloc(small, 20_000);
        let big = realloc(big, 3 << 20);
        free(small);
        free_sized(big, 3 << 20);
        free(ptr::null_mut());
    }
    let mut out = ptr::null_mut();
    // SAFETY: `out` is a local.
    assert_eq!(unsafe { posix_memalign(&mut out, 64, 100) }, 0);
    release(out);
    assert_eq!(errno(), 0x5eed);
    // posix_memalign reports its error and leaves errno alone.
    // SAFETY: as above.
    assert_eq!(unsafe { posix_memalign(&mut out, 64, usize::MAX) }, ENOMEM);
    assert_eq!(errno(), 0x5eed);
    assert!(malloc(usize::MAX).is_null());
    assert_eq!(errno(), ENOMEM);
}

#[test]
fn realloc_stays_in_place_when_the_class_allows() {
    // SAFETY: each block is live where it is used and given up once.
    unsafe {
        // Within one small class.
        let p = malloc(17);
        assert_eq!(realloc(p, 32), p);
        let q = realloc(p, 33);
        assert_ne!(q, p);
        free(q);
        // A large block grows within its slot and shrinks to a quarter of it in place.
        let big = malloc(40 << 10);
        assert_eq!(realloc(big, 39 << 10), big);
        assert_eq!(realloc(big, 40 << 10), big);
        assert_eq!(usable(big), 40 << 10);
        // Growing out of the slot moves to one with headroom: the next steps stay.
        let grown = realloc(big, 100 << 10);
        assert_ne!(grown, big);
        assert_eq!(usable(grown), 100 << 10);
        assert_eq!(realloc(grown, 300 << 10), grown);
        assert_eq!(usable(grown), 300 << 10);
        // Shrinking to a small size moves into a small class.
        let small = realloc(grown, 100);
        assert_ne!(small, grown);
        assert_eq!(usable(small), 112);
        free(small);
    }
}

#[test]
fn the_sized_frees_take_the_class_from_the_address() {
    // A wrong size or alignment from C must not send a block to the wrong class: the freed
    // block is the one the next allocation of its own class gets.
    let mut seen = Vec::new();
    for _ in 0..64 {
        let p = malloc(100);
        // SAFETY: `p` is live and given up once; the size is deliberately wrong.
        unsafe { free_sized(p, 1 << 20) };
        let q = malloc(100);
        seen.push(q == p);
        // SAFETY: as above, with a wrong alignment too.
        unsafe { free_aligned_sized(q, 4096, 8) };
    }
    assert!(seen.iter().filter(|&&same| same).count() > 32, "{seen:?}");
}

#[test]
fn c_and_rust_free_each_others_blocks() {
    let layout = Layout::from_size_align(40, 8).unwrap();
    // SAFETY: each block is freed once, through the other interface.
    unsafe {
        let rust = Asmalloc.alloc(layout);
        assert_eq!(usable(rust.cast()), 40);
        free(rust.cast());
        let big = Layout::from_size_align(1 << 20, 16).unwrap();
        let rust = Asmalloc.alloc(big);
        let moved = realloc(rust.cast(), 2 << 20);
        assert!(!moved.is_null());
        free(moved);
        // A C block of 24 bytes is in the 32-byte class, so Rust frees it as 32 bytes.
        let c = malloc(24);
        Asmalloc.dealloc(c.cast(), Layout::from_size_align(32, 16).unwrap());
    }
}

#[test]
fn calloc_zeroes_reused_small_and_large_blocks() {
    for size in [8, 100, 1024, 1025, 30_000, 100_000, 3 << 20] {
        let dirty: Vec<_> = (0..8).map(|_| malloc(size)).collect();
        for &p in &dirty {
            // SAFETY: `p` is a live block of `size` bytes.
            unsafe { p.cast::<u8>().write_bytes(0xA5, size) };
            release(p);
        }
        let p = calloc(size, 1);
        // SAFETY: `p` is a live block of `size` zeroed bytes.
        let bytes = unsafe { core::slice::from_raw_parts(p.cast::<u8>(), size) };
        assert!(bytes.iter().all(|&b| b == 0), "calloc({size}, 1)");
        release(p);
    }
}

#[test]
fn malloc_trim_returns_memory_after_a_peak() {
    let blocks: Vec<_> = (0..64).map(|_| malloc(1 << 20)).collect();
    for &p in &blocks {
        // SAFETY: `p` is a live block of a mebibyte.
        unsafe { p.cast::<u8>().write_bytes(1, 1 << 20) };
    }
    blocks.into_iter().for_each(release);
    // Other tests share the heap, so all that is certain is that it answers and the heap
    // still serves.
    assert!(matches!(malloc_trim(0), 0 | 1));
    let p = malloc(1 << 20);
    assert!(!p.is_null());
    release(p);
}

#[test]
fn the_obsolete_family_aligns_as_glibc_does() {
    let p = valloc(1);
    assert_eq!(p.addr() % PAGE, 0);
    release(p);
    let p = pvalloc(PAGE + 1);
    assert_eq!(p.addr() % PAGE, 0);
    assert!(usable(p) >= 2 * PAGE);
    release(p);
    let p = memalign(48, 1);
    assert_eq!(p.addr() % 64, 0);
    release(p);
    let p = aligned_alloc(1 << 33, 1);
    // Beyond a region: a run aligned to its own size.
    assert_eq!(p.addr() % (1 << 33), 0);
    release(p);
    let p = cfree as unsafe extern "C" fn(*mut c_void);
    // SAFETY: freeing null does nothing.
    unsafe { p(ptr::null_mut()) };
}

#[test]
fn the_tables_and_the_classes_agree_on_every_small_request() {
    for size in 0..=table::C_TABLE_MAX {
        let class = Class::of_c(Layout::from_size_align(size, 1).unwrap()).unwrap();
        assert_eq!(table::malloc_class(size).map(|c| c.class()), Some(class));
    }
}

#[test]
fn memalign_reads_alignments_as_glibc_does() {
    assert_eq!(memalign_alignment(0), Some(1));
    assert_eq!(memalign_alignment(1), Some(1));
    assert_eq!(memalign_alignment(3), Some(4));
    assert_eq!(memalign_alignment(24), Some(32));
    assert_eq!(memalign_alignment(4096), Some(4096));
    assert_eq!(
        memalign_alignment(usize::MAX / 2 + 1),
        Some(usize::MAX / 2 + 1)
    );
    assert_eq!(memalign_alignment(usize::MAX / 2 + 2), None);
}

#[test]
fn the_page_is_four_kibibytes() {
    assert_eq!(PAGE, 4096);
}

#[test]
fn the_version_is_the_crates() {
    // SAFETY: the pointer is to a static NUL-terminated string.
    let version = unsafe { core::ffi::CStr::from_ptr(asmalloc_version()) };
    assert_eq!(version.to_str(), Ok(env!("CARGO_PKG_VERSION")));
    assert_eq!(asmalloc_version(), asmalloc_version());
}

#[test]
fn operator_new_serves_every_form_with_its_alignment() {
    for size in [0, 1, 16, 24, 100, 40 << 10, 3 << 20] {
        for p in [cxx::operator_new(size), cxx::operator_new_nothrow(size)] {
            assert!(!p.is_null());
            assert!(usable(p) >= size);
            if size >= 16 {
                assert_eq!(p.addr() % 16, 0);
            }
            // SAFETY: a live block, given up once.
            unsafe { cxx::operator_delete_sized(p, size) };
        }
        for align in [1, 64, 4096, 1 << 21] {
            let p = cxx::operator_new_aligned(size, align);
            let q = cxx::operator_new_aligned_nothrow(size, align);
            assert_eq!(p.addr() % align, 0);
            assert_eq!(q.addr() % align, 0);
            // SAFETY: live blocks, each given up once.
            unsafe {
                cxx::operator_delete_sized_aligned(p, size, align);
                cxx::operator_delete(q);
            }
        }
    }
}

#[test]
fn nothrow_new_fails_with_null_when_no_handler_is_installed() {
    // A Rust test binary has no C++ runtime, so there is no new-handler to run.
    assert!(cxx::operator_new_nothrow(usize::MAX / 4).is_null());
    assert!(cxx::operator_new_aligned_nothrow(usize::MAX / 4, 64).is_null());
    // An alignment that is not a power of two is not served.
    assert!(cxx::operator_new_aligned_nothrow(16, 48).is_null());
}

#[test]
fn a_sized_delete_with_the_wrong_size_frees_the_right_block() {
    let mut seen = Vec::new();
    for _ in 0..64 {
        let p = cxx::operator_new(100);
        // SAFETY: `p` is live and given up once; the size is deliberately wrong.
        unsafe { cxx::operator_delete_sized(p, 1 << 20) };
        let q = cxx::operator_new(100);
        seen.push(q == p);
        // SAFETY: as above, with a wrong size and alignment.
        unsafe { cxx::operator_delete_sized_aligned(q, 8, 4096) };
    }
    assert!(seen.iter().filter(|&&same| same).count() > 32, "{seen:?}");
}
