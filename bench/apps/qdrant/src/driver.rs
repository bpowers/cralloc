//! The driver: Qdrant 1.20.0 as a server for the load generator, and the two jobs that make
//! its inputs.
//!
//!   qdrant serve data=DIR
//!   qdrant build out=DIR [points=N] [dim=D] [segments=S] [seed=X]
//!   qdrant requests out=FILE kind=search|mixed [n=N] [points=N] [dim=D] [seed=X]
//!
//! `serve` copies the collection `build` made (`DIR/storage`, by reflink where the file system
//! has it) to a fresh directory under `DIR`, starts Qdrant on it, waits until the collection is
//! loaded and green with every point, prints `BENCH_LISTEN addr` (the REST API), and on SIGTERM
//! checks the collection again, stops Qdrant and removes the copy.
//!
//! Qdrant reads its command line itself, so the driver runs it in a child process, this binary
//! again with Qdrant's own arguments (`--config-path`, a file the driver writes: the storage,
//! loopback ports for REST and gRPC, warnings only, no telemetry, no cluster, and idle client
//! connections kept for 10 minutes instead of 5 s, since the load generator counts a closed
//! connection as a broken run); the runner's counters and memory follow it. What differs from the `qdrant` binary is the allocator (the
//! suite's, not jemalloc; see no-jemalloc-sys), nothing else.
//!
//! The collection: `points` vectors of `dim` floats (default 1,000,000 of 384, about 1.5 GB),
//! cosine distance, HNSW with Qdrant's defaults (m 16, ef_construct 100), `segments` segments
//! (default 4, one a server CPU, so that Qdrant's optimizers have nothing to merge when a run
//! starts), vectors in RAM (`on_disk: false`), and a payload of a category (100 values,
//! indexed), a price and a title. The vectors are drawn around 256 cluster centres from a
//! fixed seed, so every input is the same on every machine.
//!
//! `requests` writes whole HTTP requests for the load generator: queries for the 10 nearest
//! points of a vector drawn as the data is, with their payloads, one in five filtered by
//! category; for `mixed`, every 20th request instead replaces 20 existing points with new
//! vectors (upsert, waiting for it to be applied), so that the collection keeps its size while
//! Qdrant's optimizers index what changed in the background.
use bench_app::{Driver, fail};
use serde_json::Value;
use std::fmt::Write as _;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

const USAGE: &str = "usage: qdrant serve data=DIR | build out=DIR [points=N] [dim=D] [segments=S] [seed=X] | \
                     requests out=FILE kind=search|mixed [n=N] [points=N] [dim=D] [seed=X]";
const COLLECTION: &str = "bench";
const CATEGORIES: u64 = 100;
const CLUSTERS: u64 = 256;
/// Every 20th request of a mixed load is an upsert of 20 points.
const UPSERT_EVERY: u64 = 20;
const UPSERT_POINTS: u64 = 20;

pub fn main() {
    // Qdrant's process: this binary run again with Qdrant's own command line.
    if std::env::args().nth(1).is_some_and(|a| a.starts_with("--")) {
        bench_alloc::init();
        if let Err(e) = crate::qdrant_main() {
            eprintln!("qdrant: {e:#}");
            std::process::exit(1);
        }
        return;
    }
    let mut d = Driver::start("qdrant");
    match d.job.as_str() {
        "serve" => serve(&mut d),
        "build" => build(&mut d),
        "requests" => requests(&mut d),
        _ => fail(USAGE),
    }
    d.finish_server();
}

/// The data, from its parameters alone.
struct Model {
    dim: usize,
    points: u64,
    seed: u64,
    centres: Vec<Vec<f32>>,
}

impl Model {
    fn new(dim: usize, points: u64, seed: u64) -> Model {
        let centres = (0..CLUSTERS)
            .map(|c| {
                let mut r = Rng::new(seed ^ 0x6365_6e74_7265 ^ c.wrapping_mul(0x9e37_79b9_7f4a_7c15));
                (0..dim).map(|_| r.gauss()).collect()
            })
            .collect();
        Model { dim, points, seed, centres }
    }

    fn from_args(d: &Driver) -> Model {
        let dim = d.args.usize("dim", 384);
        let points = d.args.num("points", 1_000_000);
        let seed = d.args.num("seed", 20261024);
        if dim == 0 || points == 0 {
            fail("dim and points must be positive");
        }
        Model::new(dim, points, seed)
    }

    /// A vector near one of the centres: point `id`'s `version`-th value, or (with `query`) the
    /// `id`-th query.
    fn vector(&self, id: u64, version: u64, query: bool) -> Vec<f32> {
        let tag = if query { 0x0071_7565_7279 } else { 0x0070_6f69_6e74 };
        let mut r = Rng::new(
            self.seed ^ tag ^ id.wrapping_mul(0xd6e8_feb8_6659_fd93) ^ version.wrapping_mul(0xa076_1d64_78bd_642f),
        );
        let centre = &self.centres[(r.next() % CLUSTERS) as usize];
        centre.iter().map(|c| c + 0.35 * r.gauss()).collect()
    }

    fn point_json(&self, out: &mut String, id: u64, version: u64) {
        let _ = write!(out, "{{\"id\":{id},\"vector\":");
        vector_json(out, &self.vector(id, version, false));
        let _ = write!(
            out,
            ",\"payload\":{{\"category\":\"c{}\",\"price\":{},\"title\":\"item {id} v{version}\"}}}}",
            id % CATEGORIES,
            (id * 37 + version) % 10_000
        );
    }
}

fn vector_json(out: &mut String, v: &[f32]) {
    out.push('[');
    for (i, x) in v.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let _ = write!(out, "{x:.5}");
    }
    out.push(']');
}

/// SplitMix64, with Box-Muller for normal deviates.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(seed)
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn unit(&mut self) -> f64 {
        ((self.next() >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }

    fn gauss(&mut self) -> f32 {
        let (u, v) = (self.unit(), self.unit());
        ((-2.0 * u.ln()).sqrt() * (std::f64::consts::TAU * v).cos()) as f32
    }
}

/// A Qdrant process this driver started.
struct Qdrant {
    child: Child,
    addr: SocketAddr,
}

impl Qdrant {
    /// Starts Qdrant on `storage`, with its configuration written to `config`, and waits until
    /// it is ready.
    fn start(storage: &Path, config: &Path) -> Qdrant {
        let addr = free_port();
        let grpc = free_port();
        let text = format!(
            "log_level: WARN\ntelemetry_disabled: true\nstorage:\n  storage_path: {:?}\n  snapshots_path: {:?}\n\
             service:\n  host: 127.0.0.1\n  http_port: {}\n  grpc_port: {}\n  http_keep_alive_timeout_sec: 600\n\
             cluster:\n  enabled: false\n",
            storage.display().to_string(),
            storage.join("snapshots").display().to_string(),
            addr.port(),
            grpc.port(),
        );
        std::fs::write(config, text).unwrap_or_else(|e| fail(&format!("{}: {e}", config.display())));
        let exe = std::env::current_exe().unwrap_or_else(|e| fail(&format!("current_exe: {e}")));
        let mut cmd = Command::new(exe);
        cmd.arg(format!("--config-path={}", config.display())).arg("--disable-telemetry");
        {
            use std::os::unix::process::CommandExt;
            // SAFETY: only async-signal-safe calls between fork and exec.
            unsafe {
                cmd.pre_exec(|| {
                    let empty = sigset(0);
                    libc::pthread_sigmask(libc::SIG_SETMASK, &empty, std::ptr::null_mut());
                    Ok(())
                });
            }
        }
        let child = cmd.spawn().unwrap_or_else(|e| fail(&format!("starting Qdrant: {e}")));
        let mut q = Qdrant { child, addr };
        let deadline = Instant::now() + Duration::from_secs(300);
        loop {
            if let Ok(Some(st)) = q.child.try_wait() {
                fail(&format!("Qdrant exited as it started: {st}"));
            }
            if matches!(call(addr, "GET", "/readyz", b""), Ok((200, _))) {
                return q;
            }
            if Instant::now() > deadline {
                q.kill();
                fail("Qdrant never became ready");
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn json(&self, method: &str, path: &str, body: &[u8]) -> Value {
        match call(self.addr, method, path, body) {
            Ok((200, text)) => serde_json::from_str(&text).unwrap_or_else(|e| fail(&format!("{path}: {e}: {text}"))),
            Ok((status, text)) => {
                fail(&format!("{method} {path}: {status}: {}", text.chars().take(400).collect::<String>()))
            }
            Err(e) => fail(&format!("{method} {path}: {e}")),
        }
    }

    fn collection(&self) -> Value {
        self.json("GET", &format!("/collections/{COLLECTION}"), b"")
    }

    /// The collection's state, once it is green (every optimisation done), or fails after
    /// `wait`.
    fn green(&self, wait: Duration) -> Value {
        let deadline = Instant::now() + wait;
        loop {
            let info = self.collection();
            if info["result"]["status"] == "green" {
                return info;
            }
            if Instant::now() > deadline {
                fail(&format!("the collection is not green: {}", info["result"]));
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }

    /// Stops Qdrant with SIGTERM, as a service manager does, and waits for it; kills it after a
    /// minute. Returns whether it stopped by itself.
    fn stop(mut self) -> bool {
        // SAFETY: kill on our own child, which has not been reaped.
        unsafe { libc::kill(self.child.id() as libc::pid_t, libc::SIGTERM) };
        let deadline = Instant::now() + Duration::from_secs(60);
        while Instant::now() < deadline {
            if let Ok(Some(_)) = self.child.try_wait() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        self.kill();
        false
    }

    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn free_port() -> SocketAddr {
    // Found by binding and letting go: nothing else binds loopback ports here.
    TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .unwrap_or_else(|e| fail(&format!("finding a port: {e}")))
}

fn sigset(sig: libc::c_int) -> libc::sigset_t {
    // SAFETY: sigset_t is plain data that sigemptyset initialises.
    unsafe {
        let mut s: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut s);
        if sig != 0 {
            libc::sigaddset(&mut s, sig);
        }
        s
    }
}

/// One HTTP/1.1 request on a connection of its own; the status and body.
fn call(addr: SocketAddr, method: &str, path: &str, body: &[u8]) -> Result<(u16, String), String> {
    let mut s = TcpStream::connect(addr).map_err(|e| format!("connect: {e}"))?;
    let head = format!(
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n",
        body.len()
    );
    s.write_all(head.as_bytes()).and_then(|_| s.write_all(body)).map_err(|e| format!("send: {e}"))?;
    let mut out = Vec::new();
    s.read_to_end(&mut out).map_err(|e| format!("receive: {e}"))?;
    let text = String::from_utf8_lossy(&out);
    let (head, body) = text.split_once("\r\n\r\n").ok_or("no response head")?;
    let status = head.split_whitespace().nth(1).and_then(|s| s.parse().ok()).ok_or("no status")?;
    let body =
        if head.to_ascii_lowercase().contains("transfer-encoding: chunked") { unchunk(body) } else { body.to_string() };
    Ok((status, body))
}

fn unchunk(mut s: &str) -> String {
    let mut out = String::new();
    while let Some((size, rest)) = s.split_once("\r\n") {
        let n = usize::from_str_radix(size.trim(), 16).unwrap_or(0);
        if n == 0 || rest.len() < n {
            break;
        }
        out.push_str(&rest[..n]);
        s = rest[n..].strip_prefix("\r\n").unwrap_or(&rest[n..]);
    }
    out
}

fn build(d: &mut Driver) {
    let out = PathBuf::from(d.args.str("out", ""));
    if out.as_os_str().is_empty() || out.exists() {
        fail("build: out=DIR must name a directory that does not exist yet");
    }
    let m = Model::from_args(d);
    let segments = d.args.usize("segments", 4);
    let storage = out.join("storage");
    std::fs::create_dir_all(&storage).unwrap_or_else(|e| fail(&format!("{}: {e}", storage.display())));
    let q = Qdrant::start(&storage, &out.join("qdrant.yaml"));
    let create = format!(
        "{{\"vectors\":{{\"size\":{},\"distance\":\"Cosine\",\"on_disk\":false}},\
         \"hnsw_config\":{{\"m\":16,\"ef_construct\":100}},\
         \"optimizers_config\":{{\"default_segment_number\":{segments}}}}}",
        m.dim
    );
    q.json("PUT", &format!("/collections/{COLLECTION}"), create.as_bytes());
    q.json(
        "PUT",
        &format!("/collections/{COLLECTION}/index?wait=true"),
        br#"{"field_name":"category","field_schema":"keyword"}"#,
    );
    let t0 = Instant::now();
    let batch = 500;
    let mut id = 0;
    while id < m.points {
        let end = (id + batch).min(m.points);
        let mut body = String::from("{\"points\":[");
        for i in id..end {
            if i > id {
                body.push(',');
            }
            m.point_json(&mut body, i, 0);
        }
        body.push_str("]}");
        q.json("PUT", &format!("/collections/{COLLECTION}/points?wait=true"), body.as_bytes());
        id = end;
    }
    let uploaded = t0.elapsed();
    let info = q.green(Duration::from_secs(4 * 3600));
    let count = info["result"]["points_count"].as_u64().unwrap_or(0);
    if count != m.points {
        fail(&format!("the collection has {count} points, not {}", m.points));
    }
    let indexed = t0.elapsed();
    eprintln!(
        "qdrant build: {} points uploaded in {:.0} s, green after {:.0} s: {}",
        m.points,
        uploaded.as_secs_f64(),
        indexed.as_secs_f64(),
        info["result"]
    );
    let clean = q.stop();
    if !clean {
        fail("Qdrant did not stop after the build");
    }
    let meta = format!("{{\"points\":{},\"dim\":{},\"seed\":{},\"segments\":{segments}}}\n", m.points, m.dim, m.seed);
    std::fs::write(out.join("collection.json"), meta).unwrap_or_else(|e| fail(&format!("collection.json: {e}")));
    let _ = std::fs::remove_file(out.join("qdrant.yaml"));
    d.report().param("points", m.points).param("build_s", indexed.as_secs_f64());
}

fn requests(d: &mut Driver) {
    let out = PathBuf::from(d.args.str("out", ""));
    let mixed = match d.args.str("kind", "").as_str() {
        "search" => false,
        "mixed" => true,
        _ => fail("requests: kind=search or kind=mixed"),
    };
    let n = d.args.num("n", 20_000);
    let m = Model::from_args(d);
    let mut r = Rng::new(m.seed ^ 0x7265_7173);
    let mut file = Vec::new();
    let mut body = String::new();
    for k in 0..n {
        body.clear();
        let (method, path) = if mixed && k % UPSERT_EVERY == UPSERT_EVERY - 1 {
            body.push_str("{\"points\":[");
            for j in 0..UPSERT_POINTS {
                if j > 0 {
                    body.push(',');
                }
                m.point_json(&mut body, r.next() % m.points, 1 + k);
            }
            body.push_str("]}");
            ("PUT", format!("/collections/{COLLECTION}/points?wait=true"))
        } else {
            body.push_str("{\"query\":");
            vector_json(&mut body, &m.vector(k, 0, true));
            if r.next().is_multiple_of(5) {
                let _ = write!(
                    body,
                    ",\"filter\":{{\"must\":[{{\"key\":\"category\",\"match\":{{\"value\":\"c{}\"}}}}]}}",
                    r.next() % CATEGORIES
                );
            }
            body.push_str(",\"limit\":10,\"with_payload\":true}");
            ("POST", format!("/collections/{COLLECTION}/points/query"))
        };
        let _ = write!(
            file,
            "{method} {path} HTTP/1.1\r\nHost: bench\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
    }
    std::fs::write(&out, &file).unwrap_or_else(|e| fail(&format!("{}: {e}", out.display())));
    d.report().param("requests", n);
}

fn serve(d: &mut Driver) {
    let data = PathBuf::from(d.args.str("data", ""));
    let template = data.join("collection/storage");
    let meta = std::fs::read_to_string(data.join("collection/collection.json"))
        .unwrap_or_else(|e| fail(&format!("data=DIR must hold the collection (bench/apps/fetch qdrant): {e}")));
    let meta: Value = serde_json::from_str(&meta).unwrap_or_else(|e| fail(&format!("collection.json: {e}")));
    let points = meta["points"].as_u64().unwrap_or_else(|| fail("collection.json: no points"));

    // SIGTERM is taken with sigwait below, so it is blocked from here on in this thread, the
    // only one; Qdrant's process gets the default mask back before it runs.
    let term = sigset(libc::SIGTERM);
    // SAFETY: a valid signal set, and a null old set.
    unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, &term, std::ptr::null_mut()) };

    let run = data.join(format!("run.{}", std::process::id()));
    let st = Command::new("cp")
        .args(["-a", "--reflink=auto"])
        .arg(&template)
        .arg(&run)
        .status()
        .unwrap_or_else(|e| fail(&format!("cp: {e}")));
    if !st.success() {
        fail("copying the collection failed");
    }
    let config = data.join(format!("run.{}.yaml", std::process::id()));
    let q = Qdrant::start(&run, &config);
    let count = |info: &Value, when: &str| {
        let count = info["result"]["points_count"].as_u64().unwrap_or(0);
        if count != points {
            fail(&format!("{when}: the collection has {count} points, not {points}"));
        }
    };
    count(&q.green(Duration::from_secs(120)), "at the start");
    d.report().param("points", points);
    println!("BENCH_LISTEN {}", q.addr);
    let _ = std::io::stdout().flush();

    let mut sig = 0;
    // SAFETY: a valid blocked set and an out pointer.
    unsafe { libc::sigwait(&term, &mut sig) };
    // After a mixed load the optimisers may still be indexing (yellow), which is not a fault.
    let info = q.collection();
    if !matches!(info["result"]["status"].as_str(), Some("green" | "yellow" | "grey")) {
        fail(&format!("after the load: {}", info["result"]));
    }
    count(&info, "after the load");
    let clean = q.stop();
    let _ = std::fs::remove_dir_all(&run);
    let _ = std::fs::remove_file(&config);
    if !clean {
        fail("Qdrant did not stop within a minute of SIGTERM");
    }
}
