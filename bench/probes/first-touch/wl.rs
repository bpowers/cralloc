//! Scratch trace of the allocator's calls that take the address space's lock, put into a copy
//! of the allocator by bench/probes/first-touch/wl_edit.py. Not product code.
//!
//! Every `mprotect`, `madvise`, `munmap`, `mmap` and `mincore` the allocator makes is recorded
//! in `ASMALLOC_WL`, an exported ring of records that a watcher reads from the process's memory
//! by its symbol (`wlwatch.py`), so the server makes no extra system call to report. A record
//! is four words:
//! - 0: CLOCK_MONOTONIC at the call's start, ns;
//! - 1: the call's duration in ns (low 32 bits) and the caller's thread id (high 32);
//! - 2: the length of the range;
//! - 3: the operation (bits 0..8), what the caller was doing (bits 8..16), and the range's
//!   page number (bits 16..);
//! - 4: the thread's CPU time during the call, ns (`getrusage(RUSAGE_THREAD)`, user and
//!   system);
//! - 5: the thread's voluntary context switches during the call (low 32 bits: it slept, as on
//!   a lock) and involuntary ones (high 32: it was preempted).
//!
//! Word 0 is written last, after a release fence, and is zero until then, so a reader that
//! sees it nonzero sees the rest. `ASMALLOC_WL.next` counts records ever begun; the ring holds
//! the last `LEN`.
#![allow(missing_docs, dead_code, clippy::all)]
use core::sync::atomic::{AtomicU8, AtomicU64, Ordering::{Relaxed, Release}, fence};

unsafe extern "C" {
    fn clock_gettime(clock: i32, ts: *mut [i64; 2]) -> i32;
    fn gettid() -> i32;
    fn getrusage(who: i32, usage: *mut [i64; 18]) -> i32;
}

pub const LEN: usize = 1 << 21;
pub const WORDS: usize = 6;

#[repr(C)]
pub struct Wl {
    pub next: AtomicU64,
    pub len: u64,
    pub recs: [[AtomicU64; WORDS]; LEN],
}

#[unsafe(no_mangle)]
pub static ASMALLOC_WL: Wl = Wl {
    next: AtomicU64::new(0),
    len: LEN as u64,
    recs: [const { [const { AtomicU64::new(0) }; WORDS] }; LEN],
};

// Operations.
pub const OP_MPROTECT: u8 = 1;
pub const OP_DONTNEED: u8 = 2;
pub const OP_HUGEPAGE: u8 = 3;
pub const OP_NOHUGEPAGE: u8 = 4;
pub const OP_MADV_OTHER: u8 = 5;
pub const OP_MUNMAP: u8 = 6;
pub const OP_MMAP: u8 = 7;
pub const OP_MINCORE: u8 = 8;

// What the caller was doing, as set by `Ctx::enter` on the same stack.
pub const C_NONE: u8 = 0;
pub const C_COMMIT: u8 = 1; // a carve making more of a region writable
pub const C_TO_HUGE: u8 = 2; // a small class's region switching to huge pages
pub const C_PASS: u8 = 3; // the clocked pass
pub const C_HOLD: u8 = 4; // the hold on free large slots
pub const C_GROWTH: u8 = 5; // growth paying for itself
pub const C_TRIM: u8 = 6; // trim / collect
pub const C_MOVE: u8 = 7; // a large block's move (give back, copy window)
pub const C_RESIZE: u8 = 8; // a resize in place (shrink tail, unspill, advice)
pub const C_FREE: u8 = 9; // a large free (advised or unwritten slot)
pub const C_ALLOC: u8 = 10; // a large claim (advice taken back)

/// What each stack is doing, by its address shifted to about a thread stack's size: threads'
/// stacks are megabytes apart, so collisions are rare and cost only a wrong tag.
static CTX: [AtomicU8; 4096] = [const { AtomicU8::new(0) }; 4096];

#[inline(always)]
fn slot() -> usize {
    let here = 0u8;
    ((&raw const here).addr() >> 21) & 4095
}

pub struct Ctx {
    at: usize,
    was: u8,
}

impl Ctx {
    #[inline(always)]
    pub fn enter(c: u8) -> Ctx {
        let at = slot();
        let was = CTX[at].swap(c, Relaxed);
        Ctx { at, was }
    }
}

impl Drop for Ctx {
    #[inline(always)]
    fn drop(&mut self) {
        CTX[self.at].store(self.was, Relaxed);
    }
}

#[inline(always)]
pub fn now() -> u64 {
    let mut ts = [0i64; 2];
    // SAFETY: scratch. clock_gettime writes the timespec it is given.
    unsafe { clock_gettime(1, &mut ts) };
    ts[0] as u64 * 1_000_000_000 + ts[1] as u64
}

/// The thread's CPU time in ns and its voluntary and involuntary context switches.
#[inline(always)]
pub fn usage() -> (u64, u64, u64) {
    let mut u = [0i64; 18];
    // SAFETY: scratch. getrusage writes one struct rusage (18 words on x86-64).
    unsafe { getrusage(1, &mut u) };
    let cpu = (u[0] * 1_000_000 + u[1]) as u64 * 1000 + (u[2] * 1_000_000 + u[3]) as u64 * 1000;
    (cpu, u[16] as u64, u[17] as u64)
}

pub type Usage = (u64, u64, u64);

pub fn record(op: u8, addr: usize, len: usize, t0: u64, t1: u64, before: Usage) {
    let after = usage();
    // SAFETY: scratch. gettid has no preconditions.
    let tid = unsafe { gettid() } as u32 as u64;
    let ctx = CTX[slot()].load(Relaxed) as u64;
    let i = ASMALLOC_WL.next.fetch_add(1, Relaxed) as usize % LEN;
    let r = &ASMALLOC_WL.recs[i];
    r[0].store(0, Relaxed);
    let dur = (t1.saturating_sub(t0)).min(u32::MAX as u64);
    r[1].store(dur | tid << 32, Relaxed);
    r[2].store(len as u64, Relaxed);
    r[3].store(op as u64 | ctx << 8 | ((addr as u64) >> 12) << 16, Relaxed);
    r[4].store(after.0.saturating_sub(before.0), Relaxed);
    let (nv, niv) = (after.1.saturating_sub(before.1), after.2.saturating_sub(before.2));
    r[5].store(nv.min(u32::MAX as u64) | niv.min(u32::MAX as u64) << 32, Relaxed);
    fence(Release);
    r[0].store(t0.max(1), Relaxed);
}
