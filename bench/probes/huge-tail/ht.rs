//! Scratch switch and counters for docs/research/huge-tail.md, put into a copy of the
//! allocator by bench/probes/huge-tail/ht_edit.py. Not product code.
//!
//! The rule under test: the part of a region's huge-page part above its aged frontier (the
//! highest page its claims reached in the last two intervals, rounded up to a huge page) is
//! advised against huge pages again, and the purge walk returns free pages there as it does in
//! the small-page part, instead of only whole 2 MiB units. When a claim reaches above the
//! boundary, the advice goes back on.
//!
//! The boundary is the aged frontier rounded up to a huge page, in both directions: the walk
//! lowers it when the frontier has fallen, and when a claim reaches above it, the claim marks
//! the region and the background thread (or, where there is none, the next heartbeat) raises
//! it to the frontier rounded up, putting the advice back on that far.
//!
//! `ASMALLOC_HT`, read once, letters each followed by digits, e.g. `t1s1m16h0b1w0d1`:
//! - `t` 1: the rule is on. Unset or 0: main's behaviour.
//! - `s` 1 (default): before returning pages of a 2 MiB unit in the tail, ask the kernel to
//!   split its huge page (`MADV_COLD` on the run), so that the return frees memory at once; 0:
//!   plain `MADV_DONTNEED`, which frees it only when the kernel splits the page under pressure.
//! - `m` MiB: the tail must be at least this long, from the boundary to the region's carved
//!   end, before a region is first advised against again (default 16).
//! - `h` intervals: a region whose advice went back on is not advised against again until this
//!   many counted intervals have passed (default 0).
//! - `b` 1 (default): the claim that crosses the boundary leaves a mark and the background
//!   thread or a heartbeat puts the advice back; 0: the claim does it itself.
//! - `w` MiB: the advice goes back on up to the frontier plus this much, rounded up to a huge
//!   page (default 16); 4095: to the region's end.
//! - `u` pages: a unit of the tail whose first run found is shorter than this is not split
//!   (default 0, the walk's own minimum run).
//! - `p` intervals: patience. 0 (default): the boundary comes down to the aged frontier,
//!   what claims reached in the last two intervals. N: it comes down only once N counted
//!   intervals have passed since the region's window opened, to the highest page claims
//!   reached in that window (or the aged frontier, if higher), and the window opens again then
//!   (a tumbling window, so the patience is between N and 2N intervals).
//! - `k` pages: a split counts against the walk step's bound as this many pages returned
//!   (default 0: not counted), so that a step on a request's slow path does at most a bounded
//!   number of splits.
//! - `g` 1: the walk lowers a boundary only in a step that may return memory (a clocked
//!   pass over the floor, or growth's payment), not in a pass that only counts. 0 (default):
//!   in any step.
//! - `r` 1: the advice comes off only where memory goes back. A walk step that may return
//!   memory walks the units above the frontier as small pages; after it, the boundary comes
//!   down, in one call, to the lowest unit from which it returned part (a unit returned whole
//!   needs no change). The raise is unchanged, and has nothing to do where the boundary never
//!   came down. 0 (default): the boundary follows the frontier down as variant 1 does.
//!   2: as 1, but the lowest unit returned is kept for the region until the walk has reached
//!   its bottom, and the boundary comes down once a region's pass.
//! - `z` 1 (default): a wholly free unit of the tail goes back in one call over its 2 MiB,
//!   which frees its page table, so that a fault there after the advice is back is a huge page.
//! - `d` 1: count, and append the counters once every ten intervals, and at each reading
//!   `ht::dump` is asked for, to the file `HT_DIAG` names.
#![allow(missing_docs, dead_code, clippy::all)]
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering::Relaxed};

unsafe extern "C" {
    fn syscall(n: i64, ...) -> i64;
    fn clock_gettime(clock: i32, ts: *mut [i64; 2]) -> i32;
    fn getenv(name: *const core::ffi::c_char) -> *const core::ffi::c_char;
    fn open(path: *const core::ffi::c_char, flags: i32, mode: u32) -> i32;
    fn write(fd: i32, buf: *const u8, n: usize) -> isize;
    fn close(fd: i32) -> i32;
}

const REGIONS: usize = crate::space::MAX_REGIONS;
/// Units of 2 MiB in a region of 4 GiB, as bits.
const UNIT_WORDS: usize = 32;

#[derive(Clone, Copy)]
pub struct Config {
    pub on: bool,
    pub split: bool,
    pub min_mib: u32,
    pub hold: u32,
    pub beat: bool,
    pub step_mib: u32,
    pub unit_min: u32,
    pub diag: bool,
    pub whole: bool,
    pub patience: u32,
    pub split_pages: u32,
    pub goal: bool,
    pub lazy: bool,
    pub per_pass: bool,
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
        on: w & 1 != 0,
        split: w & 2 != 0,
        beat: w & 4 != 0,
        diag: w & 8 != 0,
        whole: w & 16 != 0,
        patience: ((w >> 50) & 0xff) as u32,
        split_pages: ((w >> 58) & 0x1f) as u32 * 16,
        goal: w & 32 != 0,
        lazy: w & 64 != 0,
        per_pass: w & 128 != 0,
        min_mib: ((w >> 8) & 0xfff) as u32,
        hold: ((w >> 20) & 0xff) as u32,
        step_mib: ((w >> 28) & 0xfff) as u32,
        unit_min: ((w >> 40) & 0x3ff) as u32,
    }
}

#[cold]
fn load() -> u64 {
    let (mut t, mut s, mut m, mut h, mut b, mut wv, mut u, mut d, mut z, mut pa, mut k, mut g, mut r) = (0u64, 1u64, 16u64, 0u64, 1u64, 16u64, 0u64, 0u64, 1u64, 0u64, 0u64, 0u64, 0u64);
    // SAFETY: getenv returns null or a NUL-terminated string.
    let p = unsafe { getenv(c"ASMALLOC_HT".as_ptr()) };
    if !p.is_null() {
        let mut i = 0;
        let mut key = 0u8;
        let mut val: Option<u64> = None;
        loop {
            // SAFETY: within the NUL-terminated string.
            let c = unsafe { *p.add(i) } as u8;
            if c.is_ascii_digit() {
                val = Some(val.unwrap_or(0) * 10 + (c - b'0') as u64);
            } else {
                if let Some(v) = val.take() {
                    match key {
                        b't' => t = (v != 0) as u64,
                        b's' => s = (v != 0) as u64,
                        b'm' => m = v.min(0xfff),
                        b'h' => h = v.min(0xff),
                        b'b' => b = (v != 0) as u64,
                        b'w' => wv = v.min(0xfff),
                        b'u' => u = v.min(0x3ff),
                        b'd' => d = (v != 0) as u64,
                        b'z' => z = (v != 0) as u64,
                        b'p' => pa = v.min(0xff),
                        b'k' => k = (v / 16).min(0x1f),
                        b'g' => g = (v != 0) as u64,
                        b'r' => r = v.min(2),
                        _ => {}
                    }
                }
                if c == 0 {
                    break;
                }
                key = c;
            }
            i += 1;
        }
    }
    let w = LOADED | t | s << 1 | b << 2 | d << 3 | z << 4 | g << 5 | ((r != 0) as u64) << 6 | ((r == 2) as u64) << 7 | m << 8 | h << 20 | wv << 28 | u << 40 | pa << 50 | k << 58;
    CFG.store(w, Relaxed);
    w
}

/// Each region's tail word: the first page of the tail plus one in the low 32 bits (zero: no
/// tail); the counted interval of the last time the advice went back on in bits 32..62; bit 63
/// a claim's mark that it should go back on (`b1`).
static TAIL: [AtomicU64; REGIONS] = [const { AtomicU64::new(0) }; REGIONS];
const PENDING: u64 = 1 << 63;
const STAMP_MASK: u64 = (1 << 31) - 1;
/// Regions with a mark, so a heartbeat scans only when there is one.
static MARKED: AtomicU32 = AtomicU32::new(0);
/// The units of each region's tail that a split was asked for since the advice last changed.
static SPLIT: [[AtomicU64; UNIT_WORDS]; REGIONS] =
    [const { [const { AtomicU64::new(0) }; UNIT_WORDS] }; REGIONS];

/// The first page of region `index`'s tail, if it has one.
#[inline]
pub fn tail_of(index: usize) -> Option<u32> {
    match TAIL[index % REGIONS].load(Relaxed) as u32 {
        0 => None,
        n => Some(n - 1),
    }
}

fn stamp_of(word: u64) -> u64 {
    (word >> 32) & STAMP_MASK
}

/// Whether the purge walk may advise region `index` against huge pages from page `from` to its
/// end: the rule is on, the boundary would move down (or there is none), the region's
/// carved top (`top`, in pages) lies at least the threshold above `from`, and the hold since
/// the advice last went back on has passed. On `true` the caller makes the call, and then
/// [`set_tail`].
pub fn may_unadvise(index: usize, from: u32, top: u32, page_shift: u32, epoch: u64) -> bool {
    let c = config();
    if !c.on {
        return false;
    }
    let word = TAIL[index % REGIONS].load(Relaxed);
    let tail = word as u32;
    if tail != 0 && tail - 1 <= from {
        return false;
    }
    if word & PENDING != 0 {
        return false;
    }
    let min = ((c.min_mib as u64) << 20 >> page_shift) as u32;
    if top < from || top - from < min.max(1) {
        return false;
    }
    let stamp = stamp_of(word);
    if stamp != 0 && (epoch & STAMP_MASK).wrapping_sub(stamp) < c.hold as u64 {
        return false;
    }
    true
}

/// The part of each region the walk treats as small pages: the tail, or with `r1` in a step
/// that may return memory, everything above the frontier (first page plus one; zero: none).
static ZONE: [AtomicU32; REGIONS] = [const { AtomicU32::new(0) }; REGIONS];

/// With `r2`, each region's lowest unit returned in part since its boundary last came down
/// (its first page, inverted; zero: none), and when it was first noted (cycles, with `d1`).
static LOW: [AtomicU32; REGIONS] = [const { AtomicU32::new(0) }; REGIONS];
static LOW_AT: [AtomicU64; REGIONS] = [const { AtomicU64::new(0) }; REGIONS];
/// From the first return under the advice to the advice coming off, in cycles.
pub static WINDOW_MAX: AtomicU64 = AtomicU64::new(0);
pub static WINDOW_CYC: AtomicU64 = AtomicU64::new(0);
pub static WINDOWS: AtomicU64 = AtomicU64::new(0);

pub fn note_low(index: usize, low: u32) {
    if low == u32::MAX {
        return;
    }
    // Stored inverted, so that zero is none and the lowest is the largest word.
    let old = LOW[index % REGIONS].fetch_max(!low, Relaxed);
    if old == 0 && config().diag {
        LOW_AT[index % REGIONS].store(cycles(), Relaxed);
    }
}

pub fn take_low(index: usize) -> u32 {
    let l = LOW[index % REGIONS].swap(0, Relaxed);
    if l == 0 {
        return u32::MAX;
    }
    if config().diag {
        let w = cycles().wrapping_sub(LOW_AT[index % REGIONS].load(Relaxed));
        WINDOW_MAX.fetch_max(w, Relaxed);
        WINDOW_CYC.fetch_add(w, Relaxed);
        WINDOWS.fetch_add(1, Relaxed);
    }
    !l
}

pub fn set_zone(index: usize, zone: Option<u32>) {
    ZONE[index % REGIONS].store(zone.map_or(0, |z| z + 1), Relaxed);
}

pub fn zone_of(index: usize) -> Option<u32> {
    match ZONE[index % REGIONS].load(Relaxed) {
        0 => None,
        n => Some(n - 1),
    }
}

/// Each region's patience window: the counted interval it opened in (high 32 bits) and the
/// highest page claims reached since (low 32 bits).
static REACH: [AtomicU64; REGIONS] = [const { AtomicU64::new(0) }; REGIONS];

/// For a claim in region `index` that reached page `page`, with `p`: raises the window's
/// highest reach. A load and a compare when it does not reach higher.
#[inline]
pub fn note_reach(index: usize, page: u32) {
    if config().patience == 0 {
        return;
    }
    let r = &REACH[index % REGIONS];
    let mut seen = r.load(Relaxed);
    while page > seen as u32 {
        match r.compare_exchange(seen, (seen & !(u32::MAX as u64)) | page as u64, Relaxed, Relaxed) {
            Ok(_) => return,
            Err(found) => seen = found,
        }
    }
}

/// Where the purge walk may bring region `index`'s boundary down to, from the aged frontier
/// `floor` at counted interval `epoch`: the frontier itself without patience; with it, `None`
/// until the window has been open `p` intervals, then the window's highest reach, and the
/// window opens again.
pub fn patient(index: usize, floor: u32, epoch: u64) -> Option<u32> {
    let n = config().patience as u64;
    if n == 0 {
        return Some(floor);
    }
    let r = &REACH[index % REGIONS];
    let seen = r.load(Relaxed);
    let opened = seen >> 32;
    let now = epoch & (u32::MAX as u64);
    if now.wrapping_sub(opened) < n {
        return None;
    }
    let high = (seen as u32).max(floor);
    // Lost to a racing claim: the next walk decides.
    r.compare_exchange(seen, now << 32 | floor as u64, Relaxed, Relaxed).ok()?;
    Some(high)
}

/// Records region `index`'s tail as starting at page `from`, after the advice call.
pub fn set_tail(index: usize, from: u32) {
    let t = &TAIL[index % REGIONS];
    let word = t.load(Relaxed);
    t.store((word & !(u32::MAX as u64) & !PENDING) | (from as u64 + 1), Relaxed);
    count(&UNADVISE, 1);
    event();
}

/// With `d1`, a dump at an advice change, at most one each counted interval, so that a
/// process whose background thread sleeps still leaves its counters.
fn event() {
    if config().diag {
        let e = EPOCH.load(Relaxed);
        let n = EVENTS.fetch_add(1, Relaxed);
        if LAST_DUMP.swap(e, Relaxed) != e || n % 256 == 0 {
            dump(e);
        }
    }
}

static LAST_DUMP: AtomicU64 = AtomicU64::new(u64::MAX);
static EVENTS: AtomicU64 = AtomicU64::new(0);

/// For a claim that reached page `page` of region `index`: whether the advice should go back
/// on now, and from where to where (pages). With `b1` it marks the region for the next
/// heartbeat instead and returns `None`. One load when the region has no tail.
#[inline]
pub fn reached(index: usize, page: u32, region_pages: u32, huge_pages: u32) -> Option<(u32, u32)> {
    let t = &TAIL[index % REGIONS];
    let word = t.load(Relaxed);
    let tail = word as u32;
    if tail == 0 || page <= tail - 1 {
        return None;
    }
    reached_slow(t, word, page, region_pages, huge_pages)
}

#[cold]
fn reached_slow(t: &AtomicU64, word: u64, page: u32, region_pages: u32, huge_pages: u32) -> Option<(u32, u32)> {
    let c = config();
    if c.beat {
        if word & PENDING == 0 && t.compare_exchange(word, word | PENDING, Relaxed, Relaxed).is_ok() {
            MARKED.fetch_add(1, Relaxed);
            count(&MARKS, 1);
            if c.diag {
                MARK_AT[index_of(t)].store(cycles(), Relaxed);
            }
        }
        return None;
    }
    take_back(t, word, page, region_pages, huge_pages)
}

/// Moves the boundary of the tail whose word was read as `word` up past `page`, and returns
/// the range whose advice the caller puts back on, if this thread won the change.
fn take_back(t: &AtomicU64, word: u64, page: u32, region_pages: u32, huge_pages: u32) -> Option<(u32, u32)> {
    let c = config();
    let from = (word as u32).checked_sub(1)?;
    let to = if c.step_mib >= 4095 {
        region_pages
    } else {
        let end = (page + huge_pages - 1) & !(huge_pages - 1);
        if end >= region_pages { region_pages } else { end }
    };
    let next_tail = if to >= region_pages { 0 } else { to as u64 + 1 };
    let stamp = (crate::ht::epoch_now() & STAMP_MASK) << 32;
    let next = next_tail | stamp;
    match t.compare_exchange(word, next, Relaxed, Relaxed) {
        Ok(_) => Some((from, to)),
        Err(_) => None,
    }
}

/// The counted interval, as the back end last published it (set by `note_epoch`).
static EPOCH: AtomicU64 = AtomicU64::new(0);

pub fn note_epoch(epoch: u64) {
    if EPOCH.load(Relaxed) != epoch {
        EPOCH.store(epoch, Relaxed);
        LAST_DUMP.store(u64::MAX, Relaxed);
        if config().diag && epoch % 10 == 0 {
            dump(epoch);
        }
    }
}

pub fn epoch_now() -> u64 {
    EPOCH.load(Relaxed)
}

/// Whether some region is marked for its advice to go back on: work for the background
/// thread, which a slow path's heartbeat wakes it for.
#[inline]
pub fn marked() -> bool {
    config().on && MARKED.load(Relaxed) != 0
}

fn index_of(t: &AtomicU64) -> usize {
    (t as *const AtomicU64 as usize - TAIL.as_ptr() as usize) / core::mem::size_of::<AtomicU64>()
}

/// When each region was marked, in cycles (with `d1`), for the lag until its advice went back.
static MARK_AT: [AtomicU64; REGIONS] = [const { AtomicU64::new(0) }; REGIONS];

/// For the background thread, or a heartbeat where there is none, with `b1`: each marked
/// region's tail boundary through `raise`, which puts the advice back on from the boundary to
/// where it says the boundary now is (`None`: leave it) and returns that. Scans only while some
/// region is marked.
pub fn beat(regions: usize, region_pages: u32, mut raise: impl FnMut(usize, u32, bool) -> Option<u32>) {
    if MARKED.load(Relaxed) == 0 {
        return;
    }
    let c = config();
    for index in 0..regions.min(REGIONS) {
        let t = &TAIL[index];
        let word = t.load(Relaxed);
        if word & PENDING == 0 {
            continue;
        }
        let Some(from) = (word as u32).checked_sub(1) else {
            if t.compare_exchange(word, word & !PENDING, Relaxed, Relaxed).is_ok() {
                MARKED.fetch_sub(1, Relaxed);
            }
            continue;
        };
        let to = raise(index, from, c.step_mib >= 4095);
        let tail = match to {
            Some(to) if to >= region_pages => 0,
            Some(to) => to as u64 + 1,
            None => from as u64 + 1,
        };
        let stamp = if to.is_some() { (epoch_now() & STAMP_MASK) << 32 } else { word & (STAMP_MASK << 32) };
        if t.compare_exchange(word, tail | stamp, Relaxed, Relaxed).is_ok() {
            MARKED.fetch_sub(1, Relaxed);
            event();
            if c.diag && to.is_some() {
                let lag = cycles().wrapping_sub(MARK_AT[index].load(Relaxed));
                LAG_CYC.fetch_add(lag, Relaxed);
                LAG_MAX.fetch_max(lag, Relaxed);
            }
        }
    }
}

/// Whether the walk should ask for a split before it returns pages of the run starting at page
/// `page`, a run of `pages` pages in region `index`'s tail; records that it asked, so a unit
/// is asked once while the advice stays as it is.
pub fn wants_split(index: usize, page: u32, pages: u32, huge_pages: u32) -> bool {
    let c = config();
    if !c.split {
        return false;
    }
    let unit = (page / huge_pages) as usize;
    if unit >= UNIT_WORDS * 64 {
        return true;
    }
    let word = &SPLIT[index % REGIONS][unit / 64];
    let bit = 1u64 << (unit % 64);
    if word.load(Relaxed) & bit != 0 {
        return false;
    }
    if pages < c.unit_min {
        count(&SKIPPED, 1);
        return false;
    }
    word.fetch_or(bit, Relaxed);
    true
}

/// Whether a run of `pages` in a unit of region `index`'s tail may be returned: with `u`, a
/// unit not yet split whose first run is shorter is left as it is.
pub fn may_return(index: usize, page: u32, pages: u32, huge_pages: u32) -> bool {
    let c = config();
    if !c.split || c.unit_min == 0 {
        return true;
    }
    let unit = (page / huge_pages) as usize;
    if unit >= UNIT_WORDS * 64 {
        return true;
    }
    SPLIT[index % REGIONS][unit / 64].load(Relaxed) & (1u64 << (unit % 64)) != 0 || pages >= c.unit_min
}

/// Forgets the splits asked for in region `index`, when its advice goes back on.
pub fn forget_splits(index: usize) {
    for w in &SPLIT[index % REGIONS] {
        w.store(0, Relaxed);
    }
}

// --- Counters.
pub static UNADVISE: AtomicU64 = AtomicU64::new(0);
pub static READVISE: AtomicU64 = AtomicU64::new(0);
pub static MARKS: AtomicU64 = AtomicU64::new(0);
pub static SPLITS: AtomicU64 = AtomicU64::new(0);
pub static SKIPPED: AtomicU64 = AtomicU64::new(0);
pub static TAIL_PURGES: AtomicU64 = AtomicU64::new(0);
pub static TAIL_PAGES: AtomicU64 = AtomicU64::new(0);
/// Cycles spent in each kind of call.
pub static UNADVISE_CYC: AtomicU64 = AtomicU64::new(0);
pub static READVISE_CYC: AtomicU64 = AtomicU64::new(0);
pub static SPLIT_CYC: AtomicU64 = AtomicU64::new(0);
pub static TAIL_PURGE_CYC: AtomicU64 = AtomicU64::new(0);
/// The longest of each, in cycles.
pub static UNADVISE_MAX: AtomicU64 = AtomicU64::new(0);
pub static READVISE_MAX: AtomicU64 = AtomicU64::new(0);
pub static SPLIT_MAX: AtomicU64 = AtomicU64::new(0);
/// From a claim's mark to its advice going back on.
pub static LAG_CYC: AtomicU64 = AtomicU64::new(0);
/// The walk's calls in regions with a tail.
pub static STEP_CYC: AtomicU64 = AtomicU64::new(0);
pub static STEP_MAX: AtomicU64 = AtomicU64::new(0);
pub static LAG_MAX: AtomicU64 = AtomicU64::new(0);

#[inline]
pub fn count(c: &AtomicU64, n: u64) {
    if config().diag {
        c.fetch_add(n, Relaxed);
    }
}

#[inline]
pub fn cycles() -> u64 {
    if config().diag {
        // SAFETY: rdtsc has no preconditions.
        unsafe { core::arch::x86_64::_rdtsc() }
    } else {
        0
    }
}

#[inline]
pub fn timed(total: &AtomicU64, max: Option<&AtomicU64>, start: u64) {
    if config().diag {
        // SAFETY: as above.
        let d = unsafe { core::arch::x86_64::_rdtsc() }.wrapping_sub(start);
        total.fetch_add(d, Relaxed);
        if let Some(m) = max {
            m.fetch_max(d, Relaxed);
        }
    }
}

struct Buf {
    b: [u8; 768],
    n: usize,
}

impl Buf {
    fn s(&mut self, s: &str) {
        for &c in s.as_bytes() {
            if self.n < self.b.len() {
                self.b[self.n] = c;
                self.n += 1;
            }
        }
    }
    fn u(&mut self, mut v: u64) {
        let mut d = [0u8; 20];
        let mut i = 0;
        loop {
            d[i] = b'0' + (v % 10) as u8;
            v /= 10;
            i += 1;
            if v == 0 {
                break;
            }
        }
        while i > 0 {
            i -= 1;
            if self.n < self.b.len() {
                self.b[self.n] = d[i];
                self.n += 1;
            }
        }
    }
    fn kv(&mut self, k: &str, v: &AtomicU64) {
        self.s(" ");
        self.s(k);
        self.s("=");
        self.u(v.load(Relaxed));
    }
}

/// Appends the counters to the file `HT_DIAG` names.
pub fn dump(epoch: u64) {
    // SAFETY: getenv returns null or a NUL-terminated string.
    let path = unsafe { getenv(c"HT_DIAG".as_ptr()) };
    if path.is_null() {
        return;
    }
    let mut b = Buf { b: [0; 768], n: 0 };
    b.s("ht epoch=");
    b.u(epoch);
    b.kv("unadvise", &UNADVISE);
    b.kv("readvise", &READVISE);
    b.kv("marks", &MARKS);
    b.kv("splits", &SPLITS);
    b.kv("skipped", &SKIPPED);
    b.kv("tail_purges", &TAIL_PURGES);
    b.kv("tail_pages", &TAIL_PAGES);
    b.kv("unadvise_cyc", &UNADVISE_CYC);
    b.kv("readvise_cyc", &READVISE_CYC);
    b.kv("split_cyc", &SPLIT_CYC);
    b.kv("tail_purge_cyc", &TAIL_PURGE_CYC);
    b.kv("unadvise_max", &UNADVISE_MAX);
    b.kv("readvise_max", &READVISE_MAX);
    b.kv("split_max", &SPLIT_MAX);
    b.kv("lag_cyc", &LAG_CYC);
    b.kv("lag_max", &LAG_MAX);
    b.kv("step_cyc", &STEP_CYC);
    b.kv("step_max", &STEP_MAX);
    b.kv("window_cyc", &WINDOW_CYC);
    b.kv("window_max", &WINDOW_MAX);
    b.kv("windows", &WINDOWS);
    let mut tails = 0u64;
    for t in &TAIL {
        tails += (t.load(Relaxed) as u32 != 0) as u64;
    }
    b.s(" tails=");
    b.u(tails);
    b.s("\n");
    // O_WRONLY | O_CREAT | O_APPEND | O_CLOEXEC
    // SAFETY: the path is NUL-terminated; the buffer holds `n` bytes.
    unsafe {
        let fd = open(path, 0o1 | 0o100 | 0o2000 | 0o2000000, 0o644);
        if fd >= 0 {
            let _ = write(fd, b.b.as_ptr(), b.n);
            close(fd);
        }
    }
}

// --- Tracing (`HT_TRACE=<file>`): one line a walk call in a region with a huge-page part,
// an advice change or a split, with the time, the thread and its CPU, written at once.

static TRACE_FD: core::sync::atomic::AtomicI32 = core::sync::atomic::AtomicI32::new(-2);
static BG_TID: core::sync::atomic::AtomicI64 = core::sync::atomic::AtomicI64::new(0);

fn trace_fd() -> i32 {
    let fd = TRACE_FD.load(Relaxed);
    if fd != -2 {
        return fd;
    }
    // SAFETY: getenv returns null or a NUL-terminated string; open takes one.
    let path = unsafe { getenv(c"HT_TRACE".as_ptr()) };
    let fd = if path.is_null() { -1 } else { unsafe { open(path, 0o1 | 0o100 | 0o2000 | 0o2000000, 0o644) } };
    match TRACE_FD.compare_exchange(-2, fd, Relaxed, Relaxed) {
        Ok(_) => fd,
        Err(found) => {
            if fd >= 0 {
                // SAFETY: our own descriptor.
                unsafe { close(fd) };
            }
            found
        }
    }
}

#[inline]
pub fn tracing() -> bool {
    let fd = TRACE_FD.load(Relaxed);
    fd >= 0 || (fd == -2 && trace_fd() >= 0)
}

/// The time on CLOCK_MONOTONIC, in nanoseconds, for traces.
pub fn mono_ns() -> u64 {
    let mut ts = [0i64; 2];
    // SAFETY: the buffer is a timespec.
    unsafe { clock_gettime(1, &mut ts) };
    ts[0] as u64 * 1_000_000_000 + ts[1] as u64
}

fn tid() -> i64 {
    // SAFETY: gettid has no arguments and no preconditions.
    unsafe { syscall(186) }
}

/// The background thread notes itself, so that traces can say who ran a step.
pub fn note_bg() {
    if tracing() {
        BG_TID.store(tid(), Relaxed);
    }
}

/// Writes one trace line: `kind t0_ns dur_ns tid bg cpu a b c d e`.
pub fn trace(kind: &str, t0: u64, a: u64, b: u64, c: u64, d: u64, e: u64) {
    let fd = trace_fd();
    if fd < 0 {
        return;
    }
    let t1 = mono_ns();
    let mut aux = 0u32;
    // SAFETY: rdtscp has no preconditions; Linux keeps the CPU number in TSC_AUX.
    unsafe { core::arch::x86_64::__rdtscp(&mut aux) };
    let me = tid();
    let mut b2 = Buf { b: [0; 768], n: 0 };
    b2.s(kind);
    for v in [t0, t1.saturating_sub(t0), me as u64, (me == BG_TID.load(Relaxed)) as u64, (aux & 0xfff) as u64, a, b, c, d, e] {
        b2.s(" ");
        b2.u(v);
    }
    b2.s("\n");
    // SAFETY: the buffer holds `n` bytes.
    unsafe {
        let _ = write(fd, b2.b.as_ptr(), b2.n);
    }
}

static LAST_HB: AtomicU64 = AtomicU64::new(0);
static BURST: AtomicU64 = AtomicU64::new(0);

/// At a slow path's heartbeat, with tracing: an `H` line for the first heartbeat after a
/// second without one (the start of a burst), and for every heartbeat in the 20 ms after it.
pub fn note_heartbeat() {
    if !tracing() {
        return;
    }
    let now = mono_ns();
    let last = LAST_HB.swap(now, Relaxed);
    if now.saturating_sub(last) > 1_000_000_000 {
        BURST.store(now, Relaxed);
        trace("H", now, now - last, 1, 0, 0, 0);
    } else if now.saturating_sub(BURST.load(Relaxed)) < 20_000_000 {
        trace("H", now, now - last, 0, 0, 0, 0);
    }
}
