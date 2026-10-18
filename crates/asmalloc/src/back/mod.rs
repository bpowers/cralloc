//! The back end: class regions, their free bitmaps, and memory to and from the OS.
//!
//! Every class has regions of its own, each holding slots of that class at
//! `region_base + index * size`. A region's metadata block (see `layout`) holds a leaf bitmap
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
//!   one word per slot out of band; [`Back::resize`] updates it and [`Back::size`] reads it.
//! - [`Back::alloc_run`] and `Back::free_run` serve objects bigger than a region as runs of
//!   whole regions. [`Back::lookup`] and [`Back::usable_size`] find what an address is.
//! - [`Back::heartbeat`] does one bounded step of the clocked pass that returns free memory to
//!   the OS, and [`Back::trim`] returns everything it can. Fills and large allocations that
//!   carve purge toward the retained cap (`purge`); a free purges nothing but a
//!   large slot advised for huge pages.
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

use self::host::{ClassHost, ClassState};
use crate::classes::{COUNT, Class};
use crate::lined::{LINE, Lined};
use crate::os::{Os, OsError};
use crate::space::{MAX_REGIONS, RegionTable, Space, SpaceCell};
use core::fmt;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicIsize, AtomicU32, AtomicU64, AtomicUsize};

/// The back end's shared state, and the reservation it manages.
///
/// Meant to live in a static: [`State::new`] is `const`, and apart from the OS value the state
/// starts as all zero. The state owns the [`SpaceCell`] that publishes the heap's reservation, so
/// it can only ever describe that one reservation.
///
/// Every group of words is on lines of its own (`crate::lined`). The reservation and the
/// region table are read on every operation and written rarely; the retained count is written
/// by every free and claim; the decay word is read by every heartbeat; the walk's words are
/// written by its steps.
pub struct State<O: Os> {
    cell: Lined<SpaceCell<O>>,
    table: Lined<RegionTable>,
    classes: [ClassState; COUNT],
    /// Bytes of free slots in the bitmaps believed to be resident. Signed: a subtraction may
    /// run ahead of the addition it answers; read as zero when negative.
    retained: Lined<AtomicIsize>,
    /// Bytes carved since a grower last checked the cap (`purge`).
    growth: Lined<AtomicUsize>,
    /// The clocked pass: when the last one ended, or when the one under way began with the
    /// low bit set.
    decay: Lined<AtomicU64>,
    /// The purge walk's position, its laps and the pass under way, which its steps write.
    walk: Lined<Walk>,
    /// The current size of each run, at its head region's index. Out of the regions'
    /// metadata, which must stay all zero for a region that may later go to a class.
    run_sizes: Lined<[AtomicUsize; MAX_REGIONS]>,
}

/// What the steps of the purge walk share (`purge`).
struct Walk {
    /// Where the next purge step looks.
    purge_at: AtomicU32,
    /// How many times the purge walk has gone round the heap.
    laps: AtomicU32,
    /// The pass under way: its tag, the lap it began in and what it may still purge
    /// (`purge::Pass`).
    pass: AtomicU64,
}

#[cfg(all(target_os = "linux", target_arch = "x86_64", not(kani)))]
const _: () = {
    use crate::lined::lines;
    use core::mem::size_of;
    let groups = lines(size_of::<SpaceCell<crate::os::linux::LinuxOs>>())
        + lines(size_of::<RegionTable>())
        + 4
        + lines(size_of::<[AtomicUsize; MAX_REGIONS]>());
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
            retained: Lined::new(AtomicIsize::new(0)),
            growth: Lined::new(AtomicUsize::new(0)),
            decay: Lined::new(AtomicU64::new(0)),
            walk: Lined::new(Walk {
                purge_at: AtomicU32::new(0),
                laps: AtomicU32::new(0),
                pass: AtomicU64::new(0),
            }),
            run_sizes: Lined::new([const { AtomicUsize::new(0) }; MAX_REGIONS]),
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
        Some(ClassHost {
            space: self.space,
            class: state,
            layout,
        })
    }

    fn abort(&self, message: &str) -> ! {
        self.space.os().abort(message)
    }
}
