//! The C allocation functions.

use core::ffi::{c_int, c_void};
use core::ptr::{self, NonNull};

use super::{Grant, Held, Refused, Request, Resized, Subject, ZeroSize};

/// The largest fundamental alignment on x86-64 (`alignof(max_align_t)`).
pub const MAX_ALIGN: usize = 16;

/// The alignment C promises for `malloc(size)`: suitable for any object with a fundamental
/// alignment that fits in `size` bytes (C23 7.24.3). That is 16 for 16 bytes or more and the
/// largest power of two not above `size` below that, so `malloc(8)` may be 8-aligned.
pub fn c_default_align(size: usize) -> usize {
    if size == 0 {
        return 1;
    }
    let below = 1usize << (usize::BITS - 1 - size.leading_zeros());
    below.min(MAX_ALIGN)
}

/// One implementation of the C allocation functions.
///
/// [`CFns`] implements it from a table of function pointers; [`CFns::LIBC`] is the C library's.
/// Every method is the C function of the same name, with its contract.
pub trait CAbi: Sync {
    /// Name used in reports.
    fn name(&self) -> String;

    /// # Safety
    /// As C's `malloc`.
    unsafe fn malloc(&self, size: usize) -> *mut c_void;

    /// # Safety
    /// As C's `calloc`.
    unsafe fn calloc(&self, n: usize, size: usize) -> *mut c_void;

    /// # Safety
    /// As C's `realloc`.
    unsafe fn realloc(&self, p: *mut c_void, size: usize) -> *mut c_void;

    /// # Safety
    /// As C's `free`.
    unsafe fn free(&self, p: *mut c_void);

    /// # Safety
    /// As POSIX `posix_memalign`.
    unsafe fn posix_memalign(&self, out: *mut *mut c_void, align: usize, size: usize) -> c_int;

    /// # Safety
    /// As C's `aligned_alloc`.
    unsafe fn aligned_alloc(&self, align: usize, size: usize) -> *mut c_void;

    /// `None` when the implementation has no `malloc_usable_size`.
    ///
    /// # Safety
    /// As glibc's `malloc_usable_size`.
    unsafe fn malloc_usable_size(&self, p: *mut c_void) -> Option<usize>;

    /// Whether C23's `free_sized` and `free_aligned_sized` are available.
    fn has_sized_free(&self) -> bool;

    /// Whether `reallocarray` is available.
    fn has_reallocarray(&self) -> bool;

    /// Whether `memalign`, `valloc` and `pvalloc` are available.
    fn has_obsolete(&self) -> bool;

    /// `None` when the implementation has no `reallocarray`.
    ///
    /// # Safety
    /// As glibc's `reallocarray`.
    unsafe fn reallocarray(&self, p: *mut c_void, n: usize, size: usize) -> Option<*mut c_void>;

    /// `None` when the implementation lacks the obsolete aligned functions.
    ///
    /// # Safety
    /// As glibc's `memalign`.
    unsafe fn memalign(&self, align: usize, size: usize) -> Option<*mut c_void>;

    /// `None` when the implementation lacks the obsolete aligned functions.
    ///
    /// # Safety
    /// As glibc's `valloc`.
    unsafe fn valloc(&self, size: usize) -> Option<*mut c_void>;

    /// `None` when the implementation lacks the obsolete aligned functions.
    ///
    /// # Safety
    /// As glibc's `pvalloc`.
    unsafe fn pvalloc(&self, size: usize) -> Option<*mut c_void>;

    /// # Safety
    /// As C23's `free_sized`; [`has_sized_free`](Self::has_sized_free) is true.
    unsafe fn free_sized(&self, p: *mut c_void, size: usize);

    /// # Safety
    /// As C23's `free_aligned_sized`; [`has_sized_free`](Self::has_sized_free) is true.
    unsafe fn free_aligned_sized(&self, p: *mut c_void, align: usize, size: usize);

    /// The calling thread's `errno`.
    ///
    /// # Safety
    /// The implementation's errno accessor must be callable on this thread.
    unsafe fn errno(&self) -> c_int;

    /// Set the calling thread's `errno`.
    ///
    /// # Safety
    /// As for [`errno`](Self::errno).
    unsafe fn set_errno(&self, value: c_int);
}

/// The C functions of one implementation, as function pointers. Build one for another
/// implementation (cralloc's exports, or symbols looked up in a preload library) to point the
/// C adapter and the conformance cases at it.
#[derive(Clone, Copy, Debug)]
pub struct CFns {
    /// Name used in reports.
    pub name: &'static str,
    /// `malloc`.
    pub malloc: unsafe extern "C" fn(usize) -> *mut c_void,
    /// `calloc`.
    pub calloc: unsafe extern "C" fn(usize, usize) -> *mut c_void,
    /// `realloc`.
    pub realloc: unsafe extern "C" fn(*mut c_void, usize) -> *mut c_void,
    /// `free`.
    pub free: unsafe extern "C" fn(*mut c_void),
    /// `posix_memalign`.
    pub posix_memalign: unsafe extern "C" fn(*mut *mut c_void, usize, usize) -> c_int,
    /// `aligned_alloc`.
    pub aligned_alloc: unsafe extern "C" fn(usize, usize) -> *mut c_void,
    /// `malloc_usable_size`, if provided.
    pub malloc_usable_size: Option<unsafe extern "C" fn(*mut c_void) -> usize>,
    /// `free_sized` and `free_aligned_sized`, if provided.
    pub sized_free: Option<SizedFree>,
    /// `reallocarray`, if provided.
    pub reallocarray: Option<unsafe extern "C" fn(*mut c_void, usize, usize) -> *mut c_void>,
    /// `memalign`, `valloc` and `pvalloc`, if provided.
    pub obsolete: Option<Obsolete>,
    /// Where the calling thread's `errno` lives (glibc's `__errno_location`). errno belongs to
    /// the C library, so an allocator that sets it must use the same location.
    pub errno_location: unsafe extern "C" fn() -> *mut c_int,
}

/// C23's sized frees.
#[derive(Clone, Copy, Debug)]
pub struct SizedFree {
    /// `free_sized`.
    pub free_sized: unsafe extern "C" fn(*mut c_void, usize),
    /// `free_aligned_sized`.
    pub free_aligned_sized: unsafe extern "C" fn(*mut c_void, usize, usize),
}

/// The obsolete aligned allocation functions glibc still exports.
#[derive(Clone, Copy, Debug)]
pub struct Obsolete {
    /// `memalign`.
    pub memalign: unsafe extern "C" fn(usize, usize) -> *mut c_void,
    /// `valloc`.
    pub valloc: unsafe extern "C" fn(usize) -> *mut c_void,
    /// `pvalloc`.
    pub pvalloc: unsafe extern "C" fn(usize) -> *mut c_void,
}

mod libc {
    use core::ffi::{c_int, c_void};

    unsafe extern "C" {
        pub fn malloc(size: usize) -> *mut c_void;
        pub fn calloc(n: usize, size: usize) -> *mut c_void;
        pub fn realloc(p: *mut c_void, size: usize) -> *mut c_void;
        pub fn free(p: *mut c_void);
        pub fn posix_memalign(out: *mut *mut c_void, align: usize, size: usize) -> c_int;
        pub fn aligned_alloc(align: usize, size: usize) -> *mut c_void;
        pub fn malloc_usable_size(p: *mut c_void) -> usize;
        #[cfg(not(any(miri, target_env = "musl")))]
        pub fn free_sized(p: *mut c_void, size: usize);
        #[cfg(not(any(miri, target_env = "musl")))]
        pub fn free_aligned_sized(p: *mut c_void, align: usize, size: usize);
        #[cfg(not(miri))]
        pub fn reallocarray(p: *mut c_void, n: usize, size: usize) -> *mut c_void;
        #[cfg(not(any(miri, target_env = "musl")))]
        pub fn memalign(align: usize, size: usize) -> *mut c_void;
        #[cfg(not(any(miri, target_env = "musl")))]
        pub fn valloc(size: usize) -> *mut c_void;
        #[cfg(not(any(miri, target_env = "musl")))]
        pub fn pvalloc(size: usize) -> *mut c_void;
        pub fn __errno_location() -> *mut c_int;
    }
}

impl CFns {
    /// The C library this process links (glibc 2.43 or later, or musl, which has neither the
    /// sized frees nor `pvalloc`, so the kit treats it as lacking them and the obsolete
    /// group). Under Miri, Miri's functions, which lack those too, and `reallocarray`.
    pub const LIBC: CFns = CFns {
        name: "libc",
        malloc: libc::malloc,
        calloc: libc::calloc,
        realloc: libc::realloc,
        free: libc::free,
        posix_memalign: libc::posix_memalign,
        #[cfg(not(miri))]
        aligned_alloc: libc::aligned_alloc,
        #[cfg(miri)]
        aligned_alloc: aligned_alloc_rounded,
        malloc_usable_size: Some(libc::malloc_usable_size),
        #[cfg(not(any(miri, target_env = "musl")))]
        sized_free: Some(SizedFree {
            free_sized: libc::free_sized,
            free_aligned_sized: libc::free_aligned_sized,
        }),
        #[cfg(any(miri, target_env = "musl"))]
        sized_free: None,
        #[cfg(not(miri))]
        reallocarray: Some(libc::reallocarray),
        #[cfg(miri)]
        reallocarray: None,
        #[cfg(not(any(miri, target_env = "musl")))]
        obsolete: Some(Obsolete {
            memalign: libc::memalign,
            valloc: libc::valloc,
            pvalloc: libc::pvalloc,
        }),
        #[cfg(any(miri, target_env = "musl"))]
        obsolete: None,
        errno_location: libc::__errno_location,
    };
}

/// `aligned_alloc` with the size rounded up to a multiple of the alignment. C11 required that;
/// C17 dropped it, but Miri's `aligned_alloc` still refuses other sizes. The conformance cases,
/// which do not run under Miri, check the C17 rule.
#[cfg(miri)]
unsafe extern "C" fn aligned_alloc_rounded(align: usize, size: usize) -> *mut c_void {
    // SAFETY: same contract.
    unsafe { libc::aligned_alloc(align, size.next_multiple_of(align.max(1))) }
}

impl CAbi for CFns {
    fn name(&self) -> String {
        self.name.to_string()
    }

    unsafe fn malloc(&self, size: usize) -> *mut c_void {
        // SAFETY: same contract.
        unsafe { (self.malloc)(size) }
    }

    unsafe fn calloc(&self, n: usize, size: usize) -> *mut c_void {
        // SAFETY: same contract.
        unsafe { (self.calloc)(n, size) }
    }

    unsafe fn realloc(&self, p: *mut c_void, size: usize) -> *mut c_void {
        // SAFETY: same contract.
        unsafe { (self.realloc)(p, size) }
    }

    unsafe fn free(&self, p: *mut c_void) {
        // SAFETY: same contract.
        unsafe { (self.free)(p) }
    }

    unsafe fn posix_memalign(&self, out: *mut *mut c_void, align: usize, size: usize) -> c_int {
        // SAFETY: same contract.
        unsafe { (self.posix_memalign)(out, align, size) }
    }

    unsafe fn aligned_alloc(&self, align: usize, size: usize) -> *mut c_void {
        // SAFETY: same contract.
        unsafe { (self.aligned_alloc)(align, size) }
    }

    unsafe fn malloc_usable_size(&self, p: *mut c_void) -> Option<usize> {
        // SAFETY: same contract.
        self.malloc_usable_size.map(|f| unsafe { f(p) })
    }

    fn has_sized_free(&self) -> bool {
        self.sized_free.is_some()
    }

    fn has_reallocarray(&self) -> bool {
        self.reallocarray.is_some()
    }

    fn has_obsolete(&self) -> bool {
        self.obsolete.is_some()
    }

    unsafe fn reallocarray(&self, p: *mut c_void, n: usize, size: usize) -> Option<*mut c_void> {
        // SAFETY: same contract.
        self.reallocarray.map(|f| unsafe { f(p, n, size) })
    }

    unsafe fn memalign(&self, align: usize, size: usize) -> Option<*mut c_void> {
        // SAFETY: same contract.
        self.obsolete.map(|f| unsafe { (f.memalign)(align, size) })
    }

    unsafe fn valloc(&self, size: usize) -> Option<*mut c_void> {
        // SAFETY: same contract.
        self.obsolete.map(|f| unsafe { (f.valloc)(size) })
    }

    unsafe fn pvalloc(&self, size: usize) -> Option<*mut c_void> {
        // SAFETY: same contract.
        self.obsolete.map(|f| unsafe { (f.pvalloc)(size) })
    }

    unsafe fn free_sized(&self, p: *mut c_void, size: usize) {
        let f = self.sized_free.expect("caller checked has_sized_free");
        // SAFETY: same contract.
        unsafe { (f.free_sized)(p, size) }
    }

    unsafe fn free_aligned_sized(&self, p: *mut c_void, align: usize, size: usize) {
        let f = self.sized_free.expect("caller checked has_sized_free");
        // SAFETY: same contract.
        unsafe { (f.free_aligned_sized)(p, align, size) }
    }

    unsafe fn errno(&self) -> c_int {
        // SAFETY: the accessor returns this thread's errno, valid for the thread's lifetime.
        unsafe { *(self.errno_location)() }
    }

    unsafe fn set_errno(&self, value: c_int) {
        // SAFETY: as for `errno`.
        unsafe { *(self.errno_location)() = value }
    }
}

const MALLOC: u32 = 0;
const CALLOC: u32 = 1;
const POSIX_MEMALIGN: u32 = 2;
const ALIGNED_ALLOC: u32 = 3;
const REALLOC: u32 = 4;

/// Drives the C functions. Requests C's natural alignment covers go to `malloc` (or `calloc`
/// when zeroed, split into a variety of `nmemb * size` products); others, and one in eight of
/// the rest, to `posix_memalign` or `aligned_alloc`. Frees use `free` or, half the time, the
/// sized free that matches the block's origin. Resizes use `realloc`, whose result has only
/// C's natural alignment.
///
/// The usable size is `malloc_usable_size`, and the model writes and checks all of it, so an
/// implementation that over-reports usable bytes is caught as an overlap. The exception is a
/// zero-size request, whose result C forbids accessing whatever `malloc_usable_size` says.
#[derive(Clone, Copy, Debug)]
pub struct ViaC<C>(pub C);

impl<C: CAbi> ViaC<C> {
    /// # Safety
    /// `p` is a live block of this implementation, allocated for `size` bytes.
    unsafe fn usable(&self, p: NonNull<u8>, size: usize) -> usize {
        if size == 0 {
            return 0;
        }
        // SAFETY: caller contract.
        unsafe { self.0.malloc_usable_size(p.as_ptr().cast()) }.unwrap_or(size)
    }
}

/// `nmemb` and `size` arguments for a zeroed request of `total` bytes.
fn calloc_split(total: usize, variant: u32) -> (usize, usize) {
    if total == 0 {
        let other = 1 + (variant as usize / 2) % 64;
        return if variant.is_multiple_of(2) {
            (0, other)
        } else {
            (other, 0)
        };
    }
    match variant % 3 {
        0 => (1, total),
        1 => (total, 1),
        _ => {
            let f = [16, 8, 4, 3, 2]
                .into_iter()
                .find(|f| total.is_multiple_of(*f))
                .unwrap_or(1);
            (total / f, f)
        }
    }
}

impl<C: CAbi> Subject for ViaC<C> {
    fn name(&self) -> String {
        format!("C ABI of {}", self.0.name())
    }

    fn zero_size(&self) -> ZeroSize {
        // `realloc(p, 0)` is implementation-defined in C17 and undefined in C23; the
        // conformance cases cover it instead.
        ZeroSize {
            alloc: true,
            resize: false,
            distinct: true,
        }
    }

    unsafe fn alloc(&self, req: Request) -> Result<Option<Grant>, Refused> {
        let natural = c_default_align(req.size);
        let pick = req.variant % 8;
        // SAFETY: plain calls of the C functions with valid arguments: `posix_memalign` gets a
        // power of two at least `sizeof(void *)`, `aligned_alloc` a power of two.
        let (raw, align, zeroed, tag) = unsafe {
            if req.align <= natural && pick != 7 {
                if req.zeroed {
                    let (n, m) = calloc_split(req.size, req.variant / 8);
                    (self.0.calloc(n, m), natural, req.size, CALLOC)
                } else {
                    (self.0.malloc(req.size), natural, 0, MALLOC)
                }
            } else if pick.is_multiple_of(2) {
                let align = req.align.max(size_of::<*mut c_void>());
                let mut out = ptr::null_mut();
                let rc = self.0.posix_memalign(&mut out, align, req.size);
                let p = if rc == 0 { out } else { ptr::null_mut() };
                (p, align, 0, POSIX_MEMALIGN)
            } else {
                let p = self.0.aligned_alloc(req.align, req.size);
                (p, req.align, 0, ALIGNED_ALLOC)
            }
        };
        let Some(ptr) = NonNull::new(raw.cast::<u8>()) else {
            return if req.size == 0 {
                Ok(None)
            } else {
                Err(Refused)
            };
        };
        Ok(Some(Grant {
            ptr,
            // SAFETY: `ptr` was just returned by this implementation.
            usable: unsafe { self.usable(ptr, req.size) },
            align,
            zeroed,
            tag,
        }))
    }

    unsafe fn free(&self, block: Held, variant: u32) {
        let p = block.ptr.as_ptr().cast::<c_void>();
        // SAFETY: the block is live (caller contract), and each sized free gets the arguments
        // C23 requires for a block of that origin.
        unsafe {
            if variant % 2 == 1 && self.0.has_sized_free() {
                match block.tag {
                    MALLOC | CALLOC | REALLOC => self.0.free_sized(p, block.size),
                    ALIGNED_ALLOC => self.0.free_aligned_sized(p, block.align, block.size),
                    _ => self.0.free(p),
                }
            } else {
                self.0.free(p)
            }
        }
    }

    unsafe fn resize(&self, block: Held, req: Request) -> Result<Resized, Refused> {
        // SAFETY: the block is live (caller contract) and the new size is non-zero.
        let raw = unsafe { self.0.realloc(block.ptr.as_ptr().cast(), req.size) };
        let ptr = NonNull::new(raw.cast::<u8>()).ok_or(Refused)?;
        Ok(Resized {
            grant: Grant {
                ptr,
                // SAFETY: `ptr` was just returned by this implementation.
                usable: unsafe { self.usable(ptr, req.size) },
                align: c_default_align(req.size),
                zeroed: 0,
                tag: REALLOC,
            },
            preserved: block.size.min(req.size),
            zeroed_from: None,
        })
    }

    unsafe fn usable_size(&self, block: Held) -> Option<usize> {
        if block.size == 0 {
            return None;
        }
        // SAFETY: the block is live (caller contract).
        unsafe { self.0.malloc_usable_size(block.ptr.as_ptr().cast()) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_alignment_follows_c23() {
        let cases = [
            (0, 1),
            (1, 1),
            (2, 2),
            (3, 2),
            (7, 4),
            (8, 8),
            (12, 8),
            (15, 8),
        ];
        for (size, align) in cases {
            assert_eq!(c_default_align(size), align, "size {size}");
        }
        for size in [16, 17, 24, 100, 1 << 20, usize::MAX] {
            assert_eq!(c_default_align(size), 16, "size {size}");
        }
    }

    #[test]
    fn calloc_splits_multiply_back() {
        for total in [0, 1, 2, 3, 12, 48, 100, 4096, 4097] {
            for v in 0..12 {
                let (n, m) = calloc_split(total, v);
                assert_eq!(n * m, total, "{total} split by {v}");
            }
        }
    }
}
