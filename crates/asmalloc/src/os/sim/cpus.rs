//! Simulated threads, the CPUs they run on, and the restartable sequences they are in.
//!
//! Each simulated thread has a CPU number (what its rseq area's `cpu_id` would hold) and a
//! sequence state. A twin running on a [`SimThread`] calls [`Machine::step`] before each step:
//! `ReadCpu` begins a sequence, and any later step fails with [`Aborted`] if the sequence was
//! interrupted since. Interruptions come from the test ([`SimThread::abort`],
//! [`SimThread::abort_at`]), from moving the thread ([`SimThread::set_cpu`]), from a fence on
//! its CPU ([`Cpus::fence`], which the simulator's `Os::fence` calls), and from another thread
//! starting a sequence on the same CPU, which it can only do if this one was preempted.
//!
//! Threads that share a simulated CPU need a cooperative scheduler (one OS thread, switching
//! inside `step`), as the interleaving checkers have. Two free-running OS threads on one
//! simulated CPU can both pass their `Commit` steps and both commit, losing an update, where
//! one real CPU runs one sequence at a time. Free-running threads on different CPUs are
//! modelled faithfully, fences included.
//!
//! A sequence that passes its `Commit` step is committing until the twin calls
//! [`Machine::committed`] after its store. A fence does not interrupt a committing sequence (the
//! store is the routine's last instruction, past the point of abort); it waits for it to finish,
//! so `fence` returns only after every sequence on its CPU has completed or been aborted, with
//! real threads as well as cooperatively scheduled ones. A twin must not yield to a scheduler
//! between `Commit` and `committed`, or a fence from the same OS thread would wait forever.
//!
//! The same holds for the magazine access: a sequence that passes its `Access` step is accessing
//! until the twin calls [`Machine::accessed`]. A real sequence that a fence aborts executes no
//! further instruction, but a free-running twin that has passed the check cannot be stopped
//! before its access. So the fence waits for the access to end and aborts the sequence then:
//! the access lands before the fence returns, as if the fence had arrived one instruction
//! later, and never after it. Without that wait an aborted push could store its object into a
//! magazine the drainer had already taken and passed on.

use crate::os::machine::{Aborted, Machine, Step};
use core::sync::atomic::{AtomicU8, AtomicU64, Ordering};

/// The number of simulated threads a [`super::SimOs`] has.
pub const SIM_THREADS: usize = if cfg!(kani) { 2 } else { 64 };

const IDLE: u8 = 0;
const RUNNING: u8 = 1;
const ABORTED: u8 = 2;
const COMMITTING: u8 = 3;
const GONE: u8 = 4;
const ACCESSING: u8 = 5;

/// No abort is scheduled.
const NO_STEP: u8 = 0;

const fn encode(step: Step) -> u8 {
    match step {
        Step::ReadCpu => 1,
        Step::Load => 2,
        Step::Access => 3,
        Step::Commit => 4,
    }
}

/// A thread's CPU and sequence state in one word, so that one atomic instruction reads or
/// changes both: a fence that decides by the CPU acts on the state it saw with it, and a move
/// to another CPU interrupts the sequence in the same instruction, as a migration does. With
/// two words, a fence could read the new CPU and pass over a sequence still accessing the old
/// CPU's slab.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Word(u64);

impl Word {
    const fn new(cpu: i32, sequence: u8) -> Word {
        Word((cpu as u32 as u64) << 32 | sequence as u64)
    }

    const fn cpu(self) -> i32 {
        (self.0 >> 32) as u32 as i32
    }

    const fn sequence(self) -> u8 {
        self.0 as u8
    }

    const fn with_sequence(self, sequence: u8) -> Word {
        Word::new(self.cpu(), sequence)
    }

    /// Whether the sequence is past the point where an interruption can stop it, so that
    /// whoever would interrupt it must wait until it ends.
    const fn must_wait_out(self) -> bool {
        self.sequence() == COMMITTING || self.sequence() == ACCESSING
    }

    /// The word after an interruption: a running sequence is aborted.
    const fn interrupted(self) -> Word {
        if self.sequence() == RUNNING {
            self.with_sequence(ABORTED)
        } else {
            self
        }
    }
}

/// Which threads a barrier interrupts.
#[derive(Clone, Copy)]
enum Target {
    Any,
    Cpu(i32),
}

/// One simulated thread's rseq state.
pub(super) struct ThreadState {
    word: AtomicU64,
    abort_at: AtomicU8,
}

impl ThreadState {
    const fn new(cpu: i32) -> ThreadState {
        ThreadState {
            word: AtomicU64::new(Word::new(cpu, IDLE).0),
            abort_at: AtomicU8::new(NO_STEP),
        }
    }

    fn load(&self) -> Word {
        Word(self.word.load(Ordering::Acquire))
    }

    /// Replaces the word with `change(old)` in one compare-and-swap, first waiting out a
    /// committing sequence or an access in progress whenever `waits(old)` says to. Returns the
    /// word it replaced.
    fn update(&self, waits: impl Fn(Word) -> bool, change: impl Fn(Word) -> Word) -> Word {
        let mut old = self.load();
        loop {
            if waits(old) && old.must_wait_out() {
                core::hint::spin_loop();
                old = self.load();
                continue;
            }
            match self.word.compare_exchange(
                old.0,
                change(old).0,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return old,
                Err(now) => old = Word(now),
            }
        }
    }

    /// Sets the sequence state, keeping the CPU, without waiting.
    fn set_sequence(&self, sequence: u8) -> Word {
        self.update(|_| false, |old| old.with_sequence(sequence))
    }

    /// Changes the sequence state from `from` to `to`, keeping the CPU, or returns the state
    /// found instead of `from`.
    fn transition(&self, from: u8, to: u8) -> Result<(), u8> {
        let mut old = self.load();
        loop {
            if old.sequence() != from {
                return Err(old.sequence());
            }
            match self.word.compare_exchange(
                old.0,
                old.with_sequence(to).0,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Ok(()),
                Err(now) => old = Word(now),
            }
        }
    }

    /// Interrupts the thread's sequence, if it is in one, and returns whether it was. A
    /// sequence that is committing cannot be interrupted any more; this waits for it to end, as
    /// a fence waits for a routine's last instruction. One in the middle of its magazine access
    /// is interrupted once the access is over.
    fn interrupt(&self) -> bool {
        self.barrier(Target::Any)
    }

    /// A successful read-modify-write of the state, whether or not it changes it: the write
    /// orders this call against the thread's next `ReadCpu` (itself a read-modify-write) and
    /// hands that sequence everything the caller stored before, as the kernel's barrier does.
    /// If the thread is on a CPU `target` names, it also interrupts a running sequence, waiting
    /// out a committing one and an access in progress; it returns whether it interrupted one.
    /// The CPU it decides by is the one in the word it replaces.
    fn barrier(&self, target: Target) -> bool {
        let hits = move |word: Word| match target {
            Target::Any => true,
            Target::Cpu(cpu) => word.cpu() == cpu,
        };
        let old = self.update(hits, |old| if hits(old) { old.interrupted() } else { old });
        hits(old) && old.sequence() == RUNNING
    }

    /// Moves the thread to `cpu`, interrupting its sequence in the same instruction and first
    /// waiting out a committing sequence or an access in progress, which belong to the old CPU.
    fn migrate(&self, cpu: i32) {
        self.update(|_| true, |old| Word::new(cpu, old.interrupted().sequence()));
    }
}

/// `N` simulated threads. Thread `i` starts on CPU `i`, the way threads of a process with no
/// more threads than CPUs usually run.
pub(super) struct Cpus<const N: usize> {
    threads: [ThreadState; N],
}

impl<const N: usize> Cpus<N> {
    pub(super) fn new() -> Cpus<N> {
        Cpus {
            threads: core::array::from_fn(|index| ThreadState::new(index as i32)),
        }
    }

    pub(super) fn thread(&self, index: usize) -> SimThread<'_> {
        SimThread {
            state: &self.threads[index],
            threads: &self.threads,
            index,
        }
    }

    /// Aborts every sequence in flight on `cpu` and returns how many there were.
    /// Every thread, on the CPU or not, gets a read-modify-write of its state, so that a
    /// sequence it starts later sees everything stored before the fence; without that, a free-
    /// running thread's twin could read a slab word older than a drainer's marker and commit
    /// over it.
    pub(super) fn fence(&self, cpu: u32) -> usize {
        self.threads
            .iter()
            .filter(|thread| thread.barrier(Target::Cpu(cpu as i32)))
            .count()
    }

    /// Every thread but `survivor` vanishes, as in the child of a fork; the survivor's sequence,
    /// if any, is aborted, as a fork's return to user space would.
    pub(super) fn fork(&self, survivor: usize) {
        for (index, thread) in self.threads.iter().enumerate() {
            if index == survivor {
                thread.interrupt();
            } else {
                thread.set_sequence(GONE);
            }
        }
    }
}

/// A simulated thread, for running twins on: a [`Machine`] whose CPU and interruptions a test
/// controls.
#[derive(Clone, Copy)]
pub struct SimThread<'a> {
    state: &'a ThreadState,
    threads: &'a [ThreadState],
    index: usize,
}

impl SimThread<'_> {
    /// The thread's number.
    pub fn index(&self) -> usize {
        self.index
    }

    /// Moves the thread to CPU `cpu`, or takes its registration away with a negative number. A
    /// move interrupts a sequence in flight, as migration does, and waits out a committing
    /// sequence or an access in progress first, so a fence on the old CPU never finds the
    /// thread gone while its sequence still writes there.
    pub fn set_cpu(&self, cpu: i32) {
        self.state.migrate(cpu);
    }

    /// Interrupts the thread's sequence, if it is in one: a preemption or a signal.
    pub fn abort(&self) {
        self.state.interrupt();
    }

    /// Interrupts the thread once, just before it next reaches `step`.
    pub fn abort_at(&self, step: Step) {
        self.state.abort_at.store(encode(step), Ordering::Release);
    }

    /// Whether the thread is inside a sequence that has not been interrupted.
    pub fn in_sequence(&self) -> bool {
        self.state.load().sequence() == RUNNING
    }

    fn check_alive(&self) {
        assert!(
            self.state.load().sequence() != GONE,
            "simulated thread {} did not survive the fork",
            self.index
        );
    }
}

impl Machine for SimThread<'_> {
    fn cpu_id(&self) -> i32 {
        self.check_alive();
        self.state.load().cpu()
    }

    fn step(&self, step: Step) -> Result<(), Aborted> {
        self.check_alive();
        let state = self.state;
        if state
            .abort_at
            .compare_exchange(encode(step), NO_STEP, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            state.set_sequence(IDLE);
            return Err(Aborted);
        }
        match step {
            Step::ReadCpu => {
                // A read-modify-write, so that it reads what the last fence of this thread
                // wrote, and the CPU the sequence runs on comes from the same word.
                let cpu = state.set_sequence(RUNNING).cpu();
                // One CPU runs one thread at a time: if another thread is in a sequence on
                // this CPU, it was preempted for this one to start.
                for (index, other) in self.threads.iter().enumerate() {
                    if index != self.index {
                        other.barrier(Target::Cpu(cpu));
                    }
                }
                Ok(())
            }
            _ => {
                let next = match step {
                    Step::Commit => COMMITTING,
                    Step::Access => ACCESSING,
                    _ => RUNNING,
                };
                match state.transition(RUNNING, next) {
                    Ok(()) => Ok(()),
                    Err(now) => {
                        // A fence would wait for ever on an access that is never ended.
                        assert!(
                            now != ACCESSING,
                            "a twin took a step without ending its access (`Machine::accessed`)"
                        );
                        state.set_sequence(IDLE);
                        Err(Aborted)
                    }
                }
            }
        }
    }

    fn accessed(&self) {
        // Only an access in progress ends here. Nothing else changes the state meanwhile: a
        // fence or a move waits, and only this thread runs the sequence.
        let _ = self.state.transition(ACCESSING, RUNNING);
    }

    fn committed(&self) {
        // Only a committing sequence ends here; anything else (a twin that called this without
        // committing) is left as it is.
        let _ = self.state.transition(COMMITTING, IDLE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(thread: &SimThread<'_>) -> Result<(), Aborted> {
        thread.step(Step::ReadCpu)?;
        thread.step(Step::Load)?;
        thread.step(Step::Access)?;
        thread.accessed();
        thread.step(Step::Commit)?;
        thread.committed();
        Ok(())
    }

    /// What the owner test in `heap::tests` caught as a block with two owners: a fence returned
    /// while a twin stood between its `Access` check and its store, the drainer took the
    /// magazine and passed it on, and the store then landed in it.
    #[test]
    fn a_fence_waits_for_an_access_in_progress_and_then_aborts_the_sequence() {
        use std::sync::atomic::AtomicBool;
        use std::thread;
        let cpus = Cpus::<4>::new();
        let twin = cpus.thread(0);
        twin.step(Step::ReadCpu).unwrap();
        twin.step(Step::Load).unwrap();
        twin.step(Step::Access).unwrap();
        let fenced = AtomicBool::new(false);
        thread::scope(|scope| {
            let fence = scope.spawn(|| {
                let aborted = cpus.fence(0);
                fenced.store(true, Ordering::Release);
                aborted
            });
            thread::sleep(std::time::Duration::from_millis(20));
            assert!(
                !fenced.load(Ordering::Acquire),
                "the fence waits for the access"
            );
            twin.accessed();
            assert_eq!(fence.join().unwrap(), 1, "and then aborts the sequence");
        });
        assert_eq!(twin.step(Step::Commit), Err(Aborted));
        assert_eq!(run(&twin), Ok(()), "the restart succeeds");
    }

    /// A move and a fence on the old CPU, both while the thread is in its access. When the move
    /// stored the new CPU before interrupting the thread, the fence read the new CPU, passed
    /// over the thread and returned with the access still in progress on the old CPU. The test
    /// gives an old-order move time to store the new CPU (it does so at once), then fences.
    #[test]
    fn a_fence_on_the_old_cpu_waits_for_an_access_that_a_move_interrupts() {
        use std::sync::atomic::AtomicBool;
        use std::thread;
        use std::time::{Duration, Instant};
        let cpus = Cpus::<4>::new();
        let twin = cpus.thread(0);
        twin.step(Step::ReadCpu).unwrap();
        twin.step(Step::Load).unwrap();
        twin.step(Step::Access).unwrap();
        let moving = AtomicBool::new(false);
        let access_over = AtomicBool::new(false);
        let patience = Duration::from_millis(100);
        thread::scope(|scope| {
            let mover = scope.spawn(|| {
                moving.store(true, Ordering::Release);
                twin.set_cpu(1);
            });
            while !moving.load(Ordering::Acquire) {
                thread::yield_now();
            }
            let started = Instant::now();
            while twin.cpu_id() != 1 && started.elapsed() < patience {
                thread::yield_now();
            }
            let fence = scope.spawn(|| {
                cpus.fence(0);
                access_over.load(Ordering::Acquire)
            });
            let started = Instant::now();
            while !fence.is_finished() && started.elapsed() < patience {
                thread::yield_now();
            }
            access_over.store(true, Ordering::Release);
            twin.accessed();
            assert!(
                fence.join().unwrap(),
                "the fence returned during the access"
            );
            mover.join().unwrap();
        });
        assert_eq!(twin.cpu_id(), 1);
        assert_eq!(twin.step(Step::Commit), Err(Aborted), "the move aborted it");
    }

    #[test]
    fn a_sequence_between_its_access_and_its_commit_can_be_aborted() {
        let cpus = Cpus::<4>::new();
        let twin = cpus.thread(0);
        twin.step(Step::ReadCpu).unwrap();
        twin.step(Step::Access).unwrap();
        twin.accessed();
        assert_eq!(cpus.fence(0), 1);
        assert_eq!(twin.step(Step::Commit), Err(Aborted));
    }

    #[test]
    fn a_fence_waits_for_a_committing_sequence_and_does_not_abort_it() {
        use std::sync::atomic::AtomicBool;
        use std::thread;
        let cpus = Cpus::<4>::new();
        let twin = cpus.thread(0);
        twin.step(Step::ReadCpu).unwrap();
        twin.step(Step::Commit).unwrap();
        let fenced = AtomicBool::new(false);
        thread::scope(|scope| {
            let fence = scope.spawn(|| {
                let aborted = cpus.fence(0);
                fenced.store(true, Ordering::Release);
                aborted
            });
            thread::sleep(std::time::Duration::from_millis(20));
            assert!(
                !fenced.load(Ordering::Acquire),
                "the fence waits for the store"
            );
            twin.committed();
            assert_eq!(
                fence.join().unwrap(),
                0,
                "a committing sequence is not aborted"
            );
        });
        assert!(!twin.in_sequence());
    }

    #[test]
    fn a_sequence_runs_until_something_interrupts_it() {
        let cpus = Cpus::<4>::new();
        let thread = cpus.thread(2);
        assert_eq!(thread.cpu_id(), 2);
        assert_eq!(run(&thread), Ok(()));
        assert!(!thread.in_sequence());

        thread.step(Step::ReadCpu).unwrap();
        assert!(thread.in_sequence());
        thread.abort();
        assert_eq!(thread.step(Step::Load), Err(Aborted));
        assert_eq!(run(&thread), Ok(()), "the restart succeeds");
    }

    #[test]
    fn an_abort_can_be_aimed_at_one_step() {
        let cpus = Cpus::<4>::new();
        let thread = cpus.thread(0);
        thread.abort_at(Step::Commit);
        assert_eq!(run(&thread), Err(Aborted));
        assert_eq!(run(&thread), Ok(()), "it fires once");
    }

    #[test]
    fn a_fence_aborts_only_sequences_on_its_cpu() {
        let cpus = Cpus::<4>::new();
        let (a, b) = (cpus.thread(0), cpus.thread(1));
        for thread in [&a, &b] {
            thread.step(Step::ReadCpu).unwrap();
        }
        assert_eq!(cpus.fence(0), 1);
        assert_eq!(a.step(Step::Load), Err(Aborted));
        assert_eq!(b.step(Step::Load), Ok(()));
        assert_eq!(cpus.fence(0), 0, "nothing is in flight there now");
    }

    #[test]
    fn moving_a_thread_interrupts_it_and_changes_what_it_reads() {
        let cpus = Cpus::<4>::new();
        let thread = cpus.thread(1);
        thread.step(Step::ReadCpu).unwrap();
        thread.set_cpu(3);
        assert_eq!(thread.step(Step::Access), Err(Aborted));
        assert_eq!(thread.cpu_id(), 3);
        thread.set_cpu(-1);
        assert_eq!(thread.cpu_id(), -1);
    }

    #[test]
    fn starting_a_sequence_preempts_the_one_in_flight_on_the_same_cpu() {
        let cpus = Cpus::<4>::new();
        let (a, b, c) = (cpus.thread(0), cpus.thread(1), cpus.thread(2));
        b.set_cpu(0);
        a.step(Step::ReadCpu).unwrap();
        c.step(Step::ReadCpu).unwrap();
        b.step(Step::ReadCpu).unwrap();
        assert_eq!(a.step(Step::Load), Err(Aborted), "b took a's CPU");
        assert_eq!(c.step(Step::Load), Ok(()), "c is on another CPU");
        assert_eq!(b.step(Step::Load), Ok(()));
    }

    #[test]
    fn after_a_fork_only_the_survivor_runs() {
        let cpus = Cpus::<4>::new();
        let survivor = cpus.thread(1);
        survivor.step(Step::ReadCpu).unwrap();
        cpus.fork(1);
        assert_eq!(survivor.step(Step::Load), Err(Aborted));
        assert_eq!(run(&survivor), Ok(()));
    }

    #[test]
    #[should_panic(expected = "did not survive the fork")]
    fn a_thread_lost_in_a_fork_cannot_run() {
        let cpus = Cpus::<4>::new();
        cpus.fork(1);
        let _ = cpus.thread(0).cpu_id();
    }
}
