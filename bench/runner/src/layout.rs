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
//!
//! The shuffles and `loops64` move every function at once, so they show that a job is
//! sensitive to placement but hold nothing else still. The `padN` variants move all the code
//! the linker places after bench-alloc's pad, the allocator's and the program's, by exactly N
//! bytes and change nothing else, so a sweep over them (`pads`) is a controlled range of
//! placements: what one build's number stands for, and the way to compare allocators on a job
//! that is sensitive to it (`docs/research/grpc-placement.md`).
use crate::config::Allocator;
use crate::stats::{half_iqr_rel, median};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

/// The variants `benchrun layout` builds when `--layouts` is not given.
pub const DEFAULT: &str = "default,loops64,shuffle1,shuffle2";

/// What `--layouts pads` stands for: a line's worth of 16-byte steps, and a whole line.
pub const PADS: &str = "pad00,pad16,pad32,pad48,pad64";

/// Every timed measurement on the machine takes this lock (CLAUDE.md, "How we work").
const MEASURE_LOCK: &str = "/tmp/asmalloc-measure.lock";

pub struct Layout {
    pub name: String,
    pub rustflags: String,
    /// Bytes of padding code ahead of the allocator and the program (`bench_layout_pad` in
    /// bench-alloc, from `BENCH_LAYOUT_PAD` at build time); `None` for no pad.
    pub pad: Option<usize>,
}

/// Parses a comma-separated list of variants:
///
/// - `default`: the normal build.
/// - `loops64`: every loop header aligned to 64 bytes (`-align-loops=64`), which moves each
///   loop, and everything after it, by up to 63 bytes.
/// - `shuffleN`, N from 1: the linker places the functions' sections in an order shuffled
///   with seed N (lld's `--shuffle-sections`). Functions are 16-byte aligned, so each lands
///   at another offset within its 64-byte line and page.
/// - `padN`: N bytes of code ahead of the allocator and the program, N a multiple of 16 (the
///   functions' alignment) written with at least two digits, so that the variants' binaries
///   have paths of one length; `pad00` is the normal build in a directory of its own.
/// - `pads`: `pad00,pad16,pad32,pad48,pad64`.
pub fn parse(list: &str) -> Result<Vec<Layout>, String> {
    let mut v: Vec<Layout> = Vec::new();
    let expanded = list.replace("pads", PADS);
    for name in expanded.split(',').filter(|s| !s.is_empty()) {
        let mut pad = None;
        let rustflags = match name {
            "default" => String::new(),
            "loops64" => "-C llvm-args=-align-loops=64".into(),
            _ if name.starts_with("pad") => {
                let digits = &name[3..];
                match digits.parse::<usize>() {
                    Ok(n) if digits.len() >= 2 && n % 16 == 0 && n <= 1 << 16 => pad = Some(n),
                    _ => {
                        return Err(format!(
                            "--layouts: {name}: padN takes a multiple of 16 up to 65536, with at least two digits"
                        ));
                    }
                }
                String::new()
            }
            _ => match name.strip_prefix("shuffle").and_then(|n| n.parse::<u64>().ok()) {
                // Seed 0 would ask lld for a random seed, which could not be rebuilt.
                Some(seed) if seed > 0 => format!("-C link-arg=-Wl,--shuffle-sections=.text*={seed}"),
                _ => {
                    return Err(format!("--layouts: {name} is not default, loops64, shuffleN (N from 1) or padN"));
                }
            },
        };
        if v.iter().any(|l| l.name == name) {
            return Err(format!("--layouts: {name} is listed twice"));
        }
        v.push(Layout { name: name.to_string(), rustflags, pad });
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
/// the mean of the layouts' medians and their range, and the spread: the largest median over
/// the smallest, for the primary metric and for the whole process's instructions. Then, for
/// each job with more than one allocator, each allocator against the first: by the first
/// layout's medians, which is what a session of single builds compares, and by the means over
/// the layouts, which averages placement out.
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
    println!(" {:>10} {:>21} {:>7} {:>7} {:>7}", "mean", "range", "spread", "noise", "instr");
    // (job, allocator) to (first layout's median, mean of the layouts' medians).
    let mut summary: BTreeMap<(String, String), (Option<f64>, f64)> = BTreeMap::new();
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
            let mean = got.iter().map(|m| m.0).sum::<f64>() / got.len() as f64;
            let lo = got.iter().map(|m| m.0).fold(f64::MAX, f64::min);
            let hi = got.iter().map(|m| m.0).fold(f64::MIN, f64::max);
            println!(
                " {:>10} {:>21} {:>6.1}% {:>6.1}% {:>6.2}%",
                fmt(mean),
                format!("{} to {}", fmt(lo), fmt(hi)),
                spread(|m| m.0),
                noise,
                spread(|m| m.1)
            );
            summary.insert((job.clone(), a.name.clone()), (first, mean));
        }
    }
    if bases.len() > 1 {
        println!();
        println!("{:<44} {:<16} {:>16} {:>16}", "job", "allocator", "first layout", "mean of layouts");
        let jobs: Vec<&String> = {
            let mut j: Vec<&String> = summary.keys().map(|(j, _)| j).collect();
            j.dedup();
            j
        };
        for job in jobs {
            let Some(&(base_first, base_mean)) = summary.get(&(job.clone(), bases[0].name.clone())) else {
                continue;
            };
            for a in &bases[1..] {
                let Some(&(first, mean)) = summary.get(&(job.clone(), a.name.clone())) else {
                    continue;
                };
                let vs_first = match (first, base_first) {
                    (Some(f), Some(b)) => format!("{:+.1}%", (f / b - 1.0) * 100.0),
                    _ => "-".into(),
                };
                println!(
                    "{job:<44} {:<16} {:>16} {:>16}",
                    format!("{} vs {}", a.name, bases[0].name),
                    vs_first,
                    format!("{:+.1}%", (mean / base_mean - 1.0) * 100.0)
                );
            }
        }
    }
    println!();
    println!(
        "mean and range: of the layouts' medians; spread: largest layout median over smallest; \
         noise: the largest half-IQR of one layout's runs; instr: the spread of the whole process's instructions"
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
        let pads = parse("pads").unwrap();
        assert_eq!(pads.iter().map(|l| l.pad).collect::<Vec<_>>(), [0, 16, 32, 48, 64].map(Some));
        assert!(pads.iter().all(|l| l.rustflags.is_empty() && l.name.len() == 5));
        assert_eq!(parse("default,pad128").unwrap()[1].pad, Some(128));
        assert!(parse("default,pad8").is_err(), "one digit, and not a multiple of 16");
        assert!(parse("default,pad24").is_err());
        assert!(parse("default,pad0").is_err(), "one digit");
        assert!(parse("default,padx").is_err());
    }
}
