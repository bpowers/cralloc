//! The servers' tables for `benchrun report`, from the `served` record of each valid run
//! (`serve.rs`): for every server job, each allocator's medians over its runs of latency at
//! the fixed rates, missed slots, the server's work per request, and its memory under load,
//! through the silences and after the load ends; and beside the tails, how often the server's
//! CPUs entered deep idle, with each p99.9 against the reference's marked where the two
//! differ in that (`cpuidle::confounded`).
use crate::cpuidle;
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

/// One allocator's deep-idle entries per thousand requests over a group: the median over the
/// runs that recorded it, and how many did of how many.
struct Idle {
    med: Option<f64>,
    recorded: usize,
    runs: usize,
}

impl Idle {
    fn of(vs: &[Value], group: &str) -> Idle {
        let x: Vec<f64> = vs.iter().filter_map(|s| cpuidle::per_kreq(&s["groups"][group]["idle"])).collect();
        Idle { med: (!x.is_empty()).then(|| median(&x)), recorded: x.len(), runs: vs.len() }
    }

    fn cell(&self) -> String {
        match self.med {
            None => "not recorded".into(),
            Some(m) if self.recorded < self.runs => format!("{m:.1} ({} of {} runs)", self.recorded, self.runs),
            Some(m) => format!("{m:.1}"),
        }
    }
}

/// How one allocator's p99.9 compares with the reference's: the difference in us, unless
/// their deep-idle rates differ enough that it may be idle exits (`cpuidle::confounded`), and
/// whether that was so.
fn tail_vs(p999: f64, idle: &Idle, ref_p999: f64, ref_idle: &Idle) -> (String, bool) {
    if p999.is_nan() || ref_p999.is_nan() {
        return ("-".into(), false);
    }
    let d = format!("{:+.1} us", p999 - ref_p999);
    match (idle.med, ref_idle.med) {
        (Some(a), Some(b)) if cpuidle::confounded(a, b) => ("confounded by idle".into(), true),
        (Some(_), Some(_)) => (d, false),
        _ => (format!("{d}, idle not recorded"), false),
    }
}

pub fn report(files: &[PathBuf], filter: Option<&str>, reference: &str) {
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
            let p999 = |vs: &[Value]| med(vs, |s| s["groups"][g.as_str()]["lat_us"]["p999"].as_f64()).0;
            // The tails are set against the reference allocator's, or, without it, the lowest.
            let base = rows
                .iter()
                .find(|(a, _)| a.as_str() == reference)
                .or_else(|| {
                    rows.iter().filter(|(_, vs)| !p999(vs).is_nan()).min_by(|x, y| p999(x.1).total_cmp(&p999(y.1)))
                })
                .map(|(a, vs)| (a.as_str(), p999(vs), Idle::of(vs, g)));
            let mut confounded = false;
            println!(
                "\n{g}: latency in us from when each request was due; deep idle: entries of the server's CPUs into \
                 idle states with an exit latency above {} us, per thousand requests; the server's work per request; \
                 ± is half the IQR of cycles/req over runs\n",
                cpuidle::DEEP_LATENCY_US
            );
            println!(
                "| Allocator | n | req/s | p50 | p90 | p99 | p99.9 | max | deep idle/k | p99.9 vs {} | missed | cycles/req \
                 | ± | CPU us/req | faults/req | peak MiB |",
                base.as_ref().map_or("-", |b| b.0)
            );
            println!("|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|");
            for (a, vs) in &rows {
                let gv = |s: &Value| s["groups"][g.as_str()].clone();
                let lat = |k: &str| cell(med(vs, |s| gv(s)["lat_us"][k].as_f64()), 0);
                let srv = |k: &str| med(vs, |s| gv(s)["server"][k].as_f64());
                let cyc = srv("cycles_per_req");
                let idle = Idle::of(vs, g);
                let (vs_base, flagged) = match &base {
                    Some((b, bp, bi)) if *b != a.as_str() => tail_vs(p999(vs), &idle, *bp, bi),
                    _ => ("-".to_string(), false),
                };
                confounded |= flagged;
                println!(
                    "| {a} | {} | {} | {} | {} | {} | {} | {} | {} | {vs_base} | {} | {} | {} | {} | {} | {} |",
                    vs.len(),
                    cell(med(vs, |s| gv(s)["rate"].as_f64()), 0),
                    lat("p50"),
                    lat("p90"),
                    lat("p99"),
                    lat("p999"),
                    lat("max"),
                    idle.cell(),
                    cell(med(vs, |s| gv(s)["missed"].as_f64()), 0),
                    cell(cyc, 0),
                    if cyc.1.is_nan() { "-".into() } else { format!("{:.1}%", cyc.1 * 100.0) },
                    cell(srv("cpu_us_per_req"), 2),
                    cell(srv("faults_per_req"), 3),
                    cell(med(vs, |s| gv(s)["server"]["rss_max"].as_f64().map(|b| b / 1048576.0)), 1),
                );
            }
            if confounded {
                println!(
                    "\nconfounded by idle: the two allocators' deep-idle rates differ by a factor of {} or more (and by \
                     at least {} a thousand requests), so their p99.9 differ in how often the server's CPUs had to \
                     wake from deep idle, which follows the server's speed, not only the allocator \
                     (docs/research/h2-tail.md). Compare these tails with deep idle off (scripts/deep-idle).",
                    cpuidle::CONFOUND_RATIO,
                    cpuidle::CONFOUND_FLOOR
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn served(p999: f64, idle: Value) -> Value {
        json!({ "groups": { "s70": { "lat_us": { "p999": p999 }, "idle": idle } } })
    }

    fn rec(deep: f64) -> Value {
        json!({ "recorded": true, "deep_per_kreq": deep })
    }

    #[test]
    fn tails_against_the_reference() {
        let je = [served(40.4, rec(21.2))];
        let asm_slow = [served(49.1, rec(45.1)), served(43.9, rec(31.5)), served(49.0, rec(50.3))];
        let asm_clean = [served(37.5, rec(19.9)), served(37.7, rec(20.5))];
        let (ji, si, ci) = (Idle::of(&je, "s70"), Idle::of(&asm_slow, "s70"), Idle::of(&asm_clean, "s70"));
        assert_eq!(si.cell(), "45.1");
        assert_eq!(tail_vs(49.1, &si, 40.4, &ji), ("confounded by idle".into(), true));
        // 20.2 against 21.2: within the ratio, so a difference between allocators.
        assert_eq!(tail_vs(37.6, &ci, 40.4, &ji), ("-2.8 us".into(), false));
    }

    #[test]
    fn old_records_read_as_not_recorded() {
        let old = [json!({ "groups": { "s70": { "lat_us": { "p999": 45.0 } } } })];
        let mixed = [served(40.0, rec(20.0)), served(41.0, cpuidle::not_recorded("absent"))];
        let (oi, mi) = (Idle::of(&old, "s70"), Idle::of(&mixed, "s70"));
        assert_eq!(oi.cell(), "not recorded");
        assert_eq!(mi.cell(), "20.0 (1 of 2 runs)");
        assert_eq!(tail_vs(45.0, &oi, 40.0, &mi), ("+5.0 us, idle not recorded".into(), false));
    }
}
