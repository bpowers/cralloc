//! Scenario tests of the middle tier on one thread over the simulated OS.
//!
//! The heaviest run a workload that crosses every slow path (fill, install, retire to spares and
//! the depot, launder, drain) and interrupt it at every switch point in turn, that is, before
//! every atomic instruction of the slow paths and every step of every twin: with a signal
//! handler that allocates and frees, with a fork that the thread survives, and with a fork that
//! the thread does not survive. After each run a census checks that every magazine and every
//! object is in exactly one place.

use super::test_rig::{Rig, SimCpu, Small, TIGHT, tiny_class};
use super::{Drained, Tuning};
use crate::classes::SmallClass;
use crate::front::check::interrupt_at;
use crate::front::{Cpu, Routines};
use crate::os::sim::{Call, Faults, SimOs};
use core::cell::RefCell;
use core::ptr::NonNull;
use std::boxed::Box;
use std::panic::{self, AssertUnwindSafe};
use std::vec::Vec;

type TestRig = Rig<SimOs<Small>, 2>;

fn rig(count: usize, tuning: Tuning) -> TestRig {
    Rig::new(&[tiny_class()], count, tuning)
}

fn cpu(rig: &TestRig, n: i32) -> Cpu {
    rig.slabs().cpu(n).unwrap()
}

/// Allocations and frees on CPU 0, then on CPU 1 long enough for the heartbeat to find CPU 0
/// idle and drain it. `user` holds what the program holds.
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
    free(3);
    alloc(3);
    free(2);
    alloc(1);
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

#[test]
fn a_signal_handler_may_allocate_at_every_step() {
    let mut fired = 0;
    for at in 0.. {
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
    assert!(fired > 100, "the workload has {fired} switch points");
}

#[test]
fn an_interrupt_that_migrates_at_every_step_loses_nothing() {
    for at in 0.. {
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
    for at in 0.. {
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
        at += 1;
    }
    assert!(at > 100);
}

#[test]
fn a_fork_the_thread_does_not_survive_at_every_step_duplicates_nothing() {
    let mut at = 0;
    while fork_at(at, false) {
        at += 1;
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
