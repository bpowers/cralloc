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

/// Whether a large block of `size` bytes at `p` reports as usable what it asked for, to the
/// end of the page it ends in: a large block may start a stagger into its first page.
fn reports(p: *mut c_void, size: usize) -> bool {
    let n = usable(p);
    n >= size && n < size + PAGE && (p.addr() + n).is_multiple_of(PAGE)
}

#[test]
fn large_blocks_report_what_was_asked_for_rounded_to_a_page() {
    for size in [(32 << 10) + 1, 40 << 10, 100_000, 3 << 20] {
        let p = malloc(size);
        assert!(reports(p, size), "malloc({size})");
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
        // A large block grows within its slot and shrinks in place.
        let big = malloc(40 << 10);
        assert_eq!(realloc(big, 39 << 10), big);
        assert_eq!(realloc(big, 40 << 10), big);
        assert!(reports(big, 40 << 10));
        // Growing out of the slot moves to one with headroom: the next steps stay.
        let grown = realloc(big, 100 << 10);
        assert_ne!(grown, big);
        assert!(reports(grown, 100 << 10));
        assert_eq!(realloc(grown, 300 << 10), grown);
        assert!(reports(grown, 300 << 10));
        // Shrinking to a small size moves into a small class.
        let small = realloc(grown, 100);
        assert_ne!(small, grown);
        assert_eq!(usable(small), 112);
        free(small);
    }
}

#[test]
fn realloc_keeps_every_usable_byte_when_it_moves() {
    // Programs write up to `malloc_usable_size` (redis sizes its strings by it), so a move must
    // carry that much, not just the size last asked for.
    for size in [100, 5000, 40_000, 100_000, 300_000, 1_000_000, 5 << 20] {
        let p = malloc(size).cast::<u8>();
        let n = usable(p.cast());
        assert!(n >= size);
        // SAFETY: `p` is live with `n` usable bytes; `q` is live with at least `4 * n`, and each
        // block is given up once.
        unsafe {
            p.write_bytes(0xab, n);
            let q = realloc(p.cast(), 4 * n).cast::<u8>();
            assert!(!q.is_null());
            let kept = core::slice::from_raw_parts(q, n);
            assert!(
                kept.iter().all(|&b| b == 0xab),
                "malloc({size}): usable {n}, lost from {:?}",
                kept.iter().position(|&b| b != 0xab)
            );
            release(q.cast());
        }
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

/// A large block cut down by `realloc` stays where it is and gives its tail back: grown again
/// in place, the kept part still holds its bytes and the rest reads as zero, which it would
/// not if the pages had stayed. A later `realloc` that moves it copies all of that.
#[test]
fn realloc_shrinking_a_large_block_returns_its_tail() {
    let (big, small) = (4 << 20, (3 << 19) + 1);
    // SAFETY: each block is live with the size used where it is read or written, and given up
    // once.
    unsafe {
        let p = malloc(big).cast::<u8>();
        p.write_bytes(0xab, big);
        let q = realloc(p.cast(), small).cast::<u8>();
        assert_eq!(
            q, p,
            "a shrink to over an eighth of the slot stays in place"
        );
        let kept = usable(q.cast());
        assert!(kept >= small && kept < small + 4096);
        let r = realloc(q.cast(), big).cast::<u8>();
        assert_eq!(r, p, "it grows back within its slot");
        let bytes = core::slice::from_raw_parts(r, big);
        assert!(bytes[..kept].iter().all(|&b| b == 0xab));
        assert!(
            bytes[kept..].iter().all(|&b| b == 0),
            "the tail kept its old pages"
        );
        let moved = realloc(r.cast(), 8 * big).cast::<u8>();
        assert_ne!(moved, r);
        let bytes = core::slice::from_raw_parts(moved, big);
        assert!(bytes[..kept].iter().all(|&b| b == 0xab));
        assert!(bytes[kept..].iter().all(|&b| b == 0));
        free(moved.cast());
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

/// A megabyte is a class. One byte more outgrows the slot and moves to a slot with four times
/// the room; cut back to a megabyte, the block still fills a fifth of that slot, more than the
/// eighth under which a shrink moves, so it stays, and so does every later step.
#[test]
fn a_block_resized_back_and_forth_across_a_class_boundary_settles() {
    let size = 1 << 20;
    let mut moves = 0;
    // SAFETY: the block is live where it is resized and given up once.
    unsafe {
        let mut p = malloc(size);
        for _ in 0..10 {
            for next in [size + 1, size] {
                let q = realloc(p, next);
                moves += usize::from(q != p);
                p = q;
            }
        }
        free(p);
    }
    assert!(moves <= 2, "{moves} of 20 calls moved the megabyte");
}

#[test]
fn calloc_zeroes_all_it_reports_usable() {
    // As glibc's calloc does: a program that sizes a block by malloc_usable_size (redis does)
    // may rely on the whole of it reading as zero.
    for size in [1, 17, 100, 1000, 1025, 30_000, 40_001, 100_001] {
        let dirty: Vec<_> = (0..16).map(|_| malloc(size)).collect();
        for &p in &dirty {
            // SAFETY: a live block, all of whose usable bytes are the caller's.
            unsafe { p.cast::<u8>().write_bytes(0xA5, usable(p)) };
            release(p);
        }
        for p in [calloc(1, size), calloc(size, 1)] {
            let len = usable(p);
            // SAFETY: a live block of `len` usable bytes.
            let bytes = unsafe { core::slice::from_raw_parts(p.cast::<u8>(), len) };
            assert!(
                bytes.iter().all(|&b| b == 0),
                "calloc of {size} bytes left dirt in its {len} usable"
            );
            release(p);
        }
    }
}

/// The generated routines, which the export macros bind `malloc`, `free` and `free_sized` to.
mod generated {
    use super::super::routines;
    use super::*;

    #[test]
    fn they_serve_every_c_class_and_large_blocks() {
        for size in [0, 1, 8, 9, 24, 100, 4097, 32 << 10, (32 << 10) + 1, 100_000] {
            let p = routines::malloc(size);
            assert!(
                !p.is_null() && Asmalloc::contains(p.cast()),
                "malloc({size})"
            );
            assert!(usable(p) >= size);
            if size >= 16 {
                assert_eq!(p.addr() % 16, 0, "malloc({size})");
            }
            // SAFETY: a live block, given up once.
            unsafe { routines::free(p) };
        }
        // SAFETY: null is always freeable.
        unsafe {
            routines::free(ptr::null_mut());
            routines::free_sized(ptr::null_mut(), 16);
        }
    }

    #[test]
    fn they_pair_with_the_slow_paths_both_ways() {
        let a = routines::malloc(200);
        let b = slow::malloc_slow(200);
        // SAFETY: live blocks, each given up once, by the other path.
        unsafe {
            slow::free_slow(a);
            routines::free(b);
        }
        let c = routines::malloc(3000);
        // SAFETY: as above, with the size it was allocated with.
        unsafe { routines::free_sized(c, 3000) };
    }

    /// Two magazines' worth of every small class, so that each class's routines miss into the
    /// heap's slow paths (which write the masks the routines read) and then hit.
    #[test]
    fn they_serve_every_small_class_through_their_slow_paths() {
        for class in crate::classes::SmallClass::all() {
            let size = class.size();
            if table::malloc_class(size) != Some(class) {
                continue;
            }
            let held: Vec<*mut c_void> = (0..2 * class.magazine().slots())
                .map(|_| routines::malloc(size))
                .collect();
            for &p in &held {
                assert!(!p.is_null());
                assert_eq!(usable(p), size);
            }
            for p in held {
                // SAFETY: each block is live and given up once.
                unsafe { routines::free_sized(p, size) };
            }
        }
    }

    #[test]
    fn a_wrong_size_frees_what_free_would() {
        // Warm the class so that the next allocation is served from the magazine.
        let p = routines::malloc(48);
        let class = usable(p);
        // SAFETY: a live block, given up once, with a size of another class.
        unsafe { routines::free_sized(p, 16) };
        let q = routines::malloc(48);
        assert_eq!(usable(q), class);
        // SAFETY: as above, with a size above every small class.
        unsafe { routines::free_sized(q, 1 << 20) };
        let r = routines::malloc(48);
        assert_eq!(usable(r), class);
        // SAFETY: as above, with its own size.
        unsafe { routines::free_sized(r, 48) };
    }

    #[test]
    fn a_freed_block_is_the_next_one_handed_out_on_the_same_cpu() {
        // On one CPU the magazine is a stack. A thread may migrate between the calls, so the
        // test asks only that it happens most of the time.
        let mut same = 0;
        for _ in 0..1000 {
            let p = routines::malloc(64);
            // SAFETY: a live block, given up once.
            unsafe { routines::free(p) };
            let q = routines::malloc(64);
            same += usize::from(p == q);
            // SAFETY: a live block, given up once.
            unsafe { routines::free_sized(q, 64) };
        }
        assert!(same > 900, "{same}");
    }
}
