#!/usr/bin/env python3
"""Per allocator, the runs of an apps-suite job from benchrun results (docs/research/
purge-off-path.md): cycles of all threads over the measured work, in user and kernel mode,
the process's peak
resident set and the run's cgroup peak (which also holds the page cache of the inputs), and
page faults. Medians, with the range.

  apps.py RESULTS.jsonl...
"""
import json
import statistics
import sys
from collections import defaultdict

MiB = 2 ** 20
runs = defaultdict(list)
for path in sys.argv[1:]:
    for line in open(path):
        r = json.loads(line)
        if r.get("type") == "run" and r.get("valid"):
            runs[(r["job"], r["allocator"])].append(r)

def phase(r):
    return next(iter(r["workload"]["phases"].values()))


cols = [
    ("Gcycles", lambda r: phase(r)["cycles"] / 1e9, "{:.2f}"),
    ("user Gcycles", lambda r: phase(r)["cycles_user"] / 1e9, "{:.2f}"),
    ("kernel Gcycles", lambda r: (phase(r)["cycles"] - phase(r)["cycles_user"]) / 1e9, "{:.2f}"),
    ("rss peak MiB", lambda r: r["memory"]["rss_peak"] / MiB, "{:.0f}"),
    ("cgroup peak MiB", lambda r: r["cgroup"]["peak"] / MiB, "{:.0f}"),
    ("faults k", lambda r: r["process"]["page_faults"] / 1e3, "{:.0f}"),
]
print("| job | allocator | runs | " + " | ".join(c[0] for c in cols) + " |")
print("|---|---|---:|" + "---:|" * len(cols))
for (job, alloc), rs in sorted(runs.items()):
    cells = []
    for _, f, fmt in cols:
        v = [f(r) for r in rs]
        cells.append(f"{fmt.format(statistics.median(v))} ({fmt.format(min(v))} to {fmt.format(max(v))})")
    print(f"| {job} | {alloc} | {len(rs)} | " + " | ".join(cells) + " |")
