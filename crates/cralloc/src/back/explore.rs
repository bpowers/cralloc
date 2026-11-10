//! An exhaustive interleaving explorer for the back end's protocols: loom's job, for code that
//! may not depend on loom.
//!
//! Each test thread is a task of a [`Team`], and only one runs at a time. A thread runs until it is
//! about to perform an operation on a shared word ([`TWord`]), then hands control to the
//! scheduler, which picks the next thread to run. Every operation on a shared word is therefore
//! one step, as in the TLA+ models, and the explorer enumerates the schedules by depth-first
//! search over those choices, replaying each schedule from a fresh state.
//!
//! The protocols use only sequentially consistent atomics on these words, but for `Word::store`,
//! and the rest of each thread's state is private, so the interleavings of whole operations are
//! exactly the executions the memory model allows; there are no weaker behaviours to explore.
//! The heap's `store` is a release store, used only on words no protocol races on (see
//! `Word`); the explorer's is sequentially consistent, which for those words allows the same
//! executions.
//!
//! Two options cut or widen the search. A preemption bound limits how many times the scheduler
//! may switch away from a thread that could continue, which keeps three-thread scenarios
//! tractable (the usual bound of two finds almost every bug in practice). With `crash`, the
//! scheduler may also, once per schedule, make any thread but thread 0 vanish at a step, as a
//! fork does to every thread but the one that forks.
//!
//! Each `TWord` that is a leaf bitmap word also keeps a ghost record of which thread holds each
//! slot it claimed, so a test can check that a purge holds every slot it returns.

use super::testhost::TestWord;
use super::word::Word;
use crate::fingerprint::{self, Fingerprint};
use crate::turns::{Team, with_team};
use std::boxed::Box;
use std::cell::Cell;
use std::collections::BTreeMap;
use std::format;
use std::panic::{self, AssertUnwindSafe};
use std::string::String;
use std::sync::atomic::{AtomicU64, Ordering::SeqCst};
use std::sync::{Mutex, MutexGuard};
use std::time::Instant;
use std::vec::Vec;

/// How to explore.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Config {
    /// At most this many preemptions per schedule, or every schedule.
    pub(crate) preemptions: Option<usize>,
    /// Let one thread other than thread 0 vanish at any step.
    pub(crate) crash: bool,
    /// Give up after this many schedules: a guard against a scenario that is too large.
    pub(crate) limit: usize,
    /// Decisions in one schedule before it is reported as not terminating.
    pub(crate) max_steps: usize,
}

impl Config {
    /// Every schedule, no crash.
    pub(crate) const ALL: Config = Config {
        preemptions: None,
        crash: false,
        limit: 2_000_000,
        max_steps: 20_000,
    };
}

/// What happened in one schedule, for the final check.
#[derive(Clone, Debug, Default)]
pub(crate) struct Outcome {
    /// Threads that vanished.
    pub(crate) killed: Vec<usize>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Run {
    Waiting,
    Running,
    Done,
    Killed,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Choice {
    Run(usize),
    Kill(usize),
}

/// More threads than any scenario has, for a list of options on the stack.
const MOST_THREADS: usize = 8;

struct Inner {
    states: Vec<Run>,
    last: Option<usize>,
    prefix: Vec<usize>,
    trace: Vec<(usize, usize)>,
    /// Every decision of this schedule: how many options, which was taken, what it was.
    fingerprint: Fingerprint,
    preemptions: usize,
    config: Config,
    crashed: bool,
    failure: Option<String>,
    owners: BTreeMap<u64, usize>,
    /// While a thread is being unwound: who goes on when it has been.
    then: Option<usize>,
}

struct Sched<'a> {
    inner: Mutex<Inner>,
    team: &'a Team,
}

/// Unwinds a vanished thread; not a failure.
struct Killed;

std::thread_local! {
    /// The schedule the running task takes part in, and its index in it. Set only while a task
    /// runs a scenario body, and set again each time a task gets the turn back, since tasks may
    /// share an OS thread.
    static CONTEXT: Cell<Option<(*const Sched<'static>, usize)>> = const { Cell::new(None) };
}

/// Runs `f` with the running task's schedule, if there is one.
#[inline]
fn with_sched<R>(f: impl FnOnce(&Sched<'_>, usize) -> R) -> Option<R> {
    let (sched, me) = CONTEXT.with(Cell::get)?;
    // SAFETY: `CONTEXT` is set only by `Sched::set_context`, on a task for as long as it runs a
    // scenario body, and the `Sched` it points to outlives every task's job in its round.
    Some(f(unsafe { &*sched }, me))
}

impl Sched<'_> {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn set_context(&self, me: usize) {
        let erased = (self as *const Sched<'_>).cast::<Sched<'static>>();
        CONTEXT.with(|c| c.set(Some((erased, me))));
    }

    /// Gives the turn to `to` and takes it back when it comes.
    fn pass(&self, me: usize, to: usize) {
        self.team.pass(me, to);
        if me != self.team.driver() {
            self.set_context(me);
        }
    }

    /// Unwinds thread `t`, which has just been marked killed, and comes back. `me` is running
    /// the scheduler, as a thread or as the driver.
    fn unwind(&self, me: usize, t: usize) {
        self.lock().then = Some(me);
        self.pass(me, t);
    }

    /// Picks the next thread to run, which may be `me`, or the driver if none can. A schedule that
    /// has failed goes on no further: every waiting thread is killed, and is unwound here but for
    /// `me`, which is told so by getting the turn.
    fn schedule<'a>(&'a self, mut inner: MutexGuard<'a, Inner>, me: usize) -> usize {
        loop {
            if inner.failure.is_some() {
                let mut own = false;
                let mut waiting = Vec::new();
                for t in 0..inner.states.len() {
                    if inner.states[t] == Run::Waiting {
                        inner.states[t] = Run::Killed;
                        if t == me {
                            own = true;
                        } else {
                            waiting.push(t);
                        }
                    }
                }
                drop(inner);
                for t in waiting {
                    self.unwind(me, t);
                }
                return if own { me } else { self.team.driver() };
            }
            if inner.trace.len() >= inner.config.max_steps {
                let message = format!(
                    "a schedule took more than {} steps: a thread may be waiting or livelocked",
                    inner.config.max_steps
                );
                inner.failure = Some(message);
                continue;
            }
            // The options, in order: run each waiting thread, the bound allowing, then kill each
            // waiting thread but thread 0, if a crash is allowed and has not happened. None means
            // no thread is waiting, since the bound never rules out the last thread to run.
            let threads = inner.states.len();
            let last_can_run = inner.last.is_some_and(|l| inner.states[l] == Run::Waiting);
            let bounded = inner
                .config
                .preemptions
                .is_some_and(|b| inner.preemptions >= b);
            let mut options = [Choice::Run(0); 2 * MOST_THREADS];
            let mut count = 0;
            for t in 0..threads {
                let ruled_out = bounded && last_can_run && Some(t) != inner.last;
                if inner.states[t] == Run::Waiting && !ruled_out {
                    options[count] = Choice::Run(t);
                    count += 1;
                }
            }
            if inner.config.crash && !inner.crashed {
                for t in 1..threads {
                    if inner.states[t] == Run::Waiting {
                        options[count] = Choice::Kill(t);
                        count += 1;
                    }
                }
            }
            if count == 0 {
                return self.team.driver();
            }
            let pos = inner.trace.len();
            let pick = inner.prefix.get(pos).copied().unwrap_or(0);
            let choice = options[..count][pick];
            inner.trace.push((count, pick));
            inner.fingerprint.mix(count as u64);
            inner.fingerprint.mix(pick as u64);
            inner.fingerprint.mix(match choice {
                Choice::Run(t) => (t as u64) << 1,
                Choice::Kill(t) => (t as u64) << 1 | 1,
            });
            match choice {
                Choice::Run(t) => {
                    if last_can_run && Some(t) != inner.last {
                        inner.preemptions += 1;
                    }
                    inner.states[t] = Run::Running;
                    inner.last = Some(t);
                    return t;
                }
                Choice::Kill(t) => {
                    inner.states[t] = Run::Killed;
                    inner.crashed = true;
                    // A thread that kills itself unwinds once the choice of who goes next is
                    // made (`switch`).
                    if t != me {
                        drop(inner);
                        self.unwind(me, t);
                        inner = self.lock();
                    }
                }
            }
        }
    }

    /// Thread `me`, which is running, is about to touch shared state: let the scheduler choose
    /// who goes next.
    fn switch(&self, me: usize) {
        let mut inner = self.lock();
        inner.states[me] = Run::Waiting;
        let next = self.schedule(inner, me);
        if next != me {
            let mut inner = self.lock();
            if inner.states[me] == Run::Killed {
                inner.then = Some(next);
                drop(inner);
                panic::resume_unwind(Box::new(Killed));
            }
            drop(inner);
            self.pass(me, next);
        }
        self.unwind_if_killed(me);
    }

    fn unwind_if_killed(&self, me: usize) {
        if self.lock().states[me] == Run::Killed {
            panic::resume_unwind(Box::new(Killed));
        }
    }

    /// What thread `me` does when its body has returned or unwound. Returns who goes next.
    fn finish(&self, me: usize, failure: Option<String>) -> usize {
        let mut inner = self.lock();
        if inner.states[me] != Run::Killed {
            inner.states[me] = Run::Done;
        } else if let Some(then) = inner.then.take() {
            return then;
        }
        if let Some(failure) = failure {
            inner.failure.get_or_insert(failure);
        }
        self.schedule(inner, me)
    }
}

/// Called before every operation on a shared word.
#[inline]
pub(crate) fn step() {
    let _ = with_sched(|sched, me| sched.switch(me));
}

/// Records that the running thread now holds heap-wide slots `first + b` for each `b` in
/// `bits`.
fn claimed(first: u64, bits: u64) {
    let _ = with_sched(|sched, me| {
        let mut inner = sched.lock();
        let mut rest = bits;
        while rest != 0 {
            let slot = first + rest.trailing_zeros() as u64;
            rest &= rest - 1;
            if let Some(other) = inner.owners.insert(slot, me) {
                inner
                    .failure
                    .get_or_insert(format!("slot {slot} claimed by {me} while held by {other}"));
            }
        }
    });
}

fn released(first: u64, bits: u64) {
    let _ = with_sched(|sched, _| {
        let mut inner = sched.lock();
        let mut rest = bits;
        while rest != 0 {
            inner.owners.remove(&(first + rest.trailing_zeros() as u64));
            rest &= rest - 1;
        }
    });
}

/// A shared word whose every operation is a scheduling point.
pub(crate) struct TWord {
    value: AtomicU64,
    leaf: Option<u64>,
}

impl Word for TWord {
    fn load(&self) -> u64 {
        step();
        self.value.load(SeqCst)
    }

    fn fetch_or(&self, bits: u64) -> u64 {
        step();
        let old = self.value.fetch_or(bits, SeqCst);
        if let Some(first) = self.leaf {
            released(first, bits & !old);
        }
        old
    }

    fn fetch_and(&self, bits: u64) -> u64 {
        step();
        let old = self.value.fetch_and(bits, SeqCst);
        if let Some(first) = self.leaf {
            claimed(first, old & !bits);
        }
        old
    }

    fn compare_exchange(&self, current: u64, new: u64) -> Result<u64, u64> {
        step();
        let result = self.value.compare_exchange(current, new, SeqCst, SeqCst);
        if let (Ok(old), Some(first)) = (result, self.leaf) {
            claimed(first, old & !new);
            released(first, new & !old);
        }
        result
    }

    fn store(&self, value: u64) {
        step();
        self.value.store(value, SeqCst);
    }
}

impl TestWord for TWord {
    fn new(leaf: Option<u64>) -> Self {
        TWord {
            value: AtomicU64::new(0),
            leaf,
        }
    }

    fn purging(first: u64, last: u64) {
        let _ = with_sched(|sched, me| {
            let mut inner = sched.lock();
            for slot in first..=last {
                if inner.owners.get(&slot) != Some(&me) {
                    let held = inner.owners.get(&slot).copied();
                    inner.failure.get_or_insert(format!(
                        "thread {me} purges slot {slot}, which it does not hold (held by {held:?})"
                    ));
                }
            }
        });
    }
}

/// How many schedules were explored.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Stats {
    pub(crate) schedules: usize,
    /// Every decision of every schedule, in order ([`crate::fingerprint`]).
    pub(crate) fingerprint: Fingerprint,
}

/// Runs `body(state, thread)` on `threads` threads under every schedule `config` allows, each
/// time on a fresh `setup()`, and calls `check(state, outcome)` after each. Panics with the
/// failing schedule if a thread panics, the ghost records catch a double claim or an unheld
/// purge, or `check` panics.
pub(crate) fn explore<S: Sync>(
    config: Config,
    threads: usize,
    setup: impl Fn() -> S,
    body: impl Fn(&S, usize) + Sync,
    check: impl Fn(&S, &Outcome),
) -> Stats {
    let began = Instant::now();
    let mut fingerprint = Fingerprint::new();
    let mut prefix: Vec<usize> = Vec::new();
    let mut schedules = 0;
    with_team(threads, |team| {
        loop {
            schedules += 1;
            assert!(
                schedules <= config.limit,
                "more than {} schedules",
                config.limit
            );
            let state = setup();
            let sched = Sched {
                inner: Mutex::new(Inner {
                    states: std::vec![Run::Waiting; threads],
                    last: None,
                    prefix: core::mem::take(&mut prefix),
                    trace: Vec::new(),
                    fingerprint: Fingerprint::new(),
                    preemptions: 0,
                    config,
                    crashed: false,
                    failure: None,
                    owners: BTreeMap::new(),
                    then: None,
                }),
                team,
            };
            let job = |me: usize| {
                sched.set_context(me);
                let result = panic::catch_unwind(AssertUnwindSafe(|| {
                    sched.unwind_if_killed(me);
                    body(&state, me)
                }));
                let failure = match result {
                    Ok(()) => None,
                    Err(payload) if payload.is::<Killed>() => None,
                    Err(payload) => Some(describe(&payload)),
                };
                let next = sched.finish(me, failure);
                CONTEXT.with(|c| c.set(None));
                next
            };
            team.round(&job, || {
                let driver = team.driver();
                let first = sched.schedule(sched.lock(), driver);
                if first != driver {
                    sched.pass(driver, first);
                }
            });
            let inner = sched.lock();
            if let Some(failure) = &inner.failure {
                panic!(
                    "schedule {schedules} failed: {failure}\nchoices: {:?}",
                    inner.trace
                );
            }
            let outcome = Outcome {
                killed: (0..threads)
                    .filter(|&t| inner.states[t] == Run::Killed)
                    .collect(),
            };
            let trace = inner.trace.clone();
            fingerprint.mix(inner.fingerprint.0);
            drop(inner);
            let checked = panic::catch_unwind(AssertUnwindSafe(|| check(&state, &outcome)));
            if let Err(payload) = checked {
                panic!(
                    "schedule {schedules} failed its check: {}\nchoices: {trace:?}",
                    describe(&payload)
                );
            }
            // The next schedule: the deepest choice with an untried option, advanced.
            let Some(depth) = trace.iter().rposition(|&(n, pick)| pick + 1 < n) else {
                fingerprint::log(began, schedules, fingerprint);
                return Stats {
                    schedules,
                    fingerprint,
                };
            };
            prefix = trace[..depth].iter().map(|&(_, pick)| pick).collect();
            prefix.push(trace[depth].1 + 1);
        }
    })
}

fn describe(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        String::from(*s)
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        String::from("a panic")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize};

    /// Two threads each load and then store an increment, under `preemptions`. Returns what was
    /// explored and whether some schedule lost an update.
    fn increments(preemptions: Option<usize>) -> (Stats, bool) {
        let lost = AtomicBool::new(false);
        let stats = explore(
            Config {
                preemptions,
                ..Config::ALL
            },
            2,
            || TWord::new(None),
            |counter, _| {
                let seen = counter.load();
                counter.store(seen + 1);
            },
            |counter, _| {
                if counter.load() != 2 {
                    lost.store(true, SeqCst);
                }
            },
        );
        (stats, lost.load(SeqCst))
    }

    #[test]
    fn a_lost_update_needs_one_preemption() {
        let (none, lost) = increments(Some(0));
        assert!(!lost);
        let (one, lost) = increments(Some(1));
        assert!(lost && one.schedules > none.schedules);
        // A thread runs in three stretches, to its load, to its store and to its end, and three
        // stretches can interleave with three others in twenty ways.
        let (every, _) = increments(None);
        assert_eq!(every.schedules, 20);
    }

    /// One small search, pinned, so that a change to the explorer's decisions fails a test: 3
    /// threads on one plain word, nothing that varies with the machine or the build, one preemption
    /// and a crash, and a load whose value decides what the thread does next. A deliberate change
    /// to the order of the choices may update the hash. The count needs a reason: it says how many
    /// schedules the search covers.
    #[test]
    fn a_small_search_makes_the_same_decisions() {
        let stats = explore(
            Config {
                preemptions: Some(1),
                crash: true,
                ..Config::ALL
            },
            3,
            || TWord::new(None),
            |word, me| {
                if word.load() == 0 {
                    word.fetch_or(1 << me);
                }
                word.fetch_and(!8);
            },
            |_, _| {},
        );
        assert_eq!(stats.schedules, 498);
        assert_eq!(stats.fingerprint, Fingerprint(0x7d0d63a387b6d31b));
    }

    /// A thread that never stops ends in a failure that names its schedule, not at the memory cap.
    #[test]
    #[should_panic(expected = "a schedule took more than 100 steps")]
    fn a_thread_that_never_stops_is_reported() {
        explore(
            Config {
                max_steps: 100,
                ..Config::ALL
            },
            1,
            || TWord::new(None),
            |word, _| while word.load() == 0 {},
            |_, _| {},
        );
    }

    #[test]
    fn the_same_search_has_the_same_fingerprint() {
        assert_eq!(
            increments(Some(1)).0.fingerprint,
            increments(Some(1)).0.fingerprint
        );
        assert_ne!(
            increments(Some(1)).0.fingerprint,
            increments(Some(0)).0.fingerprint
        );
    }

    /// A thread that vanishes is unwound before the check, whichever thread was running the
    /// scheduler when it was chosen, and does not do the operation it stopped before. Thread 0
    /// never vanishes.
    #[test]
    fn a_thread_that_vanishes_is_unwound_before_the_check() {
        struct Leaving<'a>(&'a AtomicUsize);
        impl Drop for Leaving<'_> {
            fn drop(&mut self) {
                self.0.fetch_add(1, SeqCst);
            }
        }
        let vanished = AtomicUsize::new(0);
        let cut_short = AtomicBool::new(false);
        let stats = explore(
            Config {
                crash: true,
                ..Config::ALL
            },
            2,
            || (TWord::new(None), AtomicUsize::new(0), AtomicUsize::new(0)),
            |(word, started, left), me| {
                started.fetch_add(1, SeqCst);
                let _leaving = Leaving(left);
                word.fetch_or(1 << (2 * me));
                word.fetch_or(2 << (2 * me));
            },
            |(word, started, left), outcome| {
                assert_eq!(started.load(SeqCst), left.load(SeqCst));
                assert!(!outcome.killed.contains(&0));
                assert!(outcome.killed.len() <= 1, "one crash a schedule");
                vanished.fetch_add(outcome.killed.len(), SeqCst);
                // Thread 1 got as far as its first operation, and vanished before it.
                if !outcome.killed.is_empty() && started.load(SeqCst) == 2 && word.load() & 4 == 0 {
                    cut_short.store(true, SeqCst);
                }
            },
        );
        assert!(vanished.load(SeqCst) > 0 && vanished.load(SeqCst) < stats.schedules);
        assert!(
            cut_short.load(SeqCst),
            "a thread did the operation it vanished before"
        );
    }

    #[test]
    #[should_panic(expected = "failed: boom")]
    fn a_panic_in_a_thread_is_reported() {
        explore(
            Config::ALL,
            3,
            || TWord::new(None),
            |word, me| {
                if word.fetch_or(1) == 1 && me == 2 {
                    panic!("boom");
                }
                word.fetch_or(2);
            },
            |_, _| {},
        );
    }

    /// The ghost record fails a schedule in which a slot is claimed twice, with the other threads
    /// suspended, and the schedule is reported with its choices.
    #[test]
    #[should_panic(expected = "slot 0 claimed by 1 while held by 0")]
    fn a_double_claim_is_reported() {
        explore(
            Config::ALL,
            3,
            || {
                let word = TWord::new(Some(0));
                word.store(1);
                word
            },
            |word, me| match me {
                0 => {
                    word.fetch_and(!1);
                }
                1 => {
                    word.store(1);
                    word.fetch_and(!1);
                }
                _ => {
                    word.load();
                    word.load();
                }
            },
            |_, _| {},
        );
    }
}
