//! The depot's side of the background thread's word (`crate::thread_word`): the slow path's
//! wake, and the thread's decision to sleep or park (design: "Heartbeat").
//!
//! **Parking without losing a wake-up.** The thread parks only after a round that did no work
//! (no drain, no debt shed, the heap at or under the floor), when no slab's activity counter
//! moved since the round before, and every slab in use holds no magazine. It then stores
//! `PARKED`, fences every CPU ([`Os::fence_all`]), and looks again at the counters and the
//! slabs before it waits. A slow path adds to its slab's counter and loads its magazines before
//! it loads the word, with no fence of its own. The fence orders the two: a slow path whose
//! load of the word came before the fence reached its CPU had made its stores before it too,
//! and the thread's second look sees them and backs out; one whose load came after sees
//! `PARKED` and wakes the thread. A wake that finds the word already moved does nothing, and a
//! thread that finds it moved after its second look does not wait. Where drains are off,
//! rounds cannot empty slabs, nothing would send a program that resumes on its fast paths to a
//! slow path, and the thread never parks. The tests are `tests::thread_tests`.

use super::{Cache, Source, generation};
use crate::classes::is_marker;
use crate::front::{Routines, Slab};
use crate::os::Os;
use crate::thread_word::{AWAKE, PARKED};
use core::sync::atomic::Ordering::{Acquire, Relaxed, SeqCst};

/// What the thread does after a round.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Rest {
    /// Sleep an interval, with a timeout.
    Sleep,
    /// The word is `PARKED` and the second look found nothing: wait without a timeout.
    Park,
}

/// What the thread remembers between rounds: the activity it saw at the end of the last.
#[derive(Clone, Copy, Default, Debug)]
pub struct Seen {
    activity: Option<u64>,
}

impl<O: Os, R: Routines, S: Source> Cache<'_, O, R, S> {
    /// The slow path's part: wakes the thread if it is parked. One load otherwise.
    #[inline]
    pub(super) fn wake_if_parked(&self) {
        if self.source.thread().state.load(Relaxed) == PARKED {
            self.wake_parked();
        }
    }

    #[cold]
    #[inline(never)]
    fn wake_parked(&self) {
        let word = &self.source.thread().state;
        if word
            .compare_exchange(PARKED, AWAKE, SeqCst, Relaxed)
            .is_ok()
        {
            self.os().wake(word);
        }
    }

    /// The thread's decision after a round that did `work`: park if the heap is quiet and
    /// every slab is empty, having stored `PARKED`, fenced and looked again; otherwise sleep.
    pub fn rest(&self, seen: &mut Seen, work: bool) -> Rest {
        match self.first_look(seen, work) {
            Some(activity) if self.mark_parked() => self.second_look(activity),
            _ => Rest::Sleep,
        }
    }

    /// The look before parking: the activity the heap shows, if it may park on it.
    pub(super) fn first_look(&self, seen: &mut Seen, work: bool) -> Option<u64> {
        let activity = self.activity();
        let quiet = seen.activity == Some(activity);
        seen.activity = Some(activity);
        let parkable = !work
            && quiet
            && generation::drains_enabled(self.depot)
            && !self.source.over_floor()
            && self.slabs_empty();
        parkable.then_some(activity)
    }

    /// Stores `PARKED` over `AWAKE`; whether it did.
    pub(super) fn mark_parked(&self) -> bool {
        self.source
            .thread()
            .state
            .compare_exchange(AWAKE, PARKED, SeqCst, Relaxed)
            .is_ok()
    }

    /// The fence, and the look after it: parks if the heap still shows `activity` and every
    /// slab is still empty, and otherwise backs out to `AWAKE`. A slow path may have woken the
    /// thread already, which leaves it awake too.
    pub(super) fn second_look(&self, activity: u64) -> Rest {
        let word = &self.source.thread().state;
        if self.os().fence_all().is_err() || self.activity() != activity || !self.slabs_empty() {
            let _ = word.compare_exchange(PARKED, AWAKE, SeqCst, Relaxed);
            return Rest::Sleep;
        }
        if word.load(SeqCst) == PARKED {
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
