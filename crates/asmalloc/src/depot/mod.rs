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
//! of maintenance, the heartbeat (`heartbeat.rs`): a magazine laundered back to the source when
//! the class's depot stack is over its target, one purge step, and one look for an idle cache,
//! which it drains (`drain.rs`) under a claim that carries the process generation
//! (`generation.rs`).
//!
//! Objects parked in magazines pin the pages they lie on, so three rules keep them moving back
//! to the source, where fills take the lowest free addresses: a magazine loaded for an
//! interval is taken back from its slab (`age.rs`); a depot stack sheds the depth it did not
//! need during an interval (`shed.rs`); and one retiring full magazine in [`REFRESH_EVERY`] is
//! laundered instead of kept (`slow.rs`). A heartbeat launders at most one magazine for each
//! rule. [`Cache::collect`] does all of it at once, without waiting for the clock
//! (`collect.rs`), and [`Cache::maintain`] does an interval's worth of it for every slab and
//! stack at once, for a thread that keeps the clocked rules going when no slow path runs
//! (`maintain.rs`).
//!
//! The protocols follow the TLA+ models in `tla/`: `Exchange.tla` for the slow path,
//! `Depot.tla` for the stacks, `Drain.tla` (protocol "gen") for the drain and `Fork.tla` for
//! what a forked child inherits. `interleave.rs` checks the Rust against the models' properties
//! under every schedule within small bounds.

mod age;
mod collect;
mod drain;
mod generation;
mod heartbeat;
mod inspect;
#[cfg(test)]
mod interleave;
mod magazine;
mod maintain;
#[cfg(test)]
mod rules;
mod shed;
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
pub use heartbeat::{IDLE_TICKS, LOOK_EVERY};
pub use inspect::inspect;
pub(crate) use magazine::{Area, Magazine, UNIT};
pub use slow::REFRESH_EVERY;
pub use source::Source;
pub use spare::{MAX_SPARES, SPARE_BUDGET, spare_limit};
pub(crate) use stack::Stack;

use crate::classes::{MagazineSize, SMALL_COUNT, SmallClass};
use crate::front::sync::{AtomicU64, AtomicUsize};
use crate::front::{Cpu, Routines, Slab};
use crate::os::Os;
use crate::space::Space;

/// The bytes of objects a class's depot stack holds before the heartbeat launders magazines
/// back to the source, by default. Two magazine budgets, scaled with the budget when the
/// budget sweep chose it (`docs/research/memory.md`, "Magazine budget").
pub const DEPOT_TARGET: usize = 128 << 10;

/// The limits on memory parked in caches, how often the heartbeat looks around, and which of
/// the rules that return parked objects are on. Constants in the allocator; tests change them
/// to reach every path with small magazines and few slow paths, and to show what each rule
/// does by turning it off.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Tuning {
    /// Bytes of objects one CPU may keep in spare magazines, over all classes.
    pub spare_budget: usize,
    /// The most magazines one spare stack holds.
    pub max_spares: usize,
    /// Bytes of objects a class's depot stack holds before the heartbeat launders.
    pub depot_target: usize,
    /// How many of its own slow paths a CPU makes between looks for an idle slab, and between
    /// looks at a depot stack that no slow path is serving.
    pub look_every: u64,
    /// One retiring full magazine in this many is laundered (`slow.rs`); zero for none.
    pub refresh_every: u64,
    /// Whether a slow path takes back a magazine its CPU has had loaded for an interval
    /// (`age.rs`).
    pub age_limit: bool,
    /// Whether depot stacks shed the depth they did not need during an interval (`shed.rs`).
    pub shed_unused: bool,
}

impl Tuning {
    /// The allocator's limits.
    pub const DEFAULT: Tuning = Tuning {
        spare_budget: SPARE_BUDGET,
        max_spares: MAX_SPARES,
        depot_target: DEPOT_TARGET,
        look_every: LOOK_EVERY,
        refresh_every: REFRESH_EVERY,
        age_limit: true,
        shed_unused: true,
    };
}

/// The depot and the rest of the middle tier's shared state. One per heap, static in the
/// allocator; the all-zero value is valid: empty stacks, no magazines made, nothing prepared.
pub struct Depot {
    /// Per small class, by ordinal: magazines with objects in them.
    full: [Stack; SMALL_COUNT],
    /// Per magazine size, by ordinal: empty magazines.
    empty: [Stack; MagazineSize::COUNT],
    /// Bytes of the magazine area handed out after its first unit.
    carved: AtomicUsize,
    /// Bytes of the magazine area known to be writable, from its start.
    committed: AtomicUsize,
    /// What start-up has done (`generation.rs`).
    status: AtomicU64,
    /// One more than the largest CPU number a slow path has run on.
    cpus: AtomicUsize,
    /// Per small class, by ordinal: how low its stack has been this interval, and what the
    /// last interval left it owing (`shed.rs`).
    water: [AtomicU64; SMALL_COUNT],
    /// The classes whose stacks owe magazines to the source, one bit per ordinal (`shed.rs`).
    owing: AtomicU64,
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
            cpus: AtomicUsize::new(0),
            water: [const { AtomicU64::new(0) }; SMALL_COUNT],
            owing: AtomicU64::new(0),
            #[cfg(test)]
            created: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// The stack of magazines with objects of `class`.
    pub(crate) fn full(&self, class: SmallClass) -> &Stack {
        &self.full[class.ordinal() % SMALL_COUNT]
    }

    /// The stack of empty magazines of `size`.
    pub(crate) fn empty(&self, size: MagazineSize) -> &Stack {
        &self.empty[size.ordinal() % MagazineSize::COUNT]
    }
}

/// The caches and the depot as one thread sees them: the slow paths, the heartbeat and the
/// drain, for a heap at `space` whose middle tier is `depot`, writing slabs through `routines`
/// and getting objects from `source`.
///
/// Magazines, the magazine area and the depot stacks are the crate's own: outside the crate a
/// magazine cannot be named, so no safe call can aim a slab or a stack at the wrong memory.
pub struct Cache<'a, O: Os, R: Routines, S: Source> {
    depot: &'a Depot,
    area: Area<'a, O>,
    routines: R,
    source: &'a S,
    tuning: Tuning,
}

impl<'a, O: Os, R: Routines, S: Source> Cache<'a, O, R, S> {
    /// The view of the heap at `space`.
    ///
    /// # Safety
    ///
    /// For as long as any of them is used, `depot` is only ever used with this `space` (its
    /// counters describe that space's magazine area, which it alone carves), and the slab array
    /// of `routines` only with this depot and space (its words name that area's magazines).
    /// The heap's one static depot, slab array and published space meet this by construction.
    pub unsafe fn new(depot: &'a Depot, space: Space<'a, O>, routines: R, source: &'a S) -> Self {
        // SAFETY: CACHE-01. the caller's promise, passed on.
        unsafe { Cache::with_tuning(depot, space, routines, source, Tuning::DEFAULT) }
    }

    /// The view with limits other than the allocator's.
    ///
    /// # Safety
    ///
    /// As for [`Cache::new`].
    pub unsafe fn with_tuning(
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

    /// The magazine area, for tests' audits.
    #[cfg(test)]
    pub(crate) fn area(&self) -> Area<'a, O> {
        self.area
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

    /// Returns the objects of a magazine this thread holds to the source, in one call, and
    /// records it empty. Returns how many there were.
    fn launder_held(&self, class: SmallClass, magazine: &Magazine<'_>) -> usize {
        let level = magazine.level();
        if let Some(objects) = magazine.objects().get(..level) {
            self.source.take_back(class, objects);
        }
        magazine.set_level(0);
        level
    }

    /// Pops one magazine of `class` from its depot stack and launders it; the empty magazine
    /// goes to the empty stack. Returns how many objects it held, or `None` if the stack was
    /// empty.
    fn launder_one(&self, class: SmallClass) -> Option<usize> {
        let magazine = self.depot.full(class).pop(self.area, class.magazine())?;
        let level = self.launder_held(class, &magazine);
        self.to_depot(class, magazine);
        Some(level)
    }
}
