//! The background thread's word: one 64-bit word in the back end's state that says whether
//! the thread exists and what it is doing, names the process generation it belongs to, and
//! carries a request to look at the level of free memory (design: "Heartbeat", from "So there
//! is a background thread"). Every slow path loads it once. The depot's side, the thread's
//! decision to park, is `depot::thread`; the heap's, its creation, is `heap::background`.
//!
//! **The word.** Bits 0 to 3 are the state, bit 4 is [`LEVEL`], and the bits from
//! [`KEY_SHIFT`] up are the generation's key: the random bits of the process generation
//! (`depot::generation`), which lives on a page a forked child reads as zero. All zero is no
//! thread, none wanted, no generation. The states:
//!
//! - [`NONE`]: no thread, and none wanted.
//! - [`WANTED`]: free memory is over the floor of the allowance and the process has created a
//!   thread of its own. A slow path that came through the Rust interface creates the thread
//!   (`heap::background`); nothing else acts on it.
//! - [`AWAKE`]: the thread runs rounds, and sleeps with a timeout between them.
//! - [`PARKED`]: the thread waits with no timeout; a slow path that finds it wakes the thread.
//! - [`DORMANT`]: the preload library's thread, created when the program created its first
//!   thread, waiting with no timeout until free memory is over the floor.
//! - [`OFF`]: no thread will be started by default: turned off, or creation failed.
//!
//! A word whose key is not the process's generation's is another process's: a parent's,
//! inherited by a fork. Every reader that acts on the state reads such a word as `NONE`, and
//! the paths that write the word replace it with this generation's. So a claim is one
//! compare-and-swap of the whole word; two concurrent claims in a fork child create one thread;
//! a child that shares its parent's memory (`vfork`, `CLONE_VM`) has its parent's generation and
//! is not taken for a new process; and no process ID is read, so neither the reuse of one nor
//! a child that is process 1 of a new namespace can confuse it.
//!
//! **The level.** The back end does not know the generation, so it never decides anything: it
//! sets [`LEVEL`] when an add to the retained count takes it over the floor (`crosses`) while
//! the word reads no thread or a dormant one, and once an interval while the count stays over
//! the floor (the decay pass's look, `back::purge`). The slow path's one load tests `PARKED`
//! and `LEVEL` together, and only a word with either goes to [`ThreadWord::attend`], which
//! knows the generation: it wakes a parked thread, and on a level wakes a dormant one or, with
//! a thread of the program's own as evidence, wants one. The evidence is the C library's
//! record that the program created a thread, except in a fork child, whose copy of that record
//! says what its parent did: there it is a crossing made by a thread other than the child's
//! first, so a child whose first thread frees the memory gets no thread by default. A hidden
//! thread in a process that never created one is what the design promises not to do, and a
//! fork child is the process likeliest to depend on having one thread. So a start refused for a passing
//! reason is tried again about once an interval while the heap stays over the floor, never on
//! every slow path. `WANTED` is not in that test: the Rust slow path reads it from the same
//! load, and a C slow path never acts on it.
//!
//! **The crossing.** The retained count is two tallies per kind (added, taken) on lines that
//! every CPU freeing to the bitmaps writes; reading them is what the clocked pass avoids. So
//! an add looks at its own tally's old and new values only, and only one that crosses a
//! multiple of an eighth of the floor reads the whole count: a shift and a compare on a call
//! that was writing the line anyway.

use crate::front::sync::{AtomicU64, core_u64};
use crate::os::Os;
use core::sync::atomic::Ordering::{Relaxed, SeqCst};

/// No thread, and none wanted.
pub const NONE: u64 = 0;
/// Free memory is over the floor and the program has a thread of its own: a Rust-interface
/// slow path creates the thread.
pub const WANTED: u64 = 1;
/// The thread runs rounds and sleeps with a timeout between them.
pub const AWAKE: u64 = 2;
/// The preload library's thread waits without a timeout until free memory is over the floor.
pub const DORMANT: u64 = 3;
/// No thread will be started by default.
pub const OFF: u64 = 4;
/// The thread waits without a timeout until a slow path wakes it. The only state with this
/// bit, so that the slow path's test needs one mask.
pub const PARKED: u64 = 8;
/// The state's bits.
const STATE: u64 = 0xf;
/// Free memory may be over the floor: a slow path that knows the generation should look.
pub const LEVEL: u64 = 0x10;
/// What sends a slow path to [`ThreadWord::attend`].
const ATTENTION: u64 = PARKED | LEVEL;
/// The generation's key starts here: a generation in marker form has its random bits above a
/// marker's low bits (`classes::MARKER_SHIFT`).
pub const KEY_SHIFT: u32 = crate::classes::MARKER_SHIFT;

const _: () = assert!(STATE < 1 << KEY_SHIFT && LEVEL < 1 << KEY_SHIFT);
const _: () = assert!(PARKED & (NONE | WANTED | AWAKE | DORMANT | OFF) == 0);

/// The key of a generation in marker form (`depot::Generation::get`): its bits from
/// [`KEY_SHIFT`] up, never zero for a generation.
pub const fn key_of(generation: u64) -> u64 {
    generation & !((1 << KEY_SHIFT) - 1)
}

/// An add to a retained tally from `old` by `bytes` that crosses a multiple of `step` (a
/// power of two): the adds that should look at the whole count.
#[inline(always)]
pub const fn crosses(old: usize, bytes: usize, step: usize) -> bool {
    (old ^ old.wrapping_add(bytes)) >= step
}

/// What the word's readers need to know of the process: its generation's key (zero for none
/// yet), and whether it is a fork child (`depot::generation`), where the evidence of a thread
/// of the program's own is weaker.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Process {
    pub key: u64,
    pub forked: bool,
}

/// A value of the word, as one load saw it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Snapshot(pub u64);

impl Snapshot {
    /// The state, whoever's it is.
    #[inline(always)]
    pub const fn state(self) -> u64 {
        self.0 & STATE
    }

    /// The generation's key.
    #[inline(always)]
    pub const fn key(self) -> u64 {
        key_of(self.0)
    }

    /// Whether the level bit is set.
    #[inline(always)]
    pub const fn level(self) -> bool {
        self.0 & LEVEL != 0
    }

    /// The state as the process of generation `key` reads it: another generation's is `NONE`.
    pub const fn state_for(self, key: u64) -> u64 {
        if self.key() == key {
            self.state()
        } else {
            NONE
        }
    }

    /// Whether a slow path must go to [`ThreadWord::attend`]: one load and one test.
    #[inline(always)]
    pub const fn needs_attention(self) -> bool {
        self.0 & ATTENTION != 0
    }

    /// Whether a Rust-interface slow path should try to create the thread. Another
    /// generation's `WANTED` passes too; the claim that creates the thread reads it as `NONE`
    /// and makes it this generation's.
    #[inline(always)]
    pub const fn wanted(self) -> bool {
        self.state() == WANTED
    }
}

/// The thread's word.
pub struct ThreadWord {
    word: AtomicU64,
}

impl ThreadWord {
    /// No thread.
    pub const fn new() -> ThreadWord {
        ThreadWord {
            word: AtomicU64::new(0),
        }
    }

    /// One load of the word.
    #[inline(always)]
    pub fn load(&self) -> Snapshot {
        Snapshot(self.word.load(Relaxed))
    }

    /// The word, as the thread waits on it and a slow path wakes it (`Os::wait`, `Os::wake`).
    pub fn futex(&self) -> &core::sync::atomic::AtomicU64 {
        core_u64(&self.word)
    }

    /// Asks a slow path that knows the generation to look at the level (see the module
    /// documentation). On a `crossing` only while the word reads no thread or a dormant one,
    /// of any generation: a running thread needs nothing, and a word that is another
    /// generation's is put right by the look once an interval, which asks whatever the word
    /// reads. A load, and a write only when the bit is clear.
    pub fn raise_level(&self, crossing: bool) {
        let seen = self.load();
        if seen.level() || crossing && !matches!(seen.state(), NONE | DORMANT) {
            return;
        }
        self.word.fetch_or(LEVEL, SeqCst);
    }

    /// The slow path's part, for `process` (whose key is zero if it has no generation yet,
    /// which leaves the word as it is): one load, and [`ThreadWord::attend`] if the word asks
    /// for it. Returns the word as it leaves it, for a Rust-interface slow path to read
    /// `WANTED` from without a second load.
    #[inline(always)]
    pub fn after_slow_path<O: Os + ?Sized>(
        &self,
        os: &O,
        process: impl FnOnce() -> Process,
    ) -> Snapshot {
        let seen = self.load();
        if seen.needs_attention() {
            self.attend(os, process())
        } else {
            seen
        }
    }

    /// A word that is parked or asks for a look at the level, for `process`: wakes a parked
    /// thread; on a level, wakes a dormant thread, wants one where there is none and the
    /// program has created a thread of its own (`Os::more_than_one_thread`, whose evidence
    /// differs in a fork child), and otherwise only clears the bit; replaces another
    /// generation's parked word with this generation's `NONE`. Returns the word as it left it.
    #[cold]
    #[inline(never)]
    pub fn attend<O: Os + ?Sized>(&self, os: &O, process: Process) -> Snapshot {
        let Process { key, forked } = process;
        let mut seen = self.load();
        loop {
            if key == 0 || !seen.needs_attention() {
                return seen;
            }
            let (state, wake) = match seen.state_for(key) {
                PARKED => (AWAKE, true),
                DORMANT if seen.level() => (AWAKE, true),
                NONE if seen.level() && os.more_than_one_thread(forked) => (WANTED, false),
                state => (state, false),
            };
            let next = Snapshot(key | state);
            match self.word.compare_exchange(seen.0, next.0, SeqCst, Relaxed) {
                Ok(_) => {
                    if wake {
                        os.wake(self.futex());
                    }
                    return next;
                }
                Err(now) => seen = Snapshot(now),
            }
        }
    }

    /// Moves the word from `from` to `to` for the process of generation `key`, keeping the
    /// level bit; whether it moved. Another generation's word reads as `NONE`, and is replaced
    /// by this generation's `NONE` when it does not move.
    pub fn claim(&self, key: u64, from: u64, to: u64) -> bool {
        let mut seen = self.load();
        loop {
            let moves = seen.state_for(key) == from;
            let next = if moves {
                key | to | (seen.0 & LEVEL)
            } else if seen.key() != key && seen.0 & !LEVEL != 0 {
                key | NONE | (seen.0 & LEVEL)
            } else {
                return false;
            };
            match self.word.compare_exchange(seen.0, next, SeqCst, Relaxed) {
                Ok(_) => return moves,
                Err(now) => seen = Snapshot(now),
            }
        }
    }

    /// Claims the creation of the thread in state `to` (`AWAKE` or `DORMANT`) for the process
    /// of generation `key`: whether the caller must now create it. A thread already running in
    /// this process is left as it is, except that a dormant one asked to run is woken. One
    /// compare-and-swap decides, so of two callers at once one creates the thread.
    pub fn claim_start<O: Os + ?Sized>(&self, os: &O, key: u64, to: u64) -> bool {
        let mut seen = self.load();
        loop {
            let (next, create) = match seen.state_for(key) {
                NONE | WANTED | OFF => (to, true),
                DORMANT if to == AWAKE => (AWAKE, false),
                _ => return false,
            };
            match self
                .word
                .compare_exchange(seen.0, key | next | (seen.0 & LEVEL), SeqCst, Relaxed)
            {
                Ok(_) => {
                    if !create {
                        os.wake(self.futex());
                    }
                    return create;
                }
                Err(now) => seen = Snapshot(now),
            }
        }
    }

    /// The thread's store before it parks: `PARKED` over its own `AWAKE`, clearing the level
    /// bit, since a thread exists. Whether it stored.
    pub fn park(&self, key: u64) -> bool {
        let mut seen = self.load();
        while seen.state_for(key) == AWAKE {
            match self
                .word
                .compare_exchange(seen.0, key | PARKED, SeqCst, Relaxed)
            {
                Ok(_) => return true,
                Err(now) => seen = Snapshot(now),
            }
        }
        false
    }

    /// The thread's back-out: `AWAKE` over its own `PARKED`, if a slow path has not woken it
    /// already, keeping the level bit.
    pub fn unpark(&self, key: u64) {
        let mut seen = self.load();
        while seen.state_for(key) == PARKED {
            match self.word.compare_exchange(
                seen.0,
                key | AWAKE | (seen.0 & LEVEL),
                SeqCst,
                Relaxed,
            ) {
                Ok(_) => return,
                Err(now) => seen = Snapshot(now),
            }
        }
    }

    /// Sets the word, for tests that start from a given state.
    #[cfg(test)]
    pub fn set(&self, word: u64) {
        self.word.store(word, SeqCst);
    }
}

impl Default for ThreadWord {
    fn default() -> Self {
        Self::new()
    }
}
