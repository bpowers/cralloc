//! The back end: class regions, their free bitmaps, and memory to and from the OS.
//!
//! Every class has regions of its own, each holding slots of that class at
//! `region_base + origin + index * size`, the origin a few pages for a small class
//! (`Plan::slot_origin`) and zero for a large one. A region's metadata block (see `layout`) holds a leaf bitmap
//! with one bit per slot, set when the slot is free, and above it summary bits up to one word
//! per region; above the regions, two class-level words lead to every region that may have free
//! slots (`tree`). A watermark per region separates slots that have been handed out at least
//! once from never-used ones, whose memory reads as zero.
//!
//! What the back end offers ([`Back`]):
//!
//! - [`Back::fill`] writes up to `n` free pointers of a small class into a caller's slice,
//!   reusing freed slots before carving new ones and lowest addresses first; [`Back::launder`]
//!   returns a slice of them, with one atomic OR per bitmap word they touch. These are what
//!   the depot calls to fill and empty magazines.
//! - [`Back::alloc`] and `Back::free` serve one slot of any class straight from the bitmaps:
//!   always for large classes, and for small ones when a thread has no per-CPU cache. A large
//!   allocation says whether the slot is known to read as zero, and records the object's size,
//!   one word per slot out of band; [`Back::resize_in_place`] updates it and [`Back::size`] reads it.
//! - [`Back::alloc_run`] and `Back::free_run` serve objects bigger than a region as runs of
//!   whole regions. [`Back::lookup`] and [`Back::usable_size`] find what an address is.
//! - [`Back::heartbeat`] does one bounded step of the clocked pass that returns free memory to
//!   the OS, and [`Back::trim`] returns everything it can. Fills and large allocations that
//!   carve purge toward the retained cap (`purge`); a free purges nothing but a
//!   large slot advised for huge pages, and that only past the budget for kept grown blocks
//!   (`policy::KEPT_GROWN_HUGE_PAGES`).
//!
//! Every change to shared state is one atomic instruction, and all-zero state means an empty
//! heap: no regions, nothing free, nothing carved. The protocols follow the TLA+ models in
//! `tla/`: `Summary.tla` (with the healing free) for the summary bits and `Purge.tla` for
//! purging; the module docs of `tree` and `region` give them step by step.
//!
//! Policy values (the retained cap, purge budgets, the huge-page threshold, commit steps) are
//! in `policy`.

mod class;

pub use class::{Descending, Sink, SlotPointer};
mod group;
mod hold;
mod host;
mod inspect;
mod layout;
pub mod policy;
mod purge;
mod region;
mod run;
mod tree;
mod word;

#[cfg(test)]
mod explore;
#[cfg(test)]
mod models;
#[cfg(test)]
mod testhost;
#[cfg(test)]
mod tests;
#[cfg(kani)]
mod verify;
#[cfg(kani)]
mod verify_slow;

use self::host::{ClassHost, ClassState, Frontiers, Headers, RegionHeader};
use crate::classes::{COUNT, Class};
use crate::lined::{LINE, Lined};
use crate::os::{Os, OsError};
use crate::space::{MAX_REGIONS, RegionTable, Space, SpaceCell};
use core::fmt;
use core::ptr::NonNull;
use core::sync::atomic::Ordering::SeqCst;
use core::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize};

/// The back end's shared state, and the reservation it manages.
///
/// Meant to live in a static: [`State::new`] is `const`, and apart from the OS value the state
/// starts as all zero. The state owns the [`SpaceCell`] that publishes the heap's reservation, so
/// it can only ever describe that one reservation.
///
/// Every group of words is on lines of its own (`crate::lined`). The reservation and the
/// region table are read on every operation and written rarely; the retained count's two
/// tallies are written, one by frees and the other by claims; the decay word is read by every
/// heartbeat; the walk's words are written by its steps. The regions' headers and frontiers
/// share lines among regions (`host::Headers`, `host::Frontiers`).
pub struct State<O: Os> {
    cell: Lined<SpaceCell<O>>,
    table: Lined<RegionTable>,
    classes: [ClassState; COUNT],
    /// Bytes of free slots in the bitmaps believed to be resident (`purge`).
    retained: Retained,
    /// Bytes carved since a grower last checked the cap (`purge`).
    growth: Lined<AtomicUsize>,
    /// The clocked pass's word and the clock's interval number (`Clocked`).
    decay: Lined<Clocked>,
    /// The purge walk's position, its laps and the pass under way, which its steps write.
    walk: Lined<Walk>,
    /// The current size of each run, at its head region's index. Out of the regions'
    /// metadata, which must stay all zero for a region that may later go to a class.
    run_sizes: Lined<[AtomicUsize; MAX_REGIONS]>,
    /// Each region's header (`host::RegionHeader`). Out of the regions' metadata
    /// blocks, so that a small class whose slots never reach the bitmaps never touches its block.
    headers: Lined<Headers>,
    /// Each region's frontier, for small classes (`host::Frontiers`).
    frontiers: Lined<Frontiers>,
    /// The hold on free large slots: its fine count and when the next hold ends (`hold`).
    hold: Lined<hold::Hold>,
}

/// Bytes of free slots in the bitmaps believed to be resident, for small classes and large
/// ones: what was counted in, less what was counted out. Signed: a subtraction may run ahead of
/// the addition it answers; read as zero when negative. The decay and the cap act on the sum;
/// the large count alone decides whether a grown block is kept (`policy::kept_grown`).
///
/// What is counted in and what is counted out are two tallies that only grow (and wrap), each
/// on a line of its own, not one count. Frees and launders count in and claims and fills count
/// out, so in a program that allocates on one thread and frees on another each thread writes
/// a line the other only reads; with one count, every large allocation and free took the line
/// from the other thread (`docs/research/memory.md`, "The cost of a large block, by cause").
struct Retained {
    added: Lined<Tally>,
    taken: Lined<Tally>,
}

/// One tally of [`Retained`], for each kind of class.
struct Tally {
    small: AtomicUsize,
    large: AtomicUsize,
}

impl Tally {
    const fn new() -> Tally {
        Tally {
            small: AtomicUsize::new(0),
            large: AtomicUsize::new(0),
        }
    }

    fn of(&self, large: bool) -> &AtomicUsize {
        if large { &self.large } else { &self.small }
    }
}

impl Retained {
    /// The count of large classes if `large`, else of small ones: what was counted in less
    /// what was counted out, which is negative while a subtraction runs ahead. A value the count
    /// had at a moment during the call (`difference`). Every caller is off the fast paths: the
    /// decay pass that is due, the cap charged to growth, the decision to keep a grown block,
    /// and the heap's inspection.
    fn signed(&self, large: bool) -> isize {
        let (taken, added) = (self.taken.of(large), self.added.of(large));
        difference(|| taken.load(SeqCst), || added.load(SeqCst))
    }
}

/// How many times [`difference`] reads the tallies again before it settles for a reading that
/// errs high. Each try is three loads; a try fails only when a claim or fill counted out
/// between its first load and its last.
const EXACT_TRIES: usize = 8;

/// `added - taken` of two tallies that only grow, as a value it really had: the taken tally
/// read on both sides of the added one, again while it moved. When it did not move, no
/// subtraction fell between the loads, so the difference was the count at the moment of the
/// added load (every tally operation is sequentially consistent, in one order). Read once
/// each, in either order, the two loads straddle a window whose additions or subtractions only
/// one of them sees: taken first erred high by every addition of the window, which a reader
/// preempted between the loads saw as tens of MiB of large blocks moved meanwhile.
///
/// If the taken tally moves on every try, the last reading is taken before added, which can
/// only err high: by the additions after that taken load. Every caller is safe that way round:
/// a grown block is purged rather than kept, the decay pass or the growth that pays for the cap
/// purges a little more, and nothing is under-counted.
fn difference(taken: impl Fn() -> usize, added: impl Fn() -> usize) -> isize {
    let mut before = taken();
    for _ in 0..EXACT_TRIES {
        let added = added();
        let after = taken();
        if after == before {
            return added.wrapping_sub(before) as isize;
        }
        before = after;
    }
    added().wrapping_sub(before) as isize
}

/// What the clock decides, on a line that is written a few times an interval and read by every
/// heartbeat and every fill.
struct Clocked {
    /// The clocked pass: when the last one ended, or when the one under way began with the
    /// low bit set.
    word: AtomicU64,
    /// The number of the clock's interval (the time over `policy::DECAY_TICKS`) as a heartbeat
    /// last saw it: what a claim's frontier is recorded under and a pass reads frontiers by
    /// (`layout::Frontier`). Written by the first heartbeat of each interval that reads the
    /// clock (`purge`), so once an interval; zero until the first heartbeat.
    epoch: AtomicU64,
    /// The count of intervals every clocked rule reads its time from (`crate::clock`).
    clock: crate::clock::Counted,
    /// For tests of the clocked pass's mechanics on heaps smaller than the floor: the pass
    /// acts as if the floor were zero.
    #[cfg(test)]
    unfloored: core::sync::atomic::AtomicBool,
}

/// What the steps of the purge walk share (`purge`).
struct Walk {
    /// Where the next purge step looks: the region plus one, zero for the top. Every purger
    /// carries on from it, the clocked pass included, so the walk goes round the heap and no
    /// region waits behind the others.
    purge_at: AtomicU32,
    /// How many times the purge walk has gone round the heap: finished the lowest region.
    laps: AtomicU32,
    /// Where the clocked pass under way began, as `purge_at` was then: the pass has gone once
    /// round when the walk, a lap later, is back there.
    pass_from: AtomicU32,
    /// The pass under way: its tag, the lap it began in and what it may still purge
    /// (`purge::Pass`).
    pass: AtomicU64,
    /// How many clocked passes have started, which picks the ones that take runs of any length
    /// (`policy::PURGE_ANY_RUN_EVERY`).
    passes: AtomicU32,
    /// One past the highest region ever about to be given to a class, zero for none: the walk
    /// goes round regions below it only. Classes take the lowest unassigned regions and runs
    /// come from the top, so in a heap of a hundred class regions a lap reads a hundred table
    /// entries, not every region of the reservation. Raised with `fetch_max` before the region
    /// is tagged, so it is never below a class region; a region given back leaves it high,
    /// which costs a look at an unassigned entry.
    top: AtomicU32,
}

#[cfg(all(target_os = "linux", target_arch = "x86_64", not(kani)))]
const _: () = {
    use crate::lined::lines;
    use core::mem::size_of;
    let groups = lines(size_of::<SpaceCell<crate::os::linux::LinuxOs>>())
        + lines(size_of::<RegionTable>())
        + 5
        + lines(size_of::<[AtomicUsize; MAX_REGIONS]>())
        + lines(size_of::<[[u64; 2]; MAX_REGIONS]>())
        + lines(size_of::<[u64; MAX_REGIONS]>())
        + lines(size_of::<hold::Hold>());
    assert!(
        size_of::<State<crate::os::linux::LinuxOs>>()
            == size_of::<[ClassState; COUNT]>() + LINE * groups
    );
};

impl<O: Os> State<O> {
    /// A back end over `os` with no heap reserved yet.
    pub const fn new(os: O) -> State<O> {
        State {
            cell: Lined::new(SpaceCell::new(os)),
            table: Lined::new(RegionTable::new()),
            classes: [const { ClassState::new() }; COUNT],
            retained: Retained {
                added: Lined::new(Tally::new()),
                taken: Lined::new(Tally::new()),
            },
            growth: Lined::new(AtomicUsize::new(0)),
            decay: Lined::new(Clocked {
                word: AtomicU64::new(0),
                epoch: AtomicU64::new(0),
                clock: crate::clock::Counted::new(),
                #[cfg(test)]
                unfloored: core::sync::atomic::AtomicBool::new(false),
            }),
            walk: Lined::new(Walk {
                purge_at: AtomicU32::new(0),
                laps: AtomicU32::new(0),
                pass_from: AtomicU32::new(0),
                pass: AtomicU64::new(0),
                passes: AtomicU32::new(0),
                top: AtomicU32::new(0),
            }),
            run_sizes: Lined::new([const { AtomicUsize::new(0) }; MAX_REGIONS]),
            headers: Lined::new([const { RegionHeader::new() }; MAX_REGIONS]),
            frontiers: Lined::new([const { word::Atomic::zero() }; MAX_REGIONS]),
            hold: Lined::new(hold::Hold::new()),
        }
    }

    /// The OS.
    #[inline(always)]
    pub fn os(&self) -> &O {
        self.cell.os()
    }

    /// The back end over the heap, if the heap has been reserved.
    #[inline]
    pub fn get(&self) -> Option<Back<'_, O>> {
        self.cell.get().map(|space| Back { state: self, space })
    }

    /// The back end over the heap, reserving the heap first if no thread has (see
    /// [`SpaceCell::get_or_reserve`]).
    pub fn get_or_reserve(&self) -> Result<Back<'_, O>, OsError> {
        self.cell
            .get_or_reserve()
            .map(|space| Back { state: self, space })
    }
}

impl<O: Os> fmt::Debug for State<O> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("State")
            .field("space", &self.cell.get())
            .finish_non_exhaustive()
    }
}

/// The back end bound to its reservation: what the front end and the interfaces call.
pub struct Back<'a, O: Os> {
    state: &'a State<O>,
    space: Space<'a, O>,
}

impl<O: Os> Clone for Back<'_, O> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<O: Os> Copy for Back<'_, O> {}

impl<O: Os> fmt::Debug for Back<'_, O> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Back")
            .field("space", &self.space)
            .finish_non_exhaustive()
    }
}

/// A slot handed out by [`Back::alloc`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Block {
    /// The slot's first byte. The whole slot, the class size, is the caller's.
    pub ptr: NonNull<u8>,
    /// Whether the slot is known to read as zero: never used, or purged since it was last
    /// used. Only large classes track purged slots, so a reused small slot says `false`.
    pub zeroed: bool,
    /// What [`Back::usable_size`] says of the block, worked out here so that the caller need
    /// not look the block up again.
    pub len: usize,
}

/// What a heap address is the start of.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// A slot of this class.
    Slot(Class),
    /// A run of this many whole regions: an object bigger than a region.
    Run(usize),
}

impl<'a, O: Os> Back<'a, O> {
    /// The reservation.
    #[inline(always)]
    pub fn space(&self) -> Space<'a, O> {
        self.space
    }

    /// The class-of-region table.
    #[inline(always)]
    pub fn table(&self) -> &'a RegionTable {
        &self.state.table
    }

    /// The protocols' view of one class, if a slot of it fits in a region.
    #[inline]
    fn host(&self, class: Class) -> Option<ClassHost<'a, O>> {
        let state = self.state.classes.get(class.index() as usize - 1)?;
        let layout = state.layout.layout(self.space.plan(), class)?;
        Some(ClassHost::new(
            self.space,
            state,
            &self.state.headers,
            &self.state.frontiers,
            layout,
        ))
    }

    fn abort(&self, message: &str) -> ! {
        self.space.os().abort(message)
    }
}
