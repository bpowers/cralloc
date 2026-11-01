//! The twins: the routines' specifications as Rust, over a simulated processor.
//!
//! A twin runs as its routine would, one step at a time: read the CPU number, load the field,
//! read or write the magazine (pop, push and the stacks only), commit. Before each step it asks
//! its [`Machine`] whether the OS has interrupted the sequence; if so it starts again from the
//! top, as the real routine's abort handler does, reading the CPU number afresh. The machine
//! also says which CPU number the thread reads. Tests drive both: they abort a twin before any
//! step, move it to another CPU, run a nested call in between, or let the interleaving checker
//! choose.
//!
//! The stores that commit are release stores and the loads of a field acquire loads, which is
//! what the routines' plain `mov`s are on x86-64. A twin's memory accesses are not switch
//! points for the interleaving checker (`sync::quiet`): `Machine::step` is the only place a
//! twin can be interrupted, as the step before an instruction is the only place a sequence can
//! be, so nothing can slip in between a step's check and its access. On threads that run freely
//! the machine keeps a fence out of that gap instead, which is why a twin tells it when its
//! magazine access and its commit are done (`Machine::accessed`, `Machine::committed`).
//!
//! The unsafe blocks turn a slab word or a spare head into an atomic view of a magazine slot.
//! They rest on the slab invariant (S) (`docs/safety/cralloc/front/real-safety.md`): a hot word
//! that is neither zero nor a marker names slot 1 to `n - 1` of a committed magazine of its
//! class, and a spare head or link that is not zero names slot 0 of one. Twins are test
//! scaffolding, run only by tests and the simulated heap, so these blocks have no ledger
//! entries of their own; `scripts/unsafe_sites.py` lists this file as scaffolding.

use super::routines::{Routines, StackPop, StackPush, StoreIfEqual};
use super::slab::{Cpu, Field, Slabs};
use super::sync::quiet;
use crate::classes::{SLOT_BYTES, SmallClass, is_marker};
use crate::os::{Aborted, Machine, Step};
use core::ptr::NonNull;
use core::sync::atomic::AtomicPtr;
use core::sync::atomic::Ordering::{Acquire, Relaxed, Release};

/// The routines as Rust, on `slabs`, for a thread on `machine`.
pub struct Twin<'a, M> {
    slabs: &'a Slabs,
    machine: M,
}

impl<'a, M: Machine> Twin<'a, M> {
    /// The twins of a thread on `machine`, working on `slabs`.
    pub fn new(slabs: &'a Slabs, machine: M) -> Twin<'a, M> {
        Twin { slabs, machine }
    }

    /// The machine.
    pub fn machine(&self) -> &M {
        &self.machine
    }

    /// The first step of every sequence: the CPU number, if it has a slab.
    fn enter(&self) -> Result<Option<Cpu>, Aborted> {
        self.machine.step(Step::ReadCpu)?;
        Ok(self.slabs.cpu(self.machine.cpu_id()))
    }

    /// Runs a sequence until it completes without an abort.
    fn sequence<R>(&self, mut attempt: impl FnMut() -> Result<R, Aborted>) -> R {
        loop {
            if let Ok(result) = attempt() {
                return result;
            }
        }
    }

    fn try_pop(&self, class: SmallClass) -> Result<Option<NonNull<u8>>, Aborted> {
        let Some(cpu) = self.enter()? else {
            return Ok(None);
        };
        let field = quiet(self.slabs.slab(cpu).field(Field::hot(class)));
        self.machine.step(Step::Load)?;
        let word = field.load(Acquire);
        if class.magazine().alloc_slow(word.addr()) {
            return Ok(None);
        }
        self.machine.step(Step::Access)?;
        let top = word.wrapping_byte_sub(SLOT_BYTES);
        // SAFETY: `word` passed `alloc_slow`, so by (S) it is slot `s >= 2` of a committed
        // magazine, and `top` is slot `s - 1` of the same magazine, aligned to 8.
        let object = unsafe { slot(top) }.load(Relaxed);
        self.machine.accessed();
        self.machine.step(Step::Commit)?;
        field.store(top, Release);
        self.machine.committed();
        Ok(NonNull::new(object))
    }

    fn try_push(&self, class: SmallClass, object: NonNull<u8>) -> Result<bool, Aborted> {
        let Some(cpu) = self.enter()? else {
            return Ok(false);
        };
        let field = quiet(self.slabs.slab(cpu).field(Field::hot(class)));
        self.machine.step(Step::Load)?;
        let word = field.load(Acquire);
        if class.magazine().free_slow(word.addr()) {
            return Ok(false);
        }
        self.machine.step(Step::Access)?;
        // SAFETY: `word` passed `free_slow`, so by (S) it is slot `s <= n - 2` of a committed
        // magazine, aligned to 8.
        unsafe { slot(word) }.store(object.as_ptr(), Relaxed);
        self.machine.accessed();
        self.machine.step(Step::Commit)?;
        field.store(word.wrapping_byte_add(SLOT_BYTES), Release);
        self.machine.committed();
        Ok(true)
    }

    fn try_store_if_equal(
        &self,
        field: Field,
        expected: *mut u8,
        new: *mut u8,
    ) -> Result<StoreIfEqual, Aborted> {
        let Some(cpu) = self.enter()? else {
            return Ok(StoreIfEqual::NoCpu);
        };
        let field = quiet(self.slabs.slab(cpu).field(field));
        self.machine.step(Step::Load)?;
        let word = field.load(Acquire);
        if word.addr() != expected.addr() {
            return Ok(StoreIfEqual::Mismatch(word));
        }
        self.machine.step(Step::Commit)?;
        field.store(new, Release);
        self.machine.committed();
        Ok(StoreIfEqual::Stored)
    }

    fn try_stack_push(&self, field: Field, magazine: NonNull<u8>) -> Result<StackPush, Aborted> {
        let Some(cpu) = self.enter()? else {
            return Ok(StackPush::NoCpu);
        };
        let field = quiet(self.slabs.slab(cpu).field(field));
        self.machine.step(Step::Load)?;
        let head = field.load(Acquire);
        if is_marker(head.addr()) {
            return Ok(StackPush::Marker);
        }
        self.machine.step(Step::Access)?;
        // SAFETY: `stack_push`'s caller promises `magazine` is slot 0 of a committed magazine
        // it holds.
        unsafe { slot(magazine.as_ptr()) }.store(head, Relaxed);
        self.machine.accessed();
        self.machine.step(Step::Commit)?;
        field.store(magazine.as_ptr(), Release);
        self.machine.committed();
        Ok(StackPush::Pushed)
    }

    fn try_stack_pop(&self, field: Field) -> Result<StackPop, Aborted> {
        let Some(cpu) = self.enter()? else {
            return Ok(StackPop::NoCpu);
        };
        let field = quiet(self.slabs.slab(cpu).field(field));
        self.machine.step(Step::Load)?;
        let head = field.load(Acquire);
        let Some(magazine) = NonNull::new(head).filter(|h| !is_marker(h.addr().get())) else {
            return Ok(StackPop::Empty);
        };
        self.machine.step(Step::Access)?;
        // SAFETY: `head` is neither zero nor a marker, so by (S) it is slot 0 of a committed
        // magazine.
        let next = unsafe { slot(head) }.load(Relaxed);
        self.machine.accessed();
        self.machine.step(Step::Commit)?;
        field.store(next, Release);
        self.machine.committed();
        Ok(StackPop::Popped(magazine))
    }
}

/// The magazine slot at `address`, as an atomic word.
///
/// # Safety
///
/// `address` is a slot of a magazine in committed magazine memory, which stays mapped for the
/// rest of the program, and every access to that slot is atomic.
unsafe fn slot<'a>(address: *mut u8) -> &'a AtomicPtr<u8> {
    // SAFETY: the caller's conditions are those of `from_ptr`; a slot address is a multiple
    // of 8 because magazines are aligned to their size, at least 32 bytes.
    unsafe { AtomicPtr::from_ptr(address.cast::<*mut u8>()) }
}

// SAFETY: each method is its specification step for step (see `routines.rs`), with a commit
// that is its last action, preceded only by loads and by a store to the magazine slot the
// routine owns until the commit. Every step after the first is preceded by a `Machine::step`
// check, so an interruption restarts the sequence with nothing committed.
unsafe impl<M: Machine> Routines for Twin<'_, M> {
    fn slabs(&self) -> &Slabs {
        self.slabs
    }

    fn cpu(&self) -> Option<Cpu> {
        self.slabs.cpu(self.machine.cpu_id())
    }

    fn pop(&self, class: SmallClass) -> Option<NonNull<u8>> {
        self.sequence(|| self.try_pop(class))
    }

    fn push(&self, class: SmallClass, object: NonNull<u8>) -> bool {
        self.sequence(|| self.try_push(class, object))
    }

    unsafe fn store_if_equal(&self, field: Field, expected: *mut u8, new: *mut u8) -> StoreIfEqual {
        self.sequence(|| self.try_store_if_equal(field, expected, new))
    }

    unsafe fn stack_push(&self, field: Field, magazine: NonNull<u8>) -> StackPush {
        self.sequence(|| self.try_stack_push(field, magazine))
    }

    fn stack_pop(&self, field: Field) -> StackPop {
        self.sequence(|| self.try_stack_pop(field))
    }
}

#[cfg(test)]
mod tests {
    // The tests call the unsafe routines only with null, a marker, or addresses of the test's
    // own magazines, which are committed for the whole test: what their contracts ask.
    #![allow(clippy::undocumented_unsafe_blocks)]

    use super::*;
    use crate::classes::{Class, MARKER, MARKER_SHIFT};
    use crate::front::slab::{Slab, Spare};
    use crate::os::Pinned;
    use core::cell::Cell;
    use core::ptr;
    use core::sync::atomic::AtomicBool;
    use std::boxed::Box;
    use std::vec::Vec;

    /// The class with the smallest magazines: 4 slots, 2 objects.
    fn class() -> SmallClass {
        Class::LARGEST_SMALL.small().unwrap()
    }

    /// Four 4-slot magazines, aligned to their size.
    #[repr(C, align(32))]
    struct Magazine([AtomicPtr<u8>; 4]);

    struct World {
        slabs: Box<Slabs<[Slab; 2]>>,
        // A shared reference, not a box: slab words keep pointers into the magazines, and a
        // fresh reborrow through a box would invalidate them.
        magazines: &'static [Magazine; 4],
    }

    fn object(n: usize) -> NonNull<u8> {
        NonNull::new(ptr::without_provenance_mut(0x10_000 + 64 * n)).unwrap()
    }

    impl Drop for World {
        fn drop(&mut self) {
            let magazines = ptr::from_ref(self.magazines).cast_mut();
            // SAFETY: `magazines` came from `Box::leak` in `new`, and the world, the only
            // holder of pointers into it, is going away.
            drop(unsafe { Box::from_raw(magazines) });
        }
    }

    impl World {
        fn new() -> World {
            World {
                slabs: Box::new(Slabs::new()),
                magazines: Box::leak(Box::new(core::array::from_fn(|_| {
                    Magazine(core::array::from_fn(|_| AtomicPtr::new(ptr::null_mut())))
                }))),
            }
        }

        fn slabs(&self) -> &Slabs {
            &*self.slabs
        }

        fn field(&self, cpu: i32, field: Field) -> &AtomicPtr<u8> {
            let cpu = self.slabs().cpu(cpu).unwrap();
            quiet(self.slabs().slab(cpu).field(field))
        }

        /// The address of slot `slot` of magazine `m`, with provenance for all the magazines,
        /// as a pointer into the reservation has in the allocator.
        fn slot(&self, m: usize, slot: usize) -> *mut u8 {
            ptr::from_ref(self.magazines)
                .cast_mut()
                .cast::<u8>()
                .wrapping_add((4 * m + slot) * 8)
        }

        fn every_word(&self) -> impl Iterator<Item = &AtomicPtr<u8>> {
            let fields = (0..2).flat_map(move |cpu| {
                Slab::drained_fields().map(move |field| self.field(cpu, field))
            });
            fields.chain(self.magazines.iter().flat_map(|m| m.0.iter()))
        }

        fn snapshot(&self) -> Vec<*mut u8> {
            self.every_word().map(|word| word.load(Relaxed)).collect()
        }

        /// What another thread could see: every slab field, and every magazine slot except
        /// the free slots of a loaded magazine (at or above its hot word), which a routine may
        /// write before it commits.
        fn visible(&self) -> Vec<*mut u8> {
            let hot: Vec<usize> = (0..2)
                .map(|cpu| self.field(cpu, Field::hot(class())).load(Relaxed).addr())
                .collect();
            let mut seen: Vec<*mut u8> = (0..2)
                .flat_map(|cpu| Slab::drained_fields().map(move |f| (cpu, f)))
                .map(|(cpu, f)| self.field(cpu, f).load(Relaxed))
                .collect();
            for m in 0..self.magazines.len() {
                let base = self.slot(m, 0).addr();
                let end = hot
                    .iter()
                    .find(|&&w| w > base && w < base + 32)
                    .map_or(4, |&w| (w - base) / 8);
                seen.extend((0..end).map(|i| self.magazines[m].0[i].load(Relaxed)));
            }
            seen
        }

        fn restore(&self, snapshot: &[*mut u8]) {
            for (word, &value) in self.every_word().zip(snapshot) {
                word.store(value, Relaxed);
            }
        }

        /// CPU 0: magazine 0 loaded with one object. CPU 1: magazine 1 loaded, full. Each CPU
        /// has magazine 2 or 3 on its full spare stack.
        fn populated() -> World {
            let world = World::new();
            world.magazines[0].0[1].store(object(1).as_ptr(), Relaxed);
            world.magazines[1].0[1].store(object(2).as_ptr(), Relaxed);
            world.magazines[1].0[2].store(object(3).as_ptr(), Relaxed);
            world
                .field(0, Field::hot(class()))
                .store(world.slot(0, 2), Relaxed);
            world
                .field(1, Field::hot(class()))
                .store(world.slot(1, 3), Relaxed);
            world
                .field(0, Field::spare(class(), Spare::Full))
                .store(world.slot(2, 0), Relaxed);
            world
                .field(1, Field::spare(class(), Spare::Full))
                .store(world.slot(3, 0), Relaxed);
            world
        }
    }

    /// A machine that starts on CPU 0 and, at its `abort_at`-th step, moves to CPU `move_to`,
    /// runs `handler` as a signal handler would, and aborts the sequence.
    struct Script<'h> {
        cpu: Cell<i32>,
        steps: Cell<usize>,
        abort_at: usize,
        move_to: i32,
        aborted: Cell<bool>,
        handler: &'h dyn Fn(),
    }

    impl Machine for Script<'_> {
        fn cpu_id(&self) -> i32 {
            self.cpu.get()
        }

        fn step(&self, _: Step) -> Result<(), Aborted> {
            let step = self.steps.get();
            self.steps.set(step + 1);
            if step == self.abort_at {
                self.cpu.set(self.move_to);
                self.aborted.set(true);
                (self.handler)();
                return Err(Aborted);
            }
            Ok(())
        }
    }

    /// Runs `op` from the populated state with an abort before each of its steps in turn (moving
    /// to `move_to`, running `handler`), and checks that the outcome and the final state are
    /// those of `handler` followed by `op` run without interruption on `move_to`: the aborted
    /// attempt left no trace. Returns how many steps were tried.
    fn abort_everywhere<T: PartialEq + core::fmt::Debug>(
        op: &dyn Fn(&World, &Twin<'_, &Script<'_>>) -> T,
        reference: &dyn Fn(&World, &Twin<'_, Pinned>) -> T,
        move_to: i32,
        handler: &dyn Fn(&World),
    ) -> usize {
        let world = World::populated();
        let initial = world.snapshot();
        let mut tried = 0;
        loop {
            world.restore(&initial);
            let nested = || handler(&world);
            let script = Script {
                cpu: Cell::new(0),
                steps: Cell::new(0),
                abort_at: tried,
                move_to,
                aborted: Cell::new(false),
                handler: &nested,
            };
            let outcome = op(&world, &Twin::new(world.slabs(), &script));
            let after = world.visible();
            if !script.aborted.get() {
                return tried;
            }
            world.restore(&initial);
            handler(&world);
            let expected = reference(&world, &Twin::new(world.slabs(), Pinned(move_to)));
            assert_eq!(outcome, expected, "abort before step {tried}");
            assert_eq!(after, world.visible(), "abort before step {tried}");
            tried += 1;
        }
    }

    fn every_routine(move_to: i32, handler: &dyn Fn(&World)) {
        let c = class();
        let full = Field::spare(c, Spare::Full);
        let install = |w: &World, t: &dyn Routines| unsafe {
            t.store_if_equal(Field::hot(c), w.slot(0, 2), w.slot(2, 1))
        };
        let steps = [
            abort_everywhere(&|_, t| t.pop(c), &|_, t| t.pop(c), move_to, handler),
            abort_everywhere(
                &|_, t| t.push(c, object(9)),
                &|_, t| t.push(c, object(9)),
                move_to,
                handler,
            ),
            abort_everywhere(
                &|w, t| install(w, t),
                &|w, t| install(w, t),
                move_to,
                handler,
            ),
            abort_everywhere(
                &|_, t| t.stack_pop(full),
                &|_, t| t.stack_pop(full),
                move_to,
                handler,
            ),
        ];
        // Every routine was aborted before each of its steps: 4 for pop, push and the stack
        // pop, 3 for store-if-equal, which has no magazine access.
        assert_eq!(steps, [4, 4, 3, 4]);
    }

    #[test]
    fn an_abort_before_any_step_leaves_no_trace() {
        every_routine(0, &|_| {});
    }

    #[test]
    fn an_abort_that_migrates_leaves_no_trace() {
        every_routine(1, &|_| {});
    }

    #[test]
    fn a_signal_handler_that_pops_and_pushes_between_steps_leaves_no_trace() {
        // The handler runs on CPU 0 where the interrupted sequence started.
        every_routine(0, &|world| {
            let twin = Twin::new(world.slabs(), Pinned(0));
            let popped = twin.pop(class());
            assert_eq!(popped, Some(object(1)));
            assert!(twin.push(class(), object(7)));
        });
    }

    #[test]
    fn stack_push_aborted_anywhere_leaves_no_trace() {
        let c = class();
        let empty = Field::spare(c, Spare::Empty);
        // Magazine 2 is CPU 0's full spare in the populated world; pushing it on another
        // stack makes no sense to the allocator but exercises every step of the routine.
        let push = |w: &World, t: &dyn Routines| unsafe {
            t.stack_push(empty, NonNull::new(w.slot(2, 0)).unwrap())
        };
        for move_to in [0, 1] {
            let steps = abort_everywhere(&|w, t| push(w, t), &|w, t| push(w, t), move_to, &|_| {});
            assert_eq!(steps, 4);
        }
    }

    #[test]
    fn the_routines_do_what_their_specifications_say() {
        let c = class();
        let world = World::populated();
        let cpu0 = Twin::new(world.slabs(), Pinned(0));
        let cpu1 = Twin::new(world.slabs(), Pinned(1));
        let hot0 = world.field(0, Field::hot(c));
        // Pop takes the one object, then misses on the empty magazine.
        assert_eq!(cpu0.pop(c), Some(object(1)));
        assert_eq!(hot0.load(Relaxed), world.slot(0, 1));
        assert_eq!(cpu0.pop(c), None);
        // Push fills it to slot 3 (two objects), then misses on the full magazine.
        assert!(cpu0.push(c, object(5)));
        assert!(cpu0.push(c, object(6)));
        assert!(!cpu0.push(c, object(7)));
        assert_eq!(hot0.load(Relaxed), world.slot(0, 3));
        assert_eq!(world.magazines[0].0[2].load(Relaxed), object(6).as_ptr());
        // Store-if-equal compares the whole word.
        let full = world.slot(0, 3);
        assert_eq!(
            unsafe { cpu0.store_if_equal(Field::hot(c), ptr::null_mut(), ptr::null_mut()) },
            StoreIfEqual::Mismatch(full)
        );
        assert_eq!(
            unsafe { cpu0.store_if_equal(Field::hot(c), full, ptr::null_mut()) },
            StoreIfEqual::Stored
        );
        assert_eq!(hot0.load(Relaxed), ptr::null_mut());
        // The stacks: pop the spare, push it back with the link restored.
        let spare = Field::spare(c, Spare::Full);
        let magazine = world.slot(3, 0);
        assert_eq!(
            cpu1.stack_pop(spare),
            StackPop::Popped(NonNull::new(magazine).unwrap())
        );
        assert_eq!(cpu1.stack_pop(spare), StackPop::Empty);
        assert_eq!(
            unsafe { cpu1.stack_push(spare, NonNull::new(magazine).unwrap()) },
            StackPush::Pushed
        );
        assert_eq!(world.field(1, spare).load(Relaxed), magazine);
        assert_eq!(world.magazines[3].0[0].load(Relaxed), ptr::null_mut());
        // A marker in any field turns every routine away.
        let marker = ptr::without_provenance_mut(0x77 << MARKER_SHIFT | MARKER);
        world.field(1, spare).store(marker, Relaxed);
        world.field(1, Field::hot(c)).store(marker, Relaxed);
        assert_eq!(cpu1.stack_pop(spare), StackPop::Empty);
        let other = NonNull::new(world.slot(2, 0)).unwrap();
        assert_eq!(unsafe { cpu1.stack_push(spare, other) }, StackPush::Marker);
        assert_eq!(cpu1.pop(c), None);
        assert!(!cpu1.push(c, object(5)));
        assert_eq!(world.field(1, spare).load(Relaxed), marker);
        assert_eq!(world.field(1, Field::hot(c)).load(Relaxed), marker);
    }

    /// A simulated thread whose first push stops between the `Access` check and the store until
    /// a fence has returned, or for long enough to show that none will.
    struct Stall<'a> {
        thread: crate::os::sim::SimThread<'a>,
        at_access: &'a AtomicBool,
        fenced: &'a AtomicBool,
        stalled: Cell<bool>,
    }

    impl Machine for Stall<'_> {
        fn cpu_id(&self) -> i32 {
            self.thread.cpu_id()
        }

        fn step(&self, step: Step) -> Result<(), Aborted> {
            self.thread.step(step)?;
            if step == Step::Access && !self.stalled.replace(true) {
                self.at_access.store(true, Release);
                let give_up = std::time::Instant::now() + std::time::Duration::from_millis(50);
                while !self.fenced.load(Acquire) && std::time::Instant::now() < give_up {
                    std::thread::yield_now();
                }
            }
            Ok(())
        }

        fn accessed(&self) {
            self.thread.accessed();
        }

        fn committed(&self) {
            self.thread.committed();
        }
    }

    /// A drain in miniature against a push on a free-running thread. Once the fence has
    /// returned, the drainer owns the magazine if the marker is still in place, and the aborted
    /// push must not write to it: with the push stopped between its check and its store, the fence
    /// has to wait for the store. When it did not, the store landed in a magazine the drainer
    /// had already passed on, over an object another thread had pushed there, and the retried
    /// push cached the first object a second time: one block, two owners.
    #[test]
    fn an_aborted_push_does_not_write_the_magazine_after_the_fence_returns() {
        use crate::os::Os;
        use crate::os::sim::{SimOs, Tiny};
        let c = class();
        let world = World::populated();
        let os = SimOs::<Tiny>::new();
        os.register_fence().unwrap();
        // An address, so that the threads can share it.
        let marker = 0x77 << MARKER_SHIFT | MARKER;
        let (at_access, fenced) = (AtomicBool::new(false), AtomicBool::new(false));
        let free_slot = &world.magazines[0].0[2];
        let (pushed, old, at_return) = std::thread::scope(|scope| {
            let push = scope.spawn(|| {
                let stall = Stall {
                    thread: os.thread(0),
                    at_access: &at_access,
                    fenced: &fenced,
                    stalled: Cell::new(false),
                };
                Twin::new(world.slabs(), &stall).push(c, object(9))
            });
            let drain = scope.spawn(|| {
                while !at_access.load(Acquire) {
                    std::thread::yield_now();
                }
                let hot = world.field(0, Field::hot(c));
                let old = hot
                    .swap(ptr::without_provenance_mut(marker), Relaxed)
                    .addr();
                os.fence(0).unwrap();
                let at_return = free_slot.load(Relaxed).addr();
                fenced.store(true, Release);
                (old, at_return)
            });
            let (old, at_return) = drain.join().unwrap();
            (push.join().unwrap(), old, at_return)
        });
        assert_eq!(
            old,
            world.slot(0, 2).addr(),
            "the drainer displaced the loaded magazine"
        );
        // Once the access is over the fence races the commit. If the fence wins, the push is
        // aborted and its retry meets the marker; if the commit wins, the word is the push's
        // and the drainer takes nothing. Either way the slot was written before the fence
        // returned and not after.
        let hot = world.field(0, Field::hot(c)).load(Relaxed).addr();
        if pushed {
            assert_eq!(hot, world.slot(0, 3).addr(), "the push committed");
        } else {
            assert_eq!(
                hot, marker,
                "the push was aborted and its retry met the marker"
            );
        }
        assert_eq!(
            free_slot.load(Relaxed).addr(),
            at_return,
            "the aborted push wrote the magazine after the fence returned"
        );
        assert_eq!(
            at_return,
            object(9).as_ptr().addr(),
            "the store came before the fence"
        );
    }

    #[test]
    fn a_thread_without_a_valid_cpu_touches_no_slab() {
        let c = class();
        let world = World::populated();
        let before = world.snapshot();
        for cpu_id in [-1, -2, 2, i32::MAX, i32::MIN] {
            let twin = Twin::new(world.slabs(), Pinned(cpu_id));
            assert_eq!(twin.cpu(), None);
            assert_eq!(twin.pop(c), None);
            assert!(!twin.push(c, object(5)));
            assert_eq!(
                unsafe { twin.store_if_equal(Field::hot(c), ptr::null_mut(), ptr::null_mut()) },
                StoreIfEqual::NoCpu
            );
            let magazine = NonNull::new(world.slot(2, 0)).unwrap();
            assert_eq!(
                unsafe { twin.stack_push(Field::spare(c, Spare::Empty), magazine) },
                StackPush::NoCpu
            );
            assert_eq!(
                twin.stack_pop(Field::spare(c, Spare::Full)),
                StackPop::NoCpu
            );
        }
        assert_eq!(before, world.snapshot());
    }
}
