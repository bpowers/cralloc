#!/usr/bin/env python3
"""modes.py JOB FILE...: each valid run of JOB with the counters benchrun keeps that could tell
two modes of the program's work apart: server cycles and instructions a request at s70, CPU
time a request, the CPUs busy, voluntary and involuntary context switches over the whole run
(rusage) a request served in the run, page faults, and p99. Sorted by allocator, then cycles.
"""
import gzip
import json
import sys


def records(path):
    opener = gzip.open if path.endswith(".gz") else open
    with opener(path, "rt") as f:
        for line in f:
            if line.strip():
                yield json.loads(line)


job = sys.argv[1]
rows = []
for f in sys.argv[2:]:
    for r in records(f):
        if r.get("type") != "run" or r.get("job") != job or r.get("valid") is not True:
            continue
        g = r["served"]["groups"]
        s = g["s70"]["server"]
        total = sum(v["lat_us"]["n"] for v in g.values())
        ru = r.get("rusage", {})
        rows.append((r["allocator"], s["cycles_per_req"] / 1e3, s["instructions_per_req"] / 1e3,
                     s["cpu_us_per_req"], s["cpus_busy"], ru.get("nvcsw", 0) / total, ru.get("nivcsw", 0) / total,
                     g["s70"]["lat_us"]["p99"], g["s70"]["lat_us"]["p999"]))
rows.sort(key=lambda x: (x[0].split("@")[0], x[1]))
print(f"{'allocator':22} {'kcyc':>6} {'kins':>6} {'cpu us':>7} {'busy':>5} {'vcsw/req':>8} {'ivcsw/req':>9} {'p99':>5} {'p99.9':>6}")
for a, c, i, cpu, busy, v, iv, p99, p999 in rows:
    print(f"{a:22} {c:6.1f} {i:6.1f} {cpu:7.2f} {busy:5.2f} {v:8.3f} {iv:9.5f} {p99:5.1f} {p999:6.1f}")
