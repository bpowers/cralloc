#!/usr/bin/env python3
"""The order service's table doubling (`benchrun run --suite servers-doubling`) per allocator,
medians over runs:

  doublesum.py RESULTS.jsonl...

From the server: how many shard tables doubled to the largest size (64 expected), the median
and largest time one doubling held its shard's lock, and the median page faults the doubling
thread took in it. From the runner, over the segment `dbl` that holds the doublings: latency
p99.9 and max, requests that missed their slot, the server's page faults (all threads), the
resident set's rise over the segment, and the machine's direct compactions over the run.
"""
import json
import statistics
import sys
from collections import defaultdict

runs = defaultdict(list)
for f in sys.argv[1:]:
    for line in open(f):
        d = json.loads(line)
        if d.get("type") == "run" and d.get("valid") and d.get("served") and "doubling" in d["job"]:
            runs[d["allocator"]].append(d)


def med(xs):
    xs = [x for x in xs if x is not None]
    return statistics.median(xs) if xs else float("nan")


def rss_at(served, t):
    v = [p[1] for p in served["curve_kib"] if p[0] <= t]
    return v[-1] * 1024 if v else None


def rise(r):
    s = r["served"]
    marks = s["marks_ms"]
    i = [k for k, m in enumerate(marks) if m[1] == "dbl"][0]
    a, b = marks[i][0], marks[i + 1][0]
    return (rss_at(s, b) - rss_at(s, a)) / 2**20


print("allocator        n  doubled  ms median  ms max  faults  p99.9 us  max us  missed  faults in dbl  rise MiB  compactions")
for a, rs in sorted(runs.items(), key=lambda kv: med([r["workload"]["metrics"].get("doubling_ms_median") for r in kv[1]])):
    m = lambda k: med([r["workload"]["metrics"].get(k) for r in rs])
    g = lambda r: r["served"]["groups"]["dbl"]
    faults = med([g(r)["server"]["faults_per_req"] * g(r)["lat_us"]["n"] for r in rs])
    print(f"{a:15s} {len(rs):2d}  {m('doublings_last'):7.0f}  {m('doubling_ms_median'):9.2f}  {m('doubling_ms_max'):6.2f}"
          f"  {m('doubling_faults_median'):6.0f}  {med([g(r)['lat_us']['p999'] for r in rs]):8.0f}"
          f"  {med([g(r)['lat_us']['max'] for r in rs]):6.0f}  {med([g(r)['missed'] for r in rs]):6.0f}"
          f"  {faults:13.0f}  {med([rise(r) for r in rs]):8.0f}"
          f"  {med([r['vmstat'].get('compact_stall', 0) for r in rs]):11.0f}")
