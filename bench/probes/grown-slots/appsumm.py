#!/usr/bin/env python3
"""Medians and ranges of benchrun's records of a run job by allocator: the primary metric,
minor faults, the cgroup's peak and the mean resident set (MiB), and the system time.

  appsumm.py RUNS.jsonl [RUNS.jsonl...]
"""
import json
import statistics as st
import sys
from collections import defaultdict

rows = defaultdict(list)
for path in sys.argv[1:]:
    for line in open(path):
        r = json.loads(line)
        if r.get("type") not in (None, "run") or "allocator" not in r or r.get("problems"):
            continue
        if not r.get("primary"):
            continue
        rows[(r["job"], r["allocator"])].append(r)
for (job, alloc), v in sorted(rows.items()):
    p = [x["primary"]["value"] for x in v]
    f = [x["rusage"]["minflt"] for x in v]
    pk = [x["cgroup"]["peak"] / 2**20 for x in v]
    mean = [x["memory"]["rss_mean"] / 2**20 for x in v]
    sy = [x["rusage"]["stime_ns"] / 1e9 for x in v]
    print(f"{job[:28]:28} {alloc:10} n={len(v)} {v[0]['primary']['name']} {st.median(p):.3f} ({min(p):.3f}-{max(p):.3f}) "
          f"faults {st.median(f)/1e3:.0f}k ({min(f)/1e3:.0f}-{max(f)/1e3:.0f}) peak {st.median(pk):.0f} ({min(pk):.0f}-{max(pk):.0f}) "
          f"mean {st.median(mean):.0f} sys {st.median(sy):.2f}s")
