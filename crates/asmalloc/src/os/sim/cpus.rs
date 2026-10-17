//! Simulated threads, the CPUs they run on, and the restartable sequences they are in.
//!
//! Each simulated thread has a CPU number (what its rseq area's `cpu_id` would hold) and a
//! sequence state. A twin running on a [`SimThread`] calls [`Machine::step`] before each step:
//! `ReadCpu` begins a sequence, and any later step fails with [`Aborted`] if the sequence was
//! interrupted since. Interruptions come from the test ([`SimThread::abort`],
//! [`SimThread::abort_at`]), from moving the thread ([`SimThread::set_cpu`]), and from a fence on
//! its CPU ([`Cpus::fence`], which the simulator's `Os::fence` calls).
//!
//! This is exact for threads scheduled cooperatively, the way a deterministic scheduler runs
//! them: one OS thread, switching only inside `step`, so a twin's committing store follows its
//! `Commit` step with no switch between. With real threads, a fence can return while a twin that
//! has already passed its `Commit` step is still storing; the trait has no call after the store
//! to wait for.

use crate::os::machine::{Aborted, Machine, Step};
use core::sync::atomic::{AtomicI32, AtomicU8, Ordering};

/// The number of simulated threads a [`super::SimOs`] has.
pub const SIM_THREADS: usize = 64;

const IDLE: u8 = 0;
const RUNNING: u8 = 1;
const ABORTED: u8 = 2;
const GONE: u8 = 3;

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

/// One simulated thread's rseq state.
pub(super) struct ThreadState {
    cpu: AtomicI32,
    sequence: AtomicU8,
    abort_at: AtomicU8,
}

impl ThreadState {
    const fn new(cpu: i32) -> ThreadState {
        ThreadState {
            cpu: AtomicI32::new(cpu),
            sequence: AtomicU8::new(IDLE),
            abort_at: AtomicU8::new(NO_STEP),
        }
    }

    /// Interrupts the thread's sequence, if it is in one.
    fn interrupt(&self) -> bool {
        self.sequence
            .compare_exchange(RUNNING, ABORTED, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
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
            index,
        }
    }

    /// Aborts every sequence in flight on `cpu` and returns how many there were.
    pub(super) fn fence(&self, cpu: u32) -> usize {
        self.threads
            .iter()
            .filter(|thread| thread.cpu.load(Ordering::Acquire) == cpu as i32)
            .filter(|thread| thread.interrupt())
            .count()
    }

    /// Every thread but `survivor` vanishes, as in the child of a fork; the survivor's sequence,
    /// if any, is aborted, as a fork's return to user space would.
    pub(super) fn fork(&self, survivor: usize) {
        for (index, thread) in self.threads.iter().enumerate() {
            if index == survivor {
                thread.interrupt();
            } else {
                thread.sequence.store(GONE, Ordering::Release);
            }
        }
    }
}

/// A simulated thread, for running twins on: a [`Machine`] whose CPU and interruptions a test
/// controls.
#[derive(Clone, Copy)]
pub struct SimThread<'a> {
    state: &'a ThreadState,
    index: usize,
}

impl SimThread<'_> {
    /// The thread's number.
    pub fn index(&self) -> usize {
        self.index
    }

    /// Moves the thread to CPU `cpu`, or takes its registration away with a negative number. A
    /// move interrupts a sequence in flight, as migration does.
    pub fn set_cpu(&self, cpu: i32) {
        self.state.cpu.store(cpu, Ordering::Release);
        self.state.interrupt();
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
        self.state.sequence.load(Ordering::Acquire) == RUNNING
    }

    fn check_alive(&self) {
        assert!(
            self.state.sequence.load(Ordering::Acquire) != GONE,
            "simulated thread {} did not survive the fork",
            self.index
        );
    }
}

impl Machine for SimThread<'_> {
    fn cpu_id(&self) -> i32 {
        self.check_alive();
        self.state.cpu.load(Ordering::Acquire)
    }

    fn step(&self, step: Step) -> Result<(), Aborted> {
        self.check_alive();
        let state = self.state;
        if state
            .abort_at
            .compare_exchange(encode(step), NO_STEP, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            state.sequence.store(IDLE, Ordering::Release);
            return Err(Aborted);
        }
        match step {
            Step::ReadCpu => {
                state.sequence.store(RUNNING, Ordering::Release);
                Ok(())
            }
            _ => {
                let next = if step == Step::Commit { IDLE } else { RUNNING };
                match state.sequence.compare_exchange(
                    RUNNING,
                    next,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => Ok(()),
                    Err(_) => {
                        state.sequence.store(IDLE, Ordering::Release);
                        Err(Aborted)
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(thread: &SimThread<'_>) -> Result<(), Aborted> {
        thread.step(Step::ReadCpu)?;
        thread.step(Step::Load)?;
        thread.step(Step::Access)?;
        thread.step(Step::Commit)
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
        let (a, b, c) = (cpus.thread(0), cpus.thread(1), cpus.thread(2));
        b.set_cpu(0);
        for thread in [&a, &b, &c] {
            thread.step(Step::ReadCpu).unwrap();
        }
        assert_eq!(cpus.fence(0), 2);
        assert_eq!(a.step(Step::Load), Err(Aborted));
        assert_eq!(b.step(Step::Load), Err(Aborted));
        assert_eq!(c.step(Step::Load), Ok(()));
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
