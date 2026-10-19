//! The middle tier's part of the introspection walk (`crate::stats`): the magazines loaded in
//! each CPU's slab, on its spare stacks and in the depot, and the objects in them.
//!
//! Read-only and racy. A magazine handle is only made for an address the area admits, so a
//! stale slab word or link ends a walk early or reports a magazine at the wrong place; it never
//! reads outside the committed area. Chains are followed for a bounded number of steps, since a
//! stack that changes under the walk can lead it in a circle.

use super::magazine::{Area, Magazine};
use super::{Depot, UNIT};
use crate::classes::{MagazineSize, SmallClass, is_marker};
use crate::front::{Field, SLAB_BYTES, Slabs, Spare};
use crate::os::Os;
use crate::space::Space;
use crate::stats::{Place, Visitor};
use core::ptr;
use core::sync::atomic::Ordering::Relaxed;

/// The most magazines followed on one spare stack. The budget allows a handful.
const SPARE_STEPS: usize = 16;

/// The most magazines followed on one depot stack.
const DEPOT_STEPS: usize = 1 << 20;

fn report(
    visitor: &mut impl Visitor,
    place: Place,
    class: SmallClass,
    magazine: &Magazine<'_>,
    level: usize,
) {
    visitor.magazine(place, class, level);
    for index in 1..=level {
        visitor.cached(place, class, magazine.object(index).cast_const());
    }
}

/// Reports the magazine area, the slabs in use, every magazine with objects, and the depot's
/// empty magazines, for the heap at `space` whose middle tier is `depot` and whose slab array is
/// `slabs`.
pub fn inspect<O: Os>(
    depot: &Depot,
    space: Space<'_, O>,
    slabs: &Slabs,
    visitor: &mut impl Visitor,
) {
    let area = Area::new(space, depot);
    visitor.magazine_area(
        space.magazines().as_ptr().cast_const(),
        UNIT + depot.growth.carved.load(Relaxed),
        depot.growth.committed.load(Relaxed),
    );
    let cpus = depot.common.cpus.load(Relaxed).min(slabs.len());
    visitor.slabs(ptr::from_ref(slabs).cast::<u8>(), SLAB_BYTES, cpus);
    for cpu in (0..cpus).filter_map(|index| slabs.cpu(index as i32)) {
        let slab = slabs.slab(cpu);
        for class in SmallClass::all() {
            let size = class.magazine();
            let hot = slab.field(Field::hot(class)).load(Relaxed).addr();
            if hot != 0
                && !is_marker(hot)
                && let Some((magazine, level)) = area.loaded(hot, size)
            {
                report(visitor, Place::Loaded(cpu.get()), class, &magazine, level);
            }
            for kind in [Spare::Full, Spare::Empty] {
                let mut next = slab.field(Field::spare(class, kind)).load(Relaxed).addr();
                for _ in 0..SPARE_STEPS {
                    if next == 0 || is_marker(next) {
                        break;
                    }
                    let Some(magazine) = area.magazine(next, size) else {
                        break;
                    };
                    next = magazine.link().load(Relaxed).addr();
                    let level = magazine.level();
                    report(visitor, Place::Spare(cpu.get()), class, &magazine, level);
                }
            }
        }
    }
    for pool in depot.pools() {
        for class in SmallClass::all() {
            depot
                .full(pool, class)
                .walk(area, class.magazine(), DEPOT_STEPS, |magazine| {
                    report(visitor, Place::Depot, class, magazine, magazine.level());
                });
        }
    }
    for size in MagazineSize::all() {
        let mut count = 0;
        for pool in depot.pools() {
            depot
                .empty(pool, size)
                .walk(area, size, DEPOT_STEPS, |_| count += 1);
        }
        visitor.empties(size, count);
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_rig::{Rig, SimCpu, Small, TIGHT, tiny_class};
    use super::*;
    use crate::os::sim::SimOs;
    use std::collections::BTreeSet;
    use std::vec::Vec;

    #[derive(Default)]
    struct Seen {
        objects: BTreeSet<usize>,
        places: Vec<(Place, usize)>,
        empties: usize,
        slabs: usize,
    }

    impl Visitor for Seen {
        fn slabs(&mut self, _: *const u8, _: usize, count: usize) {
            self.slabs = count;
        }
        fn magazine(&mut self, place: Place, _: SmallClass, objects: usize) {
            self.places.push((place, objects));
        }
        fn cached(&mut self, _: Place, _: SmallClass, object: *const u8) {
            assert!(
                self.objects.insert(object.addr()),
                "an object reported twice"
            );
        }
        fn empties(&mut self, _: MagazineSize, count: usize) {
            self.empties += count;
        }
    }

    #[test]
    fn the_walk_finds_every_cached_object_once() {
        let rig: Rig<SimOs<Small>, 2> = Rig::new(&[tiny_class()], 12, TIGHT);
        let machine = SimCpu::new(0);
        let cache = rig.cache(&machine);
        let class = tiny_class();
        let mut user = Vec::new();
        for _ in 0..7 {
            user.push(cache.alloc(class).unwrap());
        }
        machine.set_cpu(1);
        for object in user.drain(..5) {
            cache.free(class, object);
        }
        let census = rig.census(&user, false);
        let mut seen = Seen::default();
        inspect(&rig.depot, rig.cell.get().unwrap(), rig.slabs(), &mut seen);
        // Every object is the user's, the source's or cached, and the census found none lost.
        let cached = census.objects - user.len() - rig.source.held().len();
        assert_eq!(seen.objects.len(), cached);
        // The tight depot target launders some of what was freed; the rest is cached.
        assert!(cached > 0);
        assert_eq!(seen.places.iter().map(|&(_, n)| n).sum::<usize>(), cached);
        assert_eq!(seen.places.len() + seen.empties, census.magazines);
        assert_eq!(seen.slabs, 2);
        assert!(
            seen.places
                .iter()
                .any(|&(place, _)| matches!(place, Place::Loaded(_)))
        );
    }
}
