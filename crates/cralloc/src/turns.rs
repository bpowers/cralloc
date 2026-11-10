//! Tasks that take turns, one running at a time: the threads of an interleaving explorer. Test
//! code only.
//!
//! An exploration runs millions of schedules of a few tasks, and each schedule hands the turn
//! around a few times. What a hand-over costs decides how long the search takes, and on OS threads
//! it costs a sleep and a wake-up: tens of microseconds on a machine with other work to do, far
//! more than the steps between hand-overs. So where the C library has `swapcontext`, the tasks are
//! coroutines on the calling thread, a hand-over is a function call and a system call for the
//! signal mask, and nothing ever sleeps. Elsewhere (Miri, musl) they are OS threads that sleep on a
//! condition variable each, and only the one named is woken. Tasks persist from one round to
//! the next, so a schedule spawns nothing.
//!
//! The explorers argue about sequential consistency at the granularity of their switch points, and
//! only one task runs at a time, each hand-over ordering everything before it against everything
//! after it, so which of the two runs the tasks changes nothing they can see, but for what is per OS
//! thread: an explorer sets its own thread-local afresh whenever a task gets the turn back, and no
//! task may hand over while it is unwinding, since the count of panics is per OS thread too.

use std::cell::Cell;

/// A running task's job for one round: returns who goes next once this task is done, a task or
/// [`Team::driver`]. If it unwinds the process ends, since the other tasks stand partway through
/// theirs and nothing would give them the turn.
pub(crate) type Job<'a> = &'a (dyn Fn(usize) -> usize + Sync);

/// Erases a job's lifetime, for [`Team::round`] to store it where the tasks can find it.
fn erase(job: Job<'_>) -> Job<'static> {
    // SAFETY: only the lifetime changes. `round` clears the erased job before it returns, and
    // its contract says that every task has then finished the job.
    unsafe { core::mem::transmute(job) }
}

#[cfg(all(target_env = "gnu", target_os = "linux", not(miri)))]
pub(crate) use self::coroutines::with_team;
#[cfg(not(all(target_env = "gnu", target_os = "linux", not(miri))))]
pub(crate) use self::threads::with_team;

#[cfg(all(target_env = "gnu", target_os = "linux", not(miri)))]
pub(crate) use self::coroutines::Team;
#[cfg(not(all(target_env = "gnu", target_os = "linux", not(miri))))]
pub(crate) use self::threads::Team;

#[cfg(all(target_env = "gnu", target_os = "linux", not(miri)))]
mod coroutines {
    use super::{Cell, Job, erase};
    use std::boxed::Box;
    use std::vec::Vec;

    /// As much of a `ucontext_t` as the code here names: both architectures' start with
    /// `uc_flags`, `uc_link` and a `stack_t`. It must stay where it is once filled in, since on
    /// x86-64 it points into itself. The whole is 968 bytes there and 4,560 on aarch64.
    #[repr(C, align(16))]
    struct Context {
        flags: u64,
        link: *mut Context,
        stack_base: *mut u8,
        stack_flags: i32,
        stack_size: usize,
        rest: [u8; 8192],
    }

    unsafe extern "C" {
        fn getcontext(context: *mut Context) -> i32;
        fn makecontext(context: *mut Context, start: extern "C" fn(), args: i32, ...);
        fn swapcontext(save: *mut Context, resume: *const Context) -> i32;
        fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
        fn mprotect(addr: *mut u8, len: usize, prot: i32) -> i32;
        fn munmap(addr: *mut u8, len: usize) -> i32;
    }

    const PROT_READ: i32 = 1;
    const PROT_WRITE: i32 = 2;
    const MAP_PRIVATE: i32 = 2;
    const MAP_ANONYMOUS: i32 = 0x20;
    const MAP_NORESERVE: i32 = 0x4000;
    /// Larger than a page on any machine we use.
    const GUARD: usize = 1 << 16;
    /// As `RUST_MIN_STACK` gives test threads. Only what a task touches is committed.
    const STACK: usize = 8 << 20;

    /// A task's stack, with a guard below it, so that running out of it is a fault.
    struct Stack(*mut u8);

    impl Stack {
        fn new() -> Stack {
            // SAFETY: a fresh anonymous mapping, and the guard is inside it.
            unsafe {
                let base = mmap(
                    core::ptr::null_mut(),
                    GUARD + STACK,
                    PROT_READ | PROT_WRITE,
                    MAP_PRIVATE | MAP_ANONYMOUS | MAP_NORESERVE,
                    -1,
                    0,
                );
                assert!(base.addr() != usize::MAX, "a task's stack can be mapped");
                assert_eq!(mprotect(base, GUARD, 0), 0, "a stack's guard can be set");
                Stack(base)
            }
        }
    }

    impl Drop for Stack {
        fn drop(&mut self) {
            // SAFETY: the mapping `new` made; nothing runs on it any more, since a team is
            // dropped by its driver and its tasks never run again.
            unsafe { munmap(self.0, GUARD + STACK) };
        }
    }

    /// `tasks` coroutines and the thread that drives them.
    pub(crate) struct Team {
        tasks: usize,
        /// One per task and one for the driver, last.
        contexts: Box<[core::cell::UnsafeCell<Context>]>,
        _stacks: Vec<Stack>,
        /// Who has the turn: a task starting reads who it is from here.
        current: Cell<usize>,
        job: Cell<Option<Job<'static>>>,
        /// Tasks partway through a job.
        working: Cell<usize>,
    }

    // SAFETY: every task runs on the OS thread that made the team, which holds it for as long as
    // the team lasts: nothing here crosses to another thread, and the `Sync` bound of a job exists
    // for the other implementation.
    unsafe impl Sync for Team {}

    std::thread_local! {
        /// The team whose tasks run on this thread, for a task starting.
        static TEAM: Cell<*const Team> = const { Cell::new(core::ptr::null()) };
    }

    /// Where every task starts. Runs one job a round, for as long as the team lasts.
    extern "C" fn start() {
        // SAFETY: `with_team` sets the pointer for the life of the team, and tasks run only
        // inside it.
        let team = unsafe { &*TEAM.get() };
        loop {
            let me = team.current.get();
            let job = team.job.get().expect("a task runs only in a round");
            team.working.set(team.working.get() + 1);
            let next = job(me);
            team.working.set(team.working.get() - 1);
            team.pass(me, next);
        }
    }

    /// Calls `f` with a team of `tasks` tasks. Tasks do not nest.
    pub(crate) fn with_team<R>(tasks: usize, f: impl FnOnce(&Team) -> R) -> R {
        let mut stacks = Vec::new();
        let contexts: Box<[_]> = (0..=tasks)
            .map(|_| {
                core::cell::UnsafeCell::new(Context {
                    flags: 0,
                    link: core::ptr::null_mut(),
                    stack_base: core::ptr::null_mut(),
                    stack_flags: 0,
                    stack_size: 0,
                    rest: [0; 8192],
                })
            })
            .collect();
        for context in &contexts[..tasks] {
            let stack = Stack::new();
            let context = context.get();
            // SAFETY: `context` is live and stays where it is, in the box, for as long as the
            // tasks can run. The stack is `STACK` bytes above its guard. `start` takes no
            // arguments and never returns.
            unsafe {
                assert_eq!(getcontext(context), 0);
                (*context).stack_base = stack.0.add(GUARD);
                (*context).stack_size = STACK;
                makecontext(context, start, 0);
            }
            stacks.push(stack);
        }
        let team = Team {
            tasks,
            contexts,
            _stacks: stacks,
            current: Cell::new(tasks),
            job: Cell::new(None),
            working: Cell::new(0),
        };
        assert!(TEAM.get().is_null(), "teams do not nest");
        TEAM.set(&team);
        // Whatever `f` does, tasks are suspended when it ends, and do not run again. The pointer
        // goes first, should `f` unwind.
        struct Clear;
        impl Drop for Clear {
            fn drop(&mut self) {
                TEAM.set(core::ptr::null());
            }
        }
        let _clear = Clear;
        f(&team)
    }

    impl Team {
        /// The number that means the thread driving the team, to [`Team::pass`].
        pub(crate) fn driver(&self) -> usize {
            self.tasks
        }

        /// Gives the turn to `to`, and returns when someone gives it back to `me`. `me` is the
        /// one running. A task that has not started in this round starts its job.
        pub(crate) fn pass(&self, me: usize, to: usize) {
            debug_assert!(!std::thread::panicking(), "a task hands over as it unwinds");
            self.current.set(to);
            // SAFETY: both contexts are live and in place; `resume` was made by `makecontext`
            // or saved here, and its stack is intact, since only the running task's is in use.
            // The call returns when `me`'s saved context is resumed.
            let status = unsafe {
                swapcontext(
                    self.contexts[me].get(),
                    self.contexts[to].get().cast_const(),
                )
            };
            debug_assert_eq!(status, 0);
        }

        /// Runs a round from the driver: every task may run `job`, once, and `begin` decides who
        /// goes first and passes the turn. By the time the turn comes back to the driver for the
        /// last time, in or after `begin`, every task must have finished `job` (the explorers
        /// run or unwind every task).
        pub(crate) fn round(&self, job: Job<'_>, begin: impl FnOnce()) {
            self.job.set(Some(erase(job)));
            begin();
            self.job.set(None);
            assert_eq!(self.working.get(), 0, "a round left a task in its job");
        }
    }
}

#[cfg(not(all(target_env = "gnu", target_os = "linux", not(miri))))]
mod threads {
    use super::{Job, erase};
    use std::boxed::Box;
    use std::sync::{Condvar, Mutex};
    use std::thread;

    struct Shared {
        /// Who has the turn.
        turn: usize,
        job: Option<Job<'static>>,
        /// Tasks partway through a job.
        working: usize,
        /// The team is over: tasks go home.
        over: bool,
    }

    /// `tasks` OS threads and the thread that drives them.
    pub(crate) struct Team {
        tasks: usize,
        shared: Mutex<Shared>,
        /// One per task and one for the driver, last.
        moved: Box<[Condvar]>,
    }

    /// Calls `f` with a team of `tasks` tasks.
    pub(crate) fn with_team<R>(tasks: usize, f: impl FnOnce(&Team) -> R) -> R {
        let team = Team {
            tasks,
            shared: Mutex::new(Shared {
                turn: tasks,
                job: None,
                working: 0,
                over: false,
            }),
            moved: (0..=tasks).map(|_| Condvar::new()).collect(),
        };
        thread::scope(|scope| {
            for me in 0..tasks {
                let team = &team;
                scope.spawn(move || team.task(me));
            }
            // Before the scope waits for the tasks, whether or not `f` unwinds.
            struct End<'a>(&'a Team);
            impl Drop for End<'_> {
                fn drop(&mut self) {
                    self.0.lock().over = true;
                    for moved in &self.0.moved {
                        moved.notify_all();
                    }
                }
            }
            let _end = End(&team);
            f(&team)
        })
    }

    impl Team {
        fn lock(&self) -> std::sync::MutexGuard<'_, Shared> {
            self.shared.lock().unwrap_or_else(|e| e.into_inner())
        }

        fn task(&self, me: usize) {
            while self.wait(me) {
                let job = {
                    let mut shared = self.lock();
                    shared.working += 1;
                    shared.job.expect("a task runs only in a round")
                };
                let next = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job(me)))
                    .unwrap_or_else(|_| std::process::abort());
                self.lock().working -= 1;
                self.give(next);
            }
        }

        /// Waits for the turn. Returns false if the team is over.
        fn wait(&self, me: usize) -> bool {
            let mut shared = self.lock();
            while shared.turn != me && !shared.over {
                shared = self.moved[me]
                    .wait(shared)
                    .unwrap_or_else(|e| e.into_inner());
            }
            !shared.over
        }

        fn give(&self, to: usize) {
            self.lock().turn = to;
            self.moved[to].notify_one();
        }

        /// The number that means the thread driving the team, to [`Team::pass`].
        pub(crate) fn driver(&self) -> usize {
            self.tasks
        }

        /// Gives the turn to `to`, and returns when someone gives it back to `me`. `me` is the
        /// one running. A task that has not started in this round starts its job.
        pub(crate) fn pass(&self, me: usize, to: usize) {
            self.give(to);
            self.wait(me);
        }

        /// Runs a round from the driver: every task may run `job`, once, and `begin` decides who
        /// goes first and passes the turn. By the time the turn comes back to the driver for the
        /// last time, in or after `begin`, every task must have finished `job` (the explorers
        /// run or unwind every task).
        pub(crate) fn round(&self, job: Job<'_>, begin: impl FnOnce()) {
            self.lock().job = Some(erase(job));
            begin();
            let mut shared = self.lock();
            shared.job = None;
            assert_eq!(shared.working, 0, "a round left a task in its job");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::vec::Vec;

    /// Tasks hand the turn round in a ring, twice, and the last hands it to the driver: the order
    /// is exactly what the passes say, round after round.
    #[test]
    fn the_turn_goes_where_it_is_passed() {
        with_team(3, |team| {
            for round in 0..3 {
                let seen = Mutex::new(Vec::new());
                let job = |me: usize| {
                    seen.lock().unwrap().push((me, 0));
                    team.pass(me, (me + 1) % 3);
                    seen.lock().unwrap().push((me, 1));
                    if me == 2 { team.driver() } else { (me + 1) % 3 }
                };
                team.round(&job, || team.pass(team.driver(), 0));
                let seen = seen.into_inner().unwrap();
                assert_eq!(
                    seen,
                    [(0, 0), (1, 0), (2, 0), (0, 1), (1, 1), (2, 1)],
                    "round {round}"
                );
            }
        });
    }

    /// A task can unwind and catch its panic while others stand suspended.
    #[test]
    fn a_task_can_unwind_beside_suspended_tasks() {
        with_team(2, |team| {
            let job = |me: usize| {
                if me == 0 {
                    team.pass(0, 1);
                    let _ = std::panic::catch_unwind(|| {
                        std::panic::resume_unwind(std::boxed::Box::new(()))
                    });
                    team.pass(0, 1);
                    team.driver()
                } else {
                    let caught = std::panic::catch_unwind(|| {
                        std::panic::resume_unwind(std::boxed::Box::new(()))
                    });
                    assert!(caught.is_err());
                    team.pass(1, 0);
                    0
                }
            };
            team.round(&job, || team.pass(team.driver(), 0));
        });
    }
}
