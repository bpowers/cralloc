//! The summary: latency percentiles and memory per group of segments, as JSON.

use crate::plan::{Kind, Segment};
use crate::worker::{CLOSED_LOOP, ERROR, MISSED, Rec};
use std::fmt::Write;

/// Nearest-rank percentile of sorted values.
pub fn pct(sorted: &[u64], q: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = (q * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

fn latency_json(lat: &mut [u64]) -> String {
    lat.sort_unstable();
    let us = |ns: u64| ns as f64 / 1e3;
    let mean = if lat.is_empty() { 0.0 } else { lat.iter().map(|&x| x as f64).sum::<f64>() / lat.len() as f64 / 1e3 };
    format!(
        "{{\"n\":{},\"p50\":{:.1},\"p90\":{:.1},\"p99\":{:.1},\"p999\":{:.1},\"max\":{:.1},\"mean\":{:.1}}}",
        lat.len(),
        us(pct(lat, 0.5)),
        us(pct(lat, 0.9)),
        us(pct(lat, 0.99)),
        us(pct(lat, 0.999)),
        us(lat.last().copied().unwrap_or(0)),
        mean
    )
}

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Groups segments by name, in order of first appearance, leaving out unrecorded ones.
fn groups(segs: &[Segment]) -> Vec<(String, Vec<usize>)> {
    let mut g: Vec<(String, Vec<usize>)> = Vec::new();
    for (i, s) in segs.iter().enumerate().filter(|(_, s)| s.recorded()) {
        match g.iter_mut().find(|(n, _)| *n == s.name) {
            Some((_, v)) => v.push(i),
            None => g.push((s.name.clone(), vec![i])),
        }
    }
    g
}

#[allow(clippy::too_many_arguments)]
pub fn summary(
    label: &str,
    start_mono_ns: u64,
    segs: &[Segment],
    recs: &[Rec],
    mem: &[(f64, u64, u64)],
    head_ns: u64,
    reconnects: u64,
    unexpected: u64,
) -> String {
    let mut s = String::new();
    write!(
        s,
        "{{\"label\":\"{}\",\"start_mono_ns\":{start_mono_ns},\"head_ms\":{},\"segments\":[",
        esc(label),
        head_ns as f64 / 1e6
    )
    .unwrap();
    for (i, g) in segs.iter().enumerate() {
        let kind = match g.kind {
            Kind::Closed => "\"closed\"".to_string(),
            Kind::Off => "\"off\"".to_string(),
            Kind::Close => "\"close\"".to_string(),
            Kind::Rate(r) => format!("{r}"),
        };
        write!(
            s,
            "{}{{\"name\":\"{}\",\"kind\":{kind},\"begin_ms\":{:.3},\"end_ms\":{:.3}}}",
            if i > 0 { "," } else { "" },
            esc(&g.name),
            g.begin_ns as f64 / 1e6,
            g.end_ns as f64 / 1e6
        )
        .unwrap();
    }
    s.push_str("],\"groups\":{");
    for (gi, (name, idx)) in groups(segs).iter().enumerate() {
        let secs: f64 = idx.iter().map(|&i| (segs[i].end_ns - segs[i].begin_ns) as f64 / 1e9).sum();
        let mine: Vec<&Rec> = recs.iter().filter(|r| idx.contains(&(r.seg as usize))).collect();
        let errors = mine.iter().filter(|r| r.flags & ERROR != 0).count();
        let missed = mine.iter().filter(|r| r.flags & MISSED != 0).count();
        let closed_loop = mine.iter().any(|r| r.flags & CLOSED_LOOP != 0);
        let mut all: Vec<u64> = mine.iter().map(|r| r.lat).collect();
        write!(
            s,
            "{}\"{}\":{{\"instances\":{},\"secs\":{secs:.3},\"closed_loop\":{closed_loop},\"errors\":{errors},\"missed\":{missed},\"rate\":{:.1},\"lat_us\":{}",
            if gi > 0 { "," } else { "" },
            esc(name),
            idx.len(),
            mine.len() as f64 / secs,
            latency_json(&mut all)
        )
        .unwrap();
        // Requests due early in a load segment that follows silence, against the rest.
        let after_silence: Vec<usize> = idx
            .iter()
            .copied()
            .filter(|&i| i > 0 && matches!(segs[i - 1].kind, Kind::Off) && matches!(segs[i].kind, Kind::Rate(_)))
            .collect();
        if !after_silence.is_empty() {
            let (mut head, mut rest) = (Vec::new(), Vec::new());
            for r in mine.iter().filter(|r| after_silence.contains(&(r.seg as usize))) {
                if r.due - segs[r.seg as usize].begin_ns < head_ns { head.push(r.lat) } else { rest.push(r.lat) }
            }
            write!(s, ",\"head\":{},\"rest\":{}", latency_json(&mut head), latency_json(&mut rest)).unwrap();
        }
        let in_group: Vec<u64> = mem
            .iter()
            .filter(|(t, _, _)| {
                idx.iter().any(|&i| *t * 1e6 >= segs[i].begin_ns as f64 && *t * 1e6 < segs[i].end_ns as f64)
            })
            .map(|m| m.1)
            .collect();
        if !in_group.is_empty() {
            let mean = in_group.iter().sum::<u64>() / in_group.len() as u64;
            write!(
                s,
                ",\"rss_kib\":{{\"mean\":{mean},\"max\":{},\"last\":{}}}",
                in_group.iter().max().unwrap(),
                in_group.last().unwrap()
            )
            .unwrap();
        }
        s.push('}');
    }
    write!(s, "}},\"reconnects\":{reconnects},\"unexpected_responses\":{unexpected}}}").unwrap();
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_rank() {
        let v: Vec<u64> = (1..=1000).collect();
        assert_eq!(pct(&v, 0.5), 500);
        assert_eq!(pct(&v, 0.999), 999);
        assert_eq!(pct(&v, 1.0), 1000);
        assert_eq!(pct(&v, 0.0), 1);
        assert_eq!(pct(&[], 0.5), 0);
    }
}
