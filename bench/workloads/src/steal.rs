//! Work-stealing pools, where a task allocates on whichever worker runs it and its result is
//! dropped on whichever worker runs the task that consumes it.
//!
//! - `forkjoin` (rayon): parallel-iterator stages over `items` objects, `stages` of them.
//!   The first stage allocates the objects with `into_par_iter().map().collect()`; each later
//!   stage maps every object to a new one and frees the old, and the last frees them all.
//!   Rayon splits each stage's work afresh by work stealing, so an object is often freed on
//!   a different worker from the one that allocated it.
//! - `spawn` (tokio): `tasks` concurrent parent tasks each spawn `fanout` children per round.
//!   A child allocates a request, yields (so an idle worker can steal it), allocates a
//!   response and frees the request; the parent frees the responses. Tokio's own task cells
//!   are allocated by the spawner and freed wherever the task ends.
//!
//! Both count how many of their own frees happened on a different worker from the
//! allocation, and report the share as `cross_free_share`.
use crate::obj::{set_worker, worker};
use bench_harness::report::Measure;
use bench_harness::{Args, Better, Counters, Cpus, Report, Rng, SizeMix};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering::*};

const MAX_WORKERS: usize = 64;

#[repr(align(128))]
struct Slot {
    frees: AtomicU64,
    cross: AtomicU64,
}

/// Per-worker free counts. Each worker writes only its own slot, so a relaxed load and
/// store is enough and costs no more than a plain increment.
struct FreeCounts([Slot; MAX_WORKERS]);

static COUNTS: FreeCounts =
    FreeCounts([const { Slot { frees: AtomicU64::new(0), cross: AtomicU64::new(0) } }; MAX_WORKERS]);

impl FreeCounts {
    #[inline]
    fn record(&self, origin: u64) {
        let me = worker();
        if me as usize >= MAX_WORKERS {
            return;
        }
        let s = &self.0[me as usize];
        s.frees.store(s.frees.load(Relaxed) + 1, Relaxed);
        if origin != me as u64 {
            s.cross.store(s.cross.load(Relaxed) + 1, Relaxed);
        }
    }

    fn take(&self) -> (u64, u64) {
        self.0.iter().fold((0, 0), |(f, c), s| (f + s.frees.swap(0, Relaxed), c + s.cross.swap(0, Relaxed)))
    }
}

/// An object owned by a task: a heap block whose first word names the worker that
/// allocated it.
struct Item(Box<[u64]>);

impl Item {
    #[inline]
    fn new(bytes: usize, salt: u64) -> Item {
        let mut b = vec![0u64; bytes / 8].into_boxed_slice();
        b[0] = worker() as u64;
        b[1] = salt;
        Item(b)
    }
}

impl Drop for Item {
    #[inline]
    fn drop(&mut self) {
        COUNTS.record(self.0[0]);
    }
}

fn size_mix(args: &Args) -> SizeMix {
    let s = args.str("sizes", "small");
    SizeMix::parse(&s).unwrap_or_else(|| panic!("sizes={s} is not a size mix"))
}

/// The size of object `i` in stage `stage`: a pure function, so every worker split gives the
/// same requests.
#[inline]
fn size_of(mix: SizeMix, stage: usize, i: usize) -> usize {
    mix.sample(&mut Rng::new(((stage as u64) << 40) ^ i as u64))
}

fn stages_run(items: usize, stages: usize, mix: SizeMix) {
    use rayon::prelude::*;
    let mut v: Vec<Item> = (0..items).into_par_iter().map(|i| Item::new(size_of(mix, 0, i), i as u64)).collect();
    for s in 1..stages {
        v = v
            .into_par_iter()
            .enumerate()
            .map(|(i, old)| {
                let new = Item::new(size_of(mix, s, i), old.0[1]);
                drop(old);
                new
            })
            .collect();
    }
    v.into_par_iter().for_each(drop);
}

pub fn forkjoin(args: &Args) {
    let cpus = Cpus::from_env();
    let threads = args.usize("threads", 4);
    let items = args.usize("items", 100_000);
    let stages = args.usize("stages", 4);
    let reps = args.usize("reps", 50);
    let mix = size_mix(args);
    let counters = Counters::region();
    let pin = cpus.clone();
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .start_handler(move |i| {
            pin.pin(i);
            set_worker(i);
        })
        .build()
        .unwrap();
    pool.install(|| stages_run(items, stages, mix));
    COUNTS.take();
    let m = Measure::start(&counters);
    pool.install(|| {
        for _ in 0..reps {
            stages_run(items, stages, mix);
        }
    });
    let phase = m.stop();
    let (frees, cross) = COUNTS.take();
    let allocations = (reps * items * stages) as u64;
    Report::new("forkjoin", args)
        .param("threads", threads as u64)
        .phase("run", &phase)
        .metric("allocations", allocations)
        .metric("cross_free_share", cross as f64 / frees.max(1) as f64)
        .primary("ns/alloc", phase.wall_ns as f64 / allocations as f64, Better::Lower)
        .emit();
}

async fn child(bytes: (usize, usize), salt: u64) -> Item {
    let request = Item::new(bytes.0, salt);
    tokio::task::yield_now().await;
    let response = Item::new(bytes.1, request.0[1].rotate_left(7));
    drop(request);
    response
}

async fn parent(id: u64, rounds: usize, fanout: usize, mix: SizeMix) {
    let mut rng = Rng::new(id);
    let mut handles = Vec::with_capacity(fanout);
    for r in 0..rounds {
        for k in 0..fanout {
            let bytes = (mix.sample(&mut rng), mix.sample(&mut rng));
            handles.push(tokio::spawn(child(bytes, (r * fanout + k) as u64)));
        }
        for h in handles.drain(..) {
            drop(h.await.unwrap());
        }
    }
}

async fn spawn_round(tasks: usize, rounds: usize, fanout: usize, mix: SizeMix) {
    let parents: Vec<_> = (0..tasks).map(|t| tokio::spawn(parent(t as u64 + 1, rounds, fanout, mix))).collect();
    for p in parents {
        p.await.unwrap();
    }
}

pub fn spawn(args: &Args) {
    let cpus = Cpus::from_env();
    let threads = args.usize("threads", 4);
    let tasks = args.usize("tasks", 4 * threads);
    let fanout = args.usize("fanout", 16);
    let rounds = args.usize("rounds", 2000);
    let mix = size_mix(args);
    let counters = Counters::region();
    let next = Arc::new(AtomicUsize::new(0));
    let pin = cpus.clone();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(threads)
        .on_thread_start(move || {
            let i = next.fetch_add(1, Relaxed);
            pin.pin(i);
            set_worker(i);
        })
        .build()
        .unwrap();
    rt.block_on(async { tokio::spawn(spawn_round(tasks, (rounds / 10).max(1), fanout, mix)).await.unwrap() });
    COUNTS.take();
    let m = Measure::start(&counters);
    rt.block_on(async { tokio::spawn(spawn_round(tasks, rounds, fanout, mix)).await.unwrap() });
    let phase = m.stop();
    drop(rt);
    let (frees, cross) = COUNTS.take();
    let children = (tasks * rounds * fanout) as u64;
    Report::new("spawn", args)
        .param("threads", threads as u64)
        .param("tasks", tasks as u64)
        .phase("run", &phase)
        .metric("children", children)
        .metric("cross_free_share", cross as f64 / frees.max(1) as f64)
        .primary("ns/task", phase.wall_ns as f64 / children as f64, Better::Lower)
        .emit();
}
