//! Test equipment for the middle tier: a small heap shape, an object source, a simulated CPU,
//! and a census that checks the TLA+ models' invariants on a quiescent heap.

use super::{Cache, Depot, Magazine, Pool, Source, Tuning};
use crate::classes::{MagazineSize, SmallClass, is_marker};
use crate::front::check::switch_point;
use crate::front::{Field, Slab, Slabs, Spare, Twin};
use crate::os::Os;
use crate::os::sim::{Shape, SimOs};
use crate::os::{Aborted, Machine, Step};
use crate::space::{Geometry, SpaceCell};
use core::cell::Cell;
use core::ptr::{self, NonNull};
use core::sync::atomic::Ordering::Relaxed;
use std::boxed::Box;
use std::collections::{BTreeMap, BTreeSet};
use std::string::String;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::vec::Vec;

/// A heap of 128 KiB: 64-byte pages, 4 KiB regions, a 32 KiB magazine area (eight regions).
/// Small enough to make afresh for every execution the interleaving checker runs.
pub(crate) enum Small {}

impl Shape for Small {
    const GEOMETRY: Geometry = Geometry::new(6, 8, 12, 12, 32, 8, MagazineSize::MAX);
}

/// The class with the smallest magazines (4 slots, 2 objects), which keeps every scenario
/// small.
pub(crate) fn tiny_class() -> SmallClass {
    crate::classes::Class::LARGEST_SMALL.small().unwrap()
}

/// Limits under which every path is reachable with the tiny class: spares on both stacks, a
/// depot target of one magazine, a look for an idle slab on every slow path, every rule that
/// returns parked objects on, and every other retiring full magazine refreshed.
pub(crate) const TIGHT: Tuning = Tuning {
    spare_budget: usize::MAX / 2,
    max_spares: 1,
    depot_target: 2 * (32 << 10),
    domain_cap: 32 << 10,
    look_every: 1,
    refresh_every: 2,
    age_limit: true,
    shed_unused: true,
};

/// A fake object: an address that is never dereferenced.
pub(crate) fn object(class: SmallClass, n: usize) -> NonNull<u8> {
    let address = 0x10_0000_0000 + (class.ordinal() << 28) + n * class.size();
    NonNull::new(ptr::without_provenance_mut(address)).unwrap()
}

/// A source with a fixed set of objects per class, which panics if one comes back that it did
/// not hand out.
pub(crate) struct TestSource {
    free: Mutex<BTreeMap<usize, Vec<usize>>>,
    out: Mutex<BTreeSet<usize>>,
    purges: AtomicUsize,
    passes: AtomicUsize,
    fills: AtomicUsize,
    universe: BTreeSet<usize>,
}

impl TestSource {
    /// `count` objects of each class in `classes`.
    pub(crate) fn new(classes: &[SmallClass], count: usize) -> TestSource {
        let mut free = BTreeMap::new();
        let mut universe = BTreeSet::new();
        for &class in classes {
            // Handed out from the end, so the lowest addresses go first.
            let objects: Vec<usize> = (0..count)
                .rev()
                .map(|n| object(class, n).addr().get())
                .collect();
            universe.extend(objects.iter().copied());
            free.insert(class.ordinal(), objects);
        }
        TestSource {
            free: Mutex::new(free),
            out: Mutex::new(BTreeSet::new()),
            purges: AtomicUsize::new(0),
            passes: AtomicUsize::new(0),
            fills: AtomicUsize::new(0),
            universe,
        }
    }

    fn take(&self, class: SmallClass) -> Option<NonNull<u8>> {
        let address = self.free.lock().unwrap().get_mut(&class.ordinal())?.pop()?;
        assert!(self.out.lock().unwrap().insert(address));
        NonNull::new(ptr::without_provenance_mut(address))
    }

    fn give(&self, class: SmallClass, object: NonNull<u8>) {
        let address = object.addr().get();
        assert!(
            self.out.lock().unwrap().remove(&address),
            "the source got back {address:#x}, which it did not hand out"
        );
        self.free
            .lock()
            .unwrap()
            .get_mut(&class.ordinal())
            .expect("an object of a class the source has")
            .push(address);
    }

    /// The objects the source holds.
    pub(crate) fn held(&self) -> Vec<usize> {
        self.free
            .lock()
            .unwrap()
            .values()
            .flatten()
            .copied()
            .collect()
    }

    pub(crate) fn purges(&self) -> usize {
        self.purges.load(Ordering::Relaxed)
    }

    /// How many times `maintain` has asked for a whole purge pass.
    pub(crate) fn passes(&self) -> usize {
        self.passes.load(Ordering::Relaxed)
    }

    /// The objects of `class` the source holds.
    pub(crate) fn held_of(&self, class: SmallClass) -> usize {
        self.free
            .lock()
            .unwrap()
            .get(&class.ordinal())
            .map_or(0, Vec::len)
    }

    /// How many magazines the source has been asked to fill.
    pub(crate) fn fills(&self) -> usize {
        self.fills.load(Ordering::Relaxed)
    }
}

impl Source for TestSource {
    fn fill(&self, class: SmallClass, slots: &[core::sync::atomic::AtomicPtr<u8>]) -> usize {
        self.fills.fetch_add(1, Ordering::Relaxed);
        let mut count = 0;
        for slot in slots.iter().rev() {
            match self.take(class) {
                Some(object) => {
                    slot.store(object.as_ptr(), Relaxed);
                    count += 1;
                }
                None => break,
            }
        }
        count
    }

    fn take_back(&self, class: SmallClass, objects: &[core::sync::atomic::AtomicPtr<u8>]) {
        for object in objects {
            let object = NonNull::new(object.load(Relaxed)).expect("a magazine holds null");
            self.give(class, object);
        }
    }

    fn alloc_one(&self, class: SmallClass) -> Option<NonNull<u8>> {
        self.take(class)
    }

    fn free_one(&self, class: SmallClass, object: NonNull<u8>) {
        self.give(class, object);
    }

    fn purge_step(&self, _: &dyn Fn() -> u64) {
        self.purges.fetch_add(1, Ordering::Relaxed);
    }

    fn purge_pass(&self, _: &dyn Fn() -> u64) {
        self.passes.fetch_add(1, Ordering::Relaxed);
    }
}

/// A simulated CPU for one thread: tests move it and abort its sequence.
pub(crate) struct SimCpu {
    cpu: Cell<i32>,
    pending: Cell<bool>,
}

impl SimCpu {
    pub(crate) fn new(cpu: i32) -> SimCpu {
        SimCpu {
            cpu: Cell::new(cpu),
            pending: Cell::new(false),
        }
    }

    /// What a signal, preemption or migration does: the sequence in flight, if any, restarts,
    /// on `cpu`.
    pub(crate) fn interrupt(&self, cpu: i32) {
        self.cpu.set(cpu);
        self.pending.set(true);
    }

    pub(crate) fn set_cpu(&self, cpu: i32) {
        self.cpu.set(cpu);
    }

    pub(crate) fn cpu_id_now(&self) -> i32 {
        self.cpu.get()
    }
}

impl Machine for SimCpu {
    fn cpu_id(&self) -> i32 {
        self.cpu.get()
    }

    fn step(&self, step: Step) -> Result<(), Aborted> {
        switch_point();
        // An interrupt before the first step needs no abort: nothing has been read.
        if self.pending.replace(false) && step != Step::ReadCpu {
            return Err(Aborted);
        }
        Ok(())
    }
}

/// A heap for tests: a simulated OS, a depot, `N` slabs, and a test source.
pub(crate) struct Rig<O: Os, const N: usize> {
    pub(crate) cell: SpaceCell<O>,
    pub(crate) depot: Depot,
    pub(crate) slabs: Box<Slabs<[Slab; N]>>,
    pub(crate) source: TestSource,
    pub(crate) tuning: Tuning,
}

impl<const N: usize> Rig<SimOs<Small>, N> {
    /// A heap with `count` objects of each class in `classes`.
    pub(crate) fn new(classes: &[SmallClass], count: usize, tuning: Tuning) -> Self {
        Rig::with_os(SimOs::new(), classes, count, tuning)
    }
}

impl<O: Os, const N: usize> Rig<O, N> {
    pub(crate) fn with_os(os: O, classes: &[SmallClass], count: usize, tuning: Tuning) -> Self {
        let rig = Rig {
            cell: SpaceCell::new(os),
            depot: Depot::new(),
            slabs: Box::new(Slabs::new()),
            source: TestSource::new(classes, count),
            tuning,
        };
        rig.cell
            .get_or_reserve()
            .expect("the simulated OS reserves");
        rig
    }

    pub(crate) fn slabs(&self) -> &Slabs {
        &*self.slabs
    }

    pub(crate) fn os(&self) -> &O {
        self.cell.os()
    }

    /// The caches as a thread on `machine` sees them.
    pub(crate) fn cache<M: Machine>(&self, machine: M) -> Cache<'_, O, Twin<'_, M>, TestSource> {
        let space = self.cell.get().expect("reserved in `new`");
        // SAFETY: a rig's depot and slabs are used only with the rig's one published space.
        unsafe {
            Cache::with_tuning(
                &self.depot,
                space,
                Twin::new(self.slabs(), machine),
                &self.source,
                self.tuning,
            )
        }
    }

    /// Checks the heap, with `user` the objects the program holds, and returns what it found.
    /// Every magazine is in exactly one place and every object in exactly one place, and each
    /// stack holds what its name says. Unless `lossy` (after a fork), nothing is missing.
    pub(crate) fn census(&self, user: &[NonNull<u8>], lossy: bool) -> Census {
        let cache = self.cache(crate::os::Pinned(-1));
        let area = cache.area();
        let mut census = Census::default();
        let mut magazines = BTreeMap::new();
        let mut objects: BTreeMap<usize, String> = BTreeMap::new();
        let mut place_magazine = |magazine: &Magazine, level: usize, place: String| {
            if let Some(other) = magazines.insert(magazine.id(), place.clone()) {
                panic!("{:?} is in two places: {other} and {place}", magazine.id());
            }
            for i in 1..=level {
                let address = magazine.object(i).addr();
                if let Some(other) = objects.insert(address, place.clone()) {
                    panic!("object {address:#x} is in two places: {other} and {place}");
                }
            }
        };
        for cpu in self.slabs().cpus() {
            let slab = self.slabs().slab(cpu);
            if slab.claim().load(Relaxed) != 0 {
                census.claims += 1;
            }
            for class in SmallClass::all() {
                let size = class.magazine();
                let hot = slab.field(Field::hot(class)).load(Relaxed);
                if is_marker(hot.addr()) {
                    census.markers += 1;
                } else if !hot.is_null() {
                    let (magazine, level) = area.loaded(hot.addr(), size).expect("a hot word");
                    place_magazine(&magazine, level, std::format!("cpu {cpu:?} {class:?}"));
                }
                for kind in [Spare::Full, Spare::Empty] {
                    let mut next = slab.field(Field::spare(class, kind)).load(Relaxed).addr();
                    if is_marker(next) {
                        census.markers += 1;
                        continue;
                    }
                    let mut depth = 0;
                    while next != 0 {
                        let magazine = area.magazine(next, size).expect("a spare");
                        let level = magazine.level();
                        assert_eq!(
                            level > 0,
                            kind == Spare::Full,
                            "a spare holds the wrong kind"
                        );
                        next = magazine.link().load(Relaxed).addr();
                        depth += 1;
                        assert!(depth < 64, "a spare stack loops");
                        place_magazine(
                            &magazine,
                            level,
                            std::format!("cpu {cpu:?} {kind:?} spare of {class:?}"),
                        );
                    }
                    census.spares += depth;
                }
            }
        }
        // Every domain, used or not, and the shared pool: a magazine must not be anywhere a
        // census misses.
        let pools = (0..super::MAX_DOMAINS)
            .map(|n| Pool::Domain(super::Domain::folded(n)))
            .chain([Pool::Shared]);
        for pool in pools {
            for class in SmallClass::all() {
                let stack = self.depot.full(pool, class);
                let mut total = 0;
                for magazine in stack.magazines(area, class.magazine()) {
                    let level = magazine.level();
                    assert!(level > 0, "an empty magazine on {class:?}'s depot stack");
                    total += level;
                    place_magazine(&magazine, level, std::format!("depot {pool:?} {class:?}"));
                    census.depot += 1;
                }
                assert_eq!(stack.count(area, class.magazine()) as usize, total);
                // A fork can cut a push off between its compare-and-swap and its hint.
                if let Pool::Domain(domain) = pool
                    && total > 0
                    && !lossy
                {
                    let bit = 1 << (class.ordinal() % crate::classes::SMALL_COUNT);
                    assert_ne!(
                        self.depot.held(domain).load(Relaxed) & bit,
                        0,
                        "{class:?}'s stack in {domain:?} holds magazines, unmarked"
                    );
                    assert_ne!(
                        self.depot.held_domains.load(Relaxed) & 1 << domain.index(),
                        0,
                        "{domain:?} holds magazines, unmarked"
                    );
                }
                // A debt the sheds cannot find is never paid: its class's bit, and its domain's,
                // must be set (`shed.rs`, `settle`). A fork can cut a look off between its
                // debt and its bits.
                let ordinal = class.ordinal() % crate::classes::SMALL_COUNT;
                let owed = super::shed::Water(self.depot.water(pool, ordinal).load(Relaxed)).owed();
                if owed > 0 && !lossy {
                    assert_ne!(
                        self.depot.owing(pool).load(Relaxed) & 1 << ordinal,
                        0,
                        "{class:?}'s stack in {pool:?} owes {owed}, unmarked"
                    );
                    if let Pool::Domain(domain) = pool {
                        assert_ne!(
                            self.depot.owing_domains.load(Relaxed) & 1 << domain.index(),
                            0,
                            "{domain:?} owes, unmarked"
                        );
                    }
                }
            }
            for size in crate::classes::MagazineSize::all() {
                let stack = self.depot.empty(pool, size);
                let mut count = 0;
                for magazine in stack.magazines(area, size) {
                    assert_eq!(magazine.level(), 0, "a full magazine on an empty stack");
                    place_magazine(&magazine, 0, std::format!("depot {pool:?} empty {size:?}"));
                    census.depot += 1;
                    count += 1;
                }
                assert_eq!(stack.count(area, size) as usize, count);
            }
        }
        for address in self.source.held() {
            if let Some(other) = objects.insert(address, String::from("source")) {
                panic!("object {address:#x} is in two places: {other} and the source");
            }
        }
        for object in user {
            let address = object.addr().get();
            if let Some(other) = objects.insert(address, String::from("user")) {
                panic!("object {address:#x} is in two places: {other} and the user's");
            }
        }
        census.magazines = magazines.len();
        census.objects = objects.len();
        let created = self.depot.created.load(Ordering::Relaxed);
        assert!(census.magazines <= created);
        for address in objects.keys() {
            assert!(
                self.source.universe.contains(address),
                "{address:#x} is no object"
            );
        }
        if !lossy {
            assert_eq!(census.magazines, created, "a magazine was lost");
            assert_eq!(
                census.objects,
                self.source.universe.len(),
                "an object was lost"
            );
        }
        census.lost_magazines = created - census.magazines;
        census.lost_objects = self.source.universe.len() - census.objects;
        census
    }
}

/// What a census found.
#[derive(Default, Debug)]
pub(crate) struct Census {
    pub(crate) magazines: usize,
    pub(crate) objects: usize,
    pub(crate) spares: usize,
    pub(crate) depot: usize,
    pub(crate) markers: usize,
    pub(crate) claims: usize,
    pub(crate) lost_magazines: usize,
    pub(crate) lost_objects: usize,
}
