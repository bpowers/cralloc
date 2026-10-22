#!/usr/bin/env python3
"""Per allocator: the s70 group's server counters per request from benchrun results, every
run and the median: cycles, instructions and any `<name>_per_req` that BENCH_PROC_EVENTS
added (diagnostics.patch's runner hunks).

usage: srvev.py RESULTS... [--group s70]
"""
import collections, json, statistics, sys

args = [a for a in sys.argv[1:] if not a.startswith("--")]
group = "s70"
if "--group" in sys.argv:
    group = sys.argv[sys.argv.index("--group") + 1]
    args.remove(group)
rows = collections.defaultdict(list)
for f in args:
    for line in open(f):
        d = json.loads(line)
        if not d.get("served"):
            continue
        a = d["allocator"] if isinstance(d["allocator"], str) else d["allocator"]["name"]
        s = d["served"]["groups"].get(group, {}).get("server", {})
        if s:
            rows[a].append(s)
for a, runs in sorted(rows.items()):
    keys = [k for k in runs[0] if k.endswith("_per_req") and k != "faults_per_req" and k != "cpu_us_per_req"]
    med = {k: statistics.median(r[k] for r in runs) for k in keys}
    print(f"{a:6} n={len(runs)} " + " ".join(f"{k[:-8]}={med[k] / 1000:.2f}k" for k in keys))
    for r in runs:
        print("        " + " ".join(f"{r[k] / 1000:.2f}" for k in keys))
