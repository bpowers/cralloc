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
//! retained count (`policy::decay_goal`), or everything above the cap if that is more, and
//! never past the floor (below). Each heartbeat then does one bounded step of the walk until
//! the goal is met or the walk has gone once round, back where the pass began a lap later. The
//! goal is in bytes and the walk stops within one unit of it, whatever the slot size.
//!
//! **The floor** (`policy::allowance_floor`) is measured against what a purge could return:
//! free pages with nothing live on them that are still resident. Not against the retained
//! count, which also holds every free slot on a page with a live neighbour: a 1.4 GiB server
//! retained 185 MiB, of which 1.4 MiB could be returned, so by the count the floor never
//! applied, and the passes took the last free pages, which the next burst faulted straight
//! back (`docs/research/default-thread-servers.md`). Every step of the walk adds up the free,
//! resident pages it sees and leaves, purgeable whether or not its scope or goal let it take
//! them, and the step that ends a lap (finishes the lowest region) makes that the count, and
//! starts what has been purged since at zero ([`Back::returnable`]). A pass is held to the
//! last lap's count less what has been purged since: while that is no more than the floor
//! it returns nothing, and over it it stops at the floor. Until the first lap has ended there
//! is no count, and a pass is held to the retained count, which is never less. A pass held to
//! nothing still walks its lap, purging nothing, so that the next count is taken: a heap with
//! nothing to return costs one walk of its bitmaps an interval, as a heap of fragmentation
//! always has. The cheap test stays first: a due pass that finds no more than the floor
//! retained ends as it starts, since no more could be returned. What a heartbeat's pass
//! walks, it walks down to the frontiers, so while the program runs the count is of what lies
//! above them, what such a pass could take; in a silence `maintain`'s passes walk every
//! region whole. The count is a hint: a lap's steps race with frees, claims and the hold, and
//! a count too high or too low costs a purge too many or too few for a lap.
//!
//! The pass takes only what the last two intervals did not reuse, whatever the count says.
//! Fills and allocations take the lowest free slots of a region, so the highest slot claimed or
//! carved in an interval marks how far up that interval's reuse reached: each region keeps
//! that frontier for an interval of the clock and the one before (`layout::Frontier`), and the
//! pass takes only units at or above it ([`Scope::Idle`]). Free pages below it were wanted
//! within the last two intervals and are what fills take next; purging them only made the
//! program fault them back. Free pages above it went a whole interval unwanted. In a region
//! whose reuse reaches high little is eligible and the walk moves on; in one no fill touched
//! for two intervals everything free is. What an interval did not need goes back: the rule the
//! depot stacks, spares and stock follow.
//!
//! That holds over the cap too. The count includes free slots on pages that also hold live
//! ones, which no purge can lower, so a heap with more than the cap of fragmentation is over
//! it for good, and a pass that went below the frontiers to bring the count down would purge
//! what the program reuses every interval. The cap's job, the memory one class frees while
//! another grows, is growth's to pay (below). What a burst over the cap that then goes quiet
//! costs is time: its memory waits until no fill has reached it for an interval.
//!
//! A frontier word carries the number of its interval (`Clocked::epoch`), so nothing has to
//! age the words: a reader in interval E takes a word of E whole, a word of E - 1 for its
//! `now`, and anything older as nothing. The number is written by the first heartbeat of each
//! interval that reads the clock: every heartbeat between passes, and a heartbeat's step of a
//! pass as it finishes each region and as it ends, so a long pass makes it lag by at most the
//! walk of one region.
//!
//! **Frontiers age while the program runs.** The number is not the clock's interval but a
//! count of the intervals in which a heartbeat ran: a heartbeat in a later interval than the
//! last counted counts one more, however many passed between (`epoch_after`). `maintain`
//! never counts. A silence is not evidence that the program has stopped needing what it last
//! used: the background thread's rounds step the counted clock through a quiet half minute,
//! and by the clock every frontier would have aged, so that its passes returned the pages
//! its own drains had just completed. So a frontier lives the interval of its claim and the
//! next interval in which the program ran. In a silence (a whole interval of the clock since
//! the last counted, [`Back::silent`], which only `maintain`'s readings can make) a step of
//! `maintain`'s takes what lies above the frontiers and below them alike ([`Scope::Quiet`]),
//! from the top of each region down, within a goal that stops at the floor, so what stays is
//! the lowest pages of the regions the walk reached last, the ones the next fills take. A
//! heartbeat's step never goes below a frontier: a heartbeat is the program running. Where
//! nothing calls `maintain` the counted clock moves one interval at most in a silence
//! (`crate::clock`), so there is no silence, and the first heartbeat after one counts one
//! interval, as before.
//!
//! Regions are in the order classes were first used, so the order across them says nothing
//! about reuse: the passes carry on round the heap instead of each starting at the top, which
//! spent every pass's goal on the pages a quiet program revived each interval in the higher
//! regions and left the free pages of the lowest resident for good. A heap whose free memory
//! is all fragmentation costs one walk of its bitmaps an interval, in steps. Between passes a
//! heartbeat reads the decay word and the clock, and the tallies only when a pass is due; a
//! due pass that finds no more than the floor retained ends as it starts, so the next is an
//! interval later. A clock that has gone back (a resume,
//! a restored checkpoint) restarts the interval from the new time, so decay waits at most one
//! interval for it. `decay_pass`, for the background thread's `maintain`, takes those steps in
//! one call, until the pass it started or found has ended.
//!
//! A pass is two words. The decay word holds the time the last pass ended, or the time the
//! pass under way began with its low bit set; one compare-and-swap starts a pass and one ends
//! it. The pass word holds the pass's remaining goal and the lap it began in, under a tag
//! derived from the decay word, and is written once, when the pass starts. A stepper acts only
//! on a pass word whose tag matches the decay word it read, so it never steps a new pass with
//! the last one's goal, and ends only the pass it saw.
//!
//! **Frontiers are hints.** Only a thread that has just claimed in a region writes its word,
//! with a compare-and-swap, and only when it reaches higher or the word is of an earlier
//! interval (`region::reach`): a load and a compare otherwise. A claim recorded late counts in
//! the next interval; one never recorded (its thread lost between claim and record) leaves its
//! slots claimed, so no purge can take them; an interval number that has wrapped round makes
//! an old word read as recent. The worst a wrong frontier does is a purge too many or too few,
//! for at most two intervals; safety rests on the purge being a claim (`region::purge_unit`).
//!
//! **The cap** (`policy::retained_cap`) is enforced when the heap grows, never when it frees.
//! A free cannot tell memory that is idle from memory about to be reused, and purging there
//! faulted reused memory straight back. Growth is carving: slots past a watermark, and runs.
//! It goes on a debt (`Back::grow`). When the debt reaches a step (`policy::growth_step`),
//! the grower takes it and, if retained memory is over the cap, purges up to the smaller of
//! the two (`policy::growth_goal`), within `policy::PURGE_CAP_WORDS` of bitmap and one circuit
//! of the heap from where the walk stands, frontiers or not ([`Scope::All`]): growth is
//! resident memory the program asked for now, and paying for it is what keeps memory within
//! the cap of what is live. A payment returns at most `policy::PURGE_STEP_PAGES`, since it
//! runs on a request's slow path; what it owes past that stays on the debt, and the next
//! heartbeats pay it, a step each. Paid at once on the carve, a slot of tens of megabytes made
//! one request return as much (2 ms on DataFusion); left for the next carve alone, it was paid
//! too late, and DataFusion's peak rose by 30 to 80 MiB.
//! Otherwise the debt is cleared whatever the walk returned, so a heap whose free memory is
//! all fragmentation pays one bounded walk per step of growth. A program that
//! frees and reuses the same classes never grows and never purges on its own traffic; one that
//! frees class X and then grows class Y purges X as Y grows.
//!
//! **While the background thread runs** (design: "Heartbeat"), returning memory is its work.
//! A heartbeat takes no step of a pass, no look of the hold and no payment
//! ([`Back::heartbeat_for`]): it keeps the readings the clocked rules count time by, and wakes
//! the thread when one of them makes a pass due or reaches the hold's next look. The debt is a
//! count any slow path adds to and the thread pays down: once a step of growth, the grower
//! wakes the thread if retained memory is over the cap, or drops the debt as a payment under the
//! cap would; the thread takes the same steps a heartbeat would ([`Back::purge_steps`]) until
//! none would do more. While more than `policy::GROWTH_UNPAID_HUGE_PAGES` is left unpaid, the
//! slow paths pay it as if there were no thread: a grower pays a step itself, and so does each
//! heartbeat, so that what the thread's lag can add to retained memory is bounded whatever the
//! scheduler does. A grower's step alone would not do: it pays half a megabyte while a carve
//! may owe tens, and on DataFusion a thread that waited for a CPU behind the four that carved
//! let the peak rise by 180 MiB in one run in five (`docs/research/purge-off-path.md`). Past
//! the limit they pay whether a pass is under way or not, and a pass still under way an
//! interval after it began, which the thread has not stepped, is the heartbeats' again.
//! The thread's own steps (`StepSize::THREAD`) are a heartbeat's in size but `maintain`'s in
//! kind: they do not count an interval as one the program ran in, and its looks of the hold
//! take `maintain`'s reach, so that a silence ages nothing and a quiet program's held slots go
//! back at the thread's second round.
//!
//! A claim that makes purged pages resident again is reuse, not growth, though it raises
//! resident memory as growth does. Charging it made the allocator chase its own tail: with
//! free memory over the cap, a purge of memory about to be reused caused a refault, which
//! purged more of it, until the count reached the cap (a `HashMap` rebuilt in a loop made four
//! times the `madvise` calls). The case it leaves open, a phase change into a class whose pages
//! decay had returned while another class's freed memory stays resident, is what the decay
//! pass's term for everything over the cap bounds: the other class's memory, which no fill
//! reaches any more, goes within about two intervals.
//!
//! **The walk** goes over the regions from the highest address down and round again from the
//! top, skipping regions whose class summary bit says nothing in them is free. Within a region it walks units of pages
//! from the watermark down (`Layout::unit_below`): whole pages in the small-page part, whole
//! huge pages in the huge-page part, so a huge page is never split. At the top of a region
//! with a huge-page part it first carves the rest of the unit that holds the watermark
//! (`region::top_up`), so that the part of that huge page the fault made resident beyond the
//! watermark can go with the rest.
//!
//! **Pieces.** A unit of the small-page part is the pages of the slots it covers, so one
//! large slot is a unit as long as the slot: tens of megabytes. A purge step that may return
//! fewer pages than a unit holds takes its top part (`piece`), claiming the slot as a whole
//! purge does, and releases it; the cursor then names the page the piece began at, and the
//! next step's unit is the rest of the slot below it. The retained count drops by what each
//! piece cleaned. A piece sets the slot's zero bit only if it leaves every page of the slot
//! clean (`region::purge_unit`), so an allocation of a slot that is partly returned is not
//! taken for zero: it gets the pages a piece returned as zero, and the rest as its last
//! tenant left them. So every step on a request's slow path returns a bounded amount whatever
//! the slot sizes: a heartbeat's step of the clocked pass `policy::PURGE_STEP_PAGES`, the
//! hold's look `policy::hold_beat`, a growth payment `PURGE_STEP_PAGES`.
//!
//! Each region keeps its own cursor in its `Hints`, and the
//! walk over regions is one word in the state that every purger carries on from: the clocked
//! pass, the cap's purge on growth. The hold on free large slots (`hold`) carries on from the
//! regions' cursors too, over a position of its own among the large classes' regions. It goes round only the regions below the highest ever
//! given to a class (`Walk::top`), so a lap costs the class regions, not the reservation. A count of completed laps and the position a pass began
//! at let a decay pass see that the walk has gone round even when another caller took the
//! step that finished the lap. All of these are hints: two purgers on the same unit are safe because each purge is a
//! claim (`region::purge_unit`), and a lost update only moves a purge earlier or later.
//!
//! **Trim** purges every unit of every region, ignoring the cap, the clock, the cursors and
//! the frontiers.

use super::Back;
use super::layout::Unit;
use super::layout::{Hints, Step};
use super::policy::{
    DECAY_TICKS, PURGE_ANY_RUN_EVERY, PURGE_CAP_WORDS, PURGE_MAINTAIN_STEP_PAGES,
    PURGE_MAINTAIN_STEP_WORDS, PURGE_MAX_RUN_PAGES, PURGE_MAX_RUN_WORDS, PURGE_MIN_RUN_PAGES,
    PURGE_STEP_PAGES, PURGE_STEP_WORDS, allowance_floor, decay_goal, growth_goal, growth_step,
    growth_unpaid, retained_cap,
};
use super::region::{self, Purge};
use super::tree::Host;
use super::word::{BITS, Word};
use crate::classes::Class;
use crate::os::Os;
use crate::space::{RegionId, RegionTag};
use crate::thread_word::{Duty, Snapshot};
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

/// The decision for a decay word read as `word` with the clock at `now`: a pass is due once
/// in each counted interval (`crate::clock`), the first time a reading falls in an interval
/// later than the word's. Due by the interval's number, not by an interval of time since the
/// word: a pass that ended a little after the start of its interval would otherwise not be an
/// interval old at the start of the next, and with readings an interval apart (a `maintain`
/// once an interval, or heartbeats that far apart) passes would come every other interval.
pub(crate) const fn decay_look(word: u64, now: u64) -> Decay {
    if word & ACTIVE != 0 {
        return Decay::Step;
    }
    let now = now & !ACTIVE;
    let since = now.wrapping_sub(word);
    if since >= 1 << 63 {
        Decay::Restamp(now)
    } else if now / DECAY_TICKS > word / DECAY_TICKS {
        Decay::Start(now | ACTIVE)
    } else {
        Decay::Wait
    }
}

/// The epoch word to store over `seen` for a heartbeat's clock reading `now`, or `None` to
/// leave it. The word counts the intervals in which a heartbeat ran in its low half and holds
/// the clock's interval of the last of them in its high half (`State::decay`). A reading in a
/// later interval than the word's counts one more, however many intervals passed between, so
/// that a silence ages a frontier by one interval at most. A reading in the word's interval or
/// the one before leaves it, so that a heartbeat that read the clock just before a boundary
/// cannot count that interval twice. A clock more than an interval behind the word has gone
/// back (a resume, a restored checkpoint), as the decay word's restamp allows for: it is
/// taken as it is and counts as a new interval.
pub(crate) const fn epoch_after(seen: u64, now: u64) -> Option<u64> {
    let interval = (now / DECAY_TICKS) & EPOCH_COUNT;
    let last = seen >> 32;
    if interval > last || interval.wrapping_add(1) < last {
        Some(interval << 32 | (seen.wrapping_add(1) & EPOCH_COUNT))
    } else {
        None
    }
}

/// The low half of the epoch word: the count of intervals in which the program ran.
const EPOCH_COUNT: u64 = u32::MAX as u64;

/// A pass word: what the steppers of one decay pass share.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Pass {
    /// Names the pass: [`Pass::tag_of`] the decay word that started it.
    pub(crate) tag: u16,
    /// The low bits of the lap count when the pass began; the pass ends when they differ and
    /// the walk is back where it began.
    pub(crate) lap: u16,
    /// Pages the pass may still purge, below 2^31.
    pub(crate) pages: u32,
    /// The pass takes runs of free pages of any length, not only long ones
    /// ([`super::policy::PURGE_ANY_RUN_EVERY`]).
    pub(crate) any: bool,
    /// The pass began with nothing to return over the floor: it walks a lap to count what a
    /// purge could return, and purges nothing ("The floor" in the module documentation).
    pub(crate) count: bool,
}

/// The pass word's flag for a pass that takes runs of any length, above its pages.
const ANY: u64 = 1 << 31;
/// The pass word's flag for a pass that only counts, below [`ANY`].
const COUNT: u64 = 1 << 30;

/// What one of `maintain`'s steps of a purge did.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct PassStep {
    /// Another step would do more.
    pub more: bool,
    /// The bytes it returned to the OS that had been resident.
    pub returned: usize,
}

/// Which free pages a purge walk may take.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Scope {
    /// Only units that start at or above their region's frontier (`region::floor`): pages
    /// that no fill of the last two intervals reached. What the clocked pass takes.
    Idle,
    /// As `Idle`, in runs of any length: what the clocked pass takes once in
    /// [`super::policy::PURGE_ANY_RUN_EVERY`]. `Idle` takes runs of at least
    /// [`super::policy::PURGE_MIN_RUN_PAGES`].
    IdleAnyRun,
    /// Every free unit: what growth over the cap and `trim` take.
    All,
    /// Every free unit, in runs of at least [`super::policy::PURGE_MIN_RUN_PAGES`]: what
    /// `maintain`'s steps of the clocked pass take in a silence, while more than the floor
    /// could be returned. From the top of each region down, so what lies above the frontiers
    /// goes first and the lowest pages stay.
    Quiet,
    /// As `Quiet`, in runs of any length.
    QuietAnyRun,
}

impl Scope {
    /// The scope of a clocked pass's step: in a silence or not, a pass that takes runs of any
    /// length or not.
    const fn of_pass(quiet: bool, any: bool) -> Scope {
        match (quiet, any) {
            (false, false) => Scope::Idle,
            (false, true) => Scope::IdleAnyRun,
            (true, false) => Scope::Quiet,
            (true, true) => Scope::QuietAnyRun,
        }
    }
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
            pages: (word & (COUNT - 1)) as u32,
            any: word & ANY != 0,
            count: word & COUNT != 0,
        }
    }

    pub(crate) const fn pack(self) -> u64 {
        (self.tag as u64) << 48
            | (self.lap as u64) << 32
            | (self.pages as u64 & (COUNT - 1))
            | if self.any { ANY } else { 0 }
            | if self.count { COUNT } else { 0 }
    }
}

impl<O: Os> Back<'_, O> {
    /// Bytes of free slots in the bitmaps that are believed to be resident, small and large
    /// classes together: what decays, and what the cap limits.
    #[inline]
    pub fn retained(&self) -> usize {
        self.clamped(false).saturating_add(self.clamped(true))
    }

    /// Whether retained memory is over the floor that the clocked pass leaves alone.
    pub fn over_floor(&self) -> bool {
        self.retained() > self.floor()
    }

    /// What the clocked pass leaves retained (`policy::allowance_floor`).
    fn floor(&self) -> usize {
        #[cfg(test)]
        if self.state.decay.unfloored.load(Relaxed) {
            return 0;
        }
        allowance_floor(self.space.plan().geometry())
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
            let old = tally.fetch_add(bytes, SeqCst);
            // Whoever raises the count past the floor has the line already: an add that
            // crosses an eighth of the floor looks at the whole count, and over the floor asks
            // a slow path to look at the level (`crate::thread_word`).
            // Every space is made on the OS's geometry, so the step is a constant.
            let floor = const { allowance_floor(O::GEOMETRY) };
            if crate::thread_word::crosses(old, bytes, floor / 8) {
                // The hold's floor is on large memory alone: a crossing that takes it over
                // tells the background thread, if its plan for the hold waited for that.
                if !class.is_small() && self.retained_large() > floor {
                    self.hold_level();
                }
                if self.over_floor() {
                    self.state.thread.raise_level();
                }
            }
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

    /// Charges `bytes` that a fill or a large allocation has just carved to the growth debt; once
    /// the debt is a step, takes it and purges as much, or what is over the cap if that is
    /// less. The caller holds its slots by now, so the walk cannot take them.
    ///
    /// The add and the take are separate atomic steps, so two growers may both see a step:
    /// the first takes it, and the second takes what accrued since, under a step, and does
    /// nothing. The few bytes it drops cost one purge later.
    ///
    /// While the background thread runs, the debt is the thread's to pay ([`Back::growth_due`]),
    /// and any slow path may add to it.
    #[inline]
    pub(super) fn grow(&self, bytes: usize) {
        if bytes == 0 {
            return;
        }
        let step = growth_step(self.space.plan().geometry());
        // Sequentially consistent: a grower that leaves the debt to the thread then reads the
        // thread's word, and the thread sets its mark before it reads the debt
        // (`crate::thread_word`, "Sleeping"). On x86-64 the add is the same locked instruction.
        let old = self.state.growth.fetch_add(bytes, SeqCst);
        if old.wrapping_add(bytes) >= step {
            self.growth_due(old, bytes, step);
        }
    }

    /// A step or more of growth is owed, after an add of `bytes` to a debt of `old`: `grow`'s
    /// rare part. Where slow paths return memory, takes the debt and pays a step of it
    /// ([`Back::pay_growth`]). While the background thread runs it is the thread's: once a
    /// step of growth, when the add crosses a multiple of the step, the grower wakes the thread
    /// if retained memory is over the cap, and otherwise drops the debt as a payment under the
    /// cap would. Unless more than [`policy::GROWTH_UNPAID_HUGE_PAGES`] of growth was unpaid
    /// before this add: then the thread has not run, and the grower pays a step itself (and the
    /// heartbeats too, `note_for_thread`), so that the bound on retained memory does not rest on
    /// the scheduler (design: "Heartbeat").
    /// Out of line, because inlined into `alloc_at` it made the compiler keep the class's
    /// layout on the stack, and every large allocation paid 19 more stores and 18 more loads
    /// for it (4% on a pipeline of large objects; `docs/research/memory.md`, "Small heaps").
    ///
    /// [`policy::GROWTH_UNPAID_HUGE_PAGES`]: super::policy::GROWTH_UNPAID_HUGE_PAGES
    #[cold]
    #[inline(never)]
    fn growth_due(&self, old: usize, bytes: usize, step: usize) {
        let thread = &self.state.thread;
        let geometry = self.space.plan().geometry();
        if thread.load().duty() == Duty::Slow {
            let _ = self.pay_growth(step);
        } else if old >= growth_unpaid(geometry) {
            let _ = self.pay_growth(step);
            // The thread is behind: what this step left owed is its to pay.
            thread.rouse_any(self.space.os());
        } else if crate::thread_word::crosses(old, bytes, step) {
            if self.retained() > retained_cap(geometry) {
                thread.rouse_any(self.space.os());
            } else {
                // What a payment under the cap does: takes the debt and returns nothing.
                self.state.growth.swap(0, SeqCst);
            }
        }
    }

    /// Takes the growth debt and purges toward it: a step of at most `PURGE_STEP_PAGES`, and
    /// the rest stays owed. Returns the bytes purged that had been resident.
    #[cold]
    #[inline(never)]
    fn pay_growth(&self, step: usize) -> usize {
        let geometry = self.space.plan().geometry();
        let debt = self.state.growth.swap(0, SeqCst);
        let owed = growth_goal(debt, self.retained(), retained_cap(geometry), step);
        // A payment may run on the slow path of the request that carved, so it returns at most
        // what a heartbeat's step may: a carve of one large slot can owe tens of megabytes. The
        // background thread takes the same steps.
        let goal = owed.min(PURGE_STEP_PAGES as usize * geometry.page_size());
        // One circuit of the heap from wherever the shared walk stands, counted in regions
        // finished: the lap count says nothing here, since the walk may stand one region from
        // the bottom with everything purgeable above it. Frontiers or not: this is growth the
        // program asked for now, and the walk carries on from here at the next payment.
        let regions = self.walk_regions().max(1);
        let (mut spent, mut finished, mut purged) = (0, 0, 0);
        while purged < goal && spent < PURGE_CAP_WORDS && finished < regions {
            let (read, bytes, done) =
                self.purge_step(PURGE_CAP_WORDS - spent, goal - purged, Scope::All, false);
            spent += read;
            purged += bytes;
            finished += usize::from(done);
        }
        // The rest stays owed, for the next heartbeats and carves to pay a step each of, if
        // this walk found anything to return. If not, the debt is cleared, as it always was:
        // a heap over the cap by fragmentation would otherwise owe a walk that returns nothing
        // on every heartbeat until the debt ran out.
        if owed > goal && purged != 0 {
            self.state.growth.fetch_add(owed - goal, SeqCst);
        }
        purged
    }

    /// The regions the walk goes round: those below the highest ever given to a class.
    pub(super) fn walk_regions(&self) -> usize {
        (self.state.walk.top.load(Relaxed) as usize).min(self.space.plan().objects())
    }

    /// The count of intervals in which the program ran, as the last heartbeat saw it, which
    /// claims record their frontiers under and the clocked pass reads them by. One load from a
    /// line written a few times an interval.
    #[inline(always)]
    pub(super) fn epoch(&self) -> u64 {
        self.state.decay.epoch.load(Relaxed) & EPOCH_COUNT
    }

    /// Whether the process is in a silence at counted time `now`: a whole interval has passed
    /// since the last one in which a heartbeat ran. Only `maintain`'s readings step the counted
    /// clock that far without a heartbeat (`crate::clock`), so where nothing calls it there is
    /// no silence.
    pub(super) fn silent(&self, now: u64) -> bool {
        let last = self.state.decay.epoch.load(Relaxed) >> 32;
        (now / DECAY_TICKS) & EPOCH_COUNT >= last + 2
    }

    /// What a purge could still return by the count of the last lap: what that lap saw, less
    /// what has been purged since. `None` until a lap has ended.
    pub(crate) fn returnable(&self) -> Option<usize> {
        let walk = &self.state.walk;
        match walk.returnable.load(Relaxed) {
            super::NO_LAP => None,
            seen => Some(seen.saturating_sub(walk.returned.load(Relaxed))),
        }
    }

    /// Counts a heartbeat's interval (`epoch_after`). Written once an interval, by the first
    /// heartbeat to see the new one.
    pub(super) fn note_epoch(&self, now: u64) {
        let word = &self.state.decay.epoch;
        let mut seen = word.load(Relaxed);
        while let Some(next) = epoch_after(seen, now) {
            match word.compare_exchange(seen, next, Relaxed, Relaxed) {
                Ok(_) => return,
                Err(found) => seen = found,
            }
        }
    }

    /// The counted time (`crate::clock`): what every clocked rule of the heap reads, the
    /// depot's included. May take a step of the count.
    pub fn now(&self) -> u64 {
        self.state.decay.clock.now(self.space.os())
    }

    /// The counted intervals, for the depot's clock to read its time from.
    pub(crate) fn clock(&self) -> &crate::clock::Counted {
        &self.state.decay.clock
    }

    /// The maintenance a slow path of the back end's own does on its way out (a run's
    /// allocation or free): a step of each purge, or, while the background thread runs, what
    /// [`Back::heartbeat_for`] leaves a slow path, waking the thread for work that came due.
    pub fn heartbeat(&self) {
        let thread = &self.state.thread;
        if self.heartbeat_for(&|| self.now(), thread.load()) {
            thread.rouse_any(self.space.os());
        }
    }

    /// [`Back::heartbeat`] where slow paths return memory, for a caller that keeps its own
    /// reading of the clock: `now` is called only when the step needs the time.
    pub fn heartbeat_with(&self, now: &dyn Fn() -> u64) {
        let _ = self.purge_steps(now);
    }

    /// The purge part of a slow path's heartbeat, where `seen` is the background thread's word
    /// as the slow path read it. Where slow paths return memory ([`Duty::Slow`]), one bounded
    /// step of each purge ([`Back::purge_steps`]). While the thread runs, none: the readings
    /// that keep the clocked rules' time, and whether this one made work due that the thread
    /// does, for the caller to wake it (design: "Heartbeat"): a pass it started (an interval's
    /// pass starts here, as it would where slow paths return memory, and the thread steps it),
    /// or a step of the fine count that reached the hold's next look. Each of those happens
    /// on one reading, and its write comes before the caller reads the thread's word afresh
    /// (`crate::thread_word`, "Sleeping").
    pub fn heartbeat_for(&self, now: &dyn Fn() -> u64, seen: Snapshot) -> bool {
        match seen.duty() {
            Duty::Slow => {
                let _ = self.purge_steps(now);
                false
            }
            Duty::Thread => self.note_for_thread(now),
        }
    }

    /// One bounded step of each purge a heartbeat takes: the clocked pass (started if due),
    /// the hold's look, and growth owed. What a slow path's heartbeat does where there is no
    /// background thread, and what the thread does in a loop while it runs. Says whether
    /// another call would do more now, and what it returned.
    pub fn purge_steps(&self, now: &dyn Fn() -> u64) -> PassStep {
        self.purge_steps_of(now, StepSize::HEARTBEAT)
    }

    /// [`Back::purge_steps`] for the background thread, which takes them while it runs: the
    /// same bounds, but its readings are not the program running (`StepSize::THREAD`).
    pub fn thread_purge_steps(&self, now: &dyn Fn() -> u64) -> PassStep {
        self.purge_steps_of(now, StepSize::THREAD)
    }

    fn purge_steps_of(&self, now: &dyn Fn() -> u64, size: StepSize) -> PassStep {
        // The hold's fine count steps on the reading the decay step makes between passes, so
        // that the hold reads the clock itself only while a pass is under way (`hold`).
        let read = core::cell::Cell::new(None);
        let mut returned = 0;
        let pass = self.decay_step_counting(
            &|| {
                let time = now();
                read.set(Some(time));
                time
            },
            size,
            &mut returned,
        );
        let time = read.get().unwrap_or_else(now);
        // The thread's look is `maintain`'s (`Back::hold_maintain_step`): the whole hold while
        // the program runs, and in a silence the shorter one that returns a quiet program's
        // held slots at the thread's second round. A heartbeat's look, taken by the thread
        // between the steps of its round, would take each fine step's one look first with the
        // whole hold, and the round's would never apply (review finding 5).
        let hold = if size.heartbeat {
            self.hold_beat(time)
        } else {
            self.hold_maintain_step(&|| time)
        };
        let (paid, owing) = self.pay_owed();
        PassStep {
            more: pass.is_some() || hold.more || owing,
            returned: returned + hold.returned + paid,
        }
    }

    /// Growth a carve owed past what its own payment could take, a step at a time: the
    /// payment that keeps memory within the cap of what is live. Not while a clocked pass is
    /// under way: its goal, set when it began, already counts what is over the cap, and both
    /// paying it would take the count below the cap. Returns what the step returned, and
    /// whether a step is still owed.
    fn pay_owed(&self) -> (usize, bool) {
        let step = growth_step(self.space.plan().geometry());
        let owed = || {
            self.state.decay.word.load(SeqCst) & ACTIVE == 0
                && self.state.growth.load(SeqCst) >= step
        };
        if !owed() {
            return (0, false);
        }
        let paid = self.pay_growth(step);
        (paid, owed())
    }

    /// A slow path's heartbeat while the background thread returns memory: the readings that
    /// keep the clocked rules' time (the interval's number, the decay word's restamp, the
    /// hold's fine count), and the start of a pass that is due, as a heartbeat starts one where
    /// slow paths return memory (one with nothing over the floor is ended as it starts). Whether
    /// this reading started a pass, or stepped the fine count to the hold's next look: work
    /// for the thread. No bitmap is read and nothing is returned to the OS.
    fn note_for_thread(&self, now: &dyn Fn() -> u64) -> bool {
        let state = self.state;
        let seen = state.decay.word.load(Acquire);
        let time = now();
        let mut started = false;
        if seen & ACTIVE == 0 {
            self.note_epoch(time);
            match decay_look(seen, time) {
                Decay::Restamp(word) => {
                    let _ = state
                        .decay
                        .word
                        .compare_exchange(seen, word, Release, Relaxed);
                }
                Decay::Start(word) => started = self.start_pass(seen, word),
                _ => {}
            }
        } else if time.wrapping_sub(seen & !ACTIVE) >= DECAY_TICKS
            && time.wrapping_sub(seen & !ACTIVE) < 1 << 63
        {
            // A pass still under way an interval after it began has not been stepped by the
            // thread: starved, or a thread a fork child inherited the word of. It is the slow
            // paths' again, a heartbeat's step at a time, and the thread is woken: while it is
            // under way no growth is paid, and the bound on retained memory must not rest on
            // the scheduler.
            let _ = self.decay_step(now, StepSize::HEARTBEAT);
            started = true;
        }
        // Growth the thread has left unpaid past the limit is the slow paths' again, a step a
        // heartbeat, as it is where there is no thread: a grower's step alone pays half a
        // megabyte while a carve may owe tens, and the thread may be waiting for a CPU behind
        // the very threads that carve. One load from a line only growers write. Whether a
        // pass is under way or not, as a grower pays: the rule that waits for a pass to end
        // keeps the payments from going below the cap, and a pass the thread does not step
        // would turn the bound off. What the step leaves owed is work for the thread, which may
        // have read the debt as paid.
        let mut owing = false;
        let geometry = self.space.plan().geometry();
        if self.state.growth.load(Relaxed) >= growth_unpaid(geometry) {
            let step = growth_step(geometry);
            let _ = self.pay_growth(step);
            owing = self.state.growth.load(SeqCst) >= step;
        }
        self.hold_note(time) || started || owing
    }

    /// Whether purge work is due that the background thread should do before it sleeps: growth
    /// owed outside a clocked pass, a pass under way or due over the floor, or a step of the
    /// fine count that reached the hold's next look. The thread's last look, after it marked
    /// its word asleep (`crate::thread_word`, "Sleeping"): every read is sequentially
    /// consistent, and the reading of the counted time may step the counts itself. A pass that
    /// is under way but not yet published is its starter's to tell the thread about.
    pub fn purge_due(&self, now: &dyn Fn() -> u64) -> bool {
        let state = self.state;
        let step = growth_step(self.space.plan().geometry());
        let seen = state.decay.word.load(SeqCst);
        if seen & ACTIVE == 0 && state.growth.load(SeqCst) >= step || self.pass_published() {
            return true;
        }
        let time = now();
        if matches!(decay_look(seen, time), Decay::Start(_)) && self.retained() > self.floor() {
            return true;
        }
        // In a silence the hold is the round's (`Back::thread_purge_steps`).
        self.hold_due(time) && !self.silent(time)
    }

    /// The clocked pass the heartbeats of an interval would take, in one call: starts a pass
    /// if one is due, and steps the pass under way until it ends, by meeting its goal or going
    /// once round the heap. It stops when that pass ends or the steps stop for another reason
    /// (nothing is due, or the pass is not yet published), and never goes on into a later
    /// pass, so it reads at most one lap of the bitmaps. Between passes it costs what a
    /// heartbeat's step costs: a load and a clock read.
    pub fn decay_pass(&self, now: &dyn Fn() -> u64) {
        let Some(pass) = self.decay_step(now, StepSize::HEARTBEAT) else {
            return;
        };
        while self.decay_step(now, StepSize::HEARTBEAT) == Some(pass) {}
    }

    /// One bounded step of the clocked pass under way or due, for `maintain`'s steps: whether
    /// the pass has more to do. Smaller than a heartbeat's step, in bitmap read and in pages
    /// purged (`StepSize::MAINTAIN`), so that a caller that yields between steps holds its
    /// thread for tens of microseconds.
    pub fn decay_pass_step(&self, now: &dyn Fn() -> u64) -> PassStep {
        let mut returned = 0;
        let more = self
            .decay_step_counting(now, StepSize::MAINTAIN, &mut returned)
            .is_some();
        PassStep { more, returned }
    }

    /// [`Back::decay_step_counting`], for a caller that does not count what it returned.
    fn decay_step(&self, now: &dyn Fn() -> u64, size: StepSize) -> Option<u64> {
        self.decay_step_counting(now, size, &mut 0)
    }

    /// One heartbeat's part of the clocked pass: starts a pass if the counted interval is later
    /// than the last pass's and anything is retained (`decay_look`), and takes one bounded step
    /// of the pass under way. A pass that is due finds nothing retained is ended as it starts,
    /// so the next one is due in the next interval. Returns the decay word of the pass it stepped if it
    /// stopped at its budget with the pass still under way, so that more steps would do more;
    /// `None` otherwise. Adds the bytes it returned that had been resident to `returned`.
    fn decay_step_counting(
        &self,
        now: &dyn Fn() -> u64,
        size: StepSize,
        returned: &mut usize,
    ) -> Option<u64> {
        let state = self.state;
        let geometry = self.space.plan().geometry();
        let page = geometry.page_size();
        let mut seen = state.decay.word.load(Acquire);
        // `decay_look` steps a pass under way without the time, as the depot's heartbeat reads
        // the clock only when a decision needs it (`depot::heartbeat`, `Clock`: a read on every
        // heartbeat cost a pipeline 3%). Between passes a heartbeat reads the clock, which
        // counts its interval; a heartbeat's step notes it as it finishes a region and as the
        // pass ends. `maintain`'s steps never do: its readings step the counted clock through a
        // silence, and a silence must not age the frontiers. Only a pass that is due reads the
        // retained count: its tallies are written by frees and claims, and reading them on every
        // heartbeat took their lines from the threads writing them (`docs/research/memory.md`,
        // "The cost of a large block, by cause").
        let time = if seen & ACTIVE != 0 { 0 } else { now() };
        if seen & ACTIVE == 0 && size.heartbeat {
            self.note_epoch(time);
        }
        match decay_look(seen, time) {
            Decay::Wait => return None,
            Decay::Restamp(word) => {
                let _ = state
                    .decay
                    .word
                    .compare_exchange(seen, word, Release, Relaxed);
                return None;
            }
            Decay::Start(word) => {
                if !self.start_pass(seen, word) {
                    return None;
                }
                seen = word;
            }
            Decay::Step => {}
        }
        let tag = Pass::tag_of(seen);
        let (mut spent, mut freed) = (0, 0);
        // A step of `maintain`'s in a silence may go below the frontiers; a heartbeat's never
        // does, since a heartbeat is the program running.
        let quiet = !size.heartbeat && self.silent(now());
        loop {
            let pass = Pass::unpack(state.walk.pass.load(Acquire));
            if pass.tag != tag {
                // The starter has not published this pass yet, or it is over and another
                // has begun: either way it is not this stepper's. But a starter that vanished
                // between its two writes (a fork abandons it there) never publishes, and
                // only steppers end passes. So a pass that is still unpublished an interval
                // after it began is ended here, and the next interval starts a fresh one. A
                // clock behind the start re-stamps it, so that its age can be told.
                let now = self.now() & !ACTIVE;
                let age = now.wrapping_sub(seen & !ACTIVE);
                if age >= 1 << 63 {
                    let _ = state
                        .decay
                        .word
                        .compare_exchange(seen, now | ACTIVE, Release, Relaxed);
                } else if age >= DECAY_TICKS {
                    let _ = state
                        .decay
                        .word
                        .compare_exchange(seen, now, Release, Relaxed);
                }
                return None;
            }
            if (pass.pages == 0 && !pass.count) || self.went_round(pass) {
                // The goal is met or the walk has gone round. End the pass this thread saw,
                // and only that one.
                let now = self.now();
                if size.heartbeat {
                    self.note_epoch(now);
                }
                let now = now & !ACTIVE;
                let _ = state
                    .decay
                    .word
                    .compare_exchange(seen, now, Release, Relaxed);
                return None;
            }
            if spent >= size.words || freed >= size.pages {
                return Some(seen);
            }
            let limit = pass.pages.min(size.pages - freed) as usize * page;
            let (read, purged, finished) = self.purge_step(
                size.words - spent,
                limit,
                Scope::of_pass(quiet, pass.any),
                pass.count,
            );
            spent += read;
            *returned += purged;
            if finished && size.heartbeat {
                // A long lap goes region by region, so the interval number lags the clock by
                // at most the walk of one region.
                self.note_epoch(self.now());
            }
            let done = (purged / page).min(u32::MAX as usize) as u32;
            freed = freed.saturating_add(done);
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

    /// Starts the clocked pass that a decay word read as `seen` and a reading of the clock say
    /// is due, storing `word` (`Decay::Start`), if anything over the floor is retained;
    /// whether this call started it. A pass with nothing to do is ended as it starts, so that
    /// the heartbeats of the next interval wait instead of reading the count.
    fn start_pass(&self, seen: u64, word: u64) -> bool {
        let state = self.state;
        let geometry = self.space.plan().geometry();
        let retained = self.retained();
        let floor = self.floor();
        if retained <= floor {
            // Nothing is retained, or no more than the floor, which is spent freely.
            let _ = state
                .decay
                .word
                .compare_exchange(seen, word & !ACTIVE, Release, Relaxed);
            return false;
        }
        // The once-an-interval look at the level: while the heap stays over the floor, a slow
        // path is asked to look at the background thread's word again, so that a start refused
        // for a passing reason is tried again (`crate::thread_word`).
        state.thread.raise_level();
        if state
            .decay
            .word
            .compare_exchange(seen, word, Release, Relaxed)
            .is_err()
        {
            return false;
        }
        // A quarter of what is retained, or what is over the cap if that is more, and never past
        // the floor of what a purge could return, by the last lap's count (before the first lap
        // has ended, by the retained count, which is never less). Nothing over the floor: the
        // pass walks a lap to count, and returns nothing.
        let over = retained.saturating_sub(retained_cap(geometry));
        let returnable = self.returnable().unwrap_or(retained);
        let goal = decay_goal(geometry, retained)
            .max(over)
            .min(returnable.saturating_sub(floor));
        // One pass in `PURGE_ANY_RUN_EVERY` takes runs of any length, counted by passes, not by
        // the interval's number, which a pass that comes every other interval could miss for
        // ever. The count is a hint: a lost update moves the pass that takes short runs by one.
        let any = state
            .walk
            .passes
            .fetch_add(1, Relaxed)
            .wrapping_add(1)
            .is_multiple_of(PURGE_ANY_RUN_EVERY as u32);
        // From where the walk stands: a pass that started at the top every time spent its goal
        // on the regions above whatever a quiet program revived each interval, and never
        // reached the free pages of the lowest regions. The pass word is written last and once:
        // until then no stepper finds its tag and none acts. Sequentially consistent, as a
        // starter that leaves the pass to the background thread then reads the thread's word,
        // and the thread marks its word before it reads this one (`crate::thread_word`,
        // "Sleeping"): on x86-64 one exchange an interval.
        let from = state.walk.purge_at.load(Relaxed);
        state.walk.pass_from.store(from, Relaxed);
        let pass = Pass {
            tag: Pass::tag_of(word),
            lap: state.walk.laps.load(Relaxed) as u16,
            pages: if goal == 0 {
                0
            } else {
                pages_of(goal, geometry.page_size())
            },
            any,
            count: goal == 0,
        };
        state.walk.pass.store(pass.pack(), SeqCst);
        true
    }

    /// Whether a clocked pass is under way and published, so that a step of it would do
    /// something: its decay word's tag is on the pass word.
    fn pass_published(&self) -> bool {
        let seen = self.state.decay.word.load(SeqCst);
        seen & ACTIVE != 0
            && Pass::unpack(self.state.walk.pass.load(SeqCst)).tag == Pass::tag_of(seen)
    }

    /// Whether the walk has gone once round since `pass` began: a lap has ended since, and the
    /// walk is back at or below where the pass began. A pass that began at the top is round
    /// when the lap ends, as every pass was before passes resumed the walk.
    fn went_round(&self, pass: Pass) -> bool {
        let walk = &self.state.walk;
        if walk.laps.load(Relaxed) as u16 == pass.lap {
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
                    .purge_region(
                        class,
                        region,
                        usize::MAX,
                        usize::MAX,
                        false,
                        Scope::All,
                        false,
                    )
                    .0;
            }
        }
        purged
    }

    /// One step of the walk: works on the region at the walk's position, and moves the
    /// position on if that region is done. Stops after `budget` words of bitmap or once it
    /// has purged `limit` bytes, finishing the unit it is on, or with `counting`, walks on to
    /// count once it has. What it sees a purge could return goes into the lap's count, and a
    /// step that ends a lap makes that count the one the clocked pass is held to. Returns the
    /// words of bitmap read, at least one, the bytes purged, and whether it finished its
    /// region.
    fn purge_step(
        &self,
        budget: usize,
        limit: usize,
        scope: Scope,
        counting: bool,
    ) -> (usize, usize, bool) {
        let plan = self.space.plan();
        let top = self.walk_regions();
        let seen = self.state.walk.purge_at.load(Relaxed);
        // The position is the region to look at next plus one; zero means the top, the highest
        // region ever given to a class.
        let index = if seen == 0 || seen as usize > top {
            top.saturating_sub(1)
        } else {
            seen as usize - 1
        };
        let Some(region) = plan.region(index) else {
            return (1, 0, true);
        };
        let (purged, read, done, found) = match self.purgeable(region) {
            Some(class) => self.purge_region(class, region, budget, limit, true, scope, counting),
            None => (0, 1, true, 0),
        };
        let walk = &self.state.walk;
        if found != 0 {
            walk.seen.fetch_add(found, Relaxed);
        }
        if done
            && walk
                .purge_at
                .compare_exchange(seen, index as u32, Relaxed, Relaxed)
                .is_ok()
            && index == 0
        {
            // The lap's count, and what has been purged since, start again. Steppers racing
            // with this add to one lap or the other: the count is a hint.
            walk.returnable.store(walk.seen.swap(0, Relaxed), Relaxed);
            walk.returned.store(0, Relaxed);
            walk.laps.fetch_add(1, Relaxed);
        }
        (read.max(1), purged, done)
    }

    /// The class of a region the walk should look at: a class region whose summary bit is set.
    pub(super) fn purgeable(&self, region: RegionId) -> Option<Class> {
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
    /// rest of the region under it. With `counting`, once it has purged `limit` bytes it walks
    /// on within its budget, looking and purging nothing. Returns the bytes purged, the words
    /// read, whether the walk reached the bottom, or the frontier, and the bytes of the free,
    /// resident pages it saw and left: what a purge could still return.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn purge_region(
        &self,
        class: Class,
        region: RegionId,
        budget: usize,
        limit: usize,
        resume: bool,
        scope: Scope,
        counting: bool,
    ) -> (usize, usize, bool, usize) {
        let Some(host) = self.host(class) else {
            return (0, 1, true, 0);
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
            Scope::Idle | Scope::IdleAnyRun => region::floor(&host, region, self.epoch()),
            Scope::All | Scope::Quiet | Scope::QuietAnyRun => 0,
        };
        // A call costs about the same whatever it returns, so the clocked pass leaves short
        // runs for a later pass, which takes runs of any length once in a while.
        let min_run = match scope {
            Scope::Idle | Scope::Quiet => PURGE_MIN_RUN_PAGES,
            Scope::IdleAnyRun | Scope::QuietAnyRun | Scope::All => 1,
        };
        if cursor != 0 && cursor <= floor {
            // Everything the cursor has left is below the frontier.
            if resume {
                set_cursor(hints, cursor, 0);
            }
            return (0, 1, true, 0);
        }
        let page = layout.page_size();
        let (mut purged, mut read, mut found) = (0, 0, 0);
        if cursor == 0 {
            // At the top of the region: bring the watermark up to the end of its huge page,
            // if that page could then be purged whole.
            let (bytes, words) = region::top_up(&host, region, floor);
            self.retain(class, bytes);
            read += words;
        }
        let mut first = true;
        // Where the last run this call purged began: a run that ends there is the tail of a
        // longer one that the cap on a run's length cut, and goes whatever its length.
        let mut below_purged = None;
        let done = loop {
            // Not before the first unit: the look above may have spent the whole budget, and
            // a call that then gave up would leave the cursor at the top for the next call to
            // do the same, for ever.
            if !first && (read >= budget || (purged >= limit && !counting)) {
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
                Step::Unit(unit) if purged >= limit => {
                    // Counting only.
                    let (ok, words) = region::purgeable_unit(&host, region, unit);
                    read += words;
                    if ok {
                        let (dirty, words) = region::dirty_pages(&host, region, unit);
                        read += words;
                        found += dirty * page;
                    }
                    unit.start
                }
                Step::Unit(unit) => {
                    // The run of purgeable units that ends at this one, down to the floor.
                    // Within what the step may still purge and read, so that a run does not
                    // carry the step far past its goal or its budget.
                    // Never cut to under `min_run`, or a step with a few pages of its goal
                    // left would count every run it met short and step over it: it may go
                    // past its goal by up to `min_run - 1` pages instead.
                    let pages = limit
                        .saturating_sub(purged)
                        .div_ceil(page)
                        .max(min_run as usize);
                    let room = Room {
                        floor,
                        pages: (pages.min(PURGE_MAX_RUN_PAGES as usize)) as u32,
                        words: budget.saturating_sub(read).clamp(1, PURGE_MAX_RUN_WORDS),
                    };
                    // A unit longer than the step may still return is one large slot (a
                    // unit of the small-page part is the pages of the slots it covers). Cut
                    // to what the step may still purge, not to a run's length: a run's bound
                    // keeps a call's claims of many slots short, and a piece of one slot
                    // claims one. In whole huge pages for a class of a huge page or more, so
                    // that a slot that kept its huge-page advice is not left with a huge page
                    // partly unmapped, which stays charged whole.
                    let huge = layout.huge_pages();
                    let granule = if layout.size() >= huge as usize * page {
                        huge
                    } else {
                        1
                    };
                    let most = (pages.min(u32::MAX as usize) as u32).max(granule);
                    if huge_from.is_none_or(|from| unit.end <= from) && unit.end - unit.start > most
                    {
                        // The whole slot first: a slot that is not free, or all clean, is
                        // passed at a look, not a piece at a time; and a piece is cut below
                        // the slot's clean top, what earlier pieces returned.
                        let (ok, words) = region::purgeable_unit(&host, region, unit);
                        read += words;
                        if !ok {
                            unit.start
                        } else {
                            let (top, words) = region::clean_top(&host, region, unit);
                            read += words;
                            if top <= unit.start {
                                // Another purger cleaned the rest since the look.
                                unit.start
                            } else {
                                let cut = piece(Unit { end: top, ..unit }, most, min_run, granule);
                                // What the piece leaves resident counts; the rest of the slot,
                                // below it, is the next step's unit and counts then.
                                let (dirty, words) = region::dirty_pages(&host, region, cut);
                                read += words;
                                found += dirty * page;
                                let (outcome, _) = region::purge_unit(&host, region, cut);
                                if let Purge::Done(bytes) = outcome {
                                    purged += bytes;
                                    found = found.saturating_sub(bytes);
                                    below_purged = Some(cut.start);
                                }
                                cut.start
                            }
                        }
                    } else {
                        let (run, words) =
                            self.run_below(&host, region, unit, carved, huge_from, room);
                        read += words;
                        // What the run leaves resident after this step counts toward the lap.
                        if let Some(run) = run {
                            let (dirty, words) = region::dirty_pages(&host, region, run);
                            read += words;
                            found += dirty * page;
                        }
                        match run {
                            Some(run)
                                if run.end - run.start < min_run
                                    && below_purged != Some(run.end) =>
                            {
                                run.start
                            }
                            Some(run) => {
                                // Its look before claiming reads the words `run_below` just read
                                // and counted; counting them twice halved what a step could purge.
                                let (outcome, _) = region::purge_unit(&host, region, run);
                                if let Purge::Done(bytes) = outcome {
                                    purged += bytes;
                                    found = found.saturating_sub(bytes);
                                    below_purged = Some(run.start);
                                }
                                run.start
                            }
                            None => unit.start,
                        }
                    }
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
        if purged != 0 {
            self.state.walk.returned.fetch_add(purged, Relaxed);
        }
        (purged, read, done, found)
    }
}

impl<O: Os> Back<'_, O> {
    /// The run of adjacent purgeable units (`region::purgeable_unit`) that ends with `unit`,
    /// extended down while the next unit below is purgeable, starts at or above `room.floor`, keeps
    /// the run within `room.pages` and the bitmap words its looks read within `room.words`
    /// (the words its purge then claims, about as many); `None` if `unit` itself is not
    /// purgeable. Units of the huge-page part are whole huge pages already and are not joined.
    /// Reads only; returns the run and the leaf words read.
    fn run_below<H: Host>(
        &self,
        host: &H,
        region: RegionId,
        unit: Unit,
        carved: u32,
        huge_from: Option<u32>,
        room: Room,
    ) -> (Option<Unit>, usize) {
        let (ok, mut read) = region::purgeable_unit(host, region, unit);
        if !ok {
            return (None, read);
        }
        let mut run = unit;
        if huge_from.is_some_and(|huge| unit.end > huge) {
            return (Some(run), read);
        }
        let layout = *host.layout();
        while let Step::Unit(next) = layout.unit_below(carved, huge_from, run.start) {
            if next.end != run.start || next.start < room.floor || run.end - next.start > room.pages
            {
                break;
            }
            let (ok, words) = region::purgeable_unit(host, region, next);
            read += words;
            if !ok || read > room.words {
                break;
            }
            run.start = next.start;
        }
        (Some(run), read)
    }
}

/// How much one step of the clocked pass may do: bitmap words read and pages purged, each
/// checked between units, so a step finishes the unit it is on.
#[derive(Clone, Copy)]
struct StepSize {
    words: usize,
    pages: u32,
    /// A heartbeat's step, which is the program running: it counts the interval
    /// (`Back::note_epoch`) and never goes below the frontiers.
    heartbeat: bool,
}

impl StepSize {
    /// A heartbeat's step (`policy::PURGE_STEP_WORDS`, `policy::PURGE_STEP_PAGES`).
    const HEARTBEAT: StepSize = StepSize {
        words: PURGE_STEP_WORDS,
        pages: PURGE_STEP_PAGES,
        heartbeat: true,
    };
    /// A step of the background thread's in the place of a heartbeat's: as large, but not
    /// the program running, so that the thread's readings through a silence neither count
    /// intervals toward a frontier's age nor keep a pass above the frontiers, as `maintain`'s.
    const THREAD: StepSize = StepSize {
        heartbeat: false,
        ..StepSize::HEARTBEAT
    };
    /// A step of `maintain`'s (`policy::PURGE_MAINTAIN_STEP_WORDS`).
    const MAINTAIN: StepSize = StepSize {
        words: PURGE_MAINTAIN_STEP_WORDS,
        pages: PURGE_MAINTAIN_STEP_PAGES,
        heartbeat: false,
    };
}

/// How far a run may extend: down to the floor, pages, and bitmap words read.
#[derive(Clone, Copy)]
struct Room {
    floor: u32,
    pages: u32,
    words: usize,
}

/// The piece of `unit`, one large slot below its clean top, that a step may purge when it may
/// purge `pages` more: its top `pages`, starting at a multiple of `granule` pages, or all of it
/// if what would be left below is shorter than `min_run` (that tail would wait for a pass that
/// takes short runs). The cursor then names the page the piece began at, and the next step's
/// unit (`Layout::unit_below`) is the rest of the slot below it.
fn piece(unit: Unit, pages: u32, min_run: u32, granule: u32) -> Unit {
    if unit.end - unit.start <= pages {
        return unit;
    }
    let start = (unit.end - pages) & !(granule - 1);
    if start <= unit.start || start - unit.start < min_run {
        return unit;
    }
    Unit { start, ..unit }
}

/// A goal of `bytes` as the pass word holds it: whole pages, at least one.
fn pages_of(bytes: usize, page: usize) -> u32 {
    bytes.div_ceil(page).clamp(1, (COUNT - 1) as usize) as u32
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
