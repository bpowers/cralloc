//! The middle tier's protocols under every schedule the interleaving checker explores
//! (`front::check`), against the properties the TLA+ models state:
//!
//! - every magazine has exactly one owner, and no object is in two places or lost
//!   (`Exchange.tla` OneOwner, `Drain.tla` NoDouble and NoLoss, `Depot.tla` OneOwner);
//! - no thread waits: every execution ends within the checker's step bound;
//! - no marker or claim outlives its drain (`Drain.tla` Clean);
//! - after a fork, nothing is duplicated, and the child's drains treat what the parent left as
//!   absent (`Drain_gen_any`, `Fork_gen_any`).
//!
//! Threads run on the simulated OS's threads and CPUs (`os::sim::SimThread`): a thread's
//! sequence is in flight on its CPU from its first step to its commit, and another thread
//! starting a sequence on that CPU, or a fence of it, aborts the sequence, as preemption and
//! `membarrier` do. A thread may also migrate at the start of any sequence, a bounded number of
//! times per execution. The census of `test_rig.rs` checks the invariants after every execution.

use super::heartbeat::Clock;
use super::test_rig::{Rig, Small, TIGHT, tiny_class};
use super::{Drained, IDLE_TICKS, Source};
use crate::front::check::{Bounds, Report, choose, explore, switch_point};
use crate::os::sim::{Call, Faults, SimOs, SimThread};
use crate::os::{Aborted, Machine, Os, Step};
use core::ptr::NonNull;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::vec::Vec;

const CPUS: usize = 2;
const THREADS: usize = 3;

/// The CPU of one checker thread: a simulated thread, with the checker's switch point before
/// every step and the migrations it may choose at the start of a sequence.
struct CheckCpu<'a> {
    thread: SimThread<'a>,
    migrations: &'a AtomicUsize,
}

impl Machine for CheckCpu<'_> {
    fn cpu_id(&self) -> i32 {
        self.thread.cpu_id()
    }

    fn step(&self, step: Step) -> Result<(), Aborted> {
        switch_point();
        if step == Step::ReadCpu {
            let budget = self.migrations.load(Ordering::SeqCst);
            if budget > 0 && choose(2) == 1 {
                self.migrations.store(budget - 1, Ordering::SeqCst);
                let next = (self.thread.cpu_id() + 1) % CPUS as i32;
                self.thread.set_cpu(next);
            }
        }
        self.thread.step(step)
    }

    fn accessed(&self) {
        self.thread.accessed();
    }

    fn committed(&self) {
        self.thread.committed();
    }
}

/// One execution's heap, and the objects the program holds.
struct World {
    rig: Rig<SimOs<Small>, CPUS>,
    migrations: AtomicUsize,
    user: Mutex<Vec<usize>>,
}

impl World {
    fn new(objects: usize, migrations: usize) -> World {
        World {
            rig: Rig::with_os(SimOs::new(), &[tiny_class()], objects, TIGHT),
            migrations: AtomicUsize::new(migrations),
            user: Mutex::new(Vec::new()),
        }
    }

    fn machine(&self, me: usize, cpu: usize) -> CheckCpu<'_> {
        let thread = self.rig.os().thread(me);
        thread.set_cpu(cpu as i32);
        CheckCpu {
            thread,
            migrations: &self.migrations,
        }
    }

    fn alloc(&self, machine: &CheckCpu<'_>) {
        if let Some(object) = self.rig.cache(machine).alloc(tiny_class()) {
            self.user.lock().unwrap().push(object.addr().get());
        }
    }

    fn free(&self, machine: &CheckCpu<'_>) {
        let object = self.user.lock().unwrap().pop();
        if let Some(address) = object {
            let object = NonNull::new(core::ptr::without_provenance_mut(address)).unwrap();
            self.rig.cache(machine).free(tiny_class(), object);
        }
    }

    fn user(&self) -> Vec<NonNull<u8>> {
        self.user
            .lock()
            .unwrap()
            .iter()
            .map(|&a| NonNull::new(core::ptr::without_provenance_mut(a)).unwrap())
            .collect()
    }

    /// Single-threaded set-up: CPU 0 with a loaded magazine and spares, so that a drain has
    /// something to take.
    fn populate(&self) {
        let machine = self.machine(THREADS - 1, 0);
        for _ in 0..4 {
            self.alloc(&machine);
        }
        self.free(&machine);
        self.free(&machine);
    }

    /// The census, and that no marker or claim is left.
    fn check(&self, lossy: bool) {
        let census = self.rig.census(&self.user(), lossy);
        assert_eq!((census.markers, census.claims), (0, 0), "{census:?}");
    }
}

/// What a mutator thread does, in order.
#[derive(Clone, Copy)]
enum Op {
    Alloc,
    Free,
}

/// The usual mutator: an allocation and two frees, which cross the slow paths both ways.
const MUTATE: &[Op] = &[Op::Alloc, Op::Free, Op::Free];

impl World {
    fn run(&self, machine: &CheckCpu<'_>, ops: &[Op]) {
        for op in ops {
            match op {
                Op::Alloc => self.alloc(machine),
                Op::Free => self.free(machine),
            }
        }
    }
}

fn report(name: &str, report: Report) {
    std::eprintln!("{name}: {report:?}");
    assert!(
        report.complete,
        "{name} did not finish within the execution bound"
    );
}

/// Two threads exchange magazines on one CPU, either of which may migrate once: allocation and
/// free race through spares, depot and installs (`Exchange.tla`).
fn exchanges_keep_one_owner_under_every_schedule(bounds: Bounds) {
    let found = explore(
        bounds,
        2,
        || World::new(6, 1),
        |world, me| {
            let machine = world.machine(me, 0);
            world.alloc(&machine);
            world.alloc(&machine);
            world.free(&machine);
            world.alloc(&machine);
        },
        |_| {},
        |world, _| world.check(false),
    );
    report("exchange", found);
}

/// A drain of CPU 0 races a thread allocating and freeing there (`Drain_claim`, protocol
/// "gen"): whatever a sequence in flight commits wins, and the drain takes exactly the rest.
fn a_drain_and_a_mutator_keep_one_owner_under_every_schedule(
    bounds: Bounds,
    ops: &[Op],
    migrations: usize,
) {
    let found = explore(
        bounds,
        2,
        || {
            let world = World::new(6, migrations);
            world.populate();
            world
        },
        |world, me| {
            let machine = world.machine(me, 0);
            if me == 0 {
                world.run(&machine, ops);
            } else {
                let cache = world.rig.cache(&machine);
                let cpu = world.rig.slabs().cpu(0).unwrap();
                assert_ne!(cache.drain(cpu), Drained::BackedOut);
            }
        },
        |_| {},
        |world, _| world.check(false),
    );
    report("drain", found);
}

/// Two drains of one slab and a mutator: the claim lets one drain at a time (`Drain_claim`,
/// finding 1).
fn two_drains_and_a_mutator_keep_one_owner_under_every_schedule(bounds: Bounds) {
    let found = explore(
        bounds,
        3,
        || {
            let world = World::new(6, 0);
            world.populate();
            world
        },
        |world, me| {
            let machine = world.machine(me, 0);
            if me == 0 {
                world.free(&machine);
                world.alloc(&machine);
            } else {
                let cache = world.rig.cache(&machine);
                cache.drain(world.rig.slabs().cpu(0).unwrap());
            }
        },
        |_| {},
        |world, _| world.check(false),
    );
    report("two drains", found);
}

/// A drain whose fence fails backs out while a mutator runs (`Drain_backout`).
fn a_drain_that_backs_out_keeps_one_owner_under_every_schedule(bounds: Bounds, ops: &[Op]) {
    let found = explore(
        bounds,
        2,
        || {
            let world = World::new(6, 0);
            world.populate();
            world.rig.os().set_faults(Call::Fence, Faults::ALWAYS);
            world
        },
        |world, me| {
            let machine = world.machine(me, 0);
            if me == 0 {
                world.run(&machine, ops);
            } else {
                let cache = world.rig.cache(&machine);
                let cpu = world.rig.slabs().cpu(0).unwrap();
                assert_ne!(cache.drain(cpu), Drained::Done);
            }
        },
        |_| {},
        |world, _| world.check(false),
    );
    report("back-out", found);
}

/// A fork at any step of a drain racing a mutator, the forking thread being either one
/// (`Drain_gen_any`, `Fork_gen_any`). The fork comes from a signal handler that allocates and
/// frees in the child before the interrupted thread resumes, so a surviving drainer meets
/// the child's installs over its now-stale markers. Afterwards the child allocates, frees and
/// drains both CPUs: nothing is in two places, and nothing the parent left blocks it.
fn a_fork_at_any_step_duplicates_nothing(bounds: Bounds, ops: &[Op]) {
    let found = explore(
        bounds,
        2,
        || {
            let world = World::new(6, 0);
            world.populate();
            world
        },
        |world, me| {
            let machine = world.machine(me, 0);
            if me == 0 {
                world.run(&machine, ops);
            } else {
                let cache = world.rig.cache(&machine);
                cache.drain(world.rig.slabs().cpu(0).unwrap());
            }
        },
        |world| {
            // SAFETY: at a fork the checker has stopped every other thread.
            unsafe { world.rig.os().wipe_for_fork() };
            // The fork came from a signal handler, which goes on in the child before the
            // interrupted code resumes: it allocates and frees on the CPU being drained, over
            // whatever the parent's drain left there.
            let child = world.machine(THREADS - 1, 0);
            world.alloc(&child);
            world.free(&child);
        },
        |world, forked| {
            if forked {
                let child = world.machine(0, 0);
                for cpu in 0..CPUS {
                    child.thread.set_cpu(cpu as i32);
                    world.alloc(&child);
                    world.free(&child);
                }
                let cache = world.rig.cache(&child);
                for cpu in world.rig.slabs().cpus() {
                    assert_ne!(cache.drain(cpu), Drained::Busy);
                }
            }
            world.check(forked);
        },
    );
    report("fork", found);
    assert!(found.forks > 0);
}

/// A world in which CPU 0's loaded magazine and spares are an interval old, so that the age
/// limit's look takes them.
fn aged_world(migrations: usize) -> World {
    let world = World::new(6, migrations);
    world.populate();
    let machine = world.machine(THREADS - 1, 0);
    let cache = world.rig.cache(&machine);
    let cpu = world.rig.slabs().cpu(0).unwrap();
    // Stamps the loaded magazine now, if its install did not.
    cache.age_look(cpu, tiny_class(), &Clock::new(world.rig.os()));
    world.rig.os().advance_clock(2 * IDLE_TICKS);
    world
}

/// The age limit at CPU 0, from a thread that started there: a look that takes the loaded
/// magazine, and a sweep's take of the full spare stack. Counts the takes that happened in
/// `taken`, so a scenario can check that some schedule took something.
fn age_looks(world: &World, machine: &CheckCpu<'_>, taken: &AtomicUsize) {
    let cache = world.rig.cache(machine);
    let cpu = world.rig.slabs().cpu(0).unwrap();
    let hot = cache.age_look(cpu, tiny_class(), &Clock::new(world.rig.os()));
    let spares = cache.take_spares(cpu, tiny_class());
    taken.fetch_add(usize::from(hot) + usize::from(spares), Ordering::SeqCst);
}

/// The age limit takes CPU 0's magazine and a spare while a mutator allocates and frees there
/// (`age.rs`). With `migrations`, either thread may move to the other CPU at the start of a
/// sequence: the taker between reading CPU 0's word and its store-if-equal, which then runs on
/// CPU 1, the mutator so that its installs land on either.
fn an_age_take_and_a_mutator_keep_one_owner_under_every_schedule(
    bounds: Bounds,
    migrations: usize,
) {
    let taken = AtomicUsize::new(0);
    let found = explore(
        bounds,
        2,
        || aged_world(migrations),
        |world, me| {
            let machine = world.machine(me, 0);
            if me == 0 {
                world.run(&machine, MUTATE);
            } else {
                age_looks(world, &machine, &taken);
            }
        },
        |_| {},
        |world, _| world.check(false),
    );
    report("age take", found);
    assert!(
        taken.load(Ordering::SeqCst) > 0,
        "no schedule took anything"
    );
}

/// The age limit's take races a drain of the same slab from the other CPU: the take's commit
/// and the drain's markers meet as a slow path's install and a drain do (`Drain_claim`).
fn an_age_take_and_a_drain_keep_one_owner_under_every_schedule(bounds: Bounds) {
    let taken = AtomicUsize::new(0);
    let found = explore(
        bounds,
        2,
        || aged_world(0),
        |world, me| {
            if me == 0 {
                age_looks(world, &world.machine(me, 0), &taken);
            } else {
                let machine = world.machine(me, 1);
                let cache = world.rig.cache(&machine);
                let cpu = world.rig.slabs().cpu(0).unwrap();
                assert_ne!(cache.drain(cpu), Drained::BackedOut);
            }
        },
        |_| {},
        |world, _| world.check(false),
    );
    report("age take and drain", found);
    assert!(
        taken.load(Ordering::SeqCst) > 0,
        "no schedule took anything"
    );
}

/// Two threads push and pop one depot stack directly, with a third holding magazines it
/// pushes back (`Depot.tla`): the version counter keeps every magazine with one owner.
fn depot_stacks_keep_one_owner_under_every_schedule(bounds: Bounds) {
    let found = explore(
        bounds,
        2,
        || {
            let world = World::new(6, 0);
            let machine = world.machine(THREADS - 1, 0);
            let cache = world.rig.cache(&machine);
            let class = tiny_class();
            // Three magazines with one object each, on the class's depot stack.
            for _ in 0..3 {
                let magazine = cache.area().create(class.magazine()).unwrap();
                let object = world.rig.source.alloc_one(class).unwrap();
                magazine.set_object(1, object);
                magazine.set_level(1);
                world.rig.depot.full(class).push(cache.area(), magazine);
            }
            world
        },
        |world, me| {
            let machine = world.machine(me, 0);
            let cache = world.rig.cache(&machine);
            let class = tiny_class();
            let stack = world.rig.depot.full(class);
            // Thread 1 pops two and pushes the first back before the second: the ABA a
            // stale head would miss. Thread 0 pops one and pushes it back.
            let first = stack.pop(cache.area(), class.magazine());
            let second = if me == 1 {
                stack.pop(cache.area(), class.magazine())
            } else {
                None
            };
            for magazine in [first, second].into_iter().flatten() {
                stack.push(cache.area(), magazine);
            }
        },
        |_| {},
        |world, _| world.check(false),
    );
    report("depot", found);
}

#[test]
fn a_fence_aborts_the_sequence_in_flight_on_its_cpu() {
    let world = World::new(2, 0);
    let a = world.machine(0, 1);
    a.step(Step::ReadCpu).unwrap();
    assert!(a.step(Step::Load).is_ok());
    world.rig.os().register_fence().unwrap();
    world.rig.os().fence(1).unwrap();
    assert_eq!(a.step(Step::Commit), Err(Aborted));
    let b = world.machine(1, 1);
    a.step(Step::ReadCpu).unwrap();
    b.step(Step::ReadCpu).unwrap();
    assert_eq!(a.step(Step::Load), Err(Aborted), "b took a's CPU");
}

/// The scenarios within bounds small enough for the merge gate, and within the larger bounds
/// they were checked with, which run on demand: `cargo test -p asmalloc -- --ignored interleave`
/// (a few minutes). Only the larger bounds reach the schedules in which a sequence commits
/// over a drainer's marker (two preemptions) or a fork takes a drainer mid-drain (one
/// preemption and a fork); deliberately broken drains, back-outs and claim checks pass the
/// quick runs and fail the full ones.
mod run {
    use super::*;

    #[test]
    #[cfg_attr(miri, ignore)]
    fn exchanges_keep_one_owner_under_every_schedule_quickly() {
        super::exchanges_keep_one_owner_under_every_schedule(Bounds::preemptions(1));
    }

    #[test]
    #[ignore = "minutes; run on demand"]
    fn exchanges_keep_one_owner_under_every_schedule_fully() {
        super::exchanges_keep_one_owner_under_every_schedule(Bounds::preemptions(2));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn a_drain_and_a_mutator_keep_one_owner_under_every_schedule_quickly() {
        super::a_drain_and_a_mutator_keep_one_owner_under_every_schedule(
            Bounds::preemptions(1),
            MUTATE,
            0,
        );
    }

    #[test]
    #[ignore = "minutes; run on demand"]
    fn a_drain_and_a_mutator_keep_one_owner_under_every_schedule_fully() {
        // The mutator may also move to the other CPU once, mid-drain.
        super::a_drain_and_a_mutator_keep_one_owner_under_every_schedule(
            Bounds::preemptions(2),
            MUTATE,
            1,
        );
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn two_drains_and_a_mutator_keep_one_owner_under_every_schedule_quickly() {
        super::two_drains_and_a_mutator_keep_one_owner_under_every_schedule(Bounds::preemptions(0));
    }

    #[test]
    #[ignore = "minutes; run on demand"]
    fn two_drains_and_a_mutator_keep_one_owner_under_every_schedule_fully() {
        super::two_drains_and_a_mutator_keep_one_owner_under_every_schedule(Bounds::preemptions(1));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn a_drain_that_backs_out_keeps_one_owner_under_every_schedule_quickly() {
        super::a_drain_that_backs_out_keeps_one_owner_under_every_schedule(
            Bounds::preemptions(1),
            MUTATE,
        );
    }

    #[test]
    #[ignore = "minutes; run on demand"]
    fn a_drain_that_backs_out_keeps_one_owner_under_every_schedule_fully() {
        super::a_drain_that_backs_out_keeps_one_owner_under_every_schedule(
            Bounds::preemptions(2),
            MUTATE,
        );
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn a_fork_at_any_step_duplicates_nothing_quickly() {
        super::a_fork_at_any_step_duplicates_nothing(Bounds::preemptions(0).with_fork(), MUTATE);
    }

    #[test]
    #[ignore = "minutes; run on demand"]
    fn a_fork_at_any_step_duplicates_nothing_fully() {
        super::a_fork_at_any_step_duplicates_nothing(Bounds::preemptions(1).with_fork(), MUTATE);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn an_age_take_and_a_mutator_keep_one_owner_under_every_schedule_quickly() {
        super::an_age_take_and_a_mutator_keep_one_owner_under_every_schedule(
            Bounds::preemptions(1),
            1,
        );
    }

    #[test]
    #[ignore = "minutes; run on demand"]
    fn an_age_take_and_a_mutator_keep_one_owner_under_every_schedule_fully() {
        super::an_age_take_and_a_mutator_keep_one_owner_under_every_schedule(
            Bounds::preemptions(2),
            2,
        );
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn an_age_take_and_a_drain_keep_one_owner_under_every_schedule_quickly() {
        super::an_age_take_and_a_drain_keep_one_owner_under_every_schedule(Bounds::preemptions(1));
    }

    #[test]
    #[ignore = "minutes; run on demand"]
    fn an_age_take_and_a_drain_keep_one_owner_under_every_schedule_fully() {
        super::an_age_take_and_a_drain_keep_one_owner_under_every_schedule(Bounds::preemptions(2));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn depot_stacks_keep_one_owner_under_every_schedule_quickly() {
        super::depot_stacks_keep_one_owner_under_every_schedule(Bounds::preemptions(2));
    }

    #[test]
    #[ignore = "minutes; run on demand"]
    fn depot_stacks_keep_one_owner_under_every_schedule_fully() {
        super::depot_stacks_keep_one_owner_under_every_schedule(Bounds::preemptions(3));
    }
}
