//! The large-slack rules' switch and counters, for the measurements of the trim at the claim
//! and per-slot ageing (`docs/research/large-slack.md`). Not product code: `lsr_edit.py` copies
//! it into a scratch copy of the allocator as `back/lsr.rs`.
//!
//! `ASMALLOC_LSR=t<0-3>a<0|1>`, read once, both fields present so that every arm of a
//! comparison has an environment of the same length: `t` the trim at the claim, `a` the walk
//! of young regions by slot ages. Unset: `t1a1`, the product as built. The trim, always from
//! the top of the slack down and never below what the claim keeps:
//! - `t0` off;
//! - `t1` all the slack at once;
//! - `t2` at most one purge step (`policy::PURGE_STEP_PAGES`), the bound of every other purge
//!   on a request's path;
//! - `t3` at most the larger of a step and the new block's own size in pages.
//! What a bounded trim leaves stays in the occupied slot until its next claim or its free.
//!
//! Counters, written at exit to the file `ASMALLOC_LSR_OUT` names (appended, one line):
//! - trims: claims that called the kernel to return slack, the pages of their ranges, a
//!   histogram of the pages by power of two, the longest in pages, and the time each call took
//!   in cycle-counter ticks (histogram by power of two, and the longest);
//! - refaults: at the tenant's departure (its free, or the move out of its slot), how many of
//!   the pages its claim's trim returned are resident again, by `mincore`: pages that tenant
//!   faulted back. Kept per slot in a side table of 2^20 entries, by a hash of region and slot;
//!   a collision loses one trim's record.
#![allow(missing_docs, dead_code, clippy::all)]
use super::host::ClassHost;
use super::layout::Unit;
use super::tree::Host;
use crate::os::Os;
use crate::space::RegionId;
use core::sync::atomic::{AtomicU64, Ordering::Relaxed};

unsafe extern "C" {
    fn getenv(name: *const core::ffi::c_char) -> *const core::ffi::c_char;
    fn atexit(f: extern "C" fn()) -> core::ffi::c_int;
    fn open(path: *const core::ffi::c_char, flags: core::ffi::c_int, mode: u32) -> core::ffi::c_int;
    fn write(fd: core::ffi::c_int, buf: *const u8, len: usize) -> isize;
    fn close(fd: core::ffi::c_int) -> core::ffi::c_int;
}

static CFG: AtomicU64 = AtomicU64::new(0);
const LOADED: u64 = 1 << 63;
const TRIM: u64 = 3;
const AGE: u64 = 4;

#[inline]
fn config() -> u64 {
    let w = CFG.load(Relaxed);
    if w & LOADED != 0 { w } else { load() }
}

#[cold]
fn load() -> u64 {
    // SAFETY: probe only; getenv with a literal name.
    let p = unsafe { getenv(c"ASMALLOC_LSR".as_ptr()) };
    let mut w = LOADED | 1 | AGE;
    if !p.is_null() {
        // SAFETY: getenv returns a NUL-terminated string.
        let s = unsafe { core::ffi::CStr::from_ptr(p) }.to_bytes();
        let mut i = 0;
        while i + 1 < s.len() {
            let v = u64::from(s[i + 1].wrapping_sub(b'0'));
            match s[i] {
                b't' => w = w & !TRIM | v & TRIM,
                b'a' if v == 0 => w &= !AGE,
                _ => {}
            }
            i += 2;
        }
    }
    if CFG.compare_exchange(0, w, Relaxed, Relaxed).is_ok() {
        // SAFETY: probe only; registers the report once.
        unsafe { atexit(report_at_exit) };
    }
    w
}

/// Whether the trim at the claim is on.
#[inline]
pub(super) fn trim() -> bool {
    config() & TRIM != 0
}

/// The part of `slack` the claim of a block of `size` bytes returns under the switch: from the
/// top down, all of it, a step, or the larger of a step and the block's pages; `None` if off.
#[inline]
pub(super) fn bound(slack: Unit, size: usize, page: usize) -> Option<Unit> {
    // The product now bounds the trim to a step itself (`Back::trim_slack`), so arm 2 is what
    // ships and arm 1, all at once, is no longer reachable from here.
    let step = super::policy::PURGE_STEP_PAGES;
    let most = match config() & TRIM {
        0 => return None,
        1 | 2 => step,
        _ => step.max(size.div_ceil(page).min(u32::MAX as usize) as u32),
    };
    let start = slack.start.max(slack.end.saturating_sub(most));
    Some(Unit { start, ..slack })
}

/// Whether the walk of young regions by slot ages is on.
#[inline]
pub(super) fn age() -> bool {
    config() & AGE != 0
}

const BUCKETS: usize = 40;
static TRIMS: AtomicU64 = AtomicU64::new(0);
static TRIM_PAGES: AtomicU64 = AtomicU64::new(0);
static MAX_PAGES: AtomicU64 = AtomicU64::new(0);
static MAX_TICKS: AtomicU64 = AtomicU64::new(0);
/// The pages of the slowest trim, and the ticks of the largest (racy: a hint).
static SLOWEST_PAGES: AtomicU64 = AtomicU64::new(0);
static LARGEST_TICKS: AtomicU64 = AtomicU64::new(0);
/// Trims of 1024 pages or more, and their ticks.
static BIG: AtomicU64 = AtomicU64::new(0);
static BIG_TICKS: AtomicU64 = AtomicU64::new(0);
static TICKS: AtomicU64 = AtomicU64::new(0);
static PAGES_HIST: [AtomicU64; BUCKETS] = [const { AtomicU64::new(0) }; BUCKETS];
static TICKS_HIST: [AtomicU64; BUCKETS] = [const { AtomicU64::new(0) }; BUCKETS];
static CHECKED: AtomicU64 = AtomicU64::new(0);
static REFAULTED: AtomicU64 = AtomicU64::new(0);
static TENANTS_REFAULTED: AtomicU64 = AtomicU64::new(0);
static TENANTS_CHECKED: AtomicU64 = AtomicU64::new(0);

const TABLE: usize = 1 << 20;
/// Per slot: the trimmed range, `start | end << 32` in pages from the region's start; zero for
/// none. The key's region and slot are not kept: a collision checks another slot's pages,
/// which only blurs the count.
static TRIMMED: [AtomicU64; TABLE] = [const { AtomicU64::new(0) }; TABLE];

fn key(region: RegionId, slot: u32) -> usize {
    let h = (region.index() as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ u64::from(slot).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    (h >> 40) as usize % TABLE
}

fn bucket(n: u64) -> usize {
    ((64 - n.leading_zeros()) as usize).min(BUCKETS - 1)
}

/// For a trim of `slack`, the pages of slot `slot` of `region`, that took `ticks`.
pub(super) fn trimmed(region: RegionId, slot: u32, slack: Unit, ticks: u64, done: bool) {
    let pages = u64::from(slack.end - slack.start);
    TRIMS.fetch_add(1, Relaxed);
    TRIM_PAGES.fetch_add(pages, Relaxed);
    if MAX_PAGES.fetch_max(pages, Relaxed) < pages {
        LARGEST_TICKS.store(ticks, Relaxed);
    }
    if MAX_TICKS.fetch_max(ticks, Relaxed) < ticks {
        SLOWEST_PAGES.store(pages, Relaxed);
    }
    if pages >= 1024 {
        BIG.fetch_add(1, Relaxed);
        BIG_TICKS.fetch_add(ticks, Relaxed);
    }
    TICKS.fetch_add(ticks, Relaxed);
    PAGES_HIST[bucket(pages)].fetch_add(1, Relaxed);
    TICKS_HIST[bucket(ticks)].fetch_add(1, Relaxed);
    if done {
        TRIMMED[key(region, slot)].store(u64::from(slack.start) | u64::from(slack.end) << 32, Relaxed);
    }
}

/// For the holder of slot `slot` of `region` that is giving it up (a free, or the move out of
/// it), before anything of it is returned: how much of what its claim trimmed it faulted back.
pub(super) fn departing<O: Os>(host: &ClassHost<'_, O>, region: RegionId, slot: u32) {
    let range = TRIMMED[key(region, slot)].swap(0, Relaxed);
    if range == 0 {
        return;
    }
    let (start, end) = (range as u32, (range >> 32) as u32);
    let layout = host.layout();
    let Some(unit) = layout.slot_unit(slot) else {
        return;
    };
    if start < unit.start || end > unit.end || start >= end {
        return;
    }
    let page = layout.page_size();
    let at = host
        .space
        .region(region)
        .map_addr(|a| a.saturating_add(start as usize * page));
    // SAFETY: probe only. Pages inside the slot its holder is giving up and not using.
    let Ok(resident) = (unsafe { host.space.resident(at, (end - start) as usize * page) }) else {
        return;
    };
    CHECKED.fetch_add(u64::from(end - start), Relaxed);
    TENANTS_CHECKED.fetch_add(1, Relaxed);
    REFAULTED.fetch_add(resident as u64, Relaxed);
    if resident != 0 {
        TENANTS_REFAULTED.fetch_add(1, Relaxed);
    }
}

struct Line {
    buf: [u8; 2048],
    len: usize,
}

impl core::fmt::Write for Line {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let n = s.len().min(self.buf.len() - self.len);
        self.buf[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        Ok(())
    }
}

fn hist(line: &mut Line, name: &str, h: &[AtomicU64; BUCKETS]) {
    use core::fmt::Write;
    let _ = write!(line, " {name}=");
    let last = (0..BUCKETS).rev().find(|&i| h[i].load(Relaxed) != 0).unwrap_or(0);
    for i in 0..=last {
        let _ = write!(line, "{}{}", if i == 0 { "" } else { "," }, h[i].load(Relaxed));
    }
}

extern "C" fn report_at_exit() {
    use core::fmt::Write;
    // SAFETY: probe only; getenv with a literal name.
    let p = unsafe { getenv(c"ASMALLOC_LSR_OUT".as_ptr()) };
    if p.is_null() {
        return;
    }
    let mut line = Line { buf: [0; 2048], len: 0 };
    let w = CFG.load(Relaxed);
    let _ = write!(
        line,
        "lsr trim={} age={} trims={} trim_pages={} max_pages={} ticks={} max_ticks={} slowest_pages={} largest_ticks={} big={} big_ticks={} checked={} refaulted={} tenants_checked={} tenants_refaulted={}",
        w & TRIM,
        u8::from(w & AGE != 0),
        TRIMS.load(Relaxed),
        TRIM_PAGES.load(Relaxed),
        MAX_PAGES.load(Relaxed),
        TICKS.load(Relaxed),
        MAX_TICKS.load(Relaxed),
        SLOWEST_PAGES.load(Relaxed),
        LARGEST_TICKS.load(Relaxed),
        BIG.load(Relaxed),
        BIG_TICKS.load(Relaxed),
        CHECKED.load(Relaxed),
        REFAULTED.load(Relaxed),
        TENANTS_CHECKED.load(Relaxed),
        TENANTS_REFAULTED.load(Relaxed),
    );
    hist(&mut line, "pages_log2", &PAGES_HIST);
    hist(&mut line, "ticks_log2", &TICKS_HIST);
    let _ = line.write_str("\n");
    // SAFETY: probe only; a NUL-terminated path from getenv, and a buffer of `len` bytes.
    unsafe {
        let fd = open(p, 0o2101, 0o644); // O_WRONLY | O_CREAT | O_APPEND
        if fd >= 0 {
            let _ = write(fd, line.buf.as_ptr(), line.len);
            let _ = close(fd);
        }
    }
}
