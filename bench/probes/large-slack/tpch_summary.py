#!/usr/bin/env python3
"""Summarizes tpch.py's records (docs/research/large-slack.md): per arm, the median cycles
(user and kernel), faults and peak RSS over the rounds, and the switch's trim and refault
counts summed over the rounds.

    python3 -I tpch_summary.py OUT.jsonl
"""
import json
import statistics
import sys
from collections import defaultdict

by = defaultdict(list)
for line in open(sys.argv[1]):
    r = json.loads(line)
    by[r["lib"]].append(r)
print(f"{'arm':6} {'n':>2} {'Gcycles':>8} {'faults':>8} {'RSS MiB':>8}  trims, pages trimmed, refaulted by the same tenant")
for arm, rs in sorted(by.items()):
    cyc = statistics.median(((r["stat"].get("cycles:u") or 0) + (r["stat"].get("cycles:k") or 0)) / 1e9 for r in rs)
    pf = statistics.median(r["stat"].get("page-faults") or 0 for r in rs)
    rss = statistics.median(r["rss_kib"] / 1024 for r in rs)
    t = defaultdict(int)
    for r in rs:
        for line in r.get("lsr", "").splitlines():
            for kv in line.split()[1:]:
                k, v = kv.split("=", 1)
                if v.isdigit():
                    t[k] += int(v)
    extra = f"{t['trims']}, {t['trim_pages']}, {t['refaulted']} of {t['checked']} ({t['refaulted'] / max(1, t['checked']):.0%})" if t else ""
    print(f"{arm:6} {len(rs):2} {cyc:8.2f} {pf:8.0f} {rss:8.0f}  {extra}")
