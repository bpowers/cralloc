//! Turns result files into tables: per workload, each allocator's median and spread, its
//! memory, the fastest and the smallest, and whether the control came out null.
use crate::stats::{half_iqr_rel, median, median_ci};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;

pub struct Options {
    pub files: Vec<PathBuf>,
    /// The allocator others are compared with.
    pub reference: String,
    /// Print one table per workload as well as the summary.
    pub detail: bool,
    /// Only workloads whose id contains this.
    pub filter: Option<String>,
    /// Leave out runs during which the machine was measurably busy around them.
    pub quiet_only: bool,
}

#[derive(Clone, Debug)]
struct Run {
    /// Session and round: runs with the same key ran back to back, so they pair.
    round: String,
    alloc: String,
    control_of: Option<String>,
    value: f64,
    /// The run's cgroup `memory.peak`, which ranks memory; NaN in results from before the
    /// runner recorded it.
    cg_peak: f64,
    rss_peak: f64,
    rss_mean: f64,
    huge_peak: f64,
    minflt: f64,
    extra: BTreeMap<String, f64>,
}

#[derive(Default)]
struct Group {
    metric: String,
    higher_better: bool,
    runs: Vec<Run>,
    invalid: Vec<(String, String)>,
    /// Valid runs left out because the machine was busy around them (`quiet_only`).
    busy: usize,
}

/// With `quiet_only`, a run is left out if its CPUs' SMT siblings were this busy (CPUs'
/// worth) while it ran...
const QUIET_SIBLINGS: f64 = 0.25;
/// ...or if this much work that was not its own ran on its CPUs.
const QUIET_FOREIGN: f64 = 0.3;

/// Per-allocator summary of one workload.
struct Line {
    alloc: String,
    n: usize,
    /// The round of each value, for pairing with another allocator's runs.
    rounds: Vec<String>,
    values: Vec<f64>,
    med: f64,
    spread: f64,
    cg_peak: Vec<f64>,
    cg_peak_med: f64,
    rss_peak: Vec<f64>,
    rss_peak_med: f64,
    rss_mean_med: f64,
    huge_med: f64,
    minflt_med: f64,
    extra: BTreeMap<String, f64>,
}

fn fmt(v: f64) -> String {
    let a = v.abs();
    if !v.is_finite() {
        "-".into()
    } else if a >= 1000.0 {
        format!("{v:.0}")
    } else if a >= 100.0 {
        format!("{v:.1}")
    } else if a >= 10.0 {
        format!("{v:.2}")
    } else {
        format!("{v:.3}")
    }
}

fn mib(b: f64) -> String {
    let m = b / (1u64 << 20) as f64;
    if m >= 100.0 { format!("{m:.0}") } else { format!("{m:.1}") }
}

fn pct(r: f64) -> String {
    format!("{:+.1}%", (r - 1.0) * 100.0)
}

fn load(files: &[PathBuf], quiet_only: bool) -> (Vec<String>, Vec<String>, BTreeMap<String, Group>) {
    let mut order = Vec::new();
    let mut alloc_order: Vec<String> = Vec::new();
    let mut groups: BTreeMap<String, Group> = BTreeMap::new();
    for f in files {
        let text = std::fs::read_to_string(f).unwrap_or_else(|e| panic!("{}: {e}", f.display()));
        for line in text.lines() {
            let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
            if v["type"] == "session" {
                for a in v["allocators"].as_array().into_iter().flatten() {
                    let n = a["name"].as_str().unwrap_or("").to_string();
                    if !alloc_order.contains(&n) {
                        alloc_order.push(n);
                    }
                }
                continue;
            }
            if v["type"] != "run" {
                continue;
            }
            let key = format!("{} ({})", v["job"].as_str().unwrap_or(""), v["mode"].as_str().unwrap_or(""));
            if !order.contains(&key) {
                order.push(key.clone());
            }
            let g = groups.entry(key).or_default();
            let alloc = v["allocator"].as_str().unwrap_or("").to_string();
            if v["valid"] != true {
                g.invalid.push((alloc, v["problems"].to_string()));
                continue;
            }
            let inter = &v["interference"];
            let busy = inter["siblings_busy"].as_f64().unwrap_or(0.0) > QUIET_SIBLINGS
                || inter["foreign_on_job_cpus"].as_f64().unwrap_or(0.0) > QUIET_FOREIGN;
            if quiet_only && busy {
                g.busy += 1;
                continue;
            }
            let p = &v["primary"];
            g.metric = p["name"].as_str().unwrap_or("").to_string();
            g.higher_better = p["better"] == "higher";
            let mut extra: BTreeMap<String, f64> = v["extra"]
                .as_object()
                .map(|m| m.iter().filter_map(|(k, x)| Some((k.clone(), x.as_f64()?))).collect())
                .unwrap_or_default();
            if let Some(c) = v["workload"]["metrics"]["cross_free_share"].as_f64() {
                extra.insert("cross_free_share".into(), c);
            }
            for k in ["rss_dropped", "rss_idle", "rss_live", "live_bytes", "cycles_per_alloc"] {
                if let Some(x) = v["workload"]["metrics"][k].as_f64() {
                    extra.insert(k.into(), x);
                }
            }
            if let Some(x) = v["driver"]["metrics"]["p99_us"].as_f64() {
                extra.insert("p99_us".into(), x);
            }
            let m = &v["memory"];
            g.runs.push(Run {
                round: format!("{}/{}", v["session"].as_str().unwrap_or(""), v["round"]),
                alloc,
                control_of: v["control_of"].as_str().map(String::from),
                value: p["value"].as_f64().unwrap_or(f64::NAN),
                cg_peak: v["cgroup"]["peak"].as_f64().unwrap_or(f64::NAN),
                rss_peak: m["rss_peak"].as_f64().unwrap_or(f64::NAN),
                rss_mean: m["rss_mean"].as_f64().unwrap_or(f64::NAN),
                huge_peak: m["anon_huge_peak"].as_f64().unwrap_or(f64::NAN),
                minflt: v["rusage"]["minflt"].as_f64().unwrap_or(f64::NAN),
                extra,
            });
        }
    }
    (order, alloc_order, groups)
}

fn lines_of(g: &Group, alloc_order: &[String]) -> Vec<Line> {
    let mut out = Vec::new();
    for a in alloc_order {
        let rs: Vec<&Run> = g.runs.iter().filter(|r| &r.alloc == a).collect();
        if rs.is_empty() {
            continue;
        }
        let col = |f: fn(&Run) -> f64| rs.iter().map(|r| f(r)).collect::<Vec<f64>>();
        let values = col(|r| r.value);
        let rss_peak = col(|r| r.rss_peak);
        let cg_peak = col(|r| r.cg_peak);
        let mut extra = BTreeMap::new();
        for k in rs[0].extra.keys() {
            let xs: Vec<f64> = rs.iter().filter_map(|r| r.extra.get(k).copied()).collect();
            extra.insert(k.clone(), median(&xs));
        }
        out.push(Line {
            alloc: a.clone(),
            n: rs.len(),
            rounds: rs.iter().map(|r| r.round.clone()).collect(),
            med: median(&values),
            spread: half_iqr_rel(&values),
            values,
            cg_peak_med: median(&cg_peak),
            cg_peak,
            rss_peak_med: median(&rss_peak),
            rss_peak,
            rss_mean_med: median(&col(|r| r.rss_mean)),
            huge_med: median(&col(|r| r.huge_peak)),
            minflt_med: median(&col(|r| r.minflt)),
            extra,
        });
    }
    out
}

/// A runner-up: its line, how much worse than the best it is (a ratio above 1), and the 95%
/// bootstrap interval of that ratio.
type RunnerUp<'a> = (&'a Line, f64, (f64, f64));

/// The best line by `key` (lower is better unless `higher`), and the runner-up.
fn best<'a>(
    ls: &[&'a Line],
    key: fn(&Line) -> (f64, &[f64]),
    higher: bool,
    seed: u64,
) -> Option<(&'a Line, Option<RunnerUp<'a>>)> {
    let mut v: Vec<&Line> = ls.iter().copied().filter(|l| key(l).0.is_finite()).collect();
    v.sort_by(|a, b| {
        let (x, y) = (key(a).0, key(b).0);
        if higher { y.total_cmp(&x) } else { x.total_cmp(&y) }
    });
    let first = *v.first()?;
    let second = v.get(1).map(|s| {
        let (bv, bs) = key(first);
        let (sv, ss) = key(s);
        // How much worse the runner-up is, round by round, so that drift between rounds
        // cancels; the interval is over those per-round ratios.
        let worse = |b: f64, s: f64| if higher { b / s } else { s / b };
        let paired: Vec<f64> = s
            .rounds
            .iter()
            .zip(ss)
            .filter_map(|(r, &sx)| first.rounds.iter().position(|fr| fr == r).map(|i| worse(bs[i], sx)))
            .collect();
        (*s, worse(bv, sv), median_ci(&paired, seed))
    });
    Some((first, second))
}

fn margin(second: &Option<RunnerUp>) -> String {
    match second {
        None => "-".into(),
        Some((s, r, (lo, _))) => {
            let tie = if lo.is_nan() {
                " (n<3)"
            } else if *lo <= 1.0 {
                " (tie)"
            } else {
                ""
            };
            format!("{} vs {}{tie}", pct(*r).trim_start_matches('+'), s.alloc)
        }
    }
}

pub fn report(o: &Options) {
    let (order, alloc_order, groups) = load(&o.files, o.quiet_only);
    let mut controls: Vec<(String, f64, (f64, f64))> = Vec::new();
    println!("| Workload | Metric | Fastest | Margin | Least memory (cgroup peak) | Margin | Control |");
    println!("|---|---|---|---|---|---|---|");
    for (i, key) in order.iter().enumerate() {
        if o.filter.as_ref().is_some_and(|f| !key.contains(f.as_str())) {
            continue;
        }
        let g = &groups[key];
        let ls = lines_of(g, &alloc_order);
        // Controls are excluded from the ranking; they are judged against their original.
        let ranked: Vec<&Line> =
            ls.iter().filter(|l| !g.runs.iter().any(|r| r.alloc == l.alloc && r.control_of.is_some())).collect();
        let fast = best(&ranked, |l| (l.med, &l.values), g.higher_better, i as u64);
        // Results from before the runner recorded cgroup peaks rank on VmHWM, and say so.
        let cgroup = g.runs.iter().all(|r| r.cg_peak.is_finite());
        let small = if cgroup {
            best(&ranked, |l| (l.cg_peak_med, &l.cg_peak), false, i as u64 + 1)
        } else {
            best(&ranked, |l| (l.rss_peak_med, &l.rss_peak), false, i as u64 + 1)
        };
        let small_by = if cgroup { "" } else { " (by VmHWM)" };
        let mut control = "-".to_string();
        for l in &ls {
            let Some(orig) = g.runs.iter().find(|r| r.alloc == l.alloc).and_then(|r| r.control_of.clone()) else {
                continue;
            };
            if ls.iter().any(|x| x.alloc == orig) {
                // Pair each control run with its original's run from the same round, so that
                // drift between rounds cancels.
                let by_round = |name: &str| -> BTreeMap<&str, f64> {
                    g.runs.iter().filter(|r| r.alloc == name).map(|r| (r.round.as_str(), r.value)).collect()
                };
                let (ctl, orig_runs) = (by_round(&l.alloc), by_round(&orig));
                let ratios: Vec<f64> = ctl.iter().filter_map(|(k, c)| orig_runs.get(k).map(|o| c / o)).collect();
                let r = median(&ratios);
                let ci = median_ci(&ratios, 7 + i as u64);
                control = if ci.0.is_nan() { pct(r) } else { format!("{} [{}, {}]", pct(r), pct(ci.0), pct(ci.1)) };
                controls.push((key.clone(), r, ci));
            }
        }
        println!(
            "| {key} | {} | {} | {} | {}{small_by} | {} | {control} |",
            g.metric,
            fast.map(|f| f.0.alloc.as_str()).unwrap_or("-"),
            fast.map(|f| margin(&f.1)).unwrap_or_default(),
            small.map(|f| f.0.alloc.as_str()).unwrap_or("-"),
            small.map(|f| margin(&f.1)).unwrap_or_default(),
        );
    }
    if o.detail {
        for key in &order {
            if o.filter.as_ref().is_some_and(|f| !key.contains(f.as_str())) {
                continue;
            }
            let g = &groups[key];
            let ls = lines_of(g, &alloc_order);
            let reference = ls.iter().find(|l| l.alloc == o.reference).map(|l| l.med);
            let extra_keys: Vec<String> = ls.first().map(|l| l.extra.keys().cloned().collect()).unwrap_or_default();
            println!("\n### {key}\n");
            println!(
                "| Allocator | n | {} ({}) | ± | vs {} | Cgroup peak MiB | VmHWM MiB | Mean RSS MiB | THP MiB | Faults |{}",
                g.metric,
                if g.higher_better { "higher is better" } else { "lower is better" },
                o.reference,
                extra_keys.iter().map(|k| format!(" {k} |")).collect::<String>()
            );
            println!("|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|{}", "---:|".repeat(extra_keys.len()));
            for l in &ls {
                let vs = reference.map(|r| pct(l.med / r)).unwrap_or_else(|| "-".into());
                let ex: String = extra_keys
                    .iter()
                    .map(|k| {
                        let x = l.extra.get(k).copied().unwrap_or(f64::NAN);
                        if k.starts_with("rss") || k.ends_with("bytes") {
                            format!(" {} |", mib(x))
                        } else {
                            format!(" {} |", fmt(x))
                        }
                    })
                    .collect();
                println!(
                    "| {} | {} | {} | {:.1}% | {vs} | {} | {} | {} | {} | {} |{ex}",
                    l.alloc,
                    l.n,
                    fmt(l.med),
                    l.spread * 100.0,
                    mib(l.cg_peak_med),
                    mib(l.rss_peak_med),
                    mib(l.rss_mean_med),
                    mib(l.huge_med),
                    fmt(l.minflt_med),
                );
            }
            if !g.invalid.is_empty() {
                println!("\nInvalid runs:");
                for (a, p) in &g.invalid {
                    println!("- {a}: {p}");
                }
            }
        }
    }
    let invalid: usize = groups.values().map(|g| g.invalid.len()).sum();
    let valid: usize = groups.values().map(|g| g.runs.len()).sum();
    let busy: usize = groups.values().map(|g| g.busy).sum();
    println!("\n{valid} valid runs used, {busy} valid runs left out as busy, {invalid} invalid.");
    println!(
        "Memory ranks on each run's cgroup peak (anonymous memory, page tables, and file pages the run was first to \
         read); VmHWM includes shared libraries. Mean RSS is sampled over the whole process."
    );
    if order.iter().any(|k| k.contains("all-alloc") || k.contains("free-only")) {
        println!(
            "all-alloc: every thread allocates once before the work, as a service's threads do. free-only: the \
             consumers never allocate, as a dedicated drop thread; jemalloc gives such a thread no cache."
        );
    }
    if !controls.is_empty() {
        let with_ci = controls.iter().filter(|(_, _, (lo, _))| !lo.is_nan()).count();
        let off: Vec<&(String, f64, (f64, f64))> =
            controls.iter().filter(|(_, _, (lo, hi))| *lo > 1.0 || *hi < 1.0).collect();
        let worst = controls.iter().map(|(_, r, _)| (r - 1.0).abs()).fold(0.0, f64::max);
        println!(
            "Controls: {} workloads, {with_ci} with enough runs for an interval; the control differs from its \
             original (95% interval excludes zero) in {}; largest control difference {:.1}%.",
            controls.len(),
            off.len(),
            worst * 100.0
        );
        println!("  By chance alone about {:.1} of {with_ci} intervals would exclude zero.", 0.05 * with_ci as f64);
        for (k, r, ci) in off {
            println!("  {k}: {} [{}, {}]", pct(*r), pct(ci.0), pct(ci.1));
        }
    }
}
