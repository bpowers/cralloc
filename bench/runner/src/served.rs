//! The servers' tables for `benchrun report`, from the `served` record of each valid run
//! (`serve.rs`): for every server job, each allocator's medians over its runs of latency at
//! the fixed rates, missed slots, the server's work per request, and its memory under load,
//! through the silences and after the load ends.
use crate::stats::{half_iqr_rel, median};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// A median and half its interquartile range over the runs that have the value.
fn med(vs: &[Value], f: impl Fn(&Value) -> Option<f64>) -> (f64, f64) {
    let x: Vec<f64> = vs.iter().filter_map(f).collect();
    if x.is_empty() { (f64::NAN, f64::NAN) } else { (median(&x), half_iqr_rel(&x)) }
}

fn cell((m, _): (f64, f64), digits: usize) -> String {
    if m.is_nan() { "-".into() } else { format!("{m:.digits$}") }
}

/// The curve's points as (ms since the first mark, MiB).
fn curve(s: &Value) -> Vec<(f64, f64)> {
    s["curve_kib"]
        .as_array()
        .map(|a| a.iter().map(|p| (p[0].as_f64().unwrap_or(0.0), p[1].as_f64().unwrap_or(0.0) / 1024.0)).collect())
        .unwrap_or_default()
}

/// The spans of the segments named `name`, as (begin, end) on the curve's clock.
fn spans(s: &Value, name: &str) -> Vec<(f64, f64)> {
    let marks: Vec<(f64, &str)> = s["marks_ms"]
        .as_array()
        .map(|a| a.iter().map(|m| (m[0].as_f64().unwrap_or(0.0), m[1].as_str().unwrap_or(""))).collect())
        .unwrap_or_default();
    marks.windows(2).filter(|w| w[0].1 == name).map(|w| (w[0].0, w[1].0)).collect()
}

/// The mean over the segments named `name` of `pick` applied to each segment's points.
fn over(s: &Value, name: &str, pick: impl Fn(&[(f64, f64)], f64) -> Option<f64>) -> Option<f64> {
    let pts = curve(s);
    let v: Vec<f64> = spans(s, name)
        .into_iter()
        .filter_map(|(b, e)| {
            let inside: Vec<(f64, f64)> = pts.iter().copied().filter(|p| p.0 >= b && p.0 < e).collect();
            pick(&inside, b)
        })
        .collect();
    if v.is_empty() { None } else { Some(v.iter().sum::<f64>() / v.len() as f64) }
}

pub fn report(files: &[PathBuf], filter: Option<&str>) {
    let mut jobs: Vec<String> = Vec::new();
    let mut allocs: Vec<String> = Vec::new();
    let mut by: BTreeMap<(String, String), Vec<Value>> = BTreeMap::new();
    for f in files {
        let Ok(text) = std::fs::read_to_string(f) else { continue };
        for v in text.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()) {
            if v["type"] != "run" || v["valid"] != true || v["served"].is_null() {
                continue;
            }
            let job = v["job"].as_str().unwrap_or("").to_string();
            let alloc = v["allocator"].as_str().unwrap_or("").to_string();
            if filter.is_some_and(|f| !job.contains(f)) {
                continue;
            }
            if !jobs.contains(&job) {
                jobs.push(job.clone());
            }
            if !allocs.contains(&alloc) {
                allocs.push(alloc.clone());
            }
            by.entry((job, alloc)).or_default().push(v["served"].clone());
        }
    }
    for job in &jobs {
        let rows: Vec<(&String, &Vec<Value>)> =
            allocs.iter().filter_map(|a| by.get(&(job.clone(), a.clone())).map(|v| (a, v))).collect();
        // In the plan's order: the groups' own map is sorted by name.
        let first = &rows[0].1[0];
        let mut names: Vec<String> = Vec::new();
        for m in first["marks_ms"].as_array().into_iter().flatten() {
            let n = m[1].as_str().unwrap_or("").to_string();
            if first["groups"].get(&n).is_some() && !names.contains(&n) {
                names.push(n);
            }
        }
        println!("\n### {job} (server)");
        if names.iter().any(|n| n == "sat") {
            let sat: Vec<String> = rows
                .iter()
                .map(|(a, vs)| format!("{a} {}", cell(med(vs, |s| s["groups"]["sat"]["rate"].as_f64()), 0)))
                .collect();
            println!("\nSaturated rate, closed loop, req/s: {}", sat.join(", "));
        }
        for g in names.iter().filter(|n| *n != "idle" && *n != "off") {
            println!(
                "\n{g}: latency in us from when each request was due; the server's work per request; \
                 ± is half the IQR of cycles/req over runs\n"
            );
            println!(
                "| Allocator | n | req/s | p50 | p90 | p99 | p99.9 | max | missed | cycles/req | ± | CPU us/req \
                 | faults/req | peak MiB |"
            );
            println!("|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|");
            for (a, vs) in &rows {
                let gv = |s: &Value| s["groups"][g.as_str()].clone();
                let lat = |k: &str| cell(med(vs, |s| gv(s)["lat_us"][k].as_f64()), 0);
                let srv = |k: &str| med(vs, |s| gv(s)["server"][k].as_f64());
                let cyc = srv("cycles_per_req");
                println!(
                    "| {a} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
                    vs.len(),
                    cell(med(vs, |s| gv(s)["rate"].as_f64()), 0),
                    lat("p50"),
                    lat("p90"),
                    lat("p99"),
                    lat("p999"),
                    lat("max"),
                    cell(med(vs, |s| gv(s)["missed"].as_f64()), 0),
                    cell(cyc, 0),
                    if cyc.1.is_nan() { "-".into() } else { format!("{:.1}%", cyc.1 * 100.0) },
                    cell(srv("cpu_us_per_req"), 2),
                    cell(srv("faults_per_req"), 3),
                    cell(med(vs, |s| gv(s)["server"]["rss_max"].as_f64().map(|b| b / 1048576.0)), 1),
                );
            }
        }
        if !spans(&rows[0].1[0], "off").is_empty() || !spans(&rows[0].1[0], "idle").is_empty() {
            println!(
                "\nResident MiB, medians: peak under load; in the silences, the lowest and the last point \
                 (mean over silences); after the connections closed, at 1, 5 and 9.75 s\n"
            );
            println!("| Allocator | loaded peak | silence low | silence end | idle +1 s | idle +5 s | idle +9.75 s |");
            println!("|---|---:|---:|---:|---:|---:|---:|");
            let at = |d: f64| move |p: &[(f64, f64)], b: f64| p.iter().rev().find(|x| x.0 <= b + d).map(|x| x.1);
            for (a, vs) in &rows {
                println!(
                    "| {a} | {} | {} | {} | {} | {} | {} |",
                    cell(med(vs, |s| s["rss_max_loaded"].as_f64().map(|b| b / 1048576.0)), 1),
                    cell(med(vs, |s| over(s, "off", |p, _| p.iter().map(|x| x.1).reduce(f64::min))), 1),
                    cell(med(vs, |s| over(s, "off", |p, _| p.last().map(|x| x.1))), 1),
                    cell(med(vs, |s| over(s, "idle", at(1000.0))), 1),
                    cell(med(vs, |s| over(s, "idle", at(5000.0))), 1),
                    cell(med(vs, |s| over(s, "idle", at(9750.0))), 1),
                );
            }
        }
    }
}
