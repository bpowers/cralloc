//! Scratch (bench/probes/server-profile, copied into bench-alloc by count_edit.py): asmalloc
//! behind a wrapper that counts calls and records a window of them. Never measured for time:
//! the wrapper and the recording cost every call something.
//!
//! - `BENCH_PROF_CALLS`: calls by kind, in 64 cache lines picked by the thread's stack, read
//!   from outside through /proc/PID/mem.
//! - `BENCH_PROF_STATE`, written from outside: 0 records nothing; 1 records every call; 2
//!   records the frees (and resizes) of blocks whose address hashes to one in 64, and every
//!   call of a large size, so that a block allocated in the window can be followed to its free.
//! - Each record: time stamp, CPU, the call, its layout and addresses, and whether the call took
//!   a slow path of asmalloc's (its CPU's slow-event count moved during the call). Records go to
//!   a chunk of their thread's own, written to `$BENCH_PROF_DIR/trace-PID.bin` at exit.
use std::alloc::{GlobalAlloc, Layout};
use std::cell::Cell;
use std::sync::atomic::{AtomicPtr, AtomicU64, AtomicUsize, Ordering::*};

static INNER: asmalloc::Asmalloc = asmalloc::Asmalloc;

#[unsafe(no_mangle)]
#[used]
pub static BENCH_PROF_STATE: AtomicU64 = AtomicU64::new(0);

#[repr(C, align(64))]
pub struct Line(pub [AtomicU64; 8]);

pub const ALLOC: usize = 0;
pub const FREE: usize = 1;
pub const REALLOC: usize = 2;
pub const ZEROED: usize = 3;
pub const LARGE_ALLOC: usize = 4;
pub const LARGE_FREE: usize = 5;
pub const LARGE_REALLOC: usize = 6;
pub const BYTES: usize = 7;

#[unsafe(no_mangle)]
#[used]
pub static BENCH_PROF_CALLS: [Line; 64] = [const { Line([const { AtomicU64::new(0) }; 8]) }; 64];

const LARGE: usize = 32 * 1024;
const CHUNK: usize = 4 << 20;
const CHUNKS: usize = 24;

#[repr(C)]
struct Rec {
    tsc: u64,
    ptr: u64,
    old: u64,
    size: u32,
    old_size: u32,
    op: u8,
    flags: u8,
    cpu: u16,
    align_log2: u8,
    _pad: [u8; 3],
}

const _: () = assert!(size_of::<Rec>() == 40);

const OP_ALLOC: u8 = 0;
const OP_FREE: u8 = 1;
const OP_REALLOC: u8 = 2;
const OP_ZEROED: u8 = 3;
const SLOW: u8 = 1;
const MOVED_CPU: u8 = 2;

static BASES: [AtomicPtr<Rec>; CHUNKS] = [const { AtomicPtr::new(std::ptr::null_mut()) }; CHUNKS];
static USED: [AtomicUsize; CHUNKS] = [const { AtomicUsize::new(0) }; CHUNKS];
static NEXT_CHUNK: AtomicUsize = AtomicUsize::new(0);
static DROPPED: AtomicU64 = AtomicU64::new(0);
static REGISTERED: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    static MINE: Cell<(usize, usize)> = const { Cell::new((usize::MAX, 0)) };
}

#[inline(always)]
fn count(i: usize, bytes: usize) {
    let here = 0u8;
    let line = &BENCH_PROF_CALLS[((&raw const here as usize) >> 21) % 64];
    line.0[i].fetch_add(1, Relaxed);
    if bytes != 0 {
        line.0[BYTES].fetch_add(bytes as u64, Relaxed);
    }
}

#[inline(always)]
fn rdtscp() -> (u64, u16) {
    let mut aux = 0u32;
    // SAFETY: rdtscp has no preconditions; Linux keeps the CPU number in TSC_AUX.
    let t = unsafe { core::arch::x86_64::__rdtscp(&mut aux) };
    (t, (aux & 0xfff) as u16)
}

/// One free in 64 by address: the same hash count.py applies to the window's allocations.
#[inline(always)]
fn followed(ptr: *mut u8) -> bool {
    ((ptr as u64 >> 4).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 58) == 0
}

struct Before {
    tsc: u64,
    cpu: u16,
    events: u64,
}

#[inline(always)]
fn before() -> Before {
    let (tsc, cpu) = rdtscp();
    Before { tsc, cpu, events: asmalloc::prof::slow_events(cpu as usize) }
}

#[inline(always)]
fn flags(b: &Before) -> u8 {
    let (_, cpu) = rdtscp();
    if cpu != b.cpu {
        MOVED_CPU
    } else if asmalloc::prof::slow_events(cpu as usize) != b.events {
        SLOW
    } else {
        0
    }
}

fn register() {
    if REGISTERED.swap(1, Relaxed) == 0 {
        // SAFETY: registering a plain extern "C" function.
        unsafe { libc::atexit(dump) };
    }
}

#[allow(clippy::too_many_arguments)]
fn record(b: &Before, fl: u8, op: u8, ptr: *mut u8, size: usize, align: usize, old: *mut u8, old_size: usize) {
    let _ = MINE.try_with(|m| {
        let (mut chunk, mut used) = m.get();
        if chunk == usize::MAX {
            register();
            chunk = NEXT_CHUNK.fetch_add(1, Relaxed);
            used = 0;
            if chunk < CHUNKS {
                // SAFETY: an anonymous private mapping; failure is checked.
                let p = unsafe {
                    libc::mmap(
                        std::ptr::null_mut(),
                        CHUNK * size_of::<Rec>(),
                        libc::PROT_READ | libc::PROT_WRITE,
                        libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_NORESERVE,
                        -1,
                        0,
                    )
                };
                if p == libc::MAP_FAILED {
                    chunk = CHUNKS;
                } else {
                    BASES[chunk].store(p.cast(), Release);
                }
            }
        }
        if chunk >= CHUNKS || used >= CHUNK {
            DROPPED.fetch_add(1, Relaxed);
            m.set((chunk, used));
            return;
        }
        // SAFETY: used < CHUNK, inside this thread's own mapping, which no other thread writes.
        unsafe {
            BASES[chunk].load(Relaxed).add(used).write(Rec {
                tsc: b.tsc,
                ptr: ptr as u64,
                old: old as u64,
                size: size.min(u32::MAX as usize) as u32,
                old_size: old_size.min(u32::MAX as usize) as u32,
                op,
                flags: fl,
                cpu: b.cpu,
                align_log2: align.trailing_zeros() as u8,
                _pad: [0; 3],
            })
        };
        used += 1;
        USED[chunk].store(used, Release);
        m.set((chunk, used));
    });
}

fn now_ns() -> u64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: a valid timespec to write into.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

extern "C" fn dump() {
    let state = BENCH_PROF_STATE.swap(0, Relaxed);
    let _ = state;
    // SAFETY: getenv with a NUL-terminated name; glibc's getenv does not allocate.
    let dir = unsafe { libc::getenv(c"BENCH_PROF_DIR".as_ptr()) };
    if dir.is_null() {
        return;
    }
    // The TSC's rate, for lifetimes in seconds.
    let (t0, n0) = (rdtscp().0, now_ns());
    while now_ns() < n0 + 50_000_000 {}
    let (t1, n1) = (rdtscp().0, now_ns());
    let hz = (t1 - t0) as f64 * 1e9 / (n1 - n0) as f64;
    // SAFETY: dir is NUL-terminated.
    let dir = unsafe { std::ffi::CStr::from_ptr(dir) }.to_string_lossy().into_owned();
    // SAFETY: getpid has no preconditions.
    let path = format!("{dir}/trace-{}.bin\0", unsafe { libc::getpid() });
    // SAFETY: path is NUL-terminated; the descriptor is written and closed below.
    let fd = unsafe { libc::open(path.as_ptr().cast(), libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC, 0o644) };
    if fd < 0 {
        return;
    }
    let write_all = |bytes: &[u8]| {
        let mut off = 0;
        while off < bytes.len() {
            // SAFETY: the slice is live and fd is ours.
            let w = unsafe { libc::write(fd, bytes[off..].as_ptr().cast(), bytes.len() - off) };
            if w <= 0 {
                return;
            }
            off += w as usize;
        }
    };
    let chunks = NEXT_CHUNK.load(Relaxed).min(CHUNKS);
    let mut head = Vec::new();
    head.extend_from_slice(b"PROF0001");
    head.extend_from_slice(&(chunks as u64).to_le_bytes());
    head.extend_from_slice(&hz.to_le_bytes());
    head.extend_from_slice(&DROPPED.load(Relaxed).to_le_bytes());
    write_all(&head);
    for c in 0..chunks {
        let n = USED[c].load(Acquire);
        let base = BASES[c].load(Acquire);
        write_all(&(c as u64).to_le_bytes());
        write_all(&(if base.is_null() { 0 } else { n } as u64).to_le_bytes());
        if !base.is_null() {
            // SAFETY: the first n records of the chunk are written.
            let bytes = unsafe { std::slice::from_raw_parts(base.cast::<u8>(), n * size_of::<Rec>()) };
            write_all(bytes);
        }
    }
    // SAFETY: fd is ours.
    unsafe { libc::close(fd) };
}

pub struct Prof;

// SAFETY: forwards every call to asmalloc unchanged; the counting and recording touch only
// their own tables and mappings.
unsafe impl GlobalAlloc for Prof {
    #[inline]
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let large = l.size() > LARGE;
        count(if large { LARGE_ALLOC } else { ALLOC }, l.size());
        let state = BENCH_PROF_STATE.load(Relaxed);
        if state == 0 || (state == 2 && !large) {
            return unsafe { INNER.alloc(l) };
        }
        let b = before();
        let p = unsafe { INNER.alloc(l) };
        let fl = flags(&b);
        record(&b, fl, OP_ALLOC, p, l.size(), l.align(), std::ptr::null_mut(), 0);
        p
    }

    #[inline]
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        let large = l.size() > LARGE;
        count(if large { LARGE_ALLOC } else { ZEROED }, l.size());
        let state = BENCH_PROF_STATE.load(Relaxed);
        if state == 0 || (state == 2 && !large) {
            return unsafe { INNER.alloc_zeroed(l) };
        }
        let b = before();
        let p = unsafe { INNER.alloc_zeroed(l) };
        let fl = flags(&b);
        record(&b, fl, OP_ZEROED, p, l.size(), l.align(), std::ptr::null_mut(), 0);
        p
    }

    #[inline]
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        let large = l.size() > LARGE;
        count(if large { LARGE_FREE } else { FREE }, 0);
        let state = BENCH_PROF_STATE.load(Relaxed);
        if state == 0 || (state == 2 && !large && !followed(p)) {
            return unsafe { INNER.dealloc(p, l) };
        }
        let b = before();
        unsafe { INNER.dealloc(p, l) };
        let fl = flags(&b);
        record(&b, fl, OP_FREE, p, l.size(), l.align(), std::ptr::null_mut(), 0);
    }

    #[inline]
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        let large = l.size() > LARGE || new > LARGE;
        count(if large { LARGE_REALLOC } else { REALLOC }, 0);
        let state = BENCH_PROF_STATE.load(Relaxed);
        if state == 0 || (state == 2 && !large && !followed(p)) {
            return unsafe { INNER.realloc(p, l, new) };
        }
        let b = before();
        let q = unsafe { INNER.realloc(p, l, new) };
        let fl = flags(&b);
        record(&b, fl, OP_REALLOC, q, new, l.align(), p, l.size());
        q
    }
}
