//! The benchmark runner.
//!
//!   benchrun build  [--allocators a,b]
//!   benchrun build-apps [--apps a,b] [--allocators a,b]
//!   benchrun run    [--suite quick|full|backend|apps|servers|servers-sat] [--rounds N] [--only TEXT] [--allocators a,b]
//!                   [--out FILE] [--seed N] [--max-busy CPUS] [--gate-wait SECONDS]
//!                   [--runner-cpus LIST] [--sample-ms N] [--mem-cap SIZE] [--no-warm] [--idle-states held|free]
//!   benchrun report FILE... [--detail] [--ref NAME] [--only TEXT] [--quiet-only] [--steady-only]
//!   benchrun list   [--suite quick|full|backend|apps|servers|servers-sat]
//!   benchrun layout --only TEXT|--group NAME [--suite S] [--allocators a,b] [--layouts L,...]
//!                   [--rounds N] [--no-build] [run's other options]
//!
//! `--suite` takes several suites joined by commas, `--only` several texts, and `run`, `list`
//! and `layout` take `--group NAME` (for example `single-thread`).
//!
//! See bench/README.md, and bench/apps/README.md for the apps suite.
mod apps;
mod cap;
mod clock;
mod config;
mod launch;
mod layout;
mod paths;
mod report;
mod run;
mod serve;
mod served;
mod stats;
mod suite;
mod sys;

use paths::Paths;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

struct Flags {
    pos: Vec<String>,
    named: Vec<(String, Option<String>)>,
}

impl Flags {
    fn parse(args: impl Iterator<Item = String>) -> Flags {
        let mut pos = Vec::new();
        let mut named = Vec::new();
        let mut it = args.peekable();
        while let Some(a) = it.next() {
            if let Some(name) = a.strip_prefix("--") {
                if let Some((k, v)) = name.split_once('=') {
                    named.push((k.to_string(), Some(v.to_string())));
                } else if it.peek().is_some_and(|n| !n.starts_with("--"))
                    && !matches!(name, "detail" | "quiet-only" | "no-build" | "no-warm" | "steady-only")
                {
                    named.push((name.to_string(), it.next()));
                } else {
                    named.push((name.to_string(), None));
                }
            } else {
                pos.push(a);
            }
        }
        Flags { pos, named }
    }

    fn get(&self, k: &str) -> Option<&str> {
        self.named.iter().rev().find(|(n, _)| n == k).and_then(|(_, v)| v.as_deref())
    }

    fn has(&self, k: &str) -> bool {
        self.named.iter().any(|(n, _)| n == k)
    }

    fn num<T: std::str::FromStr>(&self, k: &str, default: T) -> T {
        self.get(k).map(|v| v.parse().unwrap_or_else(|_| die(&format!("--{k} {v}: not a number")))).unwrap_or(default)
    }
}

fn die(msg: &str) -> ! {
    eprintln!("benchrun: {msg}");
    std::process::exit(2)
}

fn allocators(f: &Flags, p: &Paths) -> Vec<config::Allocator> {
    let all = config::load(&p.bench, &p.baselines).unwrap_or_else(|e| die(&e));
    match f.get("allocators") {
        None => all,
        Some(list) => {
            let want: Vec<&str> = list.split(',').collect();
            for w in &want {
                if !all.iter().any(|a| a.name == *w) {
                    die(&format!("no allocator {w} in allocators.toml"));
                }
            }
            all.into_iter().filter(|a| want.contains(&a.name.as_str())).collect()
        }
    }
}

/// The jobs of `--suite`: one suite, or several joined by commas, a job in more than one
/// taken once. `--group` keeps one group's jobs, and `--only` the jobs whose id contains its
/// text, or any of its comma-separated texts.
fn jobs(f: &Flags) -> (String, Vec<suite::Job>) {
    let name = f.get("suite").unwrap_or("quick").to_string();
    let mut v: Vec<suite::Job> = Vec::new();
    for s in name.split(',') {
        let js = match s {
            "quick" => suite::quick(),
            "full" => suite::full(),
            "backend" => suite::backend(),
            "apps" => suite::apps(),
            "servers" => suite::servers(),
            "servers-sat" => suite::servers_sat(),
            "servers-large" => suite::servers_large(),
            "servers-large-sat" => suite::servers_large_sat(),
            "servers-doubling" => suite::servers_doubling(),
            "servers-thread" => suite::servers_thread(),
            "meili-tail" => suite::meili_tail(),
            _ => die(
                "--suite must be quick, full, backend, apps, servers, servers-sat, servers-large, servers-large-sat, \
                 servers-doubling, servers-thread or meili-tail, \
                 or several of them joined by commas",
            ),
        };
        let chosen = js.into_iter().filter(|j| {
            f.get("group").is_none_or(|g| j.group == g)
                && f.get("only").is_none_or(|only| {
                    // A text that starts with `=` names one job exactly.
                    only.split(',').any(|o| o.strip_prefix('=').map_or(j.id.contains(o), |exact| j.id == exact))
                })
        });
        for j in chosen {
            match v.iter().find(|k| k.id == j.id && k.mode == j.mode) {
                None => v.push(j),
                Some(k) if k.args == j.args => {}
                Some(_) => die(&format!("job {} differs between the suites; run them apart", j.id)),
            }
        }
    }
    (name.replace(',', "+"), v)
}

fn build_cpus() -> String {
    std::env::var("BENCH_BUILD_CPUS").unwrap_or_else(|_| "12-15,28-31".into())
}

/// Builds the workloads (`bench-workloads`, `bench-rpc`) with bench-alloc's `feature` ("" for
/// glibc) into `target/<name>`. `rustflags`, when given, replaces whatever the environment
/// sets, so that a layout variant is built with its own flags and nothing else. `pad` is the
/// bytes of padding code a `padN` layout puts ahead of the allocator and the program; no other
/// build gets any, whatever the environment says.
fn build_workloads(p: &Paths, name: &str, feature: &str, rustflags: Option<&str>, pad: Option<usize>) {
    eprintln!("== building workloads for {name}");
    let mut c = Command::new("nice");
    c.args(["taskset", "-c", &build_cpus(), "cargo", "build", "--release", "-p", "bench-workloads", "-p", "bench-rpc"]);
    c.arg("--target-dir").arg(p.bench.join("target").join(name));
    if !feature.is_empty() {
        c.args(["--features", &format!("bench-alloc/{feature}")]);
    }
    if let Some(flags) = rustflags {
        c.env_remove("CARGO_ENCODED_RUSTFLAGS").env("RUSTFLAGS", flags);
    }
    c.env_remove("BENCH_LAYOUT_PAD");
    if let Some(bytes) = pad {
        c.env("BENCH_LAYOUT_PAD", bytes.to_string());
    }
    c.env("BENCH_BASELINES", &p.baselines);
    c.current_dir(&p.bench);
    let st = c.status().unwrap_or_else(|e| die(&format!("cargo: {e}")));
    if !st.success() {
        die(&format!("build for {name} failed"));
    }
}

fn build(f: &Flags, p: &Paths) {
    let dir = &p.bench;
    let _lock = paths::lock_baselines(&p.baselines).unwrap_or_else(|e| die(&e));
    let cpus = build_cpus();
    let names = native_builds(f, p);
    let mut preload_packages: Vec<String> = Vec::new();
    for a in allocators(f, p) {
        if let Some(pkg) = a.preload_build.clone()
            && !preload_packages.contains(&pkg)
        {
            preload_packages.push(pkg);
        }
    }
    for pkg in preload_packages {
        eprintln!("== building preload library {pkg}");
        let st = Command::new("taskset")
            .args(["-c", &cpus, "cargo", "build", "--release", "-p", &pkg, "--manifest-path"])
            .arg(dir.join("../Cargo.toml"))
            .status()
            .unwrap_or_else(|e| die(&format!("cargo: {e}")));
        if !st.success() {
            die(&format!("build of {pkg} failed"));
        }
    }
    for (name, feat) in &names {
        build_workloads(p, name, feat, None, None);
    }
    let mut files: Vec<PathBuf> = asmalloc_builds(&names)
        .flat_map(|name| {
            ["workload", "rpc-server"].map(|bin| p.bench.join("target").join(name).join("release").join(bin))
        })
        .collect();
    files.push(p.bench.join("../target/release/libasmalloc_preload.so"));
    check_rseq_offsets(p, files);
}

/// The native builds `build` and `build-apps` make: (build directory name, bench-alloc
/// feature), glibc first, one for each allocator chosen that has a feature and is not built
/// by hand.
fn native_builds(f: &Flags, p: &Paths) -> Vec<(String, String)> {
    let mut names: Vec<(String, String)> = vec![("glibc".into(), String::new())];
    for a in allocators(f, p) {
        if let (Some(b), Some(feat)) = (a.build_name(), a.feature.clone())
            && a.build.is_none()
            && !names.iter().any(|(n, _)| *n == b)
        {
            names.push((b, feat));
        }
    }
    names
}

fn asmalloc_builds(names: &[(String, String)]) -> impl Iterator<Item = &String> {
    names.iter().filter(|(_, feat)| feat.starts_with("asmalloc")).map(|(name, _)| name)
}

/// `benchrun build-apps`: every program of the apps and servers suites (or `--apps`), once per
/// allocator, with a table of what each build took and the disk each program uses; and the
/// servers' load generator, once, with glibc.
fn build_apps(f: &Flags, p: &Paths) {
    let mut programs: Vec<&'static str> = Vec::new();
    for j in suite::apps().into_iter().chain(suite::servers()).chain(suite::servers_large()) {
        if let Some(a) = j.program.app()
            && !programs.contains(&a)
        {
            programs.push(a);
        }
    }
    if let Some(list) = f.get("apps") {
        let want: Vec<&str> = list.split(',').collect();
        if let Some(w) = want.iter().find(|w| !programs.contains(w)) {
            die(&format!("--apps: no program {w} in the apps or servers suites"));
        }
        programs.retain(|a| want.contains(a));
    }
    let _lock = paths::lock_baselines(&p.baselines).unwrap_or_else(|e| die(&e));
    let names = native_builds(f, p);
    let cpus = build_cpus();
    eprintln!("== building the load generator");
    apps::build_loadgen(p, &cpus).unwrap_or_else(|e| die(&e));
    let mut summary = Vec::new();
    for program in &programs {
        apps::check_lock(p, program).unwrap_or_else(|e| die(&e));
        for (name, feat) in &names {
            eprintln!("== building {program} for {name}");
            let b = apps::build(p, program, name, feat, &cpus).unwrap_or_else(|e| die(&e));
            summary.push((*program, b));
        }
    }
    eprintln!("\n{:<12} {:<16} {:>9} {:>11} {:>10}", "program", "build", "seconds", "CPU seconds", "binary MB");
    for (program, b) in &summary {
        eprintln!(
            "{program:<12} {:<16} {:>9.0} {:>11.0} {:>10.1}",
            b.build,
            b.seconds,
            b.cpu_seconds,
            b.binary_bytes as f64 / 1e6
        );
    }
    eprintln!("\nshared build directories ({}):", p.apps_data.join("build").display());
    for program in &programs {
        eprintln!("{program:<12} {:>8.2} GB", apps::disk_use(&p.apps_data.join("build").join(program)) as f64 / 1e9);
    }
    let files: Vec<PathBuf> = asmalloc_builds(&names)
        .flat_map(|name| programs.iter().map(|program| run::app_bin(&p.bench, name, program)))
        .collect();
    check_rseq_offsets(p, files);
}

/// Runs `scripts/rseq-offset-check` on the asmalloc builds just made (those of `files` that
/// exist), and stops if one loads its rseq offset at a hot word's page offset
/// (docs/research/zen5-roofline.md, section 16): every allocation of that class could wait on
/// the previous one, so no session should run on it. `run` checks again and records the
/// offsets in its session record.
fn check_rseq_offsets(p: &Paths, mut files: Vec<PathBuf>) {
    files.retain(|f| f.exists());
    if files.is_empty() {
        return;
    }
    let st = Command::new(p.bench.join("../scripts/rseq-offset-check"))
        .arg("--quiet")
        .args(&files)
        .status()
        .unwrap_or_else(|e| die(&format!("scripts/rseq-offset-check: {e}")));
    if !st.success() {
        die("an asmalloc build loads its rseq offset at a hot word's page offset; see above");
    }
}

/// The session settings `run` and `layout` share. Re-executes this process in a capped scope
/// unless it is in one already, so everything before this runs twice.
fn run_options(f: &Flags, p: &Paths, suite: String, default_rounds: usize) -> run::Options {
    let mem_cap = f
        .get("mem-cap")
        .map(|s| cap::parse_size(s).unwrap_or_else(|| die("--mem-cap: a size such as 4G")))
        .unwrap_or(cap::DEFAULT_RUN);
    if mem_cap > cap::MAX {
        die("--mem-cap: at most 16G");
    }
    let scope = cap::enter(mem_cap).unwrap_or_else(|e| die(&e));
    let out = f.get("out").map(PathBuf::from).unwrap_or_else(|| {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        p.results.join(format!("{suite}-{t}.jsonl"))
    });
    let runner_cpus = f
        .get("runner-cpus")
        .map(|s| bench_harness::Cpus::parse(s).unwrap_or_else(|| die("--runner-cpus: not a CPU list")).0)
        .unwrap_or_else(|| vec![15]);
    run::Options {
        bench_dir: p.bench.clone(),
        baselines: p.baselines.clone(),
        apps_data: p.apps_data.clone(),
        out,
        scope,
        mem_cap,
        rounds: f.num("rounds", default_rounds),
        seed: f.num("seed", std::process::id() as u64),
        runner_cpus,
        sample_ms: f.num("sample-ms", 50),
        rollup_ms: f.num("rollup-ms", 1000),
        max_busy: f.num("max-busy", 1.0),
        gate_wait: Duration::from_secs(f.num("gate-wait", 900)),
        warm: !f.has("no-warm"),
        hold_idle: match f.get("idle-states") {
            None | Some("held") => true,
            Some("free") => false,
            Some(_) => die("--idle-states: held or free"),
        },
        suite,
    }
}

/// `benchrun layout`: the chosen jobs under builds of the workloads that put the same code at
/// other addresses, interleaved as allocators are in `run`, then each job's spread across the
/// builds. It builds first (unless `--no-build`), then takes the shared measurement lock for
/// the measurement only.
fn layout(f: &Flags, p: &Paths) {
    if f.get("only").is_none() && f.get("group").is_none() {
        die("layout needs --only TEXT or --group NAME to choose the jobs");
    }
    let (suite, js) = jobs(f);
    if js.is_empty() {
        die("no job of the suite matches --only and --group");
    }
    // Other rustflags would make every crate of a program compile again, in the build
    // directory all its builds share.
    if let Some(j) = js.iter().find(|j| j.program.app().is_some()) {
        die(&format!("{}: layout builds the workloads only, not the apps suite's programs", j.id));
    }
    let layouts = layout::parse(f.get("layouts").unwrap_or(layout::DEFAULT)).unwrap_or_else(|e| die(&e));
    let all = allocators(f, p);
    let bases: Vec<config::Allocator> =
        if f.get("allocators").is_some() { all } else { all.into_iter().filter(|a| a.name == "asmalloc").collect() };
    if let Some(a) = bases.iter().find(|a| a.build.is_some() || a.control_of.is_some()) {
        die(&format!("{}: layout variants are built here, so hand-built entries and controls cannot be used", a.name));
    }
    let variants = layout::variants(&bases, &layouts);
    // The re-executed process inside the capped scope must not build again.
    if !f.has("no-build") && !cap::in_scope() {
        let _lock = paths::lock_baselines(&p.baselines).unwrap_or_else(|e| die(&e));
        let mut built: Vec<String> = Vec::new();
        for v in &variants {
            let chosen = layouts.iter().find(|l| v.layout.as_ref() == Some(&l.name));
            let flags = chosen.map(|l| l.rustflags.as_str());
            let pad = chosen.and_then(|l| l.pad);
            let mut want = Vec::new();
            if js.iter().any(|j| j.mode == suite::Mode::Native && run::runs_under(j, v)) {
                want.push((v.build_name().expect("a native variant has a build"), v.feature.clone().unwrap()));
            }
            if js.iter().any(|j| j.mode == suite::Mode::Preload && run::runs_under(j, v)) {
                want.push((v.preload_build_name(), String::new()));
            }
            for (name, feat) in want {
                if !built.contains(&name) {
                    build_workloads(p, &name, &feat, flags, pad);
                    built.push(name);
                }
            }
        }
    }
    let o = run_options(f, p, format!("layout-{suite}"), 5);
    let _lock = paths::lock_baselines(&p.baselines).unwrap_or_else(|e| die(&e));
    let _measuring = layout::measurement_lock().unwrap_or_else(|e| die(&e));
    eprintln!("results: {}", o.out.display());
    run::session(&o, &variants, &js).unwrap_or_else(|e| die(&format!("{e}")));
    layout::summarize(&o.out, &bases, &layouts);
    eprintln!("results: {}", o.out.display());
}

fn main() {
    let mut args = std::env::args().skip(1);
    let cmd = args.next().unwrap_or_default();
    let f = Flags::parse(args);
    let p = Paths::new();
    match cmd.as_str() {
        "build" => build(&f, &p),
        "build-apps" => build_apps(&f, &p),
        "list" => {
            let (_, js) = jobs(&f);
            let allocs = allocators(&f, &p);
            for j in js {
                let n = allocs.iter().filter(|a| run::runs_under(&j, a)).count();
                println!(
                    "{:<14} {:<8} {:<44} cpus {:<18} x{n}",
                    j.group,
                    j.mode.name(),
                    j.id,
                    bench_harness::Cpus(j.cpus.clone()).to_list()
                );
            }
        }
        "run" => {
            let (suite, js) = jobs(&f);
            let allocs = allocators(&f, &p);
            let rounds = if suite == "full" { 10 } else { 1 };
            let o = run_options(&f, &p, suite, rounds);
            let _lock = paths::lock_baselines(&p.baselines).unwrap_or_else(|e| die(&e));
            eprintln!("results: {}", o.out.display());
            run::session(&o, &allocs, &js).unwrap_or_else(|e| die(&format!("{e}")));
            eprintln!("results: {}", o.out.display());
        }
        "layout" => layout(&f, &p),
        "report" => {
            if f.pos.is_empty() {
                die("report needs at least one results file");
            }
            report::report(&report::Options {
                files: f.pos.iter().map(PathBuf::from).collect(),
                reference: f.get("ref").unwrap_or("mimalloc").to_string(),
                detail: f.has("detail"),
                filter: f.get("only").map(String::from),
                quiet_only: f.has("quiet-only"),
                steady_only: f.has("steady-only"),
            });
        }
        _ => die("usage: benchrun <build|build-apps|run|report|list|layout> [options]; see bench/README.md"),
    }
}
