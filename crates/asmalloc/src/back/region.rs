//! One region's growth and purging.
//!
//! - **Carving** hands out never-used slots from the watermark. The watermark and the number of
//!   committed pages share one word (`Grow`), so a carve that finds enough memory committed
//!   advances the watermark with one compare-and-swap, and a slot is never handed out before its
//!   memory is writable. A carve that needs more memory commits ahead of the watermark first
//!   (and the leaf-bitmap words for the new slots), then publishes the new commit with a
//!   compare-and-swap, and tries again.
//! - **Huge pages.** A region starts advised against huge pages. Only classes of small, dense
//!   slots ever leave that (`policy::wants_huge`). Once such a region has carved more than
//!   `policy::huge_after` huge pages, the carve that notices marks everything from
//!   the next huge-page boundary above the committed memory to the end of the region for huge
//!   pages, records that boundary in the region's `Hints`, and later commits there are whole
//!   huge pages.
//! - **Settling** after a claim: an allocation tests and clears the slot's zero bit (large
//!   classes), and clears the clean bits of the pages its slots touch, and counts how much of
//!   what it took was resident.
//! - **Purging** one unit of pages is a claim, as `tla/Purge.tla` models it: claim the bits of
//!   every slot that touches the unit, word by word from the lowest, giving up and releasing if
//!   any is not free; return the pages; set the zero bits of slots wholly inside the unit and the
//!   clean bits of its pages; then release the bits. The zero and clean bits are written only
//!   while the purge holds the claim of every slot they describe.

use super::layout::{Grow, Hints, Unit};
use super::policy::{COMMIT_AHEAD_HUGE_PAGES, COMMIT_AHEAD_PAGES, huge_after, wants_huge};
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
    let grow = host.meta(region, layout.grow_word());
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

/// Makes the region writable up to at least page `need`, and the leaf bitmap with it. Returns
/// the new number of committed pages, or `None` if the OS refused.
fn commit_to<H: Host>(host: &H, region: RegionId, now: Grow, need: u32) -> Option<u32> {
    let layout = host.layout();
    let pages = layout.region_pages();
    let huge = switch_to_huge(host, region, now);
    let target = match huge {
        Some(from) if need > from => {
            layout.huge_ceil(need) + COMMIT_AHEAD_HUGE_PAGES as u32 * layout.huge_pages()
        }
        _ => need + COMMIT_AHEAD_PAGES as u32,
    };
    let target = target.min(pages);
    let page = layout.page_size();
    let start = now.committed as usize * page;
    let len = (target - now.committed) as usize * page;
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

/// The first page of the region's huge-page part, switching the region to huge pages first if
/// it has carved past the threshold and has no huge-page part yet.
fn switch_to_huge<H: Host>(host: &H, region: RegionId, now: Grow) -> Option<u32> {
    let layout = host.layout();
    let hints = host.meta(region, layout.hints_word());
    let mut seen = hints.load();
    if let Some(from) = Hints::unpack(seen).huge_from {
        return Some(from);
    }
    if !wants_huge(layout.size(), layout.page_size()) {
        return None;
    }
    let region_huge_pages = (layout.region_pages() / layout.huge_pages()) as usize;
    let threshold = huge_after(region_huge_pages) * layout.huge_pages() as usize;
    if (layout.slot_bytes(now.carved) >> layout.page_shift()) <= threshold {
        return None;
    }
    let pages = layout.region_pages();
    // From the latest commit, not the caller's possibly stale one, so the huge-page part starts
    // above the pages small-page purges have worked on. A commit can still land between this
    // load and the compare-and-swap below, and a purge that read no huge-page part can still
    // return a small page above `at`. That costs at most a split huge page; it is advice, and
    // safety never depends on it.
    let latest = Grow::unpack(host.meta(region, layout.grow_word()).load()).committed;
    let at = layout.huge_ceil(latest.max(now.committed));
    if at >= pages {
        return None;
    }
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
/// A racing carve only changes which slots this call gets; whatever it gets it owns and
/// releases. If the page is not purged after all, the slots are ordinary free slots.
pub(crate) fn top_up<H: Host>(host: &H, region: RegionId) -> (usize, usize) {
    let layout = host.layout();
    let Some(huge) = Hints::unpack(host.meta(region, layout.hints_word()).load()).huge_from else {
        return (0, 0);
    };
    let now = Grow::unpack(host.meta(region, layout.grow_word()).load());
    if now.carved == 0 || now.carved >= layout.slots() {
        return (0, 1);
    }
    let shift = layout.page_shift();
    let granule = layout.huge_pages();
    let mark = layout.slot_bytes(now.carved);
    let start = ((mark >> shift) as u32) & !(granule - 1);
    let end = start + granule;
    if start < huge || end > now.committed || mark == (start as usize) << shift {
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
    (layout.slot_bytes(count), words)
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
    let hints = Hints::unpack(host.meta(region, layout.hints_word()).load());
    if hints.huge_from.is_some_and(|from| unit.end > from) {
        return false;
    }
    let page = layout.page_size();
    let (offset, len) = (
        unit.start as usize * page,
        (unit.end - unit.start) as usize * page,
    );
    // SAFETY: the caller's promise, for pages that lie inside the slot.
    if !unsafe { host.purge(region, offset, len) } {
        return false;
    }
    mark_purged(host, region, slot);
    true
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

/// Returns `len` bytes at `from` bytes into slot `slot` to the OS, for a caller that holds
/// the slot and no longer needs them: the tail of an object that shrank in place, or the part
/// of a moving object already copied. Both are page multiples and the range lies inside the
/// slot. No bitmap changes: the slot stays allocated, and its clean and zero bits describe it
/// only while it is free. `false`, and nothing done, in a huge-page part, which is purged only
/// in whole huge pages, or if the OS refused.
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
    let offset = layout.slot_bytes(slot) + from;
    let end = ((offset + len) >> layout.page_shift()) as u32;
    let hints = Hints::unpack(host.meta(region, layout.hints_word()).load());
    if from + len > layout.size() || hints.huge_from.is_some_and(|huge| end > huge) {
        return false;
    }
    // SAFETY: the caller's promise, for a range inside the slot.
    unsafe { host.purge(region, offset, len) }
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
    /// The unit's pages went back to the OS; this many bytes of them had been resident.
    Done(usize),
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
    release_range(host, region, first, last, low, high + 1);
    (Purge::Done(cleaned * page), read)
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
