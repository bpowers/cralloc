//! Objects allocated on one thread and freed on another.
//!
//! - `pipeline`: a chain of `threads` stages. The first allocates; each middle stage frees
//!   what it receives and allocates a replacement for the next stage, as a stream processor
//!   converting messages does; the last only frees. Objects flow one way.
//! - `exchange`: a ring of `threads`, each allocating for the next and freeing what the
//!   previous sent. Every thread allocates and frees at the same rate but never frees its
//!   own objects.
//! - `mpmc`: `producers` threads allocate into one shared queue, `consumers` threads free.
//!
//! Hand-offs go in batches of 32 through queues that never allocate, so their cost is small
//! and the same for every allocator. Threads spin while they wait, on CPUs of their own.
//! Each run does a short unmeasured warm-up, then the measured run.
//!
//! Parameters: `threads` (or `producers` and `consumers`), `objects` per stage or per
//! producer set (default 10M), `sizes` (see `SizeMix`), `inflight` (queue capacity in objects
//! per link, default 4096).
use crate::obj::{Obj, consume, make};
use bench_harness::report::{Measure, Phase};
use bench_harness::{Args, Better, Counters, Cpus, Report, Rng, SizeMix};
use crossbeam_queue::ArrayQueue;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering::*};

const BATCH: usize = 32;

#[repr(align(128))]
struct Padded<T>(T);

/// A bounded single-producer single-consumer ring of object pointers.
struct Ring {
    slots: Box<[AtomicPtr<u8>]>,
    mask: usize,
    head: Padded<AtomicUsize>,
    tail: Padded<AtomicUsize>,
}

impl Ring {
    fn new(capacity: usize) -> Ring {
        let cap = capacity.max(2 * BATCH).next_power_of_two();
        Ring {
            slots: (0..cap).map(|_| AtomicPtr::new(std::ptr::null_mut())).collect(),
            mask: cap - 1,
            head: Padded(AtomicUsize::new(0)),
            tail: Padded(AtomicUsize::new(0)),
        }
    }

    /// Producer: room for `n` more?
    fn has_room(&self, head: usize, n: usize) -> bool {
        head + n - self.tail.0.load(Acquire) <= self.mask + 1
    }

    /// Producer: publishes `batch` at `head` (which must have room) and returns the new head.
    fn publish(&self, head: usize, batch: &[Obj]) -> usize {
        for (i, o) in batch.iter().enumerate() {
            self.slots[(head + i) & self.mask].store(o.0, Relaxed);
        }
        let h = head + batch.len();
        self.head.0.store(h, Release);
        h
    }

    /// Producer: waits for room, then publishes.
    fn push(&self, head: usize, batch: &[Obj]) -> usize {
        while !self.has_room(head, batch.len()) {
            std::hint::spin_loop();
        }
        self.publish(head, batch)
    }

    /// Consumer: objects published but not yet taken, from `tail`.
    fn available(&self, tail: usize) -> usize {
        self.head.0.load(Acquire) - tail
    }

    fn get(&self, i: usize) -> Obj {
        Obj(self.slots[i & self.mask].load(Relaxed))
    }

    fn release(&self, tail: usize) {
        self.tail.0.store(tail, Release);
    }
}

/// Runs `body(i)` on `n` threads, thread `i` pinned to `cpus.nth(i)`, and measures from the
/// moment all of them are ready until the last one finishes.
fn run_threads(cpus: &Cpus, n: usize, counters: Option<&Counters>, body: impl Fn(usize) + Sync) -> Option<Phase> {
    let ready = AtomicUsize::new(0);
    let go = AtomicBool::new(false);
    std::thread::scope(|s| {
        let handles: Vec<_> = (0..n)
            .map(|i| {
                let (ready, go, body) = (&ready, &go, &body);
                s.spawn(move || {
                    cpus.pin(i);
                    ready.fetch_add(1, AcqRel);
                    while !go.load(Acquire) {
                        std::hint::spin_loop();
                    }
                    body(i);
                })
            })
            .collect();
        while ready.load(Acquire) < n {
            std::thread::yield_now();
        }
        let m = counters.map(Measure::start);
        go.store(true, Release);
        for h in handles {
            h.join().unwrap();
        }
        m.map(Measure::stop)
    })
}

fn size_mix(args: &Args) -> SizeMix {
    let s = args.str("sizes", "small");
    SizeMix::parse(&s).unwrap_or_else(|| panic!("sizes={s} is not a size mix"))
}

fn round_to_batch(n: u64) -> usize {
    (n as usize / BATCH).max(1) * BATCH
}

fn finish(name: &str, args: &Args, phase: &Phase, allocations: usize, threads: usize) {
    let mut r = Report::new(name, args);
    r.param("threads", threads as u64)
        .phase("run", phase)
        .metric("allocations", allocations as u64)
        .primary("ns/alloc", phase.wall_ns as f64 / allocations as f64, Better::Lower)
        .emit();
}

/// One run of the chain; returns the measured phase if `counters` is given.
fn pipeline_run(
    cpus: &Cpus,
    stages: usize,
    n: usize,
    mix: SizeMix,
    inflight: usize,
    counters: Option<&Counters>,
) -> Option<Phase> {
    let rings: Vec<Ring> = (0..stages - 1).map(|_| Ring::new(inflight)).collect();
    run_threads(cpus, stages, counters, |i| {
        let mut rng = Rng::new(i as u64 + 1);
        let input = i.checked_sub(1).map(|j| &rings[j]);
        let output = rings.get(i);
        let mut batch = [Obj::NULL; BATCH];
        let (mut head, mut tail) = (0usize, 0usize);
        while head < n && output.is_some() || tail < n && input.is_some() {
            let take = match input {
                Some(r) => {
                    let avail = r.available(tail).min(BATCH);
                    if avail == 0 {
                        std::hint::spin_loop();
                        continue;
                    }
                    avail
                }
                None => BATCH.min(n - head),
            };
            for (k, slot) in batch.iter_mut().enumerate().take(take) {
                if let Some(r) = input {
                    // SAFETY: the previous stage made this object and handed it to us.
                    unsafe { consume(r.get(tail + k)) };
                }
                if output.is_some() {
                    *slot = make(mix.sample(&mut rng), i as u64);
                }
            }
            if let Some(r) = input {
                tail += take;
                r.release(tail);
            }
            if let Some(r) = output {
                head = r.push(head, &batch[..take]);
            }
        }
    })
}

pub fn pipeline(args: &Args) {
    let cpus = Cpus::from_env();
    let stages = args.usize("threads", 2);
    assert!(stages >= 2, "a pipeline needs at least two stages");
    let n = round_to_batch(args.num("objects", 10_000_000));
    let (mix, inflight) = (size_mix(args), args.usize("inflight", 4096));
    let counters = Counters::region();
    pipeline_run(&cpus, stages, n / 10, mix, inflight, None);
    let phase = pipeline_run(&cpus, stages, n, mix, inflight, Some(&counters)).unwrap();
    finish("pipeline", args, &phase, n * (stages - 1), stages);
}

fn exchange_run(
    cpus: &Cpus,
    k: usize,
    n: usize,
    mix: SizeMix,
    inflight: usize,
    counters: Option<&Counters>,
) -> Option<Phase> {
    let rings: Vec<Ring> = (0..k).map(|_| Ring::new(inflight)).collect();
    run_threads(cpus, k, counters, |i| {
        let mut rng = Rng::new(i as u64 + 1);
        let (out, inp) = (&rings[i], &rings[(i + k - 1) % k]);
        let mut batch = [Obj::NULL; BATCH];
        let (mut head, mut tail) = (0usize, 0usize);
        while head < n || tail < n {
            let mut worked = false;
            if head < n && out.has_room(head, BATCH) {
                for slot in batch.iter_mut() {
                    *slot = make(mix.sample(&mut rng), i as u64);
                }
                head = out.publish(head, &batch);
                worked = true;
            }
            let avail = inp.available(tail);
            if avail > 0 {
                for j in 0..avail {
                    // SAFETY: the previous thread in the ring made this object for us.
                    unsafe { consume(inp.get(tail + j)) };
                }
                tail += avail;
                inp.release(tail);
                worked = true;
            }
            if !worked {
                std::hint::spin_loop();
            }
        }
    })
}

pub fn exchange(args: &Args) {
    let cpus = Cpus::from_env();
    let k = args.usize("threads", 2);
    assert!(k >= 2, "an exchange needs at least two threads");
    // `objects` is the total; each thread allocates its share.
    let n = round_to_batch(args.num("objects", 10_000_000) / k as u64);
    let (mix, inflight) = (size_mix(args), args.usize("inflight", 4096));
    let counters = Counters::region();
    exchange_run(&cpus, k, n / 10, mix, inflight, None);
    let phase = exchange_run(&cpus, k, n, mix, inflight, Some(&counters)).unwrap();
    finish("exchange", args, &phase, n * k, k);
}

fn mpmc_run(
    cpus: &Cpus,
    p: usize,
    c: usize,
    batches: usize,
    mix: SizeMix,
    inflight: usize,
    counters: Option<&Counters>,
) -> Option<Phase> {
    let q: ArrayQueue<[Obj; BATCH]> = ArrayQueue::new((inflight / BATCH).max(2));
    let done = AtomicUsize::new(0);
    let total = batches * p;
    run_threads(cpus, p + c, counters, |i| {
        if i < p {
            let mut rng = Rng::new(i as u64 + 1);
            for _ in 0..batches {
                let mut b = [Obj::NULL; BATCH];
                for slot in b.iter_mut() {
                    *slot = make(mix.sample(&mut rng), i as u64);
                }
                while let Err(back) = q.push(b) {
                    b = back;
                    std::hint::spin_loop();
                }
            }
        } else {
            loop {
                if let Some(b) = q.pop() {
                    for o in b {
                        // SAFETY: a producer made this object and gave up ownership.
                        unsafe { consume(o) };
                    }
                    done.fetch_add(1, AcqRel);
                } else if done.load(Acquire) >= total {
                    break;
                } else {
                    std::hint::spin_loop();
                }
            }
        }
    })
}

pub fn mpmc(args: &Args) {
    let cpus = Cpus::from_env();
    let (p, c) = (args.usize("producers", 2), args.usize("consumers", 2));
    assert!(p >= 1 && c >= 1);
    // `objects` is the total; each producer makes its share.
    let batches = round_to_batch(args.num("objects", 10_000_000) / p as u64) / BATCH;
    let (mix, inflight) = (size_mix(args), args.usize("inflight", 4096));
    let counters = Counters::region();
    mpmc_run(&cpus, p, c, (batches / 10).max(1), mix, inflight, None);
    let phase = mpmc_run(&cpus, p, c, batches, mix, inflight, Some(&counters)).unwrap();
    finish("mpmc", args, &phase, batches * BATCH * p, p + c);
}
