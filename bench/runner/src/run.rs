//! A measurement session: rounds of every job under every allocator, in a fresh random order
//! each round, one fresh process per run, one JSON line per run.
use crate::cap::{self, Group, Scope};
use crate::config::Allocator;
use crate::launch::{Child, Exit, MemSample, ProcStat, Sampler, Spec};
use crate::suite::{Job, Metric, Mode, Program};
use crate::sys::{self, CpuTimes};
use bench_harness::mem::Rollup;
use bench_harness::perf::Sample;
use bench_harness::rng::Rng;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

pub struct Options {
    pub bench_dir: PathBuf,
    pub baselines: PathBuf,
    pub out: PathBuf,
    /// The session's capped scope; every process the session starts runs in a group of it.
    pub scope: Scope,
    /// Each measured process's memory cap.
    pub mem_cap: u64,
    pub rounds: usize,
    pub seed: u64,
    /// CPUs the runner and its samplers use; kept out of every job.
    pub runner_cpus: Vec<usize>,
    pub sample_ms: u64,
    /// How often to read the full smaps_rollup (huge pages, PSS).
    pub rollup_ms: u64,
    /// Wait before each job until no more than this many CPUs' worth of other work is
    /// running outside the measurement CPUs and their siblings...
    pub max_busy: f64,
    /// ...but no longer than this.
    pub gate_wait: Duration,
    pub suite: String,
}

/// Environment variables that change allocator behaviour; the runner's own values are never
/// passed on, so that only the allocator entry decides them.
const SCRUB: &[&str] = &[
    "LD_PRELOAD",
    "GLIBC_TUNABLES",
    "MALLOC_CONF",
    "_RJEM_MALLOC_CONF",
    "MALLOC_ARENA_MAX",
    "MALLOC_TOP_PAD_",
    "MALLOC_TRIM_THRESHOLD_",
    "MALLOC_MMAP_THRESHOLD_",
];

fn scrubbed(k: &str) -> bool {
    SCRUB.contains(&k) || k.starts_with("MIMALLOC_") || k.starts_with("TCMALLOC_") || k.starts_with("BENCH_")
}

pub fn runs_under(job: &Job, a: &Allocator) -> bool {
    match job.mode {
        Mode::Native => a.feature.is_some(),
        Mode::Preload => a.preload.is_some(),
    }
}

fn rust_bin(bench_dir: &Path, build: &str, bin: &str) -> PathBuf {
    bench_dir.join("target").join(build).join("release").join(bin)
}

/// The command line and environment for one run.
fn command(job: &Job, a: &Allocator, o: &Options, scratch: &Path) -> (Vec<String>, Vec<(String, String)>) {
    let cbench = o.baselines.join("cbench");
    let cb = cbench.display().to_string();
    let program = match (&job.program, job.mode) {
        (Program::Rust(b), Mode::Native) => rust_bin(&o.bench_dir, &a.build_name().unwrap(), b),
        (Program::Rust(b), Mode::Preload) => rust_bin(&o.bench_dir, "glibc", b),
        (Program::C(p), _) => cbench.join(p),
    };
    let mut argv = vec![program.display().to_string()];
    argv.extend(job.args.iter().map(|s| s.replace("{cbench}", &cb)));
    let mut env: Vec<(String, String)> = std::env::vars().filter(|(k, _)| !scrubbed(k)).collect();
    env.extend(a.env.iter().map(|(k, v)| (k.clone(), v.clone())));
    env.push(("BENCH_CPUS".into(), bench_harness::Cpus(job.cpus.clone()).to_list()));
    if job.mode == Mode::Preload {
        let probe = o.baselines.join("probe/libbenchprobe.so").display().to_string();
        let lib = a.preload.clone().unwrap_or_default();
        let pre = if lib.is_empty() { probe } else { format!("{lib}:{probe}") };
        env.push(("LD_PRELOAD".into(), pre));
        env.push(("BENCH_PROBE_OUT".into(), scratch.join("probe.json").display().to_string()));
    }
    (argv, env)
}

/// What a running process looked like at one instant, for the windows a driver marks.
#[derive(Clone)]
struct Snapshot {
    at: Instant,
    counters: Sample,
    stat: ProcStat,
}

impl Snapshot {
    fn take(c: &Child) -> Snapshot {
        Snapshot { at: Instant::now(), counters: c.read_counters(), stat: ProcStat::read(c.pid) }
    }
}

struct Outcome {
    exit: Exit,
    timed_out: bool,
    counters: Sample,
    memory: Vec<MemSample>,
    result: Option<Value>,
    stdout_tail: Vec<String>,
    driver: Option<Value>,
    window: Option<Value>,
    error: Option<String>,
}

impl Outcome {
    /// A run that could not be started.
    fn not_started(error: String) -> Outcome {
        Outcome {
            exit: Exit::default(),
            timed_out: false,
            counters: Sample::default(),
            memory: vec![],
            result: None,
            stdout_tail: vec![],
            driver: None,
            window: None,
            error: Some(error),
        }
    }
}

/// The cgroups of one run: the measured process's, and the driver's if the job has one.
struct Groups {
    run: Group,
    driver: Option<Group>,
}

impl Groups {
    fn create(job: &Job, o: &Options, seq: usize) -> std::io::Result<Groups> {
        let run = o.scope.group(&format!("run-{seq}"), o.mem_cap)?;
        let driver = match job.driver {
            Some(_) => Some(o.scope.group(&format!("driver-{seq}"), cap::DRIVER)?),
            None => None,
        };
        Ok(Groups { run, driver })
    }
}

fn parse_result(line: &str) -> Option<Value> {
    line.strip_prefix("BENCH_RESULT ").and_then(|j| serde_json::from_str(j).ok())
}

/// Runs the measured process (and its driver, if any) to completion.
fn execute(job: &Job, argv: &[String], env: &[(String, String)], scratch: &Path, o: &Options, g: &Groups) -> Outcome {
    let timeout = Duration::from_secs(job.timeout_s);
    let procs = g.run.procs();
    let spec = Spec { argv, env, cpus: &job.cpus, stderr: &scratch.join("stderr.txt"), count: true, cgroup: &procs };
    let child = match Child::spawn(&spec) {
        Ok(c) => c,
        Err(e) => return Outcome::not_started(format!("spawn {}: {e}", argv[0])),
    };
    let sampler = Sampler::start(
        child.pid,
        child.started,
        Duration::from_millis(o.sample_ms),
        Duration::from_millis(o.rollup_ms),
    );
    let deadline = child.started + timeout;
    let mut result = None;
    let mut tail = Vec::new();
    let mut error = None;
    let mut driver_result = None;
    let mut window = None;
    // The last 200 lines: results are printed at the end.
    let keep = |line: String, tail: &mut Vec<String>| {
        if tail.len() == 200 {
            tail.remove(0);
        }
        tail.push(line);
    };

    if let Some(d) = &job.driver {
        // Wait for the server's address, then run the driver against it.
        let mut addr = None;
        while addr.is_none() {
            match child.lines.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok((_, l)) => match l.strip_prefix("BENCH_LISTEN ") {
                    Some(a) => addr = Some(a.trim().to_string()),
                    None => keep(l, &mut tail),
                },
                Err(_) => break,
            }
        }
        match addr {
            None => error = Some("server never printed BENCH_LISTEN".to_string()),
            Some(addr) => {
                let dbin = rust_bin(&o.bench_dir, "glibc", d.bin).display().to_string();
                let mut dargv = vec![dbin];
                dargv.extend(d.args.iter().map(|s| s.replace("{addr}", &addr)));
                let mut denv: Vec<(String, String)> = std::env::vars().filter(|(k, _)| !scrubbed(k)).collect();
                denv.push(("BENCH_CPUS".into(), bench_harness::Cpus(d.cpus.clone()).to_list()));
                let dprocs = g.driver.as_ref().expect("a job with a driver has a driver cgroup").procs();
                let dspec = Spec {
                    argv: &dargv,
                    env: &denv,
                    cpus: &d.cpus,
                    stderr: &scratch.join("driver-stderr.txt"),
                    count: false,
                    cgroup: &dprocs,
                };
                match Child::spawn(&dspec) {
                    Err(e) => error = Some(format!("spawn driver: {e}")),
                    Ok(drv) => {
                        let (mut warm, mut done) = (None, None);
                        loop {
                            match drv.lines.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                                Ok((_, l)) if l == "BENCH_MARK warm" => warm = Some(Snapshot::take(&child)),
                                Ok((_, l)) if l == "BENCH_MARK done" => done = Some(Snapshot::take(&child)),
                                Ok((_, l)) => {
                                    if let Some(r) = parse_result(&l) {
                                        driver_result = Some(r);
                                    }
                                }
                                Err(_) => break,
                            }
                        }
                        if !drv.wait_exit(Duration::from_secs(10)) {
                            drv.signal(libc::SIGKILL);
                            error = Some("driver timed out".into());
                        }
                        let dexit = drv.reap();
                        if !dexit.ok() && error.is_none() {
                            error = Some(format!("driver exited with {:?}/{:?}", dexit.code, dexit.signal));
                        }
                        if let (Some(w), Some(d)) = (warm, done) {
                            let c = d.counters.since(&w.counters);
                            let wall = d.at.duration_since(w.at).as_nanos() as u64;
                            let ticks =
                                (d.stat.utime_ticks + d.stat.stime_ticks) - (w.stat.utime_ticks + w.stat.stime_ticks);
                            let mut cj = c.to_json();
                            cj["wall_ns"] = wall.into();
                            cj["cpu_ticks"] = ticks.into();
                            cj["minflt"] = (d.stat.minflt - w.stat.minflt).into();
                            window = Some(cj);
                        }
                    }
                }
            }
        }
        child.signal(libc::SIGTERM);
    }

    let mut timed_out = false;
    // Read the measured process's output until it closes stdout or the deadline passes.
    loop {
        match child.lines.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok((_, l)) => match parse_result(&l) {
                Some(r) => result = Some(r),
                None => keep(l, &mut tail),
            },
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                timed_out = true;
                break;
            }
        }
    }
    let grace = if timed_out {
        Duration::ZERO
    } else {
        deadline.saturating_duration_since(Instant::now()).max(Duration::from_secs(5))
    };
    if timed_out || !child.wait_exit(grace) {
        timed_out = true;
        child.signal(libc::SIGKILL);
        child.wait_exit(Duration::from_secs(10));
    }
    let memory = sampler.stop();
    let counters = child.read_counters();
    let exit = child.reap();
    Outcome { exit, timed_out, counters, memory, result, stdout_tail: tail, driver: driver_result, window, error }
}

fn parse_after(lines: &[String], key: &str) -> Option<f64> {
    lines.iter().rev().find_map(|l| {
        let rest = &l[l.find(key)? + key.len()..];
        let num: String = rest
            .trim_start()
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == 'e' || *c == '-')
            .collect();
        num.parse().ok()
    })
}

/// `reported_peak` is the VmHWM the process reported at its exit, from its result line or
/// the preload probe; without it the peak is the last VmHWM the sampler saw, which can miss
/// the final interval.
fn memory_json(samples: &[MemSample], reported_peak: Option<u64>, o: &Options) -> Value {
    let n = samples.len();
    let max = |f: fn(&MemSample) -> u64| samples.iter().map(f).max().unwrap_or(0);
    let rollups: Vec<Rollup> = samples.iter().filter_map(|s| s.rollup).collect();
    let rmax = |f: fn(&Rollup) -> u64| rollups.iter().map(f).max().unwrap_or(0);
    let rmean = |f: fn(&Rollup) -> u64| {
        if rollups.is_empty() { 0.0 } else { rollups.iter().map(f).sum::<u64>() as f64 / rollups.len() as f64 }
    };
    let (peak, peak_source) = match reported_peak {
        Some(p) if p > 0 => (p, "process"),
        _ => (max(|s| s.hwm), "sampler"),
    };
    let mean =
        |f: fn(&MemSample) -> u64| if n == 0 { 0.0 } else { samples.iter().map(f).sum::<u64>() as f64 / n as f64 };
    // Keep at most 240 points of the series, evenly spaced, in KiB.
    let step = n.div_ceil(240).max(1);
    // Huge-page use is known only at rollup samples; carry the last one forward.
    let mut huge = 0;
    let series: Vec<Value> = samples
        .iter()
        .map(|s| {
            huge = s.rollup.map_or(huge, |r| r.anon_huge);
            json!([(s.t_ms * 10.0).round() / 10.0, s.rss / 1024, huge / 1024])
        })
        .step_by(step)
        .collect();
    json!({
        "samples": n,
        "interval_ms": o.sample_ms,
        "rollups": rollups.len(),
        "rollup_ms": o.rollup_ms,
        "rss_peak": peak,
        "rss_peak_source": peak_source,
        "rss_sampled_peak": max(|s| s.rss),
        "rss_mean": mean(|s| s.rss),
        "pss_peak": rmax(|r| r.pss),
        "anon_huge_peak": rmax(|r| r.anon_huge),
        "anon_huge_mean": rmean(|r| r.anon_huge),
        "series_kib": series,
    })
}

struct Context<'a> {
    o: &'a Options,
    session: String,
    round: usize,
    seq: usize,
    gate: Value,
    cpus: &'a CpuSets,
}

/// The machine's CPUs as the runner sees them.
struct CpuSets {
    /// Every CPU a job may use.
    meas: BTreeSet<usize>,
    /// Their SMT siblings, which must stay idle.
    siblings: BTreeSet<usize>,
    /// Everything else but the runner's own CPUs.
    others: BTreeSet<usize>,
}

fn run_one(job: &Job, a: &Allocator, cx: &Context) -> Value {
    let scratch = cx.o.out.with_extension("work");
    let _ = std::fs::create_dir_all(&scratch);
    let _ = std::fs::remove_file(scratch.join("probe.json"));
    let (argv, env) = command(job, a, cx.o, &scratch);
    let load = sys::loadavg();
    let t0 = CpuTimes::read();
    let started = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs_f64();
    // Nothing starts without its cgroup, so nothing runs uncapped.
    let (out, usage, driver_usage) = match Groups::create(job, cx.o, cx.seq) {
        Ok(g) => {
            let out = execute(job, &argv, &env, &scratch, cx.o, &g);
            (out, Some(g.run.usage()), g.driver.as_ref().map(Group::usage))
        }
        Err(e) => (Outcome::not_started(format!("creating the run's cgroup: {e}")), None, None),
    };
    let busy = CpuTimes::read().busy_since(&t0);

    // What the process says it ran with: its own report when native, the probe when
    // preloaded.
    let self_report = match job.mode {
        Mode::Native => out.result.clone(),
        Mode::Preload => std::fs::read_to_string(scratch.join("probe.json"))
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok()),
    };
    let reported = self_report.as_ref().map(|r| {
        let s = |k: &str| r[k].as_str().unwrap_or("").to_string();
        (s("allocator"), s("mode"), s("library"), s("version"))
    });
    let reported_peak = self_report.as_ref().and_then(|r| r["vm_hwm"].as_u64());
    let mut problems = Vec::new();
    // The cap's verdict first: a process killed at its cap also fails every check below.
    if let Some(p) = usage.and_then(|u| u.problem("the measured process")) {
        problems.push(p);
    }
    if let Some(p) = driver_usage.and_then(|u| u.problem("the driver")) {
        problems.push(p);
    }
    if let Some(e) = &out.error {
        problems.push(e.clone());
    }
    if out.timed_out {
        problems.push("timed out".into());
    }
    let server = job.driver.is_some();
    // A server is stopped with SIGTERM and exits 0 when it shuts down cleanly.
    if !out.exit.ok() && !out.timed_out {
        problems.push(format!("exit {:?} signal {:?}", out.exit.code, out.exit.signal));
    }
    match &reported {
        None => problems.push("no allocator report".into()),
        Some((alloc, mode, library, version)) => {
            if let Some(p) = library_problem(job.mode, a, library) {
                problems.push(p);
            }
            // A preload run reports the preloaded library's version, which may differ.
            if job.mode == Mode::Native
                && let Some(e) = &a.expect.version
                && e != version
            {
                problems.push(format!("expected version {e}, process reported {version:?}"));
            }
            if let Some(e) = &a.expect.allocator
                && e != alloc
            {
                problems.push(format!("expected allocator {e}, process reported {alloc}"));
            }
            if let Some(e) = &a.expect.mode
                && e != mode
            {
                problems.push(format!("expected mode {e}, process reported {mode:?}"));
            }
        }
    }
    let mut extra = serde_json::Map::new();
    if server && let (Some(w), Some(d)) = (&out.window, &out.driver) {
        let requests = d["metrics"]["requests"].as_f64().unwrap_or(0.0);
        // A missing count leaves the metric out, which makes the run invalid, rather than
        // reading as zero cycles.
        if requests > 0.0
            && let Some(cycles) = w["cycles"].as_f64()
        {
            extra.insert("server_cycles_per_req".into(), (cycles / requests).into());
            extra.insert(
                "server_instructions_per_req".into(),
                (w["instructions"].as_f64().unwrap_or(0.0) / requests).into(),
            );
        }
        if let Some(rps) = d["primary"]["value"].as_f64() {
            extra.insert("req_per_s".into(), rps.into());
        }
        let busy = w["task_clock_ns"].as_f64().unwrap_or(0.0) / w["wall_ns"].as_f64().unwrap_or(1.0);
        extra.insert("server_cpus_busy".into(), busy.into());
    }
    let primary = match &job.metric {
        Metric::Reported => out.result.as_ref().and_then(|r| r.get("primary")).filter(|p| !p.is_null()).cloned(),
        Metric::ServerCycles => extra
            .get("server_cycles_per_req")
            .map(|v| json!({ "name": "server cycles/req", "value": v, "better": "lower" })),
        Metric::Wall => Some(json!({ "name": "s", "value": out.exit.wall_ns as f64 / 1e9, "better": "lower" })),
        Metric::Parsed { key, name, higher_better, scale } => parse_after(&out.stdout_tail, key).map(
            |v| json!({ "name": name, "value": v * scale, "better": if *higher_better { "higher" } else { "lower" } }),
        ),
    };
    if primary.is_none() {
        problems.push("no primary metric".into());
    }
    let multiplexed = out.counters.multiplexed
        || out
            .result
            .as_ref()
            .is_some_and(|r| r["phases"].as_object().is_some_and(|p| p.values().any(|ph| ph["multiplexed"] == true)));
    // A multiplexed hardware counter is an estimate (the count is not scaled), so the run
    // cannot be ranked by cycles; it usually means another perf user shared the core.
    if multiplexed {
        problems.push("hardware counters were multiplexed".into());
    }
    let stderr_tail = std::fs::read_to_string(scratch.join("stderr.txt"))
        .map(|s| s.lines().rev().take(5).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n"))
        .unwrap_or_default();
    json!({
        "type": "run",
        "session": cx.session,
        "round": cx.round,
        "seq": cx.seq,
        "job": job.id,
        "group": job.group,
        "mode": job.mode.name(),
        "allocator": a.name,
        "control_of": a.control_of,
        "argv": argv,
        "cpus": bench_harness::Cpus(job.cpus.clone()).to_list(),
        "started": started,
        "valid": problems.is_empty(),
        "problems": problems,
        "reported": self_report.as_ref().map(|r| json!({
            "allocator": r["allocator"], "mode": r["mode"], "library": r["library"], "version": r["version"],
        })),
        "primary": primary,
        "extra": extra,
        "multiplexed": multiplexed,
        "exit": { "code": out.exit.code, "signal": out.exit.signal, "timed_out": out.timed_out },
        "wall_ns": out.exit.wall_ns,
        "process": out.counters.to_json(),
        "rusage": {
            "utime_ns": out.exit.utime_ns, "stime_ns": out.exit.stime_ns, "maxrss": out.exit.maxrss,
            "minflt": out.exit.minflt, "majflt": out.exit.majflt, "nvcsw": out.exit.nvcsw, "nivcsw": out.exit.nivcsw,
        },
        "memory": memory_json(&out.memory, reported_peak, cx.o),
        "cgroup": usage.map(|u| u.to_json()),
        "driver_cgroup": driver_usage.map(|u| u.to_json()),
        "workload": out.result,
        "driver": out.driver,
        "window": out.window,
        "interference": {
            "loadavg_before": load,
            "siblings_busy": sys::busy_sum(&busy, &cx.cpus.siblings),
            "others_busy": sys::busy_sum(&busy, &cx.cpus.others),
            // Work on this job's CPUs that was not this job's: busy time there minus the
            // process's own CPU time, both over its lifetime. Jiffies, so coarse.
            "foreign_on_job_cpus": sys::busy_sum(&busy, &job.cpus.iter().copied().collect())
                - (out.exit.utime_ns + out.exit.stime_ns) as f64 / out.exit.wall_ns.max(1) as f64,
            "gate": cx.gate,
        },
        "stderr_tail": stderr_tail,
    })
}

/// Whether the process's allocator code came from where the entry says it must: for a
/// preload run, the preloaded library itself; otherwise `expect.library`, if the entry gives
/// one ("executable", or a shared library's file name).
fn library_problem(mode: Mode, a: &Allocator, reported: &str) -> Option<String> {
    if mode == Mode::Preload
        && let Some(lib) = a.preload.as_deref().filter(|l| !l.is_empty())
    {
        let want = std::fs::canonicalize(lib).map(|p| p.display().to_string()).unwrap_or_else(|_| lib.to_string());
        return (reported != want).then(|| format!("preloaded {want}, but malloc was served from {reported:?}"));
    }
    let want = a.expect.library.as_deref()?;
    let ok = reported == want || (want != "executable" && Path::new(reported).file_name().is_some_and(|n| n == want));
    (!ok).then(|| format!("expected the allocator in {want}, process reported it in {reported:?}"))
}

/// Waits until the machine is quiet enough, or gives up: nothing else on the measurement
/// CPUs or their siblings (other processes are not confined and may land there), and no
/// more than `max_busy` CPUs' worth of work elsewhere.
fn gate(o: &Options, cpus: &CpuSets) -> Value {
    let t0 = Instant::now();
    loop {
        let a = CpuTimes::read();
        std::thread::sleep(Duration::from_secs(1));
        let busy = CpuTimes::read().busy_since(&a);
        let (ob, sb, mb) = (
            sys::busy_sum(&busy, &cpus.others),
            sys::busy_sum(&busy, &cpus.siblings),
            sys::busy_sum(&busy, &cpus.meas),
        );
        let waited = t0.elapsed();
        let quiet = ob <= o.max_busy && sb <= 0.25 && mb <= 0.25;
        if quiet || waited >= o.gate_wait {
            if !quiet {
                eprintln!(
                    "  gate: still busy after {:.0} s (others {ob:.2} CPUs, siblings {sb:.2}, measurement CPUs {mb:.2}); \
                     running anyway",
                    waited.as_secs_f64()
                );
            }
            return json!({
                "waited_s": waited.as_secs_f64(), "others_busy": ob, "siblings_busy": sb, "measurement_busy": mb,
                "quiet": quiet,
            });
        }
        std::thread::sleep(Duration::from_secs(4));
    }
}

fn header(o: &Options, allocs: &[Allocator], jobs: &[Job], session: &str) -> Value {
    let manifests: serde_json::Map<String, Value> = std::fs::read_dir(o.baselines.join("manifest"))
        .map(|d| {
            d.filter_map(|e| e.ok())
                .filter(|e| e.path().extension().is_some_and(|x| x == "txt"))
                .filter_map(|e| {
                    Some((
                        e.file_name().to_string_lossy().into_owned(),
                        Value::String(std::fs::read_to_string(e.path()).ok()?),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    let commit = std::process::Command::new("git")
        .args(["-C", &o.bench_dir.display().to_string(), "rev-parse", "HEAD"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    json!({
        "type": "session",
        "session": session,
        "suite": o.suite,
        "rounds": o.rounds,
        "seed": o.seed,
        "commit": commit,
        "kernel": sys::read_trim("/proc/sys/kernel/osrelease"),
        "cpu": sys::cpu_model(),
        "governor": sys::read_trim("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor"),
        "epp": sys::read_trim("/sys/devices/system/cpu/cpu0/cpufreq/energy_performance_preference"),
        "boost": sys::read_trim("/sys/devices/system/cpu/cpufreq/boost"),
        "thp": sys::read_trim("/sys/kernel/mm/transparent_hugepage/enabled"),
        "thp_defrag": sys::read_trim("/sys/kernel/mm/transparent_hugepage/defrag"),
        "perf_event_paranoid": sys::read_trim("/proc/sys/kernel/perf_event_paranoid"),
        "nmi_watchdog": sys::read_trim("/proc/sys/kernel/nmi_watchdog"),
        "l3_domains": [sys::l3_domain(crate::suite::L3A[0]), sys::l3_domain(crate::suite::L3B[0])],
        "runner_cpus": o.runner_cpus,
        "sample_ms": o.sample_ms,
        "rollup_ms": o.rollup_ms,
        "max_busy": o.max_busy,
        "mem_cap": o.mem_cap,
        "driver_mem_cap": cap::DRIVER,
        "scope": { "unit": o.scope.unit, "mem_cap": o.scope.cap },
        "baselines": o.baselines,
        "allocators": allocs.iter().map(|a| json!({
            "name": a.name, "feature": a.feature, "preload": a.preload, "env": a.env,
            "expect": {
                "allocator": a.expect.allocator, "mode": a.expect.mode, "library": a.expect.library,
                "version": a.expect.version,
            },
            "control_of": a.control_of,
        })).collect::<Vec<_>>(),
        "jobs": jobs.iter().map(|j| json!({ "id": j.id, "group": j.group, "mode": j.mode.name() })).collect::<Vec<_>>(),
        "manifests": manifests,
    })
}

pub fn session(o: &Options, allocs: &[Allocator], jobs: &[Job]) -> std::io::Result<()> {
    bench_harness::cpus::pin_to(&o.runner_cpus);
    if let Some(p) = o.out.parent() {
        std::fs::create_dir_all(p)?;
    }
    let mut out = std::fs::OpenOptions::new().create(true).append(true).open(&o.out)?;
    let session =
        format!("{}-{:x}", SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs(), o.seed);
    writeln!(out, "{}", header(o, allocs, jobs, &session))?;
    let meas: BTreeSet<usize> = crate::suite::measurement_cpus().into_iter().collect();
    let siblings: BTreeSet<usize> = meas.iter().flat_map(|&c| sys::siblings(c)).filter(|c| !meas.contains(c)).collect();
    let runner: BTreeSet<usize> = o.runner_cpus.iter().flat_map(|&c| sys::siblings(c)).collect();
    let others: BTreeSet<usize> = sys::online_cpus()
        .into_iter()
        .filter(|c| !meas.contains(c) && !siblings.contains(c) && !runner.contains(c))
        .collect();
    let cpus = CpuSets { meas, siblings, others };
    let mut rng = Rng::new(o.seed);
    let total: usize =
        jobs.iter().map(|j| allocs.iter().filter(|a| runs_under(j, a)).count()).sum::<usize>() * o.rounds;
    let mut seq = 0;
    let t0 = Instant::now();
    for round in 0..o.rounds {
        for job in jobs {
            let mut order: Vec<&Allocator> = allocs.iter().filter(|a| runs_under(job, a)).collect();
            for i in (1..order.len()).rev() {
                order.swap(i, rng.below(i as u64 + 1) as usize);
            }
            let g = gate(o, &cpus);
            for a in order {
                let cx = Context { o, session: session.clone(), round, seq, gate: g.clone(), cpus: &cpus };
                let rec = run_one(job, a, &cx);
                writeln!(out, "{rec}")?;
                out.flush()?;
                seq += 1;
                let p = &rec["primary"];
                eprintln!(
                    "[{seq}/{total} {:.0}s] r{round} {:<40} {:<16} {:>12} {}{}",
                    t0.elapsed().as_secs_f64(),
                    format!("{} ({})", job.id, job.mode.name()),
                    a.name,
                    p["value"].as_f64().map(|v| format!("{v:.4}")).unwrap_or_else(|| "-".into()),
                    p["name"].as_str().unwrap_or(""),
                    if rec["valid"] == true { String::new() } else { format!("  INVALID {}", rec["problems"]) }
                );
            }
        }
    }
    Ok(())
}
