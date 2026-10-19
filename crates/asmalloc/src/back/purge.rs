//! Retained memory, its cap, the clocked decay pass and the purge walk.
//!
//! **Retained memory** is the back end's estimate of the bytes of free slots in its bitmaps
//! that are resident, for small classes and for large ones. A launder or a free raises it by
//! what it returned. Only two things lower it: reuse (a claim subtracts what it took, less
//! what the clean bits say was not resident) and purging (the pages returned that were not
//! already clean). A claim that makes a purged page resident again adds back the rest of that
//! page.
//!
//! Each count is two tallies that only grow, on lines of their own: what frees, launders and
//! revivals counted in, and what claims, fills and purges counted out (`State::retained`), so that a
//! thread that only frees and one that only allocates do not take one line from each other. The
//! count is their difference, read as signed and as zero when negative. A subtraction can run
//! before the addition it answers (a claim takes a slot between another thread's release of it
//! and that thread's add); a count that stopped at zero would lose the subtraction and keep the
//! addition, and drift upward for ever.
//!
//! The count is never reset. Free slots on pages that also hold live ones cannot be purged
//! and stay counted for as long as they are free. An earlier version set the count to zero
//! when a whole pass purged nothing, taking what was left for fragmentation. That is wrong in
//! a storm of frees: every page still has a live neighbour while a pass runs, the pages
//! become wholly free a moment later, and their bytes are then in no count, so the cap never
//! acts again (`docs/research/memory.md`: 136 MiB free and resident under a 64 MiB cap).
//!
//! **Decay** (`decay_step`, one call per heartbeat). Once an interval of the clock
//! (`policy::DECAY_TICKS`) a pass starts where the walk stands with a goal: a quarter of the
//! retained count (`policy::decay_goal`), or everything above the cap if that is more: the
//! clock's part of the cap, for memory over it that growth has not paid for. Each
//! heartbeat then does one bounded step of the walk until the goal is met or the walk has gone
//! once round, back where the pass began a lap later. The goal is in bytes and the walk stops
//! within one unit of it, whatever the slot size.
//!
//! The pass takes only what the last two intervals did not reuse. Fills and allocations take
//! the lowest free slots of a region, so the highest slot claimed or carved in an interval
//! marks how far up that interval's reuse reached: each region keeps that frontier for the
//! interval under way and the one before (`layout::Frontier`), and the pass takes only units
//! at or above the higher of the two ([`Scope::Idle`]). Free pages below it are in the zone
//! fills are reusing, and purging them only made the program fault them back; free pages
//! above it went a whole interval unwanted. In a region whose reuse reaches high little is
//! eligible and the walk moves on; in one no fill touched for two intervals everything free
//! is, as before frontiers. The thread that starts a pass rotates every region's frontier
//! (`rotate_frontiers`); a pass that starts two intervals or more after the last ended forgets
//! them (`intervals_since`). What an interval did not need goes back: the rule the depot
//! stacks, spares and stock follow.
//!
//! If a lap above the frontiers leaves more than the cap retained, the pass takes one more
//! lap, below them too, for the excess ([`Pass::below`]): the cap is a bound, and memory over
//! it goes within about an interval wherever it lies.
//!
//! Regions are in the order classes were first used, so the order across them says nothing
//! about reuse: the passes carry on round the heap instead of each starting at the top, which
//! spent every pass's goal on the pages a quiet program revived each interval in the higher
//! regions and left the free pages of the lowest resident for good. A heap whose free memory
//! is all fragmentation costs one walk of its bitmaps an interval, in steps. Between passes a
//! heartbeat reads the decay word and the clock, and the tallies only when a pass is due; a
//! due pass that finds nothing retained ends as it starts (and still rotates the frontiers),
//! so the next is an interval later. A clock that has gone back (a resume, a restored
//! checkpoint) restarts the interval from the new time, so decay waits at most one interval
//! for it. `decay_pass`, for the background thread's `maintain`, takes those steps in one
//! call, until the pass it started or found has ended.
//!
//! A pass is two words. The decay word holds the time the last pass ended, or the time the
//! pass under way began with its low bit set; one compare-and-swap starts a pass and one ends
//! it. The pass word holds the pass's remaining goal, the lap it began in and whether it may
//! go below the frontiers, under a tag derived from the decay word; it is written once when
//! the pass starts, after the frontiers are rotated, and once more by the one stepper whose
//! compare-and-swap switches it to its lap below the frontiers. A stepper acts only on a pass
//! word whose tag matches the decay word it read, so it never steps a new pass with the last
//! one's goal, and ends only the pass it saw.
//!
//! **Frontiers are hints.** A claim raises its region's frontier with a compare-and-swap only
//! when it reaches higher (`region::reach`): a load and a compare otherwise. A rotation is a
//! compare-and-swap per region whose frontier is not zero (`region::rotate`). Either may lose
//! a race to the other or be cut short by a fork; a claim recorded late counts in the next
//! interval, and one never recorded (its thread lost between claim and record) leaves its
//! slots claimed, so no purge can take them. The worst a wrong frontier does is a purge too
//! many or too few, for at most two intervals; safety rests on the purge being a claim
//! (`region::purge_unit`), as before.
//!
//! **The cap** (`policy::retained_cap`) is enforced when the heap grows, never when it frees.
//! A free cannot tell memory that is idle from memory about to be reused, and purging there
//! faulted reused memory straight back. Growth is carving: slots past a watermark, and runs.
//! It goes on a debt (`Back::grow`). When the debt reaches a step (`policy::growth_step`),
//! the grower takes it and, if retained memory is over the cap, purges up to the smaller of
//! the two (`policy::growth_goal`), within `policy::PURGE_CAP_WORDS` of bitmap: one circuit
//! of the heap from where the walk stands above the frontiers, and, if that is not enough,
//! one below them, since the cap is a bound. The debt is cleared whatever the walk returned, so
//! a heap whose free memory is all fragmentation pays one bounded walk per step of growth. A
//! program that frees and reuses the same classes never grows and never purges on its own
//! traffic; one that frees class X and then grows class Y purges X as Y grows, so resident
//! memory stays within the cap of what is live.
//!
//! A claim that makes purged pages resident again is reuse, not growth, though it raises
//! resident memory as growth does. Charging it made the allocator chase its own tail: with
//! free memory over the cap, a purge of memory about to be reused caused a refault, which
//! purged more of it, until the count reached the cap (a `HashMap` rebuilt in a loop made four
//! times the `madvise` calls). The case it leaves open, a phase change into a class whose pages
//! decay had returned, is what the decay pass's term for everything over the cap bounds, to
//! about an interval.
//!
//! **The walk** goes over the regions from the highest address down and round again from the
//! top, skipping regions whose class summary bit says nothing in them is free. Within a region it walks units of pages
//! from the watermark down (`Layout::unit_below`): whole pages in the small-page part, whole
//! huge pages in the huge-page part, so a huge page is never split. At the top of a region
//! with a huge-page part it first carves the rest of the unit that holds the watermark
//! (`region::top_up`), so that the part of that huge page the fault made resident beyond the
//! watermark can go with the rest. Each region keeps its own cursor in its `Hints`, and the
//! walk over regions is one word in the state that every purger carries on from: the clocked
//! pass, the cap's purge on growth. A count of completed laps and the position a pass began
//! at let a decay pass see that the walk has gone round even when another caller took the
//! step that finished the lap. All of these are hints: two purgers on the same unit are safe because each purge is a
//! claim (`region::purge_unit`), and a lost update only moves a purge earlier or later.
//!
//! **Trim** purges every unit of every region, ignoring the cap, the clock, the cursors and
//! the frontiers.

use super::Back;
use super::layout::{Hints, Step};
use super::policy::{
    DECAY_TICKS, PURGE_CAP_WORDS, PURGE_STEP_WORDS, decay_goal, growth_goal, growth_step,
    retained_cap,
};
use super::region::{self, Purge};
use super::tree::Host;
use super::word::{BITS, Word};
use crate::classes::Class;
use crate::os::Os;
use crate::space::{RegionId, RegionTag};
use core::sync::atomic::Ordering::{Acquire, Relaxed, Release, SeqCst};

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

/// How many intervals of the frontiers a pass that starts at `now` begins, after a decay word
/// `word` that holds the time the last pass ended: one, or two once that is two intervals ago
/// or more. Claims are made on slow paths, whose heartbeats start a pass once one is due, so a
/// pass that starts two intervals after the last ended has, but for a few large allocations
/// (which beat the heart one call in sixteen), seen no claim for at least an interval: what
/// the frontiers hold is older than the interval before it, and is forgotten. Otherwise a
/// process woken after a long quiet would spare for one more interval what it reused before it
/// went quiet. The few large claims a wrong guess forgets cost a purge too many.
pub(crate) const fn intervals_since(word: u64, now: u64) -> u32 {
    let since = (now & !ACTIVE).wrapping_sub(word & !ACTIVE);
    if since >= 1 << 63 || since < 2 * DECAY_TICKS {
        1
    } else {
        2
    }
}

/// A pass word: what the steppers of one decay pass share.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Pass {
    /// Names the pass: [`Pass::tag_of`] the decay word that started it.
    pub(crate) tag: u16,
    /// The low bits ([`Pass::LAP_BITS`]) of the lap count when the pass, or its lap below the
    /// frontiers, began; that lap is over once they differ and the walk is back where it began.
    pub(crate) lap: u16,
    /// Whether the pass may take pages below the regions' frontiers: the lap it takes once a
    /// lap above them has left more than the cap retained ([`Scope::All`]).
    pub(crate) below: bool,
    /// Pages the pass may still purge.
    pub(crate) pages: u32,
}

/// Which free pages a purge walk may take.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Scope {
    /// Only units that start at or above their region's frontier (`region::floor`): pages
    /// that no fill of the last two intervals reached. What the clocked pass takes, and what
    /// the cap takes first.
    Idle,
    /// Every free unit: what the cap takes when the idle ones are not enough, and what `trim`
    /// takes.
    All,
}

impl Pass {
    /// The bits of the lap count a pass word keeps: fifteen, the field's top bit being
    /// `below`. A lap is a walk of the whole heap, and a pass takes at most two, so the field
    /// wraps only if thousands of laps pass while one stepper looks.
    pub(crate) const LAP_BITS: u32 = 15;

    const LAP_MASK: u16 = (1 << Pass::LAP_BITS) - 1;

    /// The lap field for a lap count.
    pub(crate) const fn lap_of(laps: u32) -> u16 {
        laps as u16 & Pass::LAP_MASK
    }

    /// The tag of the pass a decay word started: every part of the word folded together, so
    /// two passes share a tag only by a one-in-65536 accident. The pass word a stepper then
    /// mistakes for the new pass's is the last pass's, whose goal is met or whose lap is over,
    /// so the stepper ends the new pass before it has purged anything: one pass is lost, and
    /// the next interval starts another.
    pub(crate) const fn tag_of(decay: u64) -> u16 {
        (decay ^ decay >> 16 ^ decay >> 32 ^ decay >> 48) as u16
    }

    pub(crate) const fn unpack(word: u64) -> Pass {
        let lap = (word >> 32) as u16;
        Pass {
            tag: (word >> 48) as u16,
            lap: lap & Pass::LAP_MASK,
            below: lap & !Pass::LAP_MASK != 0,
            pages: word as u32,
        }
    }

    /// The word for this pass. A `lap` wider than [`Pass::LAP_BITS`] keeps its low bits.
    pub(crate) const fn pack(self) -> u64 {
        let lap = (self.lap & Pass::LAP_MASK) as u64 | (self.below as u64) << Pass::LAP_BITS;
        (self.tag as u64) << 48 | lap << 32 | self.pages as u64
    }
}

impl<O: Os> Back<'_, O> {
    /// Bytes of free slots in the bitmaps that are believed to be resident, small and large
    /// classes together: what decays, and what the cap limits.
    #[inline]
    pub fn retained(&self) -> usize {
        self.clamped(false).saturating_add(self.clamped(true))
    }

    /// The part of [`Back::retained`] in large classes.
    #[inline]
    pub fn retained_large(&self) -> usize {
        self.clamped(true)
    }

    /// The count of large classes if `large`, else of small ones, read for use: zero when a
    /// subtraction has run ahead of its addition.
    fn clamped(&self, large: bool) -> usize {
        self.state.retained.signed(large).max(0) as usize
    }

    /// Counts `bytes` of free slots of `class` as retained: slots freed to the bitmaps, or
    /// free slots that became resident by something other than a free. Never purges (see the
    /// module documentation).
    pub(super) fn retain(&self, class: Class, bytes: usize) {
        if bytes != 0 {
            let tally = self.state.retained.added.of(!class.is_small());
            tally.fetch_add(bytes, SeqCst);
        }
    }

    /// Subtracts `bytes` of slots of `class`, claimed while resident or purged, from the
    /// count.
    pub(super) fn unretain(&self, class: Class, bytes: usize) {
        if bytes != 0 {
            let tally = self.state.retained.taken.of(!class.is_small());
            tally.fetch_add(bytes, SeqCst);
        }
    }

    /// Charges `bytes` that a fill or a large allocation has just carved to the growth debt; once the debt is a step, takes it and purges as much, or what is over
    /// the cap if that is less. The caller holds its slots by now, so the walk cannot take
    /// them.
    ///
    /// The add and the take are separate atomic steps, so two growers may both see a step:
    /// the first takes it, and the second takes what accrued since, under a step, and does
    /// nothing. The few bytes it drops cost one purge later.
    #[inline]
    pub(super) fn grow(&self, bytes: usize) {
        if bytes == 0 {
            return;
        }
        let step = growth_step(self.space.plan().geometry());
        let debt = self
            .state
            .growth
            .fetch_add(bytes, Relaxed)
            .wrapping_add(bytes);
        if debt >= step {
            self.pay_growth(step);
        }
    }

    /// Takes the growth debt and purges toward it: `grow`'s rare part. Out of line, because
    /// inlined into `alloc_at` it made the compiler keep the class's layout on the stack, and
    /// every large allocation paid 19 more stores and 18 more loads for it (4% on a pipeline
    /// of large objects; `docs/research/memory.md`, "Small heaps").
    #[cold]
    #[inline(never)]
    fn pay_growth(&self, step: usize) {
        let geometry = self.space.plan().geometry();
        let debt = self.state.growth.swap(0, Relaxed);
        let goal = growth_goal(debt, self.retained(), retained_cap(geometry), step);
        // One circuit of the heap from wherever the shared walk stands, counted in regions
        // finished: the lap count says nothing here, since the walk may stand one region from
        // the bottom with everything purgeable above it. The cap is a bound, so a circuit of
        // what is idle that falls short is followed by one that takes what fills reuse too.
        let regions = self.space.plan().objects();
        let (mut spent, mut purged) = (0, 0);
        for scope in [Scope::Idle, Scope::All] {
            let mut finished = 0;
            while purged < goal && spent < PURGE_CAP_WORDS && finished < regions {
                let (read, bytes, done) =
                    self.purge_step(PURGE_CAP_WORDS - spent, goal - purged, scope);
                spent += read;
                purged += bytes;
                finished += usize::from(done);
            }
        }
    }

    /// The maintenance a slow path does on its way out: one step of decay.
    pub fn heartbeat(&self) {
        self.heartbeat_with(&|| self.space.os().ticks());
    }

    /// [`Back::heartbeat`] for a caller that keeps its own reading of the clock: `now` is
    /// called only when the step needs the time.
    pub fn heartbeat_with(&self, now: &dyn Fn() -> u64) {
        let _ = self.decay_step(now);
    }

    /// The clocked pass the heartbeats of an interval would take, in one call: starts a pass
    /// if one is due, and steps the pass under way until it ends, by meeting its goal or going
    /// once round the heap. It stops when that pass ends or the steps stop for another reason
    /// (nothing is due, or the pass is not yet published), and never goes on into a later
    /// pass, so it reads at most one lap of the bitmaps. Between passes it costs what a
    /// heartbeat's step costs: a load and a clock read.
    pub fn decay_pass(&self, now: &dyn Fn() -> u64) {
        let Some(pass) = self.decay_step(now) else {
            return;
        };
        while self.decay_step(now) == Some(pass) {}
    }

    /// One heartbeat's part of the clocked pass: starts a pass if an interval has gone by
    /// since the last ended and anything is retained, and takes one bounded step of the pass
    /// under way. A pass that is due finds nothing retained is ended as it starts, so the next
    /// one is due an interval later. Returns the decay word of the pass it stepped if it
    /// stopped at its budget with the pass still under way, so that more steps would do more;
    /// `None` otherwise.
    fn decay_step(&self, now: &dyn Fn() -> u64) -> Option<u64> {
        let state = self.state;
        let geometry = self.space.plan().geometry();
        let page = geometry.page_size();
        let mut seen = state.decay.load(Acquire);
        // `decay_look` steps a pass under way without the time. Between passes it reads the
        // clock, and only a pass that is due reads the retained count: its tallies are written
        // by frees and claims, and reading them on every heartbeat took their lines from the
        // threads writing them (`docs/research/memory.md`, "The cost of a large block, by
        // cause").
        let time = if seen & ACTIVE != 0 { 0 } else { now() };
        match decay_look(seen, time) {
            Decay::Wait => return None,
            Decay::Restamp(word) => {
                let _ = state.decay.compare_exchange(seen, word, Release, Relaxed);
                return None;
            }
            Decay::Start(word) => {
                let retained = self.retained();
                if retained == 0 {
                    // A pass with nothing to do, ended as it starts, so that the heartbeats
                    // of the next interval wait instead of reading the count. It still begins
                    // an interval of the frontiers, so that a frontier says what the interval
                    // before the next pass reused, not what one long before it did.
                    if state
                        .decay
                        .compare_exchange(seen, word & !ACTIVE, Release, Relaxed)
                        .is_ok()
                    {
                        self.rotate_frontiers(intervals_since(seen, time));
                    }
                    return None;
                }
                if state
                    .decay
                    .compare_exchange(seen, word, Release, Relaxed)
                    .is_err()
                {
                    return None;
                }
                // Before the pass word is published, so that no step of this pass reads a
                // floor from before the interval that just ended.
                self.rotate_frontiers(intervals_since(seen, time));
                let over = retained.saturating_sub(retained_cap(geometry));
                let goal = decay_goal(geometry, retained).max(over);
                // From where the walk stands: a pass that started at the top every time spent
                // its goal on the regions above whatever a quiet program revived each interval,
                // and never reached the free pages of the lowest regions. The pass word is
                // written last and once: until then no stepper finds its tag and none acts.
                let from = state.walk.purge_at.load(Relaxed);
                state.walk.pass_from.store(from, Relaxed);
                let pass = Pass {
                    tag: Pass::tag_of(word),
                    lap: Pass::lap_of(state.walk.laps.load(Relaxed)),
                    below: false,
                    pages: pages_of(goal, page),
                };
                state.walk.pass.store(pass.pack(), Release);
                seen = word;
            }
            Decay::Step => {}
        }
        let tag = Pass::tag_of(seen);
        let mut spent = 0;
        loop {
            let word = state.walk.pass.load(Acquire);
            let pass = Pass::unpack(word);
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
                return None;
            }
            let round = pass.pages != 0 && self.went_round(pass);
            if round && !pass.below {
                // A lap of what is idle is done. If it left more than the cap retained, the
                // cap is a bound: one more lap takes the excess wherever it lies, under the
                // frontiers too. One stepper switches the pass, with one compare-and-swap of
                // the word it read; the others find the new word and step it.
                let over = self.retained().saturating_sub(retained_cap(geometry));
                if over != 0 {
                    let from = state.walk.purge_at.load(Relaxed);
                    let next = Pass {
                        lap: Pass::lap_of(state.walk.laps.load(Relaxed)),
                        below: true,
                        pages: pages_of(over, page),
                        ..pass
                    };
                    if state
                        .walk
                        .pass
                        .compare_exchange(word, next.pack(), Release, Relaxed)
                        .is_ok()
                    {
                        // After the switch, so that a switcher that lost cannot move the
                        // place another pass's lap ends. Until it lands, the new lap's count
                        // has not moved, so no stepper looks at it.
                        state.walk.pass_from.store(from, Relaxed);
                    }
                    continue;
                }
            }
            if pass.pages == 0 || round {
                // The goal is met or the walk has gone round. End the pass this thread saw,
                // and only that one.
                let now = self.space.os().ticks() & !ACTIVE;
                let _ = state.decay.compare_exchange(seen, now, Release, Relaxed);
                return None;
            }
            if spent >= PURGE_STEP_WORDS {
                return Some(seen);
            }
            let limit = pass.pages as usize * page;
            let scope = if pass.below { Scope::All } else { Scope::Idle };
            let (read, purged, _) = self.purge_step(PURGE_STEP_WORDS - spent, limit, scope);
            spent += read;
            let done = (purged / page).min(u32::MAX as usize) as u32;
            if done != 0 {
                // Lower this pass's goal, and no other's.
                let mut word = state.walk.pass.load(Acquire);
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
                        .walk
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

    /// Whether the walk has gone once round since `pass` began: a lap has ended since, and the
    /// walk is back at or below where the pass began. A pass that began at the top is round
    /// when the lap ends, as every pass was before passes resumed the walk.
    fn went_round(&self, pass: Pass) -> bool {
        let walk = &self.state.walk;
        if Pass::lap_of(walk.laps.load(Relaxed)) == pass.lap {
            return false;
        }
        let from = walk.pass_from.load(Relaxed);
        let at = walk.purge_at.load(Relaxed);
        from == 0 || (at != 0 && at <= from)
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
                    .purge_region(class, region, usize::MAX, usize::MAX, false, Scope::All)
                    .0;
            }
        }
        purged
    }

    /// One step of the walk: works on the region at the walk's position, and moves the
    /// position on if that region is done. Stops after `budget` words of bitmap or once it
    /// has purged `limit` bytes, finishing the unit it is on. Returns the words of bitmap
    /// read, at least one, the bytes purged, and whether it finished its region.
    fn purge_step(&self, budget: usize, limit: usize, scope: Scope) -> (usize, usize, bool) {
        let plan = self.space.plan();
        let seen = self.state.walk.purge_at.load(Relaxed);
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
            Some(class) => self.purge_region(class, region, budget, limit, true, scope),
            None => (0, 1, true),
        };
        if done
            && self
                .state
                .walk
                .purge_at
                .compare_exchange(seen, index as u32, Relaxed, Relaxed)
                .is_ok()
            && index == 0
        {
            self.state.walk.laps.fetch_add(1, Relaxed);
        }
        (read.max(1), purged, done)
    }

    /// Begins a new interval of every class region's frontier, `intervals` after the last
    /// (`region::rotate`), for the thread whose compare-and-swap of the decay word has just
    /// begun one. A load from the region table for each region of the heap, and a load and at
    /// most one compare-and-swap for each class region, which writes only a frontier that is
    /// not zero.
    pub(super) fn rotate_frontiers(&self, intervals: u32) {
        let plan = self.space.plan();
        for region in (0..plan.objects()).filter_map(|i| plan.region(i)) {
            if let RegionTag::Class(class) = self.state.table.get(region)
                && let Some(host) = self.host(class)
            {
                region::rotate(&host, region, intervals);
            }
        }
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
    /// and leaves the cursor alone. With [`Scope::Idle`] the region is done at its frontier
    /// (`region::floor`, read once a call): a unit that starts below it is left, with the
    /// rest of the region under it. Returns the bytes purged, the words read, and whether the
    /// walk reached the bottom, or the frontier.
    fn purge_region(
        &self,
        class: Class,
        region: RegionId,
        budget: usize,
        limit: usize,
        resume: bool,
        scope: Scope,
    ) -> (usize, usize, bool) {
        let Some(host) = self.host(class) else {
            return (0, 1, true);
        };
        let layout = *host.layout();
        let hints = host.hints(region);
        let grow = host.grow(region);
        let mut cursor = if resume {
            Hints::unpack(hints.load()).cursor
        } else {
            0
        };
        let floor = match scope {
            Scope::Idle => region::floor(&host, region),
            Scope::All => 0,
        };
        if cursor != 0 && cursor <= floor {
            // Everything the cursor has left is below the frontier.
            if resume {
                set_cursor(hints, cursor, 0);
            }
            return (0, 1, true);
        }
        let (mut purged, mut read) = (0, 0);
        if cursor == 0 {
            // At the top of the region: bring the watermark up to the end of its huge page,
            // if that page could then be purged whole.
            let (bytes, words) = region::top_up(&host, region, floor);
            self.retain(class, bytes);
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
                    if page <= floor { 0 } else { page }
                }
                Step::Unit(unit) if unit.start < floor => {
                    read += 1;
                    0
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

/// A goal of `bytes` as the pass word holds it: whole pages, at least one.
fn pages_of(bytes: usize, page: usize) -> u32 {
    bytes.div_ceil(page).clamp(1, u32::MAX as usize) as u32
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
