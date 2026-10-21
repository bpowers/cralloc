//! The hold on free large slots: they go back to the OS once their region has had no free
//! that left a slot resident for a short hold, down to a floor.
//!
//! Address space is divided by class, so memory freed in one large class serves another only
//! by way of the OS. A program whose phases use different large classes (a query engine whose
//! next query's blocks fall in other classes, a server whose request mix changes) otherwise
//! holds every phase's free large slots on top of the next phase's live ones, until the
//! clocked pass, a second's step, returns them (`docs/research/memory.md`, "DataFusion's peak
//! against jemalloc's"). A free large slot is reused within tens of milliseconds when it is
//! reused at all ("Faults and system calls in real programs": 86 to 100% of the slots taken
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
//! bounded walk each look and still gets to them. A region the look walked from its top to its
//! bottom loses its stamp, so later looks pass it at a load until the next free.
//!
//! **Maintain.** `maintain`'s round takes looks too, before its clocked pass, a step at a
//! time within the bounds of that pass's steps, until one returns nothing or the floor is
//! reached. It runs once a counted interval whether heartbeats run or not, and in a silence its
//! own readings are what step the fine count, once an interval, so its looks take a stamp as
//! old once it is from before the last look's count (`Back::maintain_hold`): a quiet program's
//! background thread or task returns its held large slots within two rounds, while in a busy
//! program `maintain` holds them as the heartbeats do.
//!
//! **How long.** One hold for every large class, [`HOLD_STEPS`]. A hold that adapted per
//! class (doubling when a slot was claimed soon after the hold purged its region, halving on
//! the clock) held DataFusion's peak at the same level for 9% more faults, so it was not kept
//! (`docs/research/large-hold.md`).
//!
//! **The words.** The fine count, and the count of the last look and the walk's position, on
//! the hold's line in the back end's state. A stamp per large-class region, in the header line
//! of its metadata block, which large allocations and frees read already (`layout`): a free
//! loads the fine count and its region's stamp, and stores the stamp only when it differs, so
//! once per region per fine step at most. A small class's region has no stamp. All of these
//! are hints. A stale stamp, a lost look or a lost position returns a slot early or late, never
//! one in use, because a purge is a claim (`region::purge_unit`); a fork that leaves a look
//! half done leaves nothing the next step does not take up. Stamps compare with the fine count
//! by wrapping difference: one up to a hold ahead of the count is a free that read it after
//! the look did, and is fresh; any other at least a hold behind is old, so a stamp left over
//! for half the count's range (at a step every 2 ms, about 50 days) reads as old, not as one
//! from the future; one left for nearly the whole range reads as fresh for a hold.

use super::Back;
use super::host::ClassHost;
use super::layout::Hints;
use super::policy::{
    self, HOLD_STEPS, PURGE_MAINTAIN_STEP_PAGES, PURGE_MAINTAIN_STEP_WORDS, PURGE_STEP_WORDS,
    allowance_floor,
};
use super::purge::Scope;
use super::tree::Host;
use super::word::Word;
use crate::os::Os;
use crate::space::{Geometry, RegionId, RegionTag};
use core::sync::atomic::Ordering::Relaxed;
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
        (self.word.load(Relaxed) >> 32) as u32
    }

    /// Advances the count for a reading of the counted time `time` (at most one step).
    fn note(&self, time: u64) {
        let seen = self.word.load(Relaxed);
        if let Some(next) = fine_read(seen, time) {
            // A failure means another reader stepped or re-stamped, which is the same reading
            // or a later one: nothing to retry.
            let _ = self.word.compare_exchange(seen, next, Relaxed, Relaxed);
        }
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
    /// `maintain`'s last look: the fine count it looked at plus one (zero for none) in the
    /// high half, and the hold it took, in fine steps, in the low half (`Back::maintain_hold`).
    maintained: AtomicU64,
}

impl Hold {
    pub(crate) const fn new() -> Hold {
        Hold {
            fine: Fine::new(),
            looked: AtomicU32::new(0),
            at: AtomicU32::new(0),
            maintained: AtomicU64::new(0),
        }
    }
}

/// A region's stamp for fine count `count`: the count, with a bit above it so that no stamp
/// is zero, which is a region with no stamp.
const fn stamp_of(count: u32) -> u64 {
    1 << 32 | count as u64
}

/// Whether a region stamped `stamp` has been held for `steps` at fine count `count`: the count
/// is at least `steps` past the stamp, and the stamp is not up to a hold ahead of the count,
/// which a free that read the count after the look did writes.
pub(crate) const fn held_for(stamp: u64, count: u32, steps: u32) -> bool {
    let age = count.wrapping_sub(stamp as u32);
    age >= steps && age.wrapping_neg() > HOLD_STEPS
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
        let stamp = stamp_of(self.state.hold.fine.count());
        if word.load() != stamp {
            word.store(stamp);
        }
    }

    /// A heartbeat's part of the hold: advances the fine count for the reading of the counted
    /// time `time`, and takes the look if the count has moved since the last one (or that one
    /// left more to do). Between steps, two loads from the hold's line.
    pub(super) fn hold_beat(&self, time: u64) {
        let hold = &self.state.hold;
        hold.fine.note(time);
        let count = hold.fine.count();
        if hold.looked.load(Relaxed) == count {
            return;
        }
        self.hold_take_look(count, |back| Reach::beat(back.space.plan().geometry()));
    }

    /// One of `maintain`'s steps of the hold: reads the counted time through `now`, which may
    /// step the fine count, and takes the look if the count has moved since the last one (or
    /// that one left more to do), with the hold `maintain_hold` gives. Whether the look returned
    /// something and stopped at its bound with more over the floor, so that another step would
    /// return more.
    pub fn hold_maintain_step(&self, now: &dyn Fn() -> u64) -> bool {
        let hold = &self.state.hold;
        hold.fine.note(now());
        let count = hold.fine.count();
        if hold.looked.load(Relaxed) == count {
            return false;
        }
        self.hold_take_look(count, |back| {
            Reach::maintain(back.space.plan().geometry(), back.maintain_hold(count))
        })
    }

    /// Takes the look at fine count `count`, unless another caller has, with the reach `reach`
    /// gives once it is taken; if the look stops at a bound with more to return, leaves it for
    /// the next caller to carry on. Whether it did.
    fn hold_take_look(&self, count: u32, reach: impl FnOnce(&Self) -> Reach) -> bool {
        let looked = &self.state.hold.looked;
        let seen = looked.load(Relaxed);
        // One look a step: a caller that loses the race leaves it to the winner.
        if seen == count
            || looked
                .compare_exchange(seen, count, Relaxed, Relaxed)
                .is_err()
        {
            return false;
        }
        let more = self.hold_look(count, reach(self));
        if more {
            let _ = looked.compare_exchange(count, count.wrapping_sub(1), Relaxed, Relaxed);
        }
        more
    }

    /// The hold for `maintain`'s look at fine count `count`. `maintain` runs once a counted
    /// interval whether or not heartbeats run, and in a silence its readings are what step the
    /// fine count, one step an interval, so a hold of [`HOLD_STEPS`] would last a minute there.
    /// So it takes the smaller of the hold and the steps since its last look plus one: a stamp
    /// that old was written before the reading that stepped the count to the one that look
    /// saw, so before that look, an interval or more ago in a silence. Where heartbeats step
    /// the count, the steps since its last look are many, and it takes the hold. The first look
    /// takes the hold. Looks that carry one on at the same count take what it took.
    fn maintain_hold(&self, count: u32) -> u32 {
        let word = &self.state.hold.maintained;
        let seen = word.load(Relaxed);
        let (last, steps) = ((seen >> 32) as u32, seen as u32);
        let here = count.wrapping_add(1);
        if last == here && last != 0 {
            return steps;
        }
        let steps = if last == 0 {
            HOLD_STEPS
        } else {
            count.wrapping_sub(last).wrapping_add(2).min(HOLD_STEPS)
        };
        word.store(u64::from(here) << 32 | u64::from(steps), Relaxed);
        steps
    }

    /// One look at fine count `count`: over the floor, walks the large classes' regions from
    /// where the last look stopped and purges what `reach` allows (see the module
    /// documentation). Returns whether it stopped at a bound after returning something, with
    /// more over the floor.
    #[cold]
    #[inline(never)]
    fn hold_look(&self, count: u32, reach: Reach) -> bool {
        let geometry = self.space.plan().geometry();
        // The tallies are read once a step: frees and claims write their lines.
        let over = self
            .retained_large()
            .saturating_sub(allowance_floor(geometry));
        if over == 0 {
            return false;
        }
        let (goal, budget) = (over.min(reach.bytes), reach.words);
        let top = self.walk_regions();
        let at = &self.state.hold.at;
        let seen = at.load(Relaxed) as usize;
        // The position is the region to look at next plus one.
        let mut next = if seen == 0 || seen > top { top } else { seen };
        let (mut spent, mut purged) = (0, 0);
        let mut stopped = false;
        for _ in 0..top {
            if spent >= budget || purged >= goal {
                stopped = true;
                break;
            }
            let index = next - 1;
            let (read, bytes, done) =
                self.hold_region(index, count, goal - purged, budget - spent, reach);
            spent += read.max(1);
            purged += bytes;
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
        stopped && purged != 0 && purged < over
    }

    /// The look's work on region `index`: if it is a large class's and its stamp is one
    /// `reach` takes, purges its free units from its cursor down, within `budget` words and
    /// `limit` bytes. A region walked from its top to its bottom loses its stamp, unless a free
    /// has stamped it since. Returns the words read, the bytes purged, and whether the region
    /// is done.
    pub(super) fn hold_region(
        &self,
        index: usize,
        count: u32,
        limit: usize,
        budget: usize,
        reach: Reach,
    ) -> (usize, usize, bool) {
        let Some(region) = self.space.plan().region(index) else {
            return (1, 0, true);
        };
        // The tag first: a small class's region has no stamp to read.
        let RegionTag::Class(class) = self.state.table.get(region) else {
            return (1, 0, true);
        };
        if class.is_small() {
            return (1, 0, true);
        }
        let Some(host) = self.host(class) else {
            return (1, 0, true);
        };
        let Some(at) = host.layout().hold_word() else {
            return (1, 0, true);
        };
        let word = host.meta(region, at);
        let stamp = word.load();
        if stamp == 0 || !held_for(stamp, count, reach.steps) {
            return (1, 0, true);
        }
        if self.purgeable(region).is_none() {
            // Nothing in it is free: what was freed has been taken again.
            let _ = word.compare_exchange(stamp, 0);
            return (1, 0, true);
        }
        // A walk that starts below the top leaves the units above the cursor unlooked at,
        // where a slot freed since the cursor passed may lie: the stamp stays for the next.
        let whole = Hints::unpack(host.hints(region).load()).cursor == 0;
        let (bytes, read, done) = self.purge_region(class, region, budget, limit, true, Scope::All);
        if done && whole {
            let _ = word.compare_exchange(stamp, 0);
        }
        (read, bytes, done)
    }
}

/// What tests look at and set.
#[cfg(test)]
impl<O: Os> Back<'_, O> {
    /// The stamp of the large-class region that holds `ptr`.
    pub(super) fn hold_stamp_at(&self, ptr: *const u8) -> u64 {
        let region = self.space.locate(ptr).unwrap().region;
        let RegionTag::Class(class) = self.state.table.get(region) else {
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
