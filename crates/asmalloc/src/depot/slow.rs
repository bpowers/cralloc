//! The slow paths: exchanging magazines when a routine fails (`tla/Exchange.tla`).
//!
//! For an allocation the slow path:
//!
//! 1. reads the CPU's slab word for the class without protection, and gets a magazine with
//!    objects: from the CPU's spares, else from the depot, else by filling an empty one (from
//!    the spares, the depot, or newly made) from the source;
//! 2. installs it with store-if-equal, expecting the word it read: zero, an empty magazine, or
//!    another generation's marker;
//! 3. retires the magazine it displaced, now its own, to the CPU's spares or the depot, and
//!    tries the routine again.
//!
//! If the install finds another word (another thread got there first, or this one moved to a
//! CPU that has a magazine), it retires the magazine it holds and tries again. If the word is a
//! live marker, or the thread has no valid CPU, it serves the request from the magazine it
//! holds and sends that magazine to the depot, so it never waits. With no magazine to be had at
//! all it serves the request straight from the source, as it does for a thread with no valid
//! CPU from the start. Free is the mirror image, with an empty magazine.
//!
//! The install expects a word read from the CPU the thread was on then, but runs on the CPU it
//! is on now. That is safe: if the two words are equal, either both are zero or markers, or
//! both name the same magazine, which then is on the current CPU, so the install displaces
//! exactly what the slow path decided to displace.
//!
//! **Refresh.** One full magazine in [`REFRESH_EVERY`] that a free displaces is laundered and
//! retired empty instead of going to the spares or the depot, so the objects in steady
//! circulation are gradually replaced by the lowest free ones, and the pages their predecessors
//! pinned can be purged. The count is the CPU's own, in the cold part of its slab
//! ([`Slab::count_retired`](crate::front::Slab)).

use super::heartbeat::{Chores, Tried};
use super::magazine::Magazine;
use super::{Cache, Pool, Source};
use crate::classes::{SmallClass, is_marker};
use crate::front::{Cpu, Field, Routines, Spare, StoreIfEqual};
use crate::os::Os;
use core::ptr::NonNull;
use core::sync::atomic::Ordering::{Acquire, Relaxed};

/// What a slab word read without protection holds.
enum Seen<'a> {
    /// No magazine.
    Zero,
    /// A magazine, at this level.
    Loaded(Magazine<'a>, usize),
    /// This process's marker: a drain is under way. Never stored over.
    Live,
    /// Another generation's marker: absent.
    Stale,
}

/// Where a magazine for the slow path came from, or why there is none.
enum Obtained<'a> {
    /// A magazine this thread now holds, suitable for the request.
    Magazine(Magazine<'a>),
    /// An empty magazine was had but the source has no objects: the allocation fails.
    OutOfMemory,
    /// No magazine can be had at all: serve the request from the source directly.
    NoMagazine,
}

/// One full magazine in this many that a CPU's frees retire is laundered instead of kept, by
/// default. Each costs a launder now and a fill later, and in a fragmented heap the fill's
/// search of the bitmaps dominates. One in 32 kept nearly all of one in eight's effect on
/// what stays resident after a peak, for about a third of its instructions on slow-path-heavy
/// workloads (`docs/research/memory.md`, "The parked-object rules, built"). An object in
/// circulation survives `k` retirements with probability `(31/32)^k`: half are replaced within
/// 22 and nineteen in twenty within 95, still fast next to the interval for any class that
/// exchanges often enough for its pages to matter.
pub const REFRESH_EVERY: u64 = 32;

impl<'a, O: Os, R: Routines, S: Source> Cache<'a, O, R, S> {
    /// Whether a word is a marker that a slow path must not store over: this process's, or
    /// any marker if there is no generation yet (none can be drained without one).
    pub(super) fn is_live(&self, word: *mut u8) -> bool {
        is_marker(word.addr())
            && match self.generation() {
                Some(generation) => generation.is_mine(word),
                None => true,
            }
    }

    fn classify(&self, word: *mut u8, class: SmallClass) -> Seen<'a> {
        if word.is_null() {
            Seen::Zero
        } else if is_marker(word.addr()) {
            if self.is_live(word) {
                Seen::Live
            } else {
                Seen::Stale
            }
        } else {
            match self.area.loaded(word.addr(), class.magazine()) {
                Some((magazine, level)) => Seen::Loaded(magazine, level),
                None => self
                    .os()
                    .abort("a slab word names something that is not a magazine"),
            }
        }
    }

    /// An empty magazine for `class`: from the CPU's spares, its domain's empty stack, the
    /// shared one, or newly made.
    fn obtain_empty(&self, class: SmallClass, stale: &mut bool) -> Option<Magazine<'a>> {
        if let Some(magazine) = self.spare_pop(class, Spare::Empty, stale) {
            if magazine.level() == 0 {
                probe!(self.routines.cpu(), class, EmptySpare);
                return Some(magazine);
            }
            self.to_depot(class, magazine);
        }
        let size = class.magazine();
        for pool in [Pool::Domain(self.here()), Pool::Shared] {
            if let Some((magazine, contended)) =
                self.depot.empty(pool, size).pop_noting(self.area, size)
            {
                self.note_return(class, Spare::Empty, contended);
                probe!(self.routines.cpu(), class, EmptyDepot);
                return Some(magazine);
            }
        }
        probe!(self.routines.cpu(), class, EmptyMade);
        self.area.create(size)
    }

    /// A magazine with objects of `class`: from the CPU's spares, the depot, or by filling an
    /// empty one from the source.
    fn obtain_full(&self, class: SmallClass, chores: &mut Chores) -> Obtained<'a> {
        if let Some(magazine) = self.spare_pop(class, Spare::Full, &mut chores.stale) {
            if magazine.level() > 0 {
                probe!(self.routines.cpu(), class, AllocSpare);
                return Obtained::Magazine(magazine);
            }
            self.to_depot(class, magazine);
        }
        if let Some(magazine) = self.pop_full(class, &mut chores.tried) {
            probe!(self.routines.cpu(), class, AllocDepot);
            return Obtained::Magazine(magazine);
        }
        let Some(magazine) = self.obtain_empty(class, &mut chores.stale) else {
            return Obtained::NoMagazine;
        };
        probe!(self.routines.cpu(), class, AllocFill);
        if self.fill(class, &magazine) == 0 {
            self.retire(class, Spare::Empty, magazine, &mut chores.stale);
            return Obtained::OutOfMemory;
        }
        Obtained::Magazine(magazine)
    }

    /// A magazine of `class` from the depot: from the stack of the domain the thread is in,
    /// else from the shared stack, which is where a flow of objects from one domain to another
    /// passes. Records in `tried` each stack it tried, so that the heartbeat notes their
    /// depths.
    fn pop_full(&self, class: SmallClass, tried: &mut Tried) -> Option<Magazine<'a>> {
        let size = class.magazine();
        for pool in [Pool::Domain(self.here()), Pool::Shared] {
            tried.note(pool);
            if let Some((magazine, contended)) =
                self.depot.full(pool, class).pop_noting(self.area, size)
            {
                self.note_return(class, Spare::Full, contended);
                return Some(magazine);
            }
        }
        None
    }

    /// Whether the full magazine a free on `cpu` just displaced is the one in
    /// `refresh_every` to launder.
    fn refreshes(&self, cpu: Cpu) -> bool {
        let every = self.tuning.refresh_every;
        every != 0 && self.slab(cpu).count_retired().is_multiple_of(every)
    }

    /// Fills an empty magazine this thread holds from the source, in one call; returns its
    /// new level. The source fills from the top (see [`Source::fill`]); a short fill, from
    /// sparse bitmaps or a heap out of memory, is moved down to the bottom slots.
    fn fill(&self, class: SmallClass, magazine: &Magazine) -> usize {
        let slots = magazine.objects();
        let capacity = slots.len();
        let level = self.source.fill(class, slots).min(capacity);
        if level < capacity
            && let Some(filled) = slots.get(capacity - level..)
        {
            // Upwards: each slot is read before anything is written over it.
            for (to, from) in slots.iter().zip(filled) {
                to.store(from.load(Relaxed), Relaxed);
            }
        }
        magazine.set_level(level);
        level
    }

    /// Serves an allocation from a magazine with objects that this thread holds, and sends the
    /// rest to the depot.
    fn serve_alloc(&self, class: SmallClass, magazine: Magazine) -> Option<NonNull<u8>> {
        let level = magazine.level();
        let object = NonNull::new(magazine.object(level));
        magazine.set_level(level - 1);
        self.to_depot(class, magazine);
        if object.is_none() {
            self.os().abort("a magazine holds a null object");
        }
        object
    }

    /// Serves a free with an empty magazine this thread holds, and sends it to the depot.
    fn serve_free(&self, class: SmallClass, magazine: Magazine, object: NonNull<u8>) {
        magazine.set_object(1, object);
        magazine.set_level(1);
        self.to_depot(class, magazine);
    }

    /// Allocates an object of `class`: the pop routine, then the slow path if it fails. `None`
    /// means the source has no memory.
    pub fn alloc(&self, class: SmallClass) -> Option<NonNull<u8>> {
        match self.routines.pop(class) {
            Some(object) => Some(object),
            None => self.alloc_slow(class),
        }
    }

    /// The slow path of an allocation, then the heartbeat. Where the pop routine sends a
    /// thread whose CPU has no magazine with objects for `class`.
    pub fn alloc_slow(&self, class: SmallClass) -> Option<NonNull<u8>> {
        probe!(self.routines.cpu(), class, AllocSlow);
        self.begin_slow_path();
        let mut chores = Chores::default();
        let object = self.exchange_for_alloc(class, &mut chores);
        self.heartbeat(Some(class), chores);
        object
    }

    fn exchange_for_alloc(&self, class: SmallClass, chores: &mut Chores) -> Option<NonNull<u8>> {
        let ready = self.prepare();
        let field = Field::hot(class);
        loop {
            if let Some(object) = self.routines.pop(class) {
                return Some(object);
            }
            let Some(cpu) = self.routines.cpu() else {
                return self.source.alloc_one(class);
            };
            if !ready {
                return self.source.alloc_one(class);
            }
            chores.cpu = Some(cpu);
            let word = self.slab(cpu).field(field).load(Acquire);
            let seen = self.classify(word, class);
            match seen {
                Seen::Loaded(_, level) if level > 0 => continue,
                Seen::Stale => chores.stale = true,
                _ => {}
            }
            let held = match self.obtain_full(class, chores) {
                Obtained::Magazine(magazine) => magazine,
                Obtained::OutOfMemory => return None,
                Obtained::NoMagazine => return self.source.alloc_one(class),
            };
            if let Seen::Live = seen {
                return self.serve_alloc(class, held);
            }
            // SAFETY: DEPOT-SLOW-01.
            let installed = unsafe {
                self.routines
                    .store_if_equal(field, word, held.word(held.level()))
            };
            match installed {
                StoreIfEqual::Stored => {
                    chores.installed = true;
                    probe!(Some(cpu), class, AllocInstalledLevel, held.level());
                    if let Seen::Loaded(old, _) = seen {
                        old.set_level(0);
                        #[cfg(feature = "probe-center")]
                        if crate::probe::reversed(Some(cpu), class, 1) {
                            let moved = self.center_alloc(class, &old, held.level() / 2);
                            if moved > 0 {
                                old.set_level(moved);
                                self.retire(class, Spare::Full, old, &mut chores.stale);
                                continue;
                            }
                        }
                        self.retire(class, Spare::Empty, old, &mut chores.stale);
                    }
                }
                StoreIfEqual::Mismatch(now) if self.is_live(now) => {
                    return self.serve_alloc(class, held);
                }
                StoreIfEqual::Mismatch(_) => {
                    self.retire(class, Spare::Full, held, &mut chores.stale)
                }
                StoreIfEqual::NoCpu => return self.serve_alloc(class, held),
            }
        }
    }

    /// Moves up to `n` objects out of the loaded magazine of the thread's CPU into `old`, which
    /// this thread holds empty, so that the loaded one is left half full; returns how many.
    #[cfg(feature = "probe-center")]
    fn center_alloc(&self, class: SmallClass, old: &Magazine, n: usize) -> usize {
        let mut moved = 0;
        while moved < n {
            let Some(object) = self.routines.pop(class) else {
                break;
            };
            moved += 1;
            old.set_object(moved, object);
        }
        moved
    }

    /// Moves up to half of the `level` objects of `old`, a full magazine this thread holds,
    /// into the loaded magazine of the thread's CPU; returns how many `old` keeps.
    #[cfg(feature = "probe-center")]
    fn center_free(&self, class: SmallClass, old: &Magazine, level: usize) -> usize {
        let mut level = level;
        let keep = level - level / 2;
        while level > keep {
            let Some(object) = NonNull::new(old.object(level)) else {
                break;
            };
            if !self.routines.push(class, object) {
                break;
            }
            level -= 1;
        }
        level
    }

    /// Frees an object of `class`: the push routine, then the slow path if it fails.
    pub fn free(&self, class: SmallClass, object: NonNull<u8>) {
        if !self.routines.push(class, object) {
            self.free_slow(class, object);
        }
    }

    /// The slow path of a free, then the heartbeat. Where the push routine sends a thread
    /// whose CPU has no magazine with room for `class`.
    pub fn free_slow(&self, class: SmallClass, object: NonNull<u8>) {
        probe!(self.routines.cpu(), class, FreeSlow);
        self.begin_slow_path();
        let mut chores = Chores::default();
        self.exchange_for_free(class, object, &mut chores);
        self.heartbeat(Some(class), chores);
    }

    fn exchange_for_free(&self, class: SmallClass, object: NonNull<u8>, chores: &mut Chores) {
        let ready = self.prepare();
        let field = Field::hot(class);
        let capacity = class.magazine().capacity();
        loop {
            if self.routines.push(class, object) {
                return;
            }
            let Some(cpu) = self.routines.cpu() else {
                return self.source.free_one(class, object);
            };
            if !ready {
                return self.source.free_one(class, object);
            }
            chores.cpu = Some(cpu);
            let word = self.slab(cpu).field(field).load(Acquire);
            let seen = self.classify(word, class);
            match seen {
                Seen::Loaded(_, level) if level < capacity => continue,
                Seen::Stale => chores.stale = true,
                _ => {}
            }
            let Some(held) = self.obtain_empty(class, &mut chores.stale) else {
                return self.source.free_one(class, object);
            };
            if let Seen::Live = seen {
                return self.serve_free(class, held, object);
            }
            // SAFETY: DEPOT-SLOW-01.
            let installed = unsafe { self.routines.store_if_equal(field, word, held.word(0)) };
            match installed {
                StoreIfEqual::Stored => {
                    chores.installed = true;
                    if let Seen::Loaded(old, level) = seen {
                        probe!(Some(cpu), class, FreeDisplacedLevel, level);
                        #[cfg(feature = "probe-center")]
                        let level = if crate::probe::reversed(Some(cpu), class, 2) {
                            self.center_free(class, &old, level)
                        } else {
                            level
                        };
                        old.set_level(level);
                        let due = self.refreshes(cpu);
                        if due && self.may_launder(class) && !self.has_deferred(cpu) {
                            self.launder_held(class, &old);
                            self.retire(class, Spare::Empty, old, &mut chores.stale);
                        } else {
                            if due {
                                // Postponed to the next retirement, not skipped.
                                self.slab(cpu).uncount_retired();
                            }
                            self.retire(class, Spare::Full, old, &mut chores.stale);
                        }
                    }
                }
                StoreIfEqual::Mismatch(now) if self.is_live(now) => {
                    return self.serve_free(class, held, object);
                }
                StoreIfEqual::Mismatch(_) => {
                    self.retire(class, Spare::Empty, held, &mut chores.stale)
                }
                StoreIfEqual::NoCpu => return self.serve_free(class, held, object),
            }
        }
    }
}
