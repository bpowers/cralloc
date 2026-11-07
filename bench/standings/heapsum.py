#!/usr/bin/env python3
"""Large-heap server runs as a table per job, without a live-bytes build: heapsum.py FILE...

Per allocator (`NAME@padNN` folded into NAME), medians over valid runs: server cycles a
request at 70% (`s70`), p50/p99/p99.9 in us and deep-idle entries per thousand requests at
70%, page faults a request at 70%, and the anonymous resident memory (the curve's third
column, from the rollups once a second; for a server whose store is a mapped file this is the
allocator's part) as its mean and peak from the first `s70` mark to the first `idle` mark, and
its value at the end of the plan, before the server is stopped. MiB.
"""
import json
import statistics
import sys
from collections import defaultdict

MiB = 1024  # the curve is in KiB


def med(xs):
    xs = [x for x in xs if x is not None]
    return statistics.median(xs) if xs else float("nan")


runs = defaultdict(lambda: defaultdict(list))
for f in sys.argv[1:]:
    for line in open(f):
        d = json.loads(line)
        if d.get("type") == "run" and d.get("valid") and d.get("served"):
            runs[d["job"]][d["allocator"].split("@")[0]].append(d)


def window(served):
    marks = served["marks_ms"]
    start = next((m[0] for m in marks if m[1] == "s70"), None)
    end = next((m[0] for m in marks if m[1] == "idle"), None)
    pts = [p[2] for p in served["curve_kib"] if start is not None and start <= p[0] <= (end or 1e18)]
    return pts


def at_end(served):
    """The anonymous memory at the end of the plan. The curve goes on past it into the server's
    shutdown, whose frees would count as memory it gave back while idle."""
    end = next((m[0] for m in served["marks_ms"] if m[1] == "end"), float("inf"))
    return [p[2] for p in served["curve_kib"] if p[0] <= end][-1]


for job, by in runs.items():
    print(f"\n{job}")
    print("  allocator        n  cycles/req  p50 us   p99 us  p99.9 us  deep/k  faults/req  heap mean  heap peak  heap end")
    for a, rs in sorted(by.items(), key=lambda kv: med([r["primary"]["value"] for r in kv[1]])):
        g = [r["served"]["groups"].get("s70") for r in rs]
        lat = lambda k: med([x["lat_us"][k] for x in g if x])
        deep = med([(x.get("idle") or {}).get("deep_per_kreq") for x in g if x])
        faults = med([x["server"]["faults_per_req"] for x in g if x])
        w = [window(r["served"]) for r in rs]
        mean = med([statistics.mean(p) / MiB for p in w if p])
        peak = med([max(p) / MiB for p in w if p])
        end = med([at_end(r["served"]) / MiB for r in rs])
        cyc = med([r["primary"]["value"] for r in rs])
        print(f"  {a:15s} {len(rs):2d}  {cyc:10.0f}  {lat('p50'):6.0f}  {lat('p99'):7.0f}  {lat('p999'):8.0f}  {deep:6.1f}"
              f"  {faults:10.2f}  {mean:9.0f}  {peak:9.0f}  {end:8.0f}")
