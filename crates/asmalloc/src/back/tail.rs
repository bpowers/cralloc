//! The tail of a region's huge-page part: where the advice for huge pages has come off again
//! because the purge returned part of its huge pages (`docs/research/huge-tail.md`).
//!
//! In a region's huge-page part the purge returned only whole huge pages, so a class whose
//! demand fell kept every huge page that still held one object: fills take the lowest free
//! slots, its short-lived objects packed into the bottom of the region, and the top was left a
//! few long-lived objects to a huge page. The rule here: a step of the clocked pass that may
//! return memory treats the huge pages above the region's aged frontier as small pages (the
//! walk's side is in `purge`), returns their free pages after asking the kernel to split them,
//! and then takes the advice off over the huge pages it returned part of, so that khugepaged
//! does not rebuild them. When claims reach above that boundary again, the background thread
//! (or a heartbeat where there is none) puts the advice back up to the frontier and
//! [`TAIL_HEADROOM_HUGE_PAGES`] beyond it. Below the frontier nothing changes.
//!
//! **The word.** Each region has a [`Tail`] word, beside its frontier (`host::Tails`). It
//! holds the boundary, in huge pages from the region's start (zero: none, the advice is as the
//! huge-page part was given it); the lowest huge page the walk has returned part of since the
//! boundary last came down, which the next lowering takes the boundary to; a claim's mark that
//! the boundary should go up; and an owner bit with the counted interval it was taken in. All
//! zero is a region whose advice never came off.
//!
//! **Who writes it.** A walker returning part of a huge page lowers the noted huge page with a
//! compare-and-swap. A claim sets the mark. Changing the advice needs the owner bit: one
//! compare-and-swap takes it from a word without it, and a thread that finds it taken skips
//! the change (the noted huge page or the mark stays for the next look) rather than wait. The
//! owner makes the one call, then writes the new boundary and gives the bit back with one
//! compare-and-swap. So no two advice changes of a region overlap, and the boundary the word
//! names is the advice the kernel last took.
//!
//! **Repair.** An owner that never gives the bit back (a thread a fork left behind) would stop
//! the region's advice for good. A bit taken two or more counted intervals ago is taken over,
//! and the new owner first makes the kernel's advice agree with the word: advice on from the
//! start of the huge-page part to the boundary, off above it (`reconcile`). An old owner that
//! comes back after being taken over finds its compare-and-swap refused and sets the repair
//! bit, so the advice its late call left is put right by the next owner. A call the kernel
//! refused may have acted on part of its range, so it sets the repair bit too. Either way the
//! next pass that changes the region's advice reconciles it first, and nothing waits or loops:
//! a refused reconciliation leaves the bit for the pass after.
//!
//! The protocol is `tla/Tail.tla`.

use super::layout::Unit;
use super::policy::{TAIL_HEADROOM_HUGE_PAGES, TAIL_SAMPLE_PAGES, TAIL_SPARSE_PAGES};
use super::region;
use super::tree::{self, Host};
use super::word::{BITS, Word, bits_in_word};
use crate::os::HugePages;
use crate::space::RegionId;

/// A region's tail word, unpacked. All zero: no boundary, nothing noted, no mark, no owner.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) struct Tail {
    /// The first huge page of the region, counted from its start, above which the advice is
    /// off; zero for none.
    pub(crate) from: u32,
    /// The lowest huge page the walk returned part of since the boundary last came down, if
    /// any: where the next lowering takes it.
    pub(crate) low: Option<u32>,
    /// A claim reached above the boundary: the advice should go back on.
    pub(crate) want: bool,
    /// A thread owns the advice of the region and is changing it.
    pub(crate) busy: bool,
    /// The kernel's advice may differ from the word: reconcile before the next change.
    pub(crate) repair: bool,
    /// The low bits of the counted interval in which the owner took the bit.
    pub(crate) stamp: u32,
}

/// Bits of a huge-page index in the word: a region of 2^20 huge pages, far more than a 4 GiB
/// region of 2 MiB huge pages has.
const UNIT_BITS: u32 = 20;
const UNIT_MASK: u64 = (1 << UNIT_BITS) - 1;
const LOW_SHIFT: u32 = UNIT_BITS;
pub(crate) const WANT: u64 = 1 << (2 * UNIT_BITS);
const BUSY: u64 = 1 << (2 * UNIT_BITS + 1);
pub(crate) const REPAIR: u64 = 1 << (2 * UNIT_BITS + 2);
const STAMP_SHIFT: u32 = 2 * UNIT_BITS + 3;
const STAMP_MASK: u64 = (1 << (64 - STAMP_SHIFT)) - 1;

/// An owner bit this many counted intervals old is taken over: its thread is gone. An advice
/// call takes microseconds to milliseconds, and an interval is about a second while the
/// program runs, so a live owner is never this late but by a preemption of seconds, which the
/// repair bit then covers.
const STALE_INTERVALS: u64 = 2;

impl Tail {
    /// The word for this state. Every field fits: huge-page indices are below 2^20, since no
    /// region the heap builds has more huge pages (checked below).
    pub(crate) const fn pack(self) -> u64 {
        let low = match self.low {
            Some(unit) => unit as u64 + 1,
            None => 0,
        };
        (self.from as u64 & UNIT_MASK)
            | (low & UNIT_MASK) << LOW_SHIFT
            | if self.want { WANT } else { 0 }
            | if self.busy { BUSY } else { 0 }
            | if self.repair { REPAIR } else { 0 }
            | (self.stamp as u64 & STAMP_MASK) << STAMP_SHIFT
    }

    pub(crate) const fn unpack(word: u64) -> Tail {
        let low = (word >> LOW_SHIFT) & UNIT_MASK;
        Tail {
            from: (word & UNIT_MASK) as u32,
            low: if low == 0 { None } else { Some(low as u32 - 1) },
            want: word & WANT != 0,
            busy: word & BUSY != 0,
            repair: word & REPAIR != 0,
            stamp: ((word >> STAMP_SHIFT) & STAMP_MASK) as u32,
        }
    }

    /// Whether the owner bit is held and was taken less than [`STALE_INTERVALS`] ago.
    const fn owned(self, epoch: u64) -> bool {
        self.busy
            && (epoch & STAMP_MASK).wrapping_sub(self.stamp as u64) & STAMP_MASK < STALE_INTERVALS
    }
}

// The largest regions the heap builds, in the largest huge pages' fewest: Linux's.
const _: () = {
    let g = crate::space::Geometry::DESIGN;
    let pages = (1u64 << g.max_region_shift()) / g.page_size() as u64;
    let huge = (g.huge_page_size() / g.page_size()) as u64;
    assert!(pages / huge <= UNIT_MASK);
};

/// For a claim in `region` that reached page `page`: marks the region if the page lies above
/// its boundary and no mark is there yet, and says whether this call set it. One load when the
/// region has no boundary or the claim is below it, which is every claim but the few that
/// climb into a tail.
#[inline]
pub(crate) fn mark<H: Host>(host: &H, region: RegionId, page: u32) -> bool {
    let word = host.tail(region);
    let mut seen = word.load();
    loop {
        let tail = Tail::unpack(seen);
        if tail.from == 0 || tail.want || page <= tail.from * host.layout().huge_pages() {
            return false;
        }
        match word.compare_exchange(seen, seen | WANT) {
            Ok(_) => return true,
            Err(found) => seen = found,
        }
    }
}

/// Notes that the walk returned part of huge page `unit` of `region`: the next lowering takes
/// the boundary down to it if it is the lowest noted.
pub(crate) fn note<H: Host>(host: &H, region: RegionId, unit: u32) {
    // A boundary of zero is no boundary. The region's first huge page needs none: a small
    // class's slots start a few pages in (its origin), so that huge page is never mapped
    // whole, and nothing there can be rebuilt by khugepaged.
    if unit == 0 {
        return;
    }
    let word = host.tail(region);
    let mut seen = word.load();
    loop {
        let tail = Tail::unpack(seen);
        if tail.low.is_some_and(|low| low <= unit) || (tail.from != 0 && unit >= tail.from) {
            return;
        }
        let next = Tail {
            low: Some(unit),
            ..tail
        };
        match word.compare_exchange(seen, next.pack()) {
            Ok(_) => return,
            Err(found) => seen = found,
        }
    }
}

/// What one look at a region's tail word did.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Change {
    /// Nothing to do, or another thread owns the region's advice.
    None,
    /// The advice changed as asked, and the word says so.
    Done,
    /// The OS refused a call; the word keeps its boundary and asks for a repair.
    Refused,
}

/// Takes the owner bit of `region`'s word from `seen`, at counted interval `epoch`: `None` if
/// another thread owns it, or the word changed. An owner bit that has gone stale is taken over.
/// The word's repair bit is cleared as the bit is taken, and what this returns has `repair` set
/// if the new owner must reconcile first (the word asked for it, or an owner was taken over):
/// so a repair bit found in the word when the owner gives the bit back was set during its
/// ownership, by an owner it took over whose late call may have undone its own, and is kept
/// (`tla/Tail.tla` found that the bit was lost otherwise).
fn take<W: Word>(word: &W, seen: u64, epoch: u64) -> Option<(u64, Tail)> {
    let tail = Tail::unpack(seen);
    if tail.owned(epoch) {
        return None;
    }
    let mine = Tail {
        busy: true,
        repair: false,
        stamp: (epoch & STAMP_MASK) as u32,
        ..tail
    };
    let packed = mine.pack();
    word.compare_exchange(seen, packed).ok()?;
    Some((
        packed,
        Tail {
            repair: tail.repair || tail.busy,
            ..mine
        },
    ))
}

/// Gives the owner bit back, writing `next` (with `busy` clear) over the word this thread
/// wrote when it took the bit, `mine`. If the word is not that any more, another thread took
/// the bit over: this thread's call may have landed after that thread's reconciliation, so it
/// asks for another. Fields a claim or walker changed meanwhile (the mark, the noted huge
/// page) are kept.
fn give_back<W: Word>(word: &W, mine: u64, next: Tail) {
    let taken = Tail::unpack(mine);
    let mut seen = mine;
    loop {
        let now = Tail::unpack(seen);
        if !now.busy || now.stamp != taken.stamp {
            word.fetch_or(REPAIR);
            return;
        }
        // A mark set, or a huge page noted, since the bit was taken is kept; what the owner
        // saw when it took the bit is what `next` already answers.
        let low = if now.low == taken.low {
            next.low
        } else {
            match (next.low, now.low) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            }
        };
        let merged = Tail {
            busy: false,
            repair: next.repair || now.repair,
            want: next.want || (now.want && !taken.want),
            low: low.filter(|&low| next.from == 0 || low < next.from),
            ..next
        };
        match word.compare_exchange(seen, merged.pack()) {
            Ok(_) => return,
            Err(found) => seen = found,
        }
    }
}

/// Puts the kernel's advice for `region` in agreement with a boundary `from` (huge pages, zero
/// for none): on from `huge` (the first page of the huge-page part) to the boundary, or to the
/// end, and off above it. Whether the OS took both calls.
fn reconcile<H: Host>(host: &H, region: RegionId, huge: u32, from: u32) -> bool {
    let layout = host.layout();
    let page = layout.page_size();
    let end = layout.region_pages();
    let to = if from == 0 {
        end
    } else {
        from * layout.huge_pages()
    };
    let mut ok = true;
    if to > huge {
        ok &= host.advise(
            region,
            huge as usize * page,
            (to - huge) as usize * page,
            HugePages::Prefer,
        );
    }
    if from != 0 && to < end {
        ok &= host.advise(
            region,
            to as usize * page,
            (end - to) as usize * page,
            HugePages::Avoid,
        );
    }
    ok
}

/// Lowers `region`'s boundary to the lowest huge page noted since it last came down, at
/// counted interval `epoch`, with `huge` the first page of the region's huge-page part: one
/// call taking the advice off from there to the old boundary, or the region's end. Skips if
/// nothing is noted and nothing needs repair, or another thread owns the advice.
pub(crate) fn lower<H: Host>(host: &H, region: RegionId, huge: u32, epoch: u64) -> Change {
    let word = host.tail(region);
    let seen = word.load();
    let tail = Tail::unpack(seen);
    if tail.low.is_none() && !tail.repair {
        return Change::None;
    }
    let Some((mine, owned)) = take(word, seen, epoch) else {
        return Change::None;
    };
    let layout = host.layout();
    let (page, hp) = (layout.page_size(), layout.huge_pages());
    let end_unit = layout.region_pages() / hp;
    let mut next = Tail {
        busy: false,
        low: None,
        ..owned
    };
    if owned.repair {
        next.repair = !reconcile(host, region, huge, owned.from);
        if next.repair {
            // Leave the noted huge page for the pass that can reconcile first.
            give_back(
                word,
                mine,
                Tail {
                    low: owned.low,
                    ..next
                },
            );
            return Change::Refused;
        }
    }
    let old = if owned.from == 0 {
        end_unit
    } else {
        owned.from
    };
    let change = match owned.low {
        Some(low) if low < old && low * hp >= huge => {
            let (offset, len) = (
                (low * hp) as usize * page,
                ((old - low) * hp) as usize * page,
            );
            if host.advise(region, offset, len, HugePages::Avoid) {
                next.from = low;
                Change::Done
            } else {
                next.repair = true;
                Change::Refused
            }
        }
        _ => Change::None,
    };
    give_back(word, mine, next);
    change
}

/// For the background thread, or a heartbeat where there is none: if a claim marked
/// `region`, puts the advice back on from the boundary up to `frontier` (a page, the region's
/// aged frontier) and [`TAIL_HEADROOM_HUGE_PAGES`] beyond it, or to the region's end if that
/// reaches above `top`, the page past the region's watermark: above the watermark nothing has
/// been touched, so advice there brings nothing back, and a class growing into fresh memory
/// faults huge pages again. Clears the mark either way; the next claim above the boundary
/// sets it again.
pub(crate) fn raise<H: Host>(
    host: &H,
    region: RegionId,
    huge: u32,
    frontier: u32,
    top: u32,
    epoch: u64,
) -> Change {
    let word = host.tail(region);
    let seen = word.load();
    let tail = Tail::unpack(seen);
    if !tail.want {
        return Change::None;
    }
    let Some((mine, owned)) = take(word, seen, epoch) else {
        return Change::None;
    };
    let layout = host.layout();
    let (page, hp) = (layout.page_size(), layout.huge_pages());
    let end_unit = layout.region_pages() / hp;
    let mut next = Tail {
        busy: false,
        want: false,
        ..owned
    };
    if owned.repair {
        next.repair = !reconcile(host, region, huge, owned.from);
        if next.repair {
            give_back(word, mine, next);
            return Change::Refused;
        }
    }
    if owned.from == 0 {
        give_back(word, mine, next);
        return Change::None;
    }
    let wanted = frontier.div_ceil(hp) + TAIL_HEADROOM_HUGE_PAGES;
    let to = if wanted >= top.div_ceil(hp) {
        end_unit
    } else {
        wanted.min(end_unit)
    };
    if to <= owned.from {
        give_back(word, mine, next);
        return Change::None;
    }
    let (offset, len) = (
        (owned.from * hp) as usize * page,
        ((to - owned.from) * hp) as usize * page,
    );
    let change = if host.advise(region, offset, len, HugePages::Prefer) {
        next.from = if to >= end_unit { 0 } else { to };
        // What was noted above the new boundary is under the advice now.
        next.low = next.low.filter(|&low| next.from != 0 && low < next.from);
        Change::Done
    } else {
        next.repair = true;
        Change::Refused
    };
    give_back(word, mine, next);
    change
}

/// What the walk's cheap look at one huge page above the frontier found.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Look {
    /// Enough of its sampled pages are free and resident that it is worth walking a page at a
    /// time.
    pub(crate) sparse: bool,
    /// An estimate of the free pages it has resident: its pages by the share of sampled pages
    /// that are. For a sparse one, what the lap counts toward the floor (`purge`, "The
    /// floor"); a dense one's are not what a purge could return.
    pub(crate) free: usize,
    /// Bitmap words read.
    pub(crate) read: usize,
}

/// The cheap look at huge page `unit` of `region` (a unit of whole pages, at most one huge
/// page): whether each of [`TAIL_SAMPLE_PAGES`] of its pages, evenly spread, is free and
/// resident, as the walk would find it (`region::purgeable_unit`). By pages, not by leaf words:
/// a word of a class of 1 KiB covers 16 pages, and a huge page with a live slot in every 16
/// pages has no free word and 15 free pages in 16. Reads only, at most a page's slots' words
/// and its clean word for each sample.
pub(crate) fn look<H: Host>(host: &H, region: RegionId, unit: Unit) -> Look {
    let pages = unit.end - unit.start;
    let samples = TAIL_SAMPLE_PAGES.min(pages);
    let (mut free, mut read) = (0, 0);
    for i in 0..samples {
        let start = unit.start + (i * pages) / samples;
        let (ok, words) = region::purgeable_unit(
            host,
            region,
            Unit {
                start,
                end: start + 1,
            },
        );
        read += words;
        free += u32::from(ok);
    }
    Look {
        sparse: free * TAIL_SAMPLE_PAGES >= TAIL_SPARSE_PAGES * samples,
        free: (pages * free / samples.max(1)) as usize,
        read: read.max(1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_zero_is_a_region_whose_advice_never_came_off() {
        assert_eq!(Tail::unpack(0), Tail::default());
        assert_eq!(Tail::default().pack(), 0);
    }

    #[test]
    fn the_word_round_trips() {
        let t = Tail {
            from: (1 << UNIT_BITS) - 1,
            low: Some(0),
            want: true,
            busy: true,
            repair: true,
            stamp: STAMP_MASK as u32,
        };
        assert_eq!(Tail::unpack(t.pack()), t);
        let t = Tail {
            from: 3,
            low: Some((1 << UNIT_BITS) - 2),
            ..Tail::default()
        };
        assert_eq!(Tail::unpack(t.pack()), t);
    }

    #[test]
    fn an_owner_goes_stale_after_two_intervals() {
        let t = Tail {
            busy: true,
            stamp: 5,
            ..Tail::default()
        };
        assert!(t.owned(5) && t.owned(6));
        assert!(!t.owned(7));
        // Across the stamp's wrap.
        let t = Tail {
            busy: true,
            stamp: STAMP_MASK as u32,
            ..Tail::default()
        };
        assert!(t.owned(STAMP_MASK + 1));
        assert!(!t.owned(STAMP_MASK + 2));
        assert!(!Tail::default().owned(0));
    }
}
