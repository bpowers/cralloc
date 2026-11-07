//! The heartbeat: bounded maintenance done by slow paths, paced by what they move.
//!
//! Each slow path that installs a magazine adds the bytes of objects a full magazine of its
//! class holds to its CPU's activity counter, and runs the chores below when the counter passes
//! a multiple of [`BEAT_BYTES`]; a slow path whose CPU has a rule the launder budget turned away
//! runs them at once. A large allocation or free adds a [`LONE_EVERY`]th of that, so that one in
//! that many beats when nothing else moves ([`Cache::lone_heartbeat`]). Between beats a slow
//! path does only what answers for its own call (`Cache::between_beats`): the stamp of the
//! magazine it installed, the depths of the depot stacks it tried, a drain of its own slab where
//! it met another generation's marker, and the launder over target of its own pushes below.
//! There is no timer, so maintenance happens at a rate set by allocation activity, in units
//! small enough that no single call pays much; a program that wants it paced by time instead
//! runs `maintain` (`maintain.rs`) from a thread of its own. The clock is read at most once per
//! heartbeat, and only when a decision needs it.
//!
//! - **Launder over target.** On every slow path, beat or not, and last in a heartbeat, after
//!   its own sweep and drain: for each
//!   shared depot stack the call's exchange pushed a magazine with objects on, while that stack
//!   holds more than its target, a magazine is popped and its objects go back to the source, as
//!   many as the exchange pushed there, whatever the rules before it did (they ran within what
//!   those objects left of the budget below, or, if a rule turned away earlier went first, within
//!   the whole budget); the empty magazines go to the shared empty stack.
//!   Then the same, within the budget, for what the heartbeat's own sweep and drain pushed on
//!   shared stacks; what the budget leaves over a target, the shed takes back over the
//!   heartbeats that follow. Only a push makes a stack deeper. A domain's stack needs no such
//!   rule: its cap keeps it within bounds.
//! - **Shed what was not needed** (`shed.rs`). A note of the depth of each stack the slow path
//!   tried to pop, and on every [`LOOK_EVERY`]th heartbeat of a CPU a look at one class's stacks
//!   in its own domain and the shared pool, the classes in turn, and at one stack held by
//!   another domain, in rotation (`held.rs`), each of which starts a new interval for the stack
//!   it looks at; then at most one magazine laundered from a stack that owes. So a domain whose
//!   CPUs have gone quiet is looked at by the others, at a cost per look that does not grow
//!   with the number of domains.
//! - **Age limit** (`age.rs`). A look at one class of the CPU's own slab, the classes in turn,
//!   which takes and launders the magazine loaded there if it has been loaded for an interval,
//!   or launders one full spare of a class with nothing loaded.
//! - **Purge.** One [`Source::purge_step`]: a step of returning memory, or, while the
//!   background thread runs, only the readings that keep its rules' time and a wake for work
//!   they made due (`thread`).
//! - **Idle check.** Each slow path on a CPU moves that CPU's activity counter, and every
//!   [`LOOK_EVERY`]th heartbeat looks at one other CPU's slab, chosen from the count of
//!   heartbeats, so that a CPU's looks go round every CPU number a slow path has run on. A
//!   slab whose slow paths are all swaps with its spares moves its counter all the same. A slab
//!   is idle when
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
//! **Bounded work.** Besides the exchange itself, which fills at most one magazine and that
//! from at most [`FILL_CLAIMS`](crate::back::policy::FILL_CLAIMS) bitmap words, a slow path
//! launders at most [`LAUNDER_BUDGET`](super::LAUNDER_BUDGET) objects by its own choice: the
//! refresh in the exchange (`slow.rs`), the age limit's take or the sweep's step, the shed, and
//! the launder over target past the exchange's own pushes each launder a magazine only while a
//! full one would fit in what is left of it, after the objects of the exchange's pushes that left
//! a shared stack over its target are set aside for the launder over target. A rule asks for room
//! only once it has found work. One that finds no room waits for a later slow path; the age limit
//! and the sweep, which only their CPU's slow paths run, go first at the next one, with the
//! whole budget, and that call still launders its own pushes over target. So a slow path
//! launders at most one of the largest magazines, 1,022 objects, besides a `malloc`'s fill, and
//! twice that on a call that carries a rule turned away, which happens at most once an interval
//! for each class and rule; and more only when the exchange pushes more than one magazine past
//! a target, which takes a thread moved to another CPU while it exchanges, or a half-full swap
//! whose install finds a drain's marker, which retires both magazines it holds. A launder sets one bitmap word per group of neighbours with one atomic OR, or one per object
//! when they are scattered. A sweep step also pushes the rest of one spare stack, at most a few
//! magazines, to the depot; for a class with stock it instead returns one magazine above the
//! class's limit, and the sweep stays on the class at later heartbeats (`stock.rs`). Add one
//! purge step, at most one drain (of another slab, or of the thread's own when it holds another
//! generation's markers: a bounded number of atomic instructions and one fence), and a fixed
//! number of loads and single-word writes for the looks. What a drain parks over a shared
//! stack's target is laundered within the budget, and the rest by later calls and the shed.
//!
//! **Shared writes.** A slow path that exchanges nothing with the depot writes, outside its own
//! CPU's slab: nothing, on most entries; another slab's sighting, by compare-and-swap, on one
//! entry in [`LOOK_EVERY`], and only if that slab's counter moved since its last sighting or it
//! is drained; a depot stack's water word, only when a look finds a new low, a new interval or a
//! paid debt, and the depot's sets of owing classes, only when a debt begins or is paid; the
//! sets of what the domains hold, only when a look finds a domain's stack emptied; the
//! depot's count of CPU numbers, once for each new highest CPU number. A slow path that
//! exchanges with the depot writes its domain's stack heads, and the shared ones only when its
//! domain's stack is full or empty; a push that finds its domain's stack empty also sets that
//! stack's bit in the sets of what the domains hold, if it is clear. Laundering and purging
//! write shared state only when there is something to give back. Within its own slab, the
//! activity counter, the count of heartbeats, the refresh count and the sweep share a line that
//! other CPUs only read;
//! the stamps have lines of their own; the sighting, which other CPUs write, has its own line.
//!
//! **A drain that finds the slab busy.** The look that marks a sighting drained is the only one
//! that drains through it, since the mark is a compare-and-swap. So when that look's drain finds
//! the claim held, the holder is not a look through this sighting: `collect`, a slow path
//! draining another generation's markers on its own CPU, or a look that read the sighting before
//! a change and marked it again. That holder may back out, or may have claimed the slab just
//! before the parked magazines appeared. So the look records the slab as quiet from now, not
//! drained: if it stays idle, the first look an interval later drains it, and if the holder did
//! drain it, the cost is one more drain of an empty slab, at most once an interval. A slab that
//! stays idle with magazines parked is therefore drained within about two intervals of the busy
//! drain, whatever the holder did.
//!
//! **A clock that jumps.** Any clock read here may go back or leap forward: another CPU's
//! counter, a resume from suspend, a restored checkpoint. A sighting behind the clock is
//! recorded again from now, so it waits one interval more, never until the clock catches up; a
//! clock that leaps forward drains early, once. The age limit's stamps and the depot's intervals
//! follow the same rule (`age.rs`, `shed.rs`).
//!
//! **Hints.** The counter and the sighting are hints in the cold part of the slab, changed with
//! single atomic instructions rather than routines; the all-zero slab has neither, and is never
//! looked at. A lost update, a race between two looks, or a clock that is wrong, differs between
//! CPUs or goes back only moves a drain earlier or later: a drain is safe at any time, and two
//! looks that both decide to drain are stopped by the sighting's compare-and-swap and, behind
//! it, by the drain's own claim.

use super::drain::Disposal;
use super::{Cache, Depot, Drained, Overflow, Pool, Source, generation};
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

/// How many of its own heartbeats a CPU makes between looks at another CPU's slab, by default.
/// A look reads that slab's cold line and the clock, and sometimes writes the line; doing it on
/// one heartbeat in sixteen keeps that off the heartbeat's cost, and a CPU still looks at every
/// slab many times an interval while it allocates at any appreciable rate.
pub const LOOK_EVERY: u64 = 16;

/// A large allocation or free counts this fraction of [`BEAT_BYTES`] towards its CPU's next
/// heartbeat: one in 16 beats when only large blocks move. Large blocks are seldom allocated at
/// a rate that would starve the chores, and a program that only moves large blocks still gets a
/// heartbeat every few calls. A power of two dividing [`LOOK_EVERY`] and `BEAT_BYTES`.
pub const LONE_EVERY: u64 = 16;

const _: () = assert!(LOOK_EVERY.is_multiple_of(LONE_EVERY));

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
        if self.0 & SEEN == 0 || self.activity() != activity {
            return Look::Record(Sighting::at(activity, now));
        }
        if self.0 & DRAINED != 0 {
            return Look::Wait;
        }
        let time = (now >> TICK_SHIFT) as u32;
        let quiet = time.wrapping_sub(self.time()) as i32;
        // A clock behind the sighting went back (another CPU's, a resume from suspend, a
        // restored checkpoint): count the quiet from now, so the drain waits one interval more
        // instead of until the clock catches up.
        if quiet < 0 {
            return Look::Record(Sighting::at(activity, now));
        }
        if quiet < IDLE_UNITS as i32 {
            return Look::Wait;
        }
        Look::Drain(Sighting(self.0 | DRAINED))
    }

    /// A sighting of the counter at `activity` made at `now`, not drained.
    pub(super) fn at(activity: u64, now: u64) -> Sighting {
        let time = (now >> TICK_SHIFT) as u32;
        Sighting((time as u64) << TIME_SHIFT | (activity & ACTIVITY_MASK) << ACTIVITY_SHIFT | SEEN)
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
///
/// The time is the heap's counted time (`crate::clock`), not the cycle counter's: a silence
/// ages what the rules look at by one interval, however long it lasted. Reading it may take a
/// step of the count.
pub(super) struct Clock<'a, O: Os> {
    os: &'a O,
    counted: &'a crate::clock::Counted,
    now: Cell<Option<u64>>,
}

impl<'a, O: Os> Clock<'a, O> {
    pub(super) fn new(os: &'a O, counted: &'a crate::clock::Counted) -> Self {
        Clock {
            os,
            counted,
            now: Cell::new(None),
        }
    }

    /// The time, the same for the rest of the heartbeat once read.
    pub(super) fn now(&self) -> u64 {
        match self.now.get() {
            Some(now) => now,
            None => {
                let now = self.counted.now(self.os);
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
    /// The depot stacks of the class it served that it tried to pop.
    pub(super) tried: Tried,
    /// Its purge step is already taken.
    pub(super) purged: bool,
    /// The bytes of objects a full magazine of the class holds, if it installed a magazine:
    /// an exchange moves one magazine between tiers, which paces the heartbeat (see
    /// [`BEAT_BYTES`]).
    pub(super) moved: usize,
}

/// The depot stacks of a class that a slow path tried to pop, in order: its domain's and,
/// if that was empty, the shared one. The heartbeat notes the depth of each, the empty one
/// too, since that is the lowest it has been.
#[derive(Default, Debug)]
pub(super) struct Tried([Option<Pool>; 2]);

impl Tried {
    /// Records a try of `pool`'s stack. A slow path that goes round its exchange again tries
    /// the same stacks; the first two tries are the ones kept.
    pub(super) fn note(&mut self, pool: Pool) {
        if let Some(slot) = self.0.iter_mut().find(|slot| slot.is_none()) {
            *slot = Some(pool);
        }
    }

    fn pools(&self) -> impl Iterator<Item = Pool> + '_ {
        self.0.iter().flatten().copied()
    }
}

/// What a large allocation or free's heartbeat is to do (see [`lone_count`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lone {
    /// Nothing more: the slow path is counted.
    Counted,
    /// The chores, by [`Cache::lone_beat`].
    Beat(LoneTurn),
}

/// The CPU and the turn of a large allocation or free's heartbeat that is due.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct LoneTurn {
    cpu: Option<Cpu>,
    turn: Option<u64>,
}

/// The first half of the heartbeat of a slow path that served no small class: a large
/// allocation or free, which counts as a slow path of the CPU the thread is on. It always
/// counts itself, so that the CPU looks busy to the idle check, but the chores, the purge step
/// among them, are due only on every [`LONE_EVERY`]th count: a large allocation or free is a
/// few hundred instructions of its own, and a whole heartbeat on each would cost about as much
/// again. A thread with no valid CPU has no counter, and beats every time. This half needs
/// only the depot and the routines, so that the caller builds the rest of a [`Cache`] only
/// when the chores are due.
pub fn lone_count<R: Routines>(depot: &Depot, routines: &R) -> Lone {
    let cpu = routines.cpu();
    let turn = match cpu {
        // A sixteenth of a beat's bytes: a CPU that only moves large blocks beats on one call
        // in sixteen, and one that also exchanges magazines counts them towards the same beat.
        Some(cpu) => match count(depot, routines, cpu, BEAT_BYTES / LONE_EVERY, BEAT_SHIFT) {
            Some(turn) => Some(turn),
            None => return Lone::Counted,
        },
        None => None,
    };
    Lone::Beat(LoneTurn { cpu, turn })
}

/// `log2` of [`BEAT_BYTES`].
pub(super) const BEAT_SHIFT: u32 = 15;

/// The bytes of objects a CPU's slow paths move between tiers for each heartbeat: 32 KiB.
///
/// Maintenance is paced by what moves, not by calls. A slow path that installs a magazine adds
/// the bytes a full magazine of its class holds to its CPU's activity counter, and the
/// heartbeat's chores run when the counter passes a multiple of this. A call was never a fixed
/// amount of work: a class whose magazines hold two objects reached a slow path every two, one
/// whose magazines hold 1,022 every 1,022, and a swap with a spare that does nothing else costs
/// less than the chores would add to it. Bytes rather than objects, because what the chores
/// return is memory, and magazines are already sized in bytes (`MAGAZINE_BUDGET`).
///
/// 32 KiB is the most for which the clocked rules still act within the times the rules tests
/// assert (`rules.rs`): their busy CPUs exchange magazines of two objects of 24 to 32 KiB, so
/// one exchange must still be a beat, and at 64 KiB a quiet domain's stacks went back after 8.4
/// intervals instead of within 7. At 32 KiB most classes beat on each exchange, as before. Seven
/// move less per exchange and beat on a share of them, the bytes they move over 32 KiB: the 8,
/// 16, 24 and 32-byte classes (magazines of 1,022: one in four, one in two, three in four, and
/// all but one in 512), whose swaps with spares are the commonest, and the 5,120, 12,288 and
/// 14,336-byte classes (magazines of six and two: 94%, 75% and 88%). Measured in
/// `docs/research/magazines.md`, "What a slow path costs".
pub const BEAT_BYTES: u64 = 1 << BEAT_SHIFT;

/// Adds `bytes` moved by a slow path of `cpu` to its slab's activity counter and, if the
/// counter passed a multiple of [`BEAT_BYTES`], counts a heartbeat and returns its turn: the
/// number of heartbeats the CPU has run, so that the chores that go round the classes and the
/// CPUs take every turn whatever a call moves. Every call moves something, at least the object
/// it served, so it moves the activity counter, and the idle check sees a CPU busy on slow paths
/// of any kind as active.
fn count<R: Routines>(
    depot: &Depot,
    routines: &R,
    cpu: Cpu,
    bytes: u64,
    shift: u32,
) -> Option<u64> {
    if depot.common.cpus.load(Relaxed) <= cpu.index() {
        depot.common.cpus.fetch_max(cpu.index() + 1, Relaxed);
    }
    debug_assert!(bytes > 0, "a slow path that moved nothing would look idle");
    let slab = routines.slabs().slab(cpu);
    let (before, after) = slab.note_activity(bytes);
    (after >> shift != before >> shift).then(|| slab.note_beat())
}

impl<O: Os, R: Routines, S: Source> Cache<'_, O, R, S> {
    /// The heartbeat of a large allocation or free, both halves (see [`lone_count`]).
    pub fn lone_heartbeat(&self) {
        match lone_count(self.depot, &self.routines) {
            Lone::Beat(turn) => self.lone_beat(turn),
            Lone::Counted => self.after_slow_path(),
        }
    }

    /// The look at the background thread's word that every slow path makes, then the chores of
    /// a large allocation or free's heartbeat that [`lone_count`] found due.
    pub fn lone_beat(&self, turn: LoneTurn) {
        self.begin_slow_path();
        // After the count, which `lone_count` made: see `thread`, "Parking without losing a
        // wake-up". Before the chores, which read whose duty it is to return memory from it.
        self.after_slow_path();
        let chores = Chores {
            cpu: turn.cpu,
            ..Chores::default()
        };
        self.beat(None, chores, turn.turn);
    }

    /// What follows a slow path that served `served`, if it served a small class: the look at
    /// the background thread's word, its own duties, and the heartbeat's chores when its CPU's
    /// activity has passed a multiple of [`BEAT_BYTES`]. A thread with no valid CPU has no
    /// counter, and beats every time.
    pub(super) fn heartbeat(&self, served: Option<SmallClass>, chores: Chores) {
        let Some(cpu) = chores.cpu else {
            self.after_slow_path();
            return self.beat(served, chores, None);
        };
        // A call that installed no magazine (served straight from the source, or from the
        // magazine it held) moved the one object it served.
        let moved = match (chores.moved, served) {
            (0, Some(class)) => class.size(),
            (moved, _) => moved,
        };
        let turn = count(
            self.depot,
            &self.routines,
            cpu,
            moved as u64,
            self.tuning.beat_shift,
        );
        // After the call's exchange and its count: see `thread`, "Parking without losing a
        // wake-up". Before the chores, which read whose duty it is to return memory from it.
        self.after_slow_path();
        match turn {
            Some(turn) => self.beat(served, chores, Some(turn)),
            // A rule of this CPU's that an earlier call's budget turned away runs at its next
            // slow path, as the budget promises (`LAUNDER_BUDGET`), whatever the pace: it is once
            // an interval per class, and the refresh waits for it.
            None if self.tuning.age_limit && self.has_deferred(cpu) => {
                let turn = self.slab(cpu).note_beat();
                self.beat(served, chores, Some(turn));
            }
            None => self.between_beats(served, chores, cpu),
        }
    }

    /// The duties of a slow path that are not the heartbeat's to pace, because they answer for
    /// what this call did: the stamp of a magazine it installed, the depths of the depot stacks
    /// it tried (the low-water marks the shed reads), a drain of its own slab where it met
    /// another generation's marker, and the launder over target of what it pushed on shared
    /// stacks, one magazine for each push that left a stack over its target, which bounds the
    /// stacks whatever the pace. What the drain pushed past a target is laundered within the
    /// budget ([`LAUNDER_BUDGET`](super::LAUNDER_BUDGET)), and the shed takes back the rest.
    fn between_beats(&self, served: Option<SmallClass>, chores: Chores, cpu: Cpu) {
        if let (Some(class), true) = (served, chores.installed) {
            self.unstamp(cpu, class);
        }
        let clock = Clock::new(self.os(), self.source.clock());
        if let Some(class) = served
            && self.tuning.shed_unused
        {
            for pool in chores.tried.pools() {
                self.note_depth(pool, class, &clock);
            }
        }
        // The call's own pushes before the drain's: those are laundered whatever the budget,
        // and what a drain parks over a target only within it, as in `beat`.
        let pushed = self.overflowed.take();
        if chores.stale && generation::drains_enabled(self.depot) {
            self.drain(cpu);
        }
        if pushed.magazines > 0 {
            self.launder_overflow(pushed, &clock, false);
        }
        let parked = self.overflowed.take();
        if parked.magazines > 0 {
            self.launder_overflow(parked, &clock, true);
        }
    }

    /// The chores of a heartbeat. `activity` is its turn, which picks the class and the CPU
    /// that the rotating chores look at: the number of beats its CPU's activity counter holds.
    fn beat(&self, served: Option<SmallClass>, chores: Chores, activity: Option<u64>) {
        let clock = Clock::new(self.os(), self.source.clock());
        // A rule of this CPU's that an earlier slow path's budget turned away goes first, and
        // this call's launder over target waits for what the rule leaves of the budget: the
        // rules are once an interval per class and per slab, and nothing else runs them for a
        // CPU whose every exchange spends its budget. Otherwise what the exchange pushed past a
        // shared stack's target is laundered at the end, one magazine for each such push, so
        // that the stack does not gain on what arrives, and the rules before then work within
        // what that leaves.
        let deferred = match chores.cpu {
            Some(cpu) if self.tuning.age_limit => self.take_deferred(cpu),
            _ => (false, None),
        };
        let first = deferred != (false, None);
        let pushed = self.overflowed.take();
        self.reserved.set(if first { 0 } else { pushed.objects });
        if let (Some(cpu), Some(class), true) = (chores.cpu, served, chores.installed) {
            self.unstamp(cpu, class);
        }
        if let Some(class) = served
            && self.tuning.shed_unused
        {
            for pool in chores.tried.pools() {
                self.note_depth(pool, class, &clock);
            }
        }
        let every = self.tuning.look_every.max(1);
        let looking = activity.filter(|activity| activity.is_multiple_of(every));
        if first {
            self.age_and_sweep(&chores, activity, looking, deferred.1, &clock);
        }
        if self.tuning.shed_unused {
            if let Some(turn) = looking.map(|activity| activity / every) {
                let here = self.depot.domain_of(chores.cpu);
                self.note_looked(here, clock.now());
                if let Some(class) = nth_class(turn) {
                    self.look_at_depth(Pool::Domain(here), class, clock.now());
                    self.look_at_depth(Pool::Shared, class, clock.now());
                }
                self.look_elsewhere(here, turn, clock.now());
            }
            self.shed(activity.unwrap_or(0));
        }
        if !first {
            self.age_and_sweep(&chores, activity, looking, None, &clock);
        }
        if !chores.purged {
            self.purge(&clock);
        }
        if generation::drains_enabled(self.depot) {
            match (chores.cpu, looking) {
                (Some(cpu), _) if chores.stale => {
                    self.drain(cpu);
                }
                (Some(cpu), Some(activity)) => self.idle_check(cpu, activity, clock.now()),
                _ => {}
            }
        }
        // Last, so that it sees what this heartbeat's own sweep and drain parked. A call
        // launders its own pushes even after a carried rule spent the budget, so that a CPU
        // whose every exchange overflows keeps the stack at its target: one launder a push
        // never catches up with a push it skipped. Such a call launders up to two of the
        // largest magazines, at most once an interval per class.
        self.reserved.set(0);
        self.launder_overflow(pushed, &clock, false);
        // Then, within the budget, as many as this heartbeat's own sweep and drain pushed on
        // shared stacks. What the budget leaves over a target the shed takes back later.
        let parked = self.overflowed.take();
        self.launder_overflow(parked, &clock, true);
    }

    /// The heartbeat's part in returning memory to the OS ([`Source::purge_step`]), by whoever
    /// the background thread's word, as this slow path read it, says has that duty: a step
    /// here, or, while the thread runs, a wake for work that came due. A word that turns out
    /// to name no thread of this process (a fork child's, inherited) is made this process's,
    /// and the step is taken here after all.
    fn purge(&self, clock: &Clock<'_, O>) {
        let seen = self.thread_seen.get();
        if self.source.purge_step(&|| clock.now(), seen) && !self.rouse_thread() {
            let _ = self
                .source
                .purge_step(&|| clock.now(), crate::thread_word::Snapshot(0));
        }
    }

    /// The age limit's sweep step, or else its look at one class of the CPU's own slab: the
    /// class `again` that a slow path's budget turned away, or the next in turn.
    fn age_and_sweep(
        &self,
        chores: &Chores,
        activity: Option<u64>,
        looking: Option<u64>,
        again: Option<SmallClass>,
        clock: &Clock<'_, O>,
    ) {
        if !self.tuning.age_limit {
            return;
        }
        let (Some(cpu), Some(activity)) = (chores.cpu, activity) else {
            return;
        };
        if self.sweep_step(cpu, looking.is_some(), clock) {
            // The sweep laundered, which is this heartbeat's step; an age take it carried
            // waits for the next.
            if let Some(class) = again {
                self.defer_age(cpu, class);
            }
            return;
        }
        if let Some(class) = again.or_else(|| nth_class(activity)) {
            let _ = self.age_look(cpu, class, clock);
        }
    }

    /// Launders magazines from the shared stacks of `overflow`'s classes, while each is over its
    /// target ([`Cache::shared_target`]): at most `overflow.magazines` in all, and, `within`
    /// the budget, only while [`Cache::may_launder`] allows. Only a push makes a stack deeper,
    /// so a call that pushed nothing on a class's shared stack reads no head of it: reading it
    /// on every heartbeat cost a job whose four L3-crossing flows all overflow into the shared
    /// stacks 6% of its time.
    fn launder_overflow(&self, overflow: Overflow, clock: &Clock<'_, O>, within: bool) {
        let Overflow {
            mut classes,
            mut magazines,
            ..
        } = overflow;
        while classes != 0 && magazines > 0 {
            let ordinal = classes.trailing_zeros() as usize;
            let Some(class) = SmallClass::from_ordinal(ordinal) else {
                break;
            };
            if within && !self.may_launder(class) {
                classes &= classes - 1;
                continue;
            }
            let stack = self.depot.full(Pool::Shared, class);
            if stack.count(self.area, class.magazine()) as usize <= self.shared_target(class)
                || self.launder_one(Pool::Shared, class).is_none()
            {
                classes &= classes - 1;
                continue;
            }
            magazines -= 1;
            if self.tuning.shed_unused {
                self.note_depth(Pool::Shared, class, clock);
            }
        }
    }

    /// On the `look_every`-th slow path of CPU `me`, whose counter now reads `activity`, looks
    /// at one other CPU's slab and records or drains it (see the module documentation).
    fn idle_check(&self, me: Cpu, activity: u64, now: u64) {
        let every = self.tuning.look_every.max(1);
        // At least `me + 1`: this thread raised it to that before.
        let count = self.depot.common.cpus.load(Relaxed);
        if count < 2 {
            return;
        }
        let mut index = ((activity / every) % count as u64) as usize;
        if index == me.index() {
            index = (index + 1) % count;
        }
        if let Some(cpu) = self.routines.slabs().cpu(index as i32) {
            self.idle_look(cpu, now, Disposal::Park);
        }
    }

    /// The idle check's look at `cpu`'s slab with the clock at `now`: records a counter that
    /// moved, and drains a slab whose counter has stood still for an interval, disposing of
    /// what the drain takes as `disposal` says. A slab no slow path has used is left alone.
    /// Whether it drained the slab.
    pub(super) fn idle_look(&self, cpu: Cpu, now: u64, disposal: Disposal) -> bool {
        let slab = self.slab(cpu);
        let activity = slab.activity();
        if activity == 0 {
            return false;
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
                    return false;
                }
                // A drain that did not happen leaves the slab to be tried again at once. A busy
                // one leaves it to be tried an interval from now (see the module
                // documentation).
                let undo = match self.drain_with(cpu, disposal) {
                    Drained::BackedOut | Drained::Disabled => Some(seen),
                    Drained::Busy => Some(Sighting::at(activity, now).0),
                    Drained::Done => None,
                };
                if let Some(undo) = undo {
                    let _ = slab
                        .seen()
                        .compare_exchange(marked.0, undo, Relaxed, Relaxed);
                    return false;
                }
                return true;
            }
        }
        false
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
    fn a_clock_that_goes_back_delays_the_drain_by_one_interval() {
        let Look::Record(first) = Sighting(0).look(1, 500 * IDLE_TICKS) else {
            panic!("a first look records");
        };
        // The clock goes back far: the sighting starts again from the clock's time now.
        let back = 3 * IDLE_TICKS;
        let Look::Record(again) = first.look(1, back) else {
            panic!("a clock that went back records the slab again");
        };
        assert_eq!(again, Sighting::at(1, back));
        assert_eq!(again.look(1, back + IDLE_TICKS - UNIT), Look::Wait);
        assert!(matches!(again.look(1, back + IDLE_TICKS), Look::Drain(_)));
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
