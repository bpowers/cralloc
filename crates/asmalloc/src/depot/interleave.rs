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
//! Threads run on simulated CPUs. A thread's sequence is in flight on its CPU from its first
//! step to its commit; another thread starting a sequence on that CPU, or a fence of it, aborts
//! the sequence, as preemption and `membarrier` do. A thread may also migrate at the start of
//! any sequence, a bounded number of times per execution. The census of `test_rig.rs` checks
//! the invariants after every execution.
//!
//! The CPU model and the fence that aborts in-flight sequences are a local stand-in until the
//! simulated OS provides its own.

use super::test_rig::{Rig, Small, TIGHT, tiny_class};
use super::{Drained, Source};
use crate::front::check::{Bounds, Report, choose, explore, switch_point};
use crate::front::{Aborted, Machine, Step};
use crate::os::sim::{Call, Faults, SimOs};
use crate::os::{HugePages, Os, OsError, RseqArea};
use crate::space::Geometry;
use core::cell::Cell;
use core::ptr::NonNull;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::vec::Vec;

const CPUS: usize = 2;
const THREADS: usize = 3;

/// Which thread is in flight on each CPU, and which threads' sequences have been aborted.
struct Cpus {
    in_flight: [AtomicUsize; CPUS],
    aborted: [AtomicBool; THREADS],
    migrations: AtomicUsize,
}

impl Cpus {
    fn new(migrations: usize) -> Cpus {
        Cpus {
            in_flight: [const { AtomicUsize::new(0) }; CPUS],
            aborted: [const { AtomicBool::new(false) }; THREADS],
            migrations: AtomicUsize::new(migrations),
        }
    }

    /// Aborts whatever sequence is in flight on `cpu`.
    fn abort_on(&self, cpu: usize) {
        let thread = self.in_flight[cpu].swap(0, Ordering::SeqCst);
        if thread != 0 {
            self.aborted[thread - 1].store(true, Ordering::SeqCst);
        }
    }
}

/// The simulated OS with a fence that aborts the sequences in flight on the fenced CPU.
struct CheckOs {
    sim: SimOs<Small>,
    cpus: Cpus,
}

// SAFETY: every method but `fence` is the simulator's, which keeps the promises. `fence` aborts
// the sequence in flight on the CPU, if any, before it returns; a thread that is not in flight
// there starts its next sequence after the fence, as `membarrier` promises.
unsafe impl Os for CheckOs {
    const GEOMETRY: Geometry = <SimOs<Small> as Os>::GEOMETRY;

    fn reserve(&self, len: usize, align: usize) -> Result<NonNull<u8>, OsError> {
        self.sim.reserve(len, align)
    }

    unsafe fn commit(&self, start: NonNull<u8>, len: usize) -> Result<(), OsError> {
        // SAFETY: the caller's obligations are the simulator's.
        unsafe { self.sim.commit(start, len) }
    }

    unsafe fn purge(&self, start: NonNull<u8>, len: usize) -> Result<(), OsError> {
        // SAFETY: as above.
        unsafe { self.sim.purge(start, len) }
    }

    unsafe fn advise_huge(
        &self,
        start: NonNull<u8>,
        len: usize,
        advice: HugePages,
    ) -> Result<(), OsError> {
        // SAFETY: as above.
        unsafe { self.sim.advise_huge(start, len, advice) }
    }

    unsafe fn wipe_on_fork(&self, start: NonNull<u8>, len: usize) -> Result<(), OsError> {
        // SAFETY: as above.
        unsafe { self.sim.wipe_on_fork(start, len) }
    }

    unsafe fn unmap(&self, start: NonNull<u8>, len: usize) -> Result<(), OsError> {
        // SAFETY: as above.
        unsafe { self.sim.unmap(start, len) }
    }

    fn register_fence(&self) -> Result<(), OsError> {
        self.sim.register_fence()
    }

    fn fence(&self, cpu: u32) -> Result<(), OsError> {
        self.sim.fence(cpu)?;
        self.cpus.abort_on(cpu as usize);
        Ok(())
    }

    fn rseq(&self) -> Option<RseqArea> {
        self.sim.rseq()
    }

    fn random(&self) -> u64 {
        self.sim.random()
    }

    fn abort(&self, message: &str) -> ! {
        self.sim.abort(message)
    }
}

/// The CPU of one checker thread.
struct CheckCpu<'a> {
    me: usize,
    cpu: Cell<usize>,
    cpus: &'a Cpus,
}

impl Machine for CheckCpu<'_> {
    fn cpu_id(&self) -> i32 {
        self.cpu.get() as i32
    }

    fn step(&self, step: Step) -> Result<(), Aborted> {
        switch_point();
        if step != Step::ReadCpu {
            return if self.cpus.aborted[self.me].load(Ordering::SeqCst) {
                Err(Aborted)
            } else {
                Ok(())
            };
        }
        self.cpus.aborted[self.me].store(false, Ordering::SeqCst);
        let budget = self.cpus.migrations.load(Ordering::SeqCst);
        if budget > 0 && choose(2) == 1 {
            self.cpus.migrations.store(budget - 1, Ordering::SeqCst);
            self.cpu.set((self.cpu.get() + 1) % CPUS);
        }
        // Starting a sequence here means whatever was in flight here was preempted.
        let cpu = self.cpu.get();
        let previous = self.cpus.in_flight[cpu].swap(self.me + 1, Ordering::SeqCst);
        if previous != 0 && previous != self.me + 1 {
            self.cpus.aborted[previous - 1].store(true, Ordering::SeqCst);
        }
        Ok(())
    }
}

/// One execution's heap, and the objects the program holds.
struct World {
    rig: Rig<CheckOs, CPUS>,
    user: Mutex<Vec<usize>>,
}

impl World {
    fn new(objects: usize, migrations: usize) -> World {
        let os = CheckOs {
            sim: SimOs::new(),
            cpus: Cpus::new(migrations),
        };
        World {
            rig: Rig::with_os(os, &[tiny_class()], objects, TIGHT),
            user: Mutex::new(Vec::new()),
        }
    }

    fn machine(&self, me: usize, cpu: usize) -> CheckCpu<'_> {
        CheckCpu {
            me,
            cpu: Cell::new(cpu),
            cpus: &self.rig.os().cpus,
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
fn a_drain_and_a_mutator_keep_one_owner_under_every_schedule(bounds: Bounds, ops: &[Op]) {
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
            world.rig.os().sim.set_faults(Call::Fence, Faults::ALWAYS);
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
/// (`Drain_gen_any`, `Fork_gen_any`). Afterwards the child allocates, frees and drains both
/// CPUs: nothing is in two places, and nothing the parent left blocks it.
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
        // SAFETY: at a fork the checker has stopped every other thread.
        |world| unsafe { world.rig.os().sim.wipe_for_fork() },
        |world, forked| {
            if forked {
                let child = world.machine(0, 0);
                for cpu in 0..CPUS {
                    child.cpu.set(cpu);
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
fn the_check_os_fence_aborts_the_sequence_in_flight() {
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
/// (about a minute). Only the larger bounds reach the schedules in which a sequence commits
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
        );
    }

    #[test]
    #[ignore = "minutes; run on demand"]
    fn a_drain_and_a_mutator_keep_one_owner_under_every_schedule_fully() {
        super::a_drain_and_a_mutator_keep_one_owner_under_every_schedule(
            Bounds::preemptions(2),
            MUTATE,
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
    fn depot_stacks_keep_one_owner_under_every_schedule_quickly() {
        super::depot_stacks_keep_one_owner_under_every_schedule(Bounds::preemptions(2));
    }

    #[test]
    #[ignore = "minutes; run on demand"]
    fn depot_stacks_keep_one_owner_under_every_schedule_fully() {
        super::depot_stacks_keep_one_owner_under_every_schedule(Bounds::preemptions(2));
    }
}
