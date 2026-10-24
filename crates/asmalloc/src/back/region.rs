//! One region's growth and purging.
//!
//! - **Carving** hands out never-used slots from the watermark. The watermark and the number of
//!   committed pages share one word (`Grow`), so a carve that finds enough memory committed
//!   advances the watermark with one compare-and-swap, and a slot is never handed out before its
//!   memory is writable. A carve that needs more memory commits ahead of the watermark first
//!   (and the leaf-bitmap words for the new slots), then publishes the new commit with a
//!   compare-and-swap, and tries again.
//! - **Huge pages.** A region starts advised against huge pages. Only classes of small, dense
//!   slots ever leave that (`policy::wants_huge`). Such a region commits small pages only up
//!   to `policy::huge_after` huge pages; the first carve that needs memory past them marks
//!   everything from there to the end of the region for huge pages, records that boundary in
//!   the region's `Hints`, and later commits there are whole huge pages.
//! - **Huge pages for grown blocks.** A large slot whose object moves there densely, growing past a size, gets
//!   its whole huge pages advised for huge pages, and its size word records that
//!   (`Recorded::huge`). Only the slot's holder reads or writes the flag. While it is set, a
//!   partial purge of the slot rounds inward to whole huge pages, since purging part of a huge
//!   page returns nothing to the OS until the kernel gets round to splitting it. The free, and
//!   the move out of a slot so advised, purge the whole slot and take the advice back, so no
//!   advised range stays on a free slot; a claim of a slot whose advice could not be taken back
//!   tries again (`Back::alloc_at`). The huge pages already faulted stay, and that tenant's flag
//!   is clear, so a shrink of it in place is not trimmed to whole huge pages and may purge part
//!   of one.
//!   That costs memory, not only time: a partly unmapped huge page stays charged to the cgroup
//!   whole until the kernel splits it, which it leaves to memory pressure
//!   (`crates/asmalloc-preload/tests/programs/thp_partial.c`: purging half of each huge page of
//!   64 MiB halved RSS, 67 to 35 MiB, but took the charge only to 56). It is bounded at under
//!   two huge pages a shrink, the huge pages at the shrink's two ends, and `mark_clean` marks
//!   those pages clean, so the retained count does not see them. A purge of a free slot may
//!   return it in pieces across steps (`purge::piece`); in a class of a huge page or more the
//!   pieces start at huge-page boundaries and are a huge page at least, so a slot kept with
//!   its advice is never left with a huge page partly returned.
//! - **Clean bits** say a page has not been written since it was carved or last purged. Only a
//!   purge and the first claim of a carved slot set them (`mark_fresh`, for the pages past its
//!   block); a claim and anything that widens a held block clear them under what it may write
//!   (`settle_slot`, `cover`). The retained count of a free large slot is its pages that are
//!   not clean (`charge`).
//! - **Settling** after a claim: an allocation tests and clears the slot's zero bit (large
//!   classes), and clears the clean bits of the pages its slots touch, and counts how much of
//!   what it took was resident.
//! - **Frontiers** say how far up a region's reuse reached in an interval of the clock and the
//!   one before (`Frontier`): a fill or allocation raises its region's after it claims
//!   (`reach`), and the clocked pass leaves what lies below (`floor`). The word names its
//!   interval, so nothing ages it. Hints only: no protocol's safety reads them.
//! - **Purging** one unit of pages is a claim, as `tla/Purge.tla` models it: claim the bits of
//!   every slot that touches the unit, word by word from the lowest, giving up and releasing if
//!   any is not free; return the pages; set the zero bits of slots wholly inside the unit and the
//!   clean bits of its pages; then release the bits. The zero and clean bits are written only
//!   while the purge holds the claim of every slot they describe.

use super::layout::{Grow, Hints, Layout, Recorded, Unit};
use super::policy::{
    COMMIT_AHEAD_HUGE_PAGES, COMMIT_AHEAD_MAX_HUGE_PAGES, COMMIT_AHEAD_PAGES, commit_ahead,
    grown_huge, huge_after, wants_huge,
};
use super::tree::{self, Host};
use super::word::{BITS, Word, bits_in_word};
use crate::os::HugePages;
use crate::space::RegionId;

/// What a carve got.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Carve {
    /// Slots `first..first + count`, never used before, whose memory reads as zero.
    Got { first: u32, count: u32 },
    /// The region has no uncarved slots.
    Exhausted,
    /// The OS refused to make more of the region writable.
    Failed,
}

/// Carves up to `n` slots, at least one, from a region's watermark. `n` must not be zero.
pub(crate) fn carve<H: Host>(host: &H, region: RegionId, n: u32) -> Carve {
    let layout = host.layout();
    let grow = host.grow(region);
    let mut seen = grow.load();
    loop {
        let now = Grow::unpack(seen);
        let slots = layout.slots();
        if now.carved >= slots {
            return Carve::Exhausted;
        }
        let count = n.min(slots - now.carved);
        let end = now.carved + count;
        let page = layout.page_size();
        let need = layout.slot_bytes(end).div_ceil(page) as u32;
        let next = if need > now.committed {
            let Some(committed) = commit_to(host, region, now, need) else {
                return Carve::Failed;
            };
            Grow { committed, ..now }
        } else {
            Grow { carved: end, ..now }
        };
        match grow.compare_exchange(seen, next.pack()) {
            Ok(_) if next.carved == end => {
                return Carve::Got {
                    first: now.carved,
                    count,
                };
            }
            Ok(_) => seen = next.pack(),
            Err(found) => seen = found,
        }
    }
}

/// For a thread about to take `region` for a large class: makes writable what the region's
/// first carve, of one slot, needs, as that carve would, and returns the pages now committed;
/// `None` if the OS refused. A large class takes a region for one slot at a time, and a region
/// is never given back, so the commit comes before the region is taken: a refusal then costs
/// nothing, and the region is still there for a smaller slot whose commit the OS allows.
pub(crate) fn commit_first<H: Host>(host: &H, region: RegionId) -> Option<u32> {
    let layout = host.layout();
    let need = layout.slot_bytes(1).div_ceil(layout.page_size()) as u32;
    commit_to(host, region, Grow::unpack(0), need)
}

/// Makes the region writable up to at least page `need`, and the leaf bitmap with it, running
/// ahead by as much as the region has committed (`policy::commit_ahead`). If the OS refuses
/// that much, as it may under strict overcommit or a limit on memory, it tries again with the
/// least run ahead. Returns the new number of committed pages, or `None` if the OS refused.
fn commit_to<H: Host>(host: &H, region: RegionId, now: Grow, need: u32) -> Option<u32> {
    let layout = host.layout();
    let huge = switch_to_huge(host, region, need);
    let used = now.committed.saturating_sub(layout.origin_page());
    let far = commit_target(layout, huge, need, used);
    let near = commit_target(layout, huge, need, 0);
    match commit_range(host, region, now, far) {
        None if near < far => commit_range(host, region, now, near),
        done => done,
    }
}

/// The page a commit for a carve that needs pages up to `need` makes the region writable up
/// to, in a region with `used` pages committed above its origin and its huge-page part, if it
/// has one, starting at page `huge`.
fn commit_target(layout: &Layout, huge: Option<u32>, need: u32, used: u32) -> u32 {
    let most = COMMIT_AHEAD_MAX_HUGE_PAGES as u32 * layout.huge_pages();
    let target = match huge {
        Some(from) if need > from => {
            let least = COMMIT_AHEAD_HUGE_PAGES as u32 * layout.huge_pages();
            layout.huge_ceil(need) + layout.huge_ceil(commit_ahead(used, least, most))
        }
        _ => need + commit_ahead(used, COMMIT_AHEAD_PAGES as u32, most),
    };
    // A small-page commit never runs into the huge-page part, whose pages are committed and
    // faulted whole, nor, before the region has one, past the threshold where it will start.
    let limit = huge.or_else(|| huge_threshold(layout));
    let target = match limit {
        Some(from) if need <= from => target.min(from),
        _ => target,
    };
    target.min(layout.region_pages())
}

/// Makes the region writable from what `now` says is committed up to page `target`, and the
/// leaf bitmap with it. Returns `target`, or `None` if the OS refused.
fn commit_range<H: Host>(host: &H, region: RegionId, now: Grow, target: u32) -> Option<u32> {
    let layout = host.layout();
    let page = layout.page_size();
    // Nothing below the class's origin holds a slot, so it is never made writable.
    let from = now.committed.max(layout.origin_page());
    let start = from as usize * page;
    let len = (target - from) as usize * page;
    if !host.commit(region, start, len) {
        return None;
    }
    let (from, to) = (
        layout.leaf_commit(now.committed),
        layout.leaf_commit(target),
    );
    if to > from && !host.commit_meta(region, from, to - from) {
        return None;
    }
    Some(target)
}

/// The page at which a region of this layout's class switches to huge pages
/// (`policy::huge_after`), a huge-page boundary; `None` if the class never gets them or the
/// region ends there.
fn huge_threshold(layout: &Layout) -> Option<u32> {
    if !wants_huge(layout.size(), layout.page_size()) {
        return None;
    }
    let region_huge_pages = layout.region_pages() / layout.huge_pages();
    let threshold = huge_after(region_huge_pages as usize) as u32 * layout.huge_pages();
    (threshold < layout.region_pages()).then_some(threshold)
}

/// The first page of the region's huge-page part, switching the region to huge pages first if
/// it has none yet and a carve needs memory up to page `need`, past the threshold.
///
/// Small-page commits stop at the threshold until then (`commit_to`), so the part starts at
/// the threshold itself. Were they allowed past it, the part would start at the next huge-page
/// boundary above the last of them, leaving up to one more huge page of the class on small
/// pages (`docs/research/memory.md`, "Build phases against mimalloc"). Nothing small-page
/// purges work on lies above the threshold either, since nothing was committed there.
fn switch_to_huge<H: Host>(host: &H, region: RegionId, need: u32) -> Option<u32> {
    let layout = host.layout();
    let hints = host.hints(region);
    let mut seen = hints.load();
    if let Some(from) = Hints::unpack(seen).huge_from {
        return Some(from);
    }
    let at = huge_threshold(layout)?;
    if need <= at {
        return None;
    }
    let pages = layout.region_pages();
    loop {
        let current = Hints::unpack(seen);
        if let Some(from) = current.huge_from {
            return Some(from);
        }
        let next = Hints {
            huge_from: Some(at),
            ..current
        };
        match hints.compare_exchange(seen, next.pack()) {
            Ok(_) => {
                let page = layout.page_size();
                let (offset, len) = (at as usize * page, (pages - at) as usize * page);
                host.advise(region, offset, len, HugePages::Prefer);
                return Some(at);
            }
            Err(found) => seen = found,
        }
    }
}

/// What an allocation learned about the slots it claimed from one leaf word.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Settled {
    /// The claimed slots, as bits of the leaf word, whose zero bit was set: free slots known to
    /// read as zero. Always empty for small classes.
    pub(crate) zeroed: u64,
    /// Bytes of the claimed slots that were resident, as far as the clean bits tell.
    pub(crate) resident: usize,
    /// Bytes of other free slots that the claim made resident again: the rest of the purged
    /// pages it touched. They were taken out of the retained count when the pages were purged
    /// and are resident once the claimed slots are written.
    pub(crate) revived: usize,
}

/// After claiming `bits` of leaf word `index`: tests and clears their zero bits, and clears the
/// clean bits of the pages they touch, as the claim's holder. Nothing else writes those bits
/// while the claim is held, so a load that finds them clear needs no write.
pub(crate) fn settle<H: Host>(host: &H, region: RegionId, index: u32, bits: u64) -> Settled {
    let layout = host.layout();
    let zeroed = match layout.zero_word(index) {
        Some(at) => {
            let zero = host.meta(region, at);
            if zero.load() & bits != 0 {
                zero.fetch_and(!bits) & bits
            } else {
                0
            }
        }
        None => 0,
    };
    // Clear the clean bits of the pages each run of consecutive claimed slots touches, and no
    // others: pages between runs may belong to slots another thread holds, a purge among them.
    let page = layout.page_size();
    let shift = layout.page_shift();
    let mut cleaned = 0usize;
    // Most claims find no clean page anywhere under the word's claimed slots: then one look at
    // the clean bits of the whole span, which reads and writes nothing it must not, does for
    // every run.
    let (first, last) = (
        index * BITS + bits.trailing_zeros(),
        index * BITS + (BITS - 1 - bits.leading_zeros()),
    );
    let (start, end) = (
        (layout.slot_bytes(first) >> shift) as u32,
        layout.slot_bytes(last + 1).div_ceil(page) as u32,
    );
    let any_clean = (start / BITS..=(end - 1) / BITS).any(|w| {
        host.meta(region, layout.clean_word(w)).load() & bits_in_word(w, start, end - 1) != 0
    });
    let mut rest = if any_clean { bits } else { 0 };
    while rest != 0 {
        let low = rest.trailing_zeros();
        let run = (!(rest >> low)).trailing_zeros();
        rest &= !super::word::span(low, low + run - 1);
        let (first, last) = (index * BITS + low, index * BITS + low + run - 1);
        let start = (layout.slot_bytes(first) >> shift) as u32;
        let end = layout.slot_bytes(last + 1).div_ceil(page) as u32;
        for w in start / BITS..=(end - 1) / BITS {
            let mask = bits_in_word(w, start, end - 1);
            let clean = host.meta(region, layout.clean_word(w));
            if clean.load() & mask != 0 {
                cleaned += (clean.fetch_and(!mask) & mask).count_ones() as usize;
            }
        }
    }
    let claimed = bits.count_ones() as usize * layout.size();
    Settled {
        zeroed,
        resident: claimed.saturating_sub(cleaned * page),
        revived: (cleaned * page).saturating_sub(claimed),
    }
}

/// Records that a fill or allocation in interval `epoch` has just claimed or carved slots of
/// `region` up to slot `last`: raises the region's frontier (`Frontier`) to the page past that
/// slot's end, if the word does not already say as much for this interval. One load and a
/// compare when it does, which is every claim of an interval but the region's first and the
/// few that reach a little higher; else a compare-and-swap, tried again against whatever a
/// racing claim wrote.
///
/// Only a thread that holds a claim in the region writes its word, so the word never needs the
/// region's class, and a region given back and reassigned keeps a word that only describes
/// claims. A record that lands in the interval after its claim's counts there instead, which
/// keeps the claim a little longer. A thread stopped between its claim and this record leaves
/// the claim uncounted until it runs again; meanwhile the slots it claimed are not free, so no
/// purge can take them, and only the free pages beside them are exposed a little early.
///
/// Returns whether the word recorded no claim of this interval when this record first read
/// it: the region's first claim of the interval, as far as the word can tell. Two claimers that
/// record at once at the start of an interval may both be told so.
#[inline]
pub(crate) fn reach<H: Host>(host: &H, region: RegionId, last: u32, epoch: u64) -> bool {
    let layout = host.layout();
    let page = layout.slot_bytes(last + 1).div_ceil(layout.page_size()) as u32;
    let format = layout.frontier_format();
    let word = host.frontier(region);
    let mut seen = word.load();
    let first = !format.claimed_in(seen, epoch);
    while let Some(next) = format.record(seen, epoch, page) {
        match word.compare_exchange(seen, next) {
            Ok(_) => break,
            Err(found) => seen = found,
        }
    }
    first
}

/// The page below which `region`'s reuse reached in interval `epoch` and the one before: the
/// clocked pass leaves units that start below it alone.
#[inline]
pub(crate) fn floor<H: Host>(host: &H, region: RegionId, epoch: u64) -> u32 {
    host.layout()
        .frontier_format()
        .floor(host.frontier(region).load(), epoch)
}

/// Brings a region's watermark up to the end of the huge page that holds it, when that could
/// let the page be purged: the region has a huge-page part, the watermark lies inside one of
/// its huge pages, that page is committed, and every slot of it below the watermark is free.
/// The first touch of a huge page makes all of it resident, so the part above the watermark is
/// resident too, and a purge may only take pages wholly below the watermark. So this carves
/// the slots up to the end of the page, as an allocation would, which no other thread can
/// then do, and releases them as free; the unit is then like any other and the walk's next
/// step can purge it. Returns the bytes released (free, and resident if the page was ever
/// touched) and the words of bitmap read or written.
///
/// Nothing is carved if the page starts below `floor`: a purge that may not take pages below
/// a frontier (`floor`) could not take this one, and the slots would only wait, free, for a
/// fill.
///
/// A racing carve only changes which slots this call gets; whatever it gets it owns and
/// releases. If the page is not purged after all, the slots are ordinary free slots.
pub(crate) fn top_up<H: Host>(host: &H, region: RegionId, floor: u32) -> (usize, usize) {
    let layout = host.layout();
    let Some(huge) = Hints::unpack(host.hints(region).load()).huge_from else {
        return (0, 0);
    };
    let now = Grow::unpack(host.grow(region).load());
    if now.carved == 0 || now.carved >= layout.slots() {
        return (0, 1);
    }
    let shift = layout.page_shift();
    let granule = layout.huge_pages();
    let mark = layout.slot_bytes(now.carved);
    let start = ((mark >> shift) as u32) & !(granule - 1);
    let end = start + granule;
    if start < huge || start < floor || end > now.committed || mark == (start as usize) << shift {
        return (0, 1);
    }
    // Every slot touching the page from its start up to the watermark must be free.
    let first = layout.slot_at((start as usize) << shift);
    // And one of them must begin in this page. If the only carved slot here is the one that
    // straddles in from the page below, the program never touched this page (a top-up of that
    // page carved the straddler), and carving it would walk the watermark up the region one
    // untouched huge page a pass.
    let begins_here = if layout.slot_bytes(first) < (start as usize) << shift {
        first + 1
    } else {
        first
    };
    if begins_here >= now.carved {
        return (0, 1);
    }
    let (low, high) = (first / BITS, (now.carved - 1) / BITS);
    for w in low..=high {
        let mask = bits_in_word(w, first, now.carved - 1);
        if tree::word(host, tree::Node::leaf(region, w)).load() & mask != mask {
            return (0, (w - low + 1) as usize);
        }
    }
    let mut words = (high - low + 1) as usize;
    let last = layout
        .slot_at(((end as usize) << shift) - 1)
        .min(layout.slots() - 1);
    let Carve::Got { first, count } = carve(host, region, last + 1 - now.carved) else {
        return (0, words);
    };
    let until = first + count - 1;
    for w in first / BITS..=until / BITS {
        tree::release(host, region, w, bits_in_word(w, first, until));
        words += 1;
    }
    (layout.bytes_of(count), words)
}

/// Returns the pages wholly inside slot `slot` to the OS, for a caller that holds the slot
/// (it is allocated and being freed), then marks them clean and the slot zero if they are all
/// of it. `true` if the pages went back; `false` if the slot has no whole page, lies in a
/// huge-page part (which is purged only in whole huge pages) or the OS refused.
///
/// # Safety
///
/// The caller holds the slot: its free bit is clear because an allocation claimed it for the
/// caller, who has not released it, and nothing reads or writes the slot's memory during the
/// call or needs its contents after. The slot is below the watermark.
pub(crate) unsafe fn purge_held<H: Host>(host: &H, region: RegionId, slot: u32) -> bool {
    let layout = host.layout();
    let Some(unit) = layout.slot_unit(slot) else {
        return false;
    };
    let hints = Hints::unpack(host.hints(region).load());
    if hints.huge_from.is_some_and(|from| unit.end > from) {
        return false;
    }
    let page = layout.page_size();
    let (offset, len) = (
        unit.start as usize * page,
        (unit.end - unit.start) as usize * page,
    );
    // SAFETY: BACK-REGION-02. the caller's promise, for pages that lie inside the slot.
    if !unsafe { host.purge(region, offset, len) } {
        return false;
    }
    mark_purged(host, region, slot);
    true
}

/// The page, counted from the region's start, past the last that a tenant of large slot `slot`
/// with `size` bytes may write: its size from the slot's stagger (a block starts at the slot's
/// start or there), rounded up to a page, or to a huge page if `huge`, since an advised range
/// faults a huge page at a time; within the slot's pages, `unit`.
pub(crate) const fn extent_of(
    layout: &Layout,
    slot: u32,
    unit: Unit,
    size: usize,
    huge: bool,
) -> u32 {
    let page = layout.page_size();
    let written = layout.slot_bytes(slot) + layout.stagger(slot) + size;
    let end = written.div_ceil(page);
    let end = if end > unit.end as usize {
        unit.end
    } else {
        end as u32
    };
    let end = if huge { layout.huge_ceil(end) } else { end };
    if end < unit.start {
        unit.start
    } else if end > unit.end {
        unit.end
    } else {
        end
    }
}

/// The clean words that pages `from..to` lie in, `from < to`, each with the bits of its pages
/// in the range.
#[inline(always)]
fn page_words(from: u32, to: u32) -> impl Iterator<Item = (u32, u64)> {
    let (first, last) = (from / BITS, (to - 1) / BITS);
    let head = u64::MAX << (from % BITS);
    let tail = u64::MAX >> (BITS - 1 - (to - 1) % BITS);
    (first..=last).map(move |w| {
        let mut mask = u64::MAX;
        if w == first {
            mask &= head;
        }
        if w == last {
            mask &= tail;
        }
        (w, mask)
    })
}

/// The bytes of large slot `slot` to count as retained when it is free: its pages whose clean
/// bit is clear. A clean bit is true: it says the page has not been written since it was
/// carved or last purged, so the pages that are not clean are what may be resident. The free,
/// the decision to keep a grown slot, and the claim (`settle_slot`, which counts the same way
/// before it clears anything) all use this count, and a purge subtracts the pages it newly
/// cleans, so the retained count is the free slots' pages that are not clean, exactly.
///
/// It reads the slot's clean words and writes nothing.
pub(crate) fn charge<H: Host>(host: &H, region: RegionId, slot: u32) -> usize {
    let layout = host.layout();
    let Some(unit) = layout.slot_unit(slot) else {
        return layout.size();
    };
    let mut clean = 0usize;
    for (w, mask) in page_words(unit.start, unit.end) {
        clean += count(host.meta(region, layout.clean_word(w)).load() & mask);
    }
    layout.size() - clean * layout.page_size()
}

/// The bits of clean word `w` for the pages below page `end`.
#[inline(always)]
const fn below(w: u32, end: u32) -> u64 {
    let base = w * BITS;
    if end <= base {
        0
    } else if end - base >= BITS {
        u64::MAX
    } else {
        u64::MAX >> (BITS - (end - base))
    }
}

/// The bits set in `bits`. The baseline instruction set the crate is built for has no
/// population count, and the clean pages of a slot are nearly always one run, from the end of
/// what its tenants wrote to its end: a run is counted from its ends, and only a word with
/// gaps is counted bit by bit.
#[inline(always)]
fn count(bits: u64) -> usize {
    if bits == 0 {
        return 0;
    }
    let run = bits >> bits.trailing_zeros();
    if run & run.wrapping_add(1) == 0 {
        (BITS - run.leading_zeros()) as usize
    } else {
        bits.count_ones() as usize
    }
}

/// For the thread that has just carved large slot `slot` for a block of `size` bytes: marks the
/// slot's pages past what the block may write (`extent_of`) clean. They have never been
/// written: a carved slot's pages are fresh. This is the one place a clean bit is set without
/// a purge, and a slot is carved once. A block that moved in with room to grow is then
/// charged, when it is freed, what it can have touched, not four or five times as much.
pub(crate) fn mark_fresh<H: Host>(host: &H, region: RegionId, slot: u32, size: usize) {
    let layout = host.layout();
    let Some(unit) = layout.slot_unit(slot) else {
        return;
    };
    let end = extent_of(layout, slot, unit, size, false);
    if end >= unit.end {
        return;
    }
    for (w, mask) in page_words(end, unit.end) {
        host.meta(region, layout.clean_word(w)).fetch_or(mask);
    }
}

/// For the holder of large slot `slot`, which has just returned the `len` bytes at `from` bytes
/// into it, whole pages, to the OS in full (`purge_part` said so): marks those pages clean.
pub(crate) fn mark_clean<H: Host>(host: &H, region: RegionId, slot: u32, from: usize, len: usize) {
    let layout = host.layout();
    let shift = layout.page_shift();
    let offset = layout.slot_bytes(slot) + from;
    let (first, end) = ((offset >> shift) as u32, ((offset + len) >> shift) as u32);
    if first >= end {
        return;
    }
    for (w, mask) in page_words(first, end) {
        host.meta(region, layout.clean_word(w)).fetch_or(mask);
    }
}

/// For the holder of large slot `slot`, after a change that may widen what its tenant can
/// write (`extent_of`): a larger recorded size, or huge-page advice, which rounds it up to
/// huge pages. Clears the clean bits of the pages inside the new extent, writing only words
/// that change, so that no clean bit stands over a page the tenant may write. With the claim,
/// which clears them up to the size it was claimed for (`settle_slot`), this keeps every clean
/// bit of a held slot true. Only a resize, a grown move or a claim of a slot that kept its
/// advice calls it.
pub(crate) fn cover<H: Host>(host: &H, region: RegionId, slot: u32) {
    let layout = host.layout();
    let (Some(unit), Some(now)) = (layout.slot_unit(slot), recorded(host, region, slot)) else {
        return;
    };
    let end = extent_of(layout, slot, unit, now.size, now.huge);
    if end <= unit.start {
        return;
    }
    for (w, inside) in page_words(unit.start, end) {
        let word = host.meta(region, layout.clean_word(w));
        if word.load() & inside != 0 {
            word.fetch_and(!inside);
        }
    }
}

/// `settle` for one large slot, just claimed for a block of `size` bytes: tests and clears its
/// zero bit, and clears the clean bits of the pages the block may write (`extent_of`),
/// writing only words that change; the pages past them stay clean, which is true as long as
/// nothing writes them. Its `resident` is the slot's pages that were not clean, counted as
/// `charge` counts them, which is what the free or purge that left it counted; a claim of one
/// large slot revives no other slot's pages, since a large slot is whole pages.
pub(crate) fn settle_slot<H: Host>(host: &H, region: RegionId, slot: u32, size: usize) -> Settled {
    let layout = host.layout();
    let bit = 1u64 << (slot % BITS);
    let zeroed = match layout.zero_word(slot / BITS) {
        Some(at) => {
            let zero = host.meta(region, at);
            if zero.load() & bit != 0 {
                zero.fetch_and(!bit) & bit
            } else {
                0
            }
        }
        None => 0,
    };
    let Some(unit) = layout.slot_unit(slot) else {
        return Settled {
            zeroed,
            resident: layout.size(),
            revived: 0,
        };
    };
    // One pass over the slot's clean words: count the clean pages, as `charge` does, from what
    // each word held before this claim cleared any of it, and clear those under the block.
    let end = extent_of(layout, slot, unit, size, false);
    let mut clean = 0usize;
    for (w, mask) in page_words(unit.start, unit.end) {
        let word = host.meta(region, layout.clean_word(w));
        let seen = word.load() & mask;
        clean += count(seen);
        let inside = mask & below(w, end);
        if seen & inside != 0 {
            word.fetch_and(!inside);
        }
    }
    Settled {
        zeroed,
        resident: layout.size() - clean * layout.page_size(),
        revived: 0,
    }
}

/// Records that every whole page inside slot `slot` has just been returned to the OS, for the
/// thread that holds the slot and returned them: sets the pages' clean bits and, if they are
/// all of the slot, its zero bit. Those bits are written only by the holder of every slot
/// they describe: the zero bit is this slot's, and the pages are inside this slot.
pub(crate) fn mark_purged<H: Host>(host: &H, region: RegionId, slot: u32) {
    let layout = host.layout();
    let Some(unit) = layout.slot_unit(slot) else {
        return;
    };
    let page = layout.page_size();
    let (offset, len) = (
        unit.start as usize * page,
        (unit.end - unit.start) as usize * page,
    );
    if offset == layout.slot_bytes(slot)
        && offset + len == layout.slot_bytes(slot + 1)
        && let Some(at) = layout.zero_word(slot / BITS)
    {
        host.meta(region, at).fetch_or(1u64 << (slot % BITS));
    }
    for w in unit.start / BITS..=(unit.end - 1) / BITS {
        let mask = bits_in_word(w, unit.start, unit.end - 1);
        host.meta(region, layout.clean_word(w)).fetch_or(mask);
    }
}

/// What large slot `slot`'s size word holds, if the class records sizes. Only meaningful to
/// the slot's holder.
pub(crate) fn recorded<H: Host>(host: &H, region: RegionId, slot: u32) -> Option<Recorded> {
    let at = host.layout().size_word(slot)?;
    Some(Recorded::unpack(host.meta(region, at).load()))
}

/// Writes large slot `slot`'s size word, for the slot's holder. Does nothing for a class that
/// records no sizes.
pub(crate) fn record<H: Host>(host: &H, region: RegionId, slot: u32, recorded: Recorded) {
    if let Some(at) = host.layout().size_word(slot) {
        host.meta(region, at).store(recorded.pack());
    }
}

/// For the holder of large slot `slot`, whose object starts `into` bytes into it and is
/// growing from `old` to `new` bytes, by
/// moving there or in place: if the growth looks like appending (`policy::grown_huge`) and
/// `dense` says the bytes the program wrote last hold data in most of their pages, advises
/// the slot's whole huge pages that the new size covers for huge pages.
/// Never the room past it, so the advice runs at most one growth step ahead of what the
/// program has shown it writes densely. `dense` is asked last, because it reads memory.
///
/// The flag is recorded before the advice, because a failed advice may have acted on part of
/// the range, and the flag is what makes the free take it back. Advice changes no contents,
/// and the range is inside the holder's slot, so nothing else is affected.
///
/// Returns the page, counted from the region's start, at which the advised range ends, if it
/// gave advice and the OS took it.
pub(crate) fn advise_grown<H: Host>(
    host: &H,
    region: RegionId,
    slot: u32,
    into: usize,
    old: usize,
    new: usize,
    dense: impl FnOnce() -> bool,
) -> Option<u32> {
    let layout = host.layout();
    let now = recorded(host, region, slot)?;
    let page = layout.page_size();
    let huge = page * layout.huge_pages() as usize;
    if !grown_huge(old, new, huge) {
        return None;
    }
    let unit = layout.huge_interior(slot)?;
    // Rounded down: the last huge page, which the block only starts, faults in small pages,
    // so a block that ends just past a huge page does not make all of the next one resident.
    let wanted =
        ((layout.slot_bytes(slot) + into + new) / page) as u32 & !(layout.huge_pages() - 1);
    let end = wanted.min(unit.end);
    if end <= unit.start || !dense() {
        return None;
    }
    record(host, region, slot, Recorded { huge: true, ..now });
    // From the start, though part may be advised already: advice on a range that has it is
    // harmless, and the kernel merges the mapping back into one.
    let (offset, len) = (
        unit.start as usize * page,
        (end - unit.start) as usize * page,
    );
    host.advise(region, offset, len, HugePages::Prefer)
        .then_some(end)
}

/// For the holder of large slot `slot`: takes back the huge-page advice `advise_grown` gave
/// it, if any, and clears the flag once the OS has accepted. A refusal keeps the flag, so the
/// next holder tries again (`Back::alloc`).
pub(crate) fn unadvise<H: Host>(host: &H, region: RegionId, slot: u32) {
    let layout = host.layout();
    let Some(now) = recorded(host, region, slot).filter(|r| r.huge) else {
        return;
    };
    let undone = match layout.huge_interior(slot) {
        Some(unit) => {
            let page = layout.page_size();
            let (offset, len) = (
                unit.start as usize * page,
                (unit.end - unit.start) as usize * page,
            );
            host.advise(region, offset, len, HugePages::Avoid)
        }
        None => true,
    };
    if undone {
        record(host, region, slot, Recorded { huge: false, ..now });
    }
}

/// Returns `len` bytes at `from` bytes into slot `slot` to the OS, for a caller that holds
/// the slot and no longer needs them: the tail of an object that shrank in place, or the part
/// of a moving object already copied. Both are page multiples and the range lies inside the
/// slot. It writes no bitmap: the slot stays allocated. A caller told that the whole range
/// went back may mark its pages clean (`mark_clean`), which is then true of them; the zero
/// bit describes a whole free slot, and only `mark_purged` sets it for a held one. In a slot
/// advised for huge pages the range is first trimmed to whole huge pages where it ends inside
/// one. `true` only if the whole range went back:
/// `false` if trimming left part of it, or all, resident, in a huge-page part, which is purged
/// only in whole huge pages (nothing is done), or if the OS refused. A caller that marks the
/// slot zero afterwards relies on that.
///
/// # Safety
///
/// The caller holds the slot (its free bit is clear on the caller's behalf), nothing reads or
/// writes the range during the call, and nothing in it is needed after.
pub(crate) unsafe fn purge_part<H: Host>(
    host: &H,
    region: RegionId,
    slot: u32,
    from: usize,
    len: usize,
) -> bool {
    let layout = host.layout();
    let shift = layout.page_shift();
    let offset = layout.slot_bytes(slot) + from;
    let (mut first, mut end) = ((offset >> shift) as u32, ((offset + len) >> shift) as u32);
    let hints = Hints::unpack(host.hints(region).load());
    if from + len > layout.size() || hints.huge_from.is_some_and(|huge| end > huge) {
        return false;
    }
    let whole = (first, end);
    if recorded(host, region, slot).is_some_and(|r| r.huge)
        && let Some(unit) = layout.huge_interior(slot)
    {
        if first > unit.start && first < unit.end {
            first = layout.huge_ceil(first);
        }
        if end > unit.start && end < unit.end {
            end &= !(layout.huge_pages() - 1);
        }
    }
    if first >= end {
        return false;
    }
    let page = layout.page_size();
    // SAFETY: BACK-REGION-03. the caller's promise, for a range inside the slot: trimming only shrank it.
    let purged =
        unsafe { host.purge(region, first as usize * page, (end - first) as usize * page) };
    purged && (first, end) == whole
}

/// What purging one unit did.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Purge {
    /// Every page of the unit was already purged and untouched since.
    Clean,
    /// Some slot touching the unit is not free.
    Busy,
    /// The OS refused; nothing changed.
    Failed,
    /// The unit's pages went back to the OS: how many bytes of them had been resident, and how
    /// many of those were in free slots kept by the moves their blocks outgrew them in
    /// (`Recorded::outgrown`), which leave the count of such slots.
    Done { returned: usize, outgrown: usize },
}

/// Purges a unit of a region whose pages lie wholly below the watermark, if every slot touching
/// it is free. Returns what happened and how many leaf words it read.
pub(crate) fn purge_unit<H: Host>(host: &H, region: RegionId, unit: Unit) -> (Purge, usize) {
    let layout = host.layout();
    if all_clean(host, region, unit) {
        return (Purge::Clean, 1);
    }
    let (first, last) = layout.touching(unit);
    let (low, high) = (first / BITS, last / BITS);
    // A look before claiming. Reads change nothing, and this keeps a purge from claiming and
    // releasing words under an allocator that is about to want them.
    for w in low..=high {
        let mask = bits_in_word(w, first, last);
        if tree::word(host, tree::Node::leaf(region, w)).load() & mask != mask {
            return (Purge::Busy, (w - low + 1) as usize);
        }
    }
    let read = (high - low + 1) as usize;
    for w in low..=high {
        if !tree::claim_exact(host, region, w, bits_in_word(w, first, last)) {
            release_range(host, region, first, last, low, w);
            return (Purge::Busy, read);
        }
    }
    let outgrown = outgrown_dirty(host, region, unit, first, last);
    let page = layout.page_size();
    let (offset, len) = (
        unit.start as usize * page,
        (unit.end - unit.start) as usize * page,
    );
    // SAFETY: BACK-REGION-01 in docs/safety/asmalloc/back/region-safety.md. This thread holds
    // the claim of every slot that touches the range, and the range lies below the watermark.
    if !unsafe { host.purge(region, offset, len) } {
        release_range(host, region, first, last, low, high + 1);
        return (Purge::Failed, read);
    }
    let (inside, end) = layout.inside(unit);
    if inside < end {
        for w in inside / BITS..=(end - 1) / BITS {
            if let Some(at) = layout.zero_word(w) {
                host.meta(region, at)
                    .fetch_or(bits_in_word(w, inside, end - 1));
            }
        }
    }
    let mut cleaned = 0usize;
    for w in unit.start / BITS..=(unit.end - 1) / BITS {
        let mask = bits_in_word(w, unit.start, unit.end - 1);
        let old = host.meta(region, layout.clean_word(w)).fetch_or(mask);
        cleaned += (mask & !old).count_ones() as usize;
    }
    // A unit that covers only part of a slot (a piece of a large slot, which the purge walk
    // returns across steps) leaves the slot's zero bit as it was, unless this piece leaves
    // every page of the slot clean: a free slot of whole pages, all of them clean, reads as
    // zero. Pages the claim of an earlier tenant wrote had their clean bits cleared then, so
    // a slot taken and freed between pieces is not taken for zero. The bit is written under
    // the claim this thread holds.
    for slot in [first, last] {
        if (inside..end).contains(&slot) {
            continue;
        }
        if let (Some(at), Some(whole)) = (layout.zero_word(slot / BITS), layout.slot_unit(slot))
            && whole.start as usize * page == layout.slot_bytes(slot)
            && whole.end as usize * page == layout.slot_bytes(slot + 1)
            && all_clean(host, region, whole)
        {
            host.meta(region, at).fetch_or(1u64 << (slot % BITS));
        }
    }
    release_range(host, region, first, last, low, high + 1);
    (
        Purge::Done {
            returned: cleaned * page,
            outgrown,
        },
        read,
    )
}

/// For a purge that holds the claim of slots `first..=last` touching `unit`: the bytes of the
/// pages of `unit` that lie inside a kept outgrown slot (`Recorded::outgrown`) and are not
/// clean. The purge cleans every page of the unit, so this is what it takes off that slot's
/// charge (`charge`): a slot returned in pieces subtracts each piece's own pages as it goes, and
/// the claim that takes it again subtracts the rest, so the sum is the charge the keep added.
/// Nothing else changes a free slot's clean bits while the claim is held. Only a large class
/// records sizes, and a unit of one touches a few slots.
fn outgrown_dirty<H: Host>(host: &H, region: RegionId, unit: Unit, first: u32, last: u32) -> usize {
    let layout = host.layout();
    if layout.size_word(first).is_none() {
        return 0;
    }
    let mut bytes = 0;
    for slot in first..=last {
        if !recorded(host, region, slot).is_some_and(|r| r.outgrown) {
            continue;
        }
        let Some(whole) = layout.slot_unit(slot) else {
            continue;
        };
        let (start, end) = (whole.start.max(unit.start), whole.end.min(unit.end));
        if start < end {
            bytes += dirty_pages(host, region, Unit { start, end }).0 * layout.page_size();
        }
    }
    bytes
}

/// Whether a unit could be purged now and would return something: not every page is already
/// clean, and every slot touching it is free. A look that claims nothing, as `purge_unit`
/// makes before claiming; returns it and the leaf words read.
pub(crate) fn purgeable_unit<H: Host>(host: &H, region: RegionId, unit: Unit) -> (bool, usize) {
    if all_clean(host, region, unit) {
        return (false, 1);
    }
    let (first, last) = host.layout().touching(unit);
    let (low, high) = (first / BITS, last / BITS);
    for w in low..=high {
        let mask = bits_in_word(w, first, last);
        if tree::word(host, tree::Node::leaf(region, w)).load() & mask != mask {
            return (false, (w - low + 1) as usize);
        }
    }
    (true, (high - low + 1) as usize)
}

/// Where the clean top of a unit begins: the lowest page `p` of the unit such that pages
/// `p..unit.end` are all marked clean, `unit.end` if its last page is not. Returns it and the
/// clean words read. A piece of a large slot is cut below it, so that what an earlier piece
/// returned is not walked again (`purge::piece`).
pub(crate) fn clean_top<H: Host>(host: &H, region: RegionId, unit: Unit) -> (u32, usize) {
    let layout = host.layout();
    let (mut end, mut read) = (unit.end, 0);
    while end > unit.start {
        let w = (end - 1) / BITS;
        let low = (w * BITS).max(unit.start);
        let mask = bits_in_word(w, low, end - 1);
        let dirty = !host.meta(region, layout.clean_word(w)).load() & mask;
        read += 1;
        if dirty != 0 {
            return (w * BITS + (BITS - 1 - dirty.leading_zeros()) + 1, read);
        }
        end = low;
    }
    (unit.start, read)
}

/// The pages of a unit not marked clean, which may be resident, and the clean words read:
/// what a purge of the unit, if it is free, would return. Reads only.
pub(crate) fn dirty_pages<H: Host>(host: &H, region: RegionId, unit: Unit) -> (usize, usize) {
    let layout = host.layout();
    let (mut dirty, mut read) = (0, 0);
    for w in unit.start / BITS..=(unit.end - 1) / BITS {
        let mask = bits_in_word(w, unit.start, unit.end - 1);
        dirty += (!host.meta(region, layout.clean_word(w)).load() & mask).count_ones() as usize;
        read += 1;
    }
    (dirty, read)
}

/// Whether every page of a unit is marked clean.
fn all_clean<H: Host>(host: &H, region: RegionId, unit: Unit) -> bool {
    let layout = host.layout();
    (unit.start / BITS..=(unit.end - 1) / BITS).all(|w| {
        let mask = bits_in_word(w, unit.start, unit.end - 1);
        host.meta(region, layout.clean_word(w)).load() & mask == mask
    })
}

/// Releases the bits of slots `first..=last` in leaf words `from..to`, which this thread
/// claimed.
fn release_range<H: Host>(host: &H, region: RegionId, first: u32, last: u32, from: u32, to: u32) {
    for w in from..to {
        tree::release(host, region, w, bits_in_word(w, first, last));
    }
}
