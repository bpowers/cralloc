//! The jobs: what to run, where, and which number ranks the allocators.
//!
//! Measurement CPUs on the reference machine are 0-3 (one L3 domain) and 8-11 (the other),
//! one thread per physical core; their SMT siblings (16-19, 24-27) are left idle.

/// The two L3 domains' measurement cores.
pub const L3A: [usize; 4] = [0, 1, 2, 3];
pub const L3B: [usize; 4] = [8, 9, 10, 11];
/// The core single-threaded jobs use.
pub const SINGLE: usize = 2;
/// The CPUs every server job runs on, its load generator on the other domain.
/// `scripts/deep-idle` reads this line, and the constant it names, for its default CPUs.
pub const SERVER_CPUS: [usize; 4] = L3A;

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
    /// A real program's driver, bench/apps/<name>, built once per allocator by `benchrun
    /// build-apps`; native runs only. `{inputs}` in its arguments is the program's input
    /// directory, which `bench/apps/fetch` fills.
    App(&'static str),
}

impl Program {
    /// The program of the apps suite this job runs, if it is one.
    pub fn app(&self) -> Option<&'static str> {
        match self {
            Program::App(name) => Some(name),
            _ => None,
        }
    }
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
    /// For a server under an open load: its cycles (all threads, user and kernel) per
    /// request over the load's ranking group, at a fixed request rate.
    Served,
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

/// A server job's load: the suite's open-loop generator (bench/probes/loadgen, built with
/// glibc by `build-apps`) on CPUs of its own, sending one request on a plan of segments. The
/// runner snapshots the server's counters at every segment the generator marks, samples its
/// memory throughout, and stops it when the generator is done.
#[derive(Clone, Debug)]
pub struct OpenLoad {
    pub path: &'static str,
    pub content_type: &'static str,
    pub headers: Vec<&'static str>,
    /// The request body's file; `{inputs}` and `{app}` are replaced as in arguments.
    pub body: &'static str,
    pub conns: usize,
    pub threads: usize,
    pub cpus: Vec<usize>,
    /// loadgen's plan, `NAME:KIND:SECS,...`.
    pub plan: String,
    /// The segment group whose server cycles per request rank the allocators.
    pub rank: &'static str,
    /// The directory under the apps inputs that `{inputs}` names, for a server that is not a
    /// program of the apps suite.
    pub inputs: Option<&'static str>,
    /// The generator's protocol: `http` (HTTP/1.1), `h2` (HTTP/2, prior knowledge, `conns`
    /// connections of `streams` concurrent streams each) or `resp`.
    pub proto: &'static str,
    pub streams: usize,
    /// For a protocol other than HTTP, the file of commands it sends in turn (`{inputs}` and
    /// `{app}` replaced); the HTTP fields above are then unused.
    pub requests: Option<&'static str>,
    /// For a proxy: the server it forwards to, started first. `{upstream}` in the job's
    /// arguments is replaced by its address.
    pub upstream: Option<Upstream>,
    /// A directory in RAM (on /dev/shm) for the run's data, so that a server whose data lives
    /// in files does not wait on the disk: `{ram}` in the job's arguments names it.
    pub ram: Option<RamDir>,
}

/// A run's directory on /dev/shm (`crate::run::RamGuard`): made before the server starts,
/// holding a symbolic link `link` to `target` (`{inputs}` replaced), and removed however the
/// run ends. The run waits for nothing: with less than `need_gib` of the machine's memory
/// available it does not start and is recorded as invalid. Qdrant's runs copy the collection
/// there, so that searches fault from memory and not from the disk.
#[derive(Clone, Debug)]
pub struct RamDir {
    pub link: &'static str,
    pub target: &'static str,
    pub need_gib: u64,
}

/// The upstream of a proxy job: a binary built with glibc, never the program under test, run
/// on CPUs of its own in the generator's cgroup, which prints `BENCH_LISTEN addr` when it is
/// ready and runs until killed.
#[derive(Clone, Debug)]
pub struct Upstream {
    pub bin: &'static str,
    pub args: Vec<&'static str>,
    pub cpus: Vec<usize>,
    /// The cargo workspace under bench/ the binary is built from: bench/probes/loadgen's, with
    /// the generator, when `None`; else one of its own, which `build-apps` builds with glibc
    /// into `target/upstream/<bin>` (`crate::apps::upstream_bin`).
    pub workspace: Option<&'static str>,
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
    pub load: Option<OpenLoad>,
    pub metric: Metric,
    pub timeout_s: u64,
}

impl Job {
    /// The directory under the apps inputs that `{inputs}` names: the program's own, or the
    /// one its load names.
    pub fn inputs(&self) -> Option<&'static str> {
        self.program.app().or_else(|| self.load.as_ref().and_then(|l| l.inputs))
    }
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
        load: None,
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
        load: None,
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
        load: None,
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
    v.push(rust("single-thread", "largepair 64Ki+1Mi".into(), Native, "largepair ops=2M", one(), 120));
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
        // A buffer grown by doubling and freed, over and over: the large path's moves, kept
        // slots and their charge (docs/research/memory.md, "Grown blocks kept up to a budget").
        rust(
            "single-thread",
            "vecgrow 1x4Mi grow-and-free".into(),
            Native,
            "vecgrow vecs=1 max=4Mi reps=1000",
            one(),
            60,
        ),
        rust(
            "single-thread",
            "vecgrow 1x1Mi grow-and-free".into(),
            Native,
            "vecgrow vecs=1 max=1Mi reps=4000",
            one(),
            60,
        ),
    ]
}

fn app(id: &str, program: &'static str, a: &str, cpus: Vec<usize>, timeout_s: u64) -> Job {
    Job {
        id: id.to_string(),
        group: "apps",
        mode: Mode::Native,
        program: Program::App(program),
        args: args(a),
        cpus,
        driver: None,
        load: None,
        metric: Metric::Reported,
        timeout_s,
    }
}

/// Real Rust programs on pinned inputs, each with the allocator as its global allocator
/// (bench/apps/README.md). Each ranks by the cycles of all its threads over its work.
pub fn apps() -> Vec<Job> {
    // Each V8 benchmark gets about the same time, as base.js's one second each gives them:
    // 0.6 s each with the glibc build, about 8 s in all with parsing the files.
    const V8_COUNTS: &str = "Richards:90,DeltaBlue:33,Encrypt:40,Decrypt:2,RayTrace:5,Earley:35,Boyer:2,RegExp:1,\
                             Splay:170,NavierStokes:4";
    vec![
        // Home Assistant passes its own ruff release's checks: no diagnostics. Four threads,
        // as a CI runner has; on eight the check takes about a second.
        app("ruff check ha-core t4", "ruff", "check root={inputs}/ha-core expect=0", local(4), 120),
        // Scale factor 3, about 3.6 s on four threads. The queries' constants are the ones for
        // scale factor 1, which leaves query 11 with no rows here. On eight threads across
        // both L3 domains every allocator's runs split into two modes, 2.4 and 3.5 s, nearly
        // every query slower in the slow one; on one domain they agree within 1%.
        app(
            "datafusion tpch sf3 t4",
            "datafusion",
            "tpch data={inputs}/sf3 expect=4,100,10,5,5,1,4,2,175,20,0,2,43,1,1,26798,1,100,1,556,100,7",
            local(4),
            120,
        ),
        app("boa v8", "boa", &format!("v8 dir={{inputs}}/v8 counts={V8_COUNTS}"), vec![SINGLE], 120),
    ]
}

/// A server's day at two fixed request rates, about 30% and 70% of the slowest allocator's
/// saturated rate: fill the server's caches and pools under closed-loop load, measure the
/// saturated rate, hold each fixed rate, then let load come and go, and close the connections
/// and watch the idle server. Every fixed-rate segment after a closed one starts with a
/// second that is left out, so the backlog of the closed loop does not land in it. The
/// silences are long enough for several of an allocator's clocked steps.
fn day(r30: u32, r70: u32) -> String {
    let mut segs = vec![
        "_fill:closed:2".to_string(),
        "sat:closed:4".into(),
        format!("_w30:{r30}:1"),
        format!("s30:{r30}:6"),
        format!("_w70:{r70}:1"),
        format!("s70:{r70}:6"),
    ];
    for _ in 0..3 {
        segs.push("off:off:3".into());
        segs.push(format!("on:{r70}:2"));
    }
    segs.push("idle:close:10".into());
    segs.join(",")
}

/// Only the saturated rate, to choose a server's fixed rates (`--suite servers-sat`).
const SAT: &str = "_fill:closed:2,sat:closed:5";

/// The servers, each in one L3 domain and its load generator in the other.
struct Served {
    id: &'static str,
    program: Program,
    args: &'static str,
    load: OpenLoad,
    /// The fixed rates of its day, requests per second; see `day`.
    rates: (u32, u32),
}

fn served() -> Vec<Served> {
    let order = |path| OpenLoad {
        path,
        content_type: "application/json",
        headers: vec![],
        body: "{app}/order.json",
        conns: 64,
        threads: 4,
        cpus: L3B.to_vec(),
        plan: String::new(),
        rank: "s70",
        inputs: None,
        proto: "http",
        streams: 1,
        requests: None,
        upstream: None,
        ram: None,
    };
    // HTTP/2: eight connections of eight streams, the same 64 requests at once as HTTP/1.1's
    // 64 connections.
    let h2 = |l: OpenLoad| OpenLoad { proto: "h2", conns: 8, streams: 8, ..l };
    let connect = |content_type, headers, body| OpenLoad {
        path: "/bench.v1.LogIngestService/Ingest",
        content_type,
        headers,
        body,
        inputs: Some("connect"),
        ..order("")
    };
    let router = |id, rates| Served {
        id,
        program: Program::App("router"),
        args: "serve subgraphs={upstream} schema={app}/supergraph.graphql check={inputs}/requests.http",
        load: OpenLoad {
            requests: Some("{inputs}/requests.http"),
            upstream: Some(Upstream {
                bin: "subgraphs",
                args: vec![],
                cpus: vec![12, 13, 14],
                workspace: Some("apps/router/subgraphs"),
            }),
            ..order("/")
        },
        rates,
    };
    vec![
        // connect-rust's log-ingest service (bench/rpc), a 50-record batch per request in
        // the Connect protocol over HTTP/1.1.
        Served {
            id: "connect log",
            program: Program::Rust("rpc-server"),
            args: "log",
            load: connect("application/proto", vec!["Connect-Protocol-Version: 1"], "{inputs}/log50.bin"),
            // The slowest is snmalloc at 122k req/s (closed loop, 2026-10-20), half the others'
            // 216k to 256k; glibc is next slowest.
            rates: (36_000, 85_000),
        },
        // The same service over HTTP/2: gRPC, as connect-rust's own log_bench drives it, and
        // the Connect protocol.
        Served {
            id: "connect log grpc h2",
            program: Program::Rust("rpc-server"),
            args: "log",
            load: h2(connect("application/grpc", vec!["te: trailers"], "{inputs}/log50.grpc")),
            // Slowest snmalloc 89k req/s, then glibc 146k; the others 174k to 192k (2026-10-20).
            rates: (27_000, 63_000),
        },
        Served {
            id: "connect log h2",
            program: Program::Rust("rpc-server"),
            args: "log",
            load: h2(connect("application/proto", vec!["Connect-Protocol-Version: 1"], "{inputs}/log50.bin")),
            // Slowest snmalloc 91k req/s, then glibc 148k; the others 172k to 194k.
            rates: (27_000, 63_000),
        },
        Served {
            id: "orders axum h2",
            program: Program::App("orders"),
            args: "axum",
            load: h2(order("/orders")),
            // Slowest glibc 237k req/s; the others 299k to 306k.
            rates: (71_000, 166_000),
        },
        // The slowest at saturation is glibc, 335k req/s on axum and 403k on actix.
        Served {
            id: "orders axum",
            program: Program::App("orders"),
            args: "axum",
            load: order("/orders"),
            rates: (100_000, 235_000),
        },
        Served {
            id: "orders actix",
            program: Program::App("orders"),
            args: "actix",
            load: order("/orders"),
            rates: (120_000, 280_000),
        },
        // Sōzu, the reverse proxy, forwarding the order service's POST to an upstream on
        // loopback (bench/probes/loadgen's `upstream`, 4 KiB answers) on three CPUs of the
        // generator's L3, which two of them already keep up with. One worker process a CPU.
        Served {
            id: "sozu proxy",
            program: Program::App("sozu"),
            args: "proxy upstream={upstream}",
            load: OpenLoad {
                body: "{app}/../orders/order.json",
                upstream: Some(Upstream {
                    bin: "upstream",
                    args: vec!["--body", "4096"],
                    cpus: vec![12, 13, 14],
                    workspace: None,
                }),
                ..order("/orders")
            },
            // Every allocator saturates at 265k to 276k req/s, snmalloc slowest (2026-10-21).
            rates: (80_000, 186_000),
        },
        // The Apollo Router, federating the four subgraphs of Apollo's demo (bench/apps/router):
        // GraphQL queries from a fixed list, each planned (from the Router's cache), fetched
        // from two to four subgraphs in up to three steps, and the answers merged into tens of
        // kilobytes of JSON. The subgraphs run in one process on three CPUs of the generator's
        // L3 and answer from memory after the first pass through the list.
        router("router graphql", ROUTER_RATES),
        // The same at the rates the slowest of the other four allocators sets, since glibc's
        // half rate leaves them at 37% of their capacity at "70%": run without glibc.
        router("router graphql fast", ROUTER_FAST_RATES),
        // The control: tokio's mini-redis, alternate SET and GET over 100k keys (Redis
        // protocol).
        Served {
            id: "miniredis",
            program: Program::App("miniredis"),
            args: "serve",
            load: OpenLoad { proto: "resp", requests: Some("{inputs}/commands.resp"), ..order("") },
            // Slowest cralloc 680k req/s; the others 717k to 741k.
            rates: (204_000, 476_000),
        },
    ]
}

/// 30% and 70% of the slowest allocator's saturated rate on `router graphql`: glibc's 4,790
/// req/s (cralloc 8,972, jemalloc 8,545, tcmalloc 9,024, mimalloc 8,543; 2026-10-24).
const ROUTER_RATES: (u32, u32) = (1_440, 3_350);
/// The same for the other four alone: jemalloc's 8,444 req/s (mimalloc 8,543 to 8,609).
const ROUTER_FAST_RATES: (u32, u32) = (2_530, 5_910);

fn server_job(s: Served, plan: String, rank: &'static str, suffix: &str) -> Job {
    Job {
        id: format!("{}{suffix}", s.id),
        group: "servers",
        mode: Mode::Native,
        program: s.program,
        args: args(s.args),
        cpus: SERVER_CPUS.to_vec(),
        driver: None,
        load: Some(OpenLoad { plan, rank, ..s.load }),
        metric: Metric::Served,
        timeout_s: 120,
    }
}

/// Servers under an open load, ranked by cycles per request at a fixed rate, with latency,
/// memory under load and after it (bench/apps/README.md, "Servers").
pub fn servers() -> Vec<Job> {
    served()
        .into_iter()
        .map(|s| {
            let plan = day(s.rates.0, s.rates.1);
            server_job(s, plan, "s70", "")
        })
        .collect()
}

/// Each server's saturated rate alone, closed loop.
pub fn servers_sat() -> Vec<Job> {
    served().into_iter().map(|s| server_job(s, SAT.into(), "sat", " sat")).collect()
}

/// A large-heap server's day: fill its heap with `fill_s` seconds of closed-loop load (its
/// requests build the live set), measure the saturated rate, then hold 70% of the slowest
/// allocator's saturated rate for a minute in six marked pieces of ten seconds, so that the
/// resident set and the program's live bytes are read every ten seconds while the heap churns;
/// then bursts after silences, and the connections close while the idle server is watched.
fn large_day(fill_s: u32, r70: u32) -> String {
    let mut segs = vec![format!("_fill:closed:{fill_s}"), "sat:closed:4".into(), format!("_w70:{r70}:2")];
    segs.extend((0..6).map(|_| format!("s70:{r70}:10")));
    for _ in 0..3 {
        segs.push("off:off:3".into());
        segs.push(format!("on:{r70}:2"));
    }
    segs.push("idle:close:10".into());
    segs.join(",")
}

/// A large-heap server: its id, program, arguments, request, the closed-loop seconds that fill
/// its heap, and its 70% rate.
struct Large {
    id: String,
    program: Program,
    args: String,
    load: OpenLoad,
    fill_s: u32,
    r70: u32,
}

/// The large-heap servers, each at three live sizes of about 0.5, 2 and 4 GiB: the order
/// service keeping its last `keep` orders (about 1.9 KiB live an order) in tables sized for
/// them from the start, so that the job measures steady churn and not the tables' one
/// doubling (that is `servers_doubling`), and mini-redis
/// preloaded with `preload` keys (about 1.1 KiB live a key) and churned by overwrites of mixed
/// sizes, some with an expiry.
fn large() -> Vec<Large> {
    let order = OpenLoad {
        path: "/orders",
        content_type: "application/json",
        headers: vec![],
        body: "{app}/order.json",
        conns: 64,
        threads: 4,
        cpus: L3B.to_vec(),
        plan: String::new(),
        rank: "s70",
        inputs: None,
        proto: "http",
        streams: 1,
        requests: None,
        upstream: None,
        ram: None,
    };
    let mut v = Vec::new();
    // 70% of the slowest saturated rate, mimalloc's: 348k, 349k and 355k req/s.
    for (size, keep, fill_s, r70) in
        [("0.5G", 280_000, 3, 243_000), ("2G", 1_120_000, 6, 244_000), ("4G", 2_250_000, 10, 248_000)]
    {
        v.push(Large {
            id: format!("orders axum cache {size}"),
            program: Program::App("orders"),
            args: format!("axum keep={keep} presize=1"),
            load: order.clone(),
            fill_s,
            r70,
        });
    }
    // 70% of the slowest saturated rate: tcmalloc's 672k, then cralloc's 664k and 660k req/s.
    for (size, keys, file, r70) in [
        ("0.5G", 450_000, "{inputs}/churn-450000.resp", 470_000),
        ("2G", 1_800_000, "{inputs}/churn-1800000.resp", 465_000),
        ("4G", 3_600_000, "{inputs}/churn-3600000.resp", 462_000),
    ] {
        v.push(Large {
            id: format!("miniredis churn {size}"),
            program: Program::App("miniredis"),
            args: format!("serve preload={keys}"),
            load: OpenLoad { proto: "resp", requests: Some(file), inputs: None, ..order.clone() },
            fill_s: 2,
            r70,
        });
    }
    // Meilisearch over 400k Hacker News items (about 1 GiB of heap and 3 GiB of mapped
    // store), searched by 128 connections while it indexes a batch of updates every 200
    // requests (bench/apps/meilisearch/fetch.sh). Its start indexes the items, before it
    // listens.
    v.push(Large {
        id: "meilisearch hn".into(),
        program: Program::App("meilisearch"),
        args: "serve data={inputs}".into(),
        load: OpenLoad { requests: Some("{inputs}/requests.http"), conns: 128, ..order.clone() },
        fill_s: 5,
        r70: MEILISEARCH_R70,
    });
    // Qdrant over a million vectors of 384 floats (about 1.5 GB, in RAM) and their HNSW graphs
    // (bench/apps/qdrant), queried for nearest neighbours by 64 connections over its REST API;
    // in `mixed`, every 20th request replaces 20 points, which its optimizers index in the
    // background. Each run starts from a copy of the same collection, in RAM: on the disk,
    // whose pages the optimizers write and the searches fault back, the job's time followed
    // the disk's load (docs/research/servers-qdrant.md). Not how Qdrant is deployed: it
    // measures the server's CPU and memory with the disk taken out.
    for (kind, r70) in [("search", QDRANT_SEARCH_R70), ("mixed", QDRANT_MIXED_R70)] {
        v.push(Large {
            id: format!("qdrant {kind}"),
            program: Program::App("qdrant"),
            // The driver copies `DIR/collection/storage` into `DIR/run.<pid>`.
            args: "serve data={ram}".into(),
            load: OpenLoad {
                requests: Some(if kind == "search" { "{inputs}/search.http" } else { "{inputs}/mixed.http" }),
                // The collection is 2 GB and grows to about 5 under the mixed load.
                ram: Some(RamDir { link: "collection", target: "{inputs}/collection", need_gib: 10 }),
                ..order.clone()
            },
            fill_s: 5,
            r70,
        });
    }
    // SurrealDB's in-memory engine holding 750k records with two indexes, filled as it starts
    // (bench/apps/surrealdb): record reads, index reads and updates in SurrealQL over HTTP.
    // The same at twice the records (about 3.8 GiB), for memory: the load still reads and
    // updates the first 750k, at the same rate.
    for (id, records) in [("surrealdb mem", 750_000), ("surrealdb mem 1.5M", 1_500_000)] {
        v.push(Large {
            id: id.into(),
            program: Program::App("surrealdb"),
            args: format!("serve check={{inputs}}/requests.http records={records}"),
            load: OpenLoad { requests: Some("{inputs}/requests.http"), ..order.clone() },
            fill_s: 5,
            r70: SURREALDB_R70,
        });
    }
    // connect-rust's services as a log store holding a million records (bench/rpc/src/store.rs):
    // every other request ingests 50 new records (the oldest go), every other one tails the
    // 100 most recent as a server stream, in the Connect protocol over HTTP/1.1.
    v.push(Large {
        id: "connect store 1M".into(),
        program: Program::Rust("rpc-server"),
        args: "store keep=1000000".into(),
        load: OpenLoad { requests: Some("{inputs}/store.http"), inputs: Some("connect"), ..order.clone() },
        fill_s: 3,
        r70: STORE_R70,
    });
    v
}

/// 70% of the slowest allocator's saturated rate on `connect store 1M`: mimalloc's 51,840
/// req/s (cralloc 64,062, jemalloc 59,197, tcmalloc 56,361; 2026-10-21).
const STORE_R70: u32 = 36_300;

/// 70% of the slowest allocator's saturated rate on `meilisearch hn`: mimalloc's 1,328 req/s
/// (cralloc 1,413, jemalloc 1,372, tcmalloc 1,423).
const MEILISEARCH_R70: u32 = 930;

/// 70% of the slowest allocator's saturated rate, the collection in RAM, on `qdrant search`
/// (mimalloc's 3,528 req/s; the others 3,576 to 3,635) and `qdrant mixed` (cralloc's 1,804;
/// the others 1,822 to 1,866), two rounds, 2026-10-24.
const QDRANT_SEARCH_R70: u32 = 2_470;
const QDRANT_MIXED_R70: u32 = 1_260;

/// 70% of the slowest allocator's saturated rate on `surrealdb mem`: glibc's 8,728 req/s
/// (cralloc 11,186, tcmalloc 10,460, mimalloc 10,001, jemalloc 9,672; SurrealDB's release
/// profile, one round, 2026-10-25).
const SURREALDB_R70: u32 = 6_110;

fn large_job(l: Large, plan: String, rank: &'static str, suffix: &str) -> Job {
    Job {
        id: format!("{}{suffix}", l.id),
        group: "servers-large",
        mode: Mode::Native,
        program: l.program,
        args: args(&l.args),
        cpus: SERVER_CPUS.to_vec(),
        driver: None,
        load: Some(OpenLoad { plan, rank, ..l.load }),
        metric: Metric::Served,
        timeout_s: 600,
    }
}

/// Servers holding 0.5 to 4 GiB live while they serve (bench/apps/README.md, "Servers").
/// Run each size as a session of its own with `--mem-cap` sized to it.
pub fn servers_large() -> Vec<Job> {
    large()
        .into_iter()
        .map(|l| {
            let plan = large_day(l.fill_s, l.r70);
            large_job(l, plan, "s70", "")
        })
        .collect()
}

/// The order service at 4 GiB with tables that are not pre-sized, so that each of its 64
/// shard tables doubles once under churn, long after the store is full, rehashing every entry
/// under the shard's lock (docs/research/servers-large.md, "The 4 GiB tail"). The load is a
/// fixed rate from the start, so that the doublings come at the same order count, and so the
/// same moment, for every allocator, inside the measured segment `dbl`. The server reports
/// each doubling's time and page faults; the runner, the latency, faults and resident set
/// over `dbl`.
pub fn servers_doubling() -> Vec<Job> {
    let l = large().into_iter().find(|l| l.id == "orders axum cache 4G").expect("the 4 GiB order service");
    let plan = format!("_fill:{r}:{DOUBLING_FILL_S},dbl:{r}:{DOUBLING_S},idle:close:5", r = l.r70);
    let l = Large { id: "orders axum doubling 4G".into(), args: "axum keep=2250000".into(), ..l };
    vec![large_job(l, plan, "dbl", "")]
}

/// The doublings come after about 14.4M orders: at 248k req/s, 58 to 60 s in for cralloc and
/// jemalloc alike, so 14 to 16 s into the segment.
const DOUBLING_FILL_S: u32 = 44;
const DOUBLING_S: u32 = 28;

/// Each large-heap server's saturated rate, after its heap is filled.
pub fn servers_large_sat() -> Vec<Job> {
    large()
        .into_iter()
        .map(|l| {
            let plan = format!("_fill:closed:{},sat:closed:5", l.fill_s);
            large_job(l, plan, "sat", " sat")
        })
        .collect()
}

/// What cralloc's default background thread does to a large-heap server
/// (docs/research/default-thread-servers.md): the order service at 2 GiB, the log store and
/// Meilisearch, each filled, then half a minute at 70% (`s70`, three ten-second pieces), a
/// silence of 30 s with the connections open, long enough for the thread to drain and park,
/// one burst at 70% marked in pieces so that its first 5 ms, the rest of its first 50 ms and
/// of its first 500 ms are summarised apart (`b5`, `b50`, `b500`, then `b2`), and a minute
/// after the connections close (`idle`).
pub fn servers_thread() -> Vec<Job> {
    let want = [
        "orders axum cache 2G",
        "connect store 1M",
        "meilisearch hn",
        "qdrant mixed",
        "surrealdb mem",
        "surrealdb mem 1.5M",
    ];
    large()
        .into_iter()
        .filter(|l| want.contains(&l.id.as_str()))
        .map(|l| {
            let r = l.r70;
            let mut segs = vec![format!("_fill:closed:{}", l.fill_s), format!("_w70:{r}:2")];
            segs.extend((0..3).map(|_| format!("s70:{r}:10")));
            segs.extend([
                "off:off:30".to_string(),
                format!("b5:{r}:0.005"),
                format!("b50:{r}:0.045"),
                format!("b500:{r}:0.45"),
                format!("b2:{r}:1.5"),
                "idle:close:60".into(),
            ]);
            let mut job = large_job(l, segs.join(","), "s70", " thread");
            job.group = "servers-thread";
            job
        })
        .collect()
}

/// Meilisearch's tail at 70% alone (docs/research/meili-tail.md): filled as `servers-thread`
/// fills it, then two minutes at 70% in twelve marked pieces of ten seconds (`s70`), and no
/// silence or burst, so that a session spends its minutes on the steady tail.
pub fn meili_tail() -> Vec<Job> {
    large()
        .into_iter()
        .filter(|l| l.id == "meilisearch hn")
        .map(|l| {
            let r = l.r70;
            let mut segs = vec![format!("_fill:closed:{}", l.fill_s), format!("_w70:{r}:2")];
            segs.extend((0..12).map(|_| format!("s70:{r}:10")));
            segs.push("idle:close:2".into());
            let mut job = large_job(l, segs.join(","), "s70", " tail");
            job.group = "meili-tail";
            job
        })
        .collect()
}

/// The order service as a ladder of live heaps (docs/research/large-heap-constants.md): the
/// three sizes of `servers-large` and one of about 10 GiB, 5.6M orders, each on the large-heap
/// day. Run each size as a session of its own with `--mem-cap` sized to it (13 GiB for 10G).
pub fn servers_ladder() -> Vec<Job> {
    let four = large().into_iter().find(|l| l.id == "orders axum cache 4G").expect("the 4 GiB order service");
    let ten = Large {
        id: "orders axum cache 10G".into(),
        args: "axum keep=5600000 presize=1".into(),
        fill_s: 24,
        r70: 245_000,
        ..four
    };
    large()
        .into_iter()
        .filter(|l| l.id.starts_with("orders axum cache"))
        .chain([ten])
        .map(|l| {
            let plan = large_day(l.fill_s, l.r70);
            let mut job = large_job(l, plan, "s70", "");
            job.group = "servers-ladder";
            job
        })
        .collect()
}

/// Every CPU any job may use.
pub fn measurement_cpus() -> Vec<usize> {
    both()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::process::Command;

    fn deep_idle(args: &[&str], fixture: Option<&Path>) -> (bool, String, String) {
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/deep-idle");
        let mut c = Command::new(script);
        c.args(args);
        if let Some(f) = fixture {
            c.env("CRALLOC_CPUIDLE_ROOT", f);
        }
        let out = c.output().expect("scripts/deep-idle runs");
        (out.status.success(), String::from_utf8_lossy(&out.stdout).into(), String::from_utf8_lossy(&out.stderr).into())
    }

    /// The script's default CPUs and cut-off are the runner's, read from this source.
    #[test]
    fn deep_idle_script_takes_the_runners_cpus() {
        for j in servers().into_iter().chain(servers_large()).chain(servers_thread()) {
            assert_eq!(j.cpus, SERVER_CPUS, "{}", j.id);
        }
        let (ok, out, err) = deep_idle(&["defaults"], None);
        assert!(ok, "{err}");
        let cpus: Vec<String> = SERVER_CPUS.iter().map(|c| c.to_string()).collect();
        assert_eq!(out, format!("cpus {}\ndeep-latency-us {}\n", cpus.join(","), crate::cpuidle::DEEP_LATENCY_US));
    }

    #[test]
    fn deep_idle_script_reads_a_sysfs_tree() {
        let root = std::env::temp_dir().join(format!("benchrun-deep-idle-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (k, (name, lat, off)) in [("POLL", 0, 0), ("C1", 1, 0), ("C2", 18, 1), ("C3", 350, 0)].iter().enumerate() {
            let d = root.join(format!("cpu5/cpuidle/state{k}"));
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("name"), format!("{name}\n")).unwrap();
            std::fs::write(d.join("latency"), format!("{lat}\n")).unwrap();
            std::fs::write(d.join("disable"), format!("{off}\n")).unwrap();
        }
        let (ok, out, err) = deep_idle(&["status", "5-6"], Some(&root));
        assert!(ok, "{err}");
        assert!(
            out.contains("cpu5: POLL 0 us enabled, C1 1 us enabled, C2 18 us deep disabled, C3 350 us deep enabled"),
            "{out}"
        );
        assert!(out.contains("cpu6: no idle-state counters"), "{out}");
        assert!(out.contains("enabled on CPUs 5\n"), "{out}");
        // Without root, the command to run instead of a change; this test is not run as root.
        // SAFETY: getuid has no preconditions.
        if unsafe { libc::getuid() } != 0 {
            let (ok, _, err) = deep_idle(&["off", "5"], Some(&root));
            assert!(!ok);
            assert!(err.contains("sudo ") && err.contains("deep-idle off 5"), "{err}");
            assert_eq!(std::fs::read_to_string(root.join("cpu5/cpuidle/state3/disable")).unwrap(), "0\n");
        }
        let (ok, _, err) = deep_idle(&["status", "0-x"], Some(&root));
        assert!(!ok && err.contains("not a CPU list"), "{err}");
        let _ = std::fs::remove_dir_all(&root);
    }
}
