//! Counted intervals: the time every rule that acts on the clock reads (design: "Heartbeat").
//!
//! The rules that return memory act on evidence that something went unused for an interval
//! while the program was running. Read from the cycle counter alone, a silence of a minute is
//! sixty intervals of such evidence, and the first slow paths after it found every loaded
//! magazine, spare, depot stack, slab and purge frontier idle at once and tore down what the
//! burst was about to use (`docs/research/server-latency.md`: hundreds of one-page purges and
//! a dozen drains in the first milliseconds of each burst). So intervals are counted instead:
//! the count steps by one when a reading finds that an interval of the cycle counter has
//! passed since the last step, however long ago that was.
//!
//! The rules keep reading a time in ticks, at the resolution they had: the counted time is the
//! count times [`INTERVAL`] plus the ticks since the last step, capped just short of an
//! interval. While readings keep coming it runs with the cycle counter, a step behind by at
//! most the gap between two readings; across a silence it moves by at most one interval and a
//! fraction. It never goes back while the cycle counter goes forward: at a step it moves from
//! just short of the next interval to its start.
//!
//! **The word.** One 64-bit word: the count in the high 32 bits, and the cycle counter at the
//! last step in units of 2^[`UNIT_SHIFT`] ticks in the low 32. Every change is one
//! compare-and-swap, and all zero is a valid state (no step yet: the first reading takes one).
//! The count wraps after 2^32 intervals, a century; a unit difference is read as signed, so a
//! silence longer than 2^31 units (days) reads as a clock that went back, which costs nothing
//! but a re-stamp. A clock that goes back (a resume, a restored checkpoint, another CPU's
//! counter a little behind) re-stamps the step without counting one, so the interval runs from
//! the new time, as the rules already allow for.
//!
//! **Limit.** The cycle counters of all CPUs are taken to agree to well within an interval, as
//! they do on current x86-64 machines (an invariant, synchronized TSC). Counters that differ by
//! more than an interval make readings from different CPUs step the count, or re-stamp it, at
//! each other's expense; the rules then run early or late, which costs memory or work and
//! nothing else.
//!
//! Anything that calls [`Counted::now`] may take a step: every heartbeat and `maintain`. A
//! process with a background thread or a timer calling `maintain` therefore counts its
//! silences in wall time, as before.

use crate::os::Os;
use core::sync::atomic::AtomicU64;
use core::sync::atomic::Ordering::Relaxed;

/// `log2` of [`INTERVAL`].
pub(crate) const INTERVAL_SHIFT: u32 = 32;

/// An interval in ticks of [`Os::ticks`]: 2^32, about a second of the cycle counter at a few
/// gigahertz (`depot::IDLE_TICKS` and `back::policy::DECAY_TICKS` are this).
pub(crate) const INTERVAL: u64 = 1 << INTERVAL_SHIFT;

/// The step's stamp keeps the cycle counter in units of 2^20 ticks, a fraction of a
/// millisecond, the resolution the rules keep their own stamps at.
pub(crate) const UNIT_SHIFT: u32 = 20;

/// An interval in units.
const INTERVAL_UNITS: u32 = (INTERVAL >> UNIT_SHIFT) as u32;

const _: () = assert!(INTERVAL_UNITS > 1 && INTERVAL_UNITS < 1 << 30);
const _: () = assert!(
    INTERVAL_SHIFT == 32,
    "the count is the high half of the counted time"
);

/// The count of intervals and when the last step was taken (see the module documentation).
pub struct Counted {
    word: AtomicU64,
    /// The round of `maintain` under way or last run, and its next step: one more than the
    /// count it belongs to in the high 32 bits, zero for never, and the step in the low 32
    /// (`depot::maintain`). What limits `maintain` to one round an interval and lets callers
    /// take its steps one at a time.
    maintained: AtomicU64,
}

/// A round of `maintain` and one of its steps: what [`Counted::take_step`] hands a caller.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Step {
    /// One more than the count the round belongs to.
    pub(crate) round: u32,
    /// The step, in the numbering of `depot::maintain`.
    pub(crate) index: u32,
}

/// Steps at or past this are repeated by every caller until one moves the round on with
/// [`Counted::jump`], instead of being handed out once each.
pub(crate) const REPEATED: u32 = 1 << 30;

/// The step of a round that has nothing left to do.
pub(crate) const DONE: u32 = u32::MAX;

const fn round_word(round: u32, index: u32) -> u64 {
    (round as u64) << 32 | index as u64
}

/// What a reading of the word and the clock decides.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Reading {
    /// The word to store over the one read, if any.
    pub(crate) next: Option<u64>,
    /// The counted time, in ticks.
    pub(crate) time: u64,
}

const fn pack(count: u32, unit: u32) -> u64 {
    (count as u64) << 32 | unit as u64
}

/// The counted time of a word whose step was `since` units ago.
const fn time_of(count: u32, since: u32) -> u64 {
    let since = if since < INTERVAL_UNITS {
        since
    } else {
        INTERVAL_UNITS - 1
    };
    (count as u64) << INTERVAL_SHIFT | (since as u64) << UNIT_SHIFT
}

/// The decision for the word `word` with the cycle counter at `ticks`.
pub(crate) const fn read(word: u64, ticks: u64) -> Reading {
    let count = (word >> 32) as u32;
    let step = word as u32;
    let unit = (ticks >> UNIT_SHIFT) as u32;
    let since = unit.wrapping_sub(step) as i32;
    if since < 0 {
        // The clock went back: the interval runs from now, and nothing is counted.
        let next = pack(count, unit);
        Reading {
            next: Some(next),
            time: time_of(count, 0),
        }
    } else if since as u32 >= INTERVAL_UNITS {
        // An interval or more since the last step, however much more: one step.
        let count = count.wrapping_add(1);
        Reading {
            next: Some(pack(count, unit)),
            time: time_of(count, 0),
        }
    } else {
        Reading {
            next: None,
            time: time_of(count, since as u32),
        }
    }
}

impl Counted {
    pub(crate) const fn new() -> Self {
        Counted {
            word: AtomicU64::new(0),
            maintained: AtomicU64::new(0),
        }
    }

    /// The counted time, in ticks, taking a step if an interval has passed since the last.
    /// One load, and a compare-and-swap about once an interval.
    pub(crate) fn now<O: Os + ?Sized>(&self, os: &O) -> u64 {
        loop {
            // The word before the clock, and both again after a failed compare-and-swap: a
            // reading taken before another reader's step and compared after it would look like
            // a clock that went back, re-stamp the step to the older time, and let the next
            // reader step again, two steps for one interval.
            let word = self.word.load(Relaxed);
            let reading = read(word, os.ticks());
            let Some(next) = reading.next else {
                return reading.time;
            };
            if self
                .word
                .compare_exchange(word, next, Relaxed, Relaxed)
                .is_ok()
            {
                return reading.time;
            }
        }
    }

    /// The next step of `maintain` for this caller, or `None` if the round of the interval the
    /// count is in is done. The first caller in a new counted interval starts its round at step
    /// 0; a step below [`REPEATED`] is handed to one caller each, with one compare-and-swap; a
    /// step at or above it is handed to every caller until one moves the round on. Reads the
    /// counted time first, so a call is what steps the count through a silence. Heartbeats do
    /// not count: one does a small part of what a round does, so that one ran says nothing about
    /// whether the round's work is done.
    pub(crate) fn take_step<O: Os + ?Sized>(&self, os: &O) -> Option<Step> {
        let _ = self.now(os);
        let round = self.count().wrapping_add(1);
        loop {
            let word = self.maintained.load(Relaxed);
            let (was, index) = ((word >> 32) as u32, word as u32);
            let next = if was != round {
                round_word(round, 1)
            } else if index == DONE {
                return None;
            } else if index >= REPEATED {
                return Some(Step { round, index });
            } else {
                round_word(round, index + 1)
            };
            if self
                .maintained
                .compare_exchange(word, next, Relaxed, Relaxed)
                .is_ok()
            {
                let index = if was != round { 0 } else { index };
                return Some(Step { round, index });
            }
        }
    }

    /// Moves round `round` from step `from` to step `to`, if it is still there: a phase of
    /// `maintain` that is over skips the steps it did not need.
    pub(crate) fn jump(&self, round: u32, from: u32, to: u32) {
        let _ = self.maintained.compare_exchange(
            round_word(round, from),
            round_word(round, to),
            Relaxed,
            Relaxed,
        );
    }

    /// Whether this call starts a round of `maintain`, for tests that run a round's parts by
    /// hand.
    #[cfg(test)]
    pub(crate) fn claim_round<O: Os + ?Sized>(&self, os: &O) -> bool {
        self.take_step(os).is_some_and(|step| step.index == 0)
    }

    /// The count of intervals, without reading the clock.
    pub(crate) fn count(&self) -> u32 {
        (self.word.load(Relaxed) >> 32) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    const UNIT: u64 = 1 << UNIT_SHIFT;

    fn run(ticks: &[u64]) -> (u64, Vec<u64>) {
        let mut word = 0;
        let mut times = Vec::new();
        for &t in ticks {
            let r = read(word, t);
            if let Some(next) = r.next {
                word = next;
            }
            times.push(r.time);
        }
        (word, times)
    }

    #[test]
    fn readings_that_keep_coming_follow_the_clock() {
        // A reading every tenth of an interval for ten intervals: the counted time moves by
        // what the clock moved, within the gap between readings.
        let start = 1234 * INTERVAL + 77 * UNIT;
        let ticks: Vec<u64> = (0..=100).map(|i| start + i * INTERVAL / 10).collect();
        let (_, times) = run(&ticks);
        let moved = times[100] - times[0];
        assert!(
            moved <= 10 * INTERVAL && moved + INTERVAL / 10 + 10 * UNIT >= 10 * INTERVAL,
            "{moved}"
        );
        assert!(times.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn a_silence_counts_as_one_interval_however_long() {
        for silence in [INTERVAL, 2 * INTERVAL, 60 * INTERVAL, 1 << 50] {
            let start = 5 * INTERVAL;
            let (_, times) = run(&[start, start + INTERVAL / 2, start + INTERVAL / 2 + silence]);
            let moved = times[2] - times[1];
            assert!(moved <= INTERVAL, "{silence}: {moved}");
            assert!(moved >= INTERVAL / 2 - UNIT, "{silence}: {moved}");
        }
    }

    #[test]
    fn the_counted_time_never_goes_back_while_the_clock_goes_forward() {
        let mut word = 0;
        let mut last = 0;
        let mut t = 3 * INTERVAL;
        let mut state = 7u64;
        for _ in 0..10_000 {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            t += (state >> 33) % (3 * INTERVAL);
            let r = read(word, t);
            if let Some(next) = r.next {
                word = next;
            }
            assert!(r.time >= last, "{} < {last}", r.time);
            last = r.time;
        }
    }

    #[test]
    fn a_clock_that_goes_back_restamps_without_counting() {
        let (word, times) = run(&[100 * INTERVAL, 100 * INTERVAL + 10 * UNIT, 40 * INTERVAL]);
        assert_eq!((word >> 32) as u32, 1);
        assert_eq!(times[2], times[0]);
        // The interval then runs from the new time.
        let r = read(word, 41 * INTERVAL);
        assert_eq!(r.time, 2 * INTERVAL);
    }

    #[test]
    fn all_zero_is_a_word_with_no_step_yet() {
        let r = read(0, 0);
        assert_eq!(r.time, 0);
        assert!(r.next.is_none());
        let r = read(0, 5 * INTERVAL);
        assert_eq!(r.next, Some(pack(1, ((5 * INTERVAL) >> UNIT_SHIFT) as u32)));
        assert_eq!(r.time, INTERVAL);
    }
}
