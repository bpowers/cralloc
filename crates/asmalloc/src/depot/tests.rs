//! Scenario tests of the middle tier on one thread over the simulated OS.
//!
//! The heaviest run a workload that crosses every slow path (fill, install, retire to spares and
//! the depot, launder, drain) and interrupt it at every switch point in turn, that is, before
//! every atomic instruction of the slow paths and every step of every twin: with a signal
//! handler that allocates and frees, with a fork that the thread survives, and with a fork that
//! the thread does not survive. After each run a census checks that every magazine and every
//! object is in exactly one place.

use super::test_rig::{Rig, SimCpu, Small, TIGHT, tiny_class};
use super::{Drained, IDLE_TICKS, Tuning};
use crate::classes::{Class, SmallClass};
use crate::front::check::interrupt_at;
use crate::front::{Cpu, Routines};
use crate::os::Os;
use crate::os::sim::{Call, Faults, SimOs};
use core::cell::RefCell;
use core::ptr::NonNull;
use std::boxed::Box;
use std::panic::{self, AssertUnwindSafe};
use std::vec::Vec;

type TestRig = Rig<SimOs<Small>, 2>;

/// The interrupt points a test tries: every one natively, a spread of them under Miri, which
/// runs each workload thousands of times slower.
const STRIDE: usize = if cfg!(miri) { 37 } else { 1 };

fn rig(count: usize, tuning: Tuning) -> TestRig {
    Rig::new(&[tiny_class()], count, tuning)
}

fn cpu(rig: &TestRig, n: i32) -> Cpu {
    rig.slabs().cpu(n).unwrap()
}

/// Allocations and frees on CPU 0, then on CPU 1 with the clock moving an idle interval between
/// steps, so that CPU 1's heartbeats see CPU 0 stand still for one and drain it. `user` holds
/// what the program holds.
fn workload(rig: &TestRig, machine: &SimCpu, user: &RefCell<Vec<NonNull<u8>>>) {
    let class = tiny_class();
    let cache = rig.cache(machine);
    let alloc = |n| {
        for _ in 0..n {
            if let Some(object) = cache.alloc(class) {
                user.borrow_mut().push(object);
            }
        }
    };
    let free = |n| {
        for _ in 0..n {
            let object = user.borrow_mut().pop();
            if let Some(object) = object {
                cache.free(class, object);
            }
        }
    };
    machine.set_cpu(0);
    alloc(5);
    free(4);
    alloc(2);
    machine.set_cpu(1);
    let wait = || rig.os().advance_clock(IDLE_TICKS);
    free(3);
    wait();
    alloc(3);
    wait();
    free(2);
    wait();
    alloc(1);
    wait();
    free(2);
}

#[test]
fn a_workload_keeps_every_magazine_and_object_in_one_place() {
    let rig = rig(12, TIGHT);
    let machine = SimCpu::new(0);
    let user = RefCell::new(Vec::new());
    workload(&rig, &machine, &user);
    let census = rig.census(&user.borrow(), false);
    assert!(census.magazines >= 3, "{census:?}");
    assert_eq!((census.markers, census.claims), (0, 0));
    assert!(
        rig.source.purges() > 0,
        "the heartbeat asks for purge steps"
    );
    assert!(census.spares > 0 && census.depot > 0, "{census:?}");
    assert!(
        rig.os().calls(Call::Fence) > 0,
        "the idle check drained a slab"
    );
    // The heartbeat on CPU 1 found CPU 0 idle and drained it.
    let cache = rig.cache(&machine);
    let slab0 = rig.slabs().slab(cpu(&rig, 0));
    assert!(slab0.activity() > 0);
    assert_eq!(cache.drain(cpu(&rig, 0)), Drained::Done);
}

/// Slow paths on CPU 1 with the tiny class, whose magazines hold two objects, so that every
/// call here takes at least one slow path and with it a look at CPU 0.
fn heartbeats_on_cpu_1(rig: &TestRig) {
    let machine = SimCpu::new(1);
    let cache = rig.cache(&machine);
    let class = tiny_class();
    let held: Vec<_> = (0..3).filter_map(|_| cache.alloc(class)).collect();
    assert!(!held.is_empty());
    for object in held {
        cache.free(class, object);
    }
}

/// Whether CPU 0's slab holds nothing at all.
fn cpu_0_is_empty(rig: &TestRig) -> bool {
    let slab = rig.slabs().slab(cpu(rig, 0));
    crate::front::Slab::drained_fields().all(|field| {
        slab.field(field)
            .load(core::sync::atomic::Ordering::Relaxed)
            .is_null()
    })
}

/// A CPU that allocates and frees only on fast paths never moves its activity counter, however
/// busy it is, while another CPU's heartbeats look at it many times an interval. It is drained,
/// and refills, but never twice within an interval: counted in looks rather than time, it was
/// drained on every other look.
#[test]
fn a_cpu_busy_on_fast_paths_is_drained_at_most_once_an_interval() {
    let busy = Class::of_size(64).unwrap().small().unwrap();
    let tuning = Tuning {
        look_every: 1,
        ..Tuning::DEFAULT
    };
    let rig: TestRig = Rig::new(&[busy, tiny_class()], 1024, tuning);
    let machine = SimCpu::new(0);
    let cache = rig.cache(&machine);
    let held = cache.alloc(busy).unwrap();
    let slab = rig.slabs().slab(cpu(&rig, 0));
    let mut drains = Vec::new();
    let intervals = 10;
    let steps_per_interval = 8;
    for _ in 0..intervals * steps_per_interval {
        rig.os().advance_clock(IDLE_TICKS / steps_per_interval);
        for _ in 0..4 {
            let object = cache.alloc(busy).unwrap();
            cache.free(busy, object);
        }
        let fences = rig.os().calls(Call::Fence);
        heartbeats_on_cpu_1(&rig);
        heartbeats_on_cpu_1(&rig);
        if rig.os().calls(Call::Fence) > fences {
            assert_eq!(rig.os().last_fence(), Some(0));
            assert!(cpu_0_is_empty(&rig));
            drains.push(rig.os().ticks());
        }
    }
    // Each drain costs CPU 0 one slow path, to load a magazine again, and nothing else does.
    assert_eq!(slab.activity(), 1 + drains.len() as u64);
    assert!(drains.len() >= intervals as usize / 2, "{drains:?}");
    for pair in drains.windows(2) {
        assert!(
            pair[1] - pair[0] >= IDLE_TICKS,
            "drained twice in an interval: {drains:?}"
        );
    }
    cache.free(busy, held);
    rig.census(&[], false);
}

/// A CPU that stops allocating is drained once its slab has stood still for an interval, and
/// not again until it has been used.
#[test]
fn an_idle_cpu_is_drained_after_an_interval_and_again_only_after_use() {
    let rig = rig(12, TIGHT);
    let machine = SimCpu::new(0);
    let cache = rig.cache(&machine);
    let class = tiny_class();
    let mut user: Vec<_> = (0..3).filter_map(|_| cache.alloc(class)).collect();
    cache.free(class, user.pop().unwrap());
    assert!(!cpu_0_is_empty(&rig));
    // The first look records the slab; until an interval has passed, looks only wait.
    heartbeats_on_cpu_1(&rig);
    rig.os().advance_clock(IDLE_TICKS - (IDLE_TICKS >> 8));
    heartbeats_on_cpu_1(&rig);
    assert_eq!(rig.os().calls(Call::Fence), 0);
    rig.os().advance_clock(IDLE_TICKS >> 8);
    heartbeats_on_cpu_1(&rig);
    assert_eq!(rig.os().calls(Call::Fence), 1);
    assert!(cpu_0_is_empty(&rig));
    rig.census(&user, false);
    // Drained and still idle: nothing more to do, however long it stays so.
    for _ in 0..10 {
        rig.os().advance_clock(IDLE_TICKS);
        heartbeats_on_cpu_1(&rig);
    }
    assert_eq!(rig.os().calls(Call::Fence), 1);
    // Used again, it is drained again an interval after a look sees the use.
    cache.free(class, user.pop().unwrap());
    heartbeats_on_cpu_1(&rig);
    rig.os().advance_clock(IDLE_TICKS);
    heartbeats_on_cpu_1(&rig);
    assert_eq!(rig.os().calls(Call::Fence), 2);
    assert!(cpu_0_is_empty(&rig));
    rig.census(&user, false);
}

#[test]
fn a_signal_handler_may_allocate_at_every_step() {
    let mut fired = 0;
    for at in (0..).step_by(STRIDE) {
        let rig = rig(12, TIGHT);
        let machine = SimCpu::new(0);
        let user = RefCell::new(Vec::new());
        let handler = || {
            let cache = rig.cache(&machine);
            let class = tiny_class();
            if let Some(object) = cache.alloc(class) {
                cache.free(class, object);
            }
            // The sequence the signal interrupted, if any, restarts on the same CPU.
            machine.interrupt(machine.cpu_id_now());
        };
        let ((), hit) = interrupt_at(at, &handler, || workload(&rig, &machine, &user));
        rig.census(&user.borrow(), false);
        if !hit {
            break;
        }
        fired += 1;
    }
    assert!(
        fired * STRIDE > 100,
        "the workload has {fired} switch points"
    );
}

#[test]
fn an_interrupt_that_migrates_at_every_step_loses_nothing() {
    for at in (0..).step_by(STRIDE) {
        let rig = rig(12, TIGHT);
        let machine = SimCpu::new(0);
        let user = RefCell::new(Vec::new());
        let migrate = || machine.interrupt(1 - machine.cpu_id_now());
        let ((), hit) = interrupt_at(at, &migrate, || workload(&rig, &machine, &user));
        rig.census(&user.borrow(), false);
        if !hit {
            break;
        }
    }
}

#[test]
fn losing_the_cpu_registration_at_every_step_loses_nothing() {
    for at in (0..).step_by(STRIDE) {
        let rig = rig(12, TIGHT);
        let machine = SimCpu::new(0);
        let user = RefCell::new(Vec::new());
        let unregister = || machine.interrupt(-2);
        let ((), hit) = interrupt_at(at, &unregister, || {
            workload_without_moves(&rig, &machine, &user)
        });
        rig.census(&user.borrow(), false);
        if !hit {
            break;
        }
    }
}

/// The workload's operations without its CPU changes, so an interrupt's move sticks.
fn workload_without_moves(rig: &TestRig, machine: &SimCpu, user: &RefCell<Vec<NonNull<u8>>>) {
    let class = tiny_class();
    let cache = rig.cache(machine);
    for round in 0..3 {
        for _ in 0..3 + round {
            if let Some(object) = cache.alloc(class) {
                user.borrow_mut().push(object);
            }
        }
        for _ in 0..2 + round {
            let object = user.borrow_mut().pop();
            if let Some(object) = object {
                cache.free(class, object);
            }
        }
    }
}

/// What a thread that a fork leaves behind unwinds with.
struct Vanished;

/// Runs the workload, forking at switch point `at`. With `survives` the thread goes on (a fork
/// from a signal handler in this thread); without, it is one the child does not have. Then the
/// child runs on: a fresh thread allocates and frees on both CPUs and drains both, and the
/// census finds nothing in two places, and no marker or claim left.
fn fork_at(at: usize, survives: bool) -> bool {
    let rig = rig(12, TIGHT);
    let machine = SimCpu::new(0);
    let user = RefCell::new(Vec::new());
    let fork = || {
        // SAFETY: one thread, which is inside this call; nothing else touches the page.
        unsafe { rig.os().wipe_for_fork() };
        if !survives {
            panic::resume_unwind(Box::new(Vanished));
        }
    };
    let outcome = panic::catch_unwind(AssertUnwindSafe(|| {
        interrupt_at(at, &fork, || workload(&rig, &machine, &user)).1
    }));
    let hit = match outcome {
        Ok(hit) => hit,
        Err(payload) if payload.is::<Vanished>() => true,
        Err(payload) => panic::resume_unwind(payload),
    };
    // The child.
    let child = SimCpu::new(0);
    let cache = rig.cache(&child);
    let class = tiny_class();
    for n in [0, 1, 0] {
        child.set_cpu(n);
        let held: Vec<_> = (0..3).filter_map(|_| cache.alloc(class)).collect();
        for object in held {
            cache.free(class, object);
        }
    }
    child.set_cpu(1);
    for n in [0, 1] {
        assert_ne!(
            cache.drain(cpu(&rig, n)),
            Drained::Busy,
            "a stale claim blocks a drain"
        );
    }
    let census = rig.census(&user.borrow(), !survives);
    assert_eq!((census.markers, census.claims), (0, 0), "{census:?}");
    if survives {
        assert_eq!(census.lost_magazines + census.lost_objects, 0);
    }
    hit
}

#[test]
fn a_fork_the_thread_survives_at_every_step_leaves_a_clean_child() {
    let mut at = 0;
    while fork_at(at, true) {
        at += STRIDE;
    }
    assert!(at > 100);
}

#[test]
fn a_fork_the_thread_does_not_survive_at_every_step_duplicates_nothing() {
    let mut at = 0;
    while fork_at(at, false) {
        at += STRIDE;
    }
    assert!(at > 100);
}

#[test]
fn a_thread_without_a_cpu_is_served_by_the_source() {
    let rig = rig(4, Tuning::DEFAULT);
    let machine = SimCpu::new(-1);
    let cache = rig.cache(&machine);
    let class = tiny_class();
    let a = cache.alloc(class).unwrap();
    let b = cache.alloc(class).unwrap();
    cache.free(class, a);
    let census = rig.census(&[b], false);
    assert_eq!(
        census.magazines, 0,
        "no magazine is made for a thread without a CPU"
    );
    assert!(cache.routines().cpu().is_none());
}

#[test]
fn allocation_fails_cleanly_when_the_source_is_empty() {
    let rig = rig(3, TIGHT);
    let machine = SimCpu::new(0);
    let cache = rig.cache(&machine);
    let class = tiny_class();
    let held: Vec<_> = (0..5).filter_map(|_| cache.alloc(class)).collect();
    assert_eq!(held.len(), 3);
    rig.census(&held, false);
    for object in held {
        cache.free(class, object);
    }
    rig.census(&[], false);
}

#[test]
fn a_failed_fence_backs_the_drain_out() {
    let rig = rig(12, TIGHT);
    let machine = SimCpu::new(0);
    let user = RefCell::new(Vec::new());
    workload_without_moves(&rig, &machine, &user);
    let before = rig.census(&user.borrow(), false);
    rig.os().set_faults(Call::Fence, Faults::ALWAYS);
    let cache = rig.cache(&machine);
    assert_eq!(cache.drain(cpu(&rig, 0)), Drained::BackedOut);
    let after = rig.census(&user.borrow(), false);
    assert_eq!((after.markers, after.claims), (0, 0));
    assert_eq!(
        after.depot, before.depot,
        "the back-out restored every field"
    );
    rig.os().set_faults(Call::Fence, Faults::NONE);
    assert_eq!(cache.drain(cpu(&rig, 0)), Drained::Done);
    assert_eq!(
        rig.os().last_fence(),
        Some(0),
        "the fence aims at the drained CPU"
    );
    let drained = rig.census(&user.borrow(), false);
    assert_eq!(drained.spares, 0);
}

#[test]
fn drains_are_off_when_the_os_cannot_wipe_a_page_on_fork() {
    let rig = rig(12, TIGHT);
    rig.os().set_faults(Call::WipeOnFork, Faults::ALWAYS);
    let machine = SimCpu::new(0);
    let user = RefCell::new(Vec::new());
    workload(&rig, &machine, &user);
    let cache = rig.cache(&machine);
    assert_eq!(cache.drain(cpu(&rig, 0)), Drained::Disabled);
    assert_eq!(rig.os().calls(Call::Fence), 0);
    rig.census(&user.borrow(), false);
}

#[test]
fn the_depot_launders_what_is_over_its_target() {
    // Free many objects on one CPU: full magazines pile up in the depot, and the heartbeat
    // sends what is over the target back to the source.
    let rig = rig(24, TIGHT);
    let machine = SimCpu::new(0);
    let cache = rig.cache(&machine);
    let class = tiny_class();
    let held: Vec<_> = (0..24).filter_map(|_| cache.alloc(class)).collect();
    assert_eq!(held.len(), 24);
    for object in held {
        cache.free(class, object);
    }
    let census = rig.census(&[], false);
    let in_depot = rig
        .depot
        .full(class)
        .objects(cache.area(), class.magazine()) as usize;
    assert!(in_depot * class.size() <= TIGHT.depot_target + 2 * class.size());
    assert!(rig.source.held().len() >= 12, "{census:?}");
}

#[test]
fn a_stale_marker_is_installed_over_and_its_slab_drained() {
    // A fork cuts a drain of CPU 0 off after it marked every field; the child must use CPU 0
    // as if the markers were not there, and drain them away.
    let rig = rig(12, TIGHT);
    let machine = SimCpu::new(0);
    let user = RefCell::new(Vec::new());
    workload_without_moves(&rig, &machine, &user);
    let parent = rig.cache(&machine);
    let generation = parent.generation().unwrap();
    for field in crate::front::Slab::drained_fields() {
        rig.slabs()
            .slab(cpu(&rig, 0))
            .field(field)
            .store(generation.marker(), core::sync::atomic::Ordering::Relaxed);
    }
    rig.slabs()
        .slab(cpu(&rig, 0))
        .claim()
        .store(generation.get(), core::sync::atomic::Ordering::Relaxed);
    // SAFETY: one thread, outside the allocator.
    unsafe { rig.os().wipe_for_fork() };
    let child = SimCpu::new(0);
    let cache = rig.cache(&child);
    assert_ne!(cache.generation(), Some(generation));
    let class = tiny_class();
    let object = cache
        .alloc(class)
        .expect("the child allocates on the marked CPU");
    cache.free(class, object);
    let census = rig.census(&user.borrow(), true);
    assert_eq!((census.markers, census.claims), (0, 0), "{census:?}");
}

#[test]
fn every_class_works() {
    let classes: Vec<SmallClass> = SmallClass::all().collect();
    let rig: Rig<SimOs<Small>, 2> = Rig::new(&classes, 3, Tuning::DEFAULT);
    let machine = SimCpu::new(1);
    let cache = rig.cache(&machine);
    let mut held = Vec::new();
    for &class in &classes {
        for _ in 0..3 {
            if let Some(object) = cache.alloc(class) {
                held.push((class, object));
            }
        }
    }
    let user: Vec<_> = held.iter().map(|&(_, object)| object).collect();
    // The magazine area of the small shape has room for some magazines, not one per class;
    // when it runs out the slow path serves straight from the source.
    assert_eq!(held.len(), 3 * classes.len());
    rig.census(&user, false);
    for (class, object) in held {
        cache.free(class, object);
    }
    rig.census(&[], false);
}

/// Three real threads, on CPU 0, CPU 1 and no CPU, allocate and free objects that cross between
/// them, so that the slow paths, spares and depot stacks run in parallel; under Miri this checks
/// them for data races. Drains are off: the simulated fence does not wait for a twin running
/// on another thread.
#[test]
fn threads_that_trade_objects_in_parallel_lose_nothing() {
    let rig = rig(16, TIGHT);
    rig.os().set_faults(Call::WipeOnFork, Faults::ALWAYS);
    let traded = std::sync::Mutex::new(Vec::<usize>::new());
    let rounds = if cfg!(miri) { 3 } else { 200 };
    std::thread::scope(|scope| {
        for cpu in [0, 1, -1] {
            let (rig, traded) = (&rig, &traded);
            scope.spawn(move || {
                let machine = SimCpu::new(cpu);
                let cache = rig.cache(&machine);
                let class = tiny_class();
                for _ in 0..rounds {
                    for _ in 0..2 {
                        if let Some(object) = cache.alloc(class) {
                            traded.lock().unwrap().push(object.addr().get());
                        }
                    }
                    for _ in 0..2 {
                        let taken = traded.lock().unwrap().pop();
                        if let Some(address) = taken {
                            let object =
                                NonNull::new(core::ptr::without_provenance_mut(address)).unwrap();
                            cache.free(class, object);
                        }
                    }
                }
            });
        }
    });
    let user: Vec<_> = traded
        .into_inner()
        .unwrap()
        .into_iter()
        .map(|a| NonNull::new(core::ptr::without_provenance_mut(a)).unwrap())
        .collect();
    rig.census(&user, false);
}
