//! Test equipment for the middle tier: a small heap shape, an object source, a simulated CPU,
//! and a census that checks the TLA+ models' invariants on a quiescent heap.

use super::{Cache, Depot, Magazine, Source, Tuning};
use crate::classes::{SmallClass, is_marker};
use crate::front::check::switch_point;
use crate::front::{Aborted, Field, Machine, Slab, Slabs, Spare, Step, Twin};
use crate::os::Os;
use crate::os::sim::{Shape, SimOs};
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

/// A heap of 128 KiB: 64-byte pages, 4 KiB regions, a 32 KiB magazine area. Small enough to
/// make afresh for every execution the interleaving checker runs.
pub(crate) enum Small {}

impl Shape for Small {
    const GEOMETRY: Geometry = Geometry::new(6, 8, 12, 32, 15);
}

/// The class with the smallest magazines (4 slots, 2 objects), which keeps every scenario
/// small.
pub(crate) fn tiny_class() -> SmallClass {
    crate::classes::Class::LARGEST_SMALL.small().unwrap()
}

/// Limits under which every path is reachable with the tiny class: spares on both stacks, and
/// a depot target of one magazine.
pub(crate) const TIGHT: Tuning = Tuning {
    spare_budget: usize::MAX / 2,
    max_spares: 1,
    depot_target: 2 * (32 << 10),
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
}

impl Source for TestSource {
    fn fill(&self, class: SmallClass, out: &mut [Option<NonNull<u8>>]) -> usize {
        let mut count = 0;
        for slot in out.iter_mut() {
            match self.take(class) {
                Some(object) => {
                    *slot = Some(object);
                    count += 1;
                }
                None => break,
            }
        }
        count
    }

    fn take_back(&self, class: SmallClass, objects: &[NonNull<u8>]) {
        for &object in objects {
            self.give(class, object);
        }
    }

    fn alloc_one(&self, class: SmallClass) -> Option<NonNull<u8>> {
        self.take(class)
    }

    fn free_one(&self, class: SmallClass, object: NonNull<u8>) {
        self.give(class, object);
    }

    fn purge_step(&self) {
        self.purges.fetch_add(1, Ordering::Relaxed);
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
        let cache = self.cache(crate::front::Pinned(-1));
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
        for class in SmallClass::all() {
            let stack = self.depot.full(class);
            let mut total = 0;
            for magazine in stack.magazines(area, class.magazine()) {
                let level = magazine.level();
                assert!(level > 0, "an empty magazine on {class:?}'s depot stack");
                total += level;
                place_magazine(&magazine, level, std::format!("depot {class:?}"));
                census.depot += 1;
            }
            assert_eq!(stack.objects(area, class.magazine()) as usize, total);
        }
        for size in crate::classes::MagazineSize::all() {
            for magazine in self.depot.empty(size).magazines(area, size) {
                assert_eq!(magazine.level(), 0, "a full magazine on an empty stack");
                place_magazine(&magazine, 0, std::format!("depot empty {size:?}"));
                census.depot += 1;
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
