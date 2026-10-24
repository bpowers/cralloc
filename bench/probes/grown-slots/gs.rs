//! Scratch switch and counters for docs/research/grown-slots.md, put into a copy of the
//! allocator by bench/probes/grown-slots/grown_edit.py. Not product code.
//!
//! `ASMALLOC_GROWN`, read once, letters each followed by digits, e.g. `m1b0016s00r0d0`:
//! - `m` the rule for a grown block's slot (a move's old slot, a grown block's free):
//!   0 main (kept while all retained large-slot memory is within the budget), 1 own (kept while
//!   the kept grown slots alone are within it), 2 keep (always kept), 3 never (always returned),
//!   4 and 5 (a grown block's free always kept, as a plain free is; a move's old slot by 1's
//!   rule or by 0's);
//! - `b` the budget in MiB (main's is 16);
//! - `s` a shift: when not 0 the budget is the larger of `b` and the heap in use shifted right
//!   by it (the heap is live large blocks plus small memory carved and not free);
//! - `r` 1: a free of a large slot asks `mincore` which pages of its extent are resident and
//!   marks the others clean, so that it is charged by what is resident;
//! - `d` 1: count, and write the counters at exit to the file `GS_DIAG` names.
//! Unset, it is main's behaviour with nothing counted.
#![allow(missing_docs, dead_code, clippy::all)]
use core::sync::atomic::{AtomicI64, AtomicU32, AtomicU64, AtomicU8, Ordering::Relaxed};

unsafe extern "C" {
    fn getenv(name: *const core::ffi::c_char) -> *const core::ffi::c_char;
    fn open(path: *const core::ffi::c_char, flags: i32, mode: u32) -> i32;
    fn write(fd: i32, buf: *const u8, n: usize) -> isize;
    fn close(fd: i32) -> i32;
    fn getpid() -> i32;
}

pub const MAIN: u8 = 0;
pub const OWN: u8 = 1;
pub const KEEP: u8 = 2;
pub const NEVER: u8 = 3;
/// A grown block's free keeps its slot, as any free does; a move's old slot is kept within the
/// budget counted over kept grown slots (FREE_OWN) or over all retained large memory
/// (FREE_MAIN).
pub const FREE_OWN: u8 = 4;
pub const FREE_MAIN: u8 = 5;

#[derive(Clone, Copy)]
pub struct Config {
    pub mode: u8,
    pub budget_mib: u32,
    pub shift: u8,
    pub res: bool,
    pub diag: bool,
}

impl Config {
    /// The budget in bytes, from the policy's `base` (used when `b` is unset) and the heap.
    #[inline]
    pub fn budget(&self, base: usize, heap: usize) -> usize {
        let b = if self.budget_mib == u32::MAX { base } else { (self.budget_mib as usize) << 20 };
        if self.shift == 0 { b } else { b.max(heap >> self.shift) }
    }

    /// Whether the heap estimate is needed.
    #[inline]
    pub fn tallies(&self) -> bool {
        self.shift != 0 || self.diag
    }
}

static CFG: AtomicU64 = AtomicU64::new(0);
const LOADED: u64 = 1 << 63;

#[inline]
pub fn config() -> Config {
    let mut w = CFG.load(Relaxed);
    if w & LOADED == 0 {
        w = load();
    }
    Config {
        mode: (w & 0xf) as u8,
        budget_mib: match ((w >> 8) & 0xff_ffff) as u32 {
            0xff_ffff => u32::MAX,
            v => v,
        },
        shift: ((w >> 32) & 0x3f) as u8,
        res: w & (1 << 40) != 0,
        diag: w & (1 << 41) != 0,
    }
}

#[cold]
fn load() -> u64 {
    let (mut mode, mut budget, mut shift, mut res, mut diag) = (0u64, 0xff_ffffu64, 0u64, 0u64, 0u64);
    // SAFETY: getenv returns null or a NUL-terminated string.
    let p = unsafe { getenv(c"ASMALLOC_GROWN".as_ptr()) };
    if !p.is_null() {
        let mut i = 0;
        let mut key = 0u8;
        let mut val: Option<u64> = None;
        loop {
            // SAFETY: within the NUL-terminated string.
            let c = unsafe { *p.add(i) } as u8;
            let flush = |key: u8, v: u64, mode: &mut u64, budget: &mut u64, shift: &mut u64, res: &mut u64, diag: &mut u64| match key {
                b'm' => *mode = v & 0xf,
                b'b' => *budget = v.min(0xff_fffe),
                b's' => *shift = v & 0x3f,
                b'r' => *res = (v != 0) as u64,
                b'd' => *diag = (v != 0) as u64,
                _ => {}
            };
            if c.is_ascii_digit() {
                val = Some(val.unwrap_or(0) * 10 + (c - b'0') as u64);
            } else {
                if let Some(v) = val.take() {
                    flush(key, v, &mut mode, &mut budget, &mut shift, &mut res, &mut diag);
                }
                if c == 0 {
                    break;
                }
                key = c;
            }
            i += 1;
        }
    }
    let w = LOADED | mode | budget << 8 | shift << 32 | res << 40 | diag << 41;
    CFG.store(w, Relaxed);
    w
}

// --- Tags: what last returned a large slot's pages, kept in its size word.
pub const T_NONE: u8 = 0;
pub const T_MOVE: u8 = 1;
pub const T_FREE: u8 = 2;
pub const T_ZERO: u8 = 3;
pub const T_SHRINK: u8 = 4;
pub const T_HOLD: u8 = 5;
pub const T_PASS: u8 = 6;
pub const T_GROWTH: u8 = 7;
pub const T_TRIM: u8 = 8;
pub const TAGS: usize = 9;
pub const AGES: usize = 8;

// --- Counters.
pub const N: usize = 320;
pub const MOVES: usize = 0;
pub const MOVE_BYTES: usize = 1;
pub const KEEP_MOVE_N: usize = 2;
pub const KEEP_MOVE_B: usize = 3;
pub const REFUSE_MOVE_N: usize = 4;
pub const REFUSE_MOVE_B: usize = 5;
pub const KEEP_FREE_N: usize = 6;
pub const KEEP_FREE_B: usize = 7;
pub const REFUSE_FREE_N: usize = 8;
pub const REFUSE_FREE_B: usize = 9;
/// Bytes returned, by tag (10 + tag): the moves' give-back, a grown block's free, an unwritten
/// block's free, a shrink, and the walk's rules (large classes only).
pub const RET: usize = 10;
pub const CARVE_LARGE: usize = 20;
pub const CARVE_SMALL: usize = 21;
pub const REVIVE_SMALL: usize = 22;
pub const LARGE_CLAIMS: usize = 23;
pub const LARGE_CLAIM_BYTES: usize = 24;
pub const FREES_LARGE: usize = 25;
pub const FREE_CHARGE: usize = 26;
pub const RES_CALLS: usize = 27;
pub const RES_CLEANED: usize = 28;
pub const KEPT_REUSED_N: usize = 29;
pub const KEPT_REUSED_B: usize = 30;
pub const KEPT_PURGED_B: usize = 31;
pub const KEPT_MAX: usize = 32;
pub const RETAINED_LARGE_MAX: usize = 33;
pub const HEAP_MAX: usize = 34;
pub const BUDGET_MAX: usize = 35;
pub const RET_SMALL: usize = 36;
pub const LIVE_LARGE_MAX: usize = 37;
/// Clean bytes a claim cleared under its block, by the slot's tag and the age of the tag
/// (64 + tag * AGES + age): pages its tenant faults if it writes them.
pub const CLAIM_BY: usize = 64;
/// The same for a widening of a held block (`region::cover`): growth in place.
pub const COVER_BY: usize = 160;
/// Bytes asked for by claims, by the slot's tag (256 + tag).
pub const CLAIM_ASK_BY: usize = 256;

pub static C: [AtomicU64; N] = [const { AtomicU64::new(0) }; N];

#[inline(always)]
pub fn add(i: usize, v: usize) {
    if v != 0 && i < N && config().diag {
        C[i].fetch_add(v as u64, Relaxed);
    }
}

#[inline(always)]
pub fn max(i: usize, v: usize) {
    if i < N && config().diag && C[i].load(Relaxed) < v as u64 {
        C[i].fetch_max(v as u64, Relaxed);
    }
}

/// Age buckets in fine steps (about 2 ms): under 16 (32 ms), 32, 64 (the hold), 128, 256
/// (0.5 s), 512 (1 s), 4096 (8 s), older.
pub const fn age_bucket(age: u16) -> usize {
    match age {
        0..16 => 0,
        16..32 => 1,
        32..64 => 2,
        64..128 => 3,
        128..256 => 4,
        256..512 => 5,
        512..4096 => 6,
        _ => 7,
    }
}

/// The fine count as the back end last told it, for the code that has no `Back`.
static FINE: AtomicU32 = AtomicU32::new(0);
/// The rule a walk purging now is doing (racy: a hint for the counters).
static RULE: AtomicU8 = AtomicU8::new(T_PASS);

#[inline]
pub fn set_fine(fine: u32) {
    FINE.store(fine, Relaxed);
}
#[inline]
pub fn fine() -> u32 {
    FINE.load(Relaxed)
}
#[inline]
pub fn set_rule(rule: u8, fine: u32) {
    RULE.store(rule, Relaxed);
    FINE.store(fine, Relaxed);
}
#[inline]
pub fn rule() -> u8 {
    RULE.load(Relaxed)
}

pub fn claim(clean: usize, tag: u8, when: u16, now: u32, asked: usize) {
    let t = (tag as usize).min(TAGS - 1);
    let age = age_bucket((now as u16).wrapping_sub(when));
    add(CLAIM_BY + t * AGES + age, clean);
    add(CLAIM_ASK_BY + t, asked);
    add(LARGE_CLAIMS, 1);
    add(LARGE_CLAIM_BYTES, asked);
}

pub fn cover(clean: usize, tag: u8, when: u16) {
    let t = (tag as usize).min(TAGS - 1);
    let age = age_bucket((fine() as u16).wrapping_sub(when));
    add(COVER_BY + t * AGES + age, clean);
}

pub fn ret(large: bool, bytes: usize) {
    if large {
        add(RET + (rule() as usize).min(TAGS - 1), bytes);
    } else {
        add(RET_SMALL, bytes);
    }
}

// --- Tallies: the kept grown slots' charge, live large bytes, small bytes carved.
static KEPT: AtomicI64 = AtomicI64::new(0);
static LIVE_LARGE: AtomicI64 = AtomicI64::new(0);
static CARVED_SMALL: AtomicU64 = AtomicU64::new(0);

/// Adds `delta` to the kept grown slots' charge and returns the new count, at least zero.
#[inline]
pub fn kept_add(delta: i64) -> usize {
    let now = KEPT.fetch_add(delta, Relaxed) + delta;
    let now = now.max(0) as usize;
    if delta > 0 {
        max(KEPT_MAX, now);
    }
    now
}
#[inline]
pub fn kept() -> usize {
    KEPT.load(Relaxed).max(0) as usize
}
#[inline]
pub fn live_add(delta: i64) {
    if config().tallies() {
        let now = LIVE_LARGE.fetch_add(delta, Relaxed) + delta;
        if delta > 0 {
            max(LIVE_LARGE_MAX, now.max(0) as usize);
        }
    }
}
#[inline]
pub fn live_large() -> usize {
    LIVE_LARGE.load(Relaxed).max(0) as usize
}
#[inline]
pub fn carve_small(bytes: usize) {
    if config().tallies() {
        CARVED_SMALL.fetch_add(bytes as u64, Relaxed);
    }
    add(CARVE_SMALL, bytes);
}
#[inline]
pub fn carved_small() -> usize {
    CARVED_SMALL.load(Relaxed) as usize
}

// --- The dump at exit.
#[used]
#[unsafe(link_section = ".fini_array")]
static DUMP: extern "C" fn() = dump;

struct Buf {
    b: [u8; 8192],
    n: usize,
}

impl Buf {
    fn s(&mut self, s: &[u8]) {
        for &c in s {
            if self.n < self.b.len() {
                self.b[self.n] = c;
                self.n += 1;
            }
        }
    }
    fn u(&mut self, mut v: u64) {
        let mut d = [0u8; 20];
        let mut i = d.len();
        loop {
            i -= 1;
            d[i] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        let tail = d;
        self.s(&tail[i..]);
    }
}

extern "C" fn dump() {
    let cfg = config();
    if !cfg.diag {
        return;
    }
    // SAFETY: getenv returns null or a NUL-terminated string.
    let path = unsafe { getenv(c"GS_DIAG".as_ptr()) };
    if path.is_null() {
        return;
    }
    let mut buf = Buf { b: [0; 8192], n: 0 };
    buf.s(b"gs pid=");
    // SAFETY: getpid has no preconditions.
    buf.u(unsafe { getpid() } as u64);
    buf.s(b" mode=");
    buf.u(cfg.mode as u64);
    buf.s(b" budget=");
    buf.u(if cfg.budget_mib == u32::MAX { 16 } else { cfg.budget_mib as u64 });
    buf.s(b" shift=");
    buf.u(cfg.shift as u64);
    buf.s(b" res=");
    buf.u(cfg.res as u64);
    buf.s(b" kept_now=");
    buf.u(kept() as u64);
    for (i, c) in C.iter().enumerate() {
        let v = c.load(Relaxed);
        if v != 0 {
            buf.s(b" ");
            buf.u(i as u64);
            buf.s(b"=");
            buf.u(v);
        }
    }
    buf.s(b"\n");
    // SAFETY: a NUL-terminated path; the buffer is live for `n` bytes.
    unsafe {
        let fd = open(path, 0o1 | 0o100 | 0o2000, 0o644);
        if fd >= 0 {
            let _ = write(fd, buf.b.as_ptr(), buf.n);
            let _ = close(fd);
        }
    }
}
