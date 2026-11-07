//! The per-CPU spare stacks, and their budget.
//!
//! Each CPU keeps, per class, a stack of magazines with objects and a stack of empty ones, with
//! their heads in its slab beside the class's hot word, changed only by the stack routines. A thread that oscillates
//! around a magazine boundary then swaps locally without touching shared state.
//!
//! The spares of one CPU are limited by a byte budget, [`SPARE_BUDGET`] of objects split evenly
//! among the classes, and to [`MAX_SPARES`] magazines per stack. The stack-push routine has no
//! limit of its own, so the slow path checks the depth before it pushes: each magazine records
//! how many lie below it on its stack, and the slow path reads the head's record without
//! protection. A race can put a stack one or two magazines over its limit, never more than
//! the threads racing, and the next push sees the true depth again.

use super::magazine::{Magazine, Record};
use super::{Cache, Source, Tuning};
use crate::classes::{SMALL_COUNT, SmallClass, is_marker};
use crate::front::{Field, Routines, Spare, StackPop, StackPush};
use crate::os::Os;
use core::sync::atomic::Ordering::Acquire;

/// The bytes of objects one CPU may keep in spare magazines, over all classes, by default.
pub const SPARE_BUDGET: usize = 1 << 20;

/// The most magazines one spare stack holds, by default.
///
/// A thread that allocates and frees in bursts of a few magazines stays out of the depot
/// with a stack this deep; at one or two, sh8bench on sixteen threads is a few percent
/// slower. The price falls on a CPU whose traffic in a class goes one way: it fills a stack
/// it does not take back, a CPU that only allocates with empty magazines and one that only
/// frees with full ones, until the sweep moves them on. That is about 0.5 MiB of a
/// four-to-four mpmc's peak (`docs/research/memory.md`, "Small heaps").
pub const MAX_SPARES: usize = 4;

/// How many magazines a CPU's spare stack of this kind may hold for `class`: for magazines
/// with objects, as many as fit in the class's share of the budget; for empty ones, which hold
/// no objects, the cap.
pub const fn spare_limit(tuning: Tuning, class: SmallClass, kind: Spare) -> usize {
    match kind {
        Spare::Empty => tuning.max_spares,
        Spare::Full => {
            let bytes = class.magazine().capacity() * class.size();
            let fit = tuning.spare_budget / SMALL_COUNT / bytes;
            if fit < tuning.max_spares {
                fit
            } else {
                tuning.max_spares
            }
        }
    }
}

/// [`spare_limit`] as the slow paths use it: for the allocator's limits from a table worked out
/// at compile time, since the full limit takes a division by a class's magazine bytes.
#[inline]
pub(super) fn fixed_limit(tuning: Tuning, class: SmallClass, kind: Spare) -> u32 {
    const FULL: [u16; SMALL_COUNT] = {
        let mut table = [0u16; SMALL_COUNT];
        let mut ordinal = 0;
        while let Some(class) = SmallClass::from_ordinal(ordinal) {
            table[ordinal] = spare_limit(Tuning::DEFAULT, class, Spare::Full) as u16;
            ordinal += 1;
        }
        table
    };
    if tuning == Tuning::DEFAULT {
        match kind {
            Spare::Full => FULL[class.ordinal() % SMALL_COUNT] as u32,
            Spare::Empty => MAX_SPARES as u32,
        }
    } else {
        spare_limit(tuning, class, kind) as u32
    }
}

/// A stack head read without protection, for the depth check.
pub(super) enum Head {
    /// Empty: the new magazine would be the only one.
    Empty,
    /// A magazine with this many below it.
    Depth(u32),
    /// A marker: the stack is being drained, or was by a process this one forked from.
    Marker(*mut u8),
}

impl<'a, O: Os, R: Routines, S: Source> Cache<'a, O, R, S> {
    /// Pops a spare of `kind` for `class` from the current CPU's stack. A stack that turns out
    /// to hold another generation's marker is reported, so that the heartbeat drains the slab.
    pub(super) fn spare_pop(
        &self,
        class: SmallClass,
        kind: Spare,
        stale: &mut bool,
    ) -> Option<Magazine<'a>> {
        match self.routines.stack_pop(Field::spare(class, kind)) {
            StackPop::Popped(head) => {
                let magazine = self.magazine_or_abort(head.addr().get(), class);
                self.note_spare_pop(class, kind, &magazine);
                Some(magazine)
            }
            StackPop::Empty => {
                if let Some(Head::Marker(marker)) = self.spare_head(class, kind) {
                    *stale |= !self.is_live(marker);
                }
                None
            }
            StackPop::NoCpu => None,
        }
    }

    fn magazine_or_abort(&self, address: usize, class: SmallClass) -> Magazine<'a> {
        match self.area.magazine(address, class.magazine()) {
            Some(magazine) => magazine,
            None => self
                .os()
                .abort("a spare stack names something that is not a magazine"),
        }
    }

    /// The current CPU's head of a spare stack, read without protection.
    fn spare_head(&self, class: SmallClass, kind: Spare) -> Option<Head> {
        let cpu = self.routines.cpu()?;
        let head = self
            .slab(cpu)
            .field(Field::spare(class, kind))
            .load(Acquire);
        Some(if head.is_null() {
            Head::Empty
        } else if is_marker(head.addr()) {
            Head::Marker(head)
        } else {
            let below = self.magazine_or_abort(head.addr(), class);
            Head::Depth(below.record().depth.saturating_add(1))
        })
    }

    /// How many magazines the current CPU's spare stack of `kind` for `class` holds, read
    /// without protection; `None` without a valid CPU or for a stack being drained.
    pub(super) fn spare_depth(&self, class: SmallClass, kind: Spare) -> Option<u32> {
        match self.spare_head(class, kind)? {
            Head::Empty => Some(0),
            Head::Depth(depth) => Some(depth),
            Head::Marker(_) => None,
        }
    }

    /// Pushes a magazine this thread holds, with its level recorded, on the current CPU's spare
    /// stack of `kind` for `class`, if the budget allows. Gives the magazine back if not, or if
    /// the stack is being drained or the thread has no valid CPU. A stack head that is another
    /// generation's marker is reported, so that the heartbeat drains that slab.
    pub(super) fn spare_push(
        &self,
        class: SmallClass,
        kind: Spare,
        magazine: Magazine<'a>,
        stale: &mut bool,
    ) -> Result<(), Magazine<'a>> {
        let limit = fixed_limit(self.tuning, class, kind);
        let depth = match self.spare_head(class, kind) {
            None => return Err(magazine),
            Some(Head::Marker(marker)) => {
                *stale |= !self.is_live(marker);
                return Err(magazine);
            }
            Some(Head::Empty) => 0,
            Some(Head::Depth(depth)) => depth,
        };
        if depth >= limit && !self.stock_admits(class, kind, depth - limit) {
            return Err(magazine);
        }
        magazine.set_record(Record {
            level: magazine.level(),
            depth,
        });
        // SAFETY: SPARE-01.
        let pushed = unsafe {
            self.routines
                .stack_push(Field::spare(class, kind), magazine.base())
        };
        match pushed {
            StackPush::Pushed => Ok(()),
            StackPush::Marker | StackPush::NoCpu => Err(magazine),
        }
    }

    /// Retires a magazine this thread holds, with its level recorded: to the current CPU's
    /// spares of `kind` if they have room, else to the depot.
    pub(super) fn retire(
        &self,
        class: SmallClass,
        kind: Spare,
        magazine: Magazine<'a>,
        stale: &mut bool,
    ) {
        if let Err(magazine) = self.spare_push(class, kind, magazine, stale) {
            let contended = self.to_depot(class, magazine);
            self.note_sent(class, kind, contended);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_full_spares_of_a_cpu_stay_within_the_budget() {
        let tuning = Tuning::DEFAULT;
        let total: usize = SmallClass::all()
            .map(|c| spare_limit(tuning, c, Spare::Full) * c.magazine().capacity() * c.size())
            .sum();
        assert!(total <= SPARE_BUDGET, "{total}");
        // The smallest class gets the most, two of its 8 KiB of objects in a 23 KiB share; a
        // class whose magazine holds more than its share gets none.
        let smallest = SmallClass::from_ordinal(0).unwrap();
        assert_eq!(spare_limit(tuning, smallest, Spare::Full), 2);
        let sixty_four = SmallClass::from_ordinal(7).unwrap();
        assert_eq!(sixty_four.size(), 64);
        assert_eq!(spare_limit(tuning, sixty_four, Spare::Full), 0);
        let largest = SmallClass::from_ordinal(SMALL_COUNT - 1).unwrap();
        assert_eq!(spare_limit(tuning, largest, Spare::Full), 0);
        assert!(SmallClass::all().all(|c| spare_limit(tuning, c, Spare::Empty) == MAX_SPARES));
    }
}
