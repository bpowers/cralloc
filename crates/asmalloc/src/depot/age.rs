//! The magazine age limit: no magazine stays loaded in a slab for more than an interval.
//!
//! An object parked in a loaded magazine pins its page as surely as a live one, and the idle
//! check, being per slab, never reclaims a class that fell out of use on a CPU still busy with
//! others. So each heartbeat looks at one class of its own CPU's slab, the classes in turn (by
//! the CPU's activity count). If the magazine loaded there has been loaded for an interval
//! ([`IDLE_TICKS`](super::IDLE_TICKS) of [`Os::ticks`], the interval the idle check uses), the
//! heartbeat takes it with the store-if-equal routine, expecting the word it read and storing
//! zero, as a replacement would; launders its objects to the source; and sends the empty
//! magazine to the depot's empty stack. A class in use pays one refill an interval; a class out
//! of use is emptied. A loaded magazine with no objects pins nothing and is left alone.
//!
//! **Spares.** Full spares need a rule of their own: a CPU that only frees a class (a pipeline's
//! consumer) pushes full spares that nothing on it ever pops, and its loaded magazine, replaced
//! on every exchange, never grows old; a class that fell out of use just after an allocation
//! emptied its magazine keeps its spares behind an empty one. Popping one spare at a time
//! cannot help either: frees refill the stack, and only its top ever moves. So once an interval
//! each CPU sweeps its slab: from the first heartbeat after an interval has passed since the
//! last sweep began, each heartbeat takes one class's whole stack of full spares, the classes
//! in turn, with one store-if-equal of its head to zero; launders the top magazine; and sends
//! the rest to the depot, where consumers or the depot's low-water rule (`shed.rs`) move them
//! on.
//! A class with stock (`stock.rs`) is not taken whole: the sweep takes its unused depth off the
//! credit and returns what lies above the new limit a magazine a heartbeat. A class in use pays one refill an interval for this too. The sweep's state is one word
//! in the cold part of the slab ([`Sweep`]); a heartbeat whose sweep laundered skips its look,
//! so the rule still launders at most one magazine a heartbeat.
//!
//! **Stamps.** One word per class in the cold part of the slab, a [`Stamp`], says which magazine
//! is loaded there and since when. The slow path that installs a magazine clears it in its
//! heartbeat, and a look that finds a magazine its stamp does not name (none, after a clear)
//! writes a fresh one, so a magazine ages from the first look that sees it loaded: on a busy
//! CPU microseconds after its install. Installing writes no time, because reading the clock on
//! every slow path is measurably slow (`heartbeat::Clock`); the clear matters because a
//! magazine taken back often returns from the empty stack with the same number, and must not
//! inherit its old stamp. Zero is no stamp, so the all-zero slab is valid. Each write is a
//! single store; a lost or misplaced one (a thread that moved to another CPU before its
//! heartbeat, a slow path a signal cut short) moves a take earlier or later and does nothing
//! else.
//!
//! **No fence.** The slab is the thread's own, so the drain's protocol is not needed: the
//! routine's commit is the take's only write to the slab, and it happens only if the word is
//! still the one the look read.
//!
//! **A thread that moves.** The look reads the hot word of the CPU the slow path ran on, `A`,
//! without protection. The routine then runs on whatever CPU the thread is on when its sequence
//! starts, `B`, and starts again wherever it is if it is moved or interrupted before its
//! commit. It stores zero only if `B`'s word for the class equals the word read from `A`. A hot
//! word that is neither zero nor a marker is the address of a slot of a magazine of the class
//! that nothing else refers to (the slab invariant), and the look takes only such a word. So
//! the commit happens only if that magazine is loaded on `B`, at the level the word encodes,
//! with `B`'s word its only reference; the commit removes that reference, and the magazine,
//! with its objects in slots `1..=level`, is then this thread's alone. That holds whether `B`
//! is `A` or not, and whatever happened to the magazine between the read and the commit (it may
//! have gone through the depot and come back to the same level). If the words differ, nothing
//! is written. A move only makes the decision stale: the magazine may have been loaded again
//! since its stamp was read, and taking it then costs `B` a refill. The stamp the take clears
//! is `A`'s, which is right unless the thread moved, and is a hint either way. The interleaving
//! checker runs the take against mutators and drains, with the taker moving between CPUs
//! (`interleave.rs`).
//!
//! The take of a spare stack is the same argument for a spare head: a head that is neither zero
//! nor a marker is the only reference to the top magazine of that stack, whose links are the
//! only references to the rest, so a commit that finds the word read hands this thread the
//! whole chain as it is at the commit.
//!
//! The ledger for the unsafe blocks is `docs/safety/asmalloc/depot/age-safety.md`.

use super::heartbeat::{Clock, IDLE_UNITS, TICK_SHIFT};
use super::magazine::{Magazine, MagazineId};
use super::stock::Swept;
use super::{Cache, Source};
use crate::classes::SMALL_COUNT;
use crate::classes::{SmallClass, is_marker};
use crate::front::{Cpu, Field, Routines, Spare, StoreIfEqual};
use crate::os::Os;
use core::ptr;
use core::sync::atomic::Ordering::{Acquire, Relaxed, Release};

/// Which magazine a slab had loaded for a class, and since when: the magazine's number in the
/// low 32 bits and the clock, in units of 2^[`TICK_SHIFT`] ticks, in the high 32. Zero is no
/// stamp; no magazine has the number zero, so no stamp is zero.
///
/// The time is truncated: a stamp about 2^51 ticks (days) old reads as young until it is older
/// still, which only postpones a take.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct Stamp(pub(super) u64);

/// What a look makes of a magazine's stamp.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Age {
    /// The stamp names another magazine, or none: write this one in its place.
    Unstamped(Stamp),
    /// Loaded for less than an interval.
    Young,
    /// Loaded for an interval or more: take it.
    Old,
}

impl Stamp {
    /// The stamp of magazine `id`, loaded at `now`.
    pub(super) fn new(id: MagazineId, now: u64) -> Stamp {
        let time = (now >> TICK_SHIFT) as u32;
        Stamp((time as u64) << 32 | id.get() as u64)
    }

    /// How old magazine `id` is at `now`, by this stamp.
    pub(super) fn age(self, id: MagazineId, now: u64) -> Age {
        if self.0 as u32 != id.get() {
            return Age::Unstamped(Stamp::new(id, now));
        }
        let time = (now >> TICK_SHIFT) as u32;
        let loaded = time.wrapping_sub((self.0 >> 32) as u32) as i32;
        // A clock behind the stamp went back: stamp the magazine again from now, so it waits
        // one interval more instead of until the clock catches up.
        if loaded < 0 {
            return Age::Unstamped(Stamp::new(id, now));
        }
        if loaded < IDLE_UNITS as i32 {
            Age::Young
        } else {
            Age::Old
        }
    }
}

/// Where a CPU's sweep of its full spare stacks is: when it began, in units of
/// 2^[`TICK_SHIFT`] ticks (bits 32 to 63), whether one has begun (bit 16), and the ordinal of the
/// next class to sweep (bits 0 to 7), the class count once the sweep is done. Zero is no sweep
/// yet; the first clocked heartbeat begins one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct Sweep(pub(super) u64);

const SWEEP_BEGUN: u64 = 1 << 16;
const SWEEP_CURSOR: u64 = 0xff;
const _: () = assert!(SMALL_COUNT as u64 <= SWEEP_CURSOR);

impl Sweep {
    /// A sweep beginning at `now`, at the first class.
    pub(super) fn begin(now: u64) -> Sweep {
        Sweep(((now >> TICK_SHIFT) as u32 as u64) << 32 | SWEEP_BEGUN)
    }

    /// The ordinal of the class this heartbeat sweeps, if a sweep is under way.
    pub(super) fn next(self) -> Option<usize> {
        let cursor = (self.0 & SWEEP_CURSOR) as usize;
        (self.0 & SWEEP_BEGUN != 0 && cursor < SMALL_COUNT).then_some(cursor)
    }

    /// The sweep after the class it named.
    pub(super) fn advanced(self) -> Sweep {
        Sweep(self.0 + 1)
    }

    /// What a heartbeat with the clock at `now` makes of a sweep that is not under way: a new
    /// sweep once an interval has passed since this one began (or if none has), the same
    /// sweep restarted from now if the clock went back, or nothing.
    pub(super) fn due(self, now: u64) -> Option<Sweep> {
        if self.0 & SWEEP_BEGUN == 0 {
            return Some(Sweep::begin(now));
        }
        let time = (now >> TICK_SHIFT) as u32;
        let since = time.wrapping_sub((self.0 >> 32) as u32) as i32;
        if since < 0 {
            // Done, as if it had begun now: the next one waits an interval, not for the clock.
            return Some(Sweep(Sweep::begin(now).0 | SMALL_COUNT as u64));
        }
        (since >= IDLE_UNITS as i32).then(|| Sweep::begin(now))
    }
}

/// A hot word read without protection that names a loaded magazine: the word, and the
/// magazine and level it encodes. Only a name until [`Cache::reclaim`] commits; nothing reads
/// or writes the magazine's slots through it before then.
pub(super) struct Loaded<'a> {
    word: *mut u8,
    magazine: Magazine<'a>,
    level: usize,
}

impl<'a, O: Os, R: Routines, S: Source> Cache<'a, O, R, S> {
    /// Clears the stamp of `class` on `cpu`'s slab, where a slow path has just loaded a
    /// magazine.
    pub(super) fn unstamp(&self, cpu: Cpu, class: SmallClass) {
        self.slab(cpu).stamp(class).store(0, Relaxed);
    }

    /// The magazine `cpu`'s hot word for `class` names, read without protection: `None` if
    /// the word is zero or a marker (a drain's, or another generation's, which the slow paths
    /// handle).
    pub(super) fn hot_magazine(&self, cpu: Cpu, class: SmallClass) -> Option<Loaded<'a>> {
        let word = self.slab(cpu).field(Field::hot(class)).load(Acquire);
        if word.is_null() || is_marker(word.addr()) {
            return None;
        }
        match self.area.loaded(word.addr(), class.magazine()) {
            Some((magazine, level)) => Some(Loaded {
                word,
                magazine,
                level,
            }),
            None => self
                .os()
                .abort("a slab word names something that is not a magazine"),
        }
    }

    /// Takes the magazine `seen` names out of the current CPU's hot word for `class`, if that
    /// word is still `seen.word`, by storing zero there with the store-if-equal routine; then
    /// launders it and sends it, empty, to the depot. Whether it took it.
    pub(super) fn reclaim(&self, class: SmallClass, seen: Loaded<'a>) -> bool {
        // SAFETY: AGE-01.
        let taken = unsafe {
            self.routines
                .store_if_equal(Field::hot(class), seen.word, ptr::null_mut())
        };
        if taken != StoreIfEqual::Stored {
            return false;
        }
        seen.magazine.set_level(seen.level);
        self.launder_held(class, &seen.magazine);
        self.to_depot(class, seen.magazine);
        true
    }

    /// The age limit's look at `class` on `cpu`'s slab, the CPU the slow path ran on (see the
    /// module documentation). Reads the clock only if a magazine is loaded there. Launders at
    /// most one magazine; whether it took one.
    pub(super) fn age_look(&self, cpu: Cpu, class: SmallClass, clock: &Clock<'_, O>) -> bool {
        let slab = self.slab(cpu);
        let Some(seen) = self.hot_magazine(cpu, class) else {
            return false;
        };
        let stamp = slab.stamp(class);
        match Stamp(stamp.load(Relaxed)).age(seen.magazine.id(), clock.now()) {
            Age::Unstamped(fresh) => {
                stamp.store(fresh.0, Relaxed);
                return false;
            }
            Age::Young => return false,
            Age::Old => {}
        }
        let taken = seen.level > 0 && self.reclaim(class, seen);
        if taken {
            stamp.store(0, Relaxed);
        }
        taken
    }

    /// One heartbeat's part of `cpu`'s sweep of its full spare stacks (see the module
    /// documentation), `looking` when the heartbeat may read the clock to begin a sweep.
    /// Whether it laundered a magazine.
    pub(super) fn sweep_step(&self, cpu: Cpu, looking: bool, clock: &Clock<'_, O>) -> bool {
        let word = self.slab(cpu).sweep();
        let sweep = Sweep(word.load(Relaxed));
        let Some(ordinal) = sweep.next() else {
            if looking && let Some(next) = sweep.due(clock.now()) {
                word.store(next.0, Relaxed);
            }
            return false;
        };
        let Some(class) = SmallClass::from_ordinal(ordinal) else {
            word.store(sweep.advanced().0, Relaxed);
            return false;
        };
        match self.sweep_stock(cpu, class) {
            Swept::Plain => {
                // A stack deeper than its fixed limit (stock whose word a race lost) is not
                // taken whole: its top goes back a magazine a heartbeat, as stock's does, and
                // the sweep stays on the class until the stack is within the limit.
                if self.full_spares_over_limit(cpu, class) {
                    return self.return_top_spare(class);
                }
                word.store(sweep.advanced().0, Relaxed);
                self.take_spares(cpu, class)
            }
            Swept::Stocked { laundered, more } => {
                // A class with stock returns it a magazine a heartbeat, the sweep staying on
                // it until none is left above its limit (`stock.rs`).
                if !more {
                    word.store(sweep.advanced().0, Relaxed);
                }
                laundered
            }
        }
    }

    /// Takes the current CPU's stack of full spares of `class`, if its head is still the one
    /// read from `cpu`'s slab, launders its top magazine and sends the rest to the depot.
    /// Whether it laundered one.
    pub(super) fn take_spares(&self, cpu: Cpu, class: SmallClass) -> bool {
        self.take_spares_keeping(cpu, class, 0)
    }

    /// [`Cache::take_spares`], except that the top `keep` magazines go back on the current
    /// CPU's stack, in their order, and only those under them are swept: the first of those
    /// laundered and the rest sent to the depot. A kept magazine the stack does not take back
    /// (its limit, a drain) goes to the depot too. The work is a walk of the stack's links and
    /// a push for each kept magazine, none of them atomic, and the sweep's own work on what
    /// lies under them. Whether it laundered one.
    pub(super) fn take_spares_keeping(&self, cpu: Cpu, class: SmallClass, keep: u32) -> bool {
        let field = Field::spare(class, Spare::Full);
        let head = self.slab(cpu).field(field).load(Acquire);
        if head.is_null() || is_marker(head.addr()) {
            return false;
        }
        // SAFETY: AGE-02.
        let taken = unsafe { self.routines.store_if_equal(field, head, ptr::null_mut()) };
        if taken != StoreIfEqual::Stored {
            return false;
        }
        let mut next = head.addr();
        let mut first = true;
        let mut laundered = false;
        // The kept magazines, deepest first: a list of the taken chain's own, linked through
        // their link words, which this thread holds now.
        let mut kept = ptr::null_mut::<u8>();
        let mut seen = 0u32;
        while next != 0 {
            let Some(magazine) = self.area.magazine(next, class.magazine()) else {
                self.os()
                    .abort("a spare stack names something that is not a magazine");
            };
            // The link must be read before the depot or the list overwrites it.
            next = magazine.link().load(Acquire).addr();
            if seen < keep {
                seen += 1;
                magazine.link().store(kept, Release);
                kept = magazine.base().as_ptr();
                continue;
            }
            // Drop the spare depth the record carries.
            magazine.set_level(magazine.level());
            if first {
                self.launder_held(class, &magazine);
                first = false;
                laundered = true;
            }
            self.to_depot(class, magazine);
        }
        let mut stale = false;
        let mut back = kept.addr();
        while back != 0 {
            let Some(magazine) = self.area.magazine(back, class.magazine()) else {
                self.os()
                    .abort("a spare stack names something that is not a magazine");
            };
            back = magazine.link().load(Acquire).addr();
            magazine.set_level(magazine.level());
            if let Err(magazine) = self.spare_push(class, Spare::Full, magazine, &mut stale) {
                self.to_depot(class, magazine);
            }
        }
        laundered
    }

    /// Whether `cpu`'s stack of full spares of `class` holds more than its fixed limit and its
    /// credit allow.
    fn full_spares_over_limit(&self, cpu: Cpu, class: SmallClass) -> bool {
        self.spare_depth(class, Spare::Full)
            .is_some_and(|depth| depth > self.full_keep(cpu, class))
    }

    /// Pops the top of the current CPU's full spares of `class`, launders it and sends it to
    /// the depot empty. Whether it laundered one.
    pub(super) fn return_top_spare(&self, class: SmallClass) -> bool {
        let mut stale = false;
        let Some(magazine) = self.spare_pop(class, Spare::Full, &mut stale) else {
            return false;
        };
        // Drop the spare depth the record carries.
        magazine.set_level(magazine.level());
        self.launder_held(class, &magazine);
        self.to_depot(class, magazine);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::super::IDLE_TICKS;
    use super::*;
    use crate::classes::SMALL_COUNT;

    fn id(n: u32) -> MagazineId {
        MagazineId::new(n).unwrap()
    }

    #[test]
    fn a_stamp_ages_after_an_interval_and_only_for_its_magazine() {
        let start = 9 * IDLE_TICKS + 12345;
        assert_eq!(
            Stamp(0).age(id(7), start),
            Age::Unstamped(Stamp::new(id(7), start))
        );
        let stamp = Stamp::new(id(7), start);
        assert_ne!(stamp.0, 0);
        assert_eq!(stamp.age(id(7), start), Age::Young);
        assert_eq!(
            stamp.age(id(7), start + IDLE_TICKS - (1 << TICK_SHIFT)),
            Age::Young
        );
        assert_eq!(stamp.age(id(7), start + IDLE_TICKS), Age::Old);
        assert_eq!(stamp.age(id(7), start + 40 * IDLE_TICKS), Age::Old);
        let later = start + 3 * IDLE_TICKS;
        assert_eq!(
            stamp.age(id(8), later),
            Age::Unstamped(Stamp::new(id(8), later))
        );
    }

    #[test]
    fn a_sweep_goes_round_every_class_once_an_interval() {
        let start = 3 * IDLE_TICKS;
        let first = Sweep(0).due(start).expect("the first look begins a sweep");
        let mut sweep = first;
        for ordinal in 0..SMALL_COUNT {
            assert_eq!(sweep.next(), Some(ordinal));
            sweep = sweep.advanced();
        }
        assert_eq!(sweep.next(), None);
        assert_eq!(sweep.due(start + IDLE_TICKS - (1 << TICK_SHIFT)), None);
        assert_eq!(
            sweep.due(start + IDLE_TICKS),
            Some(Sweep::begin(start + IDLE_TICKS))
        );
        // A clock that went back restarts the wait from now, not from the old start.
        let back = sweep.due(IDLE_TICKS).expect("the clock went back");
        assert_eq!(back.next(), None);
        assert_eq!(back.due(2 * IDLE_TICKS - (1 << TICK_SHIFT)), None);
        assert!(back.due(2 * IDLE_TICKS).is_some());
    }

    #[test]
    fn a_clock_that_goes_back_restamps_the_magazine() {
        let stamp = Stamp::new(id(1), 500 * IDLE_TICKS);
        let back = 2 * IDLE_TICKS;
        assert_eq!(
            stamp.age(id(1), back),
            Age::Unstamped(Stamp::new(id(1), back))
        );
        let again = Stamp::new(id(1), back);
        assert_eq!(again.age(id(1), back + IDLE_TICKS), Age::Old);
    }
}
