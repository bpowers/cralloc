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

/// Every workload, at full size.
pub fn full() -> Vec<Job> {
    use Mode::*;
    let mut v = Vec::new();
    let x = |name: &str, threads: usize, extra: &str, place: &str| {
        let cpus = match place {
            "local" => local(threads),
            "split" => split(threads),
            _ => both(),
        };
        let a = format!("{name} threads={threads} {extra}");
        rust("cross-thread", format!("{name} t{threads} {extra} {place}"), Native, &a, cpus, 300)
    };
    for sizes in ["s64", "small", "mixed"] {
        v.push(x("pipeline", 2, &format!("sizes={sizes}"), "local"));
    }
    v.push(x("pipeline", 2, "sizes=large objects=1M inflight=512", "local"));
    v.push(x("pipeline", 2, "sizes=small", "split"));
    v.push(x("pipeline", 4, "sizes=small", "local"));
    v.push(x("pipeline", 8, "sizes=small", "both"));
    for sizes in ["small", "mixed"] {
        v.push(x("exchange", 2, &format!("sizes={sizes}"), "local"));
    }
    v.push(x("exchange", 2, "sizes=small", "split"));
    v.push(x("exchange", 4, "sizes=small", "local"));
    v.push(x("exchange", 8, "sizes=small", "both"));
    for (p, c, place) in [(1, 3, "local"), (3, 1, "local"), (4, 4, "both")] {
        let cpus = if place == "local" { local(p + c) } else { both() };
        let a = format!("mpmc producers={p} consumers={c} sizes=small");
        v.push(rust("cross-thread", format!("mpmc p{p} c{c} sizes=small {place}"), Native, &a, cpus, 300));
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
        "pipeline t2 sizes=small local".into(),
        Preload,
        "pipeline threads=2 sizes=small",
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
    use Mode::*;
    let one = || vec![SINGLE];
    vec![
        rust(
            "cross-thread",
            "pipeline t2 sizes=small local".into(),
            Native,
            "pipeline threads=2 sizes=small objects=2M",
            local(2),
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
        rust(
            "cross-thread",
            "mpmc p3 c1 sizes=small local".into(),
            Native,
            "mpmc producers=3 consumers=1 sizes=small objects=2M",
            local(4),
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

/// Every CPU any job may use.
pub fn measurement_cpus() -> Vec<usize> {
    both()
}
