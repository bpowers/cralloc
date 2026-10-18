//! Builds of the workloads that place the same code at other addresses, for `benchrun layout`.
//!
//! A job whose time is one short loop can measure where that loop landed as much as what it
//! does: on Zen 5 the same instructions at another address have run 40% slower
//! (docs/research/memory.md, "The grow loop's 40%"). Every variant here compiles the same
//! sources to the same work and changes only addresses, so a job whose cycles move across
//! the variants while its instructions stay level is measuring code layout.
//!
//! The variants move the allocator's code too (the Rust in all of them; with a shuffle, the
//! C libraries linked in as well). They are a check, never the build a session measures: the
//! suite times the code as it ships.
use crate::config::Allocator;
use crate::stats::{half_iqr_rel, median};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

/// The variants `benchrun layout` builds when `--layouts` is not given.
pub const DEFAULT: &str = "default,loops64,shuffle1,shuffle2";

/// Every timed measurement on the machine takes this lock (CLAUDE.md, "How we work").
const MEASURE_LOCK: &str = "/tmp/asmalloc-measure.lock";

pub struct Layout {
    pub name: String,
    pub rustflags: String,
}

/// Parses a comma-separated list of variants:
///
/// - `default`: the normal build.
/// - `loops64`: every loop header aligned to 64 bytes (`-align-loops=64`), which moves each
///   loop, and everything after it, by up to 63 bytes.
/// - `shuffleN`, N from 1: the linker places the functions' sections in an order shuffled
///   with seed N (lld's `--shuffle-sections`). Functions are 16-byte aligned, so each lands
///   at another offset within its 64-byte line and page.
pub fn parse(list: &str) -> Result<Vec<Layout>, String> {
    let mut v: Vec<Layout> = Vec::new();
    for name in list.split(',').filter(|s| !s.is_empty()) {
        let rustflags = match name {
            "default" => String::new(),
            "loops64" => "-C llvm-args=-align-loops=64".into(),
            _ => match name.strip_prefix("shuffle").and_then(|n| n.parse::<u64>().ok()) {
                // Seed 0 would ask lld for a random seed, which could not be rebuilt.
                Some(seed) if seed > 0 => format!("-C link-arg=-Wl,--shuffle-sections=.text*={seed}"),
                _ => return Err(format!("--layouts: {name} is not default, loops64 or shuffleN (N from 1)")),
            },
        };
        if v.iter().any(|l| l.name == name) {
            return Err(format!("--layouts: {name} is listed twice"));
        }
        v.push(Layout { name: name.to_string(), rustflags });
    }
    if v.len() < 2 {
        return Err("--layouts: name at least two variants to compare".into());
    }
    Ok(v)
}

/// Each allocator under each layout, as allocators of their own (`asmalloc@loops64`), so that
/// a session interleaves them as it interleaves allocators.
pub fn variants(bases: &[Allocator], layouts: &[Layout]) -> Vec<Allocator> {
    bases
        .iter()
        .flat_map(|a| {
            layouts.iter().map(move |l| Allocator {
                name: label(&a.name, &l.name),
                layout: Some(l.name.clone()),
                ..a.clone()
            })
        })
        .collect()
}

fn label(allocator: &str, layout: &str) -> String {
    format!("{allocator}@{layout}")
}

/// Holds the machine's measurement lock until dropped, waiting for it if a teammate has it.
pub fn measurement_lock() -> Result<std::fs::File, String> {
    use std::os::fd::AsRawFd;
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(MEASURE_LOCK)
        .map_err(|e| format!("{MEASURE_LOCK}: {e}"))?;
    // SAFETY: flock on a descriptor we own.
    if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        eprintln!("waiting for {MEASURE_LOCK}");
        // SAFETY: as above.
        if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(format!("{MEASURE_LOCK}: {}", std::io::Error::last_os_error()));
        }
    }
    Ok(f)
}

/// (job, allocator) to each valid run's (primary metric, instructions).
type Runs = BTreeMap<(String, String), Vec<(f64, f64)>>;

/// The valid runs of the last session in `out`, and each job's primary metric name.
fn load(out: &Path) -> (Runs, BTreeMap<String, String>) {
    let text = std::fs::read_to_string(out).unwrap_or_else(|e| panic!("{}: {e}", out.display()));
    let records: Vec<Value> = text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    let session = records.iter().rev().find(|r| r["type"] == "session").map(|r| r["session"].clone());
    let mut runs = Runs::new();
    let mut metrics = BTreeMap::new();
    for r in records.iter().filter(|r| r["type"] == "run" && Some(&r["session"]) == session.as_ref()) {
        if r["valid"] != true {
            continue;
        }
        let (Some(value), Some(instructions)) = (r["primary"]["value"].as_f64(), r["process"]["instructions"].as_f64())
        else {
            continue;
        };
        let job = format!("{} ({})", r["job"].as_str().unwrap_or(""), r["mode"].as_str().unwrap_or(""));
        let alloc = r["allocator"].as_str().unwrap_or("").to_string();
        metrics.insert(job.clone(), r["primary"]["name"].as_str().unwrap_or("").to_string());
        runs.entry((job, alloc)).or_default().push((value, instructions));
    }
    (runs, metrics)
}

/// Prints, for each job and allocator, every layout's median against the first layout's,
/// and the spread: the largest median over the smallest, for the primary metric and for the
/// whole process's instructions.
pub fn summarize(out: &Path, bases: &[Allocator], layouts: &[Layout]) {
    let (runs, metrics) = load(out);
    let jobs: Vec<&String> = {
        let mut j: Vec<&String> = runs.keys().map(|(j, _)| j).collect();
        j.dedup();
        j
    };
    println!();
    print!("{:<44} {:<16} {:<16}", "job", "allocator", "metric");
    for l in layouts {
        print!(" {:>10}", l.name);
    }
    println!(" {:>7} {:>7} {:>7}", "spread", "noise", "instr");
    for job in jobs {
        for a in bases {
            let per: Vec<Option<&Vec<(f64, f64)>>> =
                layouts.iter().map(|l| runs.get(&(job.clone(), label(&a.name, &l.name)))).collect();
            if per.iter().all(Option::is_none) {
                continue;
            }
            let meds: Vec<Option<(f64, f64, f64)>> = per
                .iter()
                .map(|v| {
                    v.map(|v| {
                        let primary: Vec<f64> = v.iter().map(|x| x.0).collect();
                        let instr: Vec<f64> = v.iter().map(|x| x.1).collect();
                        (median(&primary), median(&instr), half_iqr_rel(&primary))
                    })
                })
                .collect();
            print!("{job:<44} {:<16} {:<16}", a.name, metrics[job]);
            let first = meds[0].map(|m| m.0);
            for (i, m) in meds.iter().enumerate() {
                match (m, first) {
                    (Some(m), _) if i == 0 => print!(" {:>10}", fmt(m.0)),
                    (Some(m), Some(f)) => print!(" {:>+9.1}%", (m.0 / f - 1.0) * 100.0),
                    (Some(m), None) => print!(" {:>10}", fmt(m.0)),
                    (None, _) => print!(" {:>10}", "-"),
                }
            }
            let got: Vec<(f64, f64, f64)> = meds.iter().flatten().copied().collect();
            let spread = |f: fn(&(f64, f64, f64)) -> f64| {
                let hi = got.iter().map(f).fold(f64::MIN, f64::max);
                let lo = got.iter().map(f).fold(f64::MAX, f64::min);
                (hi / lo - 1.0) * 100.0
            };
            let noise = got.iter().map(|m| m.2).fold(0.0, f64::max) * 100.0;
            println!(" {:>6.1}% {:>6.1}% {:>6.2}%", spread(|m| m.0), noise, spread(|m| m.1));
        }
    }
    println!();
    println!(
        "spread: largest layout median over smallest; noise: the largest half-IQR of one layout's runs; \
         instr: the spread of the whole process's instructions"
    );
}

fn fmt(v: f64) -> String {
    if v.abs() >= 1000.0 {
        format!("{v:.0}")
    } else if v.abs() >= 10.0 {
        format!("{v:.2}")
    } else {
        format!("{v:.3}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts() {
        let v = parse(DEFAULT).unwrap();
        assert_eq!(v.len(), 4);
        assert_eq!(v[0].rustflags, "");
        assert!(v[2].rustflags.ends_with("=1"));
        assert!(parse("default").is_err());
        assert!(parse("default,shuffle0").is_err());
        assert!(parse("default,default").is_err());
        assert!(parse("default,align").is_err());
    }
}
