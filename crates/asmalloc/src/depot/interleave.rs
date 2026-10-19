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
use super::{Domain, Drained, IDLE_TICKS, Pool, Source};
use crate::classes::{MagazineSize, SmallClass};
use crate::front::check::{Bounds, Report, choose, explore, switch_point};
use crate::os::sim::{Call, Faults, SimOs, SimThread, SimTopology};
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
        World::tuned(objects, migrations, TIGHT)
    }

    fn tuned(objects: usize, migrations: usize, tuning: super::Tuning) -> World {
        World {
            rig: Rig::with_os(SimOs::new(), &[tiny_class()], objects, tuning),
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

/// A world that `maintain` has looked at once, an interval ago, with CPU 0 quiet since: the next
/// `maintain` drains CPU 0 and launders what it takes, and sheds what the depot holds.
fn quiet_world(migrations: usize) -> World {
    let world = World::new(6, migrations);
    world.populate();
    let machine = world.machine(THREADS - 1, 1);
    world.rig.cache(&machine).maintain();
    world.rig.os().advance_clock(2 * IDLE_TICKS);
    world
}

/// `maintain` from CPU 1 races a mutator on CPU 0, which may migrate (`maintain.rs`): its drain
/// meets the mutator's sequences as any drain does, and its launders and sheds meet the
/// mutator's exchanges with the depot.
fn maintain_and_a_mutator_keep_one_owner_under_every_schedule(bounds: Bounds, migrations: usize) {
    let found = explore(
        bounds,
        2,
        || quiet_world(migrations),
        |world, me| {
            if me == 0 {
                world.run(&world.machine(me, 0), MUTATE);
            } else {
                world.rig.cache(&world.machine(me, 1)).maintain();
            }
        },
        |_| {},
        |world, _| world.check(false),
    );
    report("maintain", found);
}

/// A fork at any step of `maintain` racing a mutator, either thread forking, as in
/// `a_fork_at_any_step_duplicates_nothing`; the child then maintains as well.
fn a_fork_at_any_step_of_maintain_duplicates_nothing(bounds: Bounds) {
    let found = explore(
        bounds,
        2,
        || quiet_world(0),
        |world, me| {
            if me == 0 {
                world.run(&world.machine(me, 0), MUTATE);
            } else {
                world.rig.cache(&world.machine(me, 1)).maintain();
            }
        },
        |world| {
            // SAFETY: at a fork the checker has stopped every other thread.
            unsafe { world.rig.os().wipe_for_fork() };
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
                for _ in 0..3 {
                    world.rig.os().advance_clock(IDLE_TICKS);
                    cache.maintain();
                }
            }
            world.check(forked);
        },
    );
    report("fork in maintain", found);
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

/// A world in which CPU 0 has sent magazines of the tiny class to the depot and taken them back
/// until it keeps stock (`stock.rs`), with every object back in the heap's hands and the clock
/// an interval on, so that a sweep settles.
fn stocked_world(migrations: usize) -> World {
    let world = World::tuned(
        6,
        migrations,
        super::Tuning {
            stock: true,
            ..TIGHT
        },
    );
    let slab = world.rig.slabs().slab(world.rig.slabs().cpu(0).unwrap());
    slab.stock(tiny_class())
        .store(super::stock::credits(4, 4), Ordering::Relaxed);
    let machine = world.machine(THREADS - 1, 0);
    for _ in 0..4 {
        for _ in 0..6 {
            world.alloc(&machine);
        }
        for _ in 0..6 {
            world.free(&machine);
        }
    }
    world.populate();
    world.rig.os().advance_clock(2 * IDLE_TICKS);
    world
}

/// With stock on CPU 0, a thread that round-trips there races a drain of CPU 0 from CPU 1, and
/// a sweep's settling of the stock: the stock word is a hint, so all that must hold is what
/// holds without it, one owner for every magazine and object.
fn stock_keeps_one_owner_under_every_schedule(bounds: Bounds, migrations: usize) {
    let found = explore(
        bounds,
        3,
        || stocked_world(migrations),
        |world, me| {
            if me == 0 {
                world.run(&world.machine(me, 0), &[Op::Free, Op::Alloc, Op::Free]);
            } else if me == 1 {
                let machine = world.machine(me, 1);
                let cache = world.rig.cache(&machine);
                cache.drain(world.rig.slabs().cpu(0).unwrap());
            } else {
                let machine = world.machine(me, 0);
                let cache = world.rig.cache(&machine);
                let cpu = world.rig.slabs().cpu(0).unwrap();
                cache.settle_stock(cpu, tiny_class());
                cache.take_spares(cpu, tiny_class());
            }
        },
        |_| {},
        |world, _| world.check(false),
    );
    report("stock", found);
}

/// Two CPUs each send a magazine of the tiny class to the depot and take one back, racing on
/// the one stack: credit is earned only by a CPU that took back its own magazine in a schedule
/// where its push or its pop found the stack contended, and some schedule does. With no
/// preemption nothing contends, and no schedule earns any.
fn contended_round_trips_earn_stock(bounds: Bounds) -> usize {
    let earned = AtomicUsize::new(0);
    let found = explore(
        bounds,
        2,
        || {
            World::tuned(
                8,
                0,
                super::Tuning {
                    stock: true,
                    max_spares: 0,
                    ..TIGHT
                },
            )
        },
        |world, me| {
            let machine = world.machine(me, me);
            // A burst of two magazines and its frees: the second magazine of each goes to the
            // depot both ways.
            let cache = world.rig.cache(&machine);
            let held: Vec<_> = (0..4).filter_map(|_| cache.alloc(tiny_class())).collect();
            for object in held.iter().rev() {
                cache.free(tiny_class(), *object);
            }
            let again: Vec<_> = (0..4).filter_map(|_| cache.alloc(tiny_class())).collect();
            for object in again {
                cache.free(tiny_class(), object);
            }
        },
        |_| {},
        |world, _| {
            world.check(false);
            for n in 0..2 {
                let slab = world.rig.slabs().slab(world.rig.slabs().cpu(n).unwrap());
                if slab.stock(tiny_class()).load(Ordering::Relaxed).addr() & 0x3fff_ffff != 0 {
                    earned.fetch_add(1, Ordering::SeqCst);
                }
            }
        },
    );
    report("contended round trips", found);
    earned.load(Ordering::SeqCst)
}

/// Finding (review of stock, commit 1bd3010): the drain clears the count of stock bytes of the
/// slab it drained with a plain store after it has given the slab back to its CPU (markers
/// cleared in step 4's first pass, then the disposal of everything taken, a launder each for
/// `maintain`, then the store). Stock the CPU pushes meanwhile, with the credit the drain left
/// it, is counted and then wiped from the count, so the CPU can hold that much again past its
/// room: `stock.rs` says the drain puts the count right. Thread 0 frees a burst on CPU 0, which
/// has credit, while thread 1 drains CPU 0 from CPU 1; afterwards CPU 0 must count at least the
/// stock it holds.
fn a_drain_leaves_no_stock_uncounted(bounds: Bounds) {
    use crate::front::{Field, Spare};
    let tuning = super::Tuning {
        stock: true,
        age_limit: false,
        shed_unused: false,
        refresh_every: 0,
        ..TIGHT
    };
    let found = explore(
        bounds,
        2,
        || {
            let world = World::tuned(12, 0, tuning);
            let machine = world.machine(THREADS - 1, 0);
            for _ in 0..10 {
                world.alloc(&machine);
            }
            let slab = world.rig.slabs().slab(world.rig.slabs().cpu(0).unwrap());
            slab.stock(tiny_class())
                .store(super::stock::credits(4, 4), Ordering::Relaxed);
            world
        },
        |world, me| {
            if me == 0 {
                world.run(&world.machine(me, 0), &[Op::Free; 8]);
            } else {
                let machine = world.machine(me, 1);
                let cache = world.rig.cache(&machine);
                cache.drain(world.rig.slabs().cpu(0).unwrap());
            }
        },
        |_| {},
        |world, _| {
            world.check(false);
            let class = tiny_class();
            let limit = super::spare_limit(tuning, class, Spare::Full) as u32;
            let slab = world.rig.slabs().slab(world.rig.slabs().cpu(0).unwrap());
            let cache = world.rig.cache(crate::os::Pinned(-1));
            let mut next = slab
                .field(Field::spare(class, Spare::Full))
                .load(Ordering::Relaxed)
                .addr();
            let mut stock = 0;
            while next != 0 {
                let magazine = cache.area().magazine(next, class.magazine()).unwrap();
                if magazine.record().depth >= limit {
                    stock += class.magazine().capacity() * class.size();
                }
                next = magazine.link().load(Ordering::Relaxed).addr();
            }
            let counted = slab.stock_held().load(Ordering::Relaxed) as u32 as usize;
            assert!(
                counted >= stock,
                "CPU 0 holds {stock} bytes of full stock and counts {counted}"
            );
        },
    );
    report("a drain and the stock count", found);
}

/// Two threads push and pop one depot stack directly (`Depot.tla`): the version counter keeps
/// every magazine with one owner, and a capped push that gives its magazine back changes
/// nothing.
fn depot_stacks_keep_one_owner_under_every_schedule(bounds: Bounds) {
    let found = explore(
        bounds,
        2,
        || {
            let world = World::new(6, 0);
            let machine = world.machine(THREADS - 1, 0);
            let cache = world.rig.cache(&machine);
            let class = tiny_class();
            // Three magazines with one object each, on the class's shared stack.
            for _ in 0..3 {
                let magazine = cache.area().create(class.magazine()).unwrap();
                let object = world.rig.source.alloc_one(class).unwrap();
                magazine.set_object(1, object);
                magazine.set_level(1);
                world
                    .rig
                    .depot
                    .full(Pool::Shared, class)
                    .push(cache.area(), magazine, 1);
            }
            world
        },
        |world, me| {
            let machine = world.machine(me, 0);
            let cache = world.rig.cache(&machine);
            let class = tiny_class();
            let stack = world.rig.depot.full(Pool::Shared, class);
            // Thread 1 pops two and pushes the first back before the second: the ABA a
            // stale head would miss. Thread 0 pops one and pushes it back. A push back that
            // finds the stack at a cap of two objects sends its magazine to a domain's stack
            // instead, which some schedules reach and others do not.
            let first = stack.pop(cache.area(), class.magazine());
            let second = if me == 1 {
                stack.pop(cache.area(), class.magazine())
            } else {
                None
            };
            for magazine in [first, second].into_iter().flatten() {
                if let Err(magazine) = stack.push_under(cache.area(), magazine, 1, 2) {
                    cache.to_domain(Domain::ZERO, class, magazine);
                }
            }
        },
        |_| {},
        |world, _| world.check(false),
    );
    report("depot", found);
}

/// A pop and a push back onto a domain's stack race a look that finds the stack empty and
/// clears its bit in the sets of what the domains hold (`held.rs`): the census checks that a
/// stack holding a magazine ends with both its bits set.
fn held_hints_stay_set_under_every_schedule(bounds: Bounds) {
    let found = explore(
        bounds,
        2,
        || {
            let world = World::new(6, 0);
            let machine = world.machine(THREADS - 1, 0);
            let cache = world.rig.cache(&machine);
            let class = tiny_class();
            let magazine = cache.area().create(class.magazine()).unwrap();
            let object = world.rig.source.alloc_one(class).unwrap();
            magazine.set_object(1, object);
            magazine.set_level(1);
            cache.to_domain(Domain::ZERO, class, magazine);
            world
        },
        |world, me| {
            let machine = world.machine(me, 0);
            let cache = world.rig.cache(&machine);
            let class = tiny_class();
            if me == 0 {
                let stack = world.rig.depot.full(Pool::Domain(Domain::ZERO), class);
                if let Some(magazine) = stack.pop(cache.area(), class.magazine()) {
                    cache.to_domain(Domain::ZERO, class, magazine);
                }
            } else {
                cache.unhold(Domain::ZERO, class);
            }
        },
        |_| {},
        |world, _| world.check(false),
    );
    report("held hints", found);
}

impl World {
    /// A heap whose every CPU is a cache domain of its own, so that CPU 0 and CPU 1 use
    /// different stacks and each is "another domain" to the other, with `classes`.
    fn in_two_domains(classes: &[SmallClass]) -> World {
        let os = SimOs::new();
        os.set_topology(SimTopology::Blocks(1));
        let world = World {
            rig: Rig::with_os(os, classes, 6, TIGHT),
            migrations: AtomicUsize::new(0),
            user: Mutex::new(Vec::new()),
        };
        let machine = world.machine(THREADS - 1, 0);
        assert!(world.rig.cache(&machine).prepare());
        assert!(world.rig.depot.domain_count() >= 2);
        world
    }

    /// Makes a magazine with one object of `class` and sends it to `domain`'s stacks, as a
    /// slow path of that domain retiring it would.
    fn park(&self, machine: &CheckCpu<'_>, domain: Domain, class: SmallClass) {
        let cache = self.rig.cache(machine);
        let magazine = cache.area().create(class.magazine()).unwrap();
        let object = self.rig.source.alloc_one(class).unwrap();
        magazine.set_object(1, object);
        magazine.set_level(1);
        cache.to_domain(domain, class, magazine);
    }
}

/// The two largest classes, whose magazines hold two objects.
fn two_tiny_classes() -> [SmallClass; 2] {
    let mut classes: Vec<SmallClass> = SmallClass::all()
        .filter(|c| c.magazine() == MagazineSize::MIN)
        .collect();
    classes.reverse();
    [classes[0], classes[1]]
}

/// What no scenario above reaches, since each runs in one cache domain: CPU 1's periodic look
/// at a stack of CPU 0's domain (`held.rs`, `look_elsewhere`), which starts the stack's interval,
/// sets its debt and clears its hint if it finds it empty, and the shed that then launders from
/// that stack (`shed.rs`), racing the stack's own domain popping it and pushing back. Every
/// magazine keeps one owner, and a stack that ends holding a magazine has both its hints.
fn a_look_and_a_shed_from_another_domain_keep_one_owner_and_the_hints(bounds: Bounds) {
    let class = tiny_class();
    let home = Domain::ZERO;
    let found = explore(
        bounds,
        2,
        || {
            let world = World::in_two_domains(&[class]);
            let machine = world.machine(THREADS - 1, 0);
            world.park(&machine, home, class);
            // A look saw the stack this deep two intervals ago, so the next look finds that it
            // owes all of it.
            let cache = world.rig.cache(&machine);
            cache.look_at_depth(Pool::Domain(home), class, world.rig.os().ticks());
            world.rig.os().advance_clock(2 * IDLE_TICKS);
            world
        },
        |world, me| {
            let machine = world.machine(me, me);
            let cache = world.rig.cache(&machine);
            if me == 0 {
                let stack = world.rig.depot.full(Pool::Domain(home), class);
                if let Some(magazine) = stack.pop(cache.area(), class.magazine()) {
                    cache.to_domain(home, class, magazine);
                }
            } else {
                let here = world.rig.depot.domain_of(world.rig.slabs().cpu(1));
                assert_ne!(here, home);
                cache.look_elsewhere(here, 0, world.rig.os().ticks());
                cache.shed(1);
            }
        },
        |_| {},
        |world, _| world.check(false),
    );
    report("look and shed elsewhere", found);
}

/// The hint of which domains hold magazines, against a push of another class: CPU 1's look
/// finds one class's stack in CPU 0's domain empty and clears its bit, which leaves the
/// domain's set empty, so it clears the domain's bit too; meanwhile CPU 0 pushes a magazine of
/// a second class onto its empty stack there. The domain must end marked, or no look from
/// another domain would visit the stack again (`held.rs`, `unhold_domain`).
fn a_domains_hint_survives_a_look_that_empties_another_class(bounds: Bounds) {
    let [emptied, pushed] = two_tiny_classes();
    let home = Domain::ZERO;
    let found = explore(
        bounds,
        2,
        || {
            let world = World::in_two_domains(&[emptied, pushed]);
            let machine = world.machine(THREADS - 1, 0);
            world.park(&machine, home, emptied);
            // Take it back out: the stack is empty and its bits are still set, as after the
            // domain's own pop.
            let cache = world.rig.cache(&machine);
            let stack = world.rig.depot.full(Pool::Domain(home), emptied);
            let magazine = stack.pop(cache.area(), emptied.magazine()).unwrap();
            cache.launder_held(emptied, &magazine);
            cache.to_domain(home, emptied, magazine);
            world
        },
        |world, me| {
            let machine = world.machine(me, me);
            if me == 0 {
                world.park(&machine, home, pushed);
            } else {
                let cache = world.rig.cache(&machine);
                let here = world.rig.depot.domain_of(world.rig.slabs().cpu(1));
                cache.look_elsewhere(here, 0, world.rig.os().ticks());
            }
        },
        |_| {},
        |world, _| world.check(false),
    );
    report("domain hint", found);
}

/// A push that first reads its stack holding a magazine and, after another thread pops that
/// one, lands on an empty stack, while a look clears the emptied stack's hint in between: the
/// push must still count as one onto an empty stack and set the hint again (`Stack::push_under`
/// reports the emptiness its successful compare-and-swap saw, not its first read's).
fn a_push_that_retries_onto_an_emptied_stack_sets_its_hint(bounds: Bounds) {
    let class = tiny_class();
    let home = Domain::ZERO;
    let found = explore(
        bounds,
        3,
        || {
            // Room for two magazines under the cap, so that the push is not turned away.
            let os = SimOs::new();
            let tuning = super::Tuning {
                domain_cap: 4 * class.size(),
                ..TIGHT
            };
            let world = World {
                rig: Rig::with_os(os, &[class], 6, tuning),
                migrations: AtomicUsize::new(0),
                user: Mutex::new(Vec::new()),
            };
            let machine = world.machine(THREADS - 1, 0);
            world.park(&machine, home, class);
            world
        },
        |world, me| {
            let machine = world.machine(me, 0);
            let cache = world.rig.cache(&machine);
            match me {
                0 => world.park(&machine, home, class),
                1 => {
                    let stack = world.rig.depot.full(Pool::Domain(home), class);
                    if let Some(magazine) = stack.pop(cache.area(), class.magazine()) {
                        cache.launder_held(class, &magazine);
                        cache.to_domain(home, class, magazine);
                    }
                }
                _ => cache.unhold(home, class),
            }
        },
        |_| {},
        |world, _| world.check(false),
    );
    report("push onto an emptied stack", found);
}

/// Two looks from another domain and a push: one look clears the class's bit and, finding that
/// the push came, sets it back, while the other finds the domain's set empty in between and
/// clears the domain's bit. Whoever sets a class bit back must make sure of the domain's
/// (`held.rs`), or the stack ends with its class marked in a domain no look visits.
fn two_looks_and_a_push_leave_the_domain_marked(bounds: Bounds) {
    let class = tiny_class();
    let home = Domain::ZERO;
    let found = explore(
        bounds,
        3,
        || {
            let world = World::in_two_domains(&[class]);
            let machine = world.machine(THREADS - 1, 0);
            world.park(&machine, home, class);
            let cache = world.rig.cache(&machine);
            let stack = world.rig.depot.full(Pool::Domain(home), class);
            let magazine = stack.pop(cache.area(), class.magazine()).unwrap();
            cache.launder_held(class, &magazine);
            cache.to_domain(home, class, magazine);
            world
        },
        |world, me| {
            if me == 0 {
                world.park(&world.machine(me, 0), home, class);
            } else {
                let machine = world.machine(me, 1);
                let cache = world.rig.cache(&machine);
                let here = world.rig.depot.domain_of(world.rig.slabs().cpu(1));
                cache.look_elsewhere(here, 0, world.rig.os().ticks());
            }
        },
        |_| {},
        |world, _| world.check(false),
    );
    report("two looks and a push", found);
}

/// A shed pays a stack's debt and settles its owing bits while a look in a new interval writes
/// a new debt on the same stack (`shed.rs`): the class's bit and the domain's must end set, or
/// no shed would find the debt. The settle clears a bit and then rereads the debt (or the
/// class bits) to set it back; the census checks the bits.
fn a_settle_and_a_new_debt_leave_the_debt_marked(bounds: Bounds) {
    let class = tiny_class();
    let home = Domain::ZERO;
    let found = explore(
        bounds,
        2,
        || {
            // Room for two magazines in the domain's stack.
            let tuning = super::Tuning {
                domain_cap: 4 * class.size(),
                ..TIGHT
            };
            let world = World {
                rig: Rig::with_os(SimOs::new(), &[class], 6, tuning),
                migrations: AtomicUsize::new(0),
                user: Mutex::new(Vec::new()),
            };
            let machine = world.machine(THREADS - 1, 0);
            let cache = world.rig.cache(&machine);
            let pool = Pool::Domain(home);
            // One object deep for an interval, then two: the stack owes the one.
            world.park(&machine, home, class);
            cache.look_at_depth(pool, class, world.rig.os().ticks());
            world.park(&machine, home, class);
            world.rig.os().advance_clock(IDLE_TICKS);
            cache.look_at_depth(pool, class, world.rig.os().ticks());
            world
        },
        |world, me| {
            let machine = world.machine(me, me);
            let cache = world.rig.cache(&machine);
            if me == 0 {
                cache.shed(1);
            } else {
                let next = world.rig.os().ticks() + IDLE_TICKS;
                cache.look_at_depth(Pool::Domain(home), class, next);
            }
        },
        |_| {},
        |world, _| world.check(false),
    );
    report("settle and a new debt", found);
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
/// they were checked with, which run on demand in an optimized build:
/// `cargo test --release -p asmalloc --lib -- --ignored depot::interleave` (several minutes;
/// `scripts/gate --all` runs it). A debug build takes ten times as long, over ten minutes for
/// the drain and a mutator, so the full runs exist only in optimized builds; the debug build
/// runs the quick bounds. Only the larger bounds reach the schedules in which a sequence commits
/// over a drainer's marker (two preemptions) or a fork takes a drainer mid-drain (one
/// preemption and a fork); deliberately broken drains, back-outs and claim checks pass the
/// quick runs and fail the full ones.
mod run {
    use super::*;

    #[test]
    #[cfg_attr(miri, ignore)]
    fn only_contended_round_trips_earn_stock() {
        assert_eq!(
            super::contended_round_trips_earn_stock(Bounds::preemptions(0)),
            0
        );
        assert!(super::contended_round_trips_earn_stock(Bounds::preemptions(1)) > 0);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    #[ignore = "finding: a drain wipes the count of stock its CPU pushed meanwhile"]
    fn stock_review_a_drain_leaves_no_stock_uncounted() {
        super::a_drain_leaves_no_stock_uncounted(Bounds::preemptions(2));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn stock_keeps_one_owner_under_every_schedule_quickly() {
        super::stock_keeps_one_owner_under_every_schedule(Bounds::preemptions(1), 0);
    }

    #[test]
    #[cfg(not(debug_assertions))]
    #[ignore = "slow: run with scripts/gate --all, optimized"]
    fn stock_keeps_one_owner_under_every_schedule_fully() {
        // Two preemptions, or one with a move to the other CPU: both together pass the
        // explorer's bound on executions.
        super::stock_keeps_one_owner_under_every_schedule(Bounds::preemptions(2), 0);
        super::stock_keeps_one_owner_under_every_schedule(Bounds::preemptions(1), 1);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn exchanges_keep_one_owner_under_every_schedule_quickly() {
        super::exchanges_keep_one_owner_under_every_schedule(Bounds::preemptions(1));
    }

    #[test]
    #[cfg(not(debug_assertions))]
    #[ignore = "slow: run with scripts/gate --all, optimized"]
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
    #[cfg(not(debug_assertions))]
    #[ignore = "slow: run with scripts/gate --all, optimized"]
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
    #[cfg(not(debug_assertions))]
    #[ignore = "slow: run with scripts/gate --all, optimized"]
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
    #[cfg(not(debug_assertions))]
    #[ignore = "slow: run with scripts/gate --all, optimized"]
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
    #[cfg(not(debug_assertions))]
    #[ignore = "slow: run with scripts/gate --all, optimized"]
    fn a_fork_at_any_step_duplicates_nothing_fully() {
        super::a_fork_at_any_step_duplicates_nothing(Bounds::preemptions(1).with_fork(), MUTATE);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn maintain_and_a_mutator_keep_one_owner_under_every_schedule_quickly() {
        super::maintain_and_a_mutator_keep_one_owner_under_every_schedule(
            Bounds::preemptions(1),
            0,
        );
    }

    #[test]
    #[cfg(not(debug_assertions))]
    #[ignore = "slow: run with scripts/gate --all, optimized"]
    fn maintain_and_a_mutator_keep_one_owner_under_every_schedule_fully() {
        super::maintain_and_a_mutator_keep_one_owner_under_every_schedule(
            Bounds::preemptions(2),
            1,
        );
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn a_fork_at_any_step_of_maintain_duplicates_nothing_quickly() {
        super::a_fork_at_any_step_of_maintain_duplicates_nothing(
            Bounds::preemptions(0).with_fork(),
        );
    }

    #[test]
    #[cfg(not(debug_assertions))]
    #[ignore = "slow: run with scripts/gate --all, optimized"]
    fn a_fork_at_any_step_of_maintain_duplicates_nothing_fully() {
        super::a_fork_at_any_step_of_maintain_duplicates_nothing(
            Bounds::preemptions(1).with_fork(),
        );
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
    #[cfg(not(debug_assertions))]
    #[ignore = "slow: run with scripts/gate --all, optimized"]
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
    #[cfg(not(debug_assertions))]
    #[ignore = "slow: run with scripts/gate --all, optimized"]
    fn an_age_take_and_a_drain_keep_one_owner_under_every_schedule_fully() {
        super::an_age_take_and_a_drain_keep_one_owner_under_every_schedule(Bounds::preemptions(2));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn depot_stacks_keep_one_owner_under_every_schedule_quickly() {
        super::depot_stacks_keep_one_owner_under_every_schedule(Bounds::preemptions(2));
    }

    #[test]
    #[cfg(not(debug_assertions))]
    #[ignore = "slow: run with scripts/gate --all, optimized"]
    fn depot_stacks_keep_one_owner_under_every_schedule_fully() {
        super::depot_stacks_keep_one_owner_under_every_schedule(Bounds::preemptions(3));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn held_hints_stay_set_under_every_schedule_quickly() {
        super::held_hints_stay_set_under_every_schedule(Bounds::preemptions(2));
    }

    #[test]
    #[cfg(not(debug_assertions))]
    #[ignore = "slow: run with scripts/gate --all, optimized"]
    fn held_hints_stay_set_under_every_schedule_fully() {
        super::held_hints_stay_set_under_every_schedule(Bounds::preemptions(3));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn a_look_and_a_shed_from_another_domain_keep_one_owner_and_the_hints_quickly() {
        super::a_look_and_a_shed_from_another_domain_keep_one_owner_and_the_hints(
            Bounds::preemptions(2),
        );
    }

    #[test]
    #[cfg(not(debug_assertions))]
    #[ignore = "slow: run with scripts/gate --all, optimized"]
    fn a_look_and_a_shed_from_another_domain_keep_one_owner_and_the_hints_fully() {
        super::a_look_and_a_shed_from_another_domain_keep_one_owner_and_the_hints(
            Bounds::preemptions(3),
        );
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn a_push_that_retries_onto_an_emptied_stack_sets_its_hint_quickly() {
        super::a_push_that_retries_onto_an_emptied_stack_sets_its_hint(Bounds::preemptions(1));
    }

    #[test]
    #[cfg(not(debug_assertions))]
    #[ignore = "slow: run with scripts/gate --all, optimized"]
    fn a_push_that_retries_onto_an_emptied_stack_sets_its_hint_fully() {
        super::a_push_that_retries_onto_an_emptied_stack_sets_its_hint(Bounds::preemptions(2));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn two_looks_and_a_push_leave_the_domain_marked_quickly() {
        super::two_looks_and_a_push_leave_the_domain_marked(Bounds::preemptions(2));
    }

    #[test]
    #[cfg(not(debug_assertions))]
    #[ignore = "slow: run with scripts/gate --all, optimized"]
    fn two_looks_and_a_push_leave_the_domain_marked_fully() {
        super::two_looks_and_a_push_leave_the_domain_marked(Bounds::preemptions(3));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn a_domains_hint_survives_a_look_that_empties_another_class_quickly() {
        super::a_domains_hint_survives_a_look_that_empties_another_class(Bounds::preemptions(2));
    }

    #[test]
    #[cfg(not(debug_assertions))]
    #[ignore = "slow: run with scripts/gate --all, optimized"]
    fn a_domains_hint_survives_a_look_that_empties_another_class_fully() {
        super::a_domains_hint_survives_a_look_that_empties_another_class(Bounds::preemptions(3));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn a_settle_and_a_new_debt_leave_the_debt_marked_quickly() {
        super::a_settle_and_a_new_debt_leave_the_debt_marked(Bounds::preemptions(1));
    }

    #[test]
    #[cfg(not(debug_assertions))]
    #[ignore = "slow: run with scripts/gate --all, optimized"]
    fn a_settle_and_a_new_debt_leave_the_debt_marked_fully() {
        super::a_settle_and_a_new_debt_leave_the_debt_marked(Bounds::preemptions(2));
    }
}
