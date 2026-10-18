//! asmalloc's back-end launder and fill on their own: cycles and instructions per object, from
//! hardware counters, for several classes and pointer orders.
//!
//!   backbench run [classes=16,64,256,1024,4096] [reps=7] [batch=mag] [orders=...] [fills=...]
//!
//! The benchmark keeps a heap of its own (a `back::State` over the real OS), separate from the
//! process's global allocator, which stays glibc's. For each class it carves a pool of objects
//! (as many as fit in a few MiB, so that laundering them never takes the retained count over
//! its cap and starts a purge), then repeats:
//!
//! - **launder**: return every object of the pool, `batch` at a time (default `mag`: a whole magazine, as the depot does), in
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
//! - `spread`: shuffled across the whole pool, then each 254 sorted: scattered slots that a
//!   magazine holds in address order.
//! - `stride`: consecutive objects 64 words apart, so every object of a batch is in a word and
//!   a summary word of its own: the worst case for grouping.
//!
//! Fill variants: `fill` takes back the whole pool from full words; `fill/8` and `fill/64` first
//! launder one object in 8 (in 64: one per word), then time taking exactly those back, which is
//! what a fill in a fragmented heap does.
//!
//! Pin it with `BENCH_CPUS` (the first CPU listed is used) and run it under `scripts/memlimit`.
//! Each figure is the lowest of `reps` repetitions after one untimed warm-up.

use asmalloc::back::{Back, Descending, State};
use asmalloc::classes::{Class, SmallClass};
use asmalloc::os::linux::LinuxOs;
use bench_harness::report::Measure;
use bench_harness::{Args, Counters, Cpus, Rng};
use std::sync::atomic::AtomicPtr;

static STATE: State<LinuxOs> = State::new(LinuxOs);

/// Bytes of objects per pool: a quarter of the small classes' retained cap, so a launder never
/// purges.
const POOL_BYTES: usize = 16 << 20;
/// At most this many objects per pool, so small classes span several summary words but a run
/// stays short.
const MAX_POOL: usize = 1 << 16;

#[derive(Clone, Default)]
struct Cost {
    cycles: f64,
    instructions: f64,
    /// Every counter, `BENCH_EVENTS` included, per object.
    events: Vec<(&'static str, f64)>,
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
    /// Fills exactly `n` objects into `out`, `batch` at a time, as the depot fills a
    /// magazine: into its slots, from the top (`back::Descending`). Each magazine is then
    /// copied out whole, as a stand-in for the front end. Panics if the back end gives fewer.
    /// `out` should have room for `n`, so that a measured call does not grow it.
    fn take(&self, n: usize, out: &mut Vec<*mut u8>) {
        let mut magazine = vec![std::ptr::null_mut::<u8>(); self.batch];
        out.clear();
        while out.len() < n {
            let want = (n - out.len()).min(self.batch);
            let filled = self.back.fill(self.class, &mut Descending(&atomic(&mut magazine)[..want]));
            assert_eq!(filled, want, "the back end ran out of memory");
            out.extend_from_slice(&magazine[..want]);
        }
    }

    /// Launders `objects` in order, `batch` at a time, as the depot launders a magazine: the
    /// objects are copied into a magazine's slots, as frees would push them, and the back end
    /// reads them from there.
    fn give(&self, objects: &[*mut u8]) {
        let mut magazine = vec![std::ptr::null_mut::<u8>(); self.batch];
        for chunk in objects.chunks(self.batch) {
            magazine[..chunk.len()].copy_from_slice(chunk);
            // SAFETY: the bench filled these slots itself and uses none of them.
            unsafe { self.back.launder(self.class, &atomic(&mut magazine)[..chunk.len()]) };
        }
    }

    fn cost(&self, n: usize, f: impl FnOnce()) -> Cost {
        let m = Measure::start(self.counters);
        f();
        let phase = m.stop();
        assert!(!phase.counters.multiplexed, "counters were multiplexed");
        let get = |name| phase.counters.get(name).unwrap_or(0) as f64 / n as f64;
        let events = phase.counters.values.iter().map(|&(name, v)| (name, v as f64 / n as f64)).collect();
        Cost { cycles: get("cycles"), instructions: get("instructions"), events }
    }

    /// Launders the pool in `order`, then fills it back. Returns both costs and the new pool.
    fn round(&self, order: &[*mut u8]) -> (Cost, Cost, Vec<*mut u8>) {
        let n = order.len();
        let launder = self.cost(n, || self.give(order));
        let mut pool = Vec::with_capacity(n);
        let fill = self.cost(n, || self.take(n, &mut pool));
        (launder, fill, pool)
    }

    /// Launders one object in `every` of the pool (untimed), times filling exactly those back,
    /// and returns the cost.
    fn sparse_fill(&self, pool: &mut [*mut u8], every: usize) -> Cost {
        let freed: Vec<*mut u8> = pool.iter().step_by(every).copied().collect();
        self.give(&freed);
        let mut got = Vec::with_capacity(freed.len());
        let cost = self.cost(freed.len(), || self.take(freed.len(), &mut got));
        for (slot, object) in pool.iter_mut().step_by(every).zip(got) {
            *slot = object;
        }
        cost
    }
}

/// Laundering scattered magazines out of a class whose live bitmap is `bitmap` bytes, so that
/// the bitmap fits a chosen cache level or not. The class's first `8 * bitmap` slots are
/// carved and held (their memory is never touched, so only the bitmap is resident); then each
/// round launders `MAGAZINES` magazines of slots in `order`, timed, and fills them back
/// untimed. The slots are made, not stored, so the bench holds no pool as large as the bitmap.
fn bitmap_sweep(bench: &Bench, bitmap: usize, order: &str, rounds: usize) -> Cost {
    const MAGAZINES: usize = 64;
    let slots = bitmap * 8;
    let words = slots / 64;
    assert!(words.is_power_of_two() && words >= 64, "bitmap must be a power of two of at least 512 bytes");
    let mut magazine = vec![std::ptr::null_mut::<u8>(); bench.batch];
    let mut base = None;
    let mut carved = 0;
    while carved < slots {
        let want = (slots - carved).min(bench.batch);
        let got = bench.back.fill(bench.class, &mut Descending(&atomic(&mut magazine)[..want]));
        assert_eq!(got, want, "the back end ran out of memory");
        let lowest = magazine[..want].iter().copied().min().unwrap();
        base = Some(base.map_or(lowest, |b: *mut u8| b.min(lowest)));
        carved += want;
    }
    let base = base.unwrap();
    let size = bench.class.class().size();
    let feistel = Feistel::new(slots, 7);
    let slot_at = |j: usize| -> usize {
        match order {
            // Consecutive slots 64 words apart, every word and summary word met in turn.
            "stride" => {
                let (bit, w) = (j / words, j % words);
                (w * 64 % words + w * 64 / words) * 64 + bit
            }
            "scatter" => feistel.permute(j),
            _ => panic!("order {order} is not scatter or stride for a bitmap sweep"),
        }
    };
    let per_round = MAGAZINES * bench.batch;
    let mut best = Cost::default();
    let mut j = 0;
    let mut objects = vec![std::ptr::null_mut::<u8>(); per_round];
    let mut back_again = Vec::with_capacity(per_round);
    for round in 0..=rounds {
        for o in objects.iter_mut() {
            *o = base.wrapping_byte_add(slot_at(j % slots) * size);
            j += 1;
        }
        let cost = bench.cost(per_round, || bench.give(&objects));
        bench.take(per_round, &mut back_again);
        if round > 0 {
            best = best.better(cost);
        }
    }
    best
}

/// A pseudo-random permutation of `0..n`: a Feistel network on the next power of four, walked
/// until it lands below `n`.
struct Feistel {
    n: usize,
    half: u32,
    keys: [u64; 4],
}

impl Feistel {
    fn new(n: usize, seed: u64) -> Feistel {
        let bits = (usize::BITS - (n - 1).leading_zeros()).max(2);
        let half = bits.div_ceil(2);
        let mut rng = Rng::new(seed);
        Feistel { n, half, keys: [rng.next_u64(), rng.next_u64(), rng.next_u64(), rng.next_u64()] }
    }

    fn permute(&self, i: usize) -> usize {
        let mask = (1u64 << self.half) - 1;
        let mut x = i as u64;
        loop {
            let (mut l, mut r) = (x >> self.half, x & mask);
            for k in self.keys {
                let f = (r ^ k).wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 32 & mask;
                (l, r) = (r, l ^ f);
            }
            x = l << self.half | r;
            if (x as usize) < self.n {
                return x as usize;
            }
        }
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
        "spread" => {
            shuffle(&mut v, rng);
            v.chunks_mut(254).for_each(|magazine| magazine.sort());
        }
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

/// A buffer of pointers seen as a magazine's atomic slots, which the back end reads and writes
/// (plain copies in and out of the buffer stay cheap, as the front end's are).
fn atomic(v: &mut [*mut u8]) -> &[AtomicPtr<u8>] {
    // SAFETY: `AtomicPtr<u8>` has the size and alignment of `*mut u8`, and the exclusive borrow
    // means nothing else reads or writes the buffer while the view lives.
    unsafe { std::slice::from_raw_parts(v.as_mut_ptr().cast::<AtomicPtr<u8>>(), v.len()) }
}

/// `16K`, `4M` and the like, in bytes.
fn parse_size(s: &str) -> usize {
    let (num, shift) = match s.as_bytes().last() {
        Some(b'K') => (&s[..s.len() - 1], 10),
        Some(b'M') => (&s[..s.len() - 1], 20),
        _ => (s, 0),
    };
    num.parse::<usize>().expect("a size") << shift
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
    let batch = args.str("batch", "mag");
    let orders = args.str("orders", "asc,desc,two,block,scatter,stride");
    let fills = args.str("fills", "fill,fill/8,fill/64");
    let back = STATE.get_or_reserve().expect("reserve the heap");
    let counters = Counters::region();
    let bitmaps = args.str("bitmaps", "");
    if !bitmaps.is_empty() {
        // Each size carves on top of the last in the one class, so sizes go smallest first.
        let size = classes[0];
        let class = Class::of_size(size).and_then(Class::small).expect("a small class size");
        let bench = Bench { back, class, batch: class.magazine().capacity(), counters: &counters };
        for bitmap in bitmaps.split(',') {
            let bytes = parse_size(bitmap);
            for name in orders.split(',').filter(|name| !name.is_empty()) {
                let cost = bitmap_sweep(&bench, bytes, name, reps);
                let extra: Vec<String> = cost
                    .events
                    .iter()
                    .filter(|(n, _)| !matches!(*n, "cycles" | "instructions" | "cycles_user"))
                    .map(|(n, v)| format!("{n}={v:.3}"))
                    .collect();
                println!(
                    "{size:>6} {bitmap:>8} {name:>8}  {:>9.1} {:>9.1}  {}",
                    cost.cycles,
                    cost.instructions,
                    extra.join(" ")
                );
            }
        }
        return;
    }
    println!("{:>6} {:>8} {:>7}  {:>9} {:>9}", "class", "case", "objects", "cyc/obj", "ins/obj");
    for size in classes {
        let class = Class::of_size(size).and_then(Class::small).expect("a small class size");
        let batch = if batch == "mag" { class.magazine().capacity() } else { batch.parse().expect("batch=N or mag") };
        let bench = Bench { back, class, batch, counters: &counters };
        let n = (POOL_BYTES / class.size()).min(MAX_POOL);
        let mut pool = Vec::with_capacity(n);
        bench.take(n, &mut pool);
        pool.sort();
        let mut rng = Rng::new(size as u64);
        let mut dense_fill = Cost::default();
        for name in orders.split(',').filter(|name| !name.is_empty()) {
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
                "fill" => dense_fill.clone(),
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
