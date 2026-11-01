//! Running streams against a subject.
//!
//! - [`run`]: seeded streams on one or more workers, each on its own thread, with blocks handed
//!   between them in a [`Handoff`] pattern, threads replaced mid-stream ([`Op::Migrate`]), and
//!   optionally a `fork` while the others are running ([`ForkPlan`]).
//! - [`run_bytes`]: one worker whose decisions come from a fuzzer's bytes.
//! - [`replay`] and [`minimize`]: a recorded script, exactly, and the shortest part of it that
//!   still fails the same way.
//!
//! A single-worker run records its whole stream, so its failure carries a script that replays
//! exactly. A multithreaded run is reproducible only up to thread interleaving: its failure
//! carries the seed and the failing worker's last operations.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Duration;

use crate::entropy::{AnyEntropy, ByteSource, Rng, mix64};
use crate::failure::{Failure, FailureKind, Trace};
use crate::generator::{Exchange, Generator, Target};
pub use crate::mail::Handoff;
use crate::mail::Mailroom;
use crate::model::{Effect, Model, Stats};
use crate::op::{Op, Script};
use crate::profile::Profile;
use crate::shadow::Shadow;
use crate::subject::{Subject, ZeroSize};

/// A seeded run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    /// Shape of every worker's stream.
    pub profile: Profile,
    /// Seed of the whole run; each worker's stream is derived from it.
    pub seed: u64,
    /// Operations per worker.
    pub ops: usize,
    /// Workers, each on its own thread.
    pub workers: usize,
    /// How blocks move between workers.
    pub handoff: Handoff,
    /// Fork during the run.
    pub fork: Option<ForkPlan>,
}

/// When and how to fork during a run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ForkPlan {
    /// Worker 0 forks before its operation with this index, while the others run.
    pub at: usize,
    /// Workers the child runs (worker 0 of the child inherits the forking worker's blocks).
    pub child_workers: usize,
    /// Operations per child worker.
    pub child_ops: usize,
    /// The child is killed and reported as hung after this long.
    pub timeout: Duration,
}

impl Config {
    /// One worker.
    pub const fn single(profile: Profile, seed: u64, ops: usize) -> Self {
        Config {
            profile,
            seed,
            ops,
            workers: 1,
            handoff: Handoff::None,
            fork: None,
        }
    }

    /// `workers` workers handing blocks around as `handoff` says.
    pub const fn threads(
        profile: Profile,
        workers: usize,
        handoff: Handoff,
        seed: u64,
        ops: usize,
    ) -> Self {
        Config {
            profile,
            seed,
            ops,
            workers,
            handoff,
            fork: None,
        }
    }

    /// This run with a fork.
    pub const fn with_fork(self, plan: ForkPlan) -> Self {
        Config {
            fork: Some(plan),
            ..self
        }
    }
}

/// Block names of a child of `fork` start here, above any name in the parent.
const CHILD_IDS: u64 = 1 << 62;
/// Operations a multithreaded failure keeps for diagnosis.
const RECENT: usize = 32;
/// Replays a minimization may spend.
const MAX_REPLAYS: usize = 4000;

// One per worker, so the size difference between the variants costs nothing.
#[allow(clippy::large_enum_variant)]
enum Source<'a> {
    Generated {
        generator: Generator,
        entropy: AnyEntropy<'a>,
    },
    Script(std::vec::IntoIter<Op>),
}

impl Source<'_> {
    fn next(&mut self) -> Option<Op> {
        match self {
            Source::Generated { generator, entropy } => generator.next_op(entropy),
            Source::Script(ops) => ops.next(),
        }
    }

    fn observe(&mut self, effect: &Effect) {
        if let Source::Generated { generator, .. } = self {
            generator.observe(effect);
        }
    }
}

enum TraceBuf {
    Complete(Vec<Op>),
    Recent(VecDeque<Op>),
}

impl TraceBuf {
    fn push(&mut self, op: Op) {
        match self {
            TraceBuf::Complete(v) => v.push(op),
            TraceBuf::Recent(q) => {
                if q.len() == RECENT {
                    q.pop_front();
                }
                q.push_back(op);
            }
        }
    }

    fn take(&mut self) -> Trace {
        match self {
            TraceBuf::Complete(v) => Trace::Complete(Script(std::mem::take(v))),
            TraceBuf::Recent(q) => Trace::Recent(Script(q.drain(..).collect())),
        }
    }
}

struct Worker<'e, S: ?Sized> {
    model: Model<'e, S>,
    source: Source<'e>,
    limit: usize,
    done: usize,
    trace: TraceBuf,
    fork: Option<(ForkPlan, Config)>,
}

/// The mailboxes of a run of `cfg`, each holding up to one worker's share of the budget.
fn mailroom(cfg: &Config) -> Mailroom {
    Mailroom::new(
        cfg.workers,
        cfg.handoff,
        cfg.profile.max_live_bytes / cfg.workers,
    )
}

/// Worker `w`'s generator and entropy in a run of `cfg` whose block names start at `id_space`.
fn stream_of(cfg: &Config, zero: ZeroSize, w: usize, id_space: u64) -> (Generator, Rng) {
    let generator = Generator::new(
        cfg.profile,
        zero,
        id_space | ((w as u64) << 40) | 1,
        cfg.profile.max_live_bytes / cfg.workers,
        exchange(cfg, w),
    );
    (generator, Rng::new(mix64(cfg.seed) ^ mix64(w as u64 + 1)))
}

impl<'e, S: Subject + ?Sized> Worker<'e, S> {
    fn generated(
        subject: &'e S,
        shadow: &'e Shadow,
        mail: &'e Mailroom,
        cfg: &Config,
        w: usize,
        id_space: u64,
    ) -> Self {
        let (generator, rng) = stream_of(cfg, subject.zero_size(), w, id_space);
        let entropy = AnyEntropy::Rng(rng);
        Worker {
            model: Model::new(subject, shadow, Some(mail), w),
            source: Source::Generated { generator, entropy },
            limit: cfg.ops,
            done: 0,
            trace: if cfg.workers == 1 {
                TraceBuf::Complete(Vec::new())
            } else {
                TraceBuf::Recent(VecDeque::new())
            },
            fork: None,
        }
    }

    fn attach(&mut self, mut f: Failure) -> Failure {
        f.trace = Some(self.trace.take());
        f
    }
}

/// The exchange for worker `w`: who it sends to and how often it sends and receives.
fn exchange(cfg: &Config, w: usize) -> Option<Exchange> {
    let workers = cfg.workers as u32;
    let me = w as u32;
    // Hand-offs compete with frees; batch profiles free only in their drain phase, so for
    // them half the allocation weight stands in.
    let w = cfg.profile.weights;
    let weight = w.free.max(w.alloc / 2).max(2);
    let x = |send, recv, target, alloc_percent, free_percent| {
        Some(Exchange {
            workers,
            me,
            send,
            recv,
            target,
            alloc_percent,
            free_percent,
        })
    };
    match cfg.handoff {
        Handoff::None => None,
        Handoff::Pairs if me.is_multiple_of(2) && me + 1 < workers => {
            x(weight, 0, Target::Partner(me + 1), 100, 25)
        }
        Handoff::Pairs if me % 2 == 1 => x(0, weight, Target::Partner(me - 1), 25, 100),
        Handoff::Pairs => None,
        Handoff::AllToAll if workers > 1 => x(weight / 2, weight / 2, Target::AnyOther, 100, 100),
        Handoff::AllToAll => None,
        Handoff::Steal => x(weight / 2, weight / 2, Target::Own, 100, 100),
    }
}

#[derive(Default)]
struct Shared {
    stop: AtomicBool,
    failure: Mutex<Option<Failure>>,
    stats: Mutex<Stats>,
    children: Mutex<Vec<PendingChild>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Shared {
    fn fail(&self, f: Failure) {
        let mut slot = lock(&self.failure);
        if slot.is_none() {
            *slot = Some(f);
        }
        // Relaxed: the flag only asks other workers to stop soon. It must not order their
        // allocator calls, or it would hide the races the run is looking for.
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Stops the other workers if this one unwinds, so a panic does not leave them running to the
/// end of their streams before the scope reports it.
struct StopOnPanic<'a>(&'a AtomicBool);

impl Drop for StopOnPanic<'_> {
    fn drop(&mut self) {
        if thread::panicking() {
            self.0.store(true, Ordering::Relaxed);
        }
    }
}

/// Where a run's threads come from.
trait Spawn<'scope>: Sync {
    /// Starts a thread that runs `job` and is joined before the run ends.
    fn spawn(&'scope self, job: Box<dyn FnOnce() + Send + 'scope>);
}

impl<'scope> Spawn<'scope> for thread::Scope<'scope, '_> {
    fn spawn(&'scope self, job: Box<dyn FnOnce() + Send + 'scope>) {
        thread::Scope::spawn(self, job);
    }
}

#[cfg(all(target_os = "linux", not(miri)))]
impl<'scope> Spawn<'scope> for crate::sys::ChildThreads {
    fn spawn(&'scope self, job: Box<dyn FnOnce() + Send + 'scope>) {
        crate::sys::ChildThreads::spawn(self, job);
    }
}

/// Which threads a run uses.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Threads {
    /// The standard library's.
    Std,
    /// The C library's, for the child of a fork (see [`crate::sys::ChildThreads`]).
    #[cfg(all(target_os = "linux", not(miri)))]
    AfterFork,
}

/// Run one worker until its stream ends, a failure, or a migration hands it to a new thread.
fn segment<'scope, 'e: 'scope, S: Subject + ?Sized, T: Spawn<'scope>>(
    scope: &'scope T,
    mut w: Worker<'e, S>,
    shared: &'scope Shared,
) {
    let _stop = StopOnPanic(&shared.stop);
    loop {
        if shared.stop.load(Ordering::Relaxed) {
            // Another worker failed. This one's blocks are abandoned, not freed: the allocator
            // is known broken and freeing into it could crash the test.
            return;
        }
        if let Some((plan, child)) = w.fork
            && w.done == plan.at
        {
            w.fork = None;
            let pending = fork_child(&mut w, plan, child);
            lock(&shared.children).push(pending);
        }
        if w.done >= w.limit {
            break;
        }
        let Some(op) = w.source.next() else { break };
        w.trace.push(op);
        let result = w.model.execute(&op);
        w.done += 1;
        match result {
            Ok(Effect::Migrate) => {
                scope.spawn(Box::new(move || segment(scope, w, shared)));
                return;
            }
            Ok(effect) => w.source.observe(&effect),
            Err(f) => {
                let f = w.attach(f);
                shared.fail(f);
                return;
            }
        }
    }
    match w.model.drain() {
        Ok(()) => lock(&shared.stats).merge(&w.model.stats()),
        Err(f) => {
            let f = w.attach(f);
            shared.fail(f);
        }
    }
}

/// Run `workers` to completion, then free whatever is left in the mailboxes.
fn drive<'e, S: Subject + ?Sized>(
    subject: &'e S,
    shadow: &'e Shadow,
    mail: &'e Mailroom,
    workers: Vec<Worker<'e, S>>,
    threads: Threads,
) -> Result<Stats, Failure> {
    let n = workers.len();
    let shared = Shared::default();
    let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match threads {
        Threads::Std => thread::scope(|scope| {
            for w in workers {
                let shared = &shared;
                scope.spawn(move || segment(scope, w, shared));
            }
        }),
        #[cfg(all(target_os = "linux", not(miri)))]
        Threads::AfterFork => crate::sys::ChildThreads::scope(|scope| {
            for w in workers {
                let shared = &shared;
                scope.spawn(Box::new(move || segment(scope, w, shared)));
            }
        }),
    }));
    let Shared {
        failure,
        stats,
        children,
        ..
    } = shared;
    // Every child is waited for, even after a failure or a panic, so none outlives the run.
    let mut child_failure = None;
    for c in children
        .into_inner()
        .unwrap_or_else(PoisonError::into_inner)
    {
        if let Err(f) = c.wait(subject.name()) {
            child_failure.get_or_insert(f);
        }
    }
    if let Err(panic) = ran {
        std::panic::resume_unwind(panic);
    }
    if let Some(f) = failure.into_inner().unwrap_or_else(PoisonError::into_inner) {
        return Err(f);
    }
    if let Some(f) = child_failure {
        return Err(f);
    }
    let mut stats = stats.into_inner().unwrap_or_else(PoisonError::into_inner);
    let mut reaper = Model::new(subject, shadow, Some(mail), n);
    for block in mail.drain_all() {
        reaper.adopt(block)?;
    }
    reaper.drain()?;
    stats.merge(&reaper.stats());
    Ok(stats)
}

fn assert_all_released(shadow: &Shadow) {
    assert!(
        shadow.is_empty(),
        "model bug: intervals left in the shadow after a clean run: {:?}",
        shadow.intervals()
    );
}

/// Run `cfg` against `subject`. Every block is freed by the end of a clean run.
pub fn run<S: Subject + ?Sized>(subject: &S, cfg: &Config) -> Result<Stats, Failure> {
    assert!(cfg.workers >= 1, "a run needs a worker");
    let shadow = Shadow::new();
    let mail = mailroom(cfg);
    let mut workers: Vec<_> = (0..cfg.workers)
        .map(|w| Worker::generated(subject, &shadow, &mail, cfg, w, 0))
        .collect();
    if let Some(plan) = cfg.fork {
        let child = Config {
            seed: mix64(cfg.seed ^ CHILD_IDS),
            ops: plan.child_ops,
            workers: plan.child_workers.max(1),
            fork: None,
            ..*cfg
        };
        workers[0].fork = Some((plan, child));
    }
    let result = drive(subject, &shadow, &mail, workers, Threads::Std).map_err(|mut f| {
        f.origin = Some(*cfg);
        f
    });
    if result.is_ok() {
        assert_all_released(&shadow);
    }
    result
}

/// The script a single-worker run of `profile` from `seed` draws for a subject that allows the
/// zero sizes in `zero`, without running it. Replaying it does what [`run`] does, except that
/// nothing feeds back: a zero-size allocation that returns null still has its later operations
/// in the script, and replay skips them. This is how a seeded stream becomes a regression
/// test.
pub fn generate(profile: Profile, zero: ZeroSize, seed: u64, ops: usize) -> Script {
    let (mut generator, mut rng) = stream_of(&Config::single(profile, seed, ops), zero, 0, 0);
    Script(
        (0..ops)
            .map_while(|_| generator.next_op(&mut rng))
            .collect(),
    )
}

/// [`run`], panicking with the failure report.
pub fn check<S: Subject + ?Sized>(subject: &S, cfg: &Config) -> Stats {
    run(subject, cfg).unwrap_or_else(|f| panic!("{f}"))
}

/// [`run`] against a fresh subject from `make`, panicking with the failure report and, for a
/// single-worker run, the shortest script found that replays to the same failure.
pub fn check_minimized<S: Subject, F: Fn() -> S>(make: F, cfg: &Config) -> Stats {
    let f = match run(&make(), cfg) {
        Ok(stats) => return stats,
        Err(f) => f,
    };
    match &f.trace {
        Some(Trace::Complete(script)) => {
            let small = minimize(&make, script, f.kind);
            panic!(
                "{f}\n  minimized to {} ops: \"{small}\"\n  replay with driver::replay",
                small.0.len()
            )
        }
        _ => panic!("{f}"),
    }
}

/// One worker driven by `data`: every decision reads four bytes, and the run ends when the
/// bytes do. This is the fuzzing entry point.
pub fn run_bytes<S: Subject + ?Sized>(
    subject: &S,
    profile: Profile,
    data: &[u8],
) -> Result<Stats, Failure> {
    let shadow = Shadow::new();
    let mail = Mailroom::new(1, Handoff::None, usize::MAX);
    let generator = Generator::new(
        profile,
        subject.zero_size(),
        1,
        profile.max_live_bytes,
        None,
    );
    let worker = Worker {
        model: Model::new(subject, &shadow, Some(&mail), 0),
        source: Source::Generated {
            generator,
            entropy: AnyEntropy::Bytes(ByteSource::new(data)),
        },
        limit: usize::MAX,
        done: 0,
        trace: TraceBuf::Complete(Vec::new()),
        fork: None,
    };
    let result = drive(subject, &shadow, &mail, vec![worker], Threads::Std);
    if result.is_ok() {
        assert_all_released(&shadow);
    }
    result
}

/// Execute `script` exactly, then free what it left live.
pub fn replay<S: Subject + ?Sized>(subject: &S, script: &Script) -> Result<Stats, Failure> {
    let shadow = Shadow::new();
    let mail = Mailroom::new(1, Handoff::None, usize::MAX);
    let worker = Worker {
        model: Model::new(subject, &shadow, Some(&mail), 0),
        source: Source::Script(script.0.clone().into_iter()),
        limit: usize::MAX,
        done: 0,
        trace: TraceBuf::Complete(Vec::new()),
        fork: None,
    };
    let result = drive(subject, &shadow, &mail, vec![worker], Threads::Std);
    if result.is_ok() {
        assert_all_released(&shadow);
    }
    result
}

/// The shortest part of `script` found that still fails with `kind`, replaying each candidate
/// against a fresh subject from `make`. Removes ever smaller runs of operations while the
/// failure persists; an operation on a block whose allocation was removed is skipped, so every
/// candidate is a valid script.
pub fn minimize<S: Subject, F: Fn() -> S>(make: F, script: &Script, kind: FailureKind) -> Script {
    let replays = std::cell::Cell::new(0);
    let fails = |ops: &[Op]| {
        replays.set(replays.get() + 1);
        matches!(replay(&make(), &Script(ops.to_vec())), Err(f) if f.kind == kind)
    };
    let mut ops = script.0.clone();
    if !fails(&ops) {
        return Script(ops);
    }
    let mut chunk = ops.len().div_ceil(2).max(1);
    loop {
        let mut removed = false;
        let mut start = 0;
        while start < ops.len() && replays.get() < MAX_REPLAYS {
            let end = (start + chunk).min(ops.len());
            let candidate: Vec<Op> = ops[..start].iter().chain(&ops[end..]).copied().collect();
            if fails(&candidate) {
                ops = candidate;
                removed = true;
            } else {
                start = end;
            }
        }
        if replays.get() >= MAX_REPLAYS || (chunk == 1 && !removed) {
            break;
        }
        if !removed {
            chunk = chunk.div_ceil(2);
        }
    }
    Script(ops)
}

/// A fork in progress, seen from the parent.
struct PendingChild {
    #[cfg(all(target_os = "linux", not(miri)))]
    child: crate::sys::Child,
    plan: ForkPlan,
}

impl PendingChild {
    #[cfg(all(target_os = "linux", not(miri)))]
    fn wait(self, subject: String) -> Result<(), Failure> {
        use crate::sys::Exit;
        let (exit, report) = self.child.wait(self.plan.timeout);
        let report = String::from_utf8_lossy(&report).into_owned();
        let (kind, detail) = match exit {
            Exit::Code(0) => return Ok(()),
            Exit::Code(1) => {
                let (name, text) = report.split_once('\n').unwrap_or(("", &report));
                let kind = FailureKind::from_name(name).unwrap_or(FailureKind::ChildPanicked);
                (kind, text.to_string())
            }
            Exit::Code(code) => (
                FailureKind::ChildPanicked,
                format!("exited with code {code}: {report}"),
            ),
            Exit::Signal(sig) => (FailureKind::ChildCrashed, format!("killed by signal {sig}")),
            Exit::TimedOut => (
                FailureKind::ChildHung,
                format!("still running after {:?}", self.plan.timeout),
            ),
        };
        Err(Failure {
            kind,
            subject,
            worker: 0,
            op_index: self.plan.at,
            op: None,
            detail: format!("in the child forked before op {}: {detail}", self.plan.at),
            origin: None,
            trace: None,
        })
    }

    #[cfg(not(all(target_os = "linux", not(miri))))]
    fn wait(self, _subject: String) -> Result<(), Failure> {
        let _ = self.plan;
        unreachable!("no child is created where fork is unsupported")
    }
}

/// Fork. The parent gets the pending child; the child runs `child` with the forking worker's
/// blocks and never returns.
#[cfg(all(target_os = "linux", not(miri)))]
fn fork_child<S: Subject + ?Sized>(
    w: &mut Worker<'_, S>,
    plan: ForkPlan,
    child: Config,
) -> PendingChild {
    use crate::sys::{Forked, exit_now, fork_with_pipe};
    use std::panic::{AssertUnwindSafe, catch_unwind};

    match fork_with_pipe() {
        Forked::Parent(c) => {
            w.model.note_fork();
            PendingChild { child: c, plan }
        }
        Forked::Child(report) => {
            // Never return: the caller is a thread of the parent's scope, whose other threads
            // do not exist here, and returning would wait for them forever.
            let outcome = catch_unwind(AssertUnwindSafe(|| run_child(w, &child)));
            let (code, text) = match outcome {
                Ok(Ok(_)) => (0, String::new()),
                Ok(Err(f)) => (1, format!("{:?}\n{f}", f.kind)),
                Err(_) => (2, "the child panicked".to_string()),
            };
            report.send(&text.as_bytes()[..text.len().min(16 * 1024)]);
            exit_now(code)
        }
    }
}

#[cfg(not(all(target_os = "linux", not(miri))))]
fn fork_child<S: Subject + ?Sized>(_: &mut Worker<'_, S>, _: ForkPlan, _: Config) -> PendingChild {
    panic!("fork tests run only on Linux outside Miri")
}

/// The child's run: a fresh index that keeps the parent's live blocks (except where a thread
/// held a shard's lock at the fork), the forking worker's blocks, and `cfg`'s workers.
#[cfg(all(target_os = "linux", not(miri)))]
fn run_child<S: Subject + ?Sized>(w: &mut Worker<'_, S>, cfg: &Config) -> Result<Stats, Failure> {
    use std::collections::HashSet;

    let subject = w.model.subject();
    // The forking worker's blocks, and those other workers had posted and not yet collected:
    // freeing memory another thread allocated before the fork is part of what is tested.
    let mut inherited = w.model.take_blocks();
    if let Some(parent_mail) = w.model.mail() {
        inherited.extend(parent_mail.take_after_fork());
    }
    let own: HashSet<_> = inherited.iter().map(|b| b.id).collect();
    let shadow = w
        .model
        .shadow()
        .copy_after_fork(|owner| !own.contains(&owner));
    let mail = mailroom(cfg);
    let mut workers: Vec<_> = (0..cfg.workers)
        .map(|i| Worker::generated(subject, &shadow, &mail, cfg, i, CHILD_IDS))
        .collect();
    let first = &mut workers[0];
    for block in inherited {
        let adopted = first.model.adopt_unclaimed(block)?;
        first.source.observe(&Effect::Adopted(vec![adopted]));
    }
    drive(subject, &shadow, &mail, workers, Threads::AfterFork)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subject::ViaGlobalAlloc;
    use std::alloc::System;

    const OPS: usize = if cfg!(miri) { 150 } else { 3000 };

    fn small(p: Profile) -> Profile {
        if cfg!(miri) {
            p.capped(2048, 64, 1 << 20)
        } else {
            p
        }
    }

    #[test]
    fn same_seed_same_stats_and_different_seed_different_stats() {
        let s = ViaGlobalAlloc(System);
        let p = small(Profile::MIXED.with_migrate(0));
        let a = check(&s, &Config::single(p, 1, OPS));
        let b = check(&s, &Config::single(p, 1, OPS));
        let c = check(&s, &Config::single(p, 2, OPS));
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(
            a.frees,
            a.blocks_allocated(),
            "every block freed once: {a:?}"
        );
    }

    #[test]
    fn a_dry_byte_source_ends_the_run_cleanly() {
        let bytes: Vec<u8> = (0..400u32)
            .flat_map(|i| (i * 0x9E37).to_le_bytes())
            .collect();
        let stats = run_bytes(&ViaGlobalAlloc(System), small(Profile::MIXED), &bytes).unwrap();
        assert!(stats.ops > 0 && stats.ops < 400, "{stats:?}");
        assert_eq!(stats.frees, stats.blocks_allocated());
    }

    #[test]
    fn migration_continues_the_stream_on_new_threads() {
        let p = small(Profile::MIXED.with_migrate(50));
        let stats = check(&ViaGlobalAlloc(System), &Config::single(p, 3, OPS / 2));
        assert!(stats.migrations > 0, "{stats:?}");
        assert_eq!(stats.ops, OPS / 2);
    }
}
