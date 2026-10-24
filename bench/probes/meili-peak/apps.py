#!/usr/bin/env python3
"""The apps suite's runs by allocator, for docs/research/meili-peak.md: the cgroup peak of each
run, and the medians of page faults and of the primary metric (Gcycles).

  apps.py FILE...
"""
import json
import statistics
import sys


def main():
    rows = {}
    for f in sys.argv[1:]:
        for line in open(f):
            d = json.loads(line)
            if d.get("type") != "run":
                continue
            primary = (d.get("primary") or {}).get("value")
            rows.setdefault(d["allocator"], []).append((d["cgroup"]["peak"] / 2**20, d["rusage"]["minflt"], primary))
    for a, r in rows.items():
        g = [x[2] for x in r if x[2] is not None]
        print(f"{a:10s} peak MiB {sorted(round(x[0]) for x in r)} faults {statistics.median(x[1] for x in r) / 1e3:.0f}k"
              + (f" Gcycles {statistics.median(g):.2f}" if g else ""))


if __name__ == "__main__":
    main()
