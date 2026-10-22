//! The depot's side of the background thread's word (`crate::thread_word`): the slow path's
//! look at the word, and the thread's decision to sleep or park (design: "Heartbeat").
//!
//! **Returning memory.** While the thread is awake or parked, returning memory to the OS is
//! its work and not a slow path's. A slow path loads the word once, after its exchange and its
//! count and before its heartbeat's chores, and the heartbeat's purge reads whose duty it is
//! from that snapshot (`Source::purge_step`): where it is the thread's, the heartbeat keeps the
//! clocked rules' readings and takes no purge step, and if a reading made work due that the
//! thread does, looks at the word afresh and wakes the thread if it sleeps
//! ([`Cache::rouse_thread`]). The
//! thread takes the steps slow paths would have taken, the same bounded steps under the same
//! rules, until none would do more ([`Cache::thread_steps`]), plans when it next has to look,
//! and sleeps until then or its timeout ([`Cache::begin_sleep`]). The memory it returns is work
//! for the rule below, as a round's is.
//!
//! **When the thread parks.** Its rules act on what one interval or several left unused, so a
//! round that finds nothing does not show that the next will: a slab drained in one interval
//! is shed in the next, what was shed is purged once a lap has counted it over the floor, and
//! scattered pages wait for the pass that takes runs of any length. So the thread parks only after
//! [`QUIET_ROUNDS`] rounds in a row that did no work (no drain, no debt laundered, no memory
//! returned) while no slow path ran anywhere (no slab's activity counter moved), and only when
//! every slab in use holds no magazine and drains are on. A round is one that ran the steps of
//! a counted interval; a call that the rate limit turned away is no round at all. How much
//! memory the heap holds is not part of the rule: free slots on pages that also hold live
//! objects stay counted for ever, and a thread that waited for the count would wake for ever.
//!
//! **Parking without losing a wake-up.** Having decided, the thread stores `PARKED`, fences
//! every CPU ([`Os::fence_all`]), and looks again at the counters and the slabs before it waits.
//! A slow path counts its activity and installs its magazines before it loads the word, with no
//! fence of its own. The fence orders the two: a slow path whose load of the word came before
//! the fence reached its CPU had made its stores before it too, and the thread's second look sees
//! them and backs out; one whose load came after sees `PARKED` and wakes the thread. Of the
//! two looks, the one at the slabs is what the argument rests on: any thread that could run on
//! without a slow path has a magazine in a slab. The activity counter is a hint kept with a
//! load and a store, which a preempted slow path can rewind; a slow path that installs nothing
//! (a thread without a valid CPU, which has no counter either) takes a slow path again on its
//! next call, and that one finds `PARKED`. A wake that finds the word already moved does
//! nothing, and a thread that finds it moved after its second look does not wait. Where drains
//! are off, rounds cannot empty slabs, nothing would send a program that resumes on its fast
//! paths to a slow path, and the thread never parks. The tests are `tests::thread_tests`, and
//! `interleave.rs` explores the store, fence and look against a racing slow path.

use super::{Cache, Source, generation};
use crate::back::policy::PURGE_ANY_RUN_EVERY;
use crate::classes::is_marker;
use crate::front::{Routines, Slab};
use crate::os::Os;
use crate::thread_word::{PARKED, Process, Snapshot, key_of};
use core::sync::atomic::Ordering::Acquire;

/// How many rounds in a row with no work and no slow path anywhere the thread waits before it
/// parks: as many as the longest wait any of its rules imposes after the last work or slow
/// path, which is the purge pass that takes runs of any length. A silence ages no frontier, so
/// the pass does not wait for frontiers; it waits for its count. A pass is held to what the
/// last whole lap counted, and a round's pass goes at most one lap, so the first lap counted
/// wholly after the last work or slow path ends in the second round counted ([`LAP_ROUNDS`]),
/// and the third round's pass is the first held to it. From there one pass in
/// [`PURGE_ANY_RUN_EVERY`] takes the scattered pages. So the count must cover the rounds of
/// that lap and a whole period of the passes: 10.
pub const QUIET_ROUNDS: u64 = LAP_ROUNDS + PURGE_ANY_RUN_EVERY;

/// The rounds counted after the last work or slow path before a lap that began after them has
/// ended: the rest of the lap under way, then a whole one.
const LAP_ROUNDS: u64 = 2;

/// The rounds after the last slow path that the other aging rules take to act, counted as
/// [`QUIET_ROUNDS`] is: the idle drain, whose sighting starts in the round that sees the slow
/// path and drains an interval of the clock later, which may be one round more; a depot
/// stack's shed, which owes the low-water mark of the interval its first look starts at the
/// next one; and the hold on large slots, which `maintain` takes at its second round in a
/// silence.
const IDLE_DRAIN_ROUNDS: u64 = 2 + super::heartbeat::IDLE_TICKS.div_ceil(crate::clock::INTERVAL);
const SHED_ROUNDS: u64 =
    1 + (1u64 << super::heartbeat::IDLE_SHIFT).div_ceil(crate::clock::INTERVAL);
const HOLD_ROUNDS: u64 = crate::back::MAINTAIN_HOLD_ROUNDS;

const _: () = assert!(QUIET_ROUNDS + 1 >= IDLE_DRAIN_ROUNDS);
const _: () = assert!(QUIET_ROUNDS + 1 >= SHED_ROUNDS);
const _: () = assert!(QUIET_ROUNDS + 1 >= HOLD_ROUNDS);
const _: () = assert!(super::heartbeat::IDLE_TICKS == crate::clock::INTERVAL);

/// What the thread does after a round.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Rest {
    /// Sleep, with a timeout.
    Sleep,
    /// The word is `PARKED` and the second look found nothing: wait without a timeout.
    Park,
}

/// What the thread remembers between rounds: the activity it saw at the end of the last, and
/// how many rounds in a row have found no work and no slow path.
#[derive(Clone, Copy, Default, Debug)]
pub struct Seen {
    activity: Option<u64>,
    quiet: u64,
}

impl Seen {
    /// Counts a round that did `work` and ended with the heap's activity at `activity`: the
    /// quiet rounds in a row, which work or a moved counter sets back to zero.
    pub fn after_round(&mut self, activity: u64, work: bool) -> u64 {
        self.quiet = if work || self.activity != Some(activity) {
            0
        } else {
            self.quiet.saturating_add(1)
        };
        self.activity = Some(activity);
        self.quiet
    }
}

impl<O: Os, R: Routines, S: Source> Cache<'_, O, R, S> {
    /// The slow path's part, after its exchange and its count: one load of the word, and the
    /// cold look when it asks for one (`ThreadWord::attend`). What it read, or left, is kept
    /// for the caller ([`Cache::thread_seen`]).
    #[inline]
    pub(super) fn after_slow_path(&self) {
        let seen = self
            .source
            .thread()
            .after_slow_path(self.os(), || self.thread_process());
        self.thread_seen.set(seen);
    }

    /// The thread's word as this view's slow path last read or left it: for a Rust-interface
    /// slow path to see `WANTED` without a second load.
    pub fn thread_seen(&self) -> Snapshot {
        self.thread_seen.get()
    }

    /// The cold look at the word, for a slow path that loaded it elsewhere (a large one that
    /// did not beat) and found that it asks for one.
    pub fn attend_thread(&self) -> Snapshot {
        self.source
            .thread()
            .attend(self.os(), self.thread_process())
    }

    /// The cold look of a slow path whose heartbeat made work due that the thread does: wakes
    /// the thread if it sleeps, and says whether a thread of this process will do the work
    /// (`ThreadWord::rouse`).
    #[cold]
    #[inline(never)]
    pub(super) fn rouse_thread(&self) -> bool {
        self.source.thread().rouse(self.os(), self.thread_process())
    }

    /// One turn of the thread's loop while it is awake (`heap::Bound::run_thread`): the round
    /// of `maintain` if one is due, a step of the purges after each of its steps,
    /// the purges until none is due, and then its rest, a park or a timed sleep of `every_ns`,
    /// waited for here. `returned` carries memory returned between rounds to the next round's
    /// count (`Seen`).
    pub fn thread_turn(&self, seen: &mut Seen, key: u64, returned: &mut bool, every_ns: u64) {
        let word = self.source.thread();
        let mut round = None;
        while let Some(step) = self.maintain_step_with_work() {
            round = Some(round.unwrap_or(false) | step.work);
            *returned |= self.thread_step().returned;
            if !step.more {
                break;
            }
        }
        *returned |= self.thread_steps();
        let round = round.map(|work| work | core::mem::take(returned));
        match self.rest(seen, key, round) {
            Rest::Park => self.os().wait(word.futex(), key | PARKED, None),
            Rest::Sleep => {
                if let Some(asleep) = self.begin_sleep(key) {
                    self.os().wait(word.futex(), asleep.0, Some(every_ns));
                    self.end_sleep(key);
                }
            }
        }
    }

    /// What the thread does after a round, while it runs: the purge steps that slow paths
    /// leave to it, until none would do more, and then its plan for when it next has to look.
    /// Whether they returned memory, which is work for the rule that parks it.
    pub fn thread_steps(&self) -> bool {
        let mut returned = false;
        loop {
            let step = self.thread_step();
            returned |= step.returned;
            if !step.more {
                break;
            }
        }
        self.source.plan_thread();
        returned
    }

    /// One of the purge steps that slow paths leave to the thread: what the thread takes
    /// between the steps of a round, so that a long round (a pass over a large heap, many
    /// magazines shed) does not hold up the hold's looks and growth's payments, which
    /// heartbeats take beside a pass where slow paths return memory.
    pub fn thread_step(&self) -> super::PurgeStep {
        let counted = self.source.clock();
        self.source.thread_step(&|| counted.now(self.os()))
    }

    /// The thread's timed sleep, before it waits: marks its word asleep, then looks once more
    /// for work that came due meanwhile, a round of `maintain` or a purge (see
    /// `crate::thread_word`, "Sleeping"). Returns the word to wait on, or `None` if there is
    /// work now, having taken the mark back.
    pub fn begin_sleep(&self, key: u64) -> Option<Snapshot> {
        let word = self.source.thread();
        let asleep = word.begin_sleep(key);
        let counted = self.source.clock();
        let now = || counted.now(self.os());
        if !counted.round_done(self.os()) || self.source.thread_due(&now) {
            word.end_sleep(key);
            return None;
        }
        Some(asleep)
    }

    /// The thread's back-out from a sleep, once it wakes: takes its mark back, if a waker has
    /// not.
    pub fn end_sleep(&self, key: u64) {
        self.source.thread().end_sleep(key);
    }

    /// This process's generation as the thread's word keys it, readying the magazine area if
    /// no slow path has yet; zero if it cannot be had.
    pub fn thread_key(&self) -> u64 {
        if !self.prepare() {
            return 0;
        }
        self.generation()
            .map_or(0, |generation| key_of(generation.get()))
    }

    /// The key, and whether this process is a fork child: known once the key has been had,
    /// since the draw of the generation is what tells.
    fn thread_process(&self) -> Process {
        let key = self.thread_key();
        Process {
            key,
            forked: generation::forked(self.depot),
        }
    }

    /// The thread's decision after a round: `round` is whether it did work, or `None` if the
    /// call ran no round. Parks after [`QUIET_ROUNDS`] quiet rounds when every slab is empty,
    /// having stored `PARKED`, fenced and looked again; otherwise sleeps.
    pub fn rest(&self, seen: &mut Seen, key: u64, round: Option<bool>) -> Rest {
        let Some(work) = round else {
            return Rest::Sleep;
        };
        match self.first_look(seen, work) {
            Some(activity) if self.mark_parked(key) => self.second_look(key, activity),
            _ => Rest::Sleep,
        }
    }

    /// The look before parking, after a round that did `work`: the activity the heap shows,
    /// if the thread may park on it.
    pub(super) fn first_look(&self, seen: &mut Seen, work: bool) -> Option<u64> {
        let activity = self.activity();
        let parkable = seen.after_round(activity, work) >= QUIET_ROUNDS
            && generation::drains_enabled(self.depot)
            && self.slabs_empty();
        parkable.then_some(activity)
    }

    /// Stores `PARKED` over `AWAKE`; whether it did.
    pub(super) fn mark_parked(&self, key: u64) -> bool {
        self.source.thread().park(key)
    }

    /// The fence, and the look after it: parks if the heap still shows `activity` and every
    /// slab is still empty, and otherwise backs out to `AWAKE`. A slow path may have woken the
    /// thread already, which leaves it awake too.
    pub(super) fn second_look(&self, key: u64, activity: u64) -> Rest {
        let word = self.source.thread();
        if self.os().fence_all().is_err() || self.activity() != activity || !self.slabs_empty() {
            word.unpark(key);
            return Rest::Sleep;
        }
        if word.load().state_for(key) == PARKED {
            Rest::Park
        } else {
            Rest::Sleep
        }
    }

    /// The sum of every slab's activity counter: it moves whenever any slow path runs.
    fn activity(&self) -> u64 {
        let cpus = self.depot.common.cpus.load(Acquire);
        (0..cpus)
            .filter_map(|index| self.routines.slabs().cpu(index as i32))
            .map(|cpu| self.slab(cpu).activity())
            .fold(0u64, u64::wrapping_add)
    }

    /// Whether no slab in use holds a magazine: each field a drain empties is null or a
    /// drain's marker.
    fn slabs_empty(&self) -> bool {
        let cpus = self.depot.common.cpus.load(Acquire);
        (0..cpus)
            .filter_map(|index| self.routines.slabs().cpu(index as i32))
            .all(|cpu| {
                let slab = self.slab(cpu);
                Slab::drained_fields().all(|field| {
                    let word = slab.field(field).load(Acquire);
                    word.is_null() || is_marker(word.addr())
                })
            })
    }
}
