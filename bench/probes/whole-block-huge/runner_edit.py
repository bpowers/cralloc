#!/usr/bin/env python3
"""Edits a scratch copy of bench/ so that benchrun counts raw PMU events in every measured
process from outside, as it counts cycles, and reports them per server group, for
docs/research/whole-block-huge.md. Not product code.

  runner_edit.py BENCH_DIR

- `WBH_PROC_EVENTS=name=config,...` (the runner's own environment; the format of
  BENCH_EVENTS) adds the events to the process counters (user mode only). A server group then
  carries `server.extra`, the events' totals over its segments, and a batch run's `process`
  record has them.
- `WBH_BENCH_DIR` names the bench/ directory the runner reads (allocators.toml, the builds),
  since the scratch tree it was compiled in is gone.
"""
import sys
from pathlib import Path

B = Path(sys.argv[1])


def sub(path, old, new, count=1):
    p = B / path
    s = p.read_text()
    if s.count(old) != count:
        raise SystemExit(f"{path}: {s.count(old)} matches (want {count}) for {old[:90]!r}")
    p.write_text(s.replace(old, new))


sub("runner/src/paths.rs",
    "        let bench = PathBuf::from(env!(\"CARGO_MANIFEST_DIR\")).parent().unwrap().to_path_buf();",
    "        let bench = std::env::var_os(\"WBH_BENCH_DIR\").map(PathBuf::from).unwrap_or_else(|| {\n"
    "            PathBuf::from(env!(\"CARGO_MANIFEST_DIR\")).parent().unwrap().to_path_buf()\n"
    "        });")

sub("runner/src/launch.rs",
    "            match Counters::open(PROCESS_EVENTS, Target::ChildBeforeExec(pid)) {",
    "            match Counters::open(&wbh_events(), Target::ChildBeforeExec(pid)) {")
p = B / "runner/src/launch.rs"
p.write_text(p.read_text() + r'''
/// Scratch (whole-block-huge): the process events and any of `WBH_PROC_EVENTS`.
fn wbh_events() -> Vec<bench_harness::perf::Event> {
    let mut v = PROCESS_EVENTS.to_vec();
    if let Ok(list) = std::env::var("WBH_PROC_EVENTS") {
        for item in list.split(',').filter(|s| !s.is_empty()) {
            let (name, config) = item.split_once('=').expect("WBH_PROC_EVENTS: name=config");
            let config = u64::from_str_radix(config.trim_start_matches("0x"), 16).expect("hex");
            v.push(bench_harness::perf::Event::Raw { name: Box::leak(name.to_string().into_boxed_str()), config });
        }
    }
    v
}
''')

S = "runner/src/serve.rs"
sub(S, """        let mut rss_max = 0;
        for w in l.marks.windows(2) {
            if w[0].segment.is_none() || &w[0].name != name {
                continue;
            }
            let d = w[1].snap.counters.since(&w[0].snap.counters);""", """        let mut rss_max = 0;
        let mut wbh_extra = Map::new();
        for w in l.marks.windows(2) {
            if w[0].segment.is_none() || &w[0].name != name {
                continue;
            }
            let d = w[1].snap.counters.since(&w[0].snap.counters);
            for &(n, v) in &d.values {
                if !matches!(n, "cycles" | "instructions" | "task_clock_ns" | "page_faults") {
                    let now = wbh_extra.get(n).and_then(|x| x.as_u64()).unwrap_or(0);
                    wbh_extra.insert(n.to_string(), (now + v).into());
                }
            }""")
sub(S, """        let server = json!({
            "ms": ms,""", """        let mut server = json!({
            "ms": ms,""")
sub(S, """        if name == rank {
            primary = server["cycles_per_req"].as_f64();
        }""", """        server["extra"] = Value::Object(wbh_extra);
        if name == rank {
            primary = server["cycles_per_req"].as_f64();
        }""")
print("runner edit applied to", B)
