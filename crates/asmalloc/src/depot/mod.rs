//! The middle tier: the slow paths that exchange magazines, the per-CPU spares, the depot, the
//! heartbeat, and the drain of idle caches.
//!
//! When a routine fails, the slow path replaces the loaded magazine instead of refilling it
//! (`slow.rs`). It gets a magazine from the CPU's spare stacks, else the depot, else by filling
//! an empty one from the [`Source`] (the back end); installs it with the store-if-equal routine
//! if the slab word is still what it read there; and retires the magazine it displaced to the
//! spares or the depot. Each step is one commit, so between steps all shared state is valid and
//! whatever the thread holds is private to it, which is what makes the slow path safe to
//! interrupt with a signal handler that allocates and safe to abandon in a forked child. It
//! never waits: when the slab word is a live marker, or the thread has no valid CPU, it serves
//! the request from the magazine it holds and sends that magazine to the depot.
//!
//! The depot ([`Depot`]) holds, per class, a stack of magazines with objects in them and, per
//! magazine size, a stack of empty ones (`stack.rs`). Each slow path then does a bounded amount
//! of maintenance, the heartbeat (`heartbeat.rs`): at most one magazine laundered back to the
//! source when the class's depot stack is over its target, one purge step, and one look for an
//! idle cache, which it drains (`drain.rs`) under a claim that carries the process generation
//! (`generation.rs`).
//!
//! The protocols follow the TLA+ models in `tla/`: `Exchange.tla` for the slow path,
//! `Depot.tla` for the stacks, `Drain.tla` (protocol "gen") for the drain and `Fork.tla` for
//! what a forked child inherits. `interleave.rs` checks the Rust against the models' properties
//! under every schedule within small bounds.

mod drain;
mod generation;
mod heartbeat;
#[cfg(test)]
mod interleave;
mod magazine;
mod slow;
mod source;
mod spare;
mod stack;
#[cfg(test)]
pub(crate) mod test_rig;
#[cfg(test)]
mod tests;
#[cfg(kani)]
mod verify;

pub use drain::Drained;
pub use generation::Generation;
pub use magazine::{Area, Magazine, MagazineId, Record};
pub use source::Source;
pub use spare::{MAX_SPARES, SPARE_BUDGET, spare_limit};
pub use stack::Stack;

use crate::classes::{MagazineSize, SMALL_COUNT, SmallClass};
use crate::front::sync::{AtomicU64, AtomicUsize};
use crate::front::{Cpu, Routines, Slab};
use crate::os::Os;
use crate::space::Space;

/// The bytes of objects a class's depot stack holds before the heartbeat launders magazines
/// back to the source, by default.
pub const DEPOT_TARGET: usize = 256 << 10;

/// The limits on memory parked in caches. Constants in the allocator; tests change them to
/// reach every path with small magazines.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Tuning {
    /// Bytes of objects one CPU may keep in spare magazines, over all classes.
    pub spare_budget: usize,
    /// The most magazines one spare stack holds.
    pub max_spares: usize,
    /// Bytes of objects a class's depot stack holds before the heartbeat launders.
    pub depot_target: usize,
}

impl Tuning {
    /// The allocator's limits.
    pub const DEFAULT: Tuning = Tuning {
        spare_budget: SPARE_BUDGET,
        max_spares: MAX_SPARES,
        depot_target: DEPOT_TARGET,
    };
}

/// The depot and the rest of the middle tier's shared state. One per heap, static in the
/// allocator; the all-zero value is valid: empty stacks, no magazines made, nothing prepared.
pub struct Depot {
    /// Per small class, by ordinal: magazines with objects in them.
    full: [Stack; SMALL_COUNT],
    /// Per magazine size, by ordinal: empty magazines.
    empty: [Stack; MagazineSize::COUNT],
    /// Bytes of the magazine area handed out after the generation page.
    carved: AtomicUsize,
    /// Bytes of the magazine area known to be writable, from its start.
    committed: AtomicUsize,
    /// What start-up has done (`generation.rs`).
    status: AtomicU64,
    /// The next CPU the idle check looks at.
    cursor: AtomicUsize,
    /// One more than the largest CPU number a slow path has run on.
    cpus: AtomicUsize,
    /// Magazines made, for audits.
    #[cfg(test)]
    created: std::sync::atomic::AtomicUsize,
}

impl Depot {
    /// The empty depot.
    #[allow(clippy::new_without_default)]
    pub const fn new() -> Depot {
        Depot {
            full: [const { Stack::new() }; SMALL_COUNT],
            empty: [const { Stack::new() }; MagazineSize::COUNT],
            carved: AtomicUsize::new(0),
            committed: AtomicUsize::new(0),
            status: AtomicU64::new(0),
            cursor: AtomicUsize::new(0),
            cpus: AtomicUsize::new(0),
            #[cfg(test)]
            created: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// The stack of magazines with objects of `class`.
    pub fn full(&self, class: SmallClass) -> &Stack {
        &self.full[class.ordinal() % SMALL_COUNT]
    }

    /// The stack of empty magazines of `size`.
    pub fn empty(&self, size: MagazineSize) -> &Stack {
        &self.empty[size.ordinal() % MagazineSize::COUNT]
    }
}

/// The caches and the depot as one thread sees them: the slow paths, the heartbeat and the
/// drain, for a heap at `space` whose middle tier is `depot`, writing slabs through `routines`
/// and getting objects from `source`.
pub struct Cache<'a, O: Os, R: Routines, S: Source> {
    depot: &'a Depot,
    area: Area<'a, O>,
    routines: R,
    source: &'a S,
    tuning: Tuning,
}

impl<'a, O: Os, R: Routines, S: Source> Cache<'a, O, R, S> {
    /// The view of the heap at `space`.
    pub fn new(depot: &'a Depot, space: Space<'a, O>, routines: R, source: &'a S) -> Self {
        Cache::with_tuning(depot, space, routines, source, Tuning::DEFAULT)
    }

    /// The view with limits other than the allocator's.
    pub fn with_tuning(
        depot: &'a Depot,
        space: Space<'a, O>,
        routines: R,
        source: &'a S,
        tuning: Tuning,
    ) -> Self {
        Cache {
            depot,
            area: Area::new(space, depot),
            routines,
            source,
            tuning,
        }
    }

    /// The routines.
    pub fn routines(&self) -> &R {
        &self.routines
    }

    /// The magazine area.
    pub fn area(&self) -> Area<'a, O> {
        self.area
    }

    /// The depot.
    pub fn depot(&self) -> &'a Depot {
        self.depot
    }

    fn slab(&self, cpu: Cpu) -> &Slab {
        self.routines.slabs().slab(cpu)
    }

    fn os(&self) -> &'a O {
        self.area.space().os()
    }

    /// Sends a magazine this thread holds to the depot: to its class's stack if it has
    /// objects, else to its size's empty stack.
    fn to_depot(&self, class: SmallClass, magazine: Magazine<'_>) {
        if magazine.level() > 0 {
            self.depot.full(class).push(self.area, magazine);
        } else {
            self.depot.empty(magazine.size()).push(self.area, magazine);
        }
    }
}
