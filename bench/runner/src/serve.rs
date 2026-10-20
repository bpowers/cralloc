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

/// One mark of the generator, with the server's state when the runner read it.
pub struct Mark {
    /// The segment that begins here; `None` at the end of the plan.
    pub segment: Option<usize>,
    pub name: String,
    /// When the runner saw the mark, in ms since the server started (the sampler's clock).
    pub t_ms: f64,
    pub snap: Snapshot,
}

/// What one run under load produced.
pub struct Loaded {
    pub marks: Vec<Mark>,
    pub result: Value,
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
    arg("--path", l.path.to_string());
    arg("--content-type", l.content_type.to_string());
    for h in &l.headers {
        arg("--header", h.to_string());
    }
    arg("--body", expand(l.body, job, o));
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
            marks.push(Mark { segment, name, t_ms, snap });
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
            faults += w[1].snap.stat.minflt - w[0].snap.stat.minflt;
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
    // server is stopped, so it ends shortly after the last mark).
    let t0 = l.marks.first().map_or(0.0, |m| m.t_ms);
    let mut curve = Vec::new();
    let mut next = t0;
    for m in memory.iter().filter(|m| m.t_ms >= t0) {
        if m.t_ms >= next {
            curve.push(json!([((m.t_ms - t0) * 10.0).round() / 10.0, m.rss / 1024]));
            // The first sample in each 250 ms step of the grid that starts at the first mark.
            next = t0 + ((m.t_ms - t0) / CURVE_MS).floor() * CURVE_MS + CURVE_MS;
        }
    }
    let marks: Vec<Value> = l.marks.iter().map(|m| json!([((m.t_ms - t0) * 10.0).round() / 10.0, m.name])).collect();
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
