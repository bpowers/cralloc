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
//! of use is emptied. A look that finds no magazine loaded launders one of the class's full
//! spares instead, if it has any, so a class that was taken back sheds its spares too, one per
//! look. A loaded magazine with no objects pins nothing and is left alone.
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
//! The ledger for the unsafe block is `docs/safety/asmalloc/depot/age-safety.md`.

use super::heartbeat::{Clock, IDLE_UNITS, TICK_SHIFT};
use super::magazine::{Magazine, MagazineId};
use super::{Cache, Source};
use crate::classes::{SmallClass, is_marker};
use crate::front::{Cpu, Field, Routines, Spare, StoreIfEqual};
use crate::os::Os;
use core::ptr;
use core::sync::atomic::Ordering::{Acquire, Relaxed};

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
        // Signed, so that a clock that went back (another CPU's, say) reads as young.
        let loaded = time.wrapping_sub((self.0 >> 32) as u32) as i32;
        if loaded < IDLE_UNITS as i32 {
            Age::Young
        } else {
            Age::Old
        }
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
    /// most one magazine.
    pub(super) fn age_look(&self, cpu: Cpu, class: SmallClass, clock: &Clock<'_, O>) {
        let slab = self.slab(cpu);
        let Some(seen) = self.hot_magazine(cpu, class) else {
            if slab.field(Field::hot(class)).load(Relaxed).is_null() {
                self.launder_spare(cpu, class);
            }
            return;
        };
        let stamp = slab.stamp(class);
        match Stamp(stamp.load(Relaxed)).age(seen.magazine.id(), clock.now()) {
            Age::Unstamped(fresh) => {
                stamp.store(fresh.0, Relaxed);
                return;
            }
            Age::Young => return,
            Age::Old => {}
        }
        if seen.level > 0 && self.reclaim(class, seen) {
            stamp.store(0, Relaxed);
        }
    }

    /// Launders one full spare of `class` from the current CPU's stack, if the head of `cpu`'s
    /// stack, read without protection, says there is one. For a class with no magazine loaded,
    /// whose spares nothing on this CPU is about to use.
    fn launder_spare(&self, cpu: Cpu, class: SmallClass) {
        let head = self
            .slab(cpu)
            .field(Field::spare(class, Spare::Full))
            .load(Relaxed);
        if head.is_null() || is_marker(head.addr()) {
            return;
        }
        // Another generation's marker found here is the slow path's to report.
        let mut stale = false;
        if let Some(magazine) = self.spare_pop(class, Spare::Full, &mut stale) {
            self.launder_held(class, &magazine);
            self.to_depot(class, magazine);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::IDLE_TICKS;
    use super::*;

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
    fn a_clock_that_goes_back_keeps_a_magazine_young() {
        let stamp = Stamp::new(id(1), 5 * IDLE_TICKS);
        assert_eq!(stamp.age(id(1), 2 * IDLE_TICKS), Age::Young);
        assert_eq!(stamp.age(id(1), 0), Age::Young);
    }
}
