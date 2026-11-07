//! An exhaustive interleaving checker for the lock-free protocols. Test code only.
//!
//! loom would do this job, but the allocator crate takes no dependencies, so this is a small
//! checker in the style of CHESS. A scenario runs a few threads that take turns: exactly one
//! runs at a time, and before every atomic operation in the front end and the depot (the
//! switch points in `sync.rs`) the checker may let another thread run instead. Each execution
//! follows a recorded sequence of choices; a depth-first search over those sequences runs
//! every schedule with at most a given number of preemptions (a switch away from a thread that
//! could have continued). Scenarios may add choices of their own with [`choose`], which the
//! simulated CPUs use for migration and aborts.
//!
//! The checker explores sequentially consistent interleavings at the granularity of atomic
//! operations. It does not explore weaker memory orders: those are argued in
//! `docs/research/memory-ordering.md`, and checked by Miri's weak memory emulation and by the tests
//! on real threads (`crate::stress`).
//!
//! A run can also simulate `fork`: at a switch point every thread but the running one stops
//! for good (it is "killed" and later unwound out of its body), and a callback wipes what the
//! child would see wiped. What the killed threads held privately is lost, as in a forked
//! child.

use std::boxed::Box;
use std::cell::Cell;
use std::panic::{self, AssertUnwindSafe};
use std::string::{String, ToString};
use std::sync::{Condvar, Mutex, MutexGuard};
use std::thread;
use std::vec::Vec;

/// One choice made during an execution: which of `count` options was taken.
#[derive(Clone, Copy, Debug)]
struct Choice {
    taken: usize,
    count: usize,
}

/// The payload a killed thread unwinds with.
struct Killed;

/// What a run is allowed to do.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Bounds {
    /// Switches away from a thread that could have continued, per execution.
    pub(crate) preemptions: usize,
    /// Whether one simulated fork may happen per execution, at any switch point.
    pub(crate) fork: bool,
    /// Switch points per execution before the execution is reported as not terminating.
    pub(crate) max_steps: usize,
    /// Executions before the search gives up and reports how far it got.
    pub(crate) max_executions: usize,
}

impl Bounds {
    pub(crate) const fn preemptions(preemptions: usize) -> Bounds {
        Bounds {
            preemptions,
            fork: false,
            max_steps: 20_000,
            max_executions: 2_000_000,
        }
    }

    pub(crate) const fn with_fork(self) -> Bounds {
        Bounds { fork: true, ..self }
    }
}

/// What a search found.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Report {
    /// Executions run.
    pub(crate) executions: usize,
    /// Executions in which a fork happened.
    pub(crate) forks: usize,
    /// Whether every schedule within the bounds was run.
    pub(crate) complete: bool,
}

struct State {
    running: usize,
    finished: Vec<bool>,
    killed: Vec<bool>,
    prefix: Vec<Choice>,
    trail: Vec<Choice>,
    preemptions: usize,
    steps: usize,
    forked: bool,
    failure: Option<String>,
}

struct Run<'a> {
    state: Mutex<State>,
    turn: Condvar,
    bounds: Bounds,
    on_fork: &'a (dyn Fn() + Sync),
}

std::thread_local! {
    /// The run this thread takes part in, and its index in it. Set only while a checker thread
    /// runs a scenario body.
    static CONTEXT: Cell<Option<(*const Run<'static>, usize)>> = const { Cell::new(None) };
}

/// Runs `f` with the calling thread's run, if it is a checker thread.
fn with_run<R>(f: impl FnOnce(&Run<'_>, usize) -> R) -> Option<R> {
    let (run, me) = CONTEXT.with(Cell::get)?;
    // SAFETY: `CONTEXT` is set only by `Run::thread` for the duration of the scenario body,
    // and the `Run` it points to outlives every thread of the scope that runs the body.
    Some(f(unsafe { &*run }, me))
}

/// A point at which the checker may switch threads, or, on a thread inside
/// [`interrupt_at`], at which the interrupt may fire. Does nothing otherwise.
pub(crate) fn switch_point() {
    if with_run(|run, me| run.switch(me)).is_none() {
        interrupt_point();
    }
}

/// Where a thread waits for another (a futex that sleeps): the checker runs another runnable
/// thread, without counting a preemption. Does nothing outside a checker run, or when no other
/// thread can run.
pub(crate) fn wait_point() {
    let _ = with_run(|run, me| run.yield_to_others(me));
}

/// A single-threaded interrupt: `action` runs at the `remaining`-th switch point from now.
struct Interrupt {
    remaining: usize,
    action: *const (dyn Fn() + 'static),
}

std::thread_local! {
    static INTERRUPT: std::cell::RefCell<Option<Interrupt>> = const { std::cell::RefCell::new(None) };
}

fn interrupt_point() {
    let due = INTERRUPT.with(|slot| {
        let mut slot = slot.borrow_mut();
        let interrupt = slot.as_mut()?;
        if interrupt.remaining > 0 {
            interrupt.remaining -= 1;
            return None;
        }
        slot.take().map(|interrupt| interrupt.action)
    });
    if let Some(action) = due {
        // SAFETY: `interrupt_at` stores the action and removes it before it returns or
        // unwinds, so the closure it borrowed is alive whenever it is found here.
        unsafe { (*action)() };
    }
}

/// Clears the interrupt when `interrupt_at` returns or unwinds.
struct Disarm;

impl Drop for Disarm {
    fn drop(&mut self) {
        INTERRUPT.with(|slot| slot.borrow_mut().take());
    }
}

/// Runs `body` on this thread and runs `action` once, at the switch point numbered `at`
/// (from zero) within it, as a signal handler or a fork would. Switch points inside `action`
/// are not counted. Returns `body`'s result and whether the interrupt fired, that is, whether
/// `body` had more than `at` switch points.
pub(crate) fn interrupt_at<R>(at: usize, action: &dyn Fn(), body: impl FnOnce() -> R) -> (R, bool) {
    // SAFETY: only the lifetime is erased; `Disarm` removes the pointer before `action` can
    // go away, so it is never used after.
    let action: *const (dyn Fn() + 'static) = unsafe { core::mem::transmute(action) };
    INTERRUPT.with(|slot| {
        *slot.borrow_mut() = Some(Interrupt {
            remaining: at,
            action,
        })
    });
    let disarm = Disarm;
    let result = body();
    let fired = INTERRUPT.with(|slot| slot.borrow().is_none());
    drop(disarm);
    (result, fired)
}

/// A nondeterministic choice among `count` options, explored exhaustively. Outside a checker
/// run it is always 0.
pub(crate) fn choose(count: usize) -> usize {
    with_run(|run, _| run.choose(count)).unwrap_or(0)
}

impl State {
    fn pick(&mut self, count: usize) -> usize {
        if count <= 1 {
            return 0;
        }
        let at = self.trail.len();
        let taken = match self.prefix.get(at) {
            Some(choice) => {
                assert_eq!(
                    choice.count, count,
                    "the scenario is not deterministic: a replayed choice has a different number of options"
                );
                choice.taken
            }
            None => 0,
        };
        self.trail.push(Choice { taken, count });
        taken
    }

    fn runnable(&self) -> Vec<usize> {
        (0..self.finished.len())
            .filter(|&t| !self.finished[t] && !self.killed[t])
            .collect()
    }
}

impl Run<'_> {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn choose(&self, count: usize) -> usize {
        self.lock().pick(count)
    }

    fn fail(&self, state: &mut State, message: String) {
        if state.failure.is_none() {
            state.failure = Some(message);
        }
        for killed in state.killed.iter_mut() {
            *killed = true;
        }
        self.turn.notify_all();
    }

    /// A thread that cannot go on until another does (one asleep in a futex): the checker
    /// runs another runnable thread, any of them, and this is no preemption, which the bound
    /// counts only for a thread that could have continued. Returns at once if no other thread
    /// can run.
    fn yield_to_others(&self, me: usize) {
        if thread::panicking() {
            return;
        }
        let mut state = self.lock();
        if state.killed[me] {
            drop(state);
            panic::resume_unwind(Box::new(Killed));
        }
        state.steps += 1;
        if state.steps > self.bounds.max_steps {
            let message = std::format!(
                "an execution took more than {} steps: a thread may be waiting or livelocked",
                self.bounds.max_steps
            );
            self.fail(&mut state, message);
            drop(state);
            panic::resume_unwind(Box::new(Killed));
        }
        let others: Vec<usize> = state.runnable().into_iter().filter(|&t| t != me).collect();
        if others.is_empty() {
            return;
        }
        let taken = state.pick(others.len());
        state.running = others[taken];
        self.turn.notify_all();
        self.wait_turn(state, me);
    }

    fn switch(&self, me: usize) {
        if thread::panicking() {
            return;
        }
        let mut state = self.lock();
        if state.killed[me] {
            drop(state);
            panic::resume_unwind(Box::new(Killed));
        }
        state.steps += 1;
        if state.steps > self.bounds.max_steps {
            let message = std::format!(
                "an execution took more than {} steps: a thread may be waiting or livelocked",
                self.bounds.max_steps
            );
            self.fail(&mut state, message);
            drop(state);
            panic::resume_unwind(Box::new(Killed));
        }
        // Option 0 continues; then one per other runnable thread if a preemption is left; then
        // a fork if one is allowed and has not happened.
        let others: Vec<usize> = if state.preemptions < self.bounds.preemptions {
            state.runnable().into_iter().filter(|&t| t != me).collect()
        } else {
            Vec::new()
        };
        let fork = self.bounds.fork && !state.forked;
        let count = 1 + others.len() + usize::from(fork);
        let taken = state.pick(count);
        if taken == 0 {
            return;
        }
        if taken <= others.len() {
            state.preemptions += 1;
            state.running = others[taken - 1];
            self.turn.notify_all();
            self.wait_turn(state, me);
        } else {
            state.forked = true;
            for t in 0..state.killed.len() {
                if t != me && !state.finished[t] {
                    state.killed[t] = true;
                }
            }
            self.turn.notify_all();
            drop(state);
            (self.on_fork)();
        }
    }

    fn wait_turn(&self, mut state: MutexGuard<'_, State>, me: usize) {
        while state.running != me && !state.killed[me] {
            state = self.turn.wait(state).unwrap_or_else(|e| e.into_inner());
        }
        if state.killed[me] {
            drop(state);
            panic::resume_unwind(Box::new(Killed));
        }
    }

    /// Hands the turn to some runnable thread after `me` stops running for good.
    fn hand_off(&self, state: &mut State) {
        let runnable = state.runnable();
        if !runnable.is_empty() {
            let taken = state.pick(runnable.len());
            state.running = runnable[taken];
        }
        self.turn.notify_all();
    }

    fn thread(&self, me: usize, body: &(dyn Fn(usize) + Sync)) {
        let erased = (self as *const Run<'_>).cast::<Run<'static>>();
        CONTEXT.with(|c| c.set(Some((erased, me))));
        let outcome = panic::catch_unwind(AssertUnwindSafe(|| {
            self.wait_turn(self.lock(), me);
            body(me);
        }));
        CONTEXT.with(|c| c.set(None));
        let mut state = self.lock();
        state.finished[me] = true;
        if let Err(payload) = outcome
            && !payload.is::<Killed>()
        {
            let message = payload
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "a non-string panic".to_string());
            self.fail(&mut state, std::format!("thread {me} panicked: {message}"));
        }
        self.hand_off(&mut state);
    }
}

/// Runs `body` on `threads` threads under every schedule within `bounds`.
///
/// Each execution starts from `setup()`. After all threads have finished or been killed,
/// `after` runs on the calling thread: it continues the scenario single-threaded (in a forked
/// child, say) and checks the invariants, panicking on a violation. `on_fork` is what a fork
/// wipes. Panics with the schedule's choices if any execution fails.
pub(crate) fn explore<S: Sync>(
    bounds: Bounds,
    threads: usize,
    setup: impl Fn() -> S,
    body: impl Fn(&S, usize) + Sync,
    on_fork: impl Fn(&S) + Sync,
    after: impl Fn(&S, bool),
) -> Report {
    let mut prefix: Vec<Choice> = Vec::new();
    let mut executions = 0;
    let mut forks = 0;
    loop {
        let world = setup();
        let fork_world = || on_fork(&world);
        let run = Run {
            state: Mutex::new(State {
                running: usize::MAX,
                finished: std::vec![false; threads],
                killed: std::vec![false; threads],
                prefix: prefix.clone(),
                trail: Vec::new(),
                preemptions: 0,
                steps: 0,
                forked: false,
                failure: None,
            }),
            turn: Condvar::new(),
            bounds,
            on_fork: &fork_world,
        };
        {
            let mut state = run.lock();
            let first = state.pick(threads);
            state.running = first;
        }
        let each = |me: usize| body(&world, me);
        thread::scope(|scope| {
            for me in 0..threads {
                let run = &run;
                let each = &each;
                scope.spawn(move || run.thread(me, each));
            }
        });
        executions += 1;
        let state = run.state.into_inner().unwrap_or_else(|e| e.into_inner());
        let trail: Vec<(usize, usize)> = state.trail.iter().map(|c| (c.taken, c.count)).collect();
        if let Some(failure) = state.failure {
            panic!("execution {executions} failed: {failure}\nchoices (taken, of): {trail:?}");
        }
        if state.forked {
            forks += 1;
        }
        let checked = panic::catch_unwind(AssertUnwindSafe(|| after(&world, state.forked)));
        if let Err(payload) = checked {
            std::eprintln!(
                "execution {executions} broke an invariant; choices (taken, of): {trail:?}"
            );
            panic::resume_unwind(payload);
        }
        // Backtrack: the last choice with an option left takes its next one.
        let mut next = state.trail;
        loop {
            match next.last_mut() {
                None => {
                    return Report {
                        executions,
                        forks,
                        complete: true,
                    };
                }
                Some(choice) if choice.taken + 1 < choice.count => {
                    choice.taken += 1;
                    break;
                }
                Some(_) => {
                    next.pop();
                }
            }
        }
        prefix = next;
        if executions >= bounds.max_executions {
            return Report {
                executions,
                forks,
                complete: false,
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::front::sync::AtomicUsize;
    use core::sync::atomic::Ordering;
    use std::sync::atomic::AtomicBool;

    /// Two threads each load and then store an increment. With one preemption the checker must
    /// find the schedule that loses an update; with none it must not.
    #[test]
    fn a_lost_update_needs_one_preemption() {
        let lost = AtomicBool::new(false);
        let search = |preemptions| {
            explore(
                Bounds::preemptions(preemptions),
                2,
                || AtomicUsize::new(0),
                |counter, _| {
                    let seen = counter.load(Ordering::SeqCst);
                    counter.store(seen + 1, Ordering::SeqCst);
                },
                |_| {},
                |counter, _| {
                    if counter.load(Ordering::SeqCst) != 2 {
                        lost.store(true, std::sync::atomic::Ordering::SeqCst);
                    }
                },
            )
        };
        let report = search(0);
        assert!(report.complete);
        assert!(!lost.load(std::sync::atomic::Ordering::SeqCst));
        let report = search(1);
        assert!(report.complete && report.executions > 2);
        assert!(lost.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn choices_are_explored_and_a_fork_kills_the_others() {
        let seen = Mutex::new(Vec::new());
        let report = explore(
            Bounds::preemptions(0).with_fork(),
            2,
            || (AtomicUsize::new(0), AtomicUsize::new(0)),
            |(a, b), me| {
                let pick = choose(3);
                a.fetch_add(pick, Ordering::SeqCst);
                b.fetch_add(1 << (8 * me), Ordering::SeqCst);
            },
            |_| {},
            |(a, b), forked| {
                let b = b.load(Ordering::SeqCst);
                if !forked {
                    assert_eq!(b, 0x101);
                }
                seen.lock().unwrap().push(a.load(Ordering::SeqCst));
            },
        );
        assert!(report.complete && report.forks > 0);
        let seen = seen.into_inner().unwrap();
        assert!((0..=4).all(|sum| seen.contains(&sum)));
    }

    #[test]
    #[should_panic(expected = "thread 1 panicked: boom")]
    fn a_panic_in_a_thread_is_reported() {
        explore(
            Bounds::preemptions(1),
            2,
            || AtomicUsize::new(0),
            |counter, me| {
                if counter.fetch_add(1, Ordering::SeqCst) == 1 && me == 1 {
                    panic!("boom");
                }
            },
            |_| {},
            |_, _| {},
        );
    }
}
