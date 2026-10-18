//! The jobs: what to run, where, and which number ranks the allocators.
//!
//! Measurement CPUs on the reference machine are 0-3 (one L3 domain) and 8-11 (the other),
//! one thread per physical core; their SMT siblings (16-19, 24-27) are left idle.

/// The two L3 domains' measurement cores.
pub const L3A: [usize; 4] = [0, 1, 2, 3];
pub const L3B: [usize; 4] = [8, 9, 10, 11];
/// The core single-threaded jobs use.
pub const SINGLE: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// A Rust binary built with the allocator as `#[global_allocator]`.
    Native,
    /// The program run with the allocator's library in LD_PRELOAD.
    Preload,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Native => "native",
            Mode::Preload => "preload",
        }
    }
}

#[derive(Clone, Debug)]
pub enum Program {
    /// A binary of the bench workspace (`workload`, `rpc-server`), from the build for the
    /// allocator (native) or the glibc build (preload).
    Rust(&'static str),
    /// A C benchmark program from bench/baselines/out/cbench.
    C(&'static str),
}

/// Which number decides the ranking.
#[derive(Clone, Debug)]
pub enum Metric {
    /// The `primary` of the workload's `BENCH_RESULT`.
    Reported,
    /// For a server: its cycles (all threads, user and kernel) per request completed over
    /// the driver's measurement window. The driver's request rate is recorded too, but the
    /// driver, on as many CPUs as the server, can be the bottleneck, so the rate is not the
    /// ranking.
    ServerCycles,
    /// The process's wall time in seconds; lower is better.
    Wall,
    /// The number after `key` in the program's stdout, scaled by `scale`.
    Parsed { key: &'static str, name: &'static str, higher_better: bool, scale: f64 },
}

/// A load generator started against a server job; its allocator never changes.
#[derive(Clone, Debug)]
pub struct Driver {
    pub bin: &'static str,
    /// `{addr}` is replaced by the server's address.
    pub args: Vec<String>,
    pub cpus: Vec<usize>,
}

#[derive(Clone, Debug)]
pub struct Job {
    /// Stable identifier: results are grouped by it.
    pub id: String,
    pub group: &'static str,
    pub mode: Mode,
    pub program: Program,
    pub args: Vec<String>,
    /// Affinity of the measured process; also its `BENCH_CPUS`, in this order.
    pub cpus: Vec<usize>,
    pub driver: Option<Driver>,
    pub metric: Metric,
    pub timeout_s: u64,
}

fn local(n: usize) -> Vec<usize> {
    L3A[..n].to_vec()
}

/// Alternating between the two L3 domains, so that every neighbour is across domains.
fn split(n: usize) -> Vec<usize> {
    L3A.iter().zip(L3B.iter()).flat_map(|(&a, &b)| [a, b]).take(n).collect()
}

/// Both domains, one after the other.
fn both() -> Vec<usize> {
    L3A.iter().chain(L3B.iter()).copied().collect()
}

fn args(s: &str) -> Vec<String> {
    s.split_whitespace().map(String::from).collect()
}

fn rust(group: &'static str, id: String, mode: Mode, a: &str, cpus: Vec<usize>, timeout_s: u64) -> Job {
    Job {
        id,
        group,
        mode,
        program: Program::Rust("workload"),
        args: args(a),
        cpus,
        driver: None,
        metric: Metric::Reported,
        timeout_s,
    }
}

fn cprog(id: &str, prog: &'static str, a: &str, cpus: Vec<usize>, metric: Metric, timeout_s: u64) -> Job {
    Job {
        id: id.to_string(),
        group: "mimalloc-bench",
        mode: Mode::Preload,
        program: Program::C(prog),
        args: args(a),
        cpus,
        driver: None,
        metric,
        timeout_s,
    }
}

fn rpc(service: &str, label: &str, client_args: &str) -> Job {
    Job {
        id: format!("rpc {service} {label}"),
        group: "rpc",
        mode: Mode::Native,
        program: Program::Rust("rpc-server"),
        args: vec![service.to_string()],
        cpus: L3A.to_vec(),
        driver: Some(Driver {
            bin: "rpc-client",
            args: [service.to_string(), "addr={addr}".to_string()].into_iter().chain(args(client_args)).collect(),
            cpus: L3B.to_vec(),
        }),
        metric: Metric::ServerCycles,
        timeout_s: 120,
    }
}

const LARSON: Metric = Metric::Parsed { key: "Throughput =", name: "Mops/s", higher_better: true, scale: 1e-6 };
const XMALLOC: Metric = Metric::Parsed { key: "free/sec:", name: "Mfrees/s", higher_better: true, scale: 1.0 };

/// Whether the threads that only free (a pipeline's last stage, mpmc's consumers) allocate
/// once before the work. Jobs that have such threads carry the choice in their id, so that
/// neither kind can stand in for the other in a report.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Consumers {
    /// Every thread allocates once first, as a service's threads do: the primary jobs.
    AllAlloc,
    /// The consumers never allocate, as a dedicated drop thread does.
    FreeOnly,
}

impl Consumers {
    fn arg(self) -> &'static str {
        match self {
            Consumers::AllAlloc => "prime=yes",
            Consumers::FreeOnly => "prime=no",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Consumers::AllAlloc => "all-alloc",
            Consumers::FreeOnly => "free-only",
        }
    }
}

/// CPUs for `chains` two-thread pipelines, each on two adjacent cores of one L3 domain,
/// filling domain A before B: 0,1 then 2,3 then 8,9 then 10,11.
fn pairs(chains: usize) -> Vec<usize> {
    both()[..2 * chains].to_vec()
}

/// A pipeline or mpmc job; `id` and `args` get the consumer choice added.
fn xjob(id: String, a: &str, cpus: Vec<usize>, c: Consumers, timeout_s: u64) -> Job {
    rust("cross-thread", format!("{id} {}", c.label()), Mode::Native, &format!("{a} {}", c.arg()), cpus, timeout_s)
}

/// Every workload, at full size.
pub fn full() -> Vec<Job> {
    use Consumers::*;
    use Mode::*;
    let mut v = Vec::new();
    let place_cpus = |threads: usize, place: &str| match place {
        "local" => local(threads),
        "split" => split(threads),
        _ => both(),
    };
    let pipe = |threads: usize, extra: &str, place: &str, c: Consumers| {
        let a = format!("pipeline threads={threads} {extra}");
        xjob(format!("pipeline t{threads} {extra} {place}"), &a, place_cpus(threads, place), c, 300)
    };
    let exch = |threads: usize, extra: &str, place: &str| {
        let a = format!("exchange threads={threads} {extra}");
        rust(
            "cross-thread",
            format!("exchange t{threads} {extra} {place}"),
            Native,
            &a,
            place_cpus(threads, place),
            300,
        )
    };
    for sizes in ["s64", "small", "mixed"] {
        v.push(pipe(2, &format!("sizes={sizes}"), "local", AllAlloc));
    }
    v.push(pipe(2, "sizes=small", "local", FreeOnly));
    v.push(pipe(2, "sizes=large objects=1M inflight=512", "local", AllAlloc));
    v.push(pipe(2, "sizes=small", "split", AllAlloc));
    v.push(pipe(2, "sizes=small", "split", FreeOnly));
    v.push(pipe(4, "sizes=small", "local", AllAlloc));
    v.push(pipe(8, "sizes=small", "both", AllAlloc));
    // Independent two-thread pipelines side by side: two in one L3 domain, two with one in
    // each, three, and four. Per chain, they should cost what one does alone.
    for (chains, cpus, place) in
        [(2, local(4), "local"), (2, vec![0, 1, 8, 9], "split"), (3, pairs(3), "both"), (4, pairs(4), "both")]
    {
        let a = format!("pipeline threads=2 chains={chains} sizes=small");
        v.push(xjob(format!("pipeline t2 x{chains} sizes=small {place}"), &a, cpus, AllAlloc, 300));
    }
    for sizes in ["small", "mixed"] {
        v.push(exch(2, &format!("sizes={sizes}"), "local"));
    }
    v.push(exch(2, "sizes=small", "split"));
    v.push(exch(4, "sizes=small", "local"));
    v.push(exch(8, "sizes=small", "both"));
    for (p, c, place, kinds) in [
        (1, 3, "local", &[AllAlloc, FreeOnly][..]),
        (3, 1, "local", &[AllAlloc, FreeOnly]),
        (4, 4, "both", &[AllAlloc]),
    ] {
        let cpus = if place == "local" { local(p + c) } else { both() };
        let a = format!("mpmc producers={p} consumers={c} sizes=small");
        for &k in kinds {
            v.push(xjob(format!("mpmc p{p} c{c} sizes=small {place}"), &a, cpus.clone(), k, 300));
        }
    }
    for (t, place) in [(4, "local"), (8, "both")] {
        let cpus = if place == "local" { local(t) } else { both() };
        let a = format!("forkjoin threads={t} sizes=small");
        v.push(rust("work-stealing", format!("forkjoin t{t} {place}"), Native, &a, cpus.clone(), 300));
        let a = format!("spawn threads={t} rounds=20000 sizes=small");
        v.push(rust("work-stealing", format!("spawn t{t} {place}"), Native, &a, cpus, 300));
    }
    v.push(rpc("echo", "c64", "concurrency=64 warmup_ms=2000 measure_ms=5000"));
    v.push(rpc("log", "c64", "concurrency=64 conns=8 warmup_ms=2000 measure_ms=5000"));
    v.push(rpc("log", "c256", "concurrency=256 conns=8 warmup_ms=2000 measure_ms=5000"));
    let one = || vec![SINGLE];
    for live in ["100", "1k", "10k", "100k", "1M", "10M"] {
        let a = format!("churn live={live} ops=20M sizes=small");
        v.push(rust("single-thread", format!("churn live={live} small"), Native, &a, one(), 300));
    }
    v.push(rust(
        "single-thread",
        "churn live=100k mixed".into(),
        Native,
        "churn live=100k ops=20M sizes=mixed",
        one(),
        300,
    ));
    v.push(rust("single-thread", "map btree 1M".into(), Native, "map kind=btree n=1M reps=5", one(), 300));
    v.push(rust("single-thread", "map hash 1M".into(), Native, "map kind=hash n=1M reps=5", one(), 300));
    v.push(rust("single-thread", "tree depth=20".into(), Native, "tree depth=20 reps=10", one(), 300));
    v.push(rust("single-thread", "vecgrow 1x256Mi".into(), Native, "vecgrow vecs=1 max=256Mi reps=4", one(), 300));
    v.push(rust("single-thread", "vecgrow 64x4Mi".into(), Native, "vecgrow vecs=64 max=4Mi reps=4", one(), 300));
    v.push(rust("single-thread", "json 32Mi".into(), Native, "json size=32Mi reps=5", one(), 300));
    // The same Rust programs through LD_PRELOAD, where every free is C's unsized free().
    v.push(rust("preload", "churn live=100k small".into(), Preload, "churn live=100k ops=20M sizes=small", one(), 300));
    v.push(rust(
        "preload",
        "pipeline t2 sizes=small local all-alloc".into(),
        Preload,
        "pipeline threads=2 sizes=small prime=yes",
        local(2),
        300,
    ));
    v.push(rust("preload", "json 32Mi".into(), Preload, "json size=32Mi reps=5", one(), 300));
    // mimalloc-bench, with the arguments its bench.sh uses for 8 CPUs.
    v.push(cprog("larson-sized 8", "larson-sized", "3 8 1000 5000 100 4141 8", both(), LARSON, 120));
    v.push(cprog("larson 8", "larson", "3 8 1000 5000 100 4141 8", both(), LARSON, 120));
    v.push(cprog("xmalloc-test 4+4", "xmalloc-test", "-w 4 -t 3 -s 64", both(), XMALLOC, 120));
    v.push(cprog("sh8bench 16", "sh8bench", "16", both(), Metric::Wall, 300));
    v.push(cprog("mstress 8", "mstress", "8 50 25", both(), Metric::Wall, 300));
    v.push(cprog("cache-scratch 1", "cache-scratch", "1 1000 1 2000000 8", both(), Metric::Wall, 300));
    v.push(cprog("cache-scratch 8", "cache-scratch", "8 1000 1 2000000 8", both(), Metric::Wall, 300));
    v.push(cprog("cfrac", "cfrac", "17545186520507317056371138836327483792789528", one(), Metric::Wall, 300));
    v.push(cprog("espresso", "espresso", "{cbench}/largest.espresso", one(), Metric::Wall, 300));
    v
}

/// A cross-section of the suite at small sizes, for a check that finishes in a few minutes.
pub fn quick() -> Vec<Job> {
    use Consumers::*;
    use Mode::*;
    let one = || vec![SINGLE];
    let pipe =
        |c| xjob("pipeline t2 sizes=small local".into(), "pipeline threads=2 sizes=small objects=2M", local(2), c, 60);
    vec![
        pipe(AllAlloc),
        pipe(FreeOnly),
        xjob(
            "pipeline t2 x3 sizes=small both".into(),
            "pipeline threads=2 chains=3 sizes=small objects=2M",
            pairs(3),
            AllAlloc,
            60,
        ),
        rust(
            "cross-thread",
            "exchange t2 sizes=small split".into(),
            Native,
            "exchange threads=2 sizes=small objects=2M",
            split(2),
            60,
        ),
        xjob(
            "mpmc p3 c1 sizes=small local".into(),
            "mpmc producers=3 consumers=1 sizes=small objects=2M",
            local(4),
            AllAlloc,
            60,
        ),
        rust(
            "work-stealing",
            "forkjoin t4 local".into(),
            Native,
            "forkjoin threads=4 sizes=small reps=10",
            local(4),
            60,
        ),
        rust("work-stealing", "spawn t4 local".into(), Native, "spawn threads=4 rounds=4000 sizes=small", local(4), 60),
        rpc("log", "c64", "concurrency=64 conns=8 warmup_ms=500 measure_ms=1500"),
        rust("single-thread", "churn live=1k small".into(), Native, "churn live=1k ops=5M sizes=small", one(), 60),
        rust("single-thread", "churn live=1M small".into(), Native, "churn live=1M ops=5M sizes=small", one(), 60),
        rust("single-thread", "map btree 200k".into(), Native, "map kind=btree n=200k reps=2 idle_ms=200", one(), 60),
        rust("single-thread", "vecgrow 1x64Mi".into(), Native, "vecgrow vecs=1 max=64Mi reps=2", one(), 60),
        rust("single-thread", "json 8Mi".into(), Native, "json size=8Mi reps=2", one(), 60),
        cprog("larson-sized 8", "larson-sized", "1 8 1000 5000 100 4141 8", both(), LARSON, 60),
        cprog("xmalloc-test 4+4", "xmalloc-test", "-w 4 -t 1 -s 64", both(), XMALLOC, 60),
        cprog("cache-scratch 8", "cache-scratch", "8 1000 1 500000 8", both(), Metric::Wall, 60),
    ]
}

/// The shapes that exposed the back end's worst costs in the 2026-10-18 session
/// (docs/research/baselines.md, "Smoke set for the back end"), small enough to run on every
/// change to the back end in about a minute: large objects crossing threads (a purge on every
/// free past the large cap), a tree built and dropped (every object through the bitmaps,
/// recycled out of address order), sh8bench preloaded (contended refills), JSON (the
/// small-class cap purging what the next parse faults back), and two independent pipelines
/// on different L3 domains (objects handed across domains by the shared depot).
pub fn backend() -> Vec<Job> {
    use Consumers::*;
    use Mode::*;
    let one = || vec![SINGLE];
    vec![
        xjob(
            "pipeline t2 sizes=large local".into(),
            "pipeline threads=2 sizes=large objects=300k inflight=512",
            local(2),
            AllAlloc,
            60,
        ),
        xjob(
            "pipeline t2 x2 sizes=small split".into(),
            "pipeline threads=2 chains=2 sizes=small objects=2M",
            vec![0, 1, 8, 9],
            AllAlloc,
            60,
        ),
        rust("single-thread", "tree depth=18".into(), Native, "tree depth=18 reps=4", one(), 60),
        rust("single-thread", "json 8Mi".into(), Native, "json size=8Mi reps=3", one(), 60),
        cprog("sh8bench 16", "sh8bench", "16", both(), Metric::Wall, 60),
    ]
}

/// Every CPU any job may use.
pub fn measurement_cpus() -> Vec<usize> {
    both()
}
