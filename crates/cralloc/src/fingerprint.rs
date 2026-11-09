//! One number for everything an exploration did, so that two versions of an explorer can be
//! compared. Test code only.
//!
//! The interleaving explorers (`back::explore`, `front::check`) mix every decision of every
//! schedule into a fingerprint and return it with their count of schedules. Two explorers that
//! report the same numbers for a scenario ran the same schedules in the same order. With
//! `CRALLOC_EXPLORE_LOG` naming a file, each exploration also appends a line to it: the test's
//! name, the number of schedules, the fingerprint and the seconds it took.

use std::time::Instant;

/// A running hash of a sequence of numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Fingerprint(pub(crate) u64);

impl Fingerprint {
    pub(crate) const fn new() -> Fingerprint {
        Fingerprint(0xcbf2_9ce4_8422_2325)
    }

    /// Adds a number to the sequence. Not for an adversary, only to tell two sequences apart.
    #[inline]
    pub(crate) fn mix(&mut self, value: u64) {
        let hash = (self.0 ^ value).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        self.0 = hash ^ (hash >> 29);
    }
}

/// Appends an exploration's line to the file `CRALLOC_EXPLORE_LOG` names, if it names one. Each
/// line is one write, so tests running at once do not mix theirs up.
pub(crate) fn log(began: Instant, schedules: usize, fingerprint: Fingerprint) {
    use std::io::Write;
    let Some(path) = std::env::var_os("CRALLOC_EXPLORE_LOG") else {
        return;
    };
    let thread = std::thread::current();
    let line = std::format!(
        "{}\t{schedules}\t{:016x}\t{:.3}\n",
        thread.name().unwrap_or("unnamed"),
        fingerprint.0,
        began.elapsed().as_secs_f64(),
    );
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut file| file.write_all(line.as_bytes()))
        .expect("the exploration log can be written");
}
