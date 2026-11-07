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
//! That first read is only a name for the install to compare with, and may be stale: the thread
//! may have moved, and a routine's store releases nothing. Nothing is read through it until the
//! install has committed, on the CPU whose earlier threads wrote the magazine, and a context
//! switch is a full barrier (`front::routines`); after a commit the thread sees all that CPU's
//! stores to it. Its acquire is for the check of the name (`magazine`, "The committed mark").
//!
//! **Half full on a reversal.** A swap installs a magazine at one of its ends: full for an
//! allocation, empty for a free. Work that swings both ways across a magazine's end, a few
//! allocations and then as many frees, then crosses the far end of the new magazine too, and
//! does on every repetition: two swaps each, for as long as the work is steady
//! (`docs/research/magazines.md`, "The magazine-end lock"). So each CPU records which way its
//! last swaps of each class went, a hint in the class's stock word ([`Cache::plan_swap`]). A
//! swap that reverses a swap which itself reversed the one before is evidence of such a lock,
//! and the slow path leaves the loaded magazine half full; a reversal after a run of swaps one
//! way is the turn of a longer swing, and stays plain. A half-full swap that any swap of the
//! class follows before an interval of silence did not help, and the rule backs off,
//! exponentially, before it tries again (`stock::plan_swap`). It takes the loaded magazine out
//! of the slab with store-if-equal, storing zero, so that both magazines are its own; moves
//! half of one's objects into the other, a copy between two magazines nobody else can reach;
//! and installs the half full one with store-if-equal, expecting zero. Each step is one commit,
//! and between them everything the thread holds is private. If the install finds the word
//! changed (a thread that took the slow path meanwhile and installed its own, a drain's marker,
//! a move to another CPU), the thread still holds both magazines and retires them or serves
//! from them as a failed install does. No spare is needed: the loaded magazine itself receives
//! or gives the half. A swing of less than half a magazine then never leaves it. A flow never
//! reverses, so it never pays: one more store-if-equal and a copy of half a magazine fall only
//! on a swap with evidence of a lock.
//!
//! **Refresh.** One full magazine in [`REFRESH_EVERY`] that a free displaces is laundered and
//! retired empty instead of going to the spares or the depot, so the objects in steady
//! circulation are gradually replaced by the lowest free ones, and the pages their predecessors
//! pinned can be purged. The count is the CPU's own, in the cold part of its slab
//! ([`Slab::count_retired`](crate::front::Slab)).
//!
//! **Refresh on evidence.** Where frees and allocations of a class balance on a CPU, each
//! allocation takes the slot the last free gave back, so new objects land where old ones died,
//! and after a fall in the program's short-lived objects the long-lived ones it goes on making
//! stay spread over the pages of its peak, a few to a page. One in 32 replaces them too slowly
//! to matter there; one in 4 packed every class from 160 B up and took a sixth off such a
//! trough, and costs about 1% of cycles wherever it runs (`docs/research/memory.md`, "Refresh
//! on evidence"). So the source says how far its fragmentation, free slots on pages that also
//! hold objects, earns a faster refresh ([`Source::refresh_raise`]): the back end sets it once
//! a lap of its purge walk from that fragmentation's share of what the small classes have
//! carved, from none under 1 in 64 to one in 4 from 1 in 16, up at once and down a step a lap.
//! A heap in steady use, whose share stays near half a percent, pays nothing. All classes are
//! refreshed alike: what each class's pages hold is not counted, and the back end's count of
//! free slots is one for all small classes. Classes of 128 B and down do not pack this way
//! (their loaded magazine recycles in place and rarely retires); the refresh does what it can
//! for the rest.

use super::heartbeat::{Chores, Tried};
use super::magazine::Magazine;
use super::siblings;
use super::stock::{Swap, SwapPlan};
use super::{Cache, Pool, Source};
use crate::classes::{SmallClass, is_marker};
use crate::front::{Cpu, Field, Routines, Spare, StoreIfEqual};
use crate::os::Os;
use core::ptr::{self, NonNull};
use core::sync::atomic::Ordering::{Acquire, Relaxed};

/// What a swap that may leave its magazine half full did (`centre_for_alloc`,
/// `centre_for_free`).
enum Centring<'a, T> {
    /// Not a reversal, or nothing to centre: the exchange goes on as usual with these.
    Skipped(Seen<'a>, Magazine<'a>),
    /// It installed a magazine, or found the slab changed; the slow path tries the routine
    /// again.
    Again,
    /// It served the request itself.
    Finished(T),
}

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

/// The bytes of objects a full magazine of `class` holds: what one exchange moves.
fn magazine_bytes(class: SmallClass) -> usize {
    class.magazine().capacity() * class.size()
}

// Returned in registers, not through memory (see `Magazine`).
const _: () = assert!(size_of::<Obtained<'_>>() <= 16);

/// One full magazine in this many that a CPU's frees retire is laundered instead of kept, by
/// default. Each costs a launder now and a fill later, and in a fragmented heap the fill's
/// search of the bitmaps dominates. One in 32 kept nearly all of one in eight's effect on
/// what stays resident after a peak, for about a third of its instructions on slow-path-heavy
/// workloads (`docs/research/memory.md`, "Parked objects pin the heap"). An object in
/// circulation survives `k` retirements with probability `(31/32)^k`: half are replaced within
/// 22 and nineteen in twenty within 95, still fast next to the interval for any class that
/// exchanges often enough for its pages to matter. The base rate: a fragmented heap halves it
/// up to three times ("Refresh on evidence" above).
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
                return Some(magazine);
            }
        }
        self.area.create(size)
    }

    /// A magazine with objects of `class`: from the CPU's spares, the depot, or by filling an
    /// empty one from the source.
    fn obtain_full(&self, class: SmallClass, chores: &mut Chores) -> Obtained<'a> {
        if let Some(magazine) = self.spare_pop(class, Spare::Full, &mut chores.stale) {
            if magazine.level() > 0 {
                return Obtained::Magazine(magazine);
            }
            self.to_depot(class, magazine);
        }
        if let Some(magazine) = self.pop_full(class, &mut chores.tried) {
            return Obtained::Magazine(magazine);
        }
        let Some(magazine) = self.obtain_empty(class, &mut chores.stale) else {
            return Obtained::NoMagazine;
        };
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
    /// `refresh_every`, halved as often as the source's fragmentation earns
    /// ([`Source::refresh_raise`]), to launder.
    fn refreshes(&self, cpu: Cpu) -> bool {
        let every = self.tuning.refresh_every;
        if every == 0 {
            return false;
        }
        let every = (every >> self.source.refresh_raise().min(63)).max(1);
        self.slab(cpu).count_retired().is_multiple_of(every)
    }

    /// Fills an empty magazine this thread holds from the source, in one call; returns its
    /// new level. The source fills from the top (see [`Source::fill`]); a short fill, from
    /// sparse bitmaps or a heap out of memory, is moved down to the bottom slots. A fill that
    /// does not continue a run of fills of its class hands its first objects out a stride
    /// apart, if the class is smaller than a line (`siblings.rs`).
    fn fill(&self, class: SmallClass, magazine: &Magazine) -> usize {
        let slots = magazine.objects();
        let capacity = slots.len();
        let filled = self.source.fill(class, slots);
        let level = filled.count.min(capacity);
        if level < capacity
            && let Some(top) = slots.get(capacity - level..)
        {
            // Upwards: each slot is read before anything is written over it.
            for (to, from) in slots.iter().zip(top) {
                to.store(from.load(Relaxed), Relaxed);
            }
        }
        if filled.opens_run
            && let Some(stride) = siblings::stride(class.size())
            && let Some(objects) = slots.get(..level)
        {
            siblings::spread(objects, stride);
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
            let plan = self.plan_swap(cpu, class, Swap::Alloc);
            let (seen, held) = match self.centre_for_alloc(class, word, seen, held, plan, chores) {
                Centring::Skipped(seen, held) => (seen, held),
                Centring::Again => continue,
                Centring::Finished(object) => return object,
            };
            let level = held.level();
            // SAFETY: DEPOT-SLOW-01.
            let installed = unsafe { self.routines.store_if_equal(field, word, held.word(level)) };
            match installed {
                StoreIfEqual::Stored => {
                    chores.installed = true;
                    self.record_swap(cpu, class, plan.plain());
                    chores.moved = magazine_bytes(class);
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
            let plan = self.plan_swap(cpu, class, Swap::Free);
            let (seen, held) =
                match self.centre_for_free(class, word, seen, held, object, plan, chores) {
                    Centring::Skipped(seen, held) => (seen, held),
                    Centring::Again => continue,
                    Centring::Finished(()) => return,
                };
            // SAFETY: DEPOT-SLOW-01.
            let installed = unsafe { self.routines.store_if_equal(field, word, held.word(0)) };
            match installed {
                StoreIfEqual::Stored => {
                    chores.installed = true;
                    self.record_swap(cpu, class, plan.plain());
                    chores.moved = magazine_bytes(class);
                    if let Seen::Loaded(old, level) = seen {
                        old.set_level(level);
                        self.retire_displaced(cpu, class, old, chores);
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

    /// Retires a magazine with objects that a free's install displaced, now this thread's, with
    /// its level recorded: laundered and retired empty if it is the one in [`REFRESH_EVERY`]
    /// to refresh and the budget has room, else to the spares or the depot.
    fn retire_displaced(
        &self,
        cpu: Cpu,
        class: SmallClass,
        old: Magazine<'a>,
        chores: &mut Chores,
    ) {
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

    /// The swap of an allocation that reverses its CPU's last swap of `class` (see the module
    /// documentation, "Half full on a reversal"): the loaded magazine, read as `word` and empty,
    /// is taken out with the store-if-equal routine, the top half of `held` is moved into it,
    /// and `held` is installed half full in its place. Skipped, giving back what it was given,
    /// unless `plan` says to centre, the word names a magazine and `held` has two objects or
    /// more.
    fn centre_for_alloc(
        &self,
        class: SmallClass,
        word: *mut u8,
        seen: Seen<'a>,
        held: Magazine<'a>,
        plan: SwapPlan,
        chores: &mut Chores,
    ) -> Centring<'a, Option<NonNull<u8>>> {
        let level = held.level();
        let old = match seen {
            Seen::Loaded(old, _) if plan.centre && level >= 2 => old,
            seen => return Centring::Skipped(seen, held),
        };
        let field = Field::hot(class);
        // SAFETY: DEPOT-SLOW-02. Zero keeps the slab invariant.
        let taken = unsafe { self.routines.store_if_equal(field, word, ptr::null_mut()) };
        match taken {
            StoreIfEqual::Stored => {}
            StoreIfEqual::Mismatch(now) if self.is_live(now) => {
                return Centring::Finished(self.serve_alloc(class, held));
            }
            StoreIfEqual::Mismatch(_) => {
                self.retire(class, Spare::Full, held, &mut chores.stale);
                return Centring::Again;
            }
            StoreIfEqual::NoCpu => return Centring::Finished(self.serve_alloc(class, held)),
        }
        // Both are this thread's now: `old`, empty, out of the slab, and `held`.
        let half = level / 2;
        held.move_top_into(level, &old, half);
        // SAFETY: DEPOT-SLOW-01.
        let installed = unsafe {
            self.routines
                .store_if_equal(field, ptr::null_mut(), held.word(level - half))
        };
        match installed {
            StoreIfEqual::Stored => {
                chores.installed = true;
                if let Some(cpu) = chores.cpu {
                    self.record_swap(cpu, class, plan);
                }
                chores.moved = magazine_bytes(class);
                self.retire(class, Spare::Full, old, &mut chores.stale);
                Centring::Again
            }
            StoreIfEqual::Mismatch(now) if self.is_live(now) => {
                self.retire(class, Spare::Full, old, &mut chores.stale);
                Centring::Finished(self.serve_alloc(class, held))
            }
            StoreIfEqual::Mismatch(_) => {
                self.retire(class, Spare::Full, held, &mut chores.stale);
                self.retire(class, Spare::Full, old, &mut chores.stale);
                Centring::Again
            }
            StoreIfEqual::NoCpu => {
                self.retire(class, Spare::Full, old, &mut chores.stale);
                Centring::Finished(self.serve_alloc(class, held))
            }
        }
    }

    /// The swap of a free that `plan` says to centre: the loaded magazine,
    /// read as `word` and full, is taken out with the store-if-equal routine, its top half is
    /// moved into `held`, which is empty, and `held` is installed half full in its place; the
    /// other half retires as any displaced magazine does. Skipped as for an allocation.
    #[allow(clippy::too_many_arguments)]
    fn centre_for_free(
        &self,
        class: SmallClass,
        word: *mut u8,
        seen: Seen<'a>,
        held: Magazine<'a>,
        object: NonNull<u8>,
        plan: SwapPlan,
        chores: &mut Chores,
    ) -> Centring<'a, ()> {
        let (old, level) = match seen {
            Seen::Loaded(old, level) if plan.centre && level >= 2 => (old, level),
            seen => return Centring::Skipped(seen, held),
        };
        let Some(cpu) = chores.cpu else {
            return Centring::Skipped(Seen::Loaded(old, level), held);
        };
        let field = Field::hot(class);
        // SAFETY: DEPOT-SLOW-02. Zero keeps the slab invariant.
        let taken = unsafe { self.routines.store_if_equal(field, word, ptr::null_mut()) };
        match taken {
            StoreIfEqual::Stored => {}
            StoreIfEqual::Mismatch(now) if self.is_live(now) => {
                self.serve_free(class, held, object);
                return Centring::Finished(());
            }
            StoreIfEqual::Mismatch(_) => {
                self.retire(class, Spare::Empty, held, &mut chores.stale);
                return Centring::Again;
            }
            StoreIfEqual::NoCpu => {
                self.serve_free(class, held, object);
                return Centring::Finished(());
            }
        }
        // Both are this thread's now: `old`, full, out of the slab, and `held`, empty.
        let half = level / 2;
        old.move_top_into(level, &held, half);
        // SAFETY: DEPOT-SLOW-01.
        let installed = unsafe {
            self.routines
                .store_if_equal(field, ptr::null_mut(), held.word(half))
        };
        match installed {
            StoreIfEqual::Stored => {
                chores.installed = true;
                self.record_swap(cpu, class, plan);
                chores.moved = magazine_bytes(class);
                self.retire_displaced(cpu, class, old, chores);
                Centring::Again
            }
            StoreIfEqual::Mismatch(now) if self.is_live(now) => {
                self.retire(class, Spare::Full, old, &mut chores.stale);
                self.serve_free_into(class, held, half, object);
                Centring::Finished(())
            }
            StoreIfEqual::Mismatch(_) => {
                self.retire(class, Spare::Full, held, &mut chores.stale);
                self.retire(class, Spare::Full, old, &mut chores.stale);
                Centring::Again
            }
            StoreIfEqual::NoCpu => {
                self.retire(class, Spare::Full, old, &mut chores.stale);
                self.serve_free_into(class, held, half, object);
                Centring::Finished(())
            }
        }
    }

    /// Serves a free with a magazine this thread holds at `level`, below its capacity, and
    /// sends it to the depot.
    fn serve_free_into(
        &self,
        class: SmallClass,
        magazine: Magazine,
        level: usize,
        object: NonNull<u8>,
    ) {
        magazine.set_object(level + 1, object);
        magazine.set_level(level + 1);
        self.to_depot(class, magazine);
    }
}
