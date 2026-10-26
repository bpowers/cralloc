//! A probe for docs/research/large-churn.md, not product code: `lc_edit.py` copies it into a
//! scratch copy of the allocator as `back/lc.rs` and calls it from the large claim and free.
//!
//! **Slack** is the pages of an occupied large slot past what its tenant may write
//! (`region::extent_of`) whose clean bit is clear: pages an earlier tenant wrote and nothing
//! returned since. A claim settles only the pages under its block, so these stay resident while
//! the slot is held, counted in no tally and out of every purge's reach (a purge claims the
//! slot first). Each claim records, in a side table keyed by region and slot, where the slack
//! it found came from and what kind of tenant took the slot. [`report`] walks every occupied
//! large slot while the program's threads are stopped and sums its slack by those, by age, and
//! by what `mincore` says is resident.
//!
//! The side table is a hint: two slots that hash alike overwrite each other's entry, and the
//! loser's slack is reported as of unknown source.
use super::Back;
use super::host::ClassHost;
use super::layout::Recorded;
use super::region;
use super::tree::{self, Host};
use super::word::{BITS, Word};
use crate::os::Os;
use crate::space::{RegionId, RegionTag};
use core::fmt::Write as _;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering::Relaxed};

const TABLE: usize = 1 << 20;

/// Where the slack a claim found came from.
pub(super) const SOURCES: usize = 6;
const NONE: usize = 0;
/// The slot was kept by a move whose block outgrew it.
const OUTGROWN: usize = 1;
/// The last tenant grew in place past its claim and was freed.
const GREW: usize = 2;
/// The last tenant was a larger whole allocation, freed.
const LARGER_WHOLE: usize = 3;
/// The last tenant was a larger block moved in with room, freed.
const LARGER_MOVED: usize = 4;
/// None of those, or the side table had lost the last tenant.
const OTHER: usize = 5;
const SOURCE_NAMES: [&str; SOURCES] = ["none", "outgrown", "grew", "larger-whole", "larger-moved", "other"];

/// What kind of tenant took the slot: a whole allocation, or a moved block given room (a class
/// three times its size or more: room is four or sixteen times).
const KINDS: usize = 2;
const WHOLE: usize = 0;
const MOVED: usize = 1;
const KIND_NAMES: [&str; KINDS] = ["whole", "moved"];

/// Ages of slack in fine steps (about 2 ms each): under a hold, under 1 s, under 8 s, older.
const AGES: usize = 4;
const AGE_NAMES: [&str; AGES] = ["<125ms", "<1s", "<8s", ">=8s"];

fn age_bucket(steps: u32) -> usize {
    match steps {
        0..64 => 0,
        64..512 => 1,
        512..4096 => 2,
        _ => 3,
    }
}

/// Per slot: valid (63), source (60-62), kind (59), the claim's fine count (0-31). And the
/// size the tenant was claimed for.
static ENTRY: [AtomicU64; TABLE] = [const { AtomicU64::new(0) }; TABLE];
static CLAIMED: [AtomicU64; TABLE] = [const { AtomicU64::new(0) }; TABLE];
/// Over the run: claims, and slack bytes they found, by source and kind.
static CLAIMS: [[AtomicU64; KINDS]; SOURCES] = [const { [const { AtomicU64::new(0) }; KINDS] }; SOURCES];
static FOUND: [[AtomicU64; KINDS]; SOURCES] = [const { [const { AtomicU64::new(0) }; KINDS] }; SOURCES];
/// Slack bytes found at claims, by how long the tenant then held the slot (counted at its free).
static LIVED: [AtomicU64; AGES] = [const { AtomicU64::new(0) }; AGES];

/// Multiples of the claimed size past which a rule might return slack at the claim: the
/// whole slack (past the block's own extent), and past two, four and eight times its size.
const MULTIPLES: [usize; 4] = [1, 2, 4, 8];
/// Per slot, the slack pages a claim found past each multiple.
static BEYOND: [[AtomicU32; TABLE]; 4] = [const { [const { AtomicU32::new(0) }; TABLE] }; 4];
/// Over the run, by tenant kind and multiple: slack bytes found past the multiple, the part of
/// it the tenant had grown over by its free (what a return at the claim would have refaulted),
/// and the claims that found any (the returns a rule would make).
static PAST: [[AtomicU64; 4]; KINDS] = [const { [const { AtomicU64::new(0) }; 4] }; KINDS];
static GROWN_INTO: [[AtomicU64; 4]; KINDS] = [const { [const { AtomicU64::new(0) }; 4] }; KINDS];
static RETURNS: [[AtomicU64; 4]; KINDS] = [const { [const { AtomicU64::new(0) }; 4] }; KINDS];
/// By tenant kind: slack bytes a claim found past the previous tenant's final extent, and the
/// claims that found any.
static STALE: [AtomicU64; KINDS] = [const { AtomicU64::new(0) }; KINDS];
static STALE_CLAIMS: [AtomicU64; KINDS] = [const { AtomicU64::new(0) }; KINDS];

fn key(region: RegionId, slot: u32) -> usize {
    let h = (region.index() as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ u64::from(slot).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    (h >> 40) as usize % TABLE
}

/// The pages of `region` from `from` to `to` (page numbers in the region) whose clean bit is
/// clear.
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

/// For a claim of large slot `slot` for a block of `size` bytes, before it settles: counts the
/// slack it finds and records where it came from. `before` is the slot's size word as the last
/// tenant left it.
pub(super) fn claim<O: Os>(
    back: &Back<'_, O>,
    host: &ClassHost<'_, O>,
    region: RegionId,
    slot: u32,
    size: usize,
    before: Option<Recorded>,
) {
    let layout = host.layout();
    let Some(unit) = layout.slot_unit(slot) else {
        return;
    };
    let end = region::extent_of(layout, slot, unit, size, false);
    let slack = dirty(host, region, end, unit.end) * layout.page_size();
    let k = key(region, slot);
    let (prev, prev_size) = (ENTRY[k].load(Relaxed), CLAIMED[k].load(Relaxed) as usize);
    let kind = if layout.size() >= 3 * size { MOVED } else { WHOLE };
    let source = match before {
        _ if slack == 0 => NONE,
        Some(b) if b.outgrown => OUTGROWN,
        Some(b) if prev >> 63 != 0 && b.size > prev_size => GREW,
        Some(b) if prev >> 63 != 0 && b.size > size => {
            if (prev >> 59) & 1 == MOVED as u64 { LARGER_MOVED } else { LARGER_WHOLE }
        }
        _ => OTHER,
    };
    let count = back.state.hold.fine.count();
    ENTRY[k].store(1 << 63 | (source as u64) << 60 | (kind as u64) << 59 | u64::from(count), Relaxed);
    // The slack in the upper bits, in pages, for the free's count of how long it lived: up to
    // 2^24 pages.
    CLAIMED[k].store(size as u64 & ((1 << 40) - 1) | ((slack / layout.page_size()) as u64) << 40, Relaxed);
    CLAIMS[source][kind].fetch_add(1, Relaxed);
    FOUND[source][kind].fetch_add(slack as u64, Relaxed);
    // The ratchet inside a slot: dirty pages past both the new extent and the extent of the
    // tenant before, which only tenants before that one can have left.
    if let Some(b) = before {
        let prev = region::extent_of(layout, slot, unit, b.size, b.huge);
        let stale = dirty(host, region, prev.max(end), unit.end);
        if stale != 0 {
            STALE[kind].fetch_add((stale * layout.page_size()) as u64, Relaxed);
            STALE_CLAIMS[kind].fetch_add(1, Relaxed);
        }
    }
    for (m, &times) in MULTIPLES.iter().enumerate() {
        let from = region::extent_of(layout, slot, unit, size.saturating_mul(times), false);
        let pages = dirty(host, region, from, unit.end);
        BEYOND[m][k].store(pages as u32, Relaxed);
        if pages != 0 {
            PAST[kind][m].fetch_add((pages * layout.page_size()) as u64, Relaxed);
            RETURNS[kind][m].fetch_add(1, Relaxed);
        }
    }
}

/// For the free of large slot `slot`: counts the slack its claim found by how long the tenant
/// held the slot.
pub(super) fn free<O: Os>(back: &Back<'_, O>, host: &ClassHost<'_, O>, region: RegionId, slot: u32) {
    let k = key(region, slot);
    let e = ENTRY[k].load(Relaxed);
    let claimed = CLAIMED[k].load(Relaxed);
    let pages = claimed >> 40;
    if e >> 63 == 0 || pages == 0 {
        return;
    }
    let layout = host.layout();
    let held = back.state.hold.fine.count().wrapping_sub(e as u32);
    LIVED[age_bucket(held)].fetch_add(pages * layout.page_size() as u64, Relaxed);
    // How much of the slack past each multiple the tenant grew over: what it found there less
    // what is still dirty past both the multiple and the tenant's final extent.
    let (Some(unit), Some(now)) = (layout.slot_unit(slot), region::recorded(host, region, slot)) else {
        return;
    };
    let kind = ((e >> 59) & 1) as usize;
    let size = (claimed & ((1 << 40) - 1)) as usize;
    let last = region::extent_of(layout, slot, unit, now.size, now.huge);
    for (m, &times) in MULTIPLES.iter().enumerate() {
        let found = BEYOND[m][k].swap(0, Relaxed) as usize;
        if found == 0 {
            continue;
        }
        let from = region::extent_of(layout, slot, unit, size.saturating_mul(times), false);
        let left = dirty(host, region, from.max(last), unit.end);
        GROWN_INTO[kind][m].fetch_add((found.saturating_sub(left) * layout.page_size()) as u64, Relaxed);
    }
}

/// Walks every occupied large slot and writes, through `out`, its slack summed by source, kind
/// and age (bytes the clean bits say may be resident, and what `mincore` finds resident), then
/// the run's counts of claims and of the slack they found. For a caller that has stopped every
/// other thread: `mincore` is asked about pages of slots other threads hold.
pub(super) fn report<O: Os>(back: &Back<'_, O>, out: &mut dyn FnMut(&str)) {
    let mut may = [[[0u64; AGES]; KINDS]; SOURCES];
    let mut res = [[[0u64; AGES]; KINDS]; SOURCES];
    let mut slots = 0u64;
    let count = back.state.hold.fine.count();
    let plan = back.space.plan();
    for region in plan.regions() {
        let RegionTag::Class(class) = back.state.table.get(region) else { continue };
        if class.is_small() {
            continue;
        }
        let Some(host) = back.host(class) else { continue };
        let layout = *host.layout();
        let carved = super::layout::Grow::unpack(host.grow(region).load()).carved.min(layout.slots());
        let base = back.space.region(region);
        for slot in 0..carved {
            let (w, bit) = (slot / BITS, slot % BITS);
            if tree::word(&host, tree::Node::leaf(region, w)).load() & (1u64 << bit) != 0 {
                continue;
            }
            let (Some(unit), Some(now)) = (layout.slot_unit(slot), region::recorded(&host, region, slot)) else {
                continue;
            };
            slots += 1;
            let end = region::extent_of(&layout, slot, unit, now.size, now.huge);
            let pages = dirty(&host, region, end, unit.end);
            if pages == 0 {
                continue;
            }
            let e = ENTRY[key(region, slot)].load(Relaxed);
            let (source, kind, age) = if e >> 63 == 0 {
                (OTHER, WHOLE, AGES - 1)
            } else {
                (((e >> 60) & 7) as usize, ((e >> 59) & 1) as usize, age_bucket(count.wrapping_sub(e as u32)))
            };
            let source = if source == NONE { OTHER } else { source };
            let page = layout.page_size();
            may[source][kind][age] += (pages * page) as u64;
            let from = base.as_ptr().wrapping_add(end as usize * page);
            if let Some(from) = NonNull::new(from) {
                // SAFETY: probe only. The range is whole pages inside a held slot of the
                // reservation, and the caller has stopped every thread that could write it.
                if let Ok(n) = unsafe { back.space.resident(from, (unit.end - end) as usize * page) } {
                    res[source][kind][age] += (n * page) as u64;
                }
            }
        }
    }
    let mut line = Line::new();
    let _ = write!(line, "lc-slots occupied={slots}");
    out(line.as_str());
    for s in 1..SOURCES {
        for k in 0..KINDS {
            for a in 0..AGES {
                if may[s][k][a] != 0 {
                    let mut line = Line::new();
                    let _ = write!(
                        line,
                        "lc-slack source={} kind={} age={} may={} resident={}",
                        SOURCE_NAMES[s], KIND_NAMES[k], AGE_NAMES[a], may[s][k][a], res[s][k][a]
                    );
                    out(line.as_str());
                }
            }
        }
    }
    for s in 0..SOURCES {
        for k in 0..KINDS {
            let n = CLAIMS[s][k].load(Relaxed);
            if n != 0 {
                let mut line = Line::new();
                let _ = write!(
                    line,
                    "lc-claims source={} kind={} claims={n} found={}",
                    SOURCE_NAMES[s], KIND_NAMES[k], FOUND[s][k].load(Relaxed)
                );
                out(line.as_str());
            }
        }
    }
    for a in 0..AGES {
        let mut line = Line::new();
        let _ = write!(line, "lc-lived age={} bytes={}", AGE_NAMES[a], LIVED[a].load(Relaxed));
        out(line.as_str());
    }
    for k in 0..KINDS {
        let mut line = Line::new();
        let _ = write!(
            line,
            "lc-stale kind={} bytes={} claims={}",
            KIND_NAMES[k],
            STALE[k].load(Relaxed),
            STALE_CLAIMS[k].load(Relaxed)
        );
        out(line.as_str());
        for (m, &times) in MULTIPLES.iter().enumerate() {
            let mut line = Line::new();
            let _ = write!(
                line,
                "lc-past kind={} times={times} found={} grown_into={} claims={}",
                KIND_NAMES[k],
                PAST[k][m].load(Relaxed),
                GROWN_INTO[k][m].load(Relaxed),
                RETURNS[k][m].load(Relaxed)
            );
            out(line.as_str());
        }
    }
}

/// A line formatted in place, with no allocation.
struct Line {
    buf: [u8; 256],
    len: usize,
}

impl Line {
    fn new() -> Line {
        Line { buf: [0; 256], len: 0 }
    }

    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }
}

impl core::fmt::Write for Line {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let n = s.len().min(self.buf.len() - self.len);
        self.buf[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        Ok(())
    }
}
