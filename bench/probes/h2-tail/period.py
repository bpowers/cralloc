#!/usr/bin/env python3
"""period.py RAW [--slow US] [--from S] [--to S]: is there a clock behind the slow requests?

For the requests slower than --slow (us) due between --from and --to seconds of the run, the
concentration of their due times' phase (the length of the mean unit vector, 0 for uniform, 1
for one phase) for periods from 0.25 to 4.1 ms, best first; then for the best period near 1 ms,
the latency histogram of the slow requests and of all requests by phase.
"""
import argparse
import cmath
import math
import struct
from collections import Counter
from pathlib import Path


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("raw")
    ap.add_argument("--slow", type=float, default=42.0)
    ap.add_argument("--from", dest="t0", type=float, default=17.0)
    ap.add_argument("--to", dest="t1", type=float, default=1e9)
    a = ap.parse_args()
    recs = [(d, l) for d, l, s, f in struct.iter_unpack("<QQHH", Path(a.raw).read_bytes())
            if a.t0 * 1e9 <= d < a.t1 * 1e9 and not f & 4]
    slow = [d for d, l in recs if l > a.slow * 1e3]
    print(f"{len(slow)} slow of {len(recs)}")

    def conc(period_ns, ds):
        z = sum(cmath.exp(2j * math.pi * (d % period_ns) / period_ns) for d in ds)
        return abs(z) / max(1, len(ds)), cmath.phase(z)

    cands = [250_000 * k for k in range(1, 17)]
    fine = [1_000_000 + 20 * k for k in range(-100, 101)]
    res = sorted(((conc(p, slow)[0], p) for p in cands + fine), reverse=True)[:8]
    for c, p in res:
        print(f"  period {p / 1e6:.5f} ms: concentration {c:.3f}")
    best = max(fine, key=lambda p: conc(p, slow)[0])
    print(f"best near 1 ms: {best / 1e6:.5f} ms")
    nb = 20
    allb, slowb, latsum = Counter(), Counter(), Counter()
    for d, l in recs:
        b = int(nb * (d % best) / best)
        allb[b] += 1
        latsum[b] += l
        if l > a.slow * 1e3:
            slowb[b] += 1
    print("phase bin: requests, mean latency us, slow share %")
    for b in range(nb):
        print(f"  {b:2d} {allb[b]:8d} {latsum[b] / max(1, allb[b]) / 1e3:6.2f} {100 * slowb[b] / max(1, allb[b]):6.3f}")
    h = Counter(min(int(l / 5e3) * 5, 200) for d, l in recs if l > a.slow * 1e3)
    print("slow latency histogram (5 us bins): " + ", ".join(f"{k}:{v}" for k, v in sorted(h.items())))


if __name__ == "__main__":
    main()
