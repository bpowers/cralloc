//! The large-churn switch, for docs/research/large-churn.md: candidate rules for slack in
//! occupied large slots and for the hold, chosen at run time in one binary. Not product code:
//! `lcs_edit.py` copies it into a scratch copy of the allocator as `back/lcs.rs`.
//!
//! `ASMALLOC_LC`, read once, `t<k>p<0|1>b<NNNN>a<0|1>`, every field present, so that every arm
//! of a comparison has an environment of the same length:
//! - `t` k: a claim returns the slack past k times its block's size, for a block moved in
//!   with room; past its own extent for a whole allocation. 0: off.
//! - `p` 1: a claim returns the slack past both its block's extent and the previous tenant's
//!   final size (what tenants before that one left).
//! - `b` NNNN: keep slack while the slack in occupied slots totals at most NNNN MiB; past it, a
//!   claim returns what lies past twice the size of a moved-in block, past the extent of a
//!   whole one. 9999: an eighth of live large memory. 0000: off.
//! - `a` 1: each free large slot ages on its own: the hold also walks regions whose stamp is
//!   young and returns their free slots whose own free is a hold old.
//! Unset, or all zero: main.
//!
//! Every rule acts at the holder's own calls (the claim, the free); the hold's walk claims
//! each slot it returns, as every purge does.
#![allow(missing_docs, dead_code, clippy::all)]
use super::Back;
use super::hold::held_for;
use super::host::ClassHost;
use super::layout::Recorded;
use super::policy::HOLD_STEPS;
use super::region::{self, Purge};
use super::tree::{self, Host};
use super::word::{BITS, Word};
use crate::classes::Class;
use crate::os::Os;
use crate::space::RegionId;
use core::sync::atomic::{AtomicI64, AtomicU32, AtomicU64, Ordering::Relaxed};

unsafe extern "C" {
    fn getenv(name: *const core::ffi::c_char) -> *const core::ffi::c_char;
}

#[derive(Clone, Copy, Default)]
pub struct Config {
    pub trim: usize,
    pub prev: bool,
    /// MiB, or `EIGHTH`.
    pub bound: usize,
    pub age: bool,
}

pub const EIGHTH: usize = 9999;

impl Config {
    fn slack(&self) -> bool {
        self.trim != 0 || self.prev || self.bound != 0
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
        trim: (w & 0xff) as usize,
        prev: w & (1 << 8) != 0,
        bound: ((w >> 16) & 0xffff) as usize,
        age: w & (1 << 9) != 0,
    }
}

#[cold]
fn load() -> u64 {
    // SAFETY: probe only; getenv with a literal name, read before the program changes its
    // environment.
    let p = unsafe { getenv(c"ASMALLOC_LC".as_ptr()) };
    let mut w = LOADED;
    if !p.is_null() {
        // SAFETY: getenv returns a NUL-terminated string.
        let s = unsafe { core::ffi::CStr::from_ptr(p) }.to_bytes();
        let mut i = 0;
        while i < s.len() {
            let key = s[i];
            i += 1;
            let mut v = 0usize;
            while i < s.len() && s[i].is_ascii_digit() {
                v = v * 10 + (s[i] - b'0') as usize;
                i += 1;
            }
            match key {
                b't' => w |= v as u64 & 0xff,
                b'p' if v != 0 => w |= 1 << 8,
                b'a' if v != 0 => w |= 1 << 9,
                b'b' => w |= (v as u64 & 0xffff) << 16,
                _ => {}
            }
        }
    }
    CFG.store(w, Relaxed);
    w
}

const TABLE: usize = 1 << 20;
/// Per slot: the slack its tenant holds (pages), for the bound's tally; and the fine count of
/// its last free plus one, for its own age.
static SLACK_OF: [AtomicU32; TABLE] = [const { AtomicU32::new(0) }; TABLE];
static FREED_AT: [AtomicU64; TABLE] = [const { AtomicU64::new(0) }; TABLE];
/// Slack in occupied slots, and live large memory (the sizes recorded), for the bound.
static SLACK: AtomicI64 = AtomicI64::new(0);
static LIVE: AtomicI64 = AtomicI64::new(0);

/// Counters, written by `report`: claims that returned slack and the bytes they returned (by
/// the clean bits); and the per-slot hold's looks, the slots they examined and what they
/// returned.
static TRIMS: AtomicU64 = AtomicU64::new(0);
static TRIMMED: AtomicU64 = AtomicU64::new(0);
static AGE_LOOKS: AtomicU64 = AtomicU64::new(0);
static AGE_SLOTS: AtomicU64 = AtomicU64::new(0);
static AGE_RETURNED: AtomicU64 = AtomicU64::new(0);
static SLACK_MAX: AtomicI64 = AtomicI64::new(0);

fn key(region: RegionId, slot: u32) -> usize {
    let h = (region.index() as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ u64::from(slot).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    (h >> 40) as usize % TABLE
}

fn dirty<O: Os>(host: &ClassHost<'_, O>, region: RegionId, from: u32, to: u32) -> usize {
    if from >= to {
        return 0;
    }
    let layout = host.layout();
    let mut clean = 0;
    for (w, mask) in region::page_words(from, to) {
        clean += region::count(host.meta(region, layout.clean_word(w)).load() & mask);
    }
    (to - from) as usize - clean
}

/// For the claim of large slot `slot` for a block of `size` bytes, after it settled and before
/// it records the new tenant; `before` is the size word the last tenant left.
pub(super) fn claim<O: Os>(host: &ClassHost<'_, O>, region: RegionId, slot: u32, size: usize, before: Option<Recorded>) {
    let cfg = config();
    if !cfg.slack() {
        return;
    }
    let layout = host.layout();
    let Some(unit) = layout.slot_unit(slot) else {
        return;
    };
    let page = layout.page_size();
    let end = region::extent_of(layout, slot, unit, size, false);
    let moved = layout.size() >= 3 * size;
    let past = |times: usize| region::extent_of(layout, slot, unit, size.saturating_mul(times), false);
    let mut from = unit.end;
    if cfg.trim != 0 {
        from = from.min(if moved { past(cfg.trim) } else { end });
    }
    if cfg.prev
        && let Some(b) = before
    {
        from = from.min(region::extent_of(layout, slot, unit, b.size, b.huge).max(end));
    }
    if cfg.bound != 0 {
        let found = dirty(host, region, end, unit.end) * page;
        let bound = if cfg.bound == EIGHTH {
            (LIVE.load(Relaxed).max(0) as usize) / 8
        } else {
            cfg.bound << 20
        };
        if SLACK.load(Relaxed).max(0) as usize + found > bound {
            from = from.min(if moved { past(2) } else { end });
        }
    }
    // A slot that kept its huge-page advice is left alone: purging part of a huge page returns
    // nothing until the kernel splits it.
    if from < unit.end && !before.is_some_and(|b| b.huge) {
        let pages = dirty(host, region, from, unit.end);
        if pages != 0 {
            let start = from as usize * page - layout.slot_bytes(slot);
            let len = (unit.end - from) as usize * page;
            // SAFETY: probe only. The caller has just claimed the slot and its new block will
            // write nothing past `from`, which lies at or past its extent.
            if unsafe { region::purge_part(host, region, slot, start, len) } {
                region::mark_clean(host, region, slot, start, len);
                TRIMS.fetch_add(1, Relaxed);
                TRIMMED.fetch_add((pages * page) as u64, Relaxed);
            }
        }
    }
    if cfg.bound != 0 {
        let left = dirty(host, region, end, unit.end);
        let k = key(region, slot);
        let was = SLACK_OF[k].swap(left as u32, Relaxed) as i64;
        let now = SLACK.fetch_add((left * page) as i64 - was * page as i64, Relaxed);
        SLACK_MAX.fetch_max(now, Relaxed);
        LIVE.fetch_add(size as i64, Relaxed);
    }
}

/// For a resize in place of the block in large slot `slot` from `old` to `size` bytes, after it.
pub(super) fn resized<O: Os>(host: &ClassHost<'_, O>, region: RegionId, slot: u32, old: usize, size: usize) {
    if config().bound == 0 {
        return;
    }
    let layout = host.layout();
    let (Some(unit), Some(now)) = (layout.slot_unit(slot), region::recorded(host, region, slot)) else {
        return;
    };
    let end = region::extent_of(layout, slot, unit, now.size, now.huge);
    let left = dirty(host, region, end, unit.end);
    let was = SLACK_OF[key(region, slot)].swap(left as u32, Relaxed) as i64;
    SLACK.fetch_add((left as i64 - was) * layout.page_size() as i64, Relaxed);
    LIVE.fetch_add(size as i64 - old as i64, Relaxed);
}

/// For the holder giving up large slot `slot` (a free, or the old slot of a move), before it
/// releases it.
pub(super) fn left<O: Os>(back: &Back<'_, O>, host: &ClassHost<'_, O>, region: RegionId, slot: u32) {
    let cfg = config();
    let k = key(region, slot);
    if cfg.bound != 0 {
        let was = SLACK_OF[k].swap(0, Relaxed) as i64;
        SLACK.fetch_sub(was * host.layout().page_size() as i64, Relaxed);
        if let Some(now) = region::recorded(host, region, slot) {
            LIVE.fetch_sub(now.size as i64, Relaxed);
        }
    }
    if cfg.age {
        FREED_AT[k].store(u64::from(back.state.hold.fine.count()) + 1, Relaxed);
    }
}

impl<O: Os> Back<'_, O> {
    /// The hold's look at a large region whose stamp is young, with per-slot ageing on: from
    /// the top, returns its free slots whose own free is `steps` fine steps old, within `limit`
    /// bytes and `budget` slots examined. The bytes returned and the slots examined.
    pub(super) fn lcs_young_region(
        &self,
        class: Class,
        region: RegionId,
        count: u32,
        steps: u32,
        limit: usize,
        budget: usize,
    ) -> (usize, usize) {
        if !config().age {
            return (0, 0);
        }
        let Some(host) = self.host(class) else {
            return (0, 0);
        };
        let layout = *host.layout();
        let carved = super::layout::Grow::unpack(host.grow(region).load()).carved.min(layout.slots());
        AGE_LOOKS.fetch_add(1, Relaxed);
        let (mut seen, mut purged) = (0usize, 0usize);
        let mut slot = carved;
        while slot > 0 && seen < budget && purged < limit {
            slot -= 1;
            seen += 1;
            if tree::word(&host, tree::Node::leaf(region, slot / BITS)).load() & (1u64 << (slot % BITS)) == 0 {
                continue;
            }
            let at = FREED_AT[key(region, slot)].load(Relaxed);
            if at == 0 || !held_for(super::hold::stamp_of(at as u32 - 1), count, steps) {
                continue;
            }
            let Some(unit) = layout.slot_unit(slot) else { continue };
            if dirty(&host, region, unit.start, unit.end) == 0 {
                continue;
            }
            if let (Purge::Done { returned, outgrown }, _) = region::purge_unit(&host, region, unit) {
                self.forget_outgrown(outgrown);
                purged += returned;
            }
        }
        self.unretain(class, purged);
        AGE_SLOTS.fetch_add(seen as u64, Relaxed);
        AGE_RETURNED.fetch_add(purged as u64, Relaxed);
        (purged, seen)
    }
}

/// The counters, as one line.
pub fn report(out: &mut dyn FnMut(&str)) {
    let mut line = Line { buf: [0; 256], len: 0 };
    let _ = core::fmt::Write::write_fmt(
        &mut line,
        format_args!(
            "lcs trims={} trimmed={} slack_now={} slack_max={} live_large={} age_looks={} age_slots={} age_returned={}",
            TRIMS.load(Relaxed),
            TRIMMED.load(Relaxed),
            SLACK.load(Relaxed),
            SLACK_MAX.load(Relaxed),
            LIVE.load(Relaxed),
            AGE_LOOKS.load(Relaxed),
            AGE_SLOTS.load(Relaxed),
            AGE_RETURNED.load(Relaxed)
        ),
    );
    out(core::str::from_utf8(&line.buf[..line.len]).unwrap_or(""));
}

struct Line {
    buf: [u8; 256],
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
