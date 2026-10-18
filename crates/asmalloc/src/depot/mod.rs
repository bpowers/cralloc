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
pub(crate) mod domain;
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

pub use domain::{Domain, MAX_DOMAINS};
pub use drain::Drained;
pub use generation::Generation;
pub use heartbeat::{IDLE_TICKS, LOOK_EVERY, Lone, LoneTurn, lone_count};
pub use inspect::inspect;
pub(crate) use magazine::{Area, Magazine, UNIT};
pub use slow::REFRESH_EVERY;
pub use source::Source;
pub use spare::{MAX_SPARES, SPARE_BUDGET, spare_limit};
pub(crate) use stack::Stack;

use crate::classes::{MagazineSize, SMALL_COUNT, SmallClass};
use crate::front::sync::{AtomicU64, AtomicUsize};
use crate::front::{Cpu, Routines, Slab};
use crate::lined::Lined;
use crate::os::Os;
use crate::space::Space;

/// The bytes of objects a class's depot stack holds before the heartbeat launders magazines
/// back to the source, by default. Two magazine budgets, scaled with the budget when the
/// budget sweep chose it (`docs/research/memory.md`, "Magazine budget").
pub const DEPOT_TARGET: usize = 128 << 10;

/// The bytes of objects another cache domain's stack of a class must hold before a slow path
/// takes from it, by default: half the target, the share of it each of two domains in use
/// holds before its own heartbeats launder to the source anyway, so that a flow from that
/// domain to this one is taken and a balanced domain is left alone. The whole target sent half
/// of the flow of a pipeline split across two L3s through the bitmaps; a quarter let
/// independent pipelines in two L3s take each other's magazines
/// (`docs/research/launder-and-fill.md`, "A depot per cache domain").
pub const TAKE_ACROSS: usize = DEPOT_TARGET / 2;

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
    /// Bytes of objects a class's depot stack holds before the heartbeat launders, shared among
    /// the cache domains whose stacks of the class hold anything (`heartbeat.rs`).
    pub depot_target: usize,
    /// Bytes of objects a class's stack in another cache domain must hold before a slow path
    /// whose own domain's stack is empty takes from it (`slow.rs`).
    pub take_across: usize,
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
        take_across: TAKE_ACROSS,
        look_every: LOOK_EVERY,
        refresh_every: REFRESH_EVERY,
        age_limit: true,
        shed_unused: true,
    };
}

/// The depot and the rest of the middle tier's shared state. One per heap, static in the
/// allocator; the all-zero value is valid: empty stacks, no magazines made, nothing prepared,
/// one cache domain.
///
/// The stacks are per cache domain ([`domain`]): a slow path pushes to and pops from its CPU's
/// domain's, so that magazines, and the objects they name, stay in the L3 that freed them.
/// Untouched domains cost no memory.
///
/// Every group of words is on lines of its own (`crate::lined`): a class's full stack with its
/// water mark, which its pushes, pops and sheds write; each size's empty stack; each domain's
/// set of owing classes; the magazine area's growth; the start-up status and the CPU count,
/// which every slow path reads and almost nothing writes; and the set of domains that owe.
/// Packed together, the stack heads of eight classes shared a line, and two CPUs exchanging
/// magazines of different classes took it from each other on every push and pop.
pub struct Depot {
    /// Per cache domain.
    domains: [DomainDepot; MAX_DOMAINS],
    growth: Lined<Growth>,
    shared: Lined<Shared>,
    /// The domains whose stacks owe magazines to the source, one bit per domain (`shed.rs`).
    owing_domains: Lined<AtomicU64>,
    /// Which CPUs are in which domain.
    topology: domain::Domains,
    /// Magazines made, for audits.
    #[cfg(test)]
    created: std::sync::atomic::AtomicUsize,
}

/// One cache domain's stacks.
struct DomainDepot {
    /// Per small class, by ordinal.
    classes: [Lined<ClassDepot>; SMALL_COUNT],
    /// Per magazine size, by ordinal: empty magazines.
    empty: [Lined<Stack>; MagazineSize::COUNT],
    /// The classes whose stacks owe magazines to the source, one bit per ordinal (`shed.rs`).
    owing: Lined<AtomicU64>,
}

impl DomainDepot {
    const fn new() -> DomainDepot {
        DomainDepot {
            classes: [const {
                Lined::new(ClassDepot {
                    full: Stack::new(),
                    water: AtomicU64::new(0),
                })
            }; SMALL_COUNT],
            empty: [const { Lined::new(Stack::new()) }; MagazineSize::COUNT],
            owing: Lined::new(AtomicU64::new(0)),
        }
    }
}

/// One class's part of a domain's depot.
struct ClassDepot {
    /// Magazines with objects in them.
    full: Stack,
    /// How low the stack has been this interval, and what the last interval left it owing
    /// (`shed.rs`).
    water: AtomicU64,
}

/// The magazine area's growth.
struct Growth {
    /// Bytes of the magazine area handed out after its first unit.
    carved: AtomicUsize,
    /// Bytes of the magazine area known to be writable, from its start.
    committed: AtomicUsize,
}

/// What every slow path reads and almost nothing writes.
struct Shared {
    /// What start-up has done (`generation.rs`).
    status: AtomicU64,
    /// One more than the largest CPU number a slow path has run on.
    cpus: AtomicUsize,
}

impl Depot {
    /// The empty depot.
    #[allow(clippy::new_without_default)]
    pub const fn new() -> Depot {
        Depot {
            domains: [const { DomainDepot::new() }; MAX_DOMAINS],
            growth: Lined::new(Growth {
                carved: AtomicUsize::new(0),
                committed: AtomicUsize::new(0),
            }),
            shared: Lined::new(Shared {
                status: AtomicU64::new(0),
                cpus: AtomicUsize::new(0),
            }),
            owing_domains: Lined::new(AtomicU64::new(0)),
            topology: domain::Domains::new(),
            #[cfg(test)]
            created: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    fn domain(&self, domain: Domain) -> &DomainDepot {
        &self.domains[domain.index()]
    }

    /// The stack of magazines with objects of `class` in `domain`.
    pub(crate) fn full(&self, domain: Domain, class: SmallClass) -> &Stack {
        &self.domain(domain).classes[class.ordinal() % SMALL_COUNT].full
    }

    /// The water mark of the class of ordinal `ordinal` in `domain` (`shed.rs`).
    pub(crate) fn water(&self, domain: Domain, ordinal: usize) -> &AtomicU64 {
        &self.domain(domain).classes[ordinal % SMALL_COUNT].water
    }

    /// The set of classes whose stacks in `domain` owe (`shed.rs`).
    pub(crate) fn owing(&self, domain: Domain) -> &AtomicU64 {
        &self.domain(domain).owing
    }

    /// The stack of empty magazines of `size` in `domain`.
    pub(crate) fn empty(&self, domain: Domain, size: MagazineSize) -> &Stack {
        &self.domain(domain).empty[size.ordinal() % MagazineSize::COUNT]
    }

    /// How many cache domains the stacks are kept for: 1 until start-up has found more.
    pub(crate) fn domain_count(&self) -> usize {
        self.topology.count()
    }

    /// Every domain in use.
    pub(crate) fn all_domains(&self) -> impl Iterator<Item = Domain> {
        (0..self.domain_count()).map(Domain::folded)
    }

    /// The domain of CPU `cpu`; domain 0 for none.
    pub(crate) fn domain_of(&self, cpu: Option<Cpu>) -> Domain {
        self.topology.of(cpu.map(Cpu::index))
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

    /// The cache domain of the CPU the thread is on now.
    fn here(&self) -> Domain {
        self.depot.domain_of(self.routines.cpu())
    }

    /// Sends a magazine this thread holds to the depot of the domain the thread is in: to its
    /// class's stack if it has objects, else to its size's empty stack.
    fn to_depot(&self, class: SmallClass, magazine: Magazine<'_>) {
        self.to_domain(self.here(), class, magazine);
    }

    /// Sends a magazine this thread holds to `domain`'s depot, as [`Cache::to_depot`] does.
    fn to_domain(&self, domain: Domain, class: SmallClass, magazine: Magazine<'_>) {
        if magazine.level() > 0 {
            self.depot.full(domain, class).push(self.area, magazine);
        } else {
            self.depot
                .empty(domain, magazine.size())
                .push(self.area, magazine);
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

    /// Pops one magazine of `class` from its stack in `domain` and launders it; the empty
    /// magazine goes to that domain's empty stack. Returns how many objects it held, or `None`
    /// if the stack was empty.
    fn launder_one(&self, domain: Domain, class: SmallClass) -> Option<usize> {
        let magazine = self
            .depot
            .full(domain, class)
            .pop(self.area, class.magazine())?;
        let level = self.launder_held(class, &magazine);
        self.to_domain(domain, class, magazine);
        Some(level)
    }
}
