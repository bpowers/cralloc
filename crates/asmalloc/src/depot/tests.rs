//! Scenario tests of the middle tier on one thread over the simulated OS.
//!
//! The heaviest run a workload that crosses every slow path (fill, install, retire to spares and
//! the depot, launder, drain) and interrupt it at every switch point in turn, that is, before
//! every atomic instruction of the slow paths and every step of every twin: with a signal
//! handler that allocates and frees, with a fork that the thread survives, and with a fork that
//! the thread does not survive. After each run a census checks that every magazine and every
//! object is in exactly one place.

use super::siblings::{SIBLINGS, stride};
use super::test_rig::{Rig, SimCpu, Small, TIGHT, tiny_class};
use super::{Drained, IDLE_TICKS, Source, Tuning};
use crate::classes::{Class, SmallClass};
use crate::front::check::interrupt_at;
use crate::front::{Cpu, Routines, Spare};
use crate::os::Os;
use crate::os::sim::{Call, Faults, SimOs};
use core::cell::RefCell;
use core::ptr::NonNull;
use core::sync::atomic::Ordering::Relaxed;
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

/// A magazine filled from the source hands its objects out lowest address first, whether the
/// source filled it whole or, running out, only partly.
#[test]
fn a_filled_magazine_hands_out_ascending_addresses() {
    let class = Class::of_size(64).unwrap().small().unwrap();
    let capacity = class.magazine().capacity();
    for count in [capacity, capacity + capacity / 2] {
        let rig: TestRig = Rig::new(&[class], count, Tuning::DEFAULT);
        let machine = SimCpu::new(0);
        let cache = rig.cache(&machine);
        let got: Vec<usize> = (0..count)
            .map(|_| cache.alloc(class).unwrap().addr().get())
            .collect();
        assert!(got.is_sorted(), "{count}: {got:x?}");
        assert!(cache.alloc(class).is_none(), "the source is empty");
    }
}

/// Whether objects of `size` bytes at `a` and `b` share a cache line.
fn share_a_line(a: usize, b: usize, size: usize) -> bool {
    let (low, high) = (a.min(b), a.max(b));
    (low + size - 1) / 64 == high / 64
}

/// A class smaller than a line hands out the first objects of a fill that opens a run a stride
/// apart and the rest in ascending order: every object of the fill once, and no two handed out
/// one after another among the siblings, or from the last of them to the next, on a shared
/// line. The fill after it in the same interval continues the run and hands out ascending,
/// adjacent addresses; the first fill after an interval with none opens a run again.
#[test]
fn a_fill_that_opens_a_run_hands_out_siblings_on_lines_of_their_own() {
    let classes: Vec<SmallClass> = SmallClass::all().filter(|c| c.size() < 64).collect();
    assert_eq!(classes.len(), 7);
    for class in classes {
        let size = class.size();
        let capacity = class.magazine().capacity();
        let siblings = SIBLINGS * stride(size).unwrap();
        let rig: TestRig = Rig::new(&[class], 3 * capacity, Tuning::DEFAULT);
        let machine = SimCpu::new(0);
        let cache = rig.cache(&machine);
        let mut held = Vec::new();
        let mut magazine = || -> Vec<usize> {
            let got: Vec<NonNull<u8>> =
                (0..capacity).map(|_| cache.alloc(class).unwrap()).collect();
            held.extend(&got);
            got.iter().map(|p| p.addr().get()).collect()
        };
        let adjacent = |got: &[usize]| got.windows(2).all(|pair| pair[1] == pair[0] + size);
        let strided = |got: &[usize], first: usize| {
            for pair in got[..=siblings].windows(2) {
                assert!(!share_a_line(pair[0], pair[1], size), "{size}: {pair:x?}");
            }
            assert!(adjacent(&got[siblings..]), "{size}: the rest in order");
            let mut sorted = got.to_vec();
            sorted.sort_unstable();
            let all: Vec<usize> = (first..first + capacity)
                .map(|n| super::test_rig::object(class, n).addr().get())
                .collect();
            assert_eq!(sorted, all, "{size}: every object of the fill, once");
        };
        strided(&magazine(), 0);
        assert!(
            adjacent(&magazine()),
            "{size}: a fill that continues the run is dense"
        );
        rig.os().advance_clock(IDLE_TICKS);
        rig.source.clock().now(rig.os());
        strided(&magazine(), 2 * capacity);
        rig.census(&held, false);
    }
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

/// A swing that straddles a magazine's end, two allocations and then two frees from a level of
/// one, crossed both ends on every repetition when every swap installed a magazine at its end:
/// two slow paths a repetition for as long as it ran. The swaps alternate, and the one that
/// reverses a reversal (the third) leaves the loaded magazine half full; the swing then stays
/// inside it: no slow path after the first few repetitions.
#[test]
fn a_swing_across_a_magazines_end_stops_taking_slow_paths_after_the_reversal() {
    let class = SmallClass::from_ordinal(0).unwrap();
    let rig: TestRig = Rig::new(&[class], 4 * class.magazine().capacity(), Tuning::DEFAULT);
    let machine = SimCpu::new(0);
    let cache = rig.cache(&machine);
    let slab = rig.slabs().slab(cpu(&rig, 0));
    // A free loads an empty magazine at a level of one.
    cache.free(class, rig.source.alloc_one(class).unwrap());
    let mut held = Vec::new();
    let mut slow_paths = Vec::new();
    for _ in 0..20 {
        let before = slab.activity();
        for _ in 0..2 {
            held.push(cache.alloc(class).unwrap());
        }
        for object in held.drain(..).rev() {
            cache.free(class, object);
        }
        slow_paths.push(slab.activity() != before);
    }
    assert!(
        slow_paths[4..].iter().all(|&slow| !slow),
        "slow paths in each repetition: {slow_paths:?}"
    );
    rig.census(&[], false);
}

/// A CPU whose slow paths are all swaps with its own spares, each of which moves less than a
/// heartbeat's bytes, counts each swap's magazine towards its next beat, so it still beats, and
/// still counts as active: another CPU's idle checks, which look at it on every one of their
/// heartbeats, never drain it while it swaps.
#[test]
fn a_cpu_that_only_swaps_with_its_spares_beats_and_is_not_drained() {
    let small = SmallClass::from_ordinal(0).unwrap();
    // Without the refresh, which launders one displaced magazine in so many, the sweep, which
    // launders a full spare an interval, and the swing rule, which would leave these swings'
    // magazines half full, every slow path is a swap.
    let tuning = Tuning {
        look_every: 1,
        refresh_every: 0,
        age_limit: false,
        centre_swings: false,
        ..Tuning::DEFAULT
    };
    let rig: TestRig = Rig::new(&[small, tiny_class()], 8192, tuning);
    let machine = SimCpu::new(0);
    let cache = rig.cache(&machine);
    let slab = rig.slabs().slab(cpu(&rig, 0));
    // A burst a little over a magazine, allocated and freed in reverse: once the first two have
    // stocked the spares, one swap with a full spare and one with an empty spare each time.
    let burst = small.magazine().capacity() + 100;
    let exchange = (small.magazine().capacity() * small.size()) as u64;
    assert!(exchange < super::heartbeat::BEAT_BYTES);
    let swing = || {
        let held: Vec<_> = (0..burst).filter_map(|_| cache.alloc(small)).collect();
        assert_eq!(held.len(), burst);
        for object in held.into_iter().rev() {
            cache.free(small, object);
        }
    };
    swing();
    swing();
    let (beats, steps) = (slab.beats(), 8);
    let source = || (rig.source.fills(), rig.source.laundered());
    let spares = || {
        let depth = |kind| cache.spare_depth(small, kind);
        (depth(Spare::Full), depth(Spare::Empty))
    };
    let mut drained = 0;
    for _ in 0..4 * steps {
        rig.os().advance_clock(IDLE_TICKS / steps);
        for _ in 0..8 {
            let before = (slab.activity(), source(), spares());
            swing();
            assert_eq!(
                slab.activity() - before.0,
                2 * exchange,
                "two swaps' magazines"
            );
            assert_eq!((source(), spares()), (before.1, before.2), "only swaps");
        }
        let fences = rig.os().calls(Call::Fence);
        heartbeats_on_cpu_1(&rig);
        if rig.os().calls(Call::Fence) > fences && rig.os().last_fence() == Some(0) {
            drained += 1;
        }
    }
    assert!(slab.beats() > beats, "the swaps beat");
    assert_eq!(drained, 0, "a CPU busy swapping was drained");
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
    // The activity counter counts the bytes a full magazine holds for each exchange.
    let exchange = (busy.magazine().capacity() * busy.size()) as u64;
    assert_eq!(slab.activity(), (1 + drains.len() as u64) * exchange);
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

/// CPU 0 with magazines parked in its slab, then idle; the clock at `start`.
fn an_idle_cpu_0(start: u64) -> (TestRig, Vec<NonNull<u8>>) {
    let rig = rig(12, TIGHT);
    let machine = SimCpu::new(0);
    let cache = rig.cache(&machine);
    let class = tiny_class();
    let mut user: Vec<_> = (0..3).filter_map(|_| cache.alloc(class)).collect();
    cache.free(class, user.pop().unwrap());
    assert!(!cpu_0_is_empty(&rig));
    rig.os().advance_clock(start);
    (rig, user)
}

/// After the clock goes back (a resume, a restored checkpoint), an idle slab is drained one
/// interval after the first look that sees the new time, not when the clock catches up.
#[test]
fn a_clock_that_goes_back_delays_a_drain_by_one_interval_only() {
    let (rig, user) = an_idle_cpu_0(500 * IDLE_TICKS);
    heartbeats_on_cpu_1(&rig);
    rig.os().advance_clock(0u64.wrapping_sub(400 * IDLE_TICKS));
    heartbeats_on_cpu_1(&rig);
    assert_eq!(rig.os().calls(Call::Fence), 0);
    rig.os().advance_clock(IDLE_TICKS);
    heartbeats_on_cpu_1(&rig);
    assert_eq!(rig.os().calls(Call::Fence), 1);
    assert!(cpu_0_is_empty(&rig));
    rig.census(&user, false);
}

/// A silence counts as one interval (`crate::clock`): an idle slab that a look saw half an
/// interval before a long silence is not drained by the first looks after it, which find it
/// quiet for half an interval, and is drained once heartbeats have run for an interval more.
/// Read off the cycle counter, the first look after the silence drained it.
#[test]
fn a_silence_ages_an_idle_slab_by_at_most_one_interval() {
    let (rig, user) = an_idle_cpu_0(IDLE_TICKS / 2);
    heartbeats_on_cpu_1(&rig);
    rig.os().advance_clock(50 * IDLE_TICKS);
    heartbeats_on_cpu_1(&rig);
    assert_eq!(
        rig.os().calls(Call::Fence),
        0,
        "drained by the first look after a silence"
    );
    assert!(!cpu_0_is_empty(&rig));
    rig.os().advance_clock(IDLE_TICKS);
    heartbeats_on_cpu_1(&rig);
    assert_eq!(rig.os().calls(Call::Fence), 1);
    assert!(cpu_0_is_empty(&rig));
    rig.census(&user, false);
}

/// A look whose drain finds the slab claimed by someone else (here a claim left as if by a
/// `collect` that then backed out) tries again an interval later, so an idle slab with
/// magazines parked is still drained.
#[test]
fn a_drain_that_finds_the_slab_busy_is_tried_again_an_interval_later() {
    let (rig, user) = an_idle_cpu_0(7 * IDLE_TICKS);
    let machine = SimCpu::new(1);
    let cache = rig.cache(&machine);
    let claim = rig.slabs().slab(cpu(&rig, 0)).claim();
    heartbeats_on_cpu_1(&rig);
    rig.os().advance_clock(IDLE_TICKS);
    claim.store(cache.generation().unwrap().get(), Relaxed);
    heartbeats_on_cpu_1(&rig);
    assert!(!cpu_0_is_empty(&rig), "the busy drain took nothing");
    // The other drainer gives up without draining.
    claim.store(0, Relaxed);
    heartbeats_on_cpu_1(&rig);
    assert!(!cpu_0_is_empty(&rig), "the next try waits an interval");
    rig.os().advance_clock(IDLE_TICKS);
    heartbeats_on_cpu_1(&rig);
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
    let in_pool = |pool| {
        rig.depot
            .full(pool, class)
            .count(cache.area(), class.magazine()) as usize
    };
    let own = in_pool(super::Pool::Domain(super::Domain::ZERO));
    let shared = in_pool(super::Pool::Shared);
    // The domain's stack takes a magazine while it is under its cap; the shared one is
    // laundered down to its target, here a magazine.
    let magazine = class.magazine().capacity();
    assert!(own * class.size() <= TIGHT.domain_cap + magazine * class.size());
    assert!(shared * class.size() <= TIGHT.depot_target - TIGHT.domain_cap + class.size());
    assert!(
        shared > 0,
        "the domain's stack overflowed into the shared one"
    );
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

/// CPU 0 allocates and frees `count` objects of the tiny class and goes quiet, leaving objects
/// in its loaded magazine, its spares and the depot.
fn park_on_cpu_0(rig: &TestRig, count: usize) {
    let machine = SimCpu::new(0);
    let cache = rig.cache(&machine);
    let class = tiny_class();
    let held: Vec<_> = (0..count).filter_map(|_| cache.alloc(class)).collect();
    assert_eq!(held.len(), count);
    for object in held {
        cache.free(class, object);
    }
    assert!(!cpu_0_is_empty(rig));
}

/// What `maintain` changes that a test can see: the fences made, the objects the source holds,
/// and the hints the clocked rules keep (each slab's sighting, and the depot's water words and
/// sets of owing classes, in the one domain and the shared pool).
#[derive(PartialEq, Eq, Debug)]
struct Trace {
    fences: u64,
    held: Vec<usize>,
    sightings: Vec<u64>,
    water: Vec<u64>,
    owing: Vec<u64>,
}

fn trace(rig: &TestRig) -> Trace {
    let mut held = rig.source.held();
    held.sort_unstable();
    let pools = [
        super::Pool::Domain(super::Domain::ZERO),
        super::Pool::Shared,
    ];
    Trace {
        fences: rig.os().calls(Call::Fence),
        held,
        sightings: rig
            .slabs()
            .cpus()
            .map(|cpu| rig.slabs().slab(cpu).seen().load(Relaxed))
            .collect(),
        water: pools
            .iter()
            .flat_map(|&pool| {
                (0..crate::classes::SMALL_COUNT)
                    .map(move |o| rig.depot.water(pool, o).load(Relaxed))
            })
            .collect(),
        owing: pools
            .iter()
            .map(|&pool| rig.depot.owing(pool).load(Relaxed))
            .collect(),
    }
}

/// A quiet heap, with no slow path anywhere: `maintain` once an interval drains the idle slab
/// (laundering what it takes) an interval after its first look, and the depot sheds what it held
/// at the first look in the interval after the last one in which its stack was empty, so that
/// everything parked is back in the source. CPU 0 found its stack empty when it began to park,
/// in the first interval, so its stack sheds at the third call.
#[test]
fn maintain_returns_what_a_quiet_heap_parked() {
    let rig = rig(12, TIGHT);
    park_on_cpu_0(&rig, 8);
    assert!(rig.source.held().len() < 12);
    let steps = rig.source.purges();
    let keeper = SimCpu::new(1);
    let cache = rig.cache(&keeper);
    cache.maintain();
    assert_eq!(
        rig.os().calls(Call::Fence),
        0,
        "the first look only records"
    );
    rig.os().advance_clock(IDLE_TICKS);
    cache.maintain();
    assert_eq!(rig.os().calls(Call::Fence), 1);
    assert_eq!(rig.os().last_fence(), Some(0));
    assert!(cpu_0_is_empty(&rig));
    rig.os().advance_clock(IDLE_TICKS);
    cache.maintain();
    assert_eq!(rig.source.held().len(), 12, "{:?}", rig.census(&[], false));
    // `maintain` is not a slow path of the CPU it runs on.
    assert_eq!(rig.slabs().slab(cpu(&rig, 1)).activity(), 0);
    let census = rig.census(&[], false);
    assert_eq!((census.markers, census.claims), (0, 0));
    assert_eq!(rig.source.passes(), 3, "each call asks for the purge pass");
    assert_eq!(rig.source.purges(), steps, "and takes no heartbeat's step");
}

/// The clock paces `maintain` as it paces the heartbeats: a second call in the same interval
/// finds nothing to do, whatever the first did.
#[test]
fn maintain_twice_in_an_interval_changes_nothing_the_second_time() {
    let rig = rig(12, TIGHT);
    park_on_cpu_0(&rig, 8);
    let keeper = SimCpu::new(1);
    let cache = rig.cache(&keeper);
    let mut changed = 0;
    for _ in 0..4 {
        let before = trace(&rig);
        cache.maintain();
        let first = trace(&rig);
        changed += usize::from(first != before);
        cache.maintain();
        assert_eq!(trace(&rig), first);
        rig.os().advance_clock(IDLE_TICKS);
        // The CPU is used again between intervals.
        park_on_cpu_0(&rig, 3);
    }
    assert!(changed >= 3, "the first calls of an interval did something");
    rig.census(&[], false);
}

/// `maintain` runs one round in each counted interval and returns at once for every other call
/// in it, so a timer or a hook may call it freely; its own calls step the count through a
/// silence, so called once an interval it runs once an interval.
#[test]
fn maintain_runs_one_round_an_interval() {
    let rig = rig(12, TIGHT);
    let machine = SimCpu::new(0);
    let cache = rig.cache(&machine);
    cache.maintain();
    assert_eq!(rig.source.passes(), 1);
    for _ in 0..15 {
        rig.os().advance_clock(IDLE_TICKS / 16);
        cache.maintain();
    }
    assert_eq!(rig.source.passes(), 1, "two rounds in an interval");
    rig.os().advance_clock(IDLE_TICKS / 16);
    cache.maintain();
    assert_eq!(rig.source.passes(), 2);
    for round in 3..8 {
        rig.os().advance_clock(IDLE_TICKS);
        cache.maintain();
        cache.maintain();
        assert_eq!(rig.source.passes(), round);
    }
}

/// `maintain`'s idle look sees a CPU that is busy only on fast paths stand still, as a
/// heartbeat's does: it drains it, and its objects go back to the source, at most once an
/// interval however often `maintain` runs.
#[test]
fn maintain_drains_a_cpu_busy_on_fast_paths_at_most_once_an_interval() {
    let busy = Class::of_size(64).unwrap().small().unwrap();
    let rig: TestRig = Rig::new(&[busy, tiny_class()], 1024, Tuning::DEFAULT);
    let machine = SimCpu::new(0);
    let cache = rig.cache(&machine);
    let held = cache.alloc(busy).unwrap();
    let keeper = rig.cache(SimCpu::new(1));
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
        keeper.maintain();
        keeper.maintain();
        if rig.os().calls(Call::Fence) > fences {
            assert_eq!(rig.os().last_fence(), Some(0));
            assert!(cpu_0_is_empty(&rig));
            // Laundered, not parked: everything but the block the program holds is back.
            assert_eq!(rig.source.held().len(), 2 * 1024 - 1);
            drains.push(rig.os().ticks());
        }
    }
    // The activity counter counts the bytes a full magazine holds for each exchange: one
    // slow path to start, and a refill after each drain but perhaps the last, which can fall
    // in the last step.
    let exchange = (busy.magazine().capacity() * busy.size()) as u64;
    assert_eq!(slab.activity() % exchange, 0);
    let refills = slab.activity() / exchange - 1;
    assert!(
        refills == drains.len() as u64 || refills + 1 == drains.len() as u64,
        "{refills} refills after {} drains",
        drains.len()
    );
    assert!(drains.len() >= intervals as usize / 2 - 1, "{drains:?}");
    for pair in drains.windows(2) {
        assert!(
            pair[1] - pair[0] >= IDLE_TICKS,
            "drained twice in an interval: {drains:?}"
        );
    }
    cache.free(busy, held);
    rig.census(&[], false);
}

/// Without drains (no fence), `maintain` still sheds the depot and leaves every slab alone.
#[test]
fn maintain_without_drains_sheds_the_depot() {
    let rig = rig(12, TIGHT);
    rig.os().set_faults(Call::RegisterFence, Faults::ALWAYS);
    park_on_cpu_0(&rig, 8);
    let keeper = rig.cache(SimCpu::new(1));
    let class = tiny_class();
    let area = keeper.area();
    let parked = || {
        [
            super::Pool::Domain(super::Domain::ZERO),
            super::Pool::Shared,
        ]
        .map(|pool| rig.depot.full(pool, class).count(area, class.magazine()))
    };
    assert!(parked()[0] > 0, "{:?}", parked());
    for _ in 0..3 {
        keeper.maintain();
        rig.os().advance_clock(IDLE_TICKS);
    }
    assert_eq!(rig.os().calls(Call::Fence), 0);
    assert_eq!(parked(), [0, 0]);
    assert!(!cpu_0_is_empty(&rig));
    rig.census(&[], false);
}

/// Forks at switch point `at` of a `maintain` that drains an idle CPU and sheds the depot,
/// from a thread that does or does not survive. The child goes on as in `fork_at`, then keeps
/// maintaining: nothing is in two places, no marker or claim is left, and nothing the parent
/// left blocks it.
fn fork_in_maintain_at(at: usize, survives: bool) -> bool {
    let rig = rig(12, TIGHT);
    park_on_cpu_0(&rig, 8);
    let keeper = SimCpu::new(1);
    rig.cache(&keeper).maintain();
    rig.os().advance_clock(IDLE_TICKS);
    let fork = || {
        // SAFETY: one thread, which is inside this call; nothing else touches the page.
        unsafe { rig.os().wipe_for_fork() };
        if !survives {
            panic::resume_unwind(Box::new(Vanished));
        }
    };
    let outcome = panic::catch_unwind(AssertUnwindSafe(|| {
        interrupt_at(at, &fork, || rig.cache(&keeper).maintain()).1
    }));
    let hit = match outcome {
        Ok(hit) => hit,
        Err(payload) if payload.is::<Vanished>() => true,
        Err(payload) => panic::resume_unwind(payload),
    };
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
        assert_ne!(cache.drain(cpu(&rig, n)), Drained::Busy);
    }
    for _ in 0..3 {
        rig.os().advance_clock(IDLE_TICKS);
        cache.maintain();
    }
    let census = rig.census(&[], !survives);
    assert_eq!((census.markers, census.claims), (0, 0), "{census:?}");
    if survives {
        assert_eq!(census.lost_magazines + census.lost_objects, 0);
    }
    hit
}

#[test]
fn a_fork_the_thread_survives_at_every_step_of_maintain_leaves_a_clean_child() {
    let mut at = 0;
    while fork_in_maintain_at(at, true) {
        at += STRIDE;
    }
    assert!(at > 20, "maintain has {at} switch points");
}

#[test]
fn a_fork_the_thread_does_not_survive_at_every_step_of_maintain_duplicates_nothing() {
    let mut at = 0;
    while fork_in_maintain_at(at, false) {
        at += STRIDE;
    }
    assert!(at > 20, "maintain has {at} switch points");
}

/// `move_top_into` moves the top of one magazine, in order, to the bottom of another, sets both
/// levels, and cuts what it is asked to move to what the levels and capacity allow.
#[test]
fn moving_the_top_of_a_magazine_keeps_its_order_and_both_levels() {
    let class = tiny_class();
    let capacity = class.magazine().capacity();
    let rig: TestRig = Rig::new(&[class], 64, TIGHT);
    let machine = SimCpu::new(0);
    let cache = rig.cache(&machine);
    let area = cache.area();
    let fill = |magazine: &super::magazine::Magazine<'_>, level: usize, first: usize| {
        for slot in 1..=level {
            magazine.set_object(slot, super::test_rig::object(class, first + slot));
        }
        magazine.set_level(level);
    };
    let contents = |magazine: &super::magazine::Magazine<'_>| -> Vec<usize> {
        magazine.objects()[..magazine.level()]
            .iter()
            .map(|slot| slot.load(Relaxed).addr())
            .collect()
    };
    let wanted = |range: core::ops::RangeInclusive<usize>| -> Vec<usize> {
        range
            .map(|n| super::test_rig::object(class, n).addr().get())
            .collect()
    };
    let from = area.create(class.magazine()).unwrap();
    let to = area.create(class.magazine()).unwrap();
    for level in 0..=capacity {
        for n in 0..=capacity + 1 {
            fill(&from, level, 0);
            fill(&to, capacity, 100);
            from.move_top_into(level, &to, n);
            let moved = n.min(level);
            assert_eq!(contents(&from), wanted(1..=level - moved), "{level} {n}");
            assert_eq!(
                contents(&to),
                wanted(level - moved + 1..=level),
                "{level} {n}"
            );
        }
    }
}

/// The background thread's word (`thread`).
#[path = "thread_tests.rs"]
mod thread_tests;
