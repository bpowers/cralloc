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
//! alone, splitting it evenly over every class, cannot reach: 32 KiB of 1 MiB with the
//! allocator's limits, since nearly every class's magazine is bigger than its share. Stock lets the
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
//! follow. A class whose stock is in use has its fixed spares swept all the same, once its
//! visit finds nothing above the limit: the sweep takes the stack, puts the stock back on top
//! and sweeps the fixed number under it as it would any class's, so that a class that only ever
//! swaps the top of its stack does not keep the magazines under it parked for good. Objects in circulation through stock are replaced by the lowest free ones as any
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
/// let one CPU park in full spares: 32 KiB with the allocator's limits, since a budget of 1 MiB
/// split evenly over the classes, 23 KiB each, leaves every class's magazine too big for any
/// spare but the 8-byte class's two and the 16-byte class's one.
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
/// limits could park in full spares. 992 KiB with the allocator's limits, worked out at compile
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

/// One class's stock word. Per kind of spare: the credit (bits 0 to 10 full, 11 to 21 empty);
/// the lowest depth its stack was popped to this interval, plus one (bits 22 to 32 full, 33 to
/// 43 empty; zero when no pop was seen); and the marks of a magazine sent to the depot and not
/// yet matched by one taken back (bits 56 and 57 full: sent, and its push contended; 58 and 59
/// empty). Bit 60 says the sweep has settled the class's credit in its current visit and is
/// returning what is above the new limit. Eleven bits hold any credit or depth the room allows
/// (about 124 of the smallest class's magazines), and saturate beyond. Bits 44 to 55 and 61 to
/// 63 are not stock's: they are the class's swing state on the CPU ([`plan_swap`]). All zero is
/// no stock, no mark and no swap.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(super) struct Stock(pub(super) u64);

const FIELD: u32 = 11;
const MAX: u64 = (1 << FIELD) - 1;
const LOW: u32 = 2 * FIELD;
const SETTLING: u64 = 1 << 60;
/// The swing state (see [`plan_swap`]). A swap of the class has been recorded on this CPU.
const SWAPPED: u64 = 1 << 61;
/// The last swap recorded was a free's.
const FREED: u64 = 1 << 62;
/// The last swap recorded reversed the one before it.
const REVERSED: u64 = 1 << 63;
/// The last swap recorded left its magazine half full.
const CENTRED: u64 = 1 << 54;
/// The last swap recorded went the same way as a half-full swap just before it.
const AFTER_CENTRED: u64 = 1 << 55;
/// Half-full swaps in a row that did not absorb a swing.
const FAILS_AT: u32 = 44;
const FAILS: u64 = 0b111 << FAILS_AT;
/// Swaps with evidence of a lock still to be left plain after the failures.
const SKIP_AT: u32 = 47;
const SKIP: u64 = 0x7f << SKIP_AT;
/// The most such swaps left plain in a row: after the seventh failure, one try in 65.
const MAX_SKIP: u64 = 64;
const SWING: u64 = SWAPPED | FREED | REVERSED | CENTRED | AFTER_CENTRED | FAILS | SKIP;
const _: () = assert!(LOW + 2 * FIELD <= FAILS_AT && SKIP_AT + 7 <= 54 && MAX_SKIP <= 0x7f);

/// What a swap of a class is to do, worked out from the class's swing state before the
/// install, and the state to record once the install has happened.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct SwapPlan {
    /// Leave the loaded magazine half full.
    pub(super) centre: bool,
    /// The stock word to record after a plain install, and after a half-full one.
    plain: u64,
    centred: u64,
}

impl SwapPlan {
    /// The same plan with the magazine left at its end.
    pub(super) fn plain(self) -> SwapPlan {
        SwapPlan {
            centre: false,
            ..self
        }
    }
}

/// The swing rule: what a swap that goes `way` does, given the class's stock word `word` on the
/// CPU, and `allowed` whether the rule is on.
///
/// A swap installs a magazine at an end. Work that swings across that end and back, by less
/// than a magazine, then crosses the far end of the new magazine too, every repetition: the
/// swaps alternate, each reversing the one before. So a swap that reverses a swap which itself
/// reversed the one before is evidence of such a lock, and it leaves the magazine half full
/// (`slow.rs`). A reversal that follows a run of swaps the same way is the turn of a swing
/// longer than a magazine, which half a magazine cannot absorb; it stays plain.
///
/// A half-full swap that a reversal follows, at once or after one swap the same way, did not
/// absorb the swing: one between half a magazine and a whole one, say, fits at neither end nor
/// in the middle, and runs out of the half and then turns. After the `j`-th such failure in a
/// row, the next `2^(j - 1)` swaps with evidence of a lock stay plain, at most [`MAX_SKIP`], and
/// then the rule tries again. A half-full swap followed by a run, two swaps or more the same
/// way, or by no swap at all until the sweep's next visit ([`settle_swing`]), absorbed it, and
/// clears the count. Failures count in a row, not in all, because a server's level also
/// wanders, as other threads free what this one made, and one wander that ends at the other end
/// must not stop the rule.
pub(super) fn plan_swap(word: u64, way: Swap, allowed: bool) -> SwapPlan {
    let freed = way == Swap::Free;
    let reversal = word & SWAPPED != 0 && (word & FREED != 0) != freed;
    let lock = reversal && word & REVERSED != 0;
    let mut fails = (word & FAILS) >> FAILS_AT;
    let mut skip = (word & SKIP) >> SKIP_AT;
    let pending = word & (CENTRED | AFTER_CENTRED) != 0;
    if pending && reversal {
        fails = (fails + 1).min(FAILS >> FAILS_AT);
        skip = (1u64 << (fails - 1)).min(MAX_SKIP);
    } else if word & AFTER_CENTRED != 0 {
        fails = 0;
        skip = 0;
    }
    let mut centre = false;
    if lock && allowed {
        if skip > 0 {
            skip -= 1;
        } else {
            centre = true;
        }
    }
    let plain = word & !SWING
        | SWAPPED
        | if freed { FREED } else { 0 }
        | if reversal { REVERSED } else { 0 }
        | if word & CENTRED != 0 && !reversal {
            AFTER_CENTRED
        } else {
            0
        }
        | fails << FAILS_AT
        | skip << SKIP_AT;
    SwapPlan {
        centre,
        plain,
        centred: plain | CENTRED,
    }
}

/// The swing state after the sweep's visit: a half-full swap that no reversal has followed
/// since absorbed what it was for, so the failures are forgotten.
pub(super) fn settle_swing(word: u64) -> u64 {
    if word & (CENTRED | AFTER_CENTRED) == 0 {
        return word;
    }
    word & !(CENTRED | AFTER_CENTRED | FAILS | SKIP)
}

/// Which way a swap went: an allocation's, which installs a magazine with objects, or a
/// free's, which installs an empty one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Swap {
    Alloc,
    Free,
}

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
            laundered = self.return_top_spare(class);
            u32::from(laundered)
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
        if !more && returned == 0 {
            // Stock in use, nothing above the limit: the fixed spares under it are swept as
            // any class's would be, so that the objects parked there are replaced too, and
            // the stock goes back on top.
            let depth = self.depth_of(cpu, class, Spare::Full);
            let keep = depth.saturating_sub(full_limit);
            laundered = self.take_spares_keeping(cpu, class, keep);
        }
        Swept::Stocked { laundered, more }
    }

    /// What a swap of `class` on `cpu` that goes `way` is to do ([`plan_swap`]), from the
    /// class's stock word there. Reads only; [`Cache::record_swap`] writes once the install has
    /// happened, so a failed install neither gains nor loses evidence.
    pub(super) fn plan_swap(&self, cpu: Cpu, class: SmallClass, way: Swap) -> SwapPlan {
        plan_swap(self.stock(cpu, class).0, way, self.tuning.centre_swings)
    }

    /// Records a swap of `class` on `cpu` that installed a magazine as `plan` says it did. A
    /// hint in the stock word, as stock's own marks are: a lost or misplaced update (a thread
    /// preempted between the load and the store, or one that moved to another CPU) only makes
    /// one swap leave its magazine at an end or half full when the other would have been
    /// right. Writes the word only when it changes, so a flow that goes one way writes it once.
    pub(super) fn record_swap(&self, cpu: Cpu, class: SmallClass, plan: SwapPlan) {
        let now = self.stock(cpu, class);
        let next = Stock(if plan.centre {
            plan.centred
        } else {
            plan.plain
        });
        if next != now {
            self.set_stock(cpu, class, next);
        }
    }

    /// Records the swaps that make `cpu`'s next swap of `class` going `next` evidence of a
    /// lock: one going `next`, then one reversing it.
    #[cfg(test)]
    pub(super) fn prime_lock(&self, cpu: Cpu, class: SmallClass, next: Swap) {
        let other = match next {
            Swap::Alloc => Swap::Free,
            Swap::Free => Swap::Alloc,
        };
        for way in [next, other] {
            let plan = plan_swap(self.stock(cpu, class).0, way, false);
            self.record_swap(cpu, class, plan);
        }
        assert!(plan_swap(self.stock(cpu, class).0, next, true).centre);
    }

    /// The sweep's visit to `class` on `cpu`: see [`settle_swing`].
    pub(super) fn settle_swing(&self, cpu: Cpu, class: SmallClass) {
        let now = self.stock(cpu, class);
        let next = Stock(settle_swing(now.0));
        if next != now {
            self.set_stock(cpu, class, next);
        }
    }

    /// How many full spares of `class` `cpu`'s stack may hold: its fixed limit and its credit.
    pub(super) fn full_keep(&self, cpu: Cpu, class: SmallClass) -> u32 {
        let credit = if self.tuning.stock {
            self.stock(cpu, class).credit(Spare::Full)
        } else {
            0
        };
        fixed_limit(self.tuning, class, Spare::Full).saturating_add(credit)
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
    use std::vec;
    use std::vec::Vec;

    /// The swing rule's decisions for a sequence of swaps, each recorded as its install would
    /// record it: whether each one left its magazine half full.
    fn centred(ways: &[Swap]) -> Vec<bool> {
        let mut word = credits(3, 2).addr() as u64;
        let mut out = Vec::new();
        for &way in ways {
            let plan = plan_swap(word, way, true);
            out.push(plan.centre);
            word = if plan.centre {
                plan.centred
            } else {
                plan.plain
            };
        }
        // Stock's own fields are untouched.
        assert_eq!(Stock(word).credit(Spare::Full), 3);
        assert_eq!(Stock(word).credit(Spare::Empty), 2);
        out
    }

    use Swap::{Alloc as A, Free as F};

    #[test]
    fn a_flow_never_centres() {
        assert!(centred(&[F; 50]).iter().all(|&c| !c));
        assert!(centred(&[A; 50]).iter().all(|&c| !c));
    }

    #[test]
    fn a_lock_centres_at_the_reversal_that_follows_a_reversal() {
        // a, f, a: the third swap reverses a reversal.
        assert_eq!(centred(&[A, F, A]), [false, false, true]);
        // A swing longer than a magazine turns after runs, and never centres.
        let long = [A, A, F, F, A, A, F, F, A, A, F, F];
        assert!(centred(&long).iter().all(|&c| !c));
    }

    #[test]
    fn a_centring_that_fails_backs_off_twice_as_long_each_time() {
        // A lock that centring does not absorb: the swaps keep alternating. After the first
        // centred swap fails, one lock swap is plain, then it tries; after the second, two; then
        // four, eight, ..., at most 64.
        let ways: Vec<Swap> = (0..400).map(|i| if i % 2 == 0 { A } else { F }).collect();
        let tries: Vec<usize> = centred(&ways)
            .iter()
            .enumerate()
            .filter(|&(_, &c)| c)
            .map(|(i, _)| i)
            .collect();
        let gaps: Vec<usize> = tries.windows(2).map(|w| w[1] - w[0]).collect();
        assert_eq!(&gaps[..7], [2, 3, 5, 9, 17, 33, 65], "{tries:?}");
        assert!(gaps[7..].iter().all(|&gap| gap == 65), "{gaps:?}");
    }

    #[test]
    fn a_centring_that_runs_out_of_its_half_and_turns_has_failed() {
        // A swing between half a magazine and a whole one: the centred swap runs out of its
        // half, swaps once more the same way, and then the swing turns. Three swaps a
        // repetition where two would do, so it backs off as a reversal at once does.
        let ways: Vec<Swap> = (0..300).flat_map(|_| [A, A, F]).collect();
        let first = [A, F, A];
        let mut all = first.to_vec();
        all.extend(&ways);
        let tries = centred(&all).iter().filter(|&&c| c).count();
        assert!(tries <= 12, "{tries} tries in 300 repetitions");
    }

    #[test]
    fn a_centring_that_absorbs_clears_the_failures() {
        // Centred at the third, fifth and eighth swaps (failing twice), then a run of two
        // swaps the same way as the last centred swap: the failures are cleared, and the next
        // lock (a reversal after a reversal) centres at once.
        let mut ways = vec![A, F, A, F, A, F, A, F];
        let before = centred(&ways);
        assert_eq!(before.iter().filter(|&&c| c).count(), 3);
        assert!(*before.last().unwrap());
        ways.extend([F, F, A, F]);
        let after = centred(&ways);
        assert!(after[ways.len() - 1], "{after:?}");
        // So does the sweep's visit with no swap since the centred one.
        let mut word = 0;
        for way in [A, F, A, F, A] {
            let plan = plan_swap(word, way, true);
            word = if plan.centre {
                plan.centred
            } else {
                plan.plain
            };
        }
        assert!(!plan_swap(word, F, true).centre, "a failure backs off");
        let settled = settle_swing(word);
        assert!(
            plan_swap(settled, F, true).centre,
            "the sweep cleared the failures"
        );
        assert!(
            !plan_swap(settled, F, false).centre,
            "the rule off never centres"
        );
    }

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
