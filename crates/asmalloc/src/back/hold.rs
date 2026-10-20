//! The hold on free large slots: they go back to the OS once their region has had no release
//! for a short hold.
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
//! **Time is counted.** The hold is measured in steps of a fine count ([`Fine`]), which a
//! reading of the counted clock (`crate::clock`) advances by at most one step, when it finds
//! that [`policy::FINE_TICKS`] have passed since the last. Readings come with heartbeats, so
//! while the program runs the count keeps up with the clock, and across a silence it moves by
//! one step: the first slow paths after a pause do not find every held slot expired and tear
//! down what the program is about to take again. The count is advanced by the reading the
//! heartbeat's clocked pass makes anyway (`Back::heartbeat_with`); only while a hold is under
//! way and that pass made none does the hold ask for one, which the depot's heartbeat answers
//! from its own reading when it made one. A process with nothing held reads no clock for it.
//!
//! **How long.** One hold for every large class, [`HOLD_STEPS`]. A hold that adapted per
//! class (doubling when a slot was claimed soon after the hold purged its region, halving on
//! the clock) held DataFusion's peak at the same level for 9% more faults, so it was not kept
//! (`docs/research/large-hold.md`).
//!
//! **The words.** One word per region: a mark that a large slot was released into it and left
//! resident, which the next hold pass turns into a stamp, the fine count (plus one) at that
//! pass; zero for neither. One `due` word: the fine count (plus one) at which a pass should
//! run, zero when nothing is held. A release marks its region (when it is not marked already)
//! and makes a pass due at once; it reads no clock word, and a free into a region already
//! marked writes nothing. A heartbeat compares `due` with the fine count, a load each, and
//! only when a pass is due walks the large classes' regions, within a bound: it stamps the
//! marked ones, purges the free units of every region whose hold has ended down to the floor,
//! and sets `due` to the next hold to end. A hold therefore runs from the first heartbeat after
//! the last release; it is never shorter for that. All of these
//! are hints: a stale mark, stamp or `due` purges a slot early or late, never one in use,
//! because a purge is a claim (`region::purge_unit`).

use super::Back;
use super::policy::{FINE_TICKS, HOLD_STEPS, PURGE_CAP_WORDS, hold_floor};
use super::purge::Scope;
use super::word::{Atomic, Word};
use crate::classes::Class;
use crate::os::Os;
use crate::space::{RegionId, RegionTag};
use core::sync::atomic::Ordering::Relaxed;
use core::sync::atomic::{AtomicU32, AtomicU64};

/// The fine count: its value in the high 32 bits, and the counted time of its last step in
/// units of [`UNIT`] ticks in the low 32. All zero is a valid state (no step yet).
pub(crate) struct Fine {
    word: AtomicU64,
}

/// The unit of the fine count's stamp: 2^20 ticks, a fraction of a millisecond.
const UNIT_SHIFT: u32 = 20;

/// A fine step in units.
const STEP_UNITS: u32 = (FINE_TICKS >> UNIT_SHIFT) as u32;

const _: () = assert!(STEP_UNITS >= 1);

const fn pack(count: u32, unit: u32) -> u64 {
    (count as u64) << 32 | unit as u64
}

/// The word to store over `word` for a reading of the counted time `time`, if any: one step
/// if a fine step or more has passed since the last, however much more; a re-stamp, with no
/// step, if the time is behind the stamp.
pub(crate) const fn fine_read(word: u64, time: u64) -> Option<u64> {
    let count = (word >> 32) as u32;
    let stamp = word as u32;
    let unit = (time >> UNIT_SHIFT) as u32;
    let since = unit.wrapping_sub(stamp) as i32;
    if since < 0 {
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
    /// The fine count plus one at which some region's hold ends; zero when none is held.
    due: AtomicU32,
}

impl Hold {
    pub(crate) const fn new() -> Hold {
        Hold {
            fine: Fine::new(),
            due: AtomicU32::new(0),
        }
    }

    /// Lowers `due` to `at` (a fine count plus one), unless it is earlier already.
    fn lower(&self, at: u32) {
        let at = at.max(1);
        let mut seen = self.due.load(Relaxed);
        while seen == 0 || earlier(at, seen) != seen {
            match self.due.compare_exchange(seen, at, Relaxed, Relaxed) {
                Ok(_) => return,
                Err(found) => seen = found,
            }
        }
    }
}

/// Whether a hold stamped at `stamp` (a fine count plus one) has ended at fine count `now`:
/// [`HOLD_STEPS`] steps or more have passed. Counts wrap; an age past half the range reads as
/// a stamp from the future, which has not ended.
pub(crate) const fn hold_ended(stamp: u32, now: u32) -> bool {
    let age = now.wrapping_sub(stamp.wrapping_sub(1));
    age < 1 << 31 && age >= HOLD_STEPS
}

/// A region's stamp for a region released into since the last hold pass, which stamps it.
const RELEASED: u64 = u64::MAX;

impl<O: Os> Back<'_, O> {
    /// Region `index`'s stamp: the fine count plus one at the hold pass after the last release
    /// of a large slot into it that left it resident; [`RELEASED`] until that pass; zero for
    /// none. It is the `grow` word of the region's header in
    /// the back end's state, which a large class does not otherwise use (its header is in its
    /// metadata block, `host::RegionHeader`); a small class's region is never stamped, and the
    /// hold's pass reads the stamps of large classes' regions only.
    fn hold_word(&self, index: usize) -> Option<&Atomic> {
        self.state.headers.get(index).map(|h| &h.grow)
    }

    /// For a release of a large slot of `class` into `region` that leaves it resident: marks the
    /// region released ([`RELEASED`]) and makes sure a heartbeat looks at it. The next hold pass
    /// stamps it with the fine count, at the next heartbeat, so its hold runs from about the
    /// release and is never shorter for it, and a free reads no clock word. A load, and two
    /// stores at most when the region was not marked already.
    pub(super) fn hold_released(&self, class: Class, region: RegionId) {
        if class.is_small() {
            return;
        }
        let Some(word) = self.hold_word(region.index()) else {
            return;
        };
        if word.load() != RELEASED {
            word.store(RELEASED);
            // Due at once, so that the hold runs from about the release: a region left marked
            // until a later hold ended would be held up to twice as long, which on DataFusion
            // held as much as a hold twice as long (`docs/research/large-hold.md`).
            let due = &self.state.hold.due;
            if due.load(Relaxed) != 1 {
                due.store(1, Relaxed);
            }
        }
    }

    /// A heartbeat's part of the hold: advances the fine count for the reading of the counted
    /// time `time` the heartbeat made anyway, and runs a hold pass if a hold has ended. With
    /// nothing held, a load. With something held and no reading made (a clocked pass under
    /// way reads none), it reads the time through `now`, which the depot's heartbeat answers
    /// from its own reading when it made one.
    pub(super) fn hold_step(&self, time: Option<u64>, now: &dyn Fn() -> u64) {
        let hold = &self.state.hold;
        let due = hold.due.load(Relaxed);
        if due == 0 {
            if let Some(time) = time {
                hold.fine.note(time);
            }
            return;
        }
        hold.fine.note(time.unwrap_or_else(now));
        let now = hold.fine.count();
        // `due` is the end plus one; the hold has ended once the count reaches the end.
        if (now.wrapping_sub(due.wrapping_sub(1)) as i32) < 0 {
            return;
        }
        // Take the pass: a heartbeat that loses the race leaves it to the winner.
        if hold.due.compare_exchange(due, 0, Relaxed, Relaxed).is_err() {
            return;
        }
        self.hold_pass(now);
    }

    /// Purges the free units of every large-class region whose hold has ended at fine count
    /// `now`, within [`PURGE_CAP_WORDS`] of bitmap and down to the floor of free large memory
    /// (`policy::hold_floor`), and sets `due` to the next hold to end. At or under the floor it
    /// purges nothing and looks again a hold later.
    fn hold_pass(&self, now: u32) {
        let hold = &self.state.hold;
        let over = self
            .retained_large()
            .saturating_sub(hold_floor(self.space.plan().geometry()));
        if over == 0 {
            hold.lower(now.wrapping_add(1).wrapping_add(HOLD_STEPS));
            return;
        }
        let plan = self.space.plan();
        let top = self.walk_regions();
        let (mut spent, mut purged) = (0, 0);
        let mut next: Option<u32> = None;
        for index in 0..top {
            let Some(word) = self.hold_word(index) else {
                break;
            };
            let seen = word.load();
            if seen == 0 {
                continue;
            }
            if seen == RELEASED {
                // Released since the last pass: its hold runs from now. A release meanwhile
                // finds the stamp and marks it again.
                let stamp = now.wrapping_add(1);
                if word.compare_exchange(RELEASED, u64::from(stamp)).is_ok() {
                    let ends = stamp.wrapping_add(HOLD_STEPS);
                    next = Some(next.map_or(ends, |n| earlier(n, ends)));
                } else {
                    next = Some(now.wrapping_add(1));
                }
                continue;
            }
            let stamp = seen as u32;
            let Some(region) = plan.region(index) else {
                break;
            };
            if !matches!(self.state.table.get(region), RegionTag::Class(class) if !class.is_small())
            {
                let _ = word.compare_exchange(u64::from(stamp), 0);
                continue;
            }
            if !hold_ended(stamp, now) {
                let ends = stamp.wrapping_add(HOLD_STEPS);
                next = Some(next.map_or(ends, |n| earlier(n, ends)));
                continue;
            }
            if spent >= PURGE_CAP_WORDS {
                // Out of budget: the next heartbeat carries on.
                next = Some(now.wrapping_add(1));
                break;
            }
            if purged >= over {
                // Down to the floor: what is left waits for a later pass.
                next = Some(now.wrapping_add(1).wrapping_add(HOLD_STEPS));
                break;
            }
            // A region with nothing free (its summary bit clear) is done.
            let done = match self.purgeable(region) {
                Some(class) => {
                    let budget = PURGE_CAP_WORDS - spent;
                    let (bytes, read, done) =
                        self.purge_region(class, region, budget, over - purged, false, Scope::All);
                    spent += read;
                    purged += bytes;
                    done
                }
                None => true,
            };
            if done {
                // A release since the load keeps its own stamp.
                let _ = word.compare_exchange(u64::from(stamp), 0);
            } else {
                next = Some(now.wrapping_add(1));
            }
        }
        if let Some(next) = next {
            hold.lower(next);
        }
    }
}

/// The earlier of two fine counts plus one, near each other.
const fn earlier(a: u32, b: u32) -> u32 {
    if b.wrapping_sub(a) < 1 << 31 { a } else { b }
}
