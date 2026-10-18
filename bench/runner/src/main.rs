//! The benchmark runner.
//!
//!   benchrun build  [--allocators a,b]
//!   benchrun run    [--suite quick|full] [--rounds N] [--only TEXT] [--allocators a,b]
//!                   [--out FILE] [--seed N] [--max-busy CPUS] [--gate-wait SECONDS]
//!                   [--runner-cpus LIST] [--sample-ms N] [--mem-cap SIZE]
//!   benchrun report FILE... [--detail] [--ref NAME] [--only TEXT] [--quiet-only]
//!   benchrun list   [--suite quick|full]
//!
//! See bench/README.md.
mod cap;
mod config;
mod launch;
mod paths;
mod report;
mod run;
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
                } else if it.peek().is_some_and(|n| !n.starts_with("--")) && !matches!(name, "detail" | "quiet-only") {
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

fn jobs(f: &Flags) -> (String, Vec<suite::Job>) {
    let name = f.get("suite").unwrap_or("quick").to_string();
    let mut v = match name.as_str() {
        "quick" => suite::quick(),
        "full" => suite::full(),
        _ => die("--suite must be quick or full"),
    };
    if let Some(only) = f.get("only") {
        v.retain(|j| j.id.contains(only));
    }
    (name, v)
}

fn build(f: &Flags, p: &Paths) {
    let dir = &p.bench;
    let _lock = paths::lock_baselines(&p.baselines).unwrap_or_else(|e| die(&e));
    let cpus = std::env::var("BENCH_BUILD_CPUS").unwrap_or_else(|_| "4-7,20-23".into());
    let mut names: Vec<(String, String)> = vec![("glibc".into(), String::new())];
    let mut preload_packages: Vec<String> = Vec::new();
    for a in allocators(f, p) {
        if let (Some(b), Some(feat)) = (a.build_name(), a.feature.clone())
            && a.build.is_none()
            && !names.iter().any(|(n, _)| *n == b)
        {
            names.push((b, feat));
        }
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
    for (name, feat) in names {
        eprintln!("== building workloads for {name}");
        let mut c = Command::new("taskset");
        c.args(["-c", &cpus, "cargo", "build", "--release", "-p", "bench-workloads", "-p", "bench-rpc"]);
        c.arg("--target-dir").arg(dir.join("target").join(&name));
        if !feat.is_empty() {
            c.args(["--features", &format!("bench-alloc/{feat}")]);
        }
        c.env("BENCH_BASELINES", &p.baselines);
        c.current_dir(dir);
        let st = c.status().unwrap_or_else(|e| die(&format!("cargo: {e}")));
        if !st.success() {
            die(&format!("build for {name} failed"));
        }
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let cmd = args.next().unwrap_or_default();
    let f = Flags::parse(args);
    let p = Paths::new();
    match cmd.as_str() {
        "build" => build(&f, &p),
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
            let mem_cap = f
                .get("mem-cap")
                .map(|s| cap::parse_size(s).unwrap_or_else(|| die("--mem-cap: a size such as 4G")))
                .unwrap_or(cap::DEFAULT_RUN);
            if mem_cap > cap::MAX {
                die("--mem-cap: at most 16G");
            }
            // Re-executes this process in a capped scope unless it is in one already.
            let scope = cap::enter(mem_cap).unwrap_or_else(|e| die(&e));
            let _lock = paths::lock_baselines(&p.baselines).unwrap_or_else(|e| die(&e));
            let rounds = f.num("rounds", if suite == "full" { 10 } else { 1 });
            let seed = f.num("seed", std::process::id() as u64);
            let out = f.get("out").map(PathBuf::from).unwrap_or_else(|| {
                let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
                p.results.join(format!("{suite}-{t}.jsonl"))
            });
            let runner_cpus = f
                .get("runner-cpus")
                .map(|s| bench_harness::Cpus::parse(s).unwrap_or_else(|| die("--runner-cpus: not a CPU list")).0)
                .unwrap_or_else(|| vec![15]);
            let o = run::Options {
                bench_dir: p.bench.clone(),
                baselines: p.baselines.clone(),
                out: out.clone(),
                scope,
                mem_cap,
                rounds,
                seed,
                runner_cpus,
                sample_ms: f.num("sample-ms", 50),
                rollup_ms: f.num("rollup-ms", 1000),
                max_busy: f.num("max-busy", 1.0),
                gate_wait: Duration::from_secs(f.num("gate-wait", 900)),
                suite,
            };
            eprintln!("results: {}", out.display());
            run::session(&o, &allocs, &js).unwrap_or_else(|e| die(&format!("{e}")));
            eprintln!("results: {}", out.display());
        }
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
            });
        }
        _ => die("usage: benchrun <build|run|report|list> [options]; see bench/README.md"),
    }
}
