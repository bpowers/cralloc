//! Edge cases of the C allocation functions, as data.
//!
//! Each [`Case`] is one call and what the C standard (C23, and POSIX for `posix_memalign` and
//! errno) requires of its result. [`run_case`] executes a case against any [`CAbi`], checks
//! the result, and frees what it made. Where the standard leaves a choice to the
//! implementation (`malloc(0)`, `realloc(p, 0)`), the case accepts every allowed answer.

use core::ffi::{c_int, c_void};
use core::ptr;

use crate::pattern::{self, Contents};
use crate::subject::{CAbi, c_default_align};

/// `ENOMEM` on Linux.
pub const ENOMEM: c_int = 12;
/// `EINVAL` on Linux.
pub const EINVAL: c_int = 22;

/// errno is set to this before each call, so a call that sets nothing is noticed.
const ERRNO_SENTINEL: c_int = 0x5eed;

const SIZE_MAX: usize = usize::MAX;
const PTRDIFF_MAX: usize = isize::MAX as usize;

/// The call a case makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Call {
    /// `malloc(size)`.
    Malloc(usize),
    /// `calloc(n, size)`.
    Calloc(usize, usize),
    /// Fill and free a few blocks of `n * size` bytes, then `calloc(n, size)`, so the zeroed
    /// block is likely recycled memory rather than fresh pages.
    CallocAfterDirty(usize, usize),
    /// `realloc(NULL, size)`.
    ReallocNull(usize),
    /// `realloc` of a filled `malloc(from)` block to `to` bytes.
    Realloc {
        /// Size of the original block.
        from: usize,
        /// Size asked of `realloc`.
        to: usize,
    },
    /// `posix_memalign(&p, align, size)`.
    PosixMemalign {
        /// Alignment argument.
        align: usize,
        /// Size argument.
        size: usize,
    },
    /// `aligned_alloc(align, size)`.
    AlignedAlloc {
        /// Alignment argument.
        align: usize,
        /// Size argument.
        size: usize,
    },
    /// `malloc_usable_size(NULL)`.
    UsableSizeOfNull,
    /// `free(NULL)`.
    FreeNull,
    /// `free_sized(NULL, 0)`.
    FreeSizedNull,
    /// `free_aligned_sized(NULL, 64, 0)`.
    FreeAlignedSizedNull,
    /// `free_sized(malloc(size), size)`.
    FreeSized(usize),
    /// `free_sized(calloc(n, size), n * size)`.
    FreeSizedCalloc(usize, usize),
    /// `free_aligned_sized(aligned_alloc(align, size), align, size)`.
    FreeAlignedSized {
        /// Alignment argument.
        align: usize,
        /// Size argument.
        size: usize,
    },
}

/// What the call must do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Expect {
    /// A block: non-null, aligned to `align` (zero means C's default for the size), with a
    /// usable size of at least the request, writable, zero-filled for `calloc`, carrying the
    /// old contents for `realloc`, and accepted by `free`.
    Block {
        /// Required alignment, or zero for C's default.
        align: usize,
    },
    /// A null pointer, or a unique pointer that `free` accepts: two such calls never return
    /// the same pointer while both are live. The answer for zero-size requests.
    NullOrUnique,
    /// A null pointer, with errno set; for `realloc`, the original block untouched and live.
    Null {
        /// The errno value required.
        errno: c_int,
    },
    /// `posix_memalign` returns this error number and leaves the pointer argument unmodified
    /// or null.
    Error(c_int),
    /// The call returns this value.
    Value(usize),
    /// The call returns, and the allocator still serves requests afterwards.
    Returns,
}

/// One edge case.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Case {
    /// What is being checked.
    pub name: &'static str,
    /// The call.
    pub call: Call,
    /// The required result.
    pub expect: Expect,
}

/// The outcome of a case.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// The implementation did what the case requires.
    Pass,
    /// The implementation lacks a function the case needs.
    Skip(&'static str),
    /// It did not, and this is how.
    Fail(String),
}

const fn case(name: &'static str, call: Call, expect: Expect) -> Case {
    Case { name, call, expect }
}

const BLOCK: Expect = Expect::Block { align: 0 };
const NO_MEMORY: Expect = Expect::Null { errno: ENOMEM };
const BAD_ALIGN: Expect = Expect::Null { errno: EINVAL };

/// Every case.
pub const CASES: &[Case] = &[
    case(
        "malloc(0) is null or unique",
        Call::Malloc(0),
        Expect::NullOrUnique,
    ),
    case("malloc(1)", Call::Malloc(1), BLOCK),
    case("malloc(7) is 4-aligned at least", Call::Malloc(7), BLOCK),
    case("malloc(8) is 8-aligned at least", Call::Malloc(8), BLOCK),
    case("malloc(15)", Call::Malloc(15), BLOCK),
    case("malloc(16) is 16-aligned", Call::Malloc(16), BLOCK),
    case("malloc(24) is 16-aligned", Call::Malloc(24), BLOCK),
    case("malloc(100)", Call::Malloc(100), BLOCK),
    case("malloc(4096)", Call::Malloc(4096), BLOCK),
    case("malloc(64 KiB)", Call::Malloc(1 << 16), BLOCK),
    case("malloc(1 MiB)", Call::Malloc(1 << 20), BLOCK),
    case("malloc(16 MiB)", Call::Malloc(1 << 24), BLOCK),
    case(
        "malloc(SIZE_MAX) fails with ENOMEM",
        Call::Malloc(SIZE_MAX),
        NO_MEMORY,
    ),
    case(
        "malloc(PTRDIFF_MAX + 1) fails with ENOMEM",
        Call::Malloc(PTRDIFF_MAX + 1),
        NO_MEMORY,
    ),
    case(
        "malloc(2^62) fails with ENOMEM",
        Call::Malloc(1 << 62),
        NO_MEMORY,
    ),
    case(
        "calloc(0, 0) is null or unique",
        Call::Calloc(0, 0),
        Expect::NullOrUnique,
    ),
    case(
        "calloc(0, 16) is null or unique",
        Call::Calloc(0, 16),
        Expect::NullOrUnique,
    ),
    case(
        "calloc(16, 0) is null or unique",
        Call::Calloc(16, 0),
        Expect::NullOrUnique,
    ),
    case("calloc(1, 1) is zeroed", Call::Calloc(1, 1), BLOCK),
    case("calloc(3, 5) is zeroed", Call::Calloc(3, 5), BLOCK),
    case("calloc(16, 16) is zeroed", Call::Calloc(16, 16), BLOCK),
    case(
        "calloc(1, 1 MiB) is zeroed",
        Call::Calloc(1, 1 << 20),
        BLOCK,
    ),
    case(
        "calloc(1 MiB, 1) is zeroed",
        Call::Calloc(1 << 20, 1),
        BLOCK,
    ),
    case(
        "calloc(SIZE_MAX, 2) overflows to ENOMEM",
        Call::Calloc(SIZE_MAX, 2),
        NO_MEMORY,
    ),
    case(
        "calloc(2, SIZE_MAX) overflows to ENOMEM",
        Call::Calloc(2, SIZE_MAX),
        NO_MEMORY,
    ),
    case(
        "calloc(2^32, 2^32) overflows to ENOMEM",
        Call::Calloc(1 << 32, 1 << 32),
        NO_MEMORY,
    ),
    case(
        "calloc(SIZE_MAX / 2 + 1, 2) overflows to ENOMEM",
        Call::Calloc(SIZE_MAX / 2 + 1, 2),
        NO_MEMORY,
    ),
    case(
        "calloc(PTRDIFF_MAX, 2) fails with ENOMEM",
        Call::Calloc(PTRDIFF_MAX, 2),
        NO_MEMORY,
    ),
    case(
        "calloc of recycled 64 bytes is zeroed",
        Call::CallocAfterDirty(1, 64),
        BLOCK,
    ),
    case(
        "calloc of recycled 4 KiB is zeroed",
        Call::CallocAfterDirty(8, 512),
        BLOCK,
    ),
    case(
        "calloc of recycled 100 KB is zeroed",
        Call::CallocAfterDirty(1, 100_000),
        BLOCK,
    ),
    case(
        "calloc of recycled 64 KiB is zeroed",
        Call::CallocAfterDirty(64, 1024),
        BLOCK,
    ),
    case(
        "realloc(NULL, 0) is null or unique",
        Call::ReallocNull(0),
        Expect::NullOrUnique,
    ),
    case(
        "realloc(NULL, 24) is malloc(24)",
        Call::ReallocNull(24),
        BLOCK,
    ),
    case(
        "realloc 8 to 4096 keeps 8",
        Call::Realloc { from: 8, to: 4096 },
        BLOCK,
    ),
    case(
        "realloc 4096 to 8 keeps 8",
        Call::Realloc { from: 4096, to: 8 },
        BLOCK,
    ),
    case(
        "realloc 24 to 25 keeps 24",
        Call::Realloc { from: 24, to: 25 },
        BLOCK,
    ),
    case(
        "realloc 100 to 1 MiB keeps 100",
        Call::Realloc {
            from: 100,
            to: 1 << 20,
        },
        BLOCK,
    ),
    case(
        "realloc 1 MiB to 100 keeps 100",
        Call::Realloc {
            from: 1 << 20,
            to: 100,
        },
        BLOCK,
    ),
    case(
        "realloc 1 MiB to 4 MiB keeps 1 MiB",
        Call::Realloc {
            from: 1 << 20,
            to: 1 << 22,
        },
        BLOCK,
    ),
    case(
        "realloc(p, 0) frees or returns a unique pointer",
        Call::Realloc { from: 64, to: 0 },
        Expect::NullOrUnique,
    ),
    case(
        "realloc(p, SIZE_MAX) fails with ENOMEM and keeps p",
        Call::Realloc {
            from: 64,
            to: SIZE_MAX,
        },
        NO_MEMORY,
    ),
    case(
        "realloc(p, PTRDIFF_MAX + 1) fails with ENOMEM and keeps p",
        Call::Realloc {
            from: 64,
            to: PTRDIFF_MAX + 1,
        },
        NO_MEMORY,
    ),
    case(
        "posix_memalign alignment 0 is EINVAL",
        pm(0, 16),
        Expect::Error(EINVAL),
    ),
    case(
        "posix_memalign alignment 1 is EINVAL",
        pm(1, 16),
        Expect::Error(EINVAL),
    ),
    case(
        "posix_memalign alignment 2 is EINVAL",
        pm(2, 16),
        Expect::Error(EINVAL),
    ),
    case(
        "posix_memalign alignment 4 is EINVAL",
        pm(4, 16),
        Expect::Error(EINVAL),
    ),
    case(
        "posix_memalign alignment 3 is EINVAL",
        pm(3, 16),
        Expect::Error(EINVAL),
    ),
    case(
        "posix_memalign alignment 24 is EINVAL",
        pm(24, 16),
        Expect::Error(EINVAL),
    ),
    case(
        "posix_memalign alignment 48 is EINVAL",
        pm(48, 16),
        Expect::Error(EINVAL),
    ),
    case("posix_memalign(8, 1)", pm(8, 1), Expect::Block { align: 8 }),
    case(
        "posix_memalign(16, 1)",
        pm(16, 1),
        Expect::Block { align: 16 },
    ),
    case(
        "posix_memalign(32, 100)",
        pm(32, 100),
        Expect::Block { align: 32 },
    ),
    case(
        "posix_memalign(64, 64)",
        pm(64, 64),
        Expect::Block { align: 64 },
    ),
    case(
        "posix_memalign(4096, 1)",
        pm(4096, 1),
        Expect::Block { align: 4096 },
    ),
    case(
        "posix_memalign(4096, 4097)",
        pm(4096, 4097),
        Expect::Block { align: 4096 },
    ),
    case(
        "posix_memalign(64 KiB, 10)",
        pm(1 << 16, 10),
        Expect::Block { align: 1 << 16 },
    ),
    case(
        "posix_memalign(2 MiB, 1)",
        pm(1 << 21, 1),
        Expect::Block { align: 1 << 21 },
    ),
    case(
        "posix_memalign(64, 0) is null or unique",
        pm(64, 0),
        Expect::NullOrUnique,
    ),
    case(
        "posix_memalign(64, SIZE_MAX) is ENOMEM",
        pm(64, SIZE_MAX),
        Expect::Error(ENOMEM),
    ),
    case("aligned_alloc alignment 0 is EINVAL", aa(0, 16), BAD_ALIGN),
    case("aligned_alloc alignment 3 is EINVAL", aa(3, 16), BAD_ALIGN),
    case(
        "aligned_alloc alignment 24 is EINVAL",
        aa(24, 48),
        BAD_ALIGN,
    ),
    case(
        "aligned_alloc(1, 10)",
        aa(1, 10),
        Expect::Block { align: 1 },
    ),
    case(
        "aligned_alloc(2, 10)",
        aa(2, 10),
        Expect::Block { align: 2 },
    ),
    case(
        "aligned_alloc(4, 10)",
        aa(4, 10),
        Expect::Block { align: 4 },
    ),
    case("aligned_alloc(8, 8)", aa(8, 8), Expect::Block { align: 8 }),
    case(
        "aligned_alloc(16, 100)",
        aa(16, 100),
        Expect::Block { align: 16 },
    ),
    case(
        "aligned_alloc(64, 10)",
        aa(64, 10),
        Expect::Block { align: 64 },
    ),
    case(
        "aligned_alloc(4096, 4096)",
        aa(4096, 4096),
        Expect::Block { align: 4096 },
    ),
    case(
        "aligned_alloc(4096, 100), size not a multiple",
        aa(4096, 100),
        Expect::Block { align: 4096 },
    ),
    case(
        "aligned_alloc(2 MiB, 1)",
        aa(1 << 21, 1),
        Expect::Block { align: 1 << 21 },
    ),
    case(
        "aligned_alloc(64, SIZE_MAX) is ENOMEM",
        aa(64, SIZE_MAX),
        NO_MEMORY,
    ),
    case(
        "aligned_alloc(64, 0) is null or unique",
        aa(64, 0),
        Expect::NullOrUnique,
    ),
    case(
        "malloc_usable_size(NULL) is 0",
        Call::UsableSizeOfNull,
        Expect::Value(0),
    ),
    case("free(NULL) does nothing", Call::FreeNull, Expect::Returns),
    case(
        "free_sized(NULL, 0) does nothing",
        Call::FreeSizedNull,
        Expect::Returns,
    ),
    case(
        "free_aligned_sized(NULL, 64, 0) does nothing",
        Call::FreeAlignedSizedNull,
        Expect::Returns,
    ),
    case(
        "free_sized of malloc(0)",
        Call::FreeSized(0),
        Expect::Returns,
    ),
    case(
        "free_sized of malloc(1)",
        Call::FreeSized(1),
        Expect::Returns,
    ),
    case(
        "free_sized of malloc(24)",
        Call::FreeSized(24),
        Expect::Returns,
    ),
    case(
        "free_sized of malloc(4096)",
        Call::FreeSized(4096),
        Expect::Returns,
    ),
    case(
        "free_sized of malloc(1 MiB)",
        Call::FreeSized(1 << 20),
        Expect::Returns,
    ),
    case(
        "free_sized of calloc(3, 7)",
        Call::FreeSizedCalloc(3, 7),
        Expect::Returns,
    ),
    case(
        "free_aligned_sized of aligned_alloc(64, 100)",
        Call::FreeAlignedSized {
            align: 64,
            size: 100,
        },
        Expect::Returns,
    ),
    case(
        "free_aligned_sized of aligned_alloc(4096, 8192)",
        Call::FreeAlignedSized {
            align: 4096,
            size: 8192,
        },
        Expect::Returns,
    ),
];

const fn pm(align: usize, size: usize) -> Call {
    Call::PosixMemalign { align, size }
}

const fn aa(align: usize, size: usize) -> Call {
    Call::AlignedAlloc { align, size }
}

/// Run every case; returns each case with its verdict.
pub fn run_all<C: CAbi + ?Sized>(c: &C) -> Vec<(Case, Verdict)> {
    CASES
        .iter()
        .map(|case| (*case, run_case(c, case)))
        .collect()
}

/// Run one case.
pub fn run_case<C: CAbi + ?Sized>(c: &C, case: &Case) -> Verdict {
    // SAFETY: the runner calls the C functions only with arguments their contracts allow, and
    // frees exactly what it made.
    match unsafe { Runner { c }.run(case) } {
        Ok(v) => v,
        Err(msg) => Verdict::Fail(msg),
    }
}

/// What an allocating call produced.
struct Made {
    ptr: *mut c_void,
    /// The object's size, as the call described it.
    size: usize,
    /// `calloc` promised zeros.
    zeroed: bool,
    /// Alignment the call must give; zero for C's default.
    align: usize,
    /// For `realloc`: the pattern seed of the old block, its size, and the bytes to carry over.
    carried: Option<(u64, usize, usize)>,
    /// For a failed `realloc`: the original block, still owned, with its seed and size.
    original: Option<(*mut c_void, u64, usize)>,
    /// For `posix_memalign`: the return value and whether the pointer argument was left alone
    /// or nulled.
    rc: Option<(c_int, bool)>,
    /// errno after the call.
    errno: c_int,
}

impl Made {
    fn new(ptr: *mut c_void, size: usize, errno: c_int) -> Made {
        Made {
            ptr,
            size,
            zeroed: false,
            align: 0,
            carried: None,
            original: None,
            rc: None,
            errno,
        }
    }
}

struct Runner<'a, C: ?Sized> {
    c: &'a C,
}

/// A sentinel the pointer argument of `posix_memalign` starts as.
fn sentinel() -> *mut c_void {
    ptr::without_provenance_mut(0xA5A5_0000)
}

const SEED: u64 = 0x0123_4567_89AB_CDEF;

impl<C: CAbi + ?Sized> Runner<'_, C> {
    /// # Safety
    /// `p` is a live block of at least `size` bytes.
    unsafe fn fill(&self, p: *mut c_void, size: usize, seed: u64) {
        // SAFETY: caller contract.
        unsafe { pattern::fill(p.cast(), size, seed) }
    }

    /// # Safety
    /// `p` is a live block whose first `size` bytes are initialized.
    unsafe fn holds(&self, p: *mut c_void, len: usize, upto: usize, contents: Contents) -> bool {
        // SAFETY: caller contract.
        unsafe { pattern::check(p.cast(), len, 0, upto, contents) }.is_ok()
    }

    /// Run the allocating `call`, setting errno to the sentinel just before the call checked.
    ///
    /// # Safety
    /// Calls the C functions; the caller frees what is returned.
    unsafe fn make(&self, call: Call) -> Result<Made, String> {
        let c = self.c;
        // SAFETY: every call below gets arguments its contract allows; blocks made along the
        // way are filled within their size and freed.
        unsafe {
            Ok(match call {
                Call::Malloc(n) => {
                    c.set_errno(ERRNO_SENTINEL);
                    let p = c.malloc(n);
                    Made::new(p, n, c.errno())
                }
                Call::Calloc(n, m) => {
                    c.set_errno(ERRNO_SENTINEL);
                    let p = c.calloc(n, m);
                    Made {
                        zeroed: true,
                        ..Made::new(p, n.wrapping_mul(m), c.errno())
                    }
                }
                Call::CallocAfterDirty(n, m) => {
                    let total = n * m;
                    let dirty: Vec<_> = (0..4).map(|_| c.malloc(total)).collect();
                    for &p in &dirty {
                        if p.is_null() {
                            return Err(format!("malloc({total}) failed"));
                        }
                        p.cast::<u8>().write_bytes(0xA5, total);
                    }
                    for p in dirty {
                        c.free(p);
                    }
                    c.set_errno(ERRNO_SENTINEL);
                    let p = c.calloc(n, m);
                    Made {
                        zeroed: true,
                        ..Made::new(p, total, c.errno())
                    }
                }
                Call::ReallocNull(n) => {
                    c.set_errno(ERRNO_SENTINEL);
                    let p = c.realloc(ptr::null_mut(), n);
                    Made::new(p, n, c.errno())
                }
                Call::Realloc { from, to } => {
                    let old = c.malloc(from);
                    if old.is_null() {
                        return Err(format!("malloc({from}) failed"));
                    }
                    self.fill(old, from, SEED);
                    c.set_errno(ERRNO_SENTINEL);
                    let p = c.realloc(old, to);
                    let errno = c.errno();
                    if p.is_null() && to > 0 {
                        Made {
                            original: Some((old, SEED, from)),
                            ..Made::new(p, to, errno)
                        }
                    } else {
                        Made {
                            carried: Some((SEED, from, from.min(to))),
                            ..Made::new(p, to, errno)
                        }
                    }
                }
                Call::PosixMemalign { align, size } => {
                    let mut out = sentinel();
                    c.set_errno(ERRNO_SENTINEL);
                    let rc = c.posix_memalign(&mut out, align, size);
                    let errno = c.errno();
                    if rc == 0 && out == sentinel() {
                        return Err("returned 0 without setting the pointer".into());
                    }
                    if rc == 0 {
                        Made {
                            align,
                            rc: Some((0, true)),
                            ..Made::new(out, size, errno)
                        }
                    } else {
                        let untouched = out == sentinel() || out.is_null();
                        Made {
                            align,
                            rc: Some((rc, untouched)),
                            ..Made::new(ptr::null_mut(), size, errno)
                        }
                    }
                }
                Call::AlignedAlloc { align, size } => {
                    c.set_errno(ERRNO_SENTINEL);
                    let p = c.aligned_alloc(align, size);
                    Made {
                        align,
                        ..Made::new(p, size, c.errno())
                    }
                }
                other => return Err(format!("{other:?} does not allocate")),
            })
        }
    }

    /// Check a block that must be valid, then free it.
    ///
    /// # Safety
    /// `m` came from [`make`](Self::make).
    unsafe fn check_block(&self, m: &Made, align: usize) -> Result<(), String> {
        let c = self.c;
        if let Some((rc, _)) = m.rc
            && rc != 0
        {
            return Err(format!("returned error {rc}"));
        }
        if m.ptr.is_null() {
            return Err(format!("returned null (errno {})", m.errno));
        }
        let required = if align == 0 {
            c_default_align(m.size)
        } else {
            align
        };
        let addr = m.ptr.addr();
        // SAFETY (block): `m.ptr` is a live block of `m.size` bytes; the zero and carried bytes
        // are initialized by the call's contract, and the rest only after `fill`.
        unsafe {
            let result = (|| {
                if !addr.is_multiple_of(required) {
                    return Err(format!("{addr:#x} is not {required}-aligned"));
                }
                if let Some(usable) = c.malloc_usable_size(m.ptr)
                    && usable < m.size
                {
                    return Err(format!("usable size {usable} below {}", m.size));
                }
                if m.zeroed && !self.holds(m.ptr, m.size, m.size, Contents::Zero) {
                    return Err("not zero-filled".into());
                }
                if let Some((seed, len, keep)) = m.carried
                    && !self.holds(m.ptr, len, keep, Contents::Pattern(seed))
                {
                    return Err(format!("the first {keep} bytes were not carried over"));
                }
                self.fill(m.ptr, m.size, SEED ^ 1);
                if !self.holds(m.ptr, m.size, m.size, Contents::Pattern(SEED ^ 1)) {
                    return Err("does not hold what was written".into());
                }
                Ok(())
            })();
            c.free(m.ptr);
            result
        }
    }

    /// # Safety
    /// Calls the C functions.
    unsafe fn run(&self, case: &Case) -> Result<Verdict, String> {
        let c = self.c;
        // SAFETY (block): each arm makes calls with arguments their contracts allow and frees
        // what it made.
        unsafe {
            match case.expect {
                Expect::Block { align } => {
                    let m = self.make(case.call)?;
                    self.check_block(&m, align)?;
                }
                Expect::NullOrUnique => {
                    let a = self.make(case.call)?;
                    let b = self.make(case.call)?;
                    let mut problem = None;
                    for m in [&a, &b] {
                        if let Some((rc, _)) = m.rc
                            && rc != 0
                        {
                            problem = Some(format!("returned error {rc}"));
                        }
                        let required = m.align.max(1);
                        if !m.ptr.is_null() && m.ptr.addr() % required != 0 {
                            problem = Some(format!("{:p} is not {required}-aligned", m.ptr));
                        }
                    }
                    if !a.ptr.is_null() && a.ptr == b.ptr {
                        problem = Some(format!("returned {:p} twice", a.ptr));
                    }
                    c.free(a.ptr);
                    if b.ptr != a.ptr {
                        c.free(b.ptr);
                    }
                    if let Some(p) = problem {
                        return Err(p);
                    }
                }
                Expect::Null { errno } => {
                    let m = self.make(case.call)?;
                    let mut problem = None;
                    if !m.ptr.is_null() {
                        problem = Some(format!("returned {:p}, not null", m.ptr));
                        c.free(m.ptr);
                    } else if m.errno != errno {
                        problem = Some(format!("errno is {}, not {errno}", m.errno));
                    }
                    if let Some((old, seed, size)) = m.original {
                        if !self.holds(old, size, size, Contents::Pattern(seed)) {
                            problem.get_or_insert("the original block changed".into());
                        }
                        c.free(old);
                    }
                    if let Some(p) = problem {
                        return Err(p);
                    }
                }
                Expect::Error(code) => {
                    let m = self.make(case.call)?;
                    match m.rc {
                        Some((0, _)) => {
                            c.free(m.ptr);
                            return Err(format!("succeeded instead of returning {code}"));
                        }
                        Some((rc, untouched)) => {
                            if rc != code {
                                return Err(format!("returned {rc}, not {code}"));
                            }
                            if !untouched {
                                return Err("changed the pointer argument on failure".into());
                            }
                        }
                        None => return Err("only posix_memalign returns an error number".into()),
                    }
                }
                Expect::Value(v) => match case.call {
                    Call::UsableSizeOfNull => match c.malloc_usable_size(ptr::null_mut()) {
                        None => return Ok(Verdict::Skip("no malloc_usable_size")),
                        Some(got) if got != v => return Err(format!("returned {got}")),
                        Some(_) => {}
                    },
                    other => return Err(format!("{other:?} returns no value")),
                },
                Expect::Returns => {
                    if !matches!(case.call, Call::FreeNull) && !c.has_sized_free() {
                        return Ok(Verdict::Skip("no sized frees"));
                    }
                    match case.call {
                        Call::FreeNull => c.free(ptr::null_mut()),
                        Call::FreeSizedNull => c.free_sized(ptr::null_mut(), 0),
                        Call::FreeAlignedSizedNull => c.free_aligned_sized(ptr::null_mut(), 64, 0),
                        Call::FreeSized(n) => {
                            let p = c.malloc(n);
                            if p.is_null() && n > 0 {
                                return Err(format!("malloc({n}) failed"));
                            }
                            if !p.is_null() {
                                self.fill(p, n, SEED);
                            }
                            c.free_sized(p, n);
                        }
                        Call::FreeSizedCalloc(n, m) => {
                            let p = c.calloc(n, m);
                            if p.is_null() {
                                return Err(format!("calloc({n}, {m}) failed"));
                            }
                            c.free_sized(p, n * m);
                        }
                        Call::FreeAlignedSized { align, size } => {
                            let p = c.aligned_alloc(align, size);
                            if p.is_null() {
                                return Err(format!("aligned_alloc({align}, {size}) failed"));
                            }
                            self.fill(p, size, SEED);
                            c.free_aligned_sized(p, align, size);
                        }
                        other => return Err(format!("{other:?} is not a free")),
                    }
                    let p = c.malloc(32);
                    if p.is_null() {
                        return Err("malloc(32) failed afterwards".into());
                    }
                    c.free(p);
                }
            }
        }
        Ok(Verdict::Pass)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_names_are_unique() {
        let mut names: Vec<_> = CASES.iter().map(|c| c.name).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(names.len(), before);
    }
}
