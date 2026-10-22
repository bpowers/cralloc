#!/usr/bin/env python3
"""Cycles samples inside the inlined pop and push routines, by offset from the routine's
arming instruction: which instruction of the fast path the time lands on. The 16 bytes after
the commit are counted too, marked, since a sample's skid puts a stalled instruction's cost
on the ones after it.

  rseqhist.py DIR
"""
import bisect
import collections
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import elfinfo
from where import leaves, load_base

d = sys.argv[1]
meta = json.load(open(f"{d}/meta.json"))
binary = meta["server"]
base = load_base(open(f"{d}/maps.txt").read(), binary)
r = elfinfo.routines(binary)
starts = [s for s, *_ in r]
h = collections.Counter()
tot = 0
for key, pid, leaf, br in leaves(f"{d}/perf.data"):
    if pid != meta["server_pid"] or leaf is None:
        continue
    tot += 1
    a = leaf[0] - base
    i = bisect.bisect_right(starts, a) - 1
    if i >= 0 and a < r[i][1] + 16:
        h[(r[i][2], a - r[i][0], a >= r[i][1])] += 1
for kind in ("pop", "push"):
    rows = sorted((k, v) for k, v in h.items() if k[0] == kind)
    n = sum(v for _, v in rows)
    print(f"{kind}: {n} samples ({100 * n / max(1, tot):.2f}% of the server's)")
    for (k, off, after), v in rows:
        if v * 100 >= n:
            print(f"  +{off:3d}{' after commit' if after else ''}: {100 * v / n:5.1f}%")
