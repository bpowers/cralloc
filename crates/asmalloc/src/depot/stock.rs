//! Stock: spares a CPU keeps beyond the fixed limit, for a class whose magazines it sends to
//! the depot and takes back.
//!
//! A thread that allocates many objects and then frees them all, as a request handler does,
//! moves far more magazines per burst than the fixed spare limit holds (`spare.rs`). Each one
//! past the limit goes to the depot when freed and comes back when allocated, and those
//! exchanges reach lines every CPU of the domain writes, and the shared stack and the bitmaps
//! beyond them (`docs/research/launder-and-fill.md`, "sh8bench, accounted"). The rule here is
//! that a CPU keeps what it demonstrably sends away and takes back.
//!
//! **Round trips under contention.** When a slow path sends a magazine of a class to the depot
//! because its CPU's spare stack of that kind is at its limit, it marks the class's stock word:
//! sent, and contended if a compare-and-swap of that push failed. When a slow path of the same
//! CPU later takes a magazine of that kind and class back from the depot, the round trip is
//! complete and the marks are cleared; if the push or this pop found its stack contended, the
//! class's credit for that kind of spare grows by one. Contention is the condition under which
//! the exchange is expensive: a contended stack head costs each exchange a line handed between
//! cores and failed compare-and-swaps, about 1,600 cycles in sh8bench, where an exchange with a
//! stack nobody else is using costs little and stock would buy nothing but parked memory (the
//! condition Bonwick's magazines grew their size by). It regulates itself: stock relieves the
//! contention, the failures stop, the credit stops growing, and disuse takes it back. A CPU
//! whose traffic in a class goes one way, pushing full magazines and taking empty ones or the
//! reverse, never takes back a kind it sent, so its credit stays zero and its spares behave
//! exactly as the fixed limit says. Empty magazines hold no objects, but their exchanges reach
//! the same shared lines, so they have a credit of their own, earned the same way.
//!
//! **Depth.** A spare stack may hold its fixed limit plus its credit, as long as the CPU's stock
//! of that kind (the spares it holds past their fixed limits, counted in the objects a full
//! magazine of the class holds) stays within [`room`]: the CPU's spare budget less what the
//! fixed limits could already park in full spares ([`parked_full`]). So a CPU's full spares,
//! fixed and stock together, never hold more than the spare budget, which the fixed limits
//! alone, splitting it evenly over every class, cannot reach: 139 KiB of 1 MiB with the
//! allocator's limits, since most classes' magazines are bigger than their share. Stock lets the
//! classes in use take the share of those that are not. Empty spares hold no objects; their
//! stock has a room of the same size of its own, counted in the objects the magazines would
//! hold, so their memory is at most that room's worth of pointers; and a class keeps no more
//! empty stock than full, since an empty kept is room for a full one freed later. A CPU that
//! only allocates a class sends its empty magazines away and may take them back to fill from
//! the bitmaps, a round trip of empties alone, and keeps no stock of them. When the room is spent, the
//! next magazine of any class goes to the depot as it does today, and nothing is evicted.
//!
//! **Disuse.** While a class has credit, each pop from its spare stack notes the stack's depth,
//! so the low-water mark of an interval says how much of the stock nobody used. The age limit's
//! sweep (`age.rs`), which takes each class's full spares once an interval, first takes the
//! unused depth off the credit, as the depot's low-water rule does for its stacks (`shed.rs`),
//! and returns empty spares above the new limit to the depot. A class that went unused loses
//! its credit within an interval and its stock with the sweep; a CPU that goes quiet has its
//! whole slab drained by the idle check, stock included.
//!
//! **State.** One word per class in its chunk of the slab, the bytes beside the spare heads
//! (`front::Slab::stock`), and one word per CPU, what it holds as stock of each kind, on the line
//! of the activity counter that every slow path writes (`front::Slab::stock_held`). A magazine
//! is stock if it was pushed past its stack's fixed limit, which the spare depth in its record
//! says, so a pop, the sweep and the drain can each take it off the count. All are written with
//! plain loads and stores by this CPU's slow paths (the drain clears the count of the slab it
//! drains): hints, as the refresh count and the stamps are. A race (a thread preempted between a
//! load and its store, or one that moved to another CPU) loses or misplaces a credit, a mark or
//! a count, which moves a magazine to the depot or keeps it one exchange longer; the count
//! saturates at zero, and the next drain or sweep puts it right. Zero is no credit, no mark and
//! no stock, so the all-zero slab has no stock, and a CPU that never round-trips never writes
//! either word; reading them costs no line that the spare limit's check and the heartbeat do
//! not already touch.

use super::magazine::Magazine;
use super::spare::spare_limit;
use super::{Cache, Source, Tuning};
use crate::classes::SmallClass;
use crate::front::{Cpu, Routines, Spare};
use crate::os::Os;
use core::ptr;
use core::sync::atomic::Ordering::Relaxed;

/// The objects, in bytes, of every class's fixed number of full spares: what the fixed limits
/// let one CPU park in full spares: 139 KiB with the allocator's limits, since a budget of 1 MiB
/// split evenly over the classes leaves most classes' magazines too big for any spare, and the
/// smallest classes four.
pub const fn parked_full(tuning: Tuning) -> usize {
    let mut total = 0usize;
    let mut ordinal = 0;
    while let Some(class) = SmallClass::from_ordinal(ordinal) {
        let limit = spare_limit(tuning, class, Spare::Full);
        total = total.saturating_add(limit.saturating_mul(weight(class)));
        ordinal += 1;
    }
    total
}

/// What one stock magazine of `class` counts against its room: the objects a full one holds.
const fn weight(class: SmallClass) -> usize {
    class.magazine().capacity() * class.size()
}

/// The bytes of stock of each kind one CPU may hold: its spare budget less what the fixed
/// limits could park in full spares. 885 KiB with the allocator's limits, worked out at compile
/// time for them, since the sum takes a division for each class.
pub fn room(tuning: Tuning) -> usize {
    const DEFAULT: usize = room_of(Tuning::DEFAULT);
    if tuning == Tuning::DEFAULT {
        DEFAULT
    } else {
        room_of(tuning)
    }
}

const fn room_of(tuning: Tuning) -> usize {
    tuning.spare_budget.saturating_sub(parked_full(tuning))
}

/// Where a kind's count of stock bytes is in the per-CPU word: full in the low half, empty in
/// the high.
const fn held_shift(kind: Spare) -> u32 {
    match kind {
        Spare::Full => 0,
        Spare::Empty => 32,
    }
}

fn held(word: u64, kind: Spare) -> usize {
    (word >> held_shift(kind)) as u32 as usize
}

fn with_held(word: u64, kind: Spare, bytes: usize) -> u64 {
    let bytes = bytes.min(u32::MAX as usize) as u64;
    word & !(0xffff_ffff << held_shift(kind)) | bytes << held_shift(kind)
}

/// One class's stock word: per kind of spare, the credit (bits 0 to 14 full, 15 to 29 empty),
/// the lowest depth its stack was popped to this interval plus one (bits 30 to 44 full, 45 to 59
/// empty; zero when no pop was seen), and the marks of a magazine sent to the depot and not yet
/// matched by one taken back (bits 60 and 61 full: sent, and its push contended; 62 and 63
/// empty). All zero is no stock and no mark.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(super) struct Stock(pub(super) u64);

const FIELD: u32 = 15;
const MAX: u64 = (1 << FIELD) - 1;

/// The bits of `kind`'s marks: sent, and contended.
const fn marks(kind: Spare) -> (u64, u64) {
    match kind {
        Spare::Full => (1 << 60, 1 << 61),
        Spare::Empty => (1 << 62, 1 << 63),
    }
}

const fn shift(kind: Spare) -> u32 {
    match kind {
        Spare::Full => 0,
        Spare::Empty => FIELD,
    }
}

impl Stock {
    /// The extra spares of `kind` the class may hold.
    pub(super) fn credit(self, kind: Spare) -> u32 {
        (self.0 >> shift(kind) & MAX) as u32
    }

    /// The lowest depth a pop left the stack at this interval, plus one; zero for none.
    fn low(self, kind: Spare) -> u32 {
        (self.0 >> (2 * FIELD + shift(kind)) & MAX) as u32
    }

    fn with(self, at: u32, value: u32) -> Stock {
        let value = (value as u64).min(MAX);
        Stock(self.0 & !(MAX << at) | value << at)
    }

    /// The word after a magazine of `kind` was sent to the depot, `contended` if its push had a
    /// compare-and-swap fail.
    pub(super) fn sent(self, kind: Spare, contended: bool) -> Stock {
        let (sent, hard) = marks(kind);
        Stock(self.0 | sent | if contended { hard } else { 0 })
    }

    /// The word after a magazine of `kind` was taken back from the depot, `contended` if its
    /// pop had a compare-and-swap fail: the marks cleared, and whether this completed a round
    /// trip that met contention either way.
    pub(super) fn returned(self, kind: Spare, contended: bool) -> (Stock, bool) {
        let (sent, hard) = marks(kind);
        if self.0 & sent == 0 {
            return (self, false);
        }
        (
            Stock(self.0 & !(sent | hard)),
            contended || self.0 & hard != 0,
        )
    }

    /// One more credit, up to `most`.
    pub(super) fn grown(self, kind: Spare, most: u32) -> Stock {
        let credit = self.credit(kind);
        if credit >= most {
            return self;
        }
        self.with(shift(kind), credit + 1)
    }

    /// The word after a pop that left the stack `below` magazines deep.
    pub(super) fn popped(self, kind: Spare, below: u32) -> Stock {
        let mark = below.saturating_add(1);
        let low = self.low(kind);
        if low != 0 && low <= mark {
            return self;
        }
        self.with(2 * FIELD + shift(kind), mark)
    }

    /// The word at the end of an interval, for a stack whose fixed limit is `limit`: the
    /// credit less the depth beyond the limit that no pop reached (all of it if nothing was
    /// popped), and no mark. Returns the new word and the credit taken off.
    pub(super) fn settled(self, kind: Spare, limit: u32) -> (Stock, u32) {
        let credit = self.credit(kind);
        let low = self.low(kind);
        let unused = if low == 0 {
            credit
        } else {
            (low - 1).saturating_sub(limit).min(credit)
        };
        let word = self
            .with(shift(kind), credit - unused)
            .with(2 * FIELD + shift(kind), 0);
        (word, unused)
    }
}

impl<'a, O: Os, R: Routines, S: Source> Cache<'a, O, R, S> {
    fn stock(&self, cpu: Cpu, class: SmallClass) -> Stock {
        Stock(self.slab(cpu).stock(class).load(Relaxed).addr() as u64)
    }

    fn set_stock(&self, cpu: Cpu, class: SmallClass, stock: Stock) {
        self.slab(cpu)
            .stock(class)
            .store(ptr::without_provenance_mut(stock.0 as usize), Relaxed);
    }

    /// Notes a magazine of `kind` for `class` just sent to the depot because the current CPU's
    /// spare stack had no room, `contended` if its push had a compare-and-swap fail. Writes the
    /// word only when a mark changes, so a CPU that only sends writes it once.
    pub(super) fn note_sent(&self, class: SmallClass, kind: Spare, contended: bool) {
        if !self.tuning.stock {
            return;
        }
        let Some(cpu) = self.routines.cpu() else {
            return;
        };
        let stock = self.stock(cpu, class);
        let marked = stock.sent(kind, contended);
        if marked != stock {
            self.set_stock(cpu, class, marked);
        }
    }

    /// Notes a magazine of `kind` just popped from the depot for `class`, `contended` if the
    /// pop had a compare-and-swap fail: if the current CPU had sent one of that kind and class,
    /// the round trip is complete, and it earns the class one more credit if either trip met
    /// contention.
    pub(super) fn note_return(&self, class: SmallClass, kind: Spare, contended: bool) {
        if !self.tuning.stock {
            return;
        }
        let Some(cpu) = self.routines.cpu() else {
            return;
        };
        let stock = self.stock(cpu, class);
        let (cleared, earned) = stock.returned(kind, contended);
        if cleared == stock {
            return;
        }
        let most = (room(self.tuning) / weight(class)).min(MAX as usize) as u32;
        let next = if earned {
            cleared.grown(kind, most)
        } else {
            cleared
        };
        self.set_stock(cpu, class, next);
    }

    /// Notes a pop of a magazine of `kind` from the current CPU's spare stack of `class`: takes
    /// it off the CPU's stock if it was stock, and marks the depth it left for the low-water
    /// mark of a class with credit.
    pub(super) fn note_spare_pop(&self, class: SmallClass, kind: Spare, magazine: &Magazine<'_>) {
        if !self.tuning.stock {
            return;
        }
        let Some(cpu) = self.routines.cpu() else {
            return;
        };
        let below = magazine.record().depth;
        if below >= spare_limit(self.tuning, class, kind) as u32 {
            self.count_stock(cpu, kind, class, false);
        }
        let stock = self.stock(cpu, class);
        if stock.credit(kind) == 0 {
            return;
        }
        let marked = stock.popped(kind, below);
        if marked != stock {
            self.set_stock(cpu, class, marked);
        }
    }

    /// Adds a magazine of `class` to `cpu`'s count of stock of `kind`, or takes one off.
    fn count_stock(&self, cpu: Cpu, kind: Spare, class: SmallClass, add: bool) {
        let word = self.slab(cpu).stock_held();
        let now = word.load(Relaxed);
        let bytes = if add {
            held(now, kind).saturating_add(weight(class))
        } else {
            held(now, kind).saturating_sub(weight(class))
        };
        word.store(with_held(now, kind, bytes), Relaxed);
    }

    /// Notes that a push past the fixed limit, which [`Cache::stock_admits`] allowed, took the
    /// magazine.
    pub(super) fn note_stock_push(&self, class: SmallClass, kind: Spare) {
        if let Some(cpu) = self.routines.cpu() {
            self.count_stock(cpu, kind, class, true);
        }
    }

    /// Notes that the sweep took a magazine of `class` from `cpu`'s full spares, which was
    /// stock if it lay `below` past the stack's fixed limit.
    pub(super) fn note_swept(&self, cpu: Cpu, class: SmallClass, below: u32) {
        if self.tuning.stock && below >= spare_limit(self.tuning, class, Spare::Full) as u32 {
            self.count_stock(cpu, Spare::Full, class, false);
        }
    }

    /// Whether the current CPU's spare stack of `kind` for `class`, `over` magazines past its
    /// fixed limit, may take one more: if the class has that much credit and the CPU's spares of
    /// that kind stay within their bound with it.
    pub(super) fn stock_admits(&self, class: SmallClass, kind: Spare, over: u32) -> bool {
        if !self.tuning.stock {
            return false;
        }
        let Some(cpu) = self.routines.cpu() else {
            return false;
        };
        let stock = self.stock(cpu, class);
        // An empty kept is room to free a full one into later, so a class keeps no more empty
        // stock than full: a CPU that only allocates a class, sending its empties away and
        // taking them back to fill from the bitmaps, keeps none.
        let credit = match kind {
            Spare::Full => stock.credit(Spare::Full),
            Spare::Empty => stock.credit(Spare::Empty).min(stock.credit(Spare::Full)),
        };
        if over >= credit {
            return false;
        }
        let now = held(self.slab(cpu).stock_held().load(Relaxed), kind);
        now.saturating_add(weight(class)) <= room(self.tuning)
    }

    /// The sweep's part for `class` on `cpu`, once an interval before it takes the full spares:
    /// takes the unused depth off each credit, and sends the empty spares above the new limit to
    /// the depot. Does nothing for a class without credit.
    pub(super) fn settle_stock(&self, cpu: Cpu, class: SmallClass) {
        if !self.tuning.stock {
            return;
        }
        let stock = self.stock(cpu, class);
        if stock.credit(Spare::Full) == 0 && stock.credit(Spare::Empty) == 0 {
            return;
        }
        let full_limit = spare_limit(self.tuning, class, Spare::Full) as u32;
        let empty_limit = spare_limit(self.tuning, class, Spare::Empty) as u32;
        let (settled, _) = stock.settled(Spare::Full, full_limit);
        let (settled, unused) = settled.settled(Spare::Empty, empty_limit);
        self.set_stock(cpu, class, settled);
        let keep = empty_limit.saturating_add(settled.credit(Spare::Empty));
        let mut stale = false;
        for _ in 0..unused {
            match self.spare_depth(class, Spare::Empty) {
                Some(depth) if depth > keep => {}
                _ => break,
            }
            let Some(magazine) = self.spare_pop(class, Spare::Empty, &mut stale) else {
                break;
            };
            self.to_depot(class, magazine);
        }
        // Its own pops are not the class's use: the interval's mark starts clear.
        self.set_stock(cpu, class, settled);
    }
}

/// A stock word with these credits and no marks, for tests that give a class stock without
/// the contention a single-threaded test cannot make.
#[cfg(test)]
pub(super) fn credits(full: u32, empty: u32) -> *mut u8 {
    let word = Stock::default()
        .with(shift(Spare::Full), full)
        .with(shift(Spare::Empty), empty);
    ptr::without_provenance_mut(word.0 as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credit_grows_to_its_most_and_settles_by_the_unused_depth() {
        let mut stock = Stock::default();
        for _ in 0..5 {
            stock = stock.grown(Spare::Full, 3);
        }
        assert_eq!(stock.credit(Spare::Full), 3);
        assert_eq!(stock.credit(Spare::Empty), 0);
        // Nothing popped all interval: all of it goes.
        assert_eq!(stock.settled(Spare::Full, 4), (Stock::default(), 3));
        // Never below six against a limit of four: two of the three were not needed.
        let used = stock
            .popped(Spare::Full, 9)
            .popped(Spare::Full, 6)
            .popped(Spare::Full, 8);
        let (after, unused) = used.settled(Spare::Full, 4);
        assert_eq!((after.credit(Spare::Full), unused), (1, 2));
        assert_eq!(after.low(Spare::Full), 0);
        // Popped below the limit: all of it was needed.
        let (kept, unused) = stock.popped(Spare::Full, 1).settled(Spare::Full, 4);
        assert_eq!((kept.credit(Spare::Full), unused), (3, 0));
    }

    #[test]
    fn a_round_trip_earns_credit_only_if_it_met_contention() {
        let quiet = Stock::default().sent(Spare::Full, false);
        assert_eq!(
            quiet.returned(Spare::Full, false),
            (Stock::default(), false)
        );
        assert!(quiet.returned(Spare::Full, true).1);
        let hard = Stock::default().sent(Spare::Full, true);
        assert_eq!(hard.returned(Spare::Full, false), (Stock::default(), true));
        // A kind not sent is no round trip, contended or not.
        assert_eq!(hard.returned(Spare::Empty, true), (hard, false));
        assert_eq!(
            Stock::default().returned(Spare::Full, true),
            (Stock::default(), false)
        );
    }

    #[test]
    fn the_kinds_are_apart_and_counts_saturate() {
        let stock = Stock::default()
            .grown(Spare::Empty, u32::MAX)
            .popped(Spare::Empty, u32::MAX);
        assert_eq!(stock.credit(Spare::Empty), 1);
        assert_eq!(stock.credit(Spare::Full), 0);
        assert_eq!(stock.low(Spare::Empty), MAX as u32);
        assert_eq!(stock.low(Spare::Full), 0);
    }

    #[test]
    fn the_bounds_are_what_the_fixed_limits_park() {
        let tuning = Tuning::DEFAULT;
        let full: usize = SmallClass::all()
            .map(|c| spare_limit(tuning, c, Spare::Full) * c.magazine().capacity() * c.size())
            .sum();
        assert_eq!(parked_full(tuning), full);
        assert_eq!(room(tuning) + full, super::super::SPARE_BUDGET);
    }
}
