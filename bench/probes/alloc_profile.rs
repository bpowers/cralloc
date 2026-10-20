//! An `LD_PRELOAD` library that records a program's allocation profile: how many calls of
//! each kind, the sizes asked for, how long blocks live, how many are freed (or reallocated)
//! by a thread other than the one that allocated them, how reallocs grow, and the peak of
//! the bytes and blocks the program holds. It is a measuring tool, not an allocator: every
//! call is served by glibc's own functions (`__libc_malloc` and friends), so its runs say
//! nothing about speed and are kept apart from timed ones.
//!
//! Each block gets a 16-byte header in front of it holding its requested size, alignment,
//! the index of the thread that allocated it and the time stamp counter at its birth. The
//! header changes glibc's size classes, which does not matter here: everything recorded is
//! what the program asked for, not what an allocator made of it.
//!
//! Build: rustc --edition 2024 -O --crate-type cdylib alloc_profile.rs -o DIR/liballoc_profile.so
//! Use:   ALLOC_PROFILE_OUT=DIR/prof LD_PRELOAD=DIR/liballoc_profile.so program args...
//!        Each process writes DIR/prof.<pid>.json when it exits; a child started with the
//!        same environment writes its own. With ALLOC_PROFILE_SIGTERM=1 a SIGTERM also writes
//!        the report and exits, for servers that are stopped that way. `rust-screen/profile.py`
//!        summarises the files.
//!
//! What it does not see: memory a program maps itself, and allocators linked into the
//! program (a Rust `#[global_allocator]` other than `System`). Live bytes are summed per
//! thread and published every 64 KiB or 64 blocks, so the peak can be off by that much per
//! thread.
//!
//! Nothing here may allocate: the functions below are the allocator.
use core::cell::Cell;
use core::ffi::{c_char, c_int, c_void};
use core::sync::atomic::{AtomicI64, AtomicU32, AtomicU64, Ordering::Relaxed};

#[repr(C)]
struct Timespec {
    sec: i64,
    nsec: i64,
}

unsafe extern "C" {
    fn __libc_malloc(size: usize) -> *mut c_void;
    fn __libc_calloc(n: usize, size: usize) -> *mut c_void;
    fn __libc_realloc(p: *mut c_void, size: usize) -> *mut c_void;
    fn __libc_free(p: *mut c_void);
    fn __libc_memalign(align: usize, size: usize) -> *mut c_void;
    fn getenv(name: *const c_char) -> *const c_char;
    fn getpid() -> c_int;
    fn open(path: *const c_char, flags: c_int, ...) -> c_int;
    fn write(fd: c_int, buf: *const c_void, n: usize) -> isize;
    fn close(fd: c_int) -> c_int;
    fn readlink(path: *const c_char, buf: *mut c_char, n: usize) -> isize;
    fn clock_gettime(clock: c_int, ts: *mut Timespec) -> c_int;
    fn signal(sig: c_int, handler: usize) -> usize;
    fn _exit(code: c_int) -> !;
    fn __errno_location() -> *mut c_int;
}

const HDR: usize = 16;
const MAGIC: u64 = 0xa5;
const SIZE_MASK: u64 = (1 << 48) - 1;
// Birth stamps are the TSC shifted right by TICK_SHIFT, kept to 44 bits: at 5 GHz that wraps
// after about two days, and lifetimes are taken modulo 2^44.
const TICK_SHIFT: u32 = 6;
const BIRTH_BITS: u32 = 44;
const BIRTH_MASK: u64 = (1 << BIRTH_BITS) - 1;
const TID_MASK: u64 = (1 << (64 - BIRTH_BITS)) - 1;

const SIZE_BUCKETS: usize = 49;
const LIFE_BUCKETS: usize = 64;
// New size over old size of a realloc: shrink or same, then up to 1.25, 1.5, 2, 4, 16, more.
const RATIO_BUCKETS: usize = 7;
const SLOTS: usize = 1024;
const FLUSH_BYTES: i64 = 64 << 10;
const FLUSH_COUNT: i64 = 64;

struct Slot {
    allocs: AtomicU64,
    callocs: AtomicU64,
    aligned: AtomicU64,
    frees: AtomicU64,
    frees_cross: AtomicU64,
    frees_unknown: AtomicU64,
    reallocs: AtomicU64,
    reallocs_cross: AtomicU64,
    realloc_grow: AtomicU64,
    realloc_shrink: AtomicU64,
    realloc_moved: AtomicU64,
    realloc_from_null: AtomicU64,
    realloc_to_zero: AtomicU64,
    bytes_alloc: AtomicU64,
    realloc_bytes_grown: AtomicU64,
    size_count: [AtomicU64; SIZE_BUCKETS],
    size_bytes: [AtomicU64; SIZE_BUCKETS],
    life: [AtomicU64; LIFE_BUCKETS],
    ratio: [AtomicU64; RATIO_BUCKETS],
    realloc_old: [AtomicU64; SIZE_BUCKETS],
}

const Z: AtomicU64 = AtomicU64::new(0);
// Padded so that two threads' slots do not share a line.
#[repr(align(128))]
struct Padded(Slot);
#[allow(clippy::declare_interior_mutable_const)]
const EMPTY: Padded = Padded(Slot {
    allocs: Z,
    callocs: Z,
    aligned: Z,
    frees: Z,
    frees_cross: Z,
    frees_unknown: Z,
    reallocs: Z,
    reallocs_cross: Z,
    realloc_grow: Z,
    realloc_shrink: Z,
    realloc_moved: Z,
    realloc_from_null: Z,
    realloc_to_zero: Z,
    bytes_alloc: Z,
    realloc_bytes_grown: Z,
    size_count: [Z; SIZE_BUCKETS],
    size_bytes: [Z; SIZE_BUCKETS],
    life: [Z; LIFE_BUCKETS],
    ratio: [Z; RATIO_BUCKETS],
    realloc_old: [Z; SIZE_BUCKETS],
});
static STATS: [Padded; SLOTS] = [EMPTY; SLOTS];

static THREADS: AtomicU32 = AtomicU32::new(0);
static LIVE_BYTES: AtomicI64 = AtomicI64::new(0);
static LIVE_COUNT: AtomicI64 = AtomicI64::new(0);
static PEAK_BYTES: AtomicI64 = AtomicI64::new(0);
static PEAK_COUNT: AtomicI64 = AtomicI64::new(0);
static START_NS: AtomicU64 = AtomicU64::new(0);
static START_TICK: AtomicU64 = AtomicU64::new(0);
static REPORTED: AtomicU32 = AtomicU32::new(0);

thread_local! {
    static TID: Cell<u32> = const { Cell::new(0) };
    static PEND_BYTES: Cell<i64> = const { Cell::new(0) };
    static PEND_COUNT: Cell<i64> = const { Cell::new(0) };
}

fn tid() -> u32 {
    TID.with(|t| {
        let mut v = t.get();
        if v == 0 {
            v = THREADS.fetch_add(1, Relaxed) + 1;
            t.set(v);
        }
        v
    })
}

fn slot(t: u32) -> &'static Slot {
    &STATS[t as usize % SLOTS].0
}

fn tick() -> u64 {
    // SAFETY: rdtsc has no preconditions on x86-64.
    unsafe { core::arch::x86_64::_rdtsc() >> TICK_SHIFT }
}

fn now_ns() -> u64 {
    let mut ts = Timespec { sec: 0, nsec: 0 };
    // SAFETY: ts is a valid out-pointer; CLOCK_MONOTONIC is 1.
    unsafe { clock_gettime(1, &mut ts) };
    ts.sec as u64 * 1_000_000_000 + ts.nsec as u64
}

/// Ceiling of log2, so bucket k holds (2^(k-1), 2^k]; bucket 0 holds 0 and 1.
fn bucket(v: u64, n: usize) -> usize {
    let b = if v <= 1 { 0 } else { 64 - (v - 1).leading_zeros() as usize };
    b.min(n - 1)
}

fn add(a: &AtomicU64, v: u64) {
    a.fetch_add(v, Relaxed);
}

fn account(bytes: i64, count: i64) {
    let b = PEND_BYTES.with(|c| c.get()) + bytes;
    let n = PEND_COUNT.with(|c| c.get()) + count;
    if b.abs() >= FLUSH_BYTES || n.abs() >= FLUSH_COUNT {
        let lb = LIVE_BYTES.fetch_add(b, Relaxed) + b;
        PEAK_BYTES.fetch_max(lb, Relaxed);
        let ln = LIVE_COUNT.fetch_add(n, Relaxed) + n;
        PEAK_COUNT.fetch_max(ln, Relaxed);
        PEND_BYTES.with(|c| c.set(0));
        PEND_COUNT.with(|c| c.set(0));
    } else {
        PEND_BYTES.with(|c| c.set(b));
        PEND_COUNT.with(|c| c.set(n));
    }
}

struct Block {
    size: u64,
    shift: u32,
    owner: u64,
    birth: u64,
}

/// The header of a block this library handed out, or None for a pointer it did not.
unsafe fn header(p: *mut c_void) -> Option<Block> {
    // SAFETY: the caller passes a non-null pointer returned by an allocation function. Every
    // block this library returns has a header 16 bytes before it; a foreign pointer (allocated
    // before this library was loaded) is told apart by the magic byte.
    let h = unsafe { (p as *const u64).sub(2) };
    let meta = unsafe { h.read() };
    if meta >> 56 != MAGIC {
        return None;
    }
    let life = unsafe { h.add(1).read() };
    Some(Block {
        size: meta & SIZE_MASK,
        shift: ((meta >> 48) & 0xff) as u32,
        owner: life >> BIRTH_BITS,
        birth: life & BIRTH_MASK,
    })
}

unsafe fn write_header(user: *mut u8, size: u64, shift: u32, owner: u32, birth: u64) {
    let h = user as *mut u64;
    // SAFETY: user is at least 16 bytes past the start of a block from glibc.
    unsafe {
        h.sub(2).write((size & SIZE_MASK) | ((shift as u64) << 48) | (MAGIC << 56));
        h.sub(1).write(((owner as u64 & TID_MASK) << BIRTH_BITS) | (birth & BIRTH_MASK));
    }
}

fn raw_of(user: *mut c_void, b: &Block) -> *mut c_void {
    let back = if b.shift > 4 { 1usize << b.shift } else { HDR };
    user.cast::<u8>().wrapping_sub(back).cast()
}

unsafe fn finish_alloc(raw: *mut c_void, back: usize, size: usize, shift: u32) -> *mut c_void {
    if raw.is_null() {
        return raw;
    }
    let t = tid();
    let user = unsafe { raw.cast::<u8>().add(back) };
    unsafe { write_header(user, size as u64, shift, t, tick()) };
    let s = slot(t);
    add(&s.allocs, 1);
    add(&s.bytes_alloc, size as u64);
    let k = bucket(size as u64, SIZE_BUCKETS);
    add(&s.size_count[k], 1);
    add(&s.size_bytes[k], size as u64);
    account(size as i64, 1);
    user.cast()
}

unsafe fn plain(size: usize) -> *mut c_void {
    match size.checked_add(HDR) {
        Some(n) => unsafe { finish_alloc(__libc_malloc(n), HDR, size, 4) },
        None => enomem(),
    }
}

unsafe fn aligned(align: usize, size: usize) -> *mut c_void {
    if align <= HDR {
        return unsafe { plain(size) };
    }
    let align = align.next_power_of_two();
    match size.checked_add(align) {
        Some(n) => {
            let p = unsafe { finish_alloc(__libc_memalign(align, n), align, size, align.trailing_zeros()) };
            if !p.is_null() {
                add(&slot(tid()).aligned, 1);
            }
            p
        }
        None => enomem(),
    }
}

fn enomem() -> *mut c_void {
    // SAFETY: errno is this thread's.
    unsafe { *__errno_location() = 12 };
    core::ptr::null_mut()
}

unsafe fn release(p: *mut c_void) {
    if p.is_null() {
        return;
    }
    let t = tid();
    let s = slot(t);
    let Some(b) = (unsafe { header(p) }) else {
        add(&s.frees_unknown, 1);
        return unsafe { __libc_free(p) };
    };
    add(&s.frees, 1);
    if b.owner != t as u64 & TID_MASK {
        add(&s.frees_cross, 1);
    }
    let age = tick().wrapping_sub(b.birth) & BIRTH_MASK;
    add(&s.life[bucket(age, LIFE_BUCKETS)], 1);
    account(-(b.size as i64), -1);
    unsafe { __libc_free(raw_of(p, &b)) };
}

unsafe fn resize(p: *mut c_void, size: usize) -> *mut c_void {
    let t = tid();
    let s = slot(t);
    if p.is_null() {
        add(&s.realloc_from_null, 1);
        return unsafe { plain(size) };
    }
    if size == 0 {
        // glibc frees the block and returns null.
        add(&s.realloc_to_zero, 1);
        unsafe { release(p) };
        return core::ptr::null_mut();
    }
    let Some(b) = (unsafe { header(p) }) else {
        add(&s.frees_unknown, 1);
        return unsafe { __libc_realloc(p, size) };
    };
    let Some(n) = size.checked_add(HDR) else { return enomem() };
    let old = b.size;
    let user: *mut u8 = if b.shift > 4 {
        // C gives a reallocated block only malloc's alignment, so an aligned block moves to a
        // plain one.
        let raw = unsafe { __libc_malloc(n) };
        if raw.is_null() {
            return raw;
        }
        unsafe {
            core::ptr::copy_nonoverlapping(p.cast::<u8>(), raw.cast::<u8>().add(HDR), (old as usize).min(size));
            __libc_free(raw_of(p, &b));
        }
        add(&s.realloc_moved, 1);
        unsafe { raw.cast::<u8>().add(HDR) }
    } else {
        let raw = raw_of(p, &b);
        let fresh = unsafe { __libc_realloc(raw, n) };
        if fresh.is_null() {
            return fresh;
        }
        if fresh != raw {
            add(&s.realloc_moved, 1);
        }
        unsafe { fresh.cast::<u8>().add(HDR) }
    };
    unsafe { write_header(user, size as u64, 4, t, b.birth) };
    add(&s.reallocs, 1);
    if b.owner != t as u64 & TID_MASK {
        add(&s.reallocs_cross, 1);
    }
    let size = size as u64;
    if size > old {
        add(&s.realloc_grow, 1);
        add(&s.realloc_bytes_grown, size - old);
    } else if size < old {
        add(&s.realloc_shrink, 1);
    }
    // Ratio buckets in quarters: new/old <= 1, 1.25, 1.5, 2, 4, 16, above.
    let r = if size <= old {
        0
    } else if size * 4 <= old * 5 {
        1
    } else if size * 2 <= old * 3 {
        2
    } else if size <= old * 2 {
        3
    } else if size <= old * 4 {
        4
    } else if size <= old * 16 {
        5
    } else {
        6
    };
    add(&s.ratio[r], 1);
    add(&s.realloc_old[bucket(old, SIZE_BUCKETS)], 1);
    account(size as i64 - old as i64, 0);
    user.cast()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn malloc(size: usize) -> *mut c_void {
    unsafe { plain(size) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn calloc(n: usize, size: usize) -> *mut c_void {
    let Some(total) = n.checked_mul(size) else { return enomem() };
    let Some(with_hdr) = total.checked_add(HDR) else { return enomem() };
    let p = unsafe { finish_alloc(__libc_calloc(1, with_hdr), HDR, total, 4) };
    if !p.is_null() {
        add(&slot(tid()).callocs, 1);
    }
    p
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn realloc(p: *mut c_void, size: usize) -> *mut c_void {
    unsafe { resize(p, size) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn reallocarray(p: *mut c_void, n: usize, size: usize) -> *mut c_void {
    match n.checked_mul(size) {
        Some(total) => unsafe { resize(p, total) },
        None => enomem(),
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn free(p: *mut c_void) {
    unsafe { release(p) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn cfree(p: *mut c_void) {
    unsafe { release(p) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn free_sized(p: *mut c_void, _size: usize) {
    unsafe { release(p) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn free_aligned_sized(p: *mut c_void, _align: usize, _size: usize) {
    unsafe { release(p) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn posix_memalign(out: *mut *mut c_void, align: usize, size: usize) -> c_int {
    if !align.is_power_of_two() || align % size_of::<usize>() != 0 {
        return 22;
    }
    let p = unsafe { aligned(align, size) };
    if p.is_null() {
        return 12;
    }
    unsafe { out.write(p) };
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn aligned_alloc(align: usize, size: usize) -> *mut c_void {
    if !align.is_power_of_two() {
        unsafe { *__errno_location() = 22 };
        return core::ptr::null_mut();
    }
    unsafe { aligned(align, size) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn memalign(align: usize, size: usize) -> *mut c_void {
    unsafe { aligned(align, size) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn valloc(size: usize) -> *mut c_void {
    unsafe { aligned(4096, size) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pvalloc(size: usize) -> *mut c_void {
    unsafe { aligned(4096, size.div_ceil(4096).max(1) * 4096) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn malloc_usable_size(p: *mut c_void) -> usize {
    if p.is_null() {
        return 0;
    }
    match unsafe { header(p) } {
        Some(b) => b.size as usize,
        None => 0,
    }
}

struct Out {
    fd: c_int,
    buf: [u8; 4096],
    len: usize,
}

impl Out {
    fn flush(&mut self) {
        let mut at = 0;
        while at < self.len {
            // SAFETY: buf[at..len] is initialised.
            let n = unsafe { write(self.fd, self.buf.as_ptr().add(at).cast(), self.len - at) };
            if n <= 0 {
                break;
            }
            at += n as usize;
        }
        self.len = 0;
    }
    fn bytes(&mut self, s: &[u8]) -> &mut Self {
        for &b in s {
            if self.len == self.buf.len() {
                self.flush();
            }
            self.buf[self.len] = b;
            self.len += 1;
        }
        self
    }
    fn num(&mut self, v: i64) -> &mut Self {
        let mut d = [0u8; 21];
        let mut n = 0;
        let neg = v < 0;
        let mut u = v.unsigned_abs();
        loop {
            d[n] = b'0' + (u % 10) as u8;
            n += 1;
            u /= 10;
            if u == 0 {
                break;
            }
        }
        if neg {
            self.bytes(b"-");
        }
        while n > 0 {
            n -= 1;
            self.bytes(&[d[n]]);
        }
        self
    }
    fn field(&mut self, name: &str, v: i64) -> &mut Self {
        self.bytes(b",\"").bytes(name.as_bytes()).bytes(b"\":").num(v)
    }
    fn array(&mut self, name: &str, f: impl Fn(&Slot) -> &[AtomicU64]) -> &mut Self {
        self.bytes(b",\"").bytes(name.as_bytes()).bytes(b"\":[");
        let n = f(&STATS[0].0).len();
        for i in 0..n {
            if i > 0 {
                self.bytes(b",");
            }
            let total: u64 = STATS.iter().map(|s| f(&s.0)[i].load(Relaxed)).sum();
            self.num(total as i64);
        }
        self.bytes(b"]")
    }
}

fn total(f: impl Fn(&Slot) -> &AtomicU64) -> i64 {
    STATS.iter().map(|s| f(&s.0).load(Relaxed)).sum::<u64>() as i64
}

fn report() {
    if REPORTED.swap(1, Relaxed) != 0 {
        return;
    }
    let end_ns = now_ns();
    let end_tick = tick();
    // SAFETY: the name is NUL-terminated; getenv does not allocate.
    let base = unsafe { getenv(c"ALLOC_PROFILE_OUT".as_ptr()) };
    if base.is_null() {
        return;
    }
    let mut path = [0u8; 4096];
    let mut len = 0;
    // SAFETY: base is a NUL-terminated string from the environment.
    unsafe {
        while *base.add(len) != 0 && len < 4000 {
            path[len] = *base.add(len) as u8;
            len += 1;
        }
    }
    let mut pid = unsafe { getpid() } as u64;
    let mut digits = [0u8; 20];
    let mut nd = 0;
    loop {
        digits[nd] = b'0' + (pid % 10) as u8;
        nd += 1;
        pid /= 10;
        if pid == 0 {
            break;
        }
    }
    path[len] = b'.';
    len += 1;
    while nd > 0 {
        nd -= 1;
        path[len] = digits[nd];
        len += 1;
    }
    for &b in b".json\0" {
        path[len] = b;
        len += 1;
    }
    // O_WRONLY | O_CREAT | O_TRUNC | O_CLOEXEC
    let fd = unsafe { open(path.as_ptr().cast(), 0o1 | 0o100 | 0o1000 | 0o2000000, 0o644) };
    if fd < 0 {
        return;
    }
    let mut exe = [0u8; 1024];
    let n = unsafe { readlink(c"/proc/self/exe".as_ptr(), exe.as_mut_ptr().cast(), exe.len()) };
    let exe = &exe[..n.max(0) as usize];
    let mut o = Out { fd, buf: [0; 4096], len: 0 };
    o.bytes(b"{\"exe\":\"");
    for &b in exe {
        if b == b'"' || b == b'\\' || b < 0x20 {
            o.bytes(b"?");
        } else {
            o.bytes(&[b]);
        }
    }
    o.bytes(b"\"");
    o.field("pid", unsafe { getpid() } as i64)
        .field("wall_ns", end_ns.wrapping_sub(START_NS.load(Relaxed)) as i64)
        .field("ticks", end_tick.wrapping_sub(START_TICK.load(Relaxed)) as i64)
        .field("tick_shift", TICK_SHIFT as i64)
        .field("threads", THREADS.load(Relaxed) as i64)
        .field("allocs", total(|s| &s.allocs))
        .field("callocs", total(|s| &s.callocs))
        .field("aligned", total(|s| &s.aligned))
        .field("frees", total(|s| &s.frees))
        .field("frees_cross", total(|s| &s.frees_cross))
        .field("frees_unknown", total(|s| &s.frees_unknown))
        .field("reallocs", total(|s| &s.reallocs))
        .field("reallocs_cross", total(|s| &s.reallocs_cross))
        .field("realloc_grow", total(|s| &s.realloc_grow))
        .field("realloc_shrink", total(|s| &s.realloc_shrink))
        .field("realloc_moved", total(|s| &s.realloc_moved))
        .field("realloc_from_null", total(|s| &s.realloc_from_null))
        .field("realloc_to_zero", total(|s| &s.realloc_to_zero))
        .field("bytes_alloc", total(|s| &s.bytes_alloc))
        .field("realloc_bytes_grown", total(|s| &s.realloc_bytes_grown))
        .field("peak_live_bytes", PEAK_BYTES.load(Relaxed))
        .field("peak_live_count", PEAK_COUNT.load(Relaxed))
        .field("live_bytes_exit", LIVE_BYTES.load(Relaxed))
        .field("live_count_exit", LIVE_COUNT.load(Relaxed))
        .array("size_count", |s| &s.size_count)
        .array("size_bytes", |s| &s.size_bytes)
        .array("life", |s| &s.life)
        .array("ratio", |s| &s.ratio)
        .array("realloc_old", |s| &s.realloc_old)
        .bytes(b"}\n");
    o.flush();
    unsafe { close(fd) };
}

extern "C" fn on_exit() {
    report();
}

extern "C" fn on_term(_sig: c_int) {
    report();
    unsafe { _exit(143) };
}

extern "C" fn on_load() {
    START_NS.store(now_ns(), Relaxed);
    START_TICK.store(tick(), Relaxed);
    // SAFETY: getenv does not allocate; SIGTERM is 15.
    unsafe {
        if !getenv(c"ALLOC_PROFILE_SIGTERM".as_ptr()).is_null() {
            signal(15, on_term as *const () as usize);
        }
    }
}

#[used]
#[unsafe(link_section = ".init_array")]
static ON_LOAD: extern "C" fn() = on_load;
#[used]
#[unsafe(link_section = ".fini_array")]
static ON_EXIT: extern "C" fn() = on_exit;
