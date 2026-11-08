#!/usr/bin/env python3
"""The jobs under the switched binary: hugen.py FILE.jsonl... Each job's metric at each value of N
against N = 8, the control's difference (a second allocator with N = 8), the spread of the rounds, the
peak resident and huge memory, and for a server its rates' latencies. The allocators are cralloc-nN.
With --loose, a run marked invalid only for a little other work on its CPUs counts too."""
import collections
import json
import statistics as st
import sys

loose = "--loose" in sys.argv
rows = collections.defaultdict(lambda: collections.defaultdict(list))
for path in [a for a in sys.argv[1:] if a != "--loose"]:
    for line in open(path):
        r = json.loads(line)
        job, alloc = r.get("job"), r.get("allocator")
        if not job or not alloc or r.get("mode") != "native":
            continue
        problems = r.get("problems") or []
        if not r.get("valid", True) and not (loose and all(q.startswith("other work on") for q in problems)):
            continue
        rows[job][alloc.removeprefix("cralloc-")].append(r)


def med(values):
    values = [v for v in values if v is not None]
    return st.median(values) if values else 0


for job, by in rows.items():
    base = med(r["primary"]["value"] for r in by.get("n8", [])) if "n8" in by else None
    name = next(iter(by.values()))[0]["primary"]["name"]
    print(f"{job}  ({name})")
    for alloc in sorted(by, key=lambda a: (len(a), a)):
        runs = by[alloc]
        values = [r["primary"]["value"] for r in runs]
        m = st.median(values)
        spread = (max(values) - min(values)) / m * 100
        rel = f"{(m / base - 1) * 100:+6.2f}%" if base else ""
        rss = med((r.get("memory") or {}).get("rss_peak", 0) / 2**20 for r in runs)
        mean = med((r.get("memory") or {}).get("rss_mean", 0) / 2**20 for r in runs)
        huge = med((r.get("memory") or {}).get("anon_huge_peak", 0) / 2**20 for r in runs)
        print(f"  {alloc:<6} {m:>12.4f} {rel}  spread {spread:5.2f}%  rss peak {rss:8.1f} mean {mean:8.1f} huge {huge:7.1f} MiB  runs {len(runs)}")
    for alloc in sorted(by, key=lambda a: (len(a), a)):
        served = [(r.get("served") or {}).get("groups", {}).get("s70") for r in by[alloc]]
        served = [s["lat_us"] for s in served if s]
        if served:
            print(f"    {alloc:<6} at 70%: " + "  ".join(f"{k} {med(s[k] for s in served):.0f}" for k in ("p50", "p99", "p999", "max")) + " us")
