#!/usr/bin/env python3
"""Medians per program and library from bench.py output: summ.py FILE.jsonl [...]"""
import collections
import json
import statistics
import sys

rows = collections.defaultdict(list)
order = []
for path in sys.argv[1:]:
    for line in open(path):
        r = json.loads(line)
        key = (r["prog"], r["lib"])
        if key not in rows:
            order.append(key)
        rows[key].append(r)


def med(xs):
    xs = [x for x in xs if x is not None]
    return statistics.median(xs) if xs else float("nan")


def spread(xs):
    xs = [x for x in xs if x is not None]
    return (max(xs) - min(xs)) / 2 / statistics.median(xs) * 100 if len(xs) > 1 else 0


print(f"{'prog':15} {'lib':10} {'n':>2} {'wall':>6} {'±%':>4} {'cyc G':>6} {'kern G':>6} {'cpu s':>6} {'faults':>8} {'cs':>6} {'rss M':>6} {'mprot':>6} {'madv':>6} {'DONTN':>6} {'revL M':>7}")
for key in sorted(order, key=lambda k: (k[0], order.index(k))):
    rs = rows[key]
    st = lambda k: [(r["stat"].get(k)) for r in rs]
    cyc = [((r["stat"].get("cycles:u") or 0) + (r["stat"].get("cycles:k") or 0)) / 1e9 for r in rs]
    calls = lambda k: med([r["calls"].get(k, 0) for r in rs]) if any(r["calls"] for r in rs) else float("nan")
    madv = med([sum(v for k, v in r["calls"].items() if k in ("DONTNEED", "HUGEPAGE", "NOHUGEPAGE")) for r in rs]) if any(r["calls"] for r in rs) else float("nan")
    print(f"{key[0]:15} {key[1]:10} {len(rs):2d} {med([r['wall'] for r in rs]):6.3f} {spread([r['wall'] for r in rs]):4.0f} "
          f"{med(cyc):6.2f} {med(st('cycles:k')) / 1e9:6.2f} {med(st('task-clock')) / 1e3:6.2f} "
          f"{med(st('page-faults')):8.0f} {med(st('context-switches')):6.0f} {med([r['rss_kib'] for r in rs]) / 1024:6.0f} "
          f"{calls('mprotect'):6.0f} {madv:6.0f} {calls('DONTNEED'):6.0f} {calls('revived_large') / 2**20:7.0f}")
