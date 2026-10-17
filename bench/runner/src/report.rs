//! Turns result files into tables: per workload, each allocator's median and spread, its
//! memory, the fastest and the smallest, and whether the control came out null.
use crate::stats::{half_iqr_rel, median, ratio_ci};
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
}

#[derive(Clone, Debug)]
struct Run {
    alloc: String,
    control_of: Option<String>,
    value: f64,
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
}

/// Per-allocator summary of one workload.
struct Line {
    alloc: String,
    n: usize,
    values: Vec<f64>,
    med: f64,
    spread: f64,
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

fn load(files: &[PathBuf]) -> (Vec<String>, Vec<String>, BTreeMap<String, Group>) {
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
            for k in ["rss_dropped", "rss_idle", "rss_live", "live_bytes"] {
                if let Some(x) = v["workload"]["metrics"][k].as_f64() {
                    extra.insert(k.into(), x);
                }
            }
            if let Some(x) = v["driver"]["metrics"]["p99_us"].as_f64() {
                extra.insert("p99_us".into(), x);
            }
            let m = &v["memory"];
            g.runs.push(Run {
                alloc,
                control_of: v["control_of"].as_str().map(String::from),
                value: p["value"].as_f64().unwrap_or(f64::NAN),
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
        let mut extra = BTreeMap::new();
        for k in rs[0].extra.keys() {
            let xs: Vec<f64> = rs.iter().filter_map(|r| r.extra.get(k).copied()).collect();
            extra.insert(k.clone(), median(&xs));
        }
        out.push(Line {
            alloc: a.clone(),
            n: rs.len(),
            med: median(&values),
            spread: half_iqr_rel(&values),
            values,
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
        if higher {
            let (lo, hi) = ratio_ci(bs, ss, seed);
            (*s, bv / sv, (lo, hi))
        } else {
            let (lo, hi) = ratio_ci(ss, bs, seed);
            (*s, sv / bv, (lo, hi))
        }
    });
    Some((first, second))
}

fn margin(second: &Option<RunnerUp>) -> String {
    match second {
        None => "-".into(),
        Some((s, r, (lo, _))) => {
            // With too few runs for an interval (NaN), no tie is claimed either way.
            let tie = if *lo <= 1.0 { " (tie)" } else { "" };
            format!("{} vs {}{tie}", pct(*r).trim_start_matches('+'), s.alloc)
        }
    }
}

pub fn report(o: &Options) {
    let (order, alloc_order, groups) = load(&o.files);
    let mut controls: Vec<(String, f64, (f64, f64))> = Vec::new();
    println!("| Workload | Metric | Fastest | Margin | Least peak RSS | Margin | Control |");
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
        let small = best(&ranked, |l| (l.rss_peak_med, &l.rss_peak), false, i as u64 + 1);
        let mut control = "-".to_string();
        for l in &ls {
            let Some(orig) = g.runs.iter().find(|r| r.alloc == l.alloc).and_then(|r| r.control_of.clone()) else {
                continue;
            };
            if let Some(ol) = ls.iter().find(|x| x.alloc == orig) {
                let r = l.med / ol.med;
                let ci = ratio_ci(&l.values, &ol.values, 7 + i as u64);
                control = if ci.0.is_nan() { pct(r) } else { format!("{} [{}, {}]", pct(r), pct(ci.0), pct(ci.1)) };
                controls.push((key.clone(), r, ci));
            }
        }
        println!(
            "| {key} | {} | {} | {} | {} | {} | {control} |",
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
                "| Allocator | n | {} ({}) | ± | vs {} | Peak RSS MiB | Mean RSS MiB | THP MiB | Faults |{}",
                g.metric,
                if g.higher_better { "higher is better" } else { "lower is better" },
                o.reference,
                extra_keys.iter().map(|k| format!(" {k} |")).collect::<String>()
            );
            println!("|---|---:|---:|---:|---:|---:|---:|---:|---:|{}", "---:|".repeat(extra_keys.len()));
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
                    "| {} | {} | {} | {:.1}% | {vs} | {} | {} | {} | {} |{ex}",
                    l.alloc,
                    l.n,
                    fmt(l.med),
                    l.spread * 100.0,
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
    println!("\n{valid} valid runs, {invalid} invalid.");
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
        for (k, r, ci) in off {
            println!("  {k}: {} [{}, {}]", pct(*r), pct(ci.0), pct(ci.1));
        }
    }
}
