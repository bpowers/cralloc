//! Stock: spares a CPU keeps beyond the fixed limit, for a class whose magazines it sends to
//! the depot and takes back while the depot is contended.
//!
//! A thread that allocates many objects and then frees them all, as a request handler does,
//! moves far more magazines per burst than the fixed spare limit holds (`spare.rs`). Each one
//! past the limit goes to the depot when freed and comes back when allocated, and those
//! exchanges reach lines every CPU of the domain writes, and the shared stack and the bitmaps
//! beyond them (`docs/research/launder-and-fill.md`, "sh8bench, accounted"). The rule here is
//! that a CPU keeps what it demonstrably sends away and takes back, when the sending is costly.
//!
//! **Round trips under contention.** When a slow path sends a magazine of a class to the depot
//! because its CPU's spare stack of that kind is at its limit, it marks the class's stock word:
//! sent, and contended if a compare-and-swap of that push failed. When a slow path of the same
//! CPU later takes a magazine of that kind and class back from the depot, the round trip is
//! complete and the marks are cleared; if the push or this pop found its stack contended, the
//! class's credit for that kind of spare grows by one, up to as many magazines as the room
//! below holds. Contention is the condition under which the exchange is expensive: a contended
//! stack head costs each exchange a line handed between cores and failed compare-and-swaps,
//! about 1,600 cycles in sh8bench, where an exchange with a stack nobody else is using costs
//! little and stock would buy nothing but parked memory (the condition Bonwick's magazines grew
//! their size by). It regulates itself: stock relieves the contention, the failures stop, the
//! credit stops growing, and disuse takes it back. A CPU whose traffic in a class goes one way,
//! pushing full magazines and taking empty ones or the reverse, never takes back a kind it
//! sent, so its credit stays zero and its spares behave exactly as the fixed limit says. Empty
//! magazines hold no objects, but their exchanges reach the same shared lines, so they have a
//! credit of their own, earned the same way; and a class keeps no more empty stock than full,
//! since an empty kept is room for a full one freed later.
//!
//! **Depth.** A spare stack may hold its fixed limit plus its credit, as long as the CPU's stock
//! of that kind (the magazines its stacks hold past their fixed limits, counted in the objects a
//! full magazine of the class holds) stays within [`room`]: the CPU's spare budget less what the
//! fixed limits could already park in full spares ([`parked_full`]). So a CPU's full spares,
//! fixed and stock together, never hold more than the spare budget, which the fixed limits
//! alone, splitting it evenly over every class, cannot reach: 139 KiB of 1 MiB with the
//! allocator's limits, since most classes' magazines are bigger than their share. Stock lets the
//! classes in use take the share of those that are not. Empty stock has a room of the same size
//! of its own, counted the same way, so its memory is at most that room's worth of pointers.
//! When the room is spent, the next magazine of any class goes to the depot as it does today,
//! and nothing is evicted. What a CPU holds as stock is read, not kept: the check adds up, over
//! the classes in the CPU's set of classes that may keep stock, how far each stack's head lies
//! past its fixed limit, which the spare depth in the head's record says. A count kept beside the stacks could not be: a thread moved
//! between a stack routine and the count's update, or preempted between its load and its store,
//! misplaces it, and nothing but an idle drain would put an excess right.
//!
//! **Disuse.** While a class has credit, each pop from its spare stack notes the stack's depth,
//! so the low-water mark of an interval says how much of the stock nobody used. The age limit's
//! sweep (`age.rs`), which visits each class of its CPU once an interval, takes that unused
//! depth off the credit, as the depot's low-water rule does for its stacks (`shed.rs`), and then
//! returns the magazines above the new limit one a heartbeat, staying on the class until none is
//! left: full ones to the source, empty ones to the depot. Stock in use loses nothing; a class
//! that went unused loses its credit within an interval and its stock over the heartbeats that
//! follow. A class whose stock is in use keeps its fixed spares too, which the sweep otherwise
//! takes: the pops that use the stock reach them, or the low-water mark shrinks the stock until
//! they do. Objects in circulation through stock are replaced by the lowest free ones as any
//! others are, by the refresh rule (`slow.rs`), which launders one retiring full magazine in
//! [`super::REFRESH_EVERY`] before it would reach the spares. A CPU that goes quiet has its
//! whole slab drained by the idle check, stock included; its credit stays, costing nothing,
//! until its next sweep finds it unused.
//!
//! **State.** One word per class in its chunk of the slab, the bytes beside the spare heads
//! (`front::Slab::stock`), and a per-CPU set of the classes that may keep stock, on the line of
//! the activity counter (`front::Slab::stocked`). Both are written with plain loads and stores
//! by the slow paths of the CPU the thread is on when it reads it: hints, as the refresh count
//! and the stamps are. A race (a thread preempted between a load and its store, or one that
//! moved to another CPU) loses or misplaces a credit, a mark or a member of the set, which moves
//! a magazine to the depot or keeps it one exchange longer, or leaves a class's stock out of the
//! room check until the class's next sweep. Each decision reads the CPU once and acts on that
//! CPU's words and stacks; a move between the read and the routine that acts only makes the
//! decision stale. Zero is no credit, no mark and no stock, so the all-zero slab has no stock;
//! a CPU that never sends a magazine to the depot never writes either word, and one that only
//! sends writes its class's word once. Reading them costs no line that the spare limit's check
//! and the heartbeat do not already touch.

use super::magazine::Magazine;
use super::spare::fixed_limit;
use super::{Cache, Source, Tuning};
use crate::classes::SmallClass;
use crate::front::{Cpu, Field, Routines, Spare};
use crate::os::Os;
use core::ptr;
use core::sync::atomic::Ordering::{Acquire, Relaxed};

/// The objects, in bytes, of every class's fixed number of full spares: what the fixed limits
/// let one CPU park in full spares: 139 KiB with the allocator's limits, since a budget of 1 MiB
/// split evenly over the classes leaves most classes' magazines too big for any spare, and the
/// smallest classes four.
pub const fn parked_full(tuning: Tuning) -> usize {
    let mut total = 0usize;
    let mut ordinal = 0;
    while let Some(class) = SmallClass::from_ordinal(ordinal) {
        let limit = super::spare_limit(tuning, class, Spare::Full);
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

/// One class's stock word. Per kind of spare: the credit (bits 0 to 13 full, 14 to 27 empty);
/// the lowest depth its stack was popped to this interval, plus one (bits 28 to 41 full, 42 to
/// 55 empty; zero when no pop was seen); and the marks of a magazine sent to the depot and not
/// yet matched by one taken back (bits 56 and 57 full: sent, and its push contended; 58 and 59
/// empty). Bit 60 says the sweep has settled the class's credit in its current visit and is
/// returning what is above the new limit. All zero is no stock and no mark.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(super) struct Stock(pub(super) u64);

const FIELD: u32 = 14;
const MAX: u64 = (1 << FIELD) - 1;
const LOW: u32 = 2 * FIELD;
const SETTLING: u64 = 1 << 60;

const fn shift(kind: Spare) -> u32 {
    match kind {
        Spare::Full => 0,
        Spare::Empty => FIELD,
    }
}

/// The bits of `kind`'s marks: sent, and contended.
const fn marks(kind: Spare) -> (u64, u64) {
    match kind {
        Spare::Full => (1 << 56, 1 << 57),
        Spare::Empty => (1 << 58, 1 << 59),
    }
}

impl Stock {
    /// The extra spares of `kind` the class may hold.
    pub(super) fn credit(self, kind: Spare) -> u32 {
        (self.0 >> shift(kind) & MAX) as u32
    }

    /// The lowest depth a pop left the stack at this interval, plus one; zero for none.
    fn low(self, kind: Spare) -> u32 {
        (self.0 >> (LOW + shift(kind)) & MAX) as u32
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
        self.with(LOW + shift(kind), mark)
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
            .with(LOW + shift(kind), 0);
        (word, unused)
    }

    fn settling(self) -> bool {
        self.0 & SETTLING != 0
    }

    fn with_settling(self, on: bool) -> Stock {
        Stock(if on {
            self.0 | SETTLING
        } else {
            self.0 & !SETTLING
        })
    }

    /// Whether the class has credit or a visit of the sweep under way: whether the sweep must
    /// treat it as stocked.
    fn stocked(self) -> bool {
        self.credit(Spare::Full) != 0 || self.credit(Spare::Empty) != 0 || self.settling()
    }
}

/// What the sweep's step for a class did with its stock.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Swept {
    /// The class keeps no stock on this CPU: sweep its fixed spares as always.
    Plain,
    /// The class has stock. Whether a full magazine was laundered, and whether the sweep is to
    /// stay on the class for more.
    Stocked { laundered: bool, more: bool },
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
    /// the round trip is complete, and it earns the class one more credit of that kind if
    /// either trip met contention, up to as many magazines as the room holds.
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

    /// Notes a pop of a magazine of `kind` from the current CPU's spare stack of `class`, for
    /// the low-water mark of a class with credit.
    pub(super) fn note_spare_pop(&self, class: SmallClass, kind: Spare, magazine: &Magazine<'_>) {
        if !self.tuning.stock {
            return;
        }
        let Some(cpu) = self.routines.cpu() else {
            return;
        };
        let stock = self.stock(cpu, class);
        if stock.credit(kind) == 0 {
            return;
        }
        let marked = stock.popped(kind, magazine.record().depth);
        if marked != stock {
            self.set_stock(cpu, class, marked);
        }
    }

    /// Whether the current CPU's spare stack of `kind` for `class`, `over` magazines past its
    /// fixed limit, may take one more: if the class has that much credit and the CPU's stock of
    /// that kind stays within the room with it.
    pub(super) fn stock_admits(&self, class: SmallClass, kind: Spare, over: u32) -> bool {
        if !self.tuning.stock {
            return false;
        }
        let Some(cpu) = self.routines.cpu() else {
            return false;
        };
        let stock = self.stock(cpu, class);
        let credit = match kind {
            Spare::Full => stock.credit(Spare::Full),
            Spare::Empty => stock.credit(Spare::Empty).min(stock.credit(Spare::Full)),
        };
        if over >= credit {
            return false;
        }
        self.mark_stocked(cpu, class, true);
        self.stock_held(cpu, kind).saturating_add(weight(class)) <= room(self.tuning)
    }

    /// Sets or clears `class`'s bit in `cpu`'s set of classes that may keep stock, writing only
    /// if it changes.
    fn mark_stocked(&self, cpu: Cpu, class: SmallClass, on: bool) {
        let set = self.slab(cpu).stocked();
        let bit = 1u64 << (class.ordinal() % 64);
        let now = set.load(Relaxed);
        let next = if on { now | bit } else { now & !bit };
        if next != now {
            set.store(next, Relaxed);
        }
    }

    /// What `cpu`'s stacks of `kind` hold past their fixed limits, in the objects full
    /// magazines of their classes hold: for each class in the CPU's set of classes that may
    /// keep stock, how far its stack's head lies past the limit, from the depth the head's
    /// record gives. Read without protection, as the fixed limit's check is. A class is put in
    /// the set by the check that admits its first magazine past the limit, before the push, and
    /// the sweep puts it back if a race took it out while the class still has credit, and takes
    /// it out once the class has none: so a class missing from the set while it holds stock is
    /// missing for at most an interval.
    pub(crate) fn stock_held(&self, cpu: Cpu, kind: Spare) -> usize {
        let slab = self.slab(cpu);
        let mut held = 0usize;
        let mut set = slab.stocked().load(Relaxed);
        while set != 0 {
            let ordinal = set.trailing_zeros() as usize;
            set &= set - 1;
            let Some(class) = SmallClass::from_ordinal(ordinal) else {
                continue;
            };
            let over = self.depth_of(cpu, class, kind).saturating_sub(fixed_limit(
                self.tuning,
                class,
                kind,
            ));
            held = held.saturating_add((over as usize).saturating_mul(weight(class)));
        }
        held
    }

    /// How many magazines `cpu`'s spare stack of `kind` for `class` holds, read without
    /// protection; zero for a stack being drained.
    fn depth_of(&self, cpu: Cpu, class: SmallClass, kind: Spare) -> u32 {
        let head = self
            .slab(cpu)
            .field(Field::spare(class, kind))
            .load(Acquire)
            .addr();
        if head == 0 || crate::classes::is_marker(head) {
            return 0;
        }
        match self.area.magazine(head, class.magazine()) {
            Some(top) => top.record().depth.saturating_add(1),
            None => 0,
        }
    }

    /// The sweep's step for `class` on `cpu`, the CPU its slow path ran on, once an interval
    /// (see the module documentation). For a class with stock: on the first heartbeat of its
    /// visit, takes the unused depth off each credit; then on this and the following heartbeats
    /// returns one magazine a heartbeat from above the new limit, a full one to the source
    /// first, then empty ones to the depot, and asks the sweep to stay until none is left. A
    /// thread no longer on `cpu` does nothing for its stock this visit, so that it never
    /// settles one CPU's credit and returns another's magazines.
    pub(super) fn sweep_stock(&self, cpu: Cpu, class: SmallClass) -> Swept {
        if !self.tuning.stock {
            return Swept::Plain;
        }
        let stock = self.stock(cpu, class);
        if !stock.stocked() {
            return Swept::Plain;
        }
        if self.routines.cpu() != Some(cpu) {
            // The CPU's own next heartbeat carries on.
            return Swept::Stocked {
                laundered: false,
                more: true,
            };
        }
        let full_limit = fixed_limit(self.tuning, class, Spare::Full);
        let empty_limit = fixed_limit(self.tuning, class, Spare::Empty);
        let stock = if stock.settling() {
            stock
        } else {
            let (settled, _) = stock.settled(Spare::Full, full_limit);
            let (settled, _) = settled.settled(Spare::Empty, empty_limit);
            settled.with_settling(true)
        };
        self.set_stock(cpu, class, stock);
        let full_keep = full_limit.saturating_add(stock.credit(Spare::Full));
        let empty_keep =
            empty_limit.saturating_add(stock.credit(Spare::Empty).min(stock.credit(Spare::Full)));
        let full_over = self
            .depth_of(cpu, class, Spare::Full)
            .saturating_sub(full_keep);
        let empty_over = self
            .depth_of(cpu, class, Spare::Empty)
            .saturating_sub(empty_keep);
        let mut stale = false;
        let mut laundered = false;
        let returned = if full_over > 0 {
            match self.spare_pop(class, Spare::Full, &mut stale) {
                Some(magazine) => {
                    // Drop the spare depth the record carries.
                    magazine.set_level(magazine.level());
                    self.launder_held(class, &magazine);
                    self.to_depot(class, magazine);
                    laundered = true;
                    1
                }
                None => 0,
            }
        } else if empty_over > 0 {
            match self.spare_pop(class, Spare::Empty, &mut stale) {
                Some(magazine) => {
                    magazine.set_level(0);
                    self.to_depot(class, magazine);
                    1
                }
                None => 0,
            }
        } else {
            0
        };
        let more = returned > 0 && full_over + empty_over > 1;
        // The visit's own pops are not the class's use, and a finished visit starts the next
        // interval's marks clear.
        let clear = Stock(stock.0 & !(MAX << LOW) & !(MAX << (LOW + FIELD))).with_settling(more);
        self.set_stock(cpu, class, clear);
        self.mark_stocked(cpu, class, clear.stocked() || more);
        if !more && returned == 0 && !clear.stocked() {
            // Nothing left of the stock: the fixed spares are swept as any class's.
            return Swept::Plain;
        }
        Swept::Stocked { laundered, more }
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
            .popped(Spare::Empty, u32::MAX)
            .sent(Spare::Empty, true)
            .with_settling(true);
        assert_eq!(stock.credit(Spare::Empty), 1);
        assert_eq!(stock.credit(Spare::Full), 0);
        assert_eq!(stock.low(Spare::Empty), MAX as u32);
        assert_eq!(stock.low(Spare::Full), 0);
        assert!(stock.settling());
        assert!(!stock.returned(Spare::Full, true).1);
        assert!(stock.returned(Spare::Empty, false).1);
    }

    #[test]
    fn the_bounds_are_what_the_fixed_limits_park() {
        let tuning = Tuning::DEFAULT;
        let full: usize = SmallClass::all()
            .map(|c| {
                super::super::spare_limit(tuning, c, Spare::Full)
                    * c.magazine().capacity()
                    * c.size()
            })
            .sum();
        assert_eq!(parked_full(tuning), full);
        assert_eq!(room(tuning) + full, super::super::SPARE_BUDGET);
        // The credit a class may earn is what the room holds of it, and fits the field.
        let smallest = SmallClass::from_ordinal(0).unwrap();
        assert!(room(tuning) / weight(smallest) < MAX as usize);
    }
}
