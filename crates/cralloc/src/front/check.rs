//! An exhaustive interleaving checker for the lock-free protocols. Test code only.
//!
//! loom would do this job, but the allocator crate takes no dependencies, so this is a small
//! checker in the style of CHESS. A scenario runs a few threads ([`Team`]) that take turns: exactly
//! one runs at a time, and before every atomic operation in the front end and the depot (the
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
//! for good (it is "killed", and unwound out of its body before the forking thread goes on), and
//! a callback wipes what the child would see wiped. What the killed threads held privately is
//! lost, as in a forked child.

use crate::fingerprint::{self, Fingerprint};
use crate::turns::{Team, with_team};
use std::boxed::Box;
use std::cell::Cell;
use std::panic::{self, AssertUnwindSafe};
use std::string::{String, ToString};
use std::sync::{Mutex, MutexGuard};
use std::thread;
use std::time::Instant;
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
    /// Every decision of every execution, in order ([`crate::fingerprint`]).
    pub(crate) fingerprint: Fingerprint,
}

struct State {
    running: usize,
    finished: Vec<bool>,
    killed: Vec<bool>,
    prefix: Vec<Choice>,
    trail: Vec<Choice>,
    /// Every decision of this execution, the forced ones too: who was running, how many options
    /// it had, which it took.
    fingerprint: Fingerprint,
    preemptions: usize,
    steps: usize,
    forked: bool,
    failure: Option<String>,
    /// While a thread is being unwound: who is waiting for that to finish.
    killer: Option<usize>,
}

struct Run<'a> {
    state: Mutex<State>,
    team: &'a Team,
    bounds: Bounds,
    on_fork: &'a (dyn Fn() + Sync),
}

std::thread_local! {
    /// The run the running task takes part in, and its index in it. Set only while a task runs a
    /// scenario body, and set again each time a task gets the turn back, since tasks may share
    /// an OS thread.
    static CONTEXT: Cell<Option<(*const Run<'static>, usize)>> = const { Cell::new(None) };
}

/// Runs `f` with the running task's run, if there is one.
#[inline]
fn with_run<R>(f: impl FnOnce(&Run<'_>, usize) -> R) -> Option<R> {
    let (run, me) = CONTEXT.with(Cell::get)?;
    // SAFETY: `CONTEXT` is set only by `Run::set_context`, on a task for as long as it runs a
    // scenario body, and the `Run` it points to outlives every task's job in its round.
    Some(f(unsafe { &*run }, me))
}

/// A point at which the checker may switch threads, or, on a thread inside
/// [`interrupt_at`], at which the interrupt may fire. Does nothing otherwise.
#[inline]
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
    /// Whether `INTERRUPT` holds one: what most switch points look at, outside a run, since a
    /// census makes thousands of them per execution.
    static ARMED: Cell<bool> = const { Cell::new(false) };
}

#[inline]
fn interrupt_point() {
    if ARMED.with(Cell::get) {
        fire_interrupt();
    }
}

fn fire_interrupt() {
    let due = INTERRUPT.with(|slot| {
        let mut slot = slot.borrow_mut();
        let interrupt = slot.as_mut()?;
        if interrupt.remaining > 0 {
            interrupt.remaining -= 1;
            return None;
        }
        ARMED.with(|armed| armed.set(false));
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
        ARMED.with(|armed| armed.set(false));
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
    ARMED.with(|armed| armed.set(true));
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
        let taken = self.decide(count);
        self.fingerprint.mix(self.running as u64);
        self.fingerprint.mix(count as u64);
        self.fingerprint.mix(taken as u64);
        taken
    }

    fn decide(&mut self, count: usize) -> usize {
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

    fn runnable(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.finished.len()).filter(|&t| !self.finished[t] && !self.killed[t])
    }
}

impl Run<'_> {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn set_context(&self, me: usize) {
        let erased = (self as *const Run<'_>).cast::<Run<'static>>();
        CONTEXT.with(|c| c.set(Some((erased, me))));
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
    }

    /// Counts a step of the running thread `me`, and locks the state for what comes next. Unwinds
    /// the thread if it has been killed or has taken too many steps.
    fn step(&self, me: usize) -> MutexGuard<'_, State> {
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
        state
    }

    /// A thread that cannot go on until another does (one asleep in a futex): the checker
    /// runs another runnable thread, any of them, and this is no preemption, which the bound
    /// counts only for a thread that could have continued. Returns at once if no other thread
    /// can run.
    fn yield_to_others(&self, me: usize) {
        if thread::panicking() {
            return;
        }
        let mut state = self.step(me);
        let others = state.runnable().filter(|&t| t != me).count();
        if others == 0 {
            return;
        }
        let taken = state.pick(others);
        let next = state.runnable().filter(|&t| t != me).nth(taken);
        self.hand_to(state, me, next.expect("a recorded choice is an option"));
    }

    fn switch(&self, me: usize) {
        if thread::panicking() {
            return;
        }
        let mut state = self.step(me);
        // Option 0 continues; then one per other runnable thread if a preemption is left; then
        // a fork if one is allowed and has not happened.
        let others = if state.preemptions < self.bounds.preemptions {
            state.runnable().filter(|&t| t != me).count()
        } else {
            0
        };
        let fork = self.bounds.fork && !state.forked;
        let count = 1 + others + usize::from(fork);
        let taken = state.pick(count);
        if taken == 0 {
            return;
        }
        if taken <= others {
            state.preemptions += 1;
            let next = state.runnable().filter(|&t| t != me).nth(taken - 1);
            self.hand_to(state, me, next.expect("a recorded choice is an option"));
        } else {
            state.forked = true;
            let mut victims = Vec::new();
            for t in 0..state.killed.len() {
                if t != me && !state.finished[t] {
                    state.killed[t] = true;
                    victims.push(t);
                }
            }
            drop(state);
            self.unwind(me, victims);
            (self.on_fork)();
        }
    }

    /// Gives the turn to `next` and waits for it to come back. Unwinds if it comes back because
    /// this thread has been killed.
    fn hand_to(&self, mut state: MutexGuard<'_, State>, me: usize, next: usize) {
        state.running = next;
        drop(state);
        self.team.pass(me, next);
        self.set_context(me);
        self.unwind_if_killed(me);
    }

    fn unwind_if_killed(&self, me: usize) {
        if self.lock().killed[me] {
            panic::resume_unwind(Box::new(Killed));
        }
    }

    /// Unwinds each of `victims`, which have been marked killed, and comes back to `me`, which
    /// is not one of them.
    fn unwind(&self, me: usize, victims: Vec<usize>) {
        self.lock().killer = Some(me);
        for t in victims {
            self.team.pass(me, t);
            self.set_context(me);
        }
        self.lock().killer = None;
    }

    /// A whole thread, as a task's job. Returns who goes next.
    fn thread(&self, me: usize, body: &(dyn Fn(usize) + Sync)) -> usize {
        self.set_context(me);
        let outcome = panic::catch_unwind(AssertUnwindSafe(|| {
            self.unwind_if_killed(me);
            body(me);
        }));
        let next = self.end(me, outcome);
        CONTEXT.with(|c| c.set(None));
        next
    }

    /// What thread `me` does when its body has returned or unwound. Returns who goes next.
    fn end(&self, me: usize, outcome: std::thread::Result<()>) -> usize {
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
        // A thread being unwound goes back to the one that killed it. A failure ends the
        // execution, so the thread that failed unwinds the others.
        if let Some(killer) = state.killer {
            return killer;
        }
        if state.failure.is_some() {
            let victims = (0..state.finished.len())
                .filter(|&t| !state.finished[t])
                .collect();
            drop(state);
            self.unwind(me, victims);
            return self.team.driver();
        }
        let runnable = state.runnable().count();
        if runnable == 0 {
            return self.team.driver();
        }
        let taken = state.pick(runnable);
        let next = state.runnable().nth(taken);
        state.running = next.expect("a recorded choice is an option");
        state.running
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
    let began = Instant::now();
    let mut fingerprint = Fingerprint::new();
    let mut prefix: Vec<Choice> = Vec::new();
    let mut executions = 0;
    let mut forks = 0;
    with_team(threads, |team| {
        loop {
            let world = setup();
            let fork_world = || on_fork(&world);
            let run = Run {
                state: Mutex::new(State {
                    running: usize::MAX,
                    finished: std::vec![false; threads],
                    killed: std::vec![false; threads],
                    prefix: core::mem::take(&mut prefix),
                    trail: Vec::new(),
                    fingerprint: Fingerprint::new(),
                    preemptions: 0,
                    steps: 0,
                    forked: false,
                    failure: None,
                    killer: None,
                }),
                team,
                bounds,
                on_fork: &fork_world,
            };
            let first = {
                let mut state = run.lock();
                let first = state.pick(threads);
                state.running = first;
                first
            };
            let each = |me: usize| body(&world, me);
            let job = |me: usize| run.thread(me, &each);
            team.round(&job, || team.pass(team.driver(), first));
            executions += 1;
            let state = run.state.into_inner().unwrap_or_else(|e| e.into_inner());
            let trail: Vec<(usize, usize)> =
                state.trail.iter().map(|c| (c.taken, c.count)).collect();
            if let Some(failure) = state.failure {
                panic!("execution {executions} failed: {failure}\nchoices (taken, of): {trail:?}");
            }
            if state.forked {
                forks += 1;
            }
            fingerprint.mix(state.fingerprint.0);
            fingerprint.mix(u64::from(state.forked));
            fingerprint.mix(state.steps as u64);
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
                        fingerprint::log(began, executions, fingerprint);
                        return Report {
                            executions,
                            forks,
                            complete: true,
                            fingerprint,
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
                fingerprint::log(began, executions, fingerprint);
                return Report {
                    executions,
                    forks,
                    complete: false,
                    fingerprint,
                };
            }
        }
    })
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
    fn the_same_search_has_the_same_fingerprint() {
        let search = |preemptions| {
            explore(
                Bounds::preemptions(preemptions).with_fork(),
                3,
                || AtomicUsize::new(0),
                |counter, _| {
                    counter.fetch_add(1, Ordering::SeqCst);
                    counter.fetch_add(1, Ordering::SeqCst);
                },
                |_| {},
                |_, _| {},
            )
            .fingerprint
        };
        assert_eq!(search(1), search(1));
        assert_ne!(search(1), search(0));
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
