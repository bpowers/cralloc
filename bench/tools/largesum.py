#!/usr/bin/env python3
"""Large-heap server runs (`benchrun run --suite servers-large`) as a table per job: cycles
per request and p99.9 at 70%, page faults per request, and resident memory over the
program's live bytes. The live bytes come from the job's runs of the `live-bytes` build (the
program's allocations are the same whatever the allocator), read at every mark; an
allocator's resident set at the same mark comes from its run's curve.

  largesum.py RESULTS.jsonl...

Columns: `r/l s70 first` and `r/l s70 last` are resident over live at the first and last of
the six ten-second pieces at 70% (the drift over a minute of churn); `r/l idle` is resident
9.75 s after the connections closed over live at the end of the load; `peak/live` is the
cgroup peak over the largest live count.

  --heap   resident means anonymous memory only (the curve's third column, from the rollups
           once a second), for a server whose store is a mapped file (Meilisearch): that is
           the allocator's part, and `peak/live` the largest anonymous memory sampled; the
           table then also gives the mapped part, in MiB, at the end of the minute at 70%.
"""
import json
import statistics
import sys
from collections import defaultdict

heap = "--heap" in sys.argv
runs = defaultdict(lambda: defaultdict(list))
for f in [a for a in sys.argv[1:] if not a.startswith("--")]:
    for line in open(f):
        d = json.loads(line)
        if d.get("type") == "run" and d.get("valid") and d.get("served"):
            runs[d["job"]][d["allocator"]].append(d)


def rss_at(served, t, col=None):
    pts = served["curve_kib"]
    col = col if col is not None else (2 if heap else 1)
    v = [p[col] for p in pts if p[0] <= t]
    return v[-1] * 1024 if v else None


def marks(served, name):
    return [m for m in served["marks_ms"] if m[1] == name]


def med(xs):
    xs = [x for x in xs if x is not None]
    return statistics.median(xs) if xs else float("nan")


for job, by in runs.items():
    lives = by.get("live-bytes", [])
    if not lives:
        print(f"\n{job}: no live-bytes run")
        continue
    # Live at each s70 mark (start of each piece), and at the end of the load (the first
    # silence's mark), medians over the live-bytes runs.
    s70_live = [med([marks(r["served"], "s70")[i][2] for r in lives]) for i in range(6)]
    end_live = med([marks(r["served"], "idle")[0][2] for r in lives if marks(r["served"], "idle")])
    max_live = med([max(m[2] for m in r["served"]["marks_ms"] if len(m) > 2) for r in lives])
    print(f"\n{job}: live at the 70% pieces {', '.join(f'{x / 2**30:.2f}' for x in s70_live)} GiB;"
          f" at the end of the load {end_live / 2**30:.2f} GiB")
    print("  allocator        n  cycles/req  p99.9 us  missed  faults/req  r/l s70 first  r/l s70 last  r/l idle  peak/live"
          + ("  mapped MiB" if heap else ""))
    for a, rs in sorted(by.items(), key=lambda kv: med([r["primary"]["value"] for r in kv[1]])):
        g = lambda r, k: r["served"]["groups"]["s70"]
        cyc = med([r["primary"]["value"] for r in rs])
        p999 = med([g(r, 0)["lat_us"]["p999"] for r in rs])
        missed = med([g(r, 0)["missed"] for r in rs])
        faults = med([g(r, 0)["server"]["faults_per_req"] for r in rs])
        first = med([rss_at(r["served"], marks(r["served"], "s70")[1][0]) for r in rs]) / s70_live[1]
        last = med([rss_at(r["served"], marks(r["served"], "s70")[5][0] + 9900) for r in rs]) / s70_live[5]
        idle = med([rss_at(r["served"], marks(r["served"], "idle")[0][0] + 9750) for r in rs]) / end_live
        if heap:
            peak = med([max(p[2] for p in r["served"]["curve_kib"]) * 1024 for r in rs]) / max_live
        else:
            peak = med([r["cgroup"]["peak"] for r in rs]) / max_live
        end = lambda r: marks(r["served"], "s70")[5][0] + 9900
        mapped = med([rss_at(r["served"], end(r), 1) - rss_at(r["served"], end(r), 2) for r in rs]) / 2**20
        print(f"  {a:15s} {len(rs):2d}  {cyc:10.0f}  {p999:8.0f}  {missed:6.0f}  {faults:10.4f}  {first:13.3f}"
              f"  {last:12.3f}  {idle:8.3f}  {peak:9.3f}" + (f"  {mapped:10.0f}" if heap else ""))
