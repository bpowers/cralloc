//! Servers under an open load: the suite's load generator (bench/probes/loadgen) against a
//! server the runner started, and what the run reports.
//!
//! The generator runs a plan of segments (closed loop, a fixed request rate, silence, closed
//! connections) and prints `MARK <ms> <index> <name> <kind>` as each begins and `MARK <ms> end`
//! at the end, then `RESULT {json}` with latency percentiles, missed slots and errors for each
//! group of segments of one name. At every mark the runner reads the server's counters
//! (cycles, instructions, CPU time, page faults, all threads), so each group's counts per
//! request come from the server itself, over exactly that group's segments. The server's
//! memory comes from the runner's sampler, which reads it every `--sample-ms` (default 50 ms)
//! for the whole run; the report keeps a 250 ms curve of it from the first mark to the end,
//! through the silences and after the connections close.
use crate::launch::{Child, MemSample, Spec};
use crate::run::{Options, Snapshot, expand};
use crate::suite::{Job, OpenLoad};
use serde_json::{Map, Value, json};
use std::path::Path;
use std::time::{Duration, Instant};

/// Keeps a server's cores out of deep idle states for as long as it lives: one spinning
/// thread on the SMT sibling of each of the server's CPUs, at `SCHED_IDLE`, so that it gives
/// way to anything else on that hardware thread.
///
/// Why: at a fixed rate a server's cores go idle between requests, and the deepest state on
/// the reference machine (C3) takes about 350 us to leave. A faster allocator leaves more
/// idle time, so more of it in C3, and the latency tail then ranks the idle states, not the
/// allocator: with the cores held out of C3, p99.9 at 70% fell from 2 to 4 ms to 30 to 110 us
/// for every allocator (docs/research/server-latency.md). Disabling the state itself needs
/// root; a busy sibling holds its core in C0 the same way for every allocator. The spinner
/// shares the core, which raises medians by a few microseconds; every run's record says which
/// way it ran (`idle_states`).
pub struct Spinners {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    threads: Vec<std::thread::JoinHandle<()>>,
    pub cpus: Vec<usize>,
}

impl Spinners {
    pub fn start(server_cpus: &[usize]) -> Spinners {
        let cpus: Vec<usize> =
            server_cpus.iter().flat_map(|&c| crate::sys::siblings(c).into_iter().filter(move |&s| s != c)).collect();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let threads = cpus
            .iter()
            .map(|&cpu| {
                let stop = stop.clone();
                std::thread::spawn(move || {
                    bench_harness::cpus::pin_to(&[cpu]);
                    let p = libc::sched_param { sched_priority: 0 };
                    // SAFETY: a plain system call on the calling thread with a valid param.
                    unsafe { libc::sched_setscheduler(0, libc::SCHED_IDLE, &p) };
                    while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                        std::hint::spin_loop();
                    }
                })
            })
            .collect();
        Spinners { stop, threads, cpus }
    }
}

impl Drop for Spinners {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

/// One mark of the generator, with the server's state when the runner read it.
pub struct Mark {
    /// The segment that begins here; `None` at the end of the plan.
    pub segment: Option<usize>,
    pub name: String,
    /// When the runner saw the mark, in ms since the server started (the sampler's clock).
    pub t_ms: f64,
    pub snap: Snapshot,
    /// The program's live bytes, from a build that counts them (`LiveBytes`).
    pub live: Option<i64>,
}

/// Reads the live-bytes count of a server built with bench-alloc's `live-bytes` (its
/// `BENCH_LIVE_BYTES` table: 64 counters at a 64-byte stride) out of its memory.
pub struct LiveBytes {
    pid: i32,
    at: u64,
}

impl LiveBytes {
    /// The reader for `pid`, if its executable has the table.
    pub fn of(pid: i32) -> Option<LiveBytes> {
        let exe = std::fs::read_link(format!("/proc/{pid}/exe")).ok()?;
        let nm = std::process::Command::new("nm").arg(&exe).output().ok()?;
        let text = String::from_utf8_lossy(&nm.stdout);
        let value = text.lines().find(|l| l.ends_with(" BENCH_LIVE_BYTES"))?.split_whitespace().next()?;
        let value = u64::from_str_radix(value, 16).ok()?;
        // The executable's first mapping, at file offset 0, is where its addresses start.
        let maps = std::fs::read_to_string(format!("/proc/{pid}/maps")).ok()?;
        let base = maps.lines().find_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            (f.len() >= 6 && std::path::Path::new(f[5]) == exe && f[2] == "00000000")
                .then(|| u64::from_str_radix(f[0].split('-').next()?, 16).ok())
                .flatten()
        })?;
        Some(LiveBytes { pid, at: base + value })
    }

    pub fn read(&self) -> Option<i64> {
        use std::io::{Read, Seek, SeekFrom};
        let mut f = std::fs::File::open(format!("/proc/{}/mem", self.pid)).ok()?;
        f.seek(SeekFrom::Start(self.at)).ok()?;
        let mut b = [0u8; 64 * 64];
        f.read_exact(&mut b).ok()?;
        Some(b.chunks(64).map(|c| i64::from_ne_bytes(c[..8].try_into().unwrap())).sum())
    }
}

/// What one run under load produced.
pub struct Loaded {
    pub marks: Vec<Mark>,
    pub result: Value,
}

/// Other work on the server's CPUs in each counted segment, in CPUs: the task time of those
/// CPUs (`sys::TaskTime`) less the server's own (its task clock, every thread and worker
/// process), over the segment's length. Segments whose names begin with `_` are not counted,
/// and are left out.
pub fn foreign(l: &Loaded, cpus: &[usize]) -> Vec<(String, f64)> {
    l.marks
        .windows(2)
        .filter(|w| w[0].segment.is_some() && !w[0].name.starts_with('_'))
        .map(|w| {
            let (a, b) = (&w[0].snap, &w[1].snap);
            let own = b.counters.since(&a.counters).get("task_clock_ns").unwrap_or(0);
            let ns = b.at.duration_since(a.at).as_nanos() as u64;
            (w[0].name.clone(), b.cpu_task.foreign_since(&a.cpu_task, cpus, own, ns))
        })
        .collect()
}

/// Starts the generator against `addr`, records the server at each of its marks, and waits for
/// it to finish. Fails when the generator says the run cannot be trusted (its exit status) or
/// gives no result.
#[allow(clippy::too_many_arguments)]
pub fn drive(
    job: &Job,
    l: &OpenLoad,
    addr: &str,
    server: &Child,
    o: &Options,
    cgroup: &Path,
    scratch: &Path,
    deadline: Instant,
) -> Result<Loaded, String> {
    let bin = crate::apps::loadgen_bin(&o.bench_dir);
    let cpus = bench_harness::Cpus(l.cpus.clone()).to_list();
    let mut argv: Vec<String> = vec![bin.display().to_string()];
    let mut arg = |k: &str, v: String| {
        argv.push(k.to_string());
        argv.push(v);
    };
    arg("--addr", addr.to_string());
    match l.requests {
        // Another protocol: commands from a file, sent in turn.
        Some(file) => {
            arg("--proto", l.proto.to_string());
            arg("--requests", expand(file, job, o));
        }
        None => {
            if l.proto != "http" {
                arg("--proto", l.proto.to_string());
                arg("--streams", l.streams.to_string());
            }
            arg("--path", l.path.to_string());
            arg("--content-type", l.content_type.to_string());
            for h in &l.headers {
                arg("--header", h.to_string());
            }
            arg("--body", expand(l.body, job, o));
        }
    }
    arg("--conns", l.conns.to_string());
    arg("--threads", l.threads.to_string());
    arg("--cpus", cpus);
    arg("--plan", l.plan.clone());
    arg("--label", job.id.clone());
    let env: Vec<(String, String)> = std::env::vars().filter(|(k, _)| k == "PATH" || k == "HOME").collect();
    let spec = Spec {
        argv: &argv,
        env: &env,
        cpus: &l.cpus,
        stderr: &scratch.join("driver-stderr.txt"),
        count: false,
        cgroup,
    };
    let generator = Child::spawn(&spec).map_err(|e| format!("spawn {}: {e}", argv[0]))?;
    let mut marks = Vec::new();
    let mut result = None;
    let live = LiveBytes::of(server.pid);
    while let Ok((_, line)) = generator.lines.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        if let Some(rest) = line.strip_prefix("MARK ") {
            // Read first: the counters should change as little as possible after the mark.
            let snap = Snapshot::take(server);
            let f: Vec<&str> = rest.split_whitespace().collect();
            let t_ms = snap.at.duration_since(server.started).as_secs_f64() * 1e3;
            let (segment, name) = match f.get(1) {
                Some(&"end") => (None, "end".to_string()),
                Some(i) => (i.parse().ok(), f.get(2).unwrap_or(&"").to_string()),
                None => (None, String::new()),
            };
            marks.push(Mark { segment, name, t_ms, snap, live: live.as_ref().and_then(LiveBytes::read) });
        } else if let Some(j) = line.strip_prefix("RESULT ") {
            result = serde_json::from_str(j).ok();
        }
    }
    if !generator.wait_exit(Duration::from_secs(10)) {
        generator.signal(libc::SIGKILL);
        generator.reap();
        return Err("the load generator timed out".into());
    }
    let exit = generator.reap();
    if !exit.ok() {
        let why = std::fs::read_to_string(scratch.join("driver-stderr.txt")).unwrap_or_default();
        return Err(format!(
            "the load generator exited with {:?}/{:?}: {}",
            exit.code,
            exit.signal,
            why.lines().last().unwrap_or("")
        ));
    }
    let result = result.ok_or("the load generator printed no RESULT")?;
    Ok(Loaded { marks, result })
}

/// How the curve is thinned: one point per this many ms.
const CURVE_MS: f64 = 250.0;

/// The server's counts over each group of the generator's result, its memory under load and
/// after, and the curve. Returns the JSON for the run record and the cycles per request of
/// the group `rank`, the job's primary metric.
pub fn summarize(l: &Loaded, memory: &[MemSample], rank: &str) -> (Value, Option<f64>) {
    let mut groups = Map::new();
    let empty = Map::new();
    let gen_groups = l.result["groups"].as_object().unwrap_or(&empty);
    let rss_in = |a: f64, b: f64| memory.iter().filter(move |m| m.t_ms >= a && m.t_ms < b).map(|m| m.rss);
    let mut primary = None;
    for (name, g) in gen_groups {
        // The server's counts over every segment of this name: from its mark to the next.
        let (mut cycles, mut instructions, mut task_ns, mut faults, mut ms) = (0u64, 0u64, 0u64, 0u64, 0.0);
        let mut rss_max = 0;
        for w in l.marks.windows(2) {
            if w[0].segment.is_none() || &w[0].name != name {
                continue;
            }
            let d = w[1].snap.counters.since(&w[0].snap.counters);
            cycles += d.get("cycles").unwrap_or(0);
            instructions += d.get("instructions").unwrap_or(0);
            task_ns += d.get("task_clock_ns").unwrap_or(0);
            // The counter follows forks, so a server's worker processes are counted; the
            // process's own count is the fallback.
            faults += d.get("page_faults").unwrap_or(w[1].snap.stat.minflt - w[0].snap.stat.minflt);
            ms += w[1].t_ms - w[0].t_ms;
            rss_max = rss_max.max(rss_in(w[0].t_ms, w[1].t_ms).max().unwrap_or(0));
        }
        let n = g["lat_us"]["n"].as_f64().unwrap_or(0.0);
        let per = |x: u64| if n > 0.0 { Some(x as f64 / n) } else { None };
        let server = json!({
            "ms": ms,
            "cycles_per_req": per(cycles),
            "instructions_per_req": per(instructions),
            "cpu_us_per_req": per(task_ns).map(|v| v / 1e3),
            "faults_per_req": per(faults),
            "cpus_busy": if ms > 0.0 { task_ns as f64 / 1e6 / ms } else { 0.0 },
            "rss_max": rss_max,
        });
        if name == rank {
            primary = server["cycles_per_req"].as_f64();
        }
        let mut gj = g.clone();
        gj["server"] = server;
        groups.insert(name.clone(), gj);
    }
    // The curve, from the first mark to the last sample (after the end of the plan the
    // server is stopped, so it ends shortly after the last mark): the resident set, and the
    // anonymous part of it from the last rollup (once a second), which for a server that maps
    // its store (Meilisearch) is what the allocator holds.
    let t0 = l.marks.first().map_or(0.0, |m| m.t_ms);
    let mut curve = Vec::new();
    let mut next = t0;
    let mut anon = memory.iter().find_map(|m| m.rollup).map_or(0, |r| r.anonymous);
    for m in memory.iter() {
        anon = m.rollup.map_or(anon, |r| r.anonymous);
        if m.t_ms < t0 {
            continue;
        }
        if m.t_ms >= next {
            curve.push(json!([((m.t_ms - t0) * 10.0).round() / 10.0, m.rss / 1024, anon / 1024]));
            // The first sample in each 250 ms step of the grid that starts at the first mark.
            next = t0 + ((m.t_ms - t0) / CURVE_MS).floor() * CURVE_MS + CURVE_MS;
        }
    }
    let marks: Vec<Value> = l
        .marks
        .iter()
        .map(|m| match m.live {
            Some(live) => json!([((m.t_ms - t0) * 10.0).round() / 10.0, m.name, live]),
            None => json!([((m.t_ms - t0) * 10.0).round() / 10.0, m.name]),
        })
        .collect();
    // Under load: every segment that sends requests. After: from the last silence or close.
    let loaded_max = l
        .marks
        .windows(2)
        .filter(|w| w[0].segment.is_some() && !matches!(w[0].name.as_str(), "off" | "idle"))
        .filter_map(|w| rss_in(w[0].t_ms, w[1].t_ms).max())
        .max();
    let v = json!({
        "groups": groups,
        "rss_max_loaded": loaded_max,
        "curve_ms": CURVE_MS,
        "curve_kib": curve,
        "marks_ms": marks,
        "generator": {
            "reconnects": l.result["reconnects"],
            "unexpected_responses": l.result["unexpected_responses"],
            "segments": l.result["segments"],
        },
    });
    (v, primary)
}
