//! The background thread's word: one 64-bit word in the back end's state that says whether
//! the thread exists and what it is doing, names the process generation it belongs to, and
//! carries a request to look at the level of free memory (design: "Heartbeat", from "So there
//! is a background thread"). Every slow path loads it once. The depot's side, the thread's
//! decision to park, is `depot::thread`; the heap's, its creation, is `heap::background`.
//!
//! **The word.** Bits 0 to 3 are the state, bit 4 is [`LEVEL`], bit 5 is [`SLEEPING`], and the
//! bits from [`KEY_SHIFT`] up are the generation's key: the random bits of the process
//! generation (`depot::generation`), which lives on a page a forked child reads as zero. All
//! zero is no thread, none wanted, no generation. The states:
//!
//! - [`NONE`]: no thread, and none wanted.
//! - [`WANTED`]: free memory is over the floor of the allowance and the process has created a
//!   thread of its own. A slow path that came through the Rust interface creates the thread
//!   (`heap::background`); nothing else acts on it.
//! - [`AWAKE`]: the thread runs rounds, and sleeps with a timeout between them, with
//!   [`SLEEPING`] set while it sleeps.
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
//! sets [`LEVEL`] when an add to the retained count takes it over the floor (`crosses`), and
//! once an interval while the count stays over the floor (the decay pass's look,
//! `back::purge`), whatever the word reads. The slow path's one load tests `WANTED`, `PARKED`
//! and `LEVEL` with one mask, and only a word with one of them goes to the cold look
//! ([`ThreadWord::attend`]), which knows the generation: it wakes a parked thread, and on a
//! level wakes a dormant one or, with a thread of the program's own as evidence, wants one; a
//! Rust-interface slow path that leaves the word `WANTED` then creates the thread. The evidence
//! is the C library's record that the program created a thread, except in a fork child, whose
//! copy of that record says what its parent did: there it is a crossing made by a thread other
//! than the child's first, so a child whose first thread frees the memory gets no thread by
//! default. A hidden thread in a process that never created one is what the design promises
//! not to do, and a fork child is the process likeliest to depend on having one thread. So a
//! start refused for a passing reason is tried again about once an interval while the heap
//! stays over the floor, never on every slow path. A C slow path never acts on `WANTED`; it
//! finds the word so only until the next Rust-interface slow path, or, under the preload
//! library, where nothing creates the thread from a slow path, not at all, since its first
//! `pthread_create` leaves the word dormant or off.
//!
//! **The crossing.** The retained count is two tallies per kind (added, taken) on lines that
//! every CPU freeing to the bitmaps writes; reading them is what the clocked pass avoids. So
//! an add looks at its own tally's old and new values only, and only one that crosses a
//! multiple of an eighth of the floor reads the whole count: a shift and a compare on a call
//! that was writing the line anyway.
//!
//! **Whose duty.** While a thread is awake or parked, returning memory is its work and not a
//! slow path's (design: "Heartbeat"; [`Duty`]). A slow path reads that from the snapshot its
//! one load took, whatever generation the word names: telling a fork child's inherited word
//! from its own would take the generation, a second load. So a fork child of a process whose
//! thread ran leaves its work to a thread it does not have until the first time work comes
//! due: the cold look that would wake the thread ([`ThreadWord::rouse`]) knows the generation,
//! makes the word the child's `NONE`, and the slow path does the work after all.
//!
//! **Sleeping.** The thread sleeps with a timeout in state `AWAKE`, a futex wait on this word
//! that only a wake interrupts. Before it waits it sets [`SLEEPING`] and looks once more for
//! work that came due ([`ThreadWord::begin_sleep`]); whoever makes work due that the thread
//! could not foresee (growth to pay for over the cap) or reaches a deadline the thread set
//! (the next interval's pass, the hold's next look) clears the bit with a compare-and-swap and
//! wakes it ([`ThreadWord::rouse`], [`ThreadWord::rouse_any`]). Each side writes before it
//! reads: the thread the bit, then the state of the work; the waker the state of the work (a
//! debt added, the counted clock stepped), then this word. Every one of those operations is
//! sequentially consistent, so one of them sees the other's write: either the thread finds the
//! work and does not wait, or the waker finds the bit and wakes it. The wake is a system call
//! made once, by the waker that cleared the bit: under steady churn the bit is clear (the
//! thread is working, or was woken already) and a slow path writes nothing. The bit is not in
//! the slow path's mask, so a sleeping thread costs a slow path nothing.

use crate::front::sync::{AtomicU64, core_u64};
use crate::os::Os;
use core::sync::atomic::Ordering::{Relaxed, SeqCst};

/// No thread, and none wanted.
pub const NONE: u64 = 0;
/// The thread runs rounds and sleeps with a timeout between them.
pub const AWAKE: u64 = 1;
/// The preload library's thread waits without a timeout until free memory is over the floor.
pub const DORMANT: u64 = 2;
/// No thread will be started by default.
pub const OFF: u64 = 3;
/// Free memory is over the floor and the program has a thread of its own: a Rust-interface
/// slow path creates the thread. Its bit is in the slow path's mask; states 5 to 7 would be too.
pub const WANTED: u64 = 4;
/// The thread waits without a timeout until a slow path wakes it. Its bit is in the slow path's
/// mask; states 9 to 15 would be too.
pub const PARKED: u64 = 8;
/// The state's bits.
const STATE: u64 = 0xf;
/// Free memory may be over the floor: a slow path that knows the generation should look.
pub const LEVEL: u64 = 0x10;
/// The thread, `AWAKE`, sleeps with a timeout, and wants waking for work that comes due before
/// it would wake. Set by the thread, cleared by whoever wakes it (see the module
/// documentation). Not in the slow path's mask.
pub const SLEEPING: u64 = 0x20;
/// What sends a slow path to [`ThreadWord::attend`]: one mask, so one test.
const ATTENTION: u64 = WANTED | PARKED | LEVEL;
/// The generation's key starts here: a generation in marker form has its random bits above a
/// marker's low bits (`classes::MARKER_SHIFT`).
pub const KEY_SHIFT: u32 = crate::classes::MARKER_SHIFT;

const _: () = assert!(STATE < 1 << KEY_SHIFT && LEVEL < 1 << KEY_SHIFT);
// The futex compares the word's low half, where every bit a waker changes must be.
const _: () = assert!(SLEEPING < 1 << KEY_SHIFT && KEY_SHIFT < 32);
const _: () = assert!(ATTENTION & (NONE | AWAKE | DORMANT | OFF | SLEEPING) == 0);
const _: () = assert!(SLEEPING & (STATE | LEVEL) == 0);

/// Who returns memory to the OS (design: "Heartbeat"): a slow path's heartbeat, a step of each
/// purge at a time, or, while the background thread is awake or parked, the thread, which
/// slow paths only wake.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Duty {
    /// The slow paths, as when there is no thread.
    Slow,
    /// The background thread.
    Thread,
}

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

    /// Whether the thread sleeps with a timeout and wants waking for work that comes due.
    #[inline(always)]
    pub const fn sleeping(self) -> bool {
        self.0 & SLEEPING != 0
    }

    /// Who returns memory, as a slow path that does not know the generation reads it: the
    /// thread while one is awake or parked, whatever generation the word names (see the module
    /// documentation, "Whose duty").
    #[inline(always)]
    pub const fn duty(self) -> Duty {
        match self.state() {
            AWAKE | PARKED => Duty::Thread,
            _ => Duty::Slow,
        }
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
    /// documentation), whatever the word reads: the back end cannot tell a running thread of
    /// this process from one a fork child inherited with its parent's word, whose state means
    /// nothing in the child. A load, and a write only when the bit is clear; with a thread
    /// running, a crossing costs one slow path a cold look, once for each eighth of the floor
    /// freed.
    pub fn raise_level(&self) {
        if !self.load().level() {
            self.word.fetch_or(LEVEL, SeqCst);
        }
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
            // A level does not wake a sleeping thread, and clearing it must not take its
            // mark: a waker would then take the thread for working and leave it asleep.
            let asleep = if state == AWAKE && seen.key() == key {
                seen.0 & SLEEPING
            } else {
                0
            };
            let next = Snapshot(key | state | asleep);
            if next == seen {
                // `WANTED` with nothing else, on a C slow path: no write, so the line every
                // slow path reads stays shared.
                return seen;
            }
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
        if key == 0 {
            // No generation: every word would read as another's, and be replaced.
            return false;
        }
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
        if key == 0 {
            return false;
        }
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

    /// The thread's mark before a timed sleep: [`SLEEPING`] over its own `AWAKE`. Returns the
    /// word to wait on: with the mark, or, if the word is not this thread's `AWAKE` (it has
    /// been moved: a claim turned the default off after the thread's creation failed, say), as
    /// it is, which a timed wait on it also handles.
    pub fn begin_sleep(&self, key: u64) -> Snapshot {
        let mut seen = self.load();
        loop {
            if seen.state_for(key) != AWAKE || seen.sleeping() {
                return seen;
            }
            let next = Snapshot(seen.0 | SLEEPING);
            match self.word.compare_exchange(seen.0, next.0, SeqCst, SeqCst) {
                Ok(_) => return next,
                Err(now) => seen = Snapshot(now),
            }
        }
    }

    /// The thread's back-out from a sleep: clears its own [`SLEEPING`] if a waker has not.
    pub fn end_sleep(&self, key: u64) {
        let mut seen = self.load();
        while seen.sleeping() && seen.state_for(key) == AWAKE {
            match self
                .word
                .compare_exchange(seen.0, seen.0 & !SLEEPING, SeqCst, SeqCst)
            {
                Ok(_) => return,
                Err(now) => seen = Snapshot(now),
            }
        }
    }

    /// The cold look of a slow path that has just made work due that the thread does, or has
    /// reached a deadline the thread set, for `process`: wakes the thread if it sleeps or is
    /// parked, and says whether a thread of this process will do the work. A word that names
    /// no thread of this process, as a fork child finds its parent's, is made this process's
    /// `NONE`, keeping the level bit, and the caller does the work itself (see the module
    /// documentation, "Whose duty"); so does a process with no generation, which has no thread.
    #[cold]
    #[inline(never)]
    pub fn rouse<O: Os + ?Sized>(&self, os: &O, process: Process) -> bool {
        let key = process.key;
        if key == 0 {
            return false;
        }
        let mut seen = Snapshot(self.word.load(SeqCst));
        loop {
            let (next, thread) = match seen.state_for(key) {
                AWAKE if seen.sleeping() => (seen.0 & !SLEEPING, true),
                AWAKE => return true,
                PARKED => (key | AWAKE | (seen.0 & LEVEL), true),
                _ if seen.key() != key && seen.0 & !LEVEL != 0 => (key | (seen.0 & LEVEL), false),
                _ => return false,
            };
            match self.word.compare_exchange(seen.0, next, SeqCst, SeqCst) {
                Ok(_) => {
                    if thread {
                        os.wake(self.futex());
                    }
                    return thread;
                }
                Err(now) => seen = Snapshot(now),
            }
        }
    }

    /// [`ThreadWord::rouse`] for a caller that does not know the generation (the back end):
    /// wakes an awake thread that sleeps, whatever generation it belongs to. A parked thread
    /// is left to the slow path's own look at the word, which follows. In a fork child whose
    /// parent's thread slept, this wakes nobody and clears the parent's mark, once.
    #[cold]
    #[inline(never)]
    pub fn rouse_any<O: Os + ?Sized>(&self, os: &O) {
        let mut seen = Snapshot(self.word.load(SeqCst));
        while seen.state() == AWAKE && seen.sleeping() {
            match self
                .word
                .compare_exchange(seen.0, seen.0 & !SLEEPING, SeqCst, SeqCst)
            {
                Ok(_) => {
                    os.wake(self.futex());
                    return;
                }
                Err(now) => seen = Snapshot(now),
            }
        }
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
