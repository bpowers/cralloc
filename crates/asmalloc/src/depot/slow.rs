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

use super::heartbeat::Chores;
use super::magazine::Magazine;
use super::{Cache, Source};
use crate::classes::{SmallClass, is_marker};
use crate::front::{Field, Routines, Spare, StoreIfEqual};
use crate::os::Os;
use core::ptr::NonNull;
use core::sync::atomic::Ordering::Acquire;

/// What a slab word read without protection holds.
enum Seen {
    /// No magazine.
    Zero,
    /// A magazine, at this level.
    Loaded(Magazine, usize),
    /// This process's marker: a drain is under way. Never stored over.
    Live,
    /// Another generation's marker: absent.
    Stale,
}

/// Where a magazine for the slow path came from, or why there is none.
enum Obtained {
    /// A magazine this thread now holds, suitable for the request.
    Magazine(Magazine),
    /// An empty magazine was had but the source has no objects: the allocation fails.
    OutOfMemory,
    /// No magazine can be had at all: serve the request from the source directly.
    NoMagazine,
}

/// How many pointers a fill or a launder moves through the stack at a time.
pub(super) const CHUNK: usize = 64;

impl<O: Os, R: Routines, S: Source> Cache<'_, O, R, S> {
    /// Whether a word is a marker that a slow path must not store over: this process's, or
    /// any marker if there is no generation yet (none can be drained without one).
    pub(super) fn is_live(&self, word: *mut u8) -> bool {
        is_marker(word.addr())
            && match self.generation() {
                Some(generation) => generation.is_mine(word),
                None => true,
            }
    }

    fn classify(&self, word: *mut u8, class: SmallClass) -> Seen {
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

    /// An empty magazine for `class`: from the CPU's spares, the depot, or newly made.
    fn obtain_empty(&self, class: SmallClass) -> Option<Magazine> {
        if let Some(magazine) = self.spare_pop(class, Spare::Empty) {
            if magazine.level() == 0 {
                return Some(magazine);
            }
            self.to_depot(class, magazine);
        }
        if let Some(magazine) = self
            .depot
            .empty(class.magazine())
            .pop(self.area, class.magazine())
        {
            return Some(magazine);
        }
        self.area.create(class.magazine())
    }

    /// A magazine with objects of `class`: from the CPU's spares, the depot, or by filling an
    /// empty one from the source.
    fn obtain_full(&self, class: SmallClass, stale: &mut bool) -> Obtained {
        if let Some(magazine) = self.spare_pop(class, Spare::Full) {
            if magazine.level() > 0 {
                return Obtained::Magazine(magazine);
            }
            self.to_depot(class, magazine);
        }
        if let Some(magazine) = self.depot.full(class).pop(self.area, class.magazine()) {
            return Obtained::Magazine(magazine);
        }
        let Some(magazine) = self.obtain_empty(class) else {
            return Obtained::NoMagazine;
        };
        if self.fill(class, &magazine) == 0 {
            self.retire(class, Spare::Empty, magazine, stale);
            return Obtained::OutOfMemory;
        }
        Obtained::Magazine(magazine)
    }

    /// Fills an empty magazine this thread holds from the source; returns its new level.
    fn fill(&self, class: SmallClass, magazine: &Magazine) -> usize {
        let capacity = class.magazine().capacity();
        let mut level = 0;
        let mut buffer = [None; CHUNK];
        while level < capacity {
            let want = (capacity - level).min(CHUNK);
            let got = self.source.fill(class, &mut buffer[..want]).min(want);
            for object in buffer[..got].iter().flatten() {
                level += 1;
                magazine.set_object(level, *object);
            }
            if got < want {
                break;
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
        let mut chores = Chores::default();
        let object = self.exchange_for_alloc(class, &mut chores);
        self.heartbeat(class, chores);
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
            let held = match self.obtain_full(class, &mut chores.stale) {
                Obtained::Magazine(magazine) => magazine,
                Obtained::OutOfMemory => return None,
                Obtained::NoMagazine => return self.source.alloc_one(class),
            };
            if let Seen::Live = seen {
                return self.serve_alloc(class, held);
            }
            match self
                .routines
                .store_if_equal(field, word, held.word(held.level()))
            {
                StoreIfEqual::Stored => {
                    if let Seen::Loaded(old, _) = seen {
                        old.set_level(0);
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

    /// Frees an object of `class`: the push routine, then the slow path if it fails.
    pub fn free(&self, class: SmallClass, object: NonNull<u8>) {
        if !self.routines.push(class, object) {
            self.free_slow(class, object);
        }
    }

    /// The slow path of a free, then the heartbeat. Where the push routine sends a thread
    /// whose CPU has no magazine with room for `class`.
    pub fn free_slow(&self, class: SmallClass, object: NonNull<u8>) {
        let mut chores = Chores::default();
        self.exchange_for_free(class, object, &mut chores);
        self.heartbeat(class, chores);
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
            let Some(held) = self.obtain_empty(class) else {
                return self.source.free_one(class, object);
            };
            if let Seen::Live = seen {
                return self.serve_free(class, held, object);
            }
            match self.routines.store_if_equal(field, word, held.word(0)) {
                StoreIfEqual::Stored => {
                    if let Seen::Loaded(old, level) = seen {
                        old.set_level(level);
                        self.retire(class, Spare::Full, old, &mut chores.stale);
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
