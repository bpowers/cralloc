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
    /// One more than the count at which `maintain` last ran a round, zero for never: what
    /// limits it to one round an interval (design: "Heartbeat").
    maintained: AtomicU64,
    /// One more than the count at which a heartbeat last ran, zero for never: `maintain`
    /// leaves a heap whose heartbeats are running to them.
    beaten: AtomicU64,
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
            beaten: AtomicU64::new(0),
        }
    }

    /// The counted time, in ticks, taking a step if an interval has passed since the last.
    /// One load, and a compare-and-swap about once an interval.
    pub(crate) fn now<O: Os + ?Sized>(&self, os: &O) -> u64 {
        let ticks = os.ticks();
        let mut word = self.word.load(Relaxed);
        loop {
            let reading = read(word, ticks);
            let Some(next) = reading.next else {
                return reading.time;
            };
            // A failure means another reader stepped or re-stamped: read its word again, which
            // is at most one step on.
            match self.word.compare_exchange(word, next, Relaxed, Relaxed) {
                Ok(_) => return reading.time,
                Err(found) => word = found,
            }
        }
    }

    /// Notes that a heartbeat ran in the interval the count is in. A load, and a store once an
    /// interval.
    pub(crate) fn note_beat(&self) {
        let mark = self.count() as u64 + 1;
        if self.beaten.load(Relaxed) != mark {
            self.beaten.store(mark, Relaxed);
        }
    }

    /// Whether a round of `maintain` is due: no heartbeat ran in this interval or the one
    /// before, so that the heap's own maintenance is not running, and no round has run in this
    /// interval. The caller that gets `true` has claimed the round, with one compare-and-swap,
    /// so of callers in one interval at most one runs it. Reads the counted time first, so a
    /// call is what steps the count through a silence. A heap busy on its slow paths is left
    /// to its heartbeats, and a silence is maintained from its second interval on.
    pub(crate) fn claim_round<O: Os + ?Sized>(&self, os: &O) -> bool {
        let _ = self.now(os);
        let round = self.count() as u64 + 1;
        let beaten = self.beaten.load(Relaxed);
        if beaten != 0 && beaten + 1 >= round {
            return false;
        }
        let last = self.maintained.load(Relaxed);
        last != round
            && self
                .maintained
                .compare_exchange(last, round, Relaxed, Relaxed)
                .is_ok()
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
