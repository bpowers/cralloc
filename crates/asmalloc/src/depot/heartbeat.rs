//! The heartbeat: the bounded maintenance every slow path does before it returns.
//!
//! A large allocation or free counts as a slow path ([`Cache::maintain`]; the back end takes
//! its purge step itself). There is no background thread and no timer, so maintenance happens
//! at a rate set by allocation activity, in units small enough that no single call pays much.
//! The clock is read at most once per heartbeat, and only when a decision needs it.
//!
//! - **Launder over target.** If the depot stack of the class just served holds more than its
//!   target, one magazine is popped and its objects go back to the source; the empty magazine
//!   goes to the empty stack.
//! - **Shed what was not needed** (`shed.rs`). A note of the depth of the stack the slow path
//!   popped from, and on every [`LOOK_EVERY`]th slow path of a CPU a look at one class's stack,
//!   the classes in turn, which starts its new interval; then at most one magazine laundered
//!   from a stack that owes.
//! - **Age limit** (`age.rs`). A look at one class of the CPU's own slab, the classes in turn,
//!   which takes and launders the magazine loaded there if it has been loaded for an interval,
//!   or launders one full spare of a class with nothing loaded.
//! - **Purge.** One [`Source::purge_step`].
//! - **Idle check.** Each slow path on a CPU counts itself in that CPU's activity counter, and
//!   every [`LOOK_EVERY`] of them it looks at one other CPU's slab, chosen from the count, so
//!   that a CPU's looks go round every CPU number a slow path has run on. A slab is idle when
//!   its counter has not moved for [`IDLE_TICKS`] of the clock ([`Os::ticks`]). Each look
//!   compares the counter with the slab's [`Sighting`], the counter value last seen there and
//!   when: a changed counter is recorded with the time now; an unchanged one is drained if the
//!   sighting is at least the interval old and the slab has not been drained since. If the slow
//!   path met another generation's marker on its own CPU, it drains that slab instead, since
//!   such a slab stays on the slow path until drained.
//!
//! The interval is measured on the clock, not in looks or heartbeats, because those come
//! faster the harder the process allocates. A slab busy on fast paths alone does not move its
//! counter, so it looks idle and is drained once per interval; the drain costs it one refill,
//! whose slow path moves the counter, and the next drain is at least an interval after that.
//!
//! **Bounded work.** Besides the exchange itself, one slow path launders at most four
//! magazines: one in the exchange (the refresh, `slow.rs`) and one for each of the three rules
//! here. A magazine holds at most 254 objects (and at most about 128 KiB of them), and a launder
//! sorts them and sets one bitmap word per run of 64 neighbours with one atomic OR. Add one
//! purge step, at most one drain of another slab (a bounded number of atomic instructions and
//! one fence), and a fixed number of loads and single-word writes for the looks.
//!
//! **Shared writes.** A slow path that exchanges nothing with the depot writes, outside its own
//! CPU's slab: nothing, on most entries; another slab's sighting, by compare-and-swap, on one
//! entry in [`LOOK_EVERY`], and only if that slab's counter moved since its last sighting or it
//! is drained; a depot stack's water word, only when a look finds a new low, a new interval or a
//! paid debt; the depot's count of CPU numbers, once for each new highest CPU number.
//! Laundering and purging write shared state only when there is something to give back. Its own
//! slab's activity counter shares a line with the sighting that other CPUs' looks read and
//! write; the age limit's stamps are on lines only its own CPU uses.
//!
//! **Hints.** The counter and the sighting are hints in the cold part of the slab, changed with
//! single atomic instructions rather than routines; the all-zero slab has neither, and is never
//! looked at. A lost update, a race between two looks, or a clock that is wrong, differs between
//! CPUs or goes back only moves a drain earlier or later: a drain is safe at any time, and two
//! looks that both decide to drain are stopped by the sighting's compare-and-swap and, behind
//! it, by the drain's own claim.

use super::{Cache, Drained, Source, generation};
use crate::classes::{SMALL_COUNT, SmallClass};
use crate::front::{Cpu, Routines};
use crate::os::Os;
use core::cell::Cell;
use core::sync::atomic::Ordering::Relaxed;

/// `log2` of [`IDLE_TICKS`].
pub(super) const IDLE_SHIFT: u32 = 32;

/// The interval: how long a slab's activity counter must stand still before the idle check
/// drains it, how long a magazine may stay loaded (`age.rs`), and the period over which a depot
/// stack's low-water mark is kept (`shed.rs`), in ticks of [`Os::ticks`]: 2^32, about a second.
///
/// On Linux the clock is the timestamp counter, whose rate the allocator cannot learn without
/// a system call or the C library. It runs at a fixed rate near the processor's base clock: 1
/// to 6 GHz on plausible x86-64 machines, so the interval is between 0.7 and 4.3 seconds, and
/// about 1 to 2 seconds on current desktop and server parts (4.3 GHz on the reference
/// machine). The interval only paces hints, so that spread is acceptable; what matters is that
/// it is long next to the microseconds between heartbeats.
pub const IDLE_TICKS: u64 = 1 << IDLE_SHIFT;

/// How many of its own slow paths a CPU makes between looks at another CPU's slab, by default.
/// A look reads that slab's cold line and the clock, and sometimes writes the line; doing it on
/// one slow path in sixteen keeps that off the slow path's cost, and a CPU still looks at every
/// slab many times an interval while it allocates at any appreciable rate.
pub const LOOK_EVERY: u64 = 16;

/// The low bits of a tick count that a sighting or a stamp drops: they keep time in units of
/// 2^20 ticks, a fraction of a millisecond.
pub(super) const TICK_SHIFT: u32 = 20;
/// [`IDLE_TICKS`] in a sighting's units.
pub(super) const IDLE_UNITS: u32 = (IDLE_TICKS >> TICK_SHIFT) as u32;
/// Bits of the activity counter a sighting keeps.
const ACTIVITY_BITS: u32 = 30;
const ACTIVITY_MASK: u64 = (1 << ACTIVITY_BITS) - 1;
/// A sighting's flags, below the activity it saw.
const DRAINED: u64 = 1;
const SEEN: u64 = 2;
const ACTIVITY_SHIFT: u32 = 2;
const TIME_SHIFT: u32 = 32;

// Elapsed time is a wrapping difference read as signed, so the interval must be well under
// half the range of a sighting's time.
const _: () = assert!(IDLE_UNITS > 0 && IDLE_UNITS < 1 << 30);
const _: () = assert!(ACTIVITY_SHIFT + ACTIVITY_BITS == TIME_SHIFT);

/// The idle check's record of one slab, a word in its cold part: the low [`ACTIVITY_BITS`] of
/// the activity counter last seen there, when that value was first seen (bits 32 to 63, in
/// units of 2^[`TICK_SHIFT`] ticks), whether anything has been seen, and whether the slab has
/// been drained since. All zero means never looked at.
///
/// Every field is truncated, which costs at most a drain in the wrong place: an activity that
/// moved by a multiple of 2^30 between two looks reads as unchanged, and a sighting about
/// 2^51 ticks (days) old reads as new until it is older still.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct Sighting(pub(super) u64);

/// What a look at a slab decides.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Look {
    /// Nothing to do: the slab is drained, or has not been quiet for an interval.
    Wait,
    /// The counter moved: replace the sighting with this one.
    Record(Sighting),
    /// The slab has been quiet for an interval: mark the sighting drained (this one) and drain.
    Drain(Sighting),
}

impl Sighting {
    /// What a look decides that finds this sighting, the slab's counter at `activity`, and the
    /// clock at `now`.
    pub(super) fn look(self, activity: u64, now: u64) -> Look {
        let activity = activity & ACTIVITY_MASK;
        let time = (now >> TICK_SHIFT) as u32;
        if self.0 & SEEN == 0 || self.activity() != activity {
            return Look::Record(Sighting(
                (time as u64) << TIME_SHIFT | activity << ACTIVITY_SHIFT | SEEN,
            ));
        }
        if self.0 & DRAINED != 0 {
            return Look::Wait;
        }
        // Signed, so a clock that went back (another CPU's, say) reads as "not yet".
        let quiet = time.wrapping_sub(self.time()) as i32;
        if quiet < IDLE_UNITS as i32 {
            return Look::Wait;
        }
        Look::Drain(Sighting(self.0 | DRAINED))
    }

    fn activity(self) -> u64 {
        self.0 >> ACTIVITY_SHIFT & ACTIVITY_MASK
    }

    fn time(self) -> u32 {
        (self.0 >> TIME_SHIFT) as u32
    }
}

/// The clock, read at most once per heartbeat and only when a decision needs it: an idle
/// check, a look at a depot stack for a new interval, or an age look that finds a magazine
/// loaded. Reading the timestamp counter on every heartbeat cost a two-thread pipeline about
/// 3% of its time.
pub(super) struct Clock<'a, O: Os> {
    os: &'a O,
    now: Cell<Option<u64>>,
}

impl<'a, O: Os> Clock<'a, O> {
    pub(super) fn new(os: &'a O) -> Self {
        Clock {
            os,
            now: Cell::new(None),
        }
    }

    /// The time, the same for the rest of the heartbeat once read.
    pub(super) fn now(&self) -> u64 {
        match self.now.get() {
            Some(now) => now,
            None => {
                let now = self.os.ticks();
                self.now.set(Some(now));
                now
            }
        }
    }
}

/// The small class with ordinal `n` modulo the class count: how a CPU's looks go round the
/// classes, one class for each count of its activity.
fn nth_class(n: u64) -> Option<SmallClass> {
    SmallClass::from_ordinal((n % SMALL_COUNT as u64) as usize)
}

/// What a slow path noticed that its heartbeat should act on.
#[derive(Default, Debug)]
pub(super) struct Chores {
    /// The CPU the slow path ran on, if it had a valid one.
    pub(super) cpu: Option<Cpu>,
    /// It met another generation's marker on that CPU's slab.
    pub(super) stale: bool,
    /// It installed a magazine for the class it served in that CPU's slab.
    pub(super) installed: bool,
    /// It popped a magazine from the depot stack of the class it served.
    pub(super) popped: bool,
    /// Its purge step is already taken.
    pub(super) purged: bool,
}

impl<O: Os, R: Routines, S: Source> Cache<'_, O, R, S> {
    /// The heartbeat of a slow path that served no small class: a large allocation or free,
    /// which counts as a slow path of the CPU the thread is on. All but the purge step, which
    /// the back end takes inside a large allocation or free itself.
    pub fn maintain(&self) {
        let chores = Chores {
            cpu: self.routines.cpu(),
            purged: true,
            ..Chores::default()
        };
        self.heartbeat(None, chores);
    }

    /// One heartbeat, after a slow path that served `served`, if it served a small class.
    pub(super) fn heartbeat(&self, served: Option<SmallClass>, chores: Chores) {
        let clock = Clock::new(self.os());
        let activity = chores.cpu.map(|cpu| {
            if self.depot.cpus.load(Relaxed) <= cpu.index() {
                self.depot.cpus.fetch_max(cpu.index() + 1, Relaxed);
            }
            self.slab(cpu).note_activity()
        });
        if let (Some(cpu), Some(class), true) = (chores.cpu, served, chores.installed) {
            self.unstamp(cpu, class);
        }
        if let Some(class) = served {
            if self.tuning.shed_unused && chores.popped {
                self.note_depth(class, &clock);
            }
            self.launder(class, &clock);
        }
        let every = self.tuning.look_every.max(1);
        let looking = activity.filter(|activity| activity.is_multiple_of(every));
        if self.tuning.shed_unused {
            if let Some(class) = looking.and_then(|activity| nth_class(activity / every)) {
                self.look_at_depth(class, clock.now());
            }
            self.shed(activity.unwrap_or(0));
        }
        if self.tuning.age_limit
            && let (Some(cpu), Some(activity)) = (chores.cpu, activity)
            && let Some(class) = nth_class(activity)
        {
            self.age_look(cpu, class, &clock);
        }
        if !chores.purged {
            self.source.purge_step();
        }
        if !generation::drains_enabled(self.depot) {
            return;
        }
        match (chores.cpu, looking) {
            (Some(cpu), _) if chores.stale => {
                self.drain(cpu);
            }
            (Some(cpu), Some(activity)) => self.idle_check(cpu, activity, clock.now()),
            _ => {}
        }
    }

    /// Launders one magazine of `class` if its depot stack is over its target.
    fn launder(&self, class: SmallClass, clock: &Clock<'_, O>) {
        let target = self.tuning.depot_target / class.size();
        let stack = self.depot.full(class);
        if (stack.objects(self.area, class.magazine()) as usize) <= target {
            return;
        }
        if self.launder_one(class).is_some() && self.tuning.shed_unused {
            self.note_depth(class, clock);
        }
    }

    /// On the `look_every`-th slow path of CPU `me`, whose counter now reads `activity`, looks
    /// at one other CPU's slab and records or drains it (see the module documentation).
    fn idle_check(&self, me: Cpu, activity: u64, now: u64) {
        let every = self.tuning.look_every.max(1);
        // At least `me + 1`: this thread raised it to that before.
        let count = self.depot.cpus.load(Relaxed);
        if count < 2 {
            return;
        }
        let mut index = ((activity / every) % count as u64) as usize;
        if index == me.index() {
            index = (index + 1) % count;
        }
        let Some(cpu) = self.routines.slabs().cpu(index as i32) else {
            return;
        };
        let slab = self.slab(cpu);
        let activity = slab.activity();
        if activity == 0 {
            return;
        }
        let seen = slab.seen().load(Relaxed);
        // A compare-and-swap, not a store, so that a look that read the sighting before
        // another look marked it drained cannot clear the mark.
        match Sighting(seen).look(activity, now) {
            Look::Wait => {}
            Look::Record(fresh) => {
                let _ = slab
                    .seen()
                    .compare_exchange(seen, fresh.0, Relaxed, Relaxed);
            }
            Look::Drain(marked) => {
                if slab
                    .seen()
                    .compare_exchange(seen, marked.0, Relaxed, Relaxed)
                    .is_err()
                {
                    return;
                }
                // A drain that did not happen leaves the slab to be tried again; a busy one
                // means another thread is draining it now.
                if matches!(self.drain(cpu), Drained::BackedOut | Drained::Disabled) {
                    let _ = slab
                        .seen()
                        .compare_exchange(marked.0, seen, Relaxed, Relaxed);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const UNIT: u64 = 1 << TICK_SHIFT;

    #[test]
    fn a_first_look_records_and_a_quiet_interval_drains_once() {
        let start = 7 * IDLE_TICKS + 3;
        let Look::Record(first) = Sighting(0).look(5, start) else {
            panic!("a first look records");
        };
        assert_eq!(first.look(5, start), Look::Wait);
        assert_eq!(first.look(5, start + IDLE_TICKS - UNIT), Look::Wait);
        let Look::Drain(drained) = first.look(5, start + IDLE_TICKS) else {
            panic!("an interval of quiet drains");
        };
        assert_eq!(drained.look(5, start + 9 * IDLE_TICKS), Look::Wait);
        let Look::Record(moved) = drained.look(6, start + 9 * IDLE_TICKS) else {
            panic!("activity after a drain is recorded");
        };
        assert_eq!(moved.look(6, start + 10 * IDLE_TICKS - UNIT), Look::Wait);
        assert!(matches!(
            moved.look(6, start + 10 * IDLE_TICKS),
            Look::Drain(_)
        ));
    }

    #[test]
    fn a_clock_that_goes_back_postpones_the_drain() {
        let Look::Record(first) = Sighting(0).look(1, 5 * IDLE_TICKS) else {
            panic!("a first look records");
        };
        assert_eq!(first.look(1, 4 * IDLE_TICKS), Look::Wait);
        assert_eq!(first.look(1, 0), Look::Wait);
    }

    #[test]
    fn time_and_activity_wrap() {
        let Look::Record(first) = Sighting(0).look(u64::MAX, u64::MAX) else {
            panic!("a first look records");
        };
        assert!(matches!(
            first.look(u64::MAX, u64::MAX.wrapping_add(IDLE_TICKS)),
            Look::Drain(_)
        ));
        assert!(matches!(
            first.look(u64::MAX - 1, u64::MAX),
            Look::Record(_)
        ));
    }
}
