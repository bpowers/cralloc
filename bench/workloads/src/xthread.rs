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
//! Each thread first runs a tenth of the work unmeasured, then, after all have, the measured
//! run.
//!
//! Parameters: `threads` (or `producers` and `consumers`), `objects` per stage or per
//! producer set (default 10M), `sizes` (see `SizeMix`), `inflight` (queue capacity in objects
//! per link, default 4096).
use crate::obj::{Obj, consume, make};
use bench_harness::report::{Measure, Phase};
use bench_harness::{Args, Better, Counters, Cpus, Report, Rng, SizeMix};
use crossbeam_queue::ArrayQueue;
use std::sync::Barrier;
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

/// The warm-up runs this fraction of the measured work, on the same threads, so that
/// per-thread caches it fills are still there when measurement starts.
const WARMUP_DIVISOR: usize = 10;

/// Runs `body(i, phase)` on `n` threads, thread `i` pinned to `cpus.nth(i)`: first phase 0,
/// the warm-up, then, once every thread has finished it, phase 1, which is measured from
/// the moment all threads are released until the last one finishes.
fn run_threads(cpus: &Cpus, n: usize, counters: &Counters, body: impl Fn(usize, usize) + Sync) -> Phase {
    let warmed = Barrier::new(n + 1);
    let go = AtomicBool::new(false);
    std::thread::scope(|s| {
        let handles: Vec<_> = (0..n)
            .map(|i| {
                let (warmed, go, body) = (&warmed, &go, &body);
                s.spawn(move || {
                    cpus.pin(i);
                    body(i, 0);
                    warmed.wait();
                    while !go.load(Acquire) {
                        std::hint::spin_loop();
                    }
                    body(i, 1);
                })
            })
            .collect();
        warmed.wait();
        let m = Measure::start(counters);
        go.store(true, Release);
        for h in handles {
            h.join().unwrap();
        }
        m.stop()
    })
}

fn size_mix(args: &Args) -> SizeMix {
    let s = args.str("sizes", "small");
    SizeMix::parse(&s).unwrap_or_else(|| panic!("sizes={s} is not a size mix"))
}

fn round_to_batch(n: u64) -> usize {
    (n as usize / BATCH).max(1) * BATCH
}

/// Work per phase: a tenth for the warm-up, all of it for the measurement.
fn per_phase(n: usize) -> [usize; 2] {
    [round_to_batch((n / WARMUP_DIVISOR) as u64), n]
}

fn finish(name: &str, args: &Args, phase: &Phase, allocations: usize, threads: usize) {
    let cycles = phase.counters.get("cycles").expect("the cycles counter could not be read");
    Report::new(name, args)
        .param("threads", threads as u64)
        .phase("run", phase)
        .metric("allocations", allocations as u64)
        // Cycles over all threads, spinning included; frequency-independent, unlike the
        // wall-time ranking.
        .metric("cycles_per_alloc", cycles as f64 / allocations as f64)
        .primary("ns/alloc", phase.wall_ns as f64 / allocations as f64, Better::Lower)
        .emit();
}

pub fn pipeline(args: &Args) {
    let cpus = Cpus::from_env();
    let stages = args.usize("threads", 2);
    assert!(stages >= 2, "a pipeline needs at least two stages");
    let n = per_phase(round_to_batch(args.num("objects", 10_000_000)));
    let (mix, inflight) = (size_mix(args), args.usize("inflight", 4096));
    let counters = Counters::region();
    let rings: [Vec<Ring>; 2] = std::array::from_fn(|_| (0..stages - 1).map(|_| Ring::new(inflight)).collect());
    let phase = run_threads(&cpus, stages, &counters, |i, ph| {
        let (rings, n) = (&rings[ph], n[ph]);
        let mut rng = Rng::new((ph * 1000 + i) as u64 + 1);
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
    });
    finish("pipeline", args, &phase, n[1] * (stages - 1), stages);
}

pub fn exchange(args: &Args) {
    let cpus = Cpus::from_env();
    let k = args.usize("threads", 2);
    assert!(k >= 2, "an exchange needs at least two threads");
    // `objects` is the total; each thread allocates its share.
    let n = per_phase(round_to_batch(args.num("objects", 10_000_000) / k as u64));
    let (mix, inflight) = (size_mix(args), args.usize("inflight", 4096));
    let counters = Counters::region();
    let rings: [Vec<Ring>; 2] = std::array::from_fn(|_| (0..k).map(|_| Ring::new(inflight)).collect());
    let phase = run_threads(&cpus, k, &counters, |i, ph| {
        let (rings, n) = (&rings[ph], n[ph]);
        let mut rng = Rng::new((ph * 1000 + i) as u64 + 1);
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
    });
    finish("exchange", args, &phase, n[1] * k, k);
}

pub fn mpmc(args: &Args) {
    let cpus = Cpus::from_env();
    let (p, c) = (args.usize("producers", 2), args.usize("consumers", 2));
    assert!(p >= 1 && c >= 1);
    // `objects` is the total; each producer makes its share.
    let per = round_to_batch(args.num("objects", 10_000_000) / p as u64);
    let batches = per_phase(per).map(|n| n / BATCH);
    let (mix, inflight) = (size_mix(args), args.usize("inflight", 4096));
    let counters = Counters::region();
    let queues: [ArrayQueue<[Obj; BATCH]>; 2] = std::array::from_fn(|_| ArrayQueue::new((inflight / BATCH).max(2)));
    let done = [AtomicUsize::new(0), AtomicUsize::new(0)];
    let phase = run_threads(&cpus, p + c, &counters, |i, ph| {
        let (q, done, batches) = (&queues[ph], &done[ph], batches[ph]);
        let total = batches * p;
        if i < p {
            let mut rng = Rng::new((ph * 1000 + i) as u64 + 1);
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
    });
    finish("mpmc", args, &phase, batches[1] * BATCH * p, p + c);
}
