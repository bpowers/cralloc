//! Retained memory, its caps, the clocked decay pass and the purge walk.
//!
//! **Retained memory** is the back end's estimate of the bytes of free slots in its bitmaps
//! that are resident, kept as two counts: small classes and large ones. A launder or a free
//! raises a count by what it returned. Only two things lower it: reuse (a claim subtracts what
//! it took, less what the clean bits say was not resident) and purging (the pages returned
//! that were not already clean). A claim that makes a purged page resident again adds back the
//! rest of that page. The counts saturate at zero.
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
//! walk has gone once round. Fills take the lowest addresses, so what is being reused sits at
//! the bottom and the top-down pass mostly leaves it alone; what stays free is returned over
//! several intervals. A heap whose free memory is all fragmentation costs one walk of its
//! bitmaps an interval, in steps, and nothing between passes.
//!
//! **The caps** act at the free, for bursts inside an interval.
//!
//! - Small classes (`policy::retained_cap`): the launder that pushes the count past the cap
//!   purges toward it within `policy::PURGE_CAP_WORDS` of bitmap. While the count stays over
//!   (fragmentation can hold it there), a launder tries again each time it has grown by
//!   another `cap / policy::CAP_REARM`.
//! - Large classes (`policy::large_retained_cap`): a free that leaves the count over the cap
//!   purges the slot it has just freed. A large slot is whole pages of its own, so that is
//!   one claim and one system call with no search.
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
    CAP_REARM, DECAY_TICKS, PURGE_CAP_WORDS, PURGE_STEP_WORDS, decay_goal, large_retained_cap,
    retained_cap,
};
use super::region::{self, Purge};
use super::tree::Host;
use super::word::{BITS, Word};
use crate::classes::Class;
use crate::os::Os;
use crate::space::{RegionId, RegionTag};
use core::sync::atomic::AtomicUsize;
use core::sync::atomic::Ordering::Relaxed;

/// The decay word's low bit: a pass is under way. The rest is the time the pass began, or the
/// time the last one ended.
const ACTIVE: u64 = 1;

/// What a decay word and the clock say to do.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Decay {
    /// Nothing: the last pass ended less than an interval ago.
    Wait,
    /// Start a pass: store this word, if the word is still the one read.
    Start(u64),
    /// A pass is under way: take a step of it.
    Step,
}

/// The decision for a decay word read as `word` with the clock at `now`. A clock that went
/// back (another CPU's, say) reads as "not yet".
pub(crate) const fn decay_look(word: u64, now: u64) -> Decay {
    if word & ACTIVE != 0 {
        return Decay::Step;
    }
    let since = (now & !ACTIVE).wrapping_sub(word) as i64;
    if since >= 0 && since as u64 >= DECAY_TICKS {
        Decay::Start(now | ACTIVE)
    } else {
        Decay::Wait
    }
}

/// Whether a small-class count that a launder moved from `old` to `new` should make that
/// launder purge: it crossed the cap, or it is over the cap and crossed a re-arm step.
pub(crate) const fn cap_fires(old: usize, new: usize, cap: usize) -> bool {
    if new <= cap {
        return false;
    }
    let step = cap / CAP_REARM;
    old <= cap || (step != 0 && old / step != new / step)
}

/// Lowers a count by `bytes`, stopping at zero.
fn lower(count: &AtomicUsize, bytes: usize) {
    if bytes == 0 {
        return;
    }
    let mut seen = count.load(Relaxed);
    while let Err(found) =
        count.compare_exchange_weak(seen, seen.saturating_sub(bytes), Relaxed, Relaxed)
    {
        seen = found;
    }
}

impl<O: Os> Back<'_, O> {
    fn count(&self, class: Class) -> &AtomicUsize {
        if class.is_small() {
            &self.state.retained_small
        } else {
            &self.state.retained_large
        }
    }

    /// Counts `bytes` of slots of `class` freed to the bitmaps as retained. For a small class,
    /// purges toward the cap if this pushed the count past it or past the next re-arm step.
    /// A large class's cap is the caller's business (`Back::free`), which knows the slot.
    pub(super) fn retain(&self, class: Class, bytes: usize) {
        let old = self.count(class).fetch_add(bytes, Relaxed);
        if !class.is_small() {
            return;
        }
        let cap = retained_cap(self.space.plan().geometry());
        if cap_fires(old, old.saturating_add(bytes), cap) {
            let mut spent = 0;
            while self.state.retained_small.load(Relaxed) > cap && spent < PURGE_CAP_WORDS {
                spent += self.purge_step(PURGE_CAP_WORDS - spent).0;
            }
        }
    }

    /// Counts `bytes` of free slots of `class` as retained without acting on a cap: free
    /// memory that became resident by something other than a free.
    pub(super) fn revive(&self, class: Class, bytes: usize) {
        if bytes != 0 {
            self.count(class).fetch_add(bytes, Relaxed);
        }
    }

    /// Subtracts `bytes` of slots of `class`, claimed while resident or purged, from the
    /// count, stopping at zero.
    pub(super) fn unretain(&self, class: Class, bytes: usize) {
        lower(self.count(class), bytes);
    }

    /// Whether the large-class count is over its cap.
    pub(super) fn large_over_cap(&self) -> bool {
        self.state.retained_large.load(Relaxed) > large_retained_cap(self.space.plan().geometry())
    }

    /// Purges the pages of one free slot of a large class, if it is still free: what a free
    /// does when it leaves the large-class count over its cap. `slot` is below the watermark.
    pub(super) fn purge_slot(&self, class: Class, region: RegionId, slot: u32) {
        let Some(host) = self.host(class) else {
            return;
        };
        let layout = *host.layout();
        let Some(unit) = layout.slot_unit(slot) else {
            return;
        };
        // In a huge-page part a purge must be whole huge pages; large classes have none, and
        // this keeps the rule if that ever changes.
        let hints = Hints::unpack(host.meta(region, layout.hints_word()).load());
        if hints.huge_from.is_some_and(|from| unit.end > from) {
            return;
        }
        if let (Purge::Done(bytes), _) = region::purge_unit(&host, region, unit) {
            self.unretain(class, bytes);
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
        let seen = state.decay.load(Relaxed);
        match decay_look(seen, self.space.os().ticks()) {
            Decay::Wait => return,
            Decay::Start(word) => {
                let geometry = self.space.plan().geometry();
                let small = state.retained_small.load(Relaxed);
                let retained = small.saturating_add(state.retained_large.load(Relaxed));
                if retained == 0 {
                    return;
                }
                if state
                    .decay
                    .compare_exchange(seen, word, Relaxed, Relaxed)
                    .is_err()
                {
                    return;
                }
                let over = small.saturating_sub(retained_cap(geometry));
                let goal = decay_goal(geometry, retained);
                state
                    .decay_goal
                    .store(if over > goal { over } else { goal }, Relaxed);
                // From the top, and remember which lap this is so that the pass ends when
                // the walk has gone round once, whoever takes the last step of the lap.
                state.purge_at.store(0, Relaxed);
                state.decay_lap.store(state.laps.load(Relaxed), Relaxed);
            }
            Decay::Step => {}
        }
        let mut spent = 0;
        let mut left = state.decay_goal.load(Relaxed);
        while spent < PURGE_STEP_WORDS && left != 0 {
            let (read, purged) = self.purge_step(PURGE_STEP_WORDS - spent);
            spent += read;
            lower(&state.decay_goal, purged);
            left = state.decay_goal.load(Relaxed);
            if state.laps.load(Relaxed) != state.decay_lap.load(Relaxed) {
                left = 0;
            }
        }
        if left == 0 {
            // The pass is over. Only the word of the pass this thread saw is replaced, so a
            // pass another thread has started since is not cut short.
            let now = self.space.os().ticks() & !ACTIVE;
            let current = state.decay.load(Relaxed);
            if current & ACTIVE != 0 {
                let _ = state.decay.compare_exchange(current, now, Relaxed, Relaxed);
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
                purged += self.purge_region(class, region, usize::MAX, false).0;
            }
        }
        purged
    }

    /// One step of the walk: works on the region at the walk's position, and moves the
    /// position on if that region is done. Returns the words of bitmap read, at least one, and
    /// the bytes purged.
    fn purge_step(&self, budget: usize) -> (usize, usize) {
        let plan = self.space.plan();
        let seen = self.state.purge_at.load(Relaxed);
        // The position is the region to look at next plus one; zero means the top.
        let index = if seen == 0 || seen as usize > plan.objects() {
            plan.objects() - 1
        } else {
            seen as usize - 1
        };
        let Some(region) = plan.region(index) else {
            return (1, 0);
        };
        let (purged, read, done) = match self.purgeable(region) {
            Some(class) => self.purge_region(class, region, budget, true),
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
        (read.max(1), purged)
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

    /// Purges units of a region from the top down until the region is done or `budget` words
    /// have been read. With `resume`, starts at the region's cursor and keeps it up to date;
    /// without, walks the whole region and leaves the cursor alone. Returns the bytes purged,
    /// the words read, and whether the walk reached the bottom.
    fn purge_region(
        &self,
        class: Class,
        region: RegionId,
        budget: usize,
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
        let done = loop {
            if read >= budget {
                break false;
            }
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
