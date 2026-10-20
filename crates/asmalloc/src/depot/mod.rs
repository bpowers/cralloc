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
//! The depot ([`Depot`]) holds, per class, stacks of magazines with objects in them and, per
//! magazine size, stacks of empty ones (`stack.rs`): one of each per cache domain, capped, and
//! one shared (see [`Depot`]). Each slow path then does a bounded amount of maintenance, the
//! heartbeat (`heartbeat.rs`): a magazine laundered back to the source when the class's shared
//! stack is over its target, one purge step, and one look for an idle cache, which it drains
//! (`drain.rs`) under a claim that carries the process generation (`generation.rs`).
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
mod held;
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
mod stock;
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
use crate::lined::{LINE, Lined, lines};
use crate::os::Os;
use crate::space::Space;
use core::cell::Cell;

/// The bytes of objects a class's depot stack holds before the heartbeat launders magazines
/// back to the source, by default. Two magazine budgets, scaled with the budget when the
/// budget sweep chose it (`docs/research/memory.md`, "Magazine budget").
pub const DEPOT_TARGET: usize = 128 << 10;

/// The bytes of objects a cache domain's stack of a class holds before the magazines freed
/// there go to the class's shared stack instead, by default: a quarter of the target, which
/// leaves the shared stack the rest, so that a process on one domain parks what it did when
/// there was one stack per class. Half the target was no faster on any job that crosses L3s,
/// slower on some, and parked up to 0.7 MiB more in them (`docs/research/launder-and-fill.md`,
/// "Domain stacks and a shared stack").
pub const DOMAIN_CAP: usize = DEPOT_TARGET / 4;

/// The most objects one slow path launders: as many as the largest magazine holds, 1,022.
///
/// The rules that launder were each allowed a magazine a slow path, which bounded the work
/// while a magazine held at most 254 objects; at four times that, the four or so magazines a
/// slow path could launder would make one `malloc` or `free` four times as long. So a slow path
/// counts the objects it launders, and a rule that may wait (the refresh, the age limit and the
/// sweep, the shed, and the launder over target of what the heartbeat's own sweep and drain
/// pushed) launders a magazine only if the magazine could be full and still keep the count
/// within this budget, after setting aside the objects the call's exchange pushed past a shared
/// stack's target. Those the heartbeat launders last whatever the rules did, one magazine for
/// each push, so that the stack never gains on the rate at which magazines arrive
/// (`heartbeat.rs`). What a rule leaves it does at a later slow path: the clocked rules look
/// again, and what is left over a target the shed takes back. For classes of 254 objects or
/// fewer a magazine, that is four magazines, about what a slow path could launder before.
pub const LAUNDER_BUDGET: usize = MagazineSize::MAX.capacity();

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
    /// Bytes of objects a class's stacks hold in a process on one cache domain: the domain
    /// stack's cap and the shared stack's target together.
    pub depot_target: usize,
    /// Bytes of objects a class's stack in one cache domain holds before magazines go to the
    /// shared stack instead; the rest of `depot_target` is the shared stack's target, over
    /// which the heartbeat launders (`heartbeat.rs`).
    pub domain_cap: usize,
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
    /// Whether a CPU keeps stock: spares beyond `max_spares` for a class whose magazines it
    /// has sent to the depot and taken back, within the bytes the fixed limits could park on
    /// one CPU (`stock.rs`).
    pub stock: bool,
}

impl Tuning {
    /// The allocator's limits.
    pub const DEFAULT: Tuning = Tuning {
        spare_budget: SPARE_BUDGET,
        max_spares: MAX_SPARES,
        depot_target: DEPOT_TARGET,
        domain_cap: DOMAIN_CAP,
        look_every: LOOK_EVERY,
        refresh_every: REFRESH_EVERY,
        age_limit: true,
        shed_unused: true,
        stock: true,
    };
}

/// The depot and the rest of the middle tier's shared state. One per heap, static in the
/// allocator; the all-zero value is valid: empty stacks, no magazines made, nothing prepared,
/// one cache domain.
///
/// **Two levels.** Each class has a stack of magazines with objects in each cache domain
/// ([`domain`]), and one shared stack; so does each magazine size, for empty magazines. A slow
/// path retiring a magazine pushes it on its CPU's domain's stack if that holds less than its
/// cap, else on the shared stack; one that needs a magazine pops its domain's stack, else the
/// shared one, else fills or makes one. Nobody reads another domain's stack on a slow path. So
/// a domain in balance keeps its magazines, and the objects they name, in its own L3: they
/// reach the shared stack only at the ends of its range. A flow of objects from one domain to
/// another fills the freeing domain's stack once and then passes through the shared stack at
/// two atomic instructions a magazine, however many other domains use the class. The cap
/// bounds what a domain parks; the heartbeat launders the shared stack down to its target
/// (`heartbeat.rs`), and every stack sheds what it did not need (`shed.rs`), a quiet domain's
/// through the other domains' looks (`held.rs`). Untouched domains cost no memory.
///
/// Every group of words is on lines of its own (`crate::lined`): a class's full stack with its
/// water mark, which its pushes, pops and sheds write; each size's empty stack; each pool's
/// set of owing classes and of classes it holds; the magazine area's growth; the start-up
/// status and the CPU count, which every slow path reads and almost nothing writes; and the
/// sets of domains that owe and that hold magazines. Packed together, the stack heads of eight
/// classes shared a line, and two CPUs exchanging magazines of different classes took it from
/// each other on every push and pop.
pub struct Depot {
    /// Per cache domain, and the shared pool last ([`Pool::index`]).
    pools: [Stacks; POOLS],
    growth: Lined<Growth>,
    common: Lined<Common>,
    /// The domains whose stacks owe magazines to the source, one bit per domain (`shed.rs`).
    owing_domains: Lined<AtomicU64>,
    /// The domains whose stacks of some class may hold magazines, one bit per domain
    /// (`held.rs`).
    held_domains: Lined<AtomicU64>,
    /// Which CPUs are in which domain.
    topology: domain::Domains,
    /// Magazines made, for audits.
    #[cfg(test)]
    created: std::sync::atomic::AtomicUsize,
}

/// The pools of stacks: one per cache domain and the shared one.
const POOLS: usize = MAX_DOMAINS + 1;

/// Which of the depot's sets of stacks: a cache domain's, or the shared one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Pool {
    /// The stacks of the CPUs of one cache domain.
    Domain(Domain),
    /// The stacks every domain overflows into and takes from when its own are empty.
    Shared,
}

impl Pool {
    /// The pool's place in the depot: its domain's number, or [`MAX_DOMAINS`] for the shared one.
    const fn index(self) -> usize {
        match self {
            Pool::Domain(domain) => domain.index(),
            Pool::Shared => MAX_DOMAINS,
        }
    }
}

/// One pool's stacks.
struct Stacks {
    /// Per small class, by ordinal.
    classes: [Lined<ClassDepot>; SMALL_COUNT],
    /// Per magazine size, by ordinal: empty magazines.
    empty: [Lined<Stack>; MagazineSize::COUNT],
    /// The classes whose stacks here owe magazines to the source, one bit per ordinal
    /// (`shed.rs`).
    owing: Lined<AtomicU64>,
    /// What other domains' looks read here (`held.rs`); only a domain's pool keeps it.
    marks: Lined<Marks>,
}

/// A domain's words that other domains' looks read, on one line: written rarely, by a push onto
/// an empty stack, a look that finds one emptied, and the domain's own first look in an
/// interval.
struct Marks {
    /// The classes whose stacks here may hold magazines, one bit per ordinal.
    held: AtomicU64,
    /// The last interval in which one of the domain's own CPUs looked at its stacks, plus one,
    /// as `shed::interval` numbers it; zero for never.
    looked: AtomicU64,
}

impl Stacks {
    const fn new() -> Stacks {
        Stacks {
            classes: [const {
                Lined::new(ClassDepot {
                    full: Stack::new(),
                    water: AtomicU64::new(0),
                })
            }; SMALL_COUNT],
            empty: [const { Lined::new(Stack::new()) }; MagazineSize::COUNT],
            owing: Lined::new(AtomicU64::new(0)),
            marks: Lined::new(Marks {
                held: AtomicU64::new(0),
                looked: AtomicU64::new(0),
            }),
        }
    }
}

/// One class's part of a pool.
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
struct Common {
    /// What start-up has done (`generation.rs`).
    status: AtomicU64,
    /// One more than the largest CPU number a slow path has run on.
    cpus: AtomicUsize,
}

// Each group of words on lines of its own (`crate::lined`): a pool is a line per class, a line
// per magazine size and its two sets; the depot, its pools, four lines of its own words and
// the topology's table, after which tests' count of magazines made takes what the table's last
// line leaves.
const _: () = assert!(
    size_of::<Stacks>()
        == LINE * (SMALL_COUNT * lines(size_of::<ClassDepot>()) + MagazineSize::COUNT + 2)
);
const _: () = assert!(
    size_of::<Depot>()
        == POOLS * size_of::<Stacks>()
            + LINE
                * (lines(size_of::<Growth>())
                    + lines(size_of::<Common>())
                    + 2
                    + lines(
                        size_of::<domain::Domains>()
                            + if cfg!(test) { size_of::<usize>() } else { 0 }
                    ))
);

impl Depot {
    /// The empty depot.
    #[allow(clippy::new_without_default)]
    pub const fn new() -> Depot {
        Depot {
            pools: [const { Stacks::new() }; POOLS],
            growth: Lined::new(Growth {
                carved: AtomicUsize::new(0),
                committed: AtomicUsize::new(0),
            }),
            common: Lined::new(Common {
                status: AtomicU64::new(0),
                cpus: AtomicUsize::new(0),
            }),
            owing_domains: Lined::new(AtomicU64::new(0)),
            held_domains: Lined::new(AtomicU64::new(0)),
            topology: domain::Domains::new(),
            #[cfg(test)]
            created: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    fn pool(&self, pool: Pool) -> &Stacks {
        &self.pools[pool.index() % POOLS]
    }

    /// The stack of magazines with objects of `class` in `pool`.
    pub(crate) fn full(&self, pool: Pool, class: SmallClass) -> &Stack {
        &self.pool(pool).classes[class.ordinal() % SMALL_COUNT].full
    }

    /// The water mark of the class of ordinal `ordinal` in `pool` (`shed.rs`).
    pub(crate) fn water(&self, pool: Pool, ordinal: usize) -> &AtomicU64 {
        &self.pool(pool).classes[ordinal % SMALL_COUNT].water
    }

    /// The set of classes whose stacks in `pool` owe (`shed.rs`).
    pub(crate) fn owing(&self, pool: Pool) -> &AtomicU64 {
        &self.pool(pool).owing
    }

    /// The set of classes whose stacks in `domain` may hold magazines (`held.rs`).
    pub(crate) fn held(&self, domain: Domain) -> &AtomicU64 {
        &self.pool(Pool::Domain(domain)).marks.held
    }

    /// The last interval, plus one, in which `domain`'s own CPUs looked at its stacks
    /// (`held.rs`).
    pub(crate) fn looked(&self, domain: Domain) -> &AtomicU64 {
        &self.pool(Pool::Domain(domain)).marks.looked
    }

    /// The stack of empty magazines of `size` in `pool`.
    pub(crate) fn empty(&self, pool: Pool, size: MagazineSize) -> &Stack {
        &self.pool(pool).empty[size.ordinal() % MagazineSize::COUNT]
    }

    /// How many cache domains the stacks are kept for: 1 until start-up has found more.
    pub(crate) fn domain_count(&self) -> usize {
        self.topology.count()
    }

    /// Every pool in use: each domain's, then the shared one.
    pub(crate) fn pools(&self) -> impl Iterator<Item = Pool> {
        (0..self.domain_count())
            .map(|n| Pool::Domain(Domain::folded(n)))
            .chain([Pool::Shared])
    }

    /// The domain of CPU `cpu`; domain 0 for none.
    pub(crate) fn domain_of(&self, cpu: Option<Cpu>) -> Domain {
        self.topology.of(cpu.map(Cpu::index))
    }

    /// Puts CPU number `c` in cache domain `c / width` instead of the domain discovery gave
    /// it, so that a test or benchmark can run with more domains than the machine has L3s. A
    /// slow path that reads the table while it changes may use either domain; nothing but
    /// where magazines are kept depends on it. Only with the hidden feature `internals`, never
    /// in a build that serves a program.
    #[cfg(any(test, feature = "internals"))]
    #[doc(hidden)]
    pub fn split_domains<O: Os>(&self, os: &O, width: usize) {
        self.topology.split(os, width);
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
    /// What this view pushed on the shared stacks of magazines with objects, for the heartbeat's
    /// look at their targets (`heartbeat.rs`). A view lasts one call, so this is the call's,
    /// not the thread's.
    overflowed: Cell<Overflow>,
    /// The objects this view's slow path has laundered, against [`LAUNDER_BUDGET`]. Each slow
    /// path starts it again from zero.
    laundered: Cell<usize>,
    /// The objects its exchange pushed on shared stacks, which the heartbeat's launder over
    /// target may yet take back, set aside from the budget while the other rules run.
    reserved: Cell<usize>,
}

/// The shared stacks a call pushed magazines with objects on: their classes, one bit per
/// ordinal, how many magazines in all, and how many objects in them.
#[derive(Clone, Copy, Default, Debug)]
pub(super) struct Overflow {
    pub(super) classes: u64,
    pub(super) magazines: u32,
    /// The objects in those magazines.
    pub(super) objects: usize,
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
            overflowed: Cell::new(Overflow::default()),
            laundered: Cell::new(0),
            reserved: Cell::new(0),
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

    /// The objects of `class` a domain's stack holds before magazines go to the shared stack:
    /// at least one, so that the stack takes at least a magazine.
    pub(super) fn domain_cap(&self, class: SmallClass) -> u32 {
        (self.tuning.domain_cap / class.size()).clamp(1, u32::MAX as usize) as u32
    }

    /// The empty magazines of `class`'s size a domain's stack holds before they go to the
    /// shared stack: as many as the domain's full stack of the class holds, at least one.
    fn empty_cap(&self, class: SmallClass) -> u32 {
        self.domain_cap(class)
            .div_ceil(class.magazine().capacity() as u32)
            .max(1)
    }

    /// The objects of `class` the shared stack holds before the heartbeat launders from it:
    /// what the target leaves after the domain's cap, and at least a magazine, so that a flow
    /// between domains passes through it instead of being laundered as it arrives.
    pub(super) fn shared_target(&self, class: SmallClass) -> usize {
        let bytes = self
            .tuning
            .depot_target
            .saturating_sub(self.tuning.domain_cap);
        (bytes / class.size()).max(class.magazine().capacity())
    }

    /// Sends a magazine this thread holds to the depot for the domain the thread is in. Whether
    /// a compare-and-swap of the push that took it failed: whether the stack was contended.
    fn to_depot(&self, class: SmallClass, magazine: Magazine<'_>) -> bool {
        self.to_domain(self.here(), class, magazine)
    }

    /// Sends a magazine this thread holds to `domain`'s stacks: its class's stack if it has
    /// objects, else its size's empty stack, if that is under its cap; else to the shared
    /// stack of the same kind. Whether the push that took it met contention.
    fn to_domain(&self, domain: Domain, class: SmallClass, magazine: Magazine<'_>) -> bool {
        let pool = Pool::Domain(domain);
        let level = magazine.level() as u32;
        let overflow = if level > 0 {
            match self.depot.full(pool, class).push_under(
                self.area,
                magazine,
                level,
                self.domain_cap(class),
            ) {
                Ok(pushed) => {
                    if pushed.onto_empty {
                        self.hold(domain, class);
                    }
                    return pushed.contended;
                }
                Err(magazine) => magazine,
            }
        } else {
            let size = magazine.size();
            match self.depot.empty(pool, size).push_under(
                self.area,
                magazine,
                1,
                self.empty_cap(class),
            ) {
                Ok(pushed) => return pushed.contended,
                Err(magazine) => magazine,
            }
        };
        self.to_shared(class, overflow)
    }

    /// Sends a magazine this thread holds to the shared stacks. Whether the push met
    /// contention.
    fn to_shared(&self, class: SmallClass, magazine: Magazine<'_>) -> bool {
        let level = magazine.level() as u32;
        if level > 0 {
            let contended = self
                .depot
                .full(Pool::Shared, class)
                .push(self.area, magazine, level);
            let seen = self.overflowed.get();
            self.overflowed.set(Overflow {
                classes: seen.classes | 1 << (class.ordinal() % SMALL_COUNT),
                magazines: seen.magazines.saturating_add(1),
                objects: seen.objects.saturating_add(level as usize),
            });
            contended
        } else {
            self.depot
                .empty(Pool::Shared, magazine.size())
                .push(self.area, magazine, 1)
        }
    }

    /// Sends a magazine this thread holds to `pool`'s stacks, as [`Cache::to_domain`] does for
    /// a domain's.
    fn to_pool(&self, pool: Pool, class: SmallClass, magazine: Magazine<'_>) {
        let _ = match pool {
            Pool::Domain(domain) => self.to_domain(domain, class, magazine),
            Pool::Shared => self.to_shared(class, magazine),
        };
    }

    /// Returns the objects of a magazine this thread holds to the source, in one call, and
    /// records it empty. Returns how many there were.
    fn launder_held(&self, class: SmallClass, magazine: &Magazine<'_>) -> usize {
        let level = magazine.level();
        if let Some(objects) = magazine.objects().get(..level) {
            self.source.take_back(class, objects);
        }
        magazine.set_level(0);
        self.laundered
            .set(self.laundered.get().saturating_add(level));
        level
    }

    /// Whether a rule that may wait has room in this slow path's budget to launder a magazine
    /// of `class`, full or not ([`LAUNDER_BUDGET`]), beside what is set aside.
    fn may_launder(&self, class: SmallClass) -> bool {
        self.laundered
            .get()
            .saturating_add(self.reserved.get())
            .saturating_add(class.magazine().capacity())
            <= LAUNDER_BUDGET
    }

    /// Starts a slow path's count of the objects it launders.
    fn begin_slow_path(&self) {
        self.laundered.set(0);
        self.reserved.set(0);
    }

    /// Pops one magazine of `class` from its stack in `pool` and launders it; the empty
    /// magazine goes to that pool's empty stacks. Returns how many objects it held, or `None`
    /// if the stack was empty.
    fn launder_one(&self, pool: Pool, class: SmallClass) -> Option<usize> {
        let magazine = self
            .depot
            .full(pool, class)
            .pop(self.area, class.magazine())?;
        let level = self.launder_held(class, &magazine);
        self.to_pool(pool, class, magazine);
        Some(level)
    }
}
