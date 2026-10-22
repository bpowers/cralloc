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
    beat_shift: super::heartbeat::BEAT_SHIFT,
    refresh_every: 0,
    age_limit: false,
    shed_unused: false,
    stock: false,
    centre_swings: true,
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

/// A stock word's credits, both kinds, as one number: zero for none.
fn credits_of(word: usize) -> u64 {
    let stock = super::stock::Stock(word as u64);
    (stock.credit(crate::front::Spare::Full) as u64) << 32
        | stock.credit(crate::front::Spare::Empty) as u64
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
    // A loaded magazine with an object in it, and a full spare: objects made on CPU 1 and
    // freed on CPU 0, one way, so that no swap reverses and leaves a magazine half full.
    let one = SimCpu::new(1);
    let maker = rig.cache(&one);
    let held: Vec<_> = (0..5).filter_map(|_| maker.alloc(idle)).collect();
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

/// CPU 0 runs another class's slow paths for `ticks` of the clock, in eighths.
fn busy_for(
    rig: &TestRig,
    cache: &Cache<'_, SimOs<Small>, Twin<'_, &SimCpu>, TestSource>,
    ticks: u64,
) {
    let busy = tiny(1);
    for _ in 0..8 {
        rig.os().advance_clock(ticks / 8);
        for _ in 0..40 {
            churn(cache, busy, 3);
        }
    }
}

/// A silence counts as one interval (`crate::clock`): a magazine an age look stamped just
/// past the middle of an interval, before a long silence, is still young a quarter interval
/// after it, and is taken within the two intervals the rule promises once slow paths keep
/// coming. Read off the cycle counter, the first age look after the silence took it. (The
/// sweep of full spares is not what this tests: it runs once an interval, so a sweep due at the
/// next interval is due at the first heartbeat after a silence, as it would have been.)
#[test]
fn a_silence_ages_a_loaded_magazine_by_at_most_one_interval() {
    let idle = tiny(0);
    let rig: TestRig = Rig::new(
        &[idle, tiny(1)],
        64,
        Tuning {
            age_limit: true,
            ..QUIET
        },
    );
    let machine = SimCpu::new(0);
    let cache = rig.cache(&machine);
    busy_for(&rig, &cache, IDLE_TICKS / 2);
    let object = cache.alloc(idle).unwrap();
    let other = cache.alloc(idle).unwrap();
    cache.free(idle, other);
    assert!(is_loaded(&rig, 0, idle));
    // Age looks go round the classes and stamp the loaded magazine.
    busy_for(&rig, &cache, IDLE_TICKS / 8);
    assert!(is_loaded(&rig, 0, idle));
    rig.os().advance_clock(50 * IDLE_TICKS);
    busy_for(&rig, &cache, IDLE_TICKS / 4);
    assert!(
        is_loaded(&rig, 0, idle),
        "taken by the first age looks after a silence"
    );
    busy_for(&rig, &cache, 2 * IDLE_TICKS);
    assert!(!is_loaded(&rig, 0, idle));
    cache.free(idle, object);
    rig.census(&[], false);
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
    a_stack_in_use_after(shed_unused, intervals, 0)
}

/// [`a_stack_in_use`] with a silence of `silence` ticks, no slow path anywhere, after each
/// round's interval.
fn a_stack_in_use_after(
    shed_unused: bool,
    intervals: usize,
    silence: u64,
) -> (Vec<usize>, Vec<usize>) {
    let (used, busy) = (tiny(0), tiny(1));
    // The busy class's swing, three objects over magazines of two, is what makes the slow
    // paths and so the heartbeats here; left half full now and then, its magazines would change
    // how often, and the shed, not that, is the subject.
    let tuning = Tuning {
        max_spares: 0,
        shed_unused,
        centre_swings: false,
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
        rig.os().advance_clock(silence);
    }
    rig.census(&[], false);
    (depths, served)
}

/// A long silence between the rounds of `a_stack_in_use` changes nothing: the stack still keeps
/// the depth its rounds use, since the interval before a silence is the only one it ages by.
#[test]
fn a_silence_between_rounds_does_not_shed_a_stack_in_use() {
    let (plain, _) = a_stack_in_use(true, 6);
    let (silent, _) = a_stack_in_use_after(true, 6, 50 * IDLE_TICKS);
    assert_eq!(plain, silent);
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

fn stock_review_quiet_domains_go_back_at_the_allocators_look_rate() {
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

fn stock_review_a_shared_stack_that_drains_fill_is_laundered_to_its_target() {
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
        seen.stock[n as usize] = credits_of(slab.stock(class).load(Relaxed).addr());
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
        credits_of(slab.stock(bursty).load(Relaxed).addr()),
        0,
        "no credit left"
    );
    assert!(in_spares(&rig, 0, bursty).is_empty());
    rig.census(&[], false);
}

// Findings of the adversarial review of stock (`stock.rs`, commit 1bd3010). Each test below
// fails as the code stands and is ignored so that the gate stays green; each names what it
// shows. Run them with `cargo test -p asmalloc --lib stock_review_ -- --ignored`.

use crate::front::Spare;
use crate::os::{Aborted, Machine, Step};
use core::cell::Cell;

/// A thread that the OS moves to another CPU right after a routine's commit, when told to: a
/// preemption between a routine and the slow path's next instruction.
struct Mover {
    cpu: Cell<i32>,
    then: Cell<Option<i32>>,
}

impl Machine for Mover {
    fn cpu_id(&self) -> i32 {
        self.cpu.get()
    }

    fn step(&self, _: Step) -> Result<(), Aborted> {
        Ok(())
    }

    fn committed(&self) {
        if let Some(cpu) = self.then.take() {
            self.cpu.set(cpu);
        }
    }
}

fn give_credit(rig: &TestRig, n: i32, class: SmallClass, full: u32, empty: u32) {
    rig.slabs()
        .slab(cpu(rig, n))
        .stock(class)
        .store(super::stock::credits(full, empty), Relaxed);
}

/// The bytes CPU `n` counts as full stock, as the room check reads them.
fn counted_full(rig: &TestRig, n: i32) -> usize {
    counted(rig, n, Spare::Full)
}

fn counted(rig: &TestRig, n: i32, kind: Spare) -> usize {
    let machine = SimCpu::new(n);
    rig.cache(&machine).stock_held(cpu(rig, n), kind)
}

fn round_trip<M: Machine>(
    cache: &Cache<'_, SimOs<Small>, Twin<'_, M>, TestSource>,
    class: SmallClass,
    n: usize,
) {
    let held: Vec<NonNull<u8>> = (0..n).filter_map(|_| cache.alloc(class)).collect();
    assert_eq!(held.len(), n);
    for object in held {
        cache.free(class, object);
    }
}

/// Finding: `collect` (and so `malloc_trim`) empties the caller's own slab by popping at most
/// 64 magazines from each spare stack (`collect.rs`, `SPARE_POPS`, "spare stacks hold at most
/// a few"). With stock a stack holds up to its fixed limit plus the room in magazines, 450 for
/// the 8-byte class with the allocator's limits, so the caller's own CPU keeps the rest.
#[test]
fn stock_review_collect_empties_the_callers_stock() {
    let class = tiny(0);
    let tuning = Tuning {
        stock: true,
        ..QUIET
    };
    let rig: TestRig = Rig::new(&[class], 1024, tuning);
    give_credit(&rig, 0, class, 200, 200);
    let machine = SimCpu::new(0);
    let cache = rig.cache(&machine);
    churn(&cache, class, 500);
    assert!(in_spares(&rig, 0, class).len() > 2 * 64, "stock grew");
    cache.collect();
    let left = in_spares(&rig, 0, class).len();
    assert_eq!(
        left, 0,
        "collect left {left} objects in the caller's spares"
    );
    rig.census(&[], false);
}

/// Finding: the per-CPU count of stock bytes is added to whatever CPU the thread is on after
/// the push routine returns (`spare_push`, then `note_stock_push`, each reading the CPU
/// number), so a thread moved between the two counts the magazine on a CPU that does not hold
/// it. A count above what the CPU holds is never put right while the CPU stays busy: pops and
/// the sweep take off only what they find, and only the idle drain clears the count. `stock.rs`
/// says "the next drain or sweep puts it right". Each such move (and each lost update between
/// the count's load and its store, and each pop followed by a move, which leaves the count
/// behind on the CPU popped from) takes a magazine's weight off that CPU's room for good, so a
/// CPU that never idles drifts towards keeping no stock.
#[test]
fn stock_review_a_sweep_puts_a_misplaced_count_right() {
    let (bursty, busy) = (tiny(0), tiny(1));
    let tuning = Tuning {
        stock: true,
        age_limit: true,
        look_every: 1,
        ..QUIET
    };
    let rig: TestRig = Rig::new(&[bursty, busy], 512, tuning);
    let mover = Mover {
        cpu: Cell::new(0),
        then: Cell::new(None),
    };
    let cache = rig.cache(&mover);
    // The sweep that the first clocked heartbeat begins runs out first: it would settle away
    // credit that no pop has used yet.
    round_trip(&cache, busy, 400);
    give_credit(&rig, 0, bursty, 8, 8);
    round_trip(&cache, bursty, 40);
    let weight = 2 * bursty.size();
    assert_eq!(counted_full(&rig, 0), 8 * weight, "CPU 0 counts its stock");
    // One more magazine retired to CPU 0's stock, by a thread moved to CPU 1 right after the
    // push routine's commit.
    let mut stale = false;
    let magazine = cache.spare_pop(bursty, Spare::Full, &mut stale).unwrap();
    assert_eq!(counted_full(&rig, 0), 7 * weight);
    mover.then.set(Some(1));
    assert!(
        cache
            .spare_push(bursty, Spare::Full, magazine, &mut stale)
            .is_ok()
    );
    assert_eq!(mover.cpu_id(), 1);
    assert_eq!(cache.spare_depth(bursty, Spare::Full), Some(0));
    // CPU 1 stays busy with another class for three intervals, sweeps and all.
    let steps = 8;
    for _ in 0..3 * steps {
        rig.os().advance_clock(IDLE_TICKS / steps);
        for _ in 0..40 {
            round_trip(&cache, busy, 3);
        }
    }
    assert!(in_spares(&rig, 1, bursty).is_empty());
    assert_eq!(
        counted_full(&rig, 1),
        0,
        "CPU 1 holds no stock, and counts some against its room"
    );
    rig.census(&[], false);
}

/// Finding: the sweep's step for a class takes its whole stack of full spares, stock and all,
/// and sends all but the top magazine to the depot in one heartbeat; what the domain's stack
/// and the shared stack's target do not take, the same heartbeat's launder over target sends
/// back to the source. With the allocator's limits that is up to 885 KiB of one class's objects
/// less the 128 KiB the depot keeps, some 370 magazines of the 8-byte class, laundered inside
/// one call to `malloc` or `free`, once an interval for each class a CPU keeps stock of, in
/// use or not. `heartbeat.rs`, "Bounded work", says one slow path launders at most four
/// magazines and that a sweep step pushes "at most a few".
#[test]
fn stock_review_one_call_launders_a_few_magazines() {
    let (bursty, busy) = (tiny(0), tiny(1));
    // The depot's limits against the stock as the allocator's are against its room: a
    // fraction of it.
    let tuning = Tuning {
        stock: true,
        age_limit: true,
        look_every: 1,
        domain_cap: 2 * 2 * bursty.size(),
        depot_target: 6 * 2 * bursty.size(),
        ..QUIET
    };
    let rig: TestRig = Rig::new(&[bursty, busy], 512, tuning);
    let machine = SimCpu::new(0);
    let cache = rig.cache(&machine);
    // The sweep that the first clocked heartbeat begins runs out first: it would settle away
    // credit that no pop has used yet.
    churn(&cache, busy, 400);
    give_credit(&rig, 0, bursty, 40, 40);
    churn(&cache, bursty, 200);
    assert!(in_spares(&rig, 0, bursty).len() >= 80, "stock grew");
    let mut most = 0;
    let mut call = |act: &mut dyn FnMut()| {
        let before = rig.source.held_of(bursty);
        act();
        most = most.max(rig.source.held_of(bursty).saturating_sub(before));
    };
    // The stock was in use in the interval that built it, so the first sweep after keeps it;
    // the interval after that, in which nothing uses it, is when it goes, a magazine a
    // heartbeat.
    for _ in 0..2 {
        rig.os().advance_clock(IDLE_TICKS);
        for _ in 0..200 {
            let mut held = Vec::new();
            for _ in 0..3 {
                call(&mut || held.extend(cache.alloc(busy)));
            }
            for object in held {
                call(&mut || cache.free(busy, object));
            }
        }
    }
    assert!(in_spares(&rig, 0, bursty).is_empty(), "the sweep took it");
    assert!(
        most <= 4 * 2,
        "one call laundered {most} objects, {} magazines",
        most / 2
    );
    rig.census(&[], false);
}

/// Finding: the sweep settles the stock word of the CPU its slow path began on, but returns
/// the empty spares past the new limit by popping the stack of the CPU the thread is on by
/// then (`settle_stock`: `spare_depth` and `spare_pop` read the CPU again). A thread moved in
/// between takes the credit from one CPU and the magazines from another, and the first CPU's
/// empties stay: later sweeps pop only as many as the credit they take off, and with no credit
/// left that is none. They are counted against that CPU's room for empty stock until the class
/// is freed there again or the CPU idles.
#[test]
fn stock_review_empty_stock_settled_by_a_moved_thread_goes_back() {
    let (bursty, busy) = (tiny(0), tiny(1));
    let tuning = Tuning {
        stock: true,
        age_limit: true,
        look_every: 1,
        ..QUIET
    };
    let rig: TestRig = Rig::new(&[bursty, busy], 512, tuning);
    let machine = SimCpu::new(0);
    let cache = rig.cache(&machine);
    // The sweep that the first clocked heartbeat begins runs out first: it would settle away
    // credit that no pop has used yet.
    churn(&cache, busy, 400);
    give_credit(&rig, 0, bursty, 40, 0);
    churn(&cache, bursty, 200);
    // The program takes the burst and keeps it: the full stock becomes empty stock.
    give_credit(&rig, 0, bursty, 40, 40);
    let held: Vec<NonNull<u8>> = (0..120).filter_map(|_| cache.alloc(bursty)).collect();
    let limit = tuning.max_spares as u32;
    let empties = || cache.spare_depth(bursty, Spare::Empty).unwrap();
    assert!(empties() > limit + 20, "empty stock grew: {}", empties());
    // The heartbeat of a slow path that began on CPU 0, its thread on CPU 1 by now.
    let elsewhere = SimCpu::new(1);
    let _ = rig.cache(&elsewhere).sweep_stock(cpu(&rig, 0), bursty);
    // CPU 0 stays busy with another class for three intervals, sweeps and all.
    let steps = 8;
    for _ in 0..3 * steps {
        rig.os().advance_clock(IDLE_TICKS / steps);
        for _ in 0..40 {
            churn(&cache, busy, 3);
        }
    }
    assert!(
        empties() <= limit,
        "{} empty spares against a limit of {limit}",
        empties()
    );
    rig.census(&held, false);
}

// Tests the review added for rules that no test held (each mutation named was not noticed by
// the suite at 1bd3010). They pass.

/// Gives the rig's CPU 0 forty magazines of full stock of `bursty`, for the tests below.
fn stock_up(rig: &TestRig, bursty: SmallClass, busy: SmallClass) {
    let machine = SimCpu::new(0);
    let cache = rig.cache(&machine);
    // The first clocked heartbeat's sweep runs out before the class has credit.
    churn(&cache, busy, 400);
    give_credit(rig, 0, bursty, 40, 40);
    churn(&cache, bursty, 200);
    assert_eq!(counted_full(rig, 0), 40 * 2 * bursty.size());
}

/// The sweep takes what it takes off the CPU's count of stock, and so does a drain: a count
/// left behind would be room the CPU never gets back. (Not taking the sweep's magazines off
/// the count, or not clearing it in the drain, went unnoticed.)
#[test]
fn the_sweep_and_the_drain_take_stock_off_the_count() {
    let tuning = Tuning {
        stock: true,
        age_limit: true,
        look_every: 1,
        ..QUIET
    };
    let (bursty, busy) = (tiny(0), tiny(1));
    let rig: TestRig = Rig::new(&[bursty, busy], 512, tuning);
    stock_up(&rig, bursty, busy);
    let machine = SimCpu::new(0);
    let cache = rig.cache(&machine);
    let steps = 8;
    for _ in 0..3 * steps {
        rig.os().advance_clock(IDLE_TICKS / steps);
        for _ in 0..40 {
            churn(&cache, busy, 3);
        }
    }
    assert!(in_spares(&rig, 0, bursty).is_empty());
    assert_eq!(counted_full(&rig, 0), 0, "after the sweep");

    let rig: TestRig = Rig::new(&[bursty, busy], 512, tuning);
    stock_up(&rig, bursty, busy);
    let other = SimCpu::new(1);
    assert_eq!(rig.cache(&other).drain(cpu(&rig, 0)), super::Drained::Done);
    assert!(in_spares(&rig, 0, bursty).is_empty());
    assert_eq!(counted_full(&rig, 0), 0, "after the drain");
    rig.census(&[], false);
}

/// Empty stock that an interval does not use goes back to the depot with its credit. (A sweep
/// that returned no empty spares went unnoticed.)
#[test]
fn empty_stock_unused_for_an_interval_goes_back() {
    let tuning = Tuning {
        stock: true,
        age_limit: true,
        look_every: 1,
        ..QUIET
    };
    let (bursty, busy) = (tiny(0), tiny(1));
    let rig: TestRig = Rig::new(&[bursty, busy], 512, tuning);
    stock_up(&rig, bursty, busy);
    let machine = SimCpu::new(0);
    let cache = rig.cache(&machine);
    // The program takes the burst and keeps it: the full stock becomes empty stock.
    let held: Vec<NonNull<u8>> = (0..120).filter_map(|_| cache.alloc(bursty)).collect();
    let limit = tuning.max_spares as u32;
    let empties = || cache.spare_depth(bursty, Spare::Empty).unwrap();
    assert!(empties() > limit + 20, "empty stock grew: {}", empties());
    let steps = 8;
    for _ in 0..3 * steps {
        rig.os().advance_clock(IDLE_TICKS / steps);
        for _ in 0..40 {
            churn(&cache, busy, 3);
        }
    }
    assert!(empties() <= limit, "{} empty spares", empties());
    assert_eq!(
        counted(&rig, 0, Spare::Full) + counted(&rig, 0, Spare::Empty),
        0
    );
    rig.census(&held, false);
}

/// A CPU that allocates what another frees takes full magazines from the depot and retires
/// empty ones. With credit for empty stock and none for full it keeps no empty stock: an empty
/// kept is room for a full one freed there, and nothing is. (`a_cpu_that_only_allocates_keeps_
/// no_empty_stock` fills from the source, where each slow path pops the empty spare it pushed,
/// so its stack never passes one with or without the rule, and dropping the rule went
/// unnoticed.)
#[test]
fn empty_stock_needs_credit_for_full_stock() {
    let class = tiny(0);
    let tuning = Tuning {
        stock: true,
        ..QUIET
    };
    let rig: TestRig = Rig::new(&[class], 256, tuning);
    give_credit(&rig, 0, class, 0, 40);
    let (a, f) = (SimCpu::new(0), SimCpu::new(1));
    let (allocator, freer) = (rig.cache(&a), rig.cache(&f));
    for _ in 0..6 {
        let objects: Vec<_> = (0..80).filter_map(|_| allocator.alloc(class)).collect();
        assert_eq!(objects.len(), 80);
        let empties = allocator.spare_depth(class, Spare::Empty).unwrap();
        assert!(
            empties as usize <= tuning.max_spares,
            "{empties} empty spares"
        );
        for object in objects {
            freer.free(class, object);
        }
    }
    assert_eq!(
        counted(&rig, 0, Spare::Full) + counted(&rig, 0, Spare::Empty),
        0
    );
    rig.census(&[], false);
}

/// A round trip earns credit for the kind of spare that made it, and no more than the room
/// holds of the class. (Crediting the other kind, or dropping the cap, went unnoticed.)
#[test]
fn credit_goes_to_the_kind_that_round_tripped_up_to_the_room() {
    use super::stock::{Stock, room};
    let class = tiny(0);
    let tuning = Tuning {
        stock: true,
        spare_budget: 1 << 20,
        ..QUIET
    };
    let rig: TestRig = Rig::new(&[class], 16, tuning);
    let machine = SimCpu::new(0);
    let cache = rig.cache(&machine);
    let word = || {
        Stock(
            rig.slabs()
                .slab(cpu(&rig, 0))
                .stock(class)
                .load(Relaxed)
                .addr() as u64,
        )
    };
    cache.note_sent(class, Spare::Full, true);
    cache.note_return(class, Spare::Full, false);
    assert_eq!(
        (word().credit(Spare::Full), word().credit(Spare::Empty)),
        (1, 0)
    );
    cache.note_sent(class, Spare::Empty, false);
    cache.note_return(class, Spare::Empty, true);
    assert_eq!(
        (word().credit(Spare::Full), word().credit(Spare::Empty)),
        (1, 1)
    );
    // An uncontended round trip earns nothing.
    cache.note_sent(class, Spare::Full, false);
    cache.note_return(class, Spare::Full, false);
    assert_eq!(word().credit(Spare::Full), 1);
    let most = (room(tuning) / (class.magazine().capacity() * class.size())) as u32;
    assert!(most > 1);
    give_credit(&rig, 0, class, most, 0);
    cache.note_sent(class, Spare::Full, true);
    cache.note_return(class, Spare::Full, true);
    assert_eq!(word().credit(Spare::Full), most);
}

/// A class in steady use on CPU 0 whose pops take the top of its full spares and never the
/// bottom, for three intervals. Whether the full spares were ever seen below their fixed limit
/// of three: whether the sweep took them.
fn the_bottom_spares_are_taken(stock: bool) -> bool {
    let (class, busy) = (tiny(0), tiny(1));
    let tuning = Tuning {
        stock,
        age_limit: true,
        look_every: 1,
        max_spares: 3,
        ..QUIET
    };
    let rig: TestRig = Rig::new(&[class, busy], 512, tuning);
    let machine = SimCpu::new(0);
    let cache = rig.cache(&machine);
    // The first clocked heartbeat's sweep runs out before the class has credit.
    churn(&cache, busy, 400);
    if stock {
        give_credit(&rig, 0, class, 1, 1);
    }
    churn(&cache, class, 12);
    let depth = || cache.spare_depth(class, Spare::Full).unwrap();
    assert!(depth() >= 3, "{}", depth());
    let mut taken = false;
    let steps = 8;
    for _ in 0..3 * steps {
        rig.os().advance_clock(IDLE_TICKS / steps);
        for _ in 0..60 {
            let held: Vec<NonNull<u8>> = (0..3).filter_map(|_| cache.alloc(class)).collect();
            taken |= depth() < 2;
            for object in held {
                cache.free(class, object);
            }
        }
    }
    rig.census(&[], false);
    taken
}

/// Finding (check of the fix, b5070c3): a class with credit whose stock is in use by the
/// low-water rule keeps its fixed full spares out of the sweep, on the argument that "the pops
/// that use the stock reach them, or the low-water mark shrinks the stock until they do"
/// (`stock.rs`, "Disuse"). The mark shrinks the stock only down to the fixed limit: pops that
/// leave the stack at or below its limit take nothing off the credit, so a class that swaps
/// the top of its stack keeps one credit for good, and the magazines under the top are never
/// popped, never swept and never refreshed: up to the fixed limit of each class, 139 KiB of
/// objects a CPU with the allocator's limits, parked for as long as the class stays busy
/// there. Without stock the sweep takes them once an interval, which is the rule the design's
/// table gives for a CPU's full spares.
#[test]
fn stock_review_fixed_spares_under_stock_in_use_are_swept() {
    assert!(the_bottom_spares_are_taken(false), "without stock");
    assert!(the_bottom_spares_are_taken(true), "with one credit");
}

/// A stack deeper than its fixed limit whose class lost its stock word to a race (a stale
/// store of zero) is not taken whole by the sweep's plain path either: its top goes back a
/// magazine a heartbeat until the stack is within the limit, then the fixed spares are swept.
#[test]
fn a_deep_stack_without_credit_goes_back_a_magazine_a_call() {
    let (bursty, busy) = (tiny(0), tiny(1));
    let tuning = Tuning {
        stock: true,
        age_limit: true,
        look_every: 1,
        domain_cap: 2 * 2 * bursty.size(),
        depot_target: 6 * 2 * bursty.size(),
        ..QUIET
    };
    let rig: TestRig = Rig::new(&[bursty, busy], 512, tuning);
    let machine = SimCpu::new(0);
    let cache = rig.cache(&machine);
    churn(&cache, busy, 400);
    give_credit(&rig, 0, bursty, 40, 40);
    churn(&cache, bursty, 200);
    assert!(in_spares(&rig, 0, bursty).len() >= 80, "stock grew");
    // The race: the class's word is zero while its stack is deep.
    give_credit(&rig, 0, bursty, 0, 0);
    let mut most = 0;
    for _ in 0..2 {
        rig.os().advance_clock(IDLE_TICKS);
        for _ in 0..200 {
            let before = rig.source.held_of(bursty);
            churn(&cache, busy, 3);
            most = most.max(rig.source.held_of(bursty).saturating_sub(before));
        }
    }
    assert!(in_spares(&rig, 0, bursty).is_empty(), "the sweep took it");
    assert!(most <= 4 * 2, "one call laundered {most} objects");
    rig.census(&[], false);
}

/// A heap whose magazine area holds some 127 of the largest magazines (8 KiB each), for the
/// rules' bounds on work with the smallest classes' magazines of 1,022 objects.
enum Roomy {}

impl crate::os::sim::Shape for Roomy {
    const GEOMETRY: crate::space::Geometry =
        crate::space::Geometry::new(6, 8, 13, 13, 256, 128, MagazineSize::MAX);
}

/// One slow path launders at most `LAUNDER_BUDGET` objects, and a rule the budget turns away
/// does its work at a later slow path. Here a free retires a full magazine of the smallest
/// class (1,022 objects) and refreshes it, which spends the budget, while the sweep, due on the
/// same class in the same heartbeat, wants to launder a full spare: without the budget that
/// call would launder two magazines.
#[test]
fn a_rule_the_budget_turns_away_waits_for_the_next_slow_path() {
    let class = SmallClass::from_ordinal(0).unwrap();
    let capacity = class.magazine().capacity();
    assert_eq!(capacity, super::LAUNDER_BUDGET);
    // A heartbeat on every slow path, so that the sweep begins at the first: the budget, not the
    // pace, is what this checks.
    let tuning = Tuning {
        refresh_every: 1,
        age_limit: true,
        look_every: 1,
        beat_shift: 0,
        max_spares: 2,
        ..QUIET
    };
    let rig: Rig<SimOs<Roomy>, 2> = Rig::with_os(SimOs::new(), &[class], 4 * capacity, tuning);
    let machine = SimCpu::new(0);
    let cache = rig.cache(&machine);
    // Objects from the source, freed on CPU 0: its first slow path loads a magazine and begins
    // the sweep at the first class, and the frees fill the loaded magazine up to one short of
    // full. Only frees, so that no swap reverses and leaves a magazine half full.
    let held: Vec<NonNull<u8>> = (0..capacity)
        .filter_map(|_| rig.source.alloc_one(class))
        .collect();
    assert_eq!(held.len(), capacity);
    let mut held = held.into_iter();
    for object in held.by_ref().take(capacity - 1) {
        cache.free(class, object);
    }
    // A full spare for the sweep to take.
    let spare = cache.area().create(class.magazine()).unwrap();
    assert_eq!(rig.source.fill(class, spare.objects()).count, capacity);
    spare.set_level(capacity);
    let mut stale = false;
    assert!(
        cache
            .spare_push(class, Spare::Full, spare, &mut stale)
            .is_ok()
    );
    let cpu = rig.slabs().cpu(0).unwrap();
    let spare_head = || {
        rig.slabs()
            .slab(cpu)
            .field(Field::spare(class, Spare::Full))
            .load(Relaxed)
    };
    let spares = || !spare_head().is_null();
    assert!(spares());
    // The last free fills the loaded magazine; the next one exchanges it and refreshes it.
    cache.free(class, held.next().unwrap());
    let extra = rig.source.alloc_one(class).unwrap();
    let before = rig.source.laundered();
    cache.free(class, extra);
    assert_eq!(
        rig.source.laundered() - before,
        capacity,
        "the refresh alone"
    );
    assert!(spares(), "the sweep waited");
    // The CPU's next slow path runs the sweep first. Frees fill the new loaded magazine; the one
    // that finds it full exchanges it, the refresh waits for the sweep, and the sweep takes
    // the spare stack, laundering its top.
    for _ in 0..capacity - 1 {
        cache.free(class, rig.source.alloc_one(class).unwrap());
    }
    let extra = rig.source.alloc_one(class).unwrap();
    let before = rig.source.laundered();
    cache.free(class, extra);
    assert_eq!(
        rig.source.laundered() - before,
        capacity,
        "the sweep, not the refresh"
    );
    assert!(!spares(), "the sweep took the spares");
}

/// At the real pace, where the smallest class's exchanges beat on one in four, a rule the
/// budget turned away does not wait for the next beat: the CPU's next slow path beats at once,
/// and the rule does its work there. The refresh of every displaced magazine spends each
/// call's budget, so the sweep, once due on the class, is turned away at a paced beat.
#[test]
fn a_rule_the_budget_turns_away_runs_at_the_next_slow_path_whatever_the_pace() {
    let class = SmallClass::from_ordinal(0).unwrap();
    let capacity = class.magazine().capacity();
    let tuning = Tuning {
        refresh_every: 1,
        age_limit: true,
        look_every: 1,
        max_spares: 2,
        ..QUIET
    };
    assert_eq!(tuning.beat_shift, super::heartbeat::BEAT_SHIFT);
    let rig: Rig<SimOs<Roomy>, 2> = Rig::with_os(SimOs::new(), &[class], 8 * capacity, tuning);
    let machine = SimCpu::new(0);
    let cache = rig.cache(&machine);
    let cpu = rig.slabs().cpu(0).unwrap();
    let slab = rig.slabs().slab(cpu);
    // A full spare for the sweep to take.
    let spare = cache.area().create(class.magazine()).unwrap();
    assert_eq!(rig.source.fill(class, spare.objects()).count, capacity);
    spare.set_level(capacity);
    let mut stale = false;
    assert!(
        cache
            .spare_push(class, Spare::Full, spare, &mut stale)
            .is_ok()
    );
    let spares = || {
        !slab
            .field(Field::spare(class, Spare::Full))
            .load(Relaxed)
            .is_null()
    };
    // Only frees, of objects from the source, so that no swap reverses; each step frees until
    // one slow path has run.
    let mut steps = Vec::new();
    while spares() && steps.len() < 64 {
        let (activity, beats, laundered) = (slab.activity(), slab.beats(), rig.source.laundered());
        while slab.activity() == activity {
            cache.free(class, rig.source.alloc_one(class).unwrap());
        }
        let paced = slab.activity() >> super::heartbeat::BEAT_SHIFT
            != activity >> super::heartbeat::BEAT_SHIFT;
        let laundered = rig.source.laundered() - laundered;
        assert!(laundered <= 2 * capacity, "one call laundered {laundered}");
        steps.push((paced, slab.beats() - beats, laundered, spares()));
    }
    assert!(!spares(), "the sweep never took the spare: {steps:?}");
    let [.., turned_away, next] = steps[..] else {
        panic!("{steps:?}")
    };
    // The sweep was due at a paced beat whose refresh spent the budget...
    assert_eq!(turned_away, (true, 1, capacity, true), "{steps:?}");
    // ...and ran at the next slow path, which beat although the pace did not ask for it; that
    // call's refresh waited for it, within the budget.
    assert_eq!(next, (false, 1, capacity, false), "{steps:?}");
    rig.census(&[], false);
}

/// A heap whose CPUs `width` at a time share a cache domain, with room for many of the largest
/// magazines.
fn roomy<const N: usize>(width: u32, classes: &[SmallClass], count: usize) -> Rig<SimOs<Roomy>, N> {
    let os = SimOs::new();
    os.set_topology(crate::os::sim::SimTopology::Blocks(width));
    Rig::with_os(os, classes, count, Tuning::DEFAULT)
}

/// Whether CPU `n` of `rig` has a magazine of `class` loaded.
fn loaded_on<const N: usize>(rig: &Rig<SimOs<Roomy>, N>, n: i32, class: SmallClass) -> bool {
    let word = rig
        .slabs()
        .slab(rig.slabs().cpu(n).unwrap())
        .field(Field::hot(class))
        .load(Relaxed);
    !word.is_null() && !crate::classes::is_marker(word.addr())
}

/// CPU 0, in one cache domain, uses the smallest class once, then frees what CPU 1, in another,
/// allocates of a class with two-object magazines, for three intervals. With `ahead`, CPU 1
/// allocates everything first, so that every magazine CPU 0 retires goes past the shared stack's
/// target; without, the two take turns, and the shared stack stays under it. Whether CPU 0
/// still had the smallest class's magazine loaded at the end, and whether its sweep went round
/// every class.
fn idle_magazine_left_loaded(ahead: bool) -> (bool, bool) {
    let (idle, flow) = (SmallClass::from_ordinal(0).unwrap(), tiny(0));
    let steps = 24;
    let per_step = 128;
    let rig: Rig<SimOs<Roomy>, 2> = roomy(1, &[idle, flow], steps * per_step + 16);
    let (zero, one) = (SimCpu::new(0), SimCpu::new(1));
    let (frees, allocs) = (rig.cache(&zero), rig.cache(&one));
    let object = frees.alloc(idle).unwrap();
    frees.free(idle, object);
    assert!(loaded_on(&rig, 0, idle));
    let mut ahead_of_time: Vec<NonNull<u8>> = Vec::new();
    if ahead {
        ahead_of_time = (0..steps * per_step)
            .filter_map(|_| allocs.alloc(flow))
            .collect();
        assert_eq!(ahead_of_time.len(), steps * per_step);
    }
    let sweep = rig.slabs().slab(rig.slabs().cpu(0).unwrap()).sweep();
    let mut swept = false;
    for _ in 0..steps {
        rig.os().advance_clock(IDLE_TICKS / 8);
        for _ in 0..per_step {
            let object = match ahead_of_time.pop() {
                Some(object) => object,
                None => allocs.alloc(flow).unwrap(),
            };
            frees.free(flow, object);
            // The sweep's cursor, in the word's low byte, at the class count: done.
            swept |= (sweep.load(Relaxed) & 0xff) as usize == crate::classes::SMALL_COUNT;
        }
    }
    (loaded_on(&rig, 0, idle), swept)
}

/// Review of the launder budget: a CPU whose every slow path pushed a magazine on a shared
/// stack set the magazine's objects aside from the budget, over target or not, so its age limit
/// never had room for a magazine of the smallest class, and one it had stopped using stayed
/// loaded for good. Now only a push that leaves the stack over its target is set aside.
#[test]
fn a_cpu_whose_pushes_stay_under_target_still_takes_back_an_idle_magazine() {
    assert_eq!(idle_magazine_left_loaded(false), (false, true));
}

/// Review of the launder budget: where every push does leave the shared stack over its target,
/// the age limit finds the old magazine and no room on every slow path; the next slow path then
/// gives it the budget first.
#[test]
fn a_cpu_whose_pushes_overflow_still_takes_back_an_idle_magazine() {
    assert_eq!(idle_magazine_left_loaded(true), (false, true));
}

/// CPU 0 frees `class` and nothing else for `intervals`, `magazines` full magazines an eighth
/// of an interval, its objects straight from the source. Returns the shared stack's target and
/// the deepest it was in the second half, in objects.
fn shared_stack_in_a_free_only_phase(
    class: SmallClass,
    intervals: usize,
    magazines: usize,
) -> (usize, usize) {
    let per = class.magazine().capacity();
    let rig: Rig<SimOs<Roomy>, 2> = roomy(2, &[tiny(1), class], 128 * per);
    let zero = SimCpu::new(0);
    let cache = rig.cache(&zero);
    let target = cache.shared_target(class);
    let mut deepest = 0;
    for step in 0..intervals * 8 {
        rig.os().advance_clock(IDLE_TICKS / 8);
        for _ in 0..magazines * per {
            cache.free(class, rig.source.alloc_one(class).unwrap());
        }
        if step >= intervals * 4 {
            deepest = deepest.max(in_pool_roomy(&rig, Pool::Shared, class));
        }
    }
    (target, deepest)
}

fn in_pool_roomy<const N: usize>(
    rig: &Rig<SimOs<Roomy>, N>,
    pool: Pool,
    class: SmallClass,
) -> usize {
    let machine = SimCpu::new(0);
    let cache = rig.cache(&machine);
    rig.depot
        .full(pool, class)
        .count(cache.area(), class.magazine()) as usize
}

/// Review of the launder budget: on a CPU that only frees, every exchange pushes past the
/// shared stack's target, and each call that ran a rule the budget had turned away skipped its
/// own launder over target, which takes one magazine a push and so never caught up: the stack
/// levelled at three to five times its target. A call launders its own push even after such a
/// rule. The 16-byte stack then stays at its target; the 8-byte one, whose CPU keeps two full
/// spares, also holds what the sweep sends on from them, within its budget, until the shed
/// takes it back.
#[test]
fn a_free_only_phase_keeps_the_shared_stack_at_its_target() {
    for ordinal in [0, 1] {
        let class = SmallClass::from_ordinal(ordinal).unwrap();
        let per = class.magazine().capacity();
        let (target, deepest) = shared_stack_in_a_free_only_phase(class, 24, 4);
        assert!(
            deepest <= target * 5 / 2,
            "class {}: the shared stack reached {} magazines against a target of {}",
            class.size(),
            deepest / per,
            target / per
        );
    }
}

/// What a CPU's work in one class did over its last 100 repetitions of a swing.
#[derive(Debug)]
struct Swung {
    slow: usize,
    fills: usize,
    laundered: usize,
}

/// A CPU that allocates `amplitude` objects of `class` and frees them, newest first (`fifo`
/// false) or oldest first, 400 times, with the swing rule on or off.
fn swing(class: SmallClass, amplitude: usize, centre: bool, fifo: bool) -> Swung {
    use std::collections::VecDeque;
    let capacity = class.magazine().capacity();
    let tuning = Tuning {
        centre_swings: centre,
        ..Tuning::DEFAULT
    };
    let rig: Rig<SimOs<Roomy>, 2> = Rig::with_os(
        SimOs::new(),
        &[class],
        16 * capacity + 8 * amplitude,
        tuning,
    );
    let machine = SimCpu::new(0);
    let cache = rig.cache(&machine);
    let slab = rig.slabs().slab(rig.slabs().cpu(0).unwrap());
    cache.free(class, rig.source.alloc_one(class).unwrap());
    let mut held = VecDeque::new();
    let repetitions = 400;
    let (mut slow, mut fills, mut laundered) = (0, 0, 0);
    for repetition in 0..repetitions {
        if repetition == repetitions - 100 {
            slow = 0;
            fills = rig.source.fills();
            laundered = rig.source.laundered();
        }
        for _ in 0..amplitude {
            let before = slab.activity();
            held.push_back(cache.alloc(class).unwrap());
            slow += usize::from(slab.activity() != before);
        }
        while let Some(object) = if fifo {
            held.pop_front()
        } else {
            held.pop_back()
        } {
            let before = slab.activity();
            cache.free(class, object);
            slow += usize::from(slab.activity() != before);
        }
    }
    rig.census(&[], false);
    Swung {
        slow,
        fills: rig.source.fills() - fills,
        laundered: rig.source.laundered() - laundered,
    }
}

/// The swing rule's bar: across swings from a quarter of a magazine to four, newest or oldest
/// freed first, the rule never takes more than 2% more slow paths than the plain swaps do, and
/// a swing under half a magazine stops taking them.
fn the_swing_rule_pays_for_itself(sizes: &[usize]) {
    use crate::classes::Class;
    let swings = [
        (1, 4),
        (1, 2),
        (5, 8),
        (3, 4),
        (1, 1),
        (9, 8),
        (3, 2),
        (2, 1),
        (5, 2),
        (4, 1),
    ];
    for &size in sizes {
        let class = Class::of_size(size).unwrap().small().unwrap();
        let capacity = class.magazine().capacity();
        for (num, den) in swings {
            let amplitude = (capacity * num / den + 3).max(2);
            for fifo in [false, true] {
                let on = swing(class, amplitude, true, fifo);
                let off = swing(class, amplitude, false, fifo);
                let row = std::format!("{size} bytes, {num}/{den}, fifo {fifo}: {on:?} {off:?}");
                assert!(50 * on.slow <= 51 * off.slow, "{row}");
                assert!(
                    on.fills <= off.fills && on.laundered <= off.laundered,
                    "{row}"
                );
                if 2 * amplitude < capacity {
                    assert_eq!(on.slow, 0, "{row}");
                }
            }
        }
    }
}

#[test]
fn the_swing_rule_pays_for_itself_in_one_class() {
    the_swing_rule_pays_for_itself(&[1024]);
}

#[test]
#[cfg(not(debug_assertions))]
#[ignore = "slow: run with scripts/gate --all, optimized"]
fn the_swing_rule_pays_for_itself_in_the_small_classes() {
    the_swing_rule_pays_for_itself(&[8, 64, 1024]);
}

/// A slow path that is not a beat, and drains its own slab where a fork left the parent's
/// marker, launders what the drain parks over a shared stack's target only within the budget:
/// the rest waits for the shed. (A drain empties every class, and here the shared stacks of
/// two classes hold dozens of magazines past their targets.)
#[test]
fn a_drain_between_beats_launders_within_the_budget() {
    let small = SmallClass::from_ordinal(0).unwrap();
    let next = SmallClass::from_ordinal(1).unwrap();
    let capacity = small.magazine().capacity();
    assert_eq!(capacity, super::LAUNDER_BUDGET);
    // No call beats by pace, so the drain is the call's own, between beats.
    let tuning = Tuning {
        beat_shift: 40,
        ..Tuning::DEFAULT
    };
    let rig: Rig<SimOs<Roomy>, 2> =
        Rig::with_os(SimOs::new(), &[small, next], 80 * capacity, tuning);
    let machine = SimCpu::new(0);
    let cache = rig.cache(&machine);
    // Spares and depot stacks of both classes, filled one way from CPU 0.
    for class in [small, next] {
        let n = 40 * class.magazine().capacity() + 7;
        let held: Vec<_> = (0..n).filter_map(|_| rig.source.alloc_one(class)).collect();
        for object in held {
            cache.free(class, object);
        }
    }
    rig.census(&[], false);
    // A fork cut a parent's drain of CPU 0 off after its first exchange: the hot word of the
    // first class holds the parent's marker.
    let generation = cache.generation().unwrap();
    let cpu = rig.slabs().cpu(0).unwrap();
    rig.slabs()
        .slab(cpu)
        .field(Field::hot(small))
        .store(generation.marker(), Relaxed);
    // SAFETY: one thread, and no allocator call in flight.
    unsafe { rig.os().wipe_for_fork() };
    let child = SimCpu::new(0);
    let cache = rig.cache(&child);
    let before = rig.source.laundered();
    cache.free(small, rig.source.alloc_one(small).unwrap());
    let laundered = rig.source.laundered() - before;
    assert!(
        laundered <= super::LAUNDER_BUDGET,
        "one call laundered {laundered}"
    );
    // Lossy: the marker written over the hot word stands for the parent's drain, which took
    // the magazine that was there.
    rig.census(&[], true);
}

/// Slow paths in the last quarter of 400 repetitions of `pattern`, phases of (allocate `a`,
/// free the newest `f`), on one CPU loaded at level `start`, with the swing rule on or off.
#[cfg(not(debug_assertions))]
fn compound_swing(
    class: SmallClass,
    start: usize,
    pattern: &[(usize, usize)],
    centre: bool,
) -> usize {
    let capacity = class.magazine().capacity();
    let tuning = Tuning {
        centre_swings: centre,
        ..Tuning::DEFAULT
    };
    let rig: Rig<SimOs<Roomy>, 2> = Rig::with_os(SimOs::new(), &[class], 64 * capacity, tuning);
    let machine = SimCpu::new(0);
    let cache = rig.cache(&machine);
    let slab = rig.slabs().slab(rig.slabs().cpu(0).unwrap());
    for _ in 0..start {
        cache.free(class, rig.source.alloc_one(class).unwrap());
    }
    let mut held = Vec::new();
    let repetitions = 400;
    let mut slow = 0;
    for repetition in 0..repetitions {
        if repetition == repetitions - repetitions / 4 {
            slow = 0;
        }
        for &(allocs, frees) in pattern {
            for _ in 0..allocs {
                let before = slab.activity();
                held.push(cache.alloc(class).unwrap());
                slow += usize::from(slab.activity() != before);
            }
            for _ in 0..frees.min(held.len()) {
                let before = slab.activity();
                cache.free(class, held.pop().unwrap());
                slow += usize::from(slab.activity() != before);
            }
        }
    }
    for object in held {
        cache.free(class, object);
    }
    rig.census(&[], false);
    slow
}

/// Slow paths of `pattern`, summed over every level the CPU's magazine may start at. Where a
/// pattern's swaps fall depends on its level's phase against the magazines' ends, which plain
/// swaps keep and a half-full swap moves by half a magazine; so one starting level measures
/// the luck of its phase, and the sum over all of them what the rule costs.
#[cfg(not(debug_assertions))]
fn compound_over_phases(class: SmallClass, pattern: &[(usize, usize)], centre: bool) -> usize {
    (0..class.magazine().capacity())
        .map(|start| compound_swing(class, start, pattern, centre))
        .sum()
}

/// The rule on takes at most `percent` more slow paths than off for `pattern`, over phases.
#[cfg(not(debug_assertions))]
fn compound_within(class: SmallClass, pattern: &[(usize, usize)], percent: usize) {
    let on = compound_over_phases(class, pattern, true);
    let off = compound_over_phases(class, pattern, false);
    assert!(
        100 * on <= (100 + percent) * off,
        "{} bytes, {pattern:?}: on {on}, off {off} over phases",
        class.size()
    );
}

/// Two swings of different sizes in each repetition, the review of ff7c39b's worst cases
/// (1,024-byte class, magazines of 62): while a run after a half-full swap counted as absorbed,
/// the rule retried them every repetition. From a single starting level they also measured the
/// luck of its phase (`compound_over_phases`).
#[test]
#[cfg(not(debug_assertions))]
#[ignore = "slow: run with scripts/gate --all, optimized"]
fn the_swing_rule_pays_for_itself_on_two_sizes() {
    let class = crate::classes::Class::of_size(1024)
        .unwrap()
        .small()
        .unwrap();
    assert_eq!(class.magazine().capacity(), 62);
    for pattern in [
        &[(112, 112), (224, 224)][..],
        &[(158, 79), (79, 158)],
        &[(81, 81), (162, 162)],
        &[(112, 112), (37, 37)],
    ] {
        compound_within(class, pattern, 2);
    }
    // Above the bar, at 3.4%: the rule's retries, one in 65 lock swaps, each move the phase,
    // and this pattern's are not paid back (reported with the change, not special-cased).
    compound_within(class, &[(34, 34), (68, 68)], 4);
}

/// The review's wide sweep over phases: amplitudes from a sixteenth of a magazine to three
/// magazines, one size and three two-size patterns. 185 of the 192 are within 2%; seven, a
/// swing near half a magazine with one twice its size (31/62, 34/68, 38/76) or a swing near a
/// magazine with one a third its size (62/20 to 73/24), are 2.1 to 3.4% over, the cost of the
/// rule's retries moving the phase. They are held here at 4%.
#[test]
#[cfg(not(debug_assertions))]
#[ignore = "slow: run with scripts/gate --all, optimized"]
fn the_swing_rule_pays_for_itself_on_two_sizes_widely() {
    let class = crate::classes::Class::of_size(1024)
        .unwrap()
        .small()
        .unwrap();
    let capacity = class.magazine().capacity();
    for sixteenths in 1..=48 {
        let amplitude = (capacity * sixteenths / 16).max(1);
        for pattern in [
            std::vec![(amplitude, amplitude)],
            std::vec![(amplitude, amplitude / 2), (amplitude / 2, amplitude)],
            std::vec![(amplitude, amplitude), (amplitude / 3, amplitude / 3)],
            std::vec![(amplitude, amplitude), (2 * amplitude, 2 * amplitude)],
        ] {
            compound_within(class, &pattern, 4);
        }
    }
}

/// After a long stretch of a swing the rule cannot absorb has taken the backoff to its longest,
/// a quarter-magazine swing is still centred within the backoff's bound (64 lock swaps, two a
/// repetition), and then takes no slow path.
#[test]
fn a_swing_the_rule_can_absorb_is_centred_after_a_long_failing_stretch() {
    let class = crate::classes::Class::of_size(1024)
        .unwrap()
        .small()
        .unwrap();
    let capacity = class.magazine().capacity();
    let rig: Rig<SimOs<Roomy>, 2> =
        Rig::with_os(SimOs::new(), &[class], 64 * capacity, Tuning::DEFAULT);
    let machine = SimCpu::new(0);
    let cache = rig.cache(&machine);
    let slab = rig.slabs().slab(rig.slabs().cpu(0).unwrap());
    cache.free(class, rig.source.alloc_one(class).unwrap());
    let mut held = Vec::new();
    let mut swing = |amplitude: usize, repetitions: usize| -> Vec<bool> {
        (0..repetitions)
            .map(|_| {
                let before = slab.activity();
                for _ in 0..amplitude {
                    held.push(cache.alloc(class).unwrap());
                }
                while let Some(object) = held.pop() {
                    cache.free(class, object);
                }
                slab.activity() != before
            })
            .collect()
    };
    swing(capacity * 3 / 4, 2000);
    let after = swing(capacity / 4, 200);
    let last = after.iter().rposition(|&slow| slow).map_or(0, |i| i + 1);
    assert!(last <= 40, "slow paths until repetition {last}: {after:?}");
    rig.census(&[], false);
}
