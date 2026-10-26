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
//! compare-and-swap. A claim sets the mark, and the heap's flag that some region is marked.
//! Changing the advice needs the owner bit: one compare-and-swap takes it from a word without
//! it, and a thread that finds it taken skips the change (the noted huge page or the mark stays
//! for the next look) rather than wait. The owner makes the one call, then writes the new
//! boundary and gives the bit back with one compare-and-swap. So no two advice changes of a
//! region overlap, and the boundary the word names is the advice the kernel last took.
//!
//! **Marks.** A look for marks (`Back::raise_marked`) clears the flag, then raises each marked
//! region whose bit it can take. One it cannot take keeps its mark, and the look does not set
//! the flag again: the owner's give-back does, if the word it leaves carries a mark. So a
//! thread that holds the bit and does not run cannot keep the background thread awake, and no
//! mark goes unseen once its owner is done. The background thread raises every marked region;
//! a request's heartbeat, where there is no thread, one a step.
//!
//! **Repair.** An owner that never gives the bit back would stop the region's advice for good.
//! In a live process every owner comes back; a bit taken two or more counted intervals ago is
//! taken over all the same, and the new owner first makes the kernel's advice agree with the
//! word: advice on from the start of the huge-page part to the boundary, off above it
//! (`reconcile`). An old owner that comes back after being taken over finds its
//! compare-and-swap refused and sets the repair bit, so the advice its late call left is put
//! right by the next owner. In a fork's child no thread holds a bit it inherited, so the child
//! clears every owner bit and asks for a repair at its first slow path (`Back::distrust_tails`);
//! the forking thread, if it forked from a signal handler in the middle of a change, then gives
//! back as a taken-over owner does. A call the kernel refused may have acted on part of its
//! range, so it sets the repair bit too. Either way the next change of the region's advice
//! reconciles it first, and nothing waits or loops.
//!
//! **Refusals.** A refused lowering keeps its noted huge page, and a refused raise its mark, so
//! that the change is made once the kernel takes it. After the kernel refuses a split, a raise
//! or a lowering, that kind of call is not made again before the counted interval or the
//! walk's lap count moves, whichever is first (`Back::tail_refused`): the interval stands
//! still in a silence and the lap count while no pass walks. A split's refusal leaves the
//! tail's huge pages whole, as below the frontier; a raise's leaves the marks set but not due,
//! so the background thread sleeps; a lowering's leaves the notes. A program the kernel refuses
//! throughout (at `vm.max_map_count`, with its memory locked, or on a kernel without the calls)
//! pays at most one refused call of each kind a tick of that clock, and two for a
//! reconciliation, and keeps main's behaviour: free pages of whole huge pages go back, the
//! tail's partly used huge pages stay whole.
//!
//! **Accounting.** Under the advice for huge pages the kernel may make any huge page that holds
//! one page wholly resident (khugepaged rebuilds it), so there the huge page is the unit of
//! residency, as everywhere it was never split. Whenever advice comes to cover partly returned
//! huge pages again, the caller restores that (`Back::restore_tail`): every free page of each
//! such huge page that holds anything counts as resident, its clean bits cleared under a claim
//! of its slots and the retained count raised, as a fill would. That is a raise's range, and
//! the huge pages between a note the frontier has passed and the frontier, where the advice
//! stays on. A huge page wholly returned stays as it is: nothing rebuilds one with no page
//! present. And a raise counts as a fill's reach over its range, so the walk treats the range
//! as below the frontier until that ages, and the next pass does not split it again at once.
//!
//! The protocol is `tla/Tail.tla`.

use super::layout::Unit;
use super::policy::{TAIL_HEADROOM_HUGE_PAGES, TAIL_SAMPLE_PAGES, TAIL_SPARSE_PAGES};
use super::region;
use super::tree::Host;
use super::word::Word;
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
pub(crate) const BUSY: u64 = 1 << (2 * UNIT_BITS + 1);
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

    /// Whether the owner bit is held and was taken less than [`STALE_INTERVALS`] before
    /// interval `epoch`. A stamp ahead of `epoch` is fresh: the reader's reading of the interval
    /// is older than the owner's (it read the count, the count stepped, the owner took the bit),
    /// and an owner a reader cannot yet see as old is not old. Ahead means within half the
    /// stamp's range; the stamp wraps after millions of intervals.
    const fn owned(self, epoch: u64) -> bool {
        let age = (epoch & STAMP_MASK).wrapping_sub(self.stamp as u64) & STAMP_MASK;
        self.busy && (age < STALE_INTERVALS || age > STAMP_MASK / 2)
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
/// the bit over (or a fork child cleared it, `Back::distrust_tails`): this thread's call may
/// have landed after that thread's reconciliation, so it asks for another. Fields a claim or
/// walker changed meanwhile (the mark, the noted huge page) are kept.
///
/// A look for marks (`Back::raise_marked`) that meets an owned region leaves its mark and does
/// not ask to be called again, or a thread that is not running would keep the background
/// thread from ever sleeping. So the owner answers for the mark: if the word it leaves carries
/// one, it sets the flag the lookers read (`marks`), one sequentially consistent operation,
/// as a claim's mark does, before the caller wakes the background thread.
fn give_back<W: Word>(word: &W, marks: &W, mine: u64, next: Tail) {
    let taken = Tail::unpack(mine);
    let mut seen = mine;
    loop {
        let now = Tail::unpack(seen);
        if !now.busy || now.stamp != taken.stamp {
            if word.fetch_or(REPAIR) & WANT != 0 {
                marks.fetch_or(1);
            }
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
            Ok(_) => {
                if merged.want {
                    marks.fetch_or(1);
                }
                return;
            }
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
///
/// Never below the region's frontier as it stands now (`region::floor`, rounded up to a huge
/// page): the walk noted the huge page when it lay above the frontier, and fills may have
/// reached it since, where the advice must stay. A note the frontier has passed lowers the
/// boundary to the frontier's huge page instead, since huge pages above it may have been
/// returned in part too and the word keeps only the lowest noted.
/// A refused call keeps the note (`tla/Tail.tla`, `Call`), so that the advice comes off there
/// once the kernel takes it.
#[cfg(test)]
pub(crate) fn lower<H: Host>(host: &H, region: RegionId, huge: u32, epoch: u64) -> Change {
    lower_moved(host, region, huge, epoch, true).change
}

/// What a change of a region's advice did, and the huge pages `start..end` it left under the
/// advice that the walk may have returned part of, whose accounting the caller restores
/// (`Back::restore_tail`): a raise's range, or the huge pages between a note the frontier
/// has passed and the frontier.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Moved {
    pub(crate) change: Change,
    pub(crate) under: Option<(u32, u32)>,
}

impl Moved {
    const NONE: Moved = Moved {
        change: Change::None,
        under: None,
    };
}

/// `lower`, saying what it left under the advice. With `clamp` false, `trim`'s, the caller
/// asking for everything back: the advice comes off at the note even below the frontier, and
/// fills that reach there later raise it again.
pub(crate) fn lower_moved<H: Host>(
    host: &H,
    region: RegionId,
    huge: u32,
    epoch: u64,
    clamp: bool,
) -> Moved {
    let word = host.tail(region);
    let seen = word.load();
    let tail = Tail::unpack(seen);
    if tail.low.is_none() && !tail.repair {
        return Moved::NONE;
    }
    let Some((mine, owned)) = take(word, seen, epoch) else {
        return Moved::NONE;
    };
    let marks = host.tail_marks();
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
                marks,
                mine,
                Tail {
                    low: owned.low,
                    ..next
                },
            );
            return Moved {
                change: Change::Refused,
                under: None,
            };
        }
    }
    let old = if owned.from == 0 {
        end_unit
    } else {
        owned.from
    };
    let reached = if clamp {
        region::floor(host, region, epoch).div_ceil(hp)
    } else {
        0
    };
    // At the noted huge page, or at the frontier's if the frontier has passed the note: the
    // word keeps only the lowest huge page noted, and huge pages above the frontier that the
    // walk returned part of since may lie anywhere up to the old boundary.
    let at = owned.low.map(|low| low.max(reached));
    let change = match at {
        Some(at) if at < old && at * hp >= huge => {
            let (offset, len) = ((at * hp) as usize * page, ((old - at) * hp) as usize * page);
            if host.advise(region, offset, len, HugePages::Avoid) {
                next.from = at;
                Change::Done
            } else {
                next.repair = true;
                next.low = owned.low;
                Change::Refused
            }
        }
        _ => Change::None,
    };
    // The huge pages from a note the frontier has passed up to the frontier, which the walk
    // may have returned part of, stay under the advice.
    let under = owned
        .low
        .filter(|&low| low < reached && low * hp >= huge && change != Change::Refused)
        .map(|low| (low, reached.min(old)));
    give_back(word, marks, mine, next);
    Moved { change, under }
}

/// For the background thread, or a heartbeat where there is none: if a claim marked
/// `region`, puts the advice back on from the boundary up to `frontier` (a page, the region's
/// aged frontier) and [`TAIL_HEADROOM_HUGE_PAGES`] beyond it, or to the region's end if that
/// reaches above `top`, the page past the region's watermark: above the watermark nothing has
/// been touched, so advice there brings nothing back, and a class growing into fresh memory
/// faults huge pages again. Clears the mark once the advice is on; a refused call keeps it,
/// and the caller does not try again before the next interval (`Back::raise_marked`).
#[cfg(test)]
pub(crate) fn raise<H: Host>(
    host: &H,
    region: RegionId,
    huge: u32,
    frontier: u32,
    top: u32,
    epoch: u64,
) -> Change {
    raise_moved(host, region, huge, frontier, top, epoch).change
}

/// `raise`, saying the huge pages it put back under the advice.
pub(crate) fn raise_moved<H: Host>(
    host: &H,
    region: RegionId,
    huge: u32,
    frontier: u32,
    top: u32,
    epoch: u64,
) -> Moved {
    let word = host.tail(region);
    // A take lost to another write of the word (a walker's note) is tried again: the look
    // that called this cleared the marks flag, and a mark it left behind with no owner would
    // be seen by no one (`tla/Tail.tla`, `MarkVisible`). Only a mark gone or an owner ends it.
    let (mine, owned) = loop {
        let seen = word.load();
        let tail = Tail::unpack(seen);
        if !tail.want || tail.owned(epoch) {
            return Moved::NONE;
        }
        if let Some(taken) = take(word, seen, epoch) {
            break taken;
        }
    };
    let marks = host.tail_marks();
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
            next.want = owned.want;
            give_back(word, marks, mine, next);
            return Moved {
                change: Change::Refused,
                under: None,
            };
        }
    }
    if owned.from == 0 {
        give_back(word, marks, mine, next);
        return Moved::NONE;
    }
    let wanted = frontier.div_ceil(hp) + TAIL_HEADROOM_HUGE_PAGES;
    let to = if wanted >= top.div_ceil(hp) {
        end_unit
    } else {
        wanted.min(end_unit)
    };
    if to <= owned.from {
        give_back(word, marks, mine, next);
        return Moved::NONE;
    }
    let (offset, len) = (
        (owned.from * hp) as usize * page,
        ((to - owned.from) * hp) as usize * page,
    );
    let mut under = None;
    let change = if host.advise(region, offset, len, HugePages::Prefer) {
        under = Some((owned.from, to));
        next.from = if to >= end_unit { 0 } else { to };
        // What was noted above a new boundary is under the advice now; with no boundary left,
        // a note (below the old one) is kept for the next lowering.
        next.low = next.low.filter(|&low| next.from == 0 || low < next.from);
        Change::Done
    } else {
        next.repair = true;
        next.want = owned.want;
        Change::Refused
    };
    give_back(word, marks, mine, next);
    Moved { change, under }
}

/// The free pages of `unit` of `region` still resident, as the walk would return them
/// (`region::purgeable_unit` page by page), and the words read: the walk's own count, which
/// decides whether a huge page above the frontier is split (`policy::TAIL_SPLIT_SHARE`). It reads
/// what a walk of the whole huge page reads: the leaf words of its slots, `2 MiB / size / 64`
/// of them (32 for 1 KiB slots, 2048 for 16-byte ones), and a clean word for each page.
pub(crate) fn free_resident<H: Host>(host: &H, region: RegionId, unit: Unit) -> (u32, usize) {
    let (mut free, mut read) = (0, 0);
    for start in unit.start..unit.end {
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
    (free, read)
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
/// page): whether each of [`TAIL_SAMPLE_PAGES`] of its pages is free and resident, as the
/// walk would find it (`region::purgeable_unit`). One page is sampled in each of as many equal
/// shares of the huge page, at a place within its share that `salt` picks: the walk's count of
/// laps, so that every look at a huge page, in a silence too, samples other pages, and a huge
/// page misjudged once is judged again. The look only chooses which huge pages the walk counts
/// exactly ([`free_resident`]); the count decides the split. By pages, not by leaf words: a word of a class of 1 KiB covers 16
/// pages, and a huge page with a live slot in every 16 pages has no free word and 15 free
/// pages in 16. Reads only, at most a page's slots' words and its clean word for each sample.
pub(crate) fn look<H: Host>(host: &H, region: RegionId, unit: Unit, salt: u32) -> Look {
    let pages = unit.end - unit.start;
    let samples = TAIL_SAMPLE_PAGES.min(pages);
    let share = pages / samples.max(1);
    let (mut free, mut read) = (0, 0);
    for i in 0..samples {
        // A multiplicative hash of the salt and the share: any spread will do, so long as a
        // later salt moves the samples and they do not keep one phase of a periodic layout.
        let mix = (salt ^ i.wrapping_mul(0x9E37_79B9)).wrapping_mul(0x85EB_CA6B) >> 16;
        let start = unit.start + i * share + mix % share;
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
        // A reader one interval behind the owner sees a fresh owner.
        let t = Tail {
            busy: true,
            stamp: 5,
            ..Tail::default()
        };
        assert!(t.owned(4) && t.owned(3));
        let t = Tail {
            busy: true,
            stamp: 0,
            ..Tail::default()
        };
        assert!(t.owned(STAMP_MASK));
    }
}
