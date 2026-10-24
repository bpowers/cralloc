#!/usr/bin/env python3
"""runs.py JOB FILE...: one line per valid run of JOB, for spotting a per-run mode.

Prints allocator, s30 / s70 / on p99, p99.9 and max (us), and the server's cycles and
instructions a request at s70, sorted by allocator then s70 p99.9.
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

        def lat(k):
            return g.get(k, {}).get("lat_us", {})

        s = g["s70"]["server"]
        rows.append((r["allocator"], lat("s30"), lat("s70"), lat("on"), s, f.split("/")[-1]))
rows.sort(key=lambda x: (x[0].split("@")[0], x[2].get("p999", 0)))
print(f"{'allocator':24} {'s30 p99/999/max':>18} {'s70 p99/999/max':>20} {'on p99/999/max':>20} {'kcyc':>7} {'kins':>7}")


def t(d):
    return f"{d.get('p99', 0):5.1f} {d.get('p999', 0):5.1f} {d.get('max', 0):6.0f}"


for a, s30, s70, on, s, f in rows:
    print(f"{a:24} {t(s30):>18} {t(s70):>20} {t(on):>20} {s['cycles_per_req'] / 1e3:7.1f} {s['instructions_per_req'] / 1e3:7.1f}  {f}")
