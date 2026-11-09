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

/// Two drains of one slab and a mutator: the claim lets one drain at a time (`Drain_claim`).
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
    cache.age_look(
        cpu,
        tiny_class(),
        &Clock::new(world.rig.os(), world.rig.source.clock()),
    );
    world.rig.os().advance_clock(2 * IDLE_TICKS);
    world
}

/// The age limit at CPU 0, from a thread that started there: a look that takes the loaded
/// magazine, and a sweep's take of the full spare stack. Counts the takes that happened in
/// `taken`, so a scenario can check that some schedule took something.
fn age_looks(world: &World, machine: &CheckCpu<'_>, taken: &AtomicUsize) {
    let cache = world.rig.cache(machine);
    let cpu = world.rig.slabs().cpu(0).unwrap();
    let hot = cache.age_look(
        cpu,
        tiny_class(),
        &Clock::new(world.rig.os(), world.rig.source.clock()),
    );
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
                if cache.sweep_stock(cpu, tiny_class()) == super::stock::Swept::Plain {
                    cache.take_spares(cpu, tiny_class());
                }
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
    use crate::front::Spare;
    let earned = AtomicUsize::new(0);
    let ended = AtomicUsize::new(0);
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
                let cpu = world.rig.slabs().cpu(n).unwrap();
                let slab = world.rig.slabs().slab(cpu);
                let word = slab.stock(tiny_class()).load(Ordering::Relaxed).addr() as u64;
                let stock = super::stock::Stock(word);
                let (full, empty) = (stock.credit(Spare::Full), stock.credit(Spare::Empty));
                if full == 0 && empty == 0 {
                    continue;
                }
                earned.fetch_add(1, Ordering::SeqCst);
                if full == 0 {
                    // Credit for empty stock alone keeps nothing (`stock.rs`).
                    continue;
                }
                ended.fetch_add(1, Ordering::SeqCst);
                // End to end: the credit the contention earned keeps the CPU's next burst as
                // stock, and the stock goes back once an interval passes without its use.
                let machine = world.machine(0, n as usize);
                let cache = world.rig.cache(&machine);
                let held: Vec<_> = (0..4).filter_map(|_| cache.alloc(tiny_class())).collect();
                for object in held {
                    cache.free(tiny_class(), object);
                }
                let full = || cache.spare_depth(tiny_class(), Spare::Full).unwrap_or(0);
                assert!(full() > 0, "credit kept no stock");
                for _ in 0..2 {
                    world.rig.os().advance_clock(2 * IDLE_TICKS);
                    for _ in 0..8 {
                        if cache.sweep_stock(cpu, tiny_class()) == super::stock::Swept::Plain {
                            cache.take_spares(cpu, tiny_class());
                            break;
                        }
                    }
                }
                assert_eq!(full(), 0, "unused stock went back");
                world.check(false);
            }
        },
    );
    report("contended round trips", found);
    let earned = earned.load(Ordering::SeqCst);
    assert!(
        earned == 0 || ended.load(Ordering::SeqCst) > 0,
        "no schedule earned full credit and kept stock with it"
    );
    earned
}

/// Stock the CPU pushes while a drain is under way stays counted. The drain gives the slab back
/// to its CPU before it has finished (markers cleared in step 4's first pass, then the disposal of
/// everything taken, a launder each for `maintain`), so the CPU may push stock meanwhile, with the
/// credit the drain left it; a count the drain cleared after that would wipe it, and the CPU could
/// hold that much again past its room. Thread 0 frees a burst on CPU 0, which has credit, while
/// thread 1 drains CPU 0 from CPU 1; afterwards CPU 0 must count at least the stock it holds.
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
            let counted = cache.stock_held(world.rig.slabs().cpu(0).unwrap(), Spare::Full);
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

/// A `collect` whose thread moves to another CPU partway through emptying its own slab leaves
/// nothing on either: the rest of its first slab goes the way of the other CPUs'.
#[test]
fn a_collect_that_moves_empties_every_slab() {
    #[derive(Default)]
    struct OnCpus(usize);
    impl crate::stats::Visitor for OnCpus {
        fn magazine(&mut self, place: crate::stats::Place, _class: SmallClass, objects: usize) {
            if !matches!(place, crate::stats::Place::Depot) {
                self.0 += objects;
            }
        }
    }
    let found = explore(
        Bounds::preemptions(0),
        1,
        || {
            let world = World::new(6, 0);
            world.populate();
            world.migrations.store(1, Ordering::SeqCst);
            world
        },
        |world, me| world.rig.cache(&world.machine(me, 0)).collect(),
        |_| {},
        |world, _| {
            world.check(false);
            let mut left = OnCpus::default();
            let rig = &world.rig;
            super::inspect(&rig.depot, rig.cell.get().unwrap(), rig.slabs(), &mut left);
            assert_eq!(left.0, 0, "objects stayed on a CPU");
        },
    );
    report("a collect that moves", found);
}

impl World {
    /// A heap whose even and odd CPUs are two cache domains, so that CPU 0 and CPU 1 use different
    /// stacks and each is "another domain" to the other, with `classes`.
    fn in_two_domains(classes: &[SmallClass]) -> World {
        let os = SimOs::new();
        os.set_topology(SimTopology::Stripes(2));
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
/// they were checked with, which `scripts/gate --all` runs. Only the larger bounds reach the
/// schedules in which a sequence commits over a drainer's marker (two preemptions) or a fork takes
/// a drainer mid-drain (one preemption and a fork); deliberately broken drains, back-outs and
/// claim checks pass the quick runs and fail the full ones.
/// CPU 0 about to leave a magazine half full: its next swap of the tiny class goes `way` and is
/// evidence of a lock. For an allocation, its loaded magazine is empty and a full spare waits;
/// for a free, its loaded magazine is full and an empty spare waits.
fn centring_world(way: super::stock::Swap, migrations: usize) -> World {
    let world = World::new(8, migrations);
    let machine = world.machine(THREADS - 1, 0);
    for _ in 0..4 {
        let object = world.rig.source.alloc_one(tiny_class()).unwrap();
        world.user.lock().unwrap().push(object.addr().get());
    }
    // Three frees fill a magazine and swap it for an empty one, which the third fills by one; an
    // allocation empties it. For a free, the next allocation swaps it for the full spare and
    // takes one object, which a free puts back.
    world.run(&machine, &[Op::Free, Op::Free, Op::Free, Op::Alloc]);
    let cache = world.rig.cache(&machine);
    if way == super::stock::Swap::Free {
        world.run(&machine, &[Op::Alloc, Op::Free]);
        // A second empty magazine, in the depot, for another thread's free to install while
        // the centring thread holds the spare.
        let size = tiny_class().magazine();
        let spare = cache.area().create(size).unwrap();
        world
            .rig
            .depot
            .empty(Pool::Shared, size)
            .push(cache.area(), spare, 1);
    }
    let cpu = world.rig.slabs().cpu(0).unwrap();
    cache.prime_lock(cpu, tiny_class(), way);
    world
}

/// A swap that leaves its magazine half full takes the loaded magazine out of the slab, copies
/// half between the two it then holds, and installs one expecting the word it emptied
/// (`slow.rs`, "Half full on a reversal"; `Exchange.tla`, `CentreTake` to `CentrePut`).
/// Against a drain of its CPU, or another thread on the same CPU that takes slow paths of its
/// own, under every schedule, every magazine and object keeps one owner: the install's failure
/// arms retire or serve from both magazines, and nothing is lost.
fn centring_keeps_one_owner(
    way: super::stock::Swap,
    drains: bool,
    bounds: Bounds,
    migrations: usize,
) {
    // The other thread's first call on a slot the centring has emptied installs a magazine of
    // its own only if it allocates: a free there serves the object without one.
    let (ops, other): (&[Op], &[Op]) = match way {
        super::stock::Swap::Alloc => (
            &[Op::Alloc, Op::Free, Op::Free],
            &[Op::Free, Op::Alloc, Op::Alloc],
        ),
        super::stock::Swap::Free => (
            &[Op::Free, Op::Alloc, Op::Alloc],
            &[Op::Alloc, Op::Free, Op::Alloc],
        ),
    };
    let found = explore(
        bounds,
        2,
        || centring_world(way, migrations),
        |world, me| {
            let machine = world.machine(me, 0);
            if me == 0 {
                world.run(&machine, ops);
            } else if drains {
                let cpu = world.rig.slabs().cpu(0).unwrap();
                world.rig.cache(&machine).drain(cpu);
            } else {
                world.run(&machine, other);
            }
        },
        |_| {},
        |world, _| world.check(false),
    );
    report("centring keeps one owner", found);
}

/// A heap whose background thread, on CPU 1, has run quiet rounds enough to park, with every
/// slab drained, and a mutator about to resume on CPU 0: the park protocol's world
/// (`thread.rs`).
struct Parking {
    world: World,
    key: u64,
    seen: Mutex<super::thread::Seen>,
    rest: Mutex<Option<super::thread::Rest>>,
}

fn parking_world(migrations: usize) -> Parking {
    use crate::thread_word::AWAKE;
    let world = World::new(6, migrations);
    world.populate();
    let machine = world.machine(1, 1);
    let cache = world.rig.cache(&machine);
    let key = cache.thread_key();
    assert!(
        world
            .rig
            .source
            .thread
            .claim_start(world.rig.os(), key, AWAKE)
    );
    let mut seen = super::thread::Seen::default();
    let mut ready = false;
    for _ in 0..4 * super::thread::QUIET_ROUNDS {
        world.rig.os().advance_clock(IDLE_TICKS);
        cache.maintain();
        if cache.first_look(&mut seen, false).is_some() {
            ready = true;
            break;
        }
    }
    assert!(ready, "the thread never came to its look before parking");
    Parking {
        world,
        key,
        seen: Mutex::new(seen),
        rest: Mutex::new(None),
    }
}

/// Whether every slab holds no magazine: what a parked thread relies on.
fn slabs_empty(world: &World) -> bool {
    use crate::classes::is_marker;
    world.rig.slabs().cpus().all(|cpu| {
        let slab = world.rig.slabs().slab(cpu);
        crate::front::Slab::drained_fields().all(|field| {
            let word = crate::front::sync::quiet(slab.field(field)).load(Ordering::SeqCst);
            word.is_null() || is_marker(word.addr())
        })
    })
}

/// The thread's decision (its look, the store of `PARKED`, the fence and the second look)
/// races a mutator that resumes on CPU 0 with `ops`, whose slow paths count their activity,
/// install magazines and load the word, in every order the bounds allow, the mutator's wake
/// racing the thread's park and back-out. No wake-up is lost: a word left parked means every
/// slab is empty, so the next call takes a slow path and finds it; a thread that decided to
/// park on a word a slow path then moved was woken. The check names every state of the word,
/// so that a state added to it must be added here.
fn parking_loses_no_wake_up_under_every_schedule(bounds: Bounds, ops: &[Op], migrations: usize) {
    use super::thread::Rest;
    use crate::thread_word::{AWAKE, DORMANT, NONE, OFF, PARKED, WANTED};
    let parked = AtomicUsize::new(0);
    let woken = AtomicUsize::new(0);
    let found = explore(
        bounds,
        2,
        || parking_world(migrations),
        |parking, me| {
            let world = &parking.world;
            if me == 0 {
                world.run(&world.machine(0, 0), ops);
            } else {
                let machine = world.machine(1, 1);
                let cache = world.rig.cache(&machine);
                let mut seen = parking.seen.lock().unwrap();
                let rest = cache.rest(&mut seen, parking.key, Some(false));
                *parking.rest.lock().unwrap() = Some(rest);
            }
        },
        |_| {},
        |parking, _| {
            let world = &parking.world;
            world.check(false);
            let rest = parking.rest.lock().unwrap().unwrap();
            let word = world.rig.source.thread.load();
            let wakes = world.rig.os().wakes();
            if rest == Rest::Park {
                parked.fetch_add(1, Ordering::SeqCst);
            }
            match word.state_for(parking.key) {
                PARKED => {
                    assert_eq!(
                        rest,
                        Rest::Park,
                        "a word parked behind a thread that sleeps"
                    );
                    assert!(slabs_empty(world), "parked with a magazine in a slab");
                }
                AWAKE => {
                    if rest == Rest::Park {
                        assert!(wakes > 0, "the thread waits on a word nobody woke");
                    }
                    if wakes > 0 {
                        woken.fetch_add(1, Ordering::SeqCst);
                    }
                }
                state @ (NONE | WANTED | DORMANT | OFF) => {
                    panic!("the race left the word in state {state}")
                }
                state => panic!("a state this scenario does not know: {state}"),
            }
        },
    );
    report("parking", found);
    assert!(
        parked.load(Ordering::SeqCst) > 0,
        "no schedule decided to park"
    );
    assert!(
        woken.load(Ordering::SeqCst) > 0,
        "no schedule woke the thread"
    );
}

/// A heap whose background thread, on CPU 1, is awake with this interval's round done, about to
/// sleep: the sleep protocol's world (`crate::thread_word`, "Sleeping"). `due` heartbeats of
/// the mutator make work due that the thread does.
struct Sleeping {
    world: World,
    key: u64,
    asleep: Mutex<Option<Option<crate::thread_word::Snapshot>>>,
}

fn sleeping_world(due: usize) -> Sleeping {
    use crate::thread_word::AWAKE;
    let world = World::new(6, 0);
    world.populate();
    let machine = world.machine(1, 1);
    let cache = world.rig.cache(&machine);
    let key = cache.thread_key();
    assert!(
        world
            .rig
            .source
            .thread
            .claim_start(world.rig.os(), key, AWAKE)
    );
    cache.maintain();
    world.rig.source.coming_due.store(due, Ordering::SeqCst);
    Sleeping {
        world,
        key,
        asleep: Mutex::new(None),
    }
}

/// The thread's look before it waits (its mark, then its look for work) races `waker` on CPU 0,
/// which makes work due that the thread does and wakes it its own way. No wake-up is lost: a
/// thread that decided to wait on a word that still holds what it waits on, which the futex
/// would sleep on until its timeout, has no work due. In the other schedules the thread found
/// the work itself, or its word moved, so that the wait returns at once or the wake reaches it.
fn sleeping_loses_no_wake_up_under_every_schedule(
    name: &str,
    bounds: Bounds,
    due: usize,
    waker: impl Fn(&World) + Sync,
) {
    let found_work = AtomicUsize::new(0);
    let woken = AtomicUsize::new(0);
    let found = explore(
        bounds,
        2,
        || sleeping_world(due),
        |sleeping, me| {
            let world = &sleeping.world;
            if me == 0 {
                waker(world);
            } else {
                let machine = world.machine(1, 1);
                let asleep = world.rig.cache(&machine).begin_sleep(sleeping.key);
                *sleeping.asleep.lock().unwrap() = Some(asleep);
            }
        },
        |_| {},
        |sleeping, _| {
            let world = &sleeping.world;
            world.check(false);
            let asleep = sleeping.asleep.lock().unwrap().unwrap();
            let word = world.rig.source.thread.load();
            let owing = world.rig.source.owing.load(Ordering::SeqCst);
            match asleep {
                // The futex compares the word's low half.
                Some(waits) if waits.0 as u32 == word.0 as u32 => {
                    assert_eq!(
                        owing, 0,
                        "the thread sleeps with work due and nobody to wake it"
                    );
                }
                Some(_) => {
                    woken.fetch_add(1, Ordering::SeqCst);
                }
                None => {
                    found_work.fetch_add(1, Ordering::SeqCst);
                }
            }
        },
    );
    report(name, found);
    assert!(
        found_work.load(Ordering::SeqCst) > 0,
        "no schedule had the thread find the work"
    );
    assert!(
        woken.load(Ordering::SeqCst) > 0,
        "no schedule woke the thread"
    );
}

/// As [`sleeping_loses_no_wake_up_under_every_schedule`], with the thread's whole turn
/// (`Cache::thread_turn`: its steps, its look, its wait and its back-out) racing `waker`. The
/// simulator's wait sleeps, as the futex does, if the word holds what it expects when it is
/// called, until a wake. No schedule leaves the thread asleep with work due.
fn a_turn_loses_no_wake_up_under_every_schedule(
    name: &str,
    bounds: Bounds,
    due: usize,
    waker: impl Fn(&World) + Sync,
) {
    let slept = AtomicUsize::new(0);
    let found = explore(
        bounds,
        2,
        || sleeping_world(due),
        |sleeping, me| {
            let world = &sleeping.world;
            if me == 0 {
                waker(world);
            } else {
                let machine = world.machine(1, 1);
                let mut seen = super::thread::Seen::default();
                let mut returned = false;
                world
                    .rig
                    .cache(&machine)
                    .thread_turn(&mut seen, sleeping.key, &mut returned, 1);
            }
        },
        |_| {},
        |sleeping, _| {
            let world = &sleeping.world;
            world.check(false);
            if world.rig.os().blocked() {
                slept.fetch_add(1, Ordering::SeqCst);
                assert_eq!(
                    world.rig.source.owing.load(Ordering::SeqCst),
                    0,
                    "the thread sleeps with work due and nobody to wake it: word {:#x}, waited on \
                     {:?}, wakes {}",
                    world.rig.source.thread.load().0,
                    world.rig.os().last_timed_wait(),
                    world.rig.os().wakes()
                );
            }
        },
    );
    report(name, found);
    assert!(
        slept.load(Ordering::SeqCst) > 0,
        "no schedule had the thread sleep"
    );
}

/// What a grower does (`back::purge`, `Back::growth_due`): the debt added, sequentially
/// consistent, and then the word read afresh, a sleeping thread's mark cleared and the thread
/// woken.
fn grower(world: &World) {
    world.rig.source.owing.fetch_add(1, Ordering::SeqCst);
    world.rig.source.thread.rouse_any(world.rig.os());
}

/// A slow path on CPU 0 whose heartbeat makes work due (`Sleeping`): with its load of the word
/// before its exchange and the work made due after, and then the cold look at the word afresh,
/// which wakes the thread (`Cache::rouse_thread`).
fn heartbeat(world: &World) {
    let machine = world.machine(0, 0);
    let cache = world.rig.cache(&machine);
    if let Some(object) = cache.alloc_slow(tiny_class()) {
        world.user.lock().unwrap().push(object.addr().get());
    }
}

mod run {
    use super::*;

    #[test]
    #[cfg_attr(miri, ignore)]
    fn sleeping_loses_no_wake_up_to_a_grower_quickly() {
        super::sleeping_loses_no_wake_up_under_every_schedule(
            "sleeping, a grower",
            Bounds::preemptions(1),
            0,
            super::grower,
        );
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn a_turn_loses_no_wake_up_quickly() {
        super::a_turn_loses_no_wake_up_under_every_schedule(
            "a turn, a grower",
            Bounds::preemptions(1),
            0,
            super::grower,
        );
        super::a_turn_loses_no_wake_up_under_every_schedule(
            "a turn, a heartbeat",
            Bounds::preemptions(1),
            1,
            super::heartbeat,
        );
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn sleeping_loses_no_wake_up_to_a_heartbeat_quickly() {
        super::sleeping_loses_no_wake_up_under_every_schedule(
            "sleeping, a heartbeat",
            Bounds::preemptions(1),
            1,
            super::heartbeat,
        );
    }

    #[test]
    #[ignore = "slow: run with scripts/gate --all"]
    fn sleeping_loses_no_wake_up_under_every_schedule_fully() {
        super::sleeping_loses_no_wake_up_under_every_schedule(
            "sleeping, a grower",
            Bounds::preemptions(3),
            0,
            super::grower,
        );
        super::sleeping_loses_no_wake_up_under_every_schedule(
            "sleeping, a heartbeat",
            Bounds::preemptions(2),
            1,
            super::heartbeat,
        );
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn parking_loses_no_wake_up_under_every_schedule_quickly() {
        super::parking_loses_no_wake_up_under_every_schedule(
            Bounds::preemptions(1),
            &[Op::Alloc],
            0,
        );
    }

    #[test]
    #[ignore = "slow: run with scripts/gate --all"]
    fn parking_loses_no_wake_up_under_every_schedule_fully() {
        super::parking_loses_no_wake_up_under_every_schedule(
            Bounds::preemptions(2),
            &[Op::Alloc],
            1,
        );
        super::parking_loses_no_wake_up_under_every_schedule(
            Bounds::preemptions(2),
            &[Op::Alloc, Op::Free],
            0,
        );
    }

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
    fn a_drain_leaves_no_stock_uncounted_quickly() {
        super::a_drain_leaves_no_stock_uncounted(Bounds::preemptions(1));
    }

    #[test]
    #[ignore = "slow: run with scripts/gate --all"]
    fn a_drain_leaves_no_stock_uncounted_fully() {
        super::a_drain_leaves_no_stock_uncounted(Bounds::preemptions(2));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn stock_keeps_one_owner_under_every_schedule_quickly() {
        super::stock_keeps_one_owner_under_every_schedule(Bounds::preemptions(1), 0);
    }

    #[test]
    #[ignore = "slow: run with scripts/gate --all"]
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
    #[ignore = "slow: run with scripts/gate --all"]
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
    #[ignore = "slow: run with scripts/gate --all"]
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
    #[ignore = "slow: run with scripts/gate --all"]
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
    #[ignore = "slow: run with scripts/gate --all"]
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
    #[ignore = "slow: run with scripts/gate --all"]
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
    #[ignore = "slow: run with scripts/gate --all"]
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
    #[ignore = "slow: run with scripts/gate --all"]
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
    #[ignore = "slow: run with scripts/gate --all"]
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
    #[ignore = "slow: run with scripts/gate --all"]
    fn an_age_take_and_a_drain_keep_one_owner_under_every_schedule_fully() {
        super::an_age_take_and_a_drain_keep_one_owner_under_every_schedule(Bounds::preemptions(2));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn depot_stacks_keep_one_owner_under_every_schedule_quickly() {
        super::depot_stacks_keep_one_owner_under_every_schedule(Bounds::preemptions(2));
    }

    #[test]
    #[ignore = "slow: run with scripts/gate --all"]
    fn depot_stacks_keep_one_owner_under_every_schedule_fully() {
        super::depot_stacks_keep_one_owner_under_every_schedule(Bounds::preemptions(3));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn held_hints_stay_set_under_every_schedule_quickly() {
        super::held_hints_stay_set_under_every_schedule(Bounds::preemptions(2));
    }

    #[test]
    #[ignore = "slow: run with scripts/gate --all"]
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
    #[ignore = "slow: run with scripts/gate --all"]
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
    #[ignore = "slow: run with scripts/gate --all"]
    fn a_push_that_retries_onto_an_emptied_stack_sets_its_hint_fully() {
        super::a_push_that_retries_onto_an_emptied_stack_sets_its_hint(Bounds::preemptions(2));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn two_looks_and_a_push_leave_the_domain_marked_quickly() {
        super::two_looks_and_a_push_leave_the_domain_marked(Bounds::preemptions(2));
    }

    #[test]
    #[ignore = "slow: run with scripts/gate --all"]
    fn two_looks_and_a_push_leave_the_domain_marked_fully() {
        super::two_looks_and_a_push_leave_the_domain_marked(Bounds::preemptions(3));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn a_domains_hint_survives_a_look_that_empties_another_class_quickly() {
        super::a_domains_hint_survives_a_look_that_empties_another_class(Bounds::preemptions(2));
    }

    #[test]
    #[ignore = "slow: run with scripts/gate --all"]
    fn a_domains_hint_survives_a_look_that_empties_another_class_fully() {
        super::a_domains_hint_survives_a_look_that_empties_another_class(Bounds::preemptions(3));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn a_settle_and_a_new_debt_leave_the_debt_marked_quickly() {
        super::a_settle_and_a_new_debt_leave_the_debt_marked(Bounds::preemptions(1));
    }

    #[test]
    #[ignore = "slow: run with scripts/gate --all"]
    fn a_settle_and_a_new_debt_leave_the_debt_marked_fully() {
        super::a_settle_and_a_new_debt_leave_the_debt_marked(Bounds::preemptions(2));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn centring_keeps_one_owner_quickly() {
        use super::super::stock::Swap;
        for way in [Swap::Alloc, Swap::Free] {
            for drains in [false, true] {
                super::centring_keeps_one_owner(way, drains, Bounds::preemptions(1), 0);
            }
        }
    }

    #[test]
    #[ignore = "slow: run with scripts/gate --all"]
    fn centring_keeps_one_owner_fully() {
        use super::super::stock::Swap;
        for way in [Swap::Alloc, Swap::Free] {
            for drains in [false, true] {
                super::centring_keeps_one_owner(way, drains, Bounds::preemptions(2), 0);
                super::centring_keeps_one_owner(way, drains, Bounds::preemptions(1), 1);
            }
        }
    }
}
