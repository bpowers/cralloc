//! Retained memory, its caps, the clocked decay pass and the purge walk.
//!
//! **Retained memory** is the back end's estimate of the bytes of free slots in its bitmaps
//! that are resident, kept as two counts: small classes and large ones. A launder or a free
//! raises a count by what it returned. Only two things lower it: reuse (a claim subtracts what
//! it took, less what the clean bits say was not resident) and purging (the pages returned
//! that were not already clean). A claim that makes a purged page resident again adds back the
//! rest of that page.
//!
//! The counts are signed and changed by plain atomic adds and subtracts, and read as zero when
//! negative. A subtraction can run before the addition it answers (a claim takes a slot between
//! another thread's release of it and that thread's add); a count that stopped at zero would
//! lose the subtraction and keep the addition, and drift upward for ever.
//!
//! The counts are never reset. Free slots on pages that also hold live ones cannot be purged
//! and stay counted for as long as they are free. An earlier version set the count to zero
//! when a whole pass purged nothing, taking what was left for fragmentation. That is wrong in
//! a storm of frees: every page still has a live neighbour while a pass runs, the pages
//! become wholly free a moment later, and their bytes are then in no count, so the cap never
//! acts again (`docs/research/memory.md`: 136 MiB free and resident under a 64 MiB cap).
//!
//! **Decay** (`decay_step`, one call per heartbeat). Once an interval of the clock
//! (`policy::DECAY_TICKS`) a pass starts at the top of the heap with a goal: a quarter of the
//! retained counts (`policy::decay_goal`), or everything above the small-class cap if that is
//! more. Each heartbeat then does one bounded step of the walk until the goal is met or the
//! walk has gone once round. The goal is in bytes and the walk stops within one unit of it,
//! whatever the slot size. Fills take the lowest addresses, so what is being reused sits at
//! the bottom and the top-down pass mostly leaves it alone; what stays free is returned over
//! several intervals. A heap whose free memory is all fragmentation costs one walk of its
//! bitmaps an interval, in steps, and nothing between passes. A clock that has gone back
//! (a resume, a restored checkpoint) restarts the interval from the new time, so decay waits
//! at most one interval for it.
//!
//! A pass is two words. The decay word holds the time the last pass ended, or the time the
//! pass under way began with its low bit set; one compare-and-swap starts a pass and one ends
//! it. The pass word holds the pass's remaining goal and the lap it began in, under a tag
//! derived from the decay word, and is written once, after the walk's position is reset. A
//! stepper acts only on a pass word whose tag matches the decay word it read, so it never
//! steps a new pass with the last one's goal, and ends only the pass it saw.
//!
//! **The caps** act at the free, for bursts inside an interval.
//!
//! - Small classes (`policy::retained_cap`): a launder purges toward the cap, within
//!   `policy::PURGE_CAP_WORDS` of bitmap and one circuit of the heap from where the walk
//!   stands, when it leaves the count over the cap
//!   and at least `cap / policy::CAP_REARM` above the mark. After a purge that could not get
//!   under the cap, the mark is the lowest the count has been since, so a count that
//!   fragmentation holds at or over the cap acts once and then only after that much more is
//!   freed. After a purge that did get under, the next launder past the cap acts again.
//! - Large classes (`policy::large_retained_cap`): a free that would take the count over the
//!   cap returns the slot's pages to the OS before it releases the slot (`Back::free`). The
//!   freeing thread still holds the slot, so this needs no claim and no search.
//!
//! **The walk** goes over the regions from the highest address down, skipping regions whose
//! class summary bit says nothing in them is free. Within a region it walks units of pages
//! from the watermark down (`Layout::unit_below`): whole pages in the small-page part, whole
//! huge pages in the huge-page part, so a huge page is never split. At the top of a region
//! with a huge-page part it first carves the rest of the unit that holds the watermark
//! (`region::top_up`), so that the part of that huge page the fault made resident beyond the
//! watermark can go with the rest. Each region keeps its own cursor in its `Hints`, and the
//! walk over regions is one word in the state. A count of completed laps lets a decay pass
//! see that the walk has gone round even when another caller took the step that finished the
//! lap. All of these are hints: two purgers on the same unit are safe because each purge is a
//! claim (`region::purge_unit`), and a lost update only moves a purge earlier or later.
//!
//! **Trim** purges every unit of every region, ignoring the caps, the clock and the cursors.

use super::Back;
use super::layout::{Hints, Step};
use super::policy::{
    CAP_REARM, DECAY_TICKS, PURGE_CAP_WORDS, PURGE_STEP_WORDS, decay_goal, retained_cap,
};
use super::region::{self, Purge};
use super::tree::Host;
use super::word::{BITS, Word};
use crate::classes::Class;
use crate::os::Os;
use crate::space::{RegionId, RegionTag};
use core::sync::atomic::AtomicIsize;
use core::sync::atomic::Ordering::{Acquire, Relaxed, Release};

/// The decay word's low bit: a pass is under way. The rest is the time the pass began, or the
/// time the last one ended.
const ACTIVE: u64 = 1;

/// What a decay word and the clock say to do.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Decay {
    /// Nothing: the last pass ended less than an interval ago.
    Wait,
    /// The clock is behind the word: store this word, the time now, so that the interval
    /// runs from here.
    Restamp(u64),
    /// Start a pass: store this word, if the word is still the one read.
    Start(u64),
    /// A pass is under way: take a step of it.
    Step,
}

/// The decision for a decay word read as `word` with the clock at `now`.
pub(crate) const fn decay_look(word: u64, now: u64) -> Decay {
    if word & ACTIVE != 0 {
        return Decay::Step;
    }
    let now = now & !ACTIVE;
    let since = now.wrapping_sub(word);
    if since >= 1 << 63 {
        Decay::Restamp(now)
    } else if since >= DECAY_TICKS {
        Decay::Start(now | ACTIVE)
    } else {
        Decay::Wait
    }
}

/// A pass word: what the steppers of one decay pass share.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Pass {
    /// Names the pass: [`Pass::tag_of`] the decay word that started it.
    pub(crate) tag: u16,
    /// The low bits of the lap count when the pass began; the pass ends when they differ.
    pub(crate) lap: u16,
    /// Pages the pass may still purge.
    pub(crate) pages: u32,
}

impl Pass {
    /// The tag of the pass a decay word started: every part of the word folded together, so
    /// two passes share a tag only by a one-in-65536 accident. The pass word a stepper then
    /// mistakes for the new pass's is the last pass's, whose goal is met or whose lap is over,
    /// so the stepper ends the new pass before it has purged anything: one pass is lost, and
    /// the next interval starts another.
    pub(crate) const fn tag_of(decay: u64) -> u16 {
        (decay ^ decay >> 16 ^ decay >> 32 ^ decay >> 48) as u16
    }

    pub(crate) const fn unpack(word: u64) -> Pass {
        Pass {
            tag: (word >> 48) as u16,
            lap: (word >> 32) as u16,
            pages: word as u32,
        }
    }

    pub(crate) const fn pack(self) -> u64 {
        (self.tag as u64) << 48 | (self.lap as u64) << 32 | self.pages as u64
    }
}

/// Whether a launder that leaves the small-class count at `new` should purge: the count is
/// over the cap and has risen a re-arm step above `mark`, the lowest it has been since the cap
/// last acted.
pub(crate) const fn cap_fires(mark: usize, new: usize, cap: usize) -> bool {
    let low = if mark < new { mark } else { new };
    new > cap && new - low >= cap / CAP_REARM
}

/// A count read for use: zero when a subtraction has run ahead of its addition.
fn clamped(count: &AtomicIsize) -> usize {
    count.load(Relaxed).max(0) as usize
}

impl<O: Os> Back<'_, O> {
    fn count(&self, class: Class) -> &AtomicIsize {
        if class.is_small() {
            &self.state.retained_small
        } else {
            &self.state.retained_large
        }
    }

    /// Bytes of free slots in the bitmaps that are believed to be resident, small and large
    /// classes together: what decays.
    #[inline]
    pub fn retained(&self) -> usize {
        self.retained_small().saturating_add(self.retained_large())
    }

    /// The part of [`Back::retained`] in small classes, which the small-class cap limits.
    #[inline]
    pub fn retained_small(&self) -> usize {
        clamped(&self.state.retained_small)
    }

    /// The part of [`Back::retained`] in large classes, which the large-class cap limits.
    #[inline]
    pub fn retained_large(&self) -> usize {
        clamped(&self.state.retained_large)
    }

    /// Counts `bytes` of slots of `class` freed to the bitmaps as retained. For a small class,
    /// purges toward the cap if the count is now over it and a step above the mark. A large
    /// class's cap is `Back::free`'s business, which acts before the slot is released.
    pub(super) fn retain(&self, class: Class, bytes: usize) {
        let old = self.count(class).fetch_add(bytes as isize, Relaxed).max(0) as usize;
        if !class.is_small() {
            return;
        }
        let state = self.state;
        let cap = retained_cap(self.space.plan().geometry());
        let mut mark = state.cap_mark.load(Relaxed);
        if old < mark {
            state.cap_mark.store(old, Relaxed);
            mark = old;
        }
        let new = old.saturating_add(bytes);
        if !cap_fires(mark, new, cap) {
            return;
        }
        // Raise the mark first, so that launders racing this one do not all purge. The mark
        // is read and written in separate steps here and above, so racing launders can both
        // purge, or one can lower the mark the other just raised; either costs one bounded
        // purge more or one later, never a count that goes unwatched.
        state.cap_mark.store(new, Relaxed);
        // One circuit of the heap from wherever the shared walk stands, counted in regions
        // finished: the lap count says nothing here, since the walk may stand one region from
        // the bottom with everything this launder freed above it.
        let regions = self.space.plan().objects();
        let (mut spent, mut finished) = (0, 0);
        loop {
            let over = self.retained_small().saturating_sub(cap);
            if over == 0 || spent >= PURGE_CAP_WORDS || finished >= regions {
                break;
            }
            let (read, _, done) = self.purge_step(PURGE_CAP_WORDS - spent, over);
            spent += read;
            finished += usize::from(done);
        }
        // If that brought the count under the cap, the cap is working and acts again on the
        // next launder that passes it: the mark goes a step below the count. If it could
        // not (what is left is fragmentation, or beyond the bound), the mark is the count,
        // and the next purge waits for a step above it.
        let left = self.retained_small();
        let mark = if left <= cap {
            left.saturating_sub(cap / CAP_REARM)
        } else {
            left
        };
        state.cap_mark.store(mark, Relaxed);
    }

    /// Counts `bytes` of free slots of `class` as retained without acting on a cap: free
    /// memory that became resident by something other than a free.
    pub(super) fn revive(&self, class: Class, bytes: usize) {
        if bytes != 0 {
            self.count(class).fetch_add(bytes as isize, Relaxed);
        }
    }

    /// Subtracts `bytes` of slots of `class`, claimed while resident or purged, from the
    /// count.
    pub(super) fn unretain(&self, class: Class, bytes: usize) {
        if bytes != 0 {
            self.count(class).fetch_sub(bytes as isize, Relaxed);
        }
    }

    /// The maintenance a slow path does on its way out: one step of decay.
    pub fn heartbeat(&self) {
        self.decay_step();
    }

    /// One heartbeat's part of the clocked pass: starts a pass if an interval has gone by
    /// since the last ended and anything is retained, and takes one bounded step of the pass
    /// under way.
    fn decay_step(&self) {
        let state = self.state;
        let geometry = self.space.plan().geometry();
        let page = geometry.page_size();
        let mut seen = state.decay.load(Acquire);
        match decay_look(seen, self.space.os().ticks()) {
            Decay::Wait => return,
            Decay::Restamp(word) => {
                let _ = state.decay.compare_exchange(seen, word, Release, Relaxed);
                return;
            }
            Decay::Start(word) => {
                let small = self.retained_small();
                let retained = small.saturating_add(self.retained_large());
                if retained == 0 {
                    return;
                }
                if state
                    .decay
                    .compare_exchange(seen, word, Release, Relaxed)
                    .is_err()
                {
                    return;
                }
                let over = small.saturating_sub(retained_cap(geometry));
                let goal = decay_goal(geometry, retained).max(over);
                // From the top. The pass word is written last and once: until then no
                // stepper finds its tag and none acts.
                state.purge_at.store(0, Relaxed);
                let pass = Pass {
                    tag: Pass::tag_of(word),
                    lap: state.laps.load(Relaxed) as u16,
                    pages: goal.div_ceil(page).clamp(1, u32::MAX as usize) as u32,
                };
                state.pass.store(pass.pack(), Release);
                seen = word;
            }
            Decay::Step => {}
        }
        let tag = Pass::tag_of(seen);
        let mut spent = 0;
        loop {
            let pass = Pass::unpack(state.pass.load(Acquire));
            if pass.tag != tag {
                // The starter has not published this pass yet, or it is over and another
                // has begun: either way it is not this stepper's. But a starter that vanished
                // between its two writes (a fork abandons it there) never publishes, and
                // only steppers end passes. So a pass that is still unpublished an interval
                // after it began is ended here, and the next interval starts a fresh one. A
                // clock behind the start re-stamps it, so that its age can be told.
                let now = self.space.os().ticks() & !ACTIVE;
                let age = now.wrapping_sub(seen & !ACTIVE);
                if age >= 1 << 63 {
                    let _ = state
                        .decay
                        .compare_exchange(seen, now | ACTIVE, Release, Relaxed);
                } else if age >= DECAY_TICKS {
                    let _ = state.decay.compare_exchange(seen, now, Release, Relaxed);
                }
                return;
            }
            if pass.pages == 0 || state.laps.load(Relaxed) as u16 != pass.lap {
                // The goal is met or the walk has gone round. End the pass this thread saw,
                // and only that one.
                let now = self.space.os().ticks() & !ACTIVE;
                let _ = state.decay.compare_exchange(seen, now, Release, Relaxed);
                return;
            }
            if spent >= PURGE_STEP_WORDS {
                return;
            }
            let limit = pass.pages as usize * page;
            let (read, purged, _) = self.purge_step(PURGE_STEP_WORDS - spent, limit);
            spent += read;
            let done = (purged / page).min(u32::MAX as usize) as u32;
            if done != 0 {
                // Lower this pass's goal, and no other's.
                let mut word = state.pass.load(Acquire);
                loop {
                    let now = Pass::unpack(word);
                    if now.tag != tag {
                        break;
                    }
                    let next = Pass {
                        pages: now.pages.saturating_sub(done),
                        ..now
                    };
                    match state
                        .pass
                        .compare_exchange(word, next.pack(), Release, Acquire)
                    {
                        Ok(_) => break,
                        Err(found) => word = found,
                    }
                }
            }
        }
    }

    /// Returns every wholly free page the bitmaps hold to the OS, for `malloc_trim`. Slots
    /// cached in magazines are not in the bitmaps; launder them first. Returns the bytes
    /// purged that had been resident.
    pub fn trim(&self) -> usize {
        let plan = self.space.plan();
        let mut purged = 0;
        for region in (0..plan.objects()).rev().filter_map(|i| plan.region(i)) {
            if let Some(class) = self.purgeable(region) {
                purged += self
                    .purge_region(class, region, usize::MAX, usize::MAX, false)
                    .0;
            }
        }
        purged
    }

    /// One step of the walk: works on the region at the walk's position, and moves the
    /// position on if that region is done. Stops after `budget` words of bitmap or once it
    /// has purged `limit` bytes, finishing the unit it is on. Returns the words of bitmap
    /// read, at least one, the bytes purged, and whether it finished its region.
    fn purge_step(&self, budget: usize, limit: usize) -> (usize, usize, bool) {
        let plan = self.space.plan();
        let seen = self.state.purge_at.load(Relaxed);
        // The position is the region to look at next plus one; zero means the top.
        let index = if seen == 0 || seen as usize > plan.objects() {
            plan.objects() - 1
        } else {
            seen as usize - 1
        };
        let Some(region) = plan.region(index) else {
            return (1, 0, true);
        };
        let (purged, read, done) = match self.purgeable(region) {
            Some(class) => self.purge_region(class, region, budget, limit, true),
            None => (0, 1, true),
        };
        if done
            && self
                .state
                .purge_at
                .compare_exchange(seen, index as u32, Relaxed, Relaxed)
                .is_ok()
            && index == 0
        {
            self.state.laps.fetch_add(1, Relaxed);
        }
        (read.max(1), purged, done)
    }

    /// The class of a region the walk should look at: a class region whose summary bit is set.
    fn purgeable(&self, region: RegionId) -> Option<Class> {
        let RegionTag::Class(class) = self.state.table.get(region) else {
            return None;
        };
        let host = self.host(class)?;
        let index = region.index() as u32;
        let bit = 1u64 << (index % BITS);
        if host.class_word(index / BITS).load() & bit == 0 {
            return None;
        }
        Some(class)
    }

    /// Purges units of a region from the top down until the region is done, `budget` words
    /// have been read or `limit` bytes purged. It always looks at one unit at least, so a
    /// call moves the cursor whatever the look at the watermark's page cost. With `resume`,
    /// starts at the region's cursor and keeps it up to date; without, walks the whole region
    /// and leaves the cursor alone. Returns the bytes purged, the words read, and whether the
    /// walk reached the bottom.
    fn purge_region(
        &self,
        class: Class,
        region: RegionId,
        budget: usize,
        limit: usize,
        resume: bool,
    ) -> (usize, usize, bool) {
        let Some(host) = self.host(class) else {
            return (0, 1, true);
        };
        let layout = *host.layout();
        let hints = host.meta(region, layout.hints_word());
        let grow = host.meta(region, layout.grow_word());
        let mut cursor = if resume {
            Hints::unpack(hints.load()).cursor
        } else {
            0
        };
        let (mut purged, mut read) = (0, 0);
        if cursor == 0 {
            // At the top of the region: bring the watermark up to the end of its huge page,
            // if that page could then be purged whole.
            let (bytes, words) = region::top_up(&host, region);
            self.revive(class, bytes);
            read += words;
        }
        let mut first = true;
        let done = loop {
            // Not before the first unit: the look above may have spent the whole budget, and
            // a call that then gave up would leave the cursor at the top for the next call to
            // do the same, for ever.
            if !first && (read >= budget || purged >= limit) {
                break false;
            }
            first = false;
            let carved = super::layout::Grow::unpack(grow.load()).carved;
            let huge_from = Hints::unpack(hints.load()).huge_from;
            let next = match layout.unit_below(carved, huge_from, cursor) {
                Step::Done => 0,
                Step::Skip(page) => {
                    read += 1;
                    page
                }
                Step::Unit(unit) => {
                    let (outcome, words) = region::purge_unit(&host, region, unit);
                    read += words;
                    if let Purge::Done(bytes) = outcome {
                        purged += bytes;
                    }
                    unit.start
                }
            };
            if resume {
                set_cursor(hints, cursor, next);
            }
            if next == 0 {
                break true;
            }
            cursor = next;
        };
        self.unretain(class, purged);
        (purged, read, done)
    }
}

/// Moves a region's purge cursor from `from` to `to`, unless another purger has moved it.
fn set_cursor<W: Word>(hints: &W, from: u32, to: u32) {
    let mut seen = hints.load();
    loop {
        let now = Hints::unpack(seen);
        if now.cursor != from {
            return;
        }
        let next = Hints { cursor: to, ..now };
        match hints.compare_exchange(seen, next.pack()) {
            Ok(_) => return,
            Err(found) => seen = found,
        }
    }
}
