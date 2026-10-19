//! Deterministic tests of the rules that send parked objects back to the source: the magazine
//! age limit (`age.rs`), the depot's low-water rule (`shed.rs`) and the refresh fraction
//! (`slow.rs`), on the test rig with the simulated clock. Each runs its scenario with the rule
//! on and with it off, and checks that the outcome the rule promises happens only when it is
//! on, so each test fails if its rule stops working.

use super::test_rig::{Rig, SimCpu, Small, TestSource};
use super::{Cache, Domain, IDLE_TICKS, Pool, REFRESH_EVERY, Source, Tuning, inspect};
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
/// a depot target no test reaches, looks at the allocator's rate (a test that states how soon
/// something happens must not look more often than the allocator does), and no rule on.
const QUIET: Tuning = Tuning {
    spare_budget: usize::MAX / 2,
    max_spares: 1,
    depot_target: usize::MAX / 2,
    domain_cap: usize::MAX / 4,
    look_every: super::LOOK_EVERY,
    refresh_every: 0,
    age_limit: false,
    shed_unused: false,
    stock: false,
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
        .full(Pool::Domain(Domain::ZERO), class)
        .count(cache.area(), class.magazine()) as usize
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
        .full(Pool::Domain(Domain::ZERO), class)
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
    // A look on every slow path, so that sweeps begin as soon as they are due. The objects
    // circulate between the CPUs and come back onto the spares, so the outcome depends on when
    // the sweeps fall; this checks that the sweep takes the stack, not how soon.
    let tuning = Tuning {
        max_spares: 2,
        age_limit,
        look_every: 1,
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

/// A rig of `N` CPUs in which each `width` consecutive simulated CPUs share an L3, which makes
/// `SIM_THREADS / width` cache domains.
fn domains<const N: usize>(
    width: u32,
    classes: &[SmallClass],
    count: usize,
    tuning: Tuning,
) -> Rig<SimOs<Small>, N> {
    let os = SimOs::new();
    os.set_topology(crate::os::sim::SimTopology::Blocks(width));
    Rig::with_os(os, classes, count, tuning)
}

fn domain(n: usize) -> Pool {
    Pool::Domain(Domain::folded(n))
}

/// Objects of `class` in `pool`'s stack.
fn in_pool<const N: usize>(rig: &Rig<SimOs<Small>, N>, pool: Pool, class: SmallClass) -> usize {
    let machine = SimCpu::new(0);
    let cache = rig.cache(&machine);
    rig.depot
        .full(pool, class)
        .count(cache.area(), class.magazine()) as usize
}

/// Objects of `class` that CPU `n` allocates and frees again, which with no spares sends its
/// full magazines to the depot.
fn free_into_depot<const N: usize>(
    rig: &Rig<SimOs<Small>, N>,
    n: i32,
    class: SmallClass,
    count: usize,
) {
    let machine = SimCpu::new(n);
    let cache = rig.cache(&machine);
    let held: Vec<_> = (0..count).filter_map(|_| cache.alloc(class)).collect();
    assert_eq!(held.len(), count);
    for object in held {
        cache.free(class, object);
    }
}

/// Every cache domain but CPU 0's parks objects of two classes and goes quiet, while CPU 0
/// runs another class's slow paths. `width` CPUs share each domain. When, in ticks from the
/// start, the quiet domains' stacks were empty, and when every object of the two classes was
/// back in the source, if within ten intervals, at the allocator's look rate.
fn quiet_domains_emptied(width: u32, shed_unused: bool) -> (Option<u64>, Option<u64>) {
    let (a, b, busy) = (tiny(0), tiny(1), tiny(2));
    let tuning = Tuning {
        max_spares: 0,
        shed_unused,
        look_every: super::LOOK_EVERY,
        ..QUIET
    };
    let objects = 512;
    let rig: Rig<SimOs<Small>, 64> = domains(width, &[a, b, busy], objects, tuning);
    let count = crate::os::sim::SIM_THREADS / width as usize;
    for d in 1..count {
        let cpu = (d * width as usize) as i32;
        free_into_depot(&rig, cpu, a, 5);
        free_into_depot(&rig, cpu, b, 3);
    }
    assert_eq!(rig.depot.domain_count(), count);
    let in_stacks = || {
        (1..count)
            .map(|d| in_pool(&rig, domain(d), a) + in_pool(&rig, domain(d), b))
            .sum::<usize>()
    };
    assert!(in_stacks() >= (count - 1) * 4, "{}", in_stacks());
    let zero = SimCpu::new(0);
    let cache = rig.cache(&zero);
    let start = rig.os().ticks();
    let (mut stacks, mut all) = (None, None);
    let steps = 8;
    for _ in 0..10 * steps {
        rig.os().advance_clock(IDLE_TICKS / steps);
        for _ in 0..40 {
            churn(&cache, busy, 3);
        }
        let now = rig.os().ticks() - start;
        if stacks.is_none() && in_stacks() == 0 {
            stacks = Some(now);
        }
        if rig.source.held_of(a) == objects && rig.source.held_of(b) == objects {
            all = Some(now);
            break;
        }
    }
    rig.census(&[], false);
    (stacks, all)
}

/// A quiet domain's stacks shed as a quiet class's stack does, through the looks and sheds of
/// another domain's slow paths, whether there are 2, 8 or 64 domains, at the allocator's look
/// rate and one busy CPU's 640 slow paths an interval. A stack sheds the depth it did not use in
/// an interval: its debt is set at the first look in the interval after one in which nothing
/// took from it, and paid by the heartbeats after that. The quiet CPUs' slabs are drained into
/// the stacks an interval after the busy CPU's idle check first sees them still, and that check
/// visits one slab a look, so with 63 quiet CPUs the last drains come in the fourth interval and
/// their magazines shed in the sixth or seventh: 2, 4 and 6.4 intervals with 2, 8 and 64
/// domains.
#[test]
fn a_quiet_domain_gives_its_stacks_back_with_2_8_and_64_domains() {
    for width in [32, 8, 1] {
        let (stacks, all) = quiet_domains_emptied(width, true);
        let stacks = stacks.expect("the quiet domains' stacks are emptied");
        let all = all.expect("everything goes back");
        std::eprintln!(
            "{} domains: stacks empty after {:.3} intervals, all back after {:.3}",
            crate::os::sim::SIM_THREADS / width as usize,
            stacks as f64 / IDLE_TICKS as f64,
            all as f64 / IDLE_TICKS as f64
        );
        assert!(stacks <= all, "{width}: {stacks} {all}");
        assert!(all <= 7 * IDLE_TICKS, "{width}: {all}");
        assert_eq!(quiet_domains_emptied(width, false), (None, None), "{width}");
    }
}

/// A slow path takes from no other domain's stack, only from the shared stack, which holds
/// what a domain's stack had no room for.
#[test]
fn a_domain_overflows_into_the_shared_stack_and_others_take_only_from_that() {
    let class = tiny(0);
    for (parked, overflows) in [(4, false), (12, true)] {
        let tuning = Tuning {
            max_spares: 0,
            domain_cap: 4 * class.size(),
            ..QUIET
        };
        let rig: TestRig = domains(1, &[class], 64, tuning);
        free_into_depot(&rig, 0, class, parked);
        let own = in_pool(&rig, domain(0), class);
        let shared = in_pool(&rig, Pool::Shared, class);
        assert!(own > 0 && own < 4 + class.magazine().capacity(), "{own}");
        assert_eq!(shared > 0, overflows, "{parked}: {own} {shared}");
        let one = SimCpu::new(1);
        let consumer = rig.cache(&one);
        let fills = rig.source.fills();
        let object = consumer.alloc(class).unwrap();
        assert_eq!(in_pool(&rig, domain(0), class), own, "{parked}");
        assert_eq!(in_pool(&rig, Pool::Shared, class) < shared, overflows);
        assert_eq!(rig.source.fills() > fills, !overflows, "{parked}");
        consumer.free(class, object);
        rig.census(&[], false);
    }
}

/// `collect` launders every stack, the domains' and the shared one, and `maintain` sheds them
/// all.
#[test]
fn collect_and_maintain_empty_every_stack() {
    let class = tiny(0);
    let tuning = Tuning {
        max_spares: 0,
        domain_cap: 2 * class.size(),
        shed_unused: true,
        ..QUIET
    };
    for maintain in [false, true] {
        let rig: TestRig = domains(1, &[class], 64, tuning);
        free_into_depot(&rig, 0, class, 9);
        free_into_depot(&rig, 1, class, 9);
        let pools = [domain(0), domain(1), Pool::Shared];
        assert!(pools.iter().all(|&pool| in_pool(&rig, pool, class) > 0));
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
        for pool in pools {
            assert_eq!(
                in_pool(&rig, pool, class),
                0,
                "maintain {maintain}: {pool:?}"
            );
        }
        rig.census(&[], false);
    }
}

/// A class in use in four domains parks at most a cap in each and the shared stack's target:
/// the domain stacks never pass their caps by more than the magazine that reached them, and the
/// heartbeat launders the shared stack down to its target.
#[test]
fn four_domains_in_use_park_at_most_four_caps_and_the_shared_target() {
    let class = tiny(0);
    let (target, cap) = (12, 6);
    let tuning = Tuning {
        max_spares: 0,
        depot_target: target * class.size(),
        domain_cap: cap * class.size(),
        ..QUIET
    };
    let rig: Rig<SimOs<Small>, 4> = domains(1, &[class], 256, tuning);
    for _ in 0..target {
        for n in 0..4 {
            free_into_depot(&rig, n, class, 30);
        }
    }
    let magazine = class.magazine().capacity();
    let each: Vec<usize> = (0..4).map(|d| in_pool(&rig, domain(d), class)).collect();
    let shared = in_pool(&rig, Pool::Shared, class);
    assert!(
        each.iter().all(|&n| n > 0 && n < cap + magazine),
        "{each:?}"
    );
    assert!(shared <= target - cap + magazine, "{shared}");
    rig.census(&[], false);
}

/// A flow of a class from one domain to another, objects freed in one and allocated in the
/// other, goes through the shared stack once the freeing domain's stack is at its cap, whether
/// no other domain, one or sixty-two others also park the class: in steady state the
/// allocating side fills nothing from the source.
#[test]
fn a_flow_between_two_domains_is_not_stopped_by_others() {
    let class = tiny(0);
    let target = 14;
    let tuning = Tuning {
        max_spares: 0,
        depot_target: target * class.size(),
        domain_cap: target / 2 * class.size(),
        ..QUIET
    };
    for others in [0, 1, 62] {
        let rig: Rig<SimOs<Small>, 64> = domains(1, &[class], 1024, tuning);
        for n in 0..others {
            free_into_depot(&rig, 2 + n, class, 4);
        }
        let parked: usize = (0..others)
            .map(|n| in_pool(&rig, domain(2 + n as usize), class))
            .sum();
        assert!(parked >= others as usize, "{others}: {parked}");
        let (zero, one) = (SimCpu::new(0), SimCpu::new(1));
        let (freer, allocator) = (rig.cache(&zero), rig.cache(&one));
        let round = || {
            let objects: Vec<_> = (0..6).map(|_| allocator.alloc(class).unwrap()).collect();
            for object in objects {
                freer.free(class, object);
            }
        };
        for _ in 0..10 {
            round();
        }
        let shared = rig.depot.full(Pool::Shared, class).version();
        let fills = rig.source.fills();
        for _ in 0..40 {
            round();
        }
        assert_eq!(rig.source.fills(), fills, "{others} others");
        assert_ne!(rig.depot.full(Pool::Shared, class).version(), shared);
        rig.census(&[], false);
    }
}

/// Two domains that each allocate and free what they need, within their stacks' caps, keep
/// their magazines to themselves: in steady state neither touches a shared stack, and neither
/// fills from the source.
#[test]
fn balanced_domains_do_not_touch_the_shared_stacks() {
    let class = tiny(0);
    let tuning = Tuning {
        max_spares: 0,
        depot_target: 16 * class.size(),
        domain_cap: 8 * class.size(),
        ..QUIET
    };
    let rig: TestRig = domains(1, &[class], 256, tuning);
    let (zero, one) = (SimCpu::new(0), SimCpu::new(1));
    let caches = [rig.cache(&zero), rig.cache(&one)];
    let round = || {
        for cache in &caches {
            churn(cache, class, 6);
        }
    };
    for _ in 0..5 {
        round();
    }
    let area = caches[0].area();
    let versions = || {
        (
            rig.depot.full(Pool::Shared, class).version(),
            rig.depot.empty(Pool::Shared, class.magazine()).version(),
        )
    };
    let (before, fills) = (versions(), rig.source.fills());
    for _ in 0..50 {
        round();
    }
    assert_eq!(versions(), before);
    assert_eq!(rig.source.fills(), fills);
    assert!(in_pool(&rig, domain(0), class) > 0 && in_pool(&rig, domain(1), class) > 0);
    assert_eq!(
        rig.depot
            .full(Pool::Shared, class)
            .count(area, class.magazine()),
        0
    );
    rig.census(&[], false);
}

/// A forked child reaches every stack the parent filled, in every domain and the shared pool:
/// its heartbeats shed them, through its looks at other domains, and its `collect` launders
/// them.
#[test]
fn a_forked_child_reaches_every_stack_the_parent_filled() {
    let (idle, busy) = (tiny(0), tiny(1));
    let tuning = Tuning {
        max_spares: 0,
        domain_cap: 2 * idle.size(),
        shed_unused: true,
        ..QUIET
    };
    for collect in [false, true] {
        let objects = 256;
        let rig: Rig<SimOs<Small>, 64> = domains(8, &[idle, busy], objects, tuning);
        let pools: Vec<Pool> = (0..8).map(domain).chain([Pool::Shared]).collect();
        // Not from CPU 0, where the child runs with the age limit off: its slab would keep a
        // loaded magazine of the class.
        for d in 0..8 {
            free_into_depot(&rig, d * 8 + 1, idle, 7);
        }
        assert!(
            pools.iter().all(|&pool| in_pool(&rig, pool, idle) > 0),
            "{:?}",
            pools
                .iter()
                .map(|&pool| in_pool(&rig, pool, idle))
                .collect::<Vec<_>>()
        );
        // SAFETY: one thread, outside the allocator.
        unsafe { rig.os().wipe_for_fork() };
        let child = SimCpu::new(0);
        let cache = rig.cache(&child);
        if collect {
            cache.collect();
        } else {
            let steps = 8;
            for _ in 0..4 * steps {
                rig.os().advance_clock(IDLE_TICKS / steps);
                for _ in 0..40 {
                    churn(&cache, busy, 3);
                }
            }
        }
        for &pool in &pools {
            assert_eq!(in_pool(&rig, pool, idle), 0, "collect {collect}: {pool:?}");
        }
        if !collect {
            // The parent's slabs, drained by the child's idle checks, have gone back too.
            assert_eq!(rig.source.held_of(idle), objects);
        }
        rig.census(&[], false);
    }
}

/// The classes with the smallest magazines but the `skip` largest, `count` of them.
fn tiny_classes(skip: usize, count: usize) -> Vec<SmallClass> {
    (skip..skip + count).map(tiny).collect()
}

/// Every cache domain but CPU 0's holds one magazine of each of `held` classes in its stack and
/// is quiet (nothing in its slabs), while CPU 0 runs another class's slow paths at the rate of
/// `a_quiet_domain_gives_its_stacks_back_with_2_8_and_64_domains`, looking once in
/// `look_every` of them. `width` CPUs share each domain. Returns how many of the quiet domains'
/// stacks still held a magazine after each interval, for `intervals` of them, and how many slow
/// paths CPU 0 ran.
fn quiet_stacks_left(
    width: u32,
    held: usize,
    look_every: u64,
    intervals: usize,
) -> (Vec<usize>, u64) {
    let busy = tiny(0);
    let classes = tiny_classes(1, held);
    let tuning = Tuning {
        max_spares: 0,
        shed_unused: true,
        look_every,
        ..QUIET
    };
    let mut all = classes.clone();
    all.push(busy);
    let rig: Rig<SimOs<Small>, 64> = domains(width, &all, 512, tuning);
    let zero = SimCpu::new(0);
    let cache = rig.cache(&zero);
    churn(&cache, busy, 3);
    let count = crate::os::sim::SIM_THREADS / width as usize;
    assert_eq!(rig.depot.domain_count(), count);
    for d in 1..count {
        for &class in &classes {
            let magazine = cache.area().create(class.magazine()).unwrap();
            for slot in 1..=2 {
                magazine.set_object(slot, rig.source.alloc_one(class).unwrap());
            }
            magazine.set_level(2);
            cache.to_domain(Domain::folded(d), class, magazine);
        }
    }
    let left = || {
        (1..count)
            .flat_map(|d| classes.iter().map(move |&class| (d, class)))
            .filter(|&(d, class)| in_pool(&rig, domain(d), class) > 0)
            .count()
    };
    assert_eq!(left(), (count - 1) * held);
    let before = rig.slabs().slab(cpu0(&rig)).activity();
    let steps = 8;
    let mut after_each = Vec::new();
    for _ in 0..intervals {
        for _ in 0..steps {
            rig.os().advance_clock(IDLE_TICKS / steps);
            for _ in 0..40 {
                churn(&cache, busy, 3);
            }
        }
        after_each.push(left());
    }
    rig.census(&[], false);
    (after_each, rig.slabs().slab(cpu0(&rig)).activity() - before)
}

fn cpu0<const N: usize>(rig: &Rig<SimOs<Small>, N>) -> Cpu {
    rig.slabs().cpu(0).unwrap()
}

/// Review: how long a quiet domain's stacks take to go back grows with how many stacks the other
/// domains hold, because each look visits one of them and a stack needs two looks in different
/// intervals before it owes anything. The test of 2, 8 and 64 domains above looks on every slow
/// path, sixteen times the allocator's rate. At the allocator's rate and the same pace (640
/// slow paths an interval on the one busy CPU), 63 quiet domains holding five classes each take
/// 16 intervals to empty and with two classes 7; at ee4c29b, where a look read one class in
/// every domain, both took 3.
#[test]

fn review_quiet_domains_go_back_at_the_allocators_look_rate() {
    let mut slowest = 0;
    for (width, held) in [(32, 2), (8, 2), (8, 5), (1, 2), (1, 5)] {
        for look_every in [1, super::LOOK_EVERY] {
            let (left, slow_paths) = quiet_stacks_left(width, held, look_every, 24);
            let emptied = left
                .iter()
                .position(|&n| n == 0)
                .map_or(usize::MAX, |i| i + 1);
            std::eprintln!(
                "{} domains, {held} classes each, a look every {look_every}: {} stacks, \
                 empty after {emptied} intervals ({} slow paths an interval)",
                crate::os::sim::SIM_THREADS / width as usize,
                (crate::os::sim::SIM_THREADS / width as usize - 1) * held,
                slow_paths / 24,
            );
            slowest = slowest.max(emptied);
        }
    }
    assert!(slowest <= 4, "{slowest} intervals");
}

/// CPUs 1 to 15 park a class in their slabs, their domain's stack and the shared stack and go
/// quiet; CPU 0, in the same domain, runs another class's slow paths, and its idle checks drain
/// their slabs into the depot. The objects of the class on the shared stack after each eighth
/// of an interval, and the shared stack's target.
fn shared_depth_after_drains(shed_unused: bool, intervals: usize) -> (Vec<usize>, usize) {
    let (class, busy) = (tiny(0), tiny(1));
    let magazine = class.magazine().capacity();
    let tuning = Tuning {
        max_spares: 2,
        depot_target: 2 * magazine * class.size(),
        domain_cap: magazine * class.size(),
        shed_unused,
        ..QUIET
    };
    let rig: Rig<SimOs<Small>, 16> = Rig::new(&[class, busy], 256, tuning);
    for n in 1..16 {
        let quiet = SimCpu::new(n);
        churn(&rig.cache(&quiet), class, 6 * magazine);
    }
    let zero = SimCpu::new(0);
    let cache = rig.cache(&zero);
    let target = cache.shared_target(class);
    assert_eq!(target, magazine);
    let shared = || in_pool(&rig, Pool::Shared, class);
    // The pushes of the CPUs' own slow paths were each followed by the check.
    assert!(shared() <= target + magazine, "{}", shared());
    let steps = 8;
    let mut after_each = Vec::new();
    for _ in 0..intervals * steps {
        rig.os().advance_clock(IDLE_TICKS / steps as u64);
        for _ in 0..40 {
            churn(&cache, busy, 3);
        }
        after_each.push(shared());
    }
    rig.census(&[], false);
    (after_each, target)
}

/// Review: the heartbeat looks at a shared stack's target only in a slow path that pushed on
/// it, and before that slow path's own sweep and drain. What a drain parks there (and the rest
/// of a swept spare stack) is therefore never checked. With nothing else pushing the class, the
/// stack stays over its target until the low-water rule sheds the whole of it, an interval or
/// two later, and for good where that rule is off. The design says the heartbeat launders the
/// shared stack while it is over its target, and that a class parks at most the domains' caps
/// and the shared target.
#[test]

fn review_a_shared_stack_that_drains_fill_is_laundered_to_its_target() {
    let magazine = tiny(0).magazine().capacity();
    let (with_shed, target) = shared_depth_after_drains(true, 5);
    let (without, _) = shared_depth_after_drains(false, 5);
    std::eprintln!(
        "target {target}; shared after each eighth of an interval: {with_shed:?}; \
         with the low-water rule off: {without:?}"
    );
    for depths in [with_shed, without] {
        assert!(
            depths.iter().all(|&depth| depth <= target + magazine),
            "target {target}: {depths:?}"
        );
    }
}

/// A look from another domain leaves alone a domain whose own CPUs looked at its stacks in the
/// same interval, since their looks shed it, and so reads none of its lines; it looks at every
/// stack of a domain that has not.
#[test]
fn a_look_from_elsewhere_skips_a_domain_that_looked_this_interval() {
    let class = tiny(0);
    let rig: TestRig = domains(1, &[class], 64, QUIET);
    free_into_depot(&rig, 1, class, 6);
    let (zero, one) = (SimCpu::new(0), SimCpu::new(1));
    let (cache, other) = (rig.cache(&zero), rig.cache(&one));
    let (here, there) = (Domain::folded(0), Domain::folded(1));
    let water = || {
        rig.depot
            .water(
                Pool::Domain(there),
                class.ordinal() % crate::classes::SMALL_COUNT,
            )
            .load(Relaxed)
    };
    rig.os().advance_clock(5 * IDLE_TICKS);
    let now = rig.os().ticks();
    other.note_looked(there, now);
    let before = water();
    cache.look_elsewhere(here, 0, now);
    assert_eq!(
        water(),
        before,
        "a domain that looked this interval is left alone"
    );
    rig.os().advance_clock(IDLE_TICKS);
    cache.look_elsewhere(here, 0, rig.os().ticks());
    assert_ne!(water(), before, "a domain that has not is looked at");
    rig.census(&[], false);
}

/// What a run of bursts of one class did: CPU `alloc` allocates `burst` objects, CPU `free`
/// frees them all, `rounds` times.
struct Bursts {
    /// Pushes and pops of the domain's full and empty stacks of the class in the last round.
    depot_last_round: u32,
    /// The class's credits on each CPU at the end.
    stock: [u64; 2],
    /// The most objects the CPUs' spares held at the end of any round, each CPU apart.
    spares_most: [usize; 2],
}

/// `credit`, if not zero, is given to the class on both CPUs before the first round, as a
/// contended round trip would earn it.
fn bursts(stock: bool, alloc: i32, free: i32, burst: usize, rounds: usize, credit: u32) -> Bursts {
    let class = tiny(0);
    let tuning = Tuning { stock, ..QUIET };
    let rig: TestRig = Rig::new(&[class], 4 * burst, tuning);
    if credit > 0 {
        for n in 0..2 {
            let slab = rig.slabs().slab(cpu(&rig, n));
            slab.stock(class)
                .store(super::stock::credits(credit, credit), Relaxed);
        }
    }
    let (a, f) = (SimCpu::new(alloc), SimCpu::new(free));
    let (allocator, freer) = (rig.cache(&a), rig.cache(&f));
    let pool = Pool::Domain(Domain::ZERO);
    let versions = || {
        rig.depot
            .full(pool, class)
            .version()
            .wrapping_add(rig.depot.empty(pool, class.magazine()).version())
    };
    let mut seen = Bursts {
        depot_last_round: 0,
        stock: [0; 2],
        spares_most: [0; 2],
    };
    for _ in 0..rounds {
        let before = versions();
        let objects: Vec<_> = (0..burst).filter_map(|_| allocator.alloc(class)).collect();
        assert_eq!(objects.len(), burst);
        for object in objects {
            freer.free(class, object);
        }
        seen.depot_last_round = versions().wrapping_sub(before);
        for n in 0..2 {
            let held = in_spares(&rig, n, class).len();
            seen.spares_most[n as usize] = seen.spares_most[n as usize].max(held);
        }
    }
    for n in 0..2 {
        let slab = rig.slabs().slab(cpu(&rig, n));
        // The credits: a CPU that sent magazines away may have its marks set.
        seen.stock[n as usize] = slab.stock(class).load(Relaxed).addr() as u64 & 0x3fff_ffff;
    }
    rig.census(&[], false);
    seen
}

/// A thread that allocates a burst and frees it on one CPU sends magazines to the depot and
/// takes them back. With no other thread on the stack, nothing contends, and the CPU keeps no
/// stock: the request handler on a quiet server, whose exchanges are cheap.
#[test]
fn a_cpu_that_round_trips_on_an_uncontended_stack_keeps_no_stock() {
    let without = bursts(false, 0, 0, 40, 12, 0);
    let with = bursts(true, 0, 0, 40, 12, 0);
    assert!(
        without.depot_last_round >= 30,
        "{}",
        without.depot_last_round
    );
    assert_eq!(with.stock, [0, 0]);
    assert_eq!(with.depot_last_round, without.depot_last_round);
    assert_eq!(with.spares_most, without.spares_most);
}

/// A CPU whose class has earned credit keeps its bursts: the depot sees none of them.
#[test]
fn a_cpu_with_credit_keeps_its_bursts() {
    let without = bursts(false, 0, 0, 40, 12, 0);
    let with = bursts(true, 0, 0, 40, 12, 40);
    assert_eq!(with.depot_last_round, 0);
    assert!(with.spares_most[0] > without.spares_most[0]);
}

/// A pipeline (one CPU only allocates the class, the other only frees it) never takes back a
/// kind of magazine it sent away, so stock changes nothing: the same depot traffic, the same
/// spares and no credit on either CPU. This is the property that keeps `pipeline` and `mpmc`
/// jobs as they were.
#[test]
fn a_one_way_flow_behaves_as_without_stock() {
    for (alloc, free) in [(1, 0), (0, 1)] {
        let without = bursts(false, alloc, free, 40, 12, 0);
        let with = bursts(true, alloc, free, 40, 12, 0);
        assert_eq!(with.stock, [0, 0]);
        assert_eq!(with.depot_last_round, without.depot_last_round);
        assert_eq!(with.spares_most, without.spares_most);
    }
}

/// Stock is bounded by the spare budget less what the fixed limits could park: a CPU that
/// round-trips a burst far bigger than that keeps no more in its spares than its fixed limit and
/// that room.
#[test]
fn stock_stays_within_the_spare_budget() {
    let class = tiny(0);
    let tuning = Tuning {
        stock: true,
        spare_budget: 8 << 20,
        ..QUIET
    };
    let fixed = super::spare_limit(tuning, class, crate::front::Spare::Full);
    let bound = (fixed + super::stock::room(tuning) / (2 * class.size())) * 2;
    let burst = 4 * bound;
    let rig: TestRig = Rig::new(&[class], 2 * burst, tuning);
    let slab = rig.slabs().slab(cpu(&rig, 0));
    slab.stock(class)
        .store(super::stock::credits(u32::MAX, u32::MAX), Relaxed);
    let machine = SimCpu::new(0);
    let cache = rig.cache(&machine);
    for _ in 0..8 {
        churn(&cache, class, burst);
        let held = in_spares(&rig, 0, class).len();
        assert!(held <= bound, "{held} > {bound}");
    }
    assert!(in_spares(&rig, 0, class).len() > 2, "stock grew");
    rig.census(&[], false);
}

/// A class that stops round-tripping while the CPU stays busy with another loses its stock: the
/// sweep takes the unused depth off its credit and its spares with it.
#[test]
fn stock_unused_for_an_interval_goes_back() {
    let (bursty, busy) = (tiny(0), tiny(1));
    let tuning = Tuning {
        stock: true,
        age_limit: true,
        look_every: 1,
        ..QUIET
    };
    let rig: TestRig = Rig::new(&[bursty, busy], 256, tuning);
    let slab = rig.slabs().slab(cpu(&rig, 0));
    slab.stock(bursty)
        .store(super::stock::credits(40, 40), Relaxed);
    let machine = SimCpu::new(0);
    let cache = rig.cache(&machine);
    for _ in 0..12 {
        churn(&cache, bursty, 40);
    }
    assert_ne!(slab.stock(bursty).load(Relaxed).addr(), 0);
    assert!(in_spares(&rig, 0, bursty).len() > 2);
    let steps = 8;
    for _ in 0..3 * steps {
        rig.os().advance_clock(IDLE_TICKS / steps);
        for _ in 0..40 {
            churn(&cache, busy, 3);
        }
    }
    assert_eq!(
        slab.stock(bursty).load(Relaxed).addr() & 0x3fff_ffff,
        0,
        "no credit left"
    );
    assert!(in_spares(&rig, 0, bursty).is_empty());
    rig.census(&[], false);
}

/// A CPU that only allocates a class, filling magazines from the source, sends its empty
/// magazines to the depot and takes them back to fill: a round trip of empties alone. It keeps
/// no empty stock, since it has no full stock to free into it.
#[test]
fn a_cpu_that_only_allocates_keeps_no_empty_stock() {
    let class = tiny(0);
    let tuning = Tuning {
        stock: true,
        ..QUIET
    };
    let rig: TestRig = Rig::new(&[class], 400, tuning);
    let machine = SimCpu::new(0);
    let cache = rig.cache(&machine);
    let held: Vec<_> = (0..400).filter_map(|_| cache.alloc(class)).collect();
    assert_eq!(held.len(), 400);
    let slab = rig.slabs().slab(cpu(&rig, 0));
    let empties = cache
        .spare_depth(class, crate::front::Spare::Empty)
        .unwrap();
    assert!(
        empties as usize <= QUIET.max_spares,
        "{empties} empty spares"
    );
    assert_eq!(slab.stock_held().load(Relaxed), 0);
    rig.census(&held, false);
}
