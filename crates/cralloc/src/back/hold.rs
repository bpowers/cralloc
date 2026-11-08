//! The hold on free large slots: they go back to the OS once their region has had no free
//! that left a slot resident for a short hold, or once they have themselves been free for it,
//! down to a floor.
//!
//! Address space is divided by class, so memory freed in one large class serves another only
//! by way of the OS. A program whose phases use different large classes (a query engine whose
//! next query's blocks fall in other classes, a server whose request mix changes) otherwise
//! holds every phase's free large slots on top of the next phase's live ones, until the
//! clocked pass, a second's step, returns them (`docs/research/large-blocks.md`, "Free slots: a
//! hold, and why no hold serves everyone"). A free large slot is reused within tens of
//! milliseconds when it is reused at all (the same section: 86 to 100% of the slots taken
//! again after a purge were taken within 60 ms), so a hold of that order returns what the
//! program has moved away from and keeps what it is reusing.
//!
//! **The rule.** A free of a large slot that leaves it resident stamps its region with the
//! fine count ([`Fine`]). Once the fine count has passed [`HOLD_STEPS`] steps beyond a region's
//! stamp, the region's free slots may go back. They go back only while free large memory
//! believed resident, in every large class together (`Back::retained_large`), is over the floor
//! (`policy::allowance_floor`), and only down to it. So the hold is per region, and the floor is on
//! all of it.
//!
//! **Each free slot ages on its own.** A region's stamp is its latest free's, so under steady
//! churn no region is ever a hold old, and a slot idle for minutes stays resident beside one
//! freed a moment ago: on a cache of large buffers, a gigabyte of free slots stood that way
//! (`docs/research/large-blocks.md`, "Each free slot ages on its own"). So the free that leaves
//! a slot resident also stamps the slot, in its size word (`Recorded::freed`), as its holder,
//! before it releases it. A look at a region whose own stamp is young walks the region's free
//! slots from its watermark down and returns each one whose own stamp is a hold old, claiming
//! it first as every purge does ([`walk_aged`]), within the look's bounds and down to the floor
//! as any look. The walk carries on across looks from where it stopped (the region's `AgeWalk`
//! word), so every free slot of the region is looked at once a walk, wherever it lies. A walk
//! starts at most once a hold (the hold the look takes): so a slot goes back between one hold
//! and two after its free where heartbeats look, and within about three where the background
//! thread does, whose look is planned from the region's newest stamp and can arrive just before
//! a walk may start. And a young region costs one pass over its free slots a hold, whoever
//! makes it. Walking whenever some slot came of age was a walk at nearly every fine step under
//! steady churn, which is the case the rule is for (a third of the steps, on a region of 3,000
//! slots under drifting demand). A walk under way is more for the look to do: the next caller
//! carries on at once, so that a walk takes its looks one after another and not one a fine
//! step. A region whose stamp is old goes back whole, as before.
//!
//! **Time is counted.** The fine count steps when a heartbeat's reading of the counted clock
//! (`crate::clock`) finds that [`policy::FINE_TICKS`] have passed since its last step, and by
//! one step however much more has passed. Heartbeats take it, so the hold lasts
//! `HOLD_STEPS` fine steps, about 125 ms, only for a program that beats at least once a fine
//! step; one that beats less often holds longer, and across a silence a hold ages by one step,
//! so the first slow paths after a pause do not find every held slot expired and tear down what
//! the program is about to take again. The heartbeat reads no clock for this: it notes the
//! reading the clocked pass's step made, and reads one itself only when that step made none
//! (a pass under way steps without the time), which the depot's heartbeat answers from its
//! own reading when it made one.
//!
//! **The look.** Once a fine step, the first heartbeat to see the count move takes the look
//! (a compare-and-swap of [`Hold::looked`]). It reads the large tally; at or under the floor it
//! does nothing more and keeps no state. Over the floor it walks the large classes' regions
//! from where the last look stopped, purges the free units of regions whose stamp is old,
//! and stops at the floor, at a region's worth of the walk at a time, or at
//! [`policy::hold_beat`] bytes or [`PURGE_STEP_WORDS`] words of bitmap, whichever comes
//! first. A look stopped by a bound with more to return, having returned some, leaves the next
//! heartbeat to carry on at once; one that found nothing to return waits for the next step.
//! The walk of a region uses the region's own purge cursor (`layout::Hints`), the one every
//! purger carries on from, so a region whose slots above its free ones are live costs a
//! bounded walk each look and still gets to them. A region whose walk, in one look or several,
//! went from its top to its bottom with no free meanwhile loses its stamp, so later looks pass
//! it at a load until the next free.
//!
//! **Maintain.** `maintain`'s round takes looks too, before its clocked pass, a step at a
//! time within the bounds of that pass's steps, until one returns nothing or the floor is
//! reached. It runs once a counted interval whether heartbeats run or not, and in a silence its
//! own readings are what step the fine count, about once an interval, so a round decides its
//! hold once, at its first look: the hold, unless the count has moved only the few steps its
//! own readings make since the first look of the round before, and then a stamp from before
//! that look (`Back::maintain_hold`). A quiet program's background thread or task returns its
//! held large slots at its second round, while in a busy program `maintain` holds them as the
//! heartbeats do.
//!
//! **The thread.** While the background thread runs, the looks are its work (design:
//! "The background thread"). Heartbeats still step the fine count, which is how the hold keeps its time,
//! and take no look. The thread foresees its next look: after its looks it plans
//! ([`Back::plan_hold`]) the fine count at which the soonest stamp not yet held for a hold will
//! have been, and stores it in [`Hold::next`]; the heartbeat whose reading steps the count to
//! it wakes the thread. Counted steps, not a timer: the count moves only with readings, so a
//! hold measured in its steps cannot be foreseen in wall time, and in a silence, where no
//! heartbeat steps it, no hold matures and nothing wakes the thread for one (`maintain`'s
//! rounds return what a silence leaves). What the thread cannot foresee is told to it: a free
//! that stamps a region when no look is planned sets one a hold ahead, and an add that takes
//! free large memory over the floor, when the thread planned under it ([`NEXT_LEVEL`]), sets
//! one at once. A planned look a free's stamp has since moved wakes the thread for nothing,
//! and it plans again. A region held already that keeps its stamp has not been walked to its
//! bottom (a look stopped at its bound of bitmap, or at the floor), and sets the next look at
//! the next step, as a heartbeat would take it: planning only from stamps still held let a
//! look that stopped at its bound wait a whole hold or more, and DataFusion's peak rose by
//! 180 MiB in one run in ten (`docs/research/memory.md`, "Returning memory off request paths").
//! The walks by slot ages plan nothing: a young region's stamp already wakes the thread a hold
//! on, which is when a walk of it may start again, and a walk under way is carried on in the
//! same wake-up. In a silence no free stamps anything, every region's stamp goes old, and the
//! regions go back whole.
//!
//! **How long.** One hold for every large class, [`HOLD_STEPS`]. A hold that adapted per
//! class (doubling when a slot was claimed soon after the hold purged its region, halving on
//! the clock) held DataFusion's peak at the same level for 9% more faults, so it was not kept
//! (`docs/research/large-blocks.md`, "Free slots: a hold, and why no hold serves everyone").
//!
//! **The words.** The fine count, the count of the last look, the walk's position and the
//! thread's next look, on the hold's line in the back end's state. A stamp per large-class
//! region, in the header line of its metadata block, which large allocations and frees read
//! already (`layout`): a free loads the fine count and its region's stamp, and writes the stamp
//! only when it differs, so once per region per fine step at most, and then loads the
//! thread's next look. A small class's region has no stamp. All of these
//! are hints. A stale stamp, a lost look or a lost position returns a slot early or late, never
//! one in use, because a purge is a claim (`region::purge_unit`); a fork that leaves a look
//! half done leaves nothing the next step does not take up. Stamps compare with the fine count
//! by wrapping difference: one up to a hold ahead of the count is a free that read it after
//! the look did, and is fresh; any other at least a hold behind is old, so a stamp left over
//! for half the count's range (at a step every 2 ms, about 50 days) reads as old, not as one
//! from the future; one left for nearly the whole range reads as fresh for a hold.
//!
//! A slot's own stamp is the count's low 28 bits, which is what the size word has room for
//! beside a size of up to 4 GiB and its three flags: a wrap every 2^28 steps, six days or more.
//! A stamp reads as its true age less a multiple of 2^28, never older ([`until_aged`]), so a
//! slot free across a wrap of its stamp is kept up to two holds longer, and returned no
//! earlier than it would have been. The walk reads a free slot's stamp with no claim, while
//! another thread may claim and free the slot again: that may return a slot a moment after its
//! free, through the claim, and nothing worse. A region's `AgeWalk` word is written only by
//! looks, with a compare-and-swap against what the look read, so a look that read it before
//! another moved it writes nothing; a lost write moves a walk back, which looks at some slots
//! twice, or starts the next walk up to a hold late.
//!
//! **Where purges are refused** (memory locked with `mlockall`, which the move's copy allows
//! for), every claim of a free large slot with slack makes one failing call to trim it, and its
//! pages stay counted: a documented limit, one call a claim, not a mechanism.

use super::Back;
use super::host::ClassHost;
use super::layout::{AgeWalk, Grow, Hints, Recorded, STAMP_MASK, Unit};
use super::policy::{
    self, HOLD_STEPS, PURGE_MAINTAIN_STEP_PAGES, PURGE_MAINTAIN_STEP_WORDS, PURGE_STEP_WORDS,
    allowance_floor, whole_huge_pieces,
};
use super::purge::{PassStep, Scope, Splits, piece};
use super::region::{self, Purge};
use super::tree::{self, Host};
use super::word::{BITS, Word};
use crate::os::Os;
use crate::space::{Geometry, RegionId};
use core::sync::atomic::Ordering::{Relaxed, SeqCst};
use core::sync::atomic::{AtomicU32, AtomicU64};

/// The fine count: its value in the high 32 bits, and the counted time of its last step in
/// units of [`UNIT_SHIFT`] ticks in the low 32. All zero is a valid state (no step yet).
pub(crate) struct Fine {
    word: AtomicU64,
}

/// The unit of the fine count's time: 2^20 ticks, a fraction of a millisecond.
const UNIT_SHIFT: u32 = 20;

/// A fine step in units.
const STEP_UNITS: u32 = (policy::FINE_TICKS >> UNIT_SHIFT) as u32;

const _: () = assert!(STEP_UNITS >= 1);

const fn pack(count: u32, unit: u32) -> u64 {
    (count as u64) << 32 | unit as u64
}

/// The word to store over `word` for a reading of the counted time `time`, if any: one step
/// if a fine step or more has passed since the last, however much more; a re-stamp, with no
/// step, if the time is a step or more behind the stamp (the clock went back). A reading less
/// than a step behind is another thread's, taken a moment before the one that stepped: it
/// writes nothing, since every large free reads this word, and a re-stamp at each such
/// reading took its line from them (0.6% on a pipeline of large objects).
pub(crate) const fn fine_read(word: u64, time: u64) -> Option<u64> {
    let count = (word >> 32) as u32;
    let stamp = word as u32;
    let unit = (time >> UNIT_SHIFT) as u32;
    let since = unit.wrapping_sub(stamp) as i32;
    if since < 0 {
        if since.unsigned_abs() < STEP_UNITS {
            return None;
        }
        Some(pack(count, unit))
    } else if since as u32 >= STEP_UNITS {
        Some(pack(count.wrapping_add(1), unit))
    } else {
        None
    }
}

impl Fine {
    pub(crate) const fn new() -> Fine {
        Fine {
            word: AtomicU64::new(0),
        }
    }

    /// The fine count.
    #[inline(always)]
    pub(crate) fn count(&self) -> u32 {
        // Sequentially consistent, as the thread's look before it sleeps reads it after
        // marking its word (`crate::thread_word`, "Sleeping").
        (self.word.load(SeqCst) >> 32) as u32
    }

    /// Advances the count for a reading of the counted time `time` (at most one step). Returns
    /// the new count if this reading stepped it.
    fn note(&self, time: u64) -> Option<u32> {
        let seen = self.word.load(Relaxed);
        let next = fine_read(seen, time)?;
        // A failure means another reader stepped or re-stamped, which is the same reading or a
        // later one: nothing to retry, so it needs no order. Sequentially consistent on success,
        // as the reader that steps the count then reads the thread's word, and the thread marks
        // its word before it reads the count (`crate::thread_word`, "Sleeping").
        let stepped = self
            .word
            .compare_exchange(seen, next, SeqCst, Relaxed)
            .is_ok()
            && next >> 32 != seen >> 32;
        stepped.then_some((next >> 32) as u32)
    }
}

/// The hold's shared words (see the module documentation).
pub(crate) struct Hold {
    pub(crate) fine: Fine,
    /// The fine count at the last look, or one less when that look stopped at a bound with
    /// more to return, so that the next heartbeat looks again.
    looked: AtomicU32,
    /// Where the next look's walk starts: a region number plus one; zero for the top.
    at: AtomicU32,
    /// `maintain`'s hold for the round under way: the counted interval of the round plus one
    /// (zero for none) in the high half, and the hold, in fine steps, in the low half
    /// (`Back::maintain_hold`).
    maintained: AtomicU64,
    /// The fine count at the first look of `maintain`'s latest round, plus one; zero for none.
    maintain_looked: AtomicU32,
    /// When the background thread should look next (see the module documentation, "The
    /// thread"): [`NEXT_AT`] with the fine count in the low half, [`NEXT_LEVEL`], or zero.
    /// Checked by the interleaving explorations (`front::sync`): the thread's plan, frees and
    /// crossings race on it.
    next: crate::front::sync::AtomicU64,
    /// For tests of the hold's mechanics on heaps no larger than the floor: the hold acts as if
    /// the floor were zero.
    #[cfg(test)]
    pub(crate) unfloored: core::sync::atomic::AtomicBool,
    /// For tests: the bytes the walks by slot ages have returned (`walk_aged`).
    #[cfg(test)]
    pub(crate) aged_returned: core::sync::atomic::AtomicUsize,
}

/// In [`Hold::next`], with a fine count in the low half: the step of the count that reaches it
/// wakes the thread for a look.
pub(super) const NEXT_AT: u64 = 1 << 32;
/// [`Hold::next`]: free large memory was at or under the floor when the thread planned, so no
/// hold can return anything; a crossing of an add over the floor sets a look at once.
pub(super) const NEXT_LEVEL: u64 = 1 << 33;

/// Whether the step of the fine count to `count` reaches the look `next` sets.
const fn reached(next: u64, count: u32) -> bool {
    next & NEXT_AT != 0 && count.wrapping_sub(next as u32) as i32 >= 0
}

/// Of a look `armed` that a free or a crossing set and one the thread `planned`, the one that
/// comes first; zero is no look.
const fn sooner(armed: u64, planned: u64) -> u64 {
    if armed & NEXT_AT == 0 {
        planned
    } else if planned & NEXT_AT == 0 || ((planned as u32).wrapping_sub(armed as u32) as i32) > 0 {
        armed
    } else {
        planned
    }
}

impl Hold {
    pub(crate) const fn new() -> Hold {
        Hold {
            fine: Fine::new(),
            looked: AtomicU32::new(0),
            at: AtomicU32::new(0),
            maintained: AtomicU64::new(0),
            maintain_looked: AtomicU32::new(0),
            next: crate::front::sync::AtomicU64::new(0),
            #[cfg(test)]
            unfloored: core::sync::atomic::AtomicBool::new(false),
            #[cfg(test)]
            aged_returned: core::sync::atomic::AtomicUsize::new(0),
        }
    }
}

/// A region's stamp for fine count `count`: the count, with a bit above it so that no stamp
/// is zero, which is a region with no stamp.
const fn stamp_of(count: u32) -> u64 {
    1 << 32 | count as u64
}

/// The most fine steps between the first looks of two of `maintain`'s rounds that `maintain`'s
/// own readings make in a silence: one a round, and a few more for a round whose purge phase
/// lasts several fine steps (`Back::maintain_hold`). Heartbeats make hundreds an interval.
const QUIET_STEPS: u32 = 4;

/// How many of `maintain`'s rounds before this one a quiet round's hold reaches back: a stamp
/// older than the previous round's first look goes (`Back::maintain_hold`, the steps since that
/// look plus this).
const ROUNDS_BACK: u32 = 1;

/// The rounds of `maintain` after a silence begins by which its hold returns a large slot freed
/// before it: the background thread's count of quiet rounds must cover them (`depot::thread`).
pub(crate) const MAINTAIN_HOLD_ROUNDS: u64 = 1 + ROUNDS_BACK as u64;

/// Set in a stamp by the look that starts a walk of the region from its top: the walk that
/// reaches the region's bottom with the stamp unchanged clears it, however many looks the
/// walk took. A free writes a stamp without it, so a free during the walk keeps the stamp.
const WALKED_FROM_TOP: u64 = 1 << 33;

/// Whether a region stamped `stamp` has been held for `steps` at fine count `count`: the count
/// is at least `steps` past the stamp, and the stamp is not up to a hold ahead of the count,
/// which a free that read the count after the look did writes.
pub(crate) const fn held_for(stamp: u64, count: u32, steps: u32) -> bool {
    let age = count.wrapping_sub(stamp as u32);
    age >= steps && age.wrapping_neg() > HOLD_STEPS
}

/// The fine steps until a free large slot stamped `freed` (`Recorded::freed`, the count's low
/// 28 bits) will have been free for `steps` at count `count`, zero once it has: `held_for` in
/// the stamp's bits, at most `steps`. A stamp up to a hold ahead of the count is a free that
/// read the count after the look did, and is fresh. A stamp reads as its true age less a
/// multiple of 2^28, so one left for 2^28 steps or more can only read younger than it is: the
/// slot is kept up to two holds longer, never returned early. And whatever a stamp says, the
/// slot goes back only through a claim of it (`region::purge_unit`), so a wrong age costs a keep
/// or a purge, never a slot in use.
pub(crate) const fn until_aged(freed: u32, count: u32, steps: u32) -> u32 {
    let age = count.wrapping_sub(freed) & STAMP_MASK;
    if age.wrapping_neg() & STAMP_MASK <= HOLD_STEPS {
        // Freed at this count, or a moment after.
        steps
    } else {
        steps.saturating_sub(age)
    }
}

/// Whether the walk of a young region by slot ages whose word says `walk` may take a step at
/// fine count `count` with a hold of `steps`: one is under way, or the last began in the
/// step a hold before `count` or earlier, `steps - 1` steps or more (wrapping: a start the
/// count has not reached, which only a stale word can hold, reads as long ago). A step short
/// of the hold, because the background thread's look comes when the region's latest stamp is
/// a hold old, and under churn that stamp is often a free of the step before the look that
/// began the last walk: held to the whole hold, the thread's walks came every other hold. All
/// zero is a region never walked; its first walk may start once the count is a hold past zero.
pub(crate) const fn walk_due(walk: AgeWalk, count: u32, steps: u32) -> bool {
    walk.cursor != 0 || count.wrapping_sub(walk.began).saturating_add(1) >= steps
}

/// What a look's walk of one young region by its free slots' own ages did (`walk_aged`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(super) struct Aged {
    /// Metadata words read: leaf words, the size words of free slots, and the clean words of
    /// the slots it looked at.
    pub(super) read: usize,
    /// Bytes returned that had been resident.
    pub(super) returned: usize,
    /// Of those, the bytes of kept outgrown slots (`Recorded::outgrown`).
    pub(super) outgrown: usize,
    /// The walk took no step (none is under way and the last began within the hold), or
    /// reached the region's bottom.
    pub(super) done: bool,
    /// The walk took a step: one was under way, or one was due.
    pub(super) walked: bool,
}

/// One look's part of the walk of a young large region by its free slots' own ages: carries on
/// the walk under way, or starts one at the watermark if the last began `steps` or more ago
/// (`walk_due`), and goes down the region's free slots, returning each one whose own stamp
/// (`Recorded::freed`) is `steps` old, until it reaches the bottom, reads `budget` words or
/// returns `limit` bytes. It looks at one slot at least, so a walk always moves.
///
/// So a young region is walked once a hold at most, and a slot goes back between one hold and
/// two after its free.
///
/// A slot is returned as any purge returns one: claimed, its pages returned, its clean and zero
/// bits set, and released (`region::purge_unit`). One larger than what the look may still
/// return goes back a piece at a time from its top (`purge::piece`), the walk staying at it so
/// that the next look carries on there. Where the walk stands is the region's `AgeWalk` word,
/// which only this changes, with a compare-and-swap against what it read: a look abandoned part
/// way leaves the walk where it last stood, and one that finds the word moved under it leaves
/// it moved.
///
/// Each free slot that is free throughout a walk is looked at once in that walk, wherever it
/// lies, so none is starved. A slot freed above the walk's position, or carved after the walk
/// began, waits for the next walk. A stamp is read without a claim, from a slot another thread may
/// claim and free again meanwhile: a hint, which at worst returns a slot a moment after its
/// free, through the claim that every purge makes.
pub(super) fn walk_aged<H: Host>(
    host: &H,
    region: RegionId,
    count: u32,
    steps: u32,
    limit: usize,
    budget: usize,
) -> Aged {
    let layout = host.layout();
    let idle = Aged {
        read: 1,
        done: true,
        ..Aged::default()
    };
    let (Some(at), Some(_)) = (layout.age_word(), layout.size_word(0)) else {
        return idle;
    };
    let word = host.meta(region, at);
    let seen = word.load();
    let walk = AgeWalk::unpack(seen);
    if !walk_due(walk, count, steps) {
        return idle;
    }
    let (mut upper, began) = if walk.cursor != 0 {
        (walk.cursor.min(layout.slots()), walk.began)
    } else {
        let carved = Grow::unpack(host.grow(region).load()).carved;
        (carved.min(layout.slots()), count)
    };
    let page = layout.page_size();
    // In whole huge pages where a class is that large, as the purge walk's pieces are, so that a
    // slot that kept its huge-page advice is not left with one partly returned.
    let granule = if whole_huge_pieces(host.geometry(), layout.size()) {
        layout.huge_pages()
    } else {
        1
    };
    let mut out = Aged {
        walked: true,
        ..Aged::default()
    };
    let mut first = true;
    let stop = |out: Aged, upper: u32| {
        let next = AgeWalk {
            cursor: upper,
            began,
        };
        let _ = word.compare_exchange(seen, next.pack());
        out
    };
    while upper > 0 {
        if !first && (out.read >= budget || out.returned >= limit) {
            return stop(out, upper);
        }
        let w = (upper - 1) / BITS;
        // The free slots of the word below `upper`.
        let leaf = tree::word(host, tree::Node::leaf(region, w)).load();
        let mut bits = leaf & u64::MAX >> (w * BITS + BITS - upper);
        out.read += 1;
        while bits != 0 {
            let bit = BITS - 1 - bits.leading_zeros();
            bits &= !(1u64 << bit);
            let slot = w * BITS + bit;
            if !first && (out.read >= budget || out.returned >= limit) {
                return stop(out, slot + 1);
            }
            first = false;
            let freed = region::recorded(host, region, slot).map_or(0, |r| r.freed);
            out.read += 1;
            if until_aged(freed, count, steps) != 0 {
                continue;
            }
            let Some(unit) = layout.slot_unit(slot) else {
                continue;
            };
            // Free and not all clean: a slot taken since the leaf word was read, or returned
            // already, is passed at a look.
            let (purgeable, words) = region::purgeable_unit(host, region, unit);
            out.read += words;
            if !purgeable {
                continue;
            }
            let most =
                (limit.saturating_sub(out.returned).div_ceil(page)).min(u32::MAX as usize) as u32;
            let most = most.max(granule);
            let cut = if unit.end - unit.start > most {
                let (top, words) = region::clean_top(host, region, unit);
                out.read += words;
                if top <= unit.start {
                    continue;
                }
                piece(Unit { end: top, ..unit }, most, 1, granule)
            } else {
                unit
            };
            // Its look before claiming reads what `purgeable_unit` just read and counted.
            if let (Purge::Done { returned, outgrown }, _) = region::purge_unit(host, region, cut) {
                out.returned += returned;
                out.outgrown += outgrown;
                if cut.start > unit.start {
                    // The rest of the slot, below the piece, is the next look's.
                    return stop(out, slot + 1);
                }
            }
        }
        upper = w * BITS;
        first = false;
    }
    let _ = stop(out, 0);
    Aged { done: true, ..out }
}

/// What a look may take: regions held for `steps` fine steps, within `bytes` returned and
/// `words` of bitmap read.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct Reach {
    pub(super) steps: u32,
    pub(super) bytes: usize,
    pub(super) words: usize,
}

impl Reach {
    /// A heartbeat's look: the hold, within [`policy::hold_beat`] and a heartbeat's step of
    /// bitmap.
    pub(super) const fn beat(geometry: Geometry) -> Reach {
        Reach {
            steps: HOLD_STEPS,
            bytes: policy::hold_beat(geometry),
            words: PURGE_STEP_WORDS,
        }
    }

    /// One of `maintain`'s steps: a hold of `steps`, within what a step of its clocked pass may
    /// do (`policy::PURGE_MAINTAIN_STEP_PAGES`, `policy::PURGE_MAINTAIN_STEP_WORDS`).
    pub(super) const fn maintain(geometry: Geometry, steps: u32) -> Reach {
        Reach {
            steps,
            bytes: PURGE_MAINTAIN_STEP_PAGES as usize * geometry.page_size(),
            words: PURGE_MAINTAIN_STEP_WORDS,
        }
    }
}

impl<O: Os> Back<'_, O> {
    /// Stamps `region` with the fine count, for a free of one of its large slots that left the
    /// slot resident. A load of the fine count and one of the stamp, and a store only when the
    /// stamp differs. Nothing for a small class, whose layout has no stamp.
    #[inline]
    pub(super) fn hold_stamp(&self, host: &ClassHost<'_, O>, region: RegionId) {
        let Some(at) = host.layout().hold_word() else {
            return;
        };
        let word = host.meta(region, at);
        let count = self.state.hold.fine.count();
        let stamp = stamp_of(count);
        // A compare-and-swap, not a store, and the thread's next look read after it: the
        // thread's plan clears that look and then reads the stamps, so one of the two sees the
        // other's write (`plan_hold`). Until the word is this stamp: a failure may be a look's
        // own write (its mark of a walk from the top, or the stamp it clears at the bottom),
        // which would leave a slot freed resident in a region with no stamp. No test reaches
        // the retry: the region's stamp word is not on the interleaving checker's atomics, so
        // no exploration can put a look's write between this load and the compare-and-swap.
        let mut seen = word.load();
        while seen != stamp {
            match word.compare_exchange(seen, stamp) {
                Ok(_) => {
                    self.hold_arm(count);
                    return;
                }
                Err(now) => seen = now,
            }
        }
    }

    /// The floor the hold returns free large memory down to (`policy::allowance_floor`).
    fn hold_floor(&self) -> usize {
        #[cfg(test)]
        if self.state.hold.unfloored.load(Relaxed) {
            return 0;
        }
        allowance_floor(self.space.plan().geometry())
    }

    /// Adds `bytes` that a look returned to what the purge walk counts as returned since its
    /// last lap ended, as every purger's returns are (`purge`, "The floor").
    fn count_returned(&self, bytes: usize) {
        if bytes != 0 {
            self.state.walk.returned.fetch_add(bytes, Relaxed);
        }
    }

    /// The fine count as a free slot's stamp keeps it (`Recorded::freed`).
    #[inline]
    pub(super) fn fine_stamp(&self) -> u32 {
        self.state.hold.fine.count() & STAMP_MASK
    }

    /// Stamps large slot `slot` with the fine count, in its size word, for the free that holds
    /// it and is about to release it resident, so that the slot ages on its own when frees keep
    /// its region young (`walk_aged`). The holder is the word's only writer until the release,
    /// so a load and a store.
    #[inline]
    pub(super) fn stamp_slot(&self, host: &ClassHost<'_, O>, region: RegionId, slot: u32) {
        if let Some(now) = region::recorded(host, region, slot) {
            let stamped = Recorded {
                freed: self.fine_stamp(),
                ..now
            };
            region::record(host, region, slot, stamped);
        }
    }

    /// Sets the thread's next look a hold after fine count `count`, for a free that stamped a
    /// region then, if the thread's plan knew of no stamp to wait for. A load from the hold's
    /// line, which the stamp read already, and a compare-and-swap only from no look.
    fn hold_arm(&self, count: u32) {
        let next = &self.state.hold.next;
        if next.load(SeqCst) == 0 {
            let at = NEXT_AT | u64::from(count.wrapping_add(HOLD_STEPS));
            let _ = next.compare_exchange(0, at, SeqCst, Relaxed);
        }
    }

    /// Sets the thread's next look at once, for an add that has just taken free large memory
    /// over the floor, if the thread's plan was made under it, or knew of no stamp to wait for.
    pub(super) fn hold_level(&self) {
        let hold = &self.state.hold;
        let mut seen = hold.next.load(SeqCst);
        // Until a look is set: a failure may be the thread's plan storing its wait for the
        // level, made from a count read before this add (the exploration
        // `the_threads_plan_keeps_a_crossings_look_under_every_schedule` found it).
        while seen == NEXT_LEVEL || seen == 0 {
            let at = NEXT_AT | u64::from(hold.fine.count());
            match hold.next.compare_exchange(seen, at, SeqCst, SeqCst) {
                Ok(_) => return,
                Err(now) => seen = now,
            }
        }
    }

    /// A heartbeat's part of the hold: advances the fine count for the reading of the counted
    /// time `time`, and takes the look if the count has moved since the last one (or that one
    /// left more to do). Between steps, two loads from the hold's line. Says what the look
    /// returned, and whether it stopped at its bound with more to return.
    pub(super) fn hold_beat(&self, time: u64) -> PassStep {
        let hold = &self.state.hold;
        hold.fine.note(time);
        let count = hold.fine.count();
        if hold.looked.load(Relaxed) == count {
            return PassStep::default();
        }
        self.hold_take_look(count, |back| Reach::beat(back.space.plan().geometry()))
    }

    /// A slow path's part of the hold while the background thread returns memory: advances
    /// the fine count for the reading `time`, and says whether this reading's step reached the
    /// thread's next look. Two loads from the hold's line, and the second only on a step.
    pub(super) fn hold_note(&self, time: u64) -> bool {
        let hold = &self.state.hold;
        hold.fine
            .note(time)
            .is_some_and(|count| reached(hold.next.load(SeqCst), count))
    }

    /// Whether the thread's next look is due at the reading `time`, which may step the count:
    /// the thread's own last look before it sleeps.
    pub(super) fn hold_due(&self, time: u64) -> bool {
        let hold = &self.state.hold;
        hold.fine.note(time);
        reached(hold.next.load(SeqCst), hold.fine.count())
    }

    /// The background thread's plan for the hold, after its looks: when it should look next
    /// (see the module documentation, "The thread"). Clears the look first and reads the
    /// stamps after, and a free stamps and then arms the look, so a stamp this plan misses
    /// arms its own; and keeps whichever comes first of what it planned and what was armed
    /// meanwhile.
    pub(crate) fn plan_hold(&self) {
        let hold = &self.state.hold;
        hold.next.swap(0, SeqCst);
        let planned = if self.retained_large() <= self.hold_floor() {
            NEXT_LEVEL
        } else {
            let count = hold.fine.count();
            self.hold_pending(count)
                .map_or(0, |steps| NEXT_AT | u64::from(count.wrapping_add(steps)))
        };
        let mut seen = 0;
        loop {
            let next = sooner(seen, planned);
            if next == seen {
                return;
            }
            match hold.next.compare_exchange(seen, next, SeqCst, SeqCst) {
                Ok(_) => return,
                Err(now) => seen = now,
            }
        }
    }

    /// The fine steps from `count` until the next look: one, while a region held for
    /// [`HOLD_STEPS`] keeps its stamp (the looks have not walked it to its bottom: a look that
    /// stops at its bound of bitmap having returned nothing waits for the next step, as a
    /// heartbeat's does), else until the soonest stamp not yet held for the hold will have
    /// been; `None` with no stamp at all. A young region's walk by slot ages may start again a
    /// hold after the last began, and its stamp, a hold old at most, wakes the thread by then.
    fn hold_pending(&self, count: u32) -> Option<u32> {
        let plan = self.space.plan();
        (0..self.walk_regions())
            .filter_map(|index| {
                let region = plan.region(index)?;
                let class = self.state.tags.published(region)?;
                if class.is_small() {
                    return None;
                }
                let host = self.host(class)?;
                let stamp = host.meta(region, host.layout().hold_word()?).load();
                (stamp != 0).then_some(())?;
                let age = count.wrapping_sub(stamp as u32);
                if age < HOLD_STEPS {
                    Some(HOLD_STEPS - age)
                } else if age.wrapping_neg() <= HOLD_STEPS {
                    // Ahead of the count: a free that read the count after it stepped.
                    Some(HOLD_STEPS + age.wrapping_neg())
                } else {
                    // Held already, and the looks have not walked it to its bottom: a look
                    // that stopped at its bound of bitmap, or at the floor. The next look is
                    // at the next step, as a heartbeat's would be.
                    Some(1)
                }
            })
            .min()
    }

    /// One of `maintain`'s steps of the hold: reads the counted time through `now`, which may
    /// step the fine count, and takes the look if the count has moved since the last one (or
    /// that one left more to do), with the hold `maintain_hold` gives. Says what the look
    /// returned, and whether it stopped at its bound with more over the floor, so that another
    /// step would return more.
    pub fn hold_maintain_step(&self, now: &dyn Fn() -> u64) -> PassStep {
        let hold = &self.state.hold;
        hold.fine.note(now());
        let count = hold.fine.count();
        if hold.looked.load(Relaxed) == count {
            return PassStep::default();
        }
        self.hold_take_look(count, |back| {
            Reach::maintain(back.space.plan().geometry(), back.maintain_hold(count))
        })
    }

    /// Takes the look at fine count `count`, unless another caller has, with the reach `reach`
    /// gives once it is taken; if the look stops at a bound with more to return, leaves it for
    /// the next caller to carry on. What the look did, if it took it.
    fn hold_take_look(&self, count: u32, reach: impl FnOnce(&Self) -> Reach) -> PassStep {
        let looked = &self.state.hold.looked;
        let seen = looked.load(Relaxed);
        // One look a step: a caller that loses the race leaves it to the winner.
        if seen == count
            || looked
                .compare_exchange(seen, count, Relaxed, Relaxed)
                .is_err()
        {
            return PassStep::default();
        }
        let step = self.hold_look(count, reach(self));
        if step.more {
            let _ = looked.compare_exchange(count, count.wrapping_sub(1), Relaxed, Relaxed);
        }
        step
    }

    /// The hold for `maintain`'s look at fine count `count`, decided at a round's first look.
    /// `maintain` runs a round once a counted interval whether or not heartbeats run, and in a
    /// silence its own readings are what step the fine count, about one step a round, so a
    /// hold of [`HOLD_STEPS`] would last a minute there. So where the count has moved no more
    /// than [`QUIET_STEPS`] since the first look of the round before, which only `maintain`'s
    /// readings do, a round takes the steps since plus one: a stamp that old was written before
    /// the reading that stepped the count to the one that look saw, so before that round, an
    /// interval or more ago. Where heartbeats step the count, it takes the hold: a round that
    /// began a few steps after the last (one that ran on into the next counted interval, where
    /// rounds are counted) is not taken for a silence. The first round takes the hold. Every
    /// look of a round takes what its first did.
    fn maintain_hold(&self, count: u32) -> u32 {
        let hold = &self.state.hold;
        let round = self.state.decay.clock.count().wrapping_add(1);
        let seen = hold.maintained.load(Relaxed);
        if (seen >> 32) as u32 == round && round != 0 {
            return seen as u32;
        }
        let last = hold.maintain_looked.load(Relaxed);
        let since = count.wrapping_sub(last.wrapping_sub(1));
        let steps = if last != 0 && since <= QUIET_STEPS {
            since + ROUNDS_BACK
        } else {
            HOLD_STEPS
        };
        hold.maintained
            .store(u64::from(round) << 32 | u64::from(steps), Relaxed);
        hold.maintain_looked.store(count.wrapping_add(1), Relaxed);
        steps
    }

    /// One look at fine count `count`: over the floor, walks the large classes' regions from
    /// where the last look stopped and purges what `reach` allows (see the module
    /// documentation). Returns what it returned, and whether it stopped at a bound with more
    /// over the floor after returning something or while walking a young region's free slots
    /// by their own ages, so that the next caller carries on at once: a walk then takes the
    /// looks it needs one after another, not one a fine step.
    #[cold]
    #[inline(never)]
    fn hold_look(&self, count: u32, reach: Reach) -> PassStep {
        // The tallies are read once a step: frees and claims write their lines.
        let over = self.retained_large().saturating_sub(self.hold_floor());
        if over == 0 {
            return PassStep::default();
        }
        let (goal, budget) = (over.min(reach.bytes), reach.words);
        let top = self.walk_regions();
        let at = &self.state.hold.at;
        let seen = at.load(Relaxed) as usize;
        // The position is the region to look at next plus one.
        let mut next = if seen == 0 || seen > top { top } else { seen };
        let (mut spent, mut purged) = (0, 0);
        let (mut stopped, mut walked) = (false, false);
        for _ in 0..top {
            if spent >= budget || purged >= goal {
                stopped = true;
                break;
            }
            let index = next - 1;
            let (read, bytes, done, aged) =
                self.hold_region(index, count, goal - purged, budget - spent, reach);
            spent += read.max(1);
            purged += bytes;
            walked |= aged;
            if !done {
                stopped = true;
                break;
            }
            next = if index == 0 { top } else { index };
        }
        // Once a look, not once a region: every large free reads the fine count on this line.
        if top != 0 && next != seen {
            at.store(next as u32, Relaxed);
        }
        PassStep {
            more: stopped && purged < over && (purged != 0 || walked),
            returned: purged,
        }
    }

    /// The look's work on region `index`: if it is a large class's and its stamp is one
    /// `reach` takes, purges its free units from its cursor down, within `budget` words and
    /// `limit` bytes. A region walked from its top to its bottom loses its stamp, unless a free
    /// has stamped it since. A region whose stamp is young has its free slots walked by their
    /// own ages instead (`walk_aged`). Returns the words read, the bytes purged, whether the
    /// region is done, and whether it was walked by slot ages.
    pub(super) fn hold_region(
        &self,
        index: usize,
        count: u32,
        limit: usize,
        budget: usize,
        reach: Reach,
    ) -> (usize, usize, bool, bool) {
        let passed = (1, 0, true, false);
        let Some(region) = self.space.plan().region(index) else {
            return passed;
        };
        // The class first: a small class's region has no stamp to read.
        let Some(class) = self.state.tags.published(region) else {
            return passed;
        };
        if class.is_small() {
            return passed;
        }
        let Some(host) = self.host(class) else {
            return passed;
        };
        let Some(at) = host.layout().hold_word() else {
            return passed;
        };
        let word = host.meta(region, at);
        let stamp = word.load();
        if stamp == 0 {
            return passed;
        }
        if !held_for(stamp, count, reach.steps) {
            // Frees keep the region young. A region with nothing free has nothing to walk; it
            // keeps its stamp, which the next walk from its top clears once it is old.
            if self.purgeable(region).is_none() {
                return passed;
            }
            let aged = walk_aged(&host, region, count, reach.steps, limit, budget);
            self.forget_outgrown(aged.outgrown);
            self.unretain(class, aged.returned);
            self.count_returned(aged.returned);
            #[cfg(test)]
            self.state
                .hold
                .aged_returned
                .fetch_add(aged.returned, Relaxed);
            return (aged.read, aged.returned, aged.done, aged.walked);
        }
        if self.purgeable(region).is_none() {
            // Nothing in it is free: what was freed has been taken again.
            let _ = word.compare_exchange(stamp, 0);
            return passed;
        }
        // A walk that starts below the top leaves the units above the cursor unlooked at, where
        // a slot freed since the cursor passed may lie, so only a walk that began at the top
        // clears the stamp when it reaches the bottom, in this look or a later one. A free
        // since rewrites the stamp, and the compare-and-swap then fails.
        let mut stamp = stamp;
        if stamp & WALKED_FROM_TOP == 0
            && Hints::unpack(host.hints(region).load()).cursor == 0
            && word
                .compare_exchange(stamp, stamp | WALKED_FROM_TOP)
                .is_ok()
        {
            stamp |= WALKED_FROM_TOP;
        }
        // A large class has no huge-page part, so nothing here is split.
        let (bytes, read, done, _) = self.purge_region(
            class,
            region,
            budget,
            limit,
            true,
            Scope::All,
            false,
            Splits::One,
        );
        if done && stamp & WALKED_FROM_TOP != 0 {
            let _ = word.compare_exchange(stamp, 0);
        }
        (read, bytes, done, false)
    }
}

/// What tests look at and set.
#[cfg(test)]
impl<O: Os> Back<'_, O> {
    /// The stamp of the large-class region that holds `ptr`.
    pub(super) fn hold_stamp_at(&self, ptr: *const u8) -> u64 {
        let location = self.space.locate(ptr).unwrap();
        let region = location.region;
        let crate::space::RegionTag::Class(class) = self.state.tags.at(location) else {
            panic!("not a class region");
        };
        let host = self.host(class).unwrap();
        host.meta(region, host.layout().hold_word().unwrap()).load()
    }

    /// The fine count.
    pub(super) fn hold_count(&self) -> u32 {
        self.state.hold.fine.count()
    }

    /// Sets the fine count, as if it had stepped there at counted time zero.
    pub(super) fn hold_set_count(&self, count: u32) {
        self.state.hold.fine.word.store(pack(count, 0), Relaxed);
    }

    /// The thread's next look (`Hold::next`).
    pub(super) fn hold_next(&self) -> u64 {
        self.state.hold.next.load(SeqCst)
    }

    /// Sets the thread's next look.
    pub(super) fn hold_set_next(&self, next: u64) {
        self.state.hold.next.store(next, SeqCst);
    }

    /// The fine count at the last look (one less when it left more to do).
    pub(super) fn hold_looked(&self) -> u32 {
        self.state.hold.looked.load(Relaxed)
    }

    /// Where the next look's walk starts.
    pub(super) fn hold_at(&self) -> u32 {
        self.state.hold.at.load(Relaxed)
    }

    /// What a heartbeat that took the look at counted time `time` and then vanished (a fork
    /// leaves it so in the child) leaves behind.
    pub(super) fn hold_take_and_vanish(&self, time: u64) {
        let hold = &self.state.hold;
        hold.fine.note(time);
        hold.looked.store(hold.fine.count(), Relaxed);
    }
}
