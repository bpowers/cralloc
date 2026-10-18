//! Single-threaded, allocation-heavy shapes. All run on the first CPU of `BENCH_CPUS` and rank
//! allocators by cycles (user and kernel, so page faults count) per operation.
//!
//! - `churn`: `live` objects; each step frees a random one and allocates a replacement of a
//!   random size, `ops` times. From `live=100` (all in L1) to `live=10M` (DRAM).
//! - `largepair`: allocate a block of each of `sizes` bytes, write its first word, free it;
//!   `ops` times. The cost of one large allocation and free, with no other work.
//! - `map`: build a `BTreeMap` or `HashMap` of `n` entries with `String` values, drop it;
//!   `reps` times. Also reports how much memory is resident right after the last drop and
//!   after `idle_ms` of idleness.
//! - `tree`: build a complete binary tree of `Box` nodes `depth` levels deep, walk it, drop
//!   it; `reps` times.
//! - `vecgrow`: grow `vecs` vectors of `u64`, round-robin, until each holds `max` bytes, so
//!   they double by `realloc` all the way, as pushing would; then drop them; `reps` times.
//!   With `moves=1`, one more repetition after the measured phase counts the moves and the
//!   bytes they copied.
//! - `json`: parse a generated `size`-byte JSON document into `serde_json::Value` and drop
//!   it, `reps` times.
use bench_harness::mem::rss_now;
use bench_harness::report::{Measure, Phase};
use bench_harness::{Args, Better, Counters, Cpus, Report, Rng, SizeMix};
use std::alloc::{Layout, alloc, dealloc};
use std::collections::{BTreeMap, HashMap};
use std::hint::black_box;

fn pin_main() -> Cpus {
    let cpus = Cpus::from_env();
    cpus.pin(0);
    // Opens the FIFOs outside every measured phase.
    perf_control();
    cpus
}

fn cycles(p: &Phase) -> f64 {
    p.counters.get("cycles").expect("the cycles counter could not be read") as f64
}

/// Adds `b`'s counters and time into `a`.
fn accumulate(a: &mut Option<Phase>, b: Phase) {
    match a {
        None => *a = Some(b),
        Some(a) => {
            a.wall_ns += b.wall_ns;
            for (n, v) in a.counters.values.iter_mut() {
                *v += b.counters.get(n).unwrap_or(0);
            }
            a.counters.multiplexed |= b.counters.multiplexed;
        }
    }
}

pub fn churn(args: &Args) {
    pin_main();
    let live = args.usize("live", 100_000);
    let ops = args.num("ops", 20_000_000);
    let s = args.str("sizes", "small");
    let mix = SizeMix::parse(&s).unwrap_or_else(|| panic!("sizes={s} is not a size mix"));
    let counters = Counters::region();
    let mut rng = Rng::new(7);
    let lay = |n: usize| unsafe { Layout::from_size_align_unchecked(n, 8) };
    // The index of live objects lives outside the measured phases' allocations.
    let mut objs: Vec<(*mut u8, u32)> = Vec::with_capacity(live);
    let mut live_bytes = 0u64;

    let m = Measure::start(&counters);
    for i in 0..live {
        let n = mix.sample(&mut rng);
        // SAFETY: non-zero size; freed below with the same layout.
        let p = unsafe { alloc(lay(n)) };
        unsafe { p.cast::<u64>().write(i as u64) };
        objs.push((p, n as u32));
        live_bytes += n as u64;
    }
    let fill = m.stop();

    let m = Measure::start(&counters);
    let mut acc = 0u64;
    for _ in 0..ops {
        let r = rng.next_u64();
        let i = ((r as u128 * live as u128) >> 64) as usize;
        let (p, n) = objs[i];
        // SAFETY: p was allocated with this layout and is live.
        unsafe { dealloc(p, lay(n as usize)) };
        let n2 = mix.sample(&mut rng);
        let q = unsafe { alloc(lay(n2)) };
        unsafe { q.cast::<u64>().write(r) };
        acc = acc.wrapping_add(q as u64);
        objs[i] = (q, n2 as u32);
        live_bytes = live_bytes + n2 as u64 - n as u64;
    }
    let run = m.stop();
    black_box(acc);
    let rss_live = rss_now();

    let m = Measure::start(&counters);
    for &(p, n) in &objs {
        // SAFETY: as above.
        unsafe { dealloc(p, lay(n as usize)) };
    }
    let free = m.stop();
    let rss_freed = rss_now();
    Report::new("churn", args)
        .phase("fill", &fill)
        .phase("run", &run)
        .phase("free", &free)
        .metric("ops", ops)
        .metric("live_bytes", live_bytes)
        .metric("index_bytes", (live * std::mem::size_of::<(*mut u8, u32)>()) as u64)
        .metric("rss_live", rss_live)
        .metric("rss_freed", rss_freed)
        .primary("cycles/op", cycles(&run) / ops as f64, Better::Lower)
        .emit();
}

pub fn largepair(args: &Args) {
    pin_main();
    let ops = args.num("ops", 2_000_000);
    let sizes: Vec<usize> = args
        .str("sizes", "65536,1048576")
        .split(',')
        .map(|s| s.parse().unwrap_or_else(|_| panic!("sizes={s} is not a list of byte counts")))
        .collect();
    let counters = Counters::region();
    let lay = |n: usize| unsafe { Layout::from_size_align_unchecked(n, 8) };
    let m = Measure::start(&counters);
    let mut acc = 0u64;
    for i in 0..ops {
        for &n in &sizes {
            // SAFETY: non-zero size; freed at once with the same layout, and the write is
            // inside the block.
            unsafe {
                let p = alloc(lay(n));
                p.cast::<u64>().write(i);
                acc = acc.wrapping_add(black_box(p).cast::<u64>().read());
                dealloc(p, lay(n));
            }
        }
    }
    let run = m.stop();
    black_box(acc);
    let pairs = ops * sizes.len() as u64;
    Report::new("largepair", args)
        .phase("run", &run)
        .metric("pairs", pairs)
        .primary("ns/pair", run.wall_ns as f64 / pairs as f64, Better::Lower)
        .emit();
}

/// Drops a measured structure out of line, so that a profiler can name the drop phase
/// (`valgrind --toggle-collect='*drop_phase*'`): inlined, a drop has no symbol of its own, and
/// a recursive drop function toggles collection off at every nested call.
///
/// With `BENCH_PERF_CTL=CTL,ACK` naming the control and acknowledgement FIFOs of a
/// `perf record -D -1 --control fifo:CTL,ACK`, the recording is on during drops alone, so that
/// a sampled profile of the real heap covers the drop phases. Valgrind cannot stand in: it
/// cannot map asmalloc's full reservation, and smaller regions change what a launder does.
#[inline(never)]
fn drop_phase<T>(value: T) {
    let control = perf_control();
    if let Some(control) = control {
        control.send("enable");
    }
    drop(value);
    if let Some(control) = control {
        control.send("disable");
    }
}

struct PerfControl {
    ctl: std::fs::File,
    ack: std::fs::File,
}

impl PerfControl {
    /// Sends `command` and waits for perf to acknowledge it, so that the drop starts only
    /// once the recording is on.
    fn send(&self, command: &str) {
        use std::io::{Read, Write};
        (&self.ctl).write_all(format!("{command}\n").as_bytes()).expect("write to perf's control FIFO");
        let mut reply = [0u8; 5];
        (&self.ack).read_exact(&mut reply).expect("read perf's acknowledgement");
    }
}

fn perf_control() -> Option<&'static PerfControl> {
    static CONTROL: std::sync::OnceLock<Option<PerfControl>> = std::sync::OnceLock::new();
    CONTROL
        .get_or_init(|| {
            let spec = std::env::var("BENCH_PERF_CTL").ok()?;
            let (ctl, ack) = spec.split_once(',').expect("BENCH_PERF_CTL=CTL,ACK");
            let open = |path: &str, write: bool| {
                std::fs::OpenOptions::new().read(!write).write(write).open(path).expect("open a perf FIFO")
            };
            Some(PerfControl { ctl: open(ctl, true), ack: open(ack, false) })
        })
        .as_ref()
}

pub fn map(args: &Args) {
    pin_main();
    let kind = args.str("kind", "btree");
    let n = args.usize("n", 1_000_000);
    let reps = args.usize("reps", 5);
    let idle_ms = args.num("idle_ms", 500);
    let counters = Counters::region();
    let (mut build, mut teardown) = (None, None);
    let mut rss_built = 0;
    fn value(rng: &mut Rng) -> String {
        let len = 8 + rng.below(120) as usize;
        let mut s = String::with_capacity(len);
        s.extend(std::iter::repeat_n('v', len));
        s
    }
    for r in 0..reps {
        let mut rng = Rng::new(r as u64);
        match kind.as_str() {
            "btree" => {
                let m = Measure::start(&counters);
                let mut map = BTreeMap::new();
                for _ in 0..n {
                    map.insert(rng.next_u64(), value(&mut rng));
                }
                accumulate(&mut build, m.stop());
                rss_built = rss_now();
                let m = Measure::start(&counters);
                drop_phase(black_box(map));
                accumulate(&mut teardown, m.stop());
            }
            "hash" => {
                let m = Measure::start(&counters);
                let mut map = HashMap::new();
                for _ in 0..n {
                    map.insert(rng.next_u64(), value(&mut rng));
                }
                accumulate(&mut build, m.stop());
                rss_built = rss_now();
                let m = Measure::start(&counters);
                drop(black_box(map));
                accumulate(&mut teardown, m.stop());
            }
            _ => panic!("kind={kind} is not btree or hash"),
        }
    }
    let rss_dropped = rss_now();
    std::thread::sleep(std::time::Duration::from_millis(idle_ms));
    let rss_idle = rss_now();
    let (build, teardown) = (build.unwrap(), teardown.unwrap());
    let entries = (n * reps) as f64;
    Report::new("map", args)
        .phase("build", &build)
        .phase("drop", &teardown)
        .metric("entries", entries)
        .metric("rss_built", rss_built)
        .metric("rss_dropped", rss_dropped)
        .metric("rss_idle", rss_idle)
        .primary("cycles/entry", (cycles(&build) + cycles(&teardown)) / entries, Better::Lower)
        .emit();
}

struct Node {
    left: Option<Box<Node>>,
    right: Option<Box<Node>>,
    value: u64,
}

fn build_tree(depth: u32, value: u64) -> Box<Node> {
    if depth == 0 {
        return Box::new(Node { left: None, right: None, value });
    }
    Box::new(Node {
        left: Some(build_tree(depth - 1, value * 2)),
        right: Some(build_tree(depth - 1, value * 2 + 1)),
        value,
    })
}

fn walk(n: &Node) -> u64 {
    n.value + n.left.as_deref().map_or(0, walk) + n.right.as_deref().map_or(0, walk)
}

pub fn tree(args: &Args) {
    pin_main();
    let depth = args.num("depth", 20) as u32;
    let reps = args.usize("reps", 10);
    let counters = Counters::region();
    let (mut build, mut walked, mut dropped) = (None, None, None);
    let mut acc = 0u64;
    for r in 0..reps {
        let m = Measure::start(&counters);
        let t = build_tree(depth, r as u64);
        accumulate(&mut build, m.stop());
        let m = Measure::start(&counters);
        acc = acc.wrapping_add(walk(&t));
        accumulate(&mut walked, m.stop());
        let m = Measure::start(&counters);
        drop_phase(t);
        accumulate(&mut dropped, m.stop());
    }
    let (build, walked, dropped) = (build.unwrap(), walked.unwrap(), dropped.unwrap());
    let mut run = build.clone();
    for p in [&walked, &dropped] {
        run.wall_ns += p.wall_ns;
        for (n, v) in run.counters.values.iter_mut() {
            *v += p.counters.get(n).unwrap_or(0);
        }
    }
    black_box(acc);
    let nodes = reps as u64 * ((1u64 << (depth + 1)) - 1);
    Report::new("tree", args)
        .phase("run", &run)
        .phase("build", &build)
        .phase("walk", &walked)
        .phase("drop", &dropped)
        .metric("nodes", nodes)
        .primary("cycles/node", cycles(&run) / nodes as f64, Better::Lower)
        .emit();
}

pub fn vecgrow(args: &Args) {
    pin_main();
    let vecs = args.usize("vecs", 1);
    let max = args.num("max", 256 << 20) as usize;
    let reps = args.usize("reps", 4);
    let counters = Counters::region();
    let per = max / 8;
    let m = Measure::start(&counters);
    let acc = grow_and_drop(vecs, per, reps);
    let run = m.stop();
    black_box(acc);
    let elements = (reps * vecs * per) as f64;
    let mut report = Report::new("vecgrow", args);
    report.param("chunk", GROW_CHUNK as u64).phase("run", &run).metric("elements", elements);
    // After the measured phase, so that it changes nothing the phase measures.
    if args.usize("moves", 0) != 0 {
        let moved = count_moves(vecs, per);
        report
            .metric("moves", moved.moves)
            .metric("copied_bytes", moved.copied)
            .metric("large_moves", moved.large_moves)
            .metric("large_copied_bytes", moved.large_copied);
    }
    report.primary("cycles/element", cycles(&run) / elements, Better::Lower).emit();
}

/// What one repetition of [`grow_and_drop`] moved: reallocations that returned a new address,
/// and the bytes the allocator had to copy for them (the old capacity), in all and for blocks
/// above the largest small class (32 KiB).
struct Moved {
    moves: u64,
    copied: u64,
    large_moves: u64,
    large_copied: u64,
}

/// One repetition of [`grow_and_drop`], with the same calls, that counts the moves instead of
/// timing them (`moves=1`).
#[inline(never)]
fn count_moves(vecs: usize, per: usize) -> Moved {
    let mut moved = Moved { moves: 0, copied: 0, large_moves: 0, large_copied: 0 };
    let mut vs: Vec<Vec<u64>> = (0..vecs).map(|_| Vec::new()).collect();
    let mut len = 0;
    while len < per {
        let step = len.clamp(4, GROW_CHUNK).min(per - len);
        len += step;
        for v in vs.iter_mut() {
            let (at, bytes) = (v.as_ptr(), v.capacity() * 8);
            v.resize(len, GROW_FILL);
            if bytes > 0 && v.as_ptr() != at {
                moved.moves += 1;
                moved.copied += bytes as u64;
                if bytes > 32 << 10 {
                    moved.large_moves += 1;
                    moved.large_copied += bytes as u64;
                }
            }
        }
    }
    drop(black_box(vs));
    moved
}

/// The most elements one step of [`grow_and_drop`] appends to a vector.
const GROW_CHUNK: usize = 512;

/// What [`grow_and_drop`] fills vectors with: every byte the same, so the fill is a `memset`.
const GROW_FILL: u64 = 0x5a5a_5a5a_5a5a_5a5a;

/// Grows `vecs` vectors of `u64` round-robin to `per` elements each and drops them, `reps`
/// times; returns a checksum.
///
/// Each step appends with `resize`, never more than the vector already holds nor more than 4
/// to an empty one (`Vec`'s smallest capacity for 8-byte elements), so a vector reallocates
/// exactly where one `push` per element would: at 4, 8, 16, ... elements, each time to twice
/// its capacity. The allocator sees the same calls with the same sizes.
///
/// It does not push because a push loop stores the vector's length and loads it back for
/// every element, and that chain took most of the job's time at a speed that depended on
/// where the loop's code landed: an unrelated edit to this file once made the job 40% slower,
/// and each allocator's build put the loop elsewhere (docs/research/memory.md, "The grow
/// loop's 40%"). Nor does it copy a chunk from the stack: the copy's loads then alias the
/// stores to the vector at 4 KiB strides, which made it 6% slower or faster by where the
/// stack began in each process. A fill of up to 4 KiB a step by `memset`, whose code is the
/// same in every build and which only stores, leaves the time in what the job is for: the
/// allocator's growth path, its copies, and the page faults. Out of line, so that a profile
/// names it and its code does not move with `main`'s.
#[inline(never)]
fn grow_and_drop(vecs: usize, per: usize, reps: usize) -> u64 {
    let mut acc = 0u64;
    for _ in 0..reps {
        let mut vs: Vec<Vec<u64>> = (0..vecs).map(|_| Vec::new()).collect();
        let mut len = 0;
        while len < per {
            let step = len.clamp(4, GROW_CHUNK).min(per - len);
            len += step;
            for v in vs.iter_mut() {
                v.resize(len, GROW_FILL);
            }
        }
        acc = acc.wrapping_add(vs.iter().map(|v| v[v.len() / 2]).sum::<u64>());
        drop(black_box(vs));
    }
    acc
}

/// A deterministic document of about `size` bytes: an array of user records with nested
/// objects and arrays, the shape of a typical API response.
fn generate_json(size: usize) -> String {
    use std::fmt::Write;
    let mut rng = Rng::new(42);
    let mut s = String::with_capacity(size + 4096);
    s.push('[');
    let mut i = 0u64;
    while s.len() < size {
        if i > 0 {
            s.push(',');
        }
        let tags: Vec<String> = (0..rng.below(6)).map(|t| format!("\"tag{}\"", rng.below(50) + t)).collect();
        let history: Vec<String> = (0..rng.below(4))
            .map(|_| {
                format!(
                    "{{\"ts\":{},\"event\":\"evt-{}\",\"ok\":{}}}",
                    rng.next_u64() >> 20,
                    rng.below(1000),
                    rng.below(2) == 0
                )
            })
            .collect();
        write!(
            s,
            "{{\"id\":{i},\"name\":\"user_{}\",\"email\":\"user{}@example.com\",\"active\":{},\"score\":{:.4},\
             \"tags\":[{}],\"address\":{{\"street\":\"{} Main St\",\"city\":\"City{}\",\"zip\":\"{:05}\"}},\
             \"history\":[{}]}}",
            rng.next_u64() % 100_000,
            i,
            rng.below(2) == 0,
            rng.unit() * 100.0,
            tags.join(","),
            rng.below(9999),
            rng.below(500),
            rng.below(99_999),
            history.join(",")
        )
        .unwrap();
        i += 1;
    }
    s.push(']');
    s
}

pub fn json(args: &Args) {
    pin_main();
    let size = args.num("size", 32 << 20) as usize;
    let reps = args.usize("reps", 5);
    let doc = generate_json(size);
    let counters = Counters::region();
    let (mut parse, mut teardown) = (None, None);
    for _ in 0..reps {
        let m = Measure::start(&counters);
        let v: serde_json::Value = serde_json::from_str(&doc).unwrap();
        accumulate(&mut parse, m.stop());
        let m = Measure::start(&counters);
        drop_phase(black_box(v));
        accumulate(&mut teardown, m.stop());
    }
    let (parse, teardown) = (parse.unwrap(), teardown.unwrap());
    let bytes = (doc.len() * reps) as f64;
    Report::new("json", args)
        .phase("parse", &parse)
        .phase("drop", &teardown)
        .metric("bytes", bytes)
        .primary("cycles/byte", (cycles(&parse) + cycles(&teardown)) / bytes, Better::Lower)
        .emit();
}
