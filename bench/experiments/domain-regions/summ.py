"""Summarize crun.sh output: per (lib, placement), medians and ranges.

usage: python3 -I summ.py [--calls N] [--metric wall|<key>] file...
--calls N divides cycles and fills by N (per call); otherwise they are given in millions.
"""
import sys
import statistics as st
from collections import defaultdict

args = sys.argv[1:]
calls = None
if args and args[0] == "--calls":
    calls = float(args[1])
    args = args[2:]
runs = defaultdict(list)
for path in args:
    for line in open(path):
        f = line.split()
        if len(f) < 5:
            continue
        lib, place = f[1], f[2]
        kv = dict(x.split("=", 1) for x in f[4:] if "=" in x)
        if kv.get("st") != "0":
            continue
        runs[(lib, place)].append(kv)


def med(xs):
    return st.median(xs) if xs else float("nan")


keys = sorted(runs, key=lambda k: (k[1], k[0]))
print(f"{'lib':10} {'place':5} {'n':>2} {'wall med':>8} {'[min,max]':>15} {'peak':>6} "
      f"{'cyc':>7} {'near':>7} {'dram':>7} {'l3a':>5} {'metric (sorted)'}")
for k in keys:
    rs = runs[k]
    w = [float(r["wall"]) for r in rs]
    pk = [float(r["peak"]) for r in rs if r.get("peak")]
    div = calls if calls else 1e6
    cyc = [float(r["cycles"]) / div for r in rs]
    near = [float(r.get("near_cache", 0)) / div for r in rs]
    dram = [float(r.get("dram_io_near", 0)) / div for r in rs]
    l3a = [float(r.get("l3a", 0)) for r in rs]
    m = sorted(float(r["m"]) for r in rs if "m" in r)
    ms = " ".join(f"{x:.4g}" for x in m)
    print(f"{k[0]:10} {k[1]:5} {len(rs):2} {med(w):8.3f} [{min(w):.3f},{max(w):.3f}] "
          f"{med(pk):6.1f} {med(cyc):7.2f} {med(near):7.3f} {med(dram):7.3f} {med(l3a):5.2f} {ms}")
