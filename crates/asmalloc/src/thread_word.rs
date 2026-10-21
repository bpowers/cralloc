//! The background thread's word: one `u32` in the back end's state that the thread waits on,
//! that every slow path loads once, and that the back end marks when the memory it retains
//! crosses the floor of the allowance (design: "Heartbeat", from "So there is a background
//! thread"). The depot's side, the wake and the decision to park, is `depot::thread`.
//!
//! Its states:
//!
//! - [`NONE`]: no thread, and none wanted yet.
//! - [`WANTED`]: an add to the retained count crossed the floor on a thread that is not the
//!   process's first. A slow path that came through the Rust interface creates the thread
//!   (`heap::background`); nothing else acts on it.
//! - [`AWAKE`]: the thread runs rounds, and sleeps an interval with a timeout between them.
//! - [`PARKED`]: the thread waits with no timeout; a slow path that finds the word parked moves
//!   it to `AWAKE` with a compare-and-swap and wakes the thread.
//! - [`DORMANT`]: the preload library's thread, created when the program created its first
//!   thread, waiting with no timeout until an add crosses the floor, which wakes it. The slow
//!   path's one load compares against `PARKED` alone.
//! - [`OFF`]: no thread will be started by default: turned off, or creation failed.
//!
//! The compare-and-swap that moves the word to `AWAKE` or `DORMANT` from a state with no
//! thread is what lets one caller, and only one, create the thread.
//!
//! **Fork.** The word is ordinary memory, so a forked child inherits it with no thread behind
//! it. A second word holds the ID of the process that last moved the first off `NONE`; the
//! paths that would act on a thread's existence (the crossing, a claim to create one) compare
//! it with the process's ID and count words of another process as `NONE`. A child's slow path
//! that finds an inherited `PARKED` wakes no one and moves the word to `AWAKE`, which the next
//! crossing in the child finds stale.
//!
//! **The crossing.** The retained count is two tallies per kind (added, taken) on lines that
//! every CPU freeing to the bitmaps writes; reading them is what the clocked pass avoids. So
//! an add looks at its own tally's old and new values only, and only one that crosses a
//! multiple of an eighth of the floor reads the whole count: a shift and a compare on a call
//! that was writing the line anyway. A crossing is then seen within an eighth of the floor of
//! the adds that made it, whenever they came.

use crate::os::Os;
use core::sync::atomic::AtomicU32;
use core::sync::atomic::Ordering::{Relaxed, SeqCst};

/// No thread, and none wanted yet.
pub const NONE: u32 = 0;
/// An add crossed the floor; a Rust-interface slow path creates the thread.
pub const WANTED: u32 = 1;
/// The thread runs rounds and sleeps with a timeout between them.
pub const AWAKE: u32 = 3;
/// The thread waits without a timeout until a slow path wakes it.
pub const PARKED: u32 = 4;
/// The thread waits without a timeout until an add crosses the floor.
pub const DORMANT: u32 = 5;
/// No thread will be started by default.
pub const OFF: u32 = 6;

/// An add to a retained tally from `old` by `bytes` that crosses a multiple of `step` (a
/// power of two): the adds that should look at the whole count.
#[inline(always)]
pub const fn crosses(old: usize, bytes: usize, step: usize) -> bool {
    (old ^ old.wrapping_add(bytes)) >= step
}

/// The thread's words.
pub struct ThreadWord {
    /// The state, one of the constants above; the word the thread waits on.
    pub state: AtomicU32,
    /// The process that last moved the state off `NONE`.
    owner: AtomicU32,
}

impl ThreadWord {
    /// No thread.
    pub const fn new() -> ThreadWord {
        ThreadWord {
            state: AtomicU32::new(NONE),
            owner: AtomicU32::new(0),
        }
    }

    /// The state, counting words that another process left as `NONE`.
    fn state_for(&self, process: u32) -> u32 {
        let state = self.state.load(SeqCst);
        if state == NONE || self.owner.load(Relaxed) == process {
            state
        } else {
            NONE
        }
    }

    /// Moves the state from `from`, as [`ThreadWord::state_for`] reads it, to `to`, owned by
    /// `process`; whether it moved.
    fn claim(&self, process: u32, from: u32, to: u32) -> bool {
        let seen = self.state.load(SeqCst);
        let state = if seen == NONE || self.owner.load(Relaxed) == process {
            seen
        } else {
            NONE
        };
        if state != from
            || self
                .state
                .compare_exchange(seen, to, SeqCst, Relaxed)
                .is_err()
        {
            return false;
        }
        self.owner.store(process, Relaxed);
        true
    }

    /// An add has taken the retained count over the floor: wakes a dormant thread, or, on a
    /// thread that is not the process's first, wants one where there is none.
    #[cold]
    #[inline(never)]
    pub fn crossed<O: Os + ?Sized>(&self, os: &O) {
        let (thread, process) = os.ids();
        match self.state_for(process) {
            DORMANT => {
                if self.claim(process, DORMANT, AWAKE) {
                    os.wake(&self.state);
                }
            }
            NONE if thread != process => {
                self.claim(process, NONE, WANTED);
            }
            _ => {}
        }
    }

    /// Moves the state from `from` to `to` for this process; whether it moved.
    pub fn claim_for<O: Os + ?Sized>(&self, os: &O, from: u32, to: u32) -> bool {
        self.claim(os.ids().1, from, to)
    }

    /// Claims the creation of the thread in state `to` (`AWAKE` or `DORMANT`): whether the
    /// caller must now create it. A thread already running in this process is left as it is,
    /// except that a dormant one asked to run is woken.
    pub fn claim_start<O: Os + ?Sized>(&self, os: &O, to: u32) -> bool {
        let process = os.ids().1;
        loop {
            match self.state_for(process) {
                state @ (NONE | WANTED | OFF) => {
                    if self.claim(process, state, to) {
                        return true;
                    }
                }
                DORMANT if to == AWAKE => {
                    if self.claim(process, DORMANT, AWAKE) {
                        os.wake(&self.state);
                        return false;
                    }
                }
                _ => return false,
            }
        }
    }
}

impl Default for ThreadWord {
    fn default() -> Self {
        Self::new()
    }
}
