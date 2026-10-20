//! An open-loop HTTP/1.1 load generator for measuring what a server's users feel: latency at a
//! fixed request rate, its tail, and the server's memory while load comes and goes.
//!
//! ```text
//! loadgen --addr 127.0.0.1:18080 --path /orders --content-type application/json \
//!         --body order.json --conns 64 --threads 4 --cpus 8-11 \
//!         --plan '_warm:closed:2,sat:closed:4,s70:140k:10,on:140k:2,off:off:5,end:close:20' \
//!         --pid SERVER_PID --json summary.json --raw requests.bin --mem rss.csv
//! ```
//!
//! The plan is a list of `NAME:KIND:SECS` segments run back to back. KIND is `closed` (every
//! connection sends its next request as soon as its last response arrives: the saturated
//! rate), a rate such as `140k` (requests due on a fixed schedule at that many per second in
//! total, whether or not the server keeps up), `off` (silence, connections kept open) or
//! `close` (close the connections and wait). Segments that share a name are summarised
//! together; a leading `_` leaves a segment out of the summary.
//!
//! Latency is from when a request was due to when its whole response had arrived, so a
//! request that waited for a free connection is charged for the wait. Each connection carries
//! one request at a time, as a pooled HTTP/1.1 client does. A request that found every
//! connection busy when it came due is counted as having missed its slot.
//!
//! Output: a `MARK` line on stdout at the start of every segment (`MARK <ms> <index> <name>
//! <kind>`) and at the end (`MARK <ms> end`), so that a script can measure the server over a
//! segment; then the summary as one JSON object on a `RESULT` line (and in `--json`). `--raw`
//! writes every request as 20 little-endian bytes: due time in ns from the start (u64),
//! latency in ns (u64), segment index (u16), flags (u16: 1 missed its slot, 2 error, 4 closed
//! loop). `--pid` samples the server's resident set every `--sample-ms` (default 10) into
//! `--mem` as `ms,rss_kib,file_kib`.
//!
//! `--proto h2` speaks HTTP/2 over cleartext with prior knowledge: `--conns` connections,
//! each carrying up to `--streams` requests at once (src/h2.rs). The same request options.
//!
//! `--proto resp` speaks the Redis protocol instead of HTTP: `--requests FILE` holds RESP
//! commands written back to back, which the connections send in turn (each thread starting
//! at its own place in the list); a reply is a success unless it is an error. `--path`,
//! `--content-type`, `--header` and `--body` are HTTP's.
//!
//! Exit status: 0 for a clean run; 1 when any request failed, a connection broke or a response
//! arrived that no request was waiting for (the summary is still printed); 2 for bad usage or a
//! connection that could not be opened.

mod h2;
mod http;
mod plan;
mod resp;
mod stats;
mod worker;

use plan::{Kind, Segment, Spacing};
use std::io::Write;
use std::net::{SocketAddr, ToSocketAddrs};
use std::path::PathBuf;
use std::process::exit;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

struct Opts {
    addr: SocketAddr,
    host: String,
    path: String,
    content_type: String,
    headers: Vec<String>,
    body: Vec<u8>,
    proto: worker::Proto,
    requests: Option<PathBuf>,
    streams: usize,
    conns: usize,
    threads: usize,
    cpus: Vec<usize>,
    aux_cpu: Option<usize>,
    segs: Vec<Segment>,
    spacing: Spacing,
    seed: u64,
    pid: Option<u32>,
    sample_ms: u64,
    head_ms: f64,
    drain_s: f64,
    label: String,
    json: Option<PathBuf>,
    raw: Option<PathBuf>,
    mem: Option<PathBuf>,
}

const USAGE: &str = "usage: loadgen --addr HOST:PORT --plan PLAN [--path P] [--content-type T] [--header 'K: V']... \
[--body FILE] [--proto http|h2|resp] [--streams 8] [--requests FILE] [--conns 64] [--threads 4] [--cpus LIST] [--aux-cpu N] [--spacing uniform|poisson] [--seed N] \
[--pid PID] [--sample-ms 10] [--head-ms 50] [--drain-s 5] [--label TEXT] [--json FILE] [--raw FILE] [--mem FILE]";

fn die(msg: &str) -> ! {
    eprintln!("loadgen: {msg}");
    exit(2)
}

fn cpu_list(s: &str) -> Vec<usize> {
    let mut v = Vec::new();
    for part in s.split(',').filter(|p| !p.is_empty()) {
        let (a, b) = part.split_once('-').unwrap_or((part, part));
        let (a, b): (usize, usize) = match (a.parse(), b.parse()) {
            (Ok(a), Ok(b)) if a <= b => (a, b),
            _ => die(&format!("bad CPU list `{s}`")),
        };
        v.extend(a..=b);
    }
    v
}

fn parse_opts() -> Opts {
    let mut args = std::env::args().skip(1);
    let mut o = Opts {
        addr: "127.0.0.1:1".parse().unwrap(),
        host: String::new(),
        path: "/".into(),
        content_type: String::new(),
        headers: Vec::new(),
        body: Vec::new(),
        proto: worker::Proto::Http,
        requests: None,
        streams: 8,
        conns: 64,
        threads: 4,
        cpus: Vec::new(),
        aux_cpu: None,
        segs: Vec::new(),
        spacing: Spacing::Uniform,
        seed: 1,
        pid: None,
        sample_ms: 10,
        head_ms: 50.0,
        drain_s: 5.0,
        label: String::new(),
        json: None,
        raw: None,
        mem: None,
    };
    let mut have_addr = false;
    while let Some(flag) = args.next() {
        if flag == "-h" || flag == "--help" {
            println!("{USAGE}");
            exit(0);
        }
        let v = args.next().unwrap_or_else(|| die(&format!("{flag} needs a value\n{USAGE}")));
        let num = |v: &str| v.parse::<u64>().unwrap_or_else(|_| die(&format!("{flag}: bad number `{v}`")));
        let float = |v: &str| v.parse::<f64>().unwrap_or_else(|_| die(&format!("{flag}: bad number `{v}`")));
        match flag.as_str() {
            "--addr" => {
                o.addr = v
                    .to_socket_addrs()
                    .ok()
                    .and_then(|mut a| a.next())
                    .unwrap_or_else(|| die(&format!("bad address `{v}`")));
                o.host = v;
                have_addr = true;
            }
            "--path" => o.path = v,
            "--content-type" => o.content_type = v,
            "--header" => o.headers.push(v),
            "--body" => o.body = std::fs::read(&v).unwrap_or_else(|e| die(&format!("{v}: {e}"))),
            "--proto" => {
                o.proto = match v.as_str() {
                    "http" => worker::Proto::Http,
                    "resp" => worker::Proto::Resp,
                    "h2" => worker::Proto::H2,
                    _ => die("--proto is http, h2 or resp"),
                }
            }
            "--requests" => o.requests = Some(v.into()),
            "--streams" => o.streams = (num(&v) as usize).max(1),
            "--conns" => o.conns = num(&v) as usize,
            "--threads" => o.threads = num(&v) as usize,
            "--cpus" => o.cpus = cpu_list(&v),
            "--aux-cpu" => o.aux_cpu = Some(num(&v) as usize),
            "--plan" => o.segs = plan::parse(&v).unwrap_or_else(|e| die(&e)),
            "--spacing" => {
                o.spacing = match v.as_str() {
                    "uniform" => Spacing::Uniform,
                    "poisson" => Spacing::Poisson,
                    _ => die("--spacing is uniform or poisson"),
                }
            }
            "--seed" => o.seed = num(&v),
            "--pid" => o.pid = Some(num(&v) as u32),
            "--sample-ms" => o.sample_ms = num(&v).max(1),
            "--head-ms" => o.head_ms = float(&v),
            "--drain-s" => o.drain_s = float(&v),
            "--label" => o.label = v,
            "--json" => o.json = Some(v.into()),
            "--raw" => o.raw = Some(v.into()),
            "--mem" => o.mem = Some(v.into()),
            _ => die(&format!("unknown option {flag}\n{USAGE}")),
        }
    }
    if !have_addr || o.segs.is_empty() {
        die(USAGE);
    }
    if o.threads == 0 || o.conns < o.threads {
        die("need at least one connection per thread");
    }
    o
}

fn pin(cpu: usize) {
    // SAFETY: cpu_set_t is plain data that CPU_ZERO/CPU_SET initialise in place, and
    // sched_setaffinity(0, ...) only reads it, for the calling thread.
    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        libc::CPU_ZERO(&mut set);
        libc::CPU_SET(cpu, &mut set);
        if libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &set) != 0 {
            die(&format!("cannot pin to CPU {cpu}: {}", std::io::Error::last_os_error()));
        }
    }
}

/// One sample of the server's memory: ms from the start, resident and file-backed KiB.
type MemSample = (f64, u64, u64);

fn statm(pid: u32, page_kib: u64) -> Option<(u64, u64)> {
    let s = std::fs::read_to_string(format!("/proc/{pid}/statm")).ok()?;
    let mut f = s.split_ascii_whitespace().skip(1).map(|x| x.parse::<u64>().ok());
    Some((f.next()?? * page_kib, f.next()?? * page_kib))
}

fn sample_memory(pid: u32, every: Duration, start: Instant, stop: Arc<AtomicBool>) -> Vec<MemSample> {
    let page_kib = 4;
    let mut out = Vec::new();
    let mut next = start;
    while !stop.load(Ordering::Relaxed) {
        let now = Instant::now();
        if now < next {
            std::thread::sleep(next - now);
        }
        let t = start.elapsed().as_secs_f64() * 1e3;
        match statm(pid, page_kib) {
            Some((rss, file)) => out.push((t, rss, file)),
            None => break,
        }
        next += every;
    }
    out
}

fn main() {
    let o = parse_opts();
    let h2_request = (o.proto == worker::Proto::H2)
        .then(|| h2::request(&o.host, &o.path, &o.content_type, &o.headers, &o.body, o.streams));
    let requests = match (o.proto, &o.requests) {
        (worker::Proto::Http, None) => vec![http::request(&o.host, &o.path, &o.content_type, &o.headers, &o.body)],
        (worker::Proto::Resp, Some(p)) => {
            let b = std::fs::read(p).unwrap_or_else(|e| die(&format!("{}: {e}", p.display())));
            let v = resp::split(&b).unwrap_or_else(|e| die(&format!("{}: {e}", p.display())));
            if v.is_empty() {
                die(&format!("{}: no commands", p.display()));
            }
            v
        }
        (worker::Proto::H2, None) => Vec::new(),
        (worker::Proto::Http | worker::Proto::H2, Some(_)) => die("--requests is for --proto resp"),
        (worker::Proto::Resp, None) => die("--proto resp needs --requests FILE"),
    };
    // Connections are opened before the start, so it is set far enough ahead for that.
    let start = Instant::now() + Duration::from_millis(300 + 2 * o.conns as u64);
    let start_mono_ns = monotonic_ns() + start.saturating_duration_since(Instant::now()).as_nanos() as u64;
    let plan_end = o.segs.last().unwrap().end_ns;
    let stop = Arc::new(AtomicBool::new(false));
    let sampler = o.pid.map(|pid| {
        let (stop, aux, every) = (stop.clone(), o.aux_cpu, Duration::from_millis(o.sample_ms));
        std::thread::spawn(move || {
            if let Some(c) = aux {
                pin(c);
            }
            sample_memory(pid, every, start, stop)
        })
    });
    if let Some(c) = o.aux_cpu {
        pin(c);
    }
    let results = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..o.threads)
            .map(|t| {
                let conns = o.conns / o.threads + usize::from(t < o.conns % o.threads);
                let cfg = worker::Config {
                    addr: o.addr,
                    requests: &requests,
                    proto: o.proto,
                    h2: h2_request.as_ref(),
                    segs: &o.segs,
                    conns,
                    threads: o.threads,
                    thread: t,
                    spacing: o.spacing,
                    seed: o.seed,
                    drain: Duration::from_secs_f64(o.drain_s),
                    start,
                };
                let cpu = (!o.cpus.is_empty()).then(|| o.cpus[t % o.cpus.len()]);
                scope.spawn(move || {
                    if let Some(c) = cpu {
                        pin(c);
                    }
                    worker::run(&cfg)
                })
            })
            .collect();
        let mut out = std::io::stdout().lock();
        for (i, s) in o.segs.iter().enumerate() {
            let at = start + Duration::from_nanos(s.begin_ns);
            if let Some(d) = at.checked_duration_since(Instant::now()) {
                std::thread::sleep(d);
            }
            let _ =
                writeln!(out, "MARK {:.3} {i} {} {}", start.elapsed().as_secs_f64() * 1e3, s.name, kind_name(s.kind));
            let _ = out.flush();
        }
        if let Some(d) = (start + Duration::from_nanos(plan_end)).checked_duration_since(Instant::now()) {
            std::thread::sleep(d);
        }
        let _ = writeln!(out, "MARK {:.3} end", start.elapsed().as_secs_f64() * 1e3);
        let _ = out.flush();
        handles.into_iter().map(|h| h.join().expect("worker panicked")).collect::<Vec<_>>()
    });
    stop.store(true, Ordering::Relaxed);
    let mem = sampler.map(|h| h.join().expect("sampler panicked")).unwrap_or_default();
    let mut recs = Vec::new();
    let (mut reconnects, mut unexpected) = (0, 0);
    for r in results {
        let (v, c) = r.unwrap_or_else(|e| die(&format!("worker: {e}")));
        recs.extend(v);
        reconnects += c.reconnects;
        unexpected += c.unexpected;
    }
    recs.sort_unstable_by_key(|r| r.due);
    if let Some(p) = &o.raw {
        let mut buf = Vec::with_capacity(recs.len() * 20);
        for r in &recs {
            buf.extend_from_slice(&r.due.to_le_bytes());
            buf.extend_from_slice(&r.lat.to_le_bytes());
            buf.extend_from_slice(&r.seg.to_le_bytes());
            buf.extend_from_slice(&r.flags.to_le_bytes());
        }
        std::fs::write(p, buf).unwrap_or_else(|e| die(&format!("{}: {e}", p.display())));
    }
    if let Some(p) = &o.mem {
        let mut s = String::from("ms,rss_kib,file_kib\n");
        for (t, rss, file) in &mem {
            s.push_str(&format!("{t:.1},{rss},{file}\n"));
        }
        std::fs::write(p, s).unwrap_or_else(|e| die(&format!("{}: {e}", p.display())));
    }
    let summary =
        stats::summary(&o.label, start_mono_ns, &o.segs, &recs, &mem, (o.head_ms * 1e6) as u64, reconnects, unexpected);
    if let Some(p) = &o.json {
        std::fs::write(p, format!("{summary}\n")).unwrap_or_else(|e| die(&format!("{}: {e}", p.display())));
    }
    println!("RESULT {summary}");
    // A run with failed requests, broken connections or stray responses measured something
    // other than the plan; say so in the exit status, after the summary that shows what.
    let errors = recs.iter().filter(|r| r.flags & worker::ERROR != 0).count();
    if errors > 0 || reconnects > 0 || unexpected > 0 {
        eprintln!(
            "loadgen: untrustworthy run: {errors} errors, {reconnects} reconnects, {unexpected} unexpected responses"
        );
        exit(1);
    }
}

/// CLOCK_MONOTONIC, which other tools (perf, Python's time.monotonic_ns) can line up with.
fn monotonic_ns() -> u64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: clock_gettime writes only the timespec it is given.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

fn kind_name(k: Kind) -> String {
    match k {
        Kind::Closed => "closed".into(),
        Kind::Off => "off".into(),
        Kind::Close => "close".into(),
        Kind::Rate(r) => format!("{r}"),
    }
}
