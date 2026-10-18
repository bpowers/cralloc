//! asmalloc's back-end launder and fill on their own: cycles and instructions per object, from
//! hardware counters, for several classes and pointer orders.
//!
//!   backbench run [classes=16,64,256,1024,4096] [reps=7] [batch=64] [orders=...] [fills=...]
//!
//! The benchmark keeps a heap of its own (a `back::State` over the real OS), separate from the
//! process's global allocator, which stays glibc's. For each class it carves a pool of objects
//! (as many as fit in a few MiB, so that laundering them never takes the retained count over
//! its cap and starts a purge), then repeats:
//!
//! - **launder**: return every object of the pool, `batch` at a time (the depot's chunk), in
//!   one of the orders below, and count the whole loop;
//! - **fill**: take them all back, `batch` at a time, from bitmaps that are full.
//!
//! Orders, each a permutation of the pool:
//!
//! - `asc`: ascending addresses, as a fill hands them out.
//! - `desc`: descending, as a magazine filled in ascending order is popped and freed in order.
//! - `two`: two descending streams interleaved, each from its own bitmap word: what laundered
//!   magazines of a parsed and dropped JSON document look like.
//! - `block`: shuffled within each 256 objects, a magazine's worth.
//! - `scatter`: shuffled across the whole pool, as a `BTreeMap` dropped in key order frees.
//! - `stride`: consecutive objects 64 words apart, so every object of a batch is in a word and
//!   a summary word of its own: the worst case for grouping.
//!
//! Fill variants: `fill` takes back the whole pool from full words; `fill/8` and `fill/64` first
//! launder one object in 8 (in 64: one per word), then time taking exactly those back, which is
//! what a fill in a fragmented heap does.
//!
//! Pin it with `BENCH_CPUS` (the first CPU listed is used) and run it under `scripts/memlimit`.
//! Each figure is the lowest of `reps` repetitions after one untimed warm-up.

use asmalloc::back::{Back, State};
use asmalloc::classes::{Class, SmallClass};
use asmalloc::os::linux::LinuxOs;
use bench_harness::report::Measure;
use bench_harness::{Args, Counters, Cpus, Rng};

static STATE: State<LinuxOs> = State::new(LinuxOs);

/// Bytes of objects per pool: a quarter of the small classes' retained cap, so a launder never
/// purges.
const POOL_BYTES: usize = 16 << 20;
/// At most this many objects per pool, so small classes span several summary words but a run
/// stays short.
const MAX_POOL: usize = 1 << 16;

#[derive(Clone, Copy, Default)]
struct Cost {
    cycles: f64,
    instructions: f64,
}

impl Cost {
    fn better(self, other: Cost) -> Cost {
        if self.cycles == 0.0 || other.cycles < self.cycles { other } else { self }
    }
}

struct Bench<'a> {
    back: Back<'a, LinuxOs>,
    class: SmallClass,
    batch: usize,
    counters: &'a Counters,
}

impl Bench<'_> {
    /// Fills exactly `n` objects into a vector, `batch` at a time. Panics if the back end
    /// gives fewer.
    fn take(&self, n: usize) -> Vec<*mut u8> {
        let mut out = vec![std::ptr::null_mut(); n];
        let mut got = 0;
        while got < n {
            let want = (n - got).min(self.batch);
            let filled = self.back.fill(self.class, &mut out[got..got + want]);
            assert_eq!(filled, want, "the back end ran out of memory");
            got += filled;
        }
        out
    }

    /// Launders `objects` in order, `batch` at a time.
    fn give(&self, objects: &[*mut u8]) {
        let mut buffer = vec![std::ptr::null_mut(); self.batch];
        for chunk in objects.chunks(self.batch) {
            buffer[..chunk.len()].copy_from_slice(chunk);
            self.back.launder(self.class, &buffer[..chunk.len()]);
        }
    }

    fn cost(&self, n: usize, f: impl FnOnce()) -> Cost {
        let m = Measure::start(self.counters);
        f();
        let phase = m.stop();
        assert!(!phase.counters.multiplexed, "counters were multiplexed");
        let get = |name| phase.counters.get(name).unwrap_or(0) as f64 / n as f64;
        Cost { cycles: get("cycles"), instructions: get("instructions") }
    }

    /// Launders the pool in `order`, then fills it back. Returns both costs and the new pool.
    fn round(&self, order: &[*mut u8]) -> (Cost, Cost, Vec<*mut u8>) {
        let n = order.len();
        let launder = self.cost(n, || self.give(order));
        let mut pool = Vec::new();
        let fill = self.cost(n, || pool = self.take(n));
        (launder, fill, pool)
    }

    /// Launders one object in `every` of the pool (untimed), times filling exactly those back,
    /// and returns the cost.
    fn sparse_fill(&self, pool: &mut [*mut u8], every: usize) -> Cost {
        let freed: Vec<*mut u8> = pool.iter().step_by(every).copied().collect();
        self.give(&freed);
        let mut got = Vec::new();
        let cost = self.cost(freed.len(), || got = self.take(freed.len()));
        for (slot, object) in pool.iter_mut().step_by(every).zip(got) {
            *slot = object;
        }
        cost
    }
}

/// The pool in the named order. `pool` is ascending.
fn order(name: &str, pool: &[*mut u8], rng: &mut Rng) -> Vec<*mut u8> {
    let mut v = pool.to_vec();
    match name {
        "asc" => {}
        "desc" => v.reverse(),
        "two" => {
            v.reverse();
            for block in v.chunks_mut(128) {
                let half = block.len() / 2;
                let (a, b) = block.split_at(half);
                let mixed: Vec<*mut u8> = a
                    .iter()
                    .zip(b)
                    .flat_map(|(&x, &y)| [x, y])
                    .chain(b[a.len().min(b.len())..].iter().copied())
                    .collect();
                block.copy_from_slice(&mixed);
            }
        }
        "block" => v.chunks_mut(256).for_each(|block| shuffle(block, rng)),
        "scatter" => shuffle(&mut v, rng),
        "stride" => {
            // Bit within the word slowest, summary word fastest.
            let mut keyed: Vec<((usize, usize, usize), *mut u8)> =
                v.iter().enumerate().map(|(i, &p)| ((i % 64, i / 64 % 64, i / 4096), p)).collect();
            keyed.sort_by_key(|&(key, _)| key);
            v = keyed.into_iter().map(|(_, p)| p).collect();
        }
        _ => panic!("unknown order {name}"),
    }
    v
}

fn shuffle(v: &mut [*mut u8], rng: &mut Rng) {
    for i in (1..v.len()).rev() {
        v.swap(i, rng.below(i as u64 + 1) as usize);
    }
}

fn main() {
    let (command, args) = Args::from_env();
    assert_eq!(command, "run", "usage: backbench run [key=value ...]");
    Cpus::from_env().pin(0);
    let classes: Vec<usize> =
        args.str("classes", "16,64,256,1024,4096").split(',').map(|s| s.parse().expect("a class size")).collect();
    let reps = args.usize("reps", 7);
    let batch = args.usize("batch", 64);
    let orders = args.str("orders", "asc,desc,two,block,scatter,stride");
    let fills = args.str("fills", "fill,fill/8,fill/64");
    let back = STATE.get_or_reserve().expect("reserve the heap");
    let counters = Counters::region();
    println!("{:>6} {:>8} {:>7}  {:>9} {:>9}", "class", "case", "objects", "cyc/obj", "ins/obj");
    for size in classes {
        let class = Class::of_size(size).and_then(Class::small).expect("a small class size");
        let bench = Bench { back, class, batch, counters: &counters };
        let n = (POOL_BYTES / class.size()).min(MAX_POOL);
        let mut pool = bench.take(n);
        pool.sort();
        let mut rng = Rng::new(size as u64);
        let mut dense_fill = Cost::default();
        for name in orders.split(',') {
            let mut launder = Cost::default();
            for rep in 0..=reps {
                let ord = order(name, &pool, &mut rng);
                let (l, f, mut next) = bench.round(&ord);
                next.sort();
                pool = next;
                if rep > 0 {
                    launder = launder.better(l);
                    dense_fill = dense_fill.better(f);
                }
            }
            println!("{:>6} {:>8} {:>7}  {:>9.1} {:>9.1}", class.size(), name, n, launder.cycles, launder.instructions);
        }
        for name in fills.split(',') {
            let cost = match name {
                "fill" => dense_fill,
                _ => {
                    let every: usize = name.strip_prefix("fill/").and_then(|s| s.parse().ok()).expect("fill/N");
                    let mut best = Cost::default();
                    for rep in 0..=reps {
                        let c = bench.sparse_fill(&mut pool, every);
                        if rep > 0 {
                            best = best.better(c);
                        }
                    }
                    best
                }
            };
            let objects = if name == "fill" { n } else { n / name[5..].parse::<usize>().unwrap() };
            println!("{:>6} {:>8} {:>7}  {:>9.1} {:>9.1}", class.size(), name, objects, cost.cycles, cost.instructions);
        }
        // The pool stays held: freed, it would count as retained while the next class runs,
        // and enough of that would start purges.
        drop(pool);
    }
}
