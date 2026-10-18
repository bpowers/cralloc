//! Deterministic tests of the rules that send parked objects back to the source: the magazine
//! age limit (`age.rs`), the depot's low-water rule (`shed.rs`) and the refresh fraction
//! (`slow.rs`), on the test rig with the simulated clock. Each runs its scenario with the rule
//! on and with it off, and checks that the outcome the rule promises happens only when it is
//! on, so each test fails if its rule stops working.

use super::test_rig::{Rig, SimCpu, Small, TestSource};
use super::{Cache, IDLE_TICKS, REFRESH_EVERY, Tuning, inspect};
use crate::classes::{MagazineSize, SmallClass};
use crate::front::{Cpu, Field, Twin};
use crate::os::Os;
use crate::os::sim::SimOs;
use crate::stats::{Place, Visitor};
use core::ptr::NonNull;
use core::sync::atomic::Ordering::Relaxed;
use std::vec::Vec;

type TestRig = Rig<SimOs<Small>, 2>;

/// Limits that keep each rule's effect apart from the others': room for one spare per stack,
/// a depot target no test reaches, looks on every slow path, and no rule on.
const QUIET: Tuning = Tuning {
    spare_budget: usize::MAX / 2,
    max_spares: 1,
    depot_target: usize::MAX / 2,
    take_across: usize::MAX / 2,
    look_every: 1,
    refresh_every: 0,
    age_limit: false,
    shed_unused: false,
};

/// Small classes whose magazines hold two objects, largest first, so a few calls cross many
/// slow paths.
fn tiny(n: usize) -> SmallClass {
    let mut classes: Vec<SmallClass> = SmallClass::all()
        .filter(|c| c.magazine() == MagazineSize::MIN)
        .collect();
    classes.reverse();
    classes[n]
}

fn cpu(rig: &TestRig, n: i32) -> Cpu {
    rig.slabs().cpu(n).unwrap()
}

/// Objects parked in magazines, by where.
#[derive(Default)]
struct Parked {
    of: Option<SmallClass>,
    on_cpu: [usize; 2],
    depot: usize,
}

impl Visitor for Parked {
    fn cached(&mut self, place: Place, class: SmallClass, _: *const u8) {
        if self.of.is_some_and(|of| of != class) {
            return;
        }
        match place {
            Place::Loaded(n) | Place::Spare(n) => self.on_cpu[n as usize] += 1,
            Place::Depot => self.depot += 1,
        }
    }
}

fn parked(rig: &TestRig, class: SmallClass) -> Parked {
    let mut seen = Parked {
        of: Some(class),
        ..Parked::default()
    };
    inspect(&rig.depot, rig.cell.get().unwrap(), rig.slabs(), &mut seen);
    seen
}

/// Allocates `n` objects of `class` and frees them again, which with two objects a magazine
/// takes a slow path every other call.
fn churn(
    cache: &Cache<'_, SimOs<Small>, Twin<'_, &SimCpu>, TestSource>,
    class: SmallClass,
    n: usize,
) {
    let held: Vec<NonNull<u8>> = (0..n).filter_map(|_| cache.alloc(class)).collect();
    assert_eq!(held.len(), n);
    for object in held {
        cache.free(class, object);
    }
}

/// Whether CPU `n` has a magazine loaded for `class`.
fn is_loaded(rig: &TestRig, n: i32, class: SmallClass) -> bool {
    let word = rig
        .slabs()
        .slab(cpu(rig, n))
        .field(Field::hot(class))
        .load(Relaxed);
    !word.is_null() && !crate::classes::is_marker(word.addr())
}

/// A class used once on CPU 0, which then stays busy with another class: when, in ticks from
/// the end of its use, CPU 0 held none of the first class's objects, if it did within three
/// intervals.
fn emptied_after_falling_out_of_use(age_limit: bool) -> Option<u64> {
    let (idle, busy) = (tiny(0), tiny(1));
    let rig: TestRig = Rig::new(&[idle, busy], 64, Tuning { age_limit, ..QUIET });
    let machine = SimCpu::new(0);
    let cache = rig.cache(&machine);
    // A loaded magazine with an object in it, and a full spare.
    let held: Vec<_> = (0..5).filter_map(|_| cache.alloc(idle)).collect();
    for object in held {
        cache.free(idle, object);
    }
    let parked_at_start = parked(&rig, idle).on_cpu[0];
    assert!(parked_at_start >= 3, "{parked_at_start}");
    let start = rig.os().ticks();
    let steps = 8;
    for _ in 0..3 * steps {
        rig.os().advance_clock(IDLE_TICKS / steps);
        // Many slow paths, so the age limit's looks go round every class several times.
        for _ in 0..40 {
            churn(&cache, busy, 3);
        }
        if parked(&rig, idle).on_cpu[0] == 0 {
            return Some(rig.os().ticks() - start);
        }
    }
    rig.census(&[], false);
    None
}

#[test]
fn a_class_that_falls_out_of_use_on_a_busy_cpu_is_emptied_within_two_intervals() {
    let emptied = emptied_after_falling_out_of_use(true).expect("emptied");
    assert!(emptied <= 2 * IDLE_TICKS, "{emptied}");
    assert_eq!(emptied_after_falling_out_of_use(false), None);
}

/// A class kept in steady use on CPU 0 without leaving its loaded magazine, while another
/// class's slow paths run there: how many times over `intervals` the age limit took its
/// magazine, each of which costs it a refill.
fn refills_of_a_class_in_steady_use(age_limit: bool, intervals: u64) -> usize {
    let (steady, busy) = (tiny(0), tiny(1));
    let rig: TestRig = Rig::new(&[steady, busy], 256, Tuning { age_limit, ..QUIET });
    let machine = SimCpu::new(0);
    let cache = rig.cache(&machine);
    // A full magazine loaded, from which an allocation and its free stay on the fast path.
    let first = cache.alloc(steady).unwrap();
    cache.free(steady, first);
    let mut refills = 0;
    let steps = 8;
    for _ in 0..intervals * steps {
        rig.os().advance_clock(IDLE_TICKS / steps);
        for _ in 0..40 {
            let object = cache.alloc(steady).unwrap();
            cache.free(steady, object);
            assert!(is_loaded(&rig, 0, steady));
            churn(&cache, busy, 3);
            // Only a take empties the hot word; the next allocation refills it.
            if !is_loaded(&rig, 0, steady) {
                refills += 1;
            }
        }
    }
    rig.census(&[], false);
    refills
}

#[test]
fn a_class_in_steady_use_is_refilled_at_most_once_an_interval() {
    let intervals = 10;
    let refills = refills_of_a_class_in_steady_use(true, intervals);
    assert!(refills as u64 <= intervals, "{refills} refills");
    assert!(refills as u64 >= intervals - 2, "{refills} refills");
    assert_eq!(refills_of_a_class_in_steady_use(false, intervals), 0);
}

/// The objects in `class`'s depot stack.
fn in_depot(rig: &TestRig, class: SmallClass) -> usize {
    let machine = SimCpu::new(0);
    let cache = rig.cache(&machine);
    rig.depot
        .full(super::Domain::ZERO, class)
        .objects(cache.area(), class.magazine()) as usize
}

/// Twenty objects of one class freed into the depot on CPU 0, which then goes quiet, while CPU
/// 1 runs another class's slow paths for three intervals: what is left in the stack.
fn an_idle_stack_after_three_intervals(shed_unused: bool) -> usize {
    let (idle, busy) = (tiny(0), tiny(1));
    let tuning = Tuning {
        max_spares: 0,
        shed_unused,
        ..QUIET
    };
    let rig: TestRig = Rig::new(&[idle, busy], 64, tuning);
    let zero = SimCpu::new(0);
    let cache = rig.cache(&zero);
    let held: Vec<_> = (0..22).filter_map(|_| cache.alloc(idle)).collect();
    for object in held {
        cache.free(idle, object);
    }
    assert!(in_depot(&rig, idle) >= 20);
    let one = SimCpu::new(1);
    let other = rig.cache(&one);
    let steps = 8;
    for _ in 0..3 * steps {
        rig.os().advance_clock(IDLE_TICKS / steps);
        for _ in 0..40 {
            churn(&other, busy, 3);
        }
    }
    rig.census(&[], false);
    in_depot(&rig, idle)
}

#[test]
fn an_idle_depot_stack_is_laundered() {
    assert_eq!(an_idle_stack_after_three_intervals(true), 0);
    assert!(an_idle_stack_after_three_intervals(false) >= 20);
}

/// A depot stack of twenty objects, from which CPU 1 takes six and gives them back once every
/// interval for `intervals`, with another class's slow paths beating in between: the stack's
/// depth before each round, and how many of each round's allocations the stack could serve.
fn a_stack_in_use(shed_unused: bool, intervals: usize) -> (Vec<usize>, Vec<usize>) {
    let (used, busy) = (tiny(0), tiny(1));
    let tuning = Tuning {
        max_spares: 0,
        shed_unused,
        ..QUIET
    };
    let rig: TestRig = Rig::new(&[used, busy], 64, tuning);
    let zero = SimCpu::new(0);
    let filler = rig.cache(&zero);
    let held: Vec<_> = (0..21).filter_map(|_| filler.alloc(used)).collect();
    for object in held {
        filler.free(used, object);
    }
    let one = SimCpu::new(1);
    let cache = rig.cache(&one);
    let (mut depths, mut served) = (Vec::new(), Vec::new());
    let steps = 8;
    for _ in 0..intervals {
        let before = in_depot(&rig, used);
        depths.push(before);
        let objects: Vec<_> = (0..6).filter_map(|_| cache.alloc(used)).collect();
        served.push(before - in_depot(&rig, used));
        for object in objects {
            cache.free(used, object);
        }
        for _ in 0..steps {
            rig.os().advance_clock(IDLE_TICKS / steps as u64);
            for _ in 0..40 {
                churn(&cache, busy, 3);
            }
        }
    }
    rig.census(&[], false);
    (depths, served)
}

#[test]
fn a_depot_stack_in_use_sheds_only_its_unused_depth() {
    let intervals = 8;
    let (depths, served) = a_stack_in_use(true, intervals);
    // Each round takes two magazines from the stack (the third allocation comes from the one
    // CPU 1 keeps loaded) and gives two back. The rest is never used (sixteen objects, and the
    // magazine the idle check drains from CPU 0 after an interval) and is gone within a few
    // intervals; what each round uses stays.
    assert!(depths[0] >= 20, "{depths:?}");
    for &depth in &depths[3..] {
        assert_eq!(depth, 4, "{depths:?}");
    }
    for &taken in &served[1..] {
        assert_eq!(taken, 4, "the stack served every round: {served:?}");
    }
    let (kept, _) = a_stack_in_use(false, intervals);
    assert!(kept.iter().all(|&depth| depth >= 18), "{kept:?}");
}

/// `count` objects of one class allocated and then freed on CPU 0, with no spares, so that
/// every full magazine a free displaces retires to the depot or is refreshed: how many full
/// magazines retired, and how many of them were laundered.
fn refreshed_of(refresh_every: u64, count: usize) -> (usize, usize) {
    let class = tiny(0);
    let tuning = Tuning {
        max_spares: 0,
        refresh_every,
        ..QUIET
    };
    let rig: TestRig = Rig::new(&[class], count, tuning);
    let machine = SimCpu::new(0);
    let cache = rig.cache(&machine);
    let held: Vec<_> = (0..count).filter_map(|_| cache.alloc(class)).collect();
    assert_eq!(held.len(), count);
    assert!(rig.source.held().is_empty());
    for object in held {
        cache.free(class, object);
    }
    rig.census(&[], false);
    let capacity = class.magazine().capacity();
    let laundered = rig.source.held().len() / capacity;
    let area = cache.area();
    let kept = rig
        .depot
        .full(super::Domain::ZERO, class)
        .magazines(area, class.magazine())
        .len();
    (kept + laundered, laundered)
}

#[test]
fn the_refresh_launders_one_retiring_magazine_in_every_few() {
    let (retired, laundered) = refreshed_of(REFRESH_EVERY, 400);
    assert!(retired >= 150, "{retired}");
    assert_eq!(laundered, retired / REFRESH_EVERY as usize);
    assert_eq!(refreshed_of(0, 400).1, 0);
}

/// The objects of `class` on CPU `n`'s spare stacks.
fn in_spares(rig: &TestRig, n: u32, class: SmallClass) -> std::collections::BTreeSet<usize> {
    struct Spares(u32, SmallClass, std::collections::BTreeSet<usize>);
    impl Visitor for Spares {
        fn cached(&mut self, place: Place, class: SmallClass, object: *const u8) {
            if place == Place::Spare(self.0) && class == self.1 {
                self.2.insert(object.addr());
            }
        }
    }
    let mut seen = Spares(n, class, std::collections::BTreeSet::new());
    inspect(&rig.depot, rig.cell.get().unwrap(), rig.slabs(), &mut seen);
    seen.2
}

/// A pipeline's consumer: CPU 1 allocates objects of a class, CPU 0 frees them, for three
/// intervals. CPU 0's full spares are pushed and never popped there, and its loaded magazine is
/// replaced on every exchange, so it never grows old. Whether any object that was on CPU 0's
/// spares at the start is still there at the end.
fn a_consumers_first_spares_stay(age_limit: bool) -> bool {
    let class = tiny(0);
    let tuning = Tuning {
        max_spares: 2,
        age_limit,
        ..QUIET
    };
    let rig: TestRig = Rig::new(&[class], 4096, tuning);
    let (zero, one) = (SimCpu::new(0), SimCpu::new(1));
    let (consumer, producer) = (rig.cache(&zero), rig.cache(&one));
    let round = || {
        let objects: Vec<_> = (0..12).filter_map(|_| producer.alloc(class)).collect();
        for object in objects {
            consumer.free(class, object);
        }
    };
    round();
    let first = in_spares(&rig, 0, class);
    assert!(!first.is_empty());
    let steps = 8;
    for _ in 0..3 * steps {
        rig.os().advance_clock(IDLE_TICKS / steps);
        for _ in 0..20 {
            round();
        }
    }
    let last = in_spares(&rig, 0, class);
    rig.census(&[], false);
    !first.is_disjoint(&last)
}

#[test]
fn a_consumers_full_spares_are_swept() {
    assert!(!a_consumers_first_spares_stay(true));
    assert!(a_consumers_first_spares_stay(false));
}

/// A class that falls out of use on CPU 0 just after an allocation emptied its loaded magazine,
/// with full spares behind it, while CPU 0 stays busy with another class for three intervals:
/// how many of its objects CPU 0's spares hold at the end.
fn spares_behind_an_empty_magazine(age_limit: bool) -> usize {
    let (idle, busy) = (tiny(0), tiny(1));
    let tuning = Tuning {
        max_spares: 2,
        age_limit,
        ..QUIET
    };
    let rig: TestRig = Rig::new(&[idle, busy], 64, tuning);
    let machine = SimCpu::new(0);
    let cache = rig.cache(&machine);
    let held: Vec<_> = (0..8).filter_map(|_| cache.alloc(idle)).collect();
    for object in held {
        cache.free(idle, object);
    }
    // Empty the loaded magazine, which is full after the frees, on the fast path.
    let user: Vec<_> = (0..2).filter_map(|_| cache.alloc(idle)).collect();
    assert!(is_loaded(&rig, 0, idle));
    assert!(!in_spares(&rig, 0, idle).is_empty());
    let steps = 8;
    for _ in 0..3 * steps {
        rig.os().advance_clock(IDLE_TICKS / steps);
        for _ in 0..40 {
            churn(&cache, busy, 3);
        }
    }
    rig.census(&user, false);
    in_spares(&rig, 0, idle).len()
}

/// With drains off, `collect` still empties the caller's own slab, through its routines, and
/// launders the depot; another CPU's slab it cannot touch.
#[test]
fn collect_empties_its_own_slab_without_a_drain() {
    let class = tiny(0);
    let rig: TestRig = Rig::new(
        &[class],
        64,
        Tuning {
            max_spares: 2,
            ..QUIET
        },
    );
    rig.os().set_faults(
        crate::os::sim::Call::WipeOnFork,
        crate::os::sim::Faults::ALWAYS,
    );
    for n in [1, 0] {
        let machine = SimCpu::new(n);
        churn(&rig.cache(&machine), class, 9);
    }
    let before = parked(&rig, class);
    assert!(before.on_cpu[0] > 0 && before.on_cpu[1] > 0 && before.depot > 0);
    let machine = SimCpu::new(0);
    rig.cache(&machine).collect();
    let after = parked(&rig, class);
    assert_eq!((after.on_cpu[0], after.depot), (0, 0));
    assert_eq!(after.on_cpu[1], before.on_cpu[1]);
    assert_eq!(rig.os().calls(crate::os::sim::Call::Fence), 0);
    rig.census(&[], false);
}

#[test]
fn spares_behind_an_empty_loaded_magazine_are_swept() {
    assert_eq!(spares_behind_an_empty_magazine(true), 0);
    assert!(spares_behind_an_empty_magazine(false) > 0);
}

/// A rig whose two CPUs are in cache domains of their own (every simulated CPU its own L3).
fn two_domains(classes: &[SmallClass], count: usize, tuning: Tuning) -> TestRig {
    let os = SimOs::new();
    os.set_topology(crate::os::sim::SimTopology::Blocks(1));
    Rig::with_os(os, classes, count, tuning)
}

/// Objects of `class` in the depot stack of `domain`.
fn in_domain(rig: &TestRig, domain: usize, class: SmallClass) -> usize {
    let machine = SimCpu::new(0);
    let cache = rig.cache(&machine);
    rig.depot
        .full(super::Domain::folded(domain), class)
        .objects(cache.area(), class.magazine()) as usize
}

/// Objects of `class` that CPU `n` allocates and frees again, which with no spares sends its
/// full magazines to the depot.
fn free_into_depot(rig: &TestRig, n: i32, class: SmallClass, count: usize) {
    let machine = SimCpu::new(n);
    let cache = rig.cache(&machine);
    let held: Vec<_> = (0..count).filter_map(|_| cache.alloc(class)).collect();
    assert_eq!(held.len(), count);
    for object in held {
        cache.free(class, object);
    }
}

/// The depot keeps what a CPU frees in that CPU's domain, and a domain whose CPUs go quiet
/// sheds its stacks within the intervals a quiet class's stack sheds in, through the looks and
/// sheds of the other domain's slow paths.
#[test]
fn a_quiet_domain_gives_its_stacks_back() {
    let (idle, busy) = (tiny(0), tiny(1));
    for shed_unused in [true, false] {
        let tuning = Tuning {
            max_spares: 0,
            shed_unused,
            ..QUIET
        };
        let rig = two_domains(&[idle, busy], 64, tuning);
        free_into_depot(&rig, 0, idle, 22);
        assert_eq!(rig.depot.domain_count(), super::MAX_DOMAINS);
        assert!(in_domain(&rig, 0, idle) >= 20);
        assert_eq!(
            in_domain(&rig, 1, idle),
            0,
            "kept in the freeing CPU's domain"
        );
        let one = SimCpu::new(1);
        let other = rig.cache(&one);
        let steps = 8;
        for _ in 0..3 * steps {
            rig.os().advance_clock(IDLE_TICKS / steps);
            for _ in 0..40 {
                churn(&other, busy, 3);
            }
        }
        rig.census(&[], false);
        if shed_unused {
            assert_eq!(in_domain(&rig, 0, idle), 0);
        } else {
            assert!(in_domain(&rig, 0, idle) >= 20);
        }
    }
}

/// A slow path whose domain's stack is empty takes from another domain's stack only when that
/// holds at least the take threshold; otherwise it fills from the source.
#[test]
fn a_slow_path_takes_across_domains_only_from_a_stack_at_its_threshold() {
    let class = tiny(0);
    for (parked, takes) in [(4, false), (12, true)] {
        let tuning = Tuning {
            max_spares: 0,
            take_across: 10 * class.size(),
            ..QUIET
        };
        let rig = two_domains(&[class], 64, tuning);
        free_into_depot(&rig, 0, class, parked);
        let before = in_domain(&rig, 0, class);
        let one = SimCpu::new(1);
        let consumer = rig.cache(&one);
        let object = consumer.alloc(class).unwrap();
        let after = in_domain(&rig, 0, class);
        assert_eq!(
            after < before,
            takes,
            "parked {parked}: {before} then {after}"
        );
        consumer.free(class, object);
        rig.census(&[], false);
    }
}

/// `collect` launders the depot stacks of every domain, and `maintain` sheds them all.
#[test]
fn collect_and_maintain_reach_every_domain() {
    let class = tiny(0);
    let tuning = Tuning {
        max_spares: 0,
        shed_unused: true,
        ..QUIET
    };
    for maintain in [false, true] {
        let rig = two_domains(&[class], 64, tuning);
        free_into_depot(&rig, 0, class, 9);
        free_into_depot(&rig, 1, class, 9);
        assert!(in_domain(&rig, 0, class) > 0 && in_domain(&rig, 1, class) > 0);
        let machine = SimCpu::new(0);
        let cache = rig.cache(&machine);
        if maintain {
            for _ in 0..3 {
                cache.maintain();
                rig.os().advance_clock(IDLE_TICKS);
            }
        } else {
            cache.collect();
        }
        assert_eq!(in_domain(&rig, 0, class), 0, "maintain {maintain}");
        assert_eq!(in_domain(&rig, 1, class), 0, "maintain {maintain}");
        rig.census(&[], false);
    }
}
