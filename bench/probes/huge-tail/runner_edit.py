#!/usr/bin/env python3
"""Adds a suite `ht-bursts` to a copy of bench/runner for docs/research/huge-tail.md, for
measurement only (not committed to the runner):

  runner_edit.py BENCH_DIR

The log store at 1M and the order service at 2 GiB, each filled, twenty seconds at 70%, then
four rounds of a lull and a burst: the lull a trickle of a fiftieth of the 70% rate (so that
slow paths keep running and frontiers age) for `HT_LULL_S` seconds (default 6), the burst at
70% marked as servers-thread marks it (`b5`, `b50`, `b500`, `b2`; the four bursts' pieces
summarised together), then the connections close.
"""
import sys
from pathlib import Path

B = Path(sys.argv[1])


def sub(path, old, new):
    p = B / path
    s = p.read_text()
    if s.count(old) != 1:
        raise SystemExit(f"{path}: {s.count(old)} matches for {old[:80]!r}")
    p.write_text(s.replace(old, new))


sub("runner/src/main.rs", '''            "meili-tail" => suite::meili_tail(),
''', '''            "meili-tail" => suite::meili_tail(),
            "ht-bursts" => suite::ht_bursts(),
''')
p = B / "runner/src/suite.rs"
p.write_text(p.read_text() + r'''
/// Scratch (huge-tail): bursts after lulls with a trickle of traffic.
pub fn ht_bursts() -> Vec<Job> {
    let want = ["orders axum cache 2G", "connect store 1M"];
    let lull: u32 = std::env::var("HT_LULL_S").ok().and_then(|v| v.parse().ok()).unwrap_or(6);
    large()
        .into_iter()
        .filter(|l| want.contains(&l.id.as_str()))
        .map(|l| {
            let r = l.r70;
            let trickle = (r / 50).max(100);
            let mut segs = vec![format!("_fill:closed:{}", l.fill_s), format!("_w70:{r}:2")];
            segs.extend((0..2).map(|_| format!("s70:{r}:10")));
            for _ in 0..4 {
                segs.push(format!("lull:{trickle}:{lull}"));
                segs.extend([
                    format!("b5:{r}:0.005"),
                    format!("b50:{r}:0.045"),
                    format!("b500:{r}:0.45"),
                    format!("b2:{r}:1.5"),
                ]);
            }
            segs.push("idle:close:10".into());
            let mut job = large_job(l, segs.join(","), "s70", &format!(" bursts {lull}s"));
            job.group = "ht-bursts";
            job
        })
        .collect()
}
''')
