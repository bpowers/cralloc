//! An exhaustive interleaving explorer for the back end's protocols: loom's job, for code that
//! may not depend on loom.
//!
//! Each test thread is a real thread, but only one runs at a time. A thread runs until it is
//! about to perform an operation on a shared word ([`TWord`]), then hands control to the
//! scheduler, which picks the next thread to run. Every operation on a shared word is therefore
//! one step, as in the TLA+ models, and the explorer enumerates the schedules by depth-first
//! search over those choices, replaying each schedule from a fresh state.
//!
//! The protocols use only sequentially consistent atomics on these words, and the rest of each
//! thread's state is private, so the interleavings of whole operations are exactly the
//! executions the memory model allows; there are no weaker behaviours to explore.
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
use std::boxed::Box;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::format;
use std::panic::{self, AssertUnwindSafe};
use std::string::String;
use std::sync::atomic::{AtomicU64, Ordering::SeqCst};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
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
}

impl Config {
    /// Every schedule, no crash.
    pub(crate) const ALL: Config = Config {
        preemptions: None,
        crash: false,
        limit: 2_000_000,
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
    /// Not yet at its first step.
    Starting,
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

struct Inner {
    states: Vec<Run>,
    started: usize,
    last: Option<usize>,
    prefix: Vec<usize>,
    trace: Vec<(usize, usize)>,
    preemptions: usize,
    config: Config,
    crashed: bool,
    failure: Option<String>,
    owners: BTreeMap<u64, usize>,
}

struct Sched {
    inner: Mutex<Inner>,
    turn: Condvar,
}

/// Unwinds a vanished thread; not a failure.
struct Killed;

std::thread_local! {
    static CONTEXT: RefCell<Option<(Arc<Sched>, usize)>> = const { RefCell::new(None) };
}

fn context() -> Option<(Arc<Sched>, usize)> {
    CONTEXT.with(|c| c.borrow().clone())
}

impl Sched {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Picks the next thread to run, or ends the schedule if none can.
    fn schedule(&self, inner: &mut Inner) {
        loop {
            if inner.failure.is_some() {
                // Unblock everyone so the schedule can be torn down.
                for state in inner.states.iter_mut() {
                    if *state == Run::Waiting {
                        *state = Run::Killed;
                    }
                }
                self.turn.notify_all();
                return;
            }
            let waiting: Vec<usize> = (0..inner.states.len())
                .filter(|&t| inner.states[t] == Run::Waiting)
                .collect();
            if waiting.is_empty() {
                self.turn.notify_all();
                return;
            }
            let mut options: Vec<Choice> = Vec::new();
            let last_can_run = inner.last.is_some_and(|l| waiting.contains(&l));
            let bounded = inner
                .config
                .preemptions
                .is_some_and(|b| inner.preemptions >= b);
            for &t in &waiting {
                if !(bounded && last_can_run && Some(t) != inner.last) {
                    options.push(Choice::Run(t));
                }
            }
            if inner.config.crash && !inner.crashed {
                options.extend(
                    waiting
                        .iter()
                        .filter(|&&t| t != 0)
                        .map(|&t| Choice::Kill(t)),
                );
            }
            let pos = inner.trace.len();
            let pick = inner.prefix.get(pos).copied().unwrap_or(0);
            inner.trace.push((options.len(), pick));
            match options[pick] {
                Choice::Run(t) => {
                    if last_can_run && Some(t) != inner.last {
                        inner.preemptions += 1;
                    }
                    inner.states[t] = Run::Running;
                    inner.last = Some(t);
                    self.turn.notify_all();
                    return;
                }
                Choice::Kill(t) => {
                    inner.states[t] = Run::Killed;
                    inner.crashed = true;
                    self.turn.notify_all();
                }
            }
        }
    }

    /// Thread `me`, which is running, is about to touch shared state: let the scheduler choose
    /// who goes next.
    fn switch(&self, me: usize) {
        let mut inner = self.lock();
        inner.states[me] = Run::Waiting;
        self.schedule(&mut inner);
        self.wait_turn(inner, me);
    }

    /// Waits until the scheduler picks thread `me`, or unwinds it if it has been killed.
    fn wait_turn(&self, mut inner: MutexGuard<'_, Inner>, me: usize) {
        loop {
            match inner.states[me] {
                Run::Running => return,
                Run::Killed => {
                    drop(inner);
                    panic::resume_unwind(Box::new(Killed));
                }
                _ => inner = self.turn.wait(inner).unwrap_or_else(|e| e.into_inner()),
            }
        }
    }

    /// Registers thread `me` at its start, and waits for its first turn. The schedule begins
    /// once every thread has registered.
    fn start(&self, me: usize) {
        let mut inner = self.lock();
        inner.states[me] = Run::Waiting;
        inner.started += 1;
        if inner.started == inner.states.len() {
            self.schedule(&mut inner);
        }
        self.wait_turn(inner, me);
    }

    fn finish(&self, me: usize, failure: Option<String>) {
        let mut inner = self.lock();
        if inner.states[me] != Run::Killed {
            inner.states[me] = Run::Done;
        }
        if let Some(failure) = failure {
            inner.failure.get_or_insert(failure);
        }
        if inner.states[me] == Run::Done || inner.failure.is_some() {
            self.schedule(&mut inner);
        }
    }
}

/// Called before every operation on a shared word.
pub(crate) fn step() {
    if let Some((sched, me)) = context() {
        sched.switch(me);
    }
}

/// Records that the running thread now holds heap-wide slots `first + b` for each `b` in
/// `bits`.
fn claimed(first: u64, bits: u64) {
    let Some((sched, me)) = context() else { return };
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
}

fn released(first: u64, bits: u64) {
    let Some((sched, _)) = context() else { return };
    let mut inner = sched.lock();
    let mut rest = bits;
    while rest != 0 {
        inner.owners.remove(&(first + rest.trailing_zeros() as u64));
        rest &= rest - 1;
    }
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
        let Some((sched, me)) = context() else { return };
        let mut inner = sched.lock();
        for slot in first..=last {
            if inner.owners.get(&slot) != Some(&me) {
                let held = inner.owners.get(&slot).copied();
                inner.failure.get_or_insert(format!(
                    "thread {me} purges slot {slot}, which it does not hold (held by {held:?})"
                ));
            }
        }
    }
}

/// How many schedules were explored.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Stats {
    pub(crate) schedules: usize,
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
    let mut prefix: Vec<usize> = Vec::new();
    let mut schedules = 0;
    loop {
        schedules += 1;
        assert!(
            schedules <= config.limit,
            "more than {} schedules",
            config.limit
        );
        let state = setup();
        let sched = Arc::new(Sched {
            inner: Mutex::new(Inner {
                states: std::vec![Run::Starting; threads],
                started: 0,
                last: None,
                prefix: prefix.clone(),
                trace: Vec::new(),
                preemptions: 0,
                config,
                crashed: false,
                failure: None,
                owners: BTreeMap::new(),
            }),
            turn: Condvar::new(),
        });
        thread::scope(|scope| {
            for me in 0..threads {
                let sched = Arc::clone(&sched);
                let state = &state;
                let body = &body;
                scope.spawn(move || {
                    CONTEXT.with(|c| *c.borrow_mut() = Some((Arc::clone(&sched), me)));
                    let result = panic::catch_unwind(AssertUnwindSafe(|| {
                        sched.start(me);
                        body(state, me)
                    }));
                    let failure = match result {
                        Ok(()) => None,
                        Err(payload) if payload.is::<Killed>() => None,
                        Err(payload) => Some(describe(&payload)),
                    };
                    sched.finish(me, failure);
                    CONTEXT.with(|c| *c.borrow_mut() = None);
                });
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
            if std::env::var_os("EXPLORE_STATS").is_some() {
                std::eprintln!("explored {schedules} schedules");
            }
            return Stats { schedules };
        };
        prefix = trace[..depth].iter().map(|&(_, pick)| pick).collect();
        prefix.push(trace[depth].1 + 1);
    }
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
